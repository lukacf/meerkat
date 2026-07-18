//! Agent state machine internals.

use crate::budget::{BudgetDimension, BudgetExceeded};
use crate::error::{AgentError, LlmFailureReason, ToolError};
use crate::event::{
    AgentEvent, BackgroundJobTerminalStatus, BudgetType, DeferredCatalogDelta, ToolCallArguments,
    ToolCallArgumentsError, ToolConfigChangeDomain, ToolConfigChangeOperation,
    ToolConfigChangeStatus, ToolConfigChangedPayload,
};
use crate::generated::protocol_ops_barrier_satisfaction::{
    OpsBarrierSatisfactionObligation, submit_ops_barrier_satisfied,
};
use crate::hooks::{
    HookExecutionReport, HookInvocation, HookLlmRequest, HookLlmResponse, HookPoint, HookToolCall,
    HookToolResult,
};
use crate::image_content::{MissingBlobBehavior, hydrate_messages_for_execution};
use crate::lifecycle::RunId;
use crate::lifecycle::run_primitive::{ProviderParamsCarrier, ProviderParamsOverride};
use crate::ops_lifecycle::OperationCompletionWakeClass;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use crate::tool_catalog::{ToolCatalogDeferredEligibility, ToolCatalogMode, ToolPlaneClass};
use crate::turn_execution_authority::{
    CallTimeoutVerdict, ContentShape, LlmFailureRecoveryKind, TurnExecutionEffect,
    TurnExecutionInput, TurnExecutionTransition, TurnFailureSource, TurnPhase, TurnPrimitiveKind,
    TurnTerminalCauseKind, TurnTerminalOutcome,
};
use crate::types::{
    BlockAssistantMessage, Message, RunResult, SystemNoticeKind, SystemNoticeMessage, ToolCallView,
    ToolDef, ToolNameSet, UserMessage,
};
use serde_json::Value;
use serde_json::value::RawValue;
use std::collections::BTreeSet;
use std::sync::Arc;
use tokio::sync::mpsc;

use super::{
    Agent, AgentLlmClient, AgentLlmFallbackSwitch, AgentSessionStore, AgentToolDispatcher,
    LlmStreamResult, select_tool_catalog_mode,
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
    extraction_output_schema: Option<crate::types::OutputSchema>,
    allow_empty_success: bool,
    durable_visibility_parent: &'a mut Option<crate::SessionToolVisibilityState>,
}

type AppliedModelFallbackSwitch = (
    Arc<[Arc<ToolDef>]>,
    Option<ProviderParamsOverride>,
    u32,
    Message,
    crate::SessionToolVisibilityState,
);

/// Live publication retained while the runtime-owned durability coordinator
/// settles the preauthorized sticky-fallback transaction. The operation is
/// cancellation-safe; keeping the rest of the saga on `Agent` lets the
/// session task settle it after a hard interrupt drops the run future.
pub(crate) struct PendingStickyModelFallbackActivation {
    operation: Arc<dyn crate::handles::StickyModelFallbackCommitOperation>,
    previous_identity: crate::SessionLlmIdentity,
    target_identity: crate::SessionLlmIdentity,
    auth_rotation: super::runner::StagedAuthLeaseRotation,
    request_policy: crate::SessionLlmRequestPolicy,
    target_profile: crate::ModelProfileWitness,
    next_session: crate::Session,
}

struct MachineAcceptedModelFallbackActivation {
    switch: AgentLlmFallbackSwitch,
    proof: crate::StickyModelFallbackActivationProof,
}

impl MachineAcceptedModelFallbackActivation {
    fn authorize(
        mut switch: AgentLlmFallbackSwitch,
        recovery_transition: &TurnExecutionTransition,
        retry_schedule: &crate::retry::LlmRetrySchedule,
        effective_registry: Option<&crate::ModelRegistry>,
    ) -> Result<Self, AgentError> {
        if recovery_transition.prev_phase != TurnPhase::CallingLlm {
            return Err(AgentError::InternalError(format!(
                "model fallback activation requires machine-accepted RecoverableFailure from an \
                 LLM phase, got {:?}",
                recovery_transition.prev_phase
            )));
        }
        if switch.request_policy.model != switch.new_identity.model {
            return Err(AgentError::ConfigError(format!(
                "model fallback activation target '{}' does not match request policy model '{}'",
                switch.new_identity.model, switch.request_policy.model
            )));
        }
        if !switch.target_profile.matches_identity(&switch.new_identity) {
            return Err(AgentError::ConfigError(format!(
                "model fallback profile witness targets '{}:{}' but activation targets '{}:{}'",
                switch.target_profile.provider().as_str(),
                switch.target_profile.model(),
                switch.new_identity.provider.as_str(),
                switch.new_identity.model
            )));
        }
        let registry = effective_registry.ok_or_else(|| {
            AgentError::ConfigError(
                "model fallback requires a captured effective model registry".to_string(),
            )
        })?;
        let authoritative = registry
            .profile_witness_for_provider(
                switch.new_identity.provider,
                &switch.new_identity.model,
            )
            .ok_or_else(|| {
                AgentError::ConfigError(format!(
                    "model fallback target '{}:{}' is absent from the agent's effective model registry",
                    switch.new_identity.provider.as_str(),
                    switch.new_identity.model
                ))
            })?;
        if !switch.target_profile.was_minted_by(registry.authority()) {
            return Err(AgentError::ConfigError(format!(
                "model fallback profile witness for '{}:{}' was not minted by the agent's effective model registry",
                switch.new_identity.provider.as_str(),
                switch.new_identity.model
            )));
        }
        switch.target_profile = authoritative;
        let proof = crate::StickyModelFallbackActivationProof::new(
            switch.previous_identity.clone(),
            switch.new_identity.clone(),
            switch.target_profile.clone(),
            retry_schedule.plan.attempt,
        );
        Ok(Self { switch, proof })
    }
}

fn turn_input_failure_source(input: &TurnExecutionInput) -> Option<&TurnFailureSource> {
    match input {
        TurnExecutionInput::FatalFailure { failure, .. } => Some(failure),
        _ => None,
    }
}

fn public_terminal_cause_kind(
    cause_kind: Option<TurnTerminalCauseKind>,
) -> Option<TurnTerminalCauseKind> {
    cause_kind.filter(|cause_kind| cause_kind.is_specific_failure_cause())
}

fn assistant_image_events_from_blocks(
    blocks: &[crate::types::AssistantBlock],
) -> Vec<crate::event::AssistantImageEvent> {
    blocks
        .iter()
        .filter_map(crate::event::AssistantImageEvent::from_assistant_block)
        .collect()
}

fn assistant_image_events_from_effects(
    effects: &[crate::ops::SessionEffect],
) -> Vec<crate::event::AssistantImageEvent> {
    let mut images = Vec::new();
    for effect in effects {
        if let crate::ops::SessionEffect::AppendAssistantBlocks { blocks } = effect {
            images.extend(assistant_image_events_from_blocks(blocks));
        }
    }
    images
}

fn assistant_blocks_have_user_visible_output(blocks: &[crate::types::AssistantBlock]) -> bool {
    blocks.iter().any(|block| match block {
        crate::types::AssistantBlock::Text { text, .. }
        | crate::types::AssistantBlock::Transcript { text, .. }
        | crate::types::AssistantBlock::Reasoning { text, .. } => !text.trim().is_empty(),
        crate::types::AssistantBlock::Image { .. } => true,
        crate::types::AssistantBlock::ToolUse { .. }
        | crate::types::AssistantBlock::ServerToolContent { .. } => false,
    })
}

fn validate_turn_input_failure_reason(input: &TurnExecutionInput) -> Result<(), AgentError> {
    if let Some(failure) = turn_input_failure_source(input)
        && !failure.source_kind.is_known()
    {
        return Err(AgentError::InternalError(format!(
            "turn input {input:?} has unknown generated-authority failure source"
        )));
    }
    Ok(())
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

fn synthetic_notice_block_message(
    kind: SystemNoticeKind,
    body: impl Into<String>,
    block: crate::types::SystemNoticeBlock,
) -> Message {
    let body = body.into();
    Message::SystemNotice(SystemNoticeMessage::with_block(kind, Some(body), block))
}

fn fallback_activation_is_pre_stream_safe(
    error: &AgentError,
    stream_output_observed: bool,
) -> bool {
    if stream_output_observed {
        return false;
    }
    match error {
        AgentError::Llm { reason, .. } => !matches!(
            reason,
            LlmFailureReason::NetworkTimeout { .. } | LlmFailureReason::CallTimeout { .. }
        ),
        _ => false,
    }
}

/// Test-only predicate; production notice refresh goes through the atomic
/// `Session::replace_synthetic_notices` transcript authority.
#[cfg(test)]
fn is_synthetic_notice(message: &Message, kind: SystemNoticeKind) -> bool {
    matches!(message, Message::SystemNotice(notice) if notice.kind == kind)
}

fn hidden_deferred_catalog_names(
    catalog: &[crate::ToolCatalogEntry],
    visible_names: &BTreeSet<crate::types::ToolName>,
) -> BTreeSet<crate::types::ToolName> {
    catalog
        .iter()
        .filter(|entry| entry.plane == ToolPlaneClass::Session)
        .filter(|entry| {
            matches!(
                entry.deferred_eligibility,
                ToolCatalogDeferredEligibility::DeferredEligible { .. }
            )
        })
        .map(|entry| entry.tool.name.clone())
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

fn tool_call_args_projection_error(tool_name: &str, error: ToolCallArgumentsError) -> AgentError {
    // Preserve the typed `invalid_arguments` cause so its `error_code()`
    // survives terminalization to the wire surface, rather than flattening
    // it into an opaque string.
    AgentError::tool(ToolError::invalid_arguments(
        tool_name,
        format!("tool call arguments projection failed: {error}"),
    ))
}

impl<C, T, S> Agent<C, T, S>
where
    C: AgentLlmClient + ?Sized + 'static,
    T: AgentToolDispatcher + ?Sized + 'static,
    S: AgentSessionStore + ?Sized + 'static,
{
    /// Best-effort intra-loop session checkpoint.
    ///
    /// This is deliberately NOT the authoritative persistence path: the
    /// `PersistentSessionService` re-persists the full session with
    /// `?`-propagation after each turn (`persist_full_session_or_discard_live`),
    /// and that is the path callers rely on for durability. This in-loop save
    /// is a redundant convenience for the keep-alive checkpointer / store and
    /// must therefore stay non-terminal: a failure here is logged, not
    /// propagated, because the authoritative persistence boundary owns the
    /// durable outcome.
    ///
    /// Consistent with that contract, [`RunResult`](crate::types::RunResult)
    /// exposes no persistence-success field — the run result never claims a
    /// persistence outcome, so a best-effort miss here cannot launder into a
    /// false success.
    async fn save_session_best_effort(&mut self) {
        if let Err(e) = self.sync_system_context_state_to_session() {
            tracing::warn!("Failed to sync system-context state into session: {}", e);
        }
        if let Some(ref checkpointer) = self.checkpointer {
            checkpointer.checkpoint(&self.session).await;
            return;
        }
        if let Err(e) = self.store.save(&self.session).await {
            tracing::warn!("Failed to save session: {}", e);
        }
    }

    fn publish_pending_sticky_model_fallback(&mut self) -> Result<(), AgentError> {
        let pending = self
            .pending_sticky_model_fallback_activation
            .take()
            .ok_or_else(|| {
                AgentError::InternalError(
                    "sticky fallback supervisor completed without a pending live publication"
                        .to_string(),
                )
            })?;
        self.apply_llm_request_policy(pending.request_policy);
        self.active_model_profile = Some(pending.target_profile);
        self.session = pending.next_session;
        Ok(())
    }

    fn reject_pending_sticky_model_fallback(
        &mut self,
        commit_error: crate::handles::StickyModelFallbackCommitError,
    ) -> AgentError {
        let Some(pending) = self.pending_sticky_model_fallback_activation.take() else {
            return AgentError::StickyModelFallbackAuthorityUnknown {
                message: format!("{commit_error}; live fallback compensation state was missing"),
            };
        };
        let requires_teardown = commit_error.requires_teardown();
        let auth_rollback = pending.auth_rotation.rollback();
        let client_rollback = self
            .restore_model_fallback_client(&pending.previous_identity, &pending.target_identity);
        let mut failures = Vec::new();
        if let Err(error) = auth_rollback {
            failures.push(format!(
                "failed to restore the previous auth lease: {error}"
            ));
        }
        if let Err(error) = client_rollback {
            failures.push(format!(
                "failed to restore the previous fallback client: {error}"
            ));
        }
        if requires_teardown || !failures.is_empty() {
            let suffix = if failures.is_empty() {
                String::new()
            } else {
                format!("; {}", failures.join("; "))
            };
            AgentError::StickyModelFallbackAuthorityUnknown {
                message: format!("{commit_error}{suffix}"),
            }
        } else {
            AgentError::ConfigError(format!(
                "durable sticky fallback commit rejected before publication: {commit_error}"
            ))
        }
    }

    /// Settle a supervised sticky-fallback transaction whose await was
    /// interrupted when the session task dropped the run future. The durable
    /// operation continues independently; this method publishes its confirmed
    /// target or compensates client/auth state before the task reports the
    /// interruption.
    pub async fn settle_inflight_sticky_model_fallback(&mut self) -> Result<(), AgentError> {
        let Some(operation) = self
            .pending_sticky_model_fallback_activation
            .as_ref()
            .map(|pending| Arc::clone(&pending.operation))
        else {
            return Ok(());
        };
        match operation.wait().await {
            Ok(()) => self.publish_pending_sticky_model_fallback(),
            Err(error) => Err(self.reject_pending_sticky_model_fallback(error)),
        }
    }

    async fn apply_model_fallback_switch(
        &mut self,
        activation: MachineAcceptedModelFallbackActivation,
        failure: &AgentError,
        previous_tools: &[Arc<ToolDef>],
        extraction_output_schema: Option<&crate::types::OutputSchema>,
        durable_visibility_parent: Option<&crate::SessionToolVisibilityState>,
    ) -> Result<AppliedModelFallbackSwitch, AgentError> {
        let MachineAcceptedModelFallbackActivation { switch, proof } = activation;
        let retry_attempt = proof.retry_attempt();
        let capability_base_filter = crate::capability_base_filter_for_image_tool_results(
            switch.target_profile.profile().image_tool_results,
        );
        let context_window = switch.target_profile.context_window();
        let max_output_tokens = switch.target_profile.max_output_tokens();
        // Structured extraction must be lowered by the exact TARGET client,
        // not by the still-active primary client and not by merely copying the
        // raw Meerkat schema into another provider's request shape. This is a
        // prevalidation step: it happens before client activation, auth lease
        // rotation, generated-machine commit, visibility mutation, or session
        // publication.
        let compiled_extraction_output_schema = extraction_output_schema
            .map(|output_schema| {
                let compiled = self
                    .client
                    .compile_model_fallback_schema(&switch.new_identity, output_schema)
                    .map_err(|error| {
                        AgentError::ConfigError(format!(
                            "failed to compile structured output for fallback target '{}:{}': {error}",
                            switch.new_identity.provider.as_str(),
                            switch.new_identity.model
                        ))
                    })?;
                let mut lowered = output_schema.clone();
                lowered.schema = crate::schema::MeerkatSchema::new(compiled.schema).map_err(
                    |error| {
                        AgentError::ConfigError(format!(
                            "fallback target returned an invalid compiled schema: {error}"
                        ))
                    },
                )?;
                Ok(lowered)
            })
            .transpose()?;
        let (visibility_plan, next_tools) = self
            .tool_scope
            .propose_sticky_model_fallback_visibility(capability_base_filter)
            .map_err(|err| {
                AgentError::ConfigError(format!(
                    "failed to derive sticky-fallback visibility plan: {err}"
                ))
            })?;
        let mut next_session = self.session.clone();
        let mut metadata = next_session
            .try_session_metadata()
            .map_err(|err| {
                AgentError::ConfigError(format!(
                    "failed to read the durable fallback identity parent: {err}"
                ))
            })?
            .ok_or_else(|| {
                AgentError::ConfigError(
                    "sticky model fallback requires durable session metadata".to_string(),
                )
            })?;
        let durable_parent = metadata.llm_identity();
        if durable_parent != switch.previous_identity {
            return Err(AgentError::ConfigError(format!(
                "sticky model fallback durable identity parent '{}:{}' does not match the machine-authorized parent '{}:{}'",
                durable_parent.provider.as_str(),
                durable_parent.model,
                switch.previous_identity.provider.as_str(),
                switch.previous_identity.model,
            )));
        }
        metadata.apply_llm_identity(&switch.new_identity);
        next_session.set_session_metadata(metadata).map_err(|err| {
            AgentError::ConfigError(format!("failed to persist fallback LLM identity: {err}"))
        })?;
        next_session
            .set_tool_visibility_state(
                crate::session::AuthorizedSessionToolVisibilityState::from_generated_authority(
                    visibility_plan.next_state.clone(),
                ),
            )
            .map_err(|err| {
                AgentError::ConfigError(format!(
                    "failed to pre-serialize fallback tool visibility state: {err}"
                ))
            })?;
        let next_names = next_tools
            .iter()
            .map(|tool| tool.name.clone())
            .collect::<BTreeSet<_>>();
        let hidden_tools = previous_tools
            .iter()
            .filter(|tool| !next_names.contains(&tool.name))
            .map(|tool| tool.name.to_string())
            .collect::<Vec<_>>();

        let carrier = ProviderParamsCarrier {
            params: switch
                .request_policy
                .provider_params
                .clone()
                .unwrap_or_default(),
            tool_defaults: switch.request_policy.provider_tool_defaults.clone(),
        };
        let provider_params = carrier
            .effective_params()
            .map_err(|err| AgentError::ConfigError(err.to_string()))?;
        let provider_params = (!provider_params.is_empty()).then_some(provider_params);
        let provider_params = Self::apply_extraction_request_overrides(
            switch.new_identity.provider,
            provider_params,
            compiled_extraction_output_schema.as_ref(),
        )?;
        Self::validate_provider_params_for_identity(
            switch.new_identity.provider,
            provider_params.as_ref(),
        )?;
        let configured_max_tokens = self.config.resolved_max_tokens_per_turn();
        let max_tokens = max_output_tokens
            .map(|limit| configured_max_tokens.min(limit))
            .unwrap_or(configured_max_tokens);

        let skipped_targets = switch
            .skipped_targets
            .iter()
            .map(|target| {
                serde_json::json!({
                    "model": target.identity.model,
                    "provider": target.identity.provider.as_str(),
                    "reason": target.reason,
                })
            })
            .collect::<Vec<_>>();
        let hidden_detail = if hidden_tools.is_empty() {
            String::new()
        } else {
            format!(
                " Hidden tools for the fallback model: {}.",
                hidden_tools.join(", ")
            )
        };
        let body = format!(
            "The runtime switched from model '{}' ({}) to fallback model '{}' ({}) after an LLM failure: {}. Active model limits are context_window={:?}, max_output_tokens={:?}.{} Adapt to the active model and visible tools.",
            switch.previous_identity.model,
            switch.previous_identity.provider.as_str(),
            switch.new_identity.model,
            switch.new_identity.provider.as_str(),
            failure,
            context_window,
            max_output_tokens,
            hidden_detail,
        );
        let payload = serde_json::json!({
            "event": "model_fallback",
            "retry_attempt": retry_attempt,
            "from": {
                "model": switch.previous_identity.model,
                "provider": switch.previous_identity.provider.as_str(),
            },
            "to": {
                "model": switch.new_identity.model,
                "provider": switch.new_identity.provider.as_str(),
                "context_window": context_window,
                "max_output_tokens": max_output_tokens,
            },
            "hidden_tools": hidden_tools,
            "skipped_targets": skipped_targets,
            "reason": failure.to_string(),
        });
        let notice = Message::SystemNotice(SystemNoticeMessage::with_block(
            SystemNoticeKind::Generic,
            Some(body),
            crate::types::SystemNoticeBlock::RuntimeNotice {
                category: "model_fallback".to_string(),
                detail: Some(
                    "active LLM model changed after recoverable provider failure".to_string(),
                ),
                payload: Some(payload),
            },
        ));
        next_session.push(notice.clone());

        if self.pending_sticky_model_fallback_activation.is_some() {
            return Err(AgentError::StickyModelFallbackAuthorityUnknown {
                message: "a second sticky fallback was proposed while the prior durable transaction was still pending".to_string(),
            });
        }
        let coordinator = self.sticky_model_fallback_commit_coordinator.clone();
        if self.runtime_execution_kind_required && coordinator.is_none() {
            return Err(AgentError::ConfigError(
                "runtime-backed sticky fallback requires a durable commit coordinator".to_string(),
            ));
        }
        let persisted_visibility_parent = match coordinator.as_ref() {
            Some(_) => durable_visibility_parent.cloned().ok_or_else(|| {
                AgentError::ConfigError(
                    "runtime-backed sticky fallback is missing its sealed pre-turn visibility parent"
                        .to_string(),
                )
            })?,
            None => visibility_plan.previous_state.clone(),
        };

        // Generated authority preauthorizes the exact identity/capability/
        // visibility transition before any client, lease, or durable-session
        // mutation. The returned one-shot token can realize only against that
        // same authority snapshot.
        let machine_commit = match self.model_routing_handle.as_deref() {
            Some(handle) => handle.stage_sticky_model_fallback(proof, &visibility_plan),
            None => Err(crate::handles::DslTransitionError::no_matching(
                "ModelRoutingHandle::stage_sticky_model_fallback",
                "sticky fallback requires a generated model-routing authority handle",
            )),
        }
        .map_err(|machine_error| {
            AgentError::ConfigError(format!(
                "generated model-routing authority rejected sticky fallback preauthorization: {machine_error}"
            ))
        })?;
        let control_delta = crate::handles::StickyModelFallbackControlDelta::new(
            switch.previous_identity.clone(),
            switch.new_identity.clone(),
            &visibility_plan,
            persisted_visibility_parent,
        );

        // All target-dependent preparation and generated preauthorization are
        // complete. Client/auth steps remain reversible until the supervised
        // coordinator confirms the control-only RuntimeStore CAS plus the
        // synchronous generated-machine realization.
        self.activate_model_fallback_client(&switch.previous_identity, &switch.new_identity)?;
        let auth_rotation = match self.stage_auth_lease_auth_binding(
            switch.previous_identity.auth_binding.as_ref(),
            switch.new_identity.auth_binding.as_ref(),
        ) {
            Ok(rotation) => rotation,
            Err(auth_error) => {
                let client_rollback = self
                    .restore_model_fallback_client(&switch.previous_identity, &switch.new_identity);
                return Err(match client_rollback {
                    Ok(()) => auth_error,
                    Err(rollback_error) => AgentError::ConfigError(format!(
                        "{auth_error}; additionally failed to restore the previous fallback client: {rollback_error}"
                    )),
                });
            }
        };

        if let Some(coordinator) = coordinator {
            let operation = match coordinator.begin(machine_commit, control_delta) {
                Ok(operation) => operation,
                Err(commit_error) => {
                    let requires_teardown = commit_error.requires_teardown();
                    let auth_rollback = auth_rotation.rollback();
                    let client_rollback = self.restore_model_fallback_client(
                        &switch.previous_identity,
                        &switch.new_identity,
                    );
                    let mut failures = Vec::new();
                    if let Err(error) = auth_rollback {
                        failures.push(format!(
                            "failed to restore the previous auth lease: {error}"
                        ));
                    }
                    if let Err(error) = client_rollback {
                        failures.push(format!(
                            "failed to restore the previous fallback client: {error}"
                        ));
                    }
                    if requires_teardown || !failures.is_empty() {
                        let suffix = if failures.is_empty() {
                            String::new()
                        } else {
                            format!("; {}", failures.join("; "))
                        };
                        return Err(AgentError::StickyModelFallbackAuthorityUnknown {
                            message: format!("{commit_error}{suffix}"),
                        });
                    }
                    return Err(AgentError::ConfigError(format!(
                        "durable sticky fallback could not start: {commit_error}"
                    )));
                }
            };
            self.pending_sticky_model_fallback_activation =
                Some(PendingStickyModelFallbackActivation {
                    operation: Arc::clone(&operation),
                    previous_identity: switch.previous_identity.clone(),
                    target_identity: switch.new_identity.clone(),
                    auth_rotation,
                    request_policy: switch.request_policy.clone(),
                    target_profile: switch.target_profile.clone(),
                    next_session,
                });
            match operation.wait().await {
                Ok(()) => self.publish_pending_sticky_model_fallback()?,
                Err(error) => return Err(self.reject_pending_sticky_model_fallback(error)),
            }
        } else {
            if let Err(machine_error) = machine_commit.commit() {
                let auth_rollback = auth_rotation.rollback();
                let client_rollback = self
                    .restore_model_fallback_client(&switch.previous_identity, &switch.new_identity);
                let mut failures = vec![format!(
                    "generated model-routing authority rejected sticky fallback: {machine_error}"
                )];
                if let Err(error) = auth_rollback {
                    failures.push(format!(
                        "failed to restore the previous auth lease: {error}"
                    ));
                }
                if let Err(error) = client_rollback {
                    failures.push(format!(
                        "failed to restore the previous fallback client: {error}"
                    ));
                }
                return Err(AgentError::ConfigError(failures.join("; ")));
            }
            self.apply_llm_request_policy(switch.request_policy);
            self.active_model_profile = Some(switch.target_profile);
            self.session = next_session;
        }

        Ok((
            next_tools,
            provider_params,
            max_tokens,
            notice,
            visibility_plan.next_state,
        ))
    }

    fn active_model_fallback_identity(
        &self,
        context: &'static str,
    ) -> Result<crate::SessionLlmIdentity, AgentError> {
        self.client.active_model_fallback_identity().ok_or_else(|| {
            AgentError::ConfigError(format!(
                "{context}: fallback-capable LLM client did not expose its exact active identity"
            ))
        })
    }

    fn restore_model_fallback_client(
        &self,
        previous_identity: &crate::SessionLlmIdentity,
        target_identity: &crate::SessionLlmIdentity,
    ) -> Result<(), AgentError> {
        let active = self.active_model_fallback_identity("fallback client rollback")?;
        if active == *previous_identity {
            return Ok(());
        }
        if active != *target_identity {
            return Err(AgentError::ConfigError(format!(
                "fallback client rollback found divergent active identity '{}:{}' instead of target '{}:{}'",
                active.provider.as_str(),
                active.model,
                target_identity.provider.as_str(),
                target_identity.model
            )));
        }
        self.rollback_committed_model_fallback_client(previous_identity, target_identity)
    }

    fn rollback_committed_model_fallback_client(
        &self,
        previous_identity: &crate::SessionLlmIdentity,
        target_identity: &crate::SessionLlmIdentity,
    ) -> Result<(), AgentError> {
        self.client
            .commit_model_fallback(target_identity, previous_identity)
            .map_err(|error| {
                AgentError::ConfigError(format!(
                    "fallback client target-to-previous rollback failed: {error}"
                ))
            })?;
        let restored = self
            .active_model_fallback_identity("fallback client rollback verification")
            .map_err(|error| {
                AgentError::ConfigError(format!(
                    "fallback client target-to-previous rollback could not be verified: {error}"
                ))
            })?;
        if restored == *previous_identity {
            Ok(())
        } else {
            Err(AgentError::ConfigError(format!(
                "fallback client rollback reported success but remained on '{}:{}'",
                restored.provider.as_str(),
                restored.model
            )))
        }
    }

    fn activate_model_fallback_client(
        &self,
        previous_identity: &crate::SessionLlmIdentity,
        target_identity: &crate::SessionLlmIdentity,
    ) -> Result<(), AgentError> {
        let active = self.active_model_fallback_identity("fallback client activation")?;
        if active != *previous_identity {
            return Err(AgentError::ConfigError(format!(
                "fallback client activation expected previous identity '{}:{}' but client reports '{}:{}'",
                previous_identity.provider.as_str(),
                previous_identity.model,
                active.provider.as_str(),
                active.model
            )));
        }
        if let Err(activation_error) = self
            .client
            .commit_model_fallback(previous_identity, target_identity)
        {
            let rollback = self.restore_model_fallback_client(previous_identity, target_identity);
            return Err(match rollback {
                Ok(()) => activation_error,
                Err(rollback_error) => AgentError::ConfigError(format!(
                    "fallback client activation failed: {activation_error}; additionally failed to restore its previous identity: {rollback_error}"
                )),
            });
        }
        let verification_error = match self
            .active_model_fallback_identity("fallback client activation verification")
        {
            Ok(committed) if committed == *target_identity => return Ok(()),
            Ok(committed) => AgentError::ConfigError(format!(
                "fallback client activation reported success but active identity is '{}:{}', not '{}:{}'",
                committed.provider.as_str(),
                committed.model,
                target_identity.provider.as_str(),
                target_identity.model
            )),
            Err(error) => error,
        };
        // A successful activation commit transfers the client to the target.
        // Even when the subsequent identity read fails (or reports a divergent
        // identity), compensation must still ATTEMPT the exact target→previous
        // transition. The read-based restore helper cannot own this path because
        // the very observation it relies on is the failed operation.
        let rollback =
            self.rollback_committed_model_fallback_client(previous_identity, target_identity);
        Err(match rollback {
            Ok(()) => verification_error,
            Err(rollback_error) => AgentError::ConfigError(format!(
                "{verification_error}; additionally failed to restore the previous identity: {rollback_error}"
            )),
        })
    }

    fn validate_provider_params_for_identity(
        provider: crate::Provider,
        provider_params: Option<&ProviderParamsOverride>,
    ) -> Result<(), AgentError> {
        use crate::lifecycle::run_primitive::ProviderTag;

        let Some(tag) = provider_params.and_then(|params| params.provider_tag.as_ref()) else {
            return Ok(());
        };
        let matches = matches!(
            (provider, tag),
            (crate::Provider::Anthropic, ProviderTag::Anthropic(_))
                | (
                    crate::Provider::OpenAI | crate::Provider::SelfHosted,
                    ProviderTag::OpenAi(_)
                )
                | (crate::Provider::Gemini, ProviderTag::Gemini(_))
                | (_, ProviderTag::Unknown { .. })
        );
        if matches {
            Ok(())
        } else {
            Err(AgentError::ConfigError(format!(
                "provider params carry a `{}` provider tag but fallback identity targets `{}`",
                tag.provider_label(),
                provider.as_str()
            )))
        }
    }

    fn apply_extraction_request_overrides(
        provider: crate::Provider,
        mut provider_params: Option<ProviderParamsOverride>,
        output_schema: Option<&crate::types::OutputSchema>,
    ) -> Result<Option<ProviderParamsOverride>, AgentError> {
        let Some(output_schema) = output_schema.cloned() else {
            return Ok(provider_params);
        };

        let mut effective = provider_params.take().unwrap_or_default();
        effective
            .set_structured_output(provider, output_schema)
            .map_err(|error| AgentError::ConfigError(error.to_string()))?;
        effective.clear_web_search();
        Ok((!effective.is_empty()).then_some(effective))
    }

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

    /// Mirror the machine-owned turn-terminality verdict.
    ///
    /// The terminality verdict (which turn phases are terminal) is a MeerkatMachine
    /// fact emitted by `ClassifyTurnTerminality` and carried on the turn-state
    /// snapshot as `turn_terminal`; this loop mirrors it and never reclassifies
    /// `TurnPhase` locally.
    pub(super) fn turn_terminal(&self) -> Result<bool, AgentError> {
        Ok(self.runtime_turn_authority_snapshot()?.turn_terminal)
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
        // Dogma K9: the generated machine owns the total in-extraction fact
        // (`extraction_active`, set by EnterExtraction, cleared on terminal
        // transitions and run start). The former derivation from the
        // loop-local `extraction_state.primary_output` scratch was shell
        // shadow truth with a divergence window during retry rounds.
        Ok(self.runtime_turn_authority_snapshot()?.extraction_active)
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

    fn turn_terminal_cause_kind(&self) -> Result<Option<TurnTerminalCauseKind>, AgentError> {
        Ok(self.runtime_turn_authority_snapshot()?.terminal_cause_kind)
    }

    fn turn_extraction_attempts(&self) -> Result<u32, AgentError> {
        let snapshot = self.runtime_turn_authority_snapshot()?;
        u32::try_from(snapshot.extraction_attempts).map_err(|_| {
            AgentError::InternalError(
                "runtime turn-state authority projected extraction_attempts outside u32 range"
                    .to_string(),
            )
        })
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
                // Consult the injected resolver with the typed client provider
                // identity — no string boundary to re-parse.
                self.model_defaults_resolver
                    .as_ref()
                    .and_then(|r| r.call_timeout_for(self.client.provider(), self.client.model()))
                    // Fall through to RetryPolicy.call_timeout for direct builder users.
                    .or(self.retry_policy.call_timeout)
            }
        }
    }

    /// Hydrate one fully composed model request while its exact boundary state
    /// is still canonical pending rather than applied. A cancellation or blob
    /// fault here therefore leaves the context eligible for explicit run-exit
    /// cleanup/recovery instead of falsely recording model consumption.
    async fn hydrate_llm_request_messages(
        &self,
        mut messages: Vec<Message>,
    ) -> Result<Vec<Message>, AgentError> {
        let Some(blob_store) = self.blob_store.as_ref() else {
            return Ok(messages);
        };
        // The model boundary is always fail-closed: a durable blob missing at
        // live LLM-execution hydration is a typed terminal fault, never silently
        // rewritten into placeholder prompt text. Placeholder hydration exists
        // only for read-side transcript rendering.
        hydrate_messages_for_execution(
            blob_store.as_ref(),
            &mut messages,
            MissingBlobBehavior::Error,
        )
        .await
        .map_err(|err| match err {
            crate::blob::BlobStoreError::NotFound(blob_id) => AgentError::ConfigError(format!(
                "required image blob is unavailable before llm execution: {blob_id}"
            )),
            other => AgentError::InternalError(format!(
                "failed to hydrate image refs before llm execution: {other}"
            )),
        })?;
        Ok(messages)
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
            extraction_output_schema,
            allow_empty_success,
            durable_visibility_parent,
        } = request;

        let mut current_messages = messages.to_vec();
        // `tools` is already the ToolScope authority's boundary projection;
        // the client cannot narrow or widen it with a private semantic view.
        let mut current_tools: Arc<[Arc<ToolDef>]> = tools.to_vec().into();
        let mut current_max_tokens = max_tokens;
        let mut current_provider_params = provider_params.cloned();
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
            self.client.begin_stream_output_observation();
            let call_result = match (effective_call_timeout, remaining_turn) {
                (None, None) => {
                    // No timeout wrapper needed
                    self.client
                        .stream_response(
                            &current_messages,
                            &current_tools,
                            current_max_tokens,
                            temperature,
                            current_provider_params.as_ref(),
                        )
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
                            &current_messages,
                            &current_tools,
                            current_max_tokens,
                            temperature,
                            current_provider_params.as_ref(),
                        ),
                    )
                    .await
                    {
                        Ok(inner_result) => inner_result,
                        Err(_elapsed) => {
                            // Timeout fired — source is pre-selected shell-side,
                            // but MeerkatMachine (#323) owns the VERDICT: retryable
                            // call timeout vs terminal turn-budget. The loop only
                            // mirrors the emitted verdict into the existing paths.
                            let timeout_ms = effective_timeout.as_millis() as u64;
                            match self.classify_call_timeout(source, timeout_ms)? {
                                CallTimeoutVerdict::RetryableCallTimeout => Err(AgentError::Llm {
                                    provider: self.client.provider().as_str(),
                                    reason: crate::error::LlmFailureReason::CallTimeout {
                                        duration_ms: timeout_ms,
                                    },
                                    message: format!(
                                        "LLM call timed out after {}s",
                                        effective_timeout.as_secs()
                                    ),
                                }),
                                CallTimeoutVerdict::TerminalTurnBudget => {
                                    let exceeded =
                                        self.budget.observe().exceeded().unwrap_or_else(|| {
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
                Ok(result) => {
                    if !crate::types::assistant_blocks_have_visible_or_actionable_output(
                        result.blocks(),
                    ) {
                        if allow_empty_success {
                            return Ok(result);
                        }
                        let error = AgentError::llm_empty_response(self.client.provider().as_str());
                        // P0 Dogma Invariant 1: MeerkatMachine — not the shell —
                        // owns the recoverable-vs-fatal/exhaustion verdict. Only
                        // a machine `Recover` verdict drives the retry path;
                        // `Exhausted`/`Fatal` bubble the error up.
                        let recovery = self.classify_llm_failure_recovery(
                            &error,
                            attempt,
                            self.retry_policy.max_retries,
                        )?;
                        if let LlmFailureRecoveryKind::Recover = recovery {
                            // P0 Dogma Invariant 1: the MeerkatMachine
                            // `RecoverableFailure` gate must commit BEFORE the
                            // fallback target is selected or any durable/client
                            // mutation happens. Keep this as a boolean
                            // precondition only; the actual candidate proposal
                            // is requested after the machine accepts recovery.
                            let may_activate_fallback = fallback_activation_is_pre_stream_safe(
                                &error,
                                self.client.stream_output_observed(),
                            );
                            let retry_schedule = self
                                .retry_policy
                                .schedule_retry(&error, attempt, self.budget.remaining_duration())
                                .ok_or_else(|| {
                                    AgentError::InternalError(
                                        "MeerkatMachine classified LLM failure as Recover but the \
                                         retry policy produced no schedule"
                                            .to_string(),
                                    )
                                })?;
                            tracing::warn!(
                                "LLM completed without visible/actionable output (attempt {}), retrying in {}ms",
                                retry_schedule.plan.attempt,
                                retry_schedule.plan.selected_delay_ms,
                            );
                            let recover =
                                self.apply_turn_input(TurnExecutionInput::RecoverableFailure {
                                    run_id: run_id.clone(),
                                    retry: retry_schedule.clone(),
                                })?;
                            // Machine accepted the recovery: only now ask the
                            // client for a fallback proposal, wrap it in the
                            // machine-accepted activation authority, then
                            // mutate identity/policy/tool visibility atomically
                            // for the retry attempt.
                            if may_activate_fallback
                                && let Some(switch) = self.client.prepare_model_fallback(&error)
                            {
                                let activation = MachineAcceptedModelFallbackActivation::authorize(
                                    switch,
                                    &recover,
                                    &retry_schedule,
                                    self.effective_model_registry.as_deref(),
                                )?;
                                let (
                                    next_tools,
                                    next_params,
                                    next_max_tokens,
                                    notice,
                                    next_durable_visibility_parent,
                                ) = self
                                    .apply_model_fallback_switch(
                                        activation,
                                        &error,
                                        current_tools.as_ref(),
                                        extraction_output_schema.as_ref(),
                                        durable_visibility_parent.as_ref(),
                                    )
                                    .await?;
                                current_tools = next_tools;
                                current_provider_params = next_params;
                                current_max_tokens = next_max_tokens;
                                current_messages.push(notice);
                                *durable_visibility_parent = Some(next_durable_visibility_parent);
                            }
                            self.execute_turn_effects(&recover, turn_count, event_tx)
                                .await?;
                            let _ = crate::event_tap::tap_emit(
                                &self.event_tap,
                                event_tx.as_ref(),
                                AgentEvent::Retrying {
                                    retry: retry_schedule.clone(),
                                },
                            )
                            .await;
                            attempt += 1;
                            tokio::time::sleep(retry_schedule.plan.selected_delay()).await;
                            if let Some(exceeded) = self.budget.observe().exceeded() {
                                return Err(exceeded.to_agent_error());
                            }
                            let retry =
                                self.apply_turn_input(TurnExecutionInput::RetryRequested {
                                    run_id: run_id.clone(),
                                    retry_attempt: retry_schedule.plan.attempt,
                                })?;
                            self.execute_turn_effects(&retry, turn_count, event_tx)
                                .await?;
                            continue;
                        }
                        return Err(error);
                    }
                    return Ok(result);
                }
                Err(e) => {
                    // P0 Dogma Invariant 1: MeerkatMachine — not the shell —
                    // owns the recoverable-vs-fatal/exhaustion verdict. Only a
                    // machine `Recover` verdict drives the retry path;
                    // `Exhausted`/`Fatal` bubble the error up.
                    let recovery = self.classify_llm_failure_recovery(
                        &e,
                        attempt,
                        self.retry_policy.max_retries,
                    )?;
                    if let LlmFailureRecoveryKind::Recover = recovery {
                        // P0 Dogma Invariant 1: the MeerkatMachine
                        // `RecoverableFailure` gate must commit BEFORE the
                        // fallback target is selected or any durable/client
                        // mutation happens. Keep this as a boolean precondition
                        // only; the actual candidate proposal is requested
                        // after the machine accepts recovery.
                        let may_activate_fallback = fallback_activation_is_pre_stream_safe(
                            &e,
                            self.client.stream_output_observed(),
                        );
                        let retry_schedule = self
                            .retry_policy
                            .schedule_retry(&e, attempt, self.budget.remaining_duration())
                            .ok_or_else(|| {
                                AgentError::InternalError(
                                    "MeerkatMachine classified LLM failure as Recover but the \
                                     retry policy produced no schedule"
                                        .to_string(),
                                )
                            })?;
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
                        // Machine accepted the recovery: only now ask the client
                        // for a fallback proposal, wrap it in the
                        // machine-accepted activation authority, then mutate
                        // identity/policy/tool visibility atomically for the
                        // retry attempt.
                        if may_activate_fallback
                            && let Some(switch) = self.client.prepare_model_fallback(&e)
                        {
                            let activation = MachineAcceptedModelFallbackActivation::authorize(
                                switch,
                                &recover,
                                &retry_schedule,
                                self.effective_model_registry.as_deref(),
                            )?;
                            let (
                                next_tools,
                                next_params,
                                next_max_tokens,
                                notice,
                                next_durable_visibility_parent,
                            ) = self
                                .apply_model_fallback_switch(
                                    activation,
                                    &e,
                                    current_tools.as_ref(),
                                    extraction_output_schema.as_ref(),
                                    durable_visibility_parent.as_ref(),
                                )
                                .await?;
                            current_tools = next_tools;
                            current_provider_params = next_params;
                            current_max_tokens = next_max_tokens;
                            current_messages.push(notice);
                            *durable_visibility_parent = Some(next_durable_visibility_parent);
                        }
                        self.execute_turn_effects(&recover, turn_count, event_tx)
                            .await?;
                        let _ = crate::event_tap::tap_emit(
                            &self.event_tap,
                            event_tx.as_ref(),
                            AgentEvent::Retrying {
                                retry: retry_schedule.clone(),
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
                            .await?;
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
                    error_report: None,
                    error_class: None,
                    llm_request: None,
                    llm_response: None,
                    tool_call: None,
                    tool_result: None,
                },
                event_tx,
            )
            .await?;
        if let Some(error) = turn_boundary_report.denial_error(HookPoint::TurnBoundary) {
            return Err(error);
        }

        Ok(())
    }

    /// Apply a typed input to the runtime-backed turn-state authority and
    /// return the generated effects emitted by that transition.
    fn apply_turn_input_via_runtime_handle(
        &self,
        input: &TurnExecutionInput,
    ) -> Result<Vec<TurnExecutionEffect>, AgentError> {
        let Some(handle) = self.turn_state_handle.as_deref() else {
            return Ok(Vec::new());
        };

        handle.apply_turn_input(input.clone()).map_err(|err| {
            AgentError::InternalError(format!(
                "runtime turn-state handle rejected {input:?}: {err}"
            ))
        })
    }

    pub(super) fn apply_turn_input(
        &mut self,
        input: TurnExecutionInput,
    ) -> Result<TurnExecutionTransition, AgentError> {
        validate_turn_input_failure_reason(&input)?;
        let prev_phase = self.turn_phase()?;
        let effects = self.apply_turn_input_via_runtime_handle(&input)?;
        let next_phase = self.turn_phase()?;

        Ok(TurnExecutionTransition {
            prev_phase,
            next_phase,
            effects,
        })
    }

    /// P0 Dogma Invariant 1: mirror MeerkatMachine's LLM-failure recovery
    /// verdict.
    ///
    /// The agent loop EXTRACTS the typed recoverable failure kind from the
    /// `AgentError` (absent when the error yields no recoverable kind) plus the
    /// one-based `retry_attempt` and `max_retries`, then drives the machine's
    /// `ClassifyLlmFailureRecovery` classifier. MeerkatMachine — not this shell
    /// — owns whether the failure may `Recover`, is `Exhausted`, or is `Fatal`.
    /// This is a pure stateless classification (no turn-phase mutation); we
    /// mirror the emitted verdict and fail closed (returns an error) if the
    /// machine emits none.
    fn classify_llm_failure_recovery(
        &self,
        error: &AgentError,
        attempt_index: u32,
        max_retries: u32,
    ) -> Result<LlmFailureRecoveryKind, AgentError> {
        let failure_kind = crate::retry::LlmRetryFailure::from_agent_error(error).map(|f| f.kind);
        let retry_attempt = attempt_index.saturating_add(1);
        let effects = self.apply_turn_input_via_runtime_handle(
            &TurnExecutionInput::ClassifyLlmFailureRecovery {
                failure_kind,
                retry_attempt,
                max_retries,
            },
        )?;
        effects
            .into_iter()
            .find_map(|effect| match effect {
                TurnExecutionEffect::LlmFailureRecoveryClassified { recovery } => Some(recovery),
                _ => None,
            })
            .ok_or_else(|| {
                AgentError::InternalError(
                    "MeerkatMachine accepted ClassifyLlmFailureRecovery but emitted no recovery \
                     verdict"
                        .to_string(),
                )
            })
    }

    /// #128: mirror MeerkatMachine's empty-assistant-output verdict.
    ///
    /// The agent loop computes `has_visible_or_actionable` (this turn's
    /// assistant message OR any prior visible/actionable output in the run) as
    /// a pure pre-classifier, then drives the machine's
    /// `ClassifyAssistantOutput` classifier. MeerkatMachine — not this shell —
    /// owns whether the empty output is terminal. Fails closed if no verdict.
    fn classify_assistant_output_terminal(
        &self,
        has_visible_or_actionable: bool,
    ) -> Result<bool, AgentError> {
        let effects = self.apply_turn_input_via_runtime_handle(
            &TurnExecutionInput::ClassifyAssistantOutput {
                has_visible_or_actionable,
            },
        )?;
        effects
            .into_iter()
            .find_map(|effect| match effect {
                TurnExecutionEffect::AssistantOutputClassified {
                    empty_response_terminal,
                } => Some(empty_response_terminal),
                _ => None,
            })
            .ok_or_else(|| {
                AgentError::InternalError(
                    "MeerkatMachine accepted ClassifyAssistantOutput but emitted no verdict"
                        .to_string(),
                )
            })
    }

    /// #323: mirror MeerkatMachine's call-timeout verdict.
    ///
    /// Source selection stays shell-side (the loop knows which budget fired
    /// before awaiting); the machine owns whether the timeout is a retryable
    /// call timeout or a terminal turn-budget exhaustion. Fails closed if no
    /// verdict.
    fn classify_call_timeout(
        &self,
        source: CallTimeoutSource,
        timeout_ms: u64,
    ) -> Result<CallTimeoutVerdict, AgentError> {
        let dsl_source = match source {
            CallTimeoutSource::CallBudget => {
                crate::turn_execution_authority::CallTimeoutSource::CallBudget
            }
            CallTimeoutSource::TurnBudget => {
                crate::turn_execution_authority::CallTimeoutSource::TurnBudget
            }
        };
        let effects =
            self.apply_turn_input_via_runtime_handle(&TurnExecutionInput::ClassifyCallTimeout {
                source: dsl_source,
                timeout_ms,
            })?;
        effects
            .into_iter()
            .find_map(|effect| match effect {
                TurnExecutionEffect::CallTimeoutClassified { verdict, .. } => Some(verdict),
                _ => None,
            })
            .ok_or_else(|| {
                AgentError::InternalError(
                    "MeerkatMachine accepted ClassifyCallTimeout but emitted no verdict"
                        .to_string(),
                )
            })
    }

    fn started_primitive_run_from_authority(&self) -> Result<Option<RunId>, AgentError> {
        let snapshot = self.runtime_turn_authority_snapshot()?;
        if snapshot.turn_phase != TurnPhase::ApplyingPrimitive {
            return Ok(None);
        }
        let run_id = snapshot.active_run_id.ok_or_else(|| {
            AgentError::InternalError(
                "generated turn authority entered ApplyingPrimitive without active_run_id"
                    .to_string(),
            )
        })?;
        let primitive_kind = snapshot.primitive_kind.ok_or_else(|| {
            AgentError::InternalError(
                "generated turn authority entered ApplyingPrimitive without primitive_kind"
                    .to_string(),
            )
        })?;
        if primitive_kind == TurnPrimitiveKind::None {
            return Err(AgentError::InternalError(
                "generated turn authority entered ApplyingPrimitive with primitive_kind None"
                    .to_string(),
            ));
        }
        snapshot.admitted_content_shape.ok_or_else(|| {
            AgentError::InternalError(
                "generated turn authority entered ApplyingPrimitive without admitted_content_shape"
                    .to_string(),
            )
        })?;
        Ok(Some(run_id))
    }

    fn require_machine_terminal_failure_cause_kind(
        &self,
        context: String,
    ) -> Result<TurnTerminalCauseKind, AgentError> {
        let Some(cause_kind) = self.turn_terminal_cause_kind()? else {
            return Err(AgentError::InternalError(format!(
                "{context} missing machine-owned terminal_cause_kind"
            )));
        };
        if !cause_kind.is_specific_failure_cause() {
            return Err(AgentError::InternalError(format!(
                "{context} has unknown machine-owned terminal_cause_kind"
            )));
        }
        Ok(cause_kind)
    }

    /// Execute side effects from a transition. Handles CheckCompaction
    /// effects emitted on CallingLlm entry.
    ///
    /// Terminal effects (`RunCompleted`/`RunFailed`/`RunCancelled`) are applied
    /// against the runtime turn-state handle, which owns the canonical
    /// lifecycle fact. If the handle REJECTS a terminal effect, the run is NOT
    /// terminalized — fail closed by `?`-propagating the rejection as an
    /// `AgentError` (mirroring `apply_turn_input_via_runtime_handle`) instead of
    /// laundering it into a `tracing::warn!` and reporting the effect applied.
    async fn execute_turn_effects(
        &mut self,
        transition: &TurnExecutionTransition,
        // Retained in the signature as the turn context effects execute within;
        // no longer consumed since compaction indexing stopped using the turn
        // as a source-identity proxy (#152).
        _turn_count: u32,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
    ) -> Result<(), AgentError> {
        for effect in &transition.effects {
            match effect {
                TurnExecutionEffect::RunCompleted { run_id } => {
                    if let Some(handle) = self.turn_state_handle.as_deref() {
                        handle.run_completed(run_id.clone()).map_err(|err| {
                            AgentError::InternalError(format!(
                                "runtime turn-state handle rejected RunCompleted effect for \
                                 {run_id:?}: {err}"
                            ))
                        })?;
                    }
                }
                TurnExecutionEffect::RunFailed { run_id, reason } => {
                    if let Some(handle) = self.turn_state_handle.as_deref() {
                        handle
                            .run_failed(run_id.clone(), reason.clone())
                            .map_err(|err| {
                                AgentError::InternalError(format!(
                                    "runtime turn-state handle rejected RunFailed effect for \
                                     {run_id:?}: {err}"
                                ))
                            })?;
                    }
                }
                TurnExecutionEffect::RunCancelled { run_id } => {
                    if let Some(handle) = self.turn_state_handle.as_deref() {
                        handle.run_cancelled(run_id.clone()).map_err(|err| {
                            AgentError::InternalError(format!(
                                "runtime turn-state handle rejected RunCancelled effect for \
                                 {run_id:?}: {err}"
                            ))
                        })?;
                    }
                }
                TurnExecutionEffect::RunStarted { .. }
                | TurnExecutionEffect::BoundaryApplied { .. }
                | TurnExecutionEffect::CheckCompaction
                // Classifier verdicts are mirrored directly at their call
                // sites (classify_llm_failure_recovery / classify_assistant_output
                // / classify_call_timeout); never routed through turn-effect
                // execution.
                | TurnExecutionEffect::LlmFailureRecoveryClassified { .. }
                | TurnExecutionEffect::AssistantOutputClassified { .. }
                | TurnExecutionEffect::CallTimeoutClassified { .. } => {}
            }
            if let TurnExecutionEffect::CheckCompaction = effect {
                let current_boundary_index = self.compaction_cadence.session_boundary_index;
                if let Some(ref compactor) = self.compactor {
                    let last_guarded_boundary = [
                        self.compaction_cadence.last_compaction_boundary_index,
                        self.compaction_cadence
                            .last_compaction_attempt_boundary_index,
                    ]
                    .into_iter()
                    .flatten()
                    .max();
                    let ctx = crate::agent::compact::build_compaction_context(
                        self.session.messages(),
                        self.last_input_tokens,
                        last_guarded_boundary,
                        current_boundary_index,
                    );
                    if compactor.should_compact(&ctx) {
                        let rollback_state = crate::agent::CompactionRollbackState {
                            rollback_session: self.session.clone(),
                            rollback_last_input_tokens: self.last_input_tokens,
                            rollback_compaction_cadence: self.compaction_cadence.clone(),
                        };
                        self.compaction_cadence
                            .last_compaction_attempt_boundary_index = Some(current_boundary_index);
                        let outcome = crate::agent::compact::run_compaction(
                            self.client.as_ref(),
                            compactor,
                            self.compaction_curator.as_ref(),
                            crate::compact::CompactionWindow {
                                messages: self.session.messages(),
                                last_input_tokens: self.last_input_tokens,
                                session_boundary_index: current_boundary_index,
                            },
                            event_tx,
                            &self.event_tap,
                        )
                        .await;

                        if let Ok(outcome) = outcome {
                            // A successful summary call is real provider work
                            // even when the rebuilt transcript or its memory
                            // projection is later rejected. Charge it before
                            // any rewrite/projection branch so no failure path
                            // can silently refund usage.
                            self.session.record_usage(outcome.summary_usage.clone());
                            self.budget.record_usage(&outcome.summary_usage);
                            // Prepare the transcript rewrite on an isolated
                            // session value. Its exact TranscriptRewriteCommit
                            // becomes the identity of any paired memory stage.
                            let mut compacted_session = self.session.clone();
                            let prepared_rewrite = compacted_session
                                .replace_messages_for_compaction_internal(
                                    outcome.new_messages,
                                    &outcome.rewrite_authority,
                                );
                            let compacted_session = match prepared_rewrite {
                                Ok(Some(commit)) => Some((compacted_session, commit)),
                                Ok(None) => {
                                    let error =
                                        crate::agent::compact::CompactionError::InvalidRebuild(
                                            "compactor produced a no-op transcript rewrite"
                                                .to_string(),
                                        );
                                    tracing::warn!(error = %error, "rejected no-op compaction transcript rewrite");
                                    if !crate::event_tap::tap_emit(
                                        &self.event_tap,
                                        event_tx.as_ref(),
                                        AgentEvent::CompactionFailed {
                                            reason: crate::event::CompactionFailureReason::transcript_rewrite_failed(
                                                error.to_string(),
                                            ),
                                        },
                                    )
                                    .await
                                    {
                                        tracing::warn!(
                                            "compaction event stream receiver dropped before no-op rewrite CompactionFailed"
                                        );
                                    }
                                    None
                                }
                                Err(error) => {
                                    tracing::warn!(error = %error, "failed to prepare compaction transcript rewrite");
                                    if !crate::event_tap::tap_emit(
                                        &self.event_tap,
                                        event_tx.as_ref(),
                                        AgentEvent::CompactionFailed {
                                            reason: crate::event::CompactionFailureReason::transcript_rewrite_failed(
                                                error.to_string(),
                                            ),
                                        },
                                    )
                                    .await
                                    {
                                        tracing::warn!(
                                            "compaction event stream receiver dropped before rewrite CompactionFailed"
                                        );
                                    }
                                    None
                                }
                            };

                            let mut projection_failure = None;
                            let (rewrite_committed, emit_completed_now) = if let Some((
                                mut compacted_session,
                                commit,
                            )) = compacted_session
                            {
                                match self.memory_store.clone() {
                                        None => {
                                            self.session = compacted_session;
                                            (true, true)
                                        }
                                        Some(memory_store) => match memory_store
                                            .compaction_projection_persistence()
                                        {
                                            crate::memory::CompactionProjectionPersistence::Unsupported => {
                                                projection_failure = Some(
                                                    crate::memory::MemoryStoreError::Unsupported {
                                                        operation: "compaction_projection",
                                                    },
                                                );
                                                (false, false)
                                            }
                                            crate::memory::CompactionProjectionPersistence::EphemeralImmediate => {
                                                // Process-local only: publish
                                                // the in-memory rewrite first,
                                                // then index, and roll both
                                                // local facts back on failure.
                                                let original_session = std::mem::replace(
                                                    &mut self.session,
                                                    compacted_session,
                                                );
                                                let indexed = match self
                                                    .compaction_memory_batch(&outcome.discarded)
                                                {
                                                    Ok(batch) => memory_store
                                                        .index_scoped_batch(batch)
                                                        .await,
                                                    Err(error) => Err(error),
                                                };
                                                match indexed {
                                                    Ok(_) => (true, true),
                                                    Err(error) => {
                                                        self.session = original_session;
                                                        projection_failure = Some(error);
                                                        (false, false)
                                                    }
                                                }
                                            }
                                            crate::memory::CompactionProjectionPersistence::DurableStaged => {
                                                match self
                                                    .stage_durable_compaction_projection(
                                                        memory_store.as_ref(),
                                                        &commit,
                                                        &outcome.rewrite_authority,
                                                        &outcome.discarded,
                                                    )
                                                    .await
                                                {
                                                    Ok(projection) => {
                                                        let intent = crate::memory::CompactionProjectionIntent {
                                                            projection: projection.clone(),
                                                            summary_tokens: outcome.summary_usage.output_tokens,
                                                            messages_before: outcome.messages_before,
                                                            messages_after: outcome.messages_after,
                                                        };
                                                        match compacted_session
                                                            .add_compaction_projection_intent(intent)
                                                        {
                                                            Ok(()) => {
                                                                // Stage is durable but invisible;
                                                                // completion belongs to runtime
                                                                // atomic_apply + finalization.
                                                                let adopted = match self
                                                                    .compaction_transaction
                                                                    .as_mut()
                                                                {
                                                                    Some(transaction)
                                                                        if matches!(
                                                                            &transaction.phase,
                                                                            crate::agent::CompactionTransactionPhase::AwaitingRuntimeCommit(_)
                                                                        ) =>
                                                                    {
                                                                        transaction
                                                                            .projections
                                                                            .push(projection.clone());
                                                                        true
                                                                    }
                                                                    Some(_) => false,
                                                                    None => {
                                                                        self.compaction_transaction =
                                                                            Some(crate::agent::CompactionTransaction {
                                                                                phase: crate::agent::CompactionTransactionPhase::AwaitingRuntimeCommit(
                                                                                    Box::new(rollback_state),
                                                                                ),
                                                                                projections: vec![projection.clone()],
                                                                            });
                                                                        true
                                                                    }
                                                                };
                                                                if adopted {
                                                                    self.session = compacted_session;
                                                                    if self
                                                                        .in_flight_compaction_stage
                                                                        .as_ref()
                                                                        == Some(&projection)
                                                                    {
                                                                        self.in_flight_compaction_stage = None;
                                                                    }
                                                                    (true, false)
                                                                } else {
                                                                    let abort_error = self
                                                                        .abort_in_flight_compaction_stage(
                                                                            memory_store.as_ref(),
                                                                        )
                                                                        .await
                                                                        .err();
                                                                    projection_failure = Some(
                                                                        crate::memory::MemoryStoreError::Storage(
                                                                            match abort_error {
                                                                                Some(abort_error) => format!(
                                                                                    "compaction transaction is not awaiting runtime commit; additionally failed to abort invisible stage: {abort_error}"
                                                                                ),
                                                                                None => "compaction transaction is not awaiting runtime commit".to_string(),
                                                                            },
                                                                        ),
                                                                    );
                                                                    (false, false)
                                                                }
                                                            }
                                                            Err(error) => {
                                                                let abort_error = self
                                                                    .abort_in_flight_compaction_stage(
                                                                        memory_store.as_ref(),
                                                                    )
                                                                    .await
                                                                    .err();
                                                                projection_failure = Some(
                                                                    crate::memory::MemoryStoreError::Storage(
                                                                        match abort_error {
                                                                            Some(abort_error) => format!(
                                                                                "failed to carry compaction projection intent: {error}; additionally failed to abort invisible stage: {abort_error}"
                                                                            ),
                                                                            None => format!(
                                                                                "failed to carry compaction projection intent: {error}"
                                                                            ),
                                                                        },
                                                                    ),
                                                                );
                                                                (false, false)
                                                            }
                                                        }
                                                    }
                                                    Err(error) => {
                                                        projection_failure = Some(error);
                                                        (false, false)
                                                    }
                                                }
                                            }
                                        },
                                    }
                            } else {
                                (false, false)
                            };
                            if let Some(error) = projection_failure {
                                tracing::warn!(
                                    error = %error,
                                    attempted_entries = outcome.discarded.len(),
                                    "memory store rejected compaction projection; preserving original session history"
                                );
                                if !crate::event_tap::tap_emit(
                                    &self.event_tap,
                                    event_tx.as_ref(),
                                    AgentEvent::CompactionFailed {
                                        reason: crate::event::CompactionFailureReason::memory_indexing_failed(
                                            outcome.discarded.len(),
                                            error.to_string(),
                                        ),
                                    },
                                )
                                .await
                                {
                                    tracing::warn!(
                                        "compaction event stream receiver dropped before memory-indexing CompactionFailed"
                                    );
                                }
                            }
                            if let Some(projection) = self.in_flight_compaction_stage.as_ref() {
                                return Err(AgentError::InternalError(format!(
                                    "compaction stage {} remains pending cleanup",
                                    projection.revision()
                                )));
                            }
                            if rewrite_committed {
                                self.last_input_tokens = 0;
                                self.compaction_cadence.last_compaction_boundary_index =
                                    Some(outcome.session_boundary_index);
                                if emit_completed_now
                                    && !crate::event_tap::tap_emit(
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
                }
                self.compaction_cadence.session_boundary_index = self
                    .compaction_cadence
                    .session_boundary_index
                    .saturating_add(1);
                let cadence = self.compaction_cadence.clone();
                // A cadence-persist failure is a typed serialization fault, not
                // a best-effort no-op: surfacing it keeps the persisted cadence
                // and the in-memory `compaction_cadence` from silently diverging.
                crate::agent::compact::persist_compaction_cadence(self.session_mut(), &cadence)
                    .map_err(|error| {
                        AgentError::InternalError(format!(
                            "failed to persist session compaction cadence metadata: {error}"
                        ))
                    })?;
            }
        }
        Ok(())
    }

    #[cfg(test)]
    async fn index_compaction_discards(
        &self,
        discarded: &[crate::compact::CompactionDiscard],
    ) -> crate::memory::MemoryIndexDelivery {
        let session_id = self.session.id().clone();
        let scope = crate::memory::MemoryIndexScope::for_session(session_id.clone());
        let Some(memory_store) = self.memory_store.as_ref() else {
            return crate::memory::MemoryIndexDelivery::NoStore { scope };
        };
        let batch = match self.compaction_memory_batch(discarded) {
            Ok(batch) => batch,
            Err(error) => {
                return crate::memory::MemoryIndexDelivery::Rejected {
                    scope,
                    attempted_entries: discarded.len(),
                    error,
                };
            }
        };
        let attempted_entries = batch.len();
        match memory_store.index_scoped_batch(batch).await {
            Ok(receipt) => crate::memory::MemoryIndexDelivery::Delivered(receipt),
            Err(error) => crate::memory::MemoryIndexDelivery::Rejected {
                scope,
                attempted_entries,
                error,
            },
        }
    }

    fn compaction_memory_batch(
        &self,
        discarded: &[crate::compact::CompactionDiscard],
    ) -> Result<crate::memory::MemoryIndexBatch, crate::memory::MemoryStoreError> {
        self.compaction_memory_batch_at(discarded, crate::time_compat::SystemTime::now())
    }

    fn compaction_memory_batch_at(
        &self,
        discarded: &[crate::compact::CompactionDiscard],
        indexed_at: crate::time_compat::SystemTime,
    ) -> Result<crate::memory::MemoryIndexBatch, crate::memory::MemoryStoreError> {
        let session_id = self.session.id().clone();
        let scope = crate::memory::MemoryIndexScope::for_session(session_id.clone());
        let mut requests = Vec::new();
        // The compactor binds every discarded message to its canonical offset
        // in the full pre-compaction transcript. `run_compaction` validates
        // that provenance before this projection boundary.
        for discard in discarded {
            // Carry the typed indexability decision to the store; the store
            // owns the include/exclude policy rather than the producer inferring
            // it from an empty flattened string.
            let content = discard.message.indexable_content();
            let metadata = crate::memory::MemoryMetadata {
                session_id: session_id.clone(),
                source: crate::memory::MemorySource::Compaction {
                    source_range: crate::memory::MessageRange::single(discard.source_offset),
                },
                indexed_at,
            };
            let request = crate::memory::MemoryIndexRequest::new(scope.clone(), content, metadata)?;
            requests.push(request);
        }
        crate::memory::MemoryIndexBatch::new(scope, requests)
    }

    async fn stage_durable_compaction_projection(
        &mut self,
        memory_store: &dyn crate::memory::MemoryStore,
        commit: &crate::TranscriptRewriteCommit,
        authority: &crate::agent::compact::ValidatedCompactionRewrite,
        discarded: &[crate::compact::CompactionDiscard],
    ) -> Result<crate::memory::CompactionProjectionId, crate::memory::MemoryStoreError> {
        let projection = crate::memory::CompactionProjectionId::from_validated_transcript_rewrite(
            self.session.id().clone(),
            commit,
            authority,
        )
        .ok_or_else(|| {
            crate::memory::MemoryStoreError::Storage(
                "validated compaction witness does not authorize the projection commit".to_string(),
            )
        })?;
        let coordinator = self.compaction_commit_coordinator.as_ref().ok_or(
            crate::memory::MemoryStoreError::Unsupported {
                operation: "durable_compaction_without_runtime_coordinator",
            },
        )?;
        coordinator
            .authorize_projection(&projection)
            .map_err(|error| crate::memory::MemoryStoreError::Storage(error.to_string()))?;
        // Stable retry payload: the rewrite commit timestamp, not a fresh
        // wall-clock read, owns the staged metadata identity.
        let batch = self.compaction_memory_batch_at(discarded, commit.committed_at)?;
        if self.in_flight_compaction_stage.is_some() {
            return Err(crate::memory::MemoryStoreError::Storage(
                "cannot stage compaction while another exact stage identity is in flight"
                    .to_string(),
            ));
        }
        // Install the deterministic identity before the cancellation point.
        // If this await is dropped after the store commits, hard-interrupt
        // cleanup can abort this exact stage directly.
        self.in_flight_compaction_stage = Some(projection.clone());
        let staged = memory_store
            .stage_compaction_batch(projection.clone(), batch)
            .await;
        match staged {
            Ok(receipt) if receipt.projection == projection => Ok(projection),
            Ok(_) => {
                let cleanup = self
                    .abort_in_flight_compaction_stage(memory_store)
                    .await
                    .err();
                Err(crate::memory::MemoryStoreError::Storage(match cleanup {
                    Some(cleanup) => format!(
                        "memory store returned a stage receipt for a different compaction rewrite; additionally failed exact-stage cleanup: {cleanup}"
                    ),
                    None => {
                        "memory store returned a stage receipt for a different compaction rewrite"
                            .to_string()
                    }
                }))
            }
            Err(error) => {
                let cleanup = self
                    .abort_in_flight_compaction_stage(memory_store)
                    .await
                    .err();
                Err(crate::memory::MemoryStoreError::Storage(match cleanup {
                    Some(cleanup) => format!(
                        "failed to stage compaction projection: {error}; additionally failed exact-stage cleanup: {cleanup}"
                    ),
                    None => format!("failed to stage compaction projection: {error}"),
                }))
            }
        }
    }

    async fn abort_in_flight_compaction_stage(
        &mut self,
        memory_store: &dyn crate::memory::MemoryStore,
    ) -> Result<(), crate::memory::MemoryStoreError> {
        let Some(projection) = self.in_flight_compaction_stage.clone() else {
            return Ok(());
        };
        memory_store.abort_compaction_batch(&projection).await?;
        if self.in_flight_compaction_stage.as_ref() == Some(&projection) {
            self.in_flight_compaction_stage = None;
        }
        Ok(())
    }

    fn exact_compaction_projection_set(
        expected: &[crate::memory::CompactionProjectionId],
        actual: &[crate::memory::CompactionProjectionId],
    ) -> bool {
        expected.len() == actual.len()
            && expected
                .iter()
                .all(|projection| actual.contains(projection))
    }

    fn validate_runtime_compaction_authority(
        &self,
        intents: &[crate::memory::CompactionProjectionIntent],
        live_transaction: Option<(&[crate::memory::CompactionProjectionId], bool)>,
    ) -> Result<Vec<crate::memory::CompactionProjectionId>, AgentError> {
        let mut unique = std::collections::HashSet::new();
        let mut projections = Vec::with_capacity(intents.len());
        for intent in intents {
            if intent.projection.session_id() != self.session.id() {
                return Err(AgentError::InternalError(format!(
                    "runtime compaction outbox projection {} belongs to foreign session {}",
                    intent.projection.revision(),
                    intent.projection.session_id()
                )));
            }
            if !unique.insert(intent.projection.clone()) {
                return Err(AgentError::InternalError(format!(
                    "runtime compaction outbox contains duplicate projection {}",
                    intent.projection.revision()
                )));
            }
            projections.push(intent.projection.clone());
        }

        let validated_session_intents = self
            .session
            .validated_compaction_projection_intents()
            .map_err(|error| {
                AgentError::InternalError(format!(
                    "failed to validate session compaction intents against transcript history: {error}"
                ))
            })?;
        let history = self.session.transcript_history_state().map_err(|error| {
            AgentError::InternalError(format!(
                "failed to load compaction transcript history: {error}"
            ))
        })?;
        let commits = history
            .as_ref()
            .map(|history| history.commits.as_slice())
            .unwrap_or_default();
        for projection in &projections {
            if !commits
                .iter()
                .any(|commit| projection.matches_transcript_rewrite(self.session.id(), commit))
            {
                return Err(AgentError::InternalError(format!(
                    "runtime compaction outbox projection {} is not backed by the session transcript graph",
                    projection.revision()
                )));
            }
        }

        if let Some((expected, bookkeeping_complete)) = live_transaction {
            let mut expected_unique = std::collections::HashSet::new();
            if expected
                .iter()
                .any(|projection| !expected_unique.insert(projection.clone()))
            {
                return Err(AgentError::InternalError(
                    "live compaction transaction contains duplicate projection identities"
                        .to_string(),
                ));
            }
            if !Self::exact_compaction_projection_set(expected, &projections) {
                return Err(AgentError::InternalError(format!(
                    "runtime compaction outbox does not exactly match the live transaction (expected {}, received {})",
                    expected.len(),
                    projections.len()
                )));
            }
            if bookkeeping_complete {
                if !validated_session_intents.is_empty() {
                    return Err(AgentError::InternalError(
                        "runtime-committed compaction bookkeeping is complete but session intents remain"
                            .to_string(),
                    ));
                }
            } else if validated_session_intents.len() != intents.len()
                || !validated_session_intents
                    .iter()
                    .all(|session_intent| intents.contains(session_intent))
            {
                return Err(AgentError::InternalError(
                    "runtime compaction outbox does not exactly match the live session intents"
                        .to_string(),
                ));
            }
        } else {
            // Recovery may retry after an older process finalized only a
            // prefix and removed those local intents. RuntimeStore remains the
            // exact commit authority, but every still-carried session intent
            // must be present byte-for-byte and every supplied projection must
            // remain backed by the validated transcript graph.
            if !validated_session_intents
                .iter()
                .all(|session_intent| intents.contains(session_intent))
            {
                return Err(AgentError::InternalError(
                    "runtime compaction outbox omits a validated session projection intent"
                        .to_string(),
                ));
            }
        }
        Ok(projections)
    }

    /// Finalize every runtime-authorized memory stage before mutating local
    /// intent bookkeeping. The local mutation is all-or-none; a partial store
    /// failure keeps the typed transaction commit-only and fully retryable.
    pub async fn finalize_committed_compaction_projections(
        &mut self,
        projections: &[crate::memory::CompactionProjectionId],
    ) -> Result<(), AgentError> {
        if let Some(transaction) = self.compaction_transaction.as_ref()
            && !matches!(
                &transaction.phase,
                crate::agent::CompactionTransactionPhase::RuntimeCommitted { .. }
            )
        {
            return Err(AgentError::InternalError(
                "compaction finalization requires a runtime-committed transaction".to_string(),
            ));
        }
        if projections.is_empty() {
            return Ok(());
        }
        let memory_store = self.memory_store.clone().ok_or_else(|| {
            AgentError::InternalError(
                "runtime compaction outbox has work but the agent has no memory store".to_string(),
            )
        })?;
        let coordinator = self.compaction_commit_coordinator.clone().ok_or_else(|| {
            AgentError::InternalError(
                "runtime compaction outbox has work but the agent has no commit coordinator"
                    .to_string(),
            )
        })?;

        for projection in projections {
            coordinator
                .authorize_projection(projection)
                .map_err(|error| AgentError::InternalError(error.to_string()))?;
        }
        for projection in projections {
            memory_store
                .finalize_compaction_batch(projection)
                .await
                .map_err(|error| {
                    AgentError::InternalError(format!(
                        "failed to finalize compaction memory projection {}: {error}",
                        projection.revision()
                    ))
                })?;
        }

        // Build bookkeeping on a clone so serialization failure cannot remove
        // only a prefix after all memory stages have already published.
        let bookkeeping_was_complete =
            self.compaction_transaction
                .as_ref()
                .is_some_and(|transaction| {
                    matches!(
                        &transaction.phase,
                        crate::agent::CompactionTransactionPhase::RuntimeCommitted {
                            bookkeeping_complete: true
                        }
                    )
                });
        let mut next_session = self.session.clone();
        let mut completed_intents = Vec::new();
        for projection in projections {
            let completed = next_session
                .complete_compaction_projection_intent(projection)
                .map_err(|error| {
                    AgentError::InternalError(format!(
                        "failed to complete compaction projection intent {}: {error}",
                        projection.revision()
                    ))
                })?;
            if completed.is_none()
                && self.compaction_transaction.is_some()
                && !bookkeeping_was_complete
            {
                return Err(AgentError::InternalError(format!(
                    "live runtime-committed compaction intent {} disappeared before bookkeeping",
                    projection.revision()
                )));
            }
            if let Some(intent) = completed {
                completed_intents.push(intent);
            }
        }
        self.session = next_session;
        if let Some(transaction) = self.compaction_transaction.as_mut() {
            transaction.phase = crate::agent::CompactionTransactionPhase::RuntimeCommitted {
                bookkeeping_complete: true,
            };
        }

        // Authoritative intent bookkeeping is complete before event delivery.
        // Keep the explicit RuntimeCommitted phase through every await so a
        // cancelled observer delivery can never re-enable rollback.
        for intent in completed_intents {
            if !crate::event_tap::tap_emit(
                &self.event_tap,
                self.default_event_tx.as_ref(),
                AgentEvent::CompactionCompleted {
                    summary_tokens: intent.summary_tokens,
                    messages_before: intent.messages_before,
                    messages_after: intent.messages_after,
                },
            )
            .await
            {
                tracing::warn!(
                    revision = intent.projection.revision(),
                    "compaction event stream receiver dropped before committed CompactionCompleted"
                );
            }
        }
        let live_transaction_completed =
            self.compaction_transaction
                .as_ref()
                .is_some_and(|transaction| {
                    matches!(
                        &transaction.phase,
                        crate::agent::CompactionTransactionPhase::RuntimeCommitted {
                            bookkeeping_complete: true
                        }
                    ) && Self::exact_compaction_projection_set(
                        &transaction.projections,
                        projections,
                    )
                });
        if live_transaction_completed {
            self.compaction_transaction = None;
        }
        Ok(())
    }

    /// Reconcile invisible stages against the exact RuntimeStore atomic outbox
    /// and finalize those committed identities. A live transaction crosses the
    /// commit-only boundary synchronously after exact authority validation and
    /// before any store await.
    pub async fn reconcile_runtime_compaction_projections(
        &mut self,
        intents: &[crate::memory::CompactionProjectionIntent],
    ) -> Result<(), AgentError> {
        let live_transaction = match self.compaction_transaction.as_ref() {
            Some(transaction) => match &transaction.phase {
                crate::agent::CompactionTransactionPhase::AwaitingRuntimeCommit(_) => {
                    Some((transaction.projections.clone(), false))
                }
                crate::agent::CompactionTransactionPhase::RuntimeCommitted {
                    bookkeeping_complete,
                } => Some((transaction.projections.clone(), *bookkeeping_complete)),
                crate::agent::CompactionTransactionPhase::AbortPending { .. } => {
                    return Err(AgentError::InternalError(
                        "runtime compaction outbox cannot commit an abort-pending transaction"
                            .to_string(),
                    ));
                }
            },
            None => None,
        };
        let projections = self.validate_runtime_compaction_authority(
            intents,
            live_transaction
                .as_ref()
                .map(|(projections, complete)| (projections.as_slice(), *complete)),
        )?;
        if let Some(transaction) = self.compaction_transaction.as_mut()
            && matches!(
                &transaction.phase,
                crate::agent::CompactionTransactionPhase::AwaitingRuntimeCommit(_)
            )
        {
            transaction.phase = crate::agent::CompactionTransactionPhase::RuntimeCommitted {
                bookkeeping_complete: false,
            };
        }
        if self
            .in_flight_compaction_stage
            .as_ref()
            .is_some_and(|stage| projections.contains(stage))
        {
            return Err(AgentError::InternalError(
                "runtime outbox named an in-flight stage that never reached session intent authority"
                    .to_string(),
            ));
        }
        if !projections.is_empty() {
            let coordinator = self.compaction_commit_coordinator.as_ref().ok_or_else(|| {
                AgentError::InternalError(
                    "runtime compaction outbox has work but the agent has no commit coordinator"
                        .to_string(),
                )
            })?;
            for projection in &projections {
                coordinator
                    .authorize_projection(projection)
                    .map_err(|error| AgentError::InternalError(error.to_string()))?;
            }
        }

        let durable_store = self.memory_store.clone().filter(|memory_store| {
            memory_store.compaction_projection_persistence()
                == crate::memory::CompactionProjectionPersistence::DurableStaged
        });
        if (!projections.is_empty() || self.in_flight_compaction_stage.is_some())
            && durable_store.is_none()
        {
            return Err(AgentError::InternalError(
                "runtime compaction outbox has work without a durable staged memory store"
                    .to_string(),
            ));
        }
        if let Some(memory_store) = durable_store.as_ref() {
            if self.in_flight_compaction_stage.is_some() {
                self.abort_in_flight_compaction_stage(memory_store.as_ref())
                    .await
                    .map_err(|error| {
                        AgentError::InternalError(format!(
                            "failed to abort exact in-flight compaction stage before runtime reconciliation: {error}"
                        ))
                    })?;
            }
            memory_store
                .reconcile_compaction_stages(
                    &crate::memory::MemoryOwner::canonical_session(self.session.id().clone()),
                    &projections,
                )
                .await
                .map_err(|error| {
                    AgentError::InternalError(format!(
                        "failed to reconcile runtime compaction stages: {error}"
                    ))
                })?;
        }
        self.finalize_committed_compaction_projections(&projections)
            .await
    }

    fn monotonic_compaction_usage_delta(
        live: &crate::types::Usage,
        rollback: &crate::types::Usage,
    ) -> Result<crate::types::Usage, AgentError> {
        fn delta(live: u64, rollback: u64, field: &str) -> Result<u64, AgentError> {
            live.checked_sub(rollback).ok_or_else(|| {
                AgentError::InternalError(format!(
                    "compaction rollback observed non-monotonic {field} usage ({live} < {rollback})"
                ))
            })
        }
        fn optional_delta(
            live: Option<u64>,
            rollback: Option<u64>,
            field: &str,
        ) -> Result<Option<u64>, AgentError> {
            if live.is_none() && rollback.is_none() {
                return Ok(None);
            }
            delta(live.unwrap_or(0), rollback.unwrap_or(0), field).map(Some)
        }

        Ok(crate::types::Usage {
            input_tokens: delta(live.input_tokens, rollback.input_tokens, "input-token")?,
            output_tokens: delta(live.output_tokens, rollback.output_tokens, "output-token")?,
            cache_creation_tokens: optional_delta(
                live.cache_creation_tokens,
                rollback.cache_creation_tokens,
                "cache-creation-token",
            )?,
            cache_read_tokens: optional_delta(
                live.cache_read_tokens,
                rollback.cache_read_tokens,
                "cache-read-token",
            )?,
        })
    }

    /// Abort invisible stages only while the runtime commit boundary is still
    /// open. Rollback is applied exactly once, then represented as
    /// `AbortPending`; retries perform cleanup only and never reapply usage or
    /// cadence deltas. RuntimeCommitted is permanently commit-only.
    pub async fn abort_uncommitted_compaction_projections(&mut self) -> Result<(), AgentError> {
        let rollback_state = self
            .compaction_transaction
            .as_ref()
            .and_then(|transaction| match &transaction.phase {
                crate::agent::CompactionTransactionPhase::AwaitingRuntimeCommit(rollback) => {
                    Some((**rollback).clone())
                }
                _ => None,
            });
        if let Some(rollback) = rollback_state {
            let retained_usage = Self::monotonic_compaction_usage_delta(
                &self.session.total_usage(),
                &rollback.rollback_session.total_usage(),
            )?;
            let attempted_cadence = self.compaction_cadence.clone();
            let mut restored_session = rollback.rollback_session;
            if retained_usage != crate::types::Usage::default() {
                restored_session.record_usage(retained_usage);
            }
            let mut restored_cadence = rollback.rollback_compaction_cadence;
            restored_cadence.session_boundary_index = attempted_cadence.session_boundary_index;
            restored_cadence.last_compaction_attempt_boundary_index =
                attempted_cadence.last_compaction_attempt_boundary_index;
            self.session = restored_session;
            self.last_input_tokens = rollback.rollback_last_input_tokens;
            self.compaction_cadence = restored_cadence;
            if let Some(transaction) = self.compaction_transaction.as_mut() {
                transaction.phase = crate::agent::CompactionTransactionPhase::AbortPending {
                    cadence_persist_pending: true,
                };
            }
        }

        if self
            .compaction_transaction
            .as_ref()
            .is_some_and(|transaction| {
                matches!(
                    &transaction.phase,
                    crate::agent::CompactionTransactionPhase::RuntimeCommitted { .. }
                )
            })
        {
            return Err(AgentError::InternalError(
                "runtime-committed compaction transaction is commit-only and refuses abort"
                    .to_string(),
            ));
        }

        let mut errors = Vec::new();
        let cadence_persist_pending =
            self.compaction_transaction
                .as_ref()
                .is_some_and(|transaction| {
                    matches!(
                        &transaction.phase,
                        crate::agent::CompactionTransactionPhase::AbortPending {
                            cadence_persist_pending: true
                        }
                    )
                });
        if cadence_persist_pending {
            let cadence = self.compaction_cadence.clone();
            match crate::agent::compact::persist_compaction_cadence(self.session_mut(), &cadence) {
                Ok(()) => {
                    if let Some(transaction) = self.compaction_transaction.as_mut() {
                        transaction.phase =
                            crate::agent::CompactionTransactionPhase::AbortPending {
                                cadence_persist_pending: false,
                            };
                    }
                }
                Err(error) => errors.push(format!(
                    "failed to persist rolled-back compaction cadence: {error}"
                )),
            }
        }

        let projections = self
            .compaction_transaction
            .as_ref()
            .map(|transaction| transaction.projections.clone())
            .unwrap_or_default();
        let has_cleanup = self.in_flight_compaction_stage.is_some() || !projections.is_empty();
        let memory_store = if has_cleanup {
            match self.memory_store.clone() {
                Some(memory_store) => Some(memory_store),
                None => {
                    errors.push(
                        "uncommitted compaction cleanup has stages without a memory store"
                            .to_string(),
                    );
                    None
                }
            }
        } else {
            None
        };
        if let Some(memory_store) = memory_store {
            if let Some(projection) = self.in_flight_compaction_stage.clone()
                && let Err(error) = self
                    .abort_in_flight_compaction_stage(memory_store.as_ref())
                    .await
            {
                errors.push(format!(
                    "failed to abort exact in-flight compaction stage {}: {error}",
                    projection.revision()
                ));
            }
            for projection in projections {
                match memory_store.abort_compaction_batch(&projection).await {
                    Ok(()) => {
                        if let Some(transaction) = self.compaction_transaction.as_mut() {
                            transaction
                                .projections
                                .retain(|candidate| candidate != &projection);
                        }
                    }
                    Err(error) => errors.push(format!(
                        "failed to abort uncommitted compaction projection {}: {error}",
                        projection.revision()
                    )),
                }
            }
        }

        let abort_complete = self
            .compaction_transaction
            .as_ref()
            .is_some_and(|transaction| {
                transaction.projections.is_empty()
                    && matches!(
                        &transaction.phase,
                        crate::agent::CompactionTransactionPhase::AbortPending {
                            cadence_persist_pending: false
                        }
                    )
            });
        if abort_complete {
            self.compaction_transaction = None;
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(AgentError::InternalError(errors.join("; ")))
        }
    }

    fn observe_cancel_after_boundary_request(&mut self, run_id: &RunId) -> Result<(), AgentError> {
        // Non-blocking drain of the typed command channel. Coalesce any pending
        // commands into a single boundary observation, mirroring the prior
        // `AtomicBool::swap(false)` edge semantics: the request is observed at
        // most once per boundary regardless of how many commands queued. A
        // disconnected producer simply means no request is pending.
        let mut requested = false;
        while let Ok(command) = self.cancel_after_boundary_rx.try_recv() {
            // Commands are attachment/run capabilities, not SessionId-level
            // wishes. Discard stale commands instead of carrying them across a
            // turn transition and cancelling the successor run.
            if command.expected_run_id() == run_id {
                requested = true;
            }
        }
        if !requested {
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
        if self.turn_terminal()? {
            return Ok(());
        }
        self.terminal_error_detail = Some(error.to_string());
        self.terminal_error_metadata = crate::TurnErrorMetadata::from_agent_error(error);
        let transition = self.apply_turn_input(TurnExecutionInput::FatalFailure {
            run_id: run_id.clone(),
            failure: TurnFailureSource::from_agent_error(error),
        })?;
        self.execute_turn_effects(&transition, turn_count, event_tx)
            .await?;
        Ok(())
    }

    async fn complete_extraction_failed(
        &mut self,
        run_id: &RunId,
        turn_count: u32,
        tool_call_count: u32,
        reason: String,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
    ) -> Result<RunResult, AgentError> {
        let transition = self.apply_turn_input(TurnExecutionInput::ExtractionFailed {
            run_id: run_id.clone(),
            error: reason.clone(),
        })?;
        self.execute_turn_effects(&transition, turn_count, event_tx)
            .await?;

        let extraction_error = crate::types::ExtractionError {
            last_output: self
                .extraction_state
                .primary_output()
                .unwrap_or_default()
                .to_string(),
            attempts: self.turn_extraction_attempts()?,
            reason,
        };
        let result = RunResult {
            text: extraction_error.last_output.clone(),
            session_id: self.session.id().clone(),
            usage: self.session.total_usage(),
            turns: turn_count + 1,
            tool_calls: tool_call_count,
            terminal_cause_kind: None,
            structured_output: None,
            extraction_error: Some(extraction_error.clone()),
            schema_warnings: self.extraction_state.take_schema_warnings(),
            skill_diagnostics: self.resolve_skill_diagnostics_for_result().await,
        };
        self.save_session_best_effort().await;
        self.emit_extraction_failed_event(&extraction_error, event_tx.as_ref())
            .await;
        // `ExtractionFailed` IS the terminal observability event for a failed
        // extraction. Mark the run-completed event as already emitted so the
        // outer run loop does not additionally publish a success-shaped
        // `RunCompleted` (which carries `terminal_cause_kind: None` and would
        // launder the failed run as a clean completion to late observers).
        self.run_completed_event_emitted = true;
        Ok(result)
    }

    async fn execute_turn_hooks(
        &mut self,
        invocation: HookInvocation,
        run_id: &RunId,
        turn_count: u32,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
    ) -> Result<HookExecutionReport, AgentError> {
        match self.execute_hooks(invocation, event_tx.as_ref()).await {
            Ok(report) => Ok(report),
            Err(error) => {
                self.terminalize_fatal_error(run_id, turn_count, event_tx, &error)
                    .await?;
                Err(error)
            }
        }
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
        // The turn cap is resolved once at the build/composition seam
        // (`AgentBuilder::build_inner` applies `DEFAULT_MAX_TURNS` when the
        // surface left `config.max_turns` unset). The loop reads that resolved
        // value rather than re-deriving the default here; a missing resolution
        // is a build-seam invariant violation, not a license to invent a cap.
        let max_turns = self.config.max_turns.ok_or_else(|| {
            AgentError::InternalError(
                "agent turn cap was not resolved at the build seam: config.max_turns is None"
                    .to_string(),
            )
        })?;
        let mut tool_call_count = 0u32;
        let mut event_stream_open = true;
        let mut run_has_visible_or_actionable_output = false;
        self.extraction_state.reset();
        // RuntimeStore holds the pre-run Session snapshot until an explicit
        // sticky-fallback CAS advances its control projection. Seal that exact
        // parent once; CallingLlm visibility promotion/catalog refreshes mutate
        // only the live Agent session and must not rewrite the CAS expectation.
        let mut sticky_fallback_durable_visibility_parent =
            self.session.try_tool_visibility_state().map_err(|error| {
                AgentError::ConfigError(format!(
                    "failed to read the pre-run durable visibility parent: {error}"
                ))
            })?;
        // Flush any cancel-after-boundary commands left undrained by a prior
        // run so a stale request never leaks into this fresh run. The producer
        // end (the requesting surface) cannot reach the agent-owned receiver, so
        // the staleness reset lives here at the run-start seam — replacing the
        // previous surface-side `clear` of the shared `AtomicBool` carrier.
        while self.cancel_after_boundary_rx.try_recv().is_ok() {}

        // --- Authority lifecycle: consume the generated primitive start ---
        let run_id = if let Some(run_id) = self.started_primitive_run_from_authority()? {
            run_id
        } else {
            let run_id = self
                .turn_state_handle
                .as_deref()
                .and_then(|handle| handle.snapshot().active_run_id)
                .unwrap_or_else(|| RunId(uuid::Uuid::new_v4()));
            self.apply_turn_input(TurnExecutionInput::StartConversationRun {
                run_id: run_id.clone(),
                primitive_kind: TurnPrimitiveKind::ConversationTurn,
                admitted_content_shape: ContentShape::Conversation,
                vision_enabled: false,
                image_tool_results_enabled: false,
                max_extraction_retries: 0,
            })?;
            run_id
        };
        self.runtime_started_run_id = Some(run_id.clone());
        // Open the first actor-local boundary generation before the first
        // suspension point, and retain a run-scope guard so normal errors,
        // natural completion, hard-interrupt future drops, and task abort all
        // close any pending or parked preparation.
        let _system_context_boundary_run = self
            .system_context_state
            .begin_boundary_run(run_id.clone())
            .map_err(|error| AgentError::InternalError(error.to_string()))?;
        // PrimitiveApplied transitions ApplyingPrimitive -> CallingLlm
        let t = self.apply_turn_input(TurnExecutionInput::PrimitiveApplied {
            run_id: run_id.clone(),
        })?;
        self.execute_turn_effects(&t, 0, &event_tx).await?;

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
            if !self.turn_in_extraction_flow()? && turn_count >= max_turns {
                self.apply_turn_input(TurnExecutionInput::TurnLimitReached {
                    run_id: run_id.clone(),
                    turn_count: u64::from(turn_count),
                    max_turns: u64::from(max_turns),
                })?;
                return self.build_result(turn_count, tool_call_count).await;
            }

            if let Some(exceeded) = self.budget.observe().exceeded() {
                emit_event!(budget_warning_event(exceeded));
                if self.turn_in_extraction_flow()? {
                    return self
                        .complete_extraction_failed(
                            &run_id,
                            turn_count,
                            tool_call_count,
                            format!("extraction budget exceeded: {exceeded:?}"),
                            &event_tx,
                        )
                        .await;
                }
                self.apply_turn_input(TurnExecutionInput::BudgetLimitExceeded {
                    run_id: run_id.clone(),
                    exceeded,
                })?;
                return self.build_result(turn_count, tool_call_count).await;
            }

            match self.turn_phase()? {
                TurnPhase::Ready | TurnPhase::ApplyingPrimitive | TurnPhase::CallingLlm => {
                    self.system_context_state
                        .open_next_boundary(&run_id)
                        .map_err(|error| AgentError::InternalError(error.to_string()))?;
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
                    //    Notice refresh is ONE atomic transcript-authority
                    //    edit; a strip fault propagates typed instead of
                    //    leaving a stale notice in prompt truth.
                    self.session
                        .replace_synthetic_notices(SystemNoticeKind::AuthReauthRequired, Vec::new())
                        .map_err(|error| {
                            AgentError::InternalError(format!(
                                "auth synthetic notice refresh failed: {error}"
                            ))
                        })?;

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
                    //    The MCP lifecycle handle is the only authoritative
                    //    read side for pending server notices; the refresh is
                    //    ONE atomic strip+push transcript edit. A strip fault
                    //    propagates typed — no push happens on top of a stale
                    //    notice.
                    let pending_servers: Vec<String> = self
                        .mcp_server_lifecycle_handle
                        .as_deref()
                        .map(|handle| handle.pending_server_ids().into_iter().collect())
                        .unwrap_or_default();
                    let mcp_pending_notices = if pending_servers.is_empty() {
                        Vec::new()
                    } else {
                        let body = format!(
                            "Servers connecting: {}. Tools will appear when ready.",
                            pending_servers.join(", ")
                        );
                        vec![synthetic_notice_block_message(
                            SystemNoticeKind::McpPending,
                            body.clone(),
                            crate::types::SystemNoticeBlock::Mcp {
                                server_id: None,
                                operation: None,
                                phase: Some(crate::event::ExternalToolDeltaPhase::Pending),
                                persisted: false,
                                detail: Some(body),
                                pending_sources: pending_servers,
                            },
                        )]
                    };
                    self.session
                        .replace_synthetic_notices(
                            SystemNoticeKind::McpPending,
                            mcp_pending_notices,
                        )
                        .map_err(|error| {
                            AgentError::InternalError(format!(
                                "MCP pending synthetic notice refresh failed: {error}"
                            ))
                        })?;

                    // 3b. Background shell job completion notices via CompletionFeed.
                    //    Collected first, then applied through the ONE atomic
                    //    notice-refresh transcript edit below.
                    let mut background_job_notices: Vec<Message> = Vec::new();
                    // Feed path: ops-lifecycle-tracked completions from the runtime.
                    if let (Some(feed), Some(registry)) =
                        (self.completion_feed.as_ref(), self.ops_lifecycle.as_deref())
                    {
                        let batch = feed.list_since(self.applied_cursor);
                        let mut delivery_authority_ok = true;
                        for entry in &batch.entries {
                            match registry
                                .classify_operation_completion_wake(&entry.operation_id, entry.kind)
                            {
                                Ok(OperationCompletionWakeClass::Wake) => {}
                                Ok(OperationCompletionWakeClass::Ignore) => continue,
                                Err(err) => {
                                    tracing::warn!(
                                        operation_id = %entry.operation_id,
                                        kind = ?entry.kind,
                                        error = %err,
                                        "generated completion-wake authority rejected agent feed entry"
                                    );
                                    delivery_authority_ok = false;
                                    break;
                                }
                            }

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

                            emit_event!(AgentEvent::background_job_completed(
                                job_id.clone(),
                                entry.display_name.clone(),
                                terminal_status,
                                detail.clone(),
                            ));

                            let mut notice = format!(
                                "Background job `{}` (id={}) {}: {}",
                                entry.display_name, job_id, status_str, detail,
                            );
                            notice.push_str("\nUse shell_job_status to get the full output.");
                            background_job_notices.push(synthetic_notice_block_message(
                                SystemNoticeKind::BackgroundJob,
                                notice,
                                crate::types::SystemNoticeBlock::BackgroundJob {
                                    job_id,
                                    display_name: Some(entry.display_name.clone()),
                                    status: terminal_status,
                                    detail: Some(detail),
                                },
                            ));
                        }
                        if delivery_authority_ok {
                            match registry.advance_completion_cursor(
                                crate::ops_lifecycle::CompletionCursorConsumer::AgentApplied,
                                batch.watermark,
                                self.epoch_cursor_state.as_deref(),
                            ) {
                                Ok(cursor) => {
                                    self.applied_cursor = cursor;
                                }
                                Err(err) => {
                                    tracing::warn!(
                                        error = %err,
                                        cursor = batch.watermark,
                                        "generated completion cursor authority rejected agent-applied cursor advance"
                                    );
                                }
                            }
                        }
                    } else if self.completion_feed.is_some() {
                        tracing::debug!(
                            "completion feed present without generated ops cursor authority; skipping feed delivery"
                        );
                    }
                    // ONE atomic strip+push refresh for background-job
                    // notices: stale notices are cleared even when no feed is
                    // wired, and a strip fault propagates typed instead of
                    // pushing fresh notices beside stale ones.
                    self.session
                        .replace_synthetic_notices(
                            SystemNoticeKind::BackgroundJob,
                            background_job_notices,
                        )
                        .map_err(|error| {
                            AgentError::InternalError(format!(
                                "background-job synthetic notice refresh failed: {error}"
                            ))
                        })?;

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
                                    .map(|entry| entry.tool.name.clone())
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
                                                .map(|entry| entry.tool.name.clone())
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
                                self.publish_committed_visible_set().map_err(|err| {
                                    AgentError::InternalError(format!(
                                        "failed to persist canonical tool visibility state after boundary apply: {err}"
                                    ))
                                })?;
                                if applied.changed() {
                                    let status_info = ToolConfigChangeStatus::boundary_applied(
                                        applied.base_changed(),
                                        applied.visible_changed(),
                                        applied.applied_revision.0,
                                    );
                                    let status = status_info.status_text();
                                    let payload = ToolConfigChangedPayload::new(
                                        ToolConfigChangeOperation::Reload,
                                        "tool_scope",
                                        status_info,
                                        false,
                                    )
                                    .with_applied_at_turn(Some(turn_count))
                                    .with_domain(Some(ToolConfigChangeDomain::ToolScope));
                                    emit_event!(AgentEvent::ToolConfigChanged {
                                        payload: payload.clone(),
                                    });
                                    // Represent runtime notices as user-scoped synthetic context
                                    // (same pattern as peer lifecycle updates) so this does not
                                    // mutate or replace the canonical system prompt.
                                    self.session.push(synthetic_notice_block_message(
                                        SystemNoticeKind::ToolScope,
                                        format!(
                                            "Tool configuration changed at turn boundary: {status}"
                                        ),
                                        crate::types::SystemNoticeBlock::ToolConfig { payload },
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
                                    let payload = ToolConfigChangedPayload::new(
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
                                    }));
                                    emit_event!(AgentEvent::ToolConfigChanged {
                                        payload: payload.clone(),
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
                                        self.session.push(synthetic_notice_block_message(
                                            SystemNoticeKind::ToolScope,
                                            format!(
                                                "Deferred catalog changed at turn boundary: {}",
                                                notice_parts.join("; ")
                                            ),
                                            crate::types::SystemNoticeBlock::ToolConfig { payload },
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
                                let payload = ToolConfigChangedPayload::new(
                                    ToolConfigChangeOperation::Reload,
                                    "tool_scope",
                                    status_info,
                                    false,
                                )
                                .with_applied_at_turn(Some(turn_count))
                                .with_domain(Some(ToolConfigChangeDomain::ToolScope));
                                tracing::warn!(
                                    error = %err,
                                    "tool scope boundary apply failed; closing visible tool set for this boundary"
                                );
                                emit_event!(AgentEvent::ToolConfigChanged {
                                    payload: payload.clone(),
                                });
                                self.session.push(synthetic_notice_block_message(
                                    SystemNoticeKind::ToolScopeWarning,
                                    format!(
                                        "Tool scope apply failed ({err}); closing the visible tool set until the next boundary."
                                    ),
                                    crate::types::SystemNoticeBlock::ToolConfig { payload },
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

                    let in_extraction = self.turn_in_extraction_flow()?;

                    if !in_extraction {
                        emit_event!(AgentEvent::TurnStarted {
                            turn_number: turn_count,
                        });
                    }

                    let configured_max_tokens = self.config.resolved_max_tokens_per_turn();
                    let effective_max_tokens = self
                        .active_model_profile
                        .as_ref()
                        .and_then(crate::ModelProfileWitness::max_output_tokens)
                        .map(|limit| configured_max_tokens.min(limit))
                        .unwrap_or(configured_max_tokens);
                    let mut effective_temperature = self.config.temperature;
                    // Typed field-wise merge on the carrier: explicit params
                    // win, build-derived tool defaults fill unset slots. A
                    // provider-family conflict is a typed config fault.
                    let mut effective_provider_params = self
                        .config
                        .provider_params
                        .effective_params()
                        .map_err(|error| AgentError::ConfigError(error.to_string()))?;

                    let pre_llm_invocation = HookInvocation {
                        point: HookPoint::PreLlmRequest,
                        session_id: self.session.id().clone(),
                        turn_number: Some(turn_count),
                        prompt_input: None,
                        error_report: None,
                        error_class: None,
                        llm_request: Some(HookLlmRequest {
                            max_tokens: effective_max_tokens,
                            temperature: effective_temperature,
                            provider_params: (!effective_provider_params.is_empty())
                                .then(|| effective_provider_params.clone()),
                            message_count: self.session.messages().len(),
                        }),
                        llm_response: None,
                        tool_call: None,
                        tool_result: None,
                    };
                    // Pre-LLM hooks may observe or deny the turn.
                    let pre_llm_report = if in_extraction {
                        match self
                            .execute_hooks(pre_llm_invocation, event_tx.as_ref())
                            .await
                        {
                            Ok(report) => report,
                            Err(error) => {
                                return self
                                    .complete_extraction_failed(
                                        &run_id,
                                        turn_count,
                                        tool_call_count,
                                        error.to_string(),
                                        &event_tx,
                                    )
                                    .await;
                            }
                        }
                    } else {
                        self.execute_turn_hooks(pre_llm_invocation, &run_id, turn_count, &event_tx)
                            .await?
                    };

                    if let Some(error) = pre_llm_report.denial_error(HookPoint::PreLlmRequest) {
                        if in_extraction {
                            return self
                                .complete_extraction_failed(
                                    &run_id,
                                    turn_count,
                                    tool_call_count,
                                    error.to_string(),
                                    &event_tx,
                                )
                                .await;
                        }
                        self.terminalize_fatal_error(&run_id, turn_count, &event_tx, &error)
                            .await?;
                        return Err(error);
                    }

                    // In extraction mode, override tools/temperature/params
                    if in_extraction {
                        // Force temperature 0.0 for deterministic output
                        effective_temperature = Some(0.0_f32);
                        // Inject structured_output through the typed ProviderTag
                        // owner; an identity-conflict fault terminalizes the
                        // extraction instead of being silently dropped. A
                        // provider with no typed structured-output slot is a
                        // typed `NoProviderSlot` outcome — extraction proceeds
                        // prompt-based and the schema is enforced at the
                        // validation seam below.
                        if let Some(output_schema) = self.config.output_schema.clone() {
                            match effective_provider_params
                                .set_structured_output(self.client.provider(), output_schema)
                            {
                                Ok(_injection) => {}
                                Err(error) => {
                                    return self
                                        .complete_extraction_failed(
                                            &run_id,
                                            turn_count,
                                            tool_call_count,
                                            error.to_string(),
                                            &event_tx,
                                        )
                                        .await;
                                }
                            }
                        }
                        // Strip the provider-native web-search/grounding body via the typed
                        // ProviderTag owner — extraction is deterministic and tool-free.
                        effective_provider_params.clear_web_search();
                    }

                    // No tools for extraction turn (empty slice)
                    let empty_tools: Arc<[Arc<crate::types::ToolDef>]> = Arc::from([]);
                    let call_tool_defs = if in_extraction {
                        &empty_tools
                    } else {
                        &tool_defs
                    };
                    let typed_provider_params =
                        Some(effective_provider_params).filter(|params| !params.is_empty());

                    // Park on the exact boundary first, but keep the canonical
                    // context pending across fallible/async request hydration.
                    // Only the final synchronous consume below records that the
                    // model request has crossed its consumption seam.
                    let boundary_system_context = self
                        .prepare_pending_system_context_boundary(&run_id)
                        .await
                        .map_err(|e| AgentError::InternalError(e.to_string()))?;
                    let request_messages = self.llm_messages_with_runtime_system_context(
                        boundary_system_context.appends(),
                    );
                    let request_messages =
                        self.hydrate_llm_request_messages(request_messages).await?;
                    // Peer-ingestion observations are emitted while canonical
                    // context is still pending. If an awaited event send is
                    // cancelled, dropping the runner witness leaves it
                    // unapplied. Once the final send returns, consume and the
                    // LLM call follow synchronously in the same task poll.
                    for append in boundary_system_context.appends() {
                        for event in crate::event::peer_content_ingested_events(&append.content) {
                            let _ = crate::event_tap::tap_emit(
                                &self.event_tap,
                                event_tx.as_ref(),
                                event,
                            )
                            .await;
                        }
                    }
                    self.consume_pending_system_context_boundary(boundary_system_context)
                        .map_err(|e| AgentError::InternalError(e.to_string()))?;
                    // Steer-delivered peer content commits here (applied as
                    // runtime system context at the model boundary). There is
                    // no unrelated await between this consume and entry into
                    // the exact LLM future.
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
                            extraction_output_schema: if in_extraction {
                                self.config.output_schema.clone()
                            } else {
                                None
                            },
                            allow_empty_success: run_has_visible_or_actionable_output,
                            durable_visibility_parent:
                                &mut sticky_fallback_durable_visibility_parent,
                        })
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            if e.requires_session_teardown() {
                                return Err(e);
                            }
                            if in_extraction {
                                return self
                                    .complete_extraction_failed(
                                        &run_id,
                                        turn_count,
                                        tool_call_count,
                                        e.to_string(),
                                        &event_tx,
                                    )
                                    .await;
                            }
                            if let Some(exceeded) = BudgetExceeded::from_agent_error(&e) {
                                emit_event!(budget_warning_event(exceeded));
                                self.apply_turn_input(TurnExecutionInput::BudgetLimitExceeded {
                                    run_id: run_id.clone(),
                                    exceeded,
                                })?;
                                return self.build_result(turn_count, tool_call_count).await;
                            }
                            if matches!(&e, AgentError::Llm { .. }) {
                                // Recoverable LLM failures reaching this point have
                                // exhausted retry authority; non-recoverable LLM
                                // failures keep their typed LLM cause.
                                let retry_failure =
                                    crate::retry::LlmRetryFailure::from_agent_error(&e);
                                let failure = if retry_failure.is_some() {
                                    TurnFailureSource::llm_retry_exhausted(&e)
                                } else {
                                    TurnFailureSource::from_agent_error(&e)
                                };
                                self.terminal_error_detail = Some(e.to_string());
                                // Diagnostic fidelity is safe to retain only
                                // when the generated cause remains LlmFailure.
                                // Retry exhaustion is a different generated
                                // cause and is intentionally reconstructed
                                // from terminal truth by the runtime witness.
                                if retry_failure.is_none() {
                                    self.terminal_error_metadata =
                                        crate::TurnErrorMetadata::from_agent_error(&e);
                                }
                                let transition =
                                    self.apply_turn_input(TurnExecutionInput::FatalFailure {
                                        run_id: run_id.clone(),
                                        failure,
                                    })?;
                                self.execute_turn_effects(&transition, turn_count, &event_tx)
                                    .await?;
                                return self.build_result(turn_count, tool_call_count).await;
                            }
                            self.terminalize_fatal_error(&run_id, turn_count, &event_tx, &e)
                                .await?;
                            return Err(e);
                        }
                    };

                    // Update budget + session usage
                    self.budget.record_usage(&result.usage);
                    self.last_input_tokens = result.usage.input_tokens;
                    self.session.record_usage(result.usage.clone());
                    if let Some(exceeded) = self.budget.observe().exceeded() {
                        emit_event!(budget_warning_event(exceeded));
                        if in_extraction {
                            return self
                                .complete_extraction_failed(
                                    &run_id,
                                    turn_count,
                                    tool_call_count,
                                    format!("extraction budget exceeded: {exceeded:?}"),
                                    &event_tx,
                                )
                                .await;
                        }
                        self.apply_turn_input(TurnExecutionInput::BudgetLimitExceeded {
                            run_id: run_id.clone(),
                            exceeded,
                        })?;
                        return self.build_result(turn_count, tool_call_count).await;
                    }

                    let (blocks, stop_reason, usage) = result.into_parts();
                    let assistant_msg = self.stamp_assistant_message_identity(
                        BlockAssistantMessage::new(blocks, stop_reason),
                        &run_id,
                    );
                    let assistant_text = assistant_msg.to_string();

                    // #128: the shell computes the pure pre-classifier (this
                    // turn's output OR any prior visible/actionable output in the
                    // run) and MeerkatMachine owns the empty-output terminal
                    // verdict. The loop mirrors `empty_response_terminal`.
                    let turn_has_visible_or_actionable =
                        assistant_msg.has_visible_or_actionable_output();
                    let has_visible_or_actionable =
                        turn_has_visible_or_actionable || run_has_visible_or_actionable_output;
                    if self.classify_assistant_output_terminal(has_visible_or_actionable)? {
                        let error = AgentError::llm_empty_response(self.client.provider().as_str());
                        if in_extraction {
                            return self
                                .complete_extraction_failed(
                                    &run_id,
                                    turn_count,
                                    tool_call_count,
                                    error.to_string(),
                                    &event_tx,
                                )
                                .await;
                        }
                        self.terminalize_fatal_error(&run_id, turn_count, &event_tx, &error)
                            .await?;
                        return Err(error);
                    } else if turn_has_visible_or_actionable
                        && assistant_blocks_have_user_visible_output(&assistant_msg.blocks)
                    {
                        run_has_visible_or_actionable_output = true;
                    }

                    let post_llm_invocation = HookInvocation {
                        point: HookPoint::PostLlmResponse,
                        session_id: self.session.id().clone(),
                        turn_number: Some(turn_count),
                        prompt_input: None,
                        error_report: None,
                        error_class: None,
                        llm_request: None,
                        llm_response: Some(HookLlmResponse {
                            assistant_text: assistant_text.clone(),
                            tool_call_names: assistant_msg
                                .tool_calls()
                                .map(|call| call.name.to_string())
                                .collect(),
                            stop_reason: Some(stop_reason),
                            usage: Some(usage.clone()),
                            // Typed projection of the response's provider-executed
                            // server-tool evidence blocks, in block order, so a
                            // foreground PostLlmResponse hook classifies
                            // provider-native content synchronously.
                            server_tool_content: assistant_msg
                                .blocks
                                .iter()
                                .filter_map(|block| match block {
                                    crate::types::AssistantBlock::ServerToolContent {
                                        kind,
                                        ..
                                    } => Some(kind.clone()),
                                    _ => None,
                                })
                                .collect(),
                        }),
                        tool_call: None,
                        tool_result: None,
                    };
                    let post_llm_report = if in_extraction {
                        match self
                            .execute_hooks(post_llm_invocation, event_tx.as_ref())
                            .await
                        {
                            Ok(report) => report,
                            Err(error) => {
                                return self
                                    .complete_extraction_failed(
                                        &run_id,
                                        turn_count,
                                        tool_call_count,
                                        error.to_string(),
                                        &event_tx,
                                    )
                                    .await;
                            }
                        }
                    } else {
                        self.execute_turn_hooks(post_llm_invocation, &run_id, turn_count, &event_tx)
                            .await?
                    };

                    if let Some(error) = post_llm_report.denial_error(HookPoint::PostLlmResponse) {
                        if in_extraction {
                            return self
                                .complete_extraction_failed(
                                    &run_id,
                                    turn_count,
                                    tool_call_count,
                                    error.to_string(),
                                    &event_tx,
                                )
                                .await;
                        }
                        self.terminalize_fatal_error(&run_id, turn_count, &event_tx, &error)
                            .await?;
                        return Err(error);
                    }

                    if !in_extraction && !assistant_text.is_empty() {
                        emit_event!(AgentEvent::TextComplete {
                            content: assistant_text.clone(),
                        });
                    }

                    self.observe_cancel_after_boundary_request(&run_id)?;

                    // Check if we have tool calls
                    if assistant_msg.has_tool_calls() {
                        if in_extraction {
                            let tool_call_names = assistant_msg
                                .tool_calls()
                                .map(|tc| tc.name.to_string())
                                .collect::<Vec<_>>();
                            let reason = if tool_call_names.is_empty() {
                                "structured output extraction returned tool calls instead of JSON"
                                    .to_string()
                            } else {
                                format!(
                                    "structured output extraction returned tool calls instead of JSON: {}",
                                    tool_call_names.join(", ")
                                )
                            };
                            return self
                                .complete_extraction_failed(
                                    &run_id,
                                    turn_count,
                                    tool_call_count,
                                    reason,
                                    &event_tx,
                                )
                                .await;
                        }

                        let tool_calls: Vec<(ToolCallOwned, ToolCallArguments)> =
                            match assistant_msg
                                .tool_calls()
                                .map(|tc| {
                                    let args = ToolCallArguments::from_raw_json(tc.args).map_err(
                                        |error| tool_call_args_projection_error(tc.name, error),
                                    )?;
                                    Ok((ToolCallOwned::from_view(tc), args))
                                })
                                .collect::<Result<_, AgentError>>()
                            {
                                Ok(tool_calls) => tool_calls,
                                Err(error) => {
                                    self.terminalize_fatal_error(
                                        &run_id, turn_count, &event_tx, &error,
                                    )
                                    .await?;
                                    return Err(error);
                                }
                            };

                        // Add assistant message with ordered blocks
                        self.session
                            .push(Message::BlockAssistant(assistant_msg.clone()));
                        for image in assistant_image_events_from_blocks(&assistant_msg.blocks) {
                            emit_event!(AgentEvent::AssistantImageAppended { image });
                        }

                        // Emit tool call requests
                        for (tc, args) in &tool_calls {
                            emit_event!(AgentEvent::ToolCallRequested {
                                id: tc.id.clone(),
                                name: tc.name.clone(),
                                args: args.clone(),
                            });
                        }

                        // Transition to waiting for ops
                        let tc_count = tool_calls.len() as u32;
                        self.apply_turn_input(TurnExecutionInput::LlmReturnedToolCalls {
                            run_id: run_id.clone(),
                            tool_count: tc_count,
                        })?;
                        // The generated turn is now committed to a future
                        // post-tool model boundary. Open its exact actor-local
                        // generation before any tool/hook await so Steer can
                        // prepare while tools or barrier ops are still running.
                        self.system_context_state
                            .open_next_boundary(&run_id)
                            .map_err(|error| AgentError::InternalError(error.to_string()))?;

                        // Execute tool calls in parallel
                        let tools_ref = Arc::clone(&self.tools);
                        let mut executable_tool_calls = Vec::new();
                        let mut tool_results = Vec::with_capacity(tool_calls.len());
                        let visible_tool_names = tool_defs
                            .iter()
                            .map(|tool| tool.tool_name())
                            .collect::<ToolNameSet>();

                        // Typed provenance projection from the active tool
                        // catalog: hook payloads carry the `ToolDef.provenance`
                        // owner (matched by tool name), never a source
                        // re-derived from the name string.
                        let provenance_for_tool = |tool_name: &str| {
                            tool_defs
                                .iter()
                                .find(|tool| tool.name == tool_name)
                                .and_then(|tool| tool.provenance.clone())
                        };

                        let pre_tool_reports =
                            futures::future::join_all(tool_calls.iter().map(|(tc, args)| {
                                self.execute_hooks(
                                    HookInvocation {
                                        point: HookPoint::PreToolExecution,
                                        session_id: self.session.id().clone(),
                                        turn_number: Some(turn_count),
                                        prompt_input: None,
                                        error_report: None,
                                        error_class: None,
                                        llm_request: None,
                                        llm_response: None,
                                        tool_call: Some(HookToolCall {
                                            tool_use_id: tc.id.clone(),
                                            name: tc.name.clone(),
                                            args: args.clone(),
                                            provenance: provenance_for_tool(tc.name.as_str()),
                                        }),
                                        tool_result: None,
                                    },
                                    event_tx.as_ref(),
                                )
                            }))
                            .await;

                        for (tool_index, ((tc, _args), pre_tool_report)) in tool_calls
                            .into_iter()
                            .zip(pre_tool_reports.into_iter())
                            .enumerate()
                        {
                            let pre_tool_report = match pre_tool_report {
                                Ok(report) => report,
                                Err(error) => {
                                    self.terminalize_fatal_error(
                                        &run_id, turn_count, &event_tx, &error,
                                    )
                                    .await?;
                                    return Err(error);
                                }
                            };

                            if let Some(error) =
                                pre_tool_report.denial_error(HookPoint::PreToolExecution)
                            {
                                self.terminalize_fatal_error(
                                    &run_id, turn_count, &event_tx, &error,
                                )
                                .await?;
                                return Err(error);
                            }

                            if let Err(error) = precheck_visible_tool_call(
                                tools_ref.as_ref(),
                                &visible_tool_names,
                                tc.name.as_str(),
                            ) {
                                // Preserve the typed `access_denied` / `not_found`
                                // cause through terminalization so the distinction
                                // survives to `error_code()`/wire instead of being
                                // flattened into an opaque message.
                                let error = AgentError::tool(error);
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

                        // Execute all allowed tool calls in parallel, bounding
                        // concurrency with a semaphore and applying the typed
                        // per-call timeout policy from `tools_config`. Timeout
                        // expiry becomes a `ToolError::Timeout` so it flows
                        // through `terminal_tool_outcome_for_error` exactly like
                        // any other tool execution failure.
                        let dispatch_results = dispatch_tool_calls_boxed(
                            Arc::clone(&tools_ref),
                            self.tool_dispatch_context.clone(),
                            self.tools_config.default_timeout,
                            self.tools_config.tool_timeouts.clone(),
                            self.tools_config.max_concurrent.max(1),
                            executable_tool_calls,
                        )
                        .await;

                        // Process results and emit events
                        let mut all_async_ops = Vec::<crate::ops::AsyncOpRef>::new();
                        let mut accumulated_session_effects =
                            Vec::<crate::ops::SessionEffect>::new();
                        for (_, tc, dispatch_result, duration_ms) in dispatch_results {
                            let mut tool_session_effects = Vec::new();
                            let mut tool_result = match dispatch_result {
                                Ok(mut outcome) => {
                                    outcome.clear_terminal_cause();
                                    all_async_ops.extend(outcome.async_ops);
                                    tool_session_effects = outcome.session_effects;
                                    outcome.result
                                }
                                Err(crate::error::ToolError::CallbackPending {
                                    tool_name: callback_tool,
                                    args: callback_args,
                                }) => {
                                    // Callback-pending is a successful
                                    // continuation boundary for this core
                                    // turn, not a hard failure. Close the
                                    // generated turn before returning the
                                    // typed external terminal so the runtime
                                    // never observes a completed application
                                    // backed by a live WaitingForOps turn.
                                    let transition = self.apply_turn_input(
                                        TurnExecutionInput::CallbackPending {
                                            run_id: run_id.clone(),
                                        },
                                    )?;
                                    self.execute_turn_effects(&transition, turn_count, &event_tx)
                                        .await?;
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

                            // Reject unsupported video tool results BEFORE any
                            // success-shaped progress events are emitted and
                            // route the rejection through the turn machine, so
                            // public event truth never reports completion while
                            // the terminal truth is an error.
                            if tool_result.has_video() {
                                let error = AgentError::ConfigError(
                                    "video blocks are not supported in tool results".to_string(),
                                );
                                self.terminalize_fatal_error(
                                    &run_id, turn_count, &event_tx, &error,
                                )
                                .await?;
                                return Err(error);
                            }

                            let post_tool_report = self
                                .execute_turn_hooks(
                                    HookInvocation {
                                        point: HookPoint::PostToolExecution,
                                        session_id: self.session.id().clone(),
                                        turn_number: Some(turn_count),
                                        prompt_input: None,
                                        error_report: None,
                                        error_class: None,
                                        llm_request: None,
                                        llm_response: None,
                                        tool_call: None,
                                        tool_result: Some(
                                            HookToolResult::from_tool_result_with_id(
                                                tc.id.clone(),
                                                tc.name.clone(),
                                                &tool_result,
                                            )
                                            .with_provenance(provenance_for_tool(tc.name.as_str())),
                                        ),
                                    },
                                    &run_id,
                                    turn_count,
                                    &event_tx,
                                )
                                .await?;

                            if let Some(error) =
                                post_tool_report.denial_error(HookPoint::PostToolExecution)
                            {
                                self.terminalize_fatal_error(
                                    &run_id, turn_count, &event_tx, &error,
                                )
                                .await?;
                                return Err(error);
                            }

                            // Emit execution complete
                            emit_event!(AgentEvent::ToolExecutionCompleted {
                                id: tc.id.clone(),
                                name: tc.name.clone(),
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
                            self.apply_session_effects(&pre_tool_effects, Some(&run_id))?;
                        }
                        let tool_batch_produced_output =
                            tool_results.iter().any(|result| !result.is_error)
                                || post_tool_effects.iter().any(|effect| {
                                    matches!(
                                        effect,
                                        crate::ops::SessionEffect::AppendAssistantBlocks { blocks }
                                            if crate::types::assistant_blocks_have_visible_or_actionable_output(blocks)
                                    )
                                });

                        // Add tool results to session
                        self.session.push(Message::tool_results(tool_results));

                        let assistant_image_events =
                            assistant_image_events_from_effects(&post_tool_effects);
                        if !post_tool_effects.is_empty() {
                            self.apply_session_effects(&post_tool_effects, Some(&run_id))?;
                        }
                        if tool_batch_produced_output {
                            run_has_visible_or_actionable_output = true;
                        }
                        for image in assistant_image_events {
                            emit_event!(AgentEvent::AssistantImageAppended { image });
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
                        self.execute_turn_effects(&t, turn_count, &event_tx).await?;
                        turn_count += 1;
                    } else if self.turn_in_extraction_flow()? {
                        // Extraction turn response — validate against schema
                        let assistant_image_events =
                            assistant_image_events_from_blocks(&assistant_msg.blocks);
                        self.session.push(Message::BlockAssistant(assistant_msg));
                        for image in assistant_image_events {
                            emit_event!(AgentEvent::AssistantImageAppended { image });
                        }

                        // Drain turn boundary (fires TurnBoundary hooks, drains comms)
                        self.apply_turn_input(TurnExecutionInput::LlmReturnedTerminal {
                            run_id: run_id.clone(),
                        })?;
                        if let Err(error) = self
                            .drain_turn_boundary(turn_count, event_tx.as_ref())
                            .await
                        {
                            return self
                                .complete_extraction_failed(
                                    &run_id,
                                    turn_count,
                                    tool_call_count,
                                    error.to_string(),
                                    &event_tx,
                                )
                                .await;
                        }
                        self.observe_cancel_after_boundary_request(&run_id)?;

                        // Authority: DrainingBoundary -> Extracting for validation
                        self.apply_turn_input(TurnExecutionInput::EnterExtraction {
                            run_id: run_id.clone(),
                            max_retries: self.config.structured_output_retries,
                        })?;

                        let Some(output_schema) = self.config.output_schema.as_ref() else {
                            return self
                                .complete_extraction_failed(
                                    &run_id,
                                    turn_count,
                                    tool_call_count,
                                    "extraction flow without output_schema".to_string(),
                                    &event_tx,
                                )
                                .await;
                        };
                        let compiled = match self.client.compile_schema(output_schema) {
                            Ok(compiled) => compiled,
                            Err(error) => {
                                return self
                                    .complete_extraction_failed(
                                        &run_id,
                                        turn_count,
                                        tool_call_count,
                                        error.to_string(),
                                        &event_tx,
                                    )
                                    .await;
                            }
                        };
                        let validation = super::extraction::validate_response_text(
                            &assistant_text,
                            output_schema,
                            &compiled.schema,
                        );
                        let validation = match validation {
                            Ok(validation) => validation,
                            Err(error) => {
                                return self
                                    .complete_extraction_failed(
                                        &run_id,
                                        turn_count,
                                        tool_call_count,
                                        error.to_string(),
                                        &event_tx,
                                    )
                                    .await;
                            }
                        };

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

                                if !self.turn_terminal()? {
                                    // Authority decided to retry — push retry prompt
                                    self.session
                                        .push(Message::User(UserMessage::text(retry_prompt)));
                                    self.execute_turn_effects(&t, turn_count, &event_tx).await?;
                                    turn_count += 1;
                                    continue;
                                }

                                // Authority decided retries exhausted. The
                                // main run already completed; extraction is a
                                // post-run outcome and must not terminalize the
                                // run as failed.
                                self.execute_turn_effects(&t, turn_count, &event_tx).await?;
                                let extraction_error = crate::types::ExtractionError {
                                    last_output: self
                                        .extraction_state
                                        .primary_output()
                                        .unwrap_or_default()
                                        .to_string(),
                                    attempts: self.turn_extraction_attempts()?,
                                    reason: error,
                                };
                                let result = RunResult {
                                    text: extraction_error.last_output.clone(),
                                    session_id: self.session.id().clone(),
                                    usage: self.session.total_usage(),
                                    turns: turn_count + 1,
                                    tool_calls: tool_call_count,
                                    terminal_cause_kind: None,
                                    structured_output: None,
                                    extraction_error: Some(extraction_error.clone()),
                                    schema_warnings: self.extraction_state.take_schema_warnings(),
                                    skill_diagnostics: self
                                        .resolve_skill_diagnostics_for_result()
                                        .await,
                                };
                                self.save_session_best_effort().await;
                                self.emit_extraction_failed_event(
                                    &extraction_error,
                                    event_tx.as_ref(),
                                )
                                .await;
                                return Ok(result);
                            }
                            super::extraction::ExtractionValidation::Passed(normalized) => {
                                self.extraction_state.record_success(normalized);
                            }
                        }

                        let structured_output = self.extraction_state.take_result();
                        let schema_warnings = self.extraction_state.take_schema_warnings();
                        let result = RunResult {
                            text: self
                                .extraction_state
                                .primary_output()
                                .unwrap_or_default()
                                .to_string(),
                            session_id: self.session.id().clone(),
                            usage: self.session.total_usage(),
                            turns: turn_count + 1,
                            tool_calls: tool_call_count,
                            terminal_cause_kind: None,
                            structured_output,
                            extraction_error: None,
                            schema_warnings,
                            skill_diagnostics: self.resolve_skill_diagnostics_for_result().await,
                        };
                        // Validation passed — complete via authority
                        let t = self.apply_turn_input(
                            TurnExecutionInput::ExtractionValidationPassed {
                                run_id: run_id.clone(),
                            },
                        )?;
                        self.execute_turn_effects(&t, turn_count, &event_tx).await?;
                        self.save_session_best_effort().await;
                        if let Some(structured_output) = result.structured_output.clone() {
                            self.emit_extraction_succeeded_event(
                                structured_output,
                                result.schema_warnings.clone(),
                                event_tx.as_ref(),
                            )
                            .await;
                        }
                        return Ok(result);
                    } else {
                        // No tool calls - we're done with the agentic loop
                        let final_text = assistant_text.clone();
                        let assistant_image_events =
                            assistant_image_events_from_blocks(&assistant_msg.blocks);
                        self.session.push(Message::BlockAssistant(assistant_msg));
                        for image in assistant_image_events {
                            emit_event!(AgentEvent::AssistantImageAppended { image });
                        }

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
                        if let Some(output_schema) = self.config.output_schema.clone()
                            && !self.turn_in_extraction_flow()?
                        {
                            // Compile schema and capture warnings BEFORE emitting
                            // any success-shaped boundary event. Schema compile is
                            // a fallible extraction-setup step; if it fails the run
                            // must terminalize as an extraction failure, so we must
                            // not advertise `TurnCompleted`/`RunCompleted(success)`
                            // ahead of an outcome that can still fail (row #194).
                            let compiled = match self.client.compile_schema(&output_schema) {
                                Ok(compiled) => compiled,
                                Err(error) => {
                                    return self
                                        .complete_extraction_failed(
                                            &run_id,
                                            turn_count,
                                            tool_call_count,
                                            error.to_string(),
                                            &event_tx,
                                        )
                                        .await;
                                }
                            };

                            emit_event!(AgentEvent::TurnCompleted { stop_reason, usage });

                            // The main agentic turn has committed. Extraction
                            // is a separate post-run validation phase.
                            self.extraction_state.reset();
                            self.extraction_state.set_primary_output(final_text.clone());

                            let mut result = RunResult {
                                text: final_text.clone(),
                                session_id: self.session.id().clone(),
                                usage: self.session.total_usage(),
                                turns: turn_count + 1,
                                tool_calls: tool_call_count,
                                terminal_cause_kind: None,
                                structured_output: None,
                                extraction_error: None,
                                schema_warnings: None,
                                skill_diagnostics: None,
                            };
                            self.run_completed_hooks_before_terminal(
                                &mut result,
                                &run_id,
                                turn_count,
                                &event_tx,
                            )
                            .await?;
                            self.emit_run_completed_event(&result, true, event_tx.as_ref())
                                .await;
                            self.run_completed_event_emitted = true;

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
                            self.execute_turn_effects(&t, turn_count, &event_tx).await?;
                            turn_count += 1;
                            continue;
                        }

                        if self.turn_cancel_after_boundary()? {
                            let t =
                                self.apply_turn_input(TurnExecutionInput::BoundaryComplete {
                                    run_id: run_id.clone(),
                                })?;
                            self.execute_turn_effects(&t, turn_count, &event_tx).await?;
                            self.save_session_best_effort().await;
                            return self.build_result(turn_count + 1, tool_call_count).await;
                        }

                        let mut result = RunResult {
                            text: final_text,
                            session_id: self.session.id().clone(),
                            usage: self.session.total_usage(),
                            turns: turn_count + 1,
                            tool_calls: tool_call_count,
                            terminal_cause_kind: None,
                            structured_output: None,
                            extraction_error: None,
                            schema_warnings: None,
                            skill_diagnostics: self.resolve_skill_diagnostics_for_result().await,
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
                        self.execute_turn_effects(&t, turn_count, &event_tx).await?;

                        // Save session
                        self.save_session_best_effort().await;

                        return Ok(result);
                    }
                }
                TurnPhase::WaitingForOps => {
                    // Await completion of all pending barrier operations via
                    // the machine-owned turn-local wait-set. Only barrier ops
                    // block the turn; detached ops run independently.
                    // Authority/wait faults in this arm must apply
                    // `TurnExecutionInput::FatalFailure` to the turn machine
                    // before returning, mirroring the drain_turn_boundary path
                    // below; a raw `Err` would leave the turn non-terminal.
                    if !self.turn_pending_ops_registered()? {
                        let error = AgentError::InternalError(
                            "WaitingForOps entered without registered pending_op_refs".to_string(),
                        );
                        self.terminalize_fatal_error(&run_id, turn_count, &event_tx, &error)
                            .await?;
                        return Err(error);
                    }
                    let barrier_ids = self.turn_barrier_operation_ids()?;
                    if !barrier_ids.is_empty() {
                        // Clone the Arc so the immutable borrow of
                        // `self.ops_lifecycle` is released before the
                        // `&mut self` terminalization calls below.
                        let registry = match self.ops_lifecycle.clone() {
                            Some(registry) => registry,
                            None => {
                                let error = AgentError::InternalError(
                                    "barrier ops registered without ops_lifecycle registry"
                                        .to_string(),
                                );
                                self.terminalize_fatal_error(
                                    &run_id, turn_count, &event_tx, &error,
                                )
                                .await?;
                                return Err(error);
                            }
                        };
                        let wait_result = match registry.wait_all(&run_id, &barrier_ids).await {
                            Ok(wait_result) => wait_result,
                            Err(e) => {
                                let error = AgentError::InternalError(format!(
                                    "ops lifecycle wait_all failed: {e}"
                                ));
                                self.terminalize_fatal_error(
                                    &run_id, turn_count, &event_tx, &error,
                                )
                                .await?;
                                return Err(error);
                            }
                        };
                        // Feed the authority-owned wait-all obligation back through the
                        // generated handoff helper without relabeling its run identity.
                        let turn_handle = match self.turn_state_handle.as_deref() {
                            Some(turn_handle) => turn_handle,
                            None => {
                                let error = AgentError::InternalError(
                                    "runtime turn-state handle missing while submitting \
                                     ops barrier satisfaction feedback"
                                        .to_string(),
                                );
                                self.terminalize_fatal_error(
                                    &run_id, turn_count, &event_tx, &error,
                                )
                                .await?;
                                return Err(error);
                            }
                        };
                        if let Err(err) = submit_ops_barrier_satisfied(
                            turn_handle,
                            OpsBarrierSatisfactionObligation {
                                run_id: wait_result.satisfied.run_id,
                                operation_ids: wait_result
                                    .satisfied
                                    .operation_ids
                                    .into_iter()
                                    .collect(),
                            },
                        ) {
                            let error = AgentError::InternalError(format!(
                                "generated ops_barrier_satisfaction feedback rejected: {err}"
                            ));
                            self.terminalize_fatal_error(&run_id, turn_count, &event_tx, &error)
                                .await?;
                            return Err(error);
                        }
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
                    self.execute_turn_effects(&t, turn_count, &event_tx).await?;
                    turn_count += 1;
                }
                TurnPhase::DrainingBoundary | TurnPhase::Extracting => {
                    // Wait for any pending events to be processed
                    let t = self.apply_turn_input(TurnExecutionInput::BoundaryComplete {
                        run_id: run_id.clone(),
                    })?;
                    self.execute_turn_effects(&t, turn_count, &event_tx).await?;
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
                    self.execute_turn_effects(&t, turn_count, &event_tx).await?;
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
        let cause_kind = self.turn_terminal_cause_kind()?;
        match classify_terminal(&outcome, cause_kind) {
            SurfaceResultClass::Success => Ok(RunResult {
                text: self.session.last_assistant_text().unwrap_or_default(),
                session_id: self.session.id().clone(),
                usage: self.session.total_usage(),
                turns,
                tool_calls,
                terminal_cause_kind: public_terminal_cause_kind(cause_kind),
                structured_output: None,
                extraction_error: None,
                schema_warnings: None,
                skill_diagnostics: self.resolve_skill_diagnostics_for_result().await,
            }),
            SurfaceResultClass::HardFailure => {
                let cause_kind = match self.require_machine_terminal_failure_cause_kind(format!(
                    "hard-failure terminal outcome {outcome:?}"
                )) {
                    Ok(cause_kind) => cause_kind,
                    Err(error) => return Err(error),
                };
                Err(AgentError::TerminalFailure {
                    outcome,
                    cause_kind,
                    message: match &self.terminal_error_detail {
                        Some(detail) => {
                            format!("{}: {detail}", cause_kind.default_message(outcome))
                        }
                        None => cause_kind.default_message(outcome).to_string(),
                    },
                })
            }
            SurfaceResultClass::Cancelled => Err(AgentError::Cancelled),
            SurfaceResultClass::MissingTerminal => Err(AgentError::InternalError(
                "terminal result invariant violated: build_result() called without a \
                     machine terminal outcome"
                    .to_string(),
            )),
        }
    }

    /// Collect skill-runtime diagnostics for the terminal `RunResult`.
    ///
    /// Returns `Ok(None)` when there is no skill engine (genuinely absent),
    /// `Ok(Some(_))` when diagnostics were collected, and `Err(_)` when a typed
    /// collection fault occurred. This keeps a collection fault distinguishable
    /// from "healthy/absent" rather than laundering a typed `SkillError` into a
    /// whole-`None` (`health_snapshot().ok()?`) or an empty list
    /// (`quarantined_diagnostics().unwrap_or_default()`).
    ///
    async fn collect_skill_diagnostics(
        &self,
    ) -> Result<Option<crate::skills::SkillRuntimeDiagnostics>, crate::skills::SkillError> {
        let Some(runtime) = self.skill_engine.as_ref() else {
            return Ok(None);
        };
        let source_health = runtime.health_snapshot().await?;
        let quarantined = runtime.quarantined_diagnostics().await?;
        Ok(Some(crate::skills::SkillRuntimeDiagnostics {
            source_health,
            quarantined,
            collection_fault: None,
        }))
    }

    /// Resolve skill diagnostics for the terminal `RunResult`, recording any
    /// typed collection fault inside the result instead of dropping it.
    ///
    /// A collection fault produces diagnostics carrying the typed
    /// `collection_fault` (dogma row #239): surfaces receive an explicit
    /// failure fact instead of absent or healthy-looking diagnostics.
    async fn resolve_skill_diagnostics_for_result(
        &self,
    ) -> Option<crate::skills::SkillRuntimeDiagnostics> {
        match self.collect_skill_diagnostics().await {
            Ok(diagnostics) => diagnostics,
            Err(error) => {
                let reason = crate::event::SkillResolutionFailureReason::from_skill_error(&error);
                tracing::warn!(
                    %error,
                    "skill diagnostics collection failed; recording typed collection fault \
                     in the run result"
                );
                Some(crate::skills::SkillRuntimeDiagnostics {
                    source_health: Default::default(),
                    quarantined: Vec::new(),
                    collection_fault: Some(reason),
                })
            }
        }
    }
}

#[derive(Debug, Clone)]
struct ToolCallOwned {
    id: String,
    name: String,
    args: Box<RawValue>,
}

impl ToolCallOwned {
    fn from_view(view: ToolCallView<'_>) -> Self {
        Self {
            id: view.id.to_string(),
            name: view.name.into(),
            args: view.args.to_owned(),
        }
    }

    fn as_view(&self) -> ToolCallView<'_> {
        ToolCallView {
            id: &self.id,
            name: &self.name,
            args: &self.args,
        }
    }
}

type ToolDispatchResult = (
    usize,
    ToolCallOwned,
    Result<crate::ops::ToolDispatchOutcome, ToolError>,
    u64,
);

#[cfg(not(target_arch = "wasm32"))]
type ToolDispatchFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = Vec<ToolDispatchResult>> + Send + 'static>>;
#[cfg(target_arch = "wasm32")]
type ToolDispatchFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = Vec<ToolDispatchResult>> + 'static>>;

// Keep construction and polling of the concrete tool-dispatch batch out of
// the already-large agent run-loop poll frame.
#[inline(never)]
fn dispatch_tool_calls_boxed<T: AgentToolDispatcher + ?Sized + 'static>(
    tools_ref: Arc<T>,
    tool_dispatch_context: super::ToolDispatchContext,
    default_timeout: std::time::Duration,
    tool_timeouts: std::collections::HashMap<String, std::time::Duration>,
    max_concurrent: usize,
    executable_tool_calls: Vec<(usize, ToolCallOwned)>,
) -> ToolDispatchFuture {
    Box::pin(async move {
        let dispatch_semaphore = Arc::new(tokio::sync::Semaphore::new(max_concurrent));
        let dispatch_futures: Vec<_> = executable_tool_calls
            .into_iter()
            .map(|(tool_index, tc)| {
                let tools_ref = Arc::clone(&tools_ref);
                let tool_dispatch_context = tool_dispatch_context.clone();
                let dispatch_semaphore = Arc::clone(&dispatch_semaphore);
                let call_timeout = tool_timeouts
                    .get(tc.name.as_str())
                    .copied()
                    .unwrap_or(default_timeout);
                async move {
                    let start = crate::time_compat::Instant::now();
                    let dispatch_result = match dispatch_semaphore.acquire().await {
                        Ok(_permit) => {
                            match tokio::time::timeout(
                                call_timeout,
                                tools_ref
                                    .dispatch_with_context(tc.as_view(), &tool_dispatch_context),
                            )
                            .await
                            {
                                Ok(result) => result,
                                Err(_) => Err(ToolError::timeout(
                                    tc.name.clone(),
                                    u64::try_from(call_timeout.as_millis()).unwrap_or(u64::MAX),
                                )),
                            }
                        }
                        Err(_) => Err(ToolError::execution_failed(
                            "tool dispatch concurrency semaphore closed",
                        )),
                    };
                    let duration_ms = start.elapsed().as_millis() as u64;
                    (tool_index, tc, dispatch_result, duration_ms)
                }
            })
            .collect();

        let mut dispatch_results = futures::future::join_all(dispatch_futures).await;
        dispatch_results.sort_by_key(|(tool_index, _, _, _)| *tool_index);
        dispatch_results
    })
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::manual_async_fn
)]
mod tests {
    use super::{SystemNoticeKind, is_synthetic_notice};
    use crate::agent::{AgentBuilder, AgentLlmClient, AgentSessionStore, AgentToolDispatcher};
    use crate::blob::{BlobId, BlobPayload, BlobRef, BlobStore, BlobStoreError};
    use crate::budget::{Budget, BudgetLimits};
    use crate::compact::{
        COMPACTION_SUMMARY_PREFIX, CompactionContext, CompactionCurator, CompactionCuratorError,
        CompactionDiscard, CompactionResult, CompactionRetained, CompactionSummary,
        CompactionWindow, Compactor, CuratedCompactionSummary,
    };
    use crate::error::{
        AgentError, LlmFailureReason, LlmProviderError, LlmProviderErrorKind, ToolError,
    };
    use crate::event::AgentErrorClass;
    use crate::lifecycle::RunId;
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
    use crate::turn_execution_authority::{
        ContentShape, TurnFailureSource, TurnFailureSourceKind, TurnPrimitiveKind,
        TurnTerminalOutcome,
    };
    use crate::types::{
        AssistantBlock, ContentBlock, ImageData, Message, StopReason, ToolCall, ToolCallView,
        ToolDef, ToolResult, Usage, UserMessage,
    };
    use async_trait::async_trait;
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
    /// applies the generated MeerkatMachine kernel from
    /// `meerkat-machine-kernels` so lifecycle and terminal facts still move
    /// only through generated authority.
    ///
    /// See #32 Class W1 — runtime_turn_authority missing handle.
    fn with_test_turn_state_handle(builder: AgentBuilder) -> AgentBuilder {
        with_test_turn_state_handle_for_session(builder, crate::Session::new())
    }

    fn with_test_turn_state_handle_for_session(
        builder: AgentBuilder,
        mut session: crate::Session,
    ) -> AgentBuilder {
        use crate::agent::test_turn_state_handle::TestTurnStateHandle;
        session
            .set_build_state(crate::SessionBuildState::default())
            .expect("test session build state should serialize");
        builder
            .resume_session(session)
            .with_turn_state_handle(Arc::new(TestTurnStateHandle::new()))
            .with_runtime_execution_kind_for_test(
                crate::lifecycle::RuntimeExecutionKind::ContentTurn,
            )
    }

    fn explicit_test_visibility_owner() -> crate::GeneratedToolVisibilityOwner {
        let owner: Arc<dyn crate::ToolVisibilityOwner> =
            Arc::new(crate::tool_scope::GeneratedTestToolVisibilityOwner::new());
        crate::tool_scope::generated_test_tool_visibility_owner_from(owner)
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

        fn provider(&self) -> crate::provider::Provider {
            crate::provider::Provider::Other
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

    struct HotSwapLimitRecordingClient {
        model: String,
        seen_max_tokens: Mutex<Vec<u32>>,
    }

    impl HotSwapLimitRecordingClient {
        fn new(model: &str) -> Self {
            Self {
                model: model.to_string(),
                seen_max_tokens: Mutex::new(Vec::new()),
            }
        }

        fn seen_max_tokens(&self) -> Vec<u32> {
            self.seen_max_tokens.lock().unwrap().clone()
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
            _function_name: &crate::skills::SkillFunctionName,
            _arguments: crate::event::ToolCallArguments,
        ) -> impl Future<
            Output = Result<crate::skills::SkillFunctionOutput, crate::skills::SkillError>,
        > + Send {
            let missing = key.clone();
            async move { Err(crate::skills::SkillError::NotFound { key: missing }) }
        }
    }

    /// Skill engine whose health probe fails, used to assert that
    /// `collect_skill_diagnostics` surfaces the typed collection fault instead
    /// of laundering it into a whole-`None` diagnostics value (dogma row #239).
    struct FailingHealthSkillEngine;

    impl SkillEngine for FailingHealthSkillEngine {
        fn inventory_section(
            &self,
        ) -> impl Future<Output = Result<String, crate::skills::SkillError>> + Send {
            async move { Ok(String::new()) }
        }

        fn resolve_and_render(
            &self,
            _keys: &[SkillKey],
        ) -> impl Future<Output = Result<Vec<ResolvedSkill>, crate::skills::SkillError>> + Send
        {
            async move { Ok(Vec::new()) }
        }

        fn collections(
            &self,
        ) -> impl Future<Output = Result<Vec<SkillCollection>, crate::skills::SkillError>> + Send
        {
            async move { Ok(Vec::new()) }
        }

        fn list_skills(
            &self,
            _filter: &SkillFilter,
        ) -> impl Future<Output = Result<Vec<SkillDescriptor>, crate::skills::SkillError>> + Send
        {
            async move { Ok(Vec::new()) }
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
            async move {
                Err(crate::skills::SkillError::Load(
                    "skill source health probe failed".into(),
                ))
            }
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
            _function_name: &crate::skills::SkillFunctionName,
            _arguments: crate::event::ToolCallArguments,
        ) -> impl Future<
            Output = Result<crate::skills::SkillFunctionOutput, crate::skills::SkillError>,
        > + Send {
            let missing = key.clone();
            async move { Err(crate::skills::SkillError::NotFound { key: missing }) }
        }
    }

    /// Dogma row #239 (in-lane portion): a typed skill-diagnostics collection
    /// fault must remain distinguishable from "healthy/absent". The source seam
    /// no longer launders `health_snapshot()`/`quarantined_diagnostics()` errors
    /// into a whole-`None`/empty value; `collect_skill_diagnostics` returns the
    /// typed `SkillError` so the fault is surfaced rather than swallowed.
    #[tokio::test]
    async fn collect_skill_diagnostics_surfaces_typed_collection_fault() {
        let skill_runtime = Arc::new(crate::skills::SkillRuntime::new(Arc::new(
            FailingHealthSkillEngine,
        )));
        let agent = with_test_turn_state_handle(AgentBuilder::new())
            .with_skill_engine(skill_runtime)
            .build_standalone(
                Arc::new(StaticLlmClient),
                Arc::new(NoTools),
                Arc::new(NoopStore),
            )
            .await;

        match agent.collect_skill_diagnostics().await {
            Err(crate::skills::SkillError::Load(message)) => {
                assert!(
                    message.contains("health probe failed"),
                    "expected typed health-probe fault, got: {message}"
                );
            }
            Err(other) => panic!("expected typed Load fault, got: {other:?}"),
            Ok(diagnostics) => panic!(
                "collection fault must not launder into a healthy/absent diagnostics value, \
                 got: {diagnostics:?}"
            ),
        }
    }

    /// With no skill engine attached, the diagnostics collection is genuinely
    /// absent (`Ok(None)`) — distinct from the collection-fault case above.
    #[tokio::test]
    async fn collect_skill_diagnostics_absent_engine_is_ok_none() {
        let agent = build_agent(Arc::new(StaticLlmClient)).await;
        assert!(
            matches!(agent.collect_skill_diagnostics().await, Ok(None)),
            "absent skill engine must report Ok(None), not a fault"
        );
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

        fn provider(&self) -> crate::provider::Provider {
            crate::provider::Provider::OpenAI
        }

        fn model(&self) -> &'static str {
            "mock-model"
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentLlmClient for HotSwapLimitRecordingClient {
        async fn stream_response(
            &self,
            _messages: &[Message],
            _tools: &[Arc<ToolDef>],
            max_tokens: u32,
            _temperature: Option<f32>,
            _provider_params: Option<&crate::lifecycle::run_primitive::ProviderParamsOverride>,
        ) -> Result<super::LlmStreamResult, AgentError> {
            self.seen_max_tokens.lock().unwrap().push(max_tokens);
            Ok(super::LlmStreamResult::new(
                vec![AssistantBlock::Text {
                    text: "ok".to_string(),
                    meta: None,
                }],
                StopReason::EndTurn,
                Usage::default(),
            ))
        }

        fn provider(&self) -> crate::provider::Provider {
            crate::provider::Provider::OpenAI
        }

        fn model(&self) -> &str {
            &self.model
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

        fn provider(&self) -> crate::provider::Provider {
            crate::provider::Provider::Other
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

        fn provider(&self) -> crate::provider::Provider {
            crate::provider::Provider::Other
        }

        fn model(&self) -> &'static str {
            "mock-model"
        }
    }

    struct TrackingCompactor {
        compact_on_boundary: Option<u64>,
        seen_contexts: Mutex<Vec<CompactionContext>>,
    }

    fn identity_compaction_retentions(messages: &[Message]) -> Vec<CompactionRetained> {
        messages
            .iter()
            .cloned()
            .enumerate()
            .map(|(offset, message)| {
                let offset = u64::try_from(offset).unwrap_or(u64::MAX);
                CompactionRetained::new(offset, offset, message)
            })
            .collect()
    }

    fn test_compaction_summary(rebuilt_offset: u64, summary: &str) -> CompactionSummary {
        let message = Message::User(UserMessage::compaction_summary(format!(
            "{COMPACTION_SUMMARY_PREFIX}{summary}"
        )));
        CompactionSummary::new(rebuilt_offset, message)
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

        fn rebuild_history(&self, messages: &[Message], summary: &str) -> CompactionResult {
            CompactionResult {
                messages: messages.to_vec(),
                summary: test_compaction_summary(0, summary),
                retained: identity_compaction_retentions(messages),
                discarded: Vec::new(),
            }
        }
    }

    struct CadenceAwareFailingCompactor {
        seen_contexts: Mutex<Vec<CompactionContext>>,
    }

    impl CadenceAwareFailingCompactor {
        fn new() -> Self {
            Self {
                seen_contexts: Mutex::new(Vec::new()),
            }
        }

        fn seen_contexts(&self) -> Vec<CompactionContext> {
            self.seen_contexts.lock().unwrap().clone()
        }
    }

    impl Compactor for CadenceAwareFailingCompactor {
        fn should_compact(&self, ctx: &CompactionContext) -> bool {
            self.seen_contexts.lock().unwrap().push(ctx.clone());
            if ctx.session_boundary_index == 0 {
                return false;
            }
            if let Some(last) = ctx.last_compaction_boundary_index
                && ctx.session_boundary_index.saturating_sub(last) < 3
            {
                return false;
            }
            true
        }

        fn compaction_prompt(&self) -> &'static str {
            "COMPACT NOW"
        }

        fn max_summary_tokens(&self) -> u32 {
            32
        }

        fn rebuild_history(&self, messages: &[Message], summary: &str) -> CompactionResult {
            CompactionResult {
                messages: messages.to_vec(),
                summary: test_compaction_summary(0, summary),
                retained: identity_compaction_retentions(messages),
                discarded: Vec::new(),
            }
        }
    }

    struct FailingCompactionLlmClient {
        last_user_messages: Mutex<Vec<String>>,
    }

    impl FailingCompactionLlmClient {
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
    impl AgentLlmClient for FailingCompactionLlmClient {
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

            if last_user == "COMPACT NOW" {
                return Err(AgentError::Llm {
                    provider: "mock",
                    reason: LlmFailureReason::ProviderError(LlmProviderError::retryable(
                        LlmProviderErrorKind::IncompleteResponse,
                        serde_json::json!({"message": "stream ended before done"}),
                    )),
                    message: "stream ended before done".to_string(),
                });
            }

            Ok(super::LlmStreamResult::new(
                vec![AssistantBlock::Text {
                    text: "ok".to_string(),
                    meta: None,
                }],
                StopReason::EndTurn,
                Usage {
                    input_tokens: 200_000,
                    output_tokens: 5,
                    ..Usage::default()
                },
            ))
        }

        fn provider(&self) -> crate::provider::Provider {
            crate::provider::Provider::Other
        }

        fn model(&self) -> &'static str {
            "mock-model"
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
                .enumerate()
                .rev()
                .find(|(_, message)| matches!(message, Message::User(_)))
                .map(|(offset, message)| (offset, message.clone()));
            let summary_mapping = test_compaction_summary(0, summary);
            let mut compacted = vec![summary_mapping.message.clone()];
            let mut retained = Vec::new();
            if let Some((source_offset, last_user)) = last_user.as_ref() {
                retained.push(CompactionRetained::new(
                    u64::try_from(*source_offset).unwrap_or(u64::MAX),
                    u64::try_from(compacted.len()).unwrap_or(u64::MAX),
                    last_user.clone(),
                ));
                compacted.push(last_user.clone());
            }
            CompactionResult {
                messages: compacted,
                summary: summary_mapping,
                retained,
                discarded: messages
                    .iter()
                    .enumerate()
                    .filter(|(offset, _)| {
                        last_user
                            .as_ref()
                            .is_none_or(|(retained_offset, _)| offset != retained_offset)
                    })
                    .map(|(offset, message)| {
                        CompactionDiscard::new(
                            u64::try_from(offset).unwrap_or(u64::MAX),
                            message.clone(),
                        )
                    })
                    .collect(),
            }
        }
    }

    struct InvalidRewriteCompactor;

    impl Compactor for InvalidRewriteCompactor {
        fn should_compact(&self, ctx: &CompactionContext) -> bool {
            ctx.session_boundary_index == 1
        }

        fn compaction_prompt(&self) -> &'static str {
            "COMPACT NOW"
        }

        fn max_summary_tokens(&self) -> u32 {
            32
        }

        fn rebuild_history(&self, messages: &[Message], summary: &str) -> CompactionResult {
            let summary_mapping = test_compaction_summary(0, summary);
            let (tool_result_offset, orphan_tool_result) = messages
                .iter()
                .cloned()
                .enumerate()
                .find(|(_, message)| matches!(message, Message::ToolResults { .. }))
                .expect("invalid-rewrite fixture requires a source tool-result message");
            CompactionResult {
                // Both rebuilt messages have valid provenance: the summary is
                // the sole inserted message and the tool result is retained
                // byte-for-byte from the source. Dropping its paired assistant
                // tool-use message makes only the resulting transcript SHAPE
                // invalid, so the test reaches `replace_messages_internal`.
                messages: vec![summary_mapping.message.clone(), orphan_tool_result.clone()],
                summary: summary_mapping,
                retained: vec![CompactionRetained::new(
                    u64::try_from(tool_result_offset).unwrap_or(u64::MAX),
                    1,
                    orphan_tool_result,
                )],
                discarded: messages
                    .iter()
                    .cloned()
                    .enumerate()
                    .filter(|(offset, _)| *offset != tool_result_offset)
                    .map(|(offset, message)| {
                        CompactionDiscard::new(u64::try_from(offset).unwrap_or(u64::MAX), message)
                    })
                    .collect(),
            }
        }
    }

    struct NoOpClaimingDiscardCompactor;

    impl Compactor for NoOpClaimingDiscardCompactor {
        fn should_compact(&self, ctx: &CompactionContext) -> bool {
            ctx.session_boundary_index == 1
        }

        fn compaction_prompt(&self) -> &'static str {
            "COMPACT NOW"
        }

        fn max_summary_tokens(&self) -> u32 {
            32
        }

        fn rebuild_history(&self, messages: &[Message], summary: &str) -> CompactionResult {
            CompactionResult {
                // Offset zero is claimed as discarded while the identical
                // value remains at the one unmapped (nominal summary) slot.
                // The explicit provenance is structurally complete, but the
                // accepted transcript is still a no-op and must not be
                // projected into semantic memory.
                messages: messages.to_vec(),
                summary: test_compaction_summary(0, summary),
                retained: messages
                    .iter()
                    .cloned()
                    .enumerate()
                    .skip(1)
                    .map(|(offset, message)| {
                        let offset = u64::try_from(offset).unwrap_or(u64::MAX);
                        CompactionRetained::new(offset, offset, message)
                    })
                    .collect(),
                discarded: messages
                    .first()
                    .cloned()
                    .map(|message| vec![CompactionDiscard::new(0, message)])
                    .unwrap_or_default(),
            }
        }
    }

    struct SubstitutingCurator {
        seen_windows: Mutex<Vec<(usize, u64, u64)>>,
    }

    impl SubstitutingCurator {
        fn new() -> Self {
            Self {
                seen_windows: Mutex::new(Vec::new()),
            }
        }

        fn seen_windows(&self) -> Vec<(usize, u64, u64)> {
            self.seen_windows.lock().unwrap().clone()
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl CompactionCurator for SubstitutingCurator {
        async fn curate_summary(
            &self,
            window: CompactionWindow<'_>,
        ) -> Result<CuratedCompactionSummary, CompactionCuratorError> {
            self.seen_windows.lock().unwrap().push((
                window.messages.len(),
                window.last_input_tokens,
                window.session_boundary_index,
            ));
            CuratedCompactionSummary::new("curated summary")
        }
    }

    struct FailingCurator;

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl CompactionCurator for FailingCurator {
        async fn curate_summary(
            &self,
            _window: CompactionWindow<'_>,
        ) -> Result<CuratedCompactionSummary, CompactionCuratorError> {
            Err(CompactionCuratorError::Failed(
                "curator offline".to_string(),
            ))
        }
    }

    struct EmptySummaryCurator;

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl CompactionCurator for EmptySummaryCurator {
        async fn curate_summary(
            &self,
            _window: CompactionWindow<'_>,
        ) -> Result<CuratedCompactionSummary, CompactionCuratorError> {
            // Exercises the fallible constructor: whitespace-only text can
            // never mint a CuratedCompactionSummary.
            CuratedCompactionSummary::new("   ")
        }
    }

    struct RecordingMemoryStore {
        entries: Mutex<
            Vec<(
                MemoryIndexScope,
                crate::types::MemoryIndexableContent,
                MemoryMetadata,
            )>,
        >,
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
                .map(|(_, content, _)| content.clone().into_indexable_text())
                .collect()
        }

        fn typed_contents(&self) -> Vec<crate::types::MemoryIndexableContent> {
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

        fn source_ranges(&self) -> Vec<crate::memory::MessageRange> {
            self.entries
                .lock()
                .unwrap()
                .iter()
                .filter_map(|(_, _, metadata)| metadata.source.source_range())
                .collect()
        }
    }

    #[async_trait]
    impl MemoryStore for RecordingMemoryStore {
        fn compaction_projection_persistence(
            &self,
        ) -> crate::memory::CompactionProjectionPersistence {
            crate::memory::CompactionProjectionPersistence::EphemeralImmediate
        }

        async fn index_scoped_batch(
            &self,
            batch: MemoryIndexBatch,
        ) -> Result<MemoryIndexReceipt, MemoryStoreError> {
            if self.fail_indexing {
                return Err(MemoryStoreError::Storage("injected failure".to_string()));
            }
            let (receipt_scope, requests) = batch.into_parts();
            let indexed_entries = requests.len();
            let mut entries = self.entries.lock().unwrap();
            if self
                .fail_after_successful_entries
                .is_some_and(|successful_entries| indexed_entries > successful_entries)
            {
                return Err(MemoryStoreError::Storage("injected failure".to_string()));
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

    struct AbortRetryMemoryStore {
        fail_once: Mutex<Option<crate::memory::CompactionProjectionId>>,
        finalize_fail_once: Mutex<Option<crate::memory::CompactionProjectionId>>,
        staged: Mutex<Vec<crate::memory::CompactionProjectionId>>,
        aborted: Mutex<Vec<crate::memory::CompactionProjectionId>>,
        finalized: Mutex<Vec<crate::memory::CompactionProjectionId>>,
        reconciled_committed: Mutex<Vec<Vec<crate::memory::CompactionProjectionId>>>,
    }

    impl AbortRetryMemoryStore {
        fn new() -> Self {
            Self {
                fail_once: Mutex::new(None),
                finalize_fail_once: Mutex::new(None),
                staged: Mutex::new(Vec::new()),
                aborted: Mutex::new(Vec::new()),
                finalized: Mutex::new(Vec::new()),
                reconciled_committed: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl MemoryStore for AbortRetryMemoryStore {
        fn compaction_projection_persistence(
            &self,
        ) -> crate::memory::CompactionProjectionPersistence {
            crate::memory::CompactionProjectionPersistence::DurableStaged
        }

        async fn index_scoped_batch(
            &self,
            batch: MemoryIndexBatch,
        ) -> Result<MemoryIndexReceipt, MemoryStoreError> {
            let (scope, requests) = batch.into_parts();
            Ok(MemoryIndexReceipt {
                scope,
                indexed_entries: requests.len(),
            })
        }

        async fn stage_compaction_batch(
            &self,
            projection: crate::memory::CompactionProjectionId,
            batch: MemoryIndexBatch,
        ) -> Result<crate::memory::CompactionStageReceipt, MemoryStoreError> {
            let staged_entries = batch.len();
            self.staged.lock().unwrap().push(projection.clone());
            Ok(crate::memory::CompactionStageReceipt {
                projection,
                staged_entries,
            })
        }

        async fn abort_compaction_batch(
            &self,
            projection: &crate::memory::CompactionProjectionId,
        ) -> Result<(), MemoryStoreError> {
            let mut fail_once = self.fail_once.lock().unwrap();
            if fail_once.as_ref() == Some(projection) {
                *fail_once = None;
                return Err(MemoryStoreError::Storage(
                    "injected abort failure".to_string(),
                ));
            }
            self.staged
                .lock()
                .unwrap()
                .retain(|staged| staged != projection);
            self.aborted.lock().unwrap().push(projection.clone());
            Ok(())
        }

        async fn finalize_compaction_batch(
            &self,
            projection: &crate::memory::CompactionProjectionId,
        ) -> Result<MemoryIndexReceipt, MemoryStoreError> {
            let mut fail_once = self.finalize_fail_once.lock().unwrap();
            if fail_once.as_ref() == Some(projection) {
                *fail_once = None;
                return Err(MemoryStoreError::Storage(
                    "injected finalize failure".to_string(),
                ));
            }
            self.staged
                .lock()
                .unwrap()
                .retain(|staged| staged != projection);
            let mut finalized = self.finalized.lock().unwrap();
            if !finalized.contains(projection) {
                finalized.push(projection.clone());
            }
            Ok(MemoryIndexReceipt {
                scope: MemoryIndexScope::for_session(projection.session_id().clone()),
                indexed_entries: 0,
            })
        }

        async fn reconcile_compaction_stages(
            &self,
            _owner: &crate::memory::MemoryOwner,
            committed: &[crate::memory::CompactionProjectionId],
        ) -> Result<crate::memory::CompactionStageReconcileReceipt, MemoryStoreError> {
            self.reconciled_committed
                .lock()
                .unwrap()
                .push(committed.to_vec());
            Ok(crate::memory::CompactionStageReconcileReceipt::default())
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

    struct AllowCompactionCoordinator {
        session_id: crate::SessionId,
    }

    impl crate::memory::CompactionCommitCoordinator for AllowCompactionCoordinator {
        fn authorize_projection(
            &self,
            projection: &crate::memory::CompactionProjectionId,
        ) -> Result<(), crate::memory::CompactionCommitCoordinationError> {
            if projection.session_id() == &self.session_id {
                Ok(())
            } else {
                Err(
                    crate::memory::CompactionCommitCoordinationError::SessionMismatch {
                        expected: self.session_id.clone(),
                        actual: projection.session_id().clone(),
                    },
                )
            }
        }
    }

    #[derive(Clone, Copy)]
    enum PostCompactionOutcome {
        CallbackPending,
        Failure,
        SuccessWithUsage,
    }

    struct CompactionTerminalLlmClient {
        ordinary_calls: Mutex<usize>,
        second_outcome: PostCompactionOutcome,
    }

    impl CompactionTerminalLlmClient {
        fn new(second_outcome: PostCompactionOutcome) -> Self {
            Self {
                ordinary_calls: Mutex::new(0),
                second_outcome,
            }
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentLlmClient for CompactionTerminalLlmClient {
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
            if last_user == "COMPACT NOW" {
                return Ok(super::LlmStreamResult::new(
                    vec![AssistantBlock::Text {
                        text: "durable summary".to_string(),
                        meta: None,
                    }],
                    StopReason::EndTurn,
                    Usage {
                        input_tokens: 7,
                        output_tokens: 3,
                        ..Usage::default()
                    },
                ));
            }

            let ordinary_call = {
                let mut calls = self.ordinary_calls.lock().unwrap();
                let call = *calls;
                *calls = calls.saturating_add(1);
                call
            };
            if ordinary_call == 1 {
                return match self.second_outcome {
                    PostCompactionOutcome::CallbackPending => Ok(super::LlmStreamResult::new(
                        vec![AssistantBlock::ToolUse {
                            id: "callback-after-compaction".to_string(),
                            name: "external_callback".into(),
                            args: serde_json::value::RawValue::from_string("{}".to_string())
                                .unwrap(),
                            meta: None,
                        }],
                        StopReason::ToolUse,
                        Usage::default(),
                    )),
                    PostCompactionOutcome::Failure => Err(AgentError::InternalError(
                        "synthetic post-compaction turn failure".to_string(),
                    )),
                    PostCompactionOutcome::SuccessWithUsage => Ok(super::LlmStreamResult::new(
                        vec![AssistantBlock::Text {
                            text: "post-compaction success".to_string(),
                            meta: None,
                        }],
                        StopReason::EndTurn,
                        Usage {
                            input_tokens: 11,
                            output_tokens: 4,
                            cache_creation_tokens: Some(6),
                            cache_read_tokens: Some(9),
                        },
                    )),
                };
            }

            let usage = if ordinary_call == 0 {
                Usage {
                    input_tokens: 23,
                    output_tokens: 5,
                    ..Usage::default()
                }
            } else {
                Usage::default()
            };
            Ok(super::LlmStreamResult::new(
                vec![AssistantBlock::Text {
                    text: "ok".to_string(),
                    meta: None,
                }],
                StopReason::EndTurn,
                usage,
            ))
        }

        fn provider(&self) -> crate::provider::Provider {
            crate::provider::Provider::Other
        }

        fn model(&self) -> &'static str {
            "mock-model"
        }
    }

    struct CallbackPendingTools {
        tools: Arc<[Arc<ToolDef>]>,
    }

    impl CallbackPendingTools {
        fn new() -> Self {
            Self {
                tools: Arc::from([Arc::new(ToolDef {
                    name: "external_callback".into(),
                    description: "external callback fixture".to_string(),
                    input_schema: serde_json::json!({ "type": "object" }),
                    provenance: None,
                })]),
            }
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentToolDispatcher for CallbackPendingTools {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::clone(&self.tools)
        }

        async fn dispatch(
            &self,
            call: ToolCallView<'_>,
        ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
            Err(ToolError::CallbackPending {
                tool_name: call.name.to_string(),
                args: serde_json::json!({}),
            })
        }
    }

    #[tokio::test]
    async fn post_compaction_failure_rolls_back_rewrite_before_next_success() {
        let memory_store = Arc::new(AbortRetryMemoryStore::new());
        let client = Arc::new(CompactionTerminalLlmClient::new(
            PostCompactionOutcome::Failure,
        ));
        let session = crate::Session::new();
        let session_id = session.id().clone();
        let mut agent = with_test_turn_state_handle_for_session(AgentBuilder::new(), session)
            .budget(BudgetLimits::unlimited().with_max_tokens(1_000))
            .compactor(Arc::new(DiscardingCompactor::new(1)))
            .memory_store(memory_store.clone())
            .with_compaction_commit_coordinator(Arc::new(AllowCompactionCoordinator { session_id }))
            .build_standalone(client, Arc::new(NoTools), Arc::new(NoopStore))
            .await;

        agent.run("first".into()).await.unwrap();
        let usage_before = agent.session().total_usage();
        let budget_before = agent.budget.token_usage().unwrap().0;
        let last_input_tokens_before = agent.last_input_tokens;
        let cadence_before = agent.compaction_cadence.clone();
        let error = agent.run("second".into()).await.unwrap_err();

        assert!(error.to_string().contains("synthetic post-compaction"));
        assert!(agent.compaction_transaction.is_none());
        assert_eq!(memory_store.staged.lock().unwrap().len(), 0);
        assert_eq!(memory_store.aborted.lock().unwrap().len(), 1);
        assert!(
            agent
                .session()
                .messages()
                .iter()
                .any(|message| message.as_indexable_text().contains("first")),
            "rollback must restore discarded source history"
        );
        assert!(
            !agent.session().messages().iter().any(|message| {
                matches!(message, Message::User(user) if user.transcript_role.is_compaction_summary())
            }),
            "rollback must remove the uncommitted compaction summary"
        );
        assert!(
            agent
                .session()
                .compaction_projection_intents()
                .unwrap()
                .is_empty()
        );
        assert!(
            agent
                .session()
                .transcript_history_state()
                .unwrap()
                .is_none(),
            "rollback must remove the uncommitted transcript rewrite"
        );
        assert_eq!(
            agent.compaction_cadence.session_boundary_index,
            cadence_before.session_boundary_index.saturating_add(1)
        );
        assert_eq!(
            agent
                .compaction_cadence
                .last_compaction_attempt_boundary_index,
            Some(cadence_before.session_boundary_index)
        );
        assert_eq!(
            agent.compaction_cadence.last_compaction_boundary_index,
            cadence_before.last_compaction_boundary_index,
            "a rolled-back rewrite is an attempt, not a successful compaction"
        );
        let persisted_cadence: crate::compact::SessionCompactionCadence = serde_json::from_value(
            agent.session().metadata()[crate::compact::SESSION_COMPACTION_CADENCE_KEY].clone(),
        )
        .unwrap();
        assert_eq!(persisted_cadence, agent.compaction_cadence);
        assert_eq!(agent.last_input_tokens, last_input_tokens_before);
        assert_ne!(last_input_tokens_before, 0);
        let summary_usage = Usage {
            input_tokens: 7,
            output_tokens: 3,
            ..Usage::default()
        };
        let mut expected_usage = usage_before;
        expected_usage.add(&summary_usage);
        assert_eq!(agent.session().total_usage(), expected_usage);
        assert_eq!(
            agent.budget.token_usage(),
            Some((budget_before + summary_usage.total_tokens(), 1_000)),
            "provider work remains charged even when the rewrite rolls back"
        );

        let staged_before_retry = memory_store.staged.lock().unwrap().len();
        let aborted_before_retry = memory_store.aborted.lock().unwrap().len();
        agent.run("third".into()).await.unwrap();
        assert!(agent.compaction_transaction.is_none());
        assert_eq!(
            memory_store.staged.lock().unwrap().len(),
            staged_before_retry,
            "the retained attempt cadence must prevent immediate re-compaction"
        );
        assert_eq!(
            memory_store.aborted.lock().unwrap().len(),
            aborted_before_retry
        );
        assert!(
            agent
                .session()
                .compaction_projection_intents()
                .unwrap()
                .is_empty(),
            "the next successful turn must not persist an orphan rewrite intent"
        );
        assert!(
            agent
                .session()
                .transcript_history_state()
                .unwrap()
                .is_none(),
            "the next successful turn must not persist the rolled-back rewrite"
        );
    }

    #[tokio::test]
    async fn post_compaction_hook_failure_retains_full_monotonic_usage_delta() {
        use crate::hooks::{
            HookDecision, HookEngine, HookEngineError, HookExecutionReport, HookInvocation,
            HookPoint, HookReasonCode,
        };

        struct DenySecondRunCompletedHook {
            completed: Mutex<usize>,
        }

        #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
        #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
        impl HookEngine for DenySecondRunCompletedHook {
            async fn execute(
                &self,
                invocation: HookInvocation,
                _overrides: Option<&crate::config::HookRunOverrides>,
            ) -> Result<HookExecutionReport, HookEngineError> {
                if invocation.point != HookPoint::RunCompleted {
                    return Ok(HookExecutionReport::empty());
                }
                let mut completed = self.completed.lock().unwrap();
                let current = *completed;
                *completed = completed.saturating_add(1);
                if current == 0 {
                    return Ok(HookExecutionReport::empty());
                }
                Ok(HookExecutionReport {
                    decision: Some(HookDecision::Deny {
                        hook_id: crate::hooks::HookId::new("deny-post-compaction-completion"),
                        reason_code: HookReasonCode::PolicyViolation,
                        message: "deny post-compaction completion".to_string(),
                        payload: None,
                    }),
                    ..HookExecutionReport::empty()
                })
            }
        }

        let memory_store = Arc::new(AbortRetryMemoryStore::new());
        let client = Arc::new(CompactionTerminalLlmClient::new(
            PostCompactionOutcome::SuccessWithUsage,
        ));
        let session = crate::Session::new();
        let session_id = session.id().clone();
        let mut agent = with_test_turn_state_handle_for_session(AgentBuilder::new(), session)
            .budget(BudgetLimits::unlimited().with_max_tokens(2_000))
            .compactor(Arc::new(DiscardingCompactor::new(1)))
            .memory_store(memory_store.clone())
            .with_compaction_commit_coordinator(Arc::new(AllowCompactionCoordinator { session_id }))
            .with_hook_engine(Arc::new(DenySecondRunCompletedHook {
                completed: Mutex::new(0),
            }))
            .build_standalone(client, Arc::new(NoTools), Arc::new(NoopStore))
            .await;

        agent.run("first".into()).await.unwrap();
        let usage_before = agent.session().total_usage();
        let budget_before = agent.budget.token_usage().unwrap().0;
        let error = agent.run("second".into()).await.unwrap_err();
        assert!(matches!(
            error,
            AgentError::HookDenied {
                point: HookPoint::RunCompleted,
                ..
            }
        ));

        let mut expected = usage_before;
        expected.add(&Usage {
            input_tokens: 7,
            output_tokens: 3,
            ..Usage::default()
        });
        expected.add(&Usage {
            input_tokens: 11,
            output_tokens: 4,
            cache_creation_tokens: Some(6),
            cache_read_tokens: Some(9),
        });
        assert_eq!(agent.session().total_usage(), expected);
        assert_eq!(
            agent.budget.token_usage(),
            Some((budget_before + 25, 2_000))
        );
        assert!(agent.compaction_transaction.is_none());
        assert!(
            agent
                .session()
                .messages()
                .iter()
                .any(|message| message.as_indexable_text().contains("first"))
        );
        assert!(memory_store.staged.lock().unwrap().is_empty());
        assert_eq!(memory_store.aborted.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn compaction_projection_rejection_still_charges_summary_provider_usage() {
        let memory_store = Arc::new(RecordingMemoryStore::failing());
        let client = Arc::new(CompactionTerminalLlmClient::new(
            PostCompactionOutcome::SuccessWithUsage,
        ));
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .budget(BudgetLimits::unlimited().with_max_tokens(2_000))
            .compactor(Arc::new(DiscardingCompactor::new(1)))
            .memory_store(memory_store)
            .build_standalone(client, Arc::new(NoTools), Arc::new(NoopStore))
            .await;

        agent.run("first".into()).await.unwrap();
        let usage_before = agent.session().total_usage();
        let budget_before = agent.budget.token_usage().unwrap().0;
        agent.run("second".into()).await.unwrap();

        let mut expected = usage_before;
        expected.add(&Usage {
            input_tokens: 7,
            output_tokens: 3,
            ..Usage::default()
        });
        expected.add(&Usage {
            input_tokens: 11,
            output_tokens: 4,
            cache_creation_tokens: Some(6),
            cache_read_tokens: Some(9),
        });
        assert_eq!(agent.session().total_usage(), expected);
        assert_eq!(
            agent.budget.token_usage(),
            Some((budget_before + 25, 2_000)),
            "a rejected semantic-memory projection must not refund its successful summary call"
        );
        assert!(
            agent
                .session()
                .messages()
                .iter()
                .any(|message| message.as_indexable_text().contains("first")),
            "projection rejection must still preserve the source transcript"
        );
    }

    #[tokio::test]
    async fn callback_pending_preserves_compaction_pair_for_runtime_boundary_commit() {
        let memory_store = Arc::new(AbortRetryMemoryStore::new());
        let client = Arc::new(CompactionTerminalLlmClient::new(
            PostCompactionOutcome::CallbackPending,
        ));
        let session = crate::Session::new();
        let session_id = session.id().clone();
        let mut agent = with_test_turn_state_handle_for_session(AgentBuilder::new(), session)
            .compactor(Arc::new(DiscardingCompactor::new(1)))
            .memory_store(memory_store.clone())
            .with_compaction_commit_coordinator(Arc::new(AllowCompactionCoordinator { session_id }))
            .build_standalone(
                client,
                Arc::new(CallbackPendingTools::new()),
                Arc::new(NoopStore),
            )
            .await;

        agent.run("first".into()).await.unwrap();
        let error = agent.run("second".into()).await.unwrap_err();
        assert!(matches!(error, AgentError::CallbackPending { .. }));

        let transaction = agent
            .compaction_transaction
            .as_ref()
            .expect("callback-pending boundary must retain its compaction transaction");
        assert!(matches!(
            &transaction.phase,
            crate::agent::CompactionTransactionPhase::AwaitingRuntimeCommit(_)
        ));
        let intents = agent.session().compaction_projection_intents().unwrap();
        assert_eq!(intents.len(), 1);
        assert_eq!(transaction.projections, vec![intents[0].projection.clone()]);
        assert_eq!(
            memory_store.staged.lock().unwrap().as_slice(),
            transaction.projections.as_slice(),
            "callback-pending must keep the invisible stage paired with the session-carried outbox intent"
        );
        assert!(memory_store.aborted.lock().unwrap().is_empty());
        assert!(
            agent
                .session()
                .transcript_history_state()
                .unwrap()
                .is_some(),
            "callback-pending is a commit-worthy boundary and must retain the typed rewrite"
        );
    }

    fn append_abort_test_projection(
        session: &mut crate::Session,
        label: &str,
    ) -> crate::memory::CompactionProjectionId {
        session.push(Message::User(UserMessage::text(format!(
            "verbose context {label} one"
        ))));
        session.push(Message::User(UserMessage::text(format!(
            "verbose context {label} two"
        ))));
        let messages_before = session.messages().len();
        let replacement = vec![Message::User(UserMessage::compaction_summary(format!(
            "compacted {label}"
        )))];
        let authority = crate::agent::compact::ValidatedCompactionRewrite::for_test(
            session.messages(),
            &replacement,
        )
        .unwrap();
        let commit = session
            .replace_messages_for_compaction_internal(replacement, &authority)
            .unwrap()
            .unwrap();
        let projection = crate::memory::CompactionProjectionId::from_validated_transcript_rewrite(
            session.id().clone(),
            &commit,
            &authority,
        )
        .unwrap();
        session
            .add_compaction_projection_intent(crate::memory::CompactionProjectionIntent {
                projection: projection.clone(),
                summary_tokens: 1,
                messages_before,
                messages_after: 1,
            })
            .unwrap();
        projection
    }

    #[tokio::test]
    async fn empty_runtime_outbox_keeps_transaction_abortable_after_commit_failure() {
        let memory_store = Arc::new(AbortRetryMemoryStore::new());
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .memory_store(memory_store.clone())
            .build_standalone(
                Arc::new(StaticLlmClient),
                Arc::new(NoTools),
                Arc::new(NoopStore),
            )
            .await;
        let rollback_session = agent.session().clone();
        let rollback_messages = rollback_session.messages().to_vec();
        let rollback_history =
            serde_json::to_value(rollback_session.transcript_history_state().unwrap()).unwrap();
        let rollback_usage = rollback_session.total_usage();
        let rollback_last_input_tokens = agent.last_input_tokens;
        let rollback_compaction_cadence = agent.compaction_cadence.clone();
        let attempted_boundary = rollback_compaction_cadence.session_boundary_index + 1;
        let projection = append_abort_test_projection(agent.session_mut(), "atomic-apply-failure");
        let attempted_usage = Usage {
            input_tokens: 13,
            output_tokens: 7,
            cache_creation_tokens: Some(5),
            cache_read_tokens: Some(3),
        };
        agent.session_mut().record_usage(attempted_usage.clone());
        agent.last_input_tokens = rollback_last_input_tokens + 13;
        agent.compaction_cadence.session_boundary_index = attempted_boundary;
        agent
            .compaction_cadence
            .last_compaction_attempt_boundary_index = Some(attempted_boundary);
        agent.compaction_cadence.last_compaction_boundary_index = Some(attempted_boundary);
        memory_store.staged.lock().unwrap().push(projection.clone());
        agent.compaction_transaction = Some(crate::agent::CompactionTransaction {
            phase: crate::agent::CompactionTransactionPhase::AwaitingRuntimeCommit(Box::new(
                crate::agent::CompactionRollbackState {
                    rollback_session,
                    rollback_last_input_tokens,
                    rollback_compaction_cadence: rollback_compaction_cadence.clone(),
                },
            )),
            projections: vec![projection.clone()],
        });

        let error = agent
            .reconcile_runtime_compaction_projections(&[])
            .await
            .expect_err("an empty outbox cannot authorize a live staged projection");
        assert!(error.to_string().contains("does not exactly match"));
        assert!(matches!(
            agent
                .compaction_transaction
                .as_ref()
                .map(|transaction| &transaction.phase),
            Some(crate::agent::CompactionTransactionPhase::AwaitingRuntimeCommit(_))
        ));

        agent
            .abort_uncommitted_compaction_projections()
            .await
            .expect("the typed post-commit-failure path must roll back and abort");
        assert!(agent.compaction_transaction.is_none());
        assert_eq!(agent.session().messages(), rollback_messages);
        assert_eq!(
            serde_json::to_value(agent.session().transcript_history_state().unwrap()).unwrap(),
            rollback_history
        );
        let mut expected_usage = rollback_usage;
        expected_usage.add(&attempted_usage);
        assert_eq!(agent.session().total_usage(), expected_usage);
        assert_eq!(agent.last_input_tokens, rollback_last_input_tokens);
        assert_eq!(
            agent.compaction_cadence.session_boundary_index,
            attempted_boundary
        );
        assert_eq!(
            agent
                .compaction_cadence
                .last_compaction_attempt_boundary_index,
            Some(attempted_boundary)
        );
        assert_eq!(
            agent.compaction_cadence.last_compaction_boundary_index,
            rollback_compaction_cadence.last_compaction_boundary_index,
            "a rolled-back compaction must retain the attempt but not claim successful completion"
        );
        assert!(
            agent
                .session()
                .compaction_projection_intents()
                .unwrap()
                .is_empty()
        );
        assert!(memory_store.staged.lock().unwrap().is_empty());
        assert_eq!(
            memory_store.aborted.lock().unwrap().as_slice(),
            &[projection]
        );
        assert!(memory_store.finalized.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn abort_uncommitted_projections_drains_all_and_retains_only_failures_for_retry() {
        let memory_store = Arc::new(AbortRetryMemoryStore::new());
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .memory_store(memory_store.clone())
            .build_standalone(
                Arc::new(StaticLlmClient),
                Arc::new(NoTools),
                Arc::new(NoopStore),
            )
            .await;
        let rollback_session = agent.session().clone();
        let rollback_messages = rollback_session.messages().to_vec();
        let rollback_history = rollback_session.transcript_history_state().unwrap();
        let rollback_usage = rollback_session.total_usage();
        let first = append_abort_test_projection(agent.session_mut(), "first");
        let second = append_abort_test_projection(agent.session_mut(), "second");
        agent.compaction_transaction = Some(crate::agent::CompactionTransaction {
            phase: crate::agent::CompactionTransactionPhase::AwaitingRuntimeCommit(Box::new(
                crate::agent::CompactionRollbackState {
                    rollback_session,
                    rollback_last_input_tokens: agent.last_input_tokens,
                    rollback_compaction_cadence: agent.compaction_cadence.clone(),
                },
            )),
            projections: vec![first.clone(), second.clone()],
        });
        *memory_store.fail_once.lock().unwrap() = Some(first.clone());

        let error = agent
            .abort_uncommitted_compaction_projections()
            .await
            .unwrap_err();
        assert!(error.to_string().contains("injected abort failure"));
        assert_eq!(
            agent
                .compaction_transaction
                .as_ref()
                .map(|transaction| transaction.projections.as_slice()),
            Some(std::slice::from_ref(&first))
        );
        assert!(matches!(
            agent
                .compaction_transaction
                .as_ref()
                .map(|transaction| &transaction.phase),
            Some(crate::agent::CompactionTransactionPhase::AbortPending { .. })
        ));
        assert_eq!(memory_store.aborted.lock().unwrap().as_slice(), &[second]);
        assert_eq!(agent.session().messages(), rollback_messages.as_slice());
        assert_eq!(
            serde_json::to_value(agent.session().transcript_history_state().unwrap()).unwrap(),
            serde_json::to_value(rollback_history).unwrap()
        );
        assert_eq!(agent.session().total_usage(), rollback_usage);
        assert!(
            agent
                .session()
                .compaction_projection_intents()
                .unwrap()
                .is_empty(),
            "intent removal must precede abort so a missing stage cannot be outboxed"
        );

        agent
            .run("ingress retries abort cleanup before LLM".into())
            .await
            .unwrap();
        assert!(agent.compaction_transaction.is_none());
        assert!(memory_store.aborted.lock().unwrap().contains(&first));
    }

    #[tokio::test]
    async fn abort_marker_only_cleans_exact_stage_without_empty_reconciliation() {
        let memory_store = Arc::new(AbortRetryMemoryStore::new());
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .memory_store(memory_store.clone())
            .build_standalone(
                Arc::new(StaticLlmClient),
                Arc::new(NoTools),
                Arc::new(NoopStore),
            )
            .await;
        let mut staged_session = agent.session().clone();
        let projection = append_abort_test_projection(&mut staged_session, "in-flight");
        memory_store.staged.lock().unwrap().push(projection.clone());
        agent.in_flight_compaction_stage = Some(projection.clone());
        let reconciliations_before = memory_store.reconciled_committed.lock().unwrap().len();

        agent
            .abort_uncommitted_compaction_projections()
            .await
            .expect("marker-only cleanup should abort its exact deterministic identity");

        assert!(agent.in_flight_compaction_stage.is_none());
        assert_eq!(
            memory_store.aborted.lock().unwrap().as_slice(),
            &[projection]
        );
        assert_eq!(
            memory_store.reconciled_committed.lock().unwrap().len(),
            reconciliations_before,
            "marker cleanup must use its exact projection identity, not invent a new empty-authority reconciliation"
        );
    }

    #[tokio::test]
    async fn runtime_committed_finalize_failure_refuses_abort_and_retries_atomically() {
        let memory_store = Arc::new(AbortRetryMemoryStore::new());
        let session = crate::Session::new();
        let session_id = session.id().clone();
        let mut agent = with_test_turn_state_handle_for_session(AgentBuilder::new(), session)
            .memory_store(memory_store.clone())
            .with_compaction_commit_coordinator(Arc::new(AllowCompactionCoordinator { session_id }))
            .build_standalone(
                Arc::new(StaticLlmClient),
                Arc::new(NoTools),
                Arc::new(NoopStore),
            )
            .await;
        let rollback = crate::agent::CompactionRollbackState {
            rollback_session: agent.session().clone(),
            rollback_last_input_tokens: agent.last_input_tokens,
            rollback_compaction_cadence: agent.compaction_cadence.clone(),
        };
        let first = append_abort_test_projection(agent.session_mut(), "commit-first");
        let second = append_abort_test_projection(agent.session_mut(), "commit-second");
        let intents = agent
            .session()
            .validated_compaction_projection_intents()
            .unwrap();
        memory_store
            .staged
            .lock()
            .unwrap()
            .extend([first.clone(), second.clone()]);
        *memory_store.finalize_fail_once.lock().unwrap() = Some(second.clone());
        agent.compaction_transaction = Some(crate::agent::CompactionTransaction {
            phase: crate::agent::CompactionTransactionPhase::AwaitingRuntimeCommit(Box::new(
                rollback,
            )),
            projections: vec![first.clone(), second.clone()],
        });

        let error = agent
            .reconcile_runtime_compaction_projections(&intents)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("injected finalize failure"));
        assert!(matches!(
            agent
                .compaction_transaction
                .as_ref()
                .map(|transaction| &transaction.phase),
            Some(crate::agent::CompactionTransactionPhase::RuntimeCommitted {
                bookkeeping_complete: false
            })
        ));
        assert_eq!(
            agent
                .session()
                .validated_compaction_projection_intents()
                .unwrap(),
            intents,
            "a partial store finalize must not remove a prefix of local intent authority"
        );
        let abort_error = agent
            .abort_uncommitted_compaction_projections()
            .await
            .unwrap_err();
        assert!(abort_error.to_string().contains("commit-only"));

        agent
            .reconcile_runtime_compaction_projections(&intents)
            .await
            .unwrap();
        assert!(agent.compaction_transaction.is_none());
        assert!(
            agent
                .session()
                .validated_compaction_projection_intents()
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            memory_store.finalized.lock().unwrap().as_slice(),
            &[first, second]
        );
    }

    #[tokio::test]
    async fn runtime_outbox_exact_authority_rejects_subset_superset_and_duplicates() {
        let memory_store = Arc::new(AbortRetryMemoryStore::new());
        let session = crate::Session::new();
        let session_id = session.id().clone();
        let mut agent = with_test_turn_state_handle_for_session(AgentBuilder::new(), session)
            .memory_store(memory_store.clone())
            .with_compaction_commit_coordinator(Arc::new(AllowCompactionCoordinator { session_id }))
            .build_standalone(
                Arc::new(StaticLlmClient),
                Arc::new(NoTools),
                Arc::new(NoopStore),
            )
            .await;
        let rollback = crate::agent::CompactionRollbackState {
            rollback_session: agent.session().clone(),
            rollback_last_input_tokens: agent.last_input_tokens,
            rollback_compaction_cadence: agent.compaction_cadence.clone(),
        };
        let first = append_abort_test_projection(agent.session_mut(), "exact-first");
        let second = append_abort_test_projection(agent.session_mut(), "exact-second");
        let third = append_abort_test_projection(agent.session_mut(), "exact-extra");
        let intents = agent
            .session()
            .validated_compaction_projection_intents()
            .unwrap();
        agent.compaction_transaction = Some(crate::agent::CompactionTransaction {
            phase: crate::agent::CompactionTransactionPhase::AwaitingRuntimeCommit(Box::new(
                rollback,
            )),
            projections: vec![first, second],
        });
        let reconciliations_before = memory_store.reconciled_committed.lock().unwrap().len();

        let subset = vec![intents[0].clone()];
        assert!(
            agent
                .reconcile_runtime_compaction_projections(&subset)
                .await
                .unwrap_err()
                .to_string()
                .contains("does not exactly match")
        );
        let duplicate = vec![intents[0].clone(), intents[0].clone()];
        assert!(
            agent
                .reconcile_runtime_compaction_projections(&duplicate)
                .await
                .unwrap_err()
                .to_string()
                .contains("duplicate")
        );
        let superset = intents;
        assert_eq!(superset[2].projection, third);
        assert!(
            agent
                .reconcile_runtime_compaction_projections(&superset)
                .await
                .unwrap_err()
                .to_string()
                .contains("does not exactly match")
        );
        assert_eq!(
            memory_store.reconciled_committed.lock().unwrap().len(),
            reconciliations_before,
            "invalid authority sets must fail before touching durable stages"
        );
        assert!(matches!(
            agent
                .compaction_transaction
                .as_ref()
                .map(|transaction| &transaction.phase),
            Some(crate::agent::CompactionTransactionPhase::AwaitingRuntimeCommit(_))
        ));
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

    struct BoundaryContextRecordingClient {
        call_count: Mutex<u32>,
        seen_system_messages: Mutex<Vec<String>>,
    }

    impl BoundaryContextRecordingClient {
        fn new() -> Self {
            Self {
                call_count: Mutex::new(0),
                seen_system_messages: Mutex::new(Vec::new()),
            }
        }

        fn seen_system_messages(&self) -> Vec<String> {
            self.seen_system_messages.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl AgentLlmClient for BoundaryContextRecordingClient {
        async fn stream_response(
            &self,
            messages: &[Message],
            _tools: &[Arc<ToolDef>],
            _max_tokens: u32,
            _temperature: Option<f32>,
            _provider_params: Option<&crate::lifecycle::run_primitive::ProviderParamsOverride>,
        ) -> Result<super::LlmStreamResult, AgentError> {
            self.seen_system_messages.lock().unwrap().push(
                messages
                    .iter()
                    .filter_map(|message| match message {
                        Message::System(system) => Some(system.content.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            );

            let mut calls = self.call_count.lock().unwrap();
            let response = if *calls == 0 {
                super::LlmStreamResult::new(
                    vec![AssistantBlock::ToolUse {
                        id: "call-slow".to_string(),
                        name: "slow_tool".into(),
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

        fn provider(&self) -> crate::provider::Provider {
            crate::provider::Provider::Other
        }

        fn model(&self) -> &'static str {
            "mock-model"
        }
    }

    struct BlockingToolDispatcher {
        tools: Arc<[Arc<ToolDef>]>,
        started: Arc<Notify>,
        release: Arc<Notify>,
    }

    #[async_trait]
    impl AgentToolDispatcher for BlockingToolDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::clone(&self.tools)
        }

        async fn dispatch(
            &self,
            call: ToolCallView<'_>,
        ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
            self.started.notify_waiters();
            self.release.notified().await;
            Ok(ToolResult::new(call.id.to_string(), "released".to_string(), false).into())
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
                        crate::ToolProvenance {
                            kind: crate::ToolSourceKind::Callback,
                            source_id: "test".into(),
                        },
                    ),
                    crate::ToolCatalogEntry::session_deferred(
                        deferred_two,
                        true,
                        crate::ToolProvenance {
                            kind: crate::ToolSourceKind::Callback,
                            source_id: "test".into(),
                        },
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
                        crate::ToolProvenance {
                            kind: crate::ToolSourceKind::Callback,
                            source_id: "direct-builder".into(),
                        },
                    ),
                    crate::ToolCatalogEntry::session_deferred(
                        deferred_two,
                        true,
                        crate::ToolProvenance {
                            kind: crate::ToolSourceKind::Callback,
                            source_id: "direct-builder".into(),
                        },
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

        fn provider(&self) -> crate::provider::Provider {
            crate::provider::Provider::Other
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

        fn provider(&self) -> crate::provider::Provider {
            crate::provider::Provider::Other
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

        fn provider(&self) -> crate::provider::Provider {
            crate::provider::Provider::Other
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

        fn provider(&self) -> crate::provider::Provider {
            crate::provider::Provider::Other
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
                            objective_id: None,
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
                            sender_taint: None,
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
    async fn active_turn_system_context_is_visible_after_tool_boundary() {
        let client = Arc::new(BoundaryContextRecordingClient::new());
        let tool_started = Arc::new(Notify::new());
        let release_tool = Arc::new(Notify::new());
        let tools = Arc::new(BlockingToolDispatcher {
            tools: Arc::from([Arc::new(ToolDef::new(
                "slow_tool",
                "blocks until the test releases it",
                serde_json::json!({ "type": "object" }),
            ))]),
            started: Arc::clone(&tool_started),
            release: Arc::clone(&release_tool),
        });
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .system_prompt("base system prompt")
            .build_standalone(client.clone(), tools, Arc::new(NoopStore))
            .await;
        let state = agent.system_context_state();

        let (tx, mut rx) = mpsc::channel(32);
        let run = tokio::spawn(async move {
            agent
                .run_with_events("start with a tool".to_string().into(), tx)
                .await
        });

        tokio::time::timeout(std::time::Duration::from_secs(1), tool_started.notified())
            .await
            .expect("tool should start");

        state
            .stage_active_turn_append(
                &crate::service::AppendSystemContextRequest {
                    content: crate::lifecycle::run_primitive::CoreRenderable::text(
                        "STEER_MARKER_visible_after_tool".to_string(),
                    ),
                    source: Some("test:active-steer".to_string()),
                    idempotency_key: Some("test:active-steer".to_string()),
                    source_kind: crate::session::SystemContextSource::RuntimeSteer,
                    peer_response_terminal: None,
                },
                crate::time_compat::SystemTime::now(),
            )
            .expect("active-turn append should stage");

        release_tool.notify_waiters();
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), run)
            .await
            .expect("run should complete")
            .expect("run task should join")
            .expect("agent run should succeed");
        assert_eq!(result.turns, 2);

        while rx.try_recv().is_ok() {}
        let seen = client.seen_system_messages();
        assert!(
            seen.len() >= 2,
            "client should have seen both pre-tool and post-tool model requests: {seen:?}"
        );
        assert!(
            seen.iter()
                .skip(1)
                .any(|system| system.contains("STEER_MARKER_visible_after_tool")),
            "active-turn staged context must be present on the model request after the tool boundary; seen={seen:?}"
        );
    }

    fn incoming_peer_comms_renderable(
        sender_taint: Option<crate::comms::SenderContentTaint>,
    ) -> crate::lifecycle::run_primitive::CoreRenderable {
        crate::lifecycle::run_primitive::CoreRenderable::SystemNotice {
            kind: SystemNoticeKind::Comms,
            body: Some("Peer message from mob/roles/worker".to_string()),
            blocks: vec![crate::types::SystemNoticeBlock::Comms {
                kind: crate::types::CommsNoticeKind::Message,
                direction: crate::types::SystemNoticeDirection::Incoming,
                peer: Some(crate::types::SystemNoticePeer {
                    id: crate::comms::PeerId::new(),
                    display_name: Some("mob/roles/worker".to_string()),
                }),
                sender_taint,
                request_id: None,
                intent: None,
                status: None,
                summary: None,
                payload: None,
                content: Vec::new(),
            }],
        }
    }

    /// Queued peer deliveries (runtime typed appends) emit the typed
    /// PeerContentIngested observe fact — with the sender's declaration and
    /// BEFORE RunStarted — so a host taint tracker never parses projection
    /// text to classify inbound peer content.
    #[tokio::test]
    async fn queued_peer_append_emits_peer_content_ingested_before_run_started() {
        let mut agent = build_agent(Arc::new(StaticLlmClient)).await;
        let (tx, mut rx) = mpsc::channel::<crate::event::AgentEvent>(128);
        let appends = vec![
            crate::lifecycle::run_primitive::ConversationAppend {
                role: crate::lifecycle::run_primitive::ConversationAppendRole::SystemNotice,
                content: incoming_peer_comms_renderable(Some(
                    crate::comms::SenderContentTaint::Tainted,
                )),
            },
            crate::lifecycle::run_primitive::ConversationAppend {
                role: crate::lifecycle::run_primitive::ConversationAppendRole::User,
                content: crate::lifecycle::run_primitive::CoreRenderable::text(
                    "reply to the peer".to_string(),
                ),
            },
        ];
        agent
            .run_with_events_and_typed_turn_appends(
                "reply to the peer".into(),
                appends,
                Vec::new(),
                None,
                tx,
            )
            .await
            .expect("run should succeed");

        let mut ordered = Vec::new();
        while let Ok(event) = rx.try_recv() {
            ordered.push(event);
        }
        let ingested_at = ordered.iter().position(|event| {
            matches!(
                event,
                crate::event::AgentEvent::PeerContentIngested {
                    sender_taint: Some(crate::comms::SenderContentTaint::Tainted),
                    ..
                }
            )
        });
        let run_started_at = ordered
            .iter()
            .position(|event| matches!(event, crate::event::AgentEvent::RunStarted { .. }));
        let ingested_at = ingested_at.expect("queued peer append must emit PeerContentIngested");
        let run_started_at = run_started_at.expect("run must emit RunStarted");
        assert!(
            ingested_at < run_started_at,
            "the ingestion fact arrives with the delivery, before RunStarted"
        );
    }

    /// Steer-delivered peer content (runtime system context applied at the
    /// model boundary) emits the same typed observe fact through the same
    /// projection owner.
    #[tokio::test]
    async fn steer_peer_context_emits_peer_content_ingested_at_boundary() {
        let client = Arc::new(BoundaryContextRecordingClient::new());
        let tool_started = Arc::new(Notify::new());
        let release_tool = Arc::new(Notify::new());
        let tools = Arc::new(BlockingToolDispatcher {
            tools: Arc::from([Arc::new(ToolDef::new(
                "slow_tool",
                "blocks until the test releases it",
                serde_json::json!({ "type": "object" }),
            ))]),
            started: Arc::clone(&tool_started),
            release: Arc::clone(&release_tool),
        });
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .system_prompt("base system prompt")
            .build_standalone(client.clone(), tools, Arc::new(NoopStore))
            .await;
        let state = agent.system_context_state();

        let (tx, mut rx) = mpsc::channel(128);
        let run = tokio::spawn(async move {
            agent
                .run_with_events("start with a tool".to_string().into(), tx)
                .await
        });

        tokio::time::timeout(std::time::Duration::from_secs(1), tool_started.notified())
            .await
            .expect("tool should start");

        state
            .stage_active_turn_append(
                &crate::service::AppendSystemContextRequest {
                    content: incoming_peer_comms_renderable(Some(
                        crate::comms::SenderContentTaint::Tainted,
                    )),
                    source: Some("test:peer-steer".to_string()),
                    idempotency_key: Some("test:peer-steer".to_string()),
                    source_kind: crate::session::SystemContextSource::RuntimeSteer,
                    peer_response_terminal: None,
                },
                crate::time_compat::SystemTime::now(),
            )
            .expect("active-turn append should stage");

        release_tool.notify_waiters();
        tokio::time::timeout(std::time::Duration::from_secs(2), run)
            .await
            .expect("run should complete")
            .expect("run task should join")
            .expect("agent run should succeed");

        let mut saw_ingested = false;
        while let Ok(event) = rx.try_recv() {
            if matches!(
                event,
                crate::event::AgentEvent::PeerContentIngested {
                    sender_taint: Some(crate::comms::SenderContentTaint::Tainted),
                    ..
                }
            ) {
                saw_ingested = true;
            }
        }
        assert!(
            saw_ingested,
            "steer-applied peer context must emit PeerContentIngested at the boundary"
        );
    }

    /// The core trait default fails typed — a runtime that cannot carry the
    /// declaration must never silently drop it.
    #[test]
    fn comms_runtime_taint_setter_defaults_to_typed_unsupported() {
        struct InboxOnlyRuntime;
        #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
        #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
        impl crate::agent::CommsRuntime for InboxOnlyRuntime {
            async fn drain_messages(&self) -> Vec<String> {
                Vec::new()
            }
            fn inbox_notify(&self) -> Arc<Notify> {
                Arc::new(Notify::new())
            }
        }
        let runtime = InboxOnlyRuntime;
        let result = crate::agent::CommsRuntime::set_outbound_content_taint(
            &runtime,
            Some(crate::comms::SenderContentTaint::Tainted),
        );
        assert!(
            matches!(result, Err(crate::comms::SendError::Unsupported(_))),
            "default must fail typed, not silently no-op: {result:?}"
        );
    }

    fn start_test_conversation_turn(handle: &dyn crate::TurnStateHandle) -> RunId {
        let run_id = RunId::new();
        handle
            .start_conversation_run(
                run_id.clone(),
                TurnPrimitiveKind::ConversationTurn,
                ContentShape::Conversation,
                false,
                false,
                0,
            )
            .unwrap();
        handle.primitive_applied(run_id.clone()).unwrap();
        run_id
    }

    fn assert_generated_visibility_authority_required(message: impl std::fmt::Display) {
        let message = message.to_string();
        assert!(
            message.contains("generated MeerkatMachine visibility authority is required")
                || message
                    .contains("generated MeerkatMachine tool visibility authority is required")
                || message.contains("local tool visibility owner is read-only"),
            "expected generated visibility authority failure, got: {message}"
        );
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
    async fn failed_compaction_attempt_uses_cadence_guard_before_retry() {
        let client = Arc::new(FailingCompactionLlmClient::new());
        let compactor = Arc::new(CadenceAwareFailingCompactor::new());
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .compactor(compactor.clone())
            .build_standalone(client.clone(), Arc::new(NoTools), Arc::new(NoopStore))
            .await;

        agent.run("first".into()).await.unwrap();

        let (tx, mut rx) = mpsc::channel::<crate::event::AgentEvent>(128);
        agent
            .run_with_events("second".into(), tx)
            .await
            .expect("compaction failure should preserve history and continue the turn");

        let mut saw_compaction_failure = false;
        while let Ok(event) = rx.try_recv() {
            if let crate::event::AgentEvent::CompactionFailed { reason } = event {
                match reason {
                    crate::event::CompactionFailureReason::LlmFailed { message, .. } => {
                        assert!(
                            message.contains("stream ended before done"),
                            "unexpected compaction failure message: {message}"
                        );
                    }
                    other => unreachable!("unexpected compaction failure reason: {other:?}"),
                }
                saw_compaction_failure = true;
            }
        }
        assert!(
            saw_compaction_failure,
            "failed compaction attempt should be surfaced"
        );

        agent.run("third".into()).await.unwrap();

        let contexts = compactor.seen_contexts();
        assert_eq!(
            contexts
                .iter()
                .map(|ctx| (
                    ctx.session_boundary_index,
                    ctx.last_compaction_boundary_index
                ))
                .collect::<Vec<_>>(),
            vec![(0, None), (1, None), (2, Some(1))],
            "failed attempt boundary should feed the cadence guard on the next run"
        );
        assert_eq!(
            client.seen_last_user_messages(),
            vec![
                "first".to_string(),
                "COMPACT NOW".to_string(),
                "second".to_string(),
                "third".to_string(),
            ],
            "the next boundary after a failed compaction should not immediately retry compaction"
        );

        let cadence: crate::compact::SessionCompactionCadence = serde_json::from_value(
            agent
                .session()
                .metadata()
                .get(crate::compact::SESSION_COMPACTION_CADENCE_KEY)
                .expect("cadence metadata must be persisted by the run loop")
                .clone(),
        )
        .unwrap();
        assert_eq!(cadence.session_boundary_index, 3);
        assert_eq!(cadence.last_compaction_boundary_index, None);
        assert_eq!(cadence.last_compaction_attempt_boundary_index, Some(1));
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

    /// Row 319: the compaction-discard producer must carry the typed
    /// `MemoryIndexableContent` (the include/exclude policy) to the store,
    /// rather than flattening to a `String` and silently skipping excluded
    /// messages via the empty-string convention. A discarded tool-result
    /// message must reach the store as a typed `Excluded(ToolResults)` request,
    /// so the store — not the producer — owns indexability.
    #[tokio::test]
    async fn compaction_discard_producer_carries_typed_indexability_to_store() {
        let client = Arc::new(StaticLlmClient);
        let memory_store = Arc::new(RecordingMemoryStore::new());
        let agent = with_test_turn_state_handle(AgentBuilder::new())
            .memory_store(memory_store.clone())
            .build_standalone(client, Arc::new(NoTools), Arc::new(NoopStore))
            .await;

        let discarded = vec![
            CompactionDiscard::new(4, Message::User(UserMessage::text("indexable user turn"))),
            CompactionDiscard::new(
                9,
                Message::tool_results(vec![ToolResult::new(
                    "call-1".to_string(),
                    "tool output".to_string(),
                    false,
                )]),
            ),
        ];

        let delivery = agent.index_compaction_discards(&discarded).await;
        assert!(
            matches!(delivery, crate::memory::MemoryIndexDelivery::Delivered(_)),
            "typed indexability delivery should succeed: {delivery:?}"
        );

        let typed = memory_store.typed_contents();
        assert_eq!(
            typed.len(),
            2,
            "both discarded messages must reach the store as typed requests, not be skipped by the producer"
        );
        assert!(
            typed.iter().any(|content| matches!(
                content,
                crate::types::MemoryIndexableContent::Indexable(text) if text == "indexable user turn"
            )),
            "the user turn must reach the store as typed Indexable content: {typed:?}"
        );
        assert!(
            typed.iter().any(|content| matches!(
                content,
                crate::types::MemoryIndexableContent::Excluded(
                    crate::types::MemoryIndexExclusion::ToolResults
                )
            )),
            "the tool-result message must reach the store with its typed ToolResults exclusion, not be silently dropped by the producer: {typed:?}"
        );
        let source_ranges = memory_store.source_ranges();
        assert_eq!(source_ranges.len(), 2);
        assert_eq!(source_ranges[0].start(), 4);
        assert_eq!(source_ranges[1].start(), 9);
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
                crate::event::AgentEvent::CompactionFailed {
                    reason: crate::event::CompactionFailureReason::MemoryIndexingFailed { .. },
                } => {
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
        let cadence: crate::compact::SessionCompactionCadence = serde_json::from_value(
            agent
                .session()
                .metadata()
                .get(crate::compact::SESSION_COMPACTION_CADENCE_KEY)
                .expect("failed compaction cadence must be persisted")
                .clone(),
        )
        .unwrap();
        assert_eq!(cadence.session_boundary_index, 2);
        assert_eq!(cadence.last_compaction_boundary_index, None);
        assert_eq!(cadence.last_compaction_attempt_boundary_index, Some(1));
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
    async fn invalid_compaction_rewrite_is_rejected_before_memory_projection() {
        let client = Arc::new(CompactionAwareLlmClient::new());
        let memory_store = Arc::new(RecordingMemoryStore::new());
        let mut session = crate::Session::new();
        session.push(Message::BlockAssistant(
            crate::types::BlockAssistantMessage::new(
                vec![AssistantBlock::ToolUse {
                    id: "orphan-call".to_string(),
                    name: "fixture_tool".into(),
                    args: serde_json::value::RawValue::from_string("{}".to_string()).unwrap(),
                    meta: None,
                }],
                StopReason::ToolUse,
            ),
        ));
        session.push(Message::tool_results(vec![ToolResult::new(
            "orphan-call".to_string(),
            "fixture result".to_string(),
            false,
        )]));
        let mut agent = with_test_turn_state_handle_for_session(AgentBuilder::new(), session)
            .compactor(Arc::new(InvalidRewriteCompactor))
            .memory_store(memory_store.clone())
            .build_standalone(client, Arc::new(NoTools), Arc::new(NoopStore))
            .await;

        let (tx, mut rx) = mpsc::channel::<crate::event::AgentEvent>(128);
        agent
            .run_with_events("first".into(), tx)
            .await
            .expect("invalid compaction rewrite should preserve history and continue the turn");

        assert!(
            memory_store.contents().is_empty(),
            "rewrite validation must happen before any semantic-memory projection commits"
        );
        assert!(
            agent
                .session()
                .messages()
                .iter()
                .any(|message| message.as_indexable_text().contains("first")),
            "invalid compaction must preserve the authoritative transcript"
        );
        let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
        let rewrite_error = events.iter().find_map(|event| match event {
            crate::event::AgentEvent::CompactionFailed {
                reason: crate::event::CompactionFailureReason::TranscriptRewriteFailed { message },
            } => Some(message),
            _ => None,
        });
        assert!(
            rewrite_error
                .as_ref()
                .is_some_and(|message| message.contains(
                    "tool_results at message 1 follows user, not an assistant tool-use message"
                )),
            "the provenance-valid rebuild must reach transcript-shape validation, got: {events:?}"
        );

        let cadence: crate::compact::SessionCompactionCadence = serde_json::from_value(
            agent
                .session()
                .metadata()
                .get(crate::compact::SESSION_COMPACTION_CADENCE_KEY)
                .expect("invalid rewrite attempt cadence must be persisted")
                .clone(),
        )
        .unwrap();
        assert_eq!(cadence.session_boundary_index, 2);
        assert_eq!(cadence.last_compaction_boundary_index, None);
        assert_eq!(cadence.last_compaction_attempt_boundary_index, Some(1));

        // Recreate the agent from the durable session value. The next compactor
        // would retry at every non-zero boundary unless the restored failed-
        // rewrite attempt participates in the cadence guard.
        let resumed_client = Arc::new(FailingCompactionLlmClient::new());
        let resumed_compactor = Arc::new(CadenceAwareFailingCompactor::new());
        let mut resumed_agent =
            with_test_turn_state_handle_for_session(AgentBuilder::new(), agent.session().clone())
                .compactor(resumed_compactor.clone())
                .build_standalone(
                    resumed_client.clone(),
                    Arc::new(NoTools),
                    Arc::new(NoopStore),
                )
                .await;
        resumed_agent
            .run("third after invalid-rewrite restart".into())
            .await
            .expect("restored failed-attempt cadence should guard the next turn");
        assert_eq!(
            resumed_compactor
                .seen_contexts()
                .iter()
                .map(|ctx| (
                    ctx.session_boundary_index,
                    ctx.last_compaction_boundary_index
                ))
                .collect::<Vec<_>>(),
            vec![(2, Some(1))]
        );
        assert_eq!(
            resumed_client.seen_last_user_messages(),
            vec!["third after invalid-rewrite restart".to_string()],
            "restored failed-attempt cadence must suppress an immediate compaction retry"
        );
    }

    #[tokio::test]
    async fn hostile_compaction_summary_is_rejected_before_memory_projection() {
        let client = Arc::new(CompactionAwareLlmClient::new());
        let memory_store = Arc::new(RecordingMemoryStore::new());
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .compactor(Arc::new(NoOpClaimingDiscardCompactor))
            .memory_store(memory_store.clone())
            .build_standalone(client, Arc::new(NoTools), Arc::new(NoopStore))
            .await;

        agent.run("first".into()).await.unwrap();
        let messages_before = agent.session().messages().to_vec();
        let (tx, mut rx) = mpsc::channel::<crate::event::AgentEvent>(128);
        agent
            .run_with_events("second".into(), tx)
            .await
            .expect("hostile compaction rebuild should preserve history and continue the turn");

        assert!(
            memory_store.contents().is_empty(),
            "a hostile summary mapping must be rejected before memory projection"
        );
        assert!(
            agent.session().messages().starts_with(&messages_before),
            "invalid compaction must preserve the authoritative pre-turn transcript"
        );
        assert!(
            std::iter::from_fn(|| rx.try_recv().ok()).any(|event| matches!(
                event,
                crate::event::AgentEvent::CompactionFailed {
                    reason: crate::event::CompactionFailureReason::TranscriptRewriteFailed {
                        message
                    }
                } if message.contains("summary mapping")
            )),
            "hostile summary mapping must surface the typed invalid-rebuild failure"
        );
    }

    #[tokio::test]
    async fn compaction_curator_substitutes_summary_without_llm_call() {
        // The failing client errors on any summarization call ("COMPACT NOW"),
        // so a completed compaction proves the curator substituted summary
        // production and the LLM path was never entered.
        let client = Arc::new(FailingCompactionLlmClient::new());
        let compactor = Arc::new(DiscardingCompactor::new(1));
        let curator = Arc::new(SubstitutingCurator::new());
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .compactor(compactor)
            .compaction_curator(curator.clone())
            .build_standalone(client.clone(), Arc::new(NoTools), Arc::new(NoopStore))
            .await;

        agent.run("first".into()).await.unwrap();
        let (tx, mut rx) = mpsc::channel::<crate::event::AgentEvent>(128);
        agent
            .run_with_events("second".into(), tx)
            .await
            .expect("curated compaction should commit and continue the turn");

        let mut saw_completed = false;
        while let Ok(event) = rx.try_recv() {
            if let crate::event::AgentEvent::CompactionCompleted { summary_tokens, .. } = event {
                assert_eq!(
                    summary_tokens, 0,
                    "curated summaries consume no LLM tokens; summary usage must be zero"
                );
                saw_completed = true;
            }
        }
        assert!(
            saw_completed,
            "curator-produced compaction should complete via the normal commit path"
        );
        assert_eq!(
            client.seen_last_user_messages(),
            vec!["first".to_string(), "second".to_string()],
            "the summarization LLM call must be skipped entirely when a curator is configured"
        );
        assert_eq!(
            curator.seen_windows().len(),
            1,
            "the curator should be invoked exactly once per compaction"
        );
        assert!(
            agent.session().messages().iter().any(|message| matches!(
                message,
                Message::User(user)
                    if user.transcript_role.is_compaction_summary()
                        && user.text_content().contains("curated summary")
            )),
            "the committed history must carry the curated summary as a typed compaction-summary message"
        );
    }

    #[tokio::test]
    async fn compaction_curator_failure_emits_typed_event_and_preserves_history() {
        let client = Arc::new(FailingCompactionLlmClient::new());
        let compactor = Arc::new(DiscardingCompactor::new(1));
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .compactor(compactor)
            .compaction_curator(Arc::new(FailingCurator))
            .build_standalone(client.clone(), Arc::new(NoTools), Arc::new(NoopStore))
            .await;

        agent.run("first".into()).await.unwrap();
        let (tx, mut rx) = mpsc::channel::<crate::event::AgentEvent>(128);
        agent
            .run_with_events("second".into(), tx)
            .await
            .expect("curator failure should preserve history and continue the turn");

        let mut saw_curator_failure = false;
        let mut saw_completed = false;
        while let Ok(event) = rx.try_recv() {
            match event {
                crate::event::AgentEvent::CompactionFailed {
                    reason: crate::event::CompactionFailureReason::CuratorFailed { message },
                } => {
                    assert!(
                        message.contains("curator offline"),
                        "unexpected curator failure message: {message}"
                    );
                    saw_curator_failure = true;
                }
                crate::event::AgentEvent::CompactionCompleted { .. } => {
                    saw_completed = true;
                }
                _ => {}
            }
        }
        assert!(
            saw_curator_failure,
            "curator failure should surface as a typed CuratorFailed compaction failure"
        );
        assert!(
            !saw_completed,
            "compaction must not complete when the curator fails"
        );
        assert!(
            agent
                .session()
                .messages()
                .iter()
                .any(|message| message.as_indexable_text().contains("first")),
            "a failed curator must leave the original history untouched"
        );
        assert_eq!(
            client.seen_last_user_messages(),
            vec!["first".to_string(), "second".to_string()],
            "a failing curator must never fall back to the summarization LLM call"
        );
    }

    #[tokio::test]
    async fn compaction_curator_empty_summary_surfaces_typed_error() {
        let client = Arc::new(FailingCompactionLlmClient::new());
        let compactor = Arc::new(DiscardingCompactor::new(1));
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .compactor(compactor)
            .compaction_curator(Arc::new(EmptySummaryCurator))
            .build_standalone(client.clone(), Arc::new(NoTools), Arc::new(NoopStore))
            .await;

        agent.run("first".into()).await.unwrap();
        let (tx, mut rx) = mpsc::channel::<crate::event::AgentEvent>(128);
        agent
            .run_with_events("second".into(), tx)
            .await
            .expect("empty curated summary should preserve history and continue the turn");

        let mut saw_empty_failure = false;
        while let Ok(event) = rx.try_recv() {
            if let crate::event::AgentEvent::CompactionFailed {
                reason: crate::event::CompactionFailureReason::CuratorFailed { message },
            } = event
            {
                assert!(
                    message.contains("empty compaction summary"),
                    "unexpected curator failure message: {message}"
                );
                saw_empty_failure = true;
            }
        }
        assert!(
            saw_empty_failure,
            "an empty curated summary must surface as a typed CuratorFailed compaction failure"
        );
        assert!(
            agent
                .session()
                .messages()
                .iter()
                .any(|message| message.as_indexable_text().contains("first")),
            "an empty curated summary must leave the original history untouched"
        );
    }

    #[tokio::test]
    async fn run_completed_event_uses_canonical_text_after_hook_observation() {
        use crate::hooks::{
            HookEngine, HookEngineError, HookExecutionReport, HookInvocation, HookOutcome,
            HookPoint,
        };

        struct ObserveRunCompletedHook;

        #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
        #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
        impl HookEngine for ObserveRunCompletedHook {
            async fn execute(
                &self,
                invocation: HookInvocation,
                _overrides: Option<&crate::config::HookRunOverrides>,
            ) -> Result<HookExecutionReport, HookEngineError> {
                if invocation.point != HookPoint::RunCompleted {
                    return Ok(HookExecutionReport::empty());
                }
                Ok(HookExecutionReport {
                    started: vec![crate::hooks::HookId::new("observe-run-completed")],
                    outcomes: vec![HookOutcome {
                        hook_id: crate::hooks::HookId::new("observe-run-completed"),
                        point: HookPoint::RunCompleted,
                        priority: 0,
                        registration_index: 0,
                        decision: None,
                        failure_reason: None,
                        duration_ms: None,
                    }],
                    decision: None,
                })
            }
        }

        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .with_hook_engine(Arc::new(ObserveRunCompletedHook))
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
        assert_eq!(result.text, "ok");

        let mut run_completed_text = None;
        while let Ok(event) = rx.try_recv() {
            if let crate::event::AgentEvent::RunCompleted { result, .. } = event {
                run_completed_text = Some(result);
            }
        }
        assert_eq!(
            run_completed_text.as_deref(),
            Some("ok"),
            "RunCompleted should reflect canonical final text, not hook mutation"
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
            .expect("snapshot projects")
            .expect("test turn-state handle should expose a snapshot");
        assert_eq!(snapshot.turn_phase, crate::TurnPhase::Failed);
        assert_eq!(
            snapshot.terminal_outcome,
            crate::TurnTerminalOutcome::Failed,
            "RunCompleted hook denial should leave the canonical turn snapshot failed"
        );
        assert_eq!(
            snapshot.terminal_cause_kind,
            Some(crate::TurnTerminalCauseKind::HookDenied),
            "RunCompleted hook denial should leave the machine-owned terminal cause"
        );
    }

    /// `HookStarted` must mean execution actually began. The agent emits one
    /// `HookStarted` per id in `report.started` (the hooks the engine truly
    /// ran), NOT per id from `matching_hooks()` (which advertises every matched
    /// hook, including ones a deny short-circuit or a saturated background queue
    /// never ran). This engine advertises two matching ids but reports only the
    /// first as started, proving the agent drives `HookStarted` off `started`.
    #[tokio::test]
    async fn hook_started_emitted_only_for_started_hooks_not_matched() {
        use crate::hooks::{
            HookEngine, HookEngineError, HookExecutionReport, HookId, HookInvocation, HookOutcome,
            HookPoint,
        };

        struct PartialStartHook;

        #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
        #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
        impl HookEngine for PartialStartHook {
            fn matching_hooks(
                &self,
                invocation: &HookInvocation,
                _overrides: Option<&crate::config::HookRunOverrides>,
            ) -> Result<Vec<HookId>, HookEngineError> {
                if invocation.point == HookPoint::RunStarted {
                    Ok(vec![HookId::new("ran"), HookId::new("skipped")])
                } else {
                    Ok(Vec::new())
                }
            }

            async fn execute(
                &self,
                invocation: HookInvocation,
                _overrides: Option<&crate::config::HookRunOverrides>,
            ) -> Result<HookExecutionReport, HookEngineError> {
                if invocation.point != HookPoint::RunStarted {
                    return Ok(HookExecutionReport::empty());
                }
                // Two hooks matched, but only "ran" actually executed; "skipped"
                // never began (e.g. it sat behind a saturated background queue).
                Ok(HookExecutionReport {
                    started: vec![HookId::new("ran")],
                    outcomes: vec![HookOutcome {
                        hook_id: HookId::new("ran"),
                        point: HookPoint::RunStarted,
                        priority: 0,
                        registration_index: 0,
                        decision: None,
                        failure_reason: None,
                        duration_ms: Some(0),
                    }],
                    decision: None,
                })
            }
        }

        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .with_hook_engine(Arc::new(PartialStartHook))
            .build_standalone(
                Arc::new(StaticLlmClient),
                Arc::new(NoTools),
                Arc::new(NoopStore),
            )
            .await;

        let (tx, mut rx) = mpsc::channel::<crate::event::AgentEvent>(32);
        agent
            .run_with_events("prompt".to_string().into(), tx)
            .await
            .unwrap();

        let mut started_ids = Vec::new();
        let mut completed_ids = Vec::new();
        while let Ok(event) = rx.try_recv() {
            match event {
                crate::event::AgentEvent::HookStarted { hook_id, point } => {
                    assert_eq!(point, HookPoint::RunStarted);
                    started_ids.push(hook_id);
                }
                crate::event::AgentEvent::HookCompleted { hook_id, .. } => {
                    completed_ids.push(hook_id);
                }
                _ => {}
            }
        }

        assert_eq!(
            started_ids,
            vec![HookId::new("ran")],
            "HookStarted must fire only for hooks the engine actually started, \
             not for every matched hook"
        );
        assert_eq!(
            completed_ids,
            vec![HookId::new("ran")],
            "the started hook also completes"
        );
    }

    /// Ask 5 gate: hook payloads carry typed dispatch-time provenance and the
    /// response's provider-executed server-tool evidence. `HookToolCall` /
    /// `HookToolResult` project `ToolDef.provenance` from the active tool
    /// catalog (matched by name, never re-derived from the name string), and
    /// `HookLlmResponse.server_tool_content` projects the response's
    /// `AssistantBlock::ServerToolContent` kinds so a foreground
    /// `PostLlmResponse` hook classifies provider-native content synchronously
    /// instead of racing the lossy observe stream.
    #[tokio::test]
    async fn hook_payloads_carry_tool_provenance_and_server_tool_content() {
        use crate::hooks::{
            HookEngine, HookEngineError, HookExecutionReport, HookInvocation, HookPoint,
        };
        use crate::types::{ServerToolKind, ToolProvenance, ToolSourceId, ToolSourceKind};

        struct RecordingHook {
            invocations: Mutex<Vec<HookInvocation>>,
        }

        #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
        #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
        impl HookEngine for RecordingHook {
            async fn execute(
                &self,
                invocation: HookInvocation,
                _overrides: Option<&crate::config::HookRunOverrides>,
            ) -> Result<HookExecutionReport, HookEngineError> {
                self.invocations.lock().unwrap().push(invocation);
                Ok(HookExecutionReport::empty())
            }
        }

        struct ServerToolThenToolUseClient {
            call_count: Mutex<u32>,
        }

        #[async_trait]
        impl AgentLlmClient for ServerToolThenToolUseClient {
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
                        vec![
                            AssistantBlock::ServerToolContent {
                                id: Some("srv-1".to_string()),
                                kind: ServerToolKind::WebSearch,
                                content: serde_json::json!({"results": []}),
                                meta: None,
                            },
                            AssistantBlock::ToolUse {
                                id: "call-1".to_string(),
                                name: "lookup".into(),
                                args: serde_json::value::RawValue::from_string("{}".to_string())
                                    .unwrap(),
                                meta: None,
                            },
                        ],
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

            fn provider(&self) -> crate::provider::Provider {
                crate::provider::Provider::Other
            }

            fn model(&self) -> &'static str {
                "mock-model"
            }
        }

        struct ProvenanceToolDispatcher {
            tools: Arc<[Arc<ToolDef>]>,
        }

        #[async_trait]
        impl AgentToolDispatcher for ProvenanceToolDispatcher {
            fn tools(&self) -> Arc<[Arc<ToolDef>]> {
                Arc::clone(&self.tools)
            }

            async fn dispatch(
                &self,
                call: ToolCallView<'_>,
            ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
                Ok(ToolResult::new(call.id.to_string(), "looked up".to_string(), false).into())
            }
        }

        let provenance = ToolProvenance {
            kind: ToolSourceKind::Mcp,
            source_id: ToolSourceId::new("test-server"),
        };
        let dispatcher = ProvenanceToolDispatcher {
            tools: vec![Arc::new(ToolDef {
                name: "lookup".into(),
                description: "lookup tool".to_string(),
                input_schema: serde_json::json!({ "type": "object" }),
                provenance: Some(provenance.clone()),
            })]
            .into(),
        };
        let recorder = Arc::new(RecordingHook {
            invocations: Mutex::new(Vec::new()),
        });

        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .with_hook_engine(Arc::clone(&recorder) as Arc<dyn HookEngine>)
            .build_standalone(
                Arc::new(ServerToolThenToolUseClient {
                    call_count: Mutex::new(0),
                }),
                Arc::new(dispatcher),
                Arc::new(NoopStore),
            )
            .await;

        agent.run("prompt".to_string().into()).await.unwrap();

        let invocations = recorder.invocations.lock().unwrap();
        let post_llm = invocations
            .iter()
            .find(|invocation| {
                invocation.point == HookPoint::PostLlmResponse
                    && invocation
                        .llm_response
                        .as_ref()
                        .is_some_and(|response| !response.server_tool_content.is_empty())
            })
            .expect("PostLlmResponse invocation with server tool content");
        assert_eq!(
            post_llm
                .llm_response
                .as_ref()
                .expect("llm_response payload")
                .server_tool_content,
            vec![ServerToolKind::WebSearch],
            "PostLlmResponse must project ServerToolContent kinds in block order"
        );
        let final_llm = invocations
            .iter()
            .rfind(|invocation| invocation.point == HookPoint::PostLlmResponse)
            .expect("final PostLlmResponse invocation");
        assert!(
            final_llm
                .llm_response
                .as_ref()
                .expect("llm_response payload")
                .server_tool_content
                .is_empty(),
            "a response without server tool blocks projects an empty list"
        );

        let pre_tool = invocations
            .iter()
            .find(|invocation| invocation.point == HookPoint::PreToolExecution)
            .expect("PreToolExecution invocation");
        assert_eq!(
            pre_tool
                .tool_call
                .as_ref()
                .expect("tool_call payload")
                .provenance,
            Some(provenance.clone()),
            "HookToolCall must carry the ToolDef.provenance owner"
        );

        let post_tool = invocations
            .iter()
            .find(|invocation| invocation.point == HookPoint::PostToolExecution)
            .expect("PostToolExecution invocation");
        assert_eq!(
            post_tool
                .tool_result
                .as_ref()
                .expect("tool_result payload")
                .provenance,
            Some(provenance),
            "HookToolResult must carry the ToolDef.provenance owner"
        );
    }

    /// A foreground hook that BEGINS execution and then hard-errors (timeout or
    /// adapter runtime failure) must still emit `HookStarted` before its
    /// terminal `HookFailed`. When `execute()` returns `Err`, the partial
    /// `HookExecutionReport` — including the id pushed into `started` — is
    /// discarded, so the agent surfaces the start from the engine error itself
    /// (a `HookEngineError` carrying a hook_id means that hook began). Without
    /// this, a timed-out hook would emit a terminal `HookFailed` with no
    /// preceding `HookStarted` — the inverse of the started-without-terminal
    /// gap this fix closed.
    #[tokio::test]
    async fn hook_started_emitted_before_hook_failed_on_engine_error() {
        use crate::hooks::{
            HookEngine, HookEngineError, HookExecutionReport, HookId, HookInvocation, HookPoint,
        };

        struct TimingOutHook;

        #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
        #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
        impl HookEngine for TimingOutHook {
            fn matching_hooks(
                &self,
                invocation: &HookInvocation,
                _overrides: Option<&crate::config::HookRunOverrides>,
            ) -> Result<Vec<HookId>, HookEngineError> {
                if invocation.point == HookPoint::RunStarted {
                    Ok(vec![HookId::new("slow")])
                } else {
                    Ok(Vec::new())
                }
            }

            async fn execute(
                &self,
                invocation: HookInvocation,
                _overrides: Option<&crate::config::HookRunOverrides>,
            ) -> Result<HookExecutionReport, HookEngineError> {
                if invocation.point != HookPoint::RunStarted {
                    return Ok(HookExecutionReport::empty());
                }
                // The hook began executing, then timed out. The partial report
                // (with "slow" in `started`) is discarded as the error
                // propagates; the agent must still surface HookStarted.
                Err(HookEngineError::Timeout {
                    hook_id: HookId::new("slow"),
                    timeout_ms: 1,
                })
            }
        }

        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .with_hook_engine(Arc::new(TimingOutHook))
            .build_standalone(
                Arc::new(StaticLlmClient),
                Arc::new(NoTools),
                Arc::new(NoopStore),
            )
            .await;

        let (tx, mut rx) = mpsc::channel::<crate::event::AgentEvent>(32);
        // The run may fail-close on the hook engine error; the event ordering
        // is what this test pins, so the run result itself is not asserted.
        let _ = agent.run_with_events("prompt".to_string().into(), tx).await;

        let mut sequence: Vec<(&str, HookId)> = Vec::new();
        while let Ok(event) = rx.try_recv() {
            match event {
                crate::event::AgentEvent::HookStarted { hook_id, point } => {
                    assert_eq!(point, HookPoint::RunStarted);
                    sequence.push(("started", hook_id));
                }
                crate::event::AgentEvent::HookFailed { hook_id, .. } => {
                    sequence.push(("failed", hook_id));
                }
                _ => {}
            }
        }

        assert_eq!(
            sequence,
            vec![
                ("started", HookId::new("slow")),
                ("failed", HookId::new("slow")),
            ],
            "a hook that started then hard-errored must emit HookStarted before \
             HookFailed, never a terminal HookFailed with no preceding start"
        );
    }

    /// Row #102 gate: a turn-state handle that REJECTS the terminal
    /// `run_completed` effect must NOT be laundered into a `tracing::warn!`
    /// while the run reports success. `execute_turn_effects` fail-closes by
    /// `?`-propagating the rejection as an `AgentError`, so the run
    /// terminalizes with an error rather than fabricating an `Ok(RunResult)`
    /// over a handle that refused the canonical lifecycle fact.
    ///
    /// Wraps the generated-kernel-backed [`TestTurnStateHandle`] so every
    /// non-terminal transition still moves through real MeerkatMachine
    /// authority; only `run_completed` is forced to reject.
    #[derive(Debug)]
    struct RejectRunCompletedHandle {
        inner: crate::agent::test_turn_state_handle::TestTurnStateHandle,
    }

    impl RejectRunCompletedHandle {
        fn new() -> Self {
            Self {
                inner: crate::agent::test_turn_state_handle::TestTurnStateHandle::new(),
            }
        }
    }

    impl crate::handles::TurnStateHandle for RejectRunCompletedHandle {
        fn apply_turn_input(
            &self,
            input: crate::turn_execution_authority::TurnExecutionInput,
        ) -> Result<
            Vec<crate::turn_execution_authority::TurnExecutionEffect>,
            crate::handles::DslTransitionError,
        > {
            self.inner.apply_turn_input(input)
        }

        fn start_conversation_run(
            &self,
            run_id: RunId,
            primitive_kind: TurnPrimitiveKind,
            admitted_content_shape: ContentShape,
            vision_enabled: bool,
            image_tool_results_enabled: bool,
            max_extraction_retries: u64,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.start_conversation_run(
                run_id,
                primitive_kind,
                admitted_content_shape,
                vision_enabled,
                image_tool_results_enabled,
                max_extraction_retries,
            )
        }

        fn start_immediate_append(
            &self,
            run_id: RunId,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.start_immediate_append(run_id)
        }

        fn start_immediate_context(
            &self,
            run_id: RunId,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.start_immediate_context(run_id)
        }

        fn primitive_applied(
            &self,
            run_id: RunId,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.primitive_applied(run_id)
        }

        fn llm_returned_tool_calls(
            &self,
            run_id: RunId,
            tool_count: u64,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.llm_returned_tool_calls(run_id, tool_count)
        }

        fn llm_returned_terminal(
            &self,
            run_id: RunId,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.llm_returned_terminal(run_id)
        }

        fn register_pending_ops(
            &self,
            run_id: RunId,
            op_refs: std::collections::BTreeSet<crate::ops::AsyncOpRef>,
            barrier_operation_ids: std::collections::BTreeSet<crate::ops::OperationId>,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner
                .register_pending_ops(run_id, op_refs, barrier_operation_ids)
        }

        fn tool_calls_resolved(
            &self,
            run_id: RunId,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.tool_calls_resolved(run_id)
        }

        fn ops_barrier_satisfied(
            &self,
            run_id: RunId,
            operation_ids: std::collections::BTreeSet<crate::ops::OperationId>,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.ops_barrier_satisfied(run_id, operation_ids)
        }

        fn boundary_continue(
            &self,
            run_id: RunId,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.boundary_continue(run_id)
        }

        fn boundary_complete(
            &self,
            run_id: RunId,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.boundary_complete(run_id)
        }

        fn enter_extraction(
            &self,
            run_id: RunId,
            max_retries: u32,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.enter_extraction(run_id, max_retries)
        }

        fn extraction_start(
            &self,
            run_id: RunId,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.extraction_start(run_id)
        }

        fn extraction_validation_passed(
            &self,
            run_id: RunId,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.extraction_validation_passed(run_id)
        }

        fn extraction_validation_failed(
            &self,
            run_id: RunId,
            error: String,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.extraction_validation_failed(run_id, error)
        }

        fn extraction_failed(
            &self,
            run_id: RunId,
            error: String,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.extraction_failed(run_id, error)
        }

        fn recoverable_failure(
            &self,
            run_id: RunId,
            retry: crate::retry::LlmRetrySchedule,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.recoverable_failure(run_id, retry)
        }

        fn fatal_failure(
            &self,
            run_id: RunId,
            failure: TurnFailureSource,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.fatal_failure(run_id, failure)
        }

        fn retry_requested(
            &self,
            run_id: RunId,
            retry_attempt: u32,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.retry_requested(run_id, retry_attempt)
        }

        fn cancel_now(&self, run_id: RunId) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.cancel_now(run_id)
        }

        fn request_cancel_after_boundary(
            &self,
            run_id: RunId,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.request_cancel_after_boundary(run_id)
        }

        fn cancellation_observed(
            &self,
            run_id: RunId,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.cancellation_observed(run_id)
        }

        fn acknowledge_terminal(
            &self,
            run_id: RunId,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.acknowledge_terminal(run_id)
        }

        fn turn_limit_reached(
            &self,
            run_id: RunId,
            turn_count: u64,
            max_turns: u64,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.turn_limit_reached(run_id, turn_count, max_turns)
        }

        fn budget_exhausted(
            &self,
            run_id: RunId,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.budget_exhausted(run_id)
        }

        fn time_budget_exceeded(
            &self,
            run_id: RunId,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.time_budget_exceeded(run_id)
        }

        fn force_cancel_no_run(&self) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.force_cancel_no_run()
        }

        /// The fail-closed seam under test: reject the terminal completion
        /// effect. The agent loop must surface this as an error, not warn and
        /// report the run completed.
        fn run_completed(&self, _run_id: RunId) -> Result<(), crate::handles::DslTransitionError> {
            Err(crate::handles::DslTransitionError::guard_rejected(
                "RejectRunCompletedHandle::run_completed",
                "test handle rejects the terminal RunCompleted effect",
            ))
        }

        fn run_failed(
            &self,
            run_id: RunId,
            reason: crate::turn_execution_authority::TurnFailureReason,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.run_failed(run_id, reason)
        }

        fn run_cancelled(&self, run_id: RunId) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.run_cancelled(run_id)
        }

        fn snapshot(&self) -> crate::handles::TurnStateSnapshot {
            self.inner.snapshot()
        }
    }

    #[tokio::test]
    async fn run_completed_effect_rejection_fails_run_closed() {
        let mut agent = AgentBuilder::new()
            .resume_session({
                let mut session = crate::Session::new();
                session
                    .set_build_state(crate::SessionBuildState::default())
                    .expect("test session build state should serialize");
                session
            })
            .with_turn_state_handle(Arc::new(RejectRunCompletedHandle::new()))
            .with_runtime_execution_kind_for_test(
                crate::lifecycle::RuntimeExecutionKind::ContentTurn,
            )
            .build_standalone(
                Arc::new(StaticLlmClient),
                Arc::new(NoTools),
                Arc::new(NoopStore),
            )
            .await;

        let (tx, _rx) = mpsc::channel::<crate::event::AgentEvent>(32);
        let err = agent
            .run_with_events("prompt".to_string().into(), tx)
            .await
            .expect_err(
                "a turn-state handle that rejects the RunCompleted terminal effect must fail \
                 the run closed, not report success",
            );
        assert!(
            matches!(err, AgentError::InternalError(ref message)
                if message.contains("rejected RunCompleted effect")),
            "rejection must propagate as an InternalError naming the rejected terminal effect, \
             got: {err:?}"
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
                crate::event::AgentEvent::RunFailed { error_report, .. } => {
                    saw_run_failed = true;
                    saw_typed_boundary_failure = error_report.class
                        == crate::event::AgentErrorClass::Hook
                        && matches!(
                            error_report.reason.as_ref(),
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
            .expect("snapshot projects")
            .expect("test turn-state handle should expose a snapshot");
        assert_eq!(snapshot.turn_phase, crate::TurnPhase::Failed);
        assert_eq!(
            snapshot.terminal_outcome,
            crate::TurnTerminalOutcome::Failed,
            "boundary hook denial should terminalize through the turn authority"
        );
        assert_eq!(
            snapshot.terminal_cause_kind,
            Some(crate::TurnTerminalCauseKind::HookDenied),
            "boundary hook denial should surface the machine-owned terminal cause"
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
            .expect("snapshot projects")
            .expect("test turn-state handle should expose a snapshot");
        assert_eq!(snapshot.turn_phase, crate::TurnPhase::Failed);
        assert_eq!(
            snapshot.terminal_outcome,
            crate::TurnTerminalOutcome::Failed,
            "post-LLM hook denial should terminalize through the turn authority"
        );
        assert_eq!(
            snapshot.terminal_cause_kind,
            Some(crate::TurnTerminalCauseKind::HookDenied),
            "post-LLM hook denial should retain the typed machine terminal cause"
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
        use crate::lifecycle::run_primitive::{
            OpaqueProviderBody, OpenAiProviderTag, ProviderParamsOverride, ProviderTag,
        };

        let client = Arc::new(RecordingLlmClient::new());
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .provider_params(ProviderParamsOverride {
                top_p: Some(0.5),
                ..Default::default()
            })
            .provider_tool_defaults(ProviderTag::OpenAi(OpenAiProviderTag {
                web_search: Some(OpaqueProviderBody::from_value(
                    &serde_json::json!({"type": "stale"}),
                )),
                ..Default::default()
            }))
            .build_standalone(client.clone(), Arc::new(NoTools), Arc::new(NoopStore))
            .await;
        agent.config.max_turns = Some(1);

        agent.replace_client_with_request_policy(
            client.clone(),
            crate::SessionLlmRequestPolicy {
                model: "new-model".to_string(),
                provider_params: Some(ProviderParamsOverride {
                    temperature: Some(0.2),
                    ..Default::default()
                }),
                provider_tool_defaults: Some(ProviderTag::OpenAi(OpenAiProviderTag {
                    web_search: Some(OpaqueProviderBody::from_value(
                        &serde_json::json!({"type": "web_search"}),
                    )),
                    ..Default::default()
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

    fn explicit_hot_swap_identity(model: &str) -> crate::SessionLlmIdentity {
        crate::SessionLlmIdentity {
            model: model.to_string(),
            provider: crate::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        }
    }

    fn explicit_hot_swap_policy(model: &str) -> crate::SessionLlmRequestPolicy {
        crate::SessionLlmRequestPolicy {
            model: model.to_string(),
            provider_params: None,
            provider_tool_defaults: None,
        }
    }

    fn explicit_hot_swap_session(model: &str) -> crate::Session {
        let mut session = crate::Session::new();
        session
            .set_session_metadata(crate::SessionMetadata {
                schema_version: crate::SESSION_METADATA_SCHEMA_VERSION,
                model: model.to_string(),
                max_tokens: 16_384,
                structured_output_retries: 2,
                provider: crate::Provider::OpenAI,
                self_hosted_server_id: None,
                provider_params: None,
                tooling: crate::SessionTooling::default(),
                keep_alive: false,
                comms_name: None,
                peer_meta: None,
                realm_id: None,
                instance_id: None,
                backend: None,
                config_generation: None,
                auth_binding: None,
                mob_member_binding: None,
            })
            .expect("hot-swap test metadata should serialize");
        session
            .set_tool_visibility_state(
                crate::session::AuthorizedSessionToolVisibilityState::from_generated_authority(
                    crate::SessionToolVisibilityState::default(),
                ),
            )
            .expect("hot-swap test visibility parent should serialize");
        session
    }

    #[tokio::test]
    async fn explicit_hot_swap_moves_profile_limit_and_durable_identity_both_directions() {
        let registry = fallback_activation_test_registry();
        let primary = Arc::new(HotSwapLimitRecordingClient::new("primary"));
        let backup = Arc::new(HotSwapLimitRecordingClient::new("backup"));
        let mut agent = with_test_turn_state_handle_for_session(
            AgentBuilder::new()
                .model("primary")
                .max_tokens_per_turn(16_384)
                .with_effective_model_registry(Arc::clone(&registry)),
            explicit_hot_swap_session("primary"),
        )
        .with_tool_visibility_owner(explicit_test_visibility_owner())
        .build_standalone(primary.clone(), Arc::new(NoTools), Arc::new(NoopStore))
        .await;
        agent.config.max_turns = Some(1);

        agent
            .hot_swap_llm_identity(
                backup.clone(),
                explicit_hot_swap_identity("backup"),
                explicit_hot_swap_policy("backup"),
            )
            .expect("primary to backup hot-swap should commit");
        assert_eq!(
            agent
                .session()
                .session_metadata()
                .expect("durable metadata must remain present")
                .llm_identity(),
            explicit_hot_swap_identity("backup")
        );
        assert_eq!(
            agent
                .active_model_profile
                .as_ref()
                .map(crate::ModelProfileWitness::model),
            Some("backup")
        );
        agent
            .run("use backup profile".into())
            .await
            .expect("backup request should succeed");
        assert_eq!(backup.seen_max_tokens(), vec![2048]);

        agent
            .hot_swap_llm_identity(
                primary.clone(),
                explicit_hot_swap_identity("primary"),
                explicit_hot_swap_policy("primary"),
            )
            .expect("backup to primary hot-swap should commit");
        assert_eq!(
            agent
                .session()
                .session_metadata()
                .expect("durable metadata must remain present")
                .llm_identity(),
            explicit_hot_swap_identity("primary")
        );
        assert_eq!(
            agent
                .active_model_profile
                .as_ref()
                .map(crate::ModelProfileWitness::model),
            Some("primary")
        );
        agent
            .run("use primary profile again".into())
            .await
            .expect("primary request should succeed");
        assert_eq!(primary.seen_max_tokens(), vec![8192]);
    }

    #[tokio::test]
    async fn explicit_hot_swap_rejects_unknown_profile_before_live_mutation() {
        let registry = fallback_activation_test_registry();
        let primary = Arc::new(HotSwapLimitRecordingClient::new("primary"));
        let unknown = Arc::new(HotSwapLimitRecordingClient::new("unknown"));
        let mut agent = with_test_turn_state_handle_for_session(
            AgentBuilder::new()
                .model("primary")
                .max_tokens_per_turn(16_384)
                .with_effective_model_registry(Arc::clone(&registry)),
            explicit_hot_swap_session("primary"),
        )
        .with_tool_visibility_owner(explicit_test_visibility_owner())
        .build_standalone(primary.clone(), Arc::new(NoTools), Arc::new(NoopStore))
        .await;
        agent.config.max_turns = Some(1);

        let error = agent
            .hot_swap_llm_identity(
                unknown.clone(),
                explicit_hot_swap_identity("unknown"),
                explicit_hot_swap_policy("unknown"),
            )
            .expect_err("an unregistered target must fail before publication");
        assert!(
            matches!(error, AgentError::ConfigError(ref message)
                if message.contains("absent from the agent's effective model registry")),
            "unexpected rejection: {error:?}"
        );
        assert_eq!(
            agent
                .session()
                .session_metadata()
                .expect("durable metadata must remain present")
                .llm_identity(),
            explicit_hot_swap_identity("primary")
        );
        assert_eq!(
            agent
                .active_model_profile
                .as_ref()
                .map(crate::ModelProfileWitness::model),
            Some("primary")
        );

        agent
            .run("rejected swap keeps primary".into())
            .await
            .expect("the original client should remain live");
        assert_eq!(primary.seen_max_tokens(), vec![8192]);
        assert!(unknown.seen_max_tokens().is_empty());
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

        // The durable transcript must preserve the typed activation provenance
        // — a SkillContext block carrying the canonical key — never an
        // anonymous text fold of the rendered body into operator text.
        let has_typed_skill_block = agent.session().messages().iter().any(|message| {
            matches!(message, Message::User(user) if user.content.iter().any(|block| {
                matches!(
                    block,
                    ContentBlock::SkillContext { skill_key, .. }
                        if skill_key.to_string()
                            == "dc256086-0d2f-4f61-a307-320d4148107f/email-extractor"
                )
            }))
        });
        assert!(
            has_typed_skill_block,
            "durable transcript must carry the typed SkillContext block with provenance"
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

    /// Row 205: a durable image blob missing from the store at the live
    /// LLM-execution hydration seam terminalizes the turn with a typed
    /// `ConfigError` — the model boundary is unconditionally fail-closed and
    /// never degrades to placeholder prompt text.
    #[tokio::test]
    async fn llm_execution_errors_on_missing_blob_when_policy_is_error() {
        let missing_blob_id = BlobId::new("sha256:absent-image");
        // Empty blob store: the referenced blob is absent.
        let blob_store = Arc::new(RecordingBlobStore::new(Vec::new()));
        let client = Arc::new(ImageHydrationLlmClient::new());
        let mut session = crate::Session::new();
        session.push(Message::User(UserMessage::with_blocks(vec![
            ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: ImageData::Blob {
                    blob_id: missing_blob_id.clone(),
                },
            },
        ])));

        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .resume_session(session)
            .with_blob_store(blob_store.clone())
            .build_standalone(client.clone(), Arc::new(NoTools), Arc::new(NoopStore))
            .await;
        agent.config.max_turns = Some(1);

        let err = agent
            .run("current turn".to_string().into())
            .await
            .expect_err("missing required blob at the model boundary must fail the run");

        assert!(
            matches!(err, AgentError::ConfigError(ref message) if message.contains("absent-image")),
            "missing-blob refusal must surface as a typed ConfigError, got: {err:?}"
        );
        // The provider must never be reached with a placeholder-substituted
        // image: the fault is raised before the LLM call.
        assert!(
            client.seen().is_empty(),
            "provider must not be called when the required blob is missing under Error policy"
        );
    }

    #[tokio::test]
    async fn standalone_filter_staging_requires_generated_visibility_authority() {
        let client = Arc::new(VisibilityRecordingLlmClient::new());
        let tools = Arc::new(FullToolDispatcher::new(&["visible", "secret"]));
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .build_standalone(client.clone(), tools.clone(), Arc::new(NoopStore))
            .await;

        let err = agent
            .stage_external_tool_filter(ToolFilter::Deny(
                ["secret".to_string()].into_iter().collect(),
            ))
            .expect_err("standalone core must not mutate visibility without generated authority");
        assert_generated_visibility_authority_required(err);

        assert!(client.seen_tools().is_empty());
        assert!(tools.dispatched().is_empty());
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
    async fn external_tool_dispatch_timeout_uses_canonical_terminal_outcome() {
        struct HangingDispatcher {
            tools: Arc<[Arc<ToolDef>]>,
        }

        #[async_trait]
        impl AgentToolDispatcher for HangingDispatcher {
            fn tools(&self) -> Arc<[Arc<ToolDef>]> {
                Arc::clone(&self.tools)
            }

            async fn dispatch(
                &self,
                _call: ToolCallView<'_>,
            ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
                std::future::pending().await
            }
        }

        let client = Arc::new(StaticLlmClient);
        let tools = Arc::new(HangingDispatcher {
            tools: Arc::from([Arc::new(ToolDef {
                name: "slow".into(),
                description: "hangs until timeout".to_string(),
                input_schema: serde_json::json!({ "type": "object" }),
                provenance: None,
            })]),
        });
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .build_standalone(client, tools, Arc::new(NoopStore))
            .await;

        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            agent.dispatch_external_tool_call_with_timeout_policy(
                ToolCall::new(
                    "tool-call-timeout".to_string(),
                    "slow".to_string(),
                    serde_json::json!({}),
                ),
                crate::ops::ToolDispatchTimeoutPolicy::Finite {
                    timeout: std::time::Duration::from_millis(5),
                },
            ),
        )
        .await
        .expect("external dispatch should terminalize promptly")
        .expect("timeout terminalization should be a tool outcome");

        let expected = crate::ops::terminal_tool_outcome_for_error(
            "tool-call-timeout",
            ToolError::timeout("slow", 5),
        );
        assert_eq!(outcome.result.tool_use_id, expected.result.tool_use_id);
        assert!(outcome.result.is_error);
        assert_eq!(
            outcome.result.text_content(),
            expected.result.text_content()
        );
        assert!(outcome.is_runtime_tool_timeout());
        assert_eq!(outcome.terminal_cause(), expected.terminal_cause());
    }

    /// Row 166: the normal LLM-driven tool dispatch loop must apply the typed
    /// `ToolsConfig` per-call timeout. A tool that hangs forever must be
    /// terminalized as a `timeout` tool result (flowing through
    /// `terminal_tool_outcome_for_error`) rather than blocking the turn.
    #[tokio::test]
    async fn normal_loop_tool_dispatch_applies_tools_config_timeout() {
        struct HangingTurnDispatcher {
            tools: Arc<[Arc<ToolDef>]>,
        }

        #[async_trait]
        impl AgentToolDispatcher for HangingTurnDispatcher {
            fn tools(&self) -> Arc<[Arc<ToolDef>]> {
                Arc::clone(&self.tools)
            }

            async fn dispatch(
                &self,
                _call: ToolCallView<'_>,
            ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
                std::future::pending().await
            }
        }

        let client = Arc::new(BoundaryContextRecordingClient::new());
        let tools = Arc::new(HangingTurnDispatcher {
            tools: Arc::from([Arc::new(ToolDef::new(
                "slow_tool",
                "hangs until the per-call timeout fires",
                serde_json::json!({ "type": "object" }),
            ))]),
        });

        let tools_config = crate::config::ToolsConfig {
            default_timeout: std::time::Duration::from_millis(20),
            ..crate::config::ToolsConfig::default()
        };

        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .with_tools_config(tools_config)
            .build_standalone(client, tools, Arc::new(NoopStore))
            .await;

        let (tx, mut rx) = mpsc::channel(64);
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            agent.run_with_events("start with a tool".to_string().into(), tx),
        )
        .await
        .expect("run must complete promptly once the per-call timeout fires")
        .expect("agent run should succeed after timeout terminalization");

        // The hanging tool resolves to a timeout tool result, so the second
        // turn ends the conversation: two turns total.
        assert_eq!(result.turns, 2);

        let mut saw_timeout_completion = false;
        while let Ok(event) = rx.try_recv() {
            if let crate::event::AgentEvent::ToolExecutionCompleted {
                name,
                is_error,
                content,
                ..
            } = event
                && name == "slow_tool"
            {
                assert!(is_error, "timed-out tool result must be an error");
                let text = crate::types::text_content(&content);
                assert!(
                    text.contains("\"error\":\"timeout\""),
                    "timed-out tool result must carry the canonical timeout payload, got: {text}"
                );
                saw_timeout_completion = true;
            }
        }
        assert!(
            saw_timeout_completion,
            "normal dispatch loop must emit a timeout tool completion for the hanging tool"
        );
    }

    /// Ask 6: a call denied by the call-level execution gate
    /// ([`crate::tool_execution_policy::ExecutionPolicyGatedDispatcher`])
    /// surfaces as an ordinary `is_error` tool result (via
    /// `terminal_tool_outcome_for_error`) and the run CONTINUES — denial is
    /// per-call, never run-fatal. The denied tool stays in the LLM-visible
    /// list, so the visible-set precheck passes and the deny happens inside
    /// dispatch.
    #[tokio::test]
    async fn execution_policy_gate_denial_is_ordinary_tool_error_and_run_continues() {
        struct RecordingDispatcher {
            tools: Arc<[Arc<ToolDef>]>,
            dispatched: Mutex<Vec<String>>,
        }

        #[async_trait]
        impl AgentToolDispatcher for RecordingDispatcher {
            fn tools(&self) -> Arc<[Arc<ToolDef>]> {
                Arc::clone(&self.tools)
            }

            async fn dispatch(
                &self,
                call: ToolCallView<'_>,
            ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
                self.dispatched.lock().unwrap().push(call.name.to_string());
                Ok(crate::ops::ToolDispatchOutcome::from(ToolResult::new(
                    call.id.to_string(),
                    "ok".to_string(),
                    false,
                )))
            }
        }

        struct DeniedToolCallClient {
            call_count: Mutex<u32>,
        }

        #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
        #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
        impl AgentLlmClient for DeniedToolCallClient {
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
                            id: "call-blocked".to_string(),
                            name: "blocked_tool".into(),
                            args: serde_json::value::RawValue::from_string("{}".to_string())
                                .expect("static raw value should parse"),
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

            fn provider(&self) -> crate::provider::Provider {
                crate::provider::Provider::Other
            }

            fn model(&self) -> &'static str {
                "mock-model"
            }
        }

        let inner = Arc::new(RecordingDispatcher {
            tools: Arc::from([
                Arc::new(ToolDef::new(
                    "blocked_tool",
                    "denied by the execution policy",
                    serde_json::json!({ "type": "object" }),
                )),
                Arc::new(ToolDef::new(
                    "open_tool",
                    "not denied",
                    serde_json::json!({ "type": "object" }),
                )),
            ]),
            dispatched: Mutex::new(Vec::new()),
        });
        let policy = crate::tool_execution_policy::ToolExecutionPolicy::resolve(
            crate::ops::ToolAccessPolicy::DenyList(["blocked_tool"].into_iter().collect()),
        )
        .expect("deny list must resolve");
        let gated = Arc::new(
            crate::tool_execution_policy::ExecutionPolicyGatedDispatcher::new(
                Arc::clone(&inner),
                policy,
            ),
        );

        let client = Arc::new(DeniedToolCallClient {
            call_count: Mutex::new(0),
        });
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .build_standalone(client, gated, Arc::new(NoopStore))
            .await;

        let (tx, mut rx) = mpsc::channel(64);
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            agent.run_with_events("call the blocked tool".to_string().into(), tx),
        )
        .await
        .expect("run must complete promptly after the denial")
        .expect("agent run must succeed — a policy denial is never run-fatal");

        // The denied call resolves to an error tool result, so the second
        // turn ends the conversation: two turns total.
        assert_eq!(result.turns, 2);
        assert!(
            inner.dispatched.lock().unwrap().is_empty(),
            "denied call must never reach the inner dispatcher"
        );

        let mut saw_denied_completion = false;
        while let Ok(event) = rx.try_recv() {
            if let crate::event::AgentEvent::ToolExecutionCompleted {
                name,
                is_error,
                content,
                ..
            } = event
                && name == "blocked_tool"
            {
                assert!(is_error, "denied tool result must be an error");
                let text = crate::types::text_content(&content);
                assert!(
                    text.contains("\"error\":\"access_denied\""),
                    "denied tool result must carry the canonical access_denied payload, got: {text}"
                );
                saw_denied_completion = true;
            }
        }
        assert!(
            saw_denied_completion,
            "dispatch loop must emit an access_denied tool completion for the gated tool"
        );
    }

    #[tokio::test]
    async fn external_tool_dispatch_clears_tool_returned_terminal_cause() {
        struct ToolAuthoredTerminalCauseDispatcher {
            tools: Arc<[Arc<ToolDef>]>,
        }

        #[async_trait]
        impl AgentToolDispatcher for ToolAuthoredTerminalCauseDispatcher {
            fn tools(&self) -> Arc<[Arc<ToolDef>]> {
                Arc::clone(&self.tools)
            }

            async fn dispatch(
                &self,
                call: ToolCallView<'_>,
            ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
                Ok(crate::ops::terminal_tool_outcome_for_error(
                    call.id.to_string(),
                    ToolError::timeout(call.name.to_string(), 5),
                ))
            }
        }

        let client = Arc::new(StaticLlmClient);
        let tools = Arc::new(ToolAuthoredTerminalCauseDispatcher {
            tools: Arc::from([Arc::new(ToolDef {
                name: "spoof_timeout".into(),
                description: "returns timeout-shaped tool output".to_string(),
                input_schema: serde_json::json!({ "type": "object" }),
                provenance: None,
            })]),
        });
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .build_standalone(client, tools, Arc::new(NoopStore))
            .await;

        let outcome = agent
            .dispatch_external_tool_call(ToolCall::new(
                "tool-call-spoof".to_string(),
                "spoof_timeout".to_string(),
                serde_json::json!({}),
            ))
            .await
            .expect("external dispatch should return tool-authored result");

        assert!(outcome.result.is_error);
        assert!(
            outcome
                .result
                .text_content()
                .contains("\"error\":\"timeout\""),
            "tool-authored payload should keep its transcript text"
        );
        assert!(
            !outcome.is_runtime_tool_timeout(),
            "runtime must not trust terminal cause supplied by tool-authored Ok outcomes"
        );
        assert_eq!(outcome.terminal_cause(), None);
    }

    #[tokio::test]
    async fn callback_pending_closes_generated_turn_before_external_terminal() {
        struct CallbackPendingDispatcher {
            tools: Arc<[Arc<ToolDef>]>,
        }

        #[async_trait]
        impl AgentToolDispatcher for CallbackPendingDispatcher {
            fn tools(&self) -> Arc<[Arc<ToolDef>]> {
                Arc::clone(&self.tools)
            }

            async fn dispatch(
                &self,
                call: ToolCallView<'_>,
            ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
                Err(ToolError::callback_pending(
                    call.name,
                    serde_json::json!({ "question": "approve?" }),
                ))
            }
        }

        struct CallbackToolCallClient;

        #[async_trait]
        impl AgentLlmClient for CallbackToolCallClient {
            async fn stream_response(
                &self,
                _messages: &[Message],
                _tools: &[Arc<ToolDef>],
                _max_tokens: u32,
                _temperature: Option<f32>,
                _provider_params: Option<&crate::lifecycle::run_primitive::ProviderParamsOverride>,
            ) -> Result<super::LlmStreamResult, AgentError> {
                Ok(super::LlmStreamResult::new(
                    vec![AssistantBlock::ToolUse {
                        id: "callback-call".to_string(),
                        name: "ask_user".into(),
                        args: serde_json::value::RawValue::from_string(
                            r#"{"question":"approve?"}"#.to_string(),
                        )
                        .expect("static callback arguments should parse"),
                        meta: None,
                    }],
                    StopReason::ToolUse,
                    Usage::default(),
                ))
            }

            fn provider(&self) -> crate::provider::Provider {
                crate::provider::Provider::Other
            }

            fn model(&self) -> &'static str {
                "mock-model"
            }
        }

        let turn_handle =
            Arc::new(crate::agent::test_turn_state_handle::TestTurnStateHandle::new());
        let mut agent = AgentBuilder::new()
            .with_turn_state_handle(turn_handle.clone())
            .with_runtime_execution_kind_for_test(
                crate::lifecycle::RuntimeExecutionKind::ContentTurn,
            )
            .build_standalone(
                Arc::new(CallbackToolCallClient),
                Arc::new(CallbackPendingDispatcher {
                    tools: Arc::from([Arc::new(ToolDef::new(
                        "ask_user",
                        "waits for an external callback",
                        serde_json::json!({ "type": "object" }),
                    ))]),
                }),
                Arc::new(NoopStore),
            )
            .await;

        let error = agent
            .run("ask for approval".to_string().into())
            .await
            .expect_err("callback tool dispatch must return the external terminal");
        match error {
            AgentError::CallbackPending { tool_name, args } => {
                assert_eq!(tool_name, "ask_user");
                assert_eq!(args["question"], "approve?");
                assert_eq!(args["tool_use_id"], "callback-call");
            }
            other => panic!("expected callback-pending terminal, got {other:?}"),
        }

        let assistant_run_id = agent
            .session()
            .messages()
            .iter()
            .find_map(|message| match message {
                Message::BlockAssistant(assistant) if assistant.has_tool_calls() => {
                    assistant.identity.run_id.clone()
                }
                _ => None,
            })
            .expect("callback continuation must retain its run-stamped assistant tool call");
        assert!(
            agent
                .session()
                .messages()
                .iter()
                .all(|message| !matches!(message, Message::ToolResults { .. })),
            "the callback result remains an external continuation responsibility"
        );

        let snapshot = agent
            .execution_snapshot()
            .expect("snapshot projects")
            .expect("test turn-state handle should expose a snapshot");
        assert_eq!(snapshot.turn_phase, crate::TurnPhase::Completed);
        assert!(snapshot.turn_terminal);
        assert_eq!(snapshot.active_run_id, None);
        assert_eq!(snapshot.terminal_run_id, Some(assistant_run_id));
        assert_eq!(
            snapshot.terminal_outcome,
            crate::TurnTerminalOutcome::Completed
        );
        assert_eq!(snapshot.terminal_cause_kind, None);
        assert_eq!(snapshot.tool_calls_pending, 0);
        assert_eq!(turn_handle.run_completed_effect_count(), 1);
        assert_eq!(turn_handle.run_failed_effect_count(), 0);
    }

    /// Row 273: a `WaitingForOps` authority fault (here: barrier ops
    /// registered without an `ops_lifecycle` registry) must terminalize the
    /// turn through `TurnExecutionInput::FatalFailure` BEFORE returning the
    /// raw `AgentError`, so the turn is never left non-terminal.
    #[tokio::test]
    async fn waiting_for_ops_registry_missing_terminalizes_turn() {
        struct BarrierOpDispatcher {
            tools: Arc<[Arc<ToolDef>]>,
        }

        #[async_trait]
        impl AgentToolDispatcher for BarrierOpDispatcher {
            fn tools(&self) -> Arc<[Arc<ToolDef>]> {
                Arc::clone(&self.tools)
            }

            async fn dispatch(
                &self,
                call: ToolCallView<'_>,
            ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
                // Register a barrier async op with no backing ops_lifecycle
                // registry on the agent, forcing the WaitingForOps arm into
                // the "registry missing" fault path.
                let mut outcome = crate::ops::ToolDispatchOutcome::sync_result(ToolResult::new(
                    call.id.to_string(),
                    "queued".to_string(),
                    false,
                ));
                outcome.async_ops.push(crate::ops::AsyncOpRef::barrier(
                    crate::ops::OperationId::new(),
                ));
                Ok(outcome)
            }
        }

        struct SingleToolCallClient;

        #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
        #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
        impl AgentLlmClient for SingleToolCallClient {
            async fn stream_response(
                &self,
                _messages: &[Message],
                _tools: &[Arc<ToolDef>],
                _max_tokens: u32,
                _temperature: Option<f32>,
                _provider_params: Option<&crate::lifecycle::run_primitive::ProviderParamsOverride>,
            ) -> Result<super::LlmStreamResult, AgentError> {
                Ok(super::LlmStreamResult::new(
                    vec![AssistantBlock::ToolUse {
                        id: "call-barrier".to_string(),
                        name: "queue_op".into(),
                        args: serde_json::value::RawValue::from_string("{}".to_string())
                            .expect("static raw value should parse"),
                        meta: None,
                    }],
                    StopReason::ToolUse,
                    Usage::default(),
                ))
            }

            fn provider(&self) -> crate::provider::Provider {
                crate::provider::Provider::Other
            }

            fn model(&self) -> &'static str {
                "mock-model"
            }
        }

        let client = Arc::new(SingleToolCallClient);
        let tools = Arc::new(BarrierOpDispatcher {
            tools: Arc::from([Arc::new(ToolDef {
                name: "queue_op".into(),
                description: "registers a barrier async op".to_string(),
                input_schema: serde_json::json!({ "type": "object" }),
                provenance: None,
            })]),
        });
        let turn_handle =
            Arc::new(crate::agent::test_turn_state_handle::TestTurnStateHandle::new());
        let mut agent = AgentBuilder::new()
            .with_turn_state_handle(turn_handle.clone())
            .with_runtime_execution_kind_for_test(
                crate::lifecycle::RuntimeExecutionKind::ContentTurn,
            )
            .build_standalone(client, tools, Arc::new(NoopStore))
            .await;

        let err = agent
            .run("prompt".to_string().into())
            .await
            .expect_err("barrier ops without a registry must fail the run");
        assert!(
            matches!(err, AgentError::InternalError(_)),
            "registry-missing fault should surface as an internal error"
        );

        let snapshot = agent
            .execution_snapshot()
            .expect("snapshot projects")
            .expect("test turn-state handle should expose a snapshot");
        assert_eq!(
            snapshot.terminal_outcome,
            TurnTerminalOutcome::Failed,
            "WaitingForOps registry-missing fault must terminalize the turn authority"
        );
        assert_eq!(
            turn_handle.run_failed_effect_count(),
            1,
            "the fault must terminalize through the failed authority exactly once"
        );
        assert_eq!(
            turn_handle.run_completed_effect_count(),
            0,
            "a WaitingForOps fault must not publish completed authority effects"
        );
    }

    /// Row 274: an unsupported video tool result must terminalize the turn
    /// via the machine and must NOT emit success-shaped progress events
    /// (`ToolExecutionCompleted`/`ToolResultReceived`) for the rejected call.
    #[tokio::test]
    async fn unsupported_video_tool_result_terminalizes_without_success_events() {
        struct VideoResultDispatcher {
            tools: Arc<[Arc<ToolDef>]>,
        }

        #[async_trait]
        impl AgentToolDispatcher for VideoResultDispatcher {
            fn tools(&self) -> Arc<[Arc<ToolDef>]> {
                Arc::clone(&self.tools)
            }

            async fn dispatch(
                &self,
                call: ToolCallView<'_>,
            ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
                Ok(ToolResult::with_blocks(
                    call.id.to_string(),
                    vec![ContentBlock::Video {
                        media_type: "video/mp4".to_string(),
                        duration_ms: 1_000,
                        data: crate::types::VideoData::from("AAAA"),
                    }],
                    false,
                )
                .into())
            }
        }

        struct SingleToolCallClient;

        #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
        #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
        impl AgentLlmClient for SingleToolCallClient {
            async fn stream_response(
                &self,
                _messages: &[Message],
                _tools: &[Arc<ToolDef>],
                _max_tokens: u32,
                _temperature: Option<f32>,
                _provider_params: Option<&crate::lifecycle::run_primitive::ProviderParamsOverride>,
            ) -> Result<super::LlmStreamResult, AgentError> {
                Ok(super::LlmStreamResult::new(
                    vec![AssistantBlock::ToolUse {
                        id: "call-video".to_string(),
                        name: "make_video".into(),
                        args: serde_json::value::RawValue::from_string("{}".to_string())
                            .expect("static raw value should parse"),
                        meta: None,
                    }],
                    StopReason::ToolUse,
                    Usage::default(),
                ))
            }

            fn provider(&self) -> crate::provider::Provider {
                crate::provider::Provider::Other
            }

            fn model(&self) -> &'static str {
                "mock-model"
            }
        }

        let client = Arc::new(SingleToolCallClient);
        let tools = Arc::new(VideoResultDispatcher {
            tools: Arc::from([Arc::new(ToolDef {
                name: "make_video".into(),
                description: "returns an unsupported video block".to_string(),
                input_schema: serde_json::json!({ "type": "object" }),
                provenance: None,
            })]),
        });
        let turn_handle =
            Arc::new(crate::agent::test_turn_state_handle::TestTurnStateHandle::new());
        let mut agent = AgentBuilder::new()
            .with_turn_state_handle(turn_handle.clone())
            .with_runtime_execution_kind_for_test(
                crate::lifecycle::RuntimeExecutionKind::ContentTurn,
            )
            .build_standalone(client, tools, Arc::new(NoopStore))
            .await;

        let (tx, mut rx) = mpsc::channel::<crate::event::AgentEvent>(32);
        let err = agent
            .run_with_events("prompt".to_string().into(), tx)
            .await
            .expect_err("unsupported video tool result must fail the run");
        assert!(
            matches!(err, AgentError::ConfigError(_)),
            "video rejection should surface as a config error"
        );

        let mut saw_tool_progress_event = false;
        while let Ok(event) = rx.try_recv() {
            if matches!(
                event,
                crate::event::AgentEvent::ToolExecutionCompleted { .. }
                    | crate::event::AgentEvent::ToolResultReceived { .. }
            ) {
                saw_tool_progress_event = true;
            }
        }
        assert!(
            !saw_tool_progress_event,
            "rejected video tool result must not emit success-shaped progress events"
        );

        let snapshot = agent
            .execution_snapshot()
            .expect("snapshot projects")
            .expect("test turn-state handle should expose a snapshot");
        assert_eq!(
            snapshot.terminal_outcome,
            TurnTerminalOutcome::Failed,
            "unsupported video tool result must terminalize the turn authority"
        );
        assert_eq!(
            turn_handle.run_failed_effect_count(),
            1,
            "video rejection must terminalize through the failed authority exactly once"
        );
        assert_eq!(
            turn_handle.run_completed_effect_count(),
            0,
            "video rejection must not publish completed authority effects"
        );
    }

    #[tokio::test]
    async fn standalone_external_hidden_tool_policy_requires_generated_visibility_authority() {
        let client = Arc::new(StaticLlmClient);
        let tools = Arc::new(FullToolDispatcher::new(&["visible", "secret"]));
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .build_standalone(client, tools.clone(), Arc::new(NoopStore))
            .await;
        let err = agent
            .stage_external_tool_filter(ToolFilter::Deny(
                ["secret".to_string()].into_iter().collect(),
            ))
            .expect_err("hidden-tool policy cannot be staged without generated authority");
        assert_generated_visibility_authority_required(err);
        assert!(
            tools.dispatched().is_empty(),
            "failed policy staging must not reach the dispatcher"
        );
    }

    #[tokio::test]
    async fn external_tool_dispatch_fails_closed_without_deferred_visibility_authority() {
        let client = Arc::new(StaticLlmClient);
        let tools = Arc::new(DeferredLoadDispatcher::new());
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .build_standalone(client, tools, Arc::new(NoopStore))
            .await;

        let err = agent
            .dispatch_external_tool_call(ToolCall::new(
                "tool-call-2".to_string(),
                "tool_catalog_load".to_string(),
                serde_json::json!({}),
            ))
            .await
            .expect_err("deferred tool effects require generated visibility authority");

        assert_generated_visibility_authority_required(err);
        if let Some(visibility_state) = agent
            .session()
            .tool_visibility_state()
            .expect("session visibility state should decode")
        {
            assert!(
                !visibility_state
                    .staged_requested_deferred_names
                    .contains("deferred_tool"),
                "failed deferred visibility effects must not stage deferred_tool"
            );
            assert!(
                !visibility_state
                    .active_requested_deferred_names
                    .contains("deferred_tool"),
                "failed deferred visibility effects must not activate deferred_tool"
            );
        }
    }

    #[tokio::test]
    async fn llm_hidden_tool_policy_staging_requires_generated_visibility_authority() {
        use crate::hooks::{
            HookEngine, HookEngineError, HookExecutionReport, HookInvocation, HookPoint,
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
                        failure_reason: None,
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

        let err = agent
            .stage_external_tool_filter(ToolFilter::Deny(
                ["secret".to_string()].into_iter().collect(),
            ))
            .expect_err("hidden-tool policy cannot be staged without generated authority");
        assert_generated_visibility_authority_required(err);
        let seen = client.seen_tools();
        assert!(seen.is_empty());
        assert!(
            !agent
                .session()
                .messages()
                .iter()
                .any(|message| matches!(message, Message::ToolResults { .. })),
            "failed policy staging must not fabricate a transcript ToolResult"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "failed policy staging should not pass through post-tool hooks as a tool result"
        );
        assert!(
            tools.dispatched().is_empty(),
            "failed policy staging must not reach the dispatcher"
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

        fn provider(&self) -> crate::provider::Provider {
            crate::provider::Provider::Other
        }

        fn model(&self) -> &'static str {
            "mock-model"
        }
    }

    struct EmptyAfterImageEffectClient {
        call_count: Mutex<u32>,
    }

    #[async_trait]
    impl AgentLlmClient for EmptyAfterImageEffectClient {
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
                super::LlmStreamResult::new(Vec::new(), StopReason::EndTurn, Usage::default())
            };
            *calls += 1;
            Ok(response)
        }

        fn provider(&self) -> crate::provider::Provider {
            crate::provider::Provider::Other
        }

        fn model(&self) -> &'static str {
            "mock-model"
        }
    }

    struct DirectImageClient;

    #[async_trait]
    impl AgentLlmClient for DirectImageClient {
        async fn stream_response(
            &self,
            _messages: &[Message],
            _tools: &[Arc<ToolDef>],
            _max_tokens: u32,
            _temperature: Option<f32>,
            _provider_params: Option<&crate::lifecycle::run_primitive::ProviderParamsOverride>,
        ) -> Result<super::LlmStreamResult, AgentError> {
            Ok(super::LlmStreamResult::new(
                vec![AssistantBlock::Image {
                    image_id: crate::AssistantImageId::new(uuid::Uuid::from_u128(501)),
                    blob_ref: crate::BlobRef {
                        blob_id: crate::BlobId::new("direct-image-blob"),
                        media_type: "image/png".to_string(),
                    },
                    media_type: crate::MediaType::new("image/png"),
                    width: 32,
                    height: 24,
                    revised_prompt: crate::RevisedPromptDisposition::NotRequested,
                    meta: crate::ProviderImageMetadata::NotEmitted,
                }],
                StopReason::EndTurn,
                Usage::default(),
            ))
        }

        fn provider(&self) -> crate::provider::Provider {
            crate::provider::Provider::Other
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
                crate::event::AgentEvent::RunFailed { error_report, .. } => {
                    saw_run_failed = true;
                    saw_typed_post_tool_failure = error_report.class
                        == crate::event::AgentErrorClass::Hook
                        && matches!(
                            error_report.reason.as_ref(),
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
            .expect("snapshot projects")
            .expect("test turn-state handle should expose a snapshot");
        assert_eq!(snapshot.turn_phase, crate::TurnPhase::Failed);
        assert_eq!(
            snapshot.terminal_outcome,
            crate::TurnTerminalOutcome::Failed,
            "post-tool denial should leave the canonical turn snapshot failed"
        );
    }

    #[tokio::test]
    async fn direct_assistant_image_response_emits_typed_event() {
        let client = Arc::new(DirectImageClient);
        let tools = Arc::new(NoTools);
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .build_standalone(client, tools, Arc::new(NoopStore))
            .await;
        agent.config.max_turns = Some(1);

        let (tx, mut rx) = mpsc::channel::<crate::event::AgentEvent>(32);
        agent
            .run_with_events("prompt".to_string().into(), tx)
            .await
            .unwrap();

        assert!(agent.session().messages().iter().any(|message| matches!(
            message,
            Message::BlockAssistant(blocks)
                if blocks
                    .blocks
                    .iter()
                    .any(|block| matches!(block, AssistantBlock::Image { .. }))
        )));

        let mut image_events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let crate::event::AgentEvent::AssistantImageAppended { image } = event {
                image_events.push(image);
            }
        }
        assert_eq!(image_events.len(), 1);
        let image = &image_events[0];
        assert_eq!(image.blob_ref.blob_id.as_str(), "direct-image-blob");
        assert_eq!(image.media_type.as_str(), "image/png");
        assert_eq!((image.width, image.height), (32, 24));
    }

    #[tokio::test]
    async fn tool_image_session_effects_preserve_tool_result_adjacency_and_emit_typed_event() {
        let client = Arc::new(ImageEffectClient {
            call_count: Mutex::new(0),
        });
        let tools = Arc::new(ImageEffectDispatcher::new());
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .build_standalone(client, tools, Arc::new(NoopStore))
            .await;
        agent.config.max_turns = Some(2);

        let (tx, mut rx) = mpsc::channel::<crate::event::AgentEvent>(32);
        let result = agent
            .run_with_events("prompt".to_string().into(), tx)
            .await
            .unwrap();
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

        let image_event_index = {
            let mut events = Vec::new();
            while let Ok(event) = rx.try_recv() {
                events.push(event);
            }
            let tool_result_event_index = events
                .iter()
                .position(|event| {
                    matches!(event, crate::event::AgentEvent::ToolResultReceived { .. })
                })
                .expect("tool result event should be emitted");
            let image_event_index = events
                .iter()
                .position(|event| {
                    matches!(
                        event,
                        crate::event::AgentEvent::AssistantImageAppended { .. }
                    )
                })
                .expect("assistant image append event should be emitted");
            assert!(
                image_event_index > tool_result_event_index,
                "assistant image append event should follow tool-result events"
            );
            match &events[image_event_index] {
                crate::event::AgentEvent::AssistantImageAppended { image } => {
                    assert_eq!(image.blob_ref.blob_id.as_str(), "image-blob");
                    assert_eq!(image.media_type.as_str(), "image/png");
                    assert_eq!(image.width, 1);
                    assert_eq!(image.height, 1);
                }
                other => panic!("expected assistant image event, got {other:?}"),
            }
            image_event_index
        };
        assert!(image_event_index > 0);
    }

    #[tokio::test]
    async fn empty_completion_after_successful_tool_effect_completes_turn() {
        let client = Arc::new(EmptyAfterImageEffectClient {
            call_count: Mutex::new(0),
        });
        let tools = Arc::new(ImageEffectDispatcher::new());
        let turn_handle =
            Arc::new(crate::agent::test_turn_state_handle::TestTurnStateHandle::new());
        let mut session = crate::Session::new();
        session
            .set_build_state(crate::SessionBuildState::default())
            .expect("test session build state should serialize");
        let mut agent = AgentBuilder::new()
            .resume_session(session)
            .with_turn_state_handle(turn_handle.clone())
            .build_standalone(client.clone(), tools, Arc::new(NoopStore))
            .await;
        agent.config.max_turns = Some(2);

        let result = agent
            .run("prompt".to_string().into())
            .await
            .expect("empty final completion after a successful effectful tool call should succeed");

        assert_eq!(result.text, "");
        assert_eq!(
            *client.call_count.lock().unwrap(),
            2,
            "empty terminal follow-up should not be retried after successful tool output"
        );
        assert!(agent.session().messages().iter().any(|message| matches!(
            message,
            Message::BlockAssistant(blocks)
                if blocks
                    .blocks
                    .iter()
                    .any(|block| matches!(block, AssistantBlock::Image { .. }))
        )));
        assert_eq!(
            turn_handle.run_completed_effect_count(),
            1,
            "the canonical turn authority should complete, not fail"
        );
        assert_eq!(turn_handle.run_failed_effect_count(), 0);
    }

    #[tokio::test]
    async fn control_tool_visibility_policy_staging_requires_generated_authority() {
        let client = Arc::new(ControlPlaneVisibilityClient::new());
        let tools = Arc::new(PlaneAwareToolDispatcher::new());
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .build_standalone(client.clone(), tools.clone(), Arc::new(NoopStore))
            .await;

        let err = agent
            .stage_external_tool_filter(ToolFilter::Deny(
                ["secret".to_string()].into_iter().collect(),
            ))
            .expect_err("control/session filter policy cannot be staged without authority");
        assert_generated_visibility_authority_required(err);
        assert!(client.seen_tools().is_empty());

        let dispatched = tools.dispatched();
        assert_eq!(
            dispatched,
            Vec::<String>::new(),
            "failed policy staging must not invoke control-plane dispatch"
        );
    }

    #[tokio::test]
    async fn deferred_tool_load_effect_fails_closed_without_generated_visibility_authority() {
        let client = Arc::new(DeferredLoadVisibilityClient::new());
        let tools = Arc::new(DeferredLoadDispatcher::new());
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .build_standalone(client.clone(), tools, Arc::new(NoopStore))
            .await;
        agent.config.max_turns = Some(2);

        let err = agent
            .run("prompt".to_string().into())
            .await
            .expect_err("deferred load effects require generated visibility authority");
        assert_generated_visibility_authority_required(err);

        let seen = client.seen_tools();
        assert_eq!(
            seen[0],
            vec!["tool_catalog_load".to_string()],
            "deferred tools should stay hidden before the load effect is applied"
        );
        assert_eq!(
            seen.len(),
            1,
            "failed deferred visibility effects must not continue to a follow-up model request"
        );
    }

    #[tokio::test]
    async fn deferred_catalog_delta_events_require_generated_visibility_authority() {
        let client = Arc::new(DeferredLoadVisibilityClient::new());
        let tools = Arc::new(DeferredLoadDispatcher::new());
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .build_standalone(client, tools, Arc::new(NoopStore))
            .await;
        agent.config.max_turns = Some(2);

        let (tx, mut rx) = mpsc::channel::<crate::event::AgentEvent>(128);
        let err = agent
            .run_with_events("prompt".to_string().into(), tx)
            .await
            .expect_err("deferred catalog delta requires generated visibility authority");
        assert_generated_visibility_authority_required(err);

        while rx.try_recv().is_ok() {}
        if let Some(visibility_state) = agent
            .session()
            .tool_visibility_state()
            .expect("session visibility state should decode")
        {
            assert!(
                !visibility_state
                    .staged_requested_deferred_names
                    .contains("deferred_tool"),
                "failed deferred visibility effects must not stage deferred_tool"
            );
            assert!(
                !visibility_state
                    .active_requested_deferred_names
                    .contains("deferred_tool"),
                "failed deferred visibility effects must not activate deferred_tool"
            );
        }
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
    async fn run_loop_filter_boundary_requires_generated_visibility_authority() {
        let client = Arc::new(SingleTurnVisibilityClient::new());
        let tools = Arc::new(FullToolDispatcher::new(&["visible", "secret"]));
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .build_standalone(client.clone(), tools, Arc::new(NoopStore))
            .await;
        let err = agent
            .stage_external_tool_filter(ToolFilter::Deny(
                ["secret".to_string()].into_iter().collect(),
            ))
            .expect_err("boundary filter staging requires generated visibility authority");
        assert_generated_visibility_authority_required(err);
        assert!(client.seen_tools().is_empty());
        assert!(
            agent
                .session()
                .tool_visibility_state()
                .expect("visibility state should decode")
                .is_none(),
            "failed staging must not persist committed visibility state"
        );
    }

    /// Dispatcher whose tool catalog carries callback provenance so that
    /// filter staging through a generated visibility owner can mint valid
    /// identity witnesses (the generated owner requires filter witnesses).
    struct ProvenancedToolDispatcher {
        tools: Arc<[Arc<ToolDef>]>,
    }

    impl ProvenancedToolDispatcher {
        fn new(tool_names: &[&str]) -> Self {
            let tools = tool_names
                .iter()
                .map(|name| {
                    Arc::new(ToolDef {
                        name: (*name).into(),
                        description: format!("{name} tool"),
                        input_schema: serde_json::json!({ "type": "object" }),
                        provenance: Some(crate::ToolProvenance {
                            kind: crate::ToolSourceKind::Callback,
                            source_id: (*name).into(),
                        }),
                    })
                })
                .collect::<Vec<_>>()
                .into();
            Self { tools }
        }
    }

    #[async_trait]
    impl AgentToolDispatcher for ProvenancedToolDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::clone(&self.tools)
        }

        async fn dispatch(
            &self,
            call: ToolCallView<'_>,
        ) -> Result<crate::ops::ToolDispatchOutcome, ToolError> {
            Ok(ToolResult::new(
                call.id.to_string(),
                format!("dispatched {}", call.name),
                false,
            )
            .into())
        }
    }

    #[tokio::test]
    async fn generated_authority_stage_persists_projection_and_clears_legacy_only_on_success() {
        let client = Arc::new(VisibilityRecordingLlmClient::new());
        let tools = Arc::new(ProvenancedToolDispatcher::new(&["visible", "secret"]));
        let visibility_owner: Arc<dyn crate::tool_scope::ToolVisibilityOwner> =
            Arc::new(crate::tool_scope::GeneratedTestToolVisibilityOwner::new());
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .with_tool_visibility_owner(
                crate::tool_scope::generated_test_tool_visibility_owner_from(visibility_owner),
            )
            .build_standalone(client, tools, Arc::new(NoopStore))
            .await;

        // Seed the legacy fallback recovery source. The canonical persist must
        // remove it ONLY after the durable projection write commits.
        agent.session_mut().set_metadata(
            EXTERNAL_TOOL_FILTER_METADATA_KEY,
            serde_json::to_value(ToolFilter::Deny(
                ["secret".to_string()].into_iter().collect(),
            ))
            .expect("legacy filter should serialize"),
        );

        agent
            .stage_external_tool_filter(ToolFilter::Deny(
                ["secret".to_string()].into_iter().collect(),
            ))
            .expect("generated authority staging must persist canonical visibility");

        assert!(
            agent
                .session()
                .tool_visibility_state()
                .expect("visibility state should decode")
                .is_some(),
            "successful generated-authority staging must persist the canonical visibility projection"
        );
        assert!(
            !agent
                .session()
                .metadata()
                .contains_key(EXTERNAL_TOOL_FILTER_METADATA_KEY),
            "successful canonical persist must clear the legacy fallback source"
        );
    }

    #[tokio::test]
    async fn run_loop_fails_closed_on_tool_scope_boundary_failure() {
        let client = Arc::new(SingleTurnVisibilityClient::new());
        let tools = Arc::new(FullToolDispatcher::new(&["visible", "secret"]));
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .build_standalone(client.clone(), tools, Arc::new(NoopStore))
            .await;
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
                    notice.body.clone()
                }
                _ => None,
            })
            .collect();
        assert_eq!(notices.len(), 1);
    }

    #[tokio::test]
    async fn standalone_builder_rejects_legacy_external_filter_without_authority() {
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

        let result = with_test_turn_state_handle(AgentBuilder::new())
            .resume_session(session)
            .try_build_standalone(client.clone(), tools, Arc::new(NoopStore))
            .await;
        let err = match result {
            Ok(_) => panic!("legacy visibility metadata needs generated visibility authority"),
            Err(err) => err,
        };

        match err {
            crate::agent::builder::AgentBuildPolicyError::ToolVisibilityRestore { message } => {
                assert_generated_visibility_authority_required(message);
            }
            other => panic!("unexpected build error: {other}"),
        }
        assert!(client.seen_tools().is_empty());
    }

    #[tokio::test]
    async fn standalone_builder_rejects_unknown_legacy_filter_without_authority() {
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

        let result = with_test_turn_state_handle(AgentBuilder::new())
            .resume_session(session)
            .try_build_standalone(client.clone(), tools, Arc::new(NoopStore))
            .await;
        let err = match result {
            Ok(_) => panic!("legacy visibility metadata needs generated visibility authority"),
            Err(err) => err,
        };

        match err {
            crate::agent::builder::AgentBuildPolicyError::ToolVisibilityRestore { message } => {
                assert_generated_visibility_authority_required(message);
            }
            other => panic!("unexpected build error: {other}"),
        }
        assert!(client.seen_tools().is_empty());
    }

    #[tokio::test]
    async fn standalone_builder_rejects_legacy_inherited_filter_without_authority() {
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

        let result = with_test_turn_state_handle(AgentBuilder::new())
            .resume_session(session)
            .try_build_standalone(client.clone(), tools, Arc::new(NoopStore))
            .await;
        let err = match result {
            Ok(_) => panic!("legacy inherited visibility metadata needs generated authority"),
            Err(err) => err,
        };

        match err {
            crate::agent::builder::AgentBuildPolicyError::ToolVisibilityRestore { message } => {
                assert_generated_visibility_authority_required(message);
            }
            other => panic!("unexpected build error: {other}"),
        }
        assert!(client.seen_tools().is_empty());
    }

    struct FatalLlmClient;

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentLlmClient for FatalLlmClient {
        async fn stream_response(
            &self,
            _messages: &[Message],
            _tools: &[Arc<ToolDef>],
            _max_tokens: u32,
            _temperature: Option<f32>,
            _provider_params: Option<&crate::lifecycle::run_primitive::ProviderParamsOverride>,
        ) -> Result<super::LlmStreamResult, AgentError> {
            Err(AgentError::llm(
                "mock",
                crate::error::LlmFailureReason::AuthError,
                "display-only LLM failure",
            ))
        }

        fn provider(&self) -> crate::provider::Provider {
            crate::provider::Provider::Other
        }

        fn model(&self) -> &'static str {
            "mock-model"
        }
    }

    struct InvalidModelLlmClient;

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentLlmClient for InvalidModelLlmClient {
        async fn stream_response(
            &self,
            _messages: &[Message],
            _tools: &[Arc<ToolDef>],
            _max_tokens: u32,
            _temperature: Option<f32>,
            _provider_params: Option<&crate::lifecycle::run_primitive::ProviderParamsOverride>,
        ) -> Result<super::LlmStreamResult, AgentError> {
            Err(AgentError::llm(
                "mock",
                crate::error::LlmFailureReason::InvalidModel("retired-model".to_string()),
                "requested model is no longer available",
            ))
        }

        fn provider(&self) -> crate::provider::Provider {
            crate::provider::Provider::Other
        }

        fn model(&self) -> &'static str {
            "retired-model"
        }
    }

    struct ReasoningOnlyLlmClient;

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentLlmClient for ReasoningOnlyLlmClient {
        async fn stream_response(
            &self,
            _messages: &[Message],
            _tools: &[Arc<ToolDef>],
            _max_tokens: u32,
            _temperature: Option<f32>,
            _provider_params: Option<&crate::lifecycle::run_primitive::ProviderParamsOverride>,
        ) -> Result<super::LlmStreamResult, AgentError> {
            Ok(super::LlmStreamResult::new(
                vec![AssistantBlock::Reasoning {
                    text: String::new(),
                    meta: Some(Box::new(crate::types::ProviderMeta::OpenAi {
                        id: "rs_reasoning_only".to_string(),
                        encrypted_content: Some("encrypted-reasoning".to_string()),
                        phase: None,
                        response_id: None,
                    })),
                }],
                StopReason::EndTurn,
                Usage {
                    input_tokens: 11,
                    output_tokens: 7,
                    ..Usage::default()
                },
            ))
        }

        fn provider(&self) -> crate::provider::Provider {
            crate::provider::Provider::Other
        }

        fn model(&self) -> &'static str {
            "mock-model"
        }
    }

    #[tokio::test]
    async fn reasoning_only_llm_response_completes_silent_turn() {
        let mut agent = build_agent(Arc::new(ReasoningOnlyLlmClient)).await;
        agent.config.max_turns = Some(1);

        let result = agent
            .run("Use a tool".to_string().into())
            .await
            .expect("reasoning-only LLM response should commit as a silent completion");

        assert_eq!(
            result.text, "",
            "reasoning-only completions have no user-visible assistant text"
        );
        assert_eq!(result.usage.output_tokens, 7);

        let snapshot = agent
            .execution_snapshot()
            .expect("snapshot projects")
            .expect("test turn-state handle should expose a snapshot");
        assert_eq!(snapshot.turn_phase, crate::TurnPhase::Completed);
        assert_eq!(
            snapshot.terminal_outcome,
            crate::TurnTerminalOutcome::Completed
        );
        assert_eq!(snapshot.terminal_cause_kind, None);
    }

    #[tokio::test]
    async fn generic_fatal_llm_failure_uses_machine_terminal_cause_and_preserves_detail() {
        let mut agent = build_agent(Arc::new(FatalLlmClient)).await;
        agent.config.max_turns = Some(1);

        let err = agent
            .run("prompt".to_string().into())
            .await
            .expect_err("fatal LLM failure should terminalize the run");

        match err {
            AgentError::TerminalFailure {
                outcome,
                cause_kind,
                message,
            } => {
                assert_eq!(outcome, crate::TurnTerminalOutcome::Failed);
                assert_eq!(cause_kind, crate::TurnTerminalCauseKind::LlmFailure);
                assert_eq!(
                    message,
                    format!(
                        "{}: LLM error (mock): display-only LLM failure",
                        crate::TurnTerminalCauseKind::LlmFailure.default_message(outcome)
                    )
                );
            }
            other => panic!("expected machine-owned terminal failure, got {other:?}"),
        }

        let snapshot = agent
            .execution_snapshot()
            .expect("snapshot projects")
            .expect("test turn-state handle should expose a snapshot");
        assert_eq!(snapshot.turn_phase, crate::TurnPhase::Failed);
        assert_eq!(
            snapshot.terminal_outcome,
            crate::TurnTerminalOutcome::Failed
        );
        assert_eq!(
            snapshot.terminal_cause_kind,
            Some(crate::TurnTerminalCauseKind::LlmFailure)
        );
    }

    #[tokio::test]
    async fn runtime_failure_records_exact_machine_terminal_witness() {
        let mut agent = build_agent(Arc::new(FatalLlmClient)).await;
        agent.config.max_turns = Some(1);
        agent.set_runtime_execution_kind(Some(crate::lifecycle::RuntimeExecutionKind::ContentTurn));

        let error = agent
            .run("runtime prompt".to_string().into())
            .await
            .expect_err("fatal runtime failure should remain the public error");
        let durable_run_failed_detail = error.to_string();
        let witness = agent
            .take_runtime_terminal_failure_witness()
            .expect("runtime witness projection should succeed")
            .expect("admitted runtime failure should carry an exact witness");
        assert_eq!(witness.outcome, Some(crate::TurnTerminalOutcome::Failed));
        assert_eq!(witness.kind, crate::TurnTerminalCauseKind::LlmFailure);
        assert!(witness.terminal);
        assert_eq!(witness.provider.as_deref(), Some("mock"));
        assert_eq!(witness.model, None);
        assert_eq!(witness.retryable, Some(false));
        assert!(
            durable_run_failed_detail.contains("display-only LLM failure"),
            "runtime failure must retain the concrete provider diagnostic"
        );
        assert_eq!(
            witness.detail.as_deref(),
            Some(durable_run_failed_detail.as_str()),
            "the runtime witness must match the exact RunFailed error-report detail"
        );
    }

    #[tokio::test]
    async fn runtime_invalid_model_witness_preserves_model_metadata() {
        let mut agent = build_agent(Arc::new(InvalidModelLlmClient)).await;
        agent.config.max_turns = Some(1);
        agent.set_runtime_execution_kind(Some(crate::lifecycle::RuntimeExecutionKind::ContentTurn));

        agent
            .run("runtime invalid model".to_string().into())
            .await
            .expect_err("invalid model should terminalize the runtime turn");
        let witness = agent
            .take_runtime_terminal_failure_witness()
            .expect("invalid-model witness projection should succeed")
            .expect("invalid-model runtime failure should carry an exact witness");

        assert_eq!(witness.kind, crate::TurnTerminalCauseKind::LlmFailure);
        assert_eq!(witness.outcome, Some(crate::TurnTerminalOutcome::Failed));
        assert_eq!(witness.provider.as_deref(), Some("mock"));
        assert_eq!(witness.model.as_deref(), Some("retired-model"));
        assert_eq!(witness.retryable, Some(false));
    }

    #[tokio::test]
    async fn runtime_preflight_failure_cannot_reuse_prior_terminal_witness() {
        let mut agent = build_agent(Arc::new(FatalLlmClient)).await;
        agent.config.max_turns = Some(1);
        agent.set_runtime_execution_kind(Some(crate::lifecycle::RuntimeExecutionKind::ContentTurn));
        agent
            .run("first runtime prompt".to_string().into())
            .await
            .expect_err("first runtime turn should terminalize");
        assert!(
            agent
                .take_runtime_terminal_failure_witness()
                .expect("first witness projection")
                .is_some()
        );

        // `run_loop` checks the resolved turn cap before it starts a new turn
        // machine run. The previous hard-terminal snapshot therefore remains
        // visible, exactly reproducing the stale-post-snapshot hazard.
        agent.config.max_turns = None;
        agent.set_runtime_execution_kind(Some(crate::lifecycle::RuntimeExecutionKind::ContentTurn));
        let error = agent
            .run("second runtime prompt".to_string().into())
            .await
            .expect_err("missing resolved turn cap should fail before run admission");
        assert!(matches!(error, AgentError::InternalError(_)));
        assert!(
            agent
                .take_runtime_terminal_failure_witness()
                .expect("second witness projection")
                .is_none(),
            "a completed future without a newly admitted machine run must not reuse prior terminal truth"
        );
    }

    #[tokio::test]
    async fn display_failure_text_does_not_classify_after_machine_terminalization() {
        use crate::TurnExecutionInput;

        let mut agent = build_agent(Arc::new(StaticLlmClient)).await;
        let run_id = crate::lifecycle::RunId::new();
        agent
            .apply_turn_input(TurnExecutionInput::StartConversationRun {
                run_id: run_id.clone(),
                primitive_kind:
                    crate::turn_execution_authority::TurnPrimitiveKind::ConversationTurn,
                admitted_content_shape: crate::turn_execution_authority::ContentShape::Conversation,
                vision_enabled: false,
                image_tool_results_enabled: false,
                max_extraction_retries: 0,
            })
            .expect("start conversation run should apply");

        agent
            .apply_turn_input(TurnExecutionInput::FatalFailure {
                run_id,
                failure: TurnFailureSource::new(
                    TurnFailureSourceKind::Llm,
                    "misleading hook-denied display text",
                ),
            })
            .expect("fatal failure with typed source should apply");

        let err = agent
            .build_result(0, 0)
            .await
            .expect_err("fatal terminalization should return terminal failure");

        match err {
            AgentError::TerminalFailure {
                outcome,
                cause_kind,
                message,
            } => {
                assert_eq!(outcome, crate::TurnTerminalOutcome::Failed);
                assert_eq!(
                    cause_kind,
                    crate::TurnTerminalCauseKind::LlmFailure,
                    "display text must not classify terminal cause"
                );
                assert_eq!(
                    message,
                    crate::TurnTerminalCauseKind::LlmFailure
                        .default_message(outcome)
                        .to_string(),
                    "display text must not become terminal failure authority"
                );
            }
            other => panic!("expected terminal failure, got {other:?}"),
        }

        let snapshot = agent
            .execution_snapshot()
            .expect("snapshot projects")
            .expect("test turn-state handle should expose a snapshot");
        assert_eq!(
            snapshot.terminal_cause_kind,
            Some(crate::TurnTerminalCauseKind::LlmFailure)
        );
    }

    #[tokio::test]
    async fn hard_failure_without_pending_diagnostic_uses_machine_terminal_cause() {
        let mut agent = build_agent(Arc::new(StaticLlmClient)).await;
        agent.config.max_turns = Some(0);

        let err = agent
            .run("prompt".to_string().into())
            .await
            .expect_err("turn limit should terminalize the run");

        match err {
            AgentError::TerminalFailure {
                outcome,
                cause_kind,
                message,
            } => {
                assert_eq!(outcome, crate::TurnTerminalOutcome::Failed);
                assert_eq!(cause_kind, crate::TurnTerminalCauseKind::TurnLimitReached);
                assert!(message.contains("turn limit reached"));
            }
            other => panic!("expected machine-owned turn-limit terminal failure, got {other:?}"),
        }

        let snapshot = agent
            .execution_snapshot()
            .expect("snapshot projects")
            .expect("test turn-state handle should expose a snapshot");
        assert_eq!(
            snapshot.terminal_cause_kind,
            Some(crate::TurnTerminalCauseKind::TurnLimitReached)
        );
    }

    /// Dogma row #272: the agent-loop turn cap default is resolved at the
    /// build/composition seam, not privately invented inside `run_loop`.
    ///
    /// With `config.max_turns` left unset, the builder resolves the named
    /// `DEFAULT_MAX_TURNS` owner into the agent's config, so the resolved value
    /// is assertable on the built agent and the loop reads it rather than
    /// re-deriving the default.
    #[tokio::test]
    async fn max_turns_default_is_resolved_at_build_seam_not_in_loop() {
        // `build_agent` uses `AgentBuilder::new()` whose default config leaves
        // `max_turns = None`; the build seam must resolve it.
        let agent = build_agent(Arc::new(StaticLlmClient)).await;
        assert_eq!(
            agent.config.max_turns,
            Some(crate::config::DEFAULT_MAX_TURNS),
            "build seam must resolve the turn-cap default into the agent config"
        );
    }

    /// Dogma row #272: the loop terminalizes via the machine-owned
    /// `TurnLimitReached` at the build-resolved cap value, with no literal
    /// fallback default invented inside `run_loop`.
    #[tokio::test]
    async fn run_loop_terminalizes_at_build_resolved_turn_cap() {
        let mut agent = build_agent(Arc::new(StaticLlmClient)).await;
        // The build seam resolved a non-`None` cap; force it to a tiny value so
        // the loop trips the turn limit immediately, proving the loop reads the
        // resolved value rather than a privately-invented default.
        agent.config.max_turns = Some(0);

        let err = agent
            .run("prompt".to_string().into())
            .await
            .expect_err("turn limit should terminalize the run");
        match err {
            AgentError::TerminalFailure { cause_kind, .. } => {
                assert_eq!(cause_kind, crate::TurnTerminalCauseKind::TurnLimitReached);
            }
            other => panic!("expected machine-owned turn-limit terminal failure, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn run_loop_consumes_generated_prestarted_turn_without_restarting() {
        use crate::agent::test_turn_state_handle::TestTurnStateHandle;
        use crate::handles::TurnStateHandle;

        let turn_handle = Arc::new(TestTurnStateHandle::new());
        let run_id = crate::lifecycle::RunId::new();
        turn_handle
            .start_conversation_run(
                run_id,
                TurnPrimitiveKind::ConversationTurn,
                ContentShape::ConversationAndContext,
                false,
                false,
                0,
            )
            .expect("generated start should prepare ApplyingPrimitive");

        let mut agent = AgentBuilder::new()
            .with_turn_state_handle(turn_handle.clone())
            .with_runtime_execution_kind_for_test(
                crate::lifecycle::RuntimeExecutionKind::ContentTurn,
            )
            .build_standalone(
                Arc::new(StaticLlmClient),
                Arc::new(NoTools),
                Arc::new(NoopStore),
            )
            .await;

        agent
            .run("hello".to_string().into())
            .await
            .expect("prestarted generated turn should be consumed");

        let snapshot = agent
            .execution_snapshot()
            .expect("snapshot projects")
            .expect("test turn-state handle should expose a snapshot");
        assert_eq!(
            snapshot.admitted_content_shape,
            Some(ContentShape::ConversationAndContext)
        );
    }

    #[tokio::test]
    async fn fatal_failure_unknown_input_source_fails_before_machine_apply() {
        use crate::TurnExecutionInput;
        use crate::agent::test_turn_state_handle::TestTurnStateHandle;

        let turn_handle = Arc::new(TestTurnStateHandle::new());
        let mut agent = AgentBuilder::new()
            .with_turn_state_handle(turn_handle.clone())
            .with_runtime_execution_kind_for_test(
                crate::lifecycle::RuntimeExecutionKind::ContentTurn,
            )
            .build_standalone(
                Arc::new(FatalLlmClient),
                Arc::new(NoTools),
                Arc::new(NoopStore),
            )
            .await;

        let run_id = crate::lifecycle::RunId::new();
        agent
            .apply_turn_input(TurnExecutionInput::StartConversationRun {
                run_id: run_id.clone(),
                primitive_kind:
                    crate::turn_execution_authority::TurnPrimitiveKind::ConversationTurn,
                admitted_content_shape: crate::turn_execution_authority::ContentShape::Conversation,
                vision_enabled: false,
                image_tool_results_enabled: false,
                max_extraction_retries: 0,
            })
            .expect("start conversation run should apply");

        let err = agent
            .apply_turn_input(TurnExecutionInput::FatalFailure {
                run_id,
                failure: TurnFailureSource::new(
                    TurnFailureSourceKind::Unknown,
                    "display text must not classify unknown terminal source",
                ),
            })
            .expect_err("unknown failure source should fail before machine apply");

        match err {
            AgentError::InternalError(message) => {
                assert!(
                    message.contains("unknown generated-authority failure source"),
                    "unexpected unknown-source invariant error: {message}"
                );
                assert!(
                    message.contains("FatalFailure"),
                    "invariant error should name the rejected input: {message}"
                );
            }
            AgentError::TerminalFailure {
                cause_kind,
                message,
                ..
            } => panic!(
                "core shell must not publish Unknown terminal semantics, got {cause_kind:?}: {message}"
            ),
            other => panic!("expected fail-closed invariant error, got {other:?}"),
        }

        let snapshot = agent
            .execution_snapshot()
            .expect("snapshot projects")
            .expect("test turn-state handle should expose a snapshot");
        assert_eq!(
            snapshot.turn_phase,
            crate::TurnPhase::ApplyingPrimitive,
            "fixture must prove the ambiguous input did not reach the machine"
        );
        assert_eq!(
            snapshot.terminal_cause_kind, None,
            "fixture must prove the ambiguous input did not store a terminal cause"
        );
    }

    #[tokio::test]
    async fn build_result_cancelled_terminal_does_not_return_success() {
        let mut agent = build_agent(Arc::new(StaticLlmClient)).await;

        {
            let handle = agent
                .turn_state_handle
                .as_deref()
                .expect("test agent should have a turn-state handle");
            let run_id = start_test_conversation_turn(handle);
            handle.cancel_now(run_id.clone()).unwrap();
            handle.cancellation_observed(run_id).unwrap();
        }

        let error = agent
            .build_result(1, 0)
            .await
            .expect_err("cancelled terminal outcome must not return RunResult success");

        assert!(matches!(error, AgentError::Cancelled));
        assert_eq!(AgentErrorClass::from(&error), AgentErrorClass::Cancelled);
    }

    #[tokio::test]
    async fn post_llm_boundary_cancel_does_not_return_success() {
        use crate::agent::{CancelAfterBoundaryCommand, CancelAfterBoundarySender};
        use crate::hooks::{
            HookEngine, HookEngineError, HookExecutionReport, HookInvocation, HookPoint,
        };

        struct RequestBoundaryCancelHook {
            sender: CancelAfterBoundarySender,
            expected_run_id: RunId,
        }

        #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
        #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
        impl HookEngine for RequestBoundaryCancelHook {
            async fn execute(
                &self,
                invocation: HookInvocation,
                _overrides: Option<&crate::config::HookRunOverrides>,
            ) -> Result<HookExecutionReport, HookEngineError> {
                if invocation.point == HookPoint::PostLlmResponse {
                    self.sender
                        .send(CancelAfterBoundaryCommand::for_run(
                            self.expected_run_id.clone(),
                        ))
                        .expect("agent must retain the cancel-after-boundary receiver");
                }
                Ok(HookExecutionReport::empty())
            }
        }

        let mut agent = build_agent(Arc::new(StaticLlmClient)).await;
        let expected_run_id = {
            let handle = agent
                .turn_state_handle
                .as_deref()
                .expect("test agent should have a turn-state handle");
            let run_id = RunId::new();
            handle
                .start_conversation_run(
                    run_id.clone(),
                    TurnPrimitiveKind::ConversationTurn,
                    ContentShape::Conversation,
                    false,
                    false,
                    0,
                )
                .expect("stage the exact run for run_loop to apply");
            run_id
        };
        agent.hook_engine = Some(Arc::new(RequestBoundaryCancelHook {
            sender: agent.cancel_after_boundary_handle(),
            expected_run_id,
        }));

        let (tx, mut rx) = mpsc::channel::<crate::event::AgentEvent>(32);
        let error = agent
            .run_with_events("prompt".to_string().into(), tx)
            .await
            .expect_err("boundary cancellation after a terminal LLM response must cancel the run");

        assert!(
            matches!(error, AgentError::Cancelled),
            "unexpected post-LLM boundary cancellation result: {error:?}"
        );
        assert_eq!(AgentErrorClass::from(&error), AgentErrorClass::Cancelled);

        let snapshot = agent
            .execution_snapshot()
            .expect("snapshot projects")
            .expect("test turn-state handle should expose a snapshot");
        assert_eq!(snapshot.turn_phase, crate::TurnPhase::Cancelled);
        assert_eq!(
            snapshot.terminal_outcome,
            crate::TurnTerminalOutcome::Cancelled
        );

        while let Ok(event) = rx.try_recv() {
            assert!(
                !matches!(event, crate::event::AgentEvent::TurnCompleted { .. }),
                "cancelled boundary must not emit TurnCompleted"
            );
            assert!(
                !matches!(event, crate::event::AgentEvent::RunCompleted { .. }),
                "cancelled boundary must not emit RunCompleted"
            );
        }
    }

    #[tokio::test]
    async fn typed_cancel_after_boundary_command_observed_once_at_boundary() {
        use crate::agent::CancelAfterBoundaryCommand;

        let mut agent = build_agent(Arc::new(StaticLlmClient)).await;

        // Drive the turn machine to a live boundary phase (CallingLlm) so the
        // observe seam is eligible to apply the cancel-after-boundary input.
        let run_id = {
            let handle = agent
                .turn_state_handle
                .as_deref()
                .expect("test agent should have a turn-state handle");
            start_test_conversation_turn(handle)
        };
        assert_eq!(
            agent.turn_phase().expect("turn phase"),
            crate::turn_execution_authority::TurnPhase::CallingLlm
        );
        assert!(
            !agent
                .turn_cancel_after_boundary()
                .expect("cancel-after-boundary flag"),
            "fixture must start with no boundary-cancel request observed"
        );

        // A surface requests boundary cancellation by sending the typed command
        // on a cloned producer handle — the raw AtomicBool carrier is gone.
        let sender = agent.cancel_after_boundary_handle();
        sender
            .send(CancelAfterBoundaryCommand::for_run(run_id.clone()))
            .expect("agent must retain the cancel-after-boundary receiver");

        agent
            .observe_cancel_after_boundary_request(&run_id)
            .expect("observe must apply the typed command at the boundary");

        assert!(
            agent
                .turn_cancel_after_boundary()
                .expect("cancel-after-boundary flag"),
            "the typed command must be observed once and applied to the machine"
        );

        // The edge was consumed: the receiver is drained, so a subsequent
        // observe with no fresh command is a pure no-op (observed at most once).
        assert!(
            matches!(
                agent.cancel_after_boundary_rx.try_recv(),
                Err(tokio::sync::mpsc::error::TryRecvError::Empty)
            ),
            "the typed command must be drained exactly once, not left re-observable"
        );
        agent
            .observe_cancel_after_boundary_request(&run_id)
            .expect("a second observe with no fresh command must be a no-op");
    }

    #[tokio::test]
    async fn typed_cancel_after_boundary_discards_stale_run_command() {
        use crate::agent::CancelAfterBoundaryCommand;

        let mut agent = build_agent(Arc::new(StaticLlmClient)).await;
        let active_run_id = {
            let handle = agent
                .turn_state_handle
                .as_deref()
                .expect("test agent should have a turn-state handle");
            start_test_conversation_turn(handle)
        };
        let stale_run_id = RunId::new();
        assert_ne!(stale_run_id, active_run_id);

        agent
            .cancel_after_boundary_handle()
            .send(CancelAfterBoundaryCommand::for_run(stale_run_id))
            .expect("agent must retain the cancel-after-boundary receiver");
        agent
            .observe_cancel_after_boundary_request(&active_run_id)
            .expect("stale command observation must be a benign no-op");

        assert!(
            !agent
                .turn_cancel_after_boundary()
                .expect("cancel-after-boundary flag"),
            "a command for an old run must not cancel its successor"
        );
        assert!(matches!(
            agent.cancel_after_boundary_rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn build_result_missing_terminal_outcome_fails_closed() {
        let mut agent = build_agent(Arc::new(StaticLlmClient)).await;

        let error = agent
            .build_result(0, 0)
            .await
            .expect_err("missing terminal outcome must not return RunResult success");

        match &error {
            AgentError::InternalError(message) => assert!(
                message.contains("without a machine terminal outcome"),
                "unexpected invariant error message: {message}"
            ),
            other => panic!("expected InternalError for missing terminal outcome, got {other:?}"),
        }
        assert_eq!(AgentErrorClass::from(&error), AgentErrorClass::Internal);
    }

    #[tokio::test]
    async fn build_result_hard_failure_remains_typed_terminal_failure() {
        let mut agent = build_agent(Arc::new(StaticLlmClient)).await;

        {
            let handle = agent
                .turn_state_handle
                .as_deref()
                .expect("test agent should have a turn-state handle");
            let run_id = start_test_conversation_turn(handle);
            handle
                .fatal_failure(
                    run_id,
                    TurnFailureSource::new(
                        TurnFailureSourceKind::Llm,
                        "machine-owned LLM terminal cause",
                    ),
                )
                .unwrap();
        }

        let error = agent
            .build_result(1, 0)
            .await
            .expect_err("hard failure terminal outcome must remain an error");

        match error {
            AgentError::TerminalFailure {
                outcome,
                cause_kind,
                message,
            } => {
                assert_eq!(outcome, TurnTerminalOutcome::Failed);
                assert_eq!(cause_kind, crate::TurnTerminalCauseKind::LlmFailure);
                assert_eq!(
                    message,
                    crate::TurnTerminalCauseKind::LlmFailure
                        .default_message(outcome)
                        .to_string()
                );
            }
            other => panic!("expected typed TerminalFailure, got {other:?}"),
        }
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

        fn provider(&self) -> crate::provider::Provider {
            crate::provider::Provider::Other
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
        let result = agent.run_loop(None).await.expect(
            "token budget exhaustion must route through BudgetExhausted (Success), \
             not escape as raw AgentError",
        );
        assert_eq!(
            result.terminal_cause_kind,
            Some(crate::TurnTerminalCauseKind::BudgetExhausted)
        );
        assert_eq!(
            agent.state().expect("loop state projects"),
            LoopState::Completed
        );
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

        let result = agent.run_loop(None).await.expect(
            "tool-call budget exhaustion must route through BudgetExhausted (Success), \
             not escape as raw AgentError",
        );
        assert_eq!(
            result.terminal_cause_kind,
            Some(crate::TurnTerminalCauseKind::BudgetExhausted)
        );
        assert_eq!(
            agent.state().expect("loop state projects"),
            LoopState::Completed
        );
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

        fn provider(&self) -> crate::provider::Provider {
            crate::provider::Provider::Other
        }

        fn model(&self) -> &'static str {
            "mock-model"
        }
    }

    struct FallbackActivatingClient {
        registry: Arc<crate::ModelRegistry>,
        active: std::sync::atomic::AtomicUsize,
        fallback_proposals: std::sync::atomic::AtomicUsize,
        fallback_commits: std::sync::atomic::AtomicUsize,
        fail_backup: std::sync::atomic::AtomicBool,
        fail_post_activation_identity_read_once: std::sync::atomic::AtomicBool,
        seen_tools: Mutex<Vec<Vec<String>>>,
        seen_notice_bodies: Mutex<Vec<Vec<String>>>,
        seen_max_tokens: Mutex<Vec<u32>>,
        seen_provider_params:
            Mutex<Vec<Option<crate::lifecycle::run_primitive::ProviderParamsOverride>>>,
        forged_client_capability_filter: Mutex<ToolFilter>,
        forged_client_max_output_tokens: std::sync::atomic::AtomicU32,
    }

    fn fallback_activation_test_registry() -> Arc<crate::ModelRegistry> {
        let mut config = crate::Config::default();
        for (model, vision, max_output_tokens) in [("primary", true, 8192), ("backup", false, 2048)]
        {
            config.models.custom.insert(
                model.to_string(),
                crate::config::CustomModelConfig {
                    provider: crate::Provider::OpenAI,
                    display_name: None,
                    context_window: Some(128_000),
                    max_output_tokens: Some(max_output_tokens),
                    vision: Some(vision),
                    web_search: Some(false),
                    call_timeout_secs: None,
                },
            );
        }
        let empty_catalog = crate::ModelCatalog {
            entries: &[],
            capabilities: &[],
            provider_defaults: &[],
            image_generation_models: &[],
            providers: &[],
            default_models: &[],
            image_generation_defaults: &[],
            global_default_model: "",
            provider_priority: &[],
        };
        Arc::new(
            crate::ModelRegistry::from_config(&config, empty_catalog)
                .expect("fallback activation test registry"),
        )
    }

    impl FallbackActivatingClient {
        fn new() -> Self {
            Self {
                registry: fallback_activation_test_registry(),
                active: std::sync::atomic::AtomicUsize::new(0),
                fallback_proposals: std::sync::atomic::AtomicUsize::new(0),
                fallback_commits: std::sync::atomic::AtomicUsize::new(0),
                fail_backup: std::sync::atomic::AtomicBool::new(false),
                fail_post_activation_identity_read_once: std::sync::atomic::AtomicBool::new(false),
                seen_tools: Mutex::new(Vec::new()),
                seen_notice_bodies: Mutex::new(Vec::new()),
                seen_max_tokens: Mutex::new(Vec::new()),
                seen_provider_params: Mutex::new(Vec::new()),
                forged_client_capability_filter: Mutex::new(ToolFilter::All),
                forged_client_max_output_tokens: std::sync::atomic::AtomicU32::new(u32::MAX),
            }
        }

        fn with_post_activation_identity_read_failure() -> Self {
            let client = Self::new();
            client
                .fail_post_activation_identity_read_once
                .store(true, std::sync::atomic::Ordering::SeqCst);
            client
        }

        fn with_terminal_backup_failure() -> Self {
            let client = Self::new();
            client
                .fail_backup
                .store(true, std::sync::atomic::Ordering::SeqCst);
            client
        }

        fn seen_tools(&self) -> Vec<Vec<String>> {
            self.seen_tools.lock().unwrap().clone()
        }

        fn seen_notice_bodies(&self) -> Vec<Vec<String>> {
            self.seen_notice_bodies.lock().unwrap().clone()
        }

        fn seen_max_tokens(&self) -> Vec<u32> {
            self.seen_max_tokens.lock().unwrap().clone()
        }

        fn seen_provider_params(
            &self,
        ) -> Vec<Option<crate::lifecycle::run_primitive::ProviderParamsOverride>> {
            self.seen_provider_params.lock().unwrap().clone()
        }

        fn fallback_proposals(&self) -> usize {
            self.fallback_proposals
                .load(std::sync::atomic::Ordering::SeqCst)
        }

        fn fallback_commits(&self) -> usize {
            self.fallback_commits
                .load(std::sync::atomic::Ordering::SeqCst)
        }

        fn registry(&self) -> Arc<crate::ModelRegistry> {
            Arc::clone(&self.registry)
        }

        fn forge_client_local_model_facts(&self) {
            *self.forged_client_capability_filter.lock().unwrap() = ToolFilter::All;
            self.forged_client_max_output_tokens
                .store(u32::MAX, std::sync::atomic::Ordering::SeqCst);
        }
    }

    struct RecordingModelRoutingHandle {
        commits: Arc<
            Mutex<
                Vec<(
                    crate::SessionLlmIdentity,
                    crate::SessionLlmIdentity,
                    u32,
                    crate::handles::StickyModelFallbackVisibilityPlan,
                )>,
            >,
        >,
        visibility_owner: Arc<dyn crate::ToolVisibilityOwner>,
    }

    struct RecordingStickyModelFallbackCommit {
        commits: Arc<
            Mutex<
                Vec<(
                    crate::SessionLlmIdentity,
                    crate::SessionLlmIdentity,
                    u32,
                    crate::handles::StickyModelFallbackVisibilityPlan,
                )>,
            >,
        >,
        visibility_owner: Arc<dyn crate::ToolVisibilityOwner>,
        previous_identity: crate::SessionLlmIdentity,
        target_identity: crate::SessionLlmIdentity,
        retry_attempt: u32,
        visibility_plan: crate::handles::StickyModelFallbackVisibilityPlan,
    }

    impl crate::handles::StickyModelFallbackMachineCommit for RecordingStickyModelFallbackCommit {
        fn commit(self: Box<Self>) -> Result<(), crate::handles::DslTransitionError> {
            self.commits.lock().unwrap().push((
                self.previous_identity,
                self.target_identity,
                self.retry_attempt,
                self.visibility_plan.clone(),
            ));
            self.visibility_owner
                .replace_visibility_state(self.visibility_plan.next_state)
                .map_err(|error| {
                    crate::handles::DslTransitionError::no_matching(
                        "RecordingStickyModelFallbackCommit::commit",
                        error.to_string(),
                    )
                })
        }
    }

    struct ImmediateStickyFallbackOperation;

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl crate::handles::StickyModelFallbackCommitOperation for ImmediateStickyFallbackOperation {
        async fn wait(&self) -> Result<(), crate::handles::StickyModelFallbackCommitError> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecordingStickyFallbackCoordinator {
        calls: std::sync::atomic::AtomicUsize,
        target_models: Mutex<Vec<String>>,
    }

    impl RecordingStickyFallbackCoordinator {
        fn calls(&self) -> usize {
            self.calls.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    impl crate::handles::StickyModelFallbackCommitCoordinator for RecordingStickyFallbackCoordinator {
        fn begin(
            &self,
            machine_commit: Box<dyn crate::handles::StickyModelFallbackMachineCommit>,
            control_delta: crate::handles::StickyModelFallbackControlDelta,
        ) -> Result<
            Arc<dyn crate::handles::StickyModelFallbackCommitOperation>,
            crate::handles::StickyModelFallbackCommitError,
        > {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.target_models
                .lock()
                .unwrap()
                .push(control_delta.target_identity().model.clone());
            machine_commit
                .commit()
                .map_err(crate::handles::StickyModelFallbackCommitError::MachineRejected)?;
            Ok(Arc::new(ImmediateStickyFallbackOperation))
        }
    }

    struct RejectingStickyFallbackCoordinator {
        error: crate::handles::StickyModelFallbackCommitError,
    }

    impl crate::handles::StickyModelFallbackCommitCoordinator for RejectingStickyFallbackCoordinator {
        fn begin(
            &self,
            _machine_commit: Box<dyn crate::handles::StickyModelFallbackMachineCommit>,
            _control_delta: crate::handles::StickyModelFallbackControlDelta,
        ) -> Result<
            Arc<dyn crate::handles::StickyModelFallbackCommitOperation>,
            crate::handles::StickyModelFallbackCommitError,
        > {
            Err(self.error.clone())
        }
    }

    #[derive(Default)]
    struct RecordingFallbackVisibilityOwner {
        state: std::sync::RwLock<crate::SessionToolVisibilityState>,
    }

    impl crate::ToolVisibilityOwner for RecordingFallbackVisibilityOwner {
        fn visibility_state(
            &self,
        ) -> Result<crate::SessionToolVisibilityState, crate::ToolScopeApplyError> {
            self.state
                .read()
                .map(|state| state.clone())
                .map_err(|_| crate::ToolScopeApplyError::LockPoisoned)
        }

        fn replace_visibility_state(
            &self,
            visibility_state: crate::SessionToolVisibilityState,
        ) -> Result<(), crate::ToolScopeApplyError> {
            *self
                .state
                .write()
                .map_err(|_| crate::ToolScopeApplyError::LockPoisoned)? = visibility_state;
            Ok(())
        }

        fn stage_persistent_filter(
            &self,
            _filter: crate::ToolFilter,
            _witnesses: std::collections::BTreeMap<crate::ToolName, crate::ToolVisibilityWitness>,
        ) -> Result<crate::ToolScopeRevision, crate::ToolScopeStageError> {
            Err(crate::ToolScopeStageError::Owner {
                message: "recording fallback owner does not stage filters".to_string(),
            })
        }

        fn stage_requested_deferred_names(
            &self,
            _names: std::collections::BTreeSet<crate::ToolName>,
        ) -> Result<crate::ToolScopeRevision, crate::ToolScopeStageError> {
            Err(crate::ToolScopeStageError::Owner {
                message: "recording fallback owner does not stage deferred names".to_string(),
            })
        }

        fn request_deferred_tools(
            &self,
            _authorities: Vec<crate::DeferredToolLoadAuthority>,
        ) -> Result<crate::ToolScopeRevision, crate::ToolScopeStageError> {
            Err(crate::ToolScopeStageError::Owner {
                message: "recording fallback owner does not request deferred tools".to_string(),
            })
        }

        fn boundary_applied(
            &self,
        ) -> Result<crate::SessionToolVisibilityState, crate::ToolScopeApplyError> {
            self.visibility_state()
        }
    }

    impl RecordingModelRoutingHandle {
        fn new(visibility_owner: Arc<dyn crate::ToolVisibilityOwner>) -> Self {
            Self {
                commits: Arc::new(Mutex::new(Vec::new())),
                visibility_owner,
            }
        }

        fn commits(
            &self,
        ) -> Vec<(
            crate::SessionLlmIdentity,
            crate::SessionLlmIdentity,
            u32,
            crate::handles::StickyModelFallbackVisibilityPlan,
        )> {
            self.commits.lock().unwrap().clone()
        }
    }

    impl crate::handles::ModelRoutingHandle for RecordingModelRoutingHandle {
        fn set_baseline(
            &self,
            _baseline_model: crate::lifecycle::run_primitive::ModelId,
            _realtime_capable: bool,
        ) -> Result<(), crate::handles::DslTransitionError> {
            Ok(())
        }

        fn hydrate_llm_capability_surface(
            &self,
            _identity: &crate::SessionLlmIdentity,
            _profile: Option<&crate::model_profile::ModelProfile>,
            _capability_base_filter: &crate::ToolFilter,
        ) -> Result<(), crate::handles::DslTransitionError> {
            Ok(())
        }

        fn stage_sticky_model_fallback(
            &self,
            activation: crate::StickyModelFallbackActivationProof,
            visibility_plan: &crate::handles::StickyModelFallbackVisibilityPlan,
        ) -> Result<
            Box<dyn crate::handles::StickyModelFallbackMachineCommit>,
            crate::handles::DslTransitionError,
        > {
            Ok(Box::new(RecordingStickyModelFallbackCommit {
                commits: Arc::clone(&self.commits),
                visibility_owner: Arc::clone(&self.visibility_owner),
                previous_identity: activation.previous_identity().clone(),
                target_identity: activation.target_identity().clone(),
                retry_attempt: activation.retry_attempt(),
                visibility_plan: visibility_plan.clone(),
            }))
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentLlmClient for FallbackActivatingClient {
        async fn stream_response(
            &self,
            messages: &[Message],
            tools: &[Arc<ToolDef>],
            max_tokens: u32,
            _temperature: Option<f32>,
            provider_params: Option<&crate::lifecycle::run_primitive::ProviderParamsOverride>,
        ) -> Result<super::LlmStreamResult, AgentError> {
            self.seen_max_tokens.lock().unwrap().push(max_tokens);
            self.seen_provider_params
                .lock()
                .unwrap()
                .push(provider_params.cloned());
            self.seen_tools.lock().unwrap().push(
                tools
                    .iter()
                    .map(|tool| tool.name.to_string())
                    .collect::<Vec<_>>(),
            );
            self.seen_notice_bodies.lock().unwrap().push(
                messages
                    .iter()
                    .filter_map(|message| match message {
                        Message::SystemNotice(notice) => notice.body.clone(),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
            );

            if self.active.load(std::sync::atomic::Ordering::SeqCst) == 0 {
                return Err(AgentError::llm(
                    "mock",
                    crate::error::LlmFailureReason::ProviderError(
                        crate::error::LlmProviderError::retryable(
                            crate::error::LlmProviderErrorKind::ServerOverloaded,
                            serde_json::json!({"message": "busy"}),
                        ),
                    ),
                    "mock overloaded",
                ));
            }
            if self.fail_backup.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(AgentError::llm(
                    "mock",
                    crate::error::LlmFailureReason::ProviderError(
                        crate::error::LlmProviderError::retryable(
                            crate::error::LlmProviderErrorKind::ServerOverloaded,
                            serde_json::json!({"message": "backup also busy"}),
                        ),
                    ),
                    "backup also overloaded",
                ));
            }

            Ok(super::LlmStreamResult::new(
                vec![AssistantBlock::Text {
                    text: "ok after fallback".to_string(),
                    meta: None,
                }],
                StopReason::EndTurn,
                Usage::default(),
            ))
        }

        fn provider(&self) -> crate::provider::Provider {
            crate::provider::Provider::OpenAI
        }

        fn model(&self) -> &'static str {
            if self.active.load(std::sync::atomic::Ordering::SeqCst) == 0 {
                "primary"
            } else {
                "backup"
            }
        }

        fn prepare_model_fallback(
            &self,
            _failure: &AgentError,
        ) -> Option<crate::AgentLlmFallbackSwitch> {
            self.fallback_proposals
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if self.active.load(std::sync::atomic::Ordering::SeqCst) != 0 {
                return None;
            }
            Some(crate::AgentLlmFallbackSwitch {
                previous_identity: crate::SessionLlmIdentity {
                    model: "primary".to_string(),
                    provider: crate::provider::Provider::OpenAI,
                    self_hosted_server_id: None,
                    provider_params: None,
                    auth_binding: None,
                },
                new_identity: crate::SessionLlmIdentity {
                    model: "backup".to_string(),
                    provider: crate::provider::Provider::OpenAI,
                    self_hosted_server_id: None,
                    provider_params: None,
                    auth_binding: None,
                },
                target_profile: self
                    .registry
                    .profile_witness_for_provider(crate::Provider::OpenAI, "backup")
                    .expect("registered backup profile"),
                request_policy: crate::SessionLlmRequestPolicy {
                    model: "backup".to_string(),
                    provider_params: Some(
                        crate::lifecycle::run_primitive::ProviderParamsOverride {
                            temperature: Some(0.37),
                            max_output_tokens: Some(777),
                            ..Default::default()
                        },
                    ),
                    provider_tool_defaults: None,
                },
                skipped_targets: Vec::new(),
            })
        }

        fn commit_model_fallback(
            &self,
            previous_identity: &crate::SessionLlmIdentity,
            target_identity: &crate::SessionLlmIdentity,
        ) -> Result<(), AgentError> {
            self.fallback_commits
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let expected_previous = self
                .active_model_fallback_identity()
                .ok_or_else(|| AgentError::ConfigError("missing active identity".to_string()))?;
            if expected_previous != *previous_identity {
                return Err(AgentError::ConfigError(
                    "fallback test client previous identity mismatch".to_string(),
                ));
            }
            self.active.store(
                usize::from(target_identity.model == "backup"),
                std::sync::atomic::Ordering::SeqCst,
            );
            Ok(())
        }

        fn active_model_fallback_identity(&self) -> Option<crate::SessionLlmIdentity> {
            if self.active.load(std::sync::atomic::Ordering::SeqCst) != 0
                && self
                    .fail_post_activation_identity_read_once
                    .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                return None;
            }
            Some(crate::SessionLlmIdentity {
                model: self.model().to_string(),
                provider: crate::provider::Provider::OpenAI,
                self_hosted_server_id: None,
                provider_params: None,
                auth_binding: None,
            })
        }
    }

    /// A custom client that proposes a fallback and exposes exact identity but
    /// deliberately omits the activation implementation. The trait default
    /// must fail closed before any canonical fallback state changes.
    struct ProposalOnlyFallbackClient {
        inner: FallbackActivatingClient,
    }

    impl ProposalOnlyFallbackClient {
        fn new() -> Self {
            Self {
                inner: FallbackActivatingClient::new(),
            }
        }

        fn registry(&self) -> Arc<crate::ModelRegistry> {
            self.inner.registry()
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentLlmClient for ProposalOnlyFallbackClient {
        async fn stream_response(
            &self,
            messages: &[Message],
            tools: &[Arc<ToolDef>],
            max_tokens: u32,
            temperature: Option<f32>,
            provider_params: Option<&crate::lifecycle::run_primitive::ProviderParamsOverride>,
        ) -> Result<super::LlmStreamResult, AgentError> {
            self.inner
                .stream_response(messages, tools, max_tokens, temperature, provider_params)
                .await
        }

        fn provider(&self) -> crate::provider::Provider {
            self.inner.provider()
        }

        fn model(&self) -> &str {
            self.inner.model()
        }

        fn prepare_model_fallback(
            &self,
            failure: &AgentError,
        ) -> Option<crate::AgentLlmFallbackSwitch> {
            self.inner.prepare_model_fallback(failure)
        }

        fn active_model_fallback_identity(&self) -> Option<crate::SessionLlmIdentity> {
            self.inner.active_model_fallback_identity()
        }
    }

    struct PartialOutputThenRecoverClient {
        registry: Arc<crate::ModelRegistry>,
        calls: std::sync::atomic::AtomicUsize,
        stream_output_observed: std::sync::atomic::AtomicBool,
        fallback_committed: std::sync::atomic::AtomicBool,
    }

    impl PartialOutputThenRecoverClient {
        fn new() -> Self {
            Self {
                registry: fallback_activation_test_registry(),
                calls: std::sync::atomic::AtomicUsize::new(0),
                stream_output_observed: std::sync::atomic::AtomicBool::new(false),
                fallback_committed: std::sync::atomic::AtomicBool::new(false),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(std::sync::atomic::Ordering::SeqCst)
        }

        fn fallback_committed(&self) -> bool {
            self.fallback_committed
                .load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentLlmClient for PartialOutputThenRecoverClient {
        async fn stream_response(
            &self,
            _messages: &[Message],
            _tools: &[Arc<ToolDef>],
            _max_tokens: u32,
            _temperature: Option<f32>,
            _provider_params: Option<&crate::lifecycle::run_primitive::ProviderParamsOverride>,
        ) -> Result<super::LlmStreamResult, AgentError> {
            let call = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            if call == 1 {
                self.stream_output_observed
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                return Err(AgentError::llm(
                    "mock",
                    crate::error::LlmFailureReason::ProviderError(
                        crate::error::LlmProviderError::retryable(
                            crate::error::LlmProviderErrorKind::ServerOverloaded,
                            serde_json::json!({"message": "late stream error"}),
                        ),
                    ),
                    "late stream error",
                ));
            }

            Ok(super::LlmStreamResult::new(
                vec![AssistantBlock::Text {
                    text: "ok after same-model retry".to_string(),
                    meta: None,
                }],
                StopReason::EndTurn,
                Usage::default(),
            ))
        }

        fn provider(&self) -> crate::provider::Provider {
            crate::provider::Provider::OpenAI
        }

        fn model(&self) -> &'static str {
            if self.fallback_committed() {
                "backup"
            } else {
                "primary"
            }
        }

        fn prepare_model_fallback(
            &self,
            _failure: &AgentError,
        ) -> Option<crate::AgentLlmFallbackSwitch> {
            Some(crate::AgentLlmFallbackSwitch {
                previous_identity: crate::SessionLlmIdentity {
                    model: "primary".to_string(),
                    provider: crate::provider::Provider::OpenAI,
                    self_hosted_server_id: None,
                    provider_params: None,
                    auth_binding: None,
                },
                new_identity: crate::SessionLlmIdentity {
                    model: "backup".to_string(),
                    provider: crate::provider::Provider::OpenAI,
                    self_hosted_server_id: None,
                    provider_params: None,
                    auth_binding: None,
                },
                target_profile: self
                    .registry
                    .profile_witness_for_provider(crate::Provider::OpenAI, "backup")
                    .expect("registered backup profile"),
                request_policy: crate::SessionLlmRequestPolicy {
                    model: "backup".to_string(),
                    provider_params: None,
                    provider_tool_defaults: None,
                },
                skipped_targets: Vec::new(),
            })
        }

        fn commit_model_fallback(
            &self,
            previous_identity: &crate::SessionLlmIdentity,
            target_identity: &crate::SessionLlmIdentity,
        ) -> Result<(), AgentError> {
            if self.active_model_fallback_identity().as_ref() != Some(previous_identity) {
                return Err(AgentError::ConfigError(
                    "partial-output test client previous identity mismatch".to_string(),
                ));
            }
            self.fallback_committed.store(
                target_identity.model == "backup",
                std::sync::atomic::Ordering::SeqCst,
            );
            Ok(())
        }

        fn active_model_fallback_identity(&self) -> Option<crate::SessionLlmIdentity> {
            Some(crate::SessionLlmIdentity {
                model: self.model().to_string(),
                provider: crate::provider::Provider::OpenAI,
                self_hosted_server_id: None,
                provider_params: None,
                auth_binding: None,
            })
        }

        fn begin_stream_output_observation(&self) {
            self.stream_output_observed
                .store(false, std::sync::atomic::Ordering::SeqCst);
        }

        fn stream_output_observed(&self) -> bool {
            self.stream_output_observed
                .load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    #[tokio::test]
    async fn retry_exhaustion_surfaces_typed_terminal_failure() {
        use crate::retry::RetryPolicy;

        let client = Arc::new(RateLimitThenSucceedClient::new(Duration::ZERO));
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .retry_policy(RetryPolicy::no_retry())
            .build_standalone(client, Arc::new(NoTools), Arc::new(NoopStore))
            .await;

        let err = agent
            .run("test".to_string().into())
            .await
            .expect_err("retry exhaustion must surface the machine terminal cause");

        match err {
            AgentError::TerminalFailure {
                outcome,
                cause_kind,
                ..
            } => {
                assert_eq!(outcome, TurnTerminalOutcome::Failed);
                assert_eq!(cause_kind, crate::TurnTerminalCauseKind::RetryExhausted);
            }
            other => panic!("expected typed TerminalFailure, got {other:?}"),
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

    #[tokio::test]
    async fn model_fallback_activation_filters_tools_before_retry() {
        use crate::retry::RetryPolicy;

        let client = Arc::new(FallbackActivatingClient::new());
        let tools = Arc::new(FullToolDispatcher::new(&[
            crate::VIEW_IMAGE_TOOL_NAME,
            "shell",
        ]));
        let visibility_owner: Arc<dyn crate::ToolVisibilityOwner> =
            Arc::new(RecordingFallbackVisibilityOwner::default());
        let generated_visibility_owner =
            crate::tool_scope::generated_test_tool_visibility_owner_from(Arc::clone(
                &visibility_owner,
            ));
        let model_routing = Arc::new(RecordingModelRoutingHandle::new(visibility_owner));
        let coordinator = Arc::new(RecordingStickyFallbackCoordinator::default());
        let mut agent = with_test_turn_state_handle_for_session(
            AgentBuilder::new(),
            explicit_hot_swap_session("primary"),
        )
        .with_effective_model_registry(client.registry())
        .with_tool_visibility_owner(generated_visibility_owner)
        .with_model_routing_handle(model_routing.clone())
        .with_sticky_model_fallback_commit_coordinator(coordinator.clone())
        .retry_policy(RetryPolicy {
            max_retries: 1,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(1),
            multiplier: 1.0,
            call_timeout: None,
        })
        .build_standalone(client.clone(), tools, Arc::new(NoopStore))
        .await;

        let result = agent
            .run("trigger fallback".to_string().into())
            .await
            .expect("fallback retry should succeed");

        assert_eq!(result.text, "ok after fallback");
        let commits = model_routing.commits();
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].0.model, "primary");
        assert_eq!(commits[0].1.model, "backup");
        assert_eq!(commits[0].2, 1);
        assert!(commits[0].3.committed_visible_set_changed);
        assert!(commits[0].3.revision_bumped);
        assert_eq!(commits[0].3.previous_state.active_revision, 0);
        assert_eq!(commits[0].3.next_state.active_revision, 1);
        let seen_tools = client.seen_tools();
        assert_eq!(seen_tools.len(), 2);
        assert!(
            seen_tools[0].contains(&crate::VIEW_IMAGE_TOOL_NAME.to_string()),
            "primary should see the initial visible tool set"
        );
        assert!(
            !seen_tools[1].contains(&crate::VIEW_IMAGE_TOOL_NAME.to_string()),
            "fallback retry should hide view_image for a downgraded model"
        );
        assert!(
            seen_tools[1].contains(&"shell".to_string()),
            "non-filtered tools should remain visible"
        );
        let notices = client.seen_notice_bodies();
        assert!(
            notices.get(1).is_some_and(|bodies| {
                bodies.iter().any(|body| {
                    body.contains("switched from model")
                        && body.contains("Hidden tools for the fallback model: view_image")
                })
            }),
            "fallback retry should inform the active model about the switch and hidden tool"
        );
        let seen_max_tokens = client.seen_max_tokens();
        assert_eq!(seen_max_tokens.len(), 2);
        assert_eq!(seen_max_tokens[1], 2048);
        assert!(
            seen_max_tokens[0] >= seen_max_tokens[1],
            "fallback retry should clamp max tokens to the backup model limit"
        );
        let seen_provider_params = client.seen_provider_params();
        assert_eq!(seen_provider_params.len(), 2);
        assert_eq!(seen_provider_params[0], None);
        assert_eq!(
            seen_provider_params[1]
                .as_ref()
                .and_then(|params| params.temperature),
            Some(0.37)
        );
        assert_eq!(
            seen_provider_params[1]
                .as_ref()
                .and_then(|params| params.max_output_tokens),
            Some(777)
        );
        let snapshot = agent
            .tool_scope_snapshot()
            .expect("effective tool scope snapshot");
        assert!(
            !snapshot
                .visible_names
                .iter()
                .any(|name| name.as_str() == crate::VIEW_IMAGE_TOOL_NAME),
            "reported live tool scope should match active fallback model visibility"
        );
        assert_eq!(
            snapshot.capability_base_filter,
            ToolFilter::Deny(
                [crate::VIEW_IMAGE_TOOL_NAME.to_string()]
                    .into_iter()
                    .collect()
            )
        );
        assert_eq!(snapshot.active_revision, crate::ToolScopeRevision(1));
        let durable_visibility = agent
            .session()
            .try_tool_visibility_state()
            .expect("fallback visibility metadata should decode")
            .expect("fallback visibility metadata should be present");
        assert_eq!(durable_visibility, commits[0].3.next_state);

        // A fallback-capable adapter may keep arbitrary client-local model
        // facts, but those are no longer semantic AgentLlmClient APIs. Forge a
        // fail-open filter and huge limit before the next turn: ToolScope plus
        // the registry-minted active witness must remain authoritative.
        client.forge_client_local_model_facts();
        let second = agent
            .run("sticky fallback".to_string().into())
            .await
            .expect("subsequent turn should keep using fallback model");
        assert_eq!(second.text, "ok after fallback");
        let seen_tools = client.seen_tools();
        assert_eq!(seen_tools.len(), 3);
        assert!(
            !seen_tools[2].contains(&crate::VIEW_IMAGE_TOOL_NAME.to_string()),
            "subsequent fallback turn should keep the downgraded tool surface"
        );
        let seen_max_tokens = client.seen_max_tokens();
        assert_eq!(seen_max_tokens.len(), 3);
        assert_eq!(
            seen_max_tokens[2], 2048,
            "subsequent fallback turn should keep the backup model max-token clamp"
        );
    }

    #[tokio::test]
    async fn terminal_fallback_retry_keeps_the_accepted_target_sticky() {
        use crate::retry::RetryPolicy;

        let client = Arc::new(FallbackActivatingClient::with_terminal_backup_failure());
        let visibility_owner: Arc<dyn crate::ToolVisibilityOwner> =
            Arc::new(RecordingFallbackVisibilityOwner::default());
        let generated_visibility_owner =
            crate::tool_scope::generated_test_tool_visibility_owner_from(Arc::clone(
                &visibility_owner,
            ));
        let model_routing = Arc::new(RecordingModelRoutingHandle::new(visibility_owner));
        let coordinator = Arc::new(RecordingStickyFallbackCoordinator::default());
        let mut agent = with_test_turn_state_handle_for_session(
            AgentBuilder::new(),
            explicit_hot_swap_session("primary"),
        )
        .with_effective_model_registry(client.registry())
        .with_tool_visibility_owner(generated_visibility_owner)
        .with_model_routing_handle(model_routing.clone())
        .with_sticky_model_fallback_commit_coordinator(coordinator.clone())
        .retry_policy(RetryPolicy {
            max_retries: 1,
            initial_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
            multiplier: 1.0,
            call_timeout: None,
        })
        .build_standalone(client.clone(), Arc::new(NoTools), Arc::new(NoopStore))
        .await;

        let error = agent
            .run("both providers fail".to_string().into())
            .await
            .expect_err("the fallback retry should exhaust the generated retry budget");
        assert!(
            matches!(
                &error,
                AgentError::TerminalFailure {
                    outcome: TurnTerminalOutcome::Failed,
                    cause_kind: crate::TurnTerminalCauseKind::RetryExhausted,
                    ..
                }
            ),
            "fallback exhaustion must preserve the typed terminal cause: {error}"
        );
        assert_eq!(client.model(), "backup");
        assert_eq!(coordinator.calls(), 1);
        assert_eq!(
            coordinator.target_models.lock().unwrap().as_slice(),
            &["backup".to_string()]
        );
        let commits = model_routing.commits();
        assert_eq!(commits.len(), 1);
        assert_eq!(
            agent
                .session()
                .session_metadata()
                .expect("fallback session metadata")
                .llm_identity()
                .model,
            "backup",
            "a terminal retry does not undo the already accepted sticky target"
        );
        assert_eq!(
            agent.tool_scope.visibility_state().unwrap(),
            commits[0].3.next_state
        );
    }

    #[tokio::test]
    async fn rejected_fallback_coordinator_terminalizes_error_recovery_for_next_run() {
        use crate::retry::RetryPolicy;

        let client = Arc::new(FallbackActivatingClient::new());
        let visibility_owner: Arc<dyn crate::ToolVisibilityOwner> =
            Arc::new(RecordingFallbackVisibilityOwner::default());
        let generated_visibility_owner =
            crate::tool_scope::generated_test_tool_visibility_owner_from(Arc::clone(
                &visibility_owner,
            ));
        let model_routing = Arc::new(RecordingModelRoutingHandle::new(visibility_owner));
        let coordinator = Arc::new(RejectingStickyFallbackCoordinator {
            error: crate::handles::StickyModelFallbackCommitError::Store(
                "synthetic pre-commit store rejection".to_string(),
            ),
        });
        let mut agent = with_test_turn_state_handle_for_session(
            AgentBuilder::new(),
            explicit_hot_swap_session("primary"),
        )
        .with_effective_model_registry(client.registry())
        .with_tool_visibility_owner(generated_visibility_owner)
        .with_model_routing_handle(model_routing)
        .with_sticky_model_fallback_commit_coordinator(coordinator)
        .retry_policy(RetryPolicy {
            max_retries: 1,
            initial_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
            multiplier: 1.0,
            call_timeout: None,
        })
        .build_standalone(client.clone(), Arc::new(NoTools), Arc::new(NoopStore))
        .await;

        for prompt in ["first rejected fallback", "second rejected fallback"] {
            let error = agent
                .run(prompt.to_string().into())
                .await
                .expect_err("coordinator rejection must fail the turn");
            assert!(error.to_string().contains("pre-commit store rejection"));
            let snapshot = agent
                .execution_snapshot()
                .expect("turn snapshot")
                .expect("test turn authority");
            assert_eq!(snapshot.turn_phase, crate::TurnPhase::Failed);
            assert_eq!(snapshot.active_run_id, None);
            assert_eq!(
                client.model(),
                "primary",
                "safe coordinator rejection must compensate the client before the next run"
            );
        }
    }

    #[tokio::test]
    async fn divergent_durable_identity_parent_rejects_fallback_before_any_commit() {
        use crate::retry::RetryPolicy;

        let client = Arc::new(FallbackActivatingClient::new());
        let visibility_owner: Arc<dyn crate::ToolVisibilityOwner> =
            Arc::new(RecordingFallbackVisibilityOwner::default());
        let generated_visibility_owner =
            crate::tool_scope::generated_test_tool_visibility_owner_from(Arc::clone(
                &visibility_owner,
            ));
        let model_routing = Arc::new(RecordingModelRoutingHandle::new(visibility_owner));
        let coordinator = Arc::new(RecordingStickyFallbackCoordinator::default());
        let mut agent = with_test_turn_state_handle_for_session(
            AgentBuilder::new(),
            explicit_hot_swap_session("primary"),
        )
        .with_effective_model_registry(client.registry())
        .with_tool_visibility_owner(generated_visibility_owner)
        .with_model_routing_handle(model_routing.clone())
        .with_sticky_model_fallback_commit_coordinator(coordinator.clone())
        .retry_policy(RetryPolicy {
            max_retries: 1,
            initial_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
            multiplier: 1.0,
            call_timeout: None,
        })
        .build_standalone(client.clone(), Arc::new(NoTools), Arc::new(NoopStore))
        .await;

        let mut divergent = agent
            .session()
            .session_metadata()
            .expect("test session metadata");
        divergent.model = "divergent-parent".to_string();
        agent
            .session_mut()
            .set_session_metadata(divergent)
            .expect("divergent test parent should serialize");

        let error = agent
            .run(
                "reject divergent durable fallback parent"
                    .to_string()
                    .into(),
            )
            .await
            .expect_err("a divergent durable identity parent must fail closed");
        assert!(
            error
                .to_string()
                .contains("does not match the machine-authorized parent"),
            "unexpected rejection: {error}"
        );
        assert_eq!(coordinator.calls(), 0);
        assert!(model_routing.commits().is_empty());
        assert_eq!(client.model(), "primary");
    }

    #[tokio::test]
    async fn model_fallback_post_activation_identity_read_failure_rolls_back_client() {
        use crate::retry::RetryPolicy;

        let client =
            Arc::new(FallbackActivatingClient::with_post_activation_identity_read_failure());
        let visibility_owner: Arc<dyn crate::ToolVisibilityOwner> =
            Arc::new(RecordingFallbackVisibilityOwner::default());
        let generated_visibility_owner =
            crate::tool_scope::generated_test_tool_visibility_owner_from(Arc::clone(
                &visibility_owner,
            ));
        let model_routing = Arc::new(RecordingModelRoutingHandle::new(visibility_owner));
        let mut agent = with_test_turn_state_handle_for_session(
            AgentBuilder::new(),
            explicit_hot_swap_session("primary"),
        )
        .with_effective_model_registry(client.registry())
        .with_tool_visibility_owner(generated_visibility_owner)
        .with_model_routing_handle(model_routing.clone())
        .retry_policy(RetryPolicy {
            max_retries: 1,
            initial_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
            multiplier: 1.0,
            call_timeout: None,
        })
        .build_standalone(client.clone(), Arc::new(NoTools), Arc::new(NoopStore))
        .await;

        let error = agent
            .run(
                "fail post-activation identity verification"
                    .to_string()
                    .into(),
            )
            .await
            .expect_err("post-activation identity verification failure must fail closed");

        assert!(
            error
                .to_string()
                .contains("fallback client activation verification"),
            "the original verification failure must be preserved: {error}"
        );
        assert_eq!(
            client.fallback_commits(),
            2,
            "activation must be followed by an explicit target-to-previous rollback attempt"
        );
        assert_eq!(
            client.model(),
            "primary",
            "a failed post-activation identity read must not strand the client on the target"
        );
        assert!(
            model_routing.commits().is_empty(),
            "machine identity and visibility must remain untouched when client verification fails"
        );
    }

    #[tokio::test]
    async fn model_fallback_proposer_without_activation_leaves_all_fallback_state_unchanged() {
        use crate::retry::RetryPolicy;

        let client = Arc::new(ProposalOnlyFallbackClient::new());
        let visibility_owner: Arc<dyn crate::ToolVisibilityOwner> =
            Arc::new(RecordingFallbackVisibilityOwner::default());
        let generated_visibility_owner =
            crate::tool_scope::generated_test_tool_visibility_owner_from(Arc::clone(
                &visibility_owner,
            ));
        let model_routing = Arc::new(RecordingModelRoutingHandle::new(visibility_owner));
        let mut agent = with_test_turn_state_handle_for_session(
            AgentBuilder::new(),
            explicit_hot_swap_session("primary"),
        )
        .with_effective_model_registry(client.registry())
        .with_tool_visibility_owner(generated_visibility_owner)
        .with_model_routing_handle(model_routing.clone())
        .retry_policy(RetryPolicy {
            max_retries: 1,
            initial_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
            multiplier: 1.0,
            call_timeout: None,
        })
        .build_standalone(client.clone(), Arc::new(NoTools), Arc::new(NoopStore))
        .await;
        let previous_model = agent.config.model.clone();
        let previous_visibility = agent.tool_scope.visibility_state().unwrap();

        let error = agent
            .run("reject proposal-only fallback".to_string().into())
            .await
            .expect_err("a proposer without activation must fail closed");

        assert!(
            error
                .to_string()
                .contains("without an activation implementation"),
            "{error}"
        );
        assert_eq!(client.model(), "primary");
        assert_eq!(agent.config.model, previous_model);
        assert_eq!(
            agent.tool_scope.visibility_state().unwrap(),
            previous_visibility
        );
        assert!(model_routing.commits().is_empty());
        assert!(
            !agent.session().messages().iter().any(|message| matches!(
                message,
                Message::SystemNotice(notice)
                    if notice.blocks.iter().any(|block| matches!(
                        block,
                        crate::types::SystemNoticeBlock::RuntimeNotice { category, .. }
                            if category == "model_fallback"
                    ))
            )),
            "rejected client activation must not append fallback session state"
        );
    }

    #[tokio::test]
    async fn model_fallback_does_not_switch_after_visible_stream_output() {
        use crate::retry::RetryPolicy;

        let client = Arc::new(PartialOutputThenRecoverClient::new());
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .retry_policy(RetryPolicy {
                max_retries: 1,
                initial_delay: Duration::ZERO,
                max_delay: Duration::ZERO,
                multiplier: 1.0,
                call_timeout: None,
            })
            .build_standalone(client.clone(), Arc::new(NoTools), Arc::new(NoopStore))
            .await;

        let result = agent
            .run("partial stream then late error".to_string().into())
            .await
            .expect("same-model retry should recover after post-stream error");

        assert_eq!(result.text, "ok after same-model retry");
        assert_eq!(client.calls(), 2);
        assert!(
            !client.fallback_committed(),
            "fallback must not activate after user-visible stream output escaped"
        );
    }

    /// Wraps the generated-kernel-backed [`TestTurnStateHandle`] but rejects the
    /// `RecoverableFailure` machine gate. Every other input (including the
    /// `ClassifyLlmFailureRecovery` classifier that yields `Recover`) flows
    /// through real MeerkatMachine authority; only the recoverable-failure
    /// commit is forced to refuse.
    #[derive(Debug)]
    struct RejectRecoverableFailureHandle {
        inner: crate::agent::test_turn_state_handle::TestTurnStateHandle,
    }

    impl RejectRecoverableFailureHandle {
        fn new() -> Self {
            Self {
                inner: crate::agent::test_turn_state_handle::TestTurnStateHandle::new(),
            }
        }
    }

    impl crate::handles::TurnStateHandle for RejectRecoverableFailureHandle {
        fn apply_turn_input(
            &self,
            input: crate::turn_execution_authority::TurnExecutionInput,
        ) -> Result<
            Vec<crate::turn_execution_authority::TurnExecutionEffect>,
            crate::handles::DslTransitionError,
        > {
            if matches!(
                input,
                crate::turn_execution_authority::TurnExecutionInput::RecoverableFailure { .. }
            ) {
                return Err(crate::handles::DslTransitionError::guard_rejected(
                    "RejectRecoverableFailureHandle::apply_turn_input",
                    "test handle rejects the RecoverableFailure machine gate",
                ));
            }
            self.inner.apply_turn_input(input)
        }

        fn start_conversation_run(
            &self,
            run_id: RunId,
            primitive_kind: TurnPrimitiveKind,
            admitted_content_shape: ContentShape,
            vision_enabled: bool,
            image_tool_results_enabled: bool,
            max_extraction_retries: u64,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.start_conversation_run(
                run_id,
                primitive_kind,
                admitted_content_shape,
                vision_enabled,
                image_tool_results_enabled,
                max_extraction_retries,
            )
        }

        fn start_immediate_append(
            &self,
            run_id: RunId,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.start_immediate_append(run_id)
        }

        fn start_immediate_context(
            &self,
            run_id: RunId,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.start_immediate_context(run_id)
        }

        fn primitive_applied(
            &self,
            run_id: RunId,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.primitive_applied(run_id)
        }

        fn llm_returned_tool_calls(
            &self,
            run_id: RunId,
            tool_count: u64,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.llm_returned_tool_calls(run_id, tool_count)
        }

        fn llm_returned_terminal(
            &self,
            run_id: RunId,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.llm_returned_terminal(run_id)
        }

        fn register_pending_ops(
            &self,
            run_id: RunId,
            op_refs: std::collections::BTreeSet<crate::ops::AsyncOpRef>,
            barrier_operation_ids: std::collections::BTreeSet<crate::ops::OperationId>,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner
                .register_pending_ops(run_id, op_refs, barrier_operation_ids)
        }

        fn tool_calls_resolved(
            &self,
            run_id: RunId,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.tool_calls_resolved(run_id)
        }

        fn ops_barrier_satisfied(
            &self,
            run_id: RunId,
            operation_ids: std::collections::BTreeSet<crate::ops::OperationId>,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.ops_barrier_satisfied(run_id, operation_ids)
        }

        fn boundary_continue(
            &self,
            run_id: RunId,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.boundary_continue(run_id)
        }

        fn boundary_complete(
            &self,
            run_id: RunId,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.boundary_complete(run_id)
        }

        fn enter_extraction(
            &self,
            run_id: RunId,
            max_retries: u32,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.enter_extraction(run_id, max_retries)
        }

        fn extraction_start(
            &self,
            run_id: RunId,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.extraction_start(run_id)
        }

        fn extraction_validation_passed(
            &self,
            run_id: RunId,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.extraction_validation_passed(run_id)
        }

        fn extraction_validation_failed(
            &self,
            run_id: RunId,
            error: String,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.extraction_validation_failed(run_id, error)
        }

        fn extraction_failed(
            &self,
            run_id: RunId,
            error: String,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.extraction_failed(run_id, error)
        }

        fn recoverable_failure(
            &self,
            run_id: RunId,
            retry: crate::retry::LlmRetrySchedule,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.recoverable_failure(run_id, retry)
        }

        fn fatal_failure(
            &self,
            run_id: RunId,
            failure: TurnFailureSource,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.fatal_failure(run_id, failure)
        }

        fn retry_requested(
            &self,
            run_id: RunId,
            retry_attempt: u32,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.retry_requested(run_id, retry_attempt)
        }

        fn cancel_now(&self, run_id: RunId) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.cancel_now(run_id)
        }

        fn request_cancel_after_boundary(
            &self,
            run_id: RunId,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.request_cancel_after_boundary(run_id)
        }

        fn cancellation_observed(
            &self,
            run_id: RunId,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.cancellation_observed(run_id)
        }

        fn acknowledge_terminal(
            &self,
            run_id: RunId,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.acknowledge_terminal(run_id)
        }

        fn turn_limit_reached(
            &self,
            run_id: RunId,
            turn_count: u64,
            max_turns: u64,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.turn_limit_reached(run_id, turn_count, max_turns)
        }

        fn budget_exhausted(
            &self,
            run_id: RunId,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.budget_exhausted(run_id)
        }

        fn time_budget_exceeded(
            &self,
            run_id: RunId,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.time_budget_exceeded(run_id)
        }

        fn force_cancel_no_run(&self) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.force_cancel_no_run()
        }

        fn run_completed(&self, run_id: RunId) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.run_completed(run_id)
        }

        fn run_failed(
            &self,
            run_id: RunId,
            reason: crate::turn_execution_authority::TurnFailureReason,
        ) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.run_failed(run_id, reason)
        }

        fn run_cancelled(&self, run_id: RunId) -> Result<(), crate::handles::DslTransitionError> {
            self.inner.run_cancelled(run_id)
        }

        fn snapshot(&self) -> crate::handles::TurnStateSnapshot {
            self.inner.snapshot()
        }
    }

    /// P0 Dogma Invariant 1: fallback activation must be gated behind the
    /// MeerkatMachine `RecoverableFailure` acceptance. If the machine rejects
    /// the recovery, NO durable/client mutation may have happened: the client
    /// active index stays on the primary model and the persisted session
    /// metadata still names the primary model.
    #[tokio::test]
    async fn model_fallback_does_not_mutate_when_machine_rejects_recovery() {
        use crate::retry::RetryPolicy;

        let client = Arc::new(FallbackActivatingClient::new());
        let tools = Arc::new(FullToolDispatcher::new(&[
            crate::VIEW_IMAGE_TOOL_NAME,
            "shell",
        ]));
        let mut session = crate::Session::new();
        session
            .set_build_state(crate::SessionBuildState::default())
            .expect("test session build state should serialize");
        // Seed durable session metadata so the fallback persistence branch in
        // `apply_model_fallback_switch` is genuinely reachable; the guard under
        // test is that this `model` is NOT rewritten to the backup identity.
        session
            .set_session_metadata(crate::SessionMetadata {
                schema_version: crate::SESSION_METADATA_SCHEMA_VERSION,
                model: "primary".to_string(),
                max_tokens: 1024,
                structured_output_retries: 2,
                provider: crate::provider::Provider::Other,
                self_hosted_server_id: None,
                provider_params: None,
                tooling: crate::SessionTooling::default(),
                keep_alive: false,
                comms_name: None,
                peer_meta: None,
                realm_id: None,
                instance_id: None,
                backend: None,
                config_generation: None,
                auth_binding: None,
                mob_member_binding: None,
            })
            .expect("typed metadata setter should route through generated authority");
        let mut agent = AgentBuilder::new()
            .resume_session(session)
            .with_turn_state_handle(Arc::new(RejectRecoverableFailureHandle::new()))
            .with_runtime_execution_kind_for_test(
                crate::lifecycle::RuntimeExecutionKind::ContentTurn,
            )
            .with_tool_visibility_owner(explicit_test_visibility_owner())
            .retry_policy(RetryPolicy {
                max_retries: 1,
                initial_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(1),
                multiplier: 1.0,
                call_timeout: None,
            })
            .build_standalone(client.clone(), tools, Arc::new(NoopStore))
            .await;

        let err = agent
            .run("trigger fallback".to_string().into())
            .await
            .expect_err(
                "a rejected RecoverableFailure machine gate must fail the run, not silently \
                 activate the fallback",
            );
        assert!(
            matches!(err, AgentError::InternalError(ref message)
                if message.contains("RecoverableFailure")),
            "rejection must propagate as an InternalError naming the rejected gate, got: {err:?}"
        );

        // Client active index untouched: still serving the primary model.
        assert_eq!(
            client.model(),
            "primary",
            "client active index must not commit the fallback when the machine rejects recovery"
        );

        // Durable session metadata untouched: still names the primary model,
        // never the backup identity that the fallback would have persisted.
        let metadata = agent
            .session()
            .session_metadata()
            .expect("seeded session metadata should survive the build");
        assert_eq!(
            metadata.model, "primary",
            "session metadata must not persist the fallback identity when recovery is rejected"
        );

        // Only the primary stream call ever ran (no fallback retry).
        assert_eq!(
            client.seen_tools().len(),
            1,
            "exactly one (primary) stream call should have run before the rejected gate"
        );
        assert_eq!(
            client.fallback_proposals(),
            0,
            "fallback target selection must happen only after the machine accepts recovery"
        );
    }

    #[test]
    fn model_fallback_safety_gate_rejects_timeout_failures() {
        assert!(!super::fallback_activation_is_pre_stream_safe(
            &AgentError::llm(
                "mock",
                LlmFailureReason::CallTimeout { duration_ms: 1000 },
                "call timed out",
            ),
            false,
        ));
        assert!(!super::fallback_activation_is_pre_stream_safe(
            &AgentError::llm(
                "mock",
                LlmFailureReason::NetworkTimeout { duration_ms: 1000 },
                "network timed out",
            ),
            false,
        ));
        assert!(super::fallback_activation_is_pre_stream_safe(
            &AgentError::llm(
                "mock",
                LlmFailureReason::InvalidModel("removed".to_string()),
                "model removed",
            ),
            false,
        ));
        assert!(!super::fallback_activation_is_pre_stream_safe(
            &AgentError::llm(
                "mock",
                LlmFailureReason::InvalidModel("removed".to_string()),
                "model removed after partial output",
            ),
            true,
        ));
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

    struct EmptyThenSucceedClient {
        calls: Mutex<u32>,
    }

    impl EmptyThenSucceedClient {
        fn new() -> Self {
            Self {
                calls: Mutex::new(0),
            }
        }

        fn calls(&self) -> u32 {
            *self.calls.lock().unwrap()
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentLlmClient for EmptyThenSucceedClient {
        async fn stream_response(
            &self,
            _messages: &[Message],
            _tools: &[Arc<ToolDef>],
            _max_tokens: u32,
            _temperature: Option<f32>,
            _provider_params: Option<&crate::lifecycle::run_primitive::ProviderParamsOverride>,
        ) -> Result<super::LlmStreamResult, AgentError> {
            let mut calls = self.calls.lock().unwrap();
            *calls += 1;
            let call = *calls;
            drop(calls);

            if call == 1 {
                Ok(super::LlmStreamResult::new(
                    Vec::new(),
                    StopReason::EndTurn,
                    Usage::default(),
                ))
            } else {
                Ok(super::LlmStreamResult::new(
                    vec![AssistantBlock::Text {
                        text: "ok after empty retry".to_string(),
                        meta: None,
                    }],
                    StopReason::EndTurn,
                    Usage::default(),
                ))
            }
        }

        fn provider(&self) -> crate::provider::Provider {
            crate::provider::Provider::Other
        }

        fn model(&self) -> &'static str {
            "mock-model"
        }
    }

    #[tokio::test]
    async fn empty_llm_completion_retries_before_terminal_failure() {
        use crate::retry::RetryPolicy;

        let client = Arc::new(EmptyThenSucceedClient::new());
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .retry_policy(RetryPolicy {
                max_retries: 1,
                initial_delay: Duration::ZERO,
                max_delay: Duration::ZERO,
                multiplier: 1.0,
                call_timeout: None,
            })
            .build_standalone(client.clone(), Arc::new(NoTools), Arc::new(NoopStore))
            .await;

        let result = agent
            .run("test".to_string().into())
            .await
            .expect("empty completed LLM response should retry and recover");
        assert_eq!(result.text, "ok after empty retry");
        assert_eq!(client.calls(), 2);
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

        fn provider(&self) -> crate::provider::Provider {
            crate::provider::Provider::Other
        }

        fn model(&self) -> &'static str {
            "mock-model"
        }
    }

    struct FailingExtractionClient {
        call_count: Mutex<u32>,
    }

    #[async_trait]
    impl AgentLlmClient for FailingExtractionClient {
        async fn stream_response(
            &self,
            _messages: &[Message],
            _tools: &[Arc<ToolDef>],
            _max_tokens: u32,
            _temperature: Option<f32>,
            _provider_params: Option<&crate::lifecycle::run_primitive::ProviderParamsOverride>,
        ) -> Result<super::LlmStreamResult, AgentError> {
            let mut call_count = self.call_count.lock().unwrap();
            *call_count += 1;
            if *call_count == 1 {
                Ok(text_response("main answer"))
            } else {
                Err(AgentError::llm(
                    "mock",
                    crate::error::LlmFailureReason::AuthError,
                    "extraction auth failed",
                ))
            }
        }

        fn provider(&self) -> crate::provider::Provider {
            crate::provider::Provider::Other
        }

        fn model(&self) -> &'static str {
            "mock-model"
        }
    }

    /// Row #194: a client that completes the main agentic turn normally but
    /// whose `compile_schema` fails, so structured-output extraction setup
    /// cannot proceed.
    struct CompileSchemaFailingClient;

    #[async_trait]
    impl AgentLlmClient for CompileSchemaFailingClient {
        async fn stream_response(
            &self,
            _messages: &[Message],
            _tools: &[Arc<ToolDef>],
            _max_tokens: u32,
            _temperature: Option<f32>,
            _provider_params: Option<&crate::lifecycle::run_primitive::ProviderParamsOverride>,
        ) -> Result<super::LlmStreamResult, AgentError> {
            Ok(text_response("main answer"))
        }

        fn compile_schema(
            &self,
            _output_schema: &crate::types::OutputSchema,
        ) -> Result<crate::schema::CompiledSchema, crate::schema::SchemaError> {
            Err(crate::schema::SchemaError::InvalidRoot)
        }

        fn provider(&self) -> crate::provider::Provider {
            crate::provider::Provider::Other
        }

        fn model(&self) -> &'static str {
            "mock-model"
        }
    }

    struct ExtractionFallbackOverrideClient {
        registry: Arc<crate::ModelRegistry>,
        active_fallback: std::sync::atomic::AtomicBool,
        call_count: std::sync::atomic::AtomicUsize,
        mismatched_fallback_tag: bool,
        reject_target_compile: bool,
        target_profile: crate::ModelProfileWitness,
        seen_max_tokens: Mutex<Vec<u32>>,
        seen_provider_params:
            Mutex<Vec<Option<crate::lifecycle::run_primitive::ProviderParamsOverride>>>,
    }

    fn extraction_fallback_registry() -> crate::ModelRegistry {
        let mut config = crate::Config::default();
        config.models.custom.insert(
            "claude-backup".to_string(),
            crate::config::CustomModelConfig {
                provider: crate::Provider::Anthropic,
                display_name: Some("Extraction fallback".to_string()),
                context_window: Some(64_000),
                max_output_tokens: Some(1024),
                vision: Some(false),
                web_search: Some(false),
                call_timeout_secs: None,
            },
        );
        crate::ModelRegistry::from_config(
            &config,
            *crate::model_profile::test_catalog::TEST_CATALOG,
        )
        .expect("extraction fallback registry")
    }

    impl ExtractionFallbackOverrideClient {
        fn new() -> Self {
            let registry = Arc::new(extraction_fallback_registry());
            let target_profile = registry
                .profile_witness_for_provider(crate::Provider::Anthropic, "claude-backup")
                .expect("registered extraction fallback profile");
            Self {
                registry,
                active_fallback: std::sync::atomic::AtomicBool::new(false),
                call_count: std::sync::atomic::AtomicUsize::new(0),
                mismatched_fallback_tag: false,
                reject_target_compile: false,
                target_profile,
                seen_max_tokens: Mutex::new(Vec::new()),
                seen_provider_params: Mutex::new(Vec::new()),
            }
        }

        fn with_provider_mismatch() -> Self {
            Self {
                mismatched_fallback_tag: true,
                ..Self::new()
            }
        }

        fn with_target_profile(target_profile: crate::ModelProfileWitness) -> Self {
            Self {
                target_profile,
                ..Self::new()
            }
        }

        fn with_target_compile_rejection() -> Self {
            Self {
                reject_target_compile: true,
                ..Self::new()
            }
        }

        fn seen_provider_params(
            &self,
        ) -> Vec<Option<crate::lifecycle::run_primitive::ProviderParamsOverride>> {
            self.seen_provider_params.lock().unwrap().clone()
        }

        fn seen_max_tokens(&self) -> Vec<u32> {
            self.seen_max_tokens.lock().unwrap().clone()
        }

        fn registry(&self) -> Arc<crate::ModelRegistry> {
            Arc::clone(&self.registry)
        }
    }

    #[async_trait]
    impl AgentLlmClient for ExtractionFallbackOverrideClient {
        async fn stream_response(
            &self,
            _messages: &[Message],
            _tools: &[Arc<ToolDef>],
            max_tokens: u32,
            _temperature: Option<f32>,
            provider_params: Option<&crate::lifecycle::run_primitive::ProviderParamsOverride>,
        ) -> Result<super::LlmStreamResult, AgentError> {
            self.seen_max_tokens.lock().unwrap().push(max_tokens);
            self.seen_provider_params
                .lock()
                .unwrap()
                .push(provider_params.cloned());
            let call = self
                .call_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                + 1;

            match call {
                1 => Ok(text_response("main answer")),
                2 => Err(AgentError::llm(
                    "openai",
                    crate::error::LlmFailureReason::ProviderError(
                        crate::error::LlmProviderError::retryable(
                            crate::error::LlmProviderErrorKind::ServerOverloaded,
                            serde_json::json!({"message": "extraction retryable failure"}),
                        ),
                    ),
                    "extraction retryable failure",
                )),
                3 => Ok(text_response(r#"{"answer": "42"}"#)),
                _ => Err(AgentError::InternalError(
                    "ExtractionFallbackOverrideClient: unexpected extra call".to_string(),
                )),
            }
        }

        fn provider(&self) -> crate::provider::Provider {
            if self
                .active_fallback
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                crate::provider::Provider::Anthropic
            } else {
                crate::provider::Provider::OpenAI
            }
        }

        fn model(&self) -> &'static str {
            if self
                .active_fallback
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                "claude-backup"
            } else {
                "gpt-primary"
            }
        }

        fn prepare_model_fallback(
            &self,
            _failure: &AgentError,
        ) -> Option<crate::AgentLlmFallbackSwitch> {
            use crate::lifecycle::run_primitive::{
                AnthropicProviderTag, OpaqueProviderBody, OpenAiProviderTag,
                ProviderParamsOverride, ProviderTag,
            };

            if self
                .active_fallback
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                return None;
            }

            Some(crate::AgentLlmFallbackSwitch {
                previous_identity: crate::SessionLlmIdentity {
                    model: "gpt-primary".to_string(),
                    provider: crate::provider::Provider::OpenAI,
                    self_hosted_server_id: None,
                    provider_params: None,
                    auth_binding: None,
                },
                new_identity: crate::SessionLlmIdentity {
                    model: "claude-backup".to_string(),
                    provider: crate::provider::Provider::Anthropic,
                    self_hosted_server_id: None,
                    provider_params: None,
                    auth_binding: None,
                },
                target_profile: self.target_profile.clone(),
                request_policy: crate::SessionLlmRequestPolicy {
                    model: "claude-backup".to_string(),
                    provider_params: Some(ProviderParamsOverride {
                        provider_tag: Some(if self.mismatched_fallback_tag {
                            ProviderTag::OpenAi(OpenAiProviderTag::default())
                        } else {
                            ProviderTag::Anthropic(AnthropicProviderTag {
                                web_search: Some(OpaqueProviderBody::from_value(
                                    &serde_json::json!({"type": "web_search"}),
                                )),
                                ..Default::default()
                            })
                        }),
                        ..Default::default()
                    }),
                    provider_tool_defaults: None,
                },
                skipped_targets: Vec::new(),
            })
        }

        fn commit_model_fallback(
            &self,
            previous_identity: &crate::SessionLlmIdentity,
            target_identity: &crate::SessionLlmIdentity,
        ) -> Result<(), AgentError> {
            if self.active_model_fallback_identity().as_ref() != Some(previous_identity) {
                return Err(AgentError::ConfigError(
                    "extraction fallback test client previous identity mismatch".to_string(),
                ));
            }
            self.active_fallback.store(
                target_identity.provider == crate::provider::Provider::Anthropic,
                std::sync::atomic::Ordering::SeqCst,
            );
            Ok(())
        }

        fn active_model_fallback_identity(&self) -> Option<crate::SessionLlmIdentity> {
            Some(crate::SessionLlmIdentity {
                model: self.model().to_string(),
                provider: self.provider(),
                self_hosted_server_id: None,
                provider_params: None,
                auth_binding: None,
            })
        }

        fn compile_model_fallback_schema(
            &self,
            target_identity: &crate::SessionLlmIdentity,
            output_schema: &crate::OutputSchema,
        ) -> Result<crate::CompiledSchema, AgentError> {
            if target_identity.provider != crate::provider::Provider::Anthropic
                || target_identity.model != "claude-backup"
            {
                return Err(AgentError::ConfigError(
                    "unexpected fallback compile target".to_string(),
                ));
            }
            if self.reject_target_compile {
                return Err(AgentError::ConfigError(
                    "target client rejects compiled schema".to_string(),
                ));
            }
            let mut schema = output_schema.schema.as_value().clone();
            schema
                .as_object_mut()
                .ok_or_else(|| AgentError::ConfigError("schema root is not an object".to_string()))?
                .insert(
                    "x-target-compiled".to_string(),
                    serde_json::json!("anthropic"),
                );
            Ok(crate::CompiledSchema {
                schema,
                warnings: Vec::new(),
            })
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

    fn tool_call_response(name: &str) -> super::LlmStreamResult {
        super::LlmStreamResult::new(
            vec![AssistantBlock::ToolUse {
                id: "call-extraction-tool".to_string(),
                name: name.into(),
                args: serde_json::value::RawValue::from_string("{}".to_string()).unwrap(),
                meta: None,
            }],
            StopReason::ToolUse,
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

    #[tokio::test]
    async fn extraction_fallback_reapplies_structured_output_and_clears_native_search() {
        use crate::lifecycle::run_primitive::ProviderTag;
        use crate::retry::RetryPolicy;

        let schema = crate::types::OutputSchema::new(serde_json::json!({
            "type": "object",
            "properties": {
                "answer": { "type": "string" }
            },
            "required": ["answer"]
        }))
        .unwrap();

        let registry = Arc::new(extraction_fallback_registry());
        let target_profile = registry
            .profile_witness_for_provider(crate::Provider::Anthropic, "claude-backup")
            .expect("same-registry fallback profile");
        let client = Arc::new(ExtractionFallbackOverrideClient::with_target_profile(
            target_profile,
        ));
        let visibility_owner: Arc<dyn crate::ToolVisibilityOwner> =
            Arc::new(RecordingFallbackVisibilityOwner::default());
        let generated_visibility_owner =
            crate::tool_scope::generated_test_tool_visibility_owner_from(Arc::clone(
                &visibility_owner,
            ));
        let model_routing = Arc::new(RecordingModelRoutingHandle::new(visibility_owner));
        let mut agent = with_test_turn_state_handle_for_session(
            AgentBuilder::new(),
            explicit_hot_swap_session("gpt-primary"),
        )
        .output_schema(schema)
        .with_effective_model_registry(Arc::clone(&registry))
        .with_tool_visibility_owner(generated_visibility_owner)
        .with_model_routing_handle(model_routing.clone())
        .retry_policy(RetryPolicy {
            max_retries: 1,
            initial_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
            multiplier: 1.0,
            call_timeout: None,
        })
        .build_standalone(client.clone(), Arc::new(NoTools), Arc::new(NoopStore))
        .await;

        let result = agent
            .run("extract after fallback".to_string().into())
            .await
            .expect("extraction fallback should retry and validate structured output");

        assert_eq!(result.structured_output.unwrap()["answer"], "42");
        let commits = model_routing.commits();
        assert_eq!(commits.len(), 1);
        assert_eq!(
            commits[0].3.next_state.capability_base_filter,
            ToolFilter::Deny(
                [crate::VIEW_IMAGE_TOOL_NAME.to_string()]
                    .into_iter()
                    .collect()
            ),
            "visibility must derive from the same-authority registry profile, not the client's forged All filter"
        );
        assert_eq!(
            client.seen_max_tokens().last().copied(),
            Some(1024),
            "fallback request must use the registry-owned output-token limit, not the client's forged 4096 limit"
        );
        let fallback_payload = agent
            .session()
            .messages()
            .iter()
            .find_map(|message| match message {
                Message::SystemNotice(notice) => {
                    notice.blocks.iter().find_map(|block| match block {
                        crate::types::SystemNoticeBlock::RuntimeNotice {
                            category,
                            payload,
                            ..
                        } if category == "model_fallback" => payload.as_ref(),
                        _ => None,
                    })
                }
                _ => None,
            })
            .expect("fallback notice payload");
        assert_eq!(
            fallback_payload.pointer("/to/context_window"),
            Some(&serde_json::json!(64_000)),
            "fallback notice must carry the registry-owned context window, not the client's forged 128000 window"
        );
        let seen_params = client.seen_provider_params();
        assert_eq!(
            seen_params.len(),
            3,
            "expected main call, failed extraction call, and fallback extraction retry"
        );
        match seen_params[1]
            .as_ref()
            .and_then(|params| params.provider_tag.as_ref())
        {
            Some(ProviderTag::OpenAi(tag)) => {
                assert!(
                    tag.structured_output.is_some(),
                    "primary extraction call should carry structured output"
                );
                assert_eq!(
                    tag.web_search, None,
                    "primary extraction call must disable provider-native web search"
                );
            }
            other => panic!("expected OpenAI extraction params, got {other:?}"),
        }
        match seen_params[2]
            .as_ref()
            .and_then(|params| params.provider_tag.as_ref())
        {
            Some(ProviderTag::Anthropic(tag)) => {
                assert!(
                    tag.structured_output.is_some(),
                    "fallback extraction retry should carry structured output for the new provider"
                );
                assert_eq!(
                    tag.structured_output
                        .as_ref()
                        .and_then(|schema| schema.schema.as_value().get("x-target-compiled")),
                    Some(&serde_json::json!("anthropic")),
                    "fallback request must carry the target client's compiled schema, not the raw source schema"
                );
                assert_eq!(
                    tag.web_search, None,
                    "fallback extraction retry must not re-enable provider-native web search"
                );
            }
            other => panic!("expected Anthropic fallback extraction params, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn extraction_fallback_authority_unknown_survives_as_teardown_required() {
        use crate::lifecycle::core_executor::{CoreExecutorError, CoreExecutorTeardownReason};
        use crate::retry::RetryPolicy;

        let schema = crate::types::OutputSchema::new(serde_json::json!({
            "type": "object",
            "properties": { "answer": { "type": "string" } },
            "required": ["answer"]
        }))
        .unwrap();
        let registry = Arc::new(extraction_fallback_registry());
        let target_profile = registry
            .profile_witness_for_provider(crate::Provider::Anthropic, "claude-backup")
            .expect("same-registry fallback profile");
        let client = Arc::new(ExtractionFallbackOverrideClient::with_target_profile(
            target_profile,
        ));
        let visibility_owner: Arc<dyn crate::ToolVisibilityOwner> =
            Arc::new(RecordingFallbackVisibilityOwner::default());
        let generated_visibility_owner =
            crate::tool_scope::generated_test_tool_visibility_owner_from(Arc::clone(
                &visibility_owner,
            ));
        let model_routing = Arc::new(RecordingModelRoutingHandle::new(visibility_owner));
        let coordinator = Arc::new(RejectingStickyFallbackCoordinator {
            error: crate::handles::StickyModelFallbackCommitError::SnapshotOutcomeUnknown(
                "synthetic extraction CAS acknowledgement loss".to_string(),
            ),
        });
        let mut agent = with_test_turn_state_handle_for_session(
            AgentBuilder::new(),
            explicit_hot_swap_session("gpt-primary"),
        )
        .output_schema(schema)
        .with_effective_model_registry(registry)
        .with_tool_visibility_owner(generated_visibility_owner)
        .with_model_routing_handle(model_routing)
        .with_sticky_model_fallback_commit_coordinator(coordinator)
        .retry_policy(RetryPolicy {
            max_retries: 1,
            initial_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
            multiplier: 1.0,
            call_timeout: None,
        })
        .build_standalone(client, Arc::new(NoTools), Arc::new(NoopStore))
        .await;

        let error = agent
            .run("extract across unknown fallback CAS".to_string().into())
            .await
            .expect_err("authority-unknown extraction fallback must not become RunResult");
        assert!(matches!(
            &error,
            AgentError::StickyModelFallbackAuthorityUnknown { message }
                if message.contains("extraction CAS acknowledgement loss")
        ));

        let core_error =
            CoreExecutorError::apply_failed_from_session_error(crate::SessionError::Agent(error));
        assert!(matches!(
            core_error,
            CoreExecutorError::TeardownRequired {
                reason: CoreExecutorTeardownReason::SessionUnavailable,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn fallback_provider_tag_mismatch_rejects_before_any_identity_or_visibility_commit() {
        use crate::retry::RetryPolicy;

        let schema = crate::types::OutputSchema::new(serde_json::json!({
            "type": "object",
            "properties": { "answer": { "type": "string" } },
            "required": ["answer"]
        }))
        .unwrap();
        let client = Arc::new(ExtractionFallbackOverrideClient::with_provider_mismatch());
        let visibility_owner: Arc<dyn crate::ToolVisibilityOwner> =
            Arc::new(RecordingFallbackVisibilityOwner::default());
        let generated_visibility_owner =
            crate::tool_scope::generated_test_tool_visibility_owner_from(Arc::clone(
                &visibility_owner,
            ));
        let model_routing = Arc::new(RecordingModelRoutingHandle::new(visibility_owner));
        let mut agent = with_test_turn_state_handle_for_session(
            AgentBuilder::new(),
            explicit_hot_swap_session("gpt-primary"),
        )
        .output_schema(schema)
        .with_effective_model_registry(client.registry())
        .with_tool_visibility_owner(generated_visibility_owner)
        .with_model_routing_handle(model_routing.clone())
        .retry_policy(RetryPolicy {
            max_retries: 1,
            initial_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
            multiplier: 1.0,
            call_timeout: None,
        })
        .build_standalone(client.clone(), Arc::new(NoTools), Arc::new(NoopStore))
        .await;
        let previous_model = agent.config.model.clone();
        let previous_visibility = agent
            .tool_scope
            .visibility_state()
            .expect("initial visibility state");

        let error = agent
            .run("reject mismatched fallback".to_string().into())
            .await
            .expect_err("provider-tag mismatch must fail before fallback commit");

        assert!(error.to_string().contains("provider tag"), "{error}");
        assert!(
            !client
                .active_fallback
                .load(std::sync::atomic::Ordering::SeqCst),
            "client fallback pointer must remain unchanged"
        );
        assert!(
            model_routing.commits().is_empty(),
            "generated routing authority must not be called after preparation rejects"
        );
        assert_eq!(agent.config.model, previous_model);
        assert_eq!(
            agent.tool_scope.visibility_state().unwrap(),
            previous_visibility,
            "canonical visibility owner must remain unchanged"
        );
        assert!(
            !agent.session().messages().iter().any(|message| matches!(
                message,
                Message::SystemNotice(notice)
                    if notice.blocks.iter().any(|block| matches!(
                        block,
                        crate::types::SystemNoticeBlock::RuntimeNotice { category, .. }
                            if category == "model_fallback"
                    ))
            )),
            "rejected preparation must not append the fallback notice"
        );
    }

    #[tokio::test]
    async fn fallback_target_schema_compile_rejection_leaves_all_fallback_state_unchanged() {
        use crate::retry::RetryPolicy;

        let schema = crate::types::OutputSchema::new(serde_json::json!({
            "type": "object",
            "properties": { "answer": { "type": "string" } },
            "required": ["answer"]
        }))
        .unwrap();
        let client = Arc::new(ExtractionFallbackOverrideClient::with_target_compile_rejection());
        let visibility_owner: Arc<dyn crate::ToolVisibilityOwner> =
            Arc::new(RecordingFallbackVisibilityOwner::default());
        let generated_visibility_owner =
            crate::tool_scope::generated_test_tool_visibility_owner_from(Arc::clone(
                &visibility_owner,
            ));
        let model_routing = Arc::new(RecordingModelRoutingHandle::new(visibility_owner));
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .output_schema(schema)
            .with_effective_model_registry(client.registry())
            .with_tool_visibility_owner(generated_visibility_owner)
            .with_model_routing_handle(model_routing.clone())
            .retry_policy(RetryPolicy {
                max_retries: 1,
                initial_delay: Duration::ZERO,
                max_delay: Duration::ZERO,
                multiplier: 1.0,
                call_timeout: None,
            })
            .build_standalone(client.clone(), Arc::new(NoTools), Arc::new(NoopStore))
            .await;
        let previous_model = agent.config.model.clone();
        let previous_visibility = agent.tool_scope.visibility_state().unwrap();

        let error = agent
            .run("reject target schema compilation".to_string().into())
            .await
            .expect_err("target schema compile rejection must abort fallback activation");

        assert!(
            error
                .to_string()
                .contains("failed to compile structured output for fallback target"),
            "{error}"
        );
        assert_eq!(
            client.call_count.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "target compile failure must happen before a fallback provider call"
        );
        assert!(
            !client
                .active_fallback
                .load(std::sync::atomic::Ordering::SeqCst)
        );
        assert!(model_routing.commits().is_empty());
        assert_eq!(agent.config.model, previous_model);
        assert_eq!(
            agent.tool_scope.visibility_state().unwrap(),
            previous_visibility
        );
        assert!(!agent.session().messages().iter().any(|message| matches!(
            message,
            Message::SystemNotice(notice)
                if notice.blocks.iter().any(|block| matches!(
                    block,
                    crate::types::SystemNoticeBlock::RuntimeNotice { category, .. }
                        if category == "model_fallback"
                ))
        )));
    }

    #[tokio::test]
    async fn fallback_profile_witness_mismatch_rejects_atomically() {
        use crate::retry::RetryPolicy;

        let registry = crate::ModelRegistry::from_config(
            &crate::Config::default(),
            *crate::model_profile::test_catalog::TEST_CATALOG,
        )
        .expect("synthetic model registry");
        let mismatched_profile = registry
            .profile_witness_for_provider(
                crate::Provider::OpenAI,
                crate::model_profile::test_catalog::OPENAI_MODEL,
            )
            .expect("OpenAI profile witness");
        let client = Arc::new(ExtractionFallbackOverrideClient::with_target_profile(
            mismatched_profile,
        ));
        let visibility_owner: Arc<dyn crate::ToolVisibilityOwner> =
            Arc::new(RecordingFallbackVisibilityOwner::default());
        let generated_visibility_owner =
            crate::tool_scope::generated_test_tool_visibility_owner_from(Arc::clone(
                &visibility_owner,
            ));
        let model_routing = Arc::new(RecordingModelRoutingHandle::new(visibility_owner));
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .output_schema(
                crate::OutputSchema::new(serde_json::json!({
                    "type": "object",
                    "properties": { "answer": { "type": "string" } },
                    "required": ["answer"]
                }))
                .unwrap(),
            )
            .with_tool_visibility_owner(generated_visibility_owner)
            .with_model_routing_handle(model_routing.clone())
            .retry_policy(RetryPolicy {
                max_retries: 1,
                initial_delay: Duration::ZERO,
                max_delay: Duration::ZERO,
                multiplier: 1.0,
                call_timeout: None,
            })
            .build_standalone(client.clone(), Arc::new(NoTools), Arc::new(NoopStore))
            .await;
        let previous_model = agent.config.model.clone();
        let previous_visibility = agent.tool_scope.visibility_state().unwrap();

        let error = agent
            .run("reject mismatched profile witness".to_string().into())
            .await
            .expect_err("profile provenance mismatch must fail before fallback commit");

        assert!(
            error.to_string().contains("profile witness targets"),
            "{error}"
        );
        assert!(
            !client
                .active_fallback
                .load(std::sync::atomic::Ordering::SeqCst)
        );
        assert!(model_routing.commits().is_empty());
        assert_eq!(agent.config.model, previous_model);
        assert_eq!(
            agent.tool_scope.visibility_state().unwrap(),
            previous_visibility
        );
        assert!(!agent.session().messages().iter().any(|message| matches!(
            message,
            Message::SystemNotice(notice)
                if notice.blocks.iter().any(|block| matches!(
                    block,
                    crate::types::SystemNoticeBlock::RuntimeNotice { category, .. }
                        if category == "model_fallback"
                ))
        )));
    }

    #[tokio::test]
    async fn foreign_identical_registry_profile_witness_rejects_atomically() {
        use crate::retry::RetryPolicy;

        let effective_registry = Arc::new(extraction_fallback_registry());
        let foreign_registry = extraction_fallback_registry();
        assert_ne!(
            effective_registry.authority(),
            foreign_registry.authority(),
            "independently constructed identical registries must have distinct authority"
        );
        let foreign_profile = foreign_registry
            .profile_witness_for_provider(crate::Provider::Anthropic, "claude-backup")
            .expect("foreign identical-pair profile witness");
        let client = Arc::new(ExtractionFallbackOverrideClient::with_target_profile(
            foreign_profile,
        ));
        let visibility_owner: Arc<dyn crate::ToolVisibilityOwner> =
            Arc::new(RecordingFallbackVisibilityOwner::default());
        let generated_visibility_owner =
            crate::tool_scope::generated_test_tool_visibility_owner_from(Arc::clone(
                &visibility_owner,
            ));
        let model_routing = Arc::new(RecordingModelRoutingHandle::new(visibility_owner));
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .output_schema(
                crate::OutputSchema::new(serde_json::json!({
                    "type": "object",
                    "properties": { "answer": { "type": "string" } },
                    "required": ["answer"]
                }))
                .unwrap(),
            )
            .with_effective_model_registry(Arc::clone(&effective_registry))
            .with_tool_visibility_owner(generated_visibility_owner)
            .with_model_routing_handle(model_routing.clone())
            .retry_policy(RetryPolicy {
                max_retries: 1,
                initial_delay: Duration::ZERO,
                max_delay: Duration::ZERO,
                multiplier: 1.0,
                call_timeout: None,
            })
            .build_standalone(client.clone(), Arc::new(NoTools), Arc::new(NoopStore))
            .await;
        let previous_model = agent.config.model.clone();
        let previous_visibility = agent.tool_scope.visibility_state().unwrap();

        let error = agent
            .run("reject foreign registry witness".to_string().into())
            .await
            .expect_err("foreign registry witness must fail before fallback commit");

        assert!(
            error
                .to_string()
                .contains("was not minted by the agent's effective model registry"),
            "{error}"
        );
        assert!(
            !client
                .active_fallback
                .load(std::sync::atomic::Ordering::SeqCst)
        );
        assert!(model_routing.commits().is_empty());
        assert_eq!(agent.config.model, previous_model);
        assert_eq!(
            agent.tool_scope.visibility_state().unwrap(),
            previous_visibility
        );
        assert!(!agent.session().messages().iter().any(|message| matches!(
            message,
            Message::SystemNotice(notice)
                if notice.blocks.iter().any(|block| matches!(
                    block,
                    crate::types::SystemNoticeBlock::RuntimeNotice { category, .. }
                        if category == "model_fallback"
                ))
        )));
    }

    #[tokio::test]
    async fn registry_absent_fallback_target_rejects_even_with_a_valid_foreign_profile() {
        use crate::retry::RetryPolicy;

        let effective_registry = Arc::new(
            crate::ModelRegistry::from_config(
                &crate::Config::default(),
                *crate::model_profile::test_catalog::TEST_CATALOG,
            )
            .expect("effective registry without synthetic fallback target"),
        );
        assert!(
            effective_registry
                .profile_witness_for_provider(crate::Provider::Anthropic, "claude-backup")
                .is_none(),
            "test target must be unresolved in the captured effective registry"
        );
        let foreign_registry = extraction_fallback_registry();
        let foreign_profile = foreign_registry
            .profile_witness_for_provider(crate::Provider::Anthropic, "claude-backup")
            .expect("foreign registry contains the target");
        let client = Arc::new(ExtractionFallbackOverrideClient::with_target_profile(
            foreign_profile,
        ));
        let visibility_owner: Arc<dyn crate::ToolVisibilityOwner> =
            Arc::new(RecordingFallbackVisibilityOwner::default());
        let generated_visibility_owner =
            crate::tool_scope::generated_test_tool_visibility_owner_from(Arc::clone(
                &visibility_owner,
            ));
        let model_routing = Arc::new(RecordingModelRoutingHandle::new(visibility_owner));
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .output_schema(
                crate::OutputSchema::new(serde_json::json!({
                    "type": "object",
                    "properties": { "answer": { "type": "string" } },
                    "required": ["answer"]
                }))
                .unwrap(),
            )
            .with_effective_model_registry(Arc::clone(&effective_registry))
            .with_tool_visibility_owner(generated_visibility_owner)
            .with_model_routing_handle(model_routing.clone())
            .retry_policy(RetryPolicy {
                max_retries: 1,
                initial_delay: Duration::ZERO,
                max_delay: Duration::ZERO,
                multiplier: 1.0,
                call_timeout: None,
            })
            .build_standalone(client.clone(), Arc::new(NoTools), Arc::new(NoopStore))
            .await;
        let previous_visibility = agent.tool_scope.visibility_state().unwrap();

        let error = agent
            .run("reject registry-absent fallback".to_string().into())
            .await
            .expect_err("registry-absent fallback targets must fail closed");

        assert!(
            error
                .to_string()
                .contains("is absent from the agent's effective model registry"),
            "{error}"
        );
        assert!(
            !client
                .active_fallback
                .load(std::sync::atomic::Ordering::SeqCst)
        );
        assert!(model_routing.commits().is_empty());
        assert_eq!(
            agent.tool_scope.visibility_state().unwrap(),
            previous_visibility
        );
    }

    #[tokio::test]
    async fn extraction_turn_does_not_emit_normal_text_or_turn_completion() {
        let schema = crate::types::OutputSchema::new(serde_json::json!({
            "type": "object",
            "properties": {
                "answer": { "type": "string" }
            },
            "required": ["answer"]
        }))
        .unwrap();

        let client = Arc::new(ScriptedExtractionClient::new(vec![
            text_response("main answer"),
            text_response(r#"{"answer": "42"}"#),
        ]));

        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .output_schema(schema)
            .build_standalone(client, Arc::new(NoTools), Arc::new(NoopStore))
            .await;

        let (tx, mut rx) = mpsc::channel::<crate::event::AgentEvent>(32);
        let result = agent
            .run_with_events("prompt".to_string().into(), tx)
            .await
            .expect("extraction should succeed");
        assert_eq!(result.text, "main answer");
        assert_eq!(
            result.structured_output,
            Some(serde_json::json!({"answer": "42"}))
        );

        let mut text_complete_payloads = Vec::new();
        let mut turn_completed_count = 0;
        let mut run_completed_count = 0;
        let mut extraction_succeeded_count = 0;
        while let Ok(event) = rx.try_recv() {
            match event {
                crate::event::AgentEvent::TextComplete { content } => {
                    text_complete_payloads.push(content);
                }
                crate::event::AgentEvent::TurnCompleted { .. } => {
                    turn_completed_count += 1;
                }
                crate::event::AgentEvent::RunCompleted {
                    extraction_required,
                    ..
                } => {
                    assert!(extraction_required);
                    run_completed_count += 1;
                }
                crate::event::AgentEvent::ExtractionSucceeded { .. } => {
                    extraction_succeeded_count += 1;
                }
                _ => {}
            }
        }

        assert_eq!(
            text_complete_payloads,
            vec!["main answer".to_string()],
            "extraction JSON must not leak as TextComplete"
        );
        assert_eq!(
            turn_completed_count, 1,
            "only the main turn should emit TurnCompleted"
        );
        assert_eq!(run_completed_count, 1);
        assert_eq!(extraction_succeeded_count, 1);
    }

    #[tokio::test]
    async fn extraction_llm_failure_emits_extraction_failed_without_run_failed() {
        let schema = crate::types::OutputSchema::new(serde_json::json!({
            "type": "object",
            "properties": {
                "answer": { "type": "string" }
            },
            "required": ["answer"]
        }))
        .unwrap();

        let client = Arc::new(FailingExtractionClient {
            call_count: Mutex::new(0),
        });
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .output_schema(schema)
            .build_standalone(client, Arc::new(NoTools), Arc::new(NoopStore))
            .await;

        let (tx, mut rx) = mpsc::channel::<crate::event::AgentEvent>(32);
        let result = agent
            .run_with_events("prompt".to_string().into(), tx)
            .await
            .expect("extraction LLM failure should not fail the completed main run");

        assert_eq!(result.text, "main answer");
        assert!(result.structured_output.is_none());
        let extraction_error = result
            .extraction_error
            .expect("extraction error should be returned");
        assert!(
            extraction_error.reason.contains("extraction auth failed"),
            "reason should preserve extraction LLM failure: {}",
            extraction_error.reason
        );

        let mut saw_run_completed = false;
        let mut saw_run_failed = false;
        let mut saw_extraction_failed = false;
        while let Ok(event) = rx.try_recv() {
            match event {
                crate::event::AgentEvent::RunCompleted {
                    extraction_required,
                    ..
                } => {
                    assert!(extraction_required);
                    saw_run_completed = true;
                }
                crate::event::AgentEvent::RunFailed { .. } => {
                    saw_run_failed = true;
                }
                crate::event::AgentEvent::ExtractionFailed { reason, .. } => {
                    assert!(reason.contains("extraction auth failed"));
                    saw_extraction_failed = true;
                }
                _ => {}
            }
        }

        assert!(saw_run_completed);
        assert!(saw_extraction_failed);
        assert!(
            !saw_run_failed,
            "extraction failure must not re-fail the run"
        );
    }

    #[tokio::test]
    async fn extraction_tool_call_response_emits_extraction_failed_without_tool_semantics() {
        let schema = crate::types::OutputSchema::new(serde_json::json!({
            "type": "object",
            "properties": {
                "answer": { "type": "string" }
            },
            "required": ["answer"]
        }))
        .unwrap();

        let client = Arc::new(ScriptedExtractionClient::new(vec![
            text_response("main answer"),
            tool_call_response("lookup_answer"),
        ]));
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .output_schema(schema)
            .build_standalone(client, Arc::new(NoTools), Arc::new(NoopStore))
            .await;

        let (tx, mut rx) = mpsc::channel::<crate::event::AgentEvent>(32);
        let result = agent
            .run_with_events("prompt".to_string().into(), tx)
            .await
            .expect("extraction tool calls should not fail the completed main run");

        assert_eq!(result.text, "main answer");
        assert!(result.structured_output.is_none());
        let extraction_error = result
            .extraction_error
            .expect("extraction tool call should become extraction_error");
        assert!(
            extraction_error.reason.contains("returned tool calls"),
            "reason should describe extraction tool-call output: {}",
            extraction_error.reason
        );

        let mut saw_run_completed = false;
        let mut saw_run_failed = false;
        let mut saw_extraction_failed = false;
        let mut saw_tool_call_requested = false;
        let mut saw_tool_execution_started = false;
        while let Ok(event) = rx.try_recv() {
            match event {
                crate::event::AgentEvent::RunCompleted {
                    extraction_required,
                    ..
                } => {
                    assert!(extraction_required);
                    saw_run_completed = true;
                }
                crate::event::AgentEvent::RunFailed { .. } => {
                    saw_run_failed = true;
                }
                crate::event::AgentEvent::ExtractionFailed { reason, .. } => {
                    assert!(reason.contains("returned tool calls"));
                    saw_extraction_failed = true;
                }
                crate::event::AgentEvent::ToolCallRequested { .. } => {
                    saw_tool_call_requested = true;
                }
                crate::event::AgentEvent::ToolExecutionStarted { .. } => {
                    saw_tool_execution_started = true;
                }
                _ => {}
            }
        }

        assert!(saw_run_completed);
        assert!(saw_extraction_failed);
        assert!(
            !saw_run_failed,
            "extraction failure must not re-fail the run"
        );
        assert!(
            !saw_tool_call_requested,
            "extraction tool calls must not emit normal tool-call events"
        );
        assert!(
            !saw_tool_execution_started,
            "extraction tool calls must not dispatch tools"
        );
    }

    #[tokio::test]
    async fn extraction_boundary_denial_emits_extraction_failed_without_run_failed() {
        use crate::hooks::{
            HookDecision, HookEngine, HookEngineError, HookExecutionReport, HookInvocation,
            HookPoint, HookReasonCode,
        };
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct DenySecondBoundary {
            calls: AtomicUsize,
        }

        #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
        #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
        impl HookEngine for DenySecondBoundary {
            async fn execute(
                &self,
                invocation: HookInvocation,
                _overrides: Option<&crate::config::HookRunOverrides>,
            ) -> Result<HookExecutionReport, HookEngineError> {
                if invocation.point != HookPoint::TurnBoundary {
                    return Ok(HookExecutionReport::empty());
                }
                let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
                if call == 1 {
                    return Ok(HookExecutionReport::empty());
                }

                Ok(HookExecutionReport {
                    decision: Some(HookDecision::Deny {
                        hook_id: crate::hooks::HookId::new("deny-extraction-boundary"),
                        reason_code: HookReasonCode::PolicyViolation,
                        message: "deny extraction boundary".to_string(),
                        payload: None,
                    }),
                    ..HookExecutionReport::empty()
                })
            }
        }

        let schema = crate::types::OutputSchema::new(serde_json::json!({
            "type": "object",
            "properties": {
                "answer": { "type": "string" }
            },
            "required": ["answer"]
        }))
        .unwrap();

        let client = Arc::new(ScriptedExtractionClient::new(vec![
            text_response("main answer"),
            text_response(r#"{"answer": "42"}"#),
        ]));
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .output_schema(schema)
            .with_hook_engine(Arc::new(DenySecondBoundary {
                calls: AtomicUsize::new(0),
            }))
            .build_standalone(client, Arc::new(NoTools), Arc::new(NoopStore))
            .await;

        let (tx, mut rx) = mpsc::channel::<crate::event::AgentEvent>(32);
        let result = agent
            .run_with_events("prompt".to_string().into(), tx)
            .await
            .expect("extraction boundary denial should not fail completed main run");

        assert_eq!(result.text, "main answer");
        let extraction_error = result
            .extraction_error
            .expect("extraction boundary denial should become extraction_error");
        assert!(extraction_error.reason.contains("deny extraction boundary"));

        let mut saw_run_completed = false;
        let mut saw_run_failed = false;
        let mut saw_extraction_failed = false;
        while let Ok(event) = rx.try_recv() {
            match event {
                crate::event::AgentEvent::RunCompleted {
                    extraction_required,
                    ..
                } => {
                    assert!(extraction_required);
                    saw_run_completed = true;
                }
                crate::event::AgentEvent::RunFailed { .. } => {
                    saw_run_failed = true;
                }
                crate::event::AgentEvent::ExtractionFailed { reason, .. } => {
                    assert!(reason.contains("deny extraction boundary"));
                    saw_extraction_failed = true;
                }
                _ => {}
            }
        }

        assert!(saw_run_completed);
        assert!(saw_extraction_failed);
        assert!(
            !saw_run_failed,
            "extraction denial must not re-fail the run"
        );
    }

    #[tokio::test]
    async fn extraction_zero_retries_still_runs_extraction_phase() {
        let schema = crate::types::OutputSchema::new(serde_json::json!({
            "type": "object",
            "properties": {
                "answer": { "type": "string" }
            },
            "required": ["answer"]
        }))
        .unwrap();

        let client = Arc::new(ScriptedExtractionClient::new(vec![
            text_response("main answer"),
            text_response("not json"),
        ]));
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .output_schema(schema)
            .structured_output_retries(0)
            .build_standalone(client.clone(), Arc::new(NoTools), Arc::new(NoopStore))
            .await;

        let (tx, mut rx) = mpsc::channel::<crate::event::AgentEvent>(32);
        let result = agent
            .run_with_events("prompt".to_string().into(), tx)
            .await
            .expect("zero retries should still perform one extraction attempt");

        assert_eq!(client.calls_made(), 2);
        assert_eq!(result.text, "main answer");
        assert!(result.structured_output.is_none());
        assert!(
            result.extraction_error.is_some(),
            "invalid zero-retry extraction should surface extraction_error"
        );

        let mut text_complete_payloads = Vec::new();
        let mut saw_extraction_failed = false;
        while let Ok(event) = rx.try_recv() {
            match event {
                crate::event::AgentEvent::TextComplete { content } => {
                    text_complete_payloads.push(content);
                }
                crate::event::AgentEvent::ExtractionFailed { .. } => {
                    saw_extraction_failed = true;
                }
                _ => {}
            }
        }

        assert_eq!(
            text_complete_payloads,
            vec!["main answer".to_string()],
            "zero-retry extraction must not leak extraction output as TextComplete"
        );
        assert!(saw_extraction_failed);
    }

    #[tokio::test]
    async fn extraction_does_not_consume_agentic_max_turn_budget() {
        let schema = crate::types::OutputSchema::new(serde_json::json!({
            "type": "object",
            "properties": {
                "answer": { "type": "string" }
            },
            "required": ["answer"]
        }))
        .unwrap();

        let client = Arc::new(ScriptedExtractionClient::new(vec![
            text_response("main answer"),
            text_response(r#"{"answer": "42"}"#),
        ]));
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .output_schema(schema)
            .build_standalone(client, Arc::new(NoTools), Arc::new(NoopStore))
            .await;
        agent.config.max_turns = Some(1);

        let (tx, mut rx) = mpsc::channel::<crate::event::AgentEvent>(32);
        let result = agent
            .run_with_events("prompt".to_string().into(), tx)
            .await
            .expect("max_turns should not block post-run extraction");

        assert_eq!(result.text, "main answer");
        assert_eq!(
            result.structured_output,
            Some(serde_json::json!({"answer": "42"}))
        );
        assert!(result.extraction_error.is_none());

        let mut saw_run_completed = false;
        let mut saw_extraction_succeeded = false;
        let mut saw_run_failed = false;
        while let Ok(event) = rx.try_recv() {
            match event {
                crate::event::AgentEvent::RunCompleted {
                    extraction_required,
                    ..
                } => {
                    assert!(extraction_required);
                    saw_run_completed = true;
                }
                crate::event::AgentEvent::ExtractionSucceeded { .. } => {
                    saw_extraction_succeeded = true;
                }
                crate::event::AgentEvent::RunFailed { .. } => {
                    saw_run_failed = true;
                }
                _ => {}
            }
        }

        assert!(saw_run_completed);
        assert!(saw_extraction_succeeded);
        assert!(
            !saw_run_failed,
            "extraction must not trip agentic max_turns after RunCompleted"
        );
    }

    /// Row #194: when a structured-output run's `compile_schema` fails, the
    /// success-shaped boundary events (`TurnCompleted` / `RunCompleted`) must
    /// NOT be emitted before the extraction-failed terminal. Schema compile is
    /// a fallible extraction-setup step that runs before any success event.
    #[tokio::test]
    async fn compile_schema_failure_emits_no_run_completed_before_terminal() {
        let schema = crate::types::OutputSchema::new(serde_json::json!({
            "type": "object",
            "properties": {
                "answer": { "type": "string" }
            },
            "required": ["answer"]
        }))
        .unwrap();

        let client = Arc::new(CompileSchemaFailingClient);
        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .output_schema(schema)
            .build_standalone(client, Arc::new(NoTools), Arc::new(NoopStore))
            .await;

        let (tx, mut rx) = mpsc::channel::<crate::event::AgentEvent>(32);
        let result = agent
            .run_with_events("prompt".to_string().into(), tx)
            .await
            .expect(
                "compile_schema failure terminalizes as an extraction failure, not a run error",
            );

        // The run terminalizes as an extraction failure (not a success).
        assert!(
            result.extraction_error.is_some(),
            "compile_schema failure must surface as an extraction error"
        );
        assert!(result.structured_output.is_none());

        let mut saw_run_completed = false;
        let mut saw_turn_completed = false;
        let mut saw_extraction_failed = false;
        while let Ok(event) = rx.try_recv() {
            match event {
                crate::event::AgentEvent::RunCompleted { .. } => saw_run_completed = true,
                crate::event::AgentEvent::TurnCompleted { .. } => saw_turn_completed = true,
                crate::event::AgentEvent::ExtractionFailed { .. } => saw_extraction_failed = true,
                _ => {}
            }
        }

        assert!(
            !saw_run_completed,
            "RunCompleted(success) must not precede the extraction-failed terminal"
        );
        assert!(
            !saw_turn_completed,
            "TurnCompleted must not be emitted before the fallible schema compile resolves"
        );
        assert!(
            saw_extraction_failed,
            "compile_schema failure must emit ExtractionFailed"
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
    /// exceeding max retries. The main run should remain successful while the
    /// result carries extraction_error.
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
        // Call 1: extraction attempt 1 — invalid (authority: attempts 0 < retries 2 → retry)
        // Call 2: extraction retry 1 — invalid (authority: attempts 1 < retries 2 → retry)
        // Call 3: extraction retry 2 — invalid (authority: attempts 2 < retries 2 false → exhaust)
        let client = Arc::new(ScriptedExtractionClient::new(vec![
            text_response("Computing count"),
            text_response("bad json 1"),
            text_response("bad json 2"),
            text_response("bad json 3"),
        ]));

        let mut agent = with_test_turn_state_handle(AgentBuilder::new())
            .output_schema(schema)
            .structured_output_retries(2)
            .build_standalone(client.clone(), Arc::new(NoTools), Arc::new(NoopStore))
            .await;

        let result = agent
            .run("Count items".to_string().into())
            .await
            .expect("extraction exhaust should not fail the completed main run");

        assert_eq!(result.text, "Computing count");
        assert!(result.structured_output.is_none());
        let extraction_error = result
            .extraction_error
            .expect("extraction_error should describe exhausted validation");
        assert_eq!(
            extraction_error.attempts, 3,
            "should report validation failure count (initial attempt + retries)"
        );
        assert_eq!(extraction_error.last_output, "Computing count");
        assert!(
            extraction_error.reason.contains("Invalid JSON"),
            "reason should mention JSON parse failure, got: {}",
            extraction_error.reason
        );

        assert_eq!(
            client.calls_made(),
            4,
            "expect 1 agentic + 3 extraction attempts"
        );
    }
}

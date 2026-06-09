//! Crate-private `TurnStateHandle` implementation for `meerkat-core` unit tests.
//!
//! The handle is a thin adapter over the generated MeerkatMachine kernel from
//! `meerkat-machine-kernels`. It exists because core unit tests cannot use
//! `meerkat-runtime::RuntimeTurnStateHandle` without compiling a second copy of
//! `meerkat-core`; lifecycle and terminal facts still change only by applying
//! generated MeerkatMachine inputs.

#![allow(clippy::expect_used)]

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::{
    Mutex, MutexGuard,
    atomic::{AtomicUsize, Ordering},
};

use meerkat_machine_kernels::generated::meerkat;
use meerkat_machine_kernels::test_oracle::{
    GeneratedMachineKernel, KernelEffect, KernelInput, KernelState, KernelValue, TransitionRefusal,
};
use meerkat_machine_schema::identity::{
    EffectVariantId, EnumTypeId, EnumVariantId, FieldId, InputVariantId, NamedTypeId,
};

use crate::handles::{DslTransitionError, TurnStateHandle, TurnStateSnapshot};
use crate::lifecycle::RunId;
use crate::ops::{AsyncOpRef, OperationId, WaitPolicy};
use crate::retry::{LlmRetryFailureKind, LlmRetrySchedule};
use crate::turn_execution_authority::{
    CallTimeoutVerdict, ContentShape, LlmFailureRecoveryKind, TurnExecutionEffect,
    TurnExecutionInput, TurnFailureReason, TurnFailureSource, TurnFailureSourceKind, TurnPhase,
    TurnPrimitiveKind, TurnTerminalCauseKind, TurnTerminalOutcome,
    terminal_outcome_for_budget_exceeded,
};

fn field_id(slug: &str) -> FieldId {
    FieldId::parse(slug).expect("generated MeerkatMachine field id")
}

fn input_id(slug: &str) -> InputVariantId {
    InputVariantId::parse(slug).expect("generated MeerkatMachine input id")
}

fn effect_id(slug: &str) -> EffectVariantId {
    EffectVariantId::parse(slug).expect("generated MeerkatMachine effect id")
}

fn named_type_id(slug: &str) -> NamedTypeId {
    NamedTypeId::parse(slug).expect("generated MeerkatMachine named type id")
}

fn enum_type_id(slug: &str) -> EnumTypeId {
    EnumTypeId::parse(slug).expect("generated MeerkatMachine enum type id")
}

fn enum_variant_id(slug: &str) -> EnumVariantId {
    EnumVariantId::parse(slug).expect("generated MeerkatMachine enum variant id")
}

fn named_string(type_name: &str, value: String) -> KernelValue {
    KernelValue::Named {
        type_name: named_type_id(type_name),
        value: Box::new(KernelValue::String(value)),
    }
}

fn enum_value(enum_name: &str, variant: &str) -> KernelValue {
    KernelValue::NamedVariant {
        enum_name: enum_type_id(enum_name),
        variant: enum_variant_id(variant),
    }
}

fn option_value(value: KernelValue) -> KernelValue {
    KernelValue::Map(BTreeMap::from([(
        KernelValue::String("value".to_string()),
        value,
    )]))
}

/// Absent `Option` value: the kernel's dedicated `None` representation.
fn none_value() -> KernelValue {
    KernelValue::None
}

fn input(
    variant: &str,
    fields: impl IntoIterator<Item = (&'static str, KernelValue)>,
) -> KernelInput {
    KernelInput {
        variant: input_id(variant),
        fields: fields
            .into_iter()
            .map(|(name, value)| (field_id(name), value))
            .collect(),
    }
}

fn run_id_value(run_id: &RunId) -> KernelValue {
    named_string("RunId", run_id.to_string())
}

fn primitive_kind_variant(kind: TurnPrimitiveKind) -> &'static str {
    match kind {
        TurnPrimitiveKind::None => "None",
        TurnPrimitiveKind::ConversationTurn => "ConversationTurn",
        TurnPrimitiveKind::ImmediateAppend => "ImmediateAppend",
        TurnPrimitiveKind::ImmediateContextAppend => "ImmediateContextAppend",
    }
}

fn content_shape_variant(shape: ContentShape) -> &'static str {
    match shape {
        ContentShape::Conversation => "Conversation",
        ContentShape::ConversationAndContext => "ConversationAndContext",
        ContentShape::Context => "Context",
        ContentShape::Empty => "Empty",
        ContentShape::ImmediateAppend => "ImmediateAppend",
        ContentShape::ImmediateContext => "ImmediateContext",
    }
}

fn failure_source_variant(source: TurnFailureSourceKind) -> &'static str {
    match source {
        TurnFailureSourceKind::Unknown => "Unknown",
        TurnFailureSourceKind::Llm => "Llm",
        TurnFailureSourceKind::StoreError => "StoreError",
        TurnFailureSourceKind::ToolError => "ToolError",
        TurnFailureSourceKind::McpError => "McpError",
        TurnFailureSourceKind::SessionNotFound => "SessionNotFound",
        TurnFailureSourceKind::TokenBudgetExceeded => "TokenBudgetExceeded",
        TurnFailureSourceKind::TimeBudgetExceeded => "TimeBudgetExceeded",
        TurnFailureSourceKind::ToolCallBudgetExceeded => "ToolCallBudgetExceeded",
        TurnFailureSourceKind::MaxTokensReached => "MaxTokensReached",
        TurnFailureSourceKind::ContentFiltered => "ContentFiltered",
        TurnFailureSourceKind::MaxTurnsReached => "MaxTurnsReached",
        TurnFailureSourceKind::Cancelled => "Cancelled",
        TurnFailureSourceKind::InvalidStateTransition => "InvalidStateTransition",
        TurnFailureSourceKind::OperationNotFound => "OperationNotFound",
        TurnFailureSourceKind::DepthLimitExceeded => "DepthLimitExceeded",
        TurnFailureSourceKind::ConcurrencyLimitExceeded => "ConcurrencyLimitExceeded",
        TurnFailureSourceKind::ConfigError => "ConfigError",
        TurnFailureSourceKind::InvalidToolAccess => "InvalidToolAccess",
        TurnFailureSourceKind::InternalError => "InternalError",
        TurnFailureSourceKind::BuildError => "BuildError",
        TurnFailureSourceKind::AuthReauthRequired => "AuthReauthRequired",
        TurnFailureSourceKind::CallbackPending => "CallbackPending",
        TurnFailureSourceKind::StructuredOutputValidationFailed => {
            "StructuredOutputValidationFailed"
        }
        TurnFailureSourceKind::InvalidOutputSchema => "InvalidOutputSchema",
        TurnFailureSourceKind::HookDenied => "HookDenied",
        TurnFailureSourceKind::HookTimeout => "HookTimeout",
        TurnFailureSourceKind::HookExecutionFailed => "HookExecutionFailed",
        TurnFailureSourceKind::HookConfigInvalid => "HookConfigInvalid",
        TurnFailureSourceKind::TerminalFailure => "TerminalFailure",
        TurnFailureSourceKind::NoPendingBoundary => "NoPendingBoundary",
        TurnFailureSourceKind::LlmRetryExhausted => "LlmRetryExhausted",
    }
}

fn terminal_outcome_variant(outcome: TurnTerminalOutcome) -> &'static str {
    match outcome {
        TurnTerminalOutcome::None => "None",
        TurnTerminalOutcome::Completed => "Completed",
        TurnTerminalOutcome::Failed => "Failed",
        TurnTerminalOutcome::Cancelled => "Cancelled",
        TurnTerminalOutcome::BudgetExhausted => "BudgetExhausted",
        TurnTerminalOutcome::TimeBudgetExceeded => "TimeBudgetExceeded",
        TurnTerminalOutcome::StructuredOutputValidationFailed => "StructuredOutputValidationFailed",
    }
}

fn retry_failure_variant(kind: LlmRetryFailureKind) -> &'static str {
    match kind {
        LlmRetryFailureKind::RateLimited => "RateLimited",
        LlmRetryFailureKind::NetworkTimeout => "NetworkTimeout",
        LlmRetryFailureKind::CallTimeout => "CallTimeout",
        LlmRetryFailureKind::RetryableProviderError => "RetryableProviderError",
    }
}

fn string_set(values: impl IntoIterator<Item = String>) -> KernelValue {
    KernelValue::Set(values.into_iter().map(KernelValue::String).collect())
}

/// #354: build a `Set<OperationId>` kernel value (each element wrapped in the
/// `OperationId` Named token, mirroring how the generated kernel carries a
/// typed-id set — NOT a bare string set).
fn operation_id_set(values: impl IntoIterator<Item = String>) -> KernelValue {
    KernelValue::Set(
        values
            .into_iter()
            .map(|value| named_string("OperationId", value))
            .collect(),
    )
}

fn state_field<'a>(state: &'a KernelState, name: &str) -> Option<&'a KernelValue> {
    state.fields.get(&field_id(name))
}

fn named_payload<'a>(value: &'a KernelValue, expected_type: &str) -> Option<&'a KernelValue> {
    match value {
        KernelValue::Named { type_name, value } if type_name.as_str() == expected_type => {
            Some(value.as_ref())
        }
        _ => None,
    }
}

fn generated_projection_error(
    context: &'static str,
    reason: impl Into<String>,
) -> DslTransitionError {
    DslTransitionError::guard_rejected(
        context,
        format!(
            "generated MeerkatMachine turn authority projection failed: {}",
            reason.into()
        ),
    )
}

fn required_state_field<'a>(
    state: &'a KernelState,
    name: &str,
    context: &'static str,
) -> Result<&'a KernelValue, DslTransitionError> {
    state_field(state, name).ok_or_else(|| {
        generated_projection_error(context, format!("missing required state field `{name}`"))
    })
}

fn required_effect_field<'a>(
    effect: &'a KernelEffect,
    name: &str,
    context: &'static str,
) -> Result<&'a KernelValue, DslTransitionError> {
    effect.fields.get(&field_id(name)).ok_or_else(|| {
        generated_projection_error(context, format!("missing required effect field `{name}`"))
    })
}

fn required_enum_variant(
    value: &KernelValue,
    enum_name: &str,
    field_name: &str,
    context: &'static str,
) -> Result<String, DslTransitionError> {
    match value {
        KernelValue::NamedVariant {
            enum_name: actual,
            variant,
        } if actual.as_str() == enum_name => Ok(variant.as_str().to_string()),
        _ => Err(generated_projection_error(
            context,
            format!("field `{field_name}` was not generated enum `{enum_name}`"),
        )),
    }
}

fn option_inner<'a>(
    value: &'a KernelValue,
    field_name: &str,
    context: &'static str,
) -> Result<Option<&'a KernelValue>, DslTransitionError> {
    match value {
        KernelValue::None => Ok(None),
        KernelValue::Map(fields) => fields
            .get(&KernelValue::String("value".to_string()))
            .map(Some)
            .ok_or_else(|| {
                generated_projection_error(
                    context,
                    format!("option field `{field_name}` missing generated `value` payload"),
                )
            }),
        _ => Err(generated_projection_error(
            context,
            format!("field `{field_name}` was not a generated option value"),
        )),
    }
}

fn option_enum_variant(
    state: &KernelState,
    name: &str,
    enum_name: &str,
    context: &'static str,
) -> Result<Option<String>, DslTransitionError> {
    required_state_field(state, name, context)
        .and_then(|value| option_inner(value, name, context))
        .and_then(|value| {
            value
                .map(|inner| required_enum_variant(inner, enum_name, name, context))
                .transpose()
        })
}

fn state_enum_variant(
    state: &KernelState,
    name: &str,
    enum_name: &str,
    context: &'static str,
) -> Result<String, DslTransitionError> {
    required_state_field(state, name, context)
        .and_then(|value| required_enum_variant(value, enum_name, name, context))
}

fn option_named_string(
    state: &KernelState,
    name: &str,
    type_name: &str,
    context: &'static str,
) -> Result<Option<String>, DslTransitionError> {
    let Some(value) = option_inner(required_state_field(state, name, context)?, name, context)?
    else {
        return Ok(None);
    };
    let Some(KernelValue::String(value)) = named_payload(value, type_name) else {
        return Err(generated_projection_error(
            context,
            format!("field `{name}` was not generated named string `{type_name}`"),
        ));
    };
    Ok(Some(value.clone()))
}

fn state_bool(
    state: &KernelState,
    name: &str,
    context: &'static str,
) -> Result<bool, DslTransitionError> {
    match required_state_field(state, name, context)? {
        KernelValue::Bool(value) => Ok(*value),
        _ => Err(generated_projection_error(
            context,
            format!("field `{name}` was not bool"),
        )),
    }
}

fn state_u64(
    state: &KernelState,
    name: &str,
    context: &'static str,
) -> Result<u64, DslTransitionError> {
    match required_state_field(state, name, context)? {
        KernelValue::U64(value) => Ok(*value),
        _ => Err(generated_projection_error(
            context,
            format!("field `{name}` was not u64"),
        )),
    }
}

fn state_u32(
    state: &KernelState,
    name: &str,
    context: &'static str,
) -> Result<u32, DslTransitionError> {
    u32::try_from(state_u64(state, name, context)?).map_err(|_| {
        generated_projection_error(context, format!("field `{name}` exceeded u32 range"))
    })
}

fn state_string_set(
    state: &KernelState,
    name: &str,
    context: &'static str,
) -> Result<BTreeSet<String>, DslTransitionError> {
    match required_state_field(state, name, context)? {
        KernelValue::Set(values) => values
            .iter()
            .map(|value| match value {
                KernelValue::String(value) => Ok(value.clone()),
                _ => Err(generated_projection_error(
                    context,
                    format!("field `{name}` contained a non-string set member"),
                )),
            })
            .collect(),
        _ => Err(generated_projection_error(
            context,
            format!("field `{name}` was not a string set"),
        )),
    }
}

/// #354: read a `Set<OperationId>` state field, unwrapping each member from its
/// `OperationId` Named token to the inner string (the kernel projects typed-id
/// set members as `KernelValue::Named`, NOT bare strings).
fn state_operation_id_set(
    state: &KernelState,
    name: &str,
    context: &'static str,
) -> Result<BTreeSet<String>, DslTransitionError> {
    match required_state_field(state, name, context)? {
        KernelValue::Set(values) => values
            .iter()
            .map(|value| match named_payload(value, "OperationId") {
                Some(KernelValue::String(inner)) => Ok(inner.clone()),
                _ => Err(generated_projection_error(
                    context,
                    format!("field `{name}` contained a non-OperationId set member"),
                )),
            })
            .collect(),
        _ => Err(generated_projection_error(
            context,
            format!("field `{name}` was not an OperationId set"),
        )),
    }
}

fn parse_run_id(
    value: &str,
    field_name: &str,
    context: &'static str,
) -> Result<RunId, DslTransitionError> {
    uuid::Uuid::parse_str(value)
        .map(RunId::from_uuid)
        .map_err(|err| {
            generated_projection_error(
                context,
                format!("field `{field_name}` contained malformed run id `{value}`: {err}"),
            )
        })
}

fn parse_operation_id(
    value: &str,
    field_name: &str,
    context: &'static str,
) -> Result<OperationId, DslTransitionError> {
    uuid::Uuid::parse_str(value)
        .map(OperationId)
        .map_err(|err| {
            generated_projection_error(
                context,
                format!("field `{field_name}` contained malformed operation id `{value}`: {err}"),
            )
        })
}

fn map_transition_refusal(err: TransitionRefusal, context: &'static str) -> DslTransitionError {
    match err {
        TransitionRefusal::NoMatchingTransition { .. } => {
            DslTransitionError::no_matching(context, err.to_string())
        }
        _ => DslTransitionError::guard_rejected(context, err.to_string()),
    }
}

fn effect_run_id(
    effect: &KernelEffect,
    field_name: &str,
    context: &'static str,
) -> Result<RunId, DslTransitionError> {
    let value = required_effect_field(effect, field_name, context)?;
    let Some(KernelValue::String(value)) = named_payload(value, "RunId") else {
        return Err(generated_projection_error(
            context,
            format!("effect field `{field_name}` was not generated RunId"),
        ));
    };
    parse_run_id(value, field_name, context)
}

fn effect_u64(
    effect: &KernelEffect,
    field_name: &str,
    context: &'static str,
) -> Result<u64, DslTransitionError> {
    match required_effect_field(effect, field_name, context)? {
        KernelValue::U64(value) => Ok(*value),
        _ => Err(generated_projection_error(
            context,
            format!("effect field `{field_name}` was not u64"),
        )),
    }
}

fn effect_string(
    effect: &KernelEffect,
    field_name: &str,
    context: &'static str,
) -> Result<String, DslTransitionError> {
    match required_effect_field(effect, field_name, context)? {
        KernelValue::String(value) => Ok(value.clone()),
        _ => Err(generated_projection_error(
            context,
            format!("effect field `{field_name}` was not string"),
        )),
    }
}

fn effect_bool(
    effect: &KernelEffect,
    field_name: &str,
    context: &'static str,
) -> Result<bool, DslTransitionError> {
    match required_effect_field(effect, field_name, context)? {
        KernelValue::Bool(value) => Ok(*value),
        _ => Err(generated_projection_error(
            context,
            format!("effect field `{field_name}` was not bool"),
        )),
    }
}

fn effect_enum_variant(
    effect: &KernelEffect,
    field_name: &str,
    enum_name: &str,
    context: &'static str,
) -> Result<String, DslTransitionError> {
    required_effect_field(effect, field_name, context)
        .and_then(|value| required_enum_variant(value, enum_name, field_name, context))
}

fn map_generated_effect(
    effect: KernelEffect,
    context: &'static str,
) -> Result<Option<TurnExecutionEffect>, DslTransitionError> {
    Ok(Some(if effect.variant == effect_id("TurnRunStarted") {
        TurnExecutionEffect::RunStarted {
            run_id: effect_run_id(&effect, "run_id", context)?,
        }
    } else if effect.variant == effect_id("TurnBoundaryApplied") {
        TurnExecutionEffect::BoundaryApplied {
            run_id: effect_run_id(&effect, "run_id", context)?,
            boundary_sequence: effect_u64(&effect, "boundary_sequence", context)?,
        }
    } else if effect.variant == effect_id("TurnRunCompleted") {
        TurnExecutionEffect::RunCompleted {
            run_id: effect_run_id(&effect, "run_id", context)?,
        }
    } else if effect.variant == effect_id("TurnRunFailed") {
        let cause_kind = map_terminal_cause_name(
            &effect_enum_variant(
                &effect,
                "terminal_cause_kind",
                "TurnTerminalCauseKind",
                context,
            )?,
            context,
        )?;
        if !cause_kind.is_specific_failure_cause() {
            return Err(generated_projection_error(
                context,
                "generated TurnRunFailed effect carried unknown terminal_cause_kind",
            ));
        }
        let error = effect_string(&effect, "error", context)?;
        TurnExecutionEffect::RunFailed {
            run_id: effect_run_id(&effect, "run_id", context)?,
            reason: TurnFailureReason::with_cause(
                cause_kind,
                cause_kind.agent_error_class(),
                error,
            ),
        }
    } else if effect.variant == effect_id("TurnRunCancelled") {
        TurnExecutionEffect::RunCancelled {
            run_id: effect_run_id(&effect, "run_id", context)?,
        }
    } else if effect.variant == effect_id("TurnCheckCompaction") {
        TurnExecutionEffect::CheckCompaction
    } else if effect.variant == effect_id("LlmFailureRecoveryClassified") {
        let recovery = effect_enum_variant(&effect, "recovery", "LlmFailureRecoveryKind", context)?;
        TurnExecutionEffect::LlmFailureRecoveryClassified {
            recovery: match recovery.as_str() {
                "Recover" => LlmFailureRecoveryKind::Recover,
                "Exhausted" => LlmFailureRecoveryKind::Exhausted,
                "Fatal" => LlmFailureRecoveryKind::Fatal,
                other => {
                    return Err(generated_projection_error(
                        context,
                        format!("unknown LlmFailureRecoveryKind variant `{other}`"),
                    ));
                }
            },
        }
    } else if effect.variant == effect_id("AssistantOutputClassified") {
        TurnExecutionEffect::AssistantOutputClassified {
            empty_response_terminal: effect_bool(&effect, "empty_response_terminal", context)?,
        }
    } else if effect.variant == effect_id("CallTimeoutClassified") {
        let verdict = effect_enum_variant(&effect, "verdict", "CallTimeoutVerdict", context)?;
        TurnExecutionEffect::CallTimeoutClassified {
            verdict: match verdict.as_str() {
                "RetryableCallTimeout" => CallTimeoutVerdict::RetryableCallTimeout,
                "TerminalTurnBudget" => CallTimeoutVerdict::TerminalTurnBudget,
                other => {
                    return Err(generated_projection_error(
                        context,
                        format!("unknown CallTimeoutVerdict variant `{other}`"),
                    ));
                }
            },
            timeout_ms: effect_u64(&effect, "timeout_ms", context)?,
        }
    } else {
        return Ok(None);
    }))
}

/// Core unit-test handle backed by generated MeerkatMachine authority.
#[derive(Debug)]
pub(crate) struct TestTurnStateHandle {
    kernel: GeneratedMachineKernel,
    state: Mutex<KernelState>,
    run_completed_effects: AtomicUsize,
    run_failed_effects: AtomicUsize,
}

impl TestTurnStateHandle {
    pub(crate) fn new() -> Self {
        let kernel = GeneratedMachineKernel::new(meerkat::schema());
        let initial_state = kernel
            .initial_state()
            .expect("generated MeerkatMachine initial state");
        let state = seed_running_turn_authority(&kernel, initial_state);
        Self {
            kernel,
            state: Mutex::new(state),
            run_completed_effects: AtomicUsize::new(0),
            run_failed_effects: AtomicUsize::new(0),
        }
    }

    fn lock_state(&self) -> Result<MutexGuard<'_, KernelState>, DslTransitionError> {
        self.state.lock().map_err(|_| {
            DslTransitionError::guard_rejected(
                "TestTurnStateHandle",
                "generated state mutex poisoned".to_string(),
            )
        })
    }

    fn apply_with_effects(
        &self,
        input: KernelInput,
        context: &'static str,
    ) -> Result<Vec<TurnExecutionEffect>, DslTransitionError> {
        let mut state = self.lock_state()?;
        let outcome = self
            .kernel
            .transition(&state, &input)
            .map_err(|err| map_transition_refusal(err, context))?;
        *state = outcome.next_state;
        outcome
            .effects
            .into_iter()
            .map(|effect| map_generated_effect(effect, context))
            .filter_map(Result::transpose)
            .collect()
    }

    fn apply(&self, input: KernelInput, context: &'static str) -> Result<(), DslTransitionError> {
        self.apply_with_effects(input, context).map(|_| ())
    }

    /// Mirror the canonical MeerkatMachine turn-terminality verdict over `state`.
    ///
    /// The terminality verdict (which turn phases are terminal) is a machine
    /// fact: this test owner extracts no fact — it drives the read-only
    /// `ClassifyTurnTerminality` input over the supplied DSL state (discarding the
    /// self-loop's next state) and mirrors the emitted
    /// `TurnTerminalityClassified.terminal`. It decides nothing. Fails closed:
    /// an unclassifiable state is treated as terminal.
    fn classify_turn_terminal(&self, state: &KernelState) -> bool {
        let Ok(outcome) = self
            .kernel
            .transition(state, &input("ClassifyTurnTerminality", std::iter::empty()))
        else {
            return true;
        };
        let mut classified = None;
        for effect in &outcome.effects {
            if effect.variant != effect_id("TurnTerminalityClassified") {
                continue;
            }
            let Some(KernelValue::Bool(terminal)) = effect.fields.get(&field_id("terminal")) else {
                return true;
            };
            if classified.replace(*terminal).is_some() {
                return true;
            }
        }
        classified.unwrap_or(true)
    }

    pub(crate) fn run_completed_effect_count(&self) -> usize {
        self.run_completed_effects.load(Ordering::SeqCst)
    }

    pub(crate) fn run_failed_effect_count(&self) -> usize {
        self.run_failed_effects.load(Ordering::SeqCst)
    }
}

impl Default for TestTurnStateHandle {
    fn default() -> Self {
        Self::new()
    }
}

fn seed_running_turn_authority(
    kernel: &GeneratedMachineKernel,
    initial_state: KernelState,
) -> KernelState {
    let seed_run_id = RunId::new();
    kernel
        .transition(
            &initial_state,
            &input(
                "RecoverRuntimeAuthority",
                [
                    (
                        "session_id",
                        named_string("SessionId", "core-test-turn-state".to_string()),
                    ),
                    (
                        "state",
                        enum_value("RuntimeLifecycleObservedState", "Running"),
                    ),
                    ("agent_runtime_id", KernelValue::None),
                    ("fence_token", KernelValue::None),
                    ("runtime_generation", KernelValue::None),
                    ("runtime_epoch_id", KernelValue::None),
                    ("current_run_id", option_value(run_id_value(&seed_run_id))),
                    (
                        "pre_run_phase",
                        option_value(enum_value("PreRunPhase", "Idle")),
                    ),
                    ("silent_intent_overrides", string_set([])),
                ],
            ),
        )
        .expect("generated RecoverRuntimeAuthority should seed test turn authority")
        .next_state
}

impl TurnStateHandle for TestTurnStateHandle {
    fn apply_turn_input(
        &self,
        turn_input: TurnExecutionInput,
    ) -> Result<Vec<TurnExecutionEffect>, DslTransitionError> {
        let context = "TestTurnStateHandle::apply_turn_input";
        let kernel_input = match turn_input {
            TurnExecutionInput::StartConversationRun {
                run_id,
                primitive_kind,
                admitted_content_shape,
                vision_enabled,
                image_tool_results_enabled,
                max_extraction_retries,
            } => input(
                "StartConversationRun",
                [
                    ("run_id", run_id_value(&run_id)),
                    (
                        "primitive_kind",
                        enum_value(
                            "TurnPrimitiveKind",
                            primitive_kind_variant(primitive_kind),
                        ),
                    ),
                    (
                        "admitted_content_shape",
                        enum_value("ContentShape", content_shape_variant(admitted_content_shape)),
                    ),
                    ("vision_enabled", KernelValue::Bool(vision_enabled)),
                    (
                        "image_tool_results_enabled",
                        KernelValue::Bool(image_tool_results_enabled),
                    ),
                    (
                        "max_extraction_retries",
                        KernelValue::U64(max_extraction_retries),
                    ),
                ],
            ),
            TurnExecutionInput::StartImmediateAppend { run_id } => {
                input("StartImmediateAppend", [("run_id", run_id_value(&run_id))])
            }
            TurnExecutionInput::StartImmediateContext { run_id } => {
                input("StartImmediateContext", [("run_id", run_id_value(&run_id))])
            }
            TurnExecutionInput::PrimitiveApplied { run_id } => {
                input("PrimitiveApplied", [("run_id", run_id_value(&run_id))])
            }
            TurnExecutionInput::LlmReturnedToolCalls { run_id, tool_count } => input(
                "LlmReturnedToolCalls",
                [
                    ("run_id", run_id_value(&run_id)),
                    ("tool_count", KernelValue::U64(u64::from(tool_count))),
                ],
            ),
            TurnExecutionInput::LlmReturnedTerminal { run_id } => {
                input("LlmReturnedTerminal", [("run_id", run_id_value(&run_id))])
            }
            TurnExecutionInput::RegisterPendingOps {
                run_id,
                op_refs,
                barrier_operation_ids,
                ..
            } => input(
                "RegisterPendingOps",
                [
                    ("run_id", run_id_value(&run_id)),
                    (
                        "op_refs",
                        string_set(
                            op_refs
                                .into_iter()
                                .map(|op_ref| op_ref.operation_id.to_string()),
                        ),
                    ),
                    (
                        "barrier_operation_ids",
                        operation_id_set(
                            barrier_operation_ids.into_iter().map(|id| id.to_string()),
                        ),
                    ),
                ],
            ),
            TurnExecutionInput::ToolCallsResolved { run_id } => {
                input("ToolCallsResolved", [("run_id", run_id_value(&run_id))])
            }
            TurnExecutionInput::OpsBarrierSatisfied {
                run_id,
                operation_ids,
            } => input(
                "OpsBarrierSatisfied",
                [
                    ("run_id", run_id_value(&run_id)),
                    (
                        "operation_ids",
                        operation_id_set(operation_ids.into_iter().map(|id| id.to_string())),
                    ),
                ],
            ),
            TurnExecutionInput::BoundaryContinue { run_id } => {
                input("BoundaryContinue", [("run_id", run_id_value(&run_id))])
            }
            TurnExecutionInput::BoundaryComplete { run_id } => {
                input("BoundaryComplete", [("run_id", run_id_value(&run_id))])
            }
            TurnExecutionInput::RecoverableFailure { run_id, retry } => input(
                "RecoverableFailure",
                [
                    ("run_id", run_id_value(&run_id)),
                    (
                        "failure_kind",
                        enum_value(
                            "LlmRetryFailureKind",
                            retry_failure_variant(retry.failure.kind),
                        ),
                    ),
                    (
                        "retry_attempt",
                        KernelValue::U64(u64::from(retry.plan.attempt)),
                    ),
                    (
                        "max_retries",
                        KernelValue::U64(u64::from(retry.plan.max_retries)),
                    ),
                    (
                        "selected_delay_ms",
                        KernelValue::U64(retry.plan.selected_delay_ms),
                    ),
                    ("error", KernelValue::String(retry.failure.message)),
                ],
            ),
            TurnExecutionInput::FatalFailure { run_id, failure } => input(
                "FatalFailure",
                [
                    ("run_id", run_id_value(&run_id)),
                    (
                        "terminal_failure_source",
                        enum_value(
                            "RunFailureSourceKind",
                            failure_source_variant(failure.source_kind),
                        ),
                    ),
                    ("error", KernelValue::String(failure.message)),
                ],
            ),
            TurnExecutionInput::RetryRequested {
                run_id,
                retry_attempt,
            } => input(
                "RetryRequested",
                [
                    ("run_id", run_id_value(&run_id)),
                    ("retry_attempt", KernelValue::U64(u64::from(retry_attempt))),
                ],
            ),
            TurnExecutionInput::ClassifyLlmFailureRecovery {
                failure_kind,
                retry_attempt,
                max_retries,
            } => input(
                "ClassifyLlmFailureRecovery",
                [
                    (
                        "failure_kind",
                        match failure_kind {
                            Some(kind) => option_value(enum_value(
                                "LlmRetryFailureKind",
                                retry_failure_variant(kind),
                            )),
                            None => none_value(),
                        },
                    ),
                    ("retry_attempt", KernelValue::U64(u64::from(retry_attempt))),
                    ("max_retries", KernelValue::U64(u64::from(max_retries))),
                ],
            ),
            TurnExecutionInput::ClassifyAssistantOutput {
                has_visible_or_actionable,
            } => input(
                "ClassifyAssistantOutput",
                [(
                    "has_visible_or_actionable",
                    KernelValue::Bool(has_visible_or_actionable),
                )],
            ),
            TurnExecutionInput::ClassifyCallTimeout { source, timeout_ms } => input(
                "ClassifyCallTimeout",
                [
                    (
                        "source",
                        enum_value(
                            "CallTimeoutSource",
                            match source {
                                crate::turn_execution_authority::CallTimeoutSource::CallBudget => {
                                    "CallBudget"
                                }
                                crate::turn_execution_authority::CallTimeoutSource::TurnBudget => {
                                    "TurnBudget"
                                }
                            },
                        ),
                    ),
                    ("timeout_ms", KernelValue::U64(timeout_ms)),
                ],
            ),
            TurnExecutionInput::CancelNow { run_id } => {
                input("CancelNow", [("run_id", run_id_value(&run_id))])
            }
            TurnExecutionInput::CancelAfterBoundary { run_id } => {
                input(
                    "RequestCancelAfterBoundary",
                    [("run_id", run_id_value(&run_id))],
                )
            }
            TurnExecutionInput::CancellationObserved { run_id } => {
                input("CancellationObserved", [("run_id", run_id_value(&run_id))])
            }
            TurnExecutionInput::AcknowledgeTerminal { run_id } => input(
                "AcknowledgeTerminal",
                [
                    ("run_id", run_id_value(&run_id)),
                    (
                        "outcome",
                        enum_value(
                            "TurnTerminalOutcome",
                            terminal_outcome_variant(self.snapshot().terminal_outcome.ok_or_else(
                                || {
                                    DslTransitionError::guard_rejected(
                                        context,
                                        "generated MeerkatMachine terminal outcome missing for AcknowledgeTerminal",
                                    )
                                },
                            )?),
                        ),
                    ),
                ],
            ),
            TurnExecutionInput::TurnLimitReached { run_id } => {
                input("TurnLimitReached", [("run_id", run_id_value(&run_id))])
            }
            TurnExecutionInput::BudgetExhausted { run_id } => {
                input("BudgetExhausted", [("run_id", run_id_value(&run_id))])
            }
            TurnExecutionInput::TimeBudgetExceeded { run_id } => {
                input("TimeBudgetExceeded", [("run_id", run_id_value(&run_id))])
            }
            TurnExecutionInput::BudgetLimitExceeded { run_id, exceeded } => {
                match terminal_outcome_for_budget_exceeded(exceeded) {
                    TurnTerminalOutcome::TimeBudgetExceeded => {
                        input("TimeBudgetExceeded", [("run_id", run_id_value(&run_id))])
                    }
                    TurnTerminalOutcome::BudgetExhausted => {
                        input("BudgetExhausted", [("run_id", run_id_value(&run_id))])
                    }
                    _ => unreachable!("budget exceeded maps only to budget terminal outcomes"),
                }
            }
            TurnExecutionInput::EnterExtraction {
                run_id,
                max_retries,
            } => input(
                "EnterExtraction",
                [
                    ("run_id", run_id_value(&run_id)),
                    (
                        "max_extraction_retries",
                        KernelValue::U64(u64::from(max_retries)),
                    ),
                ],
            ),
            TurnExecutionInput::ExtractionValidationPassed { run_id } => {
                input(
                    "ExtractionValidationPassed",
                    [("run_id", run_id_value(&run_id))],
                )
            }
            TurnExecutionInput::ExtractionValidationFailed { run_id, error } => input(
                "ExtractionValidationFailed",
                [
                    ("run_id", run_id_value(&run_id)),
                    ("error", KernelValue::String(error)),
                ],
            ),
            TurnExecutionInput::ExtractionFailed { run_id, error } => {
                input(
                    "ExtractionFailed",
                    [
                        ("run_id", run_id_value(&run_id)),
                        ("error", KernelValue::String(error)),
                    ],
                )
            }
            TurnExecutionInput::ExtractionStart { run_id } => {
                input("ExtractionStart", [("run_id", run_id_value(&run_id))])
            }
            TurnExecutionInput::ForceCancelNoRun => input("ForceCancelNoRun", []),
        };
        self.apply_with_effects(kernel_input, context)
    }

    fn start_conversation_run(
        &self,
        run_id: RunId,
        primitive_kind: TurnPrimitiveKind,
        admitted_content_shape: ContentShape,
        vision_enabled: bool,
        image_tool_results_enabled: bool,
        max_extraction_retries: u64,
    ) -> Result<(), DslTransitionError> {
        self.apply(
            input(
                "StartConversationRun",
                [
                    ("run_id", run_id_value(&run_id)),
                    (
                        "primitive_kind",
                        enum_value("TurnPrimitiveKind", primitive_kind_variant(primitive_kind)),
                    ),
                    (
                        "admitted_content_shape",
                        enum_value(
                            "ContentShape",
                            content_shape_variant(admitted_content_shape),
                        ),
                    ),
                    ("vision_enabled", KernelValue::Bool(vision_enabled)),
                    (
                        "image_tool_results_enabled",
                        KernelValue::Bool(image_tool_results_enabled),
                    ),
                    (
                        "max_extraction_retries",
                        KernelValue::U64(max_extraction_retries),
                    ),
                ],
            ),
            "TestTurnStateHandle::start_conversation_run",
        )
    }

    fn start_immediate_append(&self, run_id: RunId) -> Result<(), DslTransitionError> {
        self.apply(
            input("StartImmediateAppend", [("run_id", run_id_value(&run_id))]),
            "TestTurnStateHandle::start_immediate_append",
        )
    }

    fn start_immediate_context(&self, run_id: RunId) -> Result<(), DslTransitionError> {
        self.apply(
            input("StartImmediateContext", [("run_id", run_id_value(&run_id))]),
            "TestTurnStateHandle::start_immediate_context",
        )
    }

    fn primitive_applied(&self, run_id: RunId) -> Result<(), DslTransitionError> {
        self.apply(
            input("PrimitiveApplied", [("run_id", run_id_value(&run_id))]),
            "TestTurnStateHandle::primitive_applied",
        )
    }

    fn llm_returned_tool_calls(
        &self,
        run_id: RunId,
        tool_count: u64,
    ) -> Result<(), DslTransitionError> {
        self.apply_turn_input(TurnExecutionInput::LlmReturnedToolCalls {
            run_id,
            tool_count: u32::try_from(tool_count).map_err(|_| {
                DslTransitionError::guard_rejected(
                    "TestTurnStateHandle::llm_returned_tool_calls",
                    "tool_count exceeds u32 turn input range",
                )
            })?,
        })
        .map(|_| ())
    }

    fn llm_returned_terminal(&self, run_id: RunId) -> Result<(), DslTransitionError> {
        self.apply_turn_input(TurnExecutionInput::LlmReturnedTerminal { run_id })
            .map(|_| ())
    }

    fn register_pending_ops(
        &self,
        run_id: RunId,
        op_refs: BTreeSet<AsyncOpRef>,
        barrier_operation_ids: BTreeSet<OperationId>,
    ) -> Result<(), DslTransitionError> {
        let has_barrier_ops = !barrier_operation_ids.is_empty();
        self.apply_turn_input(TurnExecutionInput::RegisterPendingOps {
            run_id,
            op_refs: op_refs.into_iter().collect(),
            barrier_operation_ids: barrier_operation_ids.into_iter().collect(),
            has_barrier_ops,
        })
        .map(|_| ())
    }

    fn tool_calls_resolved(&self, run_id: RunId) -> Result<(), DslTransitionError> {
        self.apply_turn_input(TurnExecutionInput::ToolCallsResolved { run_id })
            .map(|_| ())
    }

    fn ops_barrier_satisfied(
        &self,
        run_id: RunId,
        operation_ids: BTreeSet<OperationId>,
    ) -> Result<(), DslTransitionError> {
        self.apply_turn_input(TurnExecutionInput::OpsBarrierSatisfied {
            run_id,
            operation_ids: operation_ids.into_iter().collect(),
        })
        .map(|_| ())
    }

    fn boundary_continue(&self, run_id: RunId) -> Result<(), DslTransitionError> {
        self.apply_turn_input(TurnExecutionInput::BoundaryContinue { run_id })
            .map(|_| ())
    }

    fn boundary_complete(&self, run_id: RunId) -> Result<(), DslTransitionError> {
        self.apply_turn_input(TurnExecutionInput::BoundaryComplete { run_id })
            .map(|_| ())
    }

    fn enter_extraction(&self, run_id: RunId, max_retries: u32) -> Result<(), DslTransitionError> {
        self.apply_turn_input(TurnExecutionInput::EnterExtraction {
            run_id,
            max_retries,
        })
        .map(|_| ())
    }

    fn extraction_start(&self, run_id: RunId) -> Result<(), DslTransitionError> {
        self.apply_turn_input(TurnExecutionInput::ExtractionStart { run_id })
            .map(|_| ())
    }

    fn extraction_validation_passed(&self, run_id: RunId) -> Result<(), DslTransitionError> {
        self.apply_turn_input(TurnExecutionInput::ExtractionValidationPassed { run_id })
            .map(|_| ())
    }

    fn extraction_validation_failed(
        &self,
        run_id: RunId,
        error: String,
    ) -> Result<(), DslTransitionError> {
        self.apply_turn_input(TurnExecutionInput::ExtractionValidationFailed { run_id, error })
            .map(|_| ())
    }

    fn extraction_failed(&self, run_id: RunId, error: String) -> Result<(), DslTransitionError> {
        self.apply_turn_input(TurnExecutionInput::ExtractionFailed { run_id, error })
            .map(|_| ())
    }

    fn recoverable_failure(
        &self,
        run_id: RunId,
        retry: LlmRetrySchedule,
    ) -> Result<(), DslTransitionError> {
        self.apply_turn_input(TurnExecutionInput::RecoverableFailure { run_id, retry })
            .map(|_| ())
    }

    fn fatal_failure(
        &self,
        run_id: RunId,
        failure: TurnFailureSource,
    ) -> Result<(), DslTransitionError> {
        self.apply_turn_input(TurnExecutionInput::FatalFailure { run_id, failure })
            .map(|_| ())
    }

    fn retry_requested(&self, run_id: RunId, retry_attempt: u32) -> Result<(), DslTransitionError> {
        self.apply_turn_input(TurnExecutionInput::RetryRequested {
            run_id,
            retry_attempt,
        })
        .map(|_| ())
    }

    fn cancel_now(&self, run_id: RunId) -> Result<(), DslTransitionError> {
        self.apply_turn_input(TurnExecutionInput::CancelNow { run_id })
            .map(|_| ())
    }

    fn request_cancel_after_boundary(&self, run_id: RunId) -> Result<(), DslTransitionError> {
        self.apply_turn_input(TurnExecutionInput::CancelAfterBoundary { run_id })
            .map(|_| ())
    }

    fn cancellation_observed(&self, run_id: RunId) -> Result<(), DslTransitionError> {
        self.apply_turn_input(TurnExecutionInput::CancellationObserved { run_id })
            .map(|_| ())
    }

    fn acknowledge_terminal(&self, run_id: RunId) -> Result<(), DslTransitionError> {
        self.apply_turn_input(TurnExecutionInput::AcknowledgeTerminal { run_id })
            .map(|_| ())
    }

    fn turn_limit_reached(&self, run_id: RunId) -> Result<(), DslTransitionError> {
        self.apply_turn_input(TurnExecutionInput::TurnLimitReached { run_id })
            .map(|_| ())
    }

    fn budget_exhausted(&self, run_id: RunId) -> Result<(), DslTransitionError> {
        self.apply_turn_input(TurnExecutionInput::BudgetExhausted { run_id })
            .map(|_| ())
    }

    fn time_budget_exceeded(&self, run_id: RunId) -> Result<(), DslTransitionError> {
        self.apply_turn_input(TurnExecutionInput::TimeBudgetExceeded { run_id })
            .map(|_| ())
    }

    fn force_cancel_no_run(&self) -> Result<(), DslTransitionError> {
        self.apply(
            input("ForceCancelNoRun", []),
            "TestTurnStateHandle::force_cancel_no_run",
        )
    }

    fn run_completed(&self, _run_id: RunId) -> Result<(), DslTransitionError> {
        self.run_completed_effects.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn run_failed(
        &self,
        _run_id: RunId,
        _reason: TurnFailureReason,
    ) -> Result<(), DslTransitionError> {
        self.run_failed_effects.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn run_cancelled(&self, _run_id: RunId) -> Result<(), DslTransitionError> {
        Ok(())
    }

    fn snapshot(&self) -> TurnStateSnapshot {
        const CONTEXT: &str = "TestTurnStateHandle::snapshot";
        let state = self
            .state
            .lock()
            .expect("generated MeerkatMachine test state mutex poisoned");
        (|| -> Result<TurnStateSnapshot, DslTransitionError> {
            let turn_phase = map_turn_phase_name(
                &state_enum_variant(&state, "turn_phase", "TurnPhase", CONTEXT)?,
                CONTEXT,
            )?;
            let barrier_operation_ids =
                state_operation_id_set(&state, "barrier_operation_ids", CONTEXT)?
                    .into_iter()
                    .map(|id| parse_operation_id(&id, "barrier_operation_ids", CONTEXT))
                    .collect::<Result<BTreeSet<_>, _>>()?;
            let pending_op_refs = state_string_set(&state, "pending_op_refs", CONTEXT)?
                .into_iter()
                .map(|id| {
                    let operation_id = parse_operation_id(&id, "pending_op_refs", CONTEXT)?;
                    Ok(AsyncOpRef {
                        wait_policy: if barrier_operation_ids.contains(&operation_id) {
                            WaitPolicy::Barrier
                        } else {
                            WaitPolicy::Detached
                        },
                        operation_id,
                    })
                })
                .collect::<Result<BTreeSet<_>, DslTransitionError>>()?;
            let turn_terminal = self.classify_turn_terminal(&state);
            let active_run_id = match turn_phase {
                TurnPhase::Ready => None,
                _ if turn_terminal => None,
                _ => option_named_string(&state, "current_run_id", "RunId", CONTEXT)?
                    .map(|id| parse_run_id(&id, "current_run_id", CONTEXT))
                    .transpose()?,
            };

            Ok(TurnStateSnapshot {
                active_run_id,
                loop_state: loop_state_from_turn_phase(turn_phase),
                turn_phase,
                turn_terminal,
                primitive_kind: option_enum_variant(
                    &state,
                    "primitive_kind",
                    "TurnPrimitiveKind",
                    CONTEXT,
                )?
                .map(|name| map_primitive_kind_name(&name, CONTEXT))
                .transpose()?
                .flatten(),
                admitted_content_shape: option_enum_variant(
                    &state,
                    "admitted_content_shape",
                    "ContentShape",
                    CONTEXT,
                )?
                .map(|name| map_content_shape_name(&name, CONTEXT))
                .transpose()?,
                vision_enabled: state_bool(&state, "vision_enabled", CONTEXT)?,
                image_tool_results_enabled: state_bool(
                    &state,
                    "image_tool_results_enabled",
                    CONTEXT,
                )?,
                tool_calls_pending: state_u64(&state, "tool_calls_pending", CONTEXT)?,
                pending_op_refs,
                barrier_operation_ids,
                has_barrier_ops: state_bool(&state, "has_barrier_ops", CONTEXT)?,
                barrier_satisfied: state_bool(&state, "barrier_satisfied", CONTEXT)?,
                boundary_count: state_u64(&state, "boundary_count", CONTEXT)?,
                cancel_after_boundary: state_bool(&state, "cancel_after_boundary", CONTEXT)?,
                terminal_outcome: option_enum_variant(
                    &state,
                    "terminal_outcome",
                    "TurnTerminalOutcome",
                    CONTEXT,
                )?
                .map(|name| map_terminal_outcome_name(&name, CONTEXT))
                .transpose()?
                .flatten(),
                terminal_cause_kind: option_enum_variant(
                    &state,
                    "terminal_cause_kind",
                    "TurnTerminalCauseKind",
                    CONTEXT,
                )?
                .map(|name| map_terminal_cause_name(&name, CONTEXT))
                .transpose()?,
                extraction_attempts: state_u64(&state, "extraction_attempts", CONTEXT)?,
                max_extraction_retries: state_u64(&state, "max_extraction_retries", CONTEXT)?,
                llm_retry_attempt: state_u32(&state, "llm_retry_attempt", CONTEXT)?,
                llm_retry_max_retries: state_u32(&state, "llm_retry_max_retries", CONTEXT)?,
                llm_retry_selected_delay_ms: state_u64(
                    &state,
                    "llm_retry_selected_delay_ms",
                    CONTEXT,
                )?,
            })
        })()
        .expect("generated MeerkatMachine turn authority projection should be well formed")
    }
}

fn map_turn_phase_name(name: &str, context: &'static str) -> Result<TurnPhase, DslTransitionError> {
    Ok(match name {
        "Ready" => TurnPhase::Ready,
        "ApplyingPrimitive" => TurnPhase::ApplyingPrimitive,
        "CallingLlm" => TurnPhase::CallingLlm,
        "WaitingForOps" => TurnPhase::WaitingForOps,
        "DrainingBoundary" => TurnPhase::DrainingBoundary,
        "Extracting" => TurnPhase::Extracting,
        "ErrorRecovery" => TurnPhase::ErrorRecovery,
        "Cancelling" => TurnPhase::Cancelling,
        "Completed" => TurnPhase::Completed,
        "Failed" => TurnPhase::Failed,
        "Cancelled" => TurnPhase::Cancelled,
        _ => {
            return Err(generated_projection_error(
                context,
                format!("unknown generated TurnPhase variant `{name}`"),
            ));
        }
    })
}

fn map_primitive_kind_name(
    name: &str,
    context: &'static str,
) -> Result<Option<TurnPrimitiveKind>, DslTransitionError> {
    Ok(Some(match name {
        "None" => return Ok(None),
        "ConversationTurn" => TurnPrimitiveKind::ConversationTurn,
        "ImmediateAppend" => TurnPrimitiveKind::ImmediateAppend,
        "ImmediateContextAppend" => TurnPrimitiveKind::ImmediateContextAppend,
        _ => {
            return Err(generated_projection_error(
                context,
                format!("unknown generated TurnPrimitiveKind variant `{name}`"),
            ));
        }
    }))
}

fn map_content_shape_name(
    name: &str,
    context: &'static str,
) -> Result<ContentShape, DslTransitionError> {
    Ok(match name {
        "Conversation" => ContentShape::Conversation,
        "ConversationAndContext" => ContentShape::ConversationAndContext,
        "Context" => ContentShape::Context,
        "Empty" => ContentShape::Empty,
        "ImmediateAppend" => ContentShape::ImmediateAppend,
        "ImmediateContext" => ContentShape::ImmediateContext,
        _ => {
            return Err(generated_projection_error(
                context,
                format!("unknown generated ContentShape variant `{name}`"),
            ));
        }
    })
}

fn map_terminal_outcome_name(
    name: &str,
    context: &'static str,
) -> Result<Option<TurnTerminalOutcome>, DslTransitionError> {
    Ok(Some(match name {
        "None" => return Ok(None),
        "Completed" => TurnTerminalOutcome::Completed,
        "Failed" => TurnTerminalOutcome::Failed,
        "Cancelled" => TurnTerminalOutcome::Cancelled,
        "BudgetExhausted" => TurnTerminalOutcome::BudgetExhausted,
        "TimeBudgetExceeded" => TurnTerminalOutcome::TimeBudgetExceeded,
        "StructuredOutputValidationFailed" => TurnTerminalOutcome::StructuredOutputValidationFailed,
        _ => {
            return Err(generated_projection_error(
                context,
                format!("unknown generated TurnTerminalOutcome variant `{name}`"),
            ));
        }
    }))
}

fn map_terminal_cause_name(
    name: &str,
    context: &'static str,
) -> Result<TurnTerminalCauseKind, DslTransitionError> {
    Ok(match name {
        "Unknown" => TurnTerminalCauseKind::Unknown,
        "HookDenied" => TurnTerminalCauseKind::HookDenied,
        "HookFailure" => TurnTerminalCauseKind::HookFailure,
        "LlmFailure" => TurnTerminalCauseKind::LlmFailure,
        "ToolFailure" => TurnTerminalCauseKind::ToolFailure,
        "StructuredOutputValidationFailed" => {
            TurnTerminalCauseKind::StructuredOutputValidationFailed
        }
        "BudgetExhausted" => TurnTerminalCauseKind::BudgetExhausted,
        "TimeBudgetExceeded" => TurnTerminalCauseKind::TimeBudgetExceeded,
        "RetryExhausted" => TurnTerminalCauseKind::RetryExhausted,
        "TurnLimitReached" => TurnTerminalCauseKind::TurnLimitReached,
        "RuntimeApplyFailure" => TurnTerminalCauseKind::RuntimeApplyFailure,
        "FatalFailure" => TurnTerminalCauseKind::FatalFailure,
        _ => {
            return Err(generated_projection_error(
                context,
                format!("unknown generated TurnTerminalCauseKind variant `{name}`"),
            ));
        }
    })
}

fn loop_state_from_turn_phase(phase: TurnPhase) -> crate::LoopState {
    match phase {
        TurnPhase::Ready | TurnPhase::ApplyingPrimitive | TurnPhase::CallingLlm => {
            crate::LoopState::CallingLlm
        }
        TurnPhase::WaitingForOps => crate::LoopState::WaitingForOps,
        TurnPhase::DrainingBoundary | TurnPhase::Extracting => crate::LoopState::DrainingEvents,
        TurnPhase::ErrorRecovery => crate::LoopState::ErrorRecovery,
        TurnPhase::Cancelling => crate::LoopState::Cancelling,
        TurnPhase::Completed | TurnPhase::Failed | TurnPhase::Cancelled => {
            crate::LoopState::Completed
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_turn_state_handle_preserves_typed_operation_ids() {
        let handle = TestTurnStateHandle::new();
        let run_id = RunId::new();
        let detached_id = OperationId::new();
        let barrier_id = OperationId::new();

        handle
            .start_conversation_run(
                run_id.clone(),
                TurnPrimitiveKind::ConversationTurn,
                ContentShape::Conversation,
                false,
                false,
                2,
            )
            .expect("start run");
        handle
            .primitive_applied(run_id.clone())
            .expect("primitive applied");
        handle
            .llm_returned_tool_calls(run_id.clone(), 2)
            .expect("tool calls");
        handle
            .register_pending_ops(
                run_id,
                BTreeSet::from([
                    AsyncOpRef::detached(detached_id.clone()),
                    AsyncOpRef::barrier(barrier_id.clone()),
                ]),
                BTreeSet::from([barrier_id.clone()]),
            )
            .expect("register pending ops");

        let snapshot = handle.snapshot();
        assert!(
            snapshot
                .pending_op_refs
                .contains(&AsyncOpRef::detached(detached_id))
        );
        assert!(
            snapshot
                .pending_op_refs
                .contains(&AsyncOpRef::barrier(barrier_id.clone()))
        );
        assert!(snapshot.barrier_operation_ids.contains(&barrier_id));
    }

    /// P0 Dogma Invariant 1: the generated MeerkatMachine kernel — not the
    /// shell `RetryPolicy` — is the authority on retry exhaustion. A
    /// `RecoverableFailure` whose one-based `retry_attempt` exceeds
    /// `max_retries` is rejected by the kernel, so the turn cannot enter
    /// `ErrorRecovery` past exhaustion. The accepting case at the exhaustion
    /// boundary (`retry_attempt == max_retries`) is also covered.
    #[test]
    fn recoverable_failure_past_exhaustion_is_kernel_rejected() {
        use crate::retry::{LlmRetryFailure, LlmRetryFailureKind, LlmRetryPlan, LlmRetrySchedule};

        fn schedule(attempt: u32, max_retries: u32) -> LlmRetrySchedule {
            LlmRetrySchedule {
                failure: LlmRetryFailure {
                    provider: "test".to_string(),
                    kind: LlmRetryFailureKind::RateLimited,
                    retry_after_ms: Some(1_000),
                    duration_ms: None,
                    message: "rate limited".to_string(),
                },
                plan: LlmRetryPlan {
                    attempt,
                    max_retries,
                    computed_delay_ms: 500,
                    selected_delay_ms: 1_000,
                    retry_after_hint_ms: Some(1_000),
                    rate_limit_floor_applied: false,
                    budget_capped: false,
                },
            }
        }

        let handle = TestTurnStateHandle::new();
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
            .expect("start run");
        handle
            .primitive_applied(run_id.clone())
            .expect("primitive applied");

        // attempt 4 with max_retries 3 is past exhaustion → kernel refuses it.
        // The kernel oracle reports the sole matching transition's guard
        // failure as `NoMatchingTransition` (no transition fired); the
        // load-bearing fact is the refusal — the turn never enters recovery.
        handle
            .recoverable_failure(run_id.clone(), schedule(4, 3))
            .expect_err("exhausted retry must be refused by the kernel");
        assert_eq!(handle.snapshot().turn_phase, TurnPhase::CallingLlm);
        assert_eq!(handle.snapshot().llm_retry_attempt, 0);

        // attempt 3 with max_retries 3 is at the exhaustion boundary → accepted.
        handle
            .recoverable_failure(run_id, schedule(3, 3))
            .expect("retry at the exhaustion boundary is legitimate");
        let snapshot = handle.snapshot();
        assert_eq!(snapshot.turn_phase, TurnPhase::ErrorRecovery);
        assert_eq!(snapshot.llm_retry_attempt, 3);
        assert_eq!(snapshot.llm_retry_max_retries, 3);
    }

    #[test]
    fn test_turn_state_handle_preserves_supplied_content_shape() {
        let handle = TestTurnStateHandle::new();

        let run_id = RunId::new();
        handle
            .start_conversation_run(
                run_id.clone(),
                TurnPrimitiveKind::ConversationTurn,
                ContentShape::ConversationAndContext,
                false,
                false,
                0,
            )
            .expect("start run");
        handle.primitive_applied(run_id).expect("primitive applied");

        let snapshot = handle.snapshot();
        assert_eq!(
            snapshot.admitted_content_shape,
            Some(ContentShape::ConversationAndContext)
        );
    }
}

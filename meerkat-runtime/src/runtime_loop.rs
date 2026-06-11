//! RuntimeLoop — per-session tokio task that processes queued inputs.
//!
//! When `MeerkatMachine::accept_input()` queues an input and sets
//! the wake flag, it sends a signal on the wake channel. The RuntimeLoop
//! picks it up, dequeues the input, converts it to a `RunPrimitive`,
//! and applies it via the `CoreExecutor` (which calls `SessionService::start_turn()`
//! under the hood).

use meerkat_core::lifecycle::core_executor::{
    CoreApplyFailureCause, CoreApplyTerminal, CoreExecutorError,
};
use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive, StagedRunInput};
use meerkat_core::lifecycle::{InputId, RunId};
use meerkat_core::turn_execution_authority::ContentShape as TurnContentShape;

use crate::input::Input;
#[cfg(test)]
use crate::input::input_prompt_text;
#[cfg(test)]
use crate::input::runtime_input_projection_for_machine_batch;
use crate::tokio;

/// Extract a prompt string from an `Input`.
#[cfg(test)]
pub(crate) fn input_to_prompt(input: &Input) -> String {
    input_prompt_text(input)
}

/// Canonical runtime-side constructor for [`RuntimeTurnMetadata`].
///
/// This is the ONLY construction site for `RuntimeTurnMetadata` inside
/// `meerkat-runtime/src/`. Any other literal/default construction is an
/// RMAT-governed seam leak; the `turn_metadata_single_construction_site`
/// integration test greps for that invariant at build time.
pub(crate) fn for_input(
    input: &Input,
    semantics: crate::ingress_types::RuntimeInputSemantics,
) -> meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
    use meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata;
    let mut metadata = match input {
        Input::Prompt(prompt) => prompt.turn_metadata.clone().unwrap_or_default(),
        Input::FlowStep(flow_step) => flow_step.turn_metadata.clone().unwrap_or_default(),
        Input::ExternalEvent(event) => RuntimeTurnMetadata {
            handling_mode: Some(event.handling_mode),
            render_metadata: event.render_metadata.clone(),
            ..Default::default()
        },
        Input::Continuation(continuation) => RuntimeTurnMetadata {
            handling_mode: Some(continuation.handling_mode),
            flow_tool_overlay: continuation.flow_tool_overlay.clone(),
            ..Default::default()
        },
        Input::Peer(peer) => RuntimeTurnMetadata {
            handling_mode: peer.handling_mode,
            ..Default::default()
        },
        _ => RuntimeTurnMetadata::default(),
    };
    if let Some(handling_mode) = semantics.execution_handling_mode {
        metadata.handling_mode = Some(handling_mode);
    }
    metadata.execution_kind = Some(semantics.execution_kind);
    metadata.peer_response_terminal_apply_intent = semantics.peer_response_terminal_apply_intent;
    metadata
}

/// Merge the per-input turn metadata carried by a staged batch into a single
/// typed carrier. Scalar conflicts (two inputs disagreeing on e.g. `model`)
/// are refused with a typed error so caller policy is not silently replaced by
/// shell defaults.
pub(crate) fn merge_batch_turn_metadata(
    inputs: &[(meerkat_core::lifecycle::InputId, Input)],
    semantics: &[crate::ingress_types::RuntimeInputSemantics],
) -> Result<
    Option<meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata>,
    meerkat_core::lifecycle::run_primitive::TurnMetadataMergeConflict,
> {
    use meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata;
    if inputs.len() != semantics.len() {
        return Err(
            meerkat_core::lifecycle::run_primitive::TurnMetadataMergeConflict {
                field: "execution_kind",
                reason: "runtime-stamped execution kind missing for one or more inputs",
            },
        );
    }

    let mut acc: Option<RuntimeTurnMetadata> = None;
    for ((_, input), semantics) in inputs.iter().zip(semantics.iter()) {
        let meta = for_input(input, *semantics);
        match acc.as_mut() {
            None => acc = Some(meta),
            Some(existing) => {
                existing.merge(meta)?;
            }
        }
    }
    Ok(acc.filter(|m| !m.is_empty()))
}

fn completion_terminal_observation(
    terminal: Option<&CoreApplyTerminal>,
) -> crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation {
    match terminal {
        Some(CoreApplyTerminal::RunResult(_)) => {
            crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation::RunResult
        }
        Some(CoreApplyTerminal::CallbackPending { .. }) => {
            crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation::CallbackPending
        }
        Some(CoreApplyTerminal::NoPendingBoundary) | None => {
            crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation::NoResult
        }
    }
}

async fn runtime_completion_result_class(
    driver: &crate::meerkat_machine::SharedDriver,
    run_id: Option<&RunId>,
    terminal: crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation,
    finalization: crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation,
) -> Result<
    crate::meerkat_machine::driver::RuntimeCompletionResultAuthority,
    crate::RuntimeDriverError,
> {
    let driver = driver.lock().await;
    crate::meerkat_machine::driver::machine_resolve_runtime_completion_result(
        &driver,
        run_id,
        terminal,
        finalization,
    )
}

async fn resolve_runtime_completion_waiters(
    driver: &crate::meerkat_machine::SharedDriver,
    completions: Option<&crate::meerkat_machine::SharedCompletionRegistry>,
    input_ids: &[InputId],
    run_id: &RunId,
    terminal: Option<&CoreApplyTerminal>,
    finalization: crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation,
    finalization_error: Option<meerkat_core::TurnErrorMetadata>,
) {
    let Some(completions) = completions else {
        return;
    };
    let result_class = runtime_completion_result_class(
        driver,
        Some(run_id),
        completion_terminal_observation(terminal),
        finalization,
    )
    .await;
    let mut registry = completions.lock().await;
    match result_class {
        Ok(result_class) => resolve_completion_waiters_from_authority(
            &mut registry,
            input_ids,
            terminal,
            result_class,
            finalization_error,
        ),
        Err(err) => fail_closed_completion_waiters(
            &mut registry,
            input_ids,
            format!("runtime completion result authority missing: {err}"),
        ),
    }
}

async fn resolve_machine_terminal_completion_waiters(
    driver: &crate::meerkat_machine::SharedDriver,
    completions: Option<&crate::meerkat_machine::SharedCompletionRegistry>,
    input_ids: &[InputId],
    run_id: &RunId,
    reason: String,
) {
    let Some(completions) = completions else {
        return;
    };
    let result_class = runtime_completion_result_class(
        driver,
        Some(run_id),
        crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation::MachineTerminal,
        crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Succeeded,
    )
    .await;
    let error_metadata = machine_terminal_completion_error(driver, reason.clone()).await;
    let mut registry = completions.lock().await;
    match (result_class, error_metadata) {
        (Ok(result_class), Ok(error_metadata)) => resolve_completion_waiters_from_authority(
            &mut registry,
            input_ids,
            None,
            result_class,
            error_metadata,
        ),
        (Err(err), _) | (_, Err(err)) => fail_closed_completion_waiters(
            &mut registry,
            input_ids,
            format!("runtime terminal completion authority missing: {err}"),
        ),
    }
}

async fn runtime_terminated_completion_class(
    driver: &crate::meerkat_machine::SharedDriver,
) -> Result<
    crate::meerkat_machine::driver::RuntimeCompletionResultAuthority,
    crate::RuntimeDriverError,
> {
    runtime_completion_result_class(
        driver,
        None,
        crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation::RuntimeTerminated,
        crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Succeeded,
    )
    .await
}

fn resolve_completion_waiters_from_authority(
    registry: &mut crate::completion::CompletionRegistry,
    input_ids: &[InputId],
    terminal: Option<&CoreApplyTerminal>,
    authority: crate::meerkat_machine::driver::RuntimeCompletionResultAuthority,
    finalization_error: Option<meerkat_core::TurnErrorMetadata>,
) {
    use crate::meerkat_machine::dsl::RuntimeCompletionResultClass;

    match authority.class() {
        RuntimeCompletionResultClass::Completed => {
            let Some(CoreApplyTerminal::RunResult(result)) = terminal else {
                fail_closed_completion_waiters(
                    registry,
                    input_ids,
                    "runtime completion authority resolved Completed without result payload",
                );
                return;
            };
            for input_id in input_ids {
                registry.resolve_completed_authorized(
                    input_id,
                    result.as_ref().clone(),
                    authority.clone(),
                );
            }
        }
        RuntimeCompletionResultClass::CompletedWithoutResult => {
            if !matches!(terminal, Some(CoreApplyTerminal::NoPendingBoundary) | None) {
                fail_closed_completion_waiters(
                    registry,
                    input_ids,
                    "runtime completion authority resolved CompletedWithoutResult with terminal payload",
                );
                return;
            }
            for input_id in input_ids {
                registry.resolve_without_result_authorized(input_id, authority.clone());
            }
        }
        RuntimeCompletionResultClass::CallbackPending => {
            let Some(CoreApplyTerminal::CallbackPending { tool_name, args }) = terminal else {
                fail_closed_completion_waiters(
                    registry,
                    input_ids,
                    "runtime completion authority resolved CallbackPending without callback payload",
                );
                return;
            };
            for input_id in input_ids {
                registry.resolve_callback_pending_authorized(
                    input_id,
                    tool_name.clone(),
                    args.clone(),
                    authority.clone(),
                );
            }
        }
        RuntimeCompletionResultClass::Cancelled => {
            if terminal.is_some() || finalization_error.is_some() {
                fail_closed_completion_waiters(
                    registry,
                    input_ids,
                    "runtime completion authority resolved Cancelled with payload",
                );
                return;
            }
            for input_id in input_ids {
                registry.resolve_cancelled_authorized(input_id, authority.clone());
            }
        }
        RuntimeCompletionResultClass::AbandonedWithError => {
            if matches!(terminal, Some(CoreApplyTerminal::RunResult(_))) {
                fail_closed_completion_waiters(
                    registry,
                    input_ids,
                    "runtime completion authority resolved AbandonedWithError with result payload",
                );
                return;
            }
            let Some(error) = finalization_error else {
                fail_closed_completion_waiters(
                    registry,
                    input_ids,
                    "runtime completion authority resolved AbandonedWithError without typed error",
                );
                return;
            };
            let reason = error
                .detail
                .clone()
                .unwrap_or_else(|| "runtime finalization failed".to_string());
            for input_id in input_ids {
                registry.resolve_abandoned_with_error_authorized(
                    input_id,
                    reason.clone(),
                    error.clone(),
                    authority.clone(),
                );
            }
        }
        RuntimeCompletionResultClass::CompletedWithFinalizationFailure => {
            // The class authority still requires that output was produced (a
            // RunResult terminal), but the produced result is deliberately NOT
            // forwarded to waiters: finalization (durable commit) failed, so the
            // run is not durably terminal and the output must not be surfaced as
            // a usable success result.
            let Some(CoreApplyTerminal::RunResult(_result)) = terminal else {
                fail_closed_completion_waiters(
                    registry,
                    input_ids,
                    "runtime completion authority resolved CompletedWithFinalizationFailure without result payload",
                );
                return;
            };
            let Some(error) = finalization_error else {
                fail_closed_completion_waiters(
                    registry,
                    input_ids,
                    "runtime completion authority resolved finalization failure without typed error",
                );
                return;
            };
            for input_id in input_ids {
                registry.resolve_completed_with_finalization_failure_authorized(
                    input_id,
                    error.clone(),
                    authority.clone(),
                );
            }
        }
        RuntimeCompletionResultClass::RuntimeTerminated => {
            if terminal.is_some() || finalization_error.is_some() {
                fail_closed_completion_waiters(
                    registry,
                    input_ids,
                    "runtime completion authority resolved RuntimeTerminated with payload",
                );
                return;
            }
            registry.resolve_inputs_runtime_terminated(
                input_ids.iter().cloned(),
                "runtime terminated",
                authority,
            );
        }
    }
}

fn fail_closed_completion_waiters(
    registry: &mut crate::completion::CompletionRegistry,
    input_ids: &[InputId],
    reason: impl Into<String>,
) {
    let reason = reason.into();
    registry.fail_inputs(
        input_ids.iter().cloned(),
        crate::completion::CompletionWaitError::AuthorityUnavailable(reason),
    );
}

async fn stop_runtime_loop_executor_from_dsl_effect(
    driver: &crate::meerkat_machine::SharedDriver,
    completions: Option<&crate::meerkat_machine::SharedCompletionRegistry>,
    executor: &mut dyn meerkat_core::lifecycle::CoreExecutor,
    reason: String,
) -> bool {
    let authority = {
        let driver = driver.lock().await;
        driver.shared_dsl_authority()
    };

    let effects = match crate::meerkat_machine::apply_dsl_transition_on_authority(
        &authority,
        crate::meerkat_machine::dsl::MeerkatMachineInput::StopRuntimeExecutor { reason },
        "RuntimeLoopStopRuntimeExecutor",
    ) {
        Ok(effects) => effects,
        Err(error) => {
            tracing::error!(
                error = %error,
                "failed to apply DSL stop-runtime-executor transition after runtime loop snapshot failure"
            );
            return true;
        }
    };

    let projected_effect = match crate::effect::runtime_effect_projection_from_dsl_effects(&effects)
    {
        Ok(effect) => effect,
        Err(error) => {
            tracing::error!(
                error = %error,
                "DSL stop-runtime-executor transition did not emit a runtime effect fact"
            );
            return true;
        }
    };

    match crate::control_plane::apply_executor_effect(
        driver,
        completions,
        executor,
        projected_effect.into_effect(),
    )
    .await
    {
        Ok(should_stop) => should_stop,
        Err(error) => {
            tracing::error!(
                error = %error,
                "failed to apply stop-runtime-executor effect from runtime loop"
            );
            true
        }
    }
}

fn fail_completion_waiters(
    registry: &mut crate::completion::CompletionRegistry,
    input_ids: &[InputId],
    reason: impl Into<String>,
) {
    fail_closed_completion_waiters(registry, input_ids, reason);
}

async fn machine_terminal_completion_error(
    driver: &crate::meerkat_machine::SharedDriver,
    detail: String,
) -> Result<Option<meerkat_core::TurnErrorMetadata>, crate::RuntimeDriverError> {
    let authority = {
        let driver = driver.lock().await;
        driver.shared_dsl_authority()
    };
    let auth = authority
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let outcome = auth.state().terminal_outcome.ok_or_else(|| {
        crate::RuntimeDriverError::Internal(
            "missing generated terminal_outcome for runtime completion".to_string(),
        )
    })?;
    match outcome {
        crate::meerkat_machine::dsl::TurnTerminalOutcome::Cancelled => {
            match auth.state().terminal_cause_kind {
                None | Some(crate::meerkat_machine::dsl::TurnTerminalCauseKind::Unknown) => {
                    Ok(None)
                }
                Some(cause_kind) => Err(crate::RuntimeDriverError::Internal(format!(
                    "generated cancelled terminal carried failure cause {cause_kind:?}"
                ))),
            }
        }
        crate::meerkat_machine::dsl::TurnTerminalOutcome::Failed
        | crate::meerkat_machine::dsl::TurnTerminalOutcome::BudgetExhausted
        | crate::meerkat_machine::dsl::TurnTerminalOutcome::TimeBudgetExceeded
        | crate::meerkat_machine::dsl::TurnTerminalOutcome::StructuredOutputValidationFailed => {
            let cause_kind = auth.state().terminal_cause_kind.ok_or_else(|| {
                crate::RuntimeDriverError::Internal(
                    "missing generated terminal_cause_kind for failed runtime completion"
                        .to_string(),
                )
            })?;
            if cause_kind == crate::meerkat_machine::dsl::TurnTerminalCauseKind::Unknown {
                return Err(crate::RuntimeDriverError::Internal(
                    "unknown generated terminal_cause_kind for failed runtime completion"
                        .to_string(),
                ));
            }
            Ok(Some(meerkat_core::TurnErrorMetadata::terminal(
                meerkat_core::TurnTerminalCauseKind::from(cause_kind),
                meerkat_core::TurnTerminalOutcome::from(outcome),
                detail,
            )))
        }
        crate::meerkat_machine::dsl::TurnTerminalOutcome::None
        | crate::meerkat_machine::dsl::TurnTerminalOutcome::Completed => {
            Err(crate::RuntimeDriverError::Internal(format!(
                "generated terminal outcome {outcome:?} cannot resolve failed runtime completion"
            )))
        }
    }
}

fn primitive_admitted_content_shape(primitive: &RunPrimitive) -> TurnContentShape {
    match primitive {
        RunPrimitive::StagedInput(staged) => TurnContentShape::from_staged_presence(
            !staged.appends.is_empty(),
            !staged.context_appends.is_empty(),
        ),
        RunPrimitive::ImmediateAppend(_) => TurnContentShape::ImmediateAppend,
        RunPrimitive::ImmediateContextAppend(_) => TurnContentShape::ImmediateContext,
        _ => TurnContentShape::Conversation,
    }
}

fn primitive_turn_start_input(
    run_id: &RunId,
    primitive: &RunPrimitive,
) -> Option<crate::meerkat_machine::dsl::MeerkatMachineInput> {
    match primitive {
        RunPrimitive::ImmediateAppend(_) => Some(
            crate::meerkat_machine::dsl::MeerkatMachineInput::StartImmediateAppend {
                run_id: crate::meerkat_machine::dsl::RunId::from_domain(run_id),
            },
        ),
        RunPrimitive::ImmediateContextAppend(_) => Some(
            crate::meerkat_machine::dsl::MeerkatMachineInput::StartImmediateContext {
                run_id: crate::meerkat_machine::dsl::RunId::from_domain(run_id),
            },
        ),
        RunPrimitive::StagedInput(_) if primitive.is_peer_response_terminal_context_and_run() => {
            let admitted_content_shape = crate::meerkat_machine::dsl::ContentShape::from(
                primitive_admitted_content_shape(primitive),
            );
            Some(
                crate::meerkat_machine::dsl::MeerkatMachineInput::StartConversationRun {
                    run_id: crate::meerkat_machine::dsl::RunId::from_domain(run_id),
                    primitive_kind:
                        crate::meerkat_machine::dsl::TurnPrimitiveKind::ConversationTurn,
                    admitted_content_shape,
                    vision_enabled: false,
                    image_tool_results_enabled: false,
                    max_extraction_retries: 0,
                },
            )
        }
        RunPrimitive::StagedInput(_) if primitive.is_context_only_apply_without_turn() => Some(
            crate::meerkat_machine::dsl::MeerkatMachineInput::StartImmediateContext {
                run_id: crate::meerkat_machine::dsl::RunId::from_domain(run_id),
            },
        ),
        RunPrimitive::StagedInput(staged) if staged.appends.is_empty() => None,
        RunPrimitive::StagedInput(_) => {
            let admitted_content_shape = crate::meerkat_machine::dsl::ContentShape::from(
                primitive_admitted_content_shape(primitive),
            );
            Some(
                crate::meerkat_machine::dsl::MeerkatMachineInput::StartConversationRun {
                    run_id: crate::meerkat_machine::dsl::RunId::from_domain(run_id),
                    primitive_kind:
                        crate::meerkat_machine::dsl::TurnPrimitiveKind::ConversationTurn,
                    admitted_content_shape,
                    vision_enabled: false,
                    image_tool_results_enabled: false,
                    max_extraction_retries: 0,
                },
            )
        }
        _ => {
            let admitted_content_shape = crate::meerkat_machine::dsl::ContentShape::from(
                primitive_admitted_content_shape(primitive),
            );
            Some(
                crate::meerkat_machine::dsl::MeerkatMachineInput::StartConversationRun {
                    run_id: crate::meerkat_machine::dsl::RunId::from_domain(run_id),
                    primitive_kind:
                        crate::meerkat_machine::dsl::TurnPrimitiveKind::ConversationTurn,
                    admitted_content_shape,
                    vision_enabled: false,
                    image_tool_results_enabled: false,
                    max_extraction_retries: 0,
                },
            )
        }
    }
}

async fn prepare_turn_state_for_primitive(
    driver: &crate::meerkat_machine::SharedDriver,
    run_id: &RunId,
    primitive: &RunPrimitive,
) -> Result<(), crate::RuntimeDriverError> {
    if let Some(reason) = primitive.peer_response_terminal_apply_intent_violation() {
        return Err(crate::RuntimeDriverError::Internal(reason.to_string()));
    }
    let Some(input) = primitive_turn_start_input(run_id, primitive) else {
        return Ok(());
    };
    let authority = {
        let driver = driver.lock().await;
        driver.shared_dsl_authority()
    };
    let mut auth = authority
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // Retired drain path: `machine_begin_run` treats Retired as
    // `is_retired_drain` and skips the `Prepare` DSL input, leaving
    // phase at Retired. The turn-start transitions
    // (`StartConversationRun{Initializing,Attached}` /
    // `StartImmediate{Append,Context}{Initializing,Attached}`) only
    // guard on Initializing/Attached — no Retired variant. Skip the
    // signal during drain; shell-side `set_control_projection` has
    // already advanced control so `executor.apply` proceeds next.
    if auth.state().lifecycle_phase == crate::meerkat_machine::dsl::MeerkatPhase::Retired {
        return Ok(());
    }
    crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(&mut *auth, input)
        .map(|_| ())
        .map_err(|err| {
            crate::RuntimeDriverError::Internal(format!(
                "failed to start runtime turn state for run {run_id}: {err}"
            ))
        })
}

#[cfg(test)]
pub(crate) fn try_inputs_to_primitive_with_boundary(
    inputs: &[(InputId, Input)],
    boundary: RunApplyBoundary,
    semantics: &[crate::ingress_types::RuntimeInputSemantics],
) -> Result<RunPrimitive, meerkat_core::lifecycle::run_primitive::TurnMetadataMergeConflict> {
    let projections = inputs
        .iter()
        .map(|(_, input)| runtime_input_projection_for_machine_batch(input))
        .collect::<Vec<_>>();
    try_projected_inputs_to_primitive_with_boundary(inputs, &projections, boundary, semantics)
}

pub(crate) fn try_projected_inputs_to_primitive_with_boundary(
    inputs: &[(InputId, Input)],
    projections: &[crate::ingress_types::RuntimeInputProjection],
    boundary: RunApplyBoundary,
    semantics: &[crate::ingress_types::RuntimeInputSemantics],
) -> Result<RunPrimitive, meerkat_core::lifecycle::run_primitive::TurnMetadataMergeConflict> {
    let appends = projections
        .iter()
        .flat_map(|projection| {
            projection
                .append
                .clone()
                .into_iter()
                .chain(projection.additional_appends.clone())
        })
        .collect::<Vec<_>>();
    let context_appends = projections
        .iter()
        .filter_map(|projection| projection.context_append.clone())
        .collect::<Vec<_>>();
    let contributing_input_ids = inputs
        .iter()
        .map(|(input_id, _)| input_id.clone())
        .collect::<Vec<_>>();
    // Merge turn metadata from ALL inputs in the batch, not just the first.
    // Scalar conflicts are typed errors; collection fields accumulate.
    let turn_metadata = merge_batch_turn_metadata(inputs, semantics)?;

    Ok(RunPrimitive::StagedInput(StagedRunInput {
        boundary,
        appends,
        context_appends,
        contributing_input_ids,
        turn_metadata,
    }))
}

#[cfg(test)]
pub(crate) fn inputs_to_primitive_with_boundary(
    inputs: &[(InputId, Input)],
    boundary: RunApplyBoundary,
) -> Result<RunPrimitive, meerkat_core::lifecycle::run_primitive::TurnMetadataMergeConflict> {
    let semantics = fallback_batch_semantics(inputs);
    try_inputs_to_primitive_with_boundary(inputs, boundary, &semantics)
}

#[cfg(test)]
pub(crate) fn inputs_to_primitive(
    inputs: &[(InputId, Input)],
) -> Result<RunPrimitive, meerkat_core::lifecycle::run_primitive::TurnMetadataMergeConflict> {
    let boundary = inputs
        .first()
        .map(|(_, input)| fallback_unadmitted_semantics(input).boundary)
        .unwrap_or(RunApplyBoundary::RunStart);
    inputs_to_primitive_with_boundary(inputs, boundary)
}

#[cfg(test)]
fn fallback_unadmitted_semantics(input: &Input) -> crate::ingress_types::RuntimeInputSemantics {
    crate::ingress_types::RuntimeInputSemantics::try_from_generated_admission(input, true)
        .expect("generated admission semantics")
}

#[cfg(test)]
fn fallback_batch_semantics(
    inputs: &[(InputId, Input)],
) -> Vec<crate::ingress_types::RuntimeInputSemantics> {
    inputs
        .iter()
        .map(|(_, input)| fallback_unadmitted_semantics(input))
        .collect()
}

/// Convert an `Input` + its ID to a `RunPrimitive` for `CoreExecutor::apply()`.
#[cfg(test)]
pub(crate) fn input_to_primitive(
    input: &Input,
    input_id: InputId,
) -> Result<RunPrimitive, meerkat_core::lifecycle::run_primitive::TurnMetadataMergeConflict> {
    inputs_to_primitive(&[(input_id, input.clone())])
}

#[cfg(test)]
pub(crate) fn admitted_input_to_primitive(
    input: &Input,
    input_id: InputId,
    projection: crate::ingress_types::RuntimeInputProjection,
    semantics: crate::ingress_types::RuntimeInputSemantics,
) -> Result<RunPrimitive, meerkat_core::lifecycle::run_primitive::TurnMetadataMergeConflict> {
    try_projected_inputs_to_primitive_with_boundary(
        &[(input_id, input.clone())],
        &[projection],
        semantics.boundary,
        &[semantics],
    )
}

#[derive(Clone)]
struct RuntimeLoopAuthorityBinding {
    machine: std::sync::Weak<crate::meerkat_machine::MeerkatMachine>,
    session_id: meerkat_core::types::SessionId,
    #[cfg(test)]
    detached_test_gate: Option<std::sync::Arc<crate::tokio::sync::Mutex<()>>>,
}

impl RuntimeLoopAuthorityBinding {
    fn new(
        machine: std::sync::Weak<crate::meerkat_machine::MeerkatMachine>,
        session_id: meerkat_core::types::SessionId,
    ) -> Self {
        Self {
            machine,
            session_id,
            #[cfg(test)]
            detached_test_gate: None,
        }
    }

    #[cfg(test)]
    fn detached_for_test() -> Self {
        Self {
            machine: std::sync::Weak::<crate::meerkat_machine::MeerkatMachine>::new(),
            session_id: meerkat_core::types::SessionId::new(),
            detached_test_gate: Some(std::sync::Arc::new(crate::tokio::sync::Mutex::new(()))),
        }
    }

    async fn lock_current_driver_authority(
        &self,
        driver: &crate::meerkat_machine::SharedDriver,
        context: &'static str,
    ) -> Result<crate::tokio::sync::OwnedMutexGuard<()>, crate::traits::RuntimeDriverError> {
        #[cfg(test)]
        if let Some(gate) = &self.detached_test_gate {
            let _ = (driver, context);
            return Ok(std::sync::Arc::clone(gate).lock_owned().await);
        }

        let machine =
            self.machine
                .upgrade()
                .ok_or(crate::traits::RuntimeDriverError::NotReady {
                    state: crate::runtime_state::RuntimeState::Destroyed,
                })?;
        machine
            .lock_current_runtime_loop_driver_authority(&self.session_id, driver)
            .await
            .map_err(|err| {
                tracing::warn!(
                    session_id = %self.session_id,
                    error = %err,
                    context,
                    "runtime loop refused stale session driver authority"
                );
                err
            })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FeedWakeOutcome {
    Noop,
    Injected,
    StaleAuthority,
}

/// Spawn the per-session runtime loop with optional completion registry.
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_runtime_loop_with_completions(
    driver: crate::meerkat_machine::SharedDriver,
    mut executor: Box<dyn meerkat_core::lifecycle::CoreExecutor>,
    mut wake_rx: tokio::sync::mpsc::Receiver<()>,
    mut effect_rx: tokio::sync::mpsc::Receiver<crate::effect::RuntimeEffect>,
    completions: Option<crate::meerkat_machine::SharedCompletionRegistry>,
    completion_feed: Option<std::sync::Arc<dyn meerkat_core::completion_feed::CompletionFeed>>,
    ops_lifecycle: Option<std::sync::Arc<dyn meerkat_core::ops_lifecycle::OpsLifecycleRegistry>>,
    epoch_cursor_state: Option<std::sync::Arc<meerkat_core::EpochCursorState>>,
    machine_weak: std::sync::Weak<crate::meerkat_machine::MeerkatMachine>,
    session_id: meerkat_core::types::SessionId,
) -> tokio::task::JoinHandle<()> {
    #[cfg(test)]
    let authority_binding = if machine_weak.strong_count() == 0 {
        RuntimeLoopAuthorityBinding::detached_for_test()
    } else {
        RuntimeLoopAuthorityBinding::new(machine_weak, session_id)
    };
    #[cfg(not(test))]
    let authority_binding = RuntimeLoopAuthorityBinding::new(machine_weak, session_id);
    tokio::spawn(async move {
        // Feed-based idle wake state (local to this loop).
        // Seed from generated cursor authority when available. Even an
        // all-zero runtime-owned cursor must win over the feed watermark so a
        // fresh runtime loop cannot silently skip background completions that
        // land before the task reaches its first select iteration.
        let initial_watermark = match ops_lifecycle.as_ref() {
            Some(registry) => {
                let obs = match registry.completion_cursor(
                    meerkat_core::ops_lifecycle::CompletionCursorConsumer::RuntimeObserved,
                ) {
                    Ok(value) => value,
                    Err(err) => {
                        tracing::error!(
                            session_id = %authority_binding.session_id,
                            error = %err,
                            "runtime loop refused to seed idle-wake watermark from poisoned ops cursor authority"
                        );
                        return;
                    }
                };
                let inj = match registry.completion_cursor(
                    meerkat_core::ops_lifecycle::CompletionCursorConsumer::RuntimeInjected,
                ) {
                    Ok(value) => value,
                    Err(err) => {
                        tracing::error!(
                            session_id = %authority_binding.session_id,
                            error = %err,
                            "runtime loop refused to seed idle-wake watermark from poisoned ops cursor authority"
                        );
                        return;
                    }
                };
                // `Ok(None)` from either consumer means no generated cursor
                // authority for that consumer; treat the missing value as 0 so
                // the watermark falls back to the legitimate no-authority floor.
                obs.unwrap_or(0).max(inj.unwrap_or(0))
            }
            None => 0,
        };
        let mut observed_seq: meerkat_core::completion_feed::CompletionSeq = initial_watermark;
        let mut last_injected_seq: meerkat_core::completion_feed::CompletionSeq =
            match ops_lifecycle.as_ref() {
                Some(registry) => match registry.completion_cursor(
                    meerkat_core::ops_lifecycle::CompletionCursorConsumer::RuntimeInjected,
                ) {
                    Ok(Some(value)) if value > 0 => value,
                    Ok(_) => initial_watermark,
                    Err(err) => {
                        tracing::error!(
                            session_id = %authority_binding.session_id,
                            error = %err,
                            "runtime loop refused to seed idle-wake watermark from poisoned ops cursor authority"
                        );
                        return;
                    }
                },
                None => initial_watermark,
            };

        loop {
            // Build a future for the idle wake. Backed by the completion feed
            // only when generated ops cursor authority is present; otherwise
            // pends forever because the feed watermark is not delivery
            // authority.
            let idle_wake = async {
                if let (Some(feed), Some(_)) = (completion_feed.as_ref(), ops_lifecycle.as_ref()) {
                    feed.wait_for_advance(observed_seq).await;
                } else {
                    std::future::pending::<()>().await;
                }
            };

            tokio::select! {
                biased;
                maybe_effect = effect_rx.recv() => {
                    match maybe_effect {
                        Some(effect) => {
                            let _authority_guard = match authority_binding
                                .lock_current_driver_authority(
                                    &driver,
                                    "runtime loop direct executor effect",
                                )
                                .await
                            {
                                Ok(guard) => guard,
                                Err(_) => break,
                            };
                            match crate::control_plane::apply_executor_effect(
                                &driver,
                                completions.as_ref(),
                                &mut *executor,
                                effect,
                            )
                            .await
                            {
                                Ok(true) => break,
                                Ok(false) => {}
                                Err(error) => {
                                    tracing::error!(
                                        error = %error,
                                        "failed to apply runtime executor effect"
                                    );
                                    break;
                                }
                            }
                        }
                        None => {
                            // Closed effect receiver is NOT an applied stop —
                            // route through the canonical stop/terminalize
                            // path so the DSL executor-exit, durable
                            // runtime-state commit, and waiter terminalization
                            // all fire (same shape as the ready-effect drain
                            // ChannelClosed arm).
                            let _ = stop_runtime_loop_executor_from_dsl_effect(
                                &driver,
                                completions.as_ref(),
                                &mut *executor,
                                "runtime effect channel closed".to_string(),
                            )
                            .await;
                            break;
                        }
                    }
                }
                maybe_wake = wake_rx.recv() => {
                    match maybe_wake {
                        Some(()) => {
                            if process_queue(
                                &driver,
                                &mut *executor,
                                &mut effect_rx,
                                completions.as_ref(),
                                &authority_binding,
                            )
                            .await
                            {
                                break;
                            }
                            // Secondary wake path: re-check after queue drain.
                            // If a completion arrived during process_queue, inject
                            // and immediately process the continuation.
                            match maybe_inject_feed_wake(
                                &driver,
                                completion_feed.as_deref(),
                                &mut observed_seq,
                                &mut last_injected_seq,
                                epoch_cursor_state.as_deref(),
                                ops_lifecycle.as_deref(),
                                &authority_binding,
                            )
                            .await
                            {
                                FeedWakeOutcome::Injected => {
                                    if process_queue(
                                        &driver,
                                        &mut *executor,
                                        &mut effect_rx,
                                        completions.as_ref(),
                                        &authority_binding,
                                    )
                                    .await
                                    {
                                        break;
                                    }
                                }
                                FeedWakeOutcome::StaleAuthority => break,
                                FeedWakeOutcome::Noop => {}
                            }
                        }
                        None => {
                            // Closed wake channel is NOT an applied stop —
                            // terminalize through the canonical
                            // StopRuntimeExecutor path rather than silently
                            // exiting the loop.
                            let _ = stop_runtime_loop_executor_from_dsl_effect(
                                &driver,
                                completions.as_ref(),
                                &mut *executor,
                                "runtime wake channel closed".to_string(),
                            )
                            .await;
                            break;
                        }
                    }
                }
                () = idle_wake => {
                    // A completion arrived while idle. Generated ops authority
                    // classifies whether it should wake this runtime; other
                    // completions already wake through their owning channels.
                    match maybe_inject_feed_wake(
                        &driver,
                        completion_feed.as_deref(),
                        &mut observed_seq,
                        &mut last_injected_seq,
                        epoch_cursor_state.as_deref(),
                        ops_lifecycle.as_deref(),
                        &authority_binding,
                    )
                    .await
                    {
                        FeedWakeOutcome::Injected => {
                            if process_queue(
                                &driver,
                                &mut *executor,
                                &mut effect_rx,
                                completions.as_ref(),
                                &authority_binding,
                            )
                            .await
                            {
                                break;
                            }
                        }
                        FeedWakeOutcome::StaleAuthority => break,
                        FeedWakeOutcome::Noop => {}
                    }
                }
            }
        }

        // Loop exiting — resolve any pending completion waiters as terminated.
        if let Some(ref completions) = completions {
            let result_class = runtime_terminated_completion_class(&driver).await;
            let mut reg = completions.lock().await;
            match result_class {
                Ok(result_class) => {
                    reg.resolve_all_runtime_terminated("runtime loop exited", result_class);
                }
                Err(err) => {
                    let reason = format!("runtime loop exited without completion authority: {err}");
                    reg.fail_all_waiters(
                        crate::completion::CompletionWaitError::AuthorityUnavailable(reason),
                    );
                }
            }
        }
    })
}

/// Check for new background op completions and inject a continuation if needed.
///
/// Called after queue processing completes (session has returned to idle).
/// Cursor-advance plan for the quiescent detached-wake injection tail.
///
/// A successful injection advances BOTH the injected and observed cursors so
/// the completion is recorded as delivered. A failed injection must advance
/// NEITHER: the completion stays visible at the unchanged observed cursor so
/// the next wake re-presents it for retry instead of laundering a failed
/// injection into a watermark advance that hides the completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetachedWakeCursorPlan {
    /// Injection succeeded: advance the injected then the observed cursor.
    AdvanceInjectedAndObserved,
    /// Injection failed: leave both cursors untouched so the completion is
    /// re-presented on the next wake.
    LeaveVisibleForRetry,
}

impl DetachedWakeCursorPlan {
    /// Decide the cursor-advance plan from the injection outcome.
    fn from_injection(injection_succeeded: bool) -> Self {
        if injection_succeeded {
            Self::AdvanceInjectedAndObserved
        } else {
            Self::LeaveVisibleForRetry
        }
    }

    /// Test-only convenience query: the runtime path itself matches on the
    /// plan variants directly (see the `match` at the detached-wake site), so
    /// this boolean projection exists only for the unit tests.
    #[cfg(test)]
    fn advances_observed_cursor(self) -> bool {
        matches!(self, Self::AdvanceInjectedAndObserved)
    }
}

/// Distinguishes ordinary no-op from stale-authority shutdown so cursor
/// projection cannot advance after the runtime loop loses current ownership.
fn advance_runtime_completion_cursor(
    ops_lifecycle: Option<&dyn meerkat_core::ops_lifecycle::OpsLifecycleRegistry>,
    consumer: meerkat_core::ops_lifecycle::CompletionCursorConsumer,
    cursor: meerkat_core::completion_feed::CompletionSeq,
    epoch_cursor_state: Option<&meerkat_core::EpochCursorState>,
) -> Result<meerkat_core::completion_feed::CompletionSeq, FeedWakeOutcome> {
    let Some(registry) = ops_lifecycle else {
        tracing::warn!(
            ?consumer,
            cursor,
            has_epoch_projection = epoch_cursor_state.is_some(),
            "runtime loop refused completion cursor advance without generated ops authority"
        );
        return Err(FeedWakeOutcome::StaleAuthority);
    };
    registry
        .advance_completion_cursor(consumer, cursor, epoch_cursor_state)
        .map_err(|err| {
            tracing::warn!(
                ?consumer,
                cursor,
                error = %err,
                "generated ops authority rejected runtime completion cursor advance"
            );
            FeedWakeOutcome::StaleAuthority
        })
}

fn batch_has_generated_wake_completion(
    batch: &meerkat_core::completion_feed::CompletionBatch,
    last_injected_seq: meerkat_core::completion_feed::CompletionSeq,
    registry: &dyn meerkat_core::ops_lifecycle::OpsLifecycleRegistry,
) -> Result<bool, FeedWakeOutcome> {
    for entry in batch
        .entries
        .iter()
        .filter(|entry| entry.seq > last_injected_seq)
    {
        match registry.classify_operation_completion_wake(&entry.operation_id, entry.kind) {
            Ok(meerkat_core::ops_lifecycle::OperationCompletionWakeClass::Wake) => {
                return Ok(true);
            }
            Ok(meerkat_core::ops_lifecycle::OperationCompletionWakeClass::Ignore) => {}
            Err(err) => {
                tracing::warn!(
                    operation_id = %entry.operation_id,
                    kind = ?entry.kind,
                    error = %err,
                    "generated completion-wake authority rejected runtime feed entry"
                );
                return Err(FeedWakeOutcome::StaleAuthority);
            }
        }
    }
    Ok(false)
}

async fn maybe_inject_feed_wake(
    driver: &crate::meerkat_machine::SharedDriver,
    feed: Option<&dyn meerkat_core::completion_feed::CompletionFeed>,
    observed_seq: &mut meerkat_core::completion_feed::CompletionSeq,
    last_injected_seq: &mut meerkat_core::completion_feed::CompletionSeq,
    epoch_cursor_state: Option<&meerkat_core::EpochCursorState>,
    ops_lifecycle: Option<&dyn meerkat_core::ops_lifecycle::OpsLifecycleRegistry>,
    authority_binding: &RuntimeLoopAuthorityBinding,
) -> FeedWakeOutcome {
    let Some(feed) = feed else {
        return FeedWakeOutcome::Noop;
    };
    let Some(registry) = ops_lifecycle else {
        tracing::warn!(
            "runtime loop refused completion feed wake without generated ops cursor authority"
        );
        return FeedWakeOutcome::StaleAuthority;
    };
    let batch = feed.list_since(*observed_seq);

    let has_new_wake_completion =
        match batch_has_generated_wake_completion(&batch, *last_injected_seq, registry) {
            Ok(has_wake_completion) => has_wake_completion,
            Err(outcome) => return outcome,
        };

    let Ok(_authority_guard) = authority_binding
        .lock_current_driver_authority(driver, "runtime loop feed wake")
        .await
    else {
        return FeedWakeOutcome::StaleAuthority;
    };

    if !has_new_wake_completion {
        // No generated wake-worthy completions: advance to prevent hot-spin
        // on observe-only entries.
        let advanced = match advance_runtime_completion_cursor(
            Some(registry),
            meerkat_core::ops_lifecycle::CompletionCursorConsumer::RuntimeObserved,
            batch.watermark,
            epoch_cursor_state,
        ) {
            Ok(cursor) => cursor,
            Err(outcome) => return outcome,
        };
        *observed_seq = advanced;
        return FeedWakeOutcome::Noop;
    }

    // Verify quiescence before injecting.
    let d = driver.lock().await;
    if !d.is_quiescent_for_detached_wake() {
        // Non-quiescent: do NOT advance observed_seq. The completion
        // stays visible for the next wake so it's not permanently lost.
        return FeedWakeOutcome::Noop;
    }
    drop(d);

    let input = crate::input::Input::Continuation(
        crate::input::ContinuationInput::detached_background_op_completed(),
    );
    let mut d = driver.lock().await;
    let injection_succeeded = d.as_driver_mut().accept_input(input).await.is_ok();
    drop(d);

    match DetachedWakeCursorPlan::from_injection(injection_succeeded) {
        DetachedWakeCursorPlan::LeaveVisibleForRetry => {
            // Injection failed: do NOT advance either cursor. Mirror the
            // non-quiescent branch above and leave the completion visible so
            // the next wake re-presents it for retry instead of laundering the
            // failed injection into a watermark advance that hides the
            // completion.
            FeedWakeOutcome::Noop
        }
        DetachedWakeCursorPlan::AdvanceInjectedAndObserved => {
            // Injection succeeded: advance both the injected and observed
            // cursors so the completion is recorded as delivered and the loop
            // does not hot-spin on it.
            let injected = match advance_runtime_completion_cursor(
                Some(registry),
                meerkat_core::ops_lifecycle::CompletionCursorConsumer::RuntimeInjected,
                batch.watermark,
                epoch_cursor_state,
            ) {
                Ok(cursor) => cursor,
                Err(outcome) => return outcome,
            };
            *last_injected_seq = injected;
            let observed = match advance_runtime_completion_cursor(
                Some(registry),
                meerkat_core::ops_lifecycle::CompletionCursorConsumer::RuntimeObserved,
                batch.watermark,
                epoch_cursor_state,
            ) {
                Ok(cursor) => cursor,
                Err(outcome) => return outcome,
            };
            *observed_seq = observed;
            FeedWakeOutcome::Injected
        }
    }
}

/// Process all queued inputs until the queue is empty.
#[allow(clippy::too_many_arguments)]
async fn process_queue(
    driver: &crate::meerkat_machine::SharedDriver,
    executor: &mut dyn meerkat_core::lifecycle::CoreExecutor,
    effect_rx: &mut tokio::sync::mpsc::Receiver<crate::effect::RuntimeEffect>,
    completions: Option<&crate::meerkat_machine::SharedCompletionRegistry>,
    authority_binding: &RuntimeLoopAuthorityBinding,
) -> bool {
    loop {
        let effect_authority_guard = match authority_binding
            .lock_current_driver_authority(driver, "runtime loop executor effects")
            .await
        {
            Ok(guard) => guard,
            Err(_) => return true,
        };
        match crate::control_plane::drain_ready_executor_effects(
            driver,
            completions,
            executor,
            effect_rx,
        )
        .await
        {
            Ok(crate::control_plane::EffectDrainOutcome::AppliedStop) => return true,
            Ok(crate::control_plane::EffectDrainOutcome::Empty) => {}
            Ok(crate::control_plane::EffectDrainOutcome::ChannelClosed) => {
                // The effect sender was dropped: the control plane is gone.
                // Closure is NOT an applied stop — route through the canonical
                // stop/terminalize path so the DSL executor-exit, durable
                // stopped truth, and completion waiters cannot split before the
                // loop exits.
                drop(effect_authority_guard);
                return stop_runtime_loop_executor_from_dsl_effect(
                    driver,
                    completions,
                    executor,
                    "runtime effect channel closed".to_string(),
                )
                .await;
            }
            Err(error) => {
                tracing::error!(
                    error = %error,
                    "failed to drain runtime executor effect"
                );
                return true;
            }
        }
        drop(effect_authority_guard);

        let queue_authority_guard = match authority_binding
            .lock_current_driver_authority(driver, "runtime loop queue processing")
            .await
        {
            Ok(guard) => guard,
            Err(_) => return true,
        };

        // Dequeue and prepare under the driver lock
        let dequeued = {
            let mut d = driver.lock().await;

            // Immediate attached steer can pre-bind the DSL run before waking
            // the loop. The generated classifier owns whether that binding
            // admits queue processing and whether the loop must reuse it.
            let current_run_id = d.current_run_id();
            let queue_plan = match d.runtime_loop_queue_admission(current_run_id.is_some()) {
                Ok(queue_plan) => queue_plan,
                Err(error) => {
                    tracing::error!(
                        error = %error,
                        "failed closed while classifying runtime queue admission"
                    );
                    return false;
                }
            };
            if !queue_plan.can_process_queue() {
                return false;
            }
            let prebound_run_id = if queue_plan.uses_prebound_run() {
                current_run_id
            } else {
                None
            };

            // Ask the ingress authority for the next batch of input IDs.
            // The authority implements steer-first priority and same-boundary
            // batching for steer inputs; queue inputs follow the prompt-aware
            // batching policy.
            let batch_ids = crate::meerkat_machine::machine_select_runtime_loop_batch(&d);

            if batch_ids.is_empty() {
                return false;
            }

            // Dequeue the batch members from the physical queues.
            let staged_inputs: Vec<_> = batch_ids
                .iter()
                .filter_map(|id| d.dequeue_by_id(id))
                .collect();

            if staged_inputs.is_empty() {
                return false;
            }

            let run_id = prebound_run_id.unwrap_or_else(RunId::new);

            // The checked-in Meerkat machine owns the coarse "this dequeued
            // batch has now become a run" transition, including unwind on
            // staging failure.
            let staged_ids: Vec<_> = staged_inputs.iter().map(|(id, _)| id.clone()).collect();

            let contributing_input_ids = staged_inputs
                .iter()
                .map(|(staged_input_id, _)| staged_input_id.clone())
                .collect::<Vec<_>>();
            let semantics =
                crate::meerkat_machine::machine_batch_runtime_semantics(&d, &staged_ids);
            let projections =
                crate::meerkat_machine::machine_batch_primitive_projections(&d, &staged_inputs);
            let primitive = match (semantics, projections) {
                (Some(semantics), Some(projections)) => {
                    let boundary = semantics.first().map(|semantics| semantics.boundary).ok_or(
                        meerkat_core::lifecycle::run_primitive::TurnMetadataMergeConflict {
                            field: "runtime_boundary",
                            reason: "runtime-stamped boundary missing for staged inputs",
                        },
                    );
                    match boundary {
                        Ok(boundary) => try_projected_inputs_to_primitive_with_boundary(
                            &staged_inputs,
                            &projections,
                            boundary,
                            &semantics,
                        ),
                        Err(error) => Err(error),
                    }
                }
                (None, _) => Err(
                    meerkat_core::lifecycle::run_primitive::TurnMetadataMergeConflict {
                        field: "execution_kind",
                        reason: "runtime-stamped execution kind missing for one or more inputs",
                    },
                ),
                // Fail closed alongside semantics: a missing primitive projection
                // (co-recorded with runtime semantics) must reject the batch, not
                // construct a run primitive from a defaulted projection.
                (Some(_), None) => Err(
                    meerkat_core::lifecycle::run_primitive::TurnMetadataMergeConflict {
                        field: "primitive_projection",
                        reason: "runtime-stamped primitive projection missing for one or more inputs",
                    },
                ),
            };
            Some((contributing_input_ids, staged_ids, run_id, primitive))
        };

        match dequeued {
            Some((input_ids, staged_ids, run_id, primitive)) => {
                if let Err(err) = crate::meerkat_machine::prepare_runtime_loop_batch_start(
                    driver,
                    run_id.clone(),
                    &staged_ids,
                )
                .await
                {
                    tracing::error!(%run_id, error = %err, "failed to prepare runtime loop batch");
                    if let Some(completions) = completions.as_ref() {
                        let mut completions = completions.lock().await;
                        fail_completion_waiters(
                            &mut completions,
                            &input_ids,
                            format!("runtime batch preparation failed: {err}"),
                        );
                    }
                    return false;
                }
                let primitive = match primitive {
                    Ok(primitive) => primitive,
                    Err(conflict) => {
                        tracing::error!(
                            %run_id,
                            field = conflict.field,
                            reason = conflict.reason,
                            "batch turn-metadata merge conflict"
                        );
                        if let Err(err) = crate::meerkat_machine::fail_runtime_loop_run(
                            driver,
                            run_id.clone(),
                            CoreApplyFailureCause::primitive_rejected(conflict.to_string()),
                        )
                        .await
                        {
                            tracing::error!(error = %err, "failed to record primitive rejection terminal event");
                            let should_stop = stop_runtime_loop_executor_from_dsl_effect(
                                driver,
                                completions,
                                executor,
                                format!("runtime primitive rejection snapshot failed: {err}"),
                            )
                            .await;
                            if let Some(completions) = completions.as_ref() {
                                let mut completions = completions.lock().await;
                                fail_completion_waiters(
                                    &mut completions,
                                    &input_ids,
                                    format!("runtime primitive rejection snapshot failed: {err}"),
                                );
                            }
                            return should_stop;
                        }
                        resolve_machine_terminal_completion_waiters(
                            driver,
                            completions,
                            &input_ids,
                            &run_id,
                            format!("runtime primitive rejected: {conflict}"),
                        )
                        .await;
                        return false;
                    }
                };
                if let Err(error) =
                    prepare_turn_state_for_primitive(driver, &run_id, &primitive).await
                {
                    tracing::error!(%run_id, error = %error, "failed to start runtime turn state");
                    if let Err(err) = crate::meerkat_machine::fail_runtime_loop_run(
                        driver,
                        run_id.clone(),
                        CoreApplyFailureCause::executor_internal(error.to_string()),
                    )
                    .await
                    {
                        tracing::error!(error = %err, "failed to record turn-state preparation terminal event");
                        let should_stop = stop_runtime_loop_executor_from_dsl_effect(
                            driver,
                            completions,
                            executor,
                            format!("runtime turn-state preparation snapshot failed: {err}"),
                        )
                        .await;
                        if let Some(completions) = completions.as_ref() {
                            let mut completions = completions.lock().await;
                            fail_completion_waiters(
                                &mut completions,
                                &input_ids,
                                format!("runtime turn-state preparation snapshot failed: {err}"),
                            );
                        }
                        return should_stop;
                    }
                    resolve_machine_terminal_completion_waiters(
                        driver,
                        completions,
                        &input_ids,
                        &run_id,
                        format!("runtime turn-state preparation failed: {error}"),
                    )
                    .await;
                    return false;
                }
                drop(queue_authority_guard);

                // Execute outside the driver lock (this calls start_turn, which is slow)
                let result = executor.apply(run_id.clone(), primitive).await;

                // Lock again to update driver state
                let d = driver.lock().await;
                match result {
                    Ok(output) => {
                        drop(d);
                        let terminal_authority_guard = match authority_binding
                            .lock_current_driver_authority(driver, "runtime loop terminal commit")
                            .await
                        {
                            Ok(guard) => guard,
                            Err(_) => {
                                if let Some(completions) = completions.as_ref() {
                                    let mut completions = completions.lock().await;
                                    fail_completion_waiters(
                                        &mut completions,
                                        &input_ids,
                                        "runtime session unregistered before terminal commit",
                                    );
                                }
                                return true;
                            }
                        };
                        let meerkat_core::lifecycle::core_executor::CoreApplyOutput {
                            receipt,
                            session_snapshot,
                            terminal,
                        } = output;
                        let committed_session_snapshot = session_snapshot.clone();
                        if let Err(err) = crate::meerkat_machine::commit_runtime_loop_run(
                            driver,
                            run_id.clone(),
                            input_ids.clone(),
                            receipt,
                            session_snapshot,
                        )
                        .await
                        {
                            tracing::error!(%run_id, error = %err, "failed to commit runtime loop run");
                            let completion_error =
                                meerkat_core::TurnErrorMetadata::runtime_apply_failure(format!(
                                    "runtime loop commit failed: {err}"
                                ));
                            resolve_runtime_completion_waiters(
                                driver,
                                completions,
                                &input_ids,
                                &run_id,
                                terminal.as_ref(),
                                crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Failed,
                                Some(completion_error),
                            )
                            .await;
                            let should_stop = stop_runtime_loop_executor_from_dsl_effect(
                                driver,
                                completions,
                                executor,
                                format!("runtime loop commit failed for run {run_id}: {err}"),
                            )
                            .await;
                            return should_stop;
                        }

                        if let Some(session_snapshot) = committed_session_snapshot.as_deref()
                            && let Err(err) = executor
                                .checkpoint_committed_session_snapshot(session_snapshot)
                                .await
                        {
                            tracing::error!(
                                %run_id,
                                error = %err,
                                "failed to checkpoint committed runtime session snapshot"
                            );
                            let completion_error =
                                meerkat_core::TurnErrorMetadata::runtime_apply_failure(format!(
                                    "runtime session checkpoint failed after commit: {err}"
                                ));
                            resolve_runtime_completion_waiters(
                                driver,
                                completions,
                                &input_ids,
                                &run_id,
                                terminal.as_ref(),
                                crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Failed,
                                Some(completion_error),
                            )
                            .await;
                            let should_stop = stop_runtime_loop_executor_from_dsl_effect(
                                driver,
                                completions,
                                executor,
                                format!(
                                    "runtime session checkpoint failed after commit for run {run_id}: {err}"
                                ),
                            )
                            .await;
                            return should_stop;
                        }

                        // Resolve completion waiters unconditionally
                        resolve_runtime_completion_waiters(
                            driver,
                            completions,
                            &input_ids,
                            &run_id,
                            terminal.as_ref(),
                            crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Succeeded,
                            None,
                        )
                        .await;
                        drop(terminal_authority_guard);
                    }
                    Err(e) => {
                        drop(d);
                        let terminal_authority_guard = match authority_binding
                            .lock_current_driver_authority(driver, "runtime loop terminal failure")
                            .await
                        {
                            Ok(guard) => guard,
                            Err(_) => {
                                if let Some(completions) = completions.as_ref() {
                                    let mut completions = completions.lock().await;
                                    fail_completion_waiters(
                                        &mut completions,
                                        &input_ids,
                                        "runtime session unregistered before terminal failure",
                                    );
                                }
                                return true;
                            }
                        };
                        let cancelled = e.is_cancelled();
                        let error_msg = e.to_string();
                        let terminal_failure = match &e {
                            CoreExecutorError::TerminalFailure {
                                message,
                                ..
                            } => Some(
                                crate::meerkat_machine_types::MeerkatMachineRunFailure::from_machine_terminal_failure(
                                    message.clone(),
                                ),
                            ),
                            _ => None,
                        };
                        let fail_result = if cancelled {
                            crate::meerkat_machine::cancel_runtime_loop_run(driver, run_id.clone())
                                .await
                        } else if let Some(failure) = terminal_failure {
                            crate::meerkat_machine::fail_machine_run(
                                driver,
                                run_id.clone(),
                                failure,
                            )
                            .await
                        } else {
                            crate::meerkat_machine::fail_runtime_loop_run(
                                driver,
                                run_id.clone(),
                                e.apply_failure_cause(),
                            )
                            .await
                        };
                        if let Err(err) = fail_result {
                            tracing::error!(error = %err, "failed to record runtime terminal event");
                            let should_stop = stop_runtime_loop_executor_from_dsl_effect(
                                driver,
                                completions,
                                executor,
                                format!("runtime failure snapshot failed: {err}"),
                            )
                            .await;
                            // Resolve waiter before breaking so callers don't hang.
                            if let Some(completions) = completions.as_ref() {
                                let mut completions = completions.lock().await;
                                fail_completion_waiters(
                                    &mut completions,
                                    &input_ids,
                                    format!("runtime failure snapshot failed: {err}"),
                                );
                            }
                            return should_stop;
                        }
                        // Resolve completion waiter so callers don't hang.
                        let reason = format!("apply failed: {error_msg}");
                        resolve_machine_terminal_completion_waiters(
                            driver,
                            completions,
                            &input_ids,
                            &run_id,
                            reason,
                        )
                        .await;
                        let mut d = driver.lock().await;
                        let should_continue = d.has_queued_input_outside(&input_ids);
                        if should_continue {
                            if let Err(err) = d.defer_queued_inputs_behind_backlog(&input_ids) {
                                tracing::error!(
                                    error = %err,
                                    "failed to defer failed input batch behind backlog"
                                );
                                return false;
                            }
                            d.take_wake_requested();
                        }
                        drop(d);
                        if should_continue {
                            continue;
                        }
                        // Leave the failing input queued for a future wake instead of
                        // hot-looping on the same payload indefinitely.
                        drop(terminal_authority_guard);
                        return false;
                    }
                }
            }
            None => return false, // Queue empty
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::input::*;
    use chrono::Utc;
    use meerkat_core::lifecycle::run_primitive::{
        ConversationAppend, ConversationAppendRole, CoreRenderable, PeerResponseTerminalApplyIntent,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use std::time::Duration;

    use meerkat_core::ops_lifecycle::{
        OperationKind, OperationResult, OperationSource, OperationSpec, OpsLifecycleRegistry,
    };
    use meerkat_core::types::SessionId;

    const TEST_PEER_RESPONSE_ROUTE_ID: &str = "11111111-1111-4111-8111-111111111111";
    const TEST_PEER_RESPONSE_REQUEST_ID: &str = "22222222-2222-4222-8222-222222222222";
    const TEST_PEER_RESPONSE_REQUEST_ID_2: &str = "33333333-3333-4333-8333-333333333333";

    fn background_spec(name: &str) -> OperationSpec {
        OperationSpec {
            id: meerkat_core::ops_lifecycle::OperationId::new(),
            kind: OperationKind::BackgroundToolOp,
            owner_session_id: SessionId::new(),
            display_name: name.into(),
            source_label: "runtime-loop-test".into(),
            operation_source: None,
            child_session_id: None,
            expect_peer_channel: false,
        }
    }

    fn mob_child_spec(name: &str) -> OperationSpec {
        let child_session_id = SessionId::new();
        OperationSpec {
            id: meerkat_core::ops_lifecycle::OperationId::new(),
            kind: OperationKind::MobMemberChild,
            owner_session_id: SessionId::new(),
            display_name: name.into(),
            source_label: "runtime-loop-test".into(),
            operation_source: Some(OperationSource::session_child(child_session_id.clone())),
            child_session_id: Some(child_session_id),
            expect_peer_channel: true,
        }
    }

    fn op_result(id: &meerkat_core::ops_lifecycle::OperationId, content: &str) -> OperationResult {
        OperationResult {
            id: id.clone(),
            content: content.into(),
            is_error: false,
            duration_ms: 42,
            tokens_used: 7,
        }
    }

    fn make_shared_ephemeral_driver(runtime_id: &str) -> crate::meerkat_machine::SharedDriver {
        Arc::new(crate::tokio::sync::Mutex::new(
            crate::meerkat_machine::DriverEntry::Ephemeral(
                crate::driver::ephemeral::EphemeralRuntimeDriver::new(
                    crate::identifiers::LogicalRuntimeId::new(runtime_id),
                ),
            ),
        ))
    }

    fn stop_runtime_executor_effect(reason: &str) -> crate::effect::RuntimeEffect {
        crate::effect::runtime_effect_for_test(
            crate::meerkat_machine::dsl::RuntimeEffectKind::StopRuntimeExecutor,
            reason,
        )
    }

    #[tokio::test]
    async fn runtime_loop_stop_effect_failure_is_fail_closed_from_helper() {
        let driver = make_shared_ephemeral_driver("stop-helper-fail-closed");
        let stop_calls = Arc::new(AtomicUsize::new(0));
        let apply_calls = Arc::new(AtomicUsize::new(0));
        let mut executor = crate::control_plane::test_support::StopFailingExecutor::new(
            Arc::clone(&stop_calls),
            Arc::clone(&apply_calls),
        );

        let should_stop = stop_runtime_loop_executor_from_dsl_effect(
            &driver,
            None,
            &mut executor,
            "snapshot failure should stop the runtime loop".to_string(),
        )
        .await;

        assert!(
            should_stop,
            "stop-effect failures must fail closed by stopping the runtime loop"
        );
        assert_eq!(stop_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            0,
            "stop-effect failure must not fall through to queued work"
        );
    }

    #[tokio::test]
    async fn runtime_loop_drain_effect_failure_is_fail_closed() {
        let driver = make_shared_ephemeral_driver("drain-effect-fail-closed");
        let stop_calls = Arc::new(AtomicUsize::new(0));
        let apply_calls = Arc::new(AtomicUsize::new(0));
        let mut executor = crate::control_plane::test_support::StopFailingExecutor::new(
            Arc::clone(&stop_calls),
            Arc::clone(&apply_calls),
        );
        let (effect_tx, mut effect_rx) = tokio::sync::mpsc::channel(1);
        effect_tx
            .send(stop_runtime_executor_effect("drain failure should stop"))
            .await
            .expect("test effect should enqueue");

        let authority_binding = RuntimeLoopAuthorityBinding::detached_for_test();
        let should_stop = process_queue(
            &driver,
            &mut executor,
            &mut effect_rx,
            None,
            &authority_binding,
        )
        .await;

        assert!(
            should_stop,
            "drained executor-effect failures must stop the runtime loop"
        );
        assert_eq!(stop_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            0,
            "failed stop effect must not be followed by ordinary queue processing"
        );
    }

    /// Regression for #287: dropping the effect sender mid-run must drive the
    /// loop through the canonical stop/terminalize path (StopRuntimeExecutor),
    /// NOT exit bare as if a stop effect had already been applied. Before the
    /// fix, channel closure returned `Ok(true)` directly and `stop_calls`
    /// stayed at 0 — the executor stop was never invoked.
    #[tokio::test]
    async fn runtime_loop_effect_channel_closure_routes_through_stop_path() {
        let driver = make_shared_ephemeral_driver("effect-channel-closed-stop");
        let stop_calls = Arc::new(AtomicUsize::new(0));
        let apply_calls = Arc::new(AtomicUsize::new(0));
        let mut executor = crate::control_plane::test_support::StopFailingExecutor::new(
            Arc::clone(&stop_calls),
            Arc::clone(&apply_calls),
        );
        // Drop the effect sender so the drain observes a disconnected channel
        // without any stop effect ever being delivered.
        let (effect_tx, mut effect_rx) = tokio::sync::mpsc::channel(1);
        drop(effect_tx);

        let authority_binding = RuntimeLoopAuthorityBinding::detached_for_test();
        let should_stop = process_queue(
            &driver,
            &mut executor,
            &mut effect_rx,
            None,
            &authority_binding,
        )
        .await;

        assert!(
            should_stop,
            "a closed effect channel must stop the runtime loop"
        );
        assert_eq!(
            stop_calls.load(Ordering::SeqCst),
            1,
            "channel closure must route through StopRuntimeExecutor, not exit bare"
        );
        assert_eq!(
            apply_calls.load(Ordering::SeqCst),
            0,
            "channel closure must not fall through to ordinary queue processing"
        );
    }

    #[tokio::test]
    async fn runtime_loop_direct_effect_failure_exits_loop_with_channels_open() {
        let driver = make_shared_ephemeral_driver("direct-effect-fail-closed");
        let stop_calls = Arc::new(AtomicUsize::new(0));
        let apply_calls = Arc::new(AtomicUsize::new(0));
        let executor = crate::control_plane::test_support::StopFailingExecutor::new(
            Arc::clone(&stop_calls),
            Arc::clone(&apply_calls),
        );
        let (wake_tx, wake_rx) = tokio::sync::mpsc::channel(1);
        let (effect_tx, effect_rx) = tokio::sync::mpsc::channel(1);
        let handle = spawn_runtime_loop_with_completions(
            driver,
            Box::new(executor),
            wake_rx,
            effect_rx,
            None,
            None,
            None,
            None,
            std::sync::Weak::<crate::meerkat_machine::MeerkatMachine>::new(),
            SessionId::new(),
        );

        effect_tx
            .send(stop_runtime_executor_effect(
                "direct effect failure should stop",
            ))
            .await
            .expect("test effect should enqueue");

        tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .expect("runtime loop must exit after executor-effect failure")
            .expect("runtime loop task should not panic");
        assert_eq!(stop_calls.load(Ordering::SeqCst), 1);
        assert_eq!(apply_calls.load(Ordering::SeqCst), 0);
        drop((wake_tx, effect_tx));
    }

    /// Row #58 residual gate: dropping the effect sender while the loop is
    /// blocked in its main `select!` (NOT inside the ready-effect drain) must
    /// also route through the canonical StopRuntimeExecutor terminalize path —
    /// never exit the loop bare.
    #[tokio::test]
    async fn runtime_loop_direct_effect_channel_closure_routes_through_stop_path() {
        let driver = make_shared_ephemeral_driver("direct-effect-channel-closed");
        let stop_calls = Arc::new(AtomicUsize::new(0));
        let apply_calls = Arc::new(AtomicUsize::new(0));
        let executor = crate::control_plane::test_support::StopFailingExecutor::new(
            Arc::clone(&stop_calls),
            Arc::clone(&apply_calls),
        );
        let (wake_tx, wake_rx) = tokio::sync::mpsc::channel(1);
        let (effect_tx, effect_rx) = tokio::sync::mpsc::channel(1);
        let handle = spawn_runtime_loop_with_completions(
            driver,
            Box::new(executor),
            wake_rx,
            effect_rx,
            None,
            None,
            None,
            None,
            std::sync::Weak::<crate::meerkat_machine::MeerkatMachine>::new(),
            SessionId::new(),
        );

        drop(effect_tx);

        tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .expect("runtime loop must exit after effect channel closure")
            .expect("runtime loop task should not panic");
        assert_eq!(
            stop_calls.load(Ordering::SeqCst),
            1,
            "effect channel closure must route through StopRuntimeExecutor, not exit bare"
        );
        assert_eq!(apply_calls.load(Ordering::SeqCst), 0);
        drop(wake_tx);
    }

    /// Row #58 residual gate (sibling): dropping the wake sender must also
    /// terminalize through the canonical StopRuntimeExecutor path.
    #[tokio::test]
    async fn runtime_loop_wake_channel_closure_routes_through_stop_path() {
        let driver = make_shared_ephemeral_driver("wake-channel-closed");
        let stop_calls = Arc::new(AtomicUsize::new(0));
        let apply_calls = Arc::new(AtomicUsize::new(0));
        let executor = crate::control_plane::test_support::StopFailingExecutor::new(
            Arc::clone(&stop_calls),
            Arc::clone(&apply_calls),
        );
        let (wake_tx, wake_rx) = tokio::sync::mpsc::channel(1);
        let (effect_tx, effect_rx) = tokio::sync::mpsc::channel(1);
        let handle = spawn_runtime_loop_with_completions(
            driver,
            Box::new(executor),
            wake_rx,
            effect_rx,
            None,
            None,
            None,
            None,
            std::sync::Weak::<crate::meerkat_machine::MeerkatMachine>::new(),
            SessionId::new(),
        );

        drop(wake_tx);

        tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .expect("runtime loop must exit after wake channel closure")
            .expect("runtime loop task should not panic");
        assert_eq!(
            stop_calls.load(Ordering::SeqCst),
            1,
            "wake channel closure must route through StopRuntimeExecutor, not exit bare"
        );
        assert_eq!(apply_calls.load(Ordering::SeqCst), 0);
        drop(effect_tx);
    }

    fn make_prompt(text: &str) -> Input {
        Input::Prompt(PromptInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Operator,
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            content: text.into(),
            typed_turn_appends: Vec::new(),
            turn_metadata: None,
        })
    }

    #[test]
    fn input_to_prompt_extracts_text() {
        let input = make_prompt("hello world");
        assert_eq!(input_to_prompt(&input), "hello world");
    }

    #[test]
    fn input_to_prompt_peer() {
        let input = Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: "p".into(),
                    display_identity: Some("Peer P".into()),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: None,
            content: "peer message".into(),
            payload: None,
            handling_mode: None,
        });
        assert_eq!(input_to_prompt(&input), "peer message");
    }

    #[test]
    fn input_to_prompt_peer_message_uses_body_when_projection_text_is_empty() {
        let input = Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: "peer-1".into(),
                    display_identity: None,
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::Message),
            content: "plain body payload".into(),
            payload: None,
            handling_mode: None,
        });

        assert_eq!(input_to_prompt(&input), "plain body payload");
    }

    #[test]
    fn input_to_prompt_peer_request_is_runtime_owned_and_ignores_bogus_body() {
        let input = Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: "11111111-1111-4111-8111-111111111111".into(),
                    display_identity: None,
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::Request {
                request_id: "req-123".into(),
                intent: "checksum_token".into(),
            }),
            content: "stale helper-local comms prose".into(),
            payload: Some(serde_json::json!({"subject": "alpha beta gamma"})),
            handling_mode: None,
        });

        let prompt = input_to_prompt(&input);
        assert!(
            prompt.starts_with("Peer request from peer_id 11111111-1111-4111-8111-111111111111")
        );
        assert!(prompt.contains("\"peer_id\":\"11111111-1111-4111-8111-111111111111\""));
        assert!(prompt.contains("\"in_reply_to\":\"req-123\""));
        assert!(prompt.contains("\"status\":\"completed\""));
        assert!(!prompt.contains("to=\""));
        assert!(prompt.contains("Do not use send_message for this reply."));
    }

    #[test]
    fn machine_batch_projection_peer_response_terminal_is_runtime_owned_from_typed_payload() {
        let input = Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: TEST_PEER_RESPONSE_ROUTE_ID.into(),
                    display_identity: Some("Analyst".into()),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::ResponseTerminal {
                request_id: "018f6f79-7a82-7c4e-a552-a3b86f9630f1".into(),
                status: crate::input::ResponseTerminalStatus::Completed,
            }),
            content: "stale helper-local comms prose".into(),
            payload: Some(serde_json::json!({
                "request_intent": "checksum_token",
                "request_subject": "alpha beta gamma",
                "token": "birch seventeen"
            })),
            handling_mode: None,
        });

        let projection = runtime_input_projection_for_machine_batch(&input);
        let context = projection
            .context_append
            .expect("terminal peer response should project in machine batch");
        let CoreRenderable::SystemNotice { blocks, .. } = context.content else {
            panic!("expected typed terminal context");
        };
        assert!(matches!(
            blocks.first(),
            Some(meerkat_core::types::SystemNoticeBlock::Comms { peer, request_id, status, .. })
                if peer.as_ref().and_then(|peer| peer.display_name.as_deref()) == Some("Analyst")
                    && request_id.as_deref() == Some("018f6f79-7a82-7c4e-a552-a3b86f9630f1")
                    && status.as_deref() == Some("completed")
        ));
    }

    #[test]
    fn machine_batch_projection_peer_response_terminal_omits_payload_key_extraction() {
        // Regression: runtime must not reach into the peer response payload
        // to extract ad-hoc fields (request_intent, token, etc.) and bake
        // them into prompt text. The canonical projection is the typed
        // convention + the Result JSON verbatim; any per-fixture semantics
        // belong to the peer application, not the runtime.
        let input = Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: TEST_PEER_RESPONSE_ROUTE_ID.into(),
                    display_identity: Some("Analyst".into()),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::ResponseTerminal {
                request_id: "018f6f79-7a82-7c4e-a552-a3b86f9630f1".into(),
                status: crate::input::ResponseTerminalStatus::Completed,
            }),
            content: "stale helper-local comms prose".into(),
            payload: Some(serde_json::json!({
                "request_intent": "checksum_token",
                "request_subject": "alpha beta gamma",
                "token": "birch seventeen"
            })),
            handling_mode: None,
        });

        let projection = runtime_input_projection_for_machine_batch(&input);
        let context = projection
            .context_append
            .expect("terminal peer response should project in machine batch");
        let CoreRenderable::SystemNotice { blocks, .. } = context.content else {
            panic!("expected typed terminal context");
        };
        let rendered = blocks
            .first()
            .and_then(|block| match block {
                meerkat_core::types::SystemNoticeBlock::Comms { payload, .. } => {
                    payload.as_ref().map(ToString::to_string)
                }
                _ => None,
            })
            .unwrap_or_default();
        assert!(
            !rendered.contains("Authoritative result fields:"),
            "runtime must not emit scenario-specific authoritative-field hints: {rendered}"
        );
        assert!(
            !rendered.contains("the exact token answer is"),
            "runtime must not coach the model about specific payload values: {rendered}"
        );
        assert!(
            !rendered.contains("request_intent=checksum_token"),
            "runtime must not re-flatten payload keys into prompt text: {rendered}"
        );
    }

    #[test]
    fn input_to_primitive_creates_staged() -> Result<(), String> {
        let input = make_prompt("test prompt");
        let input_id = input.id().clone();
        let primitive = input_to_primitive(&input, input_id.clone())
            .expect("single input metadata cannot conflict");

        let staged = match primitive {
            RunPrimitive::StagedInput(staged) => staged,
            other => return Err(format!("expected staged input, got {other:?}")),
        };
        assert_eq!(staged.boundary, RunApplyBoundary::RunStart);
        assert_eq!(staged.contributing_input_ids, vec![input_id]);
        assert_eq!(staged.appends.len(), 1);
        assert_eq!(staged.appends[0].role, ConversationAppendRole::User);
        match &staged.appends[0].content {
            CoreRenderable::Text { text } => assert_eq!(text, "test prompt"),
            other => return Err(format!("expected text content, got {other:?}")),
        }
        Ok(())
    }

    #[test]
    fn input_to_primitive_preserves_typed_prompt_appends_without_user_text() -> Result<(), String> {
        let typed_append = ConversationAppend {
            role: ConversationAppendRole::SystemNotice,
            content: CoreRenderable::SystemNotice {
                kind: meerkat_core::types::SystemNoticeKind::Comms,
                body: Some("Peer message".to_string()),
                blocks: vec![meerkat_core::types::SystemNoticeBlock::Comms {
                    kind: meerkat_core::types::CommsNoticeKind::Message,
                    direction: meerkat_core::types::SystemNoticeDirection::Incoming,
                    peer: None,
                    request_id: None,
                    intent: None,
                    status: None,
                    summary: Some("Peer message".to_string()),
                    payload: None,
                    content: Vec::new(),
                }],
            },
        };
        let mut input = make_prompt("");
        let Input::Prompt(prompt) = &mut input else {
            return Err("expected prompt".to_string());
        };
        prompt.typed_turn_appends = vec![typed_append.clone()];
        let input_id = input.id().clone();

        let primitive = input_to_primitive(&input, input_id.clone())
            .expect("single input metadata cannot conflict");
        let RunPrimitive::StagedInput(staged) = primitive else {
            return Err("expected staged input".to_string());
        };
        assert_eq!(staged.contributing_input_ids, vec![input_id]);
        assert_eq!(staged.appends, vec![typed_append]);
        Ok(())
    }

    #[test]
    fn staged_conversation_input_starts_conversation_turn_state() -> Result<(), String> {
        let input = make_prompt("test prompt");
        let input_id = input.id().clone();
        let primitive =
            input_to_primitive(&input, input_id).expect("single input metadata cannot conflict");
        let run_id = RunId::new();

        let start_input = primitive_turn_start_input(&run_id, &primitive)
            .ok_or_else(|| "expected staged content input to start turn state".to_string())?;

        match start_input {
            crate::meerkat_machine::dsl::MeerkatMachineInput::StartConversationRun {
                run_id: got_run_id,
                primitive_kind,
                admitted_content_shape,
                ..
            } => {
                assert_eq!(
                    got_run_id,
                    crate::meerkat_machine::dsl::RunId::from_domain(&run_id)
                );
                assert_eq!(
                    primitive_kind,
                    crate::meerkat_machine::dsl::TurnPrimitiveKind::ConversationTurn
                );
                assert_eq!(
                    admitted_content_shape,
                    crate::meerkat_machine::dsl::ContentShape::Conversation
                );
                Ok(())
            }
            other => Err(format!("expected StartConversationRun, got {other:?}")),
        }
    }

    #[test]
    fn peer_response_terminal_forced_immediate_boundary_is_invalid_apply_intent()
    -> Result<(), String> {
        let input = Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: TEST_PEER_RESPONSE_ROUTE_ID.into(),
                    display_identity: Some("Analyst".into()),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::ResponseTerminal {
                request_id: "018f6f79-7a82-7c4e-a552-a3b86f9630f1".into(),
                status: crate::input::ResponseTerminalStatus::Completed,
            }),
            content: "stale helper-local comms prose".into(),
            payload: Some(serde_json::json!({
                "request_intent": "checksum_token",
                "request_subject": "alpha beta gamma",
                "token": "birch seventeen"
            })),
            handling_mode: None,
        });
        let input_id = input.id().clone();

        let primitive = inputs_to_primitive_with_boundary(
            &[(input_id.clone(), input)],
            RunApplyBoundary::Immediate,
        )
        .expect("single input metadata cannot conflict");
        assert!(
            primitive
                .peer_response_terminal_apply_intent_violation()
                .is_some(),
            "terminal peer response with an immediate boundary must fail the typed apply invariant"
        );
        assert!(!primitive.is_context_only_apply_without_turn());
        let staged = match primitive {
            RunPrimitive::StagedInput(staged) => staged,
            other => return Err(format!("expected staged input, got {other:?}")),
        };
        assert_eq!(staged.boundary, RunApplyBoundary::Immediate);
        assert_eq!(staged.contributing_input_ids, vec![input_id]);
        assert_eq!(staged.appends.len(), 1);
        assert_eq!(staged.context_appends.len(), 1);
        assert_eq!(
            staged.context_appends[0].key,
            format!(
                "peer_response_terminal:{TEST_PEER_RESPONSE_ROUTE_ID}:018f6f79-7a82-7c4e-a552-a3b86f9630f1"
            )
        );
        match &staged.context_appends[0].content {
            CoreRenderable::SystemNotice { blocks, .. } => {
                assert!(matches!(
                    blocks.first(),
                    Some(meerkat_core::types::SystemNoticeBlock::Comms { payload, .. })
                        if payload.as_ref().is_some_and(|payload| payload.to_string().contains("birch seventeen"))
                ));
            }
            other => return Err(format!("expected typed content, got {other:?}")),
        }
        Ok(())
    }

    #[test]
    fn peer_response_terminal_input_boundary_matches_machine_boundary() -> Result<(), String> {
        // Row 12 unification: `input_boundary` (typed Input view) and
        // `machine_input_boundary` (ingress handling_mode view) must agree
        // on ResponseTerminal inputs. Canonical answer is RunStart, since
        // ResponseTerminal is forbidden from carrying a handling_mode
        // override and the runtime-loop batch path already stages it as
        // RunStart.
        let input = Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: TEST_PEER_RESPONSE_ROUTE_ID.into(),
                    display_identity: Some("Analyst".into()),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::ResponseTerminal {
                request_id: "018f6f79-7a82-7c4e-a552-a3b86f9630f1".into(),
                status: crate::input::ResponseTerminalStatus::Completed,
            }),
            content: "done".into(),
            payload: Some(serde_json::json!({
                "request_intent": "checksum_token",
                "request_subject": "alpha beta gamma",
                "token": "birch seventeen"
            })),
            handling_mode: None,
        });

        let primitive = inputs_to_primitive(&[(input.id().clone(), input)])
            .expect("single input metadata cannot conflict");
        let staged = match primitive {
            RunPrimitive::StagedInput(staged) => staged,
            other => return Err(format!("expected staged input, got {other:?}")),
        };
        assert_eq!(staged.boundary, RunApplyBoundary::RunStart);
        // Terminal peer responses carry both the typed comms notice append
        // (the model-visible content of the mandatory requester reaction
        // turn) and the keyed runtime context append.
        assert_eq!(staged.appends.len(), 1);
        assert_eq!(staged.context_appends.len(), 1);
        Ok(())
    }

    #[test]
    fn queued_peer_response_terminal_reaction_turn_has_model_visible_content() -> Result<(), String>
    {
        // Regression lock for the live mob bug where a block-less terminal
        // peer response staged a context-only primitive: the mandatory
        // `AppendContextAndRun` reaction turn then ran with a fabricated
        // empty user prompt, which Anthropic rejects with HTTP 400
        // ("user messages must have non-empty content"). The terminal
        // LlmFailure tore down the member's live session (and its comms
        // runtime), which later surfaced as
        // "wire requires comms runtime for '<peer>'" during respawn.
        let input = Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: TEST_PEER_RESPONSE_ROUTE_ID.into(),
                    display_identity: Some("analyst-rt".into()),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::ResponseTerminal {
                request_id: TEST_PEER_RESPONSE_REQUEST_ID.into(),
                status: crate::input::ResponseTerminalStatus::Completed,
            }),
            content: String::new().into(),
            payload: None,
            handling_mode: None,
        });
        let primitive = inputs_to_primitive(&[(input.id().clone(), input)])
            .expect("single input metadata cannot conflict");
        assert!(primitive.is_peer_response_terminal_context_and_run());
        let staged = match &primitive {
            RunPrimitive::StagedInput(staged) => staged,
            other => return Err(format!("expected staged input, got {other:?}")),
        };
        assert_eq!(
            staged.appends.len(),
            1,
            "block-less terminal response must carry its typed comms notice append"
        );
        let provider_prompt =
            meerkat_core::lifecycle::run_primitive::model_projection_content_input_from_conversation_appends(
                &staged.appends,
            );
        assert!(
            !provider_prompt.text_content().trim().is_empty(),
            "the mandatory requester reaction turn must have non-empty model-visible content"
        );
        Ok(())
    }

    #[test]
    fn queued_peer_response_terminal_starts_requester_reaction_turn() -> Result<(), String> {
        let input = Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: TEST_PEER_RESPONSE_ROUTE_ID.into(),
                    display_identity: Some("analyst-rt".into()),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::ResponseTerminal {
                request_id: TEST_PEER_RESPONSE_REQUEST_ID.into(),
                status: crate::input::ResponseTerminalStatus::Completed,
            }),
            content: String::new().into(),
            payload: Some(serde_json::json!({
                "request_intent": "checksum_token",
                "request_subject": "alpha beta gamma",
                "token": "birch seventeen"
            })),
            handling_mode: None,
        });
        let primitive = inputs_to_primitive(&[(input.id().clone(), input)])
            .expect("single input metadata cannot conflict");
        let run_id = RunId::new();

        let start_input = primitive_turn_start_input(&run_id, &primitive).ok_or_else(|| {
            "terminal response should start a requester reaction turn".to_string()
        })?;

        match start_input {
            crate::meerkat_machine::dsl::MeerkatMachineInput::StartConversationRun {
                run_id: got_run_id,
                primitive_kind,
                admitted_content_shape,
                ..
            } => {
                assert_eq!(
                    got_run_id,
                    crate::meerkat_machine::dsl::RunId::from_domain(&run_id)
                );
                assert_eq!(
                    primitive_kind,
                    crate::meerkat_machine::dsl::TurnPrimitiveKind::ConversationTurn
                );
                assert_eq!(
                    admitted_content_shape,
                    crate::meerkat_machine::dsl::ContentShape::ConversationAndContext
                );
                Ok(())
            }
            other => Err(format!("expected StartConversationRun, got {other:?}")),
        }
    }

    #[test]
    fn peer_response_terminal_apply_intent_is_policy_runtime_and_executor_consistent()
    -> Result<(), String> {
        let input = Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: TEST_PEER_RESPONSE_ROUTE_ID.into(),
                    display_identity: Some("analyst-rt".into()),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::ResponseTerminal {
                request_id: TEST_PEER_RESPONSE_REQUEST_ID.into(),
                status: crate::input::ResponseTerminalStatus::Completed,
            }),
            content: String::new().into(),
            payload: Some(serde_json::json!({
                "request_intent": "checksum_token",
                "request_subject": "alpha beta gamma",
                "token": "birch seventeen"
            })),
            handling_mode: None,
        });

        let policy = crate::policy_table::DefaultPolicyTable::resolve(&input, true);
        assert_eq!(policy.apply_mode, crate::ApplyMode::StageRunStart);
        assert_eq!(policy.wake_mode, crate::WakeMode::WakeIfIdle);
        assert_eq!(policy.queue_mode, crate::QueueMode::Fifo);

        let semantics =
            crate::ingress_types::RuntimeInputSemantics::try_from_generated_admission(&input, true)
                .expect("generated admission semantics");
        assert_eq!(semantics.boundary, RunApplyBoundary::RunStart);
        assert_eq!(
            semantics.execution_kind,
            meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn
        );
        assert_eq!(
            semantics.peer_response_terminal_apply_intent,
            Some(PeerResponseTerminalApplyIntent::AppendContextAndRun)
        );

        let primitive = try_inputs_to_primitive_with_boundary(
            &[(input.id().clone(), input)],
            semantics.boundary,
            &[semantics],
        )
        .expect("single input metadata cannot conflict");
        let metadata = primitive
            .turn_metadata()
            .ok_or_else(|| "terminal primitive should carry metadata".to_string())?;
        assert_eq!(
            metadata.peer_response_terminal_apply_intent,
            Some(PeerResponseTerminalApplyIntent::AppendContextAndRun)
        );
        assert!(primitive.is_peer_response_terminal_context_and_run());
        assert_eq!(
            primitive.peer_response_terminal_apply_intent_violation(),
            None
        );
        assert!(
            !primitive.is_context_only_apply_without_turn(),
            "executor context-only shortcut must not swallow terminal peer responses"
        );

        Ok(())
    }

    fn make_peer_message(peer_id: &str, body: &str) -> Input {
        Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: peer_id.into(),
                    display_identity: Some("analyst-rt".into()),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(PeerConvention::Message),
            content: body.into(),
            payload: None,
            handling_mode: None,
        })
    }

    fn make_terminal_peer_response(peer_id: &str, request_id: &str) -> Input {
        Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: peer_id.into(),
                    display_identity: Some("analyst-rt".into()),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(PeerConvention::ResponseTerminal {
                request_id: request_id.into(),
                status: ResponseTerminalStatus::Completed,
            }),
            content: String::new().into(),
            payload: Some(serde_json::json!({"ok": true})),
            handling_mode: None,
        })
    }

    async fn accept_queued_input_id(
        driver: &crate::meerkat_machine::SharedDriver,
        input: Input,
    ) -> InputId {
        let mut guard = driver.lock().await;
        match guard
            .as_driver_mut()
            .accept_input(input)
            .await
            .expect("input should be accepted")
        {
            crate::accept::AcceptOutcome::Accepted { input_id, .. } => input_id,
            other => panic!("expected accepted queued input, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn terminal_peer_response_batches_do_not_mix_with_peer_messages() {
        let terminal_first = make_shared_ephemeral_driver("terminal-first");
        let terminal_id = accept_queued_input_id(
            &terminal_first,
            make_terminal_peer_response(TEST_PEER_RESPONSE_ROUTE_ID, TEST_PEER_RESPONSE_REQUEST_ID),
        )
        .await;
        let _message_id = accept_queued_input_id(
            &terminal_first,
            make_peer_message("analyst-rt", "follow-up"),
        )
        .await;

        {
            let guard = terminal_first.lock().await;
            let batch = crate::meerkat_machine::machine_select_runtime_loop_batch(&guard);
            assert_eq!(
                batch,
                vec![terminal_id],
                "terminal response batch must not absorb later normal peer messages"
            );
            assert_eq!(
                crate::meerkat_machine::machine_batch_runtime_semantics(&guard, &batch).and_then(
                    |semantics| {
                        semantics
                            .into_iter()
                            .find_map(|semantics| semantics.peer_response_terminal_apply_intent)
                    }
                ),
                Some(PeerResponseTerminalApplyIntent::AppendContextAndRun)
            );
        }

        let message_first = make_shared_ephemeral_driver("message-first");
        let message_id =
            accept_queued_input_id(&message_first, make_peer_message("analyst-rt", "first")).await;
        let _terminal_id = accept_queued_input_id(
            &message_first,
            make_terminal_peer_response(
                TEST_PEER_RESPONSE_ROUTE_ID,
                TEST_PEER_RESPONSE_REQUEST_ID_2,
            ),
        )
        .await;

        let guard = message_first.lock().await;
        let batch = crate::meerkat_machine::machine_select_runtime_loop_batch(&guard);
        assert_eq!(
            batch,
            vec![message_id],
            "normal peer message batch must not absorb later terminal responses"
        );
        assert_eq!(
            crate::meerkat_machine::machine_batch_runtime_semantics(&guard, &batch).and_then(
                |semantics| {
                    semantics
                        .into_iter()
                        .find_map(|semantics| semantics.peer_response_terminal_apply_intent)
                }
            ),
            None
        );
    }

    #[test]
    fn peer_input_with_blocks_produces_blocks_renderable() -> Result<(), String> {
        let blocks = vec![
            meerkat_core::types::ContentBlock::Text {
                text: "see this image".into(),
            },
            meerkat_core::types::ContentBlock::Image {
                media_type: "image/png".into(),
                data: "abc123".into(),
            },
        ];
        let peer_id = "018f6f79-7a82-7c4e-a552-a3b86f963001";
        let input = Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: peer_id.into(),
                    display_identity: None,
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::Message),
            content: meerkat_core::types::ContentInput::Blocks(blocks.clone()),
            payload: None,
            handling_mode: None,
        });
        let input_id = input.id().clone();
        let primitive =
            input_to_primitive(&input, input_id).expect("single input metadata cannot conflict");

        let staged = match primitive {
            RunPrimitive::StagedInput(staged) => staged,
            other => return Err(format!("expected staged input, got {other:?}")),
        };
        assert_eq!(staged.appends.len(), 1);
        match &staged.appends[0].content {
            CoreRenderable::SystemNotice { blocks: got, .. } => {
                let Some(meerkat_core::types::SystemNoticeBlock::Comms { content, peer, .. }) =
                    got.first()
                else {
                    return Err("expected comms block".into());
                };
                assert_eq!(
                    peer.as_ref().map(|peer| peer.id),
                    Some(meerkat_core::comms::PeerId::parse(peer_id).expect("valid peer id"))
                );
                assert_eq!(content, &blocks);
            }
            other => return Err(format!("expected typed content, got {other:?}")),
        }
        Ok(())
    }

    #[test]
    fn peer_multimodal_append_keeps_source_without_duplicating_block_text() -> Result<(), String> {
        let blocks = vec![
            meerkat_core::types::ContentBlock::Text {
                text: "caption text".into(),
            },
            meerkat_core::types::ContentBlock::Image {
                media_type: "image/png".into(),
                data: "abc123".into(),
            },
        ];
        let peer_id = "018f6f79-7a82-7c4e-a552-a3b86f963002";
        let input = Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: peer_id.into(),
                    display_identity: None,
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::Message),
            content: meerkat_core::types::ContentInput::Blocks(blocks.clone()),
            payload: None,
            handling_mode: None,
        });
        let staged = match input_to_primitive(&input, input.id().clone())
            .expect("single input metadata cannot conflict")
        {
            RunPrimitive::StagedInput(staged) => staged,
            other => return Err(format!("expected staged input, got {other:?}")),
        };

        match &staged.appends[0].content {
            CoreRenderable::SystemNotice { blocks: got, .. } => {
                let Some(meerkat_core::types::SystemNoticeBlock::Comms { content, peer, .. }) =
                    got.first()
                else {
                    return Err("expected comms block".into());
                };
                assert_eq!(
                    peer.as_ref().map(|peer| peer.id),
                    Some(meerkat_core::comms::PeerId::parse(peer_id).expect("valid peer id"))
                );
                assert_eq!(content, &blocks);
            }
            other => return Err(format!("expected typed content, got {other:?}")),
        }
        Ok(())
    }

    #[test]
    fn peer_image_only_blocks_keep_source_identity_without_text_duplication() -> Result<(), String>
    {
        let blocks = vec![meerkat_core::types::ContentBlock::Image {
            media_type: "image/png".into(),
            data: "abc123".into(),
        }];
        let peer_id = "018f6f79-7a82-7c4e-a552-a3b86f963003";
        let input = Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: peer_id.into(),
                    display_identity: None,
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::Message),
            content: meerkat_core::types::ContentInput::Blocks(blocks.clone()),
            payload: None,
            handling_mode: None,
        });
        let staged = match input_to_primitive(&input, input.id().clone())
            .expect("single input metadata cannot conflict")
        {
            RunPrimitive::StagedInput(staged) => staged,
            other => return Err(format!("expected staged input, got {other:?}")),
        };

        match &staged.appends[0].content {
            CoreRenderable::SystemNotice { blocks: got, .. } => {
                let Some(meerkat_core::types::SystemNoticeBlock::Comms { content, peer, .. }) =
                    got.first()
                else {
                    return Err("expected comms block".into());
                };
                assert_eq!(
                    peer.as_ref().map(|peer| peer.id),
                    Some(meerkat_core::comms::PeerId::parse(peer_id).expect("valid peer id"))
                );
                assert_eq!(content, &blocks);
            }
            other => return Err(format!("expected typed content, got {other:?}")),
        }
        Ok(())
    }

    #[test]
    fn peer_image_only_blocks_are_the_notice_content() -> Result<(), String> {
        let blocks = vec![meerkat_core::types::ContentBlock::Image {
            media_type: "image/png".into(),
            data: "abc123".into(),
        }];
        let peer_id = "018f6f79-7a82-7c4e-a552-a3b86f963004";
        let input = Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: peer_id.into(),
                    display_identity: None,
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(crate::input::PeerConvention::Message),
            content: meerkat_core::types::ContentInput::Blocks(blocks.clone()),
            payload: None,
            handling_mode: None,
        });
        let staged = match input_to_primitive(&input, input.id().clone())
            .expect("single input metadata cannot conflict")
        {
            RunPrimitive::StagedInput(staged) => staged,
            other => return Err(format!("expected staged input, got {other:?}")),
        };

        match &staged.appends[0].content {
            CoreRenderable::SystemNotice { blocks: got, .. } => {
                let Some(meerkat_core::types::SystemNoticeBlock::Comms { content, peer, .. }) =
                    got.first()
                else {
                    return Err("expected comms block".into());
                };
                assert_eq!(
                    peer.as_ref().map(|peer| peer.id),
                    Some(meerkat_core::comms::PeerId::parse(peer_id).expect("valid peer id"))
                );
                // Single-owner semantics: the typed blocks ARE the content;
                // no synthesized caption text is merged in beside them.
                assert_eq!(content, &blocks);
            }
            other => return Err(format!("expected blocks content, got {other:?}")),
        }
        Ok(())
    }

    #[test]
    fn flow_step_with_blocks_produces_blocks_renderable() -> Result<(), String> {
        let blocks = vec![
            meerkat_core::types::ContentBlock::Text {
                text: "analyze this screenshot".into(),
            },
            meerkat_core::types::ContentBlock::Image {
                media_type: "image/png".into(),
                data: "abc123".into(),
            },
        ];
        let input = Input::FlowStep(FlowStepInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Flow {
                    flow_id: "flow-1".into(),
                    step_index: 0,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            step_id: "step-1".into(),
            content: meerkat_core::types::ContentInput::Blocks(blocks),
            turn_metadata: None,
        });
        let input_id = input.id().clone();
        let primitive =
            input_to_primitive(&input, input_id).expect("single input metadata cannot conflict");

        let staged = match primitive {
            RunPrimitive::StagedInput(staged) => staged,
            other => return Err(format!("expected staged input, got {other:?}")),
        };
        assert_eq!(staged.appends.len(), 1);
        match &staged.appends[0].content {
            CoreRenderable::SystemNotice { blocks: got, .. } => {
                assert!(matches!(
                    got.first(),
                    Some(meerkat_core::types::SystemNoticeBlock::RuntimeNotice { category, detail, .. })
                        if category == "flow_step"
                            && detail.as_deref()
                                == Some("analyze this screenshot\n[image: image/png]")
                ));
            }
            other => return Err(format!("expected typed content, got {other:?}")),
        }
        Ok(())
    }

    #[test]
    fn external_event_with_blocks_produces_blocks_renderable() -> Result<(), String> {
        let blocks = vec![
            meerkat_core::types::ContentBlock::Text {
                text: "see this event".into(),
            },
            meerkat_core::types::ContentBlock::Image {
                media_type: "image/png".into(),
                data: "abc123".into(),
            },
        ];
        let input = Input::ExternalEvent(crate::input::ExternalEventInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::External {
                    source_name: "webhook".into(),
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            event_type: "webhook".into(),
            payload: serde_json::json!({"body": "see this event"}),
            blocks: Some(blocks.clone()),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            render_metadata: None,
        });
        let input_id = input.id().clone();
        let primitive =
            input_to_primitive(&input, input_id).expect("single input metadata cannot conflict");

        let staged = match primitive {
            RunPrimitive::StagedInput(staged) => staged,
            other => return Err(format!("expected staged input, got {other:?}")),
        };
        assert_eq!(staged.appends.len(), 1);
        match &staged.appends[0].content {
            CoreRenderable::SystemNotice { blocks: got, .. } => {
                let Some(meerkat_core::types::SystemNoticeBlock::ExternalEvent { content, .. }) =
                    got.first()
                else {
                    return Err("expected external event block".into());
                };
                assert_eq!(content, &blocks);
            }
            other => return Err(format!("expected typed content, got {other:?}")),
        }
        Ok(())
    }

    #[test]
    fn external_event_with_image_only_blocks_keeps_event_identity() -> Result<(), String> {
        let blocks = vec![meerkat_core::types::ContentBlock::Image {
            media_type: "image/png".into(),
            data: "abc123".into(),
        }];
        let input = Input::ExternalEvent(crate::input::ExternalEventInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::External {
                    source_name: "webhook".into(),
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            event_type: "webhook".into(),
            payload: serde_json::json!({"body": "see attached screenshot"}),
            blocks: Some(blocks.clone()),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            render_metadata: None,
        });
        let primitive = input_to_primitive(&input, input.id().clone())
            .expect("single input metadata cannot conflict");

        let staged = match primitive {
            RunPrimitive::StagedInput(staged) => staged,
            other => return Err(format!("expected staged input, got {other:?}")),
        };
        match &staged.appends[0].content {
            CoreRenderable::SystemNotice { blocks: got, .. } => {
                let Some(meerkat_core::types::SystemNoticeBlock::ExternalEvent { content, .. }) =
                    got.first()
                else {
                    return Err("expected external event block".into());
                };
                assert_eq!(content, &blocks);
            }
            other => return Err(format!("expected typed content, got {other:?}")),
        }
        Ok(())
    }

    #[test]
    fn external_event_prefers_source_name_over_event_type_without_body() -> Result<(), String> {
        let input = Input::ExternalEvent(crate::input::ExternalEventInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::External {
                    source_name: "webhook".into(),
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            event_type: "invoice.created".into(),
            payload: serde_json::json!({"invoice_id": "inv_123"}),
            blocks: None,
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            render_metadata: None,
        });

        assert_eq!(input_to_prompt(&input), "External event via webhook");
        Ok(())
    }

    #[test]
    fn plain_event_and_direct_runtime_external_event_share_projection() -> Result<(), String> {
        use crate::comms_bridge::classified_interaction_to_runtime_input;
        use crate::identifiers::LogicalRuntimeId;
        use meerkat_core::interaction::{
            InboxInteraction, InteractionContent, PeerInputCandidate, PeerInputClass,
        };

        let interaction_id = meerkat_core::interaction::InteractionId(uuid::Uuid::new_v4());
        let from_comms = classified_interaction_to_runtime_input(
            &PeerInputCandidate {
                lifecycle_peer: None,
                response_terminality: None,
                ingress: meerkat_core::PeerIngressFact::plain_event(
                    interaction_id,
                    "webhook",
                    PeerInputClass::PlainEvent,
                    meerkat_core::PeerIngressKind::PlainEvent,
                ),
                interaction: InboxInteraction {
                    id: interaction_id,
                    from_route: None,
                    from: "event:webhook".into(),
                    content: InteractionContent::Message {
                        body: "build failed".into(),
                        blocks: None,
                    },
                    rendered_text: "External event via webhook: build failed".into(),
                    handling_mode: meerkat_core::types::HandlingMode::Queue,
                    render_metadata: None,
                },
            },
            &LogicalRuntimeId::new("test"),
        )
        .map_err(|err| err.to_string())?;

        let direct = Input::ExternalEvent(crate::input::ExternalEventInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::External {
                    source_name: "webhook".into(),
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            event_type: "webhook".into(),
            payload: serde_json::json!({"body": "build failed"}),
            blocks: None,
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            render_metadata: None,
        });

        assert_eq!(input_to_prompt(&from_comms), input_to_prompt(&direct));
        assert_eq!(
            input_to_prompt(&direct),
            "External event via webhook: build failed"
        );
        Ok(())
    }

    #[test]
    fn external_event_with_steer_preserves_runtime_hints() -> Result<(), String> {
        let render_metadata = meerkat_core::types::RenderMetadata {
            class: meerkat_core::types::RenderClass::ExternalEvent,
            salience: meerkat_core::types::RenderSalience::Urgent,
        };
        let input = Input::ExternalEvent(crate::input::ExternalEventInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::External {
                    source_name: "webhook".into(),
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            event_type: "webhook".into(),
            payload: serde_json::json!({"body": "urgent"}),
            blocks: None,
            handling_mode: meerkat_core::types::HandlingMode::Steer,
            render_metadata: Some(render_metadata.clone()),
        });

        let staged = match input_to_primitive(&input, input.id().clone())
            .expect("single input metadata cannot conflict")
        {
            RunPrimitive::StagedInput(staged) => staged,
            other => return Err(format!("expected staged input, got {other:?}")),
        };

        assert_eq!(staged.boundary, RunApplyBoundary::RunCheckpoint);
        assert_eq!(
            staged
                .turn_metadata
                .as_ref()
                .and_then(|meta| meta.handling_mode),
            Some(meerkat_core::types::HandlingMode::Steer)
        );
        assert_eq!(
            staged
                .turn_metadata
                .as_ref()
                .and_then(|meta| meta.render_metadata.clone()),
            Some(render_metadata)
        );
        Ok(())
    }

    #[tokio::test]
    async fn resolve_completion_waiters_surfaces_callback_pending() {
        let mut registry = crate::completion::CompletionRegistry::new();
        let input_id = InputId::new();
        let handle = registry.register(input_id.clone());
        let terminal = Some(CoreApplyTerminal::CallbackPending {
            tool_name: "external_mock".to_string(),
            args: serde_json::json!({ "value": "browser" }),
        });

        resolve_completion_waiters_from_authority(
            &mut registry,
            std::slice::from_ref(&input_id),
            terminal.as_ref(),
            crate::meerkat_machine::driver::test_runtime_completion_authority(
                crate::meerkat_machine::dsl::RuntimeCompletionResultClass::CallbackPending,
                crate::meerkat_machine::dsl::RuntimeCompletionObservedOutcome::CallbackPending,
            ),
            None,
        );

        match handle.wait_authorized().await {
            crate::completion::CompletionOutcome::CallbackPending { tool_name, args } => {
                assert_eq!(tool_name, "external_mock");
                assert_eq!(args, serde_json::json!({ "value": "browser" }));
            }
            other => panic!("Expected CallbackPending, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn resolve_completion_waiters_surfaces_terminal_run_result() {
        let mut registry = crate::completion::CompletionRegistry::new();
        let input_id = InputId::new();
        let handle = registry.register(input_id.clone());
        let run_result = meerkat_core::types::RunResult {
            text: "terminal authority".to_string(),
            session_id: SessionId::new(),
            usage: meerkat_core::types::Usage::default(),
            turns: 1,
            tool_calls: 0,
            terminal_cause_kind: None,
            structured_output: None,
            extraction_error: None,
            schema_warnings: None,
            skill_diagnostics: None,
        };
        let terminal = Some(CoreApplyTerminal::RunResult(Box::new(run_result)));

        resolve_completion_waiters_from_authority(
            &mut registry,
            std::slice::from_ref(&input_id),
            terminal.as_ref(),
            crate::meerkat_machine::driver::test_runtime_completion_authority(
                crate::meerkat_machine::dsl::RuntimeCompletionResultClass::Completed,
                crate::meerkat_machine::dsl::RuntimeCompletionObservedOutcome::Completed,
            ),
            None,
        );

        match handle.wait_authorized().await {
            crate::completion::CompletionOutcome::Completed(result) => {
                assert_eq!(result.text, "terminal authority");
            }
            other => panic!("Expected Completed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn fail_completion_waiters_surfaces_wait_error() {
        let mut registry = crate::completion::CompletionRegistry::new();
        let input_id = InputId::new();
        let handle = registry.register(input_id.clone());

        fail_completion_waiters(
            &mut registry,
            std::slice::from_ref(&input_id),
            "runtime loop failed before executor apply",
        );

        match handle.try_wait().await {
            Err(crate::completion::CompletionWaitError::AuthorityUnavailable(reason)) => {
                assert_eq!(reason, "runtime loop failed before executor apply");
            }
            other => panic!("Expected completion wait error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn maybe_inject_feed_wake_feed_path_injects_inline_continuation_when_quiescent() {
        let driver = make_shared_ephemeral_driver("feed-inline");
        let registry = crate::ops_lifecycle::RuntimeOpsLifecycleRegistry::new();

        let spec = background_spec("feed-inline");
        let op_id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&op_id).unwrap();
        registry
            .complete_operation(&op_id, op_result(&op_id, "done"))
            .unwrap();

        let feed = registry.completion_feed_handle();
        let mut observed_seq = 0;
        let mut last_injected_seq = 0;

        let injected = maybe_inject_feed_wake(
            &driver,
            Some(feed.as_ref()),
            &mut observed_seq,
            &mut last_injected_seq,
            None,
            Some(&registry),
            &RuntimeLoopAuthorityBinding::detached_for_test(),
        )
        .await;

        assert_eq!(
            injected,
            FeedWakeOutcome::Injected,
            "feed-backed path should inject inline when quiescent"
        );
        assert_eq!(observed_seq, feed.watermark());
        assert_eq!(last_injected_seq, feed.watermark());

        let mut guard = driver.lock().await;
        assert_eq!(guard.as_driver().active_input_ids().len(), 1);
        let crate::meerkat_machine::DriverEntry::Ephemeral(ephemeral) = &mut *guard else {
            panic!("test uses an ephemeral driver");
        };
        let (_input_id, input) = ephemeral
            .dequeue_next()
            .expect("continuation should be queued inline");
        match input {
            Input::Continuation(continuation) => {
                assert_eq!(continuation.reason, "detached_background_op_completed");
                assert_eq!(
                    continuation.handling_mode,
                    meerkat_core::types::HandlingMode::Steer
                );
            }
            other => panic!("expected inline continuation injection, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn maybe_inject_feed_wake_without_feed_is_noop() {
        let driver = make_shared_ephemeral_driver("no-feed");
        let mut observed_seq = 0;
        let mut last_injected_seq = 0;

        let injected = maybe_inject_feed_wake(
            &driver,
            None,
            &mut observed_seq,
            &mut last_injected_seq,
            None,
            None,
            &RuntimeLoopAuthorityBinding::detached_for_test(),
        )
        .await;

        assert_eq!(
            injected,
            FeedWakeOutcome::Noop,
            "no feed means no injection"
        );
        assert_eq!(observed_seq, 0);
        assert_eq!(last_injected_seq, 0);

        let guard = driver.lock().await;
        assert!(
            guard.as_driver().active_input_ids().is_empty(),
            "no feed must not enqueue anything"
        );
    }

    #[tokio::test]
    async fn maybe_inject_feed_wake_with_feed_without_ops_authority_fails_closed() {
        let driver = make_shared_ephemeral_driver("feed-without-ops-authority");
        let registry = crate::ops_lifecycle::RuntimeOpsLifecycleRegistry::new();

        let spec = background_spec("feed-without-ops-authority");
        let op_id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&op_id).unwrap();
        registry
            .complete_operation(&op_id, op_result(&op_id, "done"))
            .unwrap();

        let feed = registry.completion_feed_handle();
        let mut observed_seq = 0;
        let mut last_injected_seq = 0;

        let injected = maybe_inject_feed_wake(
            &driver,
            Some(feed.as_ref()),
            &mut observed_seq,
            &mut last_injected_seq,
            None,
            None,
            &RuntimeLoopAuthorityBinding::detached_for_test(),
        )
        .await;

        assert_eq!(
            injected,
            FeedWakeOutcome::StaleAuthority,
            "feed-backed wake must fail closed without generated cursor authority"
        );
        assert_eq!(observed_seq, 0);
        assert_eq!(last_injected_seq, 0);

        let guard = driver.lock().await;
        assert!(
            guard.as_driver().active_input_ids().is_empty(),
            "missing cursor authority must not enqueue a detached continuation"
        );
    }

    #[tokio::test]
    async fn maybe_inject_feed_wake_empty_feed_advances_observed_seq() {
        let driver = make_shared_ephemeral_driver("advance-only");
        let registry = crate::ops_lifecycle::RuntimeOpsLifecycleRegistry::new();
        let feed = registry.completion_feed_handle();
        let mut observed_seq = 0;
        let mut last_injected_seq = 0;

        let injected = maybe_inject_feed_wake(
            &driver,
            Some(feed.as_ref()),
            &mut observed_seq,
            &mut last_injected_seq,
            None,
            Some(&registry),
            &RuntimeLoopAuthorityBinding::detached_for_test(),
        )
        .await;

        assert_eq!(
            injected,
            FeedWakeOutcome::Noop,
            "empty feed should not inject"
        );
        assert_eq!(observed_seq, feed.watermark());
        assert_eq!(last_injected_seq, 0);

        let guard = driver.lock().await;
        assert!(guard.as_driver().active_input_ids().is_empty());
    }

    #[tokio::test]
    async fn maybe_inject_feed_wake_generated_ignore_completion_advances_observed_seq() {
        let driver = make_shared_ephemeral_driver("advance-generated-ignore");
        let registry = crate::ops_lifecycle::RuntimeOpsLifecycleRegistry::new();
        let spec = mob_child_spec("advance-generated-ignore");
        let op_id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&op_id).unwrap();
        registry
            .complete_operation(&op_id, op_result(&op_id, "done"))
            .unwrap();
        let feed = registry.completion_feed_handle();
        let mut observed_seq = 0;
        let mut last_injected_seq = 0;

        let injected = maybe_inject_feed_wake(
            &driver,
            Some(feed.as_ref()),
            &mut observed_seq,
            &mut last_injected_seq,
            None,
            Some(&registry),
            &RuntimeLoopAuthorityBinding::detached_for_test(),
        )
        .await;

        assert_eq!(
            injected,
            FeedWakeOutcome::Noop,
            "generated observe-only completion should not inject"
        );
        assert_eq!(observed_seq, feed.watermark());
        assert_eq!(last_injected_seq, 0);

        let guard = driver.lock().await;
        assert!(guard.as_driver().active_input_ids().is_empty());
    }

    #[tokio::test]
    async fn maybe_inject_feed_wake_stale_observe_only_completion_does_not_advance_cursor() {
        let driver = make_shared_ephemeral_driver("advance-only-stale");
        let registry = crate::ops_lifecycle::RuntimeOpsLifecycleRegistry::new();
        let spec = mob_child_spec("advance-only-stale");
        let op_id = spec.id.clone();
        registry.register_operation(spec).unwrap();
        registry.provisioning_succeeded(&op_id).unwrap();
        registry
            .complete_operation(&op_id, op_result(&op_id, "done"))
            .unwrap();
        let feed = registry.completion_feed_handle();
        let mut observed_seq = 0;
        let mut last_injected_seq = 0;
        let stale_binding = RuntimeLoopAuthorityBinding::new(
            std::sync::Weak::<crate::meerkat_machine::MeerkatMachine>::new(),
            SessionId::new(),
        );

        let injected = maybe_inject_feed_wake(
            &driver,
            Some(feed.as_ref()),
            &mut observed_seq,
            &mut last_injected_seq,
            None,
            Some(&registry),
            &stale_binding,
        )
        .await;

        assert_eq!(injected, FeedWakeOutcome::StaleAuthority);
        assert_eq!(
            observed_seq, 0,
            "stale runtime loop must not advance observed cursor truth"
        );
        assert_eq!(last_injected_seq, 0);
    }

    /// Regression for #182: a detached wake whose continuation injection fails
    /// must leave the observed cursor UNCHANGED so the completion is
    /// re-presented on the next wake. A successful injection advances both
    /// cursors. The old code advanced the observed cursor unconditionally,
    /// laundering a failed injection into a watermark advance that hid the
    /// completion from retry.
    #[test]
    fn detached_wake_cursor_plan_leaves_completion_visible_on_injection_failure() {
        // Failed injection: leave both cursors untouched (completion stays
        // visible for retry).
        let failed = DetachedWakeCursorPlan::from_injection(false);
        assert_eq!(failed, DetachedWakeCursorPlan::LeaveVisibleForRetry);
        assert!(
            !failed.advances_observed_cursor(),
            "a failed injection must NOT advance the observed cursor"
        );

        // Successful injection: advance both injected and observed cursors.
        let succeeded = DetachedWakeCursorPlan::from_injection(true);
        assert_eq!(
            succeeded,
            DetachedWakeCursorPlan::AdvanceInjectedAndObserved
        );
        assert!(
            succeeded.advances_observed_cursor(),
            "a successful injection must advance the observed cursor"
        );
    }

    // --- execution_kind stamping tests ---

    #[test]
    fn primitive_from_prompt_has_content_turn() {
        let input = make_prompt("hello");
        let id = input.id().clone();
        let primitive =
            input_to_primitive(&input, id).expect("single input metadata cannot conflict");
        let meta = primitive
            .turn_metadata()
            .expect("should have turn_metadata");
        assert_eq!(
            meta.execution_kind,
            Some(meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn)
        );
    }

    #[test]
    fn primitive_from_idle_peer_steer_normalizes_execution_handling_mode() {
        let mut input = make_peer_message("peer-steer", "urgent helper update");
        let Input::Peer(peer) = &mut input else {
            panic!("make_peer_message must build a peer input");
        };
        peer.handling_mode = Some(meerkat_core::types::HandlingMode::Steer);

        let primitive = input_to_primitive(&input, input.id().clone())
            .expect("single peer input metadata cannot conflict");
        let meta = primitive
            .turn_metadata()
            .expect("peer primitive should carry turn metadata");
        assert_eq!(
            meta.handling_mode,
            Some(meerkat_core::types::HandlingMode::Queue),
            "idle steer is already admitted into the steer lane; fresh turn execution must be queue-compatible"
        );
    }

    #[test]
    fn primitive_from_continuation_has_resume_pending() {
        let input = Input::Continuation(ContinuationInput::detached_background_op_completed());
        let id = input.id().clone();
        let primitive =
            input_to_primitive(&input, id).expect("single input metadata cannot conflict");
        let meta = primitive
            .turn_metadata()
            .expect("should have turn_metadata");
        assert_eq!(
            meta.execution_kind,
            Some(meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending)
        );
    }

    #[test]
    fn admitted_input_primitive_uses_runtime_stamped_execution_kind() {
        let input = make_prompt("test prompt");
        let id = input.id().clone();
        let primitive = admitted_input_to_primitive(
            &input,
            id,
            crate::input::runtime_input_projection(&input),
            crate::ingress_types::RuntimeInputSemantics {
                boundary: RunApplyBoundary::RunStart,
                execution_kind: meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending,
                execution_handling_mode: None,
                peer_response_terminal_apply_intent: None,
                live_interrupt_required: false,
            },
        )
        .expect("single input metadata cannot conflict");
        let meta = primitive
            .turn_metadata()
            .expect("should have turn_metadata");
        assert_eq!(
            meta.execution_kind,
            Some(meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending),
            "primitive construction must use the runtime-stamped execution kind, not the local input kind"
        );
    }

    #[test]
    fn primitive_from_peer_terminal_has_content_turn() {
        let input = Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: "p".into(),
                    display_identity: None,
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(PeerConvention::ResponseTerminal {
                request_id: "018f6f79-7a82-7c4e-a552-a3b86f9630f1".into(),
                status: ResponseTerminalStatus::Completed,
            }),
            content: "done".into(),
            payload: None,
            handling_mode: None,
        });
        let id = input.id().clone();
        let primitive =
            input_to_primitive(&input, id).expect("single input metadata cannot conflict");
        let meta = primitive
            .turn_metadata()
            .expect("should have turn_metadata");
        assert_eq!(
            meta.execution_kind,
            Some(meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn)
        );
    }

    #[test]
    fn mixed_batch_execution_kind_conflict_is_rejected() {
        // Admission batches are already grouped by execution kind. A direct
        // helper batch that mixes kinds must fail instead of inventing a local
        // ContentTurn default.
        let peer = Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: "p".into(),
                    display_identity: None,
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(PeerConvention::Message),
            content: "msg".into(),
            payload: None,
            handling_mode: None,
        });
        let continuation =
            Input::Continuation(ContinuationInput::detached_background_op_completed());
        let inputs = vec![
            (peer.id().clone(), peer),
            (continuation.id().clone(), continuation),
        ];
        let err = inputs_to_primitive_with_boundary(&inputs, RunApplyBoundary::RunCheckpoint)
            .expect_err("mixed execution kinds should be rejected");

        assert_eq!(err.field, "execution_kind");
    }

    #[test]
    fn batch_metadata_conflict_surfaces_typed_error() {
        let mut first = make_prompt("first");
        if let Input::Prompt(prompt) = &mut first {
            prompt.turn_metadata = Some(
                meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                    model: Some(meerkat_core::lifecycle::run_primitive::ModelId::new(
                        "model-a",
                    )),
                    ..Default::default()
                },
            );
        }
        let mut second = make_prompt("second");
        if let Input::Prompt(prompt) = &mut second {
            prompt.turn_metadata = Some(
                meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                    model: Some(meerkat_core::lifecycle::run_primitive::ModelId::new(
                        "model-b",
                    )),
                    ..Default::default()
                },
            );
        }
        let inputs = vec![(first.id().clone(), first), (second.id().clone(), second)];
        let semantics = fallback_batch_semantics(&inputs);

        let err =
            try_inputs_to_primitive_with_boundary(&inputs, RunApplyBoundary::RunStart, &semantics)
                .expect_err("conflicting batch metadata should be rejected");

        assert_eq!(err.field, "model");
    }
}

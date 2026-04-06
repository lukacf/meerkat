// @generated — composition driver for `flow_frame_loop`
// Route target selectors:
// - `loop_completed_completes_parent_loop_node` selects `frame_id` from effect/state field `parent_frame_id`
// - `loop_exhausted_fails_parent_loop_node` selects `frame_id` from effect/state field `parent_frame_id`
// - `loop_failed_fails_parent_loop_node` selects `frame_id` from effect/state field `parent_frame_id`
// - `loop_canceled_cancels_parent_loop_node` selects `frame_id` from effect/state field `parent_frame_id`
// Transaction plans:
// - `grant_node_slot_step` via `acknowledge_node_grant` (`cas_grant_node_slot`): Grant a node slot, admit a step node, and persist the updated run/frame state before spawning step work.
// - `grant_node_slot_loop_start` via `acknowledge_node_grant` (`cas_start_loop`): Grant a node slot, admit a loop node, start the loop instance, and route pending-body-frame registration through run state.
// - `grant_body_frame_start` via `acknowledge_body_frame_start` (`cas_grant_body_frame_start`): Acknowledge a body-frame grant, transition the loop to BodyFrameActive, create the initial body frame, and register ready work.
// - `run_state_only` via `revisit_frame` (`cas_flow_state`): Apply a run-state-only routed effect such as ready-frame registration, node-slot release, or pending-body-frame registration.
// - `seal_frame` via `revisit_frame` (`cas_frame_state`): Seal a frame whose tracked nodes are all terminal so the frame machine emits its typed root/body terminal effect.
// - `complete_body_frame` via `advance_body_frame_after_seal` (`cas_complete_body_frame`): Persist body-frame terminalization into loop state and release the active body-frame slot before until evaluation feedback.
// - `loop_request_body_frame` via `resolve_until_feedback` (`cas_loop_request_body_frame`): Persist an UntilConditionFailed replay that re-requests the next body frame through the run scheduler.
// - `complete_loop` via `resolve_until_feedback` (`cas_complete_loop`): Persist a terminal loop outcome and project its routed parent-frame node transition in the same CAS bundle.

use crate::definition::{FlowNodeSpec, FrameSpec, RepeatUntilSpec};
use crate::error::MobError;
use crate::generated::protocol_flow_loop_until_evaluation::{
    FlowLoopUntilEvaluationObligation, accept_evaluate_until_condition,
    submit_until_condition_failed, submit_until_condition_met,
};
use crate::ids::{FlowNodeId, FrameId, LoopId, LoopInstanceId, StepId};
use crate::run::{FrameSnapshot, LoopIterationLedgerEntry, LoopSnapshot};
use crate::runtime::flow_frame_kernel::{build_start_body_frame_input, topological_order};
use crate::runtime::loop_iteration_authority::{
    LoopIterationAuthority, LoopUntilEvaluationRequested,
};
use meerkat_machine_kernels::generated::{flow_frame, flow_run, loop_iteration};
use meerkat_machine_kernels::{
    KernelEffect, KernelInput, KernelState, KernelValue, TransitionOutcome,
};
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct PreviewedNodeGrant {
    pub frame_id: FrameId,
    pub next_run_state: KernelState,
}

#[derive(Debug, Clone)]
pub struct PreviewedBodyFrameGrant {
    pub loop_instance_id: LoopInstanceId,
    pub next_run_state: KernelState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowFrameTerminalPhase {
    Completed,
    Failed,
    Canceled,
}

#[derive(Debug, Clone)]
pub enum FlowFrameLoopStorePlan {
    GrantNodeSlot {
        expected_run_state: KernelState,
        next_run_state: KernelState,
        frame_id: FrameId,
        expected_frame: FrameSnapshot,
        next_frame: FrameSnapshot,
    },
    StartLoop {
        loop_instance_id: LoopInstanceId,
        expected_run_state: KernelState,
        next_run_state: KernelState,
        frame_id: FrameId,
        expected_frame: FrameSnapshot,
        next_frame: FrameSnapshot,
        initial_loop: LoopSnapshot,
    },
    GrantBodyFrameStart {
        loop_instance_id: LoopInstanceId,
        expected_loop: LoopSnapshot,
        next_loop: LoopSnapshot,
        frame_id: FrameId,
        initial_frame: FrameSnapshot,
        ledger_entry: LoopIterationLedgerEntry,
        expected_run_state: KernelState,
        next_run_state: KernelState,
    },
    RunStateOnly {
        expected_run_state: KernelState,
        next_run_state: KernelState,
    },
    SealFrame {
        frame_id: FrameId,
        expected_frame: FrameSnapshot,
        next_frame: FrameSnapshot,
    },
    CompleteBodyFrame {
        loop_instance_id: LoopInstanceId,
        expected_loop: LoopSnapshot,
        next_loop: LoopSnapshot,
        frame_id: FrameId,
        expected_frame: FrameSnapshot,
        next_frame: FrameSnapshot,
        expected_run_state: KernelState,
        next_run_state: KernelState,
    },
    LoopRequestBodyFrame {
        loop_instance_id: LoopInstanceId,
        expected_loop: LoopSnapshot,
        next_loop: LoopSnapshot,
        expected_run_state: KernelState,
        next_run_state: KernelState,
    },
    CompleteLoop {
        loop_instance_id: LoopInstanceId,
        expected_loop: LoopSnapshot,
        next_loop: LoopSnapshot,
        frame_id: FrameId,
        expected_frame: FrameSnapshot,
        next_frame: FrameSnapshot,
        expected_run_state: KernelState,
        next_run_state: KernelState,
    },
}

#[derive(Debug, Clone)]
pub enum FlowFrameLoopWork {
    SpawnStep {
        frame_id: FrameId,
        node_id: FlowNodeId,
        step_id: StepId,
    },
    EvaluateUntil {
        obligation: FlowLoopUntilEvaluationObligation,
    },
    RevisitFrame {
        frame_id: FrameId,
    },
}

#[derive(Debug, Clone, Default)]
pub struct FlowFrameLoopDecision {
    pub store_plan: Option<FlowFrameLoopStorePlan>,
    pub follow_up: Vec<FlowFrameLoopWork>,
}

impl FlowFrameLoopDecision {
    fn plan(plan: FlowFrameLoopStorePlan) -> Self {
        Self {
            store_plan: Some(plan),
            follow_up: Vec::new(),
        }
    }

    fn with_work(mut self, work: FlowFrameLoopWork) -> Self {
        self.follow_up.push(work);
        self
    }
}

pub struct FlowFrameLoopDriver;

impl FlowFrameLoopDriver {
    pub fn preview_node_grant(state: &KernelState) -> Result<Option<PreviewedNodeGrant>, MobError> {
        let outcome = run_transition_outcome(state, "PumpNodeScheduler", BTreeMap::new());
        match outcome {
            Ok(outcome) => Ok(effect_frame_id(&outcome.effects, "GrantNodeSlot")?.map(
                |frame_id| PreviewedNodeGrant {
                    frame_id,
                    next_run_state: outcome.next_state,
                },
            )),
            Err(_) => Ok(None),
        }
    }

    pub fn preview_body_frame_grant(
        state: &KernelState,
    ) -> Result<Option<PreviewedBodyFrameGrant>, MobError> {
        let outcome = run_transition_outcome(state, "PumpFrameScheduler", BTreeMap::new());
        match outcome {
            Ok(outcome) => Ok(
                effect_loop_id(&outcome.effects, "GrantBodyFrameStart")?.map(|loop_instance_id| {
                    PreviewedBodyFrameGrant {
                        loop_instance_id,
                        next_run_state: outcome.next_state,
                    }
                }),
            ),
            Err(_) => Ok(None),
        }
    }

    pub fn acknowledge_node_grant(
        expected_run_state: &KernelState,
        grant: &PreviewedNodeGrant,
        current_frame: &FrameSnapshot,
        frame_spec: &FrameSpec,
        current_depth: u32,
        max_frame_depth: u32,
    ) -> Result<FlowFrameLoopDecision, MobError> {
        let frame_id = &grant.frame_id;
        let admit_outcome = frame_transition_outcome(
            &current_frame.kernel_state,
            "AdmitNextReadyNode",
            BTreeMap::new(),
        )?;
        let next_frame = FrameSnapshot {
            kernel_state: admit_outcome.next_state.clone(),
        };
        let mut next_run_state = grant.next_run_state.clone();

        if has_effect_variant(&admit_outcome.effects, "NodeExecutionReleased") {
            next_run_state = run_transition_state(
                &next_run_state,
                "NodeExecutionReleased",
                frame_id_fields(frame_id),
            )?;
        }

        if frame_ready(&next_frame.kernel_state)
            && !state_set_contains(&next_run_state, "ready_frame_membership", frame_id)?
        {
            next_run_state = run_transition_state(
                &next_run_state,
                "RegisterReadyFrame",
                frame_id_fields(frame_id),
            )?;
        }

        if let Some(node_id) = effect_node_id(&admit_outcome.effects, "StartLoopNode")? {
            let loop_spec = match frame_spec.nodes.get(&node_id) {
                Some(FlowNodeSpec::RepeatUntil(spec)) => spec.clone(),
                _ => {
                    return Err(MobError::Internal(format!(
                        "frame '{frame_id}' admitted non-loop node '{node_id}' as loop work"
                    )));
                }
            };
            let body_depth = current_depth + 1;
            if max_frame_depth > 0 && body_depth > max_frame_depth {
                return Err(MobError::NotYetImplemented(format!(
                    "loop '{}' would exceed max_frame_depth={} (current depth={}); nested loops require a higher limit in LimitsSpec.max_frame_depth",
                    loop_spec.loop_id,
                    max_frame_depth,
                    body_depth - 1
                )));
            }
            let loop_instance_id = LoopInstanceId::from(format!("{frame_id}::{node_id}").as_str());
            let initial_loop = initial_loop_snapshot(
                &loop_instance_id,
                &loop_spec,
                frame_id,
                &node_id,
                body_depth,
            )?;
            if state_is_awaiting_body_frame(&initial_loop.kernel_state)
                && !state_set_contains(
                    &next_run_state,
                    "pending_body_frame_loop_membership",
                    &loop_instance_id,
                )?
            {
                next_run_state = run_transition_state(
                    &next_run_state,
                    "RegisterPendingBodyFrame",
                    loop_register_pending_fields(&loop_instance_id, u64::from(body_depth)),
                )?;
            }
            return Ok(FlowFrameLoopDecision::plan(
                FlowFrameLoopStorePlan::StartLoop {
                    loop_instance_id,
                    expected_run_state: expected_run_state.clone(),
                    next_run_state,
                    frame_id: frame_id.clone(),
                    expected_frame: current_frame.clone(),
                    next_frame,
                    initial_loop,
                },
            ));
        }

        let mut decision = FlowFrameLoopDecision::plan(FlowFrameLoopStorePlan::GrantNodeSlot {
            expected_run_state: expected_run_state.clone(),
            next_run_state,
            frame_id: frame_id.clone(),
            expected_frame: current_frame.clone(),
            next_frame,
        });
        if let Some(node_id) = effect_node_id(&admit_outcome.effects, "AdmitStepWork")? {
            let step_id = match frame_spec.nodes.get(&node_id) {
                Some(FlowNodeSpec::Step(step)) => step.step_id.clone(),
                _ => {
                    return Err(MobError::Internal(format!(
                        "frame '{frame_id}' admitted non-step node '{node_id}' as step work"
                    )));
                }
            };
            decision.follow_up.push(FlowFrameLoopWork::SpawnStep {
                frame_id: frame_id.clone(),
                node_id,
                step_id,
            });
        } else {
            decision.follow_up.push(FlowFrameLoopWork::RevisitFrame {
                frame_id: frame_id.clone(),
            });
        }
        Ok(decision)
    }

    pub fn acknowledge_body_frame_start(
        expected_run_state: &KernelState,
        grant: &PreviewedBodyFrameGrant,
        loop_snapshot: &LoopSnapshot,
        loop_spec: &RepeatUntilSpec,
    ) -> Result<FlowFrameLoopDecision, MobError> {
        let iteration = loop_current_iteration(&loop_snapshot.kernel_state)?;
        let frame_id =
            FrameId::from(format!("{}::iter-{iteration}", grant.loop_instance_id).as_str());
        let initial_frame = initial_body_frame_snapshot(
            &frame_id,
            &grant.loop_instance_id,
            iteration,
            &loop_spec.body,
        )?;
        let body_frame_outcome = loop_transition_outcome(
            &loop_snapshot.kernel_state,
            "BodyFrameStarted",
            BTreeMap::from([
                (
                    "loop_instance_id".into(),
                    KernelValue::String(grant.loop_instance_id.to_string()),
                ),
                ("frame_id".into(), KernelValue::String(frame_id.to_string())),
                ("iteration".into(), KernelValue::U64(iteration)),
            ]),
        )?;
        let next_loop = LoopSnapshot {
            kernel_state: body_frame_outcome.next_state,
        };
        let ledger_entry = LoopIterationLedgerEntry {
            loop_instance_id: grant.loop_instance_id.clone(),
            iteration,
            frame_id: frame_id.clone(),
        };
        let mut next_run_state = grant.next_run_state.clone();
        if frame_ready(&initial_frame.kernel_state)
            && !state_set_contains(&next_run_state, "ready_frame_membership", &frame_id)?
        {
            next_run_state = run_transition_state(
                &next_run_state,
                "RegisterReadyFrame",
                frame_id_fields(&frame_id),
            )?;
        }
        Ok(
            FlowFrameLoopDecision::plan(FlowFrameLoopStorePlan::GrantBodyFrameStart {
                loop_instance_id: grant.loop_instance_id.clone(),
                expected_loop: loop_snapshot.clone(),
                next_loop,
                frame_id: frame_id.clone(),
                initial_frame,
                ledger_entry,
                expected_run_state: expected_run_state.clone(),
                next_run_state,
            })
            .with_work(FlowFrameLoopWork::RevisitFrame { frame_id }),
        )
    }

    pub fn register_ready_frame_if_needed(
        run_state: &KernelState,
        frame_id: &FrameId,
        frame_state: &KernelState,
    ) -> Result<Option<FlowFrameLoopStorePlan>, MobError> {
        if frame_state.phase != "Running"
            || !frame_ready(frame_state)
            || state_set_contains(run_state, "ready_frame_membership", frame_id)?
        {
            return Ok(None);
        }
        Ok(Some(FlowFrameLoopStorePlan::RunStateOnly {
            expected_run_state: run_state.clone(),
            next_run_state: run_transition_state(
                run_state,
                "RegisterReadyFrame",
                frame_id_fields(frame_id),
            )?,
        }))
    }

    pub fn release_node_execution(
        run_state: &KernelState,
        frame_id: &FrameId,
    ) -> Result<FlowFrameLoopStorePlan, MobError> {
        Ok(FlowFrameLoopStorePlan::RunStateOnly {
            expected_run_state: run_state.clone(),
            next_run_state: run_transition_state(
                run_state,
                "NodeExecutionReleased",
                frame_id_fields(frame_id),
            )?,
        })
    }

    pub fn seal_frame_if_ready(
        frame_id: &FrameId,
        current_frame: &FrameSnapshot,
        frame_spec: &FrameSpec,
    ) -> Result<Option<FlowFrameLoopStorePlan>, MobError> {
        if current_frame.kernel_state.phase != "Running"
            || !all_nodes_terminal(&current_frame.kernel_state, frame_spec)
        {
            return Ok(None);
        }
        let outcome =
            frame_transition_outcome(&current_frame.kernel_state, "SealFrame", BTreeMap::new())?;
        Ok(Some(FlowFrameLoopStorePlan::SealFrame {
            frame_id: frame_id.clone(),
            expected_frame: current_frame.clone(),
            next_frame: FrameSnapshot {
                kernel_state: outcome.next_state,
            },
        }))
    }

    pub fn advance_body_frame_after_seal(
        run_state: &KernelState,
        body_frame_id: &FrameId,
        body_frame: &FrameSnapshot,
        loop_snapshot: &LoopSnapshot,
        parent_frame: Option<&FrameSnapshot>,
    ) -> Result<Option<FlowFrameLoopDecision>, MobError> {
        if !frame_is_body(&body_frame.kernel_state) || body_frame.kernel_state.phase == "Running" {
            return Ok(None);
        }
        let Some(loop_instance_id) = frame_loop_instance_id(&body_frame.kernel_state) else {
            return Ok(None);
        };
        if active_body_frame_id(&loop_snapshot.kernel_state).as_ref() != Some(body_frame_id) {
            return Ok(None);
        }

        let iteration = frame_iteration(&body_frame.kernel_state)?;
        let first_variant = match terminal_phase(&body_frame.kernel_state)? {
            FlowFrameTerminalPhase::Completed => "BodyFrameCompleted",
            FlowFrameTerminalPhase::Failed => "BodyFrameFailed",
            FlowFrameTerminalPhase::Canceled => "BodyFrameCanceled",
        };
        let next_run_state =
            run_transition_state(run_state, "FrameTerminated", frame_id_fields(body_frame_id))?;
        let loop_outcome = loop_transition_outcome(
            &loop_snapshot.kernel_state,
            first_variant,
            BTreeMap::from([
                (
                    "loop_instance_id".into(),
                    KernelValue::String(loop_instance_id.to_string()),
                ),
                ("iteration".into(), KernelValue::U64(iteration)),
            ]),
        )?;
        let next_loop = LoopSnapshot {
            kernel_state: loop_outcome.next_state.clone(),
        };

        match terminal_phase(&body_frame.kernel_state)? {
            FlowFrameTerminalPhase::Completed => {
                let request = loop_outcome
                    .effects
                    .iter()
                    .find(|effect| effect.variant == "EvaluateUntilCondition")
                    .ok_or_else(|| {
                        MobError::Internal(format!(
                            "loop '{loop_instance_id}' did not emit EvaluateUntilCondition after body-frame completion"
                        ))
                    })?;
                let obligation = accept_evaluate_until_condition(
                    LoopUntilEvaluationRequested::from_effect(request)?,
                );
                Ok(Some(
                    FlowFrameLoopDecision::plan(FlowFrameLoopStorePlan::CompleteBodyFrame {
                        loop_instance_id,
                        expected_loop: loop_snapshot.clone(),
                        next_loop,
                        frame_id: body_frame_id.clone(),
                        expected_frame: body_frame.clone(),
                        next_frame: body_frame.clone(),
                        expected_run_state: run_state.clone(),
                        next_run_state,
                    })
                    .with_work(FlowFrameLoopWork::EvaluateUntil { obligation }),
                ))
            }
            FlowFrameTerminalPhase::Failed | FlowFrameTerminalPhase::Canceled => {
                let parent_frame = parent_frame.ok_or_else(|| {
                    MobError::Internal(format!(
                        "body-frame '{body_frame_id}' terminalization requires parent frame snapshot"
                    ))
                })?;
                let loop_terminal = first_matching_effect(
                    &loop_outcome.effects,
                    &["LoopFailed", "LoopCanceled"],
                )
                .ok_or_else(|| {
                    MobError::Internal(format!(
                        "loop '{loop_instance_id}' did not emit a terminal effect after '{first_variant}'"
                    ))
                })?;
                let parent_frame_id = loop_parent_frame_id(&loop_outcome.next_state)?;
                let parent_node_id = loop_parent_node_id(&loop_outcome.next_state)?;
                Ok(Some(build_complete_loop_decision(
                    CompleteLoopDecisionRequest {
                        loop_terminal,
                        expected_run_state: run_state,
                        next_run_state,
                        loop_instance_id: &loop_instance_id,
                        expected_loop: loop_snapshot,
                        next_loop,
                        parent_frame_id: &parent_frame_id,
                        expected_parent_frame: parent_frame,
                        parent_node_id: &parent_node_id,
                    },
                )?))
            }
        }
    }

    pub fn pending_until_obligation(
        loop_snapshot: &LoopSnapshot,
    ) -> Result<Option<FlowLoopUntilEvaluationObligation>, MobError> {
        if loop_snapshot.kernel_state.phase != "Running"
            || loop_stage(&loop_snapshot.kernel_state)? != "AwaitingUntil"
        {
            return Ok(None);
        }
        Ok(Some(FlowLoopUntilEvaluationObligation {
            loop_instance_id: loop_instance_id_from_state(&loop_snapshot.kernel_state)?,
            iteration: u32_from_state_field(
                &loop_snapshot.kernel_state,
                "last_completed_iteration",
            )?,
            parent_frame_id: loop_parent_frame_id(&loop_snapshot.kernel_state)?,
            parent_node_id: loop_parent_node_id(&loop_snapshot.kernel_state)?,
            loop_id: loop_id_from_state(&loop_snapshot.kernel_state)?,
        }))
    }

    pub fn resolve_until_feedback(
        run_state: &KernelState,
        loop_snapshot: &LoopSnapshot,
        parent_frame: &FrameSnapshot,
        obligation: FlowLoopUntilEvaluationObligation,
        until_met: bool,
    ) -> Result<FlowFrameLoopDecision, MobError> {
        let mut authority = LoopIterationAuthority::from_state(loop_snapshot.kernel_state.clone());
        let feedback = if until_met {
            submit_until_condition_met(&mut authority, obligation.clone())?
        } else {
            submit_until_condition_failed(&mut authority, obligation.clone())?
        };
        let next_loop = LoopSnapshot {
            kernel_state: feedback.next_state.clone(),
        };

        if let Some(depth) = maybe_effect_u64(&feedback.effects, "RequestBodyFrameStart", "depth")?
        {
            let mut next_run_state = run_state.clone();
            if !state_set_contains(
                run_state,
                "pending_body_frame_loop_membership",
                &obligation.loop_instance_id,
            )? {
                next_run_state = run_transition_state(
                    &next_run_state,
                    "RegisterPendingBodyFrame",
                    loop_register_pending_fields(&obligation.loop_instance_id, depth),
                )?;
            }
            return Ok(FlowFrameLoopDecision::plan(
                FlowFrameLoopStorePlan::LoopRequestBodyFrame {
                    loop_instance_id: obligation.loop_instance_id,
                    expected_loop: loop_snapshot.clone(),
                    next_loop,
                    expected_run_state: run_state.clone(),
                    next_run_state,
                },
            ));
        }

        let loop_terminal = first_matching_effect(
            &feedback.effects,
            &[
                "LoopCompleted",
                "LoopExhausted",
                "LoopFailed",
                "LoopCanceled",
            ],
        )
        .ok_or_else(|| {
            MobError::Internal(format!(
                "until feedback for loop '{}' produced neither RequestBodyFrameStart nor terminal effect",
                obligation.loop_instance_id
            ))
        })?;
        build_complete_loop_decision(CompleteLoopDecisionRequest {
            loop_terminal,
            expected_run_state: run_state,
            next_run_state: run_state.clone(),
            loop_instance_id: &obligation.loop_instance_id,
            expected_loop: loop_snapshot,
            next_loop,
            parent_frame_id: &obligation.parent_frame_id,
            expected_parent_frame: parent_frame,
            parent_node_id: &obligation.parent_node_id,
        })
    }

    pub fn recover_terminal_loop_projection(
        run_state: &KernelState,
        loop_snapshot: &LoopSnapshot,
        parent_frame: &FrameSnapshot,
    ) -> Result<Option<FlowFrameLoopDecision>, MobError> {
        let Some(loop_terminal) = loop_terminal_effect(loop_snapshot.kernel_state.phase.as_str())
        else {
            return Ok(None);
        };
        let loop_instance_id = loop_instance_id_from_state(&loop_snapshot.kernel_state)?;
        let parent_frame_id = loop_parent_frame_id(&loop_snapshot.kernel_state)?;
        let parent_node_id = loop_parent_node_id(&loop_snapshot.kernel_state)?;
        Ok(Some(build_complete_loop_decision(
            CompleteLoopDecisionRequest {
                loop_terminal,
                expected_run_state: run_state,
                next_run_state: run_state.clone(),
                loop_instance_id: &loop_instance_id,
                expected_loop: loop_snapshot,
                next_loop: loop_snapshot.clone(),
                parent_frame_id: &parent_frame_id,
                expected_parent_frame: parent_frame,
                parent_node_id: &parent_node_id,
            },
        )?))
    }

    pub fn root_terminal_phase(frame: &FrameSnapshot) -> Option<FlowFrameTerminalPhase> {
        terminal_phase(&frame.kernel_state).ok()
    }
}

struct CompleteLoopDecisionRequest<'a> {
    loop_terminal: &'a str,
    expected_run_state: &'a KernelState,
    next_run_state: KernelState,
    loop_instance_id: &'a LoopInstanceId,
    expected_loop: &'a LoopSnapshot,
    next_loop: LoopSnapshot,
    parent_frame_id: &'a FrameId,
    expected_parent_frame: &'a FrameSnapshot,
    parent_node_id: &'a FlowNodeId,
}

fn build_complete_loop_decision(
    request: CompleteLoopDecisionRequest<'_>,
) -> Result<FlowFrameLoopDecision, MobError> {
    let CompleteLoopDecisionRequest {
        loop_terminal,
        expected_run_state,
        mut next_run_state,
        loop_instance_id,
        expected_loop,
        next_loop,
        parent_frame_id,
        expected_parent_frame,
        parent_node_id,
    } = request;
    let parent_input_variant = match loop_terminal {
        "LoopCompleted" => "CompleteNode",
        "LoopExhausted" | "LoopFailed" => "FailNode",
        "LoopCanceled" => "CancelNode",
        other => {
            return Err(MobError::Internal(format!(
                "unknown loop terminal effect '{other}'"
            )));
        }
    };
    let parent_outcome = frame_transition_outcome(
        &expected_parent_frame.kernel_state,
        parent_input_variant,
        BTreeMap::from([(
            "node_id".into(),
            KernelValue::String(parent_node_id.to_string()),
        )]),
    )?;
    let next_parent_frame = FrameSnapshot {
        kernel_state: parent_outcome.next_state,
    };
    if frame_ready(&next_parent_frame.kernel_state)
        && !state_set_contains(&next_run_state, "ready_frame_membership", parent_frame_id)?
    {
        next_run_state = run_transition_state(
            &next_run_state,
            "RegisterReadyFrame",
            frame_id_fields(parent_frame_id),
        )?;
    }
    Ok(
        FlowFrameLoopDecision::plan(FlowFrameLoopStorePlan::CompleteLoop {
            loop_instance_id: loop_instance_id.clone(),
            expected_loop: expected_loop.clone(),
            next_loop,
            frame_id: parent_frame_id.clone(),
            expected_frame: expected_parent_frame.clone(),
            next_frame: next_parent_frame,
            expected_run_state: expected_run_state.clone(),
            next_run_state,
        })
        .with_work(FlowFrameLoopWork::RevisitFrame {
            frame_id: parent_frame_id.clone(),
        }),
    )
}

fn initial_loop_snapshot(
    loop_instance_id: &LoopInstanceId,
    loop_spec: &RepeatUntilSpec,
    parent_frame_id: &FrameId,
    parent_node_id: &FlowNodeId,
    depth: u32,
) -> Result<LoopSnapshot, MobError> {
    let initial = loop_iteration::initial_state().map_err(|error| {
        MobError::Internal(format!("loop_iteration initial_state failed: {error:?}"))
    })?;
    let start = loop_transition_outcome(
        &initial,
        "StartLoop",
        BTreeMap::from([
            (
                "loop_instance_id".into(),
                KernelValue::String(loop_instance_id.to_string()),
            ),
            (
                "max_iterations".into(),
                KernelValue::U64(loop_spec.max_iterations as u64),
            ),
            (
                "parent_frame_id".into(),
                KernelValue::String(parent_frame_id.to_string()),
            ),
            (
                "parent_node_id".into(),
                KernelValue::String(parent_node_id.to_string()),
            ),
            (
                "loop_id".into(),
                KernelValue::String(loop_spec.loop_id.to_string()),
            ),
            ("depth".into(), KernelValue::U64(u64::from(depth))),
        ]),
    )?;
    Ok(LoopSnapshot {
        kernel_state: start.next_state,
    })
}

fn initial_body_frame_snapshot(
    frame_id: &FrameId,
    loop_instance_id: &LoopInstanceId,
    iteration: u64,
    spec: &FrameSpec,
) -> Result<FrameSnapshot, MobError> {
    let initial = flow_frame::initial_state().map_err(|error| {
        MobError::Internal(format!("flow_frame initial_state failed: {error:?}"))
    })?;
    let ordered = topological_order(spec)?;
    let start_input =
        build_start_body_frame_input(frame_id, loop_instance_id, iteration, spec, &ordered);
    let outcome = flow_frame::transition(&initial, &start_input).map_err(|error| {
        MobError::Internal(format!("flow_frame StartBodyFrame failed: {error:?}"))
    })?;
    Ok(FrameSnapshot {
        kernel_state: outcome.next_state,
    })
}

fn run_transition_state(
    state: &KernelState,
    variant: &str,
    fields: BTreeMap<String, KernelValue>,
) -> Result<KernelState, MobError> {
    Ok(run_transition_outcome(state, variant, fields)?.next_state)
}

fn run_transition_outcome(
    state: &KernelState,
    variant: &str,
    fields: BTreeMap<String, KernelValue>,
) -> Result<TransitionOutcome, MobError> {
    flow_run::transition(
        state,
        &KernelInput {
            variant: variant.to_string(),
            fields,
        },
    )
    .map_err(|error| MobError::Internal(format!("flow_run {variant} transition refused: {error}")))
}

fn frame_transition_outcome(
    state: &KernelState,
    variant: &str,
    fields: BTreeMap<String, KernelValue>,
) -> Result<TransitionOutcome, MobError> {
    flow_frame::transition(
        state,
        &KernelInput {
            variant: variant.to_string(),
            fields,
        },
    )
    .map_err(|error| {
        MobError::Internal(format!("flow_frame {variant} transition refused: {error}"))
    })
}

fn loop_transition_outcome(
    state: &KernelState,
    variant: &str,
    fields: BTreeMap<String, KernelValue>,
) -> Result<TransitionOutcome, MobError> {
    loop_iteration::transition(
        state,
        &KernelInput {
            variant: variant.to_string(),
            fields,
        },
    )
    .map_err(|error| {
        MobError::Internal(format!(
            "loop_iteration {variant} transition refused: {error}"
        ))
    })
}

fn loop_register_pending_fields(
    loop_instance_id: &LoopInstanceId,
    depth: u64,
) -> BTreeMap<String, KernelValue> {
    BTreeMap::from([
        (
            "loop_instance_id".into(),
            KernelValue::String(loop_instance_id.to_string()),
        ),
        ("depth".into(), KernelValue::U64(depth)),
    ])
}

fn frame_id_fields(frame_id: &FrameId) -> BTreeMap<String, KernelValue> {
    BTreeMap::from([("frame_id".into(), KernelValue::String(frame_id.to_string()))])
}

fn all_nodes_terminal(state: &KernelState, spec: &FrameSpec) -> bool {
    let Some(KernelValue::Map(status_map)) = state.fields.get("node_status") else {
        return false;
    };
    let terminal = ["Completed", "Failed", "Skipped", "Canceled"];
    for node_id in spec.nodes.keys() {
        let key = KernelValue::String(node_id.to_string());
        match status_map.get(&key) {
            Some(KernelValue::NamedVariant { variant, .. })
                if terminal.contains(&variant.as_str()) => {}
            _ => return false,
        }
    }
    true
}

fn frame_ready(state: &KernelState) -> bool {
    match state.fields.get("ready_queue") {
        Some(KernelValue::Seq(queue)) => !queue.is_empty(),
        _ => false,
    }
}

fn state_set_contains<T>(state: &KernelState, field: &str, value: &T) -> Result<bool, MobError>
where
    T: ToString,
{
    let set = match state.fields.get(field) {
        Some(KernelValue::Set(set)) => set,
        other => {
            return Err(MobError::Internal(format!(
                "kernel state field '{field}' missing or invalid set: {other:?}"
            )));
        }
    };
    Ok(set.contains(&KernelValue::String(value.to_string())))
}

fn has_effect_variant(effects: &[KernelEffect], variant: &str) -> bool {
    effects.iter().any(|effect| effect.variant == variant)
}

fn first_matching_effect<'a>(effects: &'a [KernelEffect], variants: &[&str]) -> Option<&'a str> {
    effects
        .iter()
        .map(|effect| effect.variant.as_str())
        .find(|variant| variants.contains(variant))
}

fn effect_node_id(
    effects: &[KernelEffect],
    expected_variant: &str,
) -> Result<Option<FlowNodeId>, MobError> {
    effects
        .iter()
        .find(|effect| effect.variant == expected_variant)
        .map(|effect| {
            string_from_effect_field(effect, "node_id")
                .map(|value| FlowNodeId::from(value.as_str()))
        })
        .transpose()
}

fn effect_frame_id(
    effects: &[KernelEffect],
    expected_variant: &str,
) -> Result<Option<FrameId>, MobError> {
    effects
        .iter()
        .find(|effect| effect.variant == expected_variant)
        .map(|effect| {
            string_from_effect_field(effect, "frame_id").map(|value| FrameId::from(value.as_str()))
        })
        .transpose()
}

fn effect_loop_id(
    effects: &[KernelEffect],
    expected_variant: &str,
) -> Result<Option<LoopInstanceId>, MobError> {
    effects
        .iter()
        .find(|effect| effect.variant == expected_variant)
        .map(|effect| {
            string_from_effect_field(effect, "loop_instance_id")
                .map(|value| LoopInstanceId::from(value.as_str()))
        })
        .transpose()
}

fn maybe_effect_u64(
    effects: &[KernelEffect],
    expected_variant: &str,
    field: &str,
) -> Result<Option<u64>, MobError> {
    effects
        .iter()
        .find(|effect| effect.variant == expected_variant)
        .map(|effect| u64_from_effect_field(effect, field))
        .transpose()
}

fn string_from_effect_field(effect: &KernelEffect, field: &str) -> Result<String, MobError> {
    match effect.fields.get(field) {
        Some(KernelValue::String(value)) => Ok(value.clone()),
        other => Err(MobError::Internal(format!(
            "effect '{}' missing String field '{}': {other:?}",
            effect.variant, field
        ))),
    }
}

fn u64_from_effect_field(effect: &KernelEffect, field: &str) -> Result<u64, MobError> {
    match effect.fields.get(field) {
        Some(KernelValue::U64(value)) => Ok(*value),
        other => Err(MobError::Internal(format!(
            "effect '{}' missing U64 field '{}': {other:?}",
            effect.variant, field
        ))),
    }
}

fn string_from_state_field(state: &KernelState, field: &str) -> Result<String, MobError> {
    match state.fields.get(field) {
        Some(KernelValue::String(value)) => Ok(value.clone()),
        other => Err(MobError::Internal(format!(
            "kernel state missing String field '{field}': {other:?}"
        ))),
    }
}

fn u64_from_state_field(state: &KernelState, field: &str) -> Result<u64, MobError> {
    match state.fields.get(field) {
        Some(KernelValue::U64(value)) => Ok(*value),
        other => Err(MobError::Internal(format!(
            "kernel state missing U64 field '{field}': {other:?}"
        ))),
    }
}

fn u32_from_state_field(state: &KernelState, field: &str) -> Result<u32, MobError> {
    let value = u64_from_state_field(state, field)?;
    u32::try_from(value).map_err(|_| {
        MobError::Internal(format!(
            "kernel state field '{field}' value {value} does not fit in u32"
        ))
    })
}

fn named_variant_from_state_field<'a>(
    state: &'a KernelState,
    field: &str,
) -> Result<&'a str, MobError> {
    match state.fields.get(field) {
        Some(KernelValue::NamedVariant { variant, .. }) => Ok(variant.as_str()),
        other => Err(MobError::Internal(format!(
            "kernel state missing enum field '{field}': {other:?}"
        ))),
    }
}

fn frame_is_body(state: &KernelState) -> bool {
    matches!(
        state.fields.get("frame_scope"),
        Some(KernelValue::NamedVariant { variant, .. }) if variant == "Body"
    )
}

fn frame_loop_instance_id(state: &KernelState) -> Option<LoopInstanceId> {
    match state.fields.get("loop_instance_id") {
        Some(KernelValue::String(loop_instance_id)) if !loop_instance_id.is_empty() => {
            Some(LoopInstanceId::from(loop_instance_id.as_str()))
        }
        _ => None,
    }
}

fn frame_iteration(state: &KernelState) -> Result<u64, MobError> {
    u64_from_state_field(state, "iteration")
}

fn loop_current_iteration(state: &KernelState) -> Result<u64, MobError> {
    u64_from_state_field(state, "current_iteration")
}

fn loop_instance_id_from_state(state: &KernelState) -> Result<LoopInstanceId, MobError> {
    Ok(LoopInstanceId::from(string_from_state_field(
        state,
        "loop_instance_id",
    )?))
}

fn loop_parent_frame_id(state: &KernelState) -> Result<FrameId, MobError> {
    Ok(FrameId::from(string_from_state_field(
        state,
        "parent_frame_id",
    )?))
}

fn loop_parent_node_id(state: &KernelState) -> Result<FlowNodeId, MobError> {
    Ok(FlowNodeId::from(string_from_state_field(
        state,
        "parent_node_id",
    )?))
}

fn loop_id_from_state(state: &KernelState) -> Result<LoopId, MobError> {
    Ok(LoopId::from(string_from_state_field(state, "loop_id")?))
}

fn loop_stage(state: &KernelState) -> Result<&str, MobError> {
    named_variant_from_state_field(state, "stage")
}

fn active_body_frame_id(state: &KernelState) -> Option<FrameId> {
    match state.fields.get("active_body_frame_id") {
        Some(KernelValue::String(frame_id)) if !frame_id.is_empty() => {
            Some(FrameId::from(frame_id.as_str()))
        }
        _ => None,
    }
}

fn state_is_awaiting_body_frame(state: &KernelState) -> bool {
    matches!(loop_stage(state), Ok("AwaitingBodyFrame"))
}

fn loop_terminal_effect(phase: &str) -> Option<&'static str> {
    match phase {
        "Completed" => Some("LoopCompleted"),
        "Exhausted" => Some("LoopExhausted"),
        "Failed" => Some("LoopFailed"),
        "Canceled" => Some("LoopCanceled"),
        _ => None,
    }
}

fn terminal_phase(state: &KernelState) -> Result<FlowFrameTerminalPhase, MobError> {
    match state.phase.as_str() {
        "Completed" => Ok(FlowFrameTerminalPhase::Completed),
        "Failed" => Ok(FlowFrameTerminalPhase::Failed),
        "Canceled" => Ok(FlowFrameTerminalPhase::Canceled),
        other => Err(MobError::Internal(format!(
            "frame is not in a terminal phase: '{other}'"
        ))),
    }
}

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
use crate::flow_machine_types::{
    flow_node_id, frame_id, local_flow_node_id, local_frame_id, local_loop_id,
    local_loop_instance_id, loop_id, loop_instance_id,
};
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
use meerkat_machine_kernels::compat_generated::{flow_frame, flow_run, loop_iteration};
use meerkat_machine_kernels::test_oracle::KernelValue;
use meerkat_machine_schema::compat::types as kernel_types;
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
struct RawEffect {
    variant: String,
    fields: BTreeMap<String, KernelValue>,
}

#[derive(Debug, Clone)]
struct RawOutcome<S> {
    next_state: S,
    effects: Vec<RawEffect>,
}

#[derive(Debug, Clone)]
pub struct PreviewedNodeGrant {
    pub frame_id: FrameId,
    pub next_run_state: flow_run::State,
}

#[derive(Debug, Clone)]
pub struct PreviewedBodyFrameGrant {
    pub loop_instance_id: LoopInstanceId,
    pub next_run_state: flow_run::State,
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
        expected_run_state: flow_run::State,
        next_run_state: flow_run::State,
        frame_id: FrameId,
        expected_frame: FrameSnapshot,
        next_frame: FrameSnapshot,
    },
    StartLoop {
        loop_instance_id: LoopInstanceId,
        expected_run_state: flow_run::State,
        next_run_state: flow_run::State,
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
        expected_run_state: flow_run::State,
        next_run_state: flow_run::State,
    },
    RunStateOnly {
        expected_run_state: flow_run::State,
        next_run_state: flow_run::State,
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
        expected_run_state: flow_run::State,
        next_run_state: flow_run::State,
    },
    LoopRequestBodyFrame {
        loop_instance_id: LoopInstanceId,
        expected_loop: LoopSnapshot,
        next_loop: LoopSnapshot,
        expected_run_state: flow_run::State,
        next_run_state: flow_run::State,
    },
    CompleteLoop {
        loop_instance_id: LoopInstanceId,
        expected_loop: LoopSnapshot,
        next_loop: LoopSnapshot,
        frame_id: FrameId,
        expected_frame: FrameSnapshot,
        next_frame: FrameSnapshot,
        expected_run_state: flow_run::State,
        next_run_state: flow_run::State,
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
    pub fn preview_node_grant(
        state: &flow_run::State,
    ) -> Result<Option<PreviewedNodeGrant>, MobError> {
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
        state: &flow_run::State,
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
        expected_run_state: &flow_run::State,
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
        expected_run_state: &flow_run::State,
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
        run_state: &flow_run::State,
        frame_id: &FrameId,
        frame_state: &flow_frame::State,
    ) -> Result<Option<FlowFrameLoopStorePlan>, MobError> {
        if frame_state.phase != flow_frame::Phase::Running
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
        run_state: &flow_run::State,
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
        if current_frame.kernel_state.phase != flow_frame::Phase::Running
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
        run_state: &flow_run::State,
        body_frame_id: &FrameId,
        body_frame: &FrameSnapshot,
        loop_snapshot: &LoopSnapshot,
        parent_frame: Option<&FrameSnapshot>,
    ) -> Result<Option<FlowFrameLoopDecision>, MobError> {
        if !frame_is_body(&body_frame.kernel_state)
            || body_frame.kernel_state.phase == flow_frame::Phase::Running
        {
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
                let obligation =
                    accept_evaluate_until_condition(loop_until_request_from_raw_effect(request)?);
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
        if loop_snapshot.kernel_state.phase != loop_iteration::Phase::Running
            || loop_stage(&loop_snapshot.kernel_state)? != "AwaitingUntil"
        {
            return Ok(None);
        }
        Ok(Some(FlowLoopUntilEvaluationObligation {
            loop_instance_id: loop_instance_id_from_state(&loop_snapshot.kernel_state)?,
            iteration: loop_snapshot.kernel_state.last_completed_iteration,
            parent_frame_id: loop_parent_frame_id(&loop_snapshot.kernel_state)?,
            parent_node_id: loop_parent_node_id(&loop_snapshot.kernel_state)?,
            loop_id: loop_id_from_state(&loop_snapshot.kernel_state)?,
        }))
    }

    pub fn resolve_until_feedback(
        run_state: &flow_run::State,
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
        let feedback_effects = feedback
            .effects
            .into_iter()
            .map(raw_loop_effect)
            .collect::<Result<Vec<_>, _>>()?;
        let next_loop = LoopSnapshot {
            kernel_state: feedback.next_state,
        };

        if let Some(depth) = maybe_effect_u64(&feedback_effects, "RequestBodyFrameStart", "depth")?
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
            &feedback_effects,
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
        run_state: &flow_run::State,
        loop_snapshot: &LoopSnapshot,
        parent_frame: &FrameSnapshot,
    ) -> Result<Option<FlowFrameLoopDecision>, MobError> {
        let Some(loop_terminal) = loop_terminal_effect(&loop_snapshot.kernel_state.phase) else {
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

    pub fn recover_pending_body_frame_request(
        run_state: &flow_run::State,
        loop_snapshot: &LoopSnapshot,
    ) -> Result<Option<FlowFrameLoopDecision>, MobError> {
        if loop_snapshot.kernel_state.phase != loop_iteration::Phase::Running
            || !state_is_awaiting_body_frame(&loop_snapshot.kernel_state)
            || active_body_frame_id(&loop_snapshot.kernel_state).is_some()
        {
            return Ok(None);
        }

        let loop_instance_id = loop_instance_id_from_state(&loop_snapshot.kernel_state)?;
        if state_set_contains(
            run_state,
            "pending_body_frame_loop_membership",
            &loop_instance_id,
        )? {
            return Ok(None);
        }

        let depth = u64::from(loop_snapshot.kernel_state.depth);
        let next_run_state = run_transition_state(
            run_state,
            "RegisterPendingBodyFrame",
            loop_register_pending_fields(&loop_instance_id, depth),
        )?;
        Ok(Some(FlowFrameLoopDecision::plan(
            FlowFrameLoopStorePlan::RunStateOnly {
                expected_run_state: run_state.clone(),
                next_run_state,
            },
        )))
    }

    pub fn root_terminal_phase(frame: &FrameSnapshot) -> Option<FlowFrameTerminalPhase> {
        terminal_phase(&frame.kernel_state).ok()
    }
}

struct CompleteLoopDecisionRequest<'a> {
    loop_terminal: &'a str,
    expected_run_state: &'a flow_run::State,
    next_run_state: flow_run::State,
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
    let initial = loop_iteration::initial_state();
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
    let initial = flow_frame::initial_state();
    let ordered = topological_order(spec)?;
    let start_input =
        build_start_body_frame_input(frame_id, loop_instance_id, iteration, spec, &ordered);
    let outcome = flow_frame::transition(&initial, start_input, &flow_frame::EmptyContext)
        .map_err(|error| {
            MobError::Internal(format!("flow_frame StartBodyFrame failed: {error:?}"))
        })?;
    Ok(FrameSnapshot {
        kernel_state: outcome.next_state,
    })
}

fn run_transition_state(
    state: &flow_run::State,
    variant: &str,
    fields: BTreeMap<String, KernelValue>,
) -> Result<flow_run::State, MobError> {
    Ok(run_transition_outcome(state, variant, fields)?.next_state)
}

fn run_transition_outcome(
    state: &flow_run::State,
    variant: &str,
    fields: BTreeMap<String, KernelValue>,
) -> Result<RawOutcome<flow_run::State>, MobError> {
    let input = match variant {
        "PumpNodeScheduler" => {
            flow_run::Input::PumpNodeScheduler(flow_run::inputs::PumpNodeScheduler)
        }
        "PumpFrameScheduler" => {
            flow_run::Input::PumpFrameScheduler(flow_run::inputs::PumpFrameScheduler)
        }
        "NodeExecutionReleased" => {
            flow_run::Input::NodeExecutionReleased(flow_run::inputs::NodeExecutionReleased {
                frame_id: frame_id(&FrameId::from(string_field(&fields, "frame_id")?.as_str())),
            })
        }
        "RegisterReadyFrame" => {
            flow_run::Input::RegisterReadyFrame(flow_run::inputs::RegisterReadyFrame {
                frame_id: frame_id(&FrameId::from(string_field(&fields, "frame_id")?.as_str())),
            })
        }
        "RegisterPendingBodyFrame" => {
            flow_run::Input::RegisterPendingBodyFrame(flow_run::inputs::RegisterPendingBodyFrame {
                loop_instance_id: loop_instance_id(&LoopInstanceId::from(
                    string_field(&fields, "loop_instance_id")?.as_str(),
                )),
                depth: u32::try_from(u64_field(&fields, "depth")?).map_err(|_| {
                    MobError::Internal("RegisterPendingBodyFrame depth exceeds u32".into())
                })?,
            })
        }
        "FrameTerminated" => flow_run::Input::FrameTerminated(flow_run::inputs::FrameTerminated {
            frame_id: frame_id(&FrameId::from(string_field(&fields, "frame_id")?.as_str())),
        }),
        other => {
            return Err(MobError::Internal(format!(
                "unsupported flow_run driver variant `{other}`"
            )));
        }
    };
    let outcome = flow_run::transition(state, input, &flow_run::EmptyContext).map_err(|error| {
        MobError::Internal(format!("flow_run {variant} transition refused: {error:?}"))
    })?;
    Ok(RawOutcome {
        next_state: outcome.next_state,
        effects: outcome
            .effects
            .into_iter()
            .map(raw_flow_run_effect)
            .collect::<Result<Vec<_>, _>>()?,
    })
}

fn frame_transition_outcome(
    state: &flow_frame::State,
    variant: &str,
    fields: BTreeMap<String, KernelValue>,
) -> Result<RawOutcome<flow_frame::State>, MobError> {
    let input = match variant {
        "AdmitNextReadyNode" => {
            flow_frame::Input::AdmitNextReadyNode(flow_frame::inputs::AdmitNextReadyNode)
        }
        "SealFrame" => flow_frame::Input::SealFrame(flow_frame::inputs::SealFrame),
        "CompleteNode" => flow_frame::Input::CompleteNode(flow_frame::inputs::CompleteNode {
            node_id: flow_node_id(&FlowNodeId::from(
                string_field(&fields, "node_id")?.as_str(),
            )),
        }),
        "FailNode" => flow_frame::Input::FailNode(flow_frame::inputs::FailNode {
            node_id: flow_node_id(&FlowNodeId::from(
                string_field(&fields, "node_id")?.as_str(),
            )),
        }),
        "CancelNode" => flow_frame::Input::CancelNode(flow_frame::inputs::CancelNode {
            node_id: flow_node_id(&FlowNodeId::from(
                string_field(&fields, "node_id")?.as_str(),
            )),
        }),
        "SkipNode" => flow_frame::Input::SkipNode(flow_frame::inputs::SkipNode {
            node_id: flow_node_id(&FlowNodeId::from(
                string_field(&fields, "node_id")?.as_str(),
            )),
        }),
        other => {
            return Err(MobError::Internal(format!(
                "unsupported flow_frame driver variant `{other}`"
            )));
        }
    };
    let outcome =
        flow_frame::transition(state, input, &flow_frame::EmptyContext).map_err(|error| {
            MobError::Internal(format!(
                "flow_frame {variant} transition refused: {error:?}"
            ))
        })?;
    Ok(RawOutcome {
        next_state: outcome.next_state,
        effects: outcome
            .effects
            .into_iter()
            .map(raw_flow_frame_effect)
            .collect::<Result<Vec<_>, _>>()?,
    })
}

fn loop_transition_outcome(
    state: &loop_iteration::State,
    variant: &str,
    fields: BTreeMap<String, KernelValue>,
) -> Result<RawOutcome<loop_iteration::State>, MobError> {
    let input = match variant {
        "StartLoop" => loop_iteration::Input::StartLoop(loop_iteration::inputs::StartLoop {
            loop_instance_id: loop_instance_id(&LoopInstanceId::from(
                string_field(&fields, "loop_instance_id")?.as_str(),
            )),
            max_iterations: u32::try_from(u64_field(&fields, "max_iterations")?)
                .map_err(|_| MobError::Internal("StartLoop max_iterations exceeds u32".into()))?,
            parent_frame_id: frame_id(&FrameId::from(
                string_field(&fields, "parent_frame_id")?.as_str(),
            )),
            parent_node_id: flow_node_id(&FlowNodeId::from(
                string_field(&fields, "parent_node_id")?.as_str(),
            )),
            loop_id: loop_id(&LoopId::from(string_field(&fields, "loop_id")?.as_str())),
            depth: u32::try_from(u64_field(&fields, "depth")?)
                .map_err(|_| MobError::Internal("StartLoop depth exceeds u32".into()))?,
        }),
        "BodyFrameStarted" => {
            loop_iteration::Input::BodyFrameStarted(loop_iteration::inputs::BodyFrameStarted {
                loop_instance_id: loop_instance_id(&LoopInstanceId::from(
                    string_field(&fields, "loop_instance_id")?.as_str(),
                )),
                frame_id: frame_id(&FrameId::from(string_field(&fields, "frame_id")?.as_str())),
                iteration: u32::try_from(u64_field(&fields, "iteration")?).map_err(|_| {
                    MobError::Internal("BodyFrameStarted iteration exceeds u32".into())
                })?,
            })
        }
        "BodyFrameCompleted" => {
            loop_iteration::Input::BodyFrameCompleted(loop_iteration::inputs::BodyFrameCompleted {
                loop_instance_id: loop_instance_id(&LoopInstanceId::from(
                    string_field(&fields, "loop_instance_id")?.as_str(),
                )),
                iteration: u32::try_from(u64_field(&fields, "iteration")?).map_err(|_| {
                    MobError::Internal("BodyFrameCompleted iteration exceeds u32".into())
                })?,
            })
        }
        "BodyFrameFailed" => {
            loop_iteration::Input::BodyFrameFailed(loop_iteration::inputs::BodyFrameFailed {
                loop_instance_id: loop_instance_id(&LoopInstanceId::from(
                    string_field(&fields, "loop_instance_id")?.as_str(),
                )),
                iteration: u32::try_from(u64_field(&fields, "iteration")?).map_err(|_| {
                    MobError::Internal("BodyFrameFailed iteration exceeds u32".into())
                })?,
            })
        }
        "BodyFrameCanceled" => {
            loop_iteration::Input::BodyFrameCanceled(loop_iteration::inputs::BodyFrameCanceled {
                loop_instance_id: loop_instance_id(&LoopInstanceId::from(
                    string_field(&fields, "loop_instance_id")?.as_str(),
                )),
                iteration: u32::try_from(u64_field(&fields, "iteration")?).map_err(|_| {
                    MobError::Internal("BodyFrameCanceled iteration exceeds u32".into())
                })?,
            })
        }
        other => {
            return Err(MobError::Internal(format!(
                "unsupported loop_iteration driver variant `{other}`"
            )));
        }
    };
    let outcome = loop_iteration::transition(state, input, &loop_iteration::EmptyContext).map_err(
        |error| {
            MobError::Internal(format!(
                "loop_iteration {variant} transition refused: {error:?}"
            ))
        },
    )?;
    Ok(RawOutcome {
        next_state: outcome.next_state,
        effects: outcome
            .effects
            .into_iter()
            .map(raw_loop_effect)
            .collect::<Result<Vec<_>, _>>()?,
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

fn all_nodes_terminal(state: &flow_frame::State, spec: &FrameSpec) -> bool {
    for node_id in spec.nodes.keys() {
        let key = flow_node_id(node_id);
        match state.node_status.get(&key) {
            Some(
                kernel_types::NodeRunStatus::Completed
                | kernel_types::NodeRunStatus::Failed
                | kernel_types::NodeRunStatus::Skipped
                | kernel_types::NodeRunStatus::Canceled,
            ) => {}
            _ => return false,
        }
    }
    true
}

fn frame_ready(state: &flow_frame::State) -> bool {
    !state.ready_queue.is_empty()
}

fn state_set_contains<T>(state: &flow_run::State, field: &str, value: &T) -> Result<bool, MobError>
where
    T: ToString,
{
    match field {
        "ready_frame_membership" => Ok(state
            .ready_frame_membership
            .contains(&frame_id(&FrameId::from(value.to_string().as_str())))),
        "pending_body_frame_loop_membership" => Ok(state
            .pending_body_frame_loop_membership
            .contains(&loop_instance_id(&LoopInstanceId::from(
                value.to_string().as_str(),
            )))),
        other => Err(MobError::Internal(format!(
            "unsupported flow_run set field '{other}'"
        ))),
    }
}

fn has_effect_variant(effects: &[RawEffect], variant: &str) -> bool {
    effects.iter().any(|effect| effect.variant == variant)
}

fn first_matching_effect<'a>(effects: &'a [RawEffect], variants: &[&str]) -> Option<&'a str> {
    effects
        .iter()
        .map(|effect| effect.variant.as_str())
        .find(|variant| variants.contains(variant))
}

fn effect_node_id(
    effects: &[RawEffect],
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
    effects: &[RawEffect],
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
    effects: &[RawEffect],
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
    effects: &[RawEffect],
    expected_variant: &str,
    field: &str,
) -> Result<Option<u64>, MobError> {
    effects
        .iter()
        .find(|effect| effect.variant == expected_variant)
        .map(|effect| u64_from_effect_field(effect, field))
        .transpose()
}

fn loop_until_request_from_raw_effect(
    effect: &RawEffect,
) -> Result<LoopUntilEvaluationRequested, MobError> {
    if effect.variant != "EvaluateUntilCondition" {
        return Err(MobError::Internal(format!(
            "expected EvaluateUntilCondition effect, got '{}'",
            effect.variant
        )));
    }
    Ok(LoopUntilEvaluationRequested {
        loop_instance_id: LoopInstanceId::from(
            string_from_effect_field(effect, "loop_instance_id")?.as_str(),
        ),
        iteration: u32::try_from(u64_from_effect_field(effect, "iteration")?).map_err(|_| {
            MobError::Internal("EvaluateUntilCondition iteration exceeds u32".into())
        })?,
        parent_frame_id: FrameId::from(
            string_from_effect_field(effect, "parent_frame_id")?.as_str(),
        ),
        parent_node_id: FlowNodeId::from(
            string_from_effect_field(effect, "parent_node_id")?.as_str(),
        ),
        loop_id: LoopId::from(string_from_effect_field(effect, "loop_id")?.as_str()),
    })
}

fn string_field(fields: &BTreeMap<String, KernelValue>, field: &str) -> Result<String, MobError> {
    match fields.get(field) {
        Some(KernelValue::String(value)) => Ok(value.clone()),
        other => Err(MobError::Internal(format!(
            "raw field `{field}` expected string, got {other:?}"
        ))),
    }
}

fn u64_field(fields: &BTreeMap<String, KernelValue>, field: &str) -> Result<u64, MobError> {
    match fields.get(field) {
        Some(KernelValue::U64(value)) => Ok(*value),
        other => Err(MobError::Internal(format!(
            "raw field `{field}` expected u64, got {other:?}"
        ))),
    }
}

fn raw_flow_run_effect(effect: flow_run::Effect) -> Result<RawEffect, MobError> {
    let (variant, fields) = match effect {
        flow_run::Effect::EmitFlowRunNotice(payload) => (
            "EmitFlowRunNotice",
            BTreeMap::from([(
                "run_status".into(),
                KernelValue::NamedVariant {
                    enum_name: "FlowRunStatus".into(),
                    variant: format!("{:?}", payload.run_status),
                },
            )]),
        ),
        flow_run::Effect::EmitStepNotice(payload) => (
            "EmitStepNotice",
            BTreeMap::from([
                (
                    "step_id".into(),
                    KernelValue::String(payload.step_id.to_string()),
                ),
                (
                    "step_status".into(),
                    KernelValue::NamedVariant {
                        enum_name: "StepRunStatus".into(),
                        variant: format!("{:?}", payload.step_status),
                    },
                ),
            ]),
        ),
        flow_run::Effect::AppendFailureLedger(payload) => (
            "AppendFailureLedger",
            BTreeMap::from([(
                "step_id".into(),
                KernelValue::String(payload.step_id.to_string()),
            )]),
        ),
        flow_run::Effect::PersistStepOutput(payload) => (
            "PersistStepOutput",
            BTreeMap::from([(
                "step_id".into(),
                KernelValue::String(payload.step_id.to_string()),
            )]),
        ),
        flow_run::Effect::AdmitStepWork(payload) => (
            "AdmitStepWork",
            BTreeMap::from([(
                "step_id".into(),
                KernelValue::String(payload.step_id.to_string()),
            )]),
        ),
        flow_run::Effect::FlowTerminalized(payload) => (
            "FlowTerminalized",
            BTreeMap::from([(
                "run_status".into(),
                KernelValue::NamedVariant {
                    enum_name: "FlowRunStatus".into(),
                    variant: format!("{:?}", payload.run_status),
                },
            )]),
        ),
        flow_run::Effect::EscalateSupervisor(payload) => (
            "EscalateSupervisor",
            BTreeMap::from([(
                "step_id".into(),
                KernelValue::String(payload.step_id.to_string()),
            )]),
        ),
        flow_run::Effect::ProjectTargetSuccess(payload) => (
            "ProjectTargetSuccess",
            BTreeMap::from([
                (
                    "step_id".into(),
                    KernelValue::String(payload.step_id.to_string()),
                ),
                (
                    "target_id".into(),
                    KernelValue::String(payload.target_id.to_string()),
                ),
            ]),
        ),
        flow_run::Effect::ProjectTargetFailure(payload) => (
            "ProjectTargetFailure",
            BTreeMap::from([
                (
                    "step_id".into(),
                    KernelValue::String(payload.step_id.to_string()),
                ),
                (
                    "target_id".into(),
                    KernelValue::String(payload.target_id.to_string()),
                ),
            ]),
        ),
        flow_run::Effect::ProjectTargetCanceled(payload) => (
            "ProjectTargetCanceled",
            BTreeMap::from([
                (
                    "step_id".into(),
                    KernelValue::String(payload.step_id.to_string()),
                ),
                (
                    "target_id".into(),
                    KernelValue::String(payload.target_id.to_string()),
                ),
            ]),
        ),
        flow_run::Effect::GrantNodeSlot(payload) => (
            "GrantNodeSlot",
            BTreeMap::from([(
                "frame_id".into(),
                KernelValue::String(payload.frame_id.to_string()),
            )]),
        ),
        flow_run::Effect::GrantBodyFrameStart(payload) => (
            "GrantBodyFrameStart",
            BTreeMap::from([(
                "loop_instance_id".into(),
                KernelValue::String(payload.loop_instance_id.to_string()),
            )]),
        ),
    };
    Ok(RawEffect {
        variant: variant.into(),
        fields,
    })
}

fn raw_flow_frame_effect(effect: flow_frame::Effect) -> Result<RawEffect, MobError> {
    let (variant, fields) = match effect {
        flow_frame::Effect::ReadyFrontierChanged(payload) => (
            "ReadyFrontierChanged",
            BTreeMap::from([(
                "frame_id".into(),
                KernelValue::String(payload.frame_id.to_string()),
            )]),
        ),
        flow_frame::Effect::AdmitStepWork(payload) => (
            "AdmitStepWork",
            BTreeMap::from([
                (
                    "frame_id".into(),
                    KernelValue::String(payload.frame_id.to_string()),
                ),
                (
                    "node_id".into(),
                    KernelValue::String(payload.node_id.to_string()),
                ),
            ]),
        ),
        flow_frame::Effect::StartLoopNode(payload) => (
            "StartLoopNode",
            BTreeMap::from([
                (
                    "frame_id".into(),
                    KernelValue::String(payload.frame_id.to_string()),
                ),
                (
                    "node_id".into(),
                    KernelValue::String(payload.node_id.to_string()),
                ),
            ]),
        ),
        flow_frame::Effect::PersistStepOutput(payload) => (
            "PersistStepOutput",
            BTreeMap::from([
                (
                    "frame_id".into(),
                    KernelValue::String(payload.frame_id.to_string()),
                ),
                (
                    "node_id".into(),
                    KernelValue::String(payload.node_id.to_string()),
                ),
            ]),
        ),
        flow_frame::Effect::NodeExecutionReleased(payload) => (
            "NodeExecutionReleased",
            BTreeMap::from([
                (
                    "frame_id".into(),
                    KernelValue::String(payload.frame_id.to_string()),
                ),
                (
                    "node_id".into(),
                    KernelValue::String(payload.node_id.to_string()),
                ),
            ]),
        ),
        flow_frame::Effect::RootFrameCompleted(payload) => (
            "RootFrameCompleted",
            BTreeMap::from([(
                "frame_id".into(),
                KernelValue::String(payload.frame_id.to_string()),
            )]),
        ),
        flow_frame::Effect::RootFrameFailed(payload) => (
            "RootFrameFailed",
            BTreeMap::from([(
                "frame_id".into(),
                KernelValue::String(payload.frame_id.to_string()),
            )]),
        ),
        flow_frame::Effect::RootFrameCanceled(payload) => (
            "RootFrameCanceled",
            BTreeMap::from([(
                "frame_id".into(),
                KernelValue::String(payload.frame_id.to_string()),
            )]),
        ),
        flow_frame::Effect::BodyFrameCompleted(payload) => (
            "BodyFrameCompleted",
            BTreeMap::from([
                (
                    "frame_id".into(),
                    KernelValue::String(payload.frame_id.to_string()),
                ),
                (
                    "loop_instance_id".into(),
                    KernelValue::String(payload.loop_instance_id.to_string()),
                ),
                (
                    "iteration".into(),
                    KernelValue::U64(u64::from(payload.iteration)),
                ),
            ]),
        ),
        flow_frame::Effect::BodyFrameFailed(payload) => (
            "BodyFrameFailed",
            BTreeMap::from([
                (
                    "frame_id".into(),
                    KernelValue::String(payload.frame_id.to_string()),
                ),
                (
                    "loop_instance_id".into(),
                    KernelValue::String(payload.loop_instance_id.to_string()),
                ),
                (
                    "iteration".into(),
                    KernelValue::U64(u64::from(payload.iteration)),
                ),
            ]),
        ),
        flow_frame::Effect::BodyFrameCanceled(payload) => (
            "BodyFrameCanceled",
            BTreeMap::from([
                (
                    "frame_id".into(),
                    KernelValue::String(payload.frame_id.to_string()),
                ),
                (
                    "loop_instance_id".into(),
                    KernelValue::String(payload.loop_instance_id.to_string()),
                ),
                (
                    "iteration".into(),
                    KernelValue::U64(u64::from(payload.iteration)),
                ),
            ]),
        ),
    };
    Ok(RawEffect {
        variant: variant.into(),
        fields,
    })
}

fn raw_loop_effect(effect: loop_iteration::Effect) -> Result<RawEffect, MobError> {
    let (variant, fields) = match effect {
        loop_iteration::Effect::RequestBodyFrameStart(payload) => (
            "RequestBodyFrameStart",
            BTreeMap::from([
                (
                    "loop_instance_id".into(),
                    KernelValue::String(payload.loop_instance_id.to_string()),
                ),
                ("depth".into(), KernelValue::U64(u64::from(payload.depth))),
            ]),
        ),
        loop_iteration::Effect::EvaluateUntilCondition(payload) => (
            "EvaluateUntilCondition",
            BTreeMap::from([
                (
                    "loop_instance_id".into(),
                    KernelValue::String(payload.loop_instance_id.to_string()),
                ),
                (
                    "iteration".into(),
                    KernelValue::U64(u64::from(payload.iteration)),
                ),
                (
                    "parent_frame_id".into(),
                    KernelValue::String(payload.parent_frame_id.to_string()),
                ),
                (
                    "parent_node_id".into(),
                    KernelValue::String(payload.parent_node_id.to_string()),
                ),
                (
                    "loop_id".into(),
                    KernelValue::String(payload.loop_id.to_string()),
                ),
            ]),
        ),
        loop_iteration::Effect::LoopCompleted(payload) => (
            "LoopCompleted",
            BTreeMap::from([
                (
                    "loop_instance_id".into(),
                    KernelValue::String(payload.loop_instance_id.to_string()),
                ),
                (
                    "parent_frame_id".into(),
                    KernelValue::String(payload.parent_frame_id.to_string()),
                ),
                (
                    "parent_node_id".into(),
                    KernelValue::String(payload.parent_node_id.to_string()),
                ),
            ]),
        ),
        loop_iteration::Effect::LoopExhausted(payload) => (
            "LoopExhausted",
            BTreeMap::from([
                (
                    "loop_instance_id".into(),
                    KernelValue::String(payload.loop_instance_id.to_string()),
                ),
                (
                    "parent_frame_id".into(),
                    KernelValue::String(payload.parent_frame_id.to_string()),
                ),
                (
                    "parent_node_id".into(),
                    KernelValue::String(payload.parent_node_id.to_string()),
                ),
            ]),
        ),
        loop_iteration::Effect::LoopFailed(payload) => (
            "LoopFailed",
            BTreeMap::from([
                (
                    "loop_instance_id".into(),
                    KernelValue::String(payload.loop_instance_id.to_string()),
                ),
                (
                    "parent_frame_id".into(),
                    KernelValue::String(payload.parent_frame_id.to_string()),
                ),
                (
                    "parent_node_id".into(),
                    KernelValue::String(payload.parent_node_id.to_string()),
                ),
            ]),
        ),
        loop_iteration::Effect::LoopCanceled(payload) => (
            "LoopCanceled",
            BTreeMap::from([
                (
                    "loop_instance_id".into(),
                    KernelValue::String(payload.loop_instance_id.to_string()),
                ),
                (
                    "parent_frame_id".into(),
                    KernelValue::String(payload.parent_frame_id.to_string()),
                ),
                (
                    "parent_node_id".into(),
                    KernelValue::String(payload.parent_node_id.to_string()),
                ),
            ]),
        ),
    };
    Ok(RawEffect {
        variant: variant.into(),
        fields,
    })
}

fn string_from_effect_field(effect: &RawEffect, field: &str) -> Result<String, MobError> {
    match effect.fields.get(field) {
        Some(KernelValue::String(value)) => Ok(value.clone()),
        other => Err(MobError::Internal(format!(
            "effect '{}' missing String field '{}': {other:?}",
            effect.variant, field
        ))),
    }
}

fn u64_from_effect_field(effect: &RawEffect, field: &str) -> Result<u64, MobError> {
    match effect.fields.get(field) {
        Some(KernelValue::U64(value)) => Ok(*value),
        other => Err(MobError::Internal(format!(
            "effect '{}' missing U64 field '{}': {other:?}",
            effect.variant, field
        ))),
    }
}

fn frame_is_body(state: &flow_frame::State) -> bool {
    state.frame_scope == kernel_types::FrameScope::Body
}

fn frame_loop_instance_id(state: &flow_frame::State) -> Option<LoopInstanceId> {
    if state.loop_instance_id.as_str().is_empty() {
        None
    } else {
        Some(local_loop_instance_id(&state.loop_instance_id))
    }
}

fn frame_iteration(state: &flow_frame::State) -> Result<u64, MobError> {
    Ok(u64::from(state.iteration))
}

fn loop_current_iteration(state: &loop_iteration::State) -> Result<u64, MobError> {
    Ok(u64::from(state.current_iteration))
}

fn loop_instance_id_from_state(state: &loop_iteration::State) -> Result<LoopInstanceId, MobError> {
    Ok(local_loop_instance_id(&state.loop_instance_id))
}

fn loop_parent_frame_id(state: &loop_iteration::State) -> Result<FrameId, MobError> {
    Ok(local_frame_id(&state.parent_frame_id))
}

fn loop_parent_node_id(state: &loop_iteration::State) -> Result<FlowNodeId, MobError> {
    Ok(local_flow_node_id(&state.parent_node_id))
}

fn loop_id_from_state(state: &loop_iteration::State) -> Result<LoopId, MobError> {
    Ok(local_loop_id(&state.loop_id))
}

fn loop_stage(state: &loop_iteration::State) -> Result<&'static str, MobError> {
    Ok(match state.stage {
        kernel_types::LoopIterationStage::AwaitingBodyFrame => "AwaitingBodyFrame",
        kernel_types::LoopIterationStage::BodyFrameActive => "BodyFrameActive",
        kernel_types::LoopIterationStage::AwaitingUntil => "AwaitingUntil",
    })
}

fn active_body_frame_id(state: &loop_iteration::State) -> Option<FrameId> {
    state.active_body_frame_id.as_ref().map(local_frame_id)
}

fn state_is_awaiting_body_frame(state: &loop_iteration::State) -> bool {
    state.stage == kernel_types::LoopIterationStage::AwaitingBodyFrame
}

fn loop_terminal_effect(phase: &loop_iteration::Phase) -> Option<&'static str> {
    match phase {
        loop_iteration::Phase::Completed => Some("LoopCompleted"),
        loop_iteration::Phase::Exhausted => Some("LoopExhausted"),
        loop_iteration::Phase::Failed => Some("LoopFailed"),
        loop_iteration::Phase::Canceled => Some("LoopCanceled"),
        _ => None,
    }
}

fn terminal_phase(state: &flow_frame::State) -> Result<FlowFrameTerminalPhase, MobError> {
    match state.phase {
        flow_frame::Phase::Completed => Ok(FlowFrameTerminalPhase::Completed),
        flow_frame::Phase::Failed => Ok(FlowFrameTerminalPhase::Failed),
        flow_frame::Phase::Canceled => Ok(FlowFrameTerminalPhase::Canceled),
        other => Err(MobError::Internal(format!(
            "frame is not in a terminal phase: '{other:?}'"
        ))),
    }
}

//! MobMachine-owned loop-iteration projection reducer.
//!
//! This module is intentionally not exported through generated kernel modules.
//! It is the runtime projection shape used by `MobRun` while the canonical
//! semantic owner remains `machines::mob_machine`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::implicit_clone,
    clippy::unnecessary_cast,
    clippy::redundant_clone
)]

pub use crate::ids::{FlowNodeId, FrameId, LoopId, LoopInstanceId};

#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LoopIterationStage {
    AwaitingBodyFrame,
    BodyFrameActive,
    AwaitingUntil,
}
impl LoopIterationStage {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AwaitingBodyFrame => "AwaitingBodyFrame",
            Self::BodyFrameActive => "BodyFrameActive",
            Self::AwaitingUntil => "AwaitingUntil",
        }
    }
}
impl std::fmt::Display for LoopIterationStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

pub trait Context {}
pub struct EmptyContext;
impl Context for EmptyContext {}

#[allow(non_camel_case_types)]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Phase {
    Absent,
    Running,
    Completed,
    Exhausted,
    Failed,
    Canceled,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct State {
    pub phase: Phase,
    pub loop_instance_id: LoopInstanceId,
    pub parent_frame_id: FrameId,
    pub parent_node_id: FlowNodeId,
    pub loop_id: LoopId,
    pub depth: u32,
    pub stage: LoopIterationStage,
    pub current_iteration: u32,
    pub last_completed_iteration: u32,
    pub max_iterations: u32,
    pub active_body_frame_id: Option<FrameId>,
}
impl Default for State {
    fn default() -> Self {
        initial_state()
    }
}

pub mod inputs {
    use super::*;
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct StartLoop {
        pub loop_instance_id: LoopInstanceId,
        pub max_iterations: u32,
        pub parent_frame_id: FrameId,
        pub parent_node_id: FlowNodeId,
        pub loop_id: LoopId,
        pub depth: u32,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct BodyFrameStarted {
        pub loop_instance_id: LoopInstanceId,
        pub frame_id: FrameId,
        pub iteration: u32,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct BodyFrameCompleted {
        pub loop_instance_id: LoopInstanceId,
        pub iteration: u32,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct BodyFrameFailed {
        pub loop_instance_id: LoopInstanceId,
        pub iteration: u32,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct BodyFrameCanceled {
        pub loop_instance_id: LoopInstanceId,
        pub iteration: u32,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct UntilConditionMet {
        pub loop_instance_id: LoopInstanceId,
        pub iteration: u32,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct UntilConditionFailed {
        pub loop_instance_id: LoopInstanceId,
        pub iteration: u32,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct CancelLoop {
        pub loop_instance_id: LoopInstanceId,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Input {
    StartLoop(inputs::StartLoop),
    BodyFrameStarted(inputs::BodyFrameStarted),
    BodyFrameCompleted(inputs::BodyFrameCompleted),
    BodyFrameFailed(inputs::BodyFrameFailed),
    BodyFrameCanceled(inputs::BodyFrameCanceled),
    UntilConditionMet(inputs::UntilConditionMet),
    UntilConditionFailed(inputs::UntilConditionFailed),
    CancelLoop(inputs::CancelLoop),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum InputKind {
    StartLoop,
    BodyFrameStarted,
    BodyFrameCompleted,
    BodyFrameFailed,
    BodyFrameCanceled,
    UntilConditionMet,
    UntilConditionFailed,
    CancelLoop,
}

pub mod effects {
    use super::*;
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct RequestBodyFrameStart {
        pub loop_instance_id: LoopInstanceId,
        pub depth: u32,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct EvaluateUntilCondition {
        pub loop_instance_id: LoopInstanceId,
        pub iteration: u32,
        pub parent_frame_id: FrameId,
        pub parent_node_id: FlowNodeId,
        pub loop_id: LoopId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct LoopCompleted {
        pub loop_instance_id: LoopInstanceId,
        pub parent_frame_id: FrameId,
        pub parent_node_id: FlowNodeId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct LoopExhausted {
        pub loop_instance_id: LoopInstanceId,
        pub parent_frame_id: FrameId,
        pub parent_node_id: FlowNodeId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct LoopFailed {
        pub loop_instance_id: LoopInstanceId,
        pub parent_frame_id: FrameId,
        pub parent_node_id: FlowNodeId,
    }
    #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct LoopCanceled {
        pub loop_instance_id: LoopInstanceId,
        pub parent_frame_id: FrameId,
        pub parent_node_id: FlowNodeId,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Effect {
    RequestBodyFrameStart(effects::RequestBodyFrameStart),
    EvaluateUntilCondition(effects::EvaluateUntilCondition),
    LoopCompleted(effects::LoopCompleted),
    LoopExhausted(effects::LoopExhausted),
    LoopFailed(effects::LoopFailed),
    LoopCanceled(effects::LoopCanceled),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EffectKind {
    RequestBodyFrameStart,
    EvaluateUntilCondition,
    LoopCompleted,
    LoopExhausted,
    LoopFailed,
    LoopCanceled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TransitionId {
    StartLoop,
    BodyFrameStarted,
    BodyFrameCompleted,
    BodyFrameFailed,
    BodyFrameCanceled,
    UntilConditionMet,
    UntilConditionFailed,
    CancelLoop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GuardId {
    Phase,
    LoopInstanceId,
    Stage,
    Iteration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum HelperId {
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Outcome {
    pub transition_id: TransitionId,
    pub next_state: State,
    pub effects: Vec<Effect>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TransitionError {
    Refusal(TransitionRefusal),
    Kernel(KernelError),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TransitionRefusal {
    NoMatchingTransition {
        phase: Phase,
        trigger: TriggerDiscriminant,
    },
    GuardRejected {
        rejections: Vec<GuardRejection>,
    },
    AmbiguousTransition {
        transitions: Vec<TransitionId>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TriggerDiscriminant {
    Input(InputKind),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GuardRejection {
    pub transition_id: TransitionId,
    pub guard_id: GuardId,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum KernelError {
    ContextViolation {
        transition_id: TransitionId,
        detail: String,
    },
    HelperEvaluation {
        helper_id: HelperId,
        detail: String,
    },
    CodegenInvariant {
        detail: String,
    },
}

pub mod helpers {
    use super::*;
    pub fn none<C: Context>(_: &State, context: &C) -> Result<(), KernelError> {
        let _ = context;
        Ok(())
    }
}

pub fn initial_state() -> State {
    State {
        phase: Phase::Absent,
        loop_instance_id: LoopInstanceId::from(String::new()),
        parent_frame_id: FrameId::from(String::new()),
        parent_node_id: FlowNodeId::from(String::new()),
        loop_id: LoopId::from(String::new()),
        depth: 0,
        stage: LoopIterationStage::AwaitingBodyFrame,
        current_iteration: 0,
        last_completed_iteration: 0,
        max_iterations: 0,
        active_body_frame_id: None,
    }
}

fn guard(transition_id: TransitionId, guard_id: GuardId) -> TransitionError {
    TransitionError::Refusal(TransitionRefusal::GuardRejected {
        rejections: vec![GuardRejection {
            transition_id,
            guard_id,
        }],
    })
}

pub fn transition<C: Context>(
    state: &State,
    input: Input,
    context: &C,
) -> Result<Outcome, TransitionError> {
    let _ = context;
    match input {
        Input::StartLoop(payload) => {
            if state.phase != Phase::Absent {
                return Err(TransitionError::Refusal(
                    TransitionRefusal::NoMatchingTransition {
                        phase: state.phase.clone(),
                        trigger: TriggerDiscriminant::Input(InputKind::StartLoop),
                    },
                ));
            }
            let next_state = State {
                phase: Phase::Running,
                loop_instance_id: payload.loop_instance_id.clone(),
                parent_frame_id: payload.parent_frame_id.clone(),
                parent_node_id: payload.parent_node_id.clone(),
                loop_id: payload.loop_id.clone(),
                depth: payload.depth,
                stage: LoopIterationStage::AwaitingBodyFrame,
                current_iteration: 0,
                last_completed_iteration: 0,
                max_iterations: payload.max_iterations,
                active_body_frame_id: None,
            };
            Ok(Outcome {
                transition_id: TransitionId::StartLoop,
                effects: vec![Effect::RequestBodyFrameStart(
                    effects::RequestBodyFrameStart {
                        loop_instance_id: payload.loop_instance_id,
                        depth: payload.depth,
                    },
                )],
                next_state,
            })
        }
        Input::BodyFrameStarted(payload) => {
            if state.phase != Phase::Running {
                return Err(TransitionError::Refusal(
                    TransitionRefusal::NoMatchingTransition {
                        phase: state.phase.clone(),
                        trigger: TriggerDiscriminant::Input(InputKind::BodyFrameStarted),
                    },
                ));
            }
            if state.loop_instance_id != payload.loop_instance_id {
                return Err(guard(
                    TransitionId::BodyFrameStarted,
                    GuardId::LoopInstanceId,
                ));
            }
            if state.stage != LoopIterationStage::AwaitingBodyFrame {
                return Err(guard(TransitionId::BodyFrameStarted, GuardId::Stage));
            }
            if state.current_iteration != payload.iteration {
                return Err(guard(TransitionId::BodyFrameStarted, GuardId::Iteration));
            }
            let mut next_state = state.clone();
            next_state.stage = LoopIterationStage::BodyFrameActive;
            next_state.active_body_frame_id = Some(payload.frame_id);
            Ok(Outcome {
                transition_id: TransitionId::BodyFrameStarted,
                next_state,
                effects: Vec::new(),
            })
        }
        Input::BodyFrameCompleted(payload) => {
            if state.phase != Phase::Running {
                return Err(TransitionError::Refusal(
                    TransitionRefusal::NoMatchingTransition {
                        phase: state.phase.clone(),
                        trigger: TriggerDiscriminant::Input(InputKind::BodyFrameCompleted),
                    },
                ));
            }
            if state.loop_instance_id != payload.loop_instance_id {
                return Err(guard(
                    TransitionId::BodyFrameCompleted,
                    GuardId::LoopInstanceId,
                ));
            }
            if state.stage != LoopIterationStage::BodyFrameActive {
                return Err(guard(TransitionId::BodyFrameCompleted, GuardId::Stage));
            }
            if state.current_iteration != payload.iteration {
                return Err(guard(TransitionId::BodyFrameCompleted, GuardId::Iteration));
            }
            let mut next_state = state.clone();
            next_state.stage = LoopIterationStage::AwaitingUntil;
            next_state.last_completed_iteration = payload.iteration;
            next_state.current_iteration = state.current_iteration + 1;
            next_state.active_body_frame_id = None;
            Ok(Outcome {
                transition_id: TransitionId::BodyFrameCompleted,
                effects: vec![Effect::EvaluateUntilCondition(
                    effects::EvaluateUntilCondition {
                        loop_instance_id: state.loop_instance_id.clone(),
                        iteration: payload.iteration,
                        parent_frame_id: state.parent_frame_id.clone(),
                        parent_node_id: state.parent_node_id.clone(),
                        loop_id: state.loop_id.clone(),
                    },
                )],
                next_state,
            })
        }
        Input::BodyFrameFailed(payload) => {
            if state.phase != Phase::Running {
                return Err(TransitionError::Refusal(
                    TransitionRefusal::NoMatchingTransition {
                        phase: state.phase.clone(),
                        trigger: TriggerDiscriminant::Input(InputKind::BodyFrameFailed),
                    },
                ));
            }
            if state.loop_instance_id != payload.loop_instance_id {
                return Err(guard(
                    TransitionId::BodyFrameFailed,
                    GuardId::LoopInstanceId,
                ));
            }
            if state.stage != LoopIterationStage::BodyFrameActive {
                return Err(guard(TransitionId::BodyFrameFailed, GuardId::Stage));
            }
            if state.current_iteration != payload.iteration {
                return Err(guard(TransitionId::BodyFrameFailed, GuardId::Iteration));
            }
            let mut next_state = state.clone();
            next_state.phase = Phase::Failed;
            next_state.last_completed_iteration = payload.iteration;
            next_state.active_body_frame_id = None;
            Ok(Outcome {
                transition_id: TransitionId::BodyFrameFailed,
                effects: vec![Effect::LoopFailed(effects::LoopFailed {
                    loop_instance_id: state.loop_instance_id.clone(),
                    parent_frame_id: state.parent_frame_id.clone(),
                    parent_node_id: state.parent_node_id.clone(),
                })],
                next_state,
            })
        }
        Input::BodyFrameCanceled(payload) => {
            if state.phase != Phase::Running {
                return Err(TransitionError::Refusal(
                    TransitionRefusal::NoMatchingTransition {
                        phase: state.phase.clone(),
                        trigger: TriggerDiscriminant::Input(InputKind::BodyFrameCanceled),
                    },
                ));
            }
            if state.loop_instance_id != payload.loop_instance_id {
                return Err(guard(
                    TransitionId::BodyFrameCanceled,
                    GuardId::LoopInstanceId,
                ));
            }
            if state.stage != LoopIterationStage::BodyFrameActive {
                return Err(guard(TransitionId::BodyFrameCanceled, GuardId::Stage));
            }
            if state.current_iteration != payload.iteration {
                return Err(guard(TransitionId::BodyFrameCanceled, GuardId::Iteration));
            }
            let mut next_state = state.clone();
            next_state.phase = Phase::Canceled;
            next_state.last_completed_iteration = payload.iteration;
            next_state.active_body_frame_id = None;
            Ok(Outcome {
                transition_id: TransitionId::BodyFrameCanceled,
                effects: vec![Effect::LoopCanceled(effects::LoopCanceled {
                    loop_instance_id: state.loop_instance_id.clone(),
                    parent_frame_id: state.parent_frame_id.clone(),
                    parent_node_id: state.parent_node_id.clone(),
                })],
                next_state,
            })
        }
        Input::UntilConditionMet(payload) => {
            if state.phase != Phase::Running {
                return Err(TransitionError::Refusal(
                    TransitionRefusal::NoMatchingTransition {
                        phase: state.phase.clone(),
                        trigger: TriggerDiscriminant::Input(InputKind::UntilConditionMet),
                    },
                ));
            }
            if state.loop_instance_id != payload.loop_instance_id {
                return Err(guard(
                    TransitionId::UntilConditionMet,
                    GuardId::LoopInstanceId,
                ));
            }
            if state.stage != LoopIterationStage::AwaitingUntil {
                return Err(guard(TransitionId::UntilConditionMet, GuardId::Stage));
            }
            if state.last_completed_iteration != payload.iteration {
                return Err(guard(TransitionId::UntilConditionMet, GuardId::Iteration));
            }
            let mut next_state = state.clone();
            next_state.phase = Phase::Completed;
            Ok(Outcome {
                transition_id: TransitionId::UntilConditionMet,
                effects: vec![Effect::LoopCompleted(effects::LoopCompleted {
                    loop_instance_id: state.loop_instance_id.clone(),
                    parent_frame_id: state.parent_frame_id.clone(),
                    parent_node_id: state.parent_node_id.clone(),
                })],
                next_state,
            })
        }
        Input::UntilConditionFailed(payload) => {
            if state.phase != Phase::Running {
                return Err(TransitionError::Refusal(
                    TransitionRefusal::NoMatchingTransition {
                        phase: state.phase.clone(),
                        trigger: TriggerDiscriminant::Input(InputKind::UntilConditionFailed),
                    },
                ));
            }
            if state.loop_instance_id != payload.loop_instance_id {
                return Err(guard(
                    TransitionId::UntilConditionFailed,
                    GuardId::LoopInstanceId,
                ));
            }
            if state.stage != LoopIterationStage::AwaitingUntil {
                return Err(guard(TransitionId::UntilConditionFailed, GuardId::Stage));
            }
            if state.last_completed_iteration != payload.iteration {
                return Err(guard(
                    TransitionId::UntilConditionFailed,
                    GuardId::Iteration,
                ));
            }
            let mut next_state = state.clone();
            let effects = if state.current_iteration >= state.max_iterations {
                next_state.phase = Phase::Exhausted;
                vec![Effect::LoopExhausted(effects::LoopExhausted {
                    loop_instance_id: state.loop_instance_id.clone(),
                    parent_frame_id: state.parent_frame_id.clone(),
                    parent_node_id: state.parent_node_id.clone(),
                })]
            } else {
                next_state.stage = LoopIterationStage::AwaitingBodyFrame;
                vec![Effect::RequestBodyFrameStart(
                    effects::RequestBodyFrameStart {
                        loop_instance_id: state.loop_instance_id.clone(),
                        depth: state.depth,
                    },
                )]
            };
            Ok(Outcome {
                transition_id: TransitionId::UntilConditionFailed,
                next_state,
                effects,
            })
        }
        Input::CancelLoop(payload) => {
            if state.phase != Phase::Running {
                return Err(TransitionError::Refusal(
                    TransitionRefusal::NoMatchingTransition {
                        phase: state.phase.clone(),
                        trigger: TriggerDiscriminant::Input(InputKind::CancelLoop),
                    },
                ));
            }
            if state.loop_instance_id != payload.loop_instance_id {
                return Err(guard(TransitionId::CancelLoop, GuardId::LoopInstanceId));
            }
            let mut next_state = state.clone();
            next_state.phase = Phase::Canceled;
            next_state.active_body_frame_id = None;
            Ok(Outcome {
                transition_id: TransitionId::CancelLoop,
                effects: vec![Effect::LoopCanceled(effects::LoopCanceled {
                    loop_instance_id: state.loop_instance_id.clone(),
                    parent_frame_id: state.parent_frame_id.clone(),
                    parent_node_id: state.parent_node_id.clone(),
                })],
                next_state,
            })
        }
    }
}

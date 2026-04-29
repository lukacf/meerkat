//! FlowFrameEngine: drives a frame-backed flow through MobMachine-authorized
//! frame scheduling, execution, and terminalization mechanics.
//!
//! This executor handles:
//! - Step nodes: delegates async work to `FrameStepExecutor` and projects the
//!   typed outcome back through machine-owned transitions.
//! - Loop nodes: routes loop lifecycle and body-frame starts through the
//!   run/frame/loop kernels rather than a recursive shell executor.

use crate::definition::{FlowNodeSpec, FrameSpec};
use crate::error::MobError;
use crate::ids::{FlowNodeId, FrameId, LoopId, LoopInstanceId, RunId, StepId};
use crate::machines::mob_machine as mob_dsl;
use crate::run::{
    FlowContext, FrameSnapshot, LoopIterationLedgerEntry, LoopSnapshot,
    MobMachineFlowAuthorityToken, MobMachineFlowFrameCommand, MobMachineFlowRunCommand,
    MobMachineLoopIterationCommand, MobRun, StepRunStatus, apply_mob_machine_flow_frame_command,
    apply_mob_machine_flow_run_command, apply_mob_machine_loop_iteration_command,
};
use crate::run::{flow_frame, flow_run, loop_iteration};
use crate::runtime::MobHandle;
use crate::runtime::conditions::evaluate_condition;
use crate::store::MobRunStore;
#[cfg(target_arch = "wasm32")]
use crate::tokio::time as tokio_time;
use async_trait::async_trait;
use futures::stream::{FuturesUnordered, StreamExt};
use indexmap::IndexMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use tokio::time as tokio_time;

// ─── FrameStepResult ─────────────────────────────────────────────────────────

/// Result of executing a single step node within a frame.
pub enum FrameStepResult {
    /// Step executed successfully and produced output.
    Completed(serde_json::Value),
    /// Step was skipped (condition evaluated to false).
    Skipped,
    /// Step failed for a step-local reason; the frame should fail this node
    /// but continue admitting unrelated siblings.
    Failed {
        reason: String,
        failure_ledger_recorded: bool,
    },
}

/// Canonical outcome of executing a frame subtree.
#[derive(Debug)]
pub struct FrameExecutionOutcome {
    pub outputs: IndexMap<StepId, serde_json::Value>,
    pub step_projections: IndexMap<StepId, FrameStepProjection>,
    pub step_failures: IndexMap<StepId, FrameStepFailureProjection>,
}

#[derive(Debug, Clone)]
pub struct FrameStepProjection {
    pub frame_id: FrameId,
    pub node_id: FlowNodeId,
    pub step_id: StepId,
    pub step_status: StepRunStatus,
}

#[derive(Debug, Clone)]
pub struct FrameStepFailureProjection {
    pub reason: String,
    pub append_failure_ledger: bool,
}

#[derive(Debug, Clone)]
struct PreviewedNodeGrant {
    frame_id: FrameId,
    next_run_state: flow_run::State,
    machine_input: mob_dsl::MobMachineInput,
}

#[derive(Debug, Clone)]
struct PreviewedBodyFrameGrant {
    loop_instance_id: LoopInstanceId,
    next_run_state: flow_run::State,
    machine_input: mob_dsl::MobMachineInput,
}

#[derive(Debug, Clone, Copy)]
struct FrameDepthLimit {
    current_depth: u32,
    max_depth: u32,
}

#[derive(Debug, Clone, Copy)]
struct FrameSeedConfirmation<'a> {
    frame_scope: crate::machines::mob_machine::FrameScope,
    loop_instance_id: Option<&'a LoopInstanceId>,
    iteration: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowFrameTerminalPhase {
    Completed,
    Failed,
    Canceled,
}

#[derive(Debug, Clone)]
pub enum FlowFrameLoopStorePlan {
    InsertFrame {
        machine_inputs: Vec<mob_dsl::MobMachineInput>,
        frame_id: FrameId,
        initial_frame: FrameSnapshot,
    },
    FrameState {
        machine_inputs: Vec<mob_dsl::MobMachineInput>,
        frame_id: FrameId,
        expected_frame: FrameSnapshot,
        next_frame: FrameSnapshot,
    },
    CompleteStepAndRecordOutput {
        machine_inputs: Vec<mob_dsl::MobMachineInput>,
        frame_id: FrameId,
        expected_frame: FrameSnapshot,
        next_frame: FrameSnapshot,
        step_output_key: String,
        step_output: serde_json::Value,
        loop_context: Option<(LoopId, u64)>,
    },
    GrantNodeSlot {
        machine_inputs: Vec<mob_dsl::MobMachineInput>,
        expected_run_state: flow_run::State,
        next_run_state: flow_run::State,
        frame_id: FrameId,
        expected_frame: FrameSnapshot,
        next_frame: FrameSnapshot,
    },
    StartLoop {
        machine_inputs: Vec<mob_dsl::MobMachineInput>,
        loop_instance_id: LoopInstanceId,
        expected_run_state: flow_run::State,
        next_run_state: flow_run::State,
        frame_id: FrameId,
        expected_frame: FrameSnapshot,
        next_frame: FrameSnapshot,
        initial_loop: LoopSnapshot,
    },
    GrantBodyFrameStart {
        machine_inputs: Vec<mob_dsl::MobMachineInput>,
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
        machine_inputs: Vec<mob_dsl::MobMachineInput>,
        expected_run_state: flow_run::State,
        next_run_state: flow_run::State,
    },
    SealFrame {
        machine_inputs: Vec<mob_dsl::MobMachineInput>,
        frame_id: FrameId,
        expected_frame: FrameSnapshot,
        next_frame: FrameSnapshot,
    },
    CompleteBodyFrame {
        machine_inputs: Vec<mob_dsl::MobMachineInput>,
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
        machine_inputs: Vec<mob_dsl::MobMachineInput>,
        loop_instance_id: LoopInstanceId,
        expected_loop: LoopSnapshot,
        next_loop: LoopSnapshot,
        expected_run_state: flow_run::State,
        next_run_state: flow_run::State,
    },
    CompleteLoop {
        machine_inputs: Vec<mob_dsl::MobMachineInput>,
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

impl FlowFrameLoopStorePlan {
    pub(super) fn machine_inputs(&self) -> &[mob_dsl::MobMachineInput] {
        match self {
            Self::InsertFrame { machine_inputs, .. }
            | Self::FrameState { machine_inputs, .. }
            | Self::CompleteStepAndRecordOutput { machine_inputs, .. }
            | Self::GrantNodeSlot { machine_inputs, .. }
            | Self::StartLoop { machine_inputs, .. }
            | Self::GrantBodyFrameStart { machine_inputs, .. }
            | Self::RunStateOnly { machine_inputs, .. }
            | Self::SealFrame { machine_inputs, .. }
            | Self::CompleteBodyFrame { machine_inputs, .. }
            | Self::LoopRequestBodyFrame { machine_inputs, .. }
            | Self::CompleteLoop { machine_inputs, .. } => machine_inputs,
        }
    }
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowLoopUntilEvaluationObligation {
    pub loop_instance_id: LoopInstanceId,
    pub iteration: u32,
    pub parent_frame_id: FrameId,
    pub parent_node_id: FlowNodeId,
    pub loop_id: LoopId,
}

#[derive(Debug, Clone)]
struct LoopUntilConditionFeedbackProjection {
    loop_instance_id: LoopInstanceId,
    iteration: u32,
    until_met: bool,
}

#[derive(Debug, Clone, Default)]
pub struct FlowFrameLoopDecision {
    pub store_plan: Option<FlowFrameLoopStorePlan>,
    pub follow_up: Vec<FlowFrameLoopWork>,
}

enum FrameNodeTaskResult {
    Step {
        frame_id: FrameId,
        node_id: FlowNodeId,
        step_id: StepId,
        loop_context: Option<(LoopId, u64)>,
        result: Result<FrameStepResult, MobError>,
    },
}

#[cfg(not(target_arch = "wasm32"))]
type FrameNodeTaskFuture = Pin<Box<dyn Future<Output = FrameNodeTaskResult> + Send>>;
#[cfg(target_arch = "wasm32")]
type FrameNodeTaskFuture = Pin<Box<dyn Future<Output = FrameNodeTaskResult>>>;
type FrameNodeWorkers = FuturesUnordered<FrameNodeTaskFuture>;

#[cfg(not(target_arch = "wasm32"))]
type FrameExecutionFuture<'a> =
    Pin<Box<dyn Future<Output = Result<FrameExecutionOutcome, MobError>> + Send + 'a>>;
#[cfg(target_arch = "wasm32")]
type FrameExecutionFuture<'a> =
    Pin<Box<dyn Future<Output = Result<FrameExecutionOutcome, MobError>> + 'a>>;

struct SpawnStepTask {
    frame_id: FrameId,
    node_id: FlowNodeId,
    step_id: StepId,
    loop_context: Option<(LoopId, u64)>,
    context: FlowContext,
}

pub struct StepCompletionOpts<'a> {
    pub node_id: &'a FlowNodeId,
    pub step_id: &'a StepId,
    pub output: serde_json::Value,
    pub loop_context: Option<(&'a LoopId, u64)>,
    pub max_retries: usize,
}

// ─── FrameStepExecutor ───────────────────────────────────────────────────────

/// Trait for executing a single step node within a frame.
///
/// Implementors provide the actual work (e.g., LLM turn, mock scripted output).
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait FrameStepExecutor: Send + Sync {
    async fn check_runtime_guards(&self, _run_id: &RunId) -> Result<(), MobError> {
        Ok(())
    }

    async fn execute_step(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        node_id: &FlowNodeId,
        step_id: &StepId,
        context: &FlowContext,
    ) -> Result<FrameStepResult, MobError>;
}

mod sealed {
    pub trait Sealed {}
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait FlowFrameMutator: sealed::Sealed {
    async fn start_frame(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        spec: &FrameSpec,
    ) -> Result<FrameSnapshot, MobError>;

    async fn admit_next_ready_node(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
    ) -> Result<Option<Vec<flow_frame::Effect>>, MobError>;

    async fn admit_next_ready_node_with_retry(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        max_retries: usize,
    ) -> Result<Option<Vec<flow_frame::Effect>>, MobError>;

    async fn complete_step(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        opts: StepCompletionOpts<'_>,
    ) -> Result<(), MobError>;

    async fn complete_node(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        node_id: &FlowNodeId,
    ) -> Result<bool, MobError>;

    async fn fail_node(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        node_id: &FlowNodeId,
    ) -> Result<bool, MobError>;

    async fn skip_node(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        node_id: &FlowNodeId,
    ) -> Result<bool, MobError>;

    async fn cancel_node(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        node_id: &FlowNodeId,
    ) -> Result<bool, MobError>;

    async fn terminalize_frame(&self, run_id: &RunId, frame_id: &FrameId)
    -> Result<bool, MobError>;
}

pub struct FlowFrameKernel {
    run_store: Arc<dyn MobRunStore>,
    projector: MobHandle,
}

impl FlowFrameKernel {
    pub fn new(run_store: Arc<dyn MobRunStore>, projector: MobHandle) -> Self {
        Self {
            run_store,
            projector,
        }
    }

    async fn project_mob_machine_input(
        &self,
        input: mob_dsl::MobMachineInput,
    ) -> Result<mob_dsl::MobMachineState, MobError> {
        self.projector.project_machine_input(input).await
    }

    async fn preview_mob_machine_input(
        &self,
        input: mob_dsl::MobMachineInput,
    ) -> Result<mob_dsl::MobMachineState, MobError> {
        self.projector.preview_machine_input(input).await
    }

    async fn preview_mob_machine_inputs(
        &self,
        inputs: &[mob_dsl::MobMachineInput],
    ) -> Result<mob_dsl::MobMachineState, MobError> {
        let mut authority =
            mob_dsl::MobMachineAuthority::from_state(self.current_mob_machine_state().await?);
        for input in inputs {
            let transition = mob_dsl::MobMachineMutator::apply(&mut authority, input.clone())
                .map_err(|error| {
                    MobError::Internal(format!(
                        "MobMachine preview rejected staged input {input:?}: {error}"
                    ))
                })?;
            if transition.from_phase != transition.to_phase {
                authority.state.lifecycle_phase = transition.to_phase;
            }
        }
        Ok(authority.state)
    }

    async fn current_mob_machine_state(&self) -> Result<mob_dsl::MobMachineState, MobError> {
        self.projector.query_machine_state().await
    }

    async fn authorize_frame_command(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        command: &MobMachineFlowFrameCommand,
    ) -> Result<(MobMachineFlowAuthorityToken, mob_dsl::MobMachineState), MobError> {
        let authority_input = self
            .authority_input_for_frame_command(run_id, frame_id, command)
            .await?;
        let machine_state = self
            .preview_mob_machine_input(authority_input.clone())
            .await?;
        validate_accepted_frame_command_witness(&authority_input, &machine_state)?;
        let authority =
            MobMachineFlowAuthorityToken::from_accepted_mob_machine_input(&authority_input)?;
        Ok((authority, machine_state))
    }

    async fn confirm_frame_seed_from_snapshot(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        snapshot: &FrameSnapshot,
        spec: &FrameSpec,
    ) -> Result<(), MobError> {
        let iteration = u32::try_from(frame_iteration(&snapshot.kernel_state)?).map_err(|_| {
            MobError::Internal(format!(
                "frame '{frame_id}' iteration exceeds u32 during seed confirmation"
            ))
        })?;
        let loop_instance_id = frame_loop_instance_id(&snapshot.kernel_state);
        self.confirm_frame_seed(
            run_id,
            frame_id,
            FrameSeedConfirmation {
                frame_scope: machine_frame_scope(snapshot.kernel_state.frame_scope),
                loop_instance_id: loop_instance_id.as_ref(),
                iteration,
            },
            spec,
            Some(&snapshot.kernel_state),
        )
        .await
    }

    async fn confirm_frame_seed(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        seed: FrameSeedConfirmation<'_>,
        spec: &FrameSpec,
        existing: Option<&flow_frame::State>,
    ) -> Result<(), MobError> {
        let ordered = topological_order(spec)?;
        let seed_input = crate::run::MobRun::create_frame_seed_input(
            run_id,
            frame_id,
            seed.loop_instance_id,
            seed.iteration,
            seed.frame_scope,
            spec,
            &ordered,
        )?;
        if let Some(existing) = existing {
            let current_machine_state = self.current_mob_machine_state().await?;
            if current_machine_state
                .frame_phase
                .contains_key(&mob_dsl::FrameId::from(frame_id.as_str()))
            {
                validate_seed_confirmation_matches_snapshot(
                    frame_id,
                    existing,
                    &current_machine_state,
                )?;
                return Ok(());
            }
        }
        match self.project_mob_machine_input(seed_input).await {
            Ok(machine_state) => {
                if let Some(existing) = existing {
                    validate_seed_confirmation_matches_snapshot(
                        frame_id,
                        existing,
                        &machine_state,
                    )?;
                }
                Ok(())
            }
            Err(error) if mob_machine_seed_already_confirmed(&error) => Ok(()),
            Err(error) => Err(error),
        }
    }

    async fn authority_input_for_frame_command(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        command: &MobMachineFlowFrameCommand,
    ) -> Result<mob_dsl::MobMachineInput, MobError> {
        let _ = run_id;
        Ok(command.authority_input(frame_id))
    }

    async fn require_frame(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
    ) -> Result<FrameSnapshot, MobError> {
        let run = self
            .run_store
            .get_run(run_id)
            .await?
            .ok_or_else(|| MobError::RunNotFound(run_id.clone()))?;
        run.frames.get(frame_id).cloned().ok_or_else(|| {
            MobError::Internal(format!("frame '{frame_id}' not found in run '{run_id}'"))
        })
    }

    async fn transition_frame(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        command: MobMachineFlowFrameCommand,
        max_retries: usize,
    ) -> Result<Vec<flow_frame::Effect>, MobError> {
        for _ in 0..=max_retries {
            let current = self.require_frame(run_id, frame_id).await?;
            let authority_input = command.authority_input(frame_id);
            let (authority, machine_state) = self
                .authorize_frame_command(run_id, frame_id, &command)
                .await?;
            let outcome = apply_mob_machine_flow_frame_command(
                &current.kernel_state,
                &machine_state,
                command.clone(),
                authority,
            )?;
            let next = FrameSnapshot {
                kernel_state: outcome.next_state,
            };
            let effects = outcome.effects.clone();
            let won = self
                .projector
                .commit_flow_frame_store_plan(
                    run_id,
                    FlowFrameLoopStorePlan::FrameState {
                        machine_inputs: vec![authority_input],
                        frame_id: frame_id.clone(),
                        expected_frame: current,
                        next_frame: next,
                    },
                )
                .await?;
            if won {
                return Ok(effects);
            }
        }
        Err(MobError::Internal(format!(
            "transition_frame: CAS exhausted {max_retries} retries for frame '{frame_id}'"
        )))
    }
}

impl sealed::Sealed for FlowFrameKernel {}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl FlowFrameMutator for FlowFrameKernel {
    async fn start_frame(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        spec: &FrameSpec,
    ) -> Result<FrameSnapshot, MobError> {
        let run = self
            .run_store
            .get_run(run_id)
            .await?
            .ok_or_else(|| MobError::RunNotFound(run_id.clone()))?;
        if let Some(existing) = run.frames.get(frame_id) {
            self.confirm_frame_seed_from_snapshot(run_id, frame_id, existing, spec)
                .await?;
            return Ok(existing.clone());
        }

        let initial = flow_frame::initial_state();
        let ordered = topological_order(spec)?;
        let start_input = MobMachineFlowFrameCommand::StartRootFrame(build_start_root_frame_input(
            frame_id, spec, &ordered,
        ));
        let seed_input = crate::run::MobRun::create_frame_seed_input(
            run_id,
            frame_id,
            None,
            0,
            crate::machines::mob_machine::FrameScope::Root,
            spec,
            &ordered,
        )?;
        let machine_state = self.preview_mob_machine_input(seed_input.clone()).await?;
        let authority = MobMachineFlowAuthorityToken::from_accepted_mob_machine_input(&seed_input)?;
        let outcome =
            apply_mob_machine_flow_frame_command(&initial, &machine_state, start_input, authority)?;
        let snapshot = FrameSnapshot {
            kernel_state: outcome.next_state,
        };
        let inserted = self
            .projector
            .commit_flow_frame_store_plan(
                run_id,
                FlowFrameLoopStorePlan::InsertFrame {
                    machine_inputs: vec![seed_input],
                    frame_id: frame_id.clone(),
                    initial_frame: snapshot.clone(),
                },
            )
            .await?;
        if !inserted {
            let run2 = self
                .run_store
                .get_run(run_id)
                .await?
                .ok_or_else(|| MobError::RunNotFound(run_id.clone()))?;
            return run2.frames.get(frame_id).cloned().ok_or_else(|| {
                MobError::Internal(format!(
                    "frame '{frame_id}' missing after concurrent insert in run '{run_id}'"
                ))
            });
        }
        Ok(snapshot)
    }

    async fn admit_next_ready_node(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
    ) -> Result<Option<Vec<flow_frame::Effect>>, MobError> {
        let machine_state = self.current_mob_machine_state().await?;
        let Some(input) =
            build_admit_next_ready_node_command_from_machine(&machine_state, frame_id)
        else {
            return Ok(None);
        };
        self.transition_frame(run_id, frame_id, input, 5)
            .await
            .map(Some)
    }

    async fn admit_next_ready_node_with_retry(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        max_retries: usize,
    ) -> Result<Option<Vec<flow_frame::Effect>>, MobError> {
        for _ in 0..=max_retries {
            let snap = self.require_frame(run_id, frame_id).await?;
            let machine_state = self.current_mob_machine_state().await?;
            let Some(admit_input) =
                build_admit_next_ready_node_command_from_machine(&machine_state, frame_id)
            else {
                return Ok(None);
            };
            let machine_input = admit_input.authority_input(frame_id);
            let (authority, machine_state) = self
                .authorize_frame_command(run_id, frame_id, &admit_input)
                .await?;
            let outcome = apply_mob_machine_flow_frame_command(
                &snap.kernel_state,
                &machine_state,
                admit_input,
                authority,
            )?;
            let next_snap = FrameSnapshot {
                kernel_state: outcome.next_state,
            };
            let won = self
                .projector
                .commit_flow_frame_store_plan(
                    run_id,
                    FlowFrameLoopStorePlan::FrameState {
                        machine_inputs: vec![machine_input],
                        frame_id: frame_id.clone(),
                        expected_frame: snap,
                        next_frame: next_snap,
                    },
                )
                .await?;
            if won {
                return Ok(Some(outcome.effects));
            }
        }
        Err(MobError::Internal(format!(
            "admit_next_ready_node: CAS exhausted {max_retries} retries for frame '{frame_id}' \
             — queue was non-empty but every attempt lost the CAS"
        )))
    }

    async fn complete_step(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        opts: StepCompletionOpts<'_>,
    ) -> Result<(), MobError> {
        let StepCompletionOpts {
            node_id,
            step_id,
            output,
            loop_context,
            max_retries,
        } = opts;
        for attempt in 0..=max_retries {
            let snap = self.require_frame(run_id, frame_id).await?;
            let complete_input =
                MobMachineFlowFrameCommand::CompleteNode(flow_frame::inputs::CompleteNode {
                    node_id: node_id.clone(),
                });
            let machine_input = complete_input.authority_input(frame_id);
            let (authority, machine_state) = self
                .authorize_frame_command(run_id, frame_id, &complete_input)
                .await?;
            let next_outcome = apply_mob_machine_flow_frame_command(
                &snap.kernel_state,
                &machine_state,
                complete_input,
                authority,
            )?;
            let next_snap = FrameSnapshot {
                kernel_state: next_outcome.next_state,
            };
            let won = self
                .projector
                .commit_flow_frame_store_plan(
                    run_id,
                    FlowFrameLoopStorePlan::CompleteStepAndRecordOutput {
                        machine_inputs: vec![machine_input],
                        frame_id: frame_id.clone(),
                        expected_frame: snap,
                        next_frame: next_snap,
                        step_output_key: step_id.to_string(),
                        step_output: output.clone(),
                        loop_context: loop_context
                            .map(|(loop_id, iteration)| (loop_id.clone(), iteration)),
                    },
                )
                .await?;
            if won {
                return Ok(());
            }
            if attempt == max_retries {
                return Err(MobError::Internal(format!(
                    "CompleteNode CAS failed after {} attempts for node '{node_id}'",
                    max_retries + 1
                )));
            }
        }
        Err(MobError::Internal("CompleteNode CAS exhausted".into()))
    }

    async fn complete_node(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        node_id: &FlowNodeId,
    ) -> Result<bool, MobError> {
        let input = MobMachineFlowFrameCommand::CompleteNode(flow_frame::inputs::CompleteNode {
            node_id: node_id.clone(),
        });
        self.transition_frame(run_id, frame_id, input, 5)
            .await
            .map(|_| true)
    }

    async fn fail_node(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        node_id: &FlowNodeId,
    ) -> Result<bool, MobError> {
        let input = MobMachineFlowFrameCommand::FailNode(flow_frame::inputs::FailNode {
            node_id: node_id.clone(),
        });
        self.transition_frame(run_id, frame_id, input, 5)
            .await
            .map(|_| true)
    }

    async fn skip_node(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        node_id: &FlowNodeId,
    ) -> Result<bool, MobError> {
        let input = MobMachineFlowFrameCommand::SkipNode(flow_frame::inputs::SkipNode {
            node_id: node_id.clone(),
        });
        self.transition_frame(run_id, frame_id, input, 5)
            .await
            .map(|_| true)
    }

    async fn cancel_node(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        node_id: &FlowNodeId,
    ) -> Result<bool, MobError> {
        let input = MobMachineFlowFrameCommand::CancelNode(flow_frame::inputs::CancelNode {
            node_id: node_id.clone(),
        });
        self.transition_frame(run_id, frame_id, input, 5)
            .await
            .map(|_| true)
    }

    async fn terminalize_frame(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
    ) -> Result<bool, MobError> {
        for _ in 0..=5 {
            let current = self.require_frame(run_id, frame_id).await?;
            let machine_state = self.current_mob_machine_state().await?;
            let terminal_status =
                machine_candidate_frame_terminal_status(&machine_state, frame_id).ok_or_else(|| {
                    MobError::Internal(format!(
                        "frame '{frame_id}' cannot be sealed because MobMachine does not project a terminal frame class"
                    ))
                })?;
            let input = MobMachineFlowFrameCommand::SealFrame(flow_frame::inputs::SealFrame {
                terminal_status,
            });
            let machine_input = input.authority_input(frame_id);
            let (authority, machine_state) = self
                .authorize_frame_command(run_id, frame_id, &input)
                .await?;
            let outcome = apply_mob_machine_flow_frame_command(
                &current.kernel_state,
                &machine_state,
                input,
                authority,
            )?;
            let next = FrameSnapshot {
                kernel_state: outcome.next_state,
            };
            let won = self
                .projector
                .commit_flow_frame_store_plan(
                    run_id,
                    FlowFrameLoopStorePlan::FrameState {
                        machine_inputs: vec![machine_input],
                        frame_id: frame_id.clone(),
                        expected_frame: current,
                        next_frame: next,
                    },
                )
                .await?;
            if won {
                return Ok(true);
            }
        }
        Err(MobError::Internal(format!(
            "terminalize_frame: CAS exhausted for frame '{frame_id}'"
        )))
    }
}

// ─── FlowFrameEngine ─────────────────────────────────────────────────────────

/// Drives a `FrameSpec` to completion using a scheduler-backed concurrent
/// coordinator for frame-local work.
pub struct FlowFrameEngine {
    run_store: Arc<dyn MobRunStore>,
    executor: Arc<dyn FrameStepExecutor>,
    frame_kernel: Arc<FlowFrameKernel>,
    projector: MobHandle,
    /// Maximum nesting depth for body frames. 0 means unlimited.
    max_frame_depth: u32,
    /// Maximum number of simultaneously active body frames admitted by the run scheduler.
    /// 0 means unlimited.
    max_active_frames: u32,
}

#[allow(clippy::large_futures)]
impl FlowFrameEngine {
    pub fn new(
        run_store: Arc<dyn MobRunStore>,
        executor: Arc<dyn FrameStepExecutor>,
        projector: MobHandle,
        max_frame_depth: u32,
        max_active_frames: u32,
    ) -> Self {
        let frame_kernel = Arc::new(FlowFrameKernel::new(run_store.clone(), projector.clone()));
        Self {
            run_store,
            executor,
            frame_kernel,
            projector,
            max_frame_depth,
            max_active_frames,
        }
    }

    // ─── Public entry point ──────────────────────────────────────────────────

    /// Execute a `FrameSpec` to completion (root frame — no loop context).
    ///
    /// Returns when all nodes in the frame have reached a terminal state.
    #[allow(clippy::type_complexity)]
    pub fn execute_frame<'a>(
        &'a self,
        run_id: &'a RunId,
        frame_id: &'a FrameId,
        spec: &'a FrameSpec,
        context: &'a FlowContext,
    ) -> FrameExecutionFuture<'a> {
        Box::pin(self.execute_frame_concurrent_root(run_id, frame_id, spec, context))
    }

    pub async fn start_frame_snapshot(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        spec: &FrameSpec,
    ) -> Result<FrameSnapshot, MobError> {
        self.frame_kernel.start_frame(run_id, frame_id, spec).await
    }

    // ─── Private helpers ─────────────────────────────────────────────────────

    async fn execute_frame_concurrent_root(
        &self,
        run_id: &RunId,
        root_frame_id: &FrameId,
        root_spec: &FrameSpec,
        context: &FlowContext,
    ) -> Result<FrameExecutionOutcome, MobError> {
        self.ensure_scheduler_state_initialized(run_id).await?;
        self.frame_kernel
            .start_frame(run_id, root_frame_id, root_spec)
            .await?;
        self.heal_terminal_body_frames(run_id, root_frame_id, root_spec, context)
            .await?;
        self.heal_orphaned_running_nodes(run_id, root_frame_id, root_spec, context)
            .await?;
        self.revisit_frame(run_id, root_frame_id, root_spec, context, root_frame_id)
            .await?;
        self.revisit_known_frames(run_id, root_frame_id, root_spec, context)
            .await?;

        let mut workers: FrameNodeWorkers = FuturesUnordered::new();
        let mut step_failures = IndexMap::new();

        loop {
            self.executor.check_runtime_guards(run_id).await?;

            let mut progressed = false;
            while self
                .try_admit_one(run_id, root_frame_id, root_spec, context, &mut workers)
                .await?
            {
                progressed = true;
            }

            if !progressed {
                match workers.next().await {
                    Some(task) => {
                        self.handle_task_result(
                            run_id,
                            root_frame_id,
                            root_spec,
                            context,
                            task,
                            &mut step_failures,
                        )
                        .await?;
                        progressed = true;
                    }
                    None => {
                        progressed = self
                            .revisit_frame(run_id, root_frame_id, root_spec, context, root_frame_id)
                            .await?;
                    }
                }
            }

            let root_snapshot = self.require_frame(run_id, root_frame_id).await?;
            if root_snapshot.kernel_state.phase != flow_frame::Phase::Running && workers.is_empty()
            {
                break;
            }

            if !progressed && workers.is_empty() {
                let run = self.require_run(run_id).await?;
                let machine_state = self.frame_kernel.current_mob_machine_state().await?;
                let ready_frame_ids =
                    machine_ready_frame_registration_candidates(&machine_state, run_id);
                if !ready_frame_ids.is_empty() {
                    drop(run);
                    for frame_id in ready_frame_ids {
                        let run = self.require_run(run_id).await?;
                        let Some(frame) = run.frames.get(&frame_id) else {
                            continue;
                        };
                        if let Some(plan) = self
                            .register_ready_frame_if_needed(
                                run_id,
                                &run.flow_state,
                                &frame_id,
                                &frame.kernel_state,
                            )
                            .await?
                        {
                            let _ = self.execute_store_plan(run_id, &plan).await?;
                        }
                    }
                    continue;
                }
                if run.flow_state.active_node_count > 0 || run.flow_state.active_frame_count > 0 {
                    tokio_time::sleep(std::time::Duration::from_millis(10)).await;
                    continue;
                }
                let frame_debug = run
                    .frames
                    .iter()
                    .map(|(frame_id, frame)| {
                        format!(
                            "{frame_id}: phase={:?} scope={:?} ready={:?} statuses={:?}",
                            frame.kernel_state.phase,
                            frame.kernel_state.frame_scope,
                            frame.kernel_state.ready_queue,
                            frame.kernel_state.node_status
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
                return Err(MobError::Internal(format!(
                    "frame runtime stalled for root frame '{root_frame_id}' in run '{run_id}'; frames=[{frame_debug}]; run_ready={:?}",
                    run.flow_state.ready_frames
                )));
            }
        }

        let run = self.require_run(run_id).await?;
        let outputs = root_outputs_from_run(&run);
        let step_projections = self.collect_all_step_projections(&run, root_frame_id, root_spec)?;
        let root_frame = run.frames.get(root_frame_id).cloned().ok_or_else(|| {
            MobError::Internal(format!(
                "root frame '{root_frame_id}' missing at end of run '{run_id}'"
            ))
        })?;
        root_terminal_phase(&root_frame).ok_or_else(|| {
            MobError::Internal(format!(
                "root frame '{root_frame_id}' ended in non-terminal phase '{:?}'",
                root_frame.kernel_state.phase
            ))
        })?;

        Ok(FrameExecutionOutcome {
            outputs,
            step_projections,
            step_failures,
        })
    }

    async fn try_admit_one(
        &self,
        run_id: &RunId,
        root_frame_id: &FrameId,
        root_spec: &FrameSpec,
        context: &FlowContext,
        workers: &mut FrameNodeWorkers,
    ) -> Result<bool, MobError> {
        if self
            .try_ack_node_grant(run_id, root_frame_id, root_spec, context, workers)
            .await?
        {
            return Ok(true);
        }

        self.try_ack_body_frame_start(run_id, root_frame_id, root_spec, context)
            .await
    }

    async fn try_ack_node_grant(
        &self,
        run_id: &RunId,
        root_frame_id: &FrameId,
        root_spec: &FrameSpec,
        context: &FlowContext,
        workers: &mut FrameNodeWorkers,
    ) -> Result<bool, MobError> {
        for _ in 0..=5usize {
            let run = self.require_run(run_id).await?;
            let Some(frame_id) = local_node_grant_candidate(&run.flow_state) else {
                return Ok(false);
            };
            let pump_command =
                MobMachineFlowRunCommand::PumpNodeScheduler(flow_run::inputs::PumpNodeScheduler {
                    frame_id,
                });
            let (pump_authority, machine_state) =
                self.authorize_run_command(run_id, &pump_command).await?;
            let Some(grant) = preview_node_grant(
                &run.flow_state,
                pump_command,
                pump_authority,
                &machine_state,
                run_id,
            )?
            else {
                return Ok(false);
            };
            let frame_id = grant.frame_id.clone();

            let current_frame = run.frames.get(&frame_id).cloned().ok_or_else(|| {
                MobError::Internal(format!(
                    "run '{run_id}' granted node slot for unknown frame '{frame_id}'"
                ))
            })?;
            let frame_spec = self.resolve_frame_spec(&run, root_frame_id, root_spec, &frame_id)?;
            let frame_depth = self.frame_depth(&run, root_frame_id, &frame_id)?;
            let decision = self
                .acknowledge_node_grant(
                    run_id,
                    &run.flow_state,
                    &grant,
                    &current_frame,
                    &frame_spec,
                    FrameDepthLimit {
                        current_depth: frame_depth,
                        max_depth: self.max_frame_depth,
                    },
                )
                .await?;
            if !self
                .execute_decision(
                    run_id,
                    root_frame_id,
                    root_spec,
                    context,
                    Some(workers),
                    decision,
                )
                .await?
            {
                continue;
            }
            return Ok(true);
        }

        Err(MobError::Internal(format!(
            "node grant CAS exhausted for run '{run_id}'"
        )))
    }

    async fn try_ack_body_frame_start(
        &self,
        run_id: &RunId,
        root_frame_id: &FrameId,
        root_spec: &FrameSpec,
        context: &FlowContext,
    ) -> Result<bool, MobError> {
        for _ in 0..=5usize {
            let run = self.require_run(run_id).await?;
            let Some(loop_instance_id) = local_body_frame_grant_candidate(&run.flow_state) else {
                return Ok(false);
            };
            let pump_command = MobMachineFlowRunCommand::PumpFrameScheduler(
                flow_run::inputs::PumpFrameScheduler { loop_instance_id },
            );
            let (pump_authority, machine_state) =
                self.authorize_run_command(run_id, &pump_command).await?;
            let Some(grant) = preview_body_frame_grant(
                &run.flow_state,
                pump_command,
                pump_authority,
                &machine_state,
                run_id,
            )?
            else {
                return Ok(false);
            };
            let loop_instance_id = grant.loop_instance_id.clone();

            let loop_snapshot = run.loops.get(&loop_instance_id).cloned().ok_or_else(|| {
                MobError::Internal(format!(
                    "run '{run_id}' granted body frame start for unknown loop '{loop_instance_id}'"
                ))
            })?;
            let loop_spec =
                self.resolve_loop_spec(&run, root_frame_id, root_spec, &loop_instance_id)?;
            let decision = self
                .acknowledge_body_frame_start(
                    run_id,
                    &run.flow_state,
                    &grant,
                    &loop_snapshot,
                    &loop_spec,
                )
                .await?;
            if !self
                .execute_decision(run_id, root_frame_id, root_spec, context, None, decision)
                .await?
            {
                continue;
            }
            return Ok(true);
        }

        Err(MobError::Internal(format!(
            "body-frame grant CAS exhausted for run '{run_id}'"
        )))
    }

    fn spawn_step_task(&self, workers: &mut FrameNodeWorkers, run_id: &RunId, task: SpawnStepTask) {
        let executor = self.executor.clone();
        let run_id = run_id.clone();
        let SpawnStepTask {
            frame_id,
            node_id,
            step_id,
            loop_context,
            context,
        } = task;
        workers.push(Box::pin(async move {
            let result = executor
                .execute_step(&run_id, &frame_id, &node_id, &step_id, &context)
                .await;
            FrameNodeTaskResult::Step {
                frame_id,
                node_id,
                step_id,
                loop_context,
                result,
            }
        }));
    }

    async fn handle_task_result(
        &self,
        run_id: &RunId,
        root_frame_id: &FrameId,
        root_spec: &FrameSpec,
        context: &FlowContext,
        task: FrameNodeTaskResult,
        step_failures: &mut IndexMap<StepId, FrameStepFailureProjection>,
    ) -> Result<(), MobError> {
        match task {
            FrameNodeTaskResult::Step {
                frame_id,
                node_id,
                step_id,
                loop_context,
                result,
            } => {
                match result {
                    Ok(FrameStepResult::Completed(output)) => {
                        let loop_context = loop_context
                            .as_ref()
                            .map(|(loop_id, iteration)| (loop_id, *iteration));
                        self.frame_kernel
                            .complete_step(
                                run_id,
                                &frame_id,
                                StepCompletionOpts {
                                    node_id: &node_id,
                                    step_id: &step_id,
                                    output,
                                    loop_context,
                                    max_retries: 5,
                                },
                            )
                            .await?;
                    }
                    Ok(FrameStepResult::Skipped) => {
                        self.frame_kernel
                            .skip_node(run_id, &frame_id, &node_id)
                            .await?;
                    }
                    Ok(FrameStepResult::Failed {
                        reason,
                        failure_ledger_recorded,
                    }) => {
                        step_failures.insert(
                            step_id.clone(),
                            FrameStepFailureProjection {
                                reason,
                                append_failure_ledger: !failure_ledger_recorded,
                            },
                        );
                        self.frame_kernel
                            .fail_node(run_id, &frame_id, &node_id)
                            .await?;
                    }
                    Err(error) => {
                        if matches!(error, MobError::RunCanceled(_)) {
                            self.frame_kernel
                                .cancel_node(run_id, &frame_id, &node_id)
                                .await?;
                        } else {
                            self.frame_kernel
                                .fail_node(run_id, &frame_id, &node_id)
                                .await?;
                        }
                        return Err(error);
                    }
                }

                self.release_node_execution_and_revisit(
                    run_id,
                    &frame_id,
                    root_frame_id,
                    root_spec,
                    context,
                )
                .await?;
            }
        }
        Ok(())
    }

    async fn revisit_frame(
        &self,
        run_id: &RunId,
        root_frame_id: &FrameId,
        root_spec: &FrameSpec,
        context: &FlowContext,
        frame_id: &FrameId,
    ) -> Result<bool, MobError> {
        let mut progressed = false;
        loop {
            let run = self.require_run(run_id).await?;
            let Some(frame) = run.frames.get(frame_id).cloned() else {
                return Ok(progressed);
            };

            if let Some(plan) = self
                .register_ready_frame_if_needed(
                    run_id,
                    &run.flow_state,
                    frame_id,
                    &frame.kernel_state,
                )
                .await?
            {
                progressed |= self.execute_store_plan(run_id, &plan).await?;
                continue;
            }

            let frame_spec = self.resolve_frame_spec(&run, root_frame_id, root_spec, frame_id)?;
            if let Some(plan) = self
                .seal_frame_if_ready(run_id, frame_id, &frame, &frame_spec)
                .await?
            {
                progressed |= self.execute_store_plan(run_id, &plan).await?;
                continue;
            }

            let run = self.require_run(run_id).await?;
            let Some(frame) = run.frames.get(frame_id).cloned() else {
                return Ok(progressed);
            };

            if let Some(loop_instance_id) = frame_loop_instance_id(&frame.kernel_state)
                && let Some(loop_snapshot) = run.loops.get(&loop_instance_id).cloned()
            {
                if let Some(obligation) = pending_until_obligation(&loop_snapshot)? {
                    self.resolve_until_feedback(
                        run_id,
                        root_frame_id,
                        root_spec,
                        context,
                        obligation,
                    )
                    .await?;
                    progressed = true;
                    continue;
                }

                let parent_frame = self.parent_frame_for_loop(&run, &loop_snapshot);
                if let Some(decision) = self
                    .advance_body_frame_after_seal(
                        run_id,
                        &run.flow_state,
                        frame_id,
                        &frame,
                        &loop_snapshot,
                        parent_frame.as_ref(),
                    )
                    .await?
                {
                    let _ = self
                        .execute_decision(run_id, root_frame_id, root_spec, context, None, decision)
                        .await?;
                    progressed = true;
                    continue;
                }
            }

            return Ok(progressed);
        }
    }

    async fn revisit_known_frames(
        &self,
        run_id: &RunId,
        root_frame_id: &FrameId,
        root_spec: &FrameSpec,
        context: &FlowContext,
    ) -> Result<(), MobError> {
        let frame_ids = self
            .require_run(run_id)
            .await?
            .frames
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for frame_id in frame_ids {
            let run = self.require_run(run_id).await?;
            let Some(frame) = run.frames.get(&frame_id).cloned() else {
                continue;
            };
            let frame_spec = self.resolve_frame_spec(&run, root_frame_id, root_spec, &frame_id)?;
            self.frame_kernel
                .confirm_frame_seed_from_snapshot(run_id, &frame_id, &frame, &frame_spec)
                .await?;
            self.revisit_frame(run_id, root_frame_id, root_spec, context, &frame_id)
                .await?;
        }
        Ok(())
    }

    async fn ensure_scheduler_state_initialized(&self, run_id: &RunId) -> Result<(), MobError> {
        for _ in 0..=5usize {
            let run = self.require_run(run_id).await?;
            if run.flow_state.phase == flow_run::Phase::Absent {
                if run.status != crate::run::MobRunStatus::Pending {
                    return Err(MobError::Internal(format!(
                        "run '{run_id}' has no MobMachine-authorized flow state; CreateRunSeed/StartRun must initialize scheduler state before frame execution"
                    )));
                }
                let command = MobMachineFlowRunCommand::StartRun(flow_run::inputs::StartRun {});
                if self
                    .projector
                    .commit_flow_run_command(run_id, command, "flow_frame_scheduler_start_run")
                    .await?
                    .is_some()
                {
                    continue;
                }
                continue;
            }
            if run.flow_state.phase == flow_run::Phase::Pending
                && run.status == crate::run::MobRunStatus::Pending
            {
                let command = MobMachineFlowRunCommand::StartRun(flow_run::inputs::StartRun {});
                if self
                    .projector
                    .commit_flow_run_command(run_id, command, "flow_frame_scheduler_start_run")
                    .await?
                    .is_some()
                {
                    continue;
                }
                continue;
            }
            if self.max_frame_depth > 0 && run.flow_state.max_frame_depth != self.max_frame_depth {
                return Err(MobError::Internal(format!(
                    "run '{run_id}' max_frame_depth was not initialized by MobMachine authority"
                )));
            }
            if self.max_active_frames > 0
                && run.flow_state.max_active_frames != self.max_active_frames
            {
                return Err(MobError::Internal(format!(
                    "run '{run_id}' max_active_frames was not initialized by MobMachine authority"
                )));
            }
            return Ok(());
        }
        Err(MobError::Internal(format!(
            "scheduler state initialization CAS exhausted for run '{run_id}'"
        )))
    }

    async fn heal_orphaned_running_nodes(
        &self,
        run_id: &RunId,
        root_frame_id: &FrameId,
        root_spec: &FrameSpec,
        context: &FlowContext,
    ) -> Result<(), MobError> {
        loop {
            let run = self.require_run(run_id).await?;
            let frame_ids: Vec<FrameId> = run.frames.keys().cloned().collect();
            let mut healed_any = false;

            for frame_id in frame_ids {
                let run = self.require_run(run_id).await?;
                let Some(frame) = run.frames.get(&frame_id).cloned() else {
                    continue;
                };
                let spec = self.resolve_frame_spec(&run, root_frame_id, root_spec, &frame_id)?;
                for node_id in running_node_ids(&frame.kernel_state) {
                    match spec.nodes.get(&node_id) {
                        Some(FlowNodeSpec::Step(_)) => {
                            healed_any = true;
                            self.frame_kernel
                                .fail_node(run_id, &frame_id, &node_id)
                                .await?;
                            self.release_node_execution_and_revisit(
                                run_id,
                                &frame_id,
                                root_frame_id,
                                root_spec,
                                context,
                            )
                            .await?;
                            self.revisit_frame(
                                run_id,
                                root_frame_id,
                                root_spec,
                                context,
                                &frame_id,
                            )
                            .await?;
                        }
                        Some(FlowNodeSpec::RepeatUntil(_)) => {
                            let loop_instance_id =
                                LoopInstanceId::from(format!("{frame_id}::{node_id}").as_str());
                            if let Some(loop_snapshot) = run.loops.get(&loop_instance_id) {
                                if let Some(decision) = self
                                    .recover_terminal_loop_projection(
                                        run_id,
                                        &run.flow_state,
                                        loop_snapshot,
                                        &frame,
                                    )
                                    .await?
                                {
                                    healed_any = true;
                                    let _ = self
                                        .execute_decision(
                                            run_id,
                                            root_frame_id,
                                            root_spec,
                                            context,
                                            None,
                                            decision,
                                        )
                                        .await?;
                                    continue;
                                }
                                if let Some(decision) = self
                                    .recover_pending_body_frame_request(
                                        run_id,
                                        &run.flow_state,
                                        loop_snapshot,
                                    )
                                    .await?
                                {
                                    healed_any = true;
                                    let _ = self
                                        .execute_decision(
                                            run_id,
                                            root_frame_id,
                                            root_spec,
                                            context,
                                            None,
                                            decision,
                                        )
                                        .await?;
                                    continue;
                                }
                                continue;
                            }
                            healed_any = true;
                            self.frame_kernel
                                .fail_node(run_id, &frame_id, &node_id)
                                .await?;
                            self.release_node_execution_and_revisit(
                                run_id,
                                &frame_id,
                                root_frame_id,
                                root_spec,
                                context,
                            )
                            .await?;
                            self.revisit_frame(
                                run_id,
                                root_frame_id,
                                root_spec,
                                context,
                                &frame_id,
                            )
                            .await?;
                        }
                        None => {}
                    }
                }
            }

            if !healed_any {
                return Ok(());
            }
        }
    }

    async fn heal_terminal_body_frames(
        &self,
        run_id: &RunId,
        root_frame_id: &FrameId,
        root_spec: &FrameSpec,
        context: &FlowContext,
    ) -> Result<(), MobError> {
        loop {
            let run = self.require_run(run_id).await?;
            let body_frame_ids: Vec<FrameId> = run
                .frames
                .iter()
                .filter(|(_, frame)| frame_is_body(&frame.kernel_state))
                .map(|(frame_id, _)| frame_id.clone())
                .collect();
            let mut healed_any = false;

            for frame_id in body_frame_ids {
                let run = self.require_run(run_id).await?;
                let Some(frame) = run.frames.get(&frame_id) else {
                    continue;
                };
                let Some(loop_instance_id) = frame_loop_instance_id(&frame.kernel_state) else {
                    continue;
                };
                let Some(loop_snapshot) = run.loops.get(&loop_instance_id) else {
                    continue;
                };
                let active_body_owner =
                    active_body_frame_id(&loop_snapshot.kernel_state).as_ref() == Some(&frame_id);
                let awaiting_until = pending_until_obligation(loop_snapshot)?.is_some();
                if frame.kernel_state.phase == flow_frame::Phase::Running
                    || (!active_body_owner && !awaiting_until)
                {
                    continue;
                }
                healed_any = true;
                self.revisit_frame(run_id, root_frame_id, root_spec, context, &frame_id)
                    .await?;
                break;
            }

            if !healed_any {
                return Ok(());
            }
        }
    }

    async fn release_node_execution_and_revisit(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        root_frame_id: &FrameId,
        root_spec: &FrameSpec,
        context: &FlowContext,
    ) -> Result<(), MobError> {
        for _ in 0..=5usize {
            let run = self.require_run(run_id).await?;
            let plan = self
                .release_node_execution(run_id, &run.flow_state, frame_id)
                .await?;
            let won = self.execute_store_plan(run_id, &plan).await?;
            if won {
                self.revisit_frame(run_id, root_frame_id, root_spec, context, frame_id)
                    .await?;
                return Ok(());
            }
        }
        Err(MobError::Internal(format!(
            "NodeExecutionReleased CAS exhausted for frame '{frame_id}' in run '{run_id}'"
        )))
    }

    async fn execute_decision(
        &self,
        run_id: &RunId,
        root_frame_id: &FrameId,
        root_spec: &FrameSpec,
        context: &FlowContext,
        workers: Option<&mut FrameNodeWorkers>,
        decision: FlowFrameLoopDecision,
    ) -> Result<bool, MobError> {
        if let Some(plan) = &decision.store_plan {
            if !self.execute_store_plan(run_id, plan).await? {
                return Ok(false);
            }
            self.project_store_plan(run_id, root_frame_id, root_spec, plan)
                .await?;
        }
        let mut workers = workers;
        for work in decision.follow_up {
            match work {
                FlowFrameLoopWork::SpawnStep {
                    frame_id,
                    node_id,
                    step_id,
                } => {
                    let workers = workers.as_deref_mut().ok_or_else(|| {
                        MobError::Internal(
                            "SpawnStep follow-up requires a live worker queue".into(),
                        )
                    })?;
                    let run_after = self.require_run(run_id).await?;
                    let worker_context = self.build_context_for_frame(
                        &run_after,
                        run_id.clone(),
                        context,
                        &frame_id,
                    )?;
                    let loop_context = self.frame_loop_context(&run_after, &frame_id);
                    self.spawn_step_task(
                        workers,
                        run_id,
                        SpawnStepTask {
                            frame_id,
                            node_id,
                            step_id,
                            loop_context,
                            context: worker_context,
                        },
                    );
                }
                FlowFrameLoopWork::EvaluateUntil { obligation } => {
                    Box::pin(self.resolve_until_feedback(
                        run_id,
                        root_frame_id,
                        root_spec,
                        context,
                        obligation,
                    ))
                    .await?;
                }
                FlowFrameLoopWork::RevisitFrame { frame_id } => {
                    Box::pin(self.revisit_frame(
                        run_id,
                        root_frame_id,
                        root_spec,
                        context,
                        &frame_id,
                    ))
                    .await?;
                }
            }
        }
        Ok(true)
    }

    async fn execute_store_plan(
        &self,
        run_id: &RunId,
        plan: &FlowFrameLoopStorePlan,
    ) -> Result<bool, MobError> {
        self.frame_kernel
            .projector
            .commit_flow_frame_store_plan(run_id, plan.clone())
            .await
    }

    async fn project_store_plan(
        &self,
        _run_id: &RunId,
        _root_frame_id: &FrameId,
        _root_spec: &FrameSpec,
        plan: &FlowFrameLoopStorePlan,
    ) -> Result<(), MobError> {
        match plan {
            FlowFrameLoopStorePlan::InsertFrame { .. } => {}
            FlowFrameLoopStorePlan::FrameState { .. } => {}
            FlowFrameLoopStorePlan::CompleteStepAndRecordOutput { .. } => {}
            FlowFrameLoopStorePlan::StartLoop { .. } => {}
            FlowFrameLoopStorePlan::GrantBodyFrameStart { .. } => {}
            FlowFrameLoopStorePlan::CompleteBodyFrame { .. } => {}
            FlowFrameLoopStorePlan::GrantNodeSlot { .. }
            | FlowFrameLoopStorePlan::RunStateOnly { .. }
            | FlowFrameLoopStorePlan::SealFrame { .. }
            | FlowFrameLoopStorePlan::LoopRequestBodyFrame { .. }
            | FlowFrameLoopStorePlan::CompleteLoop { .. } => {}
        }
        Ok(())
    }

    async fn project_until_feedback(
        &self,
        feedback: Option<&LoopUntilConditionFeedbackProjection>,
    ) -> Result<(), MobError> {
        let Some(_feedback) = feedback else {
            return Ok(());
        };
        Ok(())
    }

    async fn resolve_until_feedback(
        &self,
        run_id: &RunId,
        root_frame_id: &FrameId,
        root_spec: &FrameSpec,
        context: &FlowContext,
        obligation: FlowLoopUntilEvaluationObligation,
    ) -> Result<(), MobError> {
        for _ in 0..=5usize {
            let run = self.require_run(run_id).await?;
            let Some(loop_snapshot) = run.loops.get(&obligation.loop_instance_id) else {
                return Ok(());
            };
            if pending_until_obligation(loop_snapshot)?.is_none() {
                return Ok(());
            }
            let parent_frame = run
                .frames
                .get(&obligation.parent_frame_id)
                .cloned()
                .ok_or_else(|| {
                    MobError::Internal(format!(
                        "missing parent frame '{}' for loop '{}'",
                        obligation.parent_frame_id, obligation.loop_instance_id
                    ))
                })?;
            let loop_spec = self.resolve_loop_spec_by_parent_node(
                &run,
                root_frame_id,
                root_spec,
                &obligation.parent_frame_id,
                &obligation.parent_node_id,
                &obligation.loop_id,
            )?;
            let eval_context = FlowContext::from_run_aggregate(
                &run,
                run_id.clone(),
                context.activation_params.clone(),
            );
            let until_met = evaluate_condition(&loop_spec.until, &eval_context);
            let (decision, until_feedback) = self
                .resolve_until_feedback_decision(
                    run_id,
                    &run.flow_state,
                    loop_snapshot,
                    &parent_frame,
                    obligation.clone(),
                    until_met,
                )
                .await?;
            if self
                .execute_decision(run_id, root_frame_id, root_spec, context, None, decision)
                .await?
            {
                self.project_until_feedback(Some(&until_feedback)).await?;
                return Ok(());
            }
        }

        Err(MobError::Internal(format!(
            "until feedback CAS exhausted for loop '{}' in run '{run_id}'",
            obligation.loop_instance_id
        )))
    }

    fn parent_frame_for_loop(
        &self,
        run: &MobRun,
        loop_snapshot: &LoopSnapshot,
    ) -> Option<FrameSnapshot> {
        let parent_frame_id = loop_parent_frame_id(&loop_snapshot.kernel_state).ok()?;
        run.frames.get(&parent_frame_id).cloned()
    }

    fn collect_all_step_projections(
        &self,
        run: &MobRun,
        root_frame_id: &FrameId,
        root_spec: &FrameSpec,
    ) -> Result<IndexMap<StepId, FrameStepProjection>, MobError> {
        let mut projections = IndexMap::new();
        for (frame_id, frame) in &run.frames {
            let spec = self.resolve_frame_spec(run, root_frame_id, root_spec, frame_id)?;
            merge_step_projections(
                &mut projections,
                collect_frame_step_projections(frame_id, &spec, &frame.kernel_state),
            );
        }
        Ok(projections)
    }

    fn build_context_for_frame(
        &self,
        run: &MobRun,
        run_id: RunId,
        base_context: &FlowContext,
        frame_id: &FrameId,
    ) -> Result<FlowContext, MobError> {
        let mut context =
            FlowContext::from_run_aggregate(run, run_id, base_context.activation_params.clone());
        for (step_id, output) in &base_context.step_outputs {
            context
                .step_outputs
                .entry(step_id.clone())
                .or_insert_with(|| output.clone());
        }
        for (loop_id, history) in &base_context.loop_outputs {
            context
                .loop_outputs
                .entry(loop_id.clone())
                .or_insert_with(|| history.clone());
        }
        if let Some((loop_id, iteration)) = self.frame_loop_context(run, frame_id)
            && let Some(iterations) = run.loop_iteration_outputs.get(&loop_id)
        {
            let iteration_index = usize_from_u64(iteration, "loop iteration index")?;
            if let Some(iter_outputs) = iterations.get(iteration_index) {
                for (step_id, output) in iter_outputs {
                    // Current-iteration outputs must override the last completed
                    // iteration projection from FlowContext::from_run_aggregate.
                    context.step_outputs.insert(step_id.clone(), output.clone());
                }
            }
        }
        Ok(context)
    }

    fn frame_loop_context(&self, run: &MobRun, frame_id: &FrameId) -> Option<(LoopId, u64)> {
        let frame = run.frames.get(frame_id)?;
        let loop_instance_id = frame_loop_instance_id(&frame.kernel_state)?;
        let loop_snapshot = run.loops.get(&loop_instance_id)?;
        Some((
            loop_id_from_state(&loop_snapshot.kernel_state).ok()?,
            frame_iteration(&frame.kernel_state).ok()?,
        ))
    }

    fn resolve_frame_spec(
        &self,
        run: &MobRun,
        root_frame_id: &FrameId,
        root_spec: &FrameSpec,
        frame_id: &FrameId,
    ) -> Result<FrameSpec, MobError> {
        if frame_id == root_frame_id {
            return Ok(root_spec.clone());
        }
        let frame = run.frames.get(frame_id).ok_or_else(|| {
            MobError::Internal(format!(
                "missing frame '{frame_id}' in run '{}'",
                run.run_id
            ))
        })?;
        if !frame_is_body(&frame.kernel_state) {
            return Ok(root_spec.clone());
        }
        let loop_instance_id = frame_loop_instance_id(&frame.kernel_state).ok_or_else(|| {
            MobError::Internal(format!("body frame '{frame_id}' missing loop ownership"))
        })?;
        self.resolve_loop_spec(run, root_frame_id, root_spec, &loop_instance_id)
            .map(|loop_spec| loop_spec.body)
    }

    fn resolve_loop_spec(
        &self,
        run: &MobRun,
        root_frame_id: &FrameId,
        root_spec: &FrameSpec,
        loop_instance_id: &LoopInstanceId,
    ) -> Result<crate::definition::RepeatUntilSpec, MobError> {
        let loop_snapshot = run.loops.get(loop_instance_id).ok_or_else(|| {
            MobError::Internal(format!("loop '{loop_instance_id}' missing snapshot"))
        })?;
        let parent_frame_id = loop_parent_frame_id(&loop_snapshot.kernel_state)?;
        let parent_node_id = loop_parent_node_id(&loop_snapshot.kernel_state)?;
        let loop_id = loop_id_from_state(&loop_snapshot.kernel_state)?;
        self.resolve_loop_spec_by_parent_node(
            run,
            root_frame_id,
            root_spec,
            &parent_frame_id,
            &parent_node_id,
            &loop_id,
        )
    }

    fn resolve_loop_spec_by_parent_node(
        &self,
        run: &MobRun,
        root_frame_id: &FrameId,
        root_spec: &FrameSpec,
        parent_frame_id: &FrameId,
        parent_node_id: &FlowNodeId,
        expected_loop_id: &LoopId,
    ) -> Result<crate::definition::RepeatUntilSpec, MobError> {
        let parent_spec =
            self.resolve_frame_spec(run, root_frame_id, root_spec, parent_frame_id)?;
        match parent_spec.nodes.get(parent_node_id) {
            Some(FlowNodeSpec::RepeatUntil(spec)) if spec.loop_id == *expected_loop_id => {
                Ok(spec.clone())
            }
            Some(FlowNodeSpec::RepeatUntil(spec)) => Err(MobError::Internal(format!(
                "loop node '{}' resolved unexpected loop id '{}' (expected '{}')",
                parent_node_id, spec.loop_id, expected_loop_id
            ))),
            other => Err(MobError::Internal(format!(
                "loop node '{parent_node_id}' in frame '{parent_frame_id}' does not resolve to RepeatUntil: {other:?}"
            ))),
        }
    }

    fn frame_depth(
        &self,
        run: &MobRun,
        root_frame_id: &FrameId,
        frame_id: &FrameId,
    ) -> Result<u32, MobError> {
        if frame_id == root_frame_id {
            return Ok(0);
        }
        let frame = run.frames.get(frame_id).ok_or_else(|| {
            MobError::Internal(format!(
                "missing frame '{frame_id}' in run '{}'",
                run.run_id
            ))
        })?;
        if !frame_is_body(&frame.kernel_state) {
            return Ok(0);
        }
        let loop_instance_id = frame_loop_instance_id(&frame.kernel_state).ok_or_else(|| {
            MobError::Internal(format!("body frame '{frame_id}' missing loop ownership"))
        })?;
        let loop_snapshot = run.loops.get(&loop_instance_id).ok_or_else(|| {
            MobError::Internal(format!(
                "missing loop '{loop_instance_id}' for body frame '{frame_id}'"
            ))
        })?;
        loop_depth(&loop_snapshot.kernel_state)
    }

    async fn require_run(&self, run_id: &RunId) -> Result<MobRun, MobError> {
        self.run_store
            .get_run(run_id)
            .await?
            .ok_or_else(|| MobError::RunNotFound(run_id.clone()))
    }

    async fn authorize_run_command(
        &self,
        run_id: &RunId,
        command: &MobMachineFlowRunCommand,
    ) -> Result<(MobMachineFlowAuthorityToken, mob_dsl::MobMachineState), MobError> {
        let authority_input = command.authority_input(run_id);
        let machine_state = self
            .frame_kernel
            .preview_mob_machine_input(authority_input.clone())
            .await
            .map_err(|error| {
                MobError::Internal(format!(
                    "MobMachine rejected run command {:?} for run '{run_id}': {error}",
                    command.kind()
                ))
            })?;
        let authority =
            MobMachineFlowAuthorityToken::from_accepted_mob_machine_input(&authority_input)?;
        Ok((authority, machine_state))
    }

    async fn authorize_frame_command(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        command: &MobMachineFlowFrameCommand,
    ) -> Result<(MobMachineFlowAuthorityToken, mob_dsl::MobMachineState), MobError> {
        let authority_input = command.authority_input(frame_id);
        let machine_state = self
            .frame_kernel
            .preview_mob_machine_input(authority_input.clone())
            .await?;
        validate_accepted_frame_command_witness(&authority_input, &machine_state)?;
        let authority =
            MobMachineFlowAuthorityToken::from_accepted_mob_machine_input(&authority_input)?;
        Ok((authority, machine_state))
    }

    async fn authorize_loop_command(
        &self,
        loop_instance_id: &LoopInstanceId,
        command: &MobMachineLoopIterationCommand,
    ) -> Result<(MobMachineFlowAuthorityToken, mob_dsl::MobMachineState), MobError> {
        let authority_input = command.authority_input(loop_instance_id);
        let machine_state = self
            .frame_kernel
            .preview_mob_machine_input(authority_input.clone())
            .await?;
        let authority =
            MobMachineFlowAuthorityToken::from_accepted_mob_machine_input(&authority_input)?;
        Ok((authority, machine_state))
    }

    /// Read the current frame snapshot, propagating errors from the store.
    async fn require_frame(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
    ) -> Result<crate::run::FrameSnapshot, MobError> {
        let run = self
            .run_store
            .get_run(run_id)
            .await?
            .ok_or_else(|| MobError::RunNotFound(run_id.clone()))?;
        run.frames.get(frame_id).cloned().ok_or_else(|| {
            MobError::Internal(format!("frame '{frame_id}' not found in run '{run_id}'"))
        })
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn root_outputs_from_run(run: &MobRun) -> IndexMap<StepId, serde_json::Value> {
    let mut outputs = run.root_step_outputs.clone();
    for iterations in run.loop_iteration_outputs.values() {
        for iteration in iterations {
            for (step_id, output) in iteration {
                outputs.insert(step_id.clone(), output.clone());
            }
        }
    }
    outputs
}

fn frame_is_body(kernel_state: &flow_frame::State) -> bool {
    kernel_state.frame_scope == flow_frame::FrameScope::Body
}

fn frame_loop_instance_id(kernel_state: &flow_frame::State) -> Option<LoopInstanceId> {
    (!kernel_state.loop_instance_id.as_str().is_empty())
        .then(|| kernel_state.loop_instance_id.clone())
}

fn frame_iteration(kernel_state: &flow_frame::State) -> Result<u64, MobError> {
    Ok(u64::from(kernel_state.iteration))
}

fn loop_parent_frame_id(kernel_state: &loop_iteration::State) -> Result<FrameId, MobError> {
    Ok(kernel_state.parent_frame_id.clone())
}

fn loop_parent_node_id(kernel_state: &loop_iteration::State) -> Result<FlowNodeId, MobError> {
    Ok(kernel_state.parent_node_id.clone())
}

fn loop_id_from_state(kernel_state: &loop_iteration::State) -> Result<LoopId, MobError> {
    Ok(kernel_state.loop_id.clone())
}

fn loop_depth(kernel_state: &loop_iteration::State) -> Result<u32, MobError> {
    Ok(kernel_state.depth)
}

fn usize_from_u64(value: u64, context: &str) -> Result<usize, MobError> {
    usize::try_from(value).map_err(|_| {
        MobError::Internal(format!(
            "{context} value {value} exceeds usize::MAX on this target"
        ))
    })
}

fn active_body_frame_id(kernel_state: &loop_iteration::State) -> Option<FrameId> {
    kernel_state.active_body_frame_id.clone()
}

fn running_node_ids(kernel_state: &flow_frame::State) -> Vec<FlowNodeId> {
    kernel_state
        .node_status
        .iter()
        .filter(|(_, status)| *status == &flow_frame::NodeRunStatus::Running)
        .map(|(id, _)| id.clone())
        .collect()
}

fn collect_frame_step_projections(
    frame_id: &FrameId,
    spec: &FrameSpec,
    kernel_state: &flow_frame::State,
) -> IndexMap<StepId, FrameStepProjection> {
    let mut projections = IndexMap::new();
    for (node_id, node_spec) in &spec.nodes {
        let FlowNodeSpec::Step(step_spec) = node_spec else {
            continue;
        };
        let Some(status) = kernel_state.node_status.get(node_id) else {
            continue;
        };
        let Some(step_status) = node_terminal_step_status(*status) else {
            continue;
        };
        merge_step_projection(
            &mut projections,
            FrameStepProjection {
                frame_id: frame_id.clone(),
                node_id: node_id.clone(),
                step_id: step_spec.step_id.clone(),
                step_status,
            },
        );
    }
    projections
}

fn node_terminal_step_status(status: flow_frame::NodeRunStatus) -> Option<StepRunStatus> {
    match status {
        flow_frame::NodeRunStatus::Completed => Some(StepRunStatus::Completed),
        flow_frame::NodeRunStatus::Skipped => Some(StepRunStatus::Skipped),
        flow_frame::NodeRunStatus::Failed => Some(StepRunStatus::Failed),
        _ => None,
    }
}

fn merge_step_projections(
    into: &mut IndexMap<StepId, FrameStepProjection>,
    updates: IndexMap<StepId, FrameStepProjection>,
) {
    for (_, projection) in updates {
        merge_step_projection(into, projection);
    }
}

fn merge_step_projection(
    into: &mut IndexMap<StepId, FrameStepProjection>,
    projection: FrameStepProjection,
) {
    let next_rank = step_status_rank(&projection.step_status);
    let step_id = projection.step_id.clone();
    match into.get_mut(&step_id) {
        Some(existing) if step_status_rank(&existing.step_status) >= next_rank => {}
        Some(existing) => *existing = projection,
        None => {
            into.insert(step_id, projection);
        }
    }
}

fn step_status_rank(status: &StepRunStatus) -> u8 {
    match status {
        StepRunStatus::Failed => 3,
        StepRunStatus::Completed => 2,
        StepRunStatus::Skipped => 1,
        StepRunStatus::Dispatched | StepRunStatus::Canceled => 0,
    }
}

fn preview_node_grant(
    state: &flow_run::State,
    command: MobMachineFlowRunCommand,
    authority: MobMachineFlowAuthorityToken,
    machine_state: &mob_dsl::MobMachineState,
    run_id: &RunId,
) -> Result<Option<PreviewedNodeGrant>, MobError> {
    let machine_input = command.authority_input(run_id);
    let outcome = run_transition_outcome(state, machine_state, run_id, command, authority)?;
    let frame_id = outcome.effects.iter().find_map(|effect| match effect {
        flow_run::Effect::GrantNodeSlot(payload) => Some(payload.frame_id.clone()),
        _ => None,
    });
    Ok(frame_id.map(|frame_id| PreviewedNodeGrant {
        frame_id,
        next_run_state: outcome.next_state,
        machine_input,
    }))
}

fn preview_body_frame_grant(
    state: &flow_run::State,
    command: MobMachineFlowRunCommand,
    authority: MobMachineFlowAuthorityToken,
    machine_state: &mob_dsl::MobMachineState,
    run_id: &RunId,
) -> Result<Option<PreviewedBodyFrameGrant>, MobError> {
    let machine_input = command.authority_input(run_id);
    let outcome = run_transition_outcome(state, machine_state, run_id, command, authority)?;
    let loop_instance_id = outcome.effects.iter().find_map(|effect| match effect {
        flow_run::Effect::GrantBodyFrameStart(payload) => Some(payload.loop_instance_id.clone()),
        _ => None,
    });
    Ok(
        loop_instance_id.map(|loop_instance_id| PreviewedBodyFrameGrant {
            loop_instance_id,
            next_run_state: outcome.next_state,
            machine_input,
        }),
    )
}

fn local_node_grant_candidate(run_state: &flow_run::State) -> Option<FrameId> {
    let active = run_state.active_node_count;
    let max_active = run_state.max_active_nodes;
    if max_active != 0 && active >= max_active {
        return None;
    }
    run_state
        .ready_frames
        .first()
        .cloned()
        .or_else(|| run_state.ready_frame_membership.iter().next().cloned())
}

fn local_body_frame_grant_candidate(run_state: &flow_run::State) -> Option<LoopInstanceId> {
    let active = run_state.active_frame_count;
    let max_active = run_state.max_active_frames;
    if max_active != 0 && active >= max_active {
        return None;
    }
    run_state
        .pending_body_frame_loops
        .first()
        .cloned()
        .or_else(|| {
            run_state
                .pending_body_frame_loop_membership
                .iter()
                .next()
                .cloned()
        })
}

fn machine_ready_frame_registration_candidates(
    machine_state: &mob_dsl::MobMachineState,
    run_id: &RunId,
) -> Vec<FrameId> {
    let run_key = mob_dsl::RunId::from(run_id.to_string());
    machine_state
        .frame_ready_queue
        .iter()
        .filter(|(frame_id, ready_queue)| {
            !ready_queue.is_empty()
                && machine_state.frame_run.get(*frame_id) == Some(&run_key)
                && machine_state.frame_phase.get(*frame_id) == Some(&mob_dsl::FrameStatus::Running)
                && !machine_state
                    .run_ready_frame_membership_flat
                    .contains(*frame_id)
        })
        .map(|(frame_id, _)| FrameId::from(frame_id.0.as_str()))
        .collect()
}

fn machine_frame_has_ready_nodes(
    machine_state: &mob_dsl::MobMachineState,
    frame_id: &FrameId,
) -> bool {
    let frame_key = mob_dsl::FrameId::from(frame_id.as_str());
    machine_state
        .frame_ready_queue
        .get(&frame_key)
        .is_some_and(|ready_queue| !ready_queue.is_empty())
        && machine_state.frame_phase.get(&frame_key) == Some(&mob_dsl::FrameStatus::Running)
}

fn machine_run_ready_frame_registered(
    machine_state: &mob_dsl::MobMachineState,
    run_id: &RunId,
    frame_id: &FrameId,
) -> bool {
    let run_key = mob_dsl::RunId::from(run_id.to_string());
    let frame_key = mob_dsl::FrameId::from(frame_id.as_str());
    machine_state
        .run_ready_frame_membership_flat
        .contains(&frame_key)
        || machine_state
            .run_ready_frame_membership
            .get(&run_key)
            .is_some_and(|frames| frames.contains(&frame_key))
}

fn machine_loop_pending_body_frame_registered(
    machine_state: &mob_dsl::MobMachineState,
    loop_instance_id: &LoopInstanceId,
) -> bool {
    machine_state
        .run_pending_body_frame_loop_membership_flat
        .contains(&mob_dsl::LoopInstanceId::from(loop_instance_id.as_str()))
}

fn machine_candidate_frame_terminal_status(
    machine_state: &mob_dsl::MobMachineState,
    frame_id: &FrameId,
) -> Option<flow_frame::FrameTerminalStatus> {
    let frame_key = mob_dsl::FrameId::from(frame_id.as_str());
    if machine_state.frame_phase.get(&frame_key) != Some(&mob_dsl::FrameStatus::Running) {
        return None;
    }
    let tracked_nodes = machine_state.frame_tracked_nodes.get(&frame_key)?;
    let node_status = machine_state.frame_node_status.get(&frame_key)?;
    if !tracked_nodes.iter().all(|node_id| {
        node_status
            .get(node_id)
            .copied()
            .is_some_and(machine_node_status_is_terminal)
    }) {
        return None;
    }
    if tracked_nodes
        .iter()
        .any(|node_id| node_status.get(node_id) == Some(&mob_dsl::NodeRunStatus::Failed))
    {
        Some(flow_frame::FrameTerminalStatus::Failed)
    } else if tracked_nodes
        .iter()
        .any(|node_id| node_status.get(node_id) == Some(&mob_dsl::NodeRunStatus::Canceled))
    {
        Some(flow_frame::FrameTerminalStatus::Canceled)
    } else {
        Some(flow_frame::FrameTerminalStatus::Completed)
    }
}

fn machine_node_status_is_terminal(status: mob_dsl::NodeRunStatus) -> bool {
    matches!(
        status,
        mob_dsl::NodeRunStatus::Completed
            | mob_dsl::NodeRunStatus::Failed
            | mob_dsl::NodeRunStatus::Skipped
            | mob_dsl::NodeRunStatus::Canceled
    )
}

fn root_terminal_phase(frame: &FrameSnapshot) -> Option<FlowFrameTerminalPhase> {
    match frame.kernel_state.phase {
        flow_frame::Phase::Completed => Some(FlowFrameTerminalPhase::Completed),
        flow_frame::Phase::Failed => Some(FlowFrameTerminalPhase::Failed),
        flow_frame::Phase::Canceled => Some(FlowFrameTerminalPhase::Canceled),
        _ => None,
    }
}

fn run_transition_outcome(
    state: &flow_run::State,
    machine_state: &mob_dsl::MobMachineState,
    run_id: &RunId,
    command: MobMachineFlowRunCommand,
    authority: MobMachineFlowAuthorityToken,
) -> Result<flow_run::Outcome, MobError> {
    apply_mob_machine_flow_run_command(state, machine_state, run_id, command, authority)
}

fn run_transition_state(
    state: &flow_run::State,
    machine_state: &mob_dsl::MobMachineState,
    run_id: &RunId,
    command: MobMachineFlowRunCommand,
    authority: MobMachineFlowAuthorityToken,
) -> Result<flow_run::State, MobError> {
    Ok(run_transition_outcome(state, machine_state, run_id, command, authority)?.next_state)
}

fn frame_transition_outcome(
    state: &flow_frame::State,
    machine_state: &mob_dsl::MobMachineState,
    command: MobMachineFlowFrameCommand,
    authority: MobMachineFlowAuthorityToken,
) -> Result<flow_frame::Outcome, MobError> {
    apply_mob_machine_flow_frame_command(state, machine_state, command, authority)
}

fn loop_transition_outcome(
    state: &loop_iteration::State,
    machine_state: &mob_dsl::MobMachineState,
    command: MobMachineLoopIterationCommand,
    authority: MobMachineFlowAuthorityToken,
) -> Result<loop_iteration::Outcome, MobError> {
    apply_mob_machine_loop_iteration_command(state, machine_state, command, authority)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FlowFrameEffectKind {
    NodeExecutionReleased,
}

impl FlowFrameEffectKind {
    fn parse(effect: &flow_frame::Effect) -> Option<Self> {
        match effect {
            flow_frame::Effect::NodeExecutionReleased(_) => Some(Self::NodeExecutionReleased),
            _ => None,
        }
    }
}

fn has_flow_frame_effect(effects: &[flow_frame::Effect], expected: FlowFrameEffectKind) -> bool {
    effects
        .iter()
        .any(|effect| FlowFrameEffectKind::parse(effect) == Some(expected))
}

fn effect_admit_step_node_id(effects: &[flow_frame::Effect]) -> Option<FlowNodeId> {
    effects.iter().find_map(|effect| match effect {
        flow_frame::Effect::AdmitStepWork(payload) => Some(payload.node_id.clone()),
        _ => None,
    })
}

fn effect_node_id(effects: &[flow_frame::Effect]) -> Result<Option<FlowNodeId>, MobError> {
    Ok(effects.iter().find_map(|effect| match effect {
        flow_frame::Effect::StartLoopNode(payload) => Some(payload.node_id.clone()),
        _ => None,
    }))
}

fn loop_current_iteration(state: &loop_iteration::State) -> Result<u64, MobError> {
    Ok(u64::from(state.current_iteration))
}

impl FlowFrameEngine {
    async fn initial_loop_snapshot(
        &self,
        loop_instance_id: &LoopInstanceId,
        loop_spec: &crate::definition::RepeatUntilSpec,
        parent_frame_id: &FrameId,
        parent_node_id: &FlowNodeId,
        depth: u32,
    ) -> Result<(LoopSnapshot, mob_dsl::MobMachineInput), MobError> {
        let initial = loop_iteration::initial_state();
        let command =
            MobMachineLoopIterationCommand::StartLoop(loop_iteration::inputs::StartLoop {
                loop_instance_id: loop_instance_id.clone(),
                max_iterations: loop_spec.max_iterations,
                parent_frame_id: parent_frame_id.clone(),
                parent_node_id: parent_node_id.clone(),
                loop_id: loop_spec.loop_id.clone(),
                depth,
            });
        let seed_input = crate::run::MobRun::create_loop_seed_input_for_start(
            loop_instance_id,
            parent_frame_id,
            parent_node_id,
            &loop_spec.loop_id,
            depth,
            loop_spec.max_iterations,
        );
        let machine_state = self
            .frame_kernel
            .preview_mob_machine_input(seed_input.clone())
            .await?;
        let authority = MobMachineFlowAuthorityToken::from_accepted_mob_machine_input(&seed_input)?;
        let start = loop_transition_outcome(&initial, &machine_state, command, authority)?;
        Ok((
            LoopSnapshot {
                kernel_state: start.next_state,
            },
            seed_input,
        ))
    }

    async fn initial_body_frame_snapshot(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        loop_instance_id: &LoopInstanceId,
        iteration: u64,
        spec: &FrameSpec,
    ) -> Result<
        (
            FrameSnapshot,
            mob_dsl::MobMachineInput,
            mob_dsl::MobMachineState,
        ),
        MobError,
    > {
        let initial = flow_frame::initial_state();
        let ordered = crate::runtime::flow_frame_engine::topological_order(spec)?;
        let start_input = crate::runtime::flow_frame_engine::build_start_body_frame_input(
            frame_id,
            loop_instance_id,
            iteration,
            spec,
            &ordered,
        );
        let command = MobMachineFlowFrameCommand::StartBodyFrame(start_input);
        let seed_input = crate::run::MobRun::create_frame_seed_input(
            run_id,
            frame_id,
            Some(loop_instance_id),
            u32::try_from(iteration).map_err(|_| {
                MobError::Internal("loop iteration exceeds u32 during body frame start".to_string())
            })?,
            crate::machines::mob_machine::FrameScope::Body,
            spec,
            &ordered,
        )?;
        let machine_state = self
            .frame_kernel
            .preview_mob_machine_input(seed_input.clone())
            .await?;
        let authority = MobMachineFlowAuthorityToken::from_accepted_mob_machine_input(&seed_input)?;
        let outcome = frame_transition_outcome(&initial, &machine_state, command, authority)?;
        Ok((
            FrameSnapshot {
                kernel_state: outcome.next_state,
            },
            seed_input,
            machine_state,
        ))
    }
}

fn mob_machine_seed_already_confirmed(error: &MobError) -> bool {
    error.to_string().contains("frame_seed_is_new")
}

fn machine_frame_scope(scope: flow_frame::FrameScope) -> crate::machines::mob_machine::FrameScope {
    match scope {
        flow_frame::FrameScope::Root => crate::machines::mob_machine::FrameScope::Root,
        flow_frame::FrameScope::Body => crate::machines::mob_machine::FrameScope::Body,
    }
}

fn project_frame_status(status: mob_dsl::FrameStatus) -> flow_frame::Phase {
    match status {
        mob_dsl::FrameStatus::Running => flow_frame::Phase::Running,
        mob_dsl::FrameStatus::Completed => flow_frame::Phase::Completed,
        mob_dsl::FrameStatus::Failed => flow_frame::Phase::Failed,
        mob_dsl::FrameStatus::Canceled => flow_frame::Phase::Canceled,
    }
}

fn validate_seed_confirmation_matches_snapshot(
    frame_id: &FrameId,
    existing: &flow_frame::State,
    machine_state: &mob_dsl::MobMachineState,
) -> Result<(), MobError> {
    let frame_key = mob_dsl::FrameId::from(frame_id.as_str());
    let Some(accepted_phase) = machine_state.frame_phase.get(&frame_key) else {
        return Err(MobError::Internal(format!(
            "MobMachine accepted CreateFrameSeed for existing frame '{frame_id}' without frame_phase projection"
        )));
    };
    if project_frame_status(*accepted_phase) != existing.phase {
        return Err(MobError::Internal(format!(
            "MobMachine CreateFrameSeed confirmation for existing frame '{frame_id}' projected phase {:?}, but recovered snapshot has {:?}",
            project_frame_status(*accepted_phase),
            existing.phase
        )));
    }

    let expected_statuses = existing
        .node_status
        .iter()
        .map(|(id, status)| {
            (
                mob_dsl::FlowNodeId::from(id.as_str()),
                machine_node_status(*status),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let Some(accepted_statuses) = machine_state.frame_node_status.get(&frame_key) else {
        return Err(MobError::Internal(format!(
            "MobMachine accepted CreateFrameSeed for existing frame '{frame_id}' without frame_node_status projection"
        )));
    };
    if accepted_statuses != &expected_statuses {
        return Err(MobError::Internal(format!(
            "MobMachine CreateFrameSeed confirmation for existing frame '{frame_id}' projected node statuses that differ from recovered snapshot"
        )));
    }

    let expected_ready_queue = existing
        .ready_queue
        .iter()
        .map(|id| mob_dsl::FlowNodeId::from(id.as_str()))
        .collect::<Vec<_>>();
    let Some(accepted_ready_queue) = machine_state.frame_ready_queue.get(&frame_key) else {
        return Err(MobError::Internal(format!(
            "MobMachine accepted CreateFrameSeed for existing frame '{frame_id}' without frame_ready_queue projection"
        )));
    };
    if accepted_ready_queue != &expected_ready_queue {
        return Err(MobError::Internal(format!(
            "MobMachine CreateFrameSeed confirmation for existing frame '{frame_id}' projected ready queue that differs from recovered snapshot"
        )));
    }

    Ok(())
}

fn validate_accepted_frame_command_witness(
    authority_input: &mob_dsl::MobMachineInput,
    machine_state: &mob_dsl::MobMachineState,
) -> Result<(), MobError> {
    let mob_dsl::MobMachineInput::AuthorizeFlowFrameReducerCommand {
        frame_id, command, ..
    } = authority_input
    else {
        return Ok(());
    };

    if matches!(
        command,
        mob_dsl::FlowFrameReducerCommandKind::AdmitNextReadyNode
            | mob_dsl::FlowFrameReducerCommandKind::CompleteNode
            | mob_dsl::FlowFrameReducerCommandKind::FailNode
            | mob_dsl::FlowFrameReducerCommandKind::SkipNode
            | mob_dsl::FlowFrameReducerCommandKind::CancelNode
    ) && (!machine_state.frame_node_status.contains_key(frame_id)
        || !machine_state.frame_ready_queue.contains_key(frame_id))
    {
        return Err(MobError::Internal(format!(
            "MobMachine accepted {command:?} for frame '{}' without machine-owned frame projection",
            frame_id.0
        )));
    }

    Ok(())
}

fn project_machine_node_status(status: mob_dsl::NodeRunStatus) -> flow_frame::NodeRunStatus {
    match status {
        mob_dsl::NodeRunStatus::Pending => flow_frame::NodeRunStatus::Pending,
        mob_dsl::NodeRunStatus::Ready => flow_frame::NodeRunStatus::Ready,
        mob_dsl::NodeRunStatus::Running => flow_frame::NodeRunStatus::Running,
        mob_dsl::NodeRunStatus::Completed => flow_frame::NodeRunStatus::Completed,
        mob_dsl::NodeRunStatus::Failed => flow_frame::NodeRunStatus::Failed,
        mob_dsl::NodeRunStatus::Skipped => flow_frame::NodeRunStatus::Skipped,
        mob_dsl::NodeRunStatus::Canceled => flow_frame::NodeRunStatus::Canceled,
    }
}

fn machine_node_status(status: flow_frame::NodeRunStatus) -> mob_dsl::NodeRunStatus {
    match status {
        flow_frame::NodeRunStatus::Pending => mob_dsl::NodeRunStatus::Pending,
        flow_frame::NodeRunStatus::Ready => mob_dsl::NodeRunStatus::Ready,
        flow_frame::NodeRunStatus::Running => mob_dsl::NodeRunStatus::Running,
        flow_frame::NodeRunStatus::Completed => mob_dsl::NodeRunStatus::Completed,
        flow_frame::NodeRunStatus::Failed => mob_dsl::NodeRunStatus::Failed,
        flow_frame::NodeRunStatus::Skipped => mob_dsl::NodeRunStatus::Skipped,
        flow_frame::NodeRunStatus::Canceled => mob_dsl::NodeRunStatus::Canceled,
    }
}

fn build_admit_next_ready_node_command_from_machine(
    machine_state: &mob_dsl::MobMachineState,
    frame_id: &FrameId,
) -> Option<MobMachineFlowFrameCommand> {
    let frame_key = mob_dsl::FrameId::from(frame_id.as_str());
    if machine_state.frame_phase.get(&frame_key) != Some(&mob_dsl::FrameStatus::Running) {
        return None;
    }
    let ready_queue = machine_state.frame_ready_queue.get(&frame_key)?;
    let node_id = ready_queue
        .first()
        .map(|candidate| FlowNodeId::from(candidate.0.as_str()))?;
    let ready_queue = ready_queue
        .iter()
        .filter(|candidate| *candidate != &mob_dsl::FlowNodeId::from(node_id.as_str()))
        .map(|candidate| FlowNodeId::from(candidate.0.as_str()))
        .collect();
    Some(MobMachineFlowFrameCommand::AdmitNextReadyNode(
        flow_frame::inputs::AdmitNextReadyNode {
            node_id,
            ready_queue,
        },
    ))
}

impl FlowFrameEngine {
    async fn register_ready_frame_if_needed(
        &self,
        run_id: &RunId,
        run_state: &flow_run::State,
        frame_id: &FrameId,
        _frame_state: &flow_frame::State,
    ) -> Result<Option<FlowFrameLoopStorePlan>, MobError> {
        let current_machine_state = self.frame_kernel.current_mob_machine_state().await?;
        if !machine_frame_has_ready_nodes(&current_machine_state, frame_id) {
            return Ok(None);
        }
        if machine_run_ready_frame_registered(&current_machine_state, run_id, frame_id) {
            return Ok(None);
        }
        let command =
            MobMachineFlowRunCommand::RegisterReadyFrame(flow_run::inputs::RegisterReadyFrame {
                frame_id: frame_id.clone(),
            });
        let machine_inputs = vec![command.authority_input(run_id)];
        let (authority, machine_state) = self.authorize_run_command(run_id, &command).await?;
        Ok(Some(FlowFrameLoopStorePlan::RunStateOnly {
            machine_inputs,
            expected_run_state: run_state.clone(),
            next_run_state: run_transition_state(
                run_state,
                &machine_state,
                run_id,
                command,
                authority,
            )?,
        }))
    }

    async fn release_node_execution(
        &self,
        run_id: &RunId,
        run_state: &flow_run::State,
        frame_id: &FrameId,
    ) -> Result<FlowFrameLoopStorePlan, MobError> {
        let command = MobMachineFlowRunCommand::NodeExecutionReleased(
            flow_run::inputs::NodeExecutionReleased {
                frame_id: frame_id.clone(),
            },
        );
        let machine_inputs = vec![command.authority_input(run_id)];
        let (authority, machine_state) = self.authorize_run_command(run_id, &command).await?;
        Ok(FlowFrameLoopStorePlan::RunStateOnly {
            machine_inputs,
            expected_run_state: run_state.clone(),
            next_run_state: run_transition_state(
                run_state,
                &machine_state,
                run_id,
                command,
                authority,
            )?,
        })
    }

    async fn seal_frame_if_ready(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        current_frame: &FrameSnapshot,
        frame_spec: &FrameSpec,
    ) -> Result<Option<FlowFrameLoopStorePlan>, MobError> {
        let _ = frame_spec;
        let current_machine_state = self.frame_kernel.current_mob_machine_state().await?;
        let Some(terminal_status) =
            machine_candidate_frame_terminal_status(&current_machine_state, frame_id)
        else {
            return Ok(None);
        };
        let command = MobMachineFlowFrameCommand::SealFrame(flow_frame::inputs::SealFrame {
            terminal_status,
        });
        let machine_inputs = vec![command.authority_input(frame_id)];
        let (authority, machine_state) = self
            .authorize_frame_command(run_id, frame_id, &command)
            .await?;
        let outcome = frame_transition_outcome(
            &current_frame.kernel_state,
            &machine_state,
            command,
            authority,
        )?;
        Ok(Some(FlowFrameLoopStorePlan::SealFrame {
            machine_inputs,
            frame_id: frame_id.clone(),
            expected_frame: current_frame.clone(),
            next_frame: FrameSnapshot {
                kernel_state: outcome.next_state,
            },
        }))
    }
}

fn pending_until_obligation(
    loop_snapshot: &LoopSnapshot,
) -> Result<Option<FlowLoopUntilEvaluationObligation>, MobError> {
    if loop_snapshot.kernel_state.phase != loop_iteration::Phase::Running
        || loop_snapshot.kernel_state.stage
            != loop_iteration::LoopIterationStage::AwaitingUntilEvaluation
    {
        return Ok(None);
    }
    Ok(Some(FlowLoopUntilEvaluationObligation {
        loop_instance_id: loop_snapshot.kernel_state.loop_instance_id.clone(),
        iteration: loop_snapshot.kernel_state.last_completed_iteration,
        parent_frame_id: loop_snapshot.kernel_state.parent_frame_id.clone(),
        parent_node_id: loop_snapshot.kernel_state.parent_node_id.clone(),
        loop_id: loop_snapshot.kernel_state.loop_id.clone(),
    }))
}

fn loop_until_evaluation_obligation_from_effect(
    effect: &loop_iteration::Effect,
) -> Result<FlowLoopUntilEvaluationObligation, MobError> {
    match effect {
        loop_iteration::Effect::EvaluateUntilCondition(payload) => {
            Ok(FlowLoopUntilEvaluationObligation {
                loop_instance_id: payload.loop_instance_id.clone(),
                iteration: payload.iteration,
                parent_frame_id: payload.parent_frame_id.clone(),
                parent_node_id: payload.parent_node_id.clone(),
                loop_id: payload.loop_id.clone(),
            })
        }
        other => Err(MobError::Internal(format!(
            "expected EvaluateUntilCondition effect, got '{other:?}'"
        ))),
    }
}

impl FlowFrameEngine {
    async fn acknowledge_node_grant(
        &self,
        run_id: &RunId,
        expected_run_state: &flow_run::State,
        grant: &PreviewedNodeGrant,
        current_frame: &FrameSnapshot,
        frame_spec: &FrameSpec,
        depth_limit: FrameDepthLimit,
    ) -> Result<FlowFrameLoopDecision, MobError> {
        let frame_id = &grant.frame_id;
        let machine_state = self.frame_kernel.current_mob_machine_state().await?;
        let Some(admit_command) =
            build_admit_next_ready_node_command_from_machine(&machine_state, frame_id)
        else {
            return Ok(FlowFrameLoopDecision {
                store_plan: None,
                follow_up: Vec::new(),
            });
        };
        let (admit_authority, admit_machine_state) = self
            .authorize_frame_command(run_id, frame_id, &admit_command)
            .await?;
        let mut machine_inputs = vec![
            grant.machine_input.clone(),
            admit_command.authority_input(frame_id),
        ];
        let admit_outcome = frame_transition_outcome(
            &current_frame.kernel_state,
            &admit_machine_state,
            admit_command,
            admit_authority,
        )?;
        let next_frame = FrameSnapshot {
            kernel_state: admit_outcome.next_state.clone(),
        };
        let mut next_run_state = grant.next_run_state.clone();

        if has_flow_frame_effect(
            &admit_outcome.effects,
            FlowFrameEffectKind::NodeExecutionReleased,
        ) {
            let command = MobMachineFlowRunCommand::NodeExecutionReleased(
                flow_run::inputs::NodeExecutionReleased {
                    frame_id: frame_id.clone(),
                },
            );
            machine_inputs.push(command.authority_input(run_id));
            let (authority, machine_state) = self.authorize_run_command(run_id, &command).await?;
            next_run_state =
                run_transition_state(&next_run_state, &machine_state, run_id, command, authority)?;
        }

        if machine_frame_has_ready_nodes(&admit_machine_state, frame_id) {
            let command = MobMachineFlowRunCommand::RegisterReadyFrame(
                flow_run::inputs::RegisterReadyFrame {
                    frame_id: frame_id.clone(),
                },
            );
            let authority_input = command.authority_input(run_id);
            let mut staged_inputs = machine_inputs.clone();
            staged_inputs.push(authority_input.clone());
            let machine_state = self
                .frame_kernel
                .preview_mob_machine_inputs(&staged_inputs)
                .await?;
            let authority =
                MobMachineFlowAuthorityToken::from_accepted_mob_machine_input(&authority_input)?;
            machine_inputs.push(authority_input);
            next_run_state =
                run_transition_state(&next_run_state, &machine_state, run_id, command, authority)?;
        }

        if let Some(node_id) = effect_node_id(&admit_outcome.effects)? {
            let loop_spec = match frame_spec.nodes.get(&node_id) {
                Some(FlowNodeSpec::RepeatUntil(spec)) => spec.clone(),
                _ => {
                    return Err(MobError::Internal(format!(
                        "frame '{frame_id}' admitted non-loop node '{node_id}' as loop work"
                    )));
                }
            };
            let body_depth = depth_limit.current_depth + 1;
            if depth_limit.max_depth > 0 && body_depth > depth_limit.max_depth {
                return Err(MobError::FrameDepthLimitExceeded {
                    loop_id: loop_spec.loop_id,
                    max_frame_depth: depth_limit.max_depth,
                    current_depth: body_depth - 1,
                });
            }
            let loop_instance_id = LoopInstanceId::from(format!("{frame_id}::{node_id}").as_str());
            let (initial_loop, loop_seed_input) = self
                .initial_loop_snapshot(
                    &loop_instance_id,
                    &loop_spec,
                    frame_id,
                    &node_id,
                    body_depth,
                )
                .await?;
            machine_inputs.push(loop_seed_input);
            if state_is_awaiting_body_frame(&initial_loop.kernel_state) {
                let command = MobMachineFlowRunCommand::RegisterPendingBodyFrame(
                    flow_run::inputs::RegisterPendingBodyFrame {
                        loop_instance_id: loop_instance_id.clone(),
                        depth: body_depth,
                    },
                );
                let authority_input = command.authority_input(run_id);
                let mut staged_inputs = machine_inputs.clone();
                staged_inputs.push(authority_input.clone());
                let machine_state = self
                    .frame_kernel
                    .preview_mob_machine_inputs(&staged_inputs)
                    .await?;
                let authority = MobMachineFlowAuthorityToken::from_accepted_mob_machine_input(
                    &authority_input,
                )?;
                machine_inputs.push(authority_input);
                next_run_state = run_transition_state(
                    &next_run_state,
                    &machine_state,
                    run_id,
                    command,
                    authority,
                )?;
            }
            return Ok(FlowFrameLoopDecision {
                store_plan: Some(FlowFrameLoopStorePlan::StartLoop {
                    machine_inputs,
                    loop_instance_id,
                    expected_run_state: expected_run_state.clone(),
                    next_run_state,
                    frame_id: frame_id.clone(),
                    expected_frame: current_frame.clone(),
                    next_frame,
                    initial_loop,
                }),
                follow_up: Vec::new(),
            });
        }

        let mut follow_up = Vec::new();
        if let Some(node_id) = effect_admit_step_node_id(&admit_outcome.effects) {
            let step_id = match frame_spec.nodes.get(&node_id) {
                Some(FlowNodeSpec::Step(step)) => step.step_id.clone(),
                _ => {
                    return Err(MobError::Internal(format!(
                        "frame '{frame_id}' admitted non-step node '{node_id}' as step work"
                    )));
                }
            };
            follow_up.push(FlowFrameLoopWork::SpawnStep {
                frame_id: frame_id.clone(),
                node_id,
                step_id,
            });
        } else {
            follow_up.push(FlowFrameLoopWork::RevisitFrame {
                frame_id: frame_id.clone(),
            });
        }
        Ok(FlowFrameLoopDecision {
            store_plan: Some(FlowFrameLoopStorePlan::GrantNodeSlot {
                machine_inputs,
                expected_run_state: expected_run_state.clone(),
                next_run_state,
                frame_id: frame_id.clone(),
                expected_frame: current_frame.clone(),
                next_frame,
            }),
            follow_up,
        })
    }

    async fn acknowledge_body_frame_start(
        &self,
        run_id: &RunId,
        expected_run_state: &flow_run::State,
        grant: &PreviewedBodyFrameGrant,
        loop_snapshot: &LoopSnapshot,
        loop_spec: &crate::definition::RepeatUntilSpec,
    ) -> Result<FlowFrameLoopDecision, MobError> {
        let iteration = loop_current_iteration(&loop_snapshot.kernel_state)?;
        let frame_id =
            FrameId::from(format!("{}::iter-{iteration}", grant.loop_instance_id).as_str());
        let (initial_frame, body_frame_seed_input, body_frame_seed_machine_state) = self
            .initial_body_frame_snapshot(
                run_id,
                &frame_id,
                &grant.loop_instance_id,
                iteration,
                &loop_spec.body,
            )
            .await?;
        let body_frame_started = MobMachineLoopIterationCommand::BodyFrameStarted(
            loop_iteration::inputs::BodyFrameStarted {
                loop_instance_id: grant.loop_instance_id.clone(),
                frame_id: frame_id.clone(),
                iteration: iteration as u32,
            },
        );
        let body_frame_started_authority =
            MobMachineFlowAuthorityToken::from_accepted_mob_machine_body_frame_seed(
                &body_frame_seed_input,
            )?;
        let body_frame_outcome = loop_transition_outcome(
            &loop_snapshot.kernel_state,
            &body_frame_seed_machine_state,
            body_frame_started,
            body_frame_started_authority,
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
        let mut machine_inputs = vec![grant.machine_input.clone(), body_frame_seed_input];
        if machine_frame_has_ready_nodes(&body_frame_seed_machine_state, &frame_id) {
            let command = MobMachineFlowRunCommand::RegisterReadyFrame(
                flow_run::inputs::RegisterReadyFrame {
                    frame_id: frame_id.clone(),
                },
            );
            let authority_input = command.authority_input(run_id);
            let mut staged_inputs = machine_inputs.clone();
            staged_inputs.push(authority_input.clone());
            let machine_state = self
                .frame_kernel
                .preview_mob_machine_inputs(&staged_inputs)
                .await?;
            let authority =
                MobMachineFlowAuthorityToken::from_accepted_mob_machine_input(&authority_input)?;
            machine_inputs.push(authority_input);
            next_run_state =
                run_transition_state(&next_run_state, &machine_state, run_id, command, authority)?;
        }
        Ok(FlowFrameLoopDecision {
            store_plan: Some(FlowFrameLoopStorePlan::GrantBodyFrameStart {
                machine_inputs,
                loop_instance_id: grant.loop_instance_id.clone(),
                expected_loop: loop_snapshot.clone(),
                next_loop,
                frame_id: frame_id.clone(),
                initial_frame,
                ledger_entry,
                expected_run_state: expected_run_state.clone(),
                next_run_state,
            }),
            follow_up: vec![FlowFrameLoopWork::RevisitFrame { frame_id }],
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BodyFrameTerminalProjection {
    Completed,
    Failed,
    Canceled,
}

impl BodyFrameTerminalProjection {
    fn from_frame_phase(phase: FlowFrameTerminalPhase) -> Self {
        match phase {
            FlowFrameTerminalPhase::Completed => Self::Completed,
            FlowFrameTerminalPhase::Failed => Self::Failed,
            FlowFrameTerminalPhase::Canceled => Self::Canceled,
        }
    }

    fn loop_input(
        self,
        loop_instance_id: &LoopInstanceId,
        iteration: u32,
    ) -> MobMachineLoopIterationCommand {
        match self {
            Self::Completed => MobMachineLoopIterationCommand::BodyFrameCompleted(
                loop_iteration::inputs::BodyFrameCompleted {
                    loop_instance_id: loop_instance_id.clone(),
                    iteration,
                },
            ),
            Self::Failed => MobMachineLoopIterationCommand::BodyFrameFailed(
                loop_iteration::inputs::BodyFrameFailed {
                    loop_instance_id: loop_instance_id.clone(),
                    iteration,
                },
            ),
            Self::Canceled => MobMachineLoopIterationCommand::BodyFrameCanceled(
                loop_iteration::inputs::BodyFrameCanceled {
                    loop_instance_id: loop_instance_id.clone(),
                    iteration,
                },
            ),
        }
    }
}

impl FlowFrameEngine {
    async fn advance_body_frame_after_seal(
        &self,
        run_id: &RunId,
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
        let body_terminal = BodyFrameTerminalProjection::from_frame_phase(terminal_phase(
            &body_frame.kernel_state,
        )?);
        let frame_terminated =
            MobMachineFlowRunCommand::FrameTerminated(flow_run::inputs::FrameTerminated {
                frame_id: body_frame_id.clone(),
            });
        let mut machine_inputs = vec![frame_terminated.authority_input(run_id)];
        let (frame_terminated_authority, frame_terminated_machine_state) = self
            .authorize_run_command(run_id, &frame_terminated)
            .await?;
        let next_run_state = run_transition_state(
            run_state,
            &frame_terminated_machine_state,
            run_id,
            frame_terminated,
            frame_terminated_authority,
        )?;
        let loop_command = body_terminal.loop_input(&loop_instance_id, iteration as u32);
        machine_inputs.push(loop_command.authority_input(&loop_instance_id));
        let (loop_authority, loop_machine_state) = self
            .authorize_loop_command(&loop_instance_id, &loop_command)
            .await?;
        let loop_outcome = loop_transition_outcome(
            &loop_snapshot.kernel_state,
            &loop_machine_state,
            loop_command,
            loop_authority,
        )?;
        let next_loop = LoopSnapshot {
            kernel_state: loop_outcome.next_state.clone(),
        };

        match terminal_phase(&body_frame.kernel_state)? {
            FlowFrameTerminalPhase::Completed => {
                let request = loop_outcome
                .effects
                .iter()
                .find(|effect| matches!(effect, loop_iteration::Effect::EvaluateUntilCondition(_)))
                .ok_or_else(|| {
                    MobError::Internal(format!(
                        "loop '{loop_instance_id}' did not emit EvaluateUntilCondition after body-frame completion"
                    ))
                })?;
                let obligation = loop_until_evaluation_obligation_from_effect(request)?;
                Ok(Some(FlowFrameLoopDecision {
                    store_plan: Some(FlowFrameLoopStorePlan::CompleteBodyFrame {
                        machine_inputs,
                        loop_instance_id,
                        expected_loop: loop_snapshot.clone(),
                        next_loop,
                        frame_id: body_frame_id.clone(),
                        expected_frame: body_frame.clone(),
                        next_frame: body_frame.clone(),
                        expected_run_state: run_state.clone(),
                        next_run_state,
                    }),
                    follow_up: vec![FlowFrameLoopWork::EvaluateUntil { obligation }],
                }))
            }
            FlowFrameTerminalPhase::Failed | FlowFrameTerminalPhase::Canceled => {
                let parent_frame = parent_frame.ok_or_else(|| {
                MobError::Internal(format!(
                    "body-frame '{body_frame_id}' terminalization requires parent frame snapshot"
                ))
            })?;
                let loop_terminal = first_matching_loop_terminal_effect(&loop_outcome.effects)
                .ok_or_else(|| {
                    MobError::Internal(format!(
                        "loop '{loop_instance_id}' did not emit a terminal effect after '{body_terminal:?}'"
                    ))
                })?;
                let parent_frame_id = loop_parent_frame_id(&loop_outcome.next_state)?;
                let parent_node_id = loop_parent_node_id(&loop_outcome.next_state)?;
                Ok(Some(
                    self.build_complete_loop_decision(CompleteLoopDecisionRequest {
                        run_id,
                        loop_terminal,
                        machine_inputs,
                        expected_run_state: run_state,
                        next_run_state,
                        loop_instance_id: &loop_instance_id,
                        expected_loop: loop_snapshot,
                        next_loop,
                        parent_frame_id: &parent_frame_id,
                        expected_parent_frame: parent_frame,
                        parent_node_id: &parent_node_id,
                    })
                    .await?,
                ))
            }
        }
    }

    async fn resolve_until_feedback_decision(
        &self,
        run_id: &RunId,
        run_state: &flow_run::State,
        loop_snapshot: &LoopSnapshot,
        parent_frame: &FrameSnapshot,
        obligation: FlowLoopUntilEvaluationObligation,
        until_met: bool,
    ) -> Result<(FlowFrameLoopDecision, LoopUntilConditionFeedbackProjection), MobError> {
        let feedback_input = if until_met {
            MobMachineLoopIterationCommand::UntilConditionMet(
                loop_iteration::inputs::UntilConditionMet {
                    loop_instance_id: obligation.loop_instance_id.clone(),
                    iteration: obligation.iteration,
                },
            )
        } else {
            MobMachineLoopIterationCommand::UntilConditionFailed(
                loop_iteration::inputs::UntilConditionFailed {
                    loop_instance_id: obligation.loop_instance_id.clone(),
                    iteration: obligation.iteration,
                },
            )
        };
        let (feedback_authority, feedback_machine_state) = self
            .authorize_loop_command(&obligation.loop_instance_id, &feedback_input)
            .await?;
        let mut machine_inputs = vec![feedback_input.authority_input(&obligation.loop_instance_id)];
        let feedback = loop_transition_outcome(
            &loop_snapshot.kernel_state,
            &feedback_machine_state,
            feedback_input,
            feedback_authority,
        )?;
        let next_loop = LoopSnapshot {
            kernel_state: feedback.next_state.clone(),
        };
        let until_feedback = LoopUntilConditionFeedbackProjection {
            loop_instance_id: obligation.loop_instance_id.clone(),
            iteration: obligation.iteration,
            until_met,
        };

        if let Some(depth) = maybe_effect_request_body_frame_start_depth(&feedback.effects) {
            let mut next_run_state = run_state.clone();
            if !machine_loop_pending_body_frame_registered(
                &feedback_machine_state,
                &obligation.loop_instance_id,
            ) {
                let command = MobMachineFlowRunCommand::RegisterPendingBodyFrame(
                    flow_run::inputs::RegisterPendingBodyFrame {
                        loop_instance_id: obligation.loop_instance_id.clone(),
                        depth,
                    },
                );
                machine_inputs.push(command.authority_input(run_id));
                let (authority, machine_state) =
                    self.authorize_run_command(run_id, &command).await?;
                next_run_state = run_transition_state(
                    &next_run_state,
                    &machine_state,
                    run_id,
                    command,
                    authority,
                )?;
            }
            return Ok((
                FlowFrameLoopDecision {
                    store_plan: Some(FlowFrameLoopStorePlan::LoopRequestBodyFrame {
                        machine_inputs,
                        loop_instance_id: obligation.loop_instance_id,
                        expected_loop: loop_snapshot.clone(),
                        next_loop,
                        expected_run_state: run_state.clone(),
                        next_run_state,
                    }),
                    follow_up: Vec::new(),
                },
                until_feedback,
            ));
        }

        let loop_terminal = first_matching_loop_terminal_effect(&feedback.effects).ok_or_else(|| {
        MobError::Internal(format!(
            "until feedback for loop '{}' produced neither RequestBodyFrameStart nor terminal effect",
            obligation.loop_instance_id
        ))
    })?;
        let decision = self
            .build_complete_loop_decision(CompleteLoopDecisionRequest {
                run_id,
                loop_terminal,
                machine_inputs,
                expected_run_state: run_state,
                next_run_state: run_state.clone(),
                loop_instance_id: &obligation.loop_instance_id,
                expected_loop: loop_snapshot,
                next_loop,
                parent_frame_id: &obligation.parent_frame_id,
                expected_parent_frame: parent_frame,
                parent_node_id: &obligation.parent_node_id,
            })
            .await?;
        Ok((decision, until_feedback))
    }

    async fn recover_terminal_loop_projection(
        &self,
        run_id: &RunId,
        run_state: &flow_run::State,
        loop_snapshot: &LoopSnapshot,
        parent_frame: &FrameSnapshot,
    ) -> Result<Option<FlowFrameLoopDecision>, MobError> {
        let Some(loop_terminal) = loop_terminal_effect(&loop_snapshot.kernel_state.phase) else {
            return Ok(None);
        };
        let loop_instance_id = loop_snapshot.kernel_state.loop_instance_id.clone();
        let parent_frame_id = loop_parent_frame_id(&loop_snapshot.kernel_state)?;
        let parent_node_id = loop_parent_node_id(&loop_snapshot.kernel_state)?;
        Ok(Some(
            self.build_complete_loop_decision(CompleteLoopDecisionRequest {
                run_id,
                loop_terminal,
                machine_inputs: Vec::new(),
                expected_run_state: run_state,
                next_run_state: run_state.clone(),
                loop_instance_id: &loop_instance_id,
                expected_loop: loop_snapshot,
                next_loop: loop_snapshot.clone(),
                parent_frame_id: &parent_frame_id,
                expected_parent_frame: parent_frame,
                parent_node_id: &parent_node_id,
            })
            .await?,
        ))
    }

    async fn recover_pending_body_frame_request(
        &self,
        run_id: &RunId,
        run_state: &flow_run::State,
        loop_snapshot: &LoopSnapshot,
    ) -> Result<Option<FlowFrameLoopDecision>, MobError> {
        if loop_snapshot.kernel_state.phase != loop_iteration::Phase::Running
            || !state_is_awaiting_body_frame(&loop_snapshot.kernel_state)
            || active_body_frame_id(&loop_snapshot.kernel_state).is_some()
        {
            return Ok(None);
        }

        let loop_instance_id = loop_snapshot.kernel_state.loop_instance_id.clone();
        let machine_state = self.frame_kernel.current_mob_machine_state().await?;
        if machine_loop_pending_body_frame_registered(&machine_state, &loop_instance_id) {
            return Ok(None);
        }

        let depth = loop_snapshot.kernel_state.depth;
        let command = MobMachineFlowRunCommand::RegisterPendingBodyFrame(
            flow_run::inputs::RegisterPendingBodyFrame {
                loop_instance_id,
                depth,
            },
        );
        let machine_inputs = vec![command.authority_input(run_id)];
        let (authority, machine_state) = self.authorize_run_command(run_id, &command).await?;
        let next_run_state =
            run_transition_state(run_state, &machine_state, run_id, command, authority)?;
        Ok(Some(FlowFrameLoopDecision {
            store_plan: Some(FlowFrameLoopStorePlan::RunStateOnly {
                machine_inputs,
                expected_run_state: run_state.clone(),
                next_run_state,
            }),
            follow_up: Vec::new(),
        }))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoopTerminalEffect {
    Completed,
    Exhausted,
    Failed,
    Canceled,
}

impl std::fmt::Display for LoopTerminalEffect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Completed => "LoopCompleted",
            Self::Exhausted => "LoopExhausted",
            Self::Failed => "LoopFailed",
            Self::Canceled => "LoopCanceled",
        })
    }
}

struct CompleteLoopDecisionRequest<'a> {
    run_id: &'a RunId,
    loop_terminal: LoopTerminalEffect,
    machine_inputs: Vec<mob_dsl::MobMachineInput>,
    expected_run_state: &'a flow_run::State,
    next_run_state: flow_run::State,
    loop_instance_id: &'a LoopInstanceId,
    expected_loop: &'a LoopSnapshot,
    next_loop: LoopSnapshot,
    parent_frame_id: &'a FrameId,
    expected_parent_frame: &'a FrameSnapshot,
    parent_node_id: &'a FlowNodeId,
}

impl FlowFrameEngine {
    async fn build_complete_loop_decision(
        &self,
        request: CompleteLoopDecisionRequest<'_>,
    ) -> Result<FlowFrameLoopDecision, MobError> {
        let CompleteLoopDecisionRequest {
            run_id,
            loop_terminal,
            mut machine_inputs,
            expected_run_state,
            mut next_run_state,
            loop_instance_id,
            expected_loop,
            next_loop,
            parent_frame_id,
            expected_parent_frame,
            parent_node_id,
        } = request;
        let parent_input = loop_parent_input(loop_terminal, parent_node_id);
        machine_inputs.push(parent_input.authority_input(parent_frame_id));
        let (parent_authority, parent_machine_state) = self
            .authorize_frame_command(run_id, parent_frame_id, &parent_input)
            .await?;
        let parent_outcome = frame_transition_outcome(
            &expected_parent_frame.kernel_state,
            &parent_machine_state,
            parent_input,
            parent_authority,
        )?;
        let next_parent_frame = FrameSnapshot {
            kernel_state: parent_outcome.next_state,
        };
        if machine_frame_has_ready_nodes(&parent_machine_state, parent_frame_id) {
            let command = MobMachineFlowRunCommand::RegisterReadyFrame(
                flow_run::inputs::RegisterReadyFrame {
                    frame_id: parent_frame_id.clone(),
                },
            );
            machine_inputs.push(command.authority_input(run_id));
            let (authority, machine_state) = self.authorize_run_command(run_id, &command).await?;
            next_run_state =
                run_transition_state(&next_run_state, &machine_state, run_id, command, authority)?;
        }
        Ok(FlowFrameLoopDecision {
            store_plan: Some(FlowFrameLoopStorePlan::CompleteLoop {
                machine_inputs,
                loop_instance_id: loop_instance_id.clone(),
                expected_loop: expected_loop.clone(),
                next_loop,
                frame_id: parent_frame_id.clone(),
                expected_frame: expected_parent_frame.clone(),
                next_frame: next_parent_frame,
                expected_run_state: expected_run_state.clone(),
                next_run_state,
            }),
            follow_up: vec![FlowFrameLoopWork::RevisitFrame {
                frame_id: parent_frame_id.clone(),
            }],
        })
    }
}

fn loop_parent_input(
    loop_terminal: LoopTerminalEffect,
    parent_node_id: &FlowNodeId,
) -> MobMachineFlowFrameCommand {
    match loop_terminal {
        LoopTerminalEffect::Completed => {
            MobMachineFlowFrameCommand::CompleteNode(flow_frame::inputs::CompleteNode {
                node_id: parent_node_id.clone(),
            })
        }
        LoopTerminalEffect::Exhausted | LoopTerminalEffect::Failed => {
            MobMachineFlowFrameCommand::FailNode(flow_frame::inputs::FailNode {
                node_id: parent_node_id.clone(),
            })
        }
        LoopTerminalEffect::Canceled => {
            MobMachineFlowFrameCommand::CancelNode(flow_frame::inputs::CancelNode {
                node_id: parent_node_id.clone(),
            })
        }
    }
}

#[allow(clippy::type_complexity)]
fn build_frame_start_payload(
    frame_id: &FrameId,
    spec: &FrameSpec,
    ordered: &[FlowNodeId],
) -> (
    std::collections::BTreeSet<FlowNodeId>,
    Vec<FlowNodeId>,
    std::collections::BTreeMap<FlowNodeId, flow_frame::FlowNodeKind>,
    std::collections::BTreeMap<FlowNodeId, Vec<FlowNodeId>>,
    std::collections::BTreeMap<FlowNodeId, flow_frame::DependencyMode>,
    std::collections::BTreeMap<FlowNodeId, Option<crate::ids::BranchId>>,
) {
    let ordered_kv: Vec<FlowNodeId> = ordered.to_vec();
    let tracked: std::collections::BTreeSet<FlowNodeId> = ordered.iter().cloned().collect();
    let mut node_kind = std::collections::BTreeMap::new();
    let mut node_deps = std::collections::BTreeMap::new();
    let mut node_dep_modes = std::collections::BTreeMap::new();
    let mut node_branches = std::collections::BTreeMap::new();

    for (node_id, node_spec) in &spec.nodes {
        let k = node_id.clone();
        match node_spec {
            FlowNodeSpec::Step(s) => {
                node_kind.insert(k.clone(), flow_frame::FlowNodeKind::Step);
                node_deps.insert(k.clone(), s.depends_on.clone());
                node_dep_modes.insert(k.clone(), dep_mode_kv(&s.depends_on_mode));
                node_branches.insert(k.clone(), s.branch.clone());
            }
            FlowNodeSpec::RepeatUntil(l) => {
                node_kind.insert(k.clone(), flow_frame::FlowNodeKind::Loop);
                node_deps.insert(k.clone(), l.depends_on.clone());
                node_dep_modes.insert(k.clone(), dep_mode_kv(&l.depends_on_mode));
                node_branches.insert(k.clone(), None);
            }
        }
    }
    let _ = frame_id;
    (
        tracked,
        ordered_kv,
        node_kind,
        node_deps,
        node_dep_modes,
        node_branches,
    )
}

fn build_start_root_frame_input(
    frame_id: &FrameId,
    spec: &FrameSpec,
    ordered: &[FlowNodeId],
) -> flow_frame::inputs::StartRootFrame {
    let (
        tracked_nodes,
        ordered_nodes,
        node_kind,
        node_dependencies,
        node_dependency_modes,
        node_branches,
    ) = build_frame_start_payload(frame_id, spec, ordered);
    flow_frame::inputs::StartRootFrame {
        frame_id: frame_id.clone(),
        tracked_nodes,
        ordered_nodes,
        node_kind,
        node_dependencies,
        node_dependency_modes,
        node_branches,
    }
}

fn build_start_body_frame_input(
    frame_id: &FrameId,
    loop_instance_id: &LoopInstanceId,
    iteration: u64,
    spec: &FrameSpec,
    ordered: &[FlowNodeId],
) -> flow_frame::inputs::StartBodyFrame {
    let (
        tracked_nodes,
        ordered_nodes,
        node_kind,
        node_dependencies,
        node_dependency_modes,
        node_branches,
    ) = build_frame_start_payload(frame_id, spec, ordered);
    flow_frame::inputs::StartBodyFrame {
        frame_id: frame_id.clone(),
        loop_instance_id: loop_instance_id.clone(),
        iteration: iteration as u32,
        tracked_nodes,
        ordered_nodes,
        node_kind,
        node_dependencies,
        node_dependency_modes,
        node_branches,
    }
}

fn dep_mode_kv(mode: &crate::definition::DependencyMode) -> flow_frame::DependencyMode {
    match mode {
        crate::definition::DependencyMode::All => flow_frame::DependencyMode::All,
        crate::definition::DependencyMode::Any => flow_frame::DependencyMode::Any,
    }
}

fn topological_order(spec: &FrameSpec) -> Result<Vec<FlowNodeId>, MobError> {
    let mut in_degree: std::collections::BTreeMap<FlowNodeId, usize> =
        std::collections::BTreeMap::new();
    let mut outgoing: std::collections::BTreeMap<FlowNodeId, Vec<FlowNodeId>> =
        std::collections::BTreeMap::new();

    for node_id in spec.nodes.keys() {
        in_degree.insert(node_id.clone(), 0);
        outgoing.entry(node_id.clone()).or_default();
    }

    for (node_id, node_spec) in &spec.nodes {
        let deps = match node_spec {
            FlowNodeSpec::Step(s) => s.depends_on.clone(),
            FlowNodeSpec::RepeatUntil(l) => l.depends_on.clone(),
        };
        for dep in deps {
            if !in_degree.contains_key(&dep) {
                return Err(MobError::Internal(format!(
                    "node '{node_id}' depends on unknown node '{dep}'"
                )));
            }
            *in_degree.entry(node_id.clone()).or_insert(0) += 1;
            outgoing
                .entry(dep.clone())
                .or_default()
                .push(node_id.clone());
        }
    }

    let mut queue = std::collections::VecDeque::new();
    for node_id in spec.nodes.keys() {
        if in_degree.get(node_id) == Some(&0) {
            queue.push_back(node_id.clone());
        }
    }

    let mut ordered = Vec::with_capacity(spec.nodes.len());
    while let Some(next) = queue.pop_front() {
        ordered.push(next.clone());
        if let Some(children) = outgoing.get(&next) {
            for child in children {
                if let Some(count) = in_degree.get_mut(child)
                    && *count > 0
                {
                    *count -= 1;
                    if *count == 0 {
                        queue.push_back(child.clone());
                    }
                }
            }
        }
    }

    if ordered.len() != spec.nodes.len() {
        return Err(MobError::Internal(
            "frame contains a cycle; cannot compute topological order".to_string(),
        ));
    }

    Ok(ordered)
}

fn maybe_effect_request_body_frame_start_depth(effects: &[loop_iteration::Effect]) -> Option<u32> {
    effects.iter().find_map(|effect| match effect {
        loop_iteration::Effect::RequestBodyFrameStart(payload) => Some(payload.depth),
        _ => None,
    })
}

fn first_matching_loop_terminal_effect(
    effects: &[loop_iteration::Effect],
) -> Option<LoopTerminalEffect> {
    effects.iter().find_map(|effect| match effect {
        loop_iteration::Effect::LoopCompleted(_) => Some(LoopTerminalEffect::Completed),
        loop_iteration::Effect::LoopExhausted(_) => Some(LoopTerminalEffect::Exhausted),
        loop_iteration::Effect::LoopFailed(_) => Some(LoopTerminalEffect::Failed),
        loop_iteration::Effect::LoopCanceled(_) => Some(LoopTerminalEffect::Canceled),
        _ => None,
    })
}

fn terminal_phase(state: &flow_frame::State) -> Result<FlowFrameTerminalPhase, MobError> {
    match state.phase {
        flow_frame::Phase::Completed => Ok(FlowFrameTerminalPhase::Completed),
        flow_frame::Phase::Failed => Ok(FlowFrameTerminalPhase::Failed),
        flow_frame::Phase::Canceled => Ok(FlowFrameTerminalPhase::Canceled),
        ref other => Err(MobError::Internal(format!(
            "frame is not in a terminal phase: '{other:?}'"
        ))),
    }
}

fn loop_terminal_effect(phase: &loop_iteration::Phase) -> Option<LoopTerminalEffect> {
    match phase {
        loop_iteration::Phase::Completed => Some(LoopTerminalEffect::Completed),
        loop_iteration::Phase::Exhausted => Some(LoopTerminalEffect::Exhausted),
        loop_iteration::Phase::Failed => Some(LoopTerminalEffect::Failed),
        loop_iteration::Phase::Canceled => Some(LoopTerminalEffect::Canceled),
        _ => None,
    }
}

fn state_is_awaiting_body_frame(state: &loop_iteration::State) -> bool {
    state.stage == loop_iteration::LoopIterationStage::AwaitingBodyFrame
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loop_completed_effect() -> loop_iteration::Effect {
        loop_iteration::Effect::LoopCompleted(loop_iteration::effects::LoopCompleted {
            loop_instance_id: LoopInstanceId::from("loop-1"),
            parent_frame_id: FrameId::from("frame-1"),
            parent_node_id: FlowNodeId::from("node-1"),
        })
    }

    #[test]
    fn loop_terminal_effects_project_to_typed_terminal() {
        assert_eq!(
            first_matching_loop_terminal_effect(&[loop_completed_effect()]),
            Some(LoopTerminalEffect::Completed)
        );
        assert_eq!(
            loop_terminal_effect(&loop_iteration::Phase::Exhausted),
            Some(LoopTerminalEffect::Exhausted)
        );
    }

    #[test]
    fn node_terminal_status_maps_from_typed_node_status() {
        assert_eq!(
            node_terminal_step_status(flow_frame::NodeRunStatus::Completed),
            Some(StepRunStatus::Completed)
        );
        assert_eq!(
            node_terminal_step_status(flow_frame::NodeRunStatus::Running),
            None
        );
    }
}

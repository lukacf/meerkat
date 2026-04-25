//! FlowFrameEngine: drives a frame-backed flow through the scheduler-owned
//! admission, execution, and terminalization cycle.
//!
//! This executor handles:
//! - Step nodes: delegates async work to `FrameStepExecutor` and projects the
//!   typed outcome back through machine-owned transitions.
//! - Loop nodes: routes loop lifecycle and body-frame starts through the
//!   run/frame/loop kernels rather than a recursive shell executor.

use crate::definition::{FlowNodeSpec, FrameSpec};
use crate::error::MobError;
use crate::ids::{FlowNodeId, FrameId, LoopId, LoopInstanceId, RunId, StepId};
use crate::run::{
    FlowContext, FrameSnapshot, LoopIterationLedgerEntry, LoopSnapshot, MobRun, StepRunStatus,
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
    Failed,
}

/// Canonical outcome of executing a frame subtree.
#[derive(Debug)]
pub struct FrameExecutionOutcome {
    pub outputs: IndexMap<StepId, serde_json::Value>,
    pub step_statuses: IndexMap<StepId, StepRunStatus>,
    pub root_phase: FlowFrameTerminalPhase,
}

#[derive(Debug, Clone)]
struct PreviewedNodeGrant {
    frame_id: FrameId,
    next_run_state: flow_run::State,
}

#[derive(Debug, Clone)]
struct PreviewedBodyFrameGrant {
    loop_instance_id: LoopInstanceId,
    next_run_state: flow_run::State,
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
}

impl FlowFrameKernel {
    pub fn new(run_store: Arc<dyn MobRunStore>) -> Self {
        Self { run_store }
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
        input: flow_frame::Input,
        max_retries: usize,
    ) -> Result<Vec<flow_frame::Effect>, MobError> {
        for _ in 0..=max_retries {
            let current = self.require_frame(run_id, frame_id).await?;
            let outcome = flow_frame::transition(
                &current.kernel_state,
                input.clone(),
                &flow_frame::EmptyContext,
            )
            .map_err(|e| MobError::Internal(format!("flow_frame transition failed: {e:?}")))?;
            let next = FrameSnapshot {
                kernel_state: outcome.next_state,
            };
            let effects = outcome.effects.clone();
            let won = self
                .run_store
                .cas_frame_state(run_id, frame_id, Some(&current), next)
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
            return Ok(existing.clone());
        }

        let initial = flow_frame::initial_state();
        let ordered = topological_order(spec)?;
        let start_input = build_start_root_frame_input(frame_id, spec, &ordered);
        let outcome = flow_frame::transition(&initial, start_input, &flow_frame::EmptyContext)
            .map_err(|e| MobError::Internal(format!("flow_frame StartRootFrame failed: {e:?}")))?;
        let snapshot = FrameSnapshot {
            kernel_state: outcome.next_state,
        };
        let inserted = self
            .run_store
            .cas_frame_state(run_id, frame_id, None, snapshot.clone())
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
        let input =
            flow_frame::Input::AdmitNextReadyNode(flow_frame::inputs::AdmitNextReadyNode {});
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
            if snap.kernel_state.ready_queue.is_empty() {
                return Ok(None);
            }
            let admit_input =
                flow_frame::Input::AdmitNextReadyNode(flow_frame::inputs::AdmitNextReadyNode {});
            let outcome =
                flow_frame::transition(&snap.kernel_state, admit_input, &flow_frame::EmptyContext)
                    .map_err(|e| MobError::Internal(format!("AdmitNextReadyNode failed: {e:?}")))?;
            let next_snap = FrameSnapshot {
                kernel_state: outcome.next_state,
            };
            let won = self
                .run_store
                .cas_frame_state(run_id, frame_id, Some(&snap), next_snap)
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
                flow_frame::Input::CompleteNode(flow_frame::inputs::CompleteNode {
                    node_id: node_id.clone(),
                });
            let next_outcome = flow_frame::transition(
                &snap.kernel_state,
                complete_input,
                &flow_frame::EmptyContext,
            )
            .map_err(|e| MobError::Internal(format!("CompleteNode failed: {e:?}")))?;
            let next_snap = FrameSnapshot {
                kernel_state: next_outcome.next_state,
            };
            let won = self
                .run_store
                .cas_complete_step_and_record_output(
                    run_id,
                    frame_id,
                    &snap,
                    next_snap,
                    step_id.to_string(),
                    output.clone(),
                    loop_context,
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
        let input = flow_frame::Input::CompleteNode(flow_frame::inputs::CompleteNode {
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
        let input = flow_frame::Input::FailNode(flow_frame::inputs::FailNode {
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
        let input = flow_frame::Input::SkipNode(flow_frame::inputs::SkipNode {
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
        let input = flow_frame::Input::CancelNode(flow_frame::inputs::CancelNode {
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
        let input = flow_frame::Input::SealFrame(flow_frame::inputs::SealFrame {});
        self.transition_frame(run_id, frame_id, input, 5)
            .await
            .map(|_| true)
    }
}

// ─── FlowFrameEngine ─────────────────────────────────────────────────────────

/// Drives a `FrameSpec` to completion using a scheduler-backed concurrent
/// coordinator for frame-local work.
pub struct FlowFrameEngine {
    run_store: Arc<dyn MobRunStore>,
    executor: Arc<dyn FrameStepExecutor>,
    frame_kernel: Arc<FlowFrameKernel>,
    projector: Option<MobHandle>,
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
        projector: Option<MobHandle>,
        max_frame_depth: u32,
        max_active_frames: u32,
    ) -> Self {
        let frame_kernel = Arc::new(FlowFrameKernel::new(run_store.clone()));
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
        let run = self
            .run_store
            .get_run(run_id)
            .await?
            .ok_or_else(|| MobError::RunNotFound(run_id.clone()))?;
        if let Some(existing) = run.frames.get(frame_id) {
            return Ok(existing.clone());
        }

        let initial = flow_frame::initial_state();
        let ordered = crate::runtime::flow_frame_engine::topological_order(spec)?;
        let start_input = crate::runtime::flow_frame_engine::build_start_root_frame_input(
            frame_id, spec, &ordered,
        );
        let outcome = flow_frame::transition(&initial, start_input, &flow_frame::EmptyContext)
            .map_err(|e| MobError::Internal(format!("flow_frame StartRootFrame failed: {e:?}")))?;
        let snapshot = FrameSnapshot {
            kernel_state: outcome.next_state,
        };
        let inserted = self
            .run_store
            .cas_frame_state(run_id, frame_id, None, snapshot.clone())
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
        self.project_root_frame_seed(run_id, root_frame_id, root_spec)
            .await?;
        self.project_frame_phase(
            root_frame_id,
            crate::machines::mob_machine::FrameStatus::Running,
        )
        .await?;
        self.heal_terminal_body_frames(run_id, root_frame_id, root_spec, context)
            .await?;
        self.heal_orphaned_running_nodes(run_id, root_frame_id, root_spec, context)
            .await?;
        self.revisit_frame(run_id, root_frame_id, root_spec, context, root_frame_id)
            .await?;

        let mut workers: FrameNodeWorkers = FuturesUnordered::new();

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
                        self.handle_task_result(run_id, root_frame_id, root_spec, context, task)
                            .await?;
                        progressed = true;
                    }
                    None => {
                        self.revisit_frame(
                            run_id,
                            root_frame_id,
                            root_spec,
                            context,
                            root_frame_id,
                        )
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
                if run.flow_state.active_node_count > 0 || run.flow_state.active_frame_count > 0 {
                    tokio_time::sleep(std::time::Duration::from_millis(10)).await;
                    continue;
                }
                return Err(MobError::Internal(format!(
                    "frame runtime stalled for root frame '{root_frame_id}' in run '{run_id}'"
                )));
            }
        }

        let run = self.require_run(run_id).await?;
        let outputs = root_outputs_from_run(&run);
        let step_statuses = self.collect_all_step_statuses(&run, root_frame_id, root_spec)?;
        let root_frame = run.frames.get(root_frame_id).cloned().ok_or_else(|| {
            MobError::Internal(format!(
                "root frame '{root_frame_id}' missing at end of run '{run_id}'"
            ))
        })?;
        let root_phase = root_terminal_phase(&root_frame).ok_or_else(|| {
            MobError::Internal(format!(
                "root frame '{root_frame_id}' ended in non-terminal phase '{:?}'",
                root_frame.kernel_state.phase
            ))
        })?;

        Ok(FrameExecutionOutcome {
            outputs,
            step_statuses,
            root_phase,
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
            let Some(grant) = preview_node_grant(&run.flow_state)? else {
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
            let decision = acknowledge_node_grant(
                &run.flow_state,
                &grant,
                &current_frame,
                &frame_spec,
                frame_depth,
                self.max_frame_depth,
            )?;
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
            let Some(grant) = preview_body_frame_grant(&run.flow_state)? else {
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
            let decision =
                acknowledge_body_frame_start(&run.flow_state, &grant, &loop_snapshot, &loop_spec)?;
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
                    Ok(FrameStepResult::Failed) => {
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
    ) -> Result<(), MobError> {
        loop {
            let run = self.require_run(run_id).await?;
            let Some(frame) = run.frames.get(frame_id).cloned() else {
                return Ok(());
            };

            if let Some(plan) =
                register_ready_frame_if_needed(&run.flow_state, frame_id, &frame.kernel_state)?
            {
                let _ = self.execute_store_plan(run_id, &plan).await?;
                continue;
            }

            let frame_spec = self.resolve_frame_spec(&run, root_frame_id, root_spec, frame_id)?;
            if let Some(plan) = seal_frame_if_ready(frame_id, &frame, &frame_spec)? {
                let _ = self.execute_store_plan(run_id, &plan).await?;
                continue;
            }

            let run = self.require_run(run_id).await?;
            let Some(frame) = run.frames.get(frame_id).cloned() else {
                return Ok(());
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
                    continue;
                }

                let parent_frame = self.parent_frame_for_loop(&run, &loop_snapshot);
                if let Some(decision) = advance_body_frame_after_seal(
                    &run.flow_state,
                    frame_id,
                    &frame,
                    &loop_snapshot,
                    parent_frame.as_ref(),
                )? {
                    let _ = self
                        .execute_decision(run_id, root_frame_id, root_spec, context, None, decision)
                        .await?;
                    continue;
                }
            }

            return Ok(());
        }
    }

    async fn ensure_scheduler_state_initialized(&self, run_id: &RunId) -> Result<(), MobError> {
        for _ in 0..=5usize {
            let run = self.require_run(run_id).await?;
            let mut reconciled = run.clone();
            if reconciled.flow_state.phase == flow_run::Phase::Absent {
                reconciled.flow_state.phase = flow_run::Phase::Running;
            }
            if reconciled.flow_state.max_frame_depth == 0 && self.max_frame_depth > 0 {
                reconciled.flow_state.max_frame_depth = self.max_frame_depth;
            }
            if reconciled.flow_state.max_active_frames == 0 && self.max_active_frames > 0 {
                reconciled.flow_state.max_active_frames = self.max_active_frames;
            }
            crate::runtime::recovery::reconcile_run_state(&mut reconciled).map_err(|error| {
                MobError::Internal(format!(
                    "scheduler state reconciliation failed for run '{run_id}': {error}"
                ))
            })?;
            if reconciled.flow_state == run.flow_state {
                return Ok(());
            }

            let won = self
                .run_store
                .cas_flow_state(run_id, &run.flow_state, &reconciled.flow_state)
                .await?;
            if won {
                return Ok(());
            }
        }

        Err(MobError::Internal(format!(
            "scheduler state bootstrap CAS exhausted for run '{run_id}'"
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
                                if let Some(decision) = recover_terminal_loop_projection(
                                    &run.flow_state,
                                    loop_snapshot,
                                    &frame,
                                )? {
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
                                if let Some(decision) = recover_pending_body_frame_request(
                                    &run.flow_state,
                                    loop_snapshot,
                                )? {
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
            let plan = release_node_execution(&run.flow_state, frame_id)?;
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
        match plan {
            FlowFrameLoopStorePlan::GrantNodeSlot {
                expected_run_state,
                next_run_state,
                frame_id,
                expected_frame,
                next_frame,
            } => self
                .run_store
                .cas_grant_node_slot(
                    run_id,
                    expected_run_state,
                    next_run_state.clone(),
                    frame_id,
                    expected_frame,
                    next_frame.clone(),
                )
                .await
                .map_err(MobError::from),
            FlowFrameLoopStorePlan::StartLoop {
                loop_instance_id,
                expected_run_state,
                next_run_state,
                frame_id,
                expected_frame,
                next_frame,
                initial_loop,
            } => self
                .run_store
                .cas_start_loop(
                    run_id,
                    loop_instance_id,
                    expected_run_state,
                    next_run_state.clone(),
                    frame_id,
                    expected_frame,
                    next_frame.clone(),
                    initial_loop.clone(),
                )
                .await
                .map_err(MobError::from),
            FlowFrameLoopStorePlan::GrantBodyFrameStart {
                loop_instance_id,
                expected_loop,
                next_loop,
                frame_id,
                initial_frame,
                ledger_entry,
                expected_run_state,
                next_run_state,
            } => self
                .run_store
                .cas_grant_body_frame_start(
                    run_id,
                    loop_instance_id,
                    expected_loop,
                    next_loop.clone(),
                    frame_id,
                    initial_frame.clone(),
                    ledger_entry.clone(),
                    expected_run_state,
                    next_run_state.clone(),
                )
                .await
                .map_err(MobError::from),
            FlowFrameLoopStorePlan::RunStateOnly {
                expected_run_state,
                next_run_state,
            } => self
                .run_store
                .cas_flow_state(run_id, expected_run_state, next_run_state)
                .await
                .map_err(MobError::from),
            FlowFrameLoopStorePlan::SealFrame {
                frame_id,
                expected_frame,
                next_frame,
            } => self
                .run_store
                .cas_frame_state(run_id, frame_id, Some(expected_frame), next_frame.clone())
                .await
                .map_err(MobError::from),
            FlowFrameLoopStorePlan::CompleteBodyFrame {
                loop_instance_id,
                expected_loop,
                next_loop,
                frame_id,
                expected_frame,
                next_frame,
                expected_run_state,
                next_run_state,
            } => self
                .run_store
                .cas_complete_body_frame(
                    run_id,
                    loop_instance_id,
                    expected_loop,
                    next_loop.clone(),
                    frame_id,
                    expected_frame,
                    next_frame.clone(),
                    expected_run_state,
                    next_run_state.clone(),
                )
                .await
                .map_err(MobError::from),
            FlowFrameLoopStorePlan::LoopRequestBodyFrame {
                loop_instance_id,
                expected_loop,
                next_loop,
                expected_run_state,
                next_run_state,
            } => self
                .run_store
                .cas_loop_request_body_frame(
                    run_id,
                    loop_instance_id,
                    expected_loop,
                    next_loop.clone(),
                    expected_run_state,
                    next_run_state.clone(),
                )
                .await
                .map_err(MobError::from),
            FlowFrameLoopStorePlan::CompleteLoop {
                loop_instance_id,
                expected_loop,
                next_loop,
                frame_id,
                expected_frame,
                next_frame,
                expected_run_state,
                next_run_state,
            } => self
                .run_store
                .cas_complete_loop(
                    run_id,
                    loop_instance_id,
                    expected_loop,
                    next_loop.clone(),
                    frame_id,
                    expected_frame,
                    next_frame.clone(),
                    expected_run_state,
                    next_run_state.clone(),
                )
                .await
                .map_err(MobError::from),
        }
    }

    async fn project_store_plan(
        &self,
        run_id: &RunId,
        root_frame_id: &FrameId,
        root_spec: &FrameSpec,
        plan: &FlowFrameLoopStorePlan,
    ) -> Result<(), MobError> {
        let Some(projector) = &self.projector else {
            return Ok(());
        };
        match plan {
            FlowFrameLoopStorePlan::StartLoop { initial_loop, .. } => {
                projector
                    .project_machine_input(crate::run::MobRun::create_loop_seed_input(
                        initial_loop,
                    )?)
                    .await?;
                let active_body = initial_loop.kernel_state.active_body_frame_id.clone();
                projector
                    .project_machine_input(crate::run::MobRun::create_loop_state_input(
                        &initial_loop.kernel_state.loop_instance_id,
                        crate::machines::mob_machine::LoopStatus::Running,
                        crate::machines::mob_machine::LoopIterationStage::AwaitingBodyFrame,
                        active_body.as_ref(),
                    ))
                    .await?;
            }
            FlowFrameLoopStorePlan::GrantBodyFrameStart {
                loop_instance_id,
                frame_id,
                ledger_entry,
                ..
            } => {
                let run = self.require_run(run_id).await?;
                let spec = self.resolve_frame_spec(&run, root_frame_id, root_spec, frame_id)?;
                let ordered = crate::runtime::flow_frame_engine::topological_order(&spec)?;
                projector
                    .project_machine_input(crate::run::MobRun::create_frame_seed_input(
                        run_id,
                        frame_id,
                        Some(loop_instance_id),
                        u32::try_from(ledger_entry.iteration).map_err(|_| {
                            MobError::Internal(
                                "loop iteration ledger entry exceeds u32 during frame projection"
                                    .to_string(),
                            )
                        })?,
                        crate::machines::mob_machine::FrameScope::Body,
                        &spec,
                        &ordered,
                    )?)
                    .await?;
                projector
                    .project_machine_input(crate::run::MobRun::create_loop_state_input(
                        loop_instance_id,
                        crate::machines::mob_machine::LoopStatus::Running,
                        crate::machines::mob_machine::LoopIterationStage::BodyFrameActive,
                        Some(frame_id),
                    ))
                    .await?;
                projector
                    .project_machine_input(crate::run::MobRun::create_frame_phase_input(
                        frame_id,
                        crate::machines::mob_machine::FrameStatus::Running,
                    ))
                    .await?;
            }
            FlowFrameLoopStorePlan::CompleteBodyFrame {
                loop_instance_id,
                next_loop,
                ..
            } => {
                projector
                    .project_machine_input(
                        crate::run::MobRun::record_loop_body_frame_completed_input(
                            loop_instance_id,
                            next_loop.kernel_state.last_completed_iteration,
                        ),
                    )
                    .await?;
            }
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
        let (Some(projector), Some(feedback)) = (&self.projector, feedback) else {
            return Ok(());
        };
        projector
            .project_machine_input(
                crate::run::MobRun::record_loop_until_condition_feedback_input(
                    &feedback.loop_instance_id,
                    feedback.iteration,
                    feedback.until_met,
                ),
            )
            .await
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
            let (decision, until_feedback) = resolve_until_feedback_decision(
                &run.flow_state,
                loop_snapshot,
                &parent_frame,
                obligation.clone(),
                until_met,
            )?;
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

    fn collect_all_step_statuses(
        &self,
        run: &MobRun,
        root_frame_id: &FrameId,
        root_spec: &FrameSpec,
    ) -> Result<IndexMap<StepId, StepRunStatus>, MobError> {
        let mut step_statuses = IndexMap::new();
        for (frame_id, frame) in &run.frames {
            let spec = self.resolve_frame_spec(run, root_frame_id, root_spec, frame_id)?;
            merge_step_statuses(
                &mut step_statuses,
                collect_frame_step_statuses(&spec, &frame.kernel_state),
            );
        }
        Ok(step_statuses)
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

    async fn project_root_frame_seed(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        spec: &FrameSpec,
    ) -> Result<(), MobError> {
        let Some(projector) = &self.projector else {
            return Ok(());
        };
        let ordered = crate::runtime::flow_frame_engine::topological_order(spec)?;
        let input = crate::run::MobRun::create_frame_seed_input(
            run_id,
            frame_id,
            None,
            0,
            crate::machines::mob_machine::FrameScope::Root,
            spec,
            &ordered,
        )?;
        projector.project_machine_input(input).await
    }

    async fn project_frame_phase(
        &self,
        frame_id: &FrameId,
        phase: crate::machines::mob_machine::FrameStatus,
    ) -> Result<(), MobError> {
        let Some(projector) = &self.projector else {
            return Ok(());
        };
        projector
            .project_machine_input(crate::run::MobRun::create_frame_phase_input(
                frame_id, phase,
            ))
            .await
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

fn collect_frame_step_statuses(
    spec: &FrameSpec,
    kernel_state: &flow_frame::State,
) -> IndexMap<StepId, StepRunStatus> {
    let mut step_statuses = IndexMap::new();
    for (node_id, node_spec) in &spec.nodes {
        let FlowNodeSpec::Step(step_spec) = node_spec else {
            continue;
        };
        let Some(status) = kernel_state.node_status.get(node_id) else {
            continue;
        };
        let Some(step_status) = node_terminal_step_status(status.as_str()) else {
            continue;
        };
        merge_step_status(&mut step_statuses, step_spec.step_id.clone(), step_status);
    }
    step_statuses
}

fn node_terminal_step_status(value: &str) -> Option<StepRunStatus> {
    match value {
        "Completed" => Some(StepRunStatus::Completed),
        "Skipped" => Some(StepRunStatus::Skipped),
        "Failed" => Some(StepRunStatus::Failed),
        _ => None,
    }
}

fn merge_step_statuses(
    into: &mut IndexMap<StepId, StepRunStatus>,
    updates: IndexMap<StepId, StepRunStatus>,
) {
    for (step_id, status) in updates {
        merge_step_status(into, step_id, status);
    }
}

fn merge_step_status(
    into: &mut IndexMap<StepId, StepRunStatus>,
    step_id: StepId,
    status: StepRunStatus,
) {
    let next_rank = step_status_rank(&status);
    match into.get_mut(&step_id) {
        Some(existing) if step_status_rank(existing) >= next_rank => {}
        Some(existing) => *existing = status,
        None => {
            into.insert(step_id, status);
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

fn preview_node_grant(state: &flow_run::State) -> Result<Option<PreviewedNodeGrant>, MobError> {
    let outcome = flow_run::transition(
        state,
        flow_run::Input::PumpNodeScheduler(flow_run::inputs::PumpNodeScheduler {}),
        &flow_run::EmptyContext,
    );
    match outcome {
        Ok(outcome) => Ok(outcome.effects.iter().find_map(|effect| match effect {
            flow_run::Effect::GrantNodeSlot(payload) => Some(PreviewedNodeGrant {
                frame_id: payload.frame_id.clone(),
                next_run_state: outcome.next_state.clone(),
            }),
            _ => None,
        })),
        Err(_) => Ok(None),
    }
}

fn preview_body_frame_grant(
    state: &flow_run::State,
) -> Result<Option<PreviewedBodyFrameGrant>, MobError> {
    let outcome = flow_run::transition(
        state,
        flow_run::Input::PumpFrameScheduler(flow_run::inputs::PumpFrameScheduler {}),
        &flow_run::EmptyContext,
    );
    match outcome {
        Ok(outcome) => Ok(outcome.effects.iter().find_map(|effect| match effect {
            flow_run::Effect::GrantBodyFrameStart(payload) => Some(PreviewedBodyFrameGrant {
                loop_instance_id: payload.loop_instance_id.clone(),
                next_run_state: outcome.next_state.clone(),
            }),
            _ => None,
        })),
        Err(_) => Ok(None),
    }
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
    input: flow_run::Input,
) -> Result<flow_run::Outcome, MobError> {
    flow_run::transition(state, input, &flow_run::EmptyContext)
        .map_err(|error| MobError::Internal(format!("flow_run transition refused: {error:?}")))
}

fn run_transition_state(
    state: &flow_run::State,
    input: flow_run::Input,
) -> Result<flow_run::State, MobError> {
    Ok(run_transition_outcome(state, input)?.next_state)
}

fn frame_transition_outcome(
    state: &flow_frame::State,
    input: flow_frame::Input,
) -> Result<flow_frame::Outcome, MobError> {
    flow_frame::transition(state, input, &flow_frame::EmptyContext)
        .map_err(|error| MobError::Internal(format!("flow_frame transition refused: {error:?}")))
}

fn loop_transition_outcome(
    state: &loop_iteration::State,
    input: loop_iteration::Input,
) -> Result<loop_iteration::Outcome, MobError> {
    loop_iteration::transition(state, input, &loop_iteration::EmptyContext).map_err(|error| {
        MobError::Internal(format!("loop_iteration transition refused: {error:?}"))
    })
}

fn has_effect_variant(effects: &[flow_frame::Effect], variant: &str) -> bool {
    effects.iter().any(|effect| {
        matches!(
            (variant, effect),
            (
                "NodeExecutionReleased",
                flow_frame::Effect::NodeExecutionReleased(_)
            )
        )
    })
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

fn initial_loop_snapshot(
    loop_instance_id: &LoopInstanceId,
    loop_spec: &crate::definition::RepeatUntilSpec,
    parent_frame_id: &FrameId,
    parent_node_id: &FlowNodeId,
    depth: u32,
) -> Result<LoopSnapshot, MobError> {
    let initial = loop_iteration::initial_state();
    let start = loop_transition_outcome(
        &initial,
        loop_iteration::Input::StartLoop(loop_iteration::inputs::StartLoop {
            loop_instance_id: loop_instance_id.clone(),
            max_iterations: loop_spec.max_iterations,
            parent_frame_id: parent_frame_id.clone(),
            parent_node_id: parent_node_id.clone(),
            loop_id: loop_spec.loop_id.clone(),
            depth,
        }),
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
    let ordered = crate::runtime::flow_frame_engine::topological_order(spec)?;
    let start_input = crate::runtime::flow_frame_engine::build_start_body_frame_input(
        frame_id,
        loop_instance_id,
        iteration,
        spec,
        &ordered,
    );
    let outcome = flow_frame::transition(&initial, start_input, &flow_frame::EmptyContext)
        .map_err(|error| {
            MobError::Internal(format!("flow_frame StartBodyFrame failed: {error:?}"))
        })?;
    Ok(FrameSnapshot {
        kernel_state: outcome.next_state,
    })
}

fn frame_ready(state: &flow_frame::State) -> bool {
    !state.ready_queue.is_empty()
}

fn all_nodes_terminal(state: &flow_frame::State, spec: &FrameSpec) -> bool {
    let terminal = ["Completed", "Failed", "Skipped", "Canceled"];
    spec.nodes.keys().all(|node_id| {
        state
            .node_status
            .get(node_id)
            .is_some_and(|status| terminal.contains(&status.as_str()))
    })
}

fn register_ready_frame_if_needed(
    run_state: &flow_run::State,
    frame_id: &FrameId,
    frame_state: &flow_frame::State,
) -> Result<Option<FlowFrameLoopStorePlan>, MobError> {
    if frame_state.phase != flow_frame::Phase::Running
        || !frame_ready(frame_state)
        || run_state.ready_frame_membership.contains(frame_id)
    {
        return Ok(None);
    }
    Ok(Some(FlowFrameLoopStorePlan::RunStateOnly {
        expected_run_state: run_state.clone(),
        next_run_state: flow_run::transition(
            run_state,
            flow_run::Input::RegisterReadyFrame(flow_run::inputs::RegisterReadyFrame {
                frame_id: frame_id.clone(),
            }),
            &flow_run::EmptyContext,
        )
        .map_err(|error| MobError::Internal(format!("flow_run transition refused: {error:?}")))?
        .next_state,
    }))
}

fn release_node_execution(
    run_state: &flow_run::State,
    frame_id: &FrameId,
) -> Result<FlowFrameLoopStorePlan, MobError> {
    Ok(FlowFrameLoopStorePlan::RunStateOnly {
        expected_run_state: run_state.clone(),
        next_run_state: flow_run::transition(
            run_state,
            flow_run::Input::NodeExecutionReleased(flow_run::inputs::NodeExecutionReleased {
                frame_id: frame_id.clone(),
            }),
            &flow_run::EmptyContext,
        )
        .map_err(|error| MobError::Internal(format!("flow_run transition refused: {error:?}")))?
        .next_state,
    })
}

fn seal_frame_if_ready(
    frame_id: &FrameId,
    current_frame: &FrameSnapshot,
    frame_spec: &FrameSpec,
) -> Result<Option<FlowFrameLoopStorePlan>, MobError> {
    if current_frame.kernel_state.phase != flow_frame::Phase::Running
        || !all_nodes_terminal(&current_frame.kernel_state, frame_spec)
    {
        return Ok(None);
    }
    let outcome = flow_frame::transition(
        &current_frame.kernel_state,
        flow_frame::Input::SealFrame(flow_frame::inputs::SealFrame {}),
        &flow_frame::EmptyContext,
    )
    .map_err(|error| MobError::Internal(format!("flow_frame transition refused: {error:?}")))?;
    Ok(Some(FlowFrameLoopStorePlan::SealFrame {
        frame_id: frame_id.clone(),
        expected_frame: current_frame.clone(),
        next_frame: FrameSnapshot {
            kernel_state: outcome.next_state,
        },
    }))
}

fn pending_until_obligation(
    loop_snapshot: &LoopSnapshot,
) -> Result<Option<FlowLoopUntilEvaluationObligation>, MobError> {
    if loop_snapshot.kernel_state.phase != loop_iteration::Phase::Running
        || loop_snapshot.kernel_state.stage.as_str() != "AwaitingUntil"
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

fn acknowledge_node_grant(
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
        flow_frame::Input::AdmitNextReadyNode(flow_frame::inputs::AdmitNextReadyNode {}),
    )?;
    let next_frame = FrameSnapshot {
        kernel_state: admit_outcome.next_state.clone(),
    };
    let mut next_run_state = grant.next_run_state.clone();

    if has_effect_variant(&admit_outcome.effects, "NodeExecutionReleased") {
        next_run_state = run_transition_state(
            &next_run_state,
            flow_run::Input::NodeExecutionReleased(flow_run::inputs::NodeExecutionReleased {
                frame_id: frame_id.clone(),
            }),
        )?;
    }

    if frame_ready(&next_frame.kernel_state)
        && !next_run_state.ready_frame_membership.contains(frame_id)
    {
        next_run_state = run_transition_state(
            &next_run_state,
            flow_run::Input::RegisterReadyFrame(flow_run::inputs::RegisterReadyFrame {
                frame_id: frame_id.clone(),
            }),
        )?;
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
        let body_depth = current_depth + 1;
        if max_frame_depth > 0 && body_depth > max_frame_depth {
            return Err(MobError::FrameDepthLimitExceeded {
                loop_id: loop_spec.loop_id,
                max_frame_depth,
                current_depth: body_depth - 1,
            });
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
            && !next_run_state
                .pending_body_frame_loop_membership
                .contains(&loop_instance_id)
        {
            next_run_state = run_transition_state(
                &next_run_state,
                flow_run::Input::RegisterPendingBodyFrame(
                    flow_run::inputs::RegisterPendingBodyFrame {
                        loop_instance_id: loop_instance_id.clone(),
                        depth: body_depth,
                    },
                ),
            )?;
        }
        return Ok(FlowFrameLoopDecision {
            store_plan: Some(FlowFrameLoopStorePlan::StartLoop {
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
            expected_run_state: expected_run_state.clone(),
            next_run_state,
            frame_id: frame_id.clone(),
            expected_frame: current_frame.clone(),
            next_frame,
        }),
        follow_up,
    })
}

fn acknowledge_body_frame_start(
    expected_run_state: &flow_run::State,
    grant: &PreviewedBodyFrameGrant,
    loop_snapshot: &LoopSnapshot,
    loop_spec: &crate::definition::RepeatUntilSpec,
) -> Result<FlowFrameLoopDecision, MobError> {
    let iteration = loop_current_iteration(&loop_snapshot.kernel_state)?;
    let frame_id = FrameId::from(format!("{}::iter-{iteration}", grant.loop_instance_id).as_str());
    let initial_frame = initial_body_frame_snapshot(
        &frame_id,
        &grant.loop_instance_id,
        iteration,
        &loop_spec.body,
    )?;
    let body_frame_outcome = loop_transition_outcome(
        &loop_snapshot.kernel_state,
        loop_iteration::Input::BodyFrameStarted(loop_iteration::inputs::BodyFrameStarted {
            loop_instance_id: grant.loop_instance_id.clone(),
            frame_id: frame_id.clone(),
            iteration: iteration as u32,
        }),
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
        && !next_run_state.ready_frame_membership.contains(&frame_id)
    {
        next_run_state = run_transition_state(
            &next_run_state,
            flow_run::Input::RegisterReadyFrame(flow_run::inputs::RegisterReadyFrame {
                frame_id: frame_id.clone(),
            }),
        )?;
    }
    Ok(FlowFrameLoopDecision {
        store_plan: Some(FlowFrameLoopStorePlan::GrantBodyFrameStart {
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

fn advance_body_frame_after_seal(
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
    let next_run_state = flow_run::transition(
        run_state,
        flow_run::Input::FrameTerminated(flow_run::inputs::FrameTerminated {
            frame_id: body_frame_id.clone(),
        }),
        &flow_run::EmptyContext,
    )
    .map_err(|error| MobError::Internal(format!("flow_run transition refused: {error:?}")))?
    .next_state;
    let loop_outcome = loop_iteration::transition(
        &loop_snapshot.kernel_state,
        match first_variant {
            "BodyFrameCompleted" => loop_iteration::Input::BodyFrameCompleted(
                loop_iteration::inputs::BodyFrameCompleted {
                    loop_instance_id: loop_instance_id.clone(),
                    iteration: iteration as u32,
                },
            ),
            "BodyFrameFailed" => {
                loop_iteration::Input::BodyFrameFailed(loop_iteration::inputs::BodyFrameFailed {
                    loop_instance_id: loop_instance_id.clone(),
                    iteration: iteration as u32,
                })
            }
            "BodyFrameCanceled" => loop_iteration::Input::BodyFrameCanceled(
                loop_iteration::inputs::BodyFrameCanceled {
                    loop_instance_id: loop_instance_id.clone(),
                    iteration: iteration as u32,
                },
            ),
            _ => unreachable!(),
        },
        &loop_iteration::EmptyContext,
    )
    .map_err(|error| MobError::Internal(format!("loop_iteration transition refused: {error:?}")))?;
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

fn resolve_until_feedback_decision(
    run_state: &flow_run::State,
    loop_snapshot: &LoopSnapshot,
    parent_frame: &FrameSnapshot,
    obligation: FlowLoopUntilEvaluationObligation,
    until_met: bool,
) -> Result<(FlowFrameLoopDecision, LoopUntilConditionFeedbackProjection), MobError> {
    let feedback_input = if until_met {
        loop_iteration::Input::UntilConditionMet(loop_iteration::inputs::UntilConditionMet {
            loop_instance_id: obligation.loop_instance_id.clone(),
            iteration: obligation.iteration,
        })
    } else {
        loop_iteration::Input::UntilConditionFailed(loop_iteration::inputs::UntilConditionFailed {
            loop_instance_id: obligation.loop_instance_id.clone(),
            iteration: obligation.iteration,
        })
    };
    let feedback = loop_iteration::transition(
        &loop_snapshot.kernel_state,
        feedback_input,
        &loop_iteration::EmptyContext,
    )
    .map_err(|error| MobError::Internal(format!("loop_iteration transition refused: {error:?}")))?;
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
        if !run_state
            .pending_body_frame_loop_membership
            .contains(&obligation.loop_instance_id)
        {
            next_run_state = flow_run::transition(
                &next_run_state,
                flow_run::Input::RegisterPendingBodyFrame(
                    flow_run::inputs::RegisterPendingBodyFrame {
                        loop_instance_id: obligation.loop_instance_id.clone(),
                        depth,
                    },
                ),
                &flow_run::EmptyContext,
            )
            .map_err(|error| MobError::Internal(format!("flow_run transition refused: {error:?}")))?
            .next_state;
        }
        return Ok((
            FlowFrameLoopDecision {
                store_plan: Some(FlowFrameLoopStorePlan::LoopRequestBodyFrame {
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
    let decision = build_complete_loop_decision(CompleteLoopDecisionRequest {
        loop_terminal,
        expected_run_state: run_state,
        next_run_state: run_state.clone(),
        loop_instance_id: &obligation.loop_instance_id,
        expected_loop: loop_snapshot,
        next_loop,
        parent_frame_id: &obligation.parent_frame_id,
        expected_parent_frame: parent_frame,
        parent_node_id: &obligation.parent_node_id,
    })?;
    Ok((decision, until_feedback))
}

fn recover_terminal_loop_projection(
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

fn recover_pending_body_frame_request(
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
    if run_state
        .pending_body_frame_loop_membership
        .contains(&loop_instance_id)
    {
        return Ok(None);
    }

    let depth = loop_snapshot.kernel_state.depth;
    let next_run_state = flow_run::transition(
        run_state,
        flow_run::Input::RegisterPendingBodyFrame(flow_run::inputs::RegisterPendingBodyFrame {
            loop_instance_id,
            depth,
        }),
        &flow_run::EmptyContext,
    )
    .map_err(|error| MobError::Internal(format!("flow_run transition refused: {error:?}")))?
    .next_state;
    Ok(Some(FlowFrameLoopDecision {
        store_plan: Some(FlowFrameLoopStorePlan::RunStateOnly {
            expected_run_state: run_state.clone(),
            next_run_state,
        }),
        follow_up: Vec::new(),
    }))
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
    let parent_input = match loop_terminal {
        "LoopCompleted" => flow_frame::Input::CompleteNode(flow_frame::inputs::CompleteNode {
            node_id: parent_node_id.clone(),
        }),
        "LoopExhausted" | "LoopFailed" => {
            flow_frame::Input::FailNode(flow_frame::inputs::FailNode {
                node_id: parent_node_id.clone(),
            })
        }
        "LoopCanceled" => flow_frame::Input::CancelNode(flow_frame::inputs::CancelNode {
            node_id: parent_node_id.clone(),
        }),
        other => {
            return Err(MobError::Internal(format!(
                "unknown loop terminal effect '{other}'"
            )));
        }
    };
    let parent_outcome = flow_frame::transition(
        &expected_parent_frame.kernel_state,
        parent_input,
        &flow_frame::EmptyContext,
    )
    .map_err(|error| MobError::Internal(format!("flow_frame transition refused: {error:?}")))?;
    let next_parent_frame = FrameSnapshot {
        kernel_state: parent_outcome.next_state,
    };
    if frame_ready(&next_parent_frame.kernel_state)
        && !next_run_state
            .ready_frame_membership
            .contains(parent_frame_id)
    {
        next_run_state = flow_run::transition(
            &next_run_state,
            flow_run::Input::RegisterReadyFrame(flow_run::inputs::RegisterReadyFrame {
                frame_id: parent_frame_id.clone(),
            }),
            &flow_run::EmptyContext,
        )
        .map_err(|error| MobError::Internal(format!("flow_run transition refused: {error:?}")))?
        .next_state;
    }
    Ok(FlowFrameLoopDecision {
        store_plan: Some(FlowFrameLoopStorePlan::CompleteLoop {
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
) -> flow_frame::Input {
    let (
        tracked_nodes,
        ordered_nodes,
        node_kind,
        node_dependencies,
        node_dependency_modes,
        node_branches,
    ) = build_frame_start_payload(frame_id, spec, ordered);
    flow_frame::Input::StartRootFrame(flow_frame::inputs::StartRootFrame {
        frame_id: frame_id.clone(),
        tracked_nodes,
        ordered_nodes,
        node_kind,
        node_dependencies,
        node_dependency_modes,
        node_branches,
    })
}

fn build_start_body_frame_input(
    frame_id: &FrameId,
    loop_instance_id: &LoopInstanceId,
    iteration: u64,
    spec: &FrameSpec,
    ordered: &[FlowNodeId],
) -> flow_frame::Input {
    let (
        tracked_nodes,
        ordered_nodes,
        node_kind,
        node_dependencies,
        node_dependency_modes,
        node_branches,
    ) = build_frame_start_payload(frame_id, spec, ordered);
    flow_frame::Input::StartBodyFrame(flow_frame::inputs::StartBodyFrame {
        frame_id: frame_id.clone(),
        loop_instance_id: loop_instance_id.clone(),
        iteration: iteration as u32,
        tracked_nodes,
        ordered_nodes,
        node_kind,
        node_dependencies,
        node_dependency_modes,
        node_branches,
    })
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

fn first_matching_loop_terminal_effect(effects: &[loop_iteration::Effect]) -> Option<&'static str> {
    effects.iter().find_map(|effect| match effect {
        loop_iteration::Effect::LoopCompleted(_) => Some("LoopCompleted"),
        loop_iteration::Effect::LoopExhausted(_) => Some("LoopExhausted"),
        loop_iteration::Effect::LoopFailed(_) => Some("LoopFailed"),
        loop_iteration::Effect::LoopCanceled(_) => Some("LoopCanceled"),
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

fn loop_terminal_effect(phase: &loop_iteration::Phase) -> Option<&'static str> {
    match phase {
        loop_iteration::Phase::Completed => Some("LoopCompleted"),
        loop_iteration::Phase::Exhausted => Some("LoopExhausted"),
        loop_iteration::Phase::Failed => Some("LoopFailed"),
        loop_iteration::Phase::Canceled => Some("LoopCanceled"),
        _ => None,
    }
}

fn state_is_awaiting_body_frame(state: &loop_iteration::State) -> bool {
    state.stage == loop_iteration::LoopIterationStage::AwaitingBodyFrame
}

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
use crate::generated::flow_frame_loop_driver::{
    FlowFrameLoopDecision, FlowFrameLoopDriver, FlowFrameLoopStorePlan, FlowFrameLoopWork,
    FlowFrameTerminalPhase,
};
use crate::ids::{FlowNodeId, FrameId, LoopId, LoopInstanceId, RunId, StepId};
use crate::run::{FlowContext, FrameSnapshot, LoopSnapshot, MobRun, StepRunStatus};
use crate::runtime::conditions::evaluate_condition;
use crate::runtime::flow_frame_kernel::{FlowFrameKernel, FlowFrameMutator, StepCompletionOpts};
use crate::store::MobRunStore;
use async_trait::async_trait;
use futures::stream::{FuturesUnordered, StreamExt};
use indexmap::IndexMap;
use meerkat_machine_kernels::KernelValue;
use meerkat_machine_kernels::generated::flow_run;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

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

// ─── FlowFrameEngine ─────────────────────────────────────────────────────────

/// Drives a `FrameSpec` to completion using a scheduler-backed concurrent
/// coordinator for frame-local work.
pub struct FlowFrameEngine {
    run_store: Arc<dyn MobRunStore>,
    executor: Arc<dyn FrameStepExecutor>,
    frame_kernel: Arc<FlowFrameKernel>,
    /// Maximum nesting depth for body frames. 0 means unlimited.
    max_frame_depth: u32,
    /// Maximum number of simultaneously active body frames admitted by the run scheduler.
    /// 0 means unlimited.
    max_active_frames: u32,
}

impl FlowFrameEngine {
    pub fn new(
        run_store: Arc<dyn MobRunStore>,
        executor: Arc<dyn FrameStepExecutor>,
        max_frame_depth: u32,
        max_active_frames: u32,
    ) -> Self {
        let frame_kernel = Arc::new(FlowFrameKernel::new(run_store.clone()));
        Self {
            run_store,
            executor,
            frame_kernel,
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
            if root_snapshot.kernel_state.phase != "Running" && workers.is_empty() {
                break;
            }

            if !progressed && workers.is_empty() {
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
        let root_phase =
            FlowFrameLoopDriver::root_terminal_phase(&root_frame).ok_or_else(|| {
                MobError::Internal(format!(
                    "root frame '{root_frame_id}' ended in non-terminal phase '{}'",
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
            let Some(grant) = FlowFrameLoopDriver::preview_node_grant(&run.flow_state)? else {
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
            let decision = FlowFrameLoopDriver::acknowledge_node_grant(
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
            let Some(grant) = FlowFrameLoopDriver::preview_body_frame_grant(&run.flow_state)?
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
            let decision = FlowFrameLoopDriver::acknowledge_body_frame_start(
                &run.flow_state,
                &grant,
                &loop_snapshot,
                &loop_spec,
            )?;
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
                        self.frame_kernel
                            .fail_node(run_id, &frame_id, &node_id)
                            .await?;
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

            if let Some(plan) = FlowFrameLoopDriver::register_ready_frame_if_needed(
                &run.flow_state,
                frame_id,
                &frame.kernel_state,
            )? {
                let _ = self.execute_store_plan(run_id, &plan).await?;
                continue;
            }

            let frame_spec = self.resolve_frame_spec(&run, root_frame_id, root_spec, frame_id)?;
            if let Some(plan) =
                FlowFrameLoopDriver::seal_frame_if_ready(frame_id, &frame, &frame_spec)?
            {
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
                if let Some(obligation) =
                    FlowFrameLoopDriver::pending_until_obligation(&loop_snapshot)?
                {
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
                if let Some(decision) = FlowFrameLoopDriver::advance_body_frame_after_seal(
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
            if !reconciled
                .flow_state
                .fields
                .contains_key("ready_frame_membership")
            {
                let mut next_state = flow_run::initial_state().map_err(|error| {
                    MobError::Internal(format!("flow_run initial_state failed: {error:?}"))
                })?;
                next_state.phase = "Running".into();
                next_state.fields.insert(
                    "max_frame_depth".into(),
                    KernelValue::U64(self.max_frame_depth as u64),
                );
                next_state.fields.insert(
                    "max_active_frames".into(),
                    KernelValue::U64(self.max_active_frames as u64),
                );
                reconciled.flow_state = next_state;
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
                        }
                        Some(FlowNodeSpec::RepeatUntil(_)) => {
                            let loop_instance_id =
                                LoopInstanceId::from(format!("{frame_id}::{node_id}").as_str());
                            if let Some(loop_snapshot) = run.loops.get(&loop_instance_id) {
                                if let Some(decision) =
                                    FlowFrameLoopDriver::recover_terminal_loop_projection(
                                        &run.flow_state,
                                        loop_snapshot,
                                        &frame,
                                    )?
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
                let awaiting_until =
                    FlowFrameLoopDriver::pending_until_obligation(loop_snapshot)?.is_some();
                if frame.kernel_state.phase == "Running" || (!active_body_owner && !awaiting_until)
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
            let plan = FlowFrameLoopDriver::release_node_execution(&run.flow_state, frame_id)?;
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
        if let Some(plan) = &decision.store_plan
            && !self.execute_store_plan(run_id, plan).await?
        {
            return Ok(false);
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

    async fn resolve_until_feedback(
        &self,
        run_id: &RunId,
        root_frame_id: &FrameId,
        root_spec: &FrameSpec,
        context: &FlowContext,
        obligation: crate::generated::protocol_flow_loop_until_evaluation::FlowLoopUntilEvaluationObligation,
    ) -> Result<(), MobError> {
        for _ in 0..=5usize {
            let run = self.require_run(run_id).await?;
            let Some(loop_snapshot) = run.loops.get(&obligation.loop_instance_id) else {
                return Ok(());
            };
            if FlowFrameLoopDriver::pending_until_obligation(loop_snapshot)?.is_none() {
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
            let decision = FlowFrameLoopDriver::resolve_until_feedback(
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

fn string_from_state_field(
    kernel_state: &meerkat_machine_kernels::KernelState,
    field: &str,
) -> Result<String, MobError> {
    match kernel_state.fields.get(field) {
        Some(KernelValue::String(value)) => Ok(value.clone()),
        other => Err(MobError::Internal(format!(
            "kernel state missing String field '{field}': {other:?}"
        ))),
    }
}

fn u64_from_state_field(
    kernel_state: &meerkat_machine_kernels::KernelState,
    field: &str,
) -> Result<u64, MobError> {
    match kernel_state.fields.get(field) {
        Some(KernelValue::U64(value)) => Ok(*value),
        other => Err(MobError::Internal(format!(
            "kernel state missing U64 field '{field}': {other:?}"
        ))),
    }
}

fn frame_is_body(kernel_state: &meerkat_machine_kernels::KernelState) -> bool {
    matches!(
        kernel_state.fields.get("frame_scope"),
        Some(KernelValue::NamedVariant { variant, .. }) if variant == "Body"
    )
}

fn frame_loop_instance_id(
    kernel_state: &meerkat_machine_kernels::KernelState,
) -> Option<LoopInstanceId> {
    match kernel_state.fields.get("loop_instance_id") {
        Some(KernelValue::String(loop_instance_id)) if !loop_instance_id.is_empty() => {
            Some(LoopInstanceId::from(loop_instance_id.as_str()))
        }
        _ => None,
    }
}

fn frame_iteration(kernel_state: &meerkat_machine_kernels::KernelState) -> Result<u64, MobError> {
    u64_from_state_field(kernel_state, "iteration")
}

fn loop_parent_frame_id(
    kernel_state: &meerkat_machine_kernels::KernelState,
) -> Result<FrameId, MobError> {
    string_from_state_field(kernel_state, "parent_frame_id")
        .map(|value| FrameId::from(value.as_str()))
}

fn loop_parent_node_id(
    kernel_state: &meerkat_machine_kernels::KernelState,
) -> Result<FlowNodeId, MobError> {
    string_from_state_field(kernel_state, "parent_node_id")
        .map(|value| FlowNodeId::from(value.as_str()))
}

fn loop_id_from_state(
    kernel_state: &meerkat_machine_kernels::KernelState,
) -> Result<LoopId, MobError> {
    string_from_state_field(kernel_state, "loop_id").map(|value| LoopId::from(value.as_str()))
}

fn loop_depth(kernel_state: &meerkat_machine_kernels::KernelState) -> Result<u32, MobError> {
    let value = u64_from_state_field(kernel_state, "depth")?;
    u32::try_from(value).map_err(|_| {
        MobError::Internal(format!(
            "kernel state field 'depth' value {value} exceeds u32::MAX"
        ))
    })
}

fn usize_from_u64(value: u64, context: &str) -> Result<usize, MobError> {
    usize::try_from(value).map_err(|_| {
        MobError::Internal(format!(
            "{context} value {value} exceeds usize::MAX on this target"
        ))
    })
}

fn active_body_frame_id(kernel_state: &meerkat_machine_kernels::KernelState) -> Option<FrameId> {
    match kernel_state.fields.get("active_body_frame_id") {
        Some(KernelValue::String(frame_id)) if !frame_id.is_empty() => {
            Some(FrameId::from(frame_id.as_str()))
        }
        _ => None,
    }
}

/// Collect the IDs of all nodes whose `node_status` is `Running` in the given
/// kernel state. Used to detect orphaned in-flight nodes after a process crash.
fn running_node_ids(kernel_state: &meerkat_machine_kernels::KernelState) -> Vec<FlowNodeId> {
    match kernel_state.fields.get("node_status") {
        Some(KernelValue::Map(map)) => map
            .iter()
            .filter_map(|(key, status)| {
                if matches!(status, KernelValue::NamedVariant { variant, .. } if variant == "Running")
                    && let KernelValue::String(id) = key
                {
                    return Some(FlowNodeId::from(id.as_str()));
                }
                None
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn collect_frame_step_statuses(
    spec: &FrameSpec,
    kernel_state: &meerkat_machine_kernels::KernelState,
) -> IndexMap<StepId, StepRunStatus> {
    let Some(KernelValue::Map(status_map)) = kernel_state.fields.get("node_status") else {
        return IndexMap::new();
    };

    let mut step_statuses = IndexMap::new();
    for (node_id, node_spec) in &spec.nodes {
        let FlowNodeSpec::Step(step_spec) = node_spec else {
            continue;
        };
        let Some(status) = status_map.get(&KernelValue::String(node_id.to_string())) else {
            continue;
        };
        let Some(step_status) = node_terminal_step_status(status) else {
            continue;
        };
        merge_step_status(&mut step_statuses, step_spec.step_id.clone(), step_status);
    }
    step_statuses
}

fn node_terminal_step_status(value: &KernelValue) -> Option<StepRunStatus> {
    match value {
        KernelValue::NamedVariant { variant, .. } if variant == "Completed" => {
            Some(StepRunStatus::Completed)
        }
        KernelValue::NamedVariant { variant, .. } if variant == "Skipped" => {
            Some(StepRunStatus::Skipped)
        }
        KernelValue::NamedVariant { variant, .. } if variant == "Failed" => {
            Some(StepRunStatus::Failed)
        }
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

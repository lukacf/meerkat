//! FlowFrameEngine: drives a FrameSpec to completion by stepping through
//! the FlowFrameKernel's admit/complete cycle.
//!
//! This executor handles:
//! - Step nodes: delegates to `FrameStepExecutor`, records outputs.
//! - Loop nodes: runs body frame iterations until the `until` condition is met
//!   or `max_iterations` is exhausted, storing per-iteration outputs.

use crate::definition::{ConditionExpr, FlowNodeSpec, FrameSpec};
use crate::error::MobError;
use crate::ids::{FlowNodeId, FrameId, LoopId, LoopInstanceId, RunId, StepId};
use crate::run::{FlowContext, LoopContextHistory};
use crate::runtime::conditions::evaluate_condition;
use crate::runtime::flow_frame_kernel::FlowFrameKernel;
use crate::store::MobRunStore;
use async_trait::async_trait;
use indexmap::IndexMap;
use meerkat_machine_kernels::KernelValue;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

// ─── FrameStepExecutor ───────────────────────────────────────────────────────

/// Trait for executing a single step node within a frame.
///
/// Implementors provide the actual work (e.g., LLM turn, mock scripted output).
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait FrameStepExecutor: Send + Sync {
    async fn execute_step(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        node_id: &FlowNodeId,
        step_id: &StepId,
        context: &FlowContext,
    ) -> Result<serde_json::Value, MobError>;
}

// ─── LoopResult ─────────────────────────────────────────────────────────────

/// Outcome of executing a repeat_until loop body.
enum LoopResult {
    /// The `until` condition was met; contains all iteration outputs.
    ConditionMet(Vec<IndexMap<StepId, serde_json::Value>>),
    /// `max_iterations` exhausted without the condition being met.
    Exhausted,
}

// ─── FlowFrameEngine ─────────────────────────────────────────────────────────

/// Drives a `FrameSpec` to completion using a `FrameStepExecutor` for step work
/// and recursive body-frame execution for loop nodes.
pub struct FlowFrameEngine {
    run_store: Arc<dyn MobRunStore>,
    executor: Arc<dyn FrameStepExecutor>,
    frame_kernel: Arc<FlowFrameKernel>,
}

impl FlowFrameEngine {
    pub fn new(run_store: Arc<dyn MobRunStore>, executor: Arc<dyn FrameStepExecutor>) -> Self {
        let frame_kernel = Arc::new(FlowFrameKernel::new(run_store.clone()));
        Self {
            run_store,
            executor,
            frame_kernel,
        }
    }

    /// Execute a `FrameSpec` to completion.
    ///
    /// Returns when all nodes in the frame have reached a terminal state.
    #[allow(clippy::type_complexity)]
    pub fn execute_frame<'a>(
        &'a self,
        run_id: &'a RunId,
        frame_id: &'a FrameId,
        spec: &'a FrameSpec,
        context: &'a FlowContext,
    ) -> Pin<
        Box<dyn Future<Output = Result<IndexMap<StepId, serde_json::Value>, MobError>> + Send + 'a>,
    > {
        Box::pin(self.execute_frame_inner(run_id, frame_id, spec, context))
    }

    async fn execute_frame_inner(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        spec: &FrameSpec,
        context: &FlowContext,
    ) -> Result<IndexMap<StepId, serde_json::Value>, MobError> {
        use crate::runtime::flow_frame_kernel::FlowFrameMutator;

        // Initialize the frame via the kernel (computes topo order + StartFrame).
        self.frame_kernel
            .start_frame(run_id, frame_id, spec)
            .await?;

        // Accumulate step outputs for this frame.
        let mut frame_outputs: IndexMap<StepId, serde_json::Value> = IndexMap::new();

        // Rebuild context from stored outputs (needed for resume correctness).
        let mut local_context = if let Ok(Some(run)) = self.run_store.get_run(run_id).await {
            let rebuilt = FlowContext::from_run_aggregate(
                &run,
                run_id.clone(),
                context.activation_params.clone(),
            );
            // Merge any pre-existing outputs from the caller context that aren't
            // yet persisted (e.g. from a parent frame).
            let mut ctx = rebuilt;
            for (k, v) in &context.step_outputs {
                ctx.step_outputs
                    .entry(k.clone())
                    .or_insert_with(|| v.clone());
            }
            for (k, v) in &context.loop_outputs {
                ctx.loop_outputs
                    .entry(k.clone())
                    .or_insert_with(|| v.clone());
            }
            ctx
        } else {
            context.clone()
        };

        // Pump the admit loop until the frame is terminal.
        loop {
            let effects_opt = self
                .frame_kernel
                .admit_next_ready_node_with_retry(run_id, frame_id, 5)
                .await?;
            match effects_opt {
                None => {
                    // No more work from this admit — check if frame is done.
                    let snap = self.require_frame(run_id, frame_id).await?;
                    if snap.kernel_state.phase == "Completed"
                        || snap.kernel_state.phase != "Running"
                    {
                        break;
                    }
                    // No effects but still Running — all remaining nodes are
                    // either Running (waiting) or there are no more ready nodes.
                    // In a sequential executor we can break since we've exhausted
                    // all admissible work for now.
                    break;
                }
                Some(effects) => {
                    for effect in &effects {
                        if effect.variant == "AdmitStepWork" {
                            let node_id_str = string_from_effect_field(effect, "node_id")?;
                            let node_id = FlowNodeId::from(node_id_str.as_str());

                            // Find the step_id for this node.
                            let step_id = match spec.nodes.get(&node_id) {
                                Some(FlowNodeSpec::Step(s)) => s.step_id.clone(),
                                _ => {
                                    return Err(MobError::Internal(format!(
                                        "node '{node_id}' is not a step node"
                                    )));
                                }
                            };

                            // Execute the step.
                            let output = self
                                .executor
                                .execute_step(run_id, frame_id, &node_id, &step_id, &local_context)
                                .await;

                            match output {
                                Ok(out) => {
                                    // Record output.
                                    frame_outputs.insert(step_id.clone(), out.clone());
                                    local_context
                                        .step_outputs
                                        .insert(step_id.clone(), out.clone());

                                    // CAS-complete the node + record output via kernel.
                                    self.frame_kernel
                                        .complete_node_with_output(
                                            run_id, frame_id, &node_id, &step_id, out,
                                            None, // root frame step — no loop context
                                            5,
                                        )
                                        .await?;
                                }
                                Err(err) => {
                                    // Fail the node via kernel.
                                    self.frame_kernel
                                        .fail_node(run_id, frame_id, &node_id)
                                        .await?;
                                    return Err(err);
                                }
                            }
                        } else if effect.variant == "StartLoopNode" {
                            let node_id_str = string_from_effect_field(effect, "node_id")?;
                            let node_id = FlowNodeId::from(node_id_str.as_str());

                            // Find the RepeatUntil spec.
                            let loop_spec = match spec.nodes.get(&node_id) {
                                Some(FlowNodeSpec::RepeatUntil(l)) => l.clone(),
                                _ => {
                                    return Err(MobError::Internal(format!(
                                        "node '{node_id}' is not a loop node"
                                    )));
                                }
                            };

                            // Execute the loop body iteratively.
                            let loop_instance_id =
                                LoopInstanceId::from(format!("{frame_id}-{node_id}").as_str());
                            let loop_id = loop_spec.loop_id.clone();

                            let loop_result = self
                                .execute_loop(
                                    run_id,
                                    &loop_instance_id,
                                    &loop_id,
                                    &loop_spec.body,
                                    &loop_spec.until,
                                    loop_spec.max_iterations,
                                    &local_context,
                                )
                                .await?;

                            match loop_result {
                                LoopResult::ConditionMet(all_iter_outputs) => {
                                    // Update local_context with loop iteration history.
                                    let loop_history = LoopContextHistory {
                                        iterations: all_iter_outputs,
                                    };
                                    local_context
                                        .loop_outputs
                                        .insert(loop_id.clone(), loop_history);

                                    // Complete the loop node in the parent frame.
                                    self.frame_kernel
                                        .complete_node(run_id, frame_id, &node_id)
                                        .await?;
                                }
                                LoopResult::Exhausted => {
                                    // Fail the loop node — max iterations exhausted.
                                    self.frame_kernel
                                        .fail_node(run_id, frame_id, &node_id)
                                        .await?;
                                    return Err(MobError::Internal(format!(
                                        "repeat_until loop '{loop_id}' exhausted max iterations ({max})",
                                        max = loop_spec.max_iterations
                                    )));
                                }
                            }
                        }
                        // Other effects (e.g. ReadyFrontierChanged, FrameTerminalized) are
                        // informational; we continue the admit loop.
                    }

                    // After processing effects, check if we need to terminalize.
                    let snap = self.require_frame(run_id, frame_id).await?;
                    if snap.kernel_state.phase == "Running"
                        && self.all_nodes_terminal(&snap.kernel_state, spec)
                    {
                        // Terminalize.
                        self.frame_kernel
                            .terminalize_frame(run_id, frame_id)
                            .await?;
                        break;
                    }
                }
            }
        }

        Ok(frame_outputs)
    }

    /// Execute a repeat_until loop body iteratively until the condition is met
    /// or max_iterations is exhausted.
    #[allow(clippy::too_many_arguments, clippy::type_complexity)]
    fn execute_loop<'a>(
        &'a self,
        run_id: &'a RunId,
        loop_instance_id: &'a LoopInstanceId,
        loop_id: &'a LoopId,
        body_spec: &'a FrameSpec,
        until: &'a ConditionExpr,
        max_iterations: u32,
        context: &'a FlowContext,
    ) -> Pin<Box<dyn Future<Output = Result<LoopResult, MobError>> + Send + 'a>> {
        Box::pin(self.execute_loop_inner(
            run_id,
            loop_instance_id,
            loop_id,
            body_spec,
            until,
            max_iterations,
            context,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    async fn execute_loop_inner(
        &self,
        run_id: &RunId,
        loop_instance_id: &LoopInstanceId,
        loop_id: &LoopId,
        body_spec: &FrameSpec,
        until: &ConditionExpr,
        max_iterations: u32,
        context: &FlowContext,
    ) -> Result<LoopResult, MobError> {
        let mut all_iter_outputs: Vec<IndexMap<StepId, serde_json::Value>> = Vec::new();
        let mut iter_context = context.clone();
        let mut condition_met = false;

        for iteration in 0..max_iterations as u64 {
            let body_frame_id =
                FrameId::from(format!("{loop_instance_id}-iter-{iteration}").as_str());

            let iter_outputs = self
                .execute_frame(run_id, &body_frame_id, body_spec, &iter_context)
                .await?;

            // Add this iteration's outputs to the history.
            all_iter_outputs.push(iter_outputs.clone());

            // Update the context with this iteration's outputs for condition eval.
            // We add to loop_outputs so the until condition can reference them.
            let history = LoopContextHistory {
                iterations: all_iter_outputs.clone(),
            };
            iter_context.loop_outputs.insert(loop_id.clone(), history);

            // Also merge step outputs from this iteration into the root context
            // (so sibling nodes after the loop can reference them).
            for (step_id, output) in &iter_outputs {
                iter_context
                    .step_outputs
                    .insert(step_id.clone(), output.clone());
            }

            // Evaluate the until condition.
            if evaluate_condition(until, &iter_context) {
                condition_met = true;
                break;
            }
        }

        if condition_met {
            Ok(LoopResult::ConditionMet(all_iter_outputs))
        } else {
            Ok(LoopResult::Exhausted)
        }
    }

    /// Read the current frame snapshot.
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

    /// Check if all tracked nodes in a frame have terminal status.
    fn all_nodes_terminal(
        &self,
        state: &meerkat_machine_kernels::KernelState,
        spec: &FrameSpec,
    ) -> bool {
        let status_map = match state.fields.get("node_status") {
            Some(KernelValue::Map(m)) => m,
            _ => return false,
        };
        let terminal = ["Completed", "Failed", "Skipped", "Canceled"];
        for node_id in spec.nodes.keys() {
            let key = KernelValue::String(node_id.to_string());
            match status_map.get(&key) {
                Some(KernelValue::NamedVariant { variant, .. }) => {
                    if !terminal.contains(&variant.as_str()) {
                        return false;
                    }
                }
                _ => return false,
            }
        }
        true
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Extract a String from a named field of a `KernelEffect`.
fn string_from_effect_field(
    effect: &meerkat_machine_kernels::KernelEffect,
    field: &str,
) -> Result<String, MobError> {
    match effect.fields.get(field) {
        Some(KernelValue::String(s)) => Ok(s.clone()),
        other => Err(MobError::Internal(format!(
            "effect '{}' missing String field '{}': {other:?}",
            effect.variant, field
        ))),
    }
}

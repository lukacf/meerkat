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
use crate::runtime::flow_frame_kernel::{FlowFrameKernel, FlowFrameMutator, StepCompletionOpts};
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
    ) -> Pin<
        Box<dyn Future<Output = Result<IndexMap<StepId, serde_json::Value>, MobError>> + Send + 'a>,
    > {
        Box::pin(self.execute_frame_inner(run_id, frame_id, spec, context, None))
    }

    // ─── Private helpers ─────────────────────────────────────────────────────

    /// Execute a frame, with an optional loop context for body frames.
    ///
    /// `loop_context` is `None` for root frames and `Some((loop_id, iteration))`
    /// for frames that are executing as a loop iteration body. This is threaded
    /// into `cas_complete_step_and_record_output` so each step's output lands in
    /// the correct field (`root_step_outputs` vs `loop_iteration_outputs`).
    async fn execute_frame_inner(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        spec: &FrameSpec,
        context: &FlowContext,
        loop_context: Option<(LoopId, u64)>,
    ) -> Result<IndexMap<StepId, serde_json::Value>, MobError> {
        // Initialize the frame via the kernel (computes topo order + StartFrame).
        self.frame_kernel
            .start_frame(run_id, frame_id, spec)
            .await?;

        // Rebuild context from stored outputs for resume correctness.
        // Propagate store errors — a failure here means the run is in a bad state.
        let run = self
            .run_store
            .get_run(run_id)
            .await?
            .ok_or_else(|| MobError::RunNotFound(run_id.clone()))?;
        let mut local_context = {
            let rebuilt = FlowContext::from_run_aggregate(
                &run,
                run_id.clone(),
                context.activation_params.clone(),
            );
            // Merge any caller-provided outputs not yet persisted (e.g. from a parent frame).
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
        };

        // Accumulate step outputs for this frame.
        let mut frame_outputs: IndexMap<StepId, serde_json::Value> = IndexMap::new();

        // Pump the admit loop until the frame is terminal.
        loop {
            let effects_opt = self
                .frame_kernel
                .admit_next_ready_node_with_retry(run_id, frame_id, 5)
                .await?;
            match effects_opt {
                None => {
                    // Queue empty — check if frame is fully done.
                    let snap = self.require_frame(run_id, frame_id).await?;
                    if snap.kernel_state.phase != "Running" {
                        break;
                    }
                    // Sequential executor: if queue is empty and still Running,
                    // all remaining nodes are either Running or blocked — break.
                    break;
                }
                Some(effects) => {
                    for effect in &effects {
                        if effect.variant == "AdmitStepWork" {
                            let node_id_str = string_from_effect_field(effect, "node_id")?;
                            let node_id = FlowNodeId::from(node_id_str.as_str());

                            // Resolve the step_id from the frame spec.
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
                                    // Record output in the local in-memory context.
                                    frame_outputs.insert(step_id.clone(), out.clone());
                                    local_context
                                        .step_outputs
                                        .insert(step_id.clone(), out.clone());

                                    // CAS-complete the node + route the output to the
                                    // correct store field (root vs loop body).
                                    let lc = loop_context.as_ref().map(|(lid, iter)| (lid, *iter));
                                    self.frame_kernel
                                        .complete_step(
                                            run_id,
                                            frame_id,
                                            StepCompletionOpts {
                                                node_id: &node_id,
                                                step_id: &step_id,
                                                output: out,
                                                loop_context: lc,
                                                max_retries: 5,
                                            },
                                        )
                                        .await?;
                                }
                                Err(err) => {
                                    self.frame_kernel
                                        .fail_node(run_id, frame_id, &node_id)
                                        .await?;
                                    return Err(err);
                                }
                            }
                        } else if effect.variant == "StartLoopNode" {
                            let node_id_str = string_from_effect_field(effect, "node_id")?;
                            let node_id = FlowNodeId::from(node_id_str.as_str());

                            // Resolve the RepeatUntil spec.
                            let loop_spec = match spec.nodes.get(&node_id) {
                                Some(FlowNodeSpec::RepeatUntil(l)) => l.clone(),
                                _ => {
                                    return Err(MobError::Internal(format!(
                                        "node '{node_id}' is not a loop node"
                                    )));
                                }
                            };

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
                                    // Update local_context with the completed loop history.
                                    local_context.loop_outputs.insert(
                                        loop_id.clone(),
                                        LoopContextHistory {
                                            iterations: all_iter_outputs,
                                        },
                                    );
                                    self.frame_kernel
                                        .complete_node(run_id, frame_id, &node_id)
                                        .await?;
                                }
                                LoopResult::Exhausted => {
                                    self.frame_kernel
                                        .fail_node(run_id, frame_id, &node_id)
                                        .await?;
                                    return Err(MobError::Internal(format!(
                                        "repeat_until loop '{loop_id}' exhausted \
                                         max iterations ({})",
                                        loop_spec.max_iterations
                                    )));
                                }
                            }
                        }
                        // Other effects (ReadyFrontierChanged, FrameTerminalized) are
                        // informational — continue the admit loop.
                    }

                    // Terminalize once all nodes are done.
                    let snap = self.require_frame(run_id, frame_id).await?;
                    if snap.kernel_state.phase == "Running"
                        && self.all_nodes_terminal(&snap.kernel_state, spec)
                    {
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

    // ─── Loop execution ──────────────────────────────────────────────────────

    /// Execute a repeat_until loop body iteratively.
    ///
    /// Returns `ConditionMet(outputs)` when the until condition is satisfied, or
    /// `Exhausted` when max_iterations is reached without the condition being met.
    ///
    /// The `Pin<Box<...>>` is needed because `execute_loop_inner` calls
    /// `execute_frame_inner` which in turn calls `execute_loop`, creating a
    /// recursive async call chain that cannot be represented as a plain `async fn`.
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
        let mut iter_context = context.clone();
        let mut condition_met = false;

        for iteration in 0..max_iterations as u64 {
            let body_frame_id =
                FrameId::from(format!("{loop_instance_id}-iter-{iteration}").as_str());

            // Execute the body frame with loop context so each step's output is
            // routed to loop_iteration_outputs[loop_id][iteration] in the store.
            let iter_outputs = self
                .execute_frame_inner(
                    run_id,
                    &body_frame_id,
                    body_spec,
                    &iter_context,
                    Some((loop_id.clone(), iteration)),
                )
                .await?;

            // Merge this iteration's step outputs into the rolling context so
            // sibling nodes after the loop can reference them.
            for (step_id, output) in &iter_outputs {
                iter_context
                    .step_outputs
                    .insert(step_id.clone(), output.clone());
            }

            // Append this iteration's outputs to the loop history in O(1) — we
            // push a single IndexMap rather than cloning the entire history Vec.
            // The history is used for condition evaluation and returned on success.
            //
            // TODO: consider compacting per-iteration history after N iterations
            // if loop counts grow large, to prevent unbounded MobRun growth.
            iter_context
                .loop_outputs
                .entry(loop_id.clone())
                .or_insert_with(|| LoopContextHistory {
                    iterations: Vec::new(),
                })
                .iterations
                .push(iter_outputs);

            // Evaluate the until condition against the updated context.
            if evaluate_condition(until, &iter_context) {
                condition_met = true;
                break;
            }
        }

        // Extract the accumulated iteration outputs from the context.
        let all_iter_outputs = iter_context
            .loop_outputs
            .shift_remove(loop_id)
            .map(|h| h.iterations)
            .unwrap_or_default();

        if condition_met {
            Ok(LoopResult::ConditionMet(all_iter_outputs))
        } else {
            Ok(LoopResult::Exhausted)
        }
    }

    // ─── Frame snapshot helpers ──────────────────────────────────────────────

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

    /// Return `true` if every tracked node in the frame has a terminal status.
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

/// Extract a `String` from a named field of a `KernelEffect`.
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

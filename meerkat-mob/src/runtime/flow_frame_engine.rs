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
use crate::run::{FlowContext, LoopContextHistory, LoopIterationLedgerEntry, LoopSnapshot};
use crate::runtime::conditions::evaluate_condition;
use crate::runtime::flow_frame_kernel::{FlowFrameKernel, FlowFrameMutator, StepCompletionOpts};
use crate::store::MobRunStore;
use async_trait::async_trait;
use indexmap::IndexMap;
use meerkat_machine_kernels::KernelState;
use meerkat_machine_kernels::KernelValue;
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
}

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
    ) -> Result<FrameStepResult, MobError>;
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
    /// Maximum nesting depth for body frames. 0 means unlimited.
    max_frame_depth: u32,
    /// Maximum number of simultaneously active body frames (each loop level adds one).
    /// 0 means unlimited. In the sequential executor, active_body_frames == depth, so
    /// this is enforced at the same site as max_frame_depth.
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
    ) -> Pin<
        Box<dyn Future<Output = Result<IndexMap<StepId, serde_json::Value>, MobError>> + Send + 'a>,
    > {
        Box::pin(self.execute_frame_inner(run_id, frame_id, spec, context, None, 0))
    }

    // ─── Private helpers ─────────────────────────────────────────────────────

    /// Execute a frame, with an optional loop context for body frames.
    ///
    /// `loop_context` is `None` for root frames and `Some((loop_id, iteration))`
    /// for frames that are executing as a loop iteration body. This is threaded
    /// into `cas_complete_step_and_record_output` so each step's output lands in
    /// the correct field (`root_step_outputs` vs `loop_iteration_outputs`).
    ///
    /// `depth` tracks the current nesting depth (0 = root frame). Used to enforce
    /// `max_frame_depth` before recursing into a loop body frame.
    async fn execute_frame_inner(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        spec: &FrameSpec,
        context: &FlowContext,
        loop_context: Option<(LoopId, u64)>,
        depth: u32,
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
            // [P2] When resuming a body frame mid-iteration, merge the step outputs
            // already persisted for THIS iteration slot into step_outputs. Without this,
            // a step that completed before the crash is invisible to sibling steps that
            // run after the resume — conditions/templates inside the same iteration
            // diverge from the pre-crash path (dogma Rule 13).
            if let Some((loop_id, iteration)) = &loop_context
                && let Some(iters) = run.loop_iteration_outputs.get(loop_id)
                && let Some(iter_out) = iters.get(*iteration as usize)
            {
                for (sid, out) in iter_out {
                    ctx.step_outputs
                        .entry(sid.clone())
                        .or_insert_with(|| out.clone());
                }
            }
            ctx
        };

        // [P2] Seed frame_outputs from the persisted store — not from a local empty map.
        // On a fresh run this is empty. On a resume it reflects steps that completed
        // before the crash, so advance_frame_steps_and_terminalize correctly marks
        // those steps Completed rather than Skipped.
        // Root frames read from root_step_outputs; body frames read from the correct
        // loop_iteration_outputs slot.
        let mut frame_outputs: IndexMap<StepId, serde_json::Value> = match &loop_context {
            None => run.root_step_outputs.clone(),
            Some((loop_id, iteration)) => run
                .loop_iteration_outputs
                .get(loop_id)
                .and_then(|iters| iters.get(*iteration as usize))
                .cloned()
                .unwrap_or_default(),
        };

        // [P1] Fail any Running nodes left by a previous process crash.
        // When a node is admitted (popped from ready_queue → Running) and the
        // process dies before the step completes, the persisted frame has an
        // empty ready_queue with at least one Running node. Those in-flight
        // step handles are gone — we fail the nodes now so that their dependents
        // become eligible for skip/fail admission in the loop below.
        // Ref: dogma Rule 3 — the machine's node_status is the authoritative
        // source of which nodes are running; do not infer completion from queue size.
        loop {
            let snap = self.require_frame(run_id, frame_id).await?;
            let running = running_node_ids(&snap.kernel_state);
            if running.is_empty() {
                break;
            }
            for node_id in &running {
                self.frame_kernel
                    .fail_node(run_id, frame_id, node_id)
                    .await?;
            }
        }

        // Pump the admit loop until the frame is terminal.
        //
        // Terminalization invariant: after the loop exits (via either `None` or the
        // in-loop `break`), we attempt to terminalize the frame. This covers two cases:
        //   1. Empty frames: `admit_next_ready_node_with_retry` returns `None` immediately
        //      because the ready_queue starts empty (no nodes → all nodes are already
        //      terminal vacuously). The in-loop terminalize never fires; the post-loop
        //      check below catches this.
        //   2. Normal frames: all nodes complete inside the loop and the in-loop
        //      terminalize already fired before the `break`. The post-loop check is a
        //      no-op (frame is already Completed, not Running).
        loop {
            let effects_opt = self
                .frame_kernel
                .admit_next_ready_node_with_retry(run_id, frame_id, 5)
                .await?;
            match effects_opt {
                None => {
                    // Queue empty — frame is done or every remaining node is
                    // blocked on a Running predecessor. In the sequential
                    // executor neither case can make progress; break and
                    // terminalize below if all nodes are terminal.
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
                                Ok(FrameStepResult::Completed(out)) => {
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
                                Ok(FrameStepResult::Skipped) => {
                                    // Condition was false — skip the node in the frame machine.
                                    self.frame_kernel
                                        .skip_node(run_id, frame_id, &node_id)
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

                            // Use "::" as the separator so loop_instance_id can never
                            // collide with a node whose name happens to contain "-iter-".
                            let loop_instance_id =
                                LoopInstanceId::from(format!("{frame_id}::{node_id}").as_str());
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
                                    depth,
                                )
                                .await?;

                            match loop_result {
                                LoopResult::ConditionMet(all_iter_outputs) => {
                                    // Merge the UNION of all iterations into frame_outputs
                                    // (dogma Rule 13: frame_outputs must reflect all steps
                                    // that ever completed, not just the final iteration).
                                    // advance_frame_steps_and_terminalize uses frame_outputs.keys()
                                    // to classify steps as Completed vs Skipped — a step that
                                    // completed in iteration 0 but was absent in the terminating
                                    // iteration must still be Completed, not Skipped.
                                    // Iteration outputs are accumulated in forward order; later
                                    // iterations overwrite earlier ones so the final value wins.
                                    for iter in &all_iter_outputs {
                                        for (sid, out) in iter {
                                            frame_outputs.insert(sid.clone(), out.clone());
                                        }
                                    }
                                    // local_context.step_outputs uses the last iteration only —
                                    // downstream template/condition evaluation should see the
                                    // most recent value, not all historical values.
                                    if let Some(last_iter) = all_iter_outputs.last() {
                                        for (sid, out) in last_iter {
                                            local_context
                                                .step_outputs
                                                .insert(sid.clone(), out.clone());
                                        }
                                    }
                                    // Also record the full iteration history in loop_outputs
                                    // so loops.<id>.iterations.<n>.steps.<step>... paths work.
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

        // Post-loop terminalize: catches the empty-frame case (queue was immediately
        // empty so no in-loop terminalize ever fired) and acts as a safety net for
        // any other case where the in-loop check was bypassed. Double-terminalizing is
        // safe: `terminalize_frame` returns `false` (no-op) when the frame is already
        // in a terminal phase.
        let snap = self.require_frame(run_id, frame_id).await?;
        if snap.kernel_state.phase == "Running" && self.all_nodes_terminal(&snap.kernel_state, spec)
        {
            self.frame_kernel
                .terminalize_frame(run_id, frame_id)
                .await?;
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
        depth: u32,
    ) -> Pin<Box<dyn Future<Output = Result<LoopResult, MobError>> + Send + 'a>> {
        Box::pin(self.execute_loop_inner(
            run_id,
            loop_instance_id,
            loop_id,
            body_spec,
            until,
            max_iterations,
            context,
            depth,
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
        depth: u32,
    ) -> Result<LoopResult, MobError> {
        // Context isolation note: `iter_context` starts as a clone of the
        // parent frame's context at the moment the loop node is admitted.
        // In the sequential executor, every node that could possibly have
        // completed before the loop node (i.e. every transitive dependency)
        // is already reflected in `context.step_outputs` at this point.
        //
        // In a hypothetical parallel executor, sibling nodes that share no
        // dependency with the loop node may not have completed yet — their
        // outputs would be absent from `iter_context`. That is intentional:
        // loop bodies are logically encapsulated and should not race on
        // sibling output availability.
        let mut iter_context = context.clone();
        let mut condition_met = false;

        // Resume from the persisted iteration counter (dogma Rule 13: the store is
        // authoritative for how far the loop progressed). On a fresh run, no
        // LoopSnapshot exists so start_iteration == 0. On a resume, earlier iterations
        // are already persisted — restarting from 0 would duplicate ledger entries
        // and replay stale body-frame IDs into loop_outputs.
        let start_iteration = {
            let run = self
                .run_store
                .get_run(run_id)
                .await?
                .ok_or_else(|| MobError::RunNotFound(run_id.clone()))?;
            run.loops
                .get(loop_instance_id)
                .and_then(|snap| {
                    snap.kernel_state
                        .fields
                        .get("current_iteration")
                        .and_then(|v| {
                            if let meerkat_machine_kernels::KernelValue::U64(n) = v {
                                Some(*n)
                            } else {
                                None
                            }
                        })
                })
                .unwrap_or(0)
        };

        // Hydrate iter_context with outputs from already-completed iterations so
        // the until-condition evaluator and downstream templates see the full history.
        if start_iteration > 0 {
            let run = self
                .run_store
                .get_run(run_id)
                .await?
                .ok_or_else(|| MobError::RunNotFound(run_id.clone()))?;
            if let Some(persisted_iters) = run.loop_iteration_outputs.get(loop_id) {
                for (i, iter_out) in persisted_iters.iter().enumerate() {
                    if (i as u64) < start_iteration {
                        // Merge into step_outputs (last write wins, like runtime).
                        for (sid, out) in iter_out {
                            iter_context.step_outputs.insert(sid.clone(), out.clone());
                        }
                        // Append to loop_outputs history.
                        iter_context
                            .loop_outputs
                            .entry(loop_id.clone())
                            .or_insert_with(|| LoopContextHistory {
                                iterations: Vec::new(),
                            })
                            .iterations
                            .push(iter_out.clone());
                    }
                }
            }
        }

        // Check depth and active-frame count before recursing into the loop body frame.
        // In the sequential executor, active_body_frames == depth (each recursion level
        // adds one body frame). Both limits apply at the same site.
        let next_depth = depth + 1;
        if self.max_frame_depth > 0 && next_depth > self.max_frame_depth {
            return Err(MobError::NotYetImplemented(format!(
                "loop '{}' would exceed max_frame_depth={} (current depth={}); \
                 nested loops require a higher limit in LimitsSpec.max_frame_depth",
                loop_id, self.max_frame_depth, depth
            )));
        }
        // max_active_frames: next_depth body frames would be simultaneously active.
        if self.max_active_frames > 0 && next_depth > self.max_active_frames {
            return Err(MobError::NotYetImplemented(format!(
                "loop '{}' would activate {} concurrent body frames, exceeding \
                 max_active_frames={}; raise LimitsSpec.max_active_frames to allow nesting",
                loop_id, next_depth, self.max_active_frames
            )));
        }

        for iteration in start_iteration..max_iterations as u64 {
            // Use "::" separator consistent with loop_instance_id construction.
            let body_frame_id =
                FrameId::from(format!("{loop_instance_id}::iter-{iteration}").as_str());

            // Persist the loop snapshot with active_body_frame_id so that
            // reconcile_run_state can reconstruct in-progress loop state after a
            // crash. Phase="Running", active_body_frame_id=Some(body_frame_id).
            self.run_store
                .upsert_loop_snapshot(
                    run_id,
                    loop_instance_id,
                    LoopSnapshot {
                        kernel_state: KernelState {
                            phase: "Running".into(),
                            fields: std::collections::BTreeMap::from([
                                (
                                    "loop_instance_id".into(),
                                    meerkat_machine_kernels::KernelValue::String(
                                        loop_instance_id.to_string(),
                                    ),
                                ),
                                (
                                    "current_iteration".into(),
                                    meerkat_machine_kernels::KernelValue::U64(iteration),
                                ),
                                (
                                    "max_iterations".into(),
                                    meerkat_machine_kernels::KernelValue::U64(
                                        max_iterations as u64,
                                    ),
                                ),
                                (
                                    "active_body_frame_id".into(),
                                    meerkat_machine_kernels::KernelValue::String(
                                        body_frame_id.to_string(),
                                    ),
                                ),
                            ]),
                        },
                    },
                    Some(LoopIterationLedgerEntry {
                        loop_instance_id: loop_instance_id.clone(),
                        iteration,
                        frame_id: body_frame_id.clone(),
                    }),
                )
                .await?;

            // Execute the body frame with loop context so each step's output is
            // routed to loop_iteration_outputs[loop_id][iteration] in the store.
            let iter_outputs = self
                .execute_frame_inner(
                    run_id,
                    &body_frame_id,
                    body_spec,
                    &iter_context,
                    Some((loop_id.clone(), iteration)),
                    next_depth,
                )
                .await?;

            // Mark body frame as complete: clear active_body_frame_id.
            self.run_store
                .upsert_loop_snapshot(
                    run_id,
                    loop_instance_id,
                    LoopSnapshot {
                        kernel_state: KernelState {
                            phase: "Running".into(),
                            fields: std::collections::BTreeMap::from([
                                (
                                    "loop_instance_id".into(),
                                    meerkat_machine_kernels::KernelValue::String(
                                        loop_instance_id.to_string(),
                                    ),
                                ),
                                (
                                    "current_iteration".into(),
                                    meerkat_machine_kernels::KernelValue::U64(iteration + 1),
                                ),
                                (
                                    "max_iterations".into(),
                                    meerkat_machine_kernels::KernelValue::U64(
                                        max_iterations as u64,
                                    ),
                                ),
                                (
                                    "active_body_frame_id".into(),
                                    meerkat_machine_kernels::KernelValue::None,
                                ),
                            ]),
                        },
                    },
                    None, // ledger entry already appended at iteration start
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

        // Extract the accumulated iteration outputs.
        // shift_remove preserves insertion order of the remaining map entries
        // (vs the deprecated remove which would disrupt order); since loop_outputs
        // is small this is negligible but avoids the deprecation warning.
        let all_iter_outputs = iter_context
            .loop_outputs
            .shift_remove(loop_id)
            .map(|h| h.iterations)
            .unwrap_or_default();

        // [P1] Persist terminal loop state before returning.
        // The last per-iteration snapshot leaves phase = "Running" with
        // active_body_frame_id = None. Without a final write, a crashed
        // process that restarts between loop completion and parent-frame
        // finalization would see a "Running/no active frame" snapshot and
        // enqueue the loop again via reconcile_pending_body_frame_loops.
        // Ref: dogma Rule 13 — the authoritative phase must be persisted;
        // it is not sufficient to leave it derivable from iteration count.
        let terminal_phase = if condition_met {
            "Completed"
        } else {
            "Exhausted"
        };
        self.run_store
            .upsert_loop_snapshot(
                run_id,
                loop_instance_id,
                LoopSnapshot {
                    kernel_state: KernelState {
                        phase: terminal_phase.into(),
                        fields: std::collections::BTreeMap::from([
                            (
                                "loop_instance_id".into(),
                                KernelValue::String(loop_instance_id.to_string()),
                            ),
                            (
                                "current_iteration".into(),
                                KernelValue::U64(all_iter_outputs.len() as u64),
                            ),
                            (
                                "max_iterations".into(),
                                KernelValue::U64(max_iterations as u64),
                            ),
                            ("active_body_frame_id".into(), KernelValue::None),
                        ]),
                    },
                },
                None,
            )
            .await?;

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

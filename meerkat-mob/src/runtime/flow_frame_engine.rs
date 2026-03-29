//! FlowFrameEngine: drives a FrameSpec to completion by stepping through
//! the FlowFrameKernel's admit/complete cycle.
//!
//! This executor handles:
//! - Step nodes: delegates to `FrameStepExecutor`, records outputs.
//! - Loop nodes: runs body frame iterations until the `until` condition is met
//!   or `max_iterations` is exhausted, storing per-iteration outputs.

use crate::definition::{ConditionExpr, DependencyMode, FlowNodeSpec, FrameSpec};
use crate::error::MobError;
use crate::ids::{FlowNodeId, FrameId, LoopId, LoopInstanceId, RunId, StepId};
use crate::run::{FlowContext, LoopContextHistory};
use crate::runtime::conditions::evaluate_condition;
use crate::store::MobRunStore;
use async_trait::async_trait;
use indexmap::IndexMap;
use meerkat_machine_kernels::generated::flow_frame;
use meerkat_machine_kernels::{KernelEffect, KernelInput, KernelValue};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
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

// ─── FlowFrameEngine ─────────────────────────────────────────────────────────

/// Drives a `FrameSpec` to completion using a `FrameStepExecutor` for step work
/// and recursive body-frame execution for loop nodes.
pub struct FlowFrameEngine {
    run_store: Arc<dyn MobRunStore>,
    executor: Arc<dyn FrameStepExecutor>,
}

impl FlowFrameEngine {
    pub fn new(run_store: Arc<dyn MobRunStore>, executor: Arc<dyn FrameStepExecutor>) -> Self {
        Self {
            run_store,
            executor,
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
        // Compute topological order for StartFrame input.
        let ordered = topological_order(spec)?;

        // Build StartFrame input from the FrameSpec.
        let start_input = build_start_frame_input(frame_id, spec, &ordered);

        // Initialize or retrieve frame state.
        let initial = flow_frame::initial_state()
            .map_err(|e| MobError::Internal(format!("flow_frame initial_state failed: {e:?}")))?;
        let start_outcome = flow_frame::transition(&initial, &start_input)
            .map_err(|e| MobError::Internal(format!("StartFrame failed: {e:?}")))?;

        // Store the new frame snapshot via the run store.
        let initial_snap = crate::run::FrameSnapshot {
            frame_id: frame_id.clone(),
            kernel_state: start_outcome.next_state.clone(),
        };
        let inserted = self
            .run_store
            .cas_frame_state(run_id, frame_id, None, initial_snap)
            .await?;
        if !inserted {
            return Err(MobError::Internal(format!(
                "frame '{frame_id}' already exists in run '{run_id}'"
            )));
        }

        // Accumulate step outputs for this frame.
        let mut frame_outputs: IndexMap<StepId, serde_json::Value> = IndexMap::new();
        let mut local_context = context.clone();

        // Pump the admit loop until the frame is terminal.
        loop {
            let effects_opt = self.admit_next(run_id, frame_id).await?;
            match effects_opt {
                None => {
                    // No more work from this admit — check if frame is done.
                    let snap = self.require_frame(run_id, frame_id).await?;
                    if snap.kernel_state.phase == "Completed" {
                        break;
                    }
                    // Phase is still Running but nothing ready — this means
                    // the queue is empty AND not all nodes terminal yet
                    // (should not happen in a sequential executor, but be safe).
                    if snap.kernel_state.phase != "Running" {
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

                                    // CAS-complete the node + record output.
                                    let snap = self.require_frame(run_id, frame_id).await?;
                                    let complete_input = KernelInput {
                                        variant: "CompleteNode".into(),
                                        fields: BTreeMap::from([(
                                            "node_id".into(),
                                            KernelValue::String(node_id.to_string()),
                                        )]),
                                    };
                                    let next_outcome =
                                        flow_frame::transition(&snap.kernel_state, &complete_input)
                                            .map_err(|e| {
                                                MobError::Internal(format!(
                                                    "CompleteNode failed: {e:?}"
                                                ))
                                            })?;
                                    let next_snap = crate::run::FrameSnapshot {
                                        frame_id: frame_id.clone(),
                                        kernel_state: next_outcome.next_state,
                                    };
                                    self.run_store
                                        .cas_complete_step_and_record_output(
                                            run_id,
                                            frame_id,
                                            &snap,
                                            next_snap,
                                            step_id.to_string(),
                                            out,
                                        )
                                        .await?;
                                }
                                Err(err) => {
                                    // Fail the node.
                                    let snap = self.require_frame(run_id, frame_id).await?;
                                    let fail_input = KernelInput {
                                        variant: "FailNode".into(),
                                        fields: BTreeMap::from([(
                                            "node_id".into(),
                                            KernelValue::String(node_id.to_string()),
                                        )]),
                                    };
                                    let next_outcome =
                                        flow_frame::transition(&snap.kernel_state, &fail_input)
                                            .map_err(|e| {
                                                MobError::Internal(format!(
                                                    "FailNode failed: {e:?}"
                                                ))
                                            })?;
                                    let next_snap = crate::run::FrameSnapshot {
                                        frame_id: frame_id.clone(),
                                        kernel_state: next_outcome.next_state,
                                    };
                                    self.run_store
                                        .cas_frame_state(run_id, frame_id, Some(&snap), next_snap)
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

                            let all_iter_outputs = self
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

                            // Update local_context with loop iteration history.
                            let loop_history = LoopContextHistory {
                                iterations: all_iter_outputs.clone(),
                            };
                            local_context
                                .loop_outputs
                                .insert(loop_id.clone(), loop_history);

                            // Complete the loop node in the parent frame.
                            let snap = self.require_frame(run_id, frame_id).await?;
                            let complete_input = KernelInput {
                                variant: "CompleteNode".into(),
                                fields: BTreeMap::from([(
                                    "node_id".into(),
                                    KernelValue::String(node_id.to_string()),
                                )]),
                            };
                            let next_outcome =
                                flow_frame::transition(&snap.kernel_state, &complete_input)
                                    .map_err(|e| {
                                        MobError::Internal(format!(
                                            "CompleteNode (loop) failed: {e:?}"
                                        ))
                                    })?;
                            let next_snap = crate::run::FrameSnapshot {
                                frame_id: frame_id.clone(),
                                kernel_state: next_outcome.next_state,
                            };
                            self.run_store
                                .cas_frame_state(run_id, frame_id, Some(&snap), next_snap)
                                .await?;
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
                        let term_input = KernelInput {
                            variant: "TerminalizeCompleted".into(),
                            fields: BTreeMap::new(),
                        };
                        let term_outcome = flow_frame::transition(&snap.kernel_state, &term_input)
                            .map_err(|e| {
                                MobError::Internal(format!("TerminalizeCompleted failed: {e:?}"))
                            })?;
                        let next_snap = crate::run::FrameSnapshot {
                            frame_id: frame_id.clone(),
                            kernel_state: term_outcome.next_state,
                        };
                        self.run_store
                            .cas_frame_state(run_id, frame_id, Some(&snap), next_snap)
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
    ) -> Pin<
        Box<
            dyn Future<Output = Result<Vec<IndexMap<StepId, serde_json::Value>>, MobError>>
                + Send
                + 'a,
        >,
    > {
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
    ) -> Result<Vec<IndexMap<StepId, serde_json::Value>>, MobError> {
        let mut all_iter_outputs: Vec<IndexMap<StepId, serde_json::Value>> = Vec::new();
        let mut iter_context = context.clone();

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
                break;
            }
        }

        Ok(all_iter_outputs)
    }

    /// Admit the next ready node in a frame. Returns the effects or None if queue empty.
    async fn admit_next(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
    ) -> Result<Option<Vec<KernelEffect>>, MobError> {
        let snap = self.require_frame(run_id, frame_id).await?;
        let queue_empty = match snap.kernel_state.fields.get("ready_queue") {
            Some(KernelValue::Seq(seq)) => seq.is_empty(),
            _ => true,
        };
        if queue_empty {
            return Ok(None);
        }

        let admit_input = KernelInput {
            variant: "AdmitNextReadyNode".into(),
            fields: BTreeMap::new(),
        };
        let outcome = flow_frame::transition(&snap.kernel_state, &admit_input)
            .map_err(|e| MobError::Internal(format!("AdmitNextReadyNode failed: {e:?}")))?;
        let next_snap = crate::run::FrameSnapshot {
            frame_id: frame_id.clone(),
            kernel_state: outcome.next_state,
        };
        let won = self
            .run_store
            .cas_frame_state(run_id, frame_id, Some(&snap), next_snap)
            .await?;
        if !won {
            // CAS lost — retry (simplified: return None to stop this pump cycle).
            return Ok(None);
        }
        Ok(Some(outcome.effects))
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

/// Build the `StartFrame` KernelInput from a `FrameSpec` and its topological order.
pub fn build_start_frame_input(
    frame_id: &FrameId,
    spec: &FrameSpec,
    ordered: &[FlowNodeId],
) -> KernelInput {
    let ordered_kv: Vec<KernelValue> = ordered
        .iter()
        .map(|n| KernelValue::String(n.to_string()))
        .collect();

    let tracked: BTreeSet<KernelValue> = ordered
        .iter()
        .map(|n| KernelValue::String(n.to_string()))
        .collect();

    let mut node_kind: BTreeMap<KernelValue, KernelValue> = BTreeMap::new();
    let mut node_deps: BTreeMap<KernelValue, KernelValue> = BTreeMap::new();
    let mut node_dep_modes: BTreeMap<KernelValue, KernelValue> = BTreeMap::new();
    let mut node_branches: BTreeMap<KernelValue, KernelValue> = BTreeMap::new();

    for (node_id, node_spec) in &spec.nodes {
        let k = KernelValue::String(node_id.to_string());
        match node_spec {
            FlowNodeSpec::Step(s) => {
                node_kind.insert(
                    k.clone(),
                    KernelValue::NamedVariant {
                        enum_name: "FlowNodeKind".into(),
                        variant: "Step".into(),
                    },
                );
                node_deps.insert(
                    k.clone(),
                    KernelValue::Seq(
                        s.depends_on
                            .iter()
                            .map(|d| KernelValue::String(d.to_string()))
                            .collect(),
                    ),
                );
                node_dep_modes.insert(k.clone(), dep_mode_kv(&s.depends_on_mode));
                node_branches.insert(
                    k.clone(),
                    s.branch
                        .as_ref()
                        .map_or(KernelValue::None, |b| KernelValue::String(b.to_string())),
                );
            }
            FlowNodeSpec::RepeatUntil(l) => {
                node_kind.insert(
                    k.clone(),
                    KernelValue::NamedVariant {
                        enum_name: "FlowNodeKind".into(),
                        variant: "Loop".into(),
                    },
                );
                node_deps.insert(
                    k.clone(),
                    KernelValue::Seq(
                        l.depends_on
                            .iter()
                            .map(|d| KernelValue::String(d.to_string()))
                            .collect(),
                    ),
                );
                node_dep_modes.insert(k.clone(), dep_mode_kv(&l.depends_on_mode));
                node_branches.insert(k.clone(), KernelValue::None);
            }
        }
    }

    KernelInput {
        variant: "StartFrame".into(),
        fields: BTreeMap::from([
            ("frame_id".into(), KernelValue::String(frame_id.to_string())),
            ("tracked_nodes".into(), KernelValue::Set(tracked)),
            ("ordered_nodes".into(), KernelValue::Seq(ordered_kv)),
            ("node_kind".into(), KernelValue::Map(node_kind)),
            ("node_dependencies".into(), KernelValue::Map(node_deps)),
            (
                "node_dependency_modes".into(),
                KernelValue::Map(node_dep_modes),
            ),
            ("node_branches".into(), KernelValue::Map(node_branches)),
        ]),
    }
}

fn dep_mode_kv(mode: &DependencyMode) -> KernelValue {
    let variant = match mode {
        DependencyMode::All => "All",
        DependencyMode::Any => "Any",
    };
    KernelValue::NamedVariant {
        enum_name: "DependencyMode".into(),
        variant: variant.into(),
    }
}

/// Topological sort of a `FrameSpec` (Kahn's algorithm).
pub fn topological_order(spec: &FrameSpec) -> Result<Vec<FlowNodeId>, MobError> {
    let mut in_degree: BTreeMap<FlowNodeId, usize> = BTreeMap::new();
    let mut outgoing: BTreeMap<FlowNodeId, Vec<FlowNodeId>> = BTreeMap::new();

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

    let mut queue = VecDeque::new();
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

/// Extract a String from a named field of a `KernelEffect`.
fn string_from_effect_field(effect: &KernelEffect, field: &str) -> Result<String, MobError> {
    match effect.fields.get(field) {
        Some(KernelValue::String(s)) => Ok(s.clone()),
        other => Err(MobError::Internal(format!(
            "effect '{}' missing String field '{}': {other:?}",
            effect.variant, field
        ))),
    }
}

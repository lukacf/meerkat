//! FlowFrameKernel: sealed mutator for FlowFrameMachine state.
//!
//! All frame state mutations route through the generated `flow_frame::transition`
//! + `cas_frame_state`, enforcing the machine authority rule at compile time.

use crate::definition::{FlowNodeSpec, FrameSpec};
use crate::error::MobError;
use crate::flow_machine_types::{branch_id, dependency_mode, flow_node_id, frame_id};
use crate::ids::{FlowNodeId, FrameId, LoopId, LoopInstanceId, RunId, StepId};
use crate::run::FrameSnapshot;
use crate::runtime::flow_kernels::flow_frame;
use crate::store::MobRunStore;
use meerkat_machine_schema::compat::types as kernel_types;
use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

mod sealed {
    pub trait Sealed {}
}

// ─── StepCompletionOpts ──────────────────────────────────────────────────────

/// Options for completing a step node and recording its output.
pub struct StepCompletionOpts<'a> {
    /// The frame node that was admitted as a step.
    pub node_id: &'a FlowNodeId,
    /// The step ID used to store the output.
    pub step_id: &'a StepId,
    /// The output value produced by the step executor.
    pub output: serde_json::Value,
    /// `None` for root frame steps (stored in `root_step_outputs`).
    /// `Some((loop_id, iteration))` for loop body steps (stored in
    /// `loop_iteration_outputs[loop_id][iteration]`).
    pub loop_context: Option<(&'a LoopId, u64)>,
    /// Maximum number of CAS retries before returning an error.
    pub max_retries: usize,
}

// ─── FlowFrameMutator ────────────────────────────────────────────────────────

/// Sealed mutator trait for FlowFrame state transitions.
///
/// Only `FlowFrameKernel` implements this. All frame state mutations flow
/// through `flow_frame::transition()` + `cas_frame_state`.
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
pub trait FlowFrameMutator: sealed::Sealed {
    /// Start a frame from a `FrameSpec` (arbitrary DAG).
    ///
    /// Returns the initial `FrameSnapshot` in Running state.
    async fn start_frame(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        spec: &FrameSpec,
    ) -> Result<FrameSnapshot, MobError>;

    /// Admit the next ready node in the frame. Returns the effects emitted (e.g.
    /// `AdmitStepWork` or `StartLoopNode`), or `None` if the queue was empty.
    async fn admit_next_ready_node(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
    ) -> Result<Option<Vec<flow_frame::Effect>>, MobError>;

    /// Admit the next ready node with up to `max_retries` CAS retries.
    ///
    /// Returns `Ok(Some(effects))` on success, `Ok(None)` if the queue is
    /// genuinely empty, or `Err` if every attempt lost the CAS (contention).
    async fn admit_next_ready_node_with_retry(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        max_retries: usize,
    ) -> Result<Option<Vec<flow_frame::Effect>>, MobError>;

    /// Complete a step node and record its output, with CAS retry.
    async fn complete_step(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        opts: StepCompletionOpts<'_>,
    ) -> Result<(), MobError>;

    /// Mark a node as completed. Returns `true` if the CAS succeeded.
    async fn complete_node(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        node_id: &FlowNodeId,
    ) -> Result<bool, MobError>;

    /// Mark a node as failed. Returns `true` if the CAS succeeded.
    async fn fail_node(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        node_id: &FlowNodeId,
    ) -> Result<bool, MobError>;

    /// Mark a node as skipped. Returns `true` if the CAS succeeded.
    async fn skip_node(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        node_id: &FlowNodeId,
    ) -> Result<bool, MobError>;

    /// Mark a node as canceled. Returns `true` if the CAS succeeded.
    async fn cancel_node(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        node_id: &FlowNodeId,
    ) -> Result<bool, MobError>;

    /// Terminalize the frame as completed. Returns `true` if the CAS succeeded.
    async fn terminalize_frame(&self, run_id: &RunId, frame_id: &FrameId)
    -> Result<bool, MobError>;
}

// ─── FlowFrameKernel ─────────────────────────────────────────────────────────

/// Concrete implementation of `FlowFrameMutator`.
pub struct FlowFrameKernel {
    run_store: Arc<dyn MobRunStore>,
}

impl FlowFrameKernel {
    pub fn new(run_store: Arc<dyn MobRunStore>) -> Self {
        Self { run_store }
    }

    /// Read the current `FrameSnapshot` for a frame, returning an error if not found.
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

    /// Apply a transition to the current frame state via CAS.
    ///
    /// Reads the current snapshot, applies `input` to produce `next_snapshot`,
    /// then CAS-updates the store. Returns the effects from the transition on
    /// success. Returns `Err` on both transition failure and CAS exhaustion —
    /// callers must treat `Ok(Some(effects))` as the only success path.
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

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl FlowFrameMutator for FlowFrameKernel {
    async fn start_frame(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        spec: &FrameSpec,
    ) -> Result<FrameSnapshot, MobError> {
        // Resume guard: if the frame was already started (e.g. crash-recovery),
        // return the existing snapshot rather than re-initializing it.
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
        // CAS-insert the new frame (expected = None means "must not yet exist").
        let inserted = self
            .run_store
            .cas_frame_state(run_id, frame_id, None, snapshot.clone())
            .await?;
        if !inserted {
            // A concurrent writer started the frame between our read and insert.
            // Read the winner's snapshot and return it.
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
        if self
            .require_frame(run_id, frame_id)
            .await?
            .kernel_state
            .ready_queue
            .is_empty()
        {
            return Ok(None);
        }
        let input = flow_frame::Input::AdmitNextReadyNode(flow_frame::inputs::AdmitNextReadyNode);
        // Map Ok(effects) → Ok(Some(effects)); errors propagate as-is.
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
            let queue_empty = snap.kernel_state.ready_queue.is_empty();
            if queue_empty {
                return Ok(None); // genuinely nothing to admit
            }

            let admit_input =
                flow_frame::Input::AdmitNextReadyNode(flow_frame::inputs::AdmitNextReadyNode);
            let outcome =
                flow_frame::transition(&snap.kernel_state, admit_input, &flow_frame::EmptyContext)
                    .map_err(|e| MobError::Internal(format!("AdmitNextReadyNode failed: {e:?}")))?;
            let next_snap = FrameSnapshot {
                kernel_state: outcome.next_state,
            };

            // The sequential FlowFrameEngine drives nodes one at a time and does not
            // participate in FlowRunMachine's slot scheduler (ready_frames /
            // max_active_nodes). Concurrency limits will be enforced by a future
            // orchestrated multi-frame executor that registers frames before
            // admission. For now: update frame state only via CAS.
            let won = self
                .run_store
                .cas_frame_state(run_id, frame_id, Some(&snap), next_snap)
                .await?;
            if won {
                return Ok(Some(outcome.effects));
            }
            // CAS lost — retry with a fresh snapshot read
        }

        // All retries exhausted due to CAS contention (queue was non-empty each
        // time but another writer kept winning). This is distinct from "queue
        // empty" and indicates a liveness issue rather than normal termination.
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
                    node_id: flow_node_id(node_id),
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
        // Unreachable — the loop always returns on the last attempt.
        Err(MobError::Internal("CompleteNode CAS exhausted".into()))
    }

    async fn complete_node(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        node_id: &FlowNodeId,
    ) -> Result<bool, MobError> {
        let input = flow_frame::Input::CompleteNode(flow_frame::inputs::CompleteNode {
            node_id: flow_node_id(node_id),
        });
        // transition_frame now returns Err on CAS exhaustion rather than Ok(None),
        // so Ok always means the transition fired successfully.
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
            node_id: flow_node_id(node_id),
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
            node_id: flow_node_id(node_id),
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
        let input = flow_frame::Input::SealFrame(flow_frame::inputs::SealFrame);
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
            node_id: flow_node_id(node_id),
        });
        self.transition_frame(run_id, frame_id, input, 5)
            .await
            .map(|_| true)
    }
}

// ─── Helpers (moved from flow_frame_engine.rs) ─────────────────────────────

#[allow(clippy::type_complexity)]
fn build_frame_start_payload(
    frame_id_value: &FrameId,
    spec: &FrameSpec,
    ordered: &[FlowNodeId],
) -> (
    kernel_types::FrameId,
    std::collections::BTreeSet<kernel_types::FlowNodeId>,
    Vec<kernel_types::FlowNodeId>,
    std::collections::BTreeMap<kernel_types::FlowNodeId, kernel_types::FlowNodeKind>,
    std::collections::BTreeMap<kernel_types::FlowNodeId, Vec<kernel_types::FlowNodeId>>,
    std::collections::BTreeMap<kernel_types::FlowNodeId, kernel_types::DependencyMode>,
    std::collections::BTreeMap<kernel_types::FlowNodeId, Option<kernel_types::BranchId>>,
) {
    let tracked_nodes = ordered.iter().map(flow_node_id).collect();
    let ordered_nodes = ordered.iter().map(flow_node_id).collect();
    let mut node_kind = BTreeMap::new();
    let mut node_dependencies = BTreeMap::new();
    let mut node_dependency_modes = BTreeMap::new();
    let mut node_branches = BTreeMap::new();

    for (node_id_value, node_spec) in &spec.nodes {
        let key = flow_node_id(node_id_value);
        match node_spec {
            FlowNodeSpec::Step(step) => {
                node_kind.insert(key.clone(), kernel_types::FlowNodeKind::Step);
                node_dependencies.insert(
                    key.clone(),
                    step.depends_on.iter().map(flow_node_id).collect(),
                );
                node_dependency_modes
                    .insert(key.clone(), dependency_mode(step.depends_on_mode.clone()));
                node_branches.insert(key.clone(), step.branch.as_ref().map(branch_id));
            }
            FlowNodeSpec::RepeatUntil(loop_spec) => {
                node_kind.insert(key.clone(), kernel_types::FlowNodeKind::Loop);
                node_dependencies.insert(
                    key.clone(),
                    loop_spec.depends_on.iter().map(flow_node_id).collect(),
                );
                node_dependency_modes.insert(
                    key.clone(),
                    dependency_mode(loop_spec.depends_on_mode.clone()),
                );
                node_branches.insert(key.clone(), None);
            }
        }
    }

    (
        frame_id(frame_id_value),
        tracked_nodes,
        ordered_nodes,
        node_kind,
        node_dependencies,
        node_dependency_modes,
        node_branches,
    )
}

/// Build the `StartRootFrame` typed input from a `FrameSpec` and its topological order.
pub(crate) fn build_start_root_frame_input(
    frame_id: &FrameId,
    spec: &FrameSpec,
    ordered: &[FlowNodeId],
) -> flow_frame::Input {
    let (
        frame_id,
        tracked_nodes,
        ordered_nodes,
        node_kind,
        node_dependencies,
        node_dependency_modes,
        node_branches,
    ) = build_frame_start_payload(frame_id, spec, ordered);
    flow_frame::Input::StartRootFrame(flow_frame::inputs::StartRootFrame {
        frame_id,
        tracked_nodes,
        ordered_nodes,
        node_kind,
        node_dependencies,
        node_dependency_modes,
        node_branches,
    })
}

/// Build the `StartBodyFrame` typed input from a `FrameSpec` and its topological order.
pub(crate) fn build_start_body_frame_input(
    frame_id: &FrameId,
    loop_instance_id: &LoopInstanceId,
    iteration: u64,
    spec: &FrameSpec,
    ordered: &[FlowNodeId],
) -> flow_frame::Input {
    let (
        frame_id,
        tracked_nodes,
        ordered_nodes,
        node_kind,
        node_dependencies,
        node_dependency_modes,
        node_branches,
    ) = build_frame_start_payload(frame_id, spec, ordered);
    flow_frame::Input::StartBodyFrame(flow_frame::inputs::StartBodyFrame {
        frame_id,
        loop_instance_id: crate::flow_machine_types::loop_instance_id(loop_instance_id),
        iteration: u32::try_from(iteration)
            .map_err(|_| MobError::Internal("body frame iteration exceeds u32".into()))
            .unwrap_or(0),
        tracked_nodes,
        ordered_nodes,
        node_kind,
        node_dependencies,
        node_dependency_modes,
        node_branches,
    })
}

/// Topological sort of a `FrameSpec` (Kahn's algorithm).
pub(crate) fn topological_order(spec: &FrameSpec) -> Result<Vec<FlowNodeId>, MobError> {
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

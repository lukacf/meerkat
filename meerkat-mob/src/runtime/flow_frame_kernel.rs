//! FlowFrameKernel: sealed mutator for FlowFrameMachine state.
//!
//! All frame state mutations route through the generated `flow_frame::transition`
//! + `cas_frame_state`, enforcing the machine authority rule at compile time.

use crate::definition::{DependencyMode, FlowNodeSpec, FrameSpec};
use crate::error::MobError;
use crate::ids::{FlowNodeId, FrameId, LoopId, LoopInstanceId, RunId, StepId};
use crate::run::FrameSnapshot;
use crate::store::MobRunStore;
use meerkat_machine_kernels::generated::flow_frame;
use meerkat_machine_kernels::{KernelEffect, KernelInput, KernelValue};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
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
    ) -> Result<Option<Vec<KernelEffect>>, MobError>;

    /// Admit the next ready node with up to `max_retries` CAS retries.
    ///
    /// Returns `Ok(Some(effects))` on success, `Ok(None)` if the queue is
    /// genuinely empty, or `Err` if every attempt lost the CAS (contention).
    async fn admit_next_ready_node_with_retry(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        max_retries: usize,
    ) -> Result<Option<Vec<KernelEffect>>, MobError>;

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

    fn node_val(node_id: &FlowNodeId) -> KernelValue {
        KernelValue::String(node_id.to_string())
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
        input: KernelInput,
        max_retries: usize,
    ) -> Result<Vec<KernelEffect>, MobError> {
        for _ in 0..=max_retries {
            let current = self.require_frame(run_id, frame_id).await?;
            let outcome = flow_frame::transition(&current.kernel_state, &input)
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

        let initial = flow_frame::initial_state()
            .map_err(|e| MobError::Internal(format!("flow_frame initial_state failed: {e:?}")))?;
        let ordered = topological_order(spec)?;
        let start_input = build_start_root_frame_input(frame_id, spec, &ordered);
        let outcome = flow_frame::transition(&initial, &start_input)
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
    ) -> Result<Option<Vec<KernelEffect>>, MobError> {
        let input = KernelInput {
            variant: "AdmitNextReadyNode".into(),
            fields: BTreeMap::new(),
        };
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
    ) -> Result<Option<Vec<KernelEffect>>, MobError> {
        for _ in 0..=max_retries {
            let snap = self.require_frame(run_id, frame_id).await?;
            let queue_empty = match snap.kernel_state.fields.get("ready_queue") {
                Some(KernelValue::Seq(seq)) => seq.is_empty(),
                _ => true,
            };
            if queue_empty {
                return Ok(None); // genuinely nothing to admit
            }

            let admit_input = KernelInput {
                variant: "AdmitNextReadyNode".into(),
                fields: BTreeMap::new(),
            };
            let outcome = flow_frame::transition(&snap.kernel_state, &admit_input)
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
            let complete_input = KernelInput {
                variant: "CompleteNode".into(),
                fields: BTreeMap::from([("node_id".into(), Self::node_val(node_id))]),
            };
            let next_outcome = flow_frame::transition(&snap.kernel_state, &complete_input)
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
        let input = KernelInput {
            variant: "CompleteNode".into(),
            fields: BTreeMap::from([("node_id".into(), Self::node_val(node_id))]),
        };
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
        let input = KernelInput {
            variant: "FailNode".into(),
            fields: BTreeMap::from([("node_id".into(), Self::node_val(node_id))]),
        };
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
        let input = KernelInput {
            variant: "SkipNode".into(),
            fields: BTreeMap::from([("node_id".into(), Self::node_val(node_id))]),
        };
        self.transition_frame(run_id, frame_id, input, 5)
            .await
            .map(|_| true)
    }

    async fn terminalize_frame(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
    ) -> Result<bool, MobError> {
        let input = KernelInput {
            variant: "SealFrame".into(),
            fields: BTreeMap::new(),
        };
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
        let input = KernelInput {
            variant: "CancelNode".into(),
            fields: BTreeMap::from([("node_id".into(), Self::node_val(node_id))]),
        };
        self.transition_frame(run_id, frame_id, input, 5)
            .await
            .map(|_| true)
    }
}

// ─── Helpers (moved from flow_frame_engine.rs) ─────────────────────────────

fn build_frame_start_fields(
    frame_id: &FrameId,
    spec: &FrameSpec,
    ordered: &[FlowNodeId],
) -> BTreeMap<String, KernelValue> {
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

    BTreeMap::from([
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
    ])
}

/// Build the `StartRootFrame` KernelInput from a `FrameSpec` and its topological order.
pub(crate) fn build_start_root_frame_input(
    frame_id: &FrameId,
    spec: &FrameSpec,
    ordered: &[FlowNodeId],
) -> KernelInput {
    KernelInput {
        variant: "StartRootFrame".into(),
        fields: build_frame_start_fields(frame_id, spec, ordered),
    }
}

/// Build the `StartBodyFrame` KernelInput from a `FrameSpec` and its topological order.
pub(crate) fn build_start_body_frame_input(
    frame_id: &FrameId,
    loop_instance_id: &LoopInstanceId,
    iteration: u64,
    spec: &FrameSpec,
    ordered: &[FlowNodeId],
) -> KernelInput {
    let mut fields = build_frame_start_fields(frame_id, spec, ordered);
    fields.insert(
        "loop_instance_id".into(),
        KernelValue::String(loop_instance_id.to_string()),
    );
    fields.insert("iteration".into(), KernelValue::U64(iteration));
    KernelInput {
        variant: "StartBodyFrame".into(),
        fields,
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

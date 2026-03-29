//! FlowFrameKernel: sealed mutator for FlowFrameMachine state.
//!
//! All frame state mutations route through the generated `flow_frame::transition`
//! + `cas_frame_state`, enforcing the machine authority rule at compile time.

use crate::error::MobError;
use crate::ids::{FlowNodeId, FrameId, RunId};
use crate::run::FrameSnapshot;
use crate::store::MobRunStore;
use meerkat_machine_kernels::generated::flow_frame;
use meerkat_machine_kernels::{KernelEffect, KernelInput, KernelValue};
use std::collections::BTreeMap;
use std::sync::Arc;

mod sealed {
    pub trait Sealed {}
}

/// Sealed mutator trait for FlowFrame state transitions.
///
/// Only `FlowFrameKernel` implements this. All frame state mutations flow
/// through `flow_frame::transition()` + `cas_frame_state`.
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
pub trait FlowFrameMutator: sealed::Sealed {
    /// Start a frame with a 2-node A→B DAG (node_a as root, node_b depending on node_a).
    ///
    /// Returns the initial `FrameSnapshot` in Running state.
    async fn start_frame(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        node_a: &FlowNodeId,
        node_b: &FlowNodeId,
    ) -> Result<FrameSnapshot, MobError>;

    /// Admit the next ready node in the frame. Returns the effects emitted (e.g.
    /// `AdmitStepWork` or `StartLoopNode`), or `None` if the queue was empty.
    async fn admit_next_ready_node(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
    ) -> Result<Option<Vec<KernelEffect>>, MobError>;

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

    /// Terminalize the frame as completed. Returns `true` if the CAS succeeded.
    async fn terminalize_frame(&self, run_id: &RunId, frame_id: &FrameId)
    -> Result<bool, MobError>;
}

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

    fn step_kind() -> KernelValue {
        KernelValue::NamedVariant {
            enum_name: "FlowNodeKind".into(),
            variant: "Step".into(),
        }
    }

    fn dep_mode_all() -> KernelValue {
        KernelValue::NamedVariant {
            enum_name: "DependencyMode".into(),
            variant: "All".into(),
        }
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
    /// success, or `None` if the CAS was lost (retry up to `max_retries` times).
    async fn transition_frame(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
        input: KernelInput,
        max_retries: usize,
    ) -> Result<Option<Vec<KernelEffect>>, MobError> {
        for _ in 0..=max_retries {
            let current = self.require_frame(run_id, frame_id).await?;
            let outcome = flow_frame::transition(&current.kernel_state, &input)
                .map_err(|e| MobError::Internal(format!("flow_frame transition failed: {e:?}")))?;
            let next = FrameSnapshot {
                frame_id: frame_id.clone(),
                kernel_state: outcome.next_state,
            };
            let effects = outcome.effects.clone();
            let won = self
                .run_store
                .cas_frame_state(run_id, frame_id, Some(&current), next)
                .await?;
            if won {
                return Ok(Some(effects));
            }
        }
        Ok(None)
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
        node_a: &FlowNodeId,
        node_b: &FlowNodeId,
    ) -> Result<FrameSnapshot, MobError> {
        let initial = flow_frame::initial_state()
            .map_err(|e| MobError::Internal(format!("flow_frame initial_state failed: {e:?}")))?;
        let a = Self::node_val(node_a);
        let b = Self::node_val(node_b);
        let start_input = KernelInput {
            variant: "StartFrame".into(),
            fields: BTreeMap::from([
                ("frame_id".into(), KernelValue::String(frame_id.to_string())),
                (
                    "tracked_nodes".into(),
                    KernelValue::Set([a.clone(), b.clone()].into_iter().collect()),
                ),
                (
                    "ordered_nodes".into(),
                    KernelValue::Seq(vec![a.clone(), b.clone()]),
                ),
                (
                    "node_kind".into(),
                    KernelValue::Map(
                        [
                            (a.clone(), Self::step_kind()),
                            (b.clone(), Self::step_kind()),
                        ]
                        .into_iter()
                        .collect(),
                    ),
                ),
                (
                    "node_dependencies".into(),
                    KernelValue::Map(
                        [
                            (a.clone(), KernelValue::Seq(vec![])),
                            (b.clone(), KernelValue::Seq(vec![a.clone()])),
                        ]
                        .into_iter()
                        .collect(),
                    ),
                ),
                (
                    "node_dependency_modes".into(),
                    KernelValue::Map(
                        [
                            (a.clone(), Self::dep_mode_all()),
                            (b.clone(), Self::dep_mode_all()),
                        ]
                        .into_iter()
                        .collect(),
                    ),
                ),
                (
                    "node_branches".into(),
                    KernelValue::Map(
                        [
                            (a.clone(), KernelValue::None),
                            (b.clone(), KernelValue::None),
                        ]
                        .into_iter()
                        .collect(),
                    ),
                ),
            ]),
        };
        let outcome = flow_frame::transition(&initial, &start_input)
            .map_err(|e| MobError::Internal(format!("flow_frame StartFrame failed: {e:?}")))?;
        let snapshot = FrameSnapshot {
            frame_id: frame_id.clone(),
            kernel_state: outcome.next_state,
        };
        // Insert as a new frame (expected = None)
        let inserted = self
            .run_store
            .cas_frame_state(run_id, frame_id, None, snapshot.clone())
            .await?;
        if !inserted {
            return Err(MobError::Internal(format!(
                "frame '{frame_id}' already exists in run '{run_id}'"
            )));
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
        self.transition_frame(run_id, frame_id, input, 5).await
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
        Ok(self
            .transition_frame(run_id, frame_id, input, 5)
            .await?
            .is_some())
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
        Ok(self
            .transition_frame(run_id, frame_id, input, 5)
            .await?
            .is_some())
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
        Ok(self
            .transition_frame(run_id, frame_id, input, 5)
            .await?
            .is_some())
    }

    async fn terminalize_frame(
        &self,
        run_id: &RunId,
        frame_id: &FrameId,
    ) -> Result<bool, MobError> {
        let input = KernelInput {
            variant: "TerminalizeCompleted".into(),
            fields: BTreeMap::new(),
        };
        Ok(self
            .transition_frame(run_id, frame_id, input, 5)
            .await?
            .is_some())
    }
}

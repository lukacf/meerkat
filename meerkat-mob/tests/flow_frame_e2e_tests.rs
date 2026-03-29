#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Phase 4 E2E tests: E2E-01, E2E-02, E2E-03
//!
//! These tests drive `FlowFrameEngine` with scripted step executors to verify
//! that the full frame execution loop (including loops, nested loops, and
//! sibling parallelism) produces correct outputs.

use async_trait::async_trait;
use indexmap::IndexMap;
use meerkat_machine_kernels::KernelState;
use meerkat_mob::definition::{
    ConditionExpr, DependencyMode, FlowNodeSpec, FrameSpec, FrameStepSpec, RepeatUntilSpec,
};
use meerkat_mob::error::MobError;
use meerkat_mob::ids::{FlowNodeId, FrameId, LoopId, RunId, StepId};
use meerkat_mob::run::{FlowContext, MobRun};
use meerkat_mob::runtime::flow_frame_engine::{FlowFrameEngine, FrameStepExecutor};
use meerkat_mob::store::MobRunStore as _;
use meerkat_mob::{FlowId, InMemoryMobRunStore, MobId};
use std::sync::{Arc, Mutex};

// ─── Scripted executor ───────────────────────────────────────────────────────

struct StepScript {
    /// Node ID this script applies to (for ordering + validation).
    node_id: String,
    output: serde_json::Value,
}

/// A scripted step executor that returns pre-configured outputs in order.
struct ScriptedStepExecutor {
    scripts: Vec<StepScript>,
    index: Mutex<usize>,
}

impl ScriptedStepExecutor {
    fn new(scripts: Vec<StepScript>) -> Self {
        Self {
            scripts,
            index: Mutex::new(0),
        }
    }

    fn assert_all_consumed(&self) {
        let idx = *self.index.lock().unwrap();
        assert_eq!(
            idx,
            self.scripts.len(),
            "Not all scripted steps consumed: consumed {idx}, expected {}",
            self.scripts.len()
        );
    }
}

#[async_trait]
impl FrameStepExecutor for ScriptedStepExecutor {
    async fn execute_step(
        &self,
        _run_id: &RunId,
        _frame_id: &FrameId,
        node_id: &FlowNodeId,
        _step_id: &StepId,
        _context: &FlowContext,
    ) -> Result<serde_json::Value, MobError> {
        let mut idx = self.index.lock().unwrap();
        let script = self
            .scripts
            .get(*idx)
            .unwrap_or_else(|| panic!("no script for call #{idx}, node_id={node_id}"));
        assert_eq!(
            script.node_id,
            node_id.to_string(),
            "Script #{idx} expected node_id='{}', got '{node_id}'",
            script.node_id
        );
        let out = script.output.clone();
        *idx += 1;
        Ok(out)
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn build_run() -> (RunId, MobRun) {
    let run = MobRun::pending(
        MobId::from("test-mob"),
        FlowId::from("test-flow"),
        KernelState::default(),
        serde_json::json!({}),
    );
    let run_id = run.run_id.clone();
    (run_id, run)
}

async fn setup_engine(
    executor: Arc<dyn FrameStepExecutor>,
) -> (RunId, Arc<InMemoryMobRunStore>, FlowFrameEngine) {
    let (run_id, run) = build_run();
    let store = Arc::new(InMemoryMobRunStore::new());
    store.create_run(run).await.expect("create_run");
    let engine = FlowFrameEngine::new(store.clone(), executor);
    (run_id, store, engine)
}

fn empty_context(run_id: RunId) -> FlowContext {
    FlowContext {
        run_id,
        activation_params: serde_json::json!({}),
        step_outputs: IndexMap::new(),
        loop_outputs: IndexMap::new(),
    }
}

// ─── Flow specs ──────────────────────────────────────────────────────────────

/// [setup] → [review-loop (repeat_until passes=true)] → [finalize]
/// review-loop body: [review] → [revise]
fn build_reviewer_implementer_flow() -> FrameSpec {
    let review_body = FrameSpec {
        nodes: {
            let mut nodes = IndexMap::new();
            nodes.insert(
                FlowNodeId::from("review-node"),
                FlowNodeSpec::Step(FrameStepSpec {
                    step_id: StepId::from("review"),
                    depends_on: vec![],
                    depends_on_mode: DependencyMode::All,
                    branch: None,
                }),
            );
            nodes.insert(
                FlowNodeId::from("revise-node"),
                FlowNodeSpec::Step(FrameStepSpec {
                    step_id: StepId::from("revise"),
                    depends_on: vec![FlowNodeId::from("review-node")],
                    depends_on_mode: DependencyMode::All,
                    branch: None,
                }),
            );
            nodes
        },
    };

    let until = ConditionExpr::Eq {
        path: "steps.review.passes".to_string(),
        value: serde_json::json!(true),
    };

    let mut root_nodes = IndexMap::new();
    root_nodes.insert(
        FlowNodeId::from("setup-node"),
        FlowNodeSpec::Step(FrameStepSpec {
            step_id: StepId::from("setup"),
            depends_on: vec![],
            depends_on_mode: DependencyMode::All,
            branch: None,
        }),
    );
    root_nodes.insert(
        FlowNodeId::from("loop-node"),
        FlowNodeSpec::RepeatUntil(RepeatUntilSpec {
            loop_id: LoopId::from("review-loop"),
            depends_on: vec![FlowNodeId::from("setup-node")],
            depends_on_mode: DependencyMode::All,
            body: review_body,
            until,
            max_iterations: 5,
        }),
    );
    root_nodes.insert(
        FlowNodeId::from("finalize-node"),
        FlowNodeSpec::Step(FrameStepSpec {
            step_id: StepId::from("finalize"),
            depends_on: vec![FlowNodeId::from("loop-node")],
            depends_on_mode: DependencyMode::All,
            branch: None,
        }),
    );

    FrameSpec { nodes: root_nodes }
}

// ─── E2E-01: reviewer→implementer loop inside a larger DAG ──────────────────

/// E2E-01: reviewer→implementer loop inside a larger DAG runs to completion.
///
/// Flow: [setup] → [review-loop (repeat_until passes=true)] → [finalize]
/// Iteration 0: review returns passes=false → revise runs.
/// Iteration 1: review returns passes=true → loop exits.
/// finalize runs after loop.
#[tokio::test]
async fn test_reviewer_implementer_loop_inside_larger_dag() {
    let executor = Arc::new(ScriptedStepExecutor::new(vec![
        // setup
        StepScript {
            node_id: "setup-node".into(),
            output: serde_json::json!({"done": true}),
        },
        // iteration 0: review fails
        StepScript {
            node_id: "review-node".into(),
            output: serde_json::json!({"passes": false}),
        },
        StepScript {
            node_id: "revise-node".into(),
            output: serde_json::json!({"revised": true}),
        },
        // iteration 1: review passes
        StepScript {
            node_id: "review-node".into(),
            output: serde_json::json!({"passes": true}),
        },
        StepScript {
            node_id: "revise-node".into(),
            output: serde_json::json!({"revised": true}),
        },
        // finalize
        StepScript {
            node_id: "finalize-node".into(),
            output: serde_json::json!({"complete": true}),
        },
    ]));

    let (run_id, _store, engine) = setup_engine(executor.clone()).await;
    let spec = build_reviewer_implementer_flow();
    let context = empty_context(run_id.clone());
    let frame_id = FrameId::from("root-frame");

    let outputs = engine
        .execute_frame(&run_id, &frame_id, &spec, &context)
        .await
        .expect("execute_frame");

    // All 3 root step nodes should have output recorded.
    assert!(
        outputs.contains_key(&StepId::from("setup")),
        "setup output missing"
    );
    assert!(
        outputs.contains_key(&StepId::from("finalize")),
        "finalize output missing"
    );
    assert_eq!(
        outputs[&StepId::from("finalize")],
        serde_json::json!({"complete": true})
    );

    executor.assert_all_consumed();
}

// ─── E2E-02: nested loop ─────────────────────────────────────────────────────

/// E2E-02: Nested loop (loop body contains a loop) runs to completion.
///
/// Outer loop body: [outer-step] → [inner-loop (1 iteration)]
/// Inner loop body: [inner-step]
/// Outer loop: 1 iteration (condition met immediately).
/// Inner loop: 1 iteration (condition met immediately).
#[tokio::test]
async fn test_nested_loop() {
    let inner_body = FrameSpec {
        nodes: {
            let mut nodes = IndexMap::new();
            nodes.insert(
                FlowNodeId::from("inner-step-node"),
                FlowNodeSpec::Step(FrameStepSpec {
                    step_id: StepId::from("inner-step"),
                    depends_on: vec![],
                    depends_on_mode: DependencyMode::All,
                    branch: None,
                }),
            );
            nodes
        },
    };

    let outer_body = FrameSpec {
        nodes: {
            let mut nodes = IndexMap::new();
            nodes.insert(
                FlowNodeId::from("outer-step-node"),
                FlowNodeSpec::Step(FrameStepSpec {
                    step_id: StepId::from("outer-step"),
                    depends_on: vec![],
                    depends_on_mode: DependencyMode::All,
                    branch: None,
                }),
            );
            nodes.insert(
                FlowNodeId::from("inner-loop-node"),
                FlowNodeSpec::RepeatUntil(RepeatUntilSpec {
                    loop_id: LoopId::from("inner-loop"),
                    depends_on: vec![FlowNodeId::from("outer-step-node")],
                    depends_on_mode: DependencyMode::All,
                    body: inner_body,
                    until: ConditionExpr::Eq {
                        path: "steps.inner-step.done".to_string(),
                        value: serde_json::json!(true),
                    },
                    max_iterations: 3,
                }),
            );
            nodes
        },
    };

    let root_spec = FrameSpec {
        nodes: {
            let mut nodes = IndexMap::new();
            nodes.insert(
                FlowNodeId::from("outer-loop-node"),
                FlowNodeSpec::RepeatUntil(RepeatUntilSpec {
                    loop_id: LoopId::from("outer-loop"),
                    depends_on: vec![],
                    depends_on_mode: DependencyMode::All,
                    body: outer_body,
                    until: ConditionExpr::Eq {
                        path: "steps.outer-step.ok".to_string(),
                        value: serde_json::json!(true),
                    },
                    max_iterations: 3,
                }),
            );
            nodes
        },
    };

    let executor = Arc::new(ScriptedStepExecutor::new(vec![
        // outer iteration 0: outer-step
        StepScript {
            node_id: "outer-step-node".into(),
            output: serde_json::json!({"ok": true}),
        },
        // inner iteration 0: inner-step
        StepScript {
            node_id: "inner-step-node".into(),
            output: serde_json::json!({"done": true}),
        },
    ]));

    let (run_id, _store, engine) = setup_engine(executor.clone()).await;
    let context = empty_context(run_id.clone());
    let frame_id = FrameId::from("root-frame-nested");

    engine
        .execute_frame(&run_id, &frame_id, &root_spec, &context)
        .await
        .expect("nested loop execute_frame");

    executor.assert_all_consumed();
}

// ─── E2E-03: sibling steps advance while loop body runs ─────────────────────

/// E2E-03: Sibling steps in the root frame complete alongside a loop.
///
/// Frame: [step-a (no deps), loop-L (no deps), step-b (no deps)]
/// All three nodes start as roots. In the sequential executor:
/// step-a, loop-L (1 iter of body-step), step-b all complete.
/// Verify all 3 nodes complete by checking outputs.
#[tokio::test]
async fn test_sibling_advances_during_loop() {
    let loop_body = FrameSpec {
        nodes: {
            let mut nodes = IndexMap::new();
            nodes.insert(
                FlowNodeId::from("body-step-node"),
                FlowNodeSpec::Step(FrameStepSpec {
                    step_id: StepId::from("body-step"),
                    depends_on: vec![],
                    depends_on_mode: DependencyMode::All,
                    branch: None,
                }),
            );
            nodes
        },
    };

    let root_spec = FrameSpec {
        nodes: {
            let mut nodes = IndexMap::new();
            nodes.insert(
                FlowNodeId::from("step-a-node"),
                FlowNodeSpec::Step(FrameStepSpec {
                    step_id: StepId::from("step-a"),
                    depends_on: vec![],
                    depends_on_mode: DependencyMode::All,
                    branch: None,
                }),
            );
            nodes.insert(
                FlowNodeId::from("loop-L-node"),
                FlowNodeSpec::RepeatUntil(RepeatUntilSpec {
                    loop_id: LoopId::from("loop-L"),
                    depends_on: vec![],
                    depends_on_mode: DependencyMode::All,
                    body: loop_body,
                    until: ConditionExpr::Eq {
                        path: "steps.body-step.done".to_string(),
                        value: serde_json::json!(true),
                    },
                    max_iterations: 3,
                }),
            );
            nodes.insert(
                FlowNodeId::from("step-b-node"),
                FlowNodeSpec::Step(FrameStepSpec {
                    step_id: StepId::from("step-b"),
                    depends_on: vec![],
                    depends_on_mode: DependencyMode::All,
                    branch: None,
                }),
            );
            nodes
        },
    };

    // In the sequential executor, nodes are admitted in topological order.
    // All three nodes have no deps and will be admitted in insertion order.
    let executor = Arc::new(ScriptedStepExecutor::new(vec![
        StepScript {
            node_id: "step-a-node".into(),
            output: serde_json::json!({"a": 1}),
        },
        StepScript {
            node_id: "body-step-node".into(),
            output: serde_json::json!({"done": true}),
        },
        StepScript {
            node_id: "step-b-node".into(),
            output: serde_json::json!({"b": 2}),
        },
    ]));

    let (run_id, _store, engine) = setup_engine(executor.clone()).await;
    let context = empty_context(run_id.clone());
    let frame_id = FrameId::from("root-frame-sibling");

    let outputs = engine
        .execute_frame(&run_id, &frame_id, &root_spec, &context)
        .await
        .expect("sibling execute_frame");

    // All three leaf step nodes (step-a, step-b, and the loop body's body-step)
    // should have completed. step-a and step-b are directly in frame_outputs.
    assert!(
        outputs.contains_key(&StepId::from("step-a")),
        "step-a output missing; outputs: {outputs:?}"
    );
    assert!(
        outputs.contains_key(&StepId::from("step-b")),
        "step-b output missing; outputs: {outputs:?}"
    );
    assert_eq!(
        outputs[&StepId::from("step-a")],
        serde_json::json!({"a": 1})
    );
    assert_eq!(
        outputs[&StepId::from("step-b")],
        serde_json::json!({"b": 2})
    );

    executor.assert_all_consumed();
}

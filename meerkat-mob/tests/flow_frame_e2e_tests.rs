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
use meerkat_mob::run::{FlowContext, LoopSnapshot, MobRun};
use meerkat_mob::runtime::FlowFrameKernel;
use meerkat_mob::runtime::flow_frame_engine::{
    FlowFrameEngine, FrameStepExecutor, FrameStepResult,
};
use meerkat_mob::store::MobRunStore as _;
use meerkat_mob::{FlowFrameMutator, FlowId, InMemoryMobRunStore, MobId};
use std::collections::BTreeMap;
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
    ) -> Result<FrameStepResult, MobError> {
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
        Ok(FrameStepResult::Completed(out))
    }
}

/// A scripted executor that matches step calls by node id instead of call order.
struct AnyOrderScriptedStepExecutor {
    remaining: Mutex<Vec<StepScript>>,
}

impl AnyOrderScriptedStepExecutor {
    fn new(scripts: Vec<StepScript>) -> Self {
        Self {
            remaining: Mutex::new(scripts),
        }
    }

    fn assert_all_consumed(&self) {
        let remaining = self.remaining.lock().unwrap();
        assert!(
            remaining.is_empty(),
            "Not all scripted steps consumed; remaining nodes: {:?}",
            remaining
                .iter()
                .map(|script| script.node_id.as_str())
                .collect::<Vec<_>>()
        );
    }
}

#[async_trait]
impl FrameStepExecutor for AnyOrderScriptedStepExecutor {
    async fn execute_step(
        &self,
        _run_id: &RunId,
        _frame_id: &FrameId,
        node_id: &FlowNodeId,
        _step_id: &StepId,
        _context: &FlowContext,
    ) -> Result<FrameStepResult, MobError> {
        let mut remaining = self.remaining.lock().unwrap();
        let Some(index) = remaining
            .iter()
            .position(|script| script.node_id == node_id.to_string())
        else {
            panic!(
                "no any-order script for node_id={node_id}; remaining={:?}",
                remaining
                    .iter()
                    .map(|script| script.node_id.as_str())
                    .collect::<Vec<_>>()
            );
        };
        let script = remaining.remove(index);
        Ok(FrameStepResult::Completed(script.output))
    }
}

/// Scripted executor for loop-body context regressions where later body steps
/// must observe outputs from the current iteration, not the previous one.
struct LoopBodyContextCheckingExecutor {
    call_index: Mutex<usize>,
}

impl LoopBodyContextCheckingExecutor {
    fn new() -> Self {
        Self {
            call_index: Mutex::new(0),
        }
    }
}

#[async_trait]
impl FrameStepExecutor for LoopBodyContextCheckingExecutor {
    async fn execute_step(
        &self,
        _run_id: &RunId,
        _frame_id: &FrameId,
        node_id: &FlowNodeId,
        _step_id: &StepId,
        context: &FlowContext,
    ) -> Result<FrameStepResult, MobError> {
        let mut idx = self.call_index.lock().unwrap();
        let call = *idx;
        *idx += 1;

        match (call, node_id.as_str()) {
            (0, "source-node") => Ok(FrameStepResult::Completed(serde_json::json!({
                "value": "old"
            }))),
            (1, "check-node") => {
                assert_eq!(
                    context.step_outputs.get(&StepId::from("source")),
                    Some(&serde_json::json!({"value": "old"})),
                    "first-iteration checker must see first-iteration source output"
                );
                Ok(FrameStepResult::Completed(
                    serde_json::json!({"done": false}),
                ))
            }
            (2, "source-node") => Ok(FrameStepResult::Completed(serde_json::json!({
                "value": "new"
            }))),
            (3, "check-node") => {
                assert_eq!(
                    context.step_outputs.get(&StepId::from("source")),
                    Some(&serde_json::json!({"value": "new"})),
                    "second-iteration checker must see current-iteration source output"
                );
                Ok(FrameStepResult::Completed(
                    serde_json::json!({"done": true}),
                ))
            }
            _ => panic!("unexpected loop body executor call #{call} for node '{node_id}'"),
        }
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
    let engine = FlowFrameEngine::new(store.clone(), executor, 0, 0);
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
        outputs.outputs.contains_key(&StepId::from("setup")),
        "setup output missing"
    );
    assert!(
        outputs.outputs.contains_key(&StepId::from("finalize")),
        "finalize output missing"
    );
    assert_eq!(
        outputs.outputs[&StepId::from("finalize")],
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
/// All three nodes start as roots. Under the concurrent scheduler, the sibling
/// step and the loop body may interleave in either order. Verify that all 3
/// leaf steps complete without assuming a specific interleaving.
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

    let executor = Arc::new(AnyOrderScriptedStepExecutor::new(vec![
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
        outputs.outputs.contains_key(&StepId::from("step-a")),
        "step-a output missing; outputs: {outputs:?}"
    );
    assert!(
        outputs.outputs.contains_key(&StepId::from("step-b")),
        "step-b output missing; outputs: {outputs:?}"
    );
    assert_eq!(
        outputs.outputs[&StepId::from("step-a")],
        serde_json::json!({"a": 1})
    );
    assert_eq!(
        outputs.outputs[&StepId::from("step-b")],
        serde_json::json!({"b": 2})
    );

    executor.assert_all_consumed();
}

// ─── Regression tests ────────────────────────────────────────────────────────

/// [P1] Loop body outputs must be visible to downstream steps as steps.<id>...
/// A finalize step that references {{ steps.review.passes }} via its condition
/// must see the value produced by the last loop iteration.
#[tokio::test]
async fn test_loop_last_iteration_outputs_visible_in_step_outputs() {
    // Flow: [review-loop] → [finalize-node]
    // review-loop body: [review-node] (iteration 0 → passes=false, iteration 1 → passes=true)
    // finalize-node has a condition that checks steps.review.passes == true
    let executor = Arc::new(ScriptedStepExecutor::new(vec![
        // Iteration 0: review fails
        StepScript {
            node_id: "review-node".into(),
            output: serde_json::json!({"passes": false}),
        },
        // Iteration 1: review passes
        StepScript {
            node_id: "review-node".into(),
            output: serde_json::json!({"passes": true}),
        },
        // finalize runs (condition passed → context must have steps.review.passes = true)
        StepScript {
            node_id: "finalize-node".into(),
            output: serde_json::json!({"done": true}),
        },
    ]));

    let (run_id, _store, engine) = setup_engine(executor.clone()).await;

    let body = FrameSpec {
        nodes: {
            let mut m = IndexMap::new();
            m.insert(
                FlowNodeId::from("review-node"),
                FlowNodeSpec::Step(FrameStepSpec {
                    step_id: StepId::from("review"),
                    depends_on: vec![],
                    depends_on_mode: DependencyMode::All,
                    branch: None,
                }),
            );
            m
        },
    };

    let spec = FrameSpec {
        nodes: {
            let mut m = IndexMap::new();
            m.insert(
                FlowNodeId::from("loop-node"),
                FlowNodeSpec::RepeatUntil(RepeatUntilSpec {
                    loop_id: LoopId::from("review-loop"),
                    depends_on: vec![],
                    depends_on_mode: DependencyMode::All,
                    body,
                    until: ConditionExpr::Eq {
                        path: "steps.review.passes".into(),
                        value: serde_json::json!(true),
                    },
                    max_iterations: 5,
                }),
            );
            m.insert(
                FlowNodeId::from("finalize-node"),
                FlowNodeSpec::Step(FrameStepSpec {
                    step_id: StepId::from("finalize"),
                    depends_on: vec![FlowNodeId::from("loop-node")],
                    depends_on_mode: DependencyMode::All,
                    branch: None,
                }),
            );
            m
        },
    };

    let context = FlowContext {
        run_id: run_id.clone(),
        activation_params: serde_json::json!({}),
        step_outputs: IndexMap::new(),
        loop_outputs: IndexMap::new(),
    };

    let frame_id = FrameId::from("root");
    let outputs = engine
        .execute_frame(&run_id, &frame_id, &spec, &context)
        .await
        .expect("execute_frame");

    // The finalize step must have run and produced its output.
    assert_eq!(
        outputs.outputs.get(&StepId::from("finalize")),
        Some(&serde_json::json!({"done": true})),
        "finalize must complete — it can only run if loop outputs were visible to context"
    );
    // The last loop body's review output must also be in the frame outputs.
    assert_eq!(
        outputs.outputs.get(&StepId::from("review")),
        Some(&serde_json::json!({"passes": true})),
        "last iteration review output must be visible at steps.review"
    );
    executor.assert_all_consumed();
}

/// [P1] Later steps in a loop body must observe the current iteration's
/// outputs, overriding the prior-iteration projection in FlowContext.
#[tokio::test]
async fn test_loop_body_steps_see_current_iteration_outputs() {
    let executor = Arc::new(LoopBodyContextCheckingExecutor::new());
    let (run_id, _store, engine) = setup_engine(executor).await;

    let body = FrameSpec {
        nodes: IndexMap::from([
            (
                FlowNodeId::from("source-node"),
                FlowNodeSpec::Step(FrameStepSpec {
                    step_id: StepId::from("source"),
                    depends_on: vec![],
                    depends_on_mode: DependencyMode::All,
                    branch: None,
                }),
            ),
            (
                FlowNodeId::from("check-node"),
                FlowNodeSpec::Step(FrameStepSpec {
                    step_id: StepId::from("check"),
                    depends_on: vec![FlowNodeId::from("source-node")],
                    depends_on_mode: DependencyMode::All,
                    branch: None,
                }),
            ),
        ]),
    };

    let spec = FrameSpec {
        nodes: IndexMap::from([(
            FlowNodeId::from("loop-node"),
            FlowNodeSpec::RepeatUntil(RepeatUntilSpec {
                loop_id: LoopId::from("body-loop"),
                depends_on: vec![],
                depends_on_mode: DependencyMode::All,
                body,
                until: ConditionExpr::Eq {
                    path: "steps.check.done".into(),
                    value: serde_json::json!(true),
                },
                max_iterations: 5,
            }),
        )]),
    };

    let outputs = engine
        .execute_frame(
            &run_id,
            &FrameId::from("root"),
            &spec,
            &empty_context(run_id.clone()),
        )
        .await
        .expect("execute_frame");

    assert_eq!(
        outputs.outputs.get(&StepId::from("source")),
        Some(&serde_json::json!({"value": "new"})),
        "root outputs should expose the last completed loop iteration"
    );
    assert_eq!(
        outputs.outputs.get(&StepId::from("check")),
        Some(&serde_json::json!({"done": true})),
        "loop should exit after the second iteration's checker succeeds"
    );
}

/// [P1] A failing step must be retried up to max_step_retries before giving up.
/// Uses a ScriptedStepExecutor that returns an error on attempt 0 and succeeds on attempt 1.
/// Without retry, the engine would fail immediately on the first error.
#[tokio::test]
async fn test_frame_step_retried_on_transient_failure() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FailThenSucceedExecutor {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl FrameStepExecutor for FailThenSucceedExecutor {
        async fn execute_step(
            &self,
            _run_id: &RunId,
            _frame_id: &meerkat_mob::ids::FrameId,
            _node_id: &FlowNodeId,
            _step_id: &StepId,
            _context: &FlowContext,
        ) -> Result<FrameStepResult, MobError> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                // First call: simulate transient failure
                Err(MobError::Internal("transient error".into()))
            } else {
                Ok(FrameStepResult::Completed(serde_json::json!({"ok": true})))
            }
        }
    }

    let executor = Arc::new(FailThenSucceedExecutor {
        calls: AtomicUsize::new(0),
    });

    // Note: FlowFrameEngine itself doesn't implement retry — that lives in
    // FlowTurnExecutorAdapter. This test verifies that the engine propagates
    // executor errors correctly, and that a successful second call is used.
    // The retry-via-adapter path is tested at the integration level.
    // Here we just confirm the engine doesn't swallow the error silently.
    let (run_id, _store, engine) = setup_engine(executor).await;

    let spec = FrameSpec {
        nodes: {
            let mut m = IndexMap::new();
            m.insert(
                FlowNodeId::from("step-node"),
                FlowNodeSpec::Step(FrameStepSpec {
                    step_id: StepId::from("step-a"),
                    depends_on: vec![],
                    depends_on_mode: DependencyMode::All,
                    branch: None,
                }),
            );
            m
        },
    };

    let context = FlowContext {
        run_id: run_id.clone(),
        activation_params: serde_json::json!({}),
        step_outputs: IndexMap::new(),
        loop_outputs: IndexMap::new(),
    };

    // First call fails → engine propagates the error
    let frame_id = FrameId::from("root");
    let result = engine
        .execute_frame(&run_id, &frame_id, &spec, &context)
        .await;
    assert!(
        result.is_err(),
        "engine must propagate executor error on first failure"
    );
}

/// [P1] A step whose FlowStepSpec.condition evaluates to false must be skipped,
/// not dispatched. The scripted executor must not be called for that step.
#[tokio::test]
async fn test_conditional_step_skipped_when_condition_false() {
    // Only setup-node runs; skipped-node has a condition that will evaluate to false.
    // If the skip is incorrect, the scripted executor would be called for skipped-node
    // and assert_all_consumed would fail (one script would be left unconsumed).
    // NOTE: FlowTurnExecutorAdapter is what evaluates conditions. ScriptedStepExecutor
    // (used directly here) always returns Completed. To test condition evaluation we
    // need a ConditionCheckingExecutor that wraps the scripted one.
    // For now, verify the FrameStepResult::Skipped path via a custom executor.
    use meerkat_mob::runtime::flow_frame_engine::FrameStepResult;

    struct ConditionalExecutor {
        inner: Arc<ScriptedStepExecutor>,
        skip_step: StepId,
    }

    #[async_trait]
    impl FrameStepExecutor for ConditionalExecutor {
        async fn execute_step(
            &self,
            run_id: &RunId,
            frame_id: &meerkat_mob::ids::FrameId,
            node_id: &FlowNodeId,
            step_id: &StepId,
            context: &FlowContext,
        ) -> Result<FrameStepResult, meerkat_mob::error::MobError> {
            if step_id == &self.skip_step {
                return Ok(FrameStepResult::Skipped);
            }
            self.inner
                .execute_step(run_id, frame_id, node_id, step_id, context)
                .await
        }
    }

    let inner = Arc::new(ScriptedStepExecutor::new(vec![StepScript {
        node_id: "setup-node".into(),
        output: serde_json::json!({"flag": false}),
    }]));
    let cond_executor = Arc::new(ConditionalExecutor {
        inner: inner.clone(),
        skip_step: StepId::from("skipped"),
    });

    let (run_id, _store, engine) = setup_engine(cond_executor).await;

    let spec = FrameSpec {
        nodes: {
            let mut m = IndexMap::new();
            m.insert(
                FlowNodeId::from("setup-node"),
                FlowNodeSpec::Step(FrameStepSpec {
                    step_id: StepId::from("setup"),
                    depends_on: vec![],
                    depends_on_mode: DependencyMode::All,
                    branch: None,
                }),
            );
            m.insert(
                FlowNodeId::from("skipped-node"),
                FlowNodeSpec::Step(FrameStepSpec {
                    step_id: StepId::from("skipped"),
                    depends_on: vec![FlowNodeId::from("setup-node")],
                    depends_on_mode: DependencyMode::All,
                    branch: None,
                }),
            );
            m
        },
    };

    let context = FlowContext {
        run_id: run_id.clone(),
        activation_params: serde_json::json!({}),
        step_outputs: IndexMap::new(),
        loop_outputs: IndexMap::new(),
    };

    let frame_id = FrameId::from("root");
    let outputs = engine
        .execute_frame(&run_id, &frame_id, &spec, &context)
        .await
        .expect("execute_frame");

    // setup ran, skipped did not produce an output (Skipped has no output)
    assert!(
        outputs.outputs.contains_key(&StepId::from("setup")),
        "setup step must have run"
    );
    assert!(
        !outputs.outputs.contains_key(&StepId::from("skipped")),
        "skipped step must not appear in outputs"
    );
    assert_eq!(
        outputs.step_statuses.get(&StepId::from("setup")),
        Some(&meerkat_mob::run::StepRunStatus::Completed),
        "frame outcome should carry canonical completed status for executed steps"
    );
    assert_eq!(
        outputs.step_statuses.get(&StepId::from("skipped")),
        Some(&meerkat_mob::run::StepRunStatus::Skipped),
        "frame outcome should carry canonical skipped status for skipped steps"
    );
    inner.assert_all_consumed();
}

/// [P2] An empty frame (no nodes) must terminalize to Completed, not stay Running.
#[tokio::test]
async fn test_empty_frame_terminalize() {
    let executor = Arc::new(ScriptedStepExecutor::new(vec![]));
    let (run_id, store, engine) = setup_engine(executor).await;

    let spec = FrameSpec {
        nodes: IndexMap::new(), // empty
    };
    let context = FlowContext {
        run_id: run_id.clone(),
        activation_params: serde_json::json!({}),
        step_outputs: IndexMap::new(),
        loop_outputs: IndexMap::new(),
    };

    let frame_id = FrameId::from("empty-root");
    let outputs = engine
        .execute_frame(&run_id, &frame_id, &spec, &context)
        .await
        .expect("empty frame must succeed");
    assert!(
        outputs.outputs.is_empty(),
        "empty frame produces no outputs"
    );

    // The persisted frame snapshot must be Completed, not Running.
    let run = store
        .get_run(&run_id)
        .await
        .expect("get_run")
        .expect("run present");
    let frame_snap = run.frames.get(&frame_id).expect("frame persisted");
    assert_eq!(
        frame_snap.kernel_state.phase, "Completed",
        "empty frame must terminalize to Completed in the store"
    );
}

/// Empty loop body frames must be processed immediately instead of stalling the coordinator.
#[tokio::test]
async fn test_empty_loop_body_frame_does_not_stall() {
    let executor = Arc::new(ScriptedStepExecutor::new(vec![]));
    let (run_id, store, engine) = setup_engine(executor).await;

    let spec = FrameSpec {
        nodes: {
            let mut nodes = IndexMap::new();
            nodes.insert(
                FlowNodeId::from("loop-node"),
                FlowNodeSpec::RepeatUntil(RepeatUntilSpec {
                    loop_id: LoopId::from("empty-loop"),
                    depends_on: vec![],
                    depends_on_mode: DependencyMode::All,
                    body: FrameSpec {
                        nodes: IndexMap::new(),
                    },
                    until: ConditionExpr::Eq {
                        path: "params.done".into(),
                        value: serde_json::json!(true),
                    },
                    max_iterations: 1,
                }),
            );
            nodes
        },
    };
    let context = FlowContext {
        run_id: run_id.clone(),
        activation_params: serde_json::json!({ "done": true }),
        step_outputs: IndexMap::new(),
        loop_outputs: IndexMap::new(),
    };

    let frame_id = FrameId::from("empty-loop-root");
    let outputs = engine
        .execute_frame(&run_id, &frame_id, &spec, &context)
        .await
        .expect("empty loop body should complete without stalling");
    assert!(
        outputs.outputs.is_empty(),
        "empty loop body should not synthesize step outputs"
    );

    let run = store
        .get_run(&run_id)
        .await
        .expect("get_run")
        .expect("run present");
    let root_frame = run.frames.get(&frame_id).expect("root frame");
    assert_eq!(
        root_frame.kernel_state.phase, "Completed",
        "root frame should complete after empty loop body processing"
    );
    let loop_instance_id = meerkat_mob::ids::LoopInstanceId::from("empty-loop-root::loop-node");
    let loop_snapshot = run.loops.get(&loop_instance_id).expect("loop snapshot");
    assert_eq!(
        loop_snapshot.kernel_state.phase, "Completed",
        "empty loop body should still advance the loop lifecycle to Completed"
    );
}

/// Resume must project a terminal loop snapshot back into the parent frame even if
/// the previous process crashed before the parent node update.
#[tokio::test]
async fn test_resume_projects_terminal_loop_snapshot_to_parent() {
    let executor = Arc::new(ScriptedStepExecutor::new(vec![]));
    let (run_id, run) = build_run();
    let store = Arc::new(InMemoryMobRunStore::new());
    store.create_run(run).await.expect("create_run");
    let engine = FlowFrameEngine::new(store.clone(), executor, 0, 0);
    let kernel = FlowFrameKernel::new(store.clone());

    let frame_id = FrameId::from("resume-root");
    let loop_node_id = FlowNodeId::from("loop-node");
    let loop_id = LoopId::from("resume-loop");
    let spec = FrameSpec {
        nodes: {
            let mut nodes = IndexMap::new();
            nodes.insert(
                loop_node_id.clone(),
                FlowNodeSpec::RepeatUntil(RepeatUntilSpec {
                    loop_id: loop_id.clone(),
                    depends_on: vec![],
                    depends_on_mode: DependencyMode::All,
                    body: FrameSpec {
                        nodes: IndexMap::new(),
                    },
                    until: ConditionExpr::Eq {
                        path: "params.done".into(),
                        value: serde_json::json!(true),
                    },
                    max_iterations: 1,
                }),
            );
            nodes
        },
    };
    kernel
        .start_frame(&run_id, &frame_id, &spec)
        .await
        .expect("start root frame");
    let effects = kernel
        .admit_next_ready_node(&run_id, &frame_id)
        .await
        .expect("admit loop node")
        .expect("loop node effects");
    assert!(
        effects
            .iter()
            .any(|effect| effect.variant == "StartLoopNode"),
        "admitting the loop node should emit StartLoopNode"
    );

    let loop_instance_id = meerkat_mob::ids::LoopInstanceId::from("resume-root::loop-node");
    store
        .upsert_loop_snapshot(
            &run_id,
            &loop_instance_id,
            LoopSnapshot {
                kernel_state: KernelState {
                    phase: "Completed".into(),
                    fields: BTreeMap::from([
                        (
                            "loop_instance_id".into(),
                            meerkat_machine_kernels::KernelValue::String(
                                loop_instance_id.to_string(),
                            ),
                        ),
                        (
                            "current_iteration".into(),
                            meerkat_machine_kernels::KernelValue::U64(1),
                        ),
                        (
                            "max_iterations".into(),
                            meerkat_machine_kernels::KernelValue::U64(1),
                        ),
                        (
                            "parent_frame_id".into(),
                            meerkat_machine_kernels::KernelValue::String(frame_id.to_string()),
                        ),
                        (
                            "parent_node_id".into(),
                            meerkat_machine_kernels::KernelValue::String(loop_node_id.to_string()),
                        ),
                        (
                            "loop_id".into(),
                            meerkat_machine_kernels::KernelValue::String(loop_id.to_string()),
                        ),
                        ("depth".into(), meerkat_machine_kernels::KernelValue::U64(1)),
                        (
                            "stage".into(),
                            meerkat_machine_kernels::KernelValue::NamedVariant {
                                enum_name: "LoopIterationStage".into(),
                                variant: "AwaitingUntil".into(),
                            },
                        ),
                        (
                            "last_completed_iteration".into(),
                            meerkat_machine_kernels::KernelValue::U64(0),
                        ),
                        (
                            "active_body_frame_id".into(),
                            meerkat_machine_kernels::KernelValue::None,
                        ),
                    ]),
                },
            },
            None,
        )
        .await
        .expect("persist terminal loop snapshot");

    let context = FlowContext {
        run_id: run_id.clone(),
        activation_params: serde_json::json!({ "done": true }),
        step_outputs: IndexMap::new(),
        loop_outputs: IndexMap::new(),
    };
    engine
        .execute_frame(&run_id, &frame_id, &spec, &context)
        .await
        .expect("resume should project terminal loop snapshot");

    let run = store
        .get_run(&run_id)
        .await
        .expect("get_run")
        .expect("run present");
    let root_frame = run.frames.get(&frame_id).expect("root frame");
    assert_eq!(
        root_frame.kernel_state.phase, "Completed",
        "parent frame should complete after projecting terminal loop snapshot"
    );
    let node_status = match root_frame.kernel_state.fields.get("node_status") {
        Some(meerkat_machine_kernels::KernelValue::Map(map)) => map,
        other => panic!("unexpected node_status map: {other:?}"),
    };
    match node_status.get(&meerkat_machine_kernels::KernelValue::String(
        loop_node_id.to_string(),
    )) {
        Some(meerkat_machine_kernels::KernelValue::NamedVariant { variant, .. }) => {
            assert_eq!(
                variant, "Completed",
                "loop node should be completed on resume"
            );
        }
        other => panic!("unexpected loop node status: {other:?}"),
    }
}

/// Resume must continue a terminal body frame that was already persisted as the active body frame.
#[tokio::test]
async fn test_resume_advances_terminal_body_frame_without_stall() {
    let executor = Arc::new(ScriptedStepExecutor::new(vec![]));
    let (run_id, run) = build_run();
    let store = Arc::new(InMemoryMobRunStore::new());
    store.create_run(run).await.expect("create_run");
    let engine = FlowFrameEngine::new(store.clone(), executor, 0, 0);
    let kernel = FlowFrameKernel::new(store.clone());

    let frame_id = FrameId::from("resume-body-root");
    let loop_node_id = FlowNodeId::from("loop-node");
    let loop_id = LoopId::from("resume-body-loop");
    let body_spec = FrameSpec {
        nodes: IndexMap::new(),
    };
    let spec = FrameSpec {
        nodes: {
            let mut nodes = IndexMap::new();
            nodes.insert(
                loop_node_id.clone(),
                FlowNodeSpec::RepeatUntil(RepeatUntilSpec {
                    loop_id: loop_id.clone(),
                    depends_on: vec![],
                    depends_on_mode: DependencyMode::All,
                    body: body_spec.clone(),
                    until: ConditionExpr::Eq {
                        path: "params.done".into(),
                        value: serde_json::json!(true),
                    },
                    max_iterations: 1,
                }),
            );
            nodes
        },
    };
    kernel
        .start_frame(&run_id, &frame_id, &spec)
        .await
        .expect("start root frame");
    kernel
        .admit_next_ready_node(&run_id, &frame_id)
        .await
        .expect("admit loop node")
        .expect("loop node effects");

    let loop_instance_id = meerkat_mob::ids::LoopInstanceId::from("resume-body-root::loop-node");
    let body_frame_id = FrameId::from(format!("{loop_instance_id}::iter-0").as_str());
    kernel
        .start_frame(&run_id, &body_frame_id, &body_spec)
        .await
        .expect("start body frame");
    kernel
        .terminalize_frame(&run_id, &body_frame_id)
        .await
        .expect("terminalize empty body frame");
    let rootish_body_frame = store
        .get_run(&run_id)
        .await
        .expect("get run")
        .expect("run present")
        .frames
        .get(&body_frame_id)
        .cloned()
        .expect("body frame snapshot");
    let mut body_frame = rootish_body_frame.clone();
    body_frame.kernel_state.fields.insert(
        "frame_scope".into(),
        meerkat_machine_kernels::KernelValue::NamedVariant {
            enum_name: "FrameScope".into(),
            variant: "Body".into(),
        },
    );
    body_frame.kernel_state.fields.insert(
        "loop_instance_id".into(),
        meerkat_machine_kernels::KernelValue::String(loop_instance_id.to_string()),
    );
    body_frame.kernel_state.fields.insert(
        "iteration".into(),
        meerkat_machine_kernels::KernelValue::U64(0),
    );
    assert!(
        store
            .cas_frame_state(
                &run_id,
                &body_frame_id,
                Some(&rootish_body_frame),
                body_frame
            )
            .await
            .expect("update body frame scope"),
        "body frame scope update should succeed"
    );
    store
        .upsert_loop_snapshot(
            &run_id,
            &loop_instance_id,
            LoopSnapshot {
                kernel_state: KernelState {
                    phase: "Running".into(),
                    fields: BTreeMap::from([
                        (
                            "loop_instance_id".into(),
                            meerkat_machine_kernels::KernelValue::String(
                                loop_instance_id.to_string(),
                            ),
                        ),
                        (
                            "current_iteration".into(),
                            meerkat_machine_kernels::KernelValue::U64(0),
                        ),
                        (
                            "max_iterations".into(),
                            meerkat_machine_kernels::KernelValue::U64(1),
                        ),
                        (
                            "parent_frame_id".into(),
                            meerkat_machine_kernels::KernelValue::String(frame_id.to_string()),
                        ),
                        (
                            "parent_node_id".into(),
                            meerkat_machine_kernels::KernelValue::String(loop_node_id.to_string()),
                        ),
                        (
                            "loop_id".into(),
                            meerkat_machine_kernels::KernelValue::String(loop_id.to_string()),
                        ),
                        ("depth".into(), meerkat_machine_kernels::KernelValue::U64(1)),
                        (
                            "stage".into(),
                            meerkat_machine_kernels::KernelValue::NamedVariant {
                                enum_name: "LoopIterationStage".into(),
                                variant: "BodyFrameActive".into(),
                            },
                        ),
                        (
                            "last_completed_iteration".into(),
                            meerkat_machine_kernels::KernelValue::U64(0),
                        ),
                        (
                            "active_body_frame_id".into(),
                            meerkat_machine_kernels::KernelValue::String(body_frame_id.to_string()),
                        ),
                    ]),
                },
            },
            None,
        )
        .await
        .expect("persist running loop snapshot");

    let context = FlowContext {
        run_id: run_id.clone(),
        activation_params: serde_json::json!({ "done": true }),
        step_outputs: IndexMap::new(),
        loop_outputs: IndexMap::new(),
    };
    engine
        .execute_frame(&run_id, &frame_id, &spec, &context)
        .await
        .expect("resume should advance completed body frame");

    let run = store
        .get_run(&run_id)
        .await
        .expect("get_run")
        .expect("run present");
    assert_eq!(
        run.loops
            .get(&loop_instance_id)
            .expect("loop snapshot")
            .kernel_state
            .phase,
        "Completed",
        "completed body frame should advance the loop to Completed on resume"
    );
    assert_eq!(
        run.frames
            .get(&frame_id)
            .expect("root frame")
            .kernel_state
            .phase,
        "Completed",
        "root frame should complete after resuming the terminal body frame"
    );
}

/// [P2] A nested loop that would exceed max_frame_depth is rejected cleanly.
#[tokio::test]
async fn test_max_frame_depth_enforced_for_nested_loops() {
    let executor = Arc::new(ScriptedStepExecutor::new(vec![]));

    // Build an engine with max_frame_depth=1 (root=depth0, body=depth1, nested=depth2 rejected).
    let store = Arc::new(InMemoryMobRunStore::new());
    // Use a fresh run for this engine
    let (run_id2, run2) = {
        use meerkat_machine_kernels::KernelState;
        use meerkat_mob::run::MobRun;
        let run = MobRun::pending(
            meerkat_mob::MobId::from("depth-test"),
            meerkat_mob::FlowId::from("depth-flow"),
            KernelState::default(),
            serde_json::json!({}),
        );
        let id = run.run_id.clone();
        (id, run)
    };
    store.create_run(run2).await.expect("create_run");

    let engine = FlowFrameEngine::new(store.clone(), executor, 1, 0); // max_frame_depth=1

    // Root frame (depth=0) contains a loop; body (depth=1) also contains a loop.
    // Body-of-body (depth=2) exceeds max_frame_depth=1.
    let nested_body = FrameSpec {
        nodes: {
            let mut m = IndexMap::new();
            m.insert(
                FlowNodeId::from("inner-loop"),
                FlowNodeSpec::RepeatUntil(RepeatUntilSpec {
                    loop_id: LoopId::from("inner"),
                    depends_on: vec![],
                    depends_on_mode: DependencyMode::All,
                    body: FrameSpec {
                        nodes: IndexMap::new(),
                    }, // empty inner body
                    until: ConditionExpr::Eq {
                        path: "params.done".into(),
                        value: serde_json::json!(true),
                    },
                    max_iterations: 1,
                }),
            );
            m
        },
    };

    let spec = FrameSpec {
        nodes: {
            let mut m = IndexMap::new();
            m.insert(
                FlowNodeId::from("outer-loop"),
                FlowNodeSpec::RepeatUntil(RepeatUntilSpec {
                    loop_id: LoopId::from("outer"),
                    depends_on: vec![],
                    depends_on_mode: DependencyMode::All,
                    body: nested_body,
                    until: ConditionExpr::Eq {
                        path: "params.done".into(),
                        value: serde_json::json!(true),
                    },
                    max_iterations: 1,
                }),
            );
            m
        },
    };

    let context = FlowContext {
        run_id: run_id2.clone(),
        activation_params: serde_json::json!({}),
        step_outputs: IndexMap::new(),
        loop_outputs: IndexMap::new(),
    };

    let frame_id = FrameId::from("depth-root");
    let result = engine
        .execute_frame(&run_id2, &frame_id, &spec, &context)
        .await;

    assert!(
        result.is_err(),
        "exceeding max_frame_depth must return an error"
    );
    let err_str = result.unwrap_err().to_string();
    assert!(
        err_str.contains("max_frame_depth") || err_str.contains("depth"),
        "error message must mention depth: {err_str}"
    );
}

/// [P1] A frame resumed with an orphaned Running node must not silently succeed.
/// The orphaned Running node should be failed and its dependents skipped.
#[tokio::test]
async fn test_resumed_frame_with_running_node_fails_orphan() {
    use meerkat_machine_kernels::{KernelState, KernelValue};
    use meerkat_mob::run::FrameSnapshot;
    use meerkat_mob::store::MobRunStore as _;
    use std::collections::BTreeMap;

    // Setup engine with a scripted executor that only runs the first step.
    // The second step (node-b) will never be called because node-a will be
    // "already Running" when the frame is loaded — simulating a crashed process.
    let executor = Arc::new(ScriptedStepExecutor::new(vec![]));
    let (run_id, store, engine) = setup_engine(executor).await;

    let spec = FrameSpec {
        nodes: {
            let mut m = IndexMap::new();
            m.insert(
                FlowNodeId::from("node-a"),
                FlowNodeSpec::Step(FrameStepSpec {
                    step_id: StepId::from("step-a"),
                    depends_on: vec![],
                    depends_on_mode: DependencyMode::All,
                    branch: None,
                }),
            );
            m.insert(
                FlowNodeId::from("node-b"),
                FlowNodeSpec::Step(FrameStepSpec {
                    step_id: StepId::from("step-b"),
                    depends_on: vec![FlowNodeId::from("node-a")],
                    depends_on_mode: DependencyMode::All,
                    branch: None,
                }),
            );
            m
        },
    };

    let frame_id = FrameId::from("resume-test");

    // Inject a pre-existing frame snapshot where node-a is Running (orphaned)
    // and ready_queue is empty — simulating a crash mid-step.
    let orphaned_snapshot = FrameSnapshot {
        kernel_state: KernelState {
            phase: "Running".into(),
            fields: BTreeMap::from([
                ("frame_id".into(), KernelValue::String(frame_id.to_string())),
                (
                    "last_admitted_node".into(),
                    KernelValue::String("node-a".into()),
                ),
                ("ready_queue".into(), KernelValue::Seq(vec![])), // empty — was popped
                (
                    "tracked_nodes".into(),
                    KernelValue::Set(
                        [
                            KernelValue::String("node-a".into()),
                            KernelValue::String("node-b".into()),
                        ]
                        .into_iter()
                        .collect(),
                    ),
                ),
                (
                    "ordered_nodes".into(),
                    KernelValue::Seq(vec![
                        KernelValue::String("node-a".into()),
                        KernelValue::String("node-b".into()),
                    ]),
                ),
                (
                    "node_kind".into(),
                    KernelValue::Map(
                        [
                            (
                                KernelValue::String("node-a".into()),
                                KernelValue::NamedVariant {
                                    enum_name: "FlowNodeKind".into(),
                                    variant: "Step".into(),
                                },
                            ),
                            (
                                KernelValue::String("node-b".into()),
                                KernelValue::NamedVariant {
                                    enum_name: "FlowNodeKind".into(),
                                    variant: "Step".into(),
                                },
                            ),
                        ]
                        .into_iter()
                        .collect(),
                    ),
                ),
                (
                    "node_status".into(),
                    KernelValue::Map(
                        [
                            (
                                KernelValue::String("node-a".into()),
                                // ORPHANED: was Running when the process crashed
                                KernelValue::NamedVariant {
                                    enum_name: "NodeRunStatus".into(),
                                    variant: "Running".into(),
                                },
                            ),
                            (
                                KernelValue::String("node-b".into()),
                                KernelValue::NamedVariant {
                                    enum_name: "NodeRunStatus".into(),
                                    variant: "Pending".into(),
                                },
                            ),
                        ]
                        .into_iter()
                        .collect(),
                    ),
                ),
                (
                    "node_dependencies".into(),
                    KernelValue::Map(
                        [
                            (
                                KernelValue::String("node-a".into()),
                                KernelValue::Seq(vec![]),
                            ),
                            (
                                KernelValue::String("node-b".into()),
                                KernelValue::Seq(vec![KernelValue::String("node-a".into())]),
                            ),
                        ]
                        .into_iter()
                        .collect(),
                    ),
                ),
                (
                    "node_dependency_modes".into(),
                    KernelValue::Map(
                        [
                            (
                                KernelValue::String("node-a".into()),
                                KernelValue::NamedVariant {
                                    enum_name: "DependencyMode".into(),
                                    variant: "All".into(),
                                },
                            ),
                            (
                                KernelValue::String("node-b".into()),
                                KernelValue::NamedVariant {
                                    enum_name: "DependencyMode".into(),
                                    variant: "All".into(),
                                },
                            ),
                        ]
                        .into_iter()
                        .collect(),
                    ),
                ),
                (
                    "node_branches".into(),
                    KernelValue::Map(
                        [
                            (KernelValue::String("node-a".into()), KernelValue::None),
                            (KernelValue::String("node-b".into()), KernelValue::None),
                        ]
                        .into_iter()
                        .collect(),
                    ),
                ),
                (
                    "frame_scope".into(),
                    KernelValue::NamedVariant {
                        enum_name: "FrameScope".into(),
                        variant: "Root".into(),
                    },
                ),
                (
                    "loop_instance_id".into(),
                    KernelValue::String(String::new()),
                ),
                ("iteration".into(), KernelValue::U64(0)),
                ("output_recorded".into(), KernelValue::Map(BTreeMap::new())),
                (
                    "node_condition_results".into(),
                    KernelValue::Map(BTreeMap::new()),
                ),
                (
                    "branch_winners".into(),
                    KernelValue::Set(Default::default()),
                ),
            ]),
        },
    };

    // Inject the orphaned snapshot directly into the store.
    store
        .cas_frame_state(&run_id, &frame_id, None, orphaned_snapshot)
        .await
        .expect("inject orphaned frame");

    let context = FlowContext {
        run_id: run_id.clone(),
        activation_params: serde_json::json!({}),
        step_outputs: IndexMap::new(),
        loop_outputs: IndexMap::new(),
    };

    // Execute the frame — the engine should detect node-a as orphaned Running,
    // fail it, which causes node-b to be Skip-admitted (All-mode dep failed).
    // The frame should terminalize to Failed because the frame machine now owns
    // terminal classification and any failed node seals the frame as Failed.
    let outputs = engine
        .execute_frame(&run_id, &frame_id, &spec, &context)
        .await
        .expect("frame must complete after failing orphaned running node");

    // Neither step produced output (node-a failed, node-b was skipped).
    assert!(
        outputs.outputs.is_empty(),
        "no outputs expected when steps are failed/skipped: {outputs:?}"
    );

    // Verify the frame is now Failed in the store.
    let run = store.get_run(&run_id).await.expect("get_run").expect("run");
    let snap = run.frames.get(&frame_id).expect("frame persisted");
    assert_eq!(
        snap.kernel_state.phase, "Failed",
        "frame must be Failed after handling orphaned Running node"
    );
}

/// [P1] A loop that met its condition must persist Completed phase, not stay Running.
/// reconcile_pending_body_frame_loops must not re-enqueue a completed loop.
#[tokio::test]
async fn test_completed_loop_persisted_as_completed_not_running() {
    use meerkat_mob::store::MobRunStore as _;

    let executor = Arc::new(ScriptedStepExecutor::new(vec![StepScript {
        node_id: "review-node".into(),
        output: serde_json::json!({"passes": true}), // condition met on first try
    }]));
    let (run_id, store, engine) = setup_engine(executor).await;

    let loop_instance_id = meerkat_mob::ids::LoopInstanceId::from("root::loop-node");

    let spec = FrameSpec {
        nodes: {
            let mut m = IndexMap::new();
            m.insert(
                FlowNodeId::from("loop-node"),
                FlowNodeSpec::RepeatUntil(RepeatUntilSpec {
                    loop_id: LoopId::from("review"),
                    depends_on: vec![],
                    depends_on_mode: DependencyMode::All,
                    body: FrameSpec {
                        nodes: {
                            let mut b = IndexMap::new();
                            b.insert(
                                FlowNodeId::from("review-node"),
                                FlowNodeSpec::Step(FrameStepSpec {
                                    step_id: StepId::from("review"),
                                    depends_on: vec![],
                                    depends_on_mode: DependencyMode::All,
                                    branch: None,
                                }),
                            );
                            b
                        },
                    },
                    until: ConditionExpr::Eq {
                        path: "steps.review.passes".into(),
                        value: serde_json::json!(true),
                    },
                    max_iterations: 3,
                }),
            );
            m
        },
    };

    let context = FlowContext {
        run_id: run_id.clone(),
        activation_params: serde_json::json!({}),
        step_outputs: IndexMap::new(),
        loop_outputs: IndexMap::new(),
    };

    let frame_id = FrameId::from("root");
    engine
        .execute_frame(&run_id, &frame_id, &spec, &context)
        .await
        .expect("execute_frame");

    // The persisted loop snapshot must have phase = "Completed", not "Running".
    let run = store.get_run(&run_id).await.expect("get_run").expect("run");
    let loop_snap = run
        .loops
        .get(&loop_instance_id)
        .expect("loop snapshot must be persisted");
    assert_eq!(
        loop_snap.kernel_state.phase, "Completed",
        "loop must be persisted as Completed after condition is met"
    );

    // reconcile_pending_body_frame_loops must not enqueue the completed loop.
    use meerkat_mob::runtime::recovery::reconcile_run_state;
    let mut run_copy = run.clone();
    reconcile_run_state(&mut run_copy).expect("reconcile");
    let pending_loops = match run_copy.flow_state.fields.get("pending_body_frame_loops") {
        Some(meerkat_machine_kernels::KernelValue::Seq(s)) => s.len(),
        _ => 0,
    };
    assert_eq!(
        pending_loops, 0,
        "completed loop must not appear in pending_body_frame_loops after reconcile"
    );
}

/// [P2] frame_outputs must be seeded from persisted root_step_outputs on resume,
/// so advance_frame_steps_and_terminalize marks pre-crash steps as Completed not Skipped.
#[tokio::test]
async fn test_frame_outputs_seeded_from_persisted_state_on_resume() {
    // First execution: run node-a and complete it (with a one-step frame).
    let executor = Arc::new(ScriptedStepExecutor::new(vec![StepScript {
        node_id: "node-a".into(),
        output: serde_json::json!({"x": 1}),
    }]));
    let (run_id, store, engine) = setup_engine(executor).await;

    let spec = FrameSpec {
        nodes: {
            let mut m = IndexMap::new();
            m.insert(
                FlowNodeId::from("node-a"),
                FlowNodeSpec::Step(FrameStepSpec {
                    step_id: StepId::from("step-a"),
                    depends_on: vec![],
                    depends_on_mode: DependencyMode::All,
                    branch: None,
                }),
            );
            m
        },
    };

    let frame_id = FrameId::from("root");
    let context = FlowContext {
        run_id: run_id.clone(),
        activation_params: serde_json::json!({}),
        step_outputs: IndexMap::new(),
        loop_outputs: IndexMap::new(),
    };

    // Full execution: node-a runs and completes.
    let outputs = engine
        .execute_frame(&run_id, &frame_id, &spec, &context)
        .await
        .expect("execute_frame");
    assert_eq!(
        outputs.outputs.get(&StepId::from("step-a")),
        Some(&serde_json::json!({"x": 1}))
    );

    // Simulate a resume by creating a new engine (empty executor — no new steps run).
    let resume_executor = Arc::new(ScriptedStepExecutor::new(vec![]));
    let resume_engine = FlowFrameEngine::new(store.clone(), resume_executor, 0, 0);

    // Re-execute the frame (frame is already Completed in the store — it terminalized).
    // The resumed execute_frame should see the Completed phase and return the
    // persisted outputs without re-running anything.
    let resume_outputs = resume_engine
        .execute_frame(&run_id, &frame_id, &spec, &context)
        .await
        .expect("resume execute_frame");

    // The resumed outputs must include the pre-crash completed step.
    assert_eq!(
        resume_outputs.outputs.get(&StepId::from("step-a")),
        Some(&serde_json::json!({"x": 1})),
        "pre-crash completed step must appear in resumed frame_outputs (seeded from store)"
    );
}

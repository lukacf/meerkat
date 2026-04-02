#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::redundant_clone,
    clippy::uninlined_format_args
)]

use meerkat_machine_kernels::generated::flow_frame;
use meerkat_machine_kernels::{KernelInput, KernelState, KernelValue};
use std::collections::{BTreeMap, BTreeSet};

// Helper: build a KernelValue::NamedVariant for NodeRunStatus
fn status(variant: &str) -> KernelValue {
    KernelValue::NamedVariant {
        enum_name: "NodeRunStatus".into(),
        variant: variant.into(),
    }
}

fn dep_mode_all() -> KernelValue {
    KernelValue::NamedVariant {
        enum_name: "DependencyMode".into(),
        variant: "All".into(),
    }
}

fn node_kind_step() -> KernelValue {
    KernelValue::NamedVariant {
        enum_name: "FlowNodeKind".into(),
        variant: "Step".into(),
    }
}

fn node_kind_loop() -> KernelValue {
    KernelValue::NamedVariant {
        enum_name: "FlowNodeKind".into(),
        variant: "Loop".into(),
    }
}

fn str_val(s: &str) -> KernelValue {
    KernelValue::String(s.into())
}
fn seq(items: Vec<KernelValue>) -> KernelValue {
    KernelValue::Seq(items)
}
fn set(items: Vec<KernelValue>) -> KernelValue {
    KernelValue::Set(items.into_iter().collect())
}
fn map(entries: Vec<(KernelValue, KernelValue)>) -> KernelValue {
    KernelValue::Map(entries.into_iter().collect())
}

// Assert the invariant: {n: node_status[n] == Ready} == set(ready_queue)
fn assert_ready_queue_invariant(state: &KernelState, label: &str) {
    let queue = match state.fields.get("ready_queue").expect("ready_queue") {
        KernelValue::Seq(v) => v.clone(),
        other => panic!("ready_queue is not Seq: {:?}", other),
    };
    let queue_set: BTreeSet<KernelValue> = queue.iter().cloned().collect();

    let status_map = match state.fields.get("node_status").expect("node_status") {
        KernelValue::Map(m) => m.clone(),
        other => panic!("node_status is not Map: {:?}", other),
    };

    let ready_nodes: BTreeSet<KernelValue> = status_map
        .iter()
        .filter_map(|(k, v)| {
            if matches!(v, KernelValue::NamedVariant { variant, .. } if variant == "Ready") {
                Some(k.clone())
            } else {
                None
            }
        })
        .collect();

    assert_eq!(
        queue_set, ready_nodes,
        "{}: ready_queue set {:?} != nodes with Ready status {:?}",
        label, queue_set, ready_nodes
    );
}

fn frame_id_val() -> KernelValue {
    str_val("frame-test-1")
}

fn start_frame_input_single_root_node() -> KernelInput {
    // Single node "A" with no deps, kind=Step
    let node_a = str_val("node-a");
    KernelInput {
        variant: "StartRootFrame".into(),
        fields: BTreeMap::from([
            ("frame_id".into(), frame_id_val()),
            ("tracked_nodes".into(), set(vec![node_a.clone()])),
            ("ordered_nodes".into(), seq(vec![node_a.clone()])),
            (
                "node_kind".into(),
                map(vec![(node_a.clone(), node_kind_step())]),
            ),
            (
                "node_dependencies".into(),
                map(vec![(node_a.clone(), seq(vec![]))]),
            ),
            (
                "node_dependency_modes".into(),
                map(vec![(node_a.clone(), dep_mode_all())]),
            ),
            (
                "node_branches".into(),
                map(vec![(node_a.clone(), KernelValue::None)]),
            ),
        ]),
    }
}

fn start_frame_input_dag_a_then_b() -> KernelInput {
    // Two nodes: A (root) -> B (depends on A)
    let node_a = str_val("node-a");
    let node_b = str_val("node-b");
    KernelInput {
        variant: "StartRootFrame".into(),
        fields: BTreeMap::from([
            ("frame_id".into(), frame_id_val()),
            (
                "tracked_nodes".into(),
                set(vec![node_a.clone(), node_b.clone()]),
            ),
            (
                "ordered_nodes".into(),
                seq(vec![node_a.clone(), node_b.clone()]),
            ),
            (
                "node_kind".into(),
                map(vec![
                    (node_a.clone(), node_kind_step()),
                    (node_b.clone(), node_kind_step()),
                ]),
            ),
            (
                "node_dependencies".into(),
                map(vec![
                    (node_a.clone(), seq(vec![])),
                    (node_b.clone(), seq(vec![node_a.clone()])),
                ]),
            ),
            (
                "node_dependency_modes".into(),
                map(vec![
                    (node_a.clone(), dep_mode_all()),
                    (node_b.clone(), dep_mode_all()),
                ]),
            ),
            (
                "node_branches".into(),
                map(vec![
                    (node_a.clone(), KernelValue::None),
                    (node_b.clone(), KernelValue::None),
                ]),
            ),
        ]),
    }
}

/// CHOKE-01: The ready_queue invariant must hold after EVERY transition.
/// node_status[n] == Ready <-> n in ready_queue
#[test]
fn test_ready_queue_invariant_holds_across_all_transitions() {
    let state = flow_frame::initial_state().expect("init");
    assert_ready_queue_invariant(&state, "initial");

    // StartRootFrame with A->B DAG
    let start_input = start_frame_input_dag_a_then_b();
    let outcome = flow_frame::transition(&state, &start_input).expect("StartRootFrame");
    let state = outcome.next_state;
    assert_ready_queue_invariant(&state, "after StartRootFrame");

    // Only A should be in ready_queue (B has dep on A)
    let queue = match state.fields.get("ready_queue").expect("ready_queue") {
        KernelValue::Seq(v) => v.clone(),
        _ => panic!("not seq"),
    };
    assert_eq!(queue.len(), 1, "only root node should be ready");
    assert_eq!(queue[0], str_val("node-a"));

    // AdmitNextReadyNode (should admit A as step-run)
    let admit_input = KernelInput {
        variant: "AdmitNextReadyNode".into(),
        fields: BTreeMap::new(),
    };
    let outcome = flow_frame::transition(&state, &admit_input).expect("AdmitNextReadyNode");
    let state = outcome.next_state;
    assert_ready_queue_invariant(&state, "after AdmitNextReadyNode");
    // A should now be Running, not in queue
    let queue = match state.fields.get("ready_queue").expect("rq") {
        KernelValue::Seq(v) => v.clone(),
        _ => panic!("not seq"),
    };
    assert!(
        queue.is_empty(),
        "queue should be empty after admitting only node"
    );

    // CompleteNode A -> B should become ready
    let complete_input = KernelInput {
        variant: "CompleteNode".into(),
        fields: BTreeMap::from([("node_id".into(), str_val("node-a"))]),
    };
    let outcome = flow_frame::transition(&state, &complete_input).expect("CompleteNode");
    let state = outcome.next_state;
    assert_ready_queue_invariant(&state, "after CompleteNode A");
    // B should now be in queue
    let queue = match state.fields.get("ready_queue").expect("rq") {
        KernelValue::Seq(v) => v.clone(),
        _ => panic!("not seq"),
    };
    assert_eq!(queue.len(), 1, "B should be ready after A completes");
    assert_eq!(queue[0], str_val("node-b"));

    // AdmitNextReadyNode (B)
    let admit_input = KernelInput {
        variant: "AdmitNextReadyNode".into(),
        fields: BTreeMap::new(),
    };
    let outcome = flow_frame::transition(&state, &admit_input).expect("AdmitNextReadyNode B");
    let state = outcome.next_state;
    assert_ready_queue_invariant(&state, "after AdmitNextReadyNode B");

    // CompleteNode B
    let complete_b = KernelInput {
        variant: "CompleteNode".into(),
        fields: BTreeMap::from([("node_id".into(), str_val("node-b"))]),
    };
    let outcome = flow_frame::transition(&state, &complete_b).expect("CompleteNode B");
    let state = outcome.next_state;
    assert_ready_queue_invariant(&state, "after CompleteNode B");

    // SealFrame
    let term_input = KernelInput {
        variant: "SealFrame".into(),
        fields: BTreeMap::new(),
    };
    let outcome = flow_frame::transition(&state, &term_input).expect("SealFrame");
    let state = outcome.next_state;
    assert_ready_queue_invariant(&state, "after SealFrame");
    assert_eq!(state.phase, "Completed");
}

#[test]
fn test_refresh_ready_frontier_seeds_roots_on_start_frame() {
    // A->B->C (all All-mode), only A should be in queue after StartRootFrame
    let node_a = str_val("node-a");
    let node_b = str_val("node-b");
    let node_c = str_val("node-c");
    let state = flow_frame::initial_state().expect("init");
    let start = KernelInput {
        variant: "StartRootFrame".into(),
        fields: BTreeMap::from([
            ("frame_id".into(), frame_id_val()),
            (
                "tracked_nodes".into(),
                set(vec![node_a.clone(), node_b.clone(), node_c.clone()]),
            ),
            (
                "ordered_nodes".into(),
                seq(vec![node_a.clone(), node_b.clone(), node_c.clone()]),
            ),
            (
                "node_kind".into(),
                map(vec![
                    (node_a.clone(), node_kind_step()),
                    (node_b.clone(), node_kind_step()),
                    (node_c.clone(), node_kind_step()),
                ]),
            ),
            (
                "node_dependencies".into(),
                map(vec![
                    (node_a.clone(), seq(vec![])),
                    (node_b.clone(), seq(vec![node_a.clone()])),
                    (node_c.clone(), seq(vec![node_b.clone()])),
                ]),
            ),
            (
                "node_dependency_modes".into(),
                map(vec![
                    (node_a.clone(), dep_mode_all()),
                    (node_b.clone(), dep_mode_all()),
                    (node_c.clone(), dep_mode_all()),
                ]),
            ),
            (
                "node_branches".into(),
                map(vec![
                    (node_a.clone(), KernelValue::None),
                    (node_b.clone(), KernelValue::None),
                    (node_c.clone(), KernelValue::None),
                ]),
            ),
        ]),
    };
    let outcome = flow_frame::transition(&state, &start).expect("StartRootFrame");
    let state = outcome.next_state;
    let queue = match state.fields.get("ready_queue").expect("rq") {
        KernelValue::Seq(v) => v.clone(),
        _ => panic!("not seq"),
    };
    assert_eq!(queue, vec![node_a.clone()], "only A (root) should be ready");
    let status_a = state.fields.get("node_status").and_then(|m| {
        if let KernelValue::Map(m) = m {
            m.get(&node_a).cloned()
        } else {
            None
        }
    });
    assert_eq!(status_a, Some(status("Ready")));
    let status_b = state.fields.get("node_status").and_then(|m| {
        if let KernelValue::Map(m) = m {
            m.get(&node_b).cloned()
        } else {
            None
        }
    });
    assert_eq!(status_b, Some(status("Pending")));
}

#[test]
fn test_admit_step_run_pops_head_and_marks_running() {
    let state = flow_frame::initial_state().expect("init");
    let start = start_frame_input_single_root_node();
    let outcome = flow_frame::transition(&state, &start).expect("StartRootFrame");
    let state = outcome.next_state;

    // Queue: [A], status[A] = Ready
    let admit = KernelInput {
        variant: "AdmitNextReadyNode".into(),
        fields: BTreeMap::new(),
    };
    let outcome = flow_frame::transition(&state, &admit).expect("AdmitNextReadyNode");
    let state = outcome.next_state;

    // Queue should be empty, status[A] should be Running
    let queue = match state.fields.get("ready_queue").expect("rq") {
        KernelValue::Seq(v) => v.clone(),
        _ => panic!(),
    };
    assert!(queue.is_empty());
    let status_a = state.fields.get("node_status").and_then(|m| {
        if let KernelValue::Map(m) = m {
            m.get(&str_val("node-a")).cloned()
        } else {
            None
        }
    });
    assert_eq!(status_a, Some(status("Running")));

    // AdmitStepWork effect should be emitted
    assert!(
        outcome.effects.iter().any(|e| e.variant == "AdmitStepWork"),
        "AdmitStepWork effect expected, got: {:?}",
        outcome
            .effects
            .iter()
            .map(|e| &e.variant)
            .collect::<Vec<_>>()
    );
}

#[test]
fn test_admit_skip_when_all_dep_failed() {
    // A (root) -> B (depends on A, All mode)
    // After A fails, B should be admitted as Skip
    let node_a = str_val("node-a");
    let node_b = str_val("node-b");
    let state = flow_frame::initial_state().expect("init");
    let start = start_frame_input_dag_a_then_b();
    let outcome = flow_frame::transition(&state, &start).expect("StartRootFrame");
    let state = outcome.next_state;

    // Admit A
    let admit = KernelInput {
        variant: "AdmitNextReadyNode".into(),
        fields: BTreeMap::new(),
    };
    let outcome = flow_frame::transition(&state, &admit).expect("admit A");
    let state = outcome.next_state;

    // Fail A
    let fail_a = KernelInput {
        variant: "FailNode".into(),
        fields: BTreeMap::from([("node_id".into(), node_a.clone())]),
    };
    let outcome = flow_frame::transition(&state, &fail_a).expect("FailNode A");
    let state = outcome.next_state;
    // After A fails, B should be in ready_queue (eligible for skip)
    assert_ready_queue_invariant(&state, "after FailNode A");
    let queue = match state.fields.get("ready_queue").expect("rq") {
        KernelValue::Seq(v) => v.clone(),
        _ => panic!(),
    };
    assert!(
        queue.contains(&node_b),
        "B should be queued after A fails (for skip admission)"
    );

    // Admit B -- should be Skipped (because its only dep A failed under All mode)
    let admit_b = KernelInput {
        variant: "AdmitNextReadyNode".into(),
        fields: BTreeMap::new(),
    };
    let outcome = flow_frame::transition(&state, &admit_b).expect("admit B");
    let state = outcome.next_state;
    let status_b = state.fields.get("node_status").and_then(|m| {
        if let KernelValue::Map(m) = m {
            m.get(&node_b).cloned()
        } else {
            None
        }
    });
    assert_eq!(
        status_b,
        Some(status("Skipped")),
        "B should be Skipped when its All-dep failed"
    );
    assert_ready_queue_invariant(&state, "after admit B as skip");
}

#[test]
fn test_admit_loop_node_emits_start_loop_node() {
    // Single node "loop-node" with kind=Loop, no deps
    let loop_node = str_val("loop-node");
    let state = flow_frame::initial_state().expect("init");
    let start = KernelInput {
        variant: "StartRootFrame".into(),
        fields: BTreeMap::from([
            ("frame_id".into(), frame_id_val()),
            ("tracked_nodes".into(), set(vec![loop_node.clone()])),
            ("ordered_nodes".into(), seq(vec![loop_node.clone()])),
            (
                "node_kind".into(),
                map(vec![(loop_node.clone(), node_kind_loop())]),
            ),
            (
                "node_dependencies".into(),
                map(vec![(loop_node.clone(), seq(vec![]))]),
            ),
            (
                "node_dependency_modes".into(),
                map(vec![(loop_node.clone(), dep_mode_all())]),
            ),
            (
                "node_branches".into(),
                map(vec![(loop_node.clone(), KernelValue::None)]),
            ),
        ]),
    };
    let outcome = flow_frame::transition(&state, &start).expect("StartRootFrame");
    let state = outcome.next_state;
    assert_ready_queue_invariant(&state, "after StartRootFrame loop");

    let admit = KernelInput {
        variant: "AdmitNextReadyNode".into(),
        fields: BTreeMap::new(),
    };
    let outcome = flow_frame::transition(&state, &admit).expect("AdmitNextReadyNode loop");

    // Should emit StartLoopNode (not AdmitStepWork)
    assert!(
        outcome.effects.iter().any(|e| e.variant == "StartLoopNode"),
        "StartLoopNode effect expected, got: {:?}",
        outcome
            .effects
            .iter()
            .map(|e| &e.variant)
            .collect::<Vec<_>>()
    );
    assert!(
        !outcome.effects.iter().any(|e| e.variant == "AdmitStepWork"),
        "AdmitStepWork should NOT be emitted for loop node"
    );
    assert_ready_queue_invariant(&outcome.next_state, "after admit loop node");
}

#[test]
fn test_admit_fail_when_any_mode_all_deps_skipped_or_failed() {
    // Setup: C depends on A and B via Any mode
    // A gets admitted and Skipped, B gets admitted and Skipped
    // Then C should be admitted as Fail (Any mode, no dep Completed)
    let node_a = str_val("node-a");
    let node_b = str_val("node-b");
    let node_c = str_val("node-c");

    let dep_mode_any = || KernelValue::NamedVariant {
        enum_name: "DependencyMode".into(),
        variant: "Any".into(),
    };

    let state = flow_frame::initial_state().expect("init");
    let start = KernelInput {
        variant: "StartRootFrame".into(),
        fields: BTreeMap::from([
            ("frame_id".into(), str_val("frame-test")),
            (
                "tracked_nodes".into(),
                set(vec![node_a.clone(), node_b.clone(), node_c.clone()]),
            ),
            (
                "ordered_nodes".into(),
                seq(vec![node_a.clone(), node_b.clone(), node_c.clone()]),
            ),
            (
                "node_kind".into(),
                map(vec![
                    (node_a.clone(), node_kind_step()),
                    (node_b.clone(), node_kind_step()),
                    (node_c.clone(), node_kind_step()),
                ]),
            ),
            (
                "node_dependencies".into(),
                map(vec![
                    (node_a.clone(), seq(vec![])),
                    (node_b.clone(), seq(vec![])),
                    (node_c.clone(), seq(vec![node_a.clone(), node_b.clone()])),
                ]),
            ),
            (
                "node_dependency_modes".into(),
                map(vec![
                    (node_a.clone(), dep_mode_all()),
                    (node_b.clone(), dep_mode_all()),
                    (node_c.clone(), dep_mode_any()),
                ]),
            ),
            (
                "node_branches".into(),
                map(vec![
                    (node_a.clone(), KernelValue::None),
                    (node_b.clone(), KernelValue::None),
                    (node_c.clone(), KernelValue::None),
                ]),
            ),
        ]),
    };
    let outcome = flow_frame::transition(&state, &start).expect("StartRootFrame");
    let mut state = outcome.next_state;
    assert_ready_queue_invariant(&state, "after StartRootFrame");

    // Admit A and B (both roots), skip each after admission
    for node in [&node_a, &node_b] {
        let admit = KernelInput {
            variant: "AdmitNextReadyNode".into(),
            fields: BTreeMap::new(),
        };
        let outcome = flow_frame::transition(&state, &admit).expect("admit root");
        state = outcome.next_state;
        assert_ready_queue_invariant(&state, "after admit root");

        // Skip A/B (must be Running at this point)
        let skip = KernelInput {
            variant: "SkipNode".into(),
            fields: BTreeMap::from([("node_id".into(), node.clone())]),
        };
        let outcome = flow_frame::transition(&state, &skip).expect("skip root");
        state = outcome.next_state;
        assert_ready_queue_invariant(&state, "after skip root");
    }

    // Now A and B are both Skipped. C depends on A and B via Any mode.
    // C should be in ready_queue (all deps terminal, none Completed → eligible for fail admission)
    let queue = match state.fields.get("ready_queue").expect("rq") {
        KernelValue::Seq(v) => v.clone(),
        _ => panic!("not seq"),
    };
    assert!(
        queue.contains(&node_c),
        "C should be queued when all Any-mode deps are terminal"
    );

    // Admit C — should be Failed (Any-mode, no dep Completed)
    let admit_c = KernelInput {
        variant: "AdmitNextReadyNode".into(),
        fields: BTreeMap::new(),
    };
    let outcome = flow_frame::transition(&state, &admit_c).expect("admit C");
    let state = outcome.next_state;

    let status_c = state.fields.get("node_status").and_then(|m| {
        if let KernelValue::Map(m) = m {
            m.get(&node_c).cloned()
        } else {
            None
        }
    });
    assert_eq!(
        status_c,
        Some(status("Failed")),
        "C should be Failed when all Any-mode deps are Skipped (none Completed)"
    );
    assert_ready_queue_invariant(&state, "after admit C as fail");
}

#[test]
fn test_ready_frontier_emits_ready_frontier_changed_effect() {
    // StartRootFrame should emit ReadyFrontierChanged when root nodes exist
    let state = flow_frame::initial_state().expect("init");
    let start = start_frame_input_single_root_node();
    let outcome = flow_frame::transition(&state, &start).expect("StartRootFrame");
    assert!(
        outcome
            .effects
            .iter()
            .any(|e| e.variant == "ReadyFrontierChanged"),
        "StartRootFrame should emit ReadyFrontierChanged when ready_queue becomes non-empty, got: {:?}",
        outcome
            .effects
            .iter()
            .map(|e| &e.variant)
            .collect::<Vec<_>>()
    );
}

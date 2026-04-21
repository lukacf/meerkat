#![allow(clippy::expect_used)]

use indexmap::IndexMap;
use meerkat_mob::ids::RunId;
use meerkat_mob::run::{FlowContext, LoopContextHistory, MobRun, MobRunStatus};
use meerkat_mob::runtime::recovery::reconcile_run_state;
use std::collections::BTreeMap;

fn minimal_run(schema_version: u32, status: MobRunStatus) -> MobRun {
    MobRun {
        run_id: RunId::new(),
        mob_id: meerkat_mob::MobId::from("row22-mob"),
        flow_id: meerkat_mob::FlowId::from("row22-flow"),
        status,
        flow_state: meerkat_machine_kernels::legacy_generated::flow_run::initial_state()
            .expect("init"),
        activation_params: serde_json::json!({}),
        created_at: chrono::Utc::now(),
        completed_at: None,
        step_ledger: vec![],
        failure_ledger: vec![],
        frames: BTreeMap::new(),
        loops: BTreeMap::new(),
        loop_iteration_ledger: vec![],
        schema_version,
        root_step_outputs: IndexMap::new(),
        loop_iteration_outputs: BTreeMap::new(),
    }
}

#[test]
fn typed_flow_run_persists_as_schema_version_5() {
    let run = minimal_run(5, MobRunStatus::Pending);
    assert_eq!(
        run.schema_version, 5,
        "row-22 compat snapshot cut should persist schema_version 5"
    );
}

#[test]
fn decode_pre_row22_run_state_is_rejected() {
    let mut run = minimal_run(4, MobRunStatus::Running);
    let result = reconcile_run_state(&mut run);
    assert!(
        result.is_err(),
        "schema_version 4 active runs should be rejected once row-22 hard-cut lands, got {result:?}"
    );
}

#[test]
fn recovery_rejects_pre_row22_active_run() {
    let mut run = minimal_run(4, MobRunStatus::Running);
    let result = reconcile_run_state(&mut run);
    assert!(
        result.is_err(),
        "recovery should reject pre-row-22 active runs, got {result:?}"
    );
}

#[test]
fn flow_context_carries_typed_loop_iteration_payloads() {
    let mut per_iteration = IndexMap::new();
    per_iteration.insert(
        meerkat_mob::ids::StepId::from("impl"),
        serde_json::json!({"ok": true}),
    );
    let context = FlowContext {
        run_id: RunId::new(),
        activation_params: serde_json::json!({}),
        step_outputs: IndexMap::new(),
        loop_outputs: IndexMap::from([(
            meerkat_mob::ids::LoopId::from("review"),
            LoopContextHistory {
                iterations: vec![per_iteration],
            },
        )]),
    };
    assert_eq!(
        context
            .loop_outputs
            .get(&meerkat_mob::ids::LoopId::from("review"))
            .map(|history| history.iterations.len()),
        Some(1)
    );
}

#![allow(clippy::expect_used, clippy::panic)]

use std::fs;

use super::*;
use tempfile::tempdir;

#[test]
fn snake_case_handles_machine_names_and_existing_snake_case() {
    assert_eq!(machine_slug("RuntimeControlMachine"), "runtime_control");
    assert_eq!(
        machine_slug("ExternalToolSurfaceMachine"),
        "external_tool_surface"
    );
    assert_eq!(composition_slug("runtime_pipeline"), "runtime_pipeline");
    assert_eq!(to_snake_case("Mob-Orchestrator"), "mob_orchestrator");
}

#[test]
fn output_paths_land_under_specs() {
    let root = repo_root().expect("repo root");
    assert_eq!(
        machine_model_path(&root, "runtime_control"),
        root.join("specs/machines/runtime_control/model.tla")
    );
    assert_eq!(
        composition_model_path(&root, "runtime_pipeline"),
        root.join("specs/compositions/runtime_pipeline/model.tla")
    );
}

#[test]
fn richer_machine_owner_tests_are_registered_with_exact_filters() {
    let peer = owner_test_specs_for_machine("peer_comms");
    assert_eq!(peer.len(), 2);
    assert!(peer.iter().all(|spec| spec.package == "meerkat-comms"));
    assert!(
        peer.iter()
            .any(|spec| spec.filter.contains("trust_snapshot"))
    );

    let turn = owner_test_specs_for_machine("turn_execution");
    assert_eq!(turn.len(), 3);
    assert!(turn.iter().all(|spec| spec.package == "meerkat-core"));
    assert!(
        turn.iter()
            .any(|spec| spec.filter.contains("immediate_context"))
    );

    let external = owner_test_specs_for_machine("external_tool_surface");
    assert_eq!(external.len(), 2);
    assert!(external.iter().all(|spec| spec.package == "meerkat-mcp"));
    assert!(
        external
            .iter()
            .any(|spec| spec.filter.contains("forced_finalize"))
    );

    let flow_run = owner_test_specs_for_machine("flow_run");
    assert_eq!(flow_run.len(), 1);
    assert!(flow_run.iter().all(|spec| spec.package == "meerkat-mob"));
    assert!(
        flow_run
            .iter()
            .any(|spec| spec.filter.contains("pending_and_terminal_truth"))
    );

    let mob_orchestrator = owner_test_specs_for_machine("mob_orchestrator");
    assert_eq!(mob_orchestrator.len(), 1);
    assert!(
        mob_orchestrator
            .iter()
            .all(|spec| spec.package == "meerkat-mob")
    );
    assert!(
        mob_orchestrator
            .iter()
            .any(|spec| spec.filter.contains("binding_pending_spawn"))
    );

    assert!(owner_test_specs_for_machine("runtime_control").is_empty());
}

#[test]
#[ignore = "Phase 1 red-ok machine workflow E2E"]
fn machine_workflow_red_ok_detects_missing_and_stale_generated_artifacts() {
    let registry = CanonicalRegistry::load();
    let selection = registry
        .select(&SelectionArgs {
            all: false,
            machines: vec!["runtime_control".into()],
            compositions: vec!["runtime_pipeline".into()],
        })
        .expect("selection should resolve canonical workflow artifacts");
    let dir = tempdir().expect("tempdir");

    let missing = collect_drift_mismatches(dir.path(), &selection).expect("missing drift");
    assert!(
        !missing.is_empty(),
        "fresh temp roots should surface missing machine workflow artifacts"
    );

    machine_codegen_at_root(dir.path(), &selection).expect("generate workflow artifacts");

    let clean = collect_drift_mismatches(dir.path(), &selection).expect("clean drift");
    assert!(
        clean.is_empty(),
        "generated workflow artifacts should satisfy the anti-drift contract: {clean:#?}"
    );

    let machine_model = dir.path().join("specs/machines/runtime_control/model.tla");
    let composition_model = dir
        .path()
        .join("specs/compositions/runtime_pipeline/model.tla");
    assert!(machine_model.exists(), "machine model should be generated");
    assert!(
        composition_model.exists(),
        "composition model should be generated"
    );

    fs::write(&machine_model, "---- MODULE stale ----\n").expect("write stale model");
    let stale = collect_drift_mismatches(dir.path(), &selection).expect("stale drift");
    assert!(
        !stale.is_empty(),
        "editing a generated artifact should be caught by anti-drift checks"
    );
}

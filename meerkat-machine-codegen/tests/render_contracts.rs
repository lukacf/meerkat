#![allow(clippy::expect_used)]

use meerkat_machine_codegen::{
    GENERATED_COVERAGE_END, GENERATED_COVERAGE_START, merge_mapping_document,
    render_composition_mapping_coverage, render_composition_module,
    render_machine_mapping_coverage, render_machine_module,
};
use meerkat_machine_schema::catalog::{
    canonical_composition_coverage_manifests, canonical_machine_coverage_manifests,
    mob_orchestrator_machine, peer_runtime_bundle_composition, runtime_control_machine,
    runtime_pipeline_composition,
};

#[test]
fn renders_machine_fixture_with_stable_sections() {
    let rendered = render_machine_module(&mob_orchestrator_machine());

    assert!(rendered.starts_with("---- MODULE Machine_MobOrchestratorMachine ----"));
    assert!(rendered.contains(
        "STATE\n  phase : {\"Creating\", \"Running\", \"Stopped\", \"Completed\", \"Destroyed\"}"
    ));
    assert!(rendered.contains("INPUTS\n  MobOrchestratorInput = {\"InitializeOrchestrator\", \"BindCoordinator\", \"UnbindCoordinator\", \"StageSpawn\", \"CompleteSpawn\", \"StartFlow\", \"CompleteFlow\", \"StopOrchestrator\", \"ResumeOrchestrator\", \"MarkCompleted\", \"DestroyOrchestrator\"}"));
    assert!(rendered.contains("TRANSITIONS\n  InitializeOrchestrator"));
    assert!(rendered.ends_with("====\n"));
}

#[test]
fn renders_composition_fixture_with_routes_and_scheduler_rules() {
    let rendered = render_composition_module(&runtime_pipeline_composition());

    assert!(rendered.starts_with("---- MODULE Composition_runtime_pipeline ----"));
    assert!(rendered.contains(
        "staged_run_notifies_control == runtime_ingress.ReadyForRun -> runtime_control.BeginRun [Immediate]"
    ));
    assert!(
        rendered.contains("SCHEDULER_RULES\n  PreemptWhenReady(control_plane, ordinary_ingress)")
    );
    assert!(rendered.contains("execution_failure_is_handled =="));
    assert!(rendered.ends_with("====\n"));
}

#[test]
fn renders_runtime_control_with_transition_content() {
    let rendered = render_machine_module(&runtime_control_machine());

    assert!(rendered.contains("TRANSITIONS\n  Initialize"));
    assert!(rendered.contains("BeginRunFromIdle"));
    assert!(rendered.contains("AdmissionAcceptedIdleSteer"));
    assert!(rendered.contains("RetireRequestedFromIdle"));
}

#[test]
fn renders_route_aliases_and_literals_in_compositions() {
    let coverage = canonical_composition_coverage_manifests()
        .into_iter()
        .find(|item| item.composition == "peer_runtime_bundle")
        .expect("peer runtime coverage");
    let rendered = render_composition_module(&peer_runtime_bundle_composition());
    let mapping =
        render_composition_mapping_coverage(&peer_runtime_bundle_composition(), &coverage);

    assert!(rendered.contains(
        "peer_candidate_enters_runtime_admission == peer_comms.SubmitPeerInputCandidate -> runtime_control.SubmitWork [Immediate]"
    ));
    assert!(rendered.contains("raw_item_id ~> work_id"));
    assert!(rendered.contains("handling_mode := \"Steer\""));
    assert!(mapping.contains("### Code Anchors"));
    assert!(mapping.contains("peer-message-admission"));
}

#[test]
fn renders_machine_mapping_coverage_with_named_items() {
    let coverage = canonical_machine_coverage_manifests()
        .into_iter()
        .find(|item| item.machine == "RuntimeControlMachine")
        .expect("runtime control coverage");
    let rendered = render_machine_mapping_coverage(&runtime_control_machine(), &coverage);

    assert!(rendered.contains("## Generated Coverage"));
    assert!(rendered.contains("### Code Anchors"));
    assert!(rendered.contains("### Scenarios"));
    assert!(rendered.contains("### Transitions"));
    assert!(rendered.contains("- `Initialize`"));
    assert!(rendered.contains("- `ResolveAdmission`"));
    assert!(rendered.contains("- `running_implies_active_run`"));
}

#[test]
fn renders_composition_mapping_coverage_with_routes_and_scheduler_rules() {
    let coverage = canonical_composition_coverage_manifests()
        .into_iter()
        .find(|item| item.composition == "runtime_pipeline")
        .expect("runtime pipeline coverage");
    let rendered = render_composition_mapping_coverage(&runtime_pipeline_composition(), &coverage);

    assert!(rendered.contains("### Code Anchors"));
    assert!(rendered.contains("### Scenarios"));
    assert!(rendered.contains("### Routes"));
    assert!(rendered.contains("- `staged_run_notifies_control`"));
    assert!(rendered.contains("### Scheduler Rules"));
    assert!(rendered.contains("- `PreemptWhenReady(control_plane, ordinary_ingress)`"));
    assert!(rendered.contains("- `execution_failure_is_handled`"));
}

#[test]
fn merges_mapping_document_by_appending_generated_block() {
    let coverage = canonical_machine_coverage_manifests()
        .into_iter()
        .find(|item| item.machine == "RuntimeControlMachine")
        .expect("runtime control coverage");
    let merged = merge_mapping_document(
        Some("# RuntimeControlMachine Mapping Note\n\nManual text."),
        "RuntimeControlMachine",
        &render_machine_mapping_coverage(&runtime_control_machine(), &coverage),
    );

    assert!(merged.contains("Manual text."));
    assert!(merged.contains(GENERATED_COVERAGE_START));
    assert!(merged.contains("- `Initialize`"));
    assert!(merged.contains(GENERATED_COVERAGE_END));
}

#[test]
fn merges_mapping_document_by_replacing_existing_generated_block() {
    let coverage = canonical_machine_coverage_manifests()
        .into_iter()
        .find(|item| item.machine == "RuntimeControlMachine")
        .expect("runtime control coverage");
    let existing = format!(
        "# RuntimeControlMachine Mapping Note\n\nManual text.\n\n{GENERATED_COVERAGE_START}\nold block\n{GENERATED_COVERAGE_END}\n"
    );
    let merged = merge_mapping_document(
        Some(&existing),
        "RuntimeControlMachine",
        &render_machine_mapping_coverage(&runtime_control_machine(), &coverage),
    );

    assert!(!merged.contains("old block"));
    assert!(merged.contains("Manual text."));
    assert!(merged.contains("- `BeginRunFromIdle`"));
}

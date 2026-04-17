#![allow(clippy::expect_used)]

use meerkat_machine_codegen::{
    GENERATED_COVERAGE_END, GENERATED_COVERAGE_START, merge_mapping_document,
    render_composition_mapping_coverage, render_composition_module,
    render_machine_mapping_coverage, render_machine_module,
};
use meerkat_machine_schema::catalog::dsl::{
    dsl_meerkat_machine as meerkat_machine, dsl_mob_machine as mob_machine,
};
use meerkat_machine_schema::catalog::{
    canonical_composition_coverage_manifests, canonical_machine_coverage_manifests,
    meerkat_mob_seam_composition,
};

#[test]
fn renders_canonical_meerkat_machine_fixture_with_stable_sections() {
    let rendered = render_machine_module(&meerkat_machine());

    assert!(rendered.starts_with("---- MODULE Machine_MeerkatMachine ----"));
    assert!(rendered.contains(
        "STATE\n  phase : {\"Initializing\", \"Idle\", \"Attached\", \"Running\", \"Retired\", \"Stopped\", \"Destroyed\"}"
    ));
    assert!(rendered.contains("INPUTS\n  MeerkatMachineInput = {"));
    assert!(rendered.contains("SIGNALS\n  MeerkatMachineSignal = {"));
    for required in [
        "\"RegisterSession\"",
        "\"UnregisterSession\"",
        "\"PrepareBindings\"",
        "\"InterruptCurrentRun\"",
        "\"CancelAfterBoundary\"",
        "\"PublishCommittedVisibleSet\"",
        "\"SetPeerIngressContext\"",
        "\"AcceptWithCompletion\"",
        "\"AcceptWithoutWake\"",
        "\"StagePersistentFilter\"",
        "\"RequestDeferredTools\"",
        "\"AbortAll\"",
        "\"Wait\"",
        "\"Prepare\"",
        "\"Commit\"",
        "\"Fail\"",
    ] {
        assert!(
            rendered.contains(required),
            "rendered MeerkatMachine module should include input {required}"
        );
    }
    let required = "\"Initialize\"";
    assert!(
        rendered.contains(required),
        "rendered MeerkatMachine module should include signal {required}"
    );
    assert!(rendered.contains("TRANSITIONS\n  Initialize"));
    assert!(rendered.contains("PrepareBindings"));
    assert!(rendered.ends_with("====\n"));
}

#[test]
fn renders_canonical_mob_machine_fixture_with_identity_native_inputs() {
    let rendered = render_machine_module(&mob_machine());

    assert!(rendered.starts_with("---- MODULE Machine_MobMachine ----"));
    assert!(
        rendered
            .contains("STATE\n  phase : {\"Running\", \"Stopped\", \"Completed\", \"Destroyed\"}")
    );
    assert!(rendered.contains("INPUTS\n  MobMachineInput = {"));
    assert!(rendered.contains("SIGNALS\n  MobMachineSignal = {"));
    for required in [
        "\"Spawn\"",
        "\"SubmitWork\"",
        "\"RunFlow\"",
        "\"ForceCancel\"",
    ] {
        assert!(rendered.contains(required));
    }
    for required in ["\"ObserveRuntimeReady\"", "\"StartRun\"", "\"FinishRun\""] {
        assert!(rendered.contains(required));
    }
    assert!(rendered.contains("AgentIdentity"));
    assert!(!rendered.contains("MeerkatId"));
}

#[test]
fn renders_kernel_seam_composition_with_routes() {
    let rendered = render_composition_module(&meerkat_mob_seam_composition());

    assert!(rendered.starts_with("---- MODULE Composition_meerkat_mob_seam ----"));
    assert!(rendered.contains(
        "binding_request_reaches_meerkat == mob.RequestRuntimeBinding -> meerkat.PrepareBindings (Input) [Immediate]"
    ));
    assert!(rendered.contains("ROUTES"));
    assert!(rendered.ends_with("====\n"));
}

#[test]
fn renders_machine_mapping_coverage_with_named_items() {
    let coverage = canonical_machine_coverage_manifests()
        .into_iter()
        .find(|item| item.machine == "MeerkatMachine")
        .expect("meerkat coverage");
    let rendered = render_machine_mapping_coverage(&meerkat_machine(), &coverage);

    assert!(rendered.contains("## Generated Coverage"));
    assert!(rendered.contains("### Code Anchors"));
    assert!(rendered.contains("### Scenarios"));
    assert!(rendered.contains("### Transitions"));
    assert!(rendered.contains("- `PrepareBindingsInitializing`"));
    assert!(rendered.contains("- `bind-run-boundary-terminal`"));
}

#[test]
fn renders_composition_mapping_coverage_with_routes() {
    let coverage = canonical_composition_coverage_manifests()
        .into_iter()
        .find(|item| item.composition == "meerkat_mob_seam")
        .expect("kernel seam coverage");
    let rendered = render_composition_mapping_coverage(&meerkat_mob_seam_composition(), &coverage);

    assert!(rendered.contains("### Code Anchors"));
    assert!(rendered.contains("### Scenarios"));
    assert!(rendered.contains("### Routes"));
    assert!(rendered.contains("- `binding_request_reaches_meerkat`"));
    assert!(rendered.contains("- `work_request_reaches_meerkat`"));
}

#[test]
fn merges_mapping_document_by_appending_and_replacing_generated_block() {
    let coverage = canonical_machine_coverage_manifests()
        .into_iter()
        .find(|item| item.machine == "MeerkatMachine")
        .expect("meerkat coverage");
    let generated = render_machine_mapping_coverage(&meerkat_machine(), &coverage);

    let appended = merge_mapping_document(
        Some("# MeerkatMachine Mapping Note\n\nManual text."),
        "MeerkatMachine",
        &generated,
    );
    assert!(appended.contains("Manual text."));
    assert!(appended.contains(GENERATED_COVERAGE_START));
    assert!(appended.contains("- `PrepareBindingsInitializing`"));
    assert!(appended.contains(GENERATED_COVERAGE_END));

    let existing = format!(
        "# MeerkatMachine Mapping Note\n\nManual text.\n\n{GENERATED_COVERAGE_START}\nold block\n{GENERATED_COVERAGE_END}\n"
    );
    let replaced = merge_mapping_document(Some(&existing), "MeerkatMachine", &generated);
    assert!(!replaced.contains("old block"));
    assert!(replaced.contains("Manual text."));
    assert!(replaced.contains("- `PrepareBindingsInitializing`"));
}

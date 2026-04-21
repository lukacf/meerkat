#![allow(clippy::expect_used)]

use std::fs;
use std::path::PathBuf;

use meerkat_machine_codegen::{
    GENERATED_COVERAGE_END, GENERATED_COVERAGE_START, merge_mapping_document,
    render_composition_mapping_coverage, render_composition_module, render_generated_kernel_mod,
    render_machine_kernel_module, render_machine_mapping_coverage, render_machine_module,
};
use meerkat_machine_schema::catalog::dsl::{
    dsl_meerkat_machine as meerkat_machine, dsl_mob_machine as mob_machine,
};
use meerkat_machine_schema::catalog::{
    canonical_composition_coverage_manifests, canonical_machine_coverage_manifests,
    meerkat_mob_seam_composition,
};
use meerkat_machine_schema::{
    CompositionSchema, MachineSchema, canonical_composition_schemas, canonical_machine_schemas,
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

fn row22_machine_schemas() -> Vec<MachineSchema> {
    canonical_machine_schemas()
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root")
        .to_path_buf()
}

fn contains_public_item(rendered: &str, name: &str, kinds: &[&str]) -> bool {
    kinds
        .iter()
        .any(|kind| rendered.contains(&format!("pub {kind} {name}")))
}

fn assert_public_item(rendered: &str, machine: &str, name: &str, kinds: &[&str]) {
    assert!(
        contains_public_item(rendered, name, kinds),
        "{machine} should render public {name} as one of {kinds:?}, got:\n{rendered}"
    );
}

fn assert_typed_machine_surface(schema: &MachineSchema, rendered: &str) {
    let machine = schema.machine.as_str();
    assert_public_item(rendered, machine, "State", &["struct"]);
    assert_public_item(rendered, machine, "Phase", &["enum", "type"]);
    assert_public_item(rendered, machine, "Input", &["enum", "type"]);
    assert_public_item(rendered, machine, "InputKind", &["enum", "type"]);
    assert_public_item(rendered, machine, "Effect", &["enum", "type"]);
    assert_public_item(rendered, machine, "EffectKind", &["enum", "type"]);
    assert_public_item(rendered, machine, "TransitionId", &["struct", "type"]);
    assert_public_item(rendered, machine, "GuardId", &["struct", "type"]);
    assert_public_item(rendered, machine, "HelperId", &["struct", "type"]);
    assert_public_item(rendered, machine, "Outcome", &["struct", "enum", "type"]);
    assert_public_item(
        rendered,
        machine,
        "TransitionError",
        &["struct", "enum", "type"],
    );
    assert_public_item(
        rendered,
        machine,
        "TransitionRefusal",
        &["struct", "enum", "type"],
    );
    assert_public_item(
        rendered,
        machine,
        "KernelError",
        &["struct", "enum", "type"],
    );
    assert_public_item(rendered, machine, "Context", &["trait", "struct", "type"]);
    assert_public_item(rendered, machine, "EmptyContext", &["struct", "type"]);
    assert!(
        rendered.contains("pub mod helpers"),
        "{machine} should render a typed helpers module"
    );
    assert!(
        rendered.contains("pub fn initial_state("),
        "{machine} should render initial_state()"
    );
    assert!(
        rendered.contains("pub fn transition("),
        "{machine} should render transition()"
    );
    assert!(
        rendered.contains("context:"),
        "{machine} should thread typed Context through transition entrypoints"
    );

    if !schema.signals.variants.is_empty() {
        assert_public_item(rendered, machine, "Signal", &["enum", "type"]);
        assert_public_item(rendered, machine, "SignalKind", &["enum", "type"]);
        assert!(
            rendered.contains("pub fn transition_signal("),
            "{machine} should render transition_signal()"
        );
    }

    for forbidden in [
        "KernelState",
        "KernelInput",
        "KernelSignal",
        "KernelEffect",
        "KernelValue",
        "helper_name: &str",
        "evaluate_helper(",
    ] {
        assert!(
            !rendered.contains(forbidden),
            "{machine} should not expose legacy public surface `{forbidden}`"
        );
    }
}

fn assert_no_legacy_machine_surface(schema: &MachineSchema, rendered: &str) {
    for forbidden in [
        "KernelState",
        "KernelInput",
        "KernelSignal",
        "KernelEffect",
        "KernelValue",
        "helper_name: &str",
        "evaluate_helper(",
    ] {
        assert!(
            !rendered.contains(forbidden),
            "{} should not expose legacy public surface `{forbidden}`",
            schema.machine
        );
    }
}

fn composition_surface_paths(schema: &CompositionSchema) -> Vec<PathBuf> {
    let root = repo_root();
    let mut paths = Vec::new();
    if let Some(driver) = &schema.driver {
        paths.push(root.join(&driver.module_path));
    }
    paths.extend(
        schema
            .handoff_protocols
            .iter()
            .map(|protocol| root.join(&protocol.rust.module_path)),
    );
    paths
}

#[test]
fn renders_all_eight_typed_machine_modules() {
    for schema in row22_machine_schemas() {
        let rendered = render_machine_kernel_module(&schema);
        assert_typed_machine_surface(&schema, &rendered);
    }
}

#[test]
fn renders_typed_canonical_machine_module() {
    for schema in canonical_machine_schemas() {
        let rendered = render_machine_kernel_module(&schema);
        assert_typed_machine_surface(&schema, &rendered);
    }
}

#[test]
fn renders_typed_compat_machine_module() {
    let rendered = render_generated_kernel_mod(&canonical_machine_schemas());
    for slug in ["flow_frame", "flow_run", "loop_iteration"] {
        assert!(
            !rendered.contains(&format!("pub mod {slug};")),
            "public generated kernel mod should not export compat module {slug}"
        );
    }
}

#[test]
fn render_generated_kernel_mod_exports_all_row22_modules() {
    let rendered = render_generated_kernel_mod(&canonical_machine_schemas());

    for slug in [
        "auth",
        "meerkat",
        "mob",
        "occurrence_lifecycle",
        "schedule_lifecycle",
    ] {
        assert!(
            rendered.contains(&format!("pub mod {slug};")),
            "generated kernel mod should export {slug}"
        );
    }
    for compat in ["flow_frame", "flow_run", "loop_iteration"] {
        assert!(
            !rendered.contains(&format!("pub mod {compat};")),
            "generated kernel mod should not export compat module {compat}"
        );
    }
}

#[test]
fn typed_machine_modules_do_not_reference_legacy_kernel_types() {
    for schema in row22_machine_schemas() {
        let rendered = render_machine_kernel_module(&schema);
        assert_no_legacy_machine_surface(&schema, &rendered);
    }
}

#[test]
fn renders_all_four_typed_composition_modules() {
    let compositions = canonical_composition_schemas();
    assert_eq!(
        compositions.len(),
        4,
        "row-22 expects four canonical compositions"
    );

    for schema in compositions {
        let paths = composition_surface_paths(&schema);
        assert!(
            paths.is_empty(),
            "{} should not require standalone generated helper artifacts on the public canonical-5 surface",
            schema.name
        );
    }
}

#[test]
fn renders_typed_flow_frame_loop_driver() {
    let path = repo_root().join("meerkat-mob/src/generated/flow_frame_loop_driver.rs");
    let rendered = fs::read_to_string(&path).expect("flow_frame_loop_driver");

    assert!(
        rendered.contains("FlowFrameLoopDriver"),
        "legacy flow-frame loop driver should still exist until compat bridges are fully deleted"
    );
}

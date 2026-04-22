#![allow(clippy::expect_used)]

use meerkat_machine_codegen::{
    GENERATED_COVERAGE_END, GENERATED_COVERAGE_START, merge_mapping_document,
    render_composition_driver, render_composition_mapping_coverage, render_composition_module,
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
    CompositionDriverRustBinding, EnumSchema, FieldInit, FieldSchema, InitSchema, MachineSchema,
    RustBinding, StateSchema, TypeRef, VariantSchema,
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

fn dummy_driver_binding() -> CompositionDriverRustBinding {
    CompositionDriverRustBinding {
        module_path: "meerkat-mob/src/generated/dummy_driver.rs".into(),
        driver_type: "DummyDriver".into(),
        store_plan_type: "DummyStorePlan".into(),
        work_type: "DummyWork".into(),
        decision_type: "DummyDecision".into(),
        required_imports: vec![],
    }
}

#[test]
fn flow_frame_loop_driver_codegen_is_explicitly_retired() {
    let mut schema = meerkat_mob_seam_composition();
    schema.name = "flow_frame_loop".into();
    schema.driver = Some(dummy_driver_binding());

    assert!(
        render_composition_driver(&schema).is_none(),
        "legacy flow_frame_loop driver should stay retired rather than falling through to generic codegen"
    );
}

#[test]
fn non_legacy_composition_drivers_fail_closed() {
    let mut schema = meerkat_mob_seam_composition();
    schema.driver = Some(dummy_driver_binding());

    let rendered = render_composition_driver(&schema)
        .expect("non-legacy composition drivers should render a fail-closed stub");
    assert!(rendered.contains("compile_error!"));
    assert!(rendered.contains("unsupported composition driver codegen"));
    assert!(rendered.contains("meerkat_mob_seam"));
}

#[test]
fn kernel_module_renders_named_variant_accessors_with_keyword_escaping() {
    let schema = MachineSchema {
        machine: "FlowFrameMachine".into(),
        version: 1,
        rust: RustBinding {
            crate_name: "meerkat-test".into(),
            module: "keyword_enum".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "KeywordEnumPhase".into(),
                variants: vec![VariantSchema {
                    name: "Idle".into(),
                    fields: vec![],
                }],
            },
            fields: vec![FieldSchema {
                name: "kind".into(),
                ty: TypeRef::Enum("FlowNodeKind".into()),
            }],
            init: InitSchema {
                phase: "Idle".into(),
                fields: vec![FieldInit {
                    field: "kind".into(),
                    expr: meerkat_machine_schema::Expr::NamedVariant {
                        enum_name: "FlowNodeKind".into(),
                        variant: "Loop".into(),
                    },
                }],
            },
            terminal_phases: vec![],
        },
        inputs: EnumSchema {
            name: "KeywordEnumInput".into(),
            variants: vec![],
        },
        surface_only_inputs: vec![],
        signals: EnumSchema {
            name: "KeywordEnumSignal".into(),
            variants: vec![],
        },
        effects: EnumSchema {
            name: "KeywordEnumEffect".into(),
            variants: vec![],
        },
        helpers: vec![],
        derived: vec![],
        invariants: vec![],
        transitions: vec![],
        effect_dispositions: vec![],
        ci_step_limit: None,
    };

    let rendered = render_machine_kernel_module(&schema);

    assert!(rendered.contains("pub mod named_variant {"));
    assert!(rendered.contains("pub mod flow_node_kind {"));
    assert!(rendered.contains("pub fn r#loop() -> super::super::KernelNamedVariant {"));
    assert!(rendered.contains("pub mod named_value {"));
    assert!(rendered.contains("named_variant::flow_node_kind::r#loop()"));
}

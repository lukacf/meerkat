#![allow(clippy::expect_used)]

use meerkat_machine_codegen::{
    GENERATED_COVERAGE_END, GENERATED_COVERAGE_START, merge_mapping_document,
    render_composition_driver, render_composition_mapping_coverage, render_composition_module,
    render_generated_kernel_mod, render_machine_kernel_module, render_machine_mapping_coverage,
    render_machine_module,
};
use meerkat_machine_schema::catalog::dsl::{
    dsl_meerkat_machine as meerkat_machine, dsl_mob_machine as mob_machine,
};
use meerkat_machine_schema::catalog::{
    canonical_composition_coverage_manifests, canonical_machine_coverage_manifests,
    meerkat_mob_seam_composition,
};
use meerkat_machine_schema::{
    CompositionDriver, CompositionDriverRustBinding, DriverDispatchRoute, RouteTargetKind,
    WatchedEffect, canonical_machine_schemas, flow_run_machine,
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

#[test]
fn typed_kernel_module_contract_rejects_legacy_kernel_surface() {
    let rendered = render_machine_kernel_module(&meerkat_machine());

    for forbidden in [
        "KernelState",
        "KernelInput",
        "KernelSignal",
        "KernelEffect",
        "KernelValue",
        "TransitionOutcome",
        "evaluate_helper(",
        "GeneratedMachineKernel::new",
    ] {
        assert!(
            !rendered.contains(forbidden),
            "typed kernel module contract should not mention `{forbidden}`:\n{rendered}"
        );
    }

    for required in [
        "pub struct State",
        "pub enum Phase",
        "pub enum Input",
        "pub enum InputKind",
        "pub enum Signal",
        "pub enum SignalKind",
        "pub enum Effect",
        "pub enum EffectKind",
        "pub enum TransitionId",
        "pub enum GuardId",
        "pub enum HelperId",
        "pub struct Outcome",
        "pub enum TransitionError",
        "pub enum TransitionRefusal",
        "pub enum KernelError",
        "pub trait Context",
        "pub struct EmptyContext",
        "pub fn initial_state() -> State",
        "pub fn transition<C: Context>(",
        "pub fn transition_signal<C: Context>(",
        "pub mod helpers",
    ] {
        assert!(
            rendered.contains(required),
            "typed kernel module contract should contain `{required}`:\n{rendered}"
        );
    }
}

#[test]
fn typed_compat_kernel_module_contract_rejects_legacy_kernel_surface() {
    let rendered = render_machine_kernel_module(&flow_run_machine());

    for forbidden in [
        "KernelState",
        "KernelInput",
        "KernelSignal",
        "KernelEffect",
        "KernelValue",
        "TransitionOutcome",
        "evaluate_helper(",
    ] {
        assert!(
            !rendered.contains(forbidden),
            "typed compat kernel module contract should not mention `{forbidden}`:\n{rendered}"
        );
    }

    for required in [
        "pub struct State",
        "pub enum Phase",
        "pub enum Input",
        "pub enum InputKind",
        "pub enum Effect",
        "pub enum EffectKind",
        "pub enum TransitionId",
        "pub enum GuardId",
        "pub enum HelperId",
        "pub struct Outcome",
        "pub enum TransitionError",
        "pub enum TransitionRefusal",
        "pub enum KernelError",
        "pub trait Context",
        "pub struct EmptyContext",
        "pub fn transition<C: Context>(",
        "pub mod helpers",
    ] {
        assert!(
            rendered.contains(required),
            "typed compat kernel module contract should contain `{required}`:\n{rendered}"
        );
    }
}

#[test]
fn generated_kernel_inventory_contract_lists_all_typed_machine_modules() {
    let schemas = canonical_machine_schemas();
    let rendered = render_generated_kernel_mod(&schemas);

    for slug in [
        "meerkat",
        "mob",
        "schedule_lifecycle",
        "occurrence_lifecycle",
        "auth",
    ] {
        assert!(
            rendered.contains(&format!("pub mod {slug};")),
            "expected generated inventory to include `{slug}`:\n{rendered}"
        );
    }

    for hidden_slug in ["flow_run", "flow_frame", "loop_iteration"] {
        assert!(
            !rendered.contains(&format!("pub mod {hidden_slug};")),
            "canonical generated kernel inventory should not export hidden compat module `{hidden_slug}`:\n{rendered}"
        );
    }

    assert!(
        !rendered.contains("GeneratedMachineKernel"),
        "typed generated inventory should not expose the legacy GeneratedMachineKernel wrapper:\n{rendered}"
    );
}

// --------------------------------------------------------------------------
// Composition driver codegen (Track-B, R5).
//
// These tests pin the contract that `render_composition_driver` returns
// `None` for driverless compositions and emits a declarative descriptor
// module for compositions that declare a `CompositionDriver`. This
// replaces the hand-crafted `flow_frame_loop` template specialization
// with a generic framework emission.
// --------------------------------------------------------------------------

fn sample_driver() -> CompositionDriver {
    CompositionDriver {
        name: "noop_driver".into(),
        rust: CompositionDriverRustBinding {
            module_path: "meerkat-runtime/src/generated/noop_driver.rs".into(),
            driver_type: "NoopDriver".into(),
            store_plan_type: "NoopStorePlan".into(),
            work_type: "NoopWork".into(),
            decision_type: "NoopDecision".into(),
            required_imports: vec!["use meerkat_runtime::composition_dispatch::*;".into()],
        },
        watched_effects: vec![WatchedEffect {
            producer_instance: "mob".into(),
            effect_variant: "RequestRuntimeBinding".into(),
        }],
        dispatch_routes: vec![DriverDispatchRoute {
            name: "noop_dispatch".into(),
            target_instance: "meerkat".into(),
            target_kind: RouteTargetKind::Input,
            input_variant: "PrepareBindings".into(),
        }],
    }
}

#[test]
fn render_composition_driver_returns_none_for_driverless_composition() {
    let composition = meerkat_mob_seam_composition();
    assert!(composition.driver.is_none());
    assert!(
        render_composition_driver(&composition).is_none(),
        "driverless composition must not produce driver module output"
    );
}

#[test]
fn render_composition_driver_emits_descriptor_constants_for_declared_driver() {
    let mut composition = meerkat_mob_seam_composition();
    composition.driver = Some(sample_driver());

    let rendered =
        render_composition_driver(&composition).expect("driver-bearing composition emits");

    assert!(
        rendered.contains("pub const DRIVER_NAME: &str = \"noop_driver\";"),
        "rendered driver module must declare DRIVER_NAME:\n{rendered}"
    );
    assert!(
        rendered.contains("pub const WATCHED_EFFECTS:"),
        "rendered driver module must declare WATCHED_EFFECTS:\n{rendered}"
    );
    assert!(
        rendered.contains("(\"mob\", \"RequestRuntimeBinding\")"),
        "WATCHED_EFFECTS must list declared (producer, variant) pairs:\n{rendered}"
    );
    assert!(
        rendered.contains("pub const DISPATCH_ROUTES:"),
        "rendered driver module must declare DISPATCH_ROUTES:\n{rendered}"
    );
    assert!(
        rendered.contains("(\"noop_dispatch\", \"meerkat\", \"Input\", \"PrepareBindings\")"),
        "DISPATCH_ROUTES must list declared (name, target, kind, variant) tuples:\n{rendered}"
    );
    assert!(
        rendered.contains("pub const DRIVER_TYPE: &str = \"NoopDriver\";"),
        "rendered driver module must declare DRIVER_TYPE:\n{rendered}"
    );
    assert!(
        rendered.contains("use meerkat_runtime::composition_dispatch::*;"),
        "rendered driver module must include driver-declared imports:\n{rendered}"
    );
    assert!(
        rendered.contains("@generated"),
        "rendered driver module must carry the @generated marker:\n{rendered}"
    );
}

#[test]
fn render_composition_driver_emission_is_composition_name_agnostic() {
    // The framework must work for any composition with a driver, not just
    // one hardcoded name — this pins the replacement of the old
    // `if schema.name != "flow_frame_loop"` specialization.
    let mut composition = meerkat_mob_seam_composition();
    composition.name = "arbitrary_composition".into();
    composition.driver = Some(sample_driver());

    let rendered =
        render_composition_driver(&composition).expect("driver-bearing composition emits");
    assert!(
        rendered.contains("composition driver descriptor for `arbitrary_composition`"),
        "codegen must use the composition name in the header, not a hardcoded name:\n{rendered}"
    );
}

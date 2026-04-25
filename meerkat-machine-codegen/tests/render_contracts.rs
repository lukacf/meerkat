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
    meerkat_mob_seam_composition, schedule_runtime_bundle_composition,
};
use meerkat_machine_schema::{
    CompositionDriver, CompositionDriverRustBinding, DriverDispatchRoute, RouteTargetKind,
    WatchedEffect, canonical_machine_schemas,
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
        .find(|item| item.machine.as_str() == "MeerkatMachine")
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
        .find(|item| item.composition.as_str() == "meerkat_mob_seam")
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
        .find(|item| item.machine.as_str() == "MeerkatMachine")
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
// Composition module codegen (Track-B, wave-b V2).
//
// These tests pin the typed per-composition module emission: a seam-effect
// enum wrapping each participant machine's `Effect` type and a
// `route_to_input` function resolving producer effects to typed consumer
// inputs via the composition's route table. Stringly tables
// (`(&str, &str, &str, &str)`) are dogma violations and no longer emitted.
// --------------------------------------------------------------------------

use meerkat_machine_schema::RouteVariantId;
use meerkat_machine_schema::identity::{
    CompositionId, EffectVariantId, InputVariantId, MachineInstanceId, RouteId,
};

fn sample_driver() -> CompositionDriver {
    CompositionDriver {
        name: "noop_driver".into(),
        rust: CompositionDriverRustBinding {
            module_path: "meerkat-runtime/src/generated/meerkat_mob_seam.rs".into(),
            driver_type: "NoopDriver".into(),
            store_plan_type: "NoopStorePlan".into(),
            work_type: "NoopWork".into(),
            decision_type: "NoopDecision".into(),
            required_imports: vec!["use meerkat_runtime::composition_dispatch::*;".into()],
        },
        watched_effects: vec![WatchedEffect {
            producer_instance: MachineInstanceId::parse("mob").expect("instance slug"),
            effect_variant: EffectVariantId::parse("RequestRuntimeBinding").expect("effect slug"),
        }],
        dispatch_routes: vec![DriverDispatchRoute {
            name: RouteId::parse("noop_dispatch").expect("route slug"),
            target_instance: MachineInstanceId::parse("meerkat").expect("instance slug"),
            target_kind: RouteTargetKind::Input,
            input_variant: RouteVariantId::Input(
                InputVariantId::parse("PrepareBindings").expect("input slug"),
            ),
        }],
    }
}

#[test]
fn render_composition_driver_returns_none_for_driverless_composition() {
    // The emitter is driver-gated: xtask's composition-driver output path
    // derives from `CompositionDriver.rust.module_path`, so without a
    // driver descriptor there is nowhere to write the module.
    let composition = schedule_runtime_bundle_composition();
    assert!(composition.driver.is_none());
    assert!(
        render_composition_driver(&composition).is_none(),
        "driverless composition must not produce module output"
    );
}

#[test]
fn render_composition_driver_emits_typed_seam_effect_and_route_to_input() {
    let mut composition = meerkat_mob_seam_composition();
    composition.driver = Some(sample_driver());

    let rendered =
        render_composition_driver(&composition).expect("driver-bearing composition emits");

    // Typed identity imports + route descriptors are present; no stringly
    // tuple tables survive.
    assert!(
        rendered.contains(
            "use meerkat_machine_schema::identity::{FieldId, InputVariantId, MachineInstanceId, SignalVariantId};"
        ),
        "rendered module must import typed identity newtypes:\n{rendered}"
    );
    assert!(
        rendered.contains("pub struct TypedRoutedInput"),
        "rendered module must declare TypedRoutedInput:\n{rendered}"
    );
    assert!(
        rendered.contains("pub struct TypedRoutedSignal"),
        "rendered module must declare TypedRoutedSignal:\n{rendered}"
    );
    for forbidden in [
        "pub const WATCHED_EFFECTS",
        "pub const DISPATCH_ROUTES",
        "(&str, &str, &str, &str)",
        "(&str, &str)",
        "pub const DRIVER_TYPE",
    ] {
        assert!(
            !rendered.contains(forbidden),
            "stringly/legacy shape `{forbidden}` must not survive:\n{rendered}"
        );
    }

    // Seam effect enum with one variant per producer instance, wrapping
    // each participant's generated `Effect` type.
    assert!(
        rendered.contains("pub enum MeerkatMobSeamEffect"),
        "rendered module must declare the typed seam-effect enum:\n{rendered}"
    );
    assert!(
        rendered.contains("Mob(crate::generated::mob::Effect)"),
        "seam-effect enum must wrap the mob producer Effect type:\n{rendered}"
    );
    assert!(
        rendered.contains("Meerkat(crate::generated::meerkat::Effect)"),
        "seam-effect enum must wrap the meerkat producer Effect type:\n{rendered}"
    );

    // route_to_input signature and a sample Input-route arm.
    assert!(
        rendered.contains(
            "pub fn route_to_input(effect: &MeerkatMobSeamEffect) -> Option<TypedRoutedInput>"
        ),
        "rendered module must declare route_to_input:\n{rendered}"
    );
    assert!(
        rendered.contains("crate::generated::mob::Effect::RequestRuntimeBinding(_)"),
        "route_to_input must match on the mob RequestRuntimeBinding effect variant:\n{rendered}"
    );
    assert!(
        rendered.contains("InputVariantId::parse(\"PrepareBindings\")"),
        "route_to_input must target the PrepareBindings input variant:\n{rendered}"
    );

    // Signal-kind routes are emitted through the generated signal surface.
    assert!(
        rendered.contains(
            "pub fn route_to_signal(effect: &MeerkatMobSeamEffect) -> Option<TypedRoutedSignal>"
        ),
        "rendered module must declare route_to_signal:\n{rendered}"
    );
    assert!(
        rendered.contains("SignalVariantId::parse(\"ObserveRuntimeReady\")"),
        "route_to_signal must target the ObserveRuntimeReady signal variant:\n{rendered}"
    );

    assert!(
        rendered.contains("@generated"),
        "rendered module must carry the @generated marker:\n{rendered}"
    );
}

#[test]
fn render_composition_driver_emission_is_composition_name_agnostic() {
    // The framework must work for any composition — the seam-effect enum
    // name derives from the composition slug and the per-producer match
    // arms stay intact.
    let mut composition = meerkat_mob_seam_composition();
    composition.name = CompositionId::parse("arbitrary_composition").expect("composition slug");
    composition.driver = Some(sample_driver());

    let rendered =
        render_composition_driver(&composition).expect("driver-bearing composition emits");
    assert!(
        rendered.contains("composition module for `arbitrary_composition`"),
        "codegen must use the composition name in the header:\n{rendered}"
    );
    assert!(
        rendered.contains("pub enum ArbitraryCompositionEffect"),
        "seam-effect enum name must derive from the composition slug:\n{rendered}"
    );
    assert!(
        rendered.contains(
            "pub fn route_to_input(effect: &ArbitraryCompositionEffect) -> Option<TypedRoutedInput>"
        ),
        "route_to_input must reference the renamed seam-effect enum:\n{rendered}"
    );
}

#![allow(clippy::expect_used)]

use meerkat_core::turn_execution_authority::ContentShape;
use meerkat_machine_codegen::{render_machine_ci_cfg, render_machine_semantic_model};
use meerkat_machine_schema::catalog::dsl::{
    dsl_meerkat_machine as meerkat_machine, dsl_mob_machine as mob_machine,
};
use meerkat_machine_schema::{NamedTypeBinding, RustTypeAtom};

fn tool_filter_override_line(rendered: &str) -> &str {
    rendered
        .lines()
        .find(|line| line.trim_start().starts_with("ToolFilterValues <- "))
        .map(str::trim)
        .expect("ToolFilterValues override")
}

fn tool_filter_operator_line<'a>(rendered: &'a str, operator: &str) -> &'a str {
    let prefix = format!("{operator} == ");
    rendered
        .lines()
        .find(|line| line.starts_with(&prefix))
        .expect("ToolFilterValues operator")
}

fn tool_filter_domain(operator: &str, name_sets: &[&str]) -> String {
    let structural_samples = ["Allow", "Deny"]
        .into_iter()
        .flat_map(|variant| {
            name_sets
                .iter()
                .map(move |names| format!("[tag |-> \"{variant}\", names |-> {names}]"))
        })
        .collect::<Vec<_>>();
    format!(
        "{operator} == {{[tag |-> \"All\"], {}}}",
        structural_samples.join(", ")
    )
}

fn tool_filter_unit_domain(operator: &str) -> String {
    format!("{operator} == {{[tag |-> \"All\"]}}")
}

#[test]
fn meerkat_semantic_model_keeps_internal_session_transport_domain() {
    let rendered = render_machine_semantic_model(&meerkat_machine());

    assert!(rendered.contains("SessionIdValues"));
    assert!(rendered.contains(
        "PrepareBindingsInitializing(agent_runtime_id, fence_token, generation, arg_session_id) =="
    ));
    assert!(rendered.contains("MapIncrement(map, key, amount) == [x \\in DOMAIN map \\cup {key}"));
    assert!(
        !rendered
            .contains("IF \"value\" \\in DOMAIN (IF surface_id \\in DOMAIN surface_base_state"),
        "enum map value projections should not render DOMAIN checks against scalar enum values"
    );
    assert!(
        !rendered.contains(
            "IF \"value\" \\in DOMAIN (IF surface_id \\in DOMAIN surface_staged_intent_sequence"
        ),
        "integer map value projections should not render DOMAIN checks against scalar values"
    );
    assert!(!rendered.contains("MeerkatIdValues"));
}

#[test]
fn meerkat_ci_cfg_uses_closed_string_enum_binding_domains() {
    let rendered = render_machine_ci_cfg(&meerkat_machine(), false);

    let content_shape_values = ContentShape::ALL
        .into_iter()
        .map(|shape| format!("\"{}\"", shape.as_str()))
        .collect::<Vec<_>>()
        .join(", ");
    assert!(
        rendered.contains(&format!("ContentShapeValues = {{{content_shape_values}}}")),
        "ContentShapeValues must come from the closed core content-shape contract:\n{rendered}"
    );
    assert!(
        !rendered.contains("ContentShapeValues = {\"contentshape_1\""),
        "ContentShapeValues must not fall back to open placeholder strings:\n{rendered}"
    );
    assert!(
        rendered.contains("OperationKindValues = {\"MobMemberChild\", \"BackgroundToolOp\"}"),
        "OperationKindValues must come from the closed StringEnum binding:\n{rendered}"
    );
    assert!(
        rendered.contains(
            "OperationStatusValues = {\"Absent\", \"Provisioning\", \"Running\", \"Retiring\", \"Completed\", \"Failed\", \"Aborted\", \"Cancelled\", \"Retired\", \"Terminated\"}"
        ),
        "OperationStatusValues must come from the closed StringEnum binding:\n{rendered}"
    );
    assert_eq!(
        tool_filter_override_line(&rendered),
        "ToolFilterValues <- ToolFilterValuesCi"
    );
    let model = render_machine_semantic_model(&meerkat_machine());
    assert_eq!(
        tool_filter_operator_line(&model, "ToolFilterValuesCi"),
        tool_filter_unit_domain("ToolFilterValuesCi"),
        "CI ToolFilterValues must stay on the canonical unit variant; deep covers structural payloads"
    );
    assert!(
        !rendered.contains("toolfilter_2"),
        "ToolFilterValues must not fall back to generated placeholder strings:\n{rendered}"
    );
}

#[test]
fn meerkat_deep_cfg_uses_closed_tool_filter_domain() {
    let rendered = render_machine_ci_cfg(&meerkat_machine(), true);

    assert_eq!(
        tool_filter_override_line(&rendered),
        "ToolFilterValues <- ToolFilterValuesDeep"
    );
    let model = render_machine_semantic_model(&meerkat_machine());
    assert_eq!(
        tool_filter_operator_line(&model, "ToolFilterValuesDeep"),
        tool_filter_domain(
            "ToolFilterValuesDeep",
            &[r"{}", r#"{"alpha"}"#, r#"{"alpha", "beta"}"#]
        ),
        "deep ToolFilterValues must cover differing same-variant structural payloads"
    );
    assert!(
        !rendered.contains("toolfilter_2"),
        "deep ToolFilterValues must not fall back to generated placeholder strings:\n{rendered}"
    );
}

#[test]
fn meerkat_cfg_uses_model_operators_for_record_valued_domains() {
    let rendered = render_machine_ci_cfg(&meerkat_machine(), false);
    let model = render_machine_semantic_model(&meerkat_machine());

    assert!(
        rendered.contains("PeerEndpointValues <- PeerEndpointValuesCi"),
        "record-valued PeerEndpointValues must be supplied through a model operator:\n{rendered}"
    );
    assert!(
        rendered.contains("SetOfPeerEndpointValues <- SetOfPeerEndpointValuesCi"),
        "record-valued SetOfPeerEndpointValues must be supplied through a model operator:\n{rendered}"
    );
    assert!(
        !rendered.contains("PeerEndpointValues = {["),
        "cfg CONSTANTS must not contain TLC record literals:\n{rendered}"
    );
    assert!(
        model.contains("PeerEndpointValuesCi == {}"),
        "CI PeerEndpointValues must stay bounded while still using model operator substitution:\n{model}"
    );
    assert!(
        model.contains("SetOfPeerEndpointValuesCi == {{}}"),
        "CI SetOfPeerEndpointValues must stay bounded while still using model operator substitution:\n{model}"
    );
    assert!(
        model.contains(
            "PeerEndpointValuesDeep == {[name |-> \"peername_1\", peer_id |-> \"peerid_1\", address |-> \"peeraddress_1\", signing_key |-> \"peersigningkey_1\"]"
        ),
        "deep model operator must preserve typed PeerEndpoint record shape:\n{model}"
    );
    assert!(
        model.contains("SetOfPeerEndpointValuesDeep == {{}, {[name |-> \"peername_1\""),
        "deep model operator must preserve bounded set samples for PeerEndpoint records:\n{model}"
    );
}

#[test]
#[should_panic(expected = "missing NamedTypeBinding for generated domain `FlowNodeKind`")]
fn mob_ci_cfg_fails_closed_when_closed_enum_binding_is_missing() {
    let mut schema = mob_machine();
    schema
        .named_types
        .retain(|binding| binding.name.as_str() != "FlowNodeKind");

    let _ = render_machine_ci_cfg(&schema, false);
}

#[test]
#[should_panic(
    expected = "generated machine `MobMachine` named-type `FlowNodeKind` binding must match canonical domain shape"
)]
fn mob_ci_cfg_fails_closed_when_closed_enum_binding_changes_shape() {
    let mut schema = mob_machine();
    let binding = schema
        .named_types
        .iter_mut()
        .find(|binding| binding.name.as_str() == "FlowNodeKind")
        .expect("FlowNodeKind binding");
    binding.rust = RustTypeAtom::String;

    let _ = render_machine_ci_cfg(&schema, false);
}

#[test]
#[should_panic(
    expected = "missing NamedTypeBinding for generated domain `ExternalToolSurfaceFailureCause`"
)]
fn meerkat_ci_cfg_fails_closed_when_external_tool_failure_binding_is_missing() {
    let mut schema = meerkat_machine();
    schema
        .named_types
        .retain(|binding| binding.name.as_str() != "ExternalToolSurfaceFailureCause");

    let _ = render_machine_ci_cfg(&schema, false);
}

#[test]
#[should_panic(expected = "missing NamedTypeBinding for generated domain `ToolVisibilityWitness`")]
fn meerkat_ci_cfg_fails_closed_when_structural_named_binding_is_missing() {
    let mut schema = meerkat_machine();
    schema
        .named_types
        .retain(|binding| binding.name.as_str() != "ToolVisibilityWitness");

    let _ = render_machine_ci_cfg(&schema, false);
}

#[test]
#[should_panic(
    expected = "generated machine `MeerkatMachine` named-type `ToolVisibilityWitness` binding must match canonical domain shape"
)]
fn meerkat_ci_cfg_fails_closed_when_structural_named_binding_changes_shape() {
    let mut schema = meerkat_machine();
    let binding = schema
        .named_types
        .iter_mut()
        .find(|binding| binding.name.as_str() == "ToolVisibilityWitness")
        .expect("ToolVisibilityWitness binding");
    *binding = NamedTypeBinding {
        name: binding.name.clone(),
        rust: RustTypeAtom::String,
    };

    let _ = render_machine_ci_cfg(&schema, false);
}

#[test]
#[should_panic(
    expected = "generated machine `MeerkatMachine` missing canonical named-type `ToolProvenance` binding"
)]
fn meerkat_ci_cfg_fails_closed_when_tool_provenance_binding_is_missing() {
    let mut schema = meerkat_machine();
    schema
        .named_types
        .retain(|binding| binding.name.as_str() != "ToolProvenance");

    let _ = render_machine_ci_cfg(&schema, false);
}

#[test]
#[should_panic(
    expected = "generated machine `MeerkatMachine` named-type `ToolProvenance` binding must match canonical domain shape"
)]
fn meerkat_ci_cfg_fails_closed_when_tool_provenance_binding_changes_shape() {
    let mut schema = meerkat_machine();
    let binding = schema
        .named_types
        .iter_mut()
        .find(|binding| binding.name.as_str() == "ToolProvenance")
        .expect("ToolProvenance binding");
    *binding = NamedTypeBinding {
        name: binding.name.clone(),
        rust: RustTypeAtom::String,
    };

    let _ = render_machine_ci_cfg(&schema, false);
}

#[test]
fn meerkat_semantic_model_renders_content_shape_wire_labels() {
    let rendered = render_machine_semantic_model(&meerkat_machine());

    assert!(
        rendered.contains("admitted_content_shape' = Some(\"immediate_append\")"),
        "derived immediate content-shape assignments must use stable wire labels:\n{rendered}"
    );
    assert!(
        rendered.contains("arg_admitted_content_shape = \"conversation\""),
        "content-shape guards must compare against stable wire labels:\n{rendered}"
    );
    assert!(
        !rendered.contains("admitted_content_shape' = Some(\"ImmediateAppend\")"),
        "semantic model must not mix schema variant names with wire-label domains:\n{rendered}"
    );
}

#[test]
fn mob_semantic_model_is_identity_and_runtime_native() {
    let rendered = render_machine_semantic_model(&mob_machine());

    assert!(rendered.contains("AgentIdentityValues"));
    assert!(rendered.contains("AgentRuntimeIdValues"));
    assert!(rendered.contains("FenceTokenValues"));
    // W3-H-1: SessionIdValues is now present in the MobMachine semantic
    // model because `member_session_bindings: Map<AgentIdentity, SessionId>`
    // makes the bridge session id a first-class MobMachine-owned value.
    // This is the intentional expansion that issue #264 calls for — the
    // binding map is the canonical join between identity continuity
    // (MobMachine) and realtime attachment (MeerkatMachine).
    assert!(rendered.contains("SessionIdValues"));
    assert!(!rendered.contains("MeerkatIdValues"));
}

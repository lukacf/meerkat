#![allow(clippy::expect_used)]

use meerkat_machine_codegen::{
    GENERATED_COVERAGE_END, GENERATED_COVERAGE_START, merge_mapping_document,
    render_composition_ci_cfg, render_composition_driver, render_composition_mapping_coverage,
    render_composition_module, render_composition_semantic_model, render_composition_witness_cfg,
    render_generated_kernel_mod, render_machine_ci_cfg, render_machine_contract_markdown,
    render_machine_kernel_module, render_machine_mapping_coverage, render_machine_module,
    render_machine_semantic_model,
};
use meerkat_machine_schema::catalog::dsl::{
    dsl_auth_machine as auth_machine, dsl_meerkat_machine as meerkat_machine,
    dsl_mob_machine as mob_machine, dsl_occurrence_lifecycle_machine as occurrence_lifecycle,
    dsl_schedule_lifecycle_machine as schedule_lifecycle,
    dsl_workgraph_lifecycle_machine as workgraph_lifecycle,
};
use meerkat_machine_schema::catalog::{
    adaptive_mob_bundle_composition, canonical_composition_coverage_manifests,
    canonical_machine_coverage_manifests, meerkat_mob_seam_composition,
    schedule_bundle_composition, schedule_runtime_bundle_composition,
};
use meerkat_machine_schema::identity::{EnumVariantId, RustTypeAtom};
use meerkat_machine_schema::{
    CompositionDriver, CompositionDriverRustBinding, DriverDispatchRoute, Expr, RouteTargetKind,
    TriggerMatch, Update, WatchedEffect, canonical_machine_schemas,
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
fn tla_renderer_abstracts_u64_max_literals_for_tlc() {
    let model =
        render_machine_semantic_model(&meerkat_machine()).expect("render machine semantic model");
    let cfg = render_machine_ci_cfg(&meerkat_machine(), false);

    assert!(
        !model.contains("18446744073709551615"),
        "TLC rejects u64::MAX-sized literals in the model body"
    );
    assert!(
        !model.contains("2147483647"),
        "the model must use the explicit RustU64Max boundary, not a hidden literal rewrite"
    );
    assert!(model.contains("CONSTANTS"));
    assert!(model.contains("RustU64Max"));
    assert!(model.contains("RustU64Max - now_ms"));
    assert!(
        cfg.contains("RustU64Max = 2147483647"),
        "TLC-safe finite value belongs in cfg, not in expression rendering"
    );
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
        // `Spawn` was decomposed into the machine-driven spawn-exec ladder.
        "\"BeginSpawnExec\"",
        "\"CommitSpawnMembership\"",
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
fn renders_mob_spawn_command_plan_in_contract_markdown() {
    let schema = mob_machine();
    let coverage = canonical_machine_coverage_manifests()
        .into_iter()
        .find(|coverage| coverage.machine == schema.machine)
        .expect("mob machine coverage manifest");
    let rendered = render_machine_contract_markdown(&schema, &coverage);

    for required in [
        "### `AuthorizedMobSpawnStart`",
        "- Authority: `PendingSpawnOperationOwnerAuthorized`",
        "- Source Inputs: `CancelPendingSpawn`",
        "- Source Signals: `StageSpawn`, `CompleteSpawn`",
        "- Command Effects: `PendingSpawnOperationOwnerAuthorized`, `ExposePendingSpawn`, `EmitMemberLifecycleNotice`",
        "`PendingSpawnOperationOwnerAuthorized` via `PendingSpawnOperationOwnerAuthorized` (LocalPendingSpawnOwner) states: `Authorized`, `Attempted`, `Realized`, `Failed`, `Cancelled`, `Abandoned`",
        "`EmitMemberLifecycleNotice` via `CompleteSpawn` (LocalSpawnCompletion) states: `Authorized`, `Attempted`, `Realized`, `Failed`, `Cancelled`, `Abandoned`",
        "### `CanStartSpawn`",
        "- Authority: `CanStartSpawn`",
        "### `SpawnStarted`",
        "- Authority: `SpawnStarted`",
        "### `SpawnEffect`",
        "- Authority: `SpawnEffect`",
        "### `FailSpawn`",
        "- Authority: `FailSpawn`",
    ] {
        assert!(
            rendered.contains(required),
            "MobMachine contract markdown must expose `{required}`:\n{rendered}"
        );
    }
}

#[test]
fn mob_native_tla_helpers_escape_state_field_parameter_names() {
    let rendered =
        render_machine_semantic_model(&mob_machine()).expect("render machine semantic model");

    assert!(rendered.contains(
        "mob_machine_identity_has_session_binding(arg_member_session_bindings, agent_identity) =="
    ));
    assert!(rendered.contains("agent_identity \\in DOMAIN arg_member_session_bindings"));
    assert!(rendered.contains(
        "mob_machine_session_bound_live_runtime_ids_match(arg_identity_to_runtime, arg_member_session_bindings, arg_live_runtime_ids, expected_runtime_ids) =="
    ));
    assert!(
        rendered
            .contains("LET eligible_ids == { candidate \\in DOMAIN arg_member_session_bindings :")
    );
    assert!(rendered.contains(
        "IN expected_runtime_ids = { arg_identity_to_runtime[id] : id \\in eligible_ids }"
    ));
    assert!(rendered.contains(
        "mob_machine_member_peer_overlay(arg_wiring_edges, arg_member_peer_endpoints, arg_external_peer_edges, agent_identity) =="
    ));
    assert!(rendered.contains("LET outgoing_edges == { edge \\in arg_wiring_edges :"));
    assert!(!rendered.contains(
        "mob_machine_identity_has_session_binding(member_session_bindings, agent_identity) =="
    ));
    assert!(!rendered.contains(
        "mob_machine_member_peer_overlay(wiring_edges, member_peer_endpoints, external_peer_edges, agent_identity) =="
    ));
    assert!(!rendered.contains(": edge \\in arg_wiring_edges /\\"));
}

#[test]
fn seam_tla_renders_mob_external_peer_native_projection_helpers() {
    let rendered = render_composition_semantic_model(&meerkat_mob_seam_composition())
        .expect("render composition semantic model");
    let cfg = render_composition_ci_cfg(&meerkat_mob_seam_composition(), false);

    assert!(
        rendered
            .contains("mob__mob_machine_external_peer_edge_peer_id(edge) == edge.endpoint.peer_id")
    );
    assert!(rendered.contains(
        "mob__entry_packet__mob_machine_external_peer_edge_peer_id(edge) == edge.endpoint.peer_id"
    ));
    assert!(
        rendered.contains("RustU64Max") && cfg.contains("RustU64Max = 2147483647"),
        "composition model and cfg should carry the explicit u64::MAX TLC boundary"
    );
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
fn renders_kernel_seam_composition_with_namespaced_mob_native_helpers() {
    let rendered = render_composition_semantic_model(&meerkat_mob_seam_composition())
        .expect("render composition semantic model");

    for helper in [
        "mob__mob_machine_external_peer_edge_has_matching_key(edges_by_key, edge) ==",
        "mob__mob_machine_external_peer_key_matches_edge(key, edge) ==",
        "mob__entry_packet__mob_machine_external_peer_key_matches_edge(key, edge) ==",
        "mob__mob_machine_external_peer_identity_absent(edges, agent_identity) ==",
        "mob__mob_machine_identity_has_session_binding(arg_member_session_bindings, agent_identity) ==",
        "mob__mob_machine_remote_turn_custody_admits(pending, committed, resolved, candidate) ==",
        "mob__entry_packet__mob_machine_remote_turn_custody_admits(pending, committed, resolved, candidate) ==",
        "mob__mob_machine_member_peer_endpoint_peer_id(endpoint) ==",
        "mob__mob_machine_placed_cleanup_absent_for_identity(arg_obligations, arg_agent_identity) ==",
        "mob__mob_machine_placed_cleanup_obligation(arg_agent_identity, arg_spawn_id, arg_generation, arg_fence_token, arg_provision_operation_id, arg_operation_owner_session_id, arg_expected_phase) ==",
        "mob__entry_packet__mob_machine_placed_cleanup_absent_for_identity(arg_obligations, arg_agent_identity) ==",
        "mob__entry_packet__mob_machine_placed_cleanup_obligation(arg_agent_identity, arg_spawn_id, arg_generation, arg_fence_token, arg_provision_operation_id, arg_operation_owner_session_id, arg_expected_phase) ==",
        "mob__mob_machine_run_step_status_after_set(",
        "mob__mob_machine_run_step_bool_after_set(",
        "mob__mob_machine_run_step_condition_result_after_set(",
        "mob__mob_machine_run_step_u64_after_set(",
        "mob__mob_machine_run_step_u64_after_increment(",
        "mob__mob_machine_run_retry_count_after_increment(",
        "mob__mob_machine_frame_node_bool_after_set(",
        "mob__mob_machine_node_terminal(status) ==",
        "mob__mob_machine_step_status_from_frame_node_status(status) ==",
        "mob__mob_machine_frame_node_status_after_admit(",
        "mob__mob_machine_frame_ready_queue_after_admit(",
        "mob__mob_machine_frame_node_status_after_terminal_branch(",
        "mob__mob_machine_frame_node_status_after_terminal_dependencies(",
        "mob__mob_machine_frame_node_status_after_terminal(",
        "mob__mob_machine_frame_ready_queue_after_terminal(",
    ] {
        assert!(
            rendered.contains(helper),
            "composition model must define namespaced MobMachine native helper `{helper}`"
        );
    }

    assert!(
        rendered.contains(
            "Cardinality(pending) + Cardinality(committed) + Cardinality(resolved) < 4096"
        ) && rendered.contains("same_member_host_count < 256")
            && rendered.contains("row.dispatch_sequence # candidate.dispatch_sequence")
            && rendered.contains(
                "\\lnot (/\\ row.agent_identity = candidate.agent_identity /\\ row.host_id = candidate.host_id /\\ row.generation = candidate.generation /\\ row.fence_token = candidate.fence_token /\\ row.input_id = candidate.input_id)"
            ),
        "composition model must preserve the Rust remote-turn custody admission limits and uniqueness check"
    );
    assert!(
        rendered.contains("provision_operation_id |-> arg_provision_operation_id")
            && rendered.contains("operation_owner_session_id |-> arg_operation_owner_session_id"),
        "placed-cleanup TLA records must retain the full durable operation tuple"
    );
}

#[test]
fn adaptive_bundle_scopes_obligations_by_producer_instance() {
    let rendered = render_composition_semantic_model(&adaptive_mob_bundle_composition())
        .expect("render adaptive composition semantic model");

    for symbol in [
        "obligation_control_mob_mob_destroying_session_ingress",
        "obligation_layer_mob_mob_destroying_session_ingress",
        "NoOpenObligationsOnTerminal_control_mob_mob_destroying_session_ingress ==",
        "NoOpenObligationsOnTerminal_layer_mob_mob_destroying_session_ingress ==",
        "OwnerFeedback_control_mob_mob_destroying_session_ingress ==",
        "OwnerFeedback_layer_mob_mob_destroying_session_ingress ==",
    ] {
        assert!(
            rendered.contains(symbol),
            "adaptive composition model must scope obligation symbol `{symbol}` by producer instance"
        );
    }

    for stale_symbol in [
        "obligation_mob_destroying_session_ingress",
        "NoOpenObligationsOnTerminal_mob_destroying_session_ingress ==",
        "OwnerFeedback_mob_destroying_session_ingress ==",
    ] {
        assert!(
            !rendered.contains(stale_symbol),
            "adaptive composition model must not render unscoped obligation symbol `{stale_symbol}`"
        );
    }
}

#[test]
fn renders_composition_witness_fairness_in_tlc_safe_chunks() {
    let rendered = render_composition_semantic_model(&meerkat_mob_seam_composition())
        .expect("render composition semantic model");

    assert!(!rendered.contains("WitnessFairness_basic_round_trip_1 =="));
    assert!(!rendered.contains("    /\\ WitnessFairness_basic_round_trip_1"));
    assert!(rendered.contains("WitnessScriptComplete_basic_round_trip =="));
    let basic_completion = rendered
        .split("WitnessScriptComplete_basic_round_trip ==")
        .nth(1)
        .and_then(|tail| {
            tail.split("WitnessNoPrematureStutter_basic_round_trip ==")
                .next()
        })
        .expect("basic completion section");
    assert!(
        basic_completion.contains("Len(witness_remaining_script_inputs) = 0")
            && basic_completion
                .contains("~(witness_current_script_input \\in SeqElements(pending_inputs))")
            && basic_completion.contains("Len(pending_routes) = 0")
            && basic_completion
                .contains("\\E packet \\in delivered_routes : packet.route = \"runtime_bound_reaches_mob\"")
            && basic_completion.contains(
                "\\E packet \\in observed_transitions : /\\ packet.machine = \"meerkat\" /\\ packet.transition = \"IngestAttached\""
            )
            && !basic_completion.contains("Len(pending_inputs) = 0"),
        "completion should prove the scripted inputs and routes are drained without requiring terminal routed inputs to be consumed:\n{basic_completion}"
    );
    assert!(rendered.contains(
        "WitnessNoPrematureStutter_basic_round_trip ==\n    \\/ WitnessScriptComplete_basic_round_trip\n    \\/ model_step_count' # model_step_count"
    ));
    assert!(rendered.contains(
        "WitnessSpec_basic_round_trip ==\n    /\\ WitnessInit_basic_round_trip\n    /\\ [] [WitnessNext_basic_round_trip]_vars"
    ));
    assert!(rendered.contains(
        "WitnessSatisfiedStutter_basic_round_trip ==\n    /\\ WitnessScriptComplete_basic_round_trip"
    ));
    assert!(rendered.contains(
        "WitnessRouteObserved_basic_round_trip_binding_request_reaches_meerkat == WitnessScriptComplete_basic_round_trip => (RouteObserved_binding_request_reaches_meerkat)"
    ));
    let seam = meerkat_mob_seam_composition();
    let basic_witness = seam
        .witnesses
        .iter()
        .find(|witness| witness.name.as_str() == "basic_round_trip")
        .expect("basic round trip witness");
    let basic_cfg = render_composition_witness_cfg(&seam, basic_witness);
    assert!(
        basic_cfg.contains("INVARIANTS")
            && basic_cfg.contains(
                "  WitnessRouteObserved_basic_round_trip_binding_request_reaches_meerkat"
            )
            && basic_cfg.contains("ACTION_CONSTRAINTS")
            && basic_cfg.contains("  WitnessNoPrematureStutter_basic_round_trip")
            && !basic_cfg.contains("PROPERTIES"),
        "explicit seam witness cfg should check observations as guarded invariants and reject pre-completion stutter via an action constraint:\n{basic_cfg}"
    );
    let seam_witness_next = rendered
        .split("WitnessNext_basic_round_trip ==")
        .nth(1)
        .and_then(|tail| tail.split("WitnessNext_retire_runtime_path ==").next())
        .expect("seam basic witness next section");
    assert!(
        !seam_witness_next.contains("CoreNext")
            && seam_witness_next.contains("meerkat_RecoverRuntimeAuthorityIdle")
            && seam_witness_next.contains("mob_AuthorizeSpawnProfileRunning")
            && seam_witness_next.contains("mob_BeginSpawnExecFresh")
            && seam_witness_next.contains("mob_CommitSpawnMembershipFresh")
            && seam_witness_next.contains("meerkat_PrepareBindingsIdle")
            && seam_witness_next.contains("mob_SubmitWorkRunningExternal")
            && seam_witness_next.contains("meerkat_IngestAttached")
            && seam_witness_next.contains("WitnessSatisfiedStutter_basic_round_trip"),
        "meerkat_mob_seam witness must be scripted through concrete seam transitions, not CoreNext:\n{seam_witness_next}"
    );
    assert!(
        rendered.contains(
            "WitnessInit_basic_round_trip ==\n    /\\ BaseInit\n    /\\ pending_inputs = <<[machine |-> \"meerkat\", variant |-> \"RecoverRuntimeAuthority\""
        ),
        "seam witness must preload the runtime authority recovery input that creates the session"
    );
    let seam_destroy_witness_next = rendered
        .split("WitnessNext_destroy_runtime_path ==")
        .nth(1)
        .and_then(|tail| tail.split("CiStateConstraint ==").next())
        .expect("seam destroy witness next section");
    assert!(
        !seam_destroy_witness_next.contains("CoreNext")
            && seam_destroy_witness_next.contains("mob_DestroyMob")
            && seam_destroy_witness_next.contains("meerkat_Destroy")
            && !seam_destroy_witness_next.contains("mob_ObserveRuntimeDestroyed"),
        "destroy seam witness must use the signal transition that emits RequestRuntimeDestroy; RuntimeDestroyed is a delivered terminal route, not a consumable post-destroy mob transition:\n{seam_destroy_witness_next}"
    );

    let adaptive = render_composition_semantic_model(&adaptive_mob_bundle_composition())
        .expect("render adaptive composition semantic model");
    let witness_next = adaptive
        .split("WitnessNext_layer_terminal_feedback ==")
        .nth(1)
        .and_then(|tail| tail.split("CiStateConstraint ==").next())
        .expect("adaptive witness next section");
    assert!(
        !witness_next.contains("CoreNext"),
        "witness specs must use a bounded transition relation instead of the full composition CoreNext"
    );
    assert!(
        witness_next.contains("DeliverQueuedRoute")
            && witness_next.contains("layer_mob_ClassifyFlowRunPublicResultSuccessRunning")
            && !witness_next.contains("QuiescentStutter")
            && witness_next.contains("WitnessSatisfiedStutter_layer_terminal_feedback"),
        "adaptive witness next must include route delivery, expected source transition, and terminal witness stutter without a pre-satisfaction quiescent loop:\n{witness_next}"
    );
    assert!(
        adaptive.contains(
            "RouteObserved_layer_terminal_reaches_adaptive_kernel == \\E packet \\in delivered_routes : packet.route = \"layer_terminal_reaches_adaptive_kernel\""
        ),
        "witness route observation must require delivered route state, not merely declared coverage"
    );
    assert!(
        !adaptive.contains("WitnessFairness_layer_terminal_feedback_1 =="),
        "adaptive witness fairness should stay bounded to the scripted route proof"
    );
    assert!(adaptive.contains(
        "WitnessNoPrematureStutter_layer_terminal_feedback ==\n    \\/ WitnessScriptComplete_layer_terminal_feedback\n    \\/ model_step_count' # model_step_count"
    ));

    let schedule = render_composition_semantic_model(&schedule_bundle_composition())
        .expect("render schedule composition semantic model");
    let route_only_witness = schedule
        .split("WitnessNext_revision_supersede_route ==")
        .nth(1)
        .and_then(|tail| {
            tail.split("WitnessNext_occurrence_supersede_ack_route ==")
                .next()
        })
        .expect("schedule revision supersede witness next section");
    assert!(
        !route_only_witness.contains("CoreNext")
            && route_only_witness.contains("schedule_ReviseActive")
            && route_only_witness.contains("WitnessSatisfiedStutter_revision_supersede_route"),
        "schedule route witnesses must be scripted through the producer transition, not the full composition CoreNext:\n{route_only_witness}"
    );
    assert!(
        schedule.contains(
            "WitnessInit_revision_supersede_route ==\n    /\\ BaseInit\n    /\\ pending_inputs = <<[machine |-> \"schedule\", variant |-> \"Revise\""
        ),
        "revision supersede witness must preload the schedule Revise input that produces its expected route"
    );
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
        "pub fn transition<C: Context>(",
        "pub fn transition_signal<C: Context>(",
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
        "pub mod helpers",
    ] {
        assert!(
            rendered.contains(required),
            "typed kernel module contract should contain `{required}`:\n{rendered}"
        );
    }
}

#[test]
fn generated_meerkat_operation_status_is_closed_enum() {
    let rendered = render_machine_kernel_module(&meerkat_machine());

    assert!(
        rendered.contains("pub enum OperationStatus"),
        "OperationStatus must be generated as a closed enum:\n{rendered}"
    );
    for variant in [
        "Absent",
        "Provisioning",
        "Running",
        "Retiring",
        "Completed",
        "Failed",
        "Aborted",
        "Cancelled",
        "Retired",
        "Terminated",
    ] {
        assert!(
            rendered.contains(&format!("    {variant},")),
            "OperationStatus must include variant `{variant}`:\n{rendered}"
        );
    }
    assert!(
        !rendered.contains("pub struct OperationStatus(pub String);"),
        "OperationStatus must not accept arbitrary strings:\n{rendered}"
    );
    assert!(
        !rendered.contains("impl From<&str> for OperationStatus"),
        "OperationStatus must not expose unchecked string construction:\n{rendered}"
    );
}

#[test]
fn generated_meerkat_operation_kind_uses_string_enum_binding() {
    let rendered = render_machine_kernel_module(&meerkat_machine());

    assert!(
        rendered.contains("pub enum OperationKind"),
        "OperationKind must be generated as a closed enum:\n{rendered}"
    );
    assert!(
        rendered.contains("impl std::convert::TryFrom<&str> for OperationKind"),
        "OperationKind must expose checked string parsing from the authoritative binding:\n{rendered}"
    );
    for variant in ["MobMemberChild", "BackgroundToolOp"] {
        assert!(
            rendered.contains(&format!(
                "    #[serde(rename = \"{variant}\")]\n    {variant},"
            )),
            "OperationKind serde must preserve raw value `{variant}`:\n{rendered}"
        );
    }
}

#[test]
fn generated_meerkat_schema_content_shape_binding_is_closed() -> Result<(), String> {
    use meerkat_core::turn_execution_authority::ContentShape;

    let schema = meerkat_machine();
    let binding = schema
        .named_types
        .iter()
        .find(|binding| binding.name.as_str() == ContentShape::SCHEMA_TYPE_NAME)
        .expect("ContentShape binding");

    let RustTypeAtom::StringEnum { variants } = &binding.rust else {
        return Err("ContentShape binding must be a closed StringEnum".to_string());
    };
    let variants = variants
        .iter()
        .map(EnumVariantId::as_str)
        .collect::<Vec<_>>();

    assert_eq!(
        variants.as_slice(),
        ContentShape::SCHEMA_VARIANTS.as_slice()
    );
    Ok(())
}

#[test]
fn generated_meerkat_immediate_starts_derive_content_shape() -> Result<(), String> {
    use meerkat_core::turn_execution_authority::ContentShape;

    let schema = meerkat_machine();
    for input_name in ["StartImmediateAppend", "StartImmediateContext"] {
        let input = schema
            .inputs
            .variant_named(input_name)
            .expect("immediate start input");
        assert!(
            input.field_named("admitted_content_shape").is_err(),
            "{input_name} must derive its content shape instead of accepting caller-supplied shape"
        );
    }

    for (input_name, expected_shape) in [
        ("StartImmediateAppend", ContentShape::ImmediateAppend),
        ("StartImmediateContext", ContentShape::ImmediateContext),
    ] {
        for phase in ["Initializing", "Attached", "Running"] {
            let transition_name = format!("{input_name}{phase}");
            let transition = schema
                .transitions
                .iter()
                .find(|transition| transition.name.as_str() == transition_name)
                .expect("immediate start transition");

            let TriggerMatch::Input { variant, bindings } = &transition.on else {
                return Err(format!("{transition_name} must trigger on an input"));
            };
            assert_eq!(variant.as_str(), input_name);
            assert!(
                !bindings
                    .iter()
                    .any(|binding| binding.as_str() == "admitted_content_shape"),
                "{transition_name} must not bind caller-supplied content shape"
            );

            let assigned_shape = transition
                .updates
                .iter()
                .find_map(|update| match update {
                    Update::Assign { field, expr }
                        if field.as_str() == "admitted_content_shape" =>
                    {
                        Some(expr)
                    }
                    _ => None,
                })
                .expect("admitted_content_shape assignment");

            let Expr::Some(inner) = assigned_shape else {
                return Err(format!(
                    "{transition_name} must assign Some(ContentShape::...)"
                ));
            };
            let Expr::NamedVariant { enum_name, variant } = inner.as_ref() else {
                return Err(format!(
                    "{transition_name} must assign a typed ContentShape variant"
                ));
            };
            assert_eq!(enum_name.as_str(), ContentShape::SCHEMA_TYPE_NAME);
            assert_eq!(variant.as_str(), expected_shape.schema_variant());
        }
    }

    for phase in ["Initializing", "Attached", "Running"] {
        let transition_name = format!("StartConversationRun{phase}");
        let transition = schema
            .transitions
            .iter()
            .find(|transition| transition.name.as_str() == transition_name)
            .expect("conversation start transition");
        assert!(
            transition
                .guards
                .iter()
                .any(|guard| guard.name == "conversation_shape_matches_primitive"),
            "{transition_name} must reject immediate-only shapes on conversation starts"
        );
    }
    Ok(())
}

#[test]
fn generated_meerkat_kernel_content_shape_routes_wire_labels_through_core() {
    use meerkat_core::turn_execution_authority::ContentShape;

    let rendered = render_machine_kernel_module(&meerkat_machine());
    for (variant, core_shape) in [
        ("Conversation", ContentShape::Conversation),
        (
            "ConversationAndContext",
            ContentShape::ConversationAndContext,
        ),
        ("Context", ContentShape::Context),
        ("Empty", ContentShape::Empty),
        ("ImmediateAppend", ContentShape::ImmediateAppend),
        ("ImmediateContext", ContentShape::ImmediateContext),
    ] {
        let rename = format!("    #[serde(rename = \"{}\")]", core_shape.as_str());
        assert!(
            rendered.contains(&rename),
            "generated ContentShape::{variant} must serialize as the core wire label:\n{rendered}"
        );

        let match_arm = format!(
            "Self::{variant} => meerkat_core::turn_execution_authority::ContentShape::{variant}.as_str(),"
        );
        assert!(
            rendered.contains(&match_arm),
            "generated ContentShape::{variant} as_str must route through the core contract:\n{rendered}"
        );
    }
}

#[test]
fn generated_meerkat_terminal_failures_derive_typed_cause_from_source() {
    let rendered = render_machine_kernel_module(&meerkat_machine());

    assert!(
        rendered.contains("pub enum TurnTerminalCauseKind"),
        "generated kernel must expose a closed terminal-cause enum:\n{rendered}"
    );
    assert!(
        !rendered.contains("pub struct FatalFailure {\n        pub error: String,\n    }"),
        "FatalFailure must not remain constructible as a string-only semantic cause path:\n{rendered}"
    );
    assert!(
        rendered.contains("pub terminal_failure_source: RunFailureSourceKind,"),
        "FatalFailure input must carry a typed raw failure source beside display text:\n{rendered}"
    );
    assert!(
        !rendered.contains(
            "pub struct FatalFailure {\n        pub run_id: RunId,\n        pub terminal_cause_kind: TurnTerminalCauseKind,"
        ),
        "FatalFailure input must not accept caller-supplied terminal cause beside display text:\n{rendered}"
    );
    assert!(
        rendered.contains("pub terminal_cause_kind: TurnTerminalCauseKind,"),
        "TurnRunFailed effect must carry the generated typed cause payload:\n{rendered}"
    );
    assert!(
        rendered.contains("pub struct TurnRunFailed")
            && rendered.contains("TurnRunFailed(effects::TurnRunFailed)"),
        "TurnRunFailed must remain emitted with the typed cause payload:\n{rendered}"
    );
}

#[test]
fn generated_meerkat_closed_dsl_domains_use_string_enum_bindings() {
    let rendered = render_machine_kernel_module(&meerkat_machine());

    for (domain, variants) in [
        (
            "TurnPhase",
            &[
                "Ready",
                "ApplyingPrimitive",
                "CallingLlm",
                "WaitingForOps",
                "DrainingBoundary",
                "Extracting",
                "ErrorRecovery",
                "Cancelling",
                "Completed",
                "Failed",
                "Cancelled",
            ][..],
        ),
        (
            "DrainPhase",
            &["Inactive", "Running", "Stopped", "ExitedRespawnable"],
        ),
        ("DrainMode", &["Timed", "AttachedSession", "PersistentHost"]),
        (
            "McpServerState",
            &["PendingConnect", "Connected", "Failed", "Disconnected"],
        ),
        (
            "OperationTerminalOutcomeKind",
            &[
                "Completed",
                "Failed",
                "Aborted",
                "Cancelled",
                "Retired",
                "Terminated",
            ],
        ),
        (
            "TurnTerminalCauseKind",
            &[
                "Unknown",
                "HookDenied",
                "HookFailure",
                "LlmFailure",
                "ToolFailure",
                "StructuredOutputValidationFailed",
                "BudgetExhausted",
                "TimeBudgetExceeded",
                "TurnLimitReached",
                "RuntimeApplyFailure",
                "FatalFailure",
            ],
        ),
    ] {
        assert!(
            rendered.contains(&format!("pub enum {domain}")),
            "{domain} must be generated as a closed enum:\n{rendered}"
        );
        assert!(
            rendered.contains(&format!("impl std::convert::TryFrom<&str> for {domain}")),
            "{domain} must expose checked string parsing from its binding:\n{rendered}"
        );
        assert!(
            !rendered.contains(&format!("pub struct {domain}(pub String);")),
            "{domain} must not accept arbitrary strings:\n{rendered}"
        );
        assert!(
            !rendered.contains(&format!("impl From<&str> for {domain}")),
            "{domain} must not expose unchecked string construction:\n{rendered}"
        );
        for variant in variants {
            assert!(
                rendered.contains(&format!(
                    "    #[serde(rename = \"{variant}\")]\n    {variant},"
                )),
                "{domain} must preserve raw variant `{variant}` through serde:\n{rendered}"
            );
        }
    }
}

#[test]
fn generated_catalog_lifecycle_domains_use_string_enum_bindings() {
    for (rendered, domains) in [
        (
            render_machine_kernel_module(&auth_machine()),
            vec![(
                "AuthLifecyclePhase",
                vec![
                    "Valid",
                    "Expiring",
                    "Refreshing",
                    "ReauthRequired",
                    "Released",
                ],
            )],
        ),
        (
            render_machine_kernel_module(&mob_machine()),
            vec![
                (
                    "KickoffPhase",
                    vec![
                        "Pending",
                        "Starting",
                        "CallbackPending",
                        "Started",
                        "Failed",
                        "Cancelled",
                    ],
                ),
                ("MobMemberState", vec!["Active", "Retiring"]),
                ("WorkOrigin", vec!["External", "Internal", "Ingest"]),
            ],
        ),
        (
            render_machine_kernel_module(&workgraph_lifecycle()),
            vec![(
                "WorkLifecycleState",
                vec![
                    "Absent",
                    "Open",
                    "InProgress",
                    "Blocked",
                    "Completed",
                    "Cancelled",
                    "Failed",
                ],
            )],
        ),
        (
            render_machine_kernel_module(&schedule_lifecycle()),
            vec![(
                "ScheduleLifecycleState",
                vec!["Active", "Paused", "Deleted"],
            )],
        ),
        (
            render_machine_kernel_module(&occurrence_lifecycle()),
            vec![
                (
                    "OccurrenceFailureClass",
                    vec![
                        "TargetMaterializationFailed",
                        "TargetMissing",
                        "TargetBusy",
                        "RuntimeRejected",
                        "MobRejected",
                        "LeaseLost",
                        "TransportError",
                        "InternalError",
                    ],
                ),
                (
                    "OccurrenceLifecycleState",
                    vec![
                        "Pending",
                        "Claimed",
                        "Dispatching",
                        "AwaitingCompletion",
                        "Completed",
                        "Skipped",
                        "Misfired",
                        "Superseded",
                        "DeliveryFailed",
                    ],
                ),
            ],
        ),
    ] {
        for (domain, variants) in domains {
            assert!(
                rendered.contains(&format!("pub enum {domain}")),
                "{domain} must be generated as a closed enum:\n{rendered}"
            );
            assert!(
                rendered.contains(&format!("impl std::convert::TryFrom<&str> for {domain}")),
                "{domain} must expose checked string parsing from its binding:\n{rendered}"
            );
            assert!(
                !rendered.contains(&format!("pub struct {domain}(pub String);")),
                "{domain} must not accept arbitrary strings:\n{rendered}"
            );
            assert!(
                !rendered.contains(&format!("impl From<&str> for {domain}")),
                "{domain} must not expose unchecked string construction:\n{rendered}"
            );
            for variant in variants {
                assert!(
                    rendered.contains(&format!(
                        "    #[serde(rename = \"{variant}\")]\n    {variant},"
                    )),
                    "{domain} must preserve raw variant `{variant}` through serde:\n{rendered}"
                );
            }
        }
    }
}

#[test]
fn generated_meerkat_operation_kind_rejects_string_enum_ident_collisions() {
    let mut schema = meerkat_machine();
    let binding = schema
        .named_types
        .iter_mut()
        .find(|binding| binding.name.as_str() == "OperationKind")
        .expect("OperationKind binding");
    binding.rust = RustTypeAtom::StringEnum {
        variants: vec![
            EnumVariantId::parse("mob-member").expect("variant slug"),
            EnumVariantId::parse("mob_member").expect("variant slug"),
        ],
    };

    let rendered = render_machine_kernel_module(&schema);
    assert!(
        rendered.contains(
            "compile_error!(\"string enum OperationKind variants `mob-member` and `mob_member` sanitize to duplicate Rust identifier `mob_member`\");"
        ),
        "OperationKind must use named-type binding collision checks:\n{rendered}"
    );
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

#[test]
fn canonical_kernel_modules_are_rendered_from_catalog_schema_not_production_sources() {
    for schema in canonical_machine_schemas() {
        let rendered = render_machine_kernel_module(&schema);

        assert!(
            rendered
                .starts_with("// @generated — Generated by `cargo xtask machine-codegen --all`."),
            "{} kernel module should be generated output",
            schema.machine.as_str()
        );
        for forbidden in [
            "mod source {",
            "include_str!",
            "read_to_string",
            "meerkat-mob/src/machines/mob_machine.rs",
            "meerkat-runtime/src/auth_machine/dsl.rs",
            "meerkat-schedule/src/machines/schedule_lifecycle.rs",
            "meerkat-schedule/src/machines/occurrence_lifecycle.rs",
            "meerkat_machine_schema::mob_catalog_machine_dsl!",
            "meerkat_machine_schema::auth_catalog_machine_dsl!",
        ] {
            assert!(
                !rendered.contains(forbidden),
                "{} kernel module must be catalog/schema-fed, but rendered `{forbidden}`:\n{rendered}",
                schema.machine.as_str()
            );
        }
    }
}

#[test]
fn production_machine_bridges_do_not_own_option_value_helper_semantics() {
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root");

    for rel in [
        "meerkat-mob/src/machines/mob_machine.rs",
        "meerkat-runtime/src/meerkat_machine/dsl.rs",
    ] {
        let source = std::fs::read_to_string(repo_root.join(rel)).expect("read production bridge");
        assert!(
            !source.contains("trait OptionValueExt"),
            "{rel} must import catalog-owned OptionValueExt semantics instead of defining them"
        );
        assert!(
            source.contains("use meerkat_machine_schema::catalog::dsl::OptionValueExt;"),
            "{rel} must keep the catalog-owned OptionValueExt trait in scope for DSL expansion"
        );
    }
}

#[test]
fn mob_flow_projection_kernel_audit_classifies_helpers_as_non_canonical() {
    let canonical_machine_names = canonical_machine_schemas()
        .into_iter()
        .map(|schema| schema.machine.as_str().to_owned())
        .collect::<Vec<_>>();
    let mob_schema = mob_machine();
    let mob_inputs = mob_schema
        .inputs
        .variants
        .iter()
        .map(|variant| variant.name.as_str())
        .collect::<Vec<_>>();
    let audit = meerkat_mob::run::flow_projection_kernel_audit();

    assert_eq!(
        audit.iter().map(|entry| entry.module).collect::<Vec<_>>(),
        vec!["flow_run", "flow_frame", "loop_iteration"],
        "the helper-kernel audit must name every carried-forward flow projection"
    );

    for entry in audit {
        assert_eq!(entry.canonical_owner, "MobMachine");
        assert_eq!(
            entry.role,
            meerkat_mob::run::FlowProjectionKernelRole::MobMachineOwnedFailClosedProjection
        );
        assert!(!entry.canonical_machine);
        assert!(
            !canonical_machine_names
                .iter()
                .any(|machine| machine == entry.module)
        );
        for owning_input in entry.owning_inputs {
            let owning_input_name = owning_input.as_str();
            assert!(
                mob_inputs.contains(&owning_input_name),
                "{} owning input `{owning_input_name}` must exist on canonical MobMachine",
                entry.module
            );
        }
    }
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
    CompositionDriverId, CompositionId, EffectVariantId, InputVariantId, MachineInstanceId, RouteId,
};

fn sample_driver() -> CompositionDriver {
    CompositionDriver {
        name: CompositionDriverId::parse("noop_driver").expect("driver slug"),
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
        refusal_closures: vec![],
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
fn render_composition_driver_emits_generated_route_facts() {
    let mut composition = meerkat_mob_seam_composition();
    composition.driver = Some(sample_driver());

    let rendered =
        render_composition_driver(&composition).expect("driver-bearing composition emits");

    // Typed identity imports + route descriptors are present; no stringly
    // tuple tables or generated-shape effect mirrors survive.
    assert!(
        rendered.contains(
            "use meerkat_machine_schema::identity::{CompositionId, EffectVariantId, FieldId, InputVariantId, MachineId, MachineInstanceId, RouteId, SignalVariantId};"
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
    assert!(
        rendered.contains("pub struct NoopStorePlan"),
        "rendered module must declare the catalog-named store plan:\n{rendered}"
    );
    assert!(
        rendered.contains("pub struct NoopWork"),
        "rendered module must declare the catalog-named work packet:\n{rendered}"
    );
    assert!(
        rendered.contains("pub enum NoopDecision"),
        "rendered module must declare the catalog-named decision enum:\n{rendered}"
    );
    assert!(
        rendered.contains("pub struct NoopDriver"),
        "rendered module must declare the catalog-named driver:\n{rendered}"
    );
    assert!(
        rendered.contains("pub fn decide(work: &NoopWork) -> NoopDecision"),
        "rendered driver must expose generated production dispatch:\n{rendered}"
    );
    assert!(
        rendered.contains("pub fn store_plan(decision: NoopDecision) -> Option<NoopStorePlan>"),
        "rendered driver must expose generated store-plan conversion:\n{rendered}"
    );
    for forbidden in [
        "pub const WATCHED_EFFECTS",
        "pub const DISPATCH_ROUTES",
        "(&str, &str, &str, &str)",
        "(&str, &str)",
        "pub const DRIVER_TYPE",
        "pub enum MeerkatMobSeamEffect",
        "crate::generated::mob::Effect",
        "crate::generated::meerkat::Effect",
    ] {
        assert!(
            !rendered.contains(forbidden),
            "stringly/legacy shape `{forbidden}` must not survive:\n{rendered}"
        );
    }

    // Generated producer, variant, target, and field facts are available
    // without depending on generated-shape payload enums.
    assert!(
        rendered.contains("pub struct ProducerFacts"),
        "rendered module must declare producer facts:\n{rendered}"
    );
    assert!(
        rendered.contains("pub fn composition_id() -> CompositionId"),
        "rendered module must declare composition_id:\n{rendered}"
    );
    assert!(
        rendered.contains("pub mod producers"),
        "rendered module must declare producer facts module:\n{rendered}"
    );
    assert!(
        rendered.contains("pub mod effects"),
        "rendered module must declare effect variant facts module:\n{rendered}"
    );
    assert!(
        rendered.contains("pub mod fields"),
        "rendered module must declare field facts module:\n{rendered}"
    );

    // route_to_input signature and a sample Input-route fact.
    assert!(
        rendered.contains("pub fn route_to_input("),
        "rendered module must declare route_to_input:\n{rendered}"
    );
    assert!(
        rendered.contains("pub fn route_binding_request_reaches_meerkat() -> TypedRoutedInput"),
        "rendered module must expose the binding route fact:\n{rendered}"
    );
    assert!(
        rendered.contains("effects::mob::request_runtime_binding()"),
        "route_to_input must resolve the generated mob RequestRuntimeBinding variant fact:\n{rendered}"
    );

    // Signal-kind routes are emitted through the generated signal surface.
    assert!(
        rendered.contains("pub fn route_to_signal("),
        "rendered module must declare route_to_signal:\n{rendered}"
    );
    assert!(
        rendered.contains("pub fn route_runtime_bound_reaches_mob() -> TypedRoutedSignal"),
        "rendered module must expose the runtime-bound signal route fact:\n{rendered}"
    );

    assert!(
        rendered.contains("@generated"),
        "rendered module must carry the @generated marker:\n{rendered}"
    );
}

#[test]
fn meerkat_mob_driver_generates_typed_consumer_refusal_closures() {
    let rendered = render_composition_driver(&meerkat_mob_seam_composition())
        .expect("canonical mob seam has a generated driver");

    for required in [
        "pub struct TypedDispatchRefusalClosure",
        "pub enum GeneratedRefusalFieldSource",
        "pub fn refusal_closure_binding_request_reaches_meerkat()",
        "pub fn refusal_closure_work_request_reaches_meerkat()",
        "pub fn refusal_closure_retire_request_reaches_meerkat()",
        "pub fn refusal_closure_for_route(",
        "GeneratedRefusalFieldSource::ConsumerErrorCode",
        "GeneratedRefusalFieldSource::ConsumerErrorMessage",
        "InputVariantId::parse(\"ResolveRuntimeBindingRefusal\")",
        "InputVariantId::parse(\"ResolveRuntimeIngressRefusal\")",
        "InputVariantId::parse(\"ResolveRuntimeRetireRefusal\")",
    ] {
        assert!(
            rendered.contains(required),
            "generated refusal closure is missing `{required}`:\n{rendered}"
        );
    }
    assert!(
        !rendered.contains("refusal_closure_destroy_request_reaches_meerkat"),
        "destroy keeps the explicit incomplete-destroy retry contract rather than an ordinary refusal closure"
    );
}

#[test]
fn render_composition_driver_emission_is_composition_name_agnostic() {
    // The framework must work for any composition: the header and
    // composition_id follow the composition slug, while the generated fact
    // resolvers remain available.
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
        rendered.contains("CompositionId::parse(\"arbitrary_composition\")"),
        "composition_id must use the renamed composition slug:\n{rendered}"
    );
    assert!(
        rendered.contains("pub fn route_to_input("),
        "route_to_input must stay available for renamed compositions:\n{rendered}"
    );
}

#[test]
fn render_adaptive_mob_bundle_driver_emits_layer_terminal_route() {
    let rendered = render_composition_driver(&adaptive_mob_bundle_composition())
        .expect("adaptive mob bundle emits driver facts");

    assert!(
        rendered
            .contains("pub fn route_layer_terminal_reaches_adaptive_kernel() -> TypedRoutedInput"),
        "adaptive bundle must emit its canonical layer-terminal route fact:\n{rendered}"
    );
    assert!(
        rendered.contains("effects::layer_mob::flow_run_public_result_classified()"),
        "adaptive route_to_input must resolve the watched layer-mob terminal effect:\n{rendered}"
    );
    assert!(
        rendered.contains("pub struct AdaptiveMobBundleDriver"),
        "adaptive bundle must emit its catalog-named driver type:\n{rendered}"
    );
    assert!(
        rendered
            .contains("pub fn decide(work: &AdaptiveMobBundleWork) -> AdaptiveMobBundleDecision"),
        "adaptive bundle must emit generated driver dispatch:\n{rendered}"
    );
    assert!(
        rendered.contains(
            "pub fn store_plan(decision: AdaptiveMobBundleDecision) -> Option<AdaptiveMobBundleStorePlan>"
        ),
        "adaptive bundle must emit generated store-plan conversion:\n{rendered}"
    );
    assert!(
        rendered.contains("InputVariantId::parse(\"IngestLayerTerminal\")"),
        "adaptive route must target control_mob.IngestLayerTerminal:\n{rendered}"
    );
    assert!(
        rendered.contains("(FieldId::parse(\"result\").expect(\"route producer field slug\"), FieldId::parse(\"result_class\").expect(\"route consumer field slug\")),"),
        "adaptive route must carry the producer result into result_class while owner-provided context stays driver-owned:\n{rendered}"
    );
}

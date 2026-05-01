#![allow(clippy::expect_used, clippy::panic)]

use std::{collections::BTreeMap, fs};

use meerkat_mob::runtime::flow_frame_engine::{
    FlowFrameLoopStorePlanVariant, canonical_flow_frame_loop_store_plan_commit_requirements,
};
use tempfile::tempdir;
use xtask::machines::*;

const LIVE_WORKSPACE_RUNFILES: &str = "required";

fn require_live_workspace_runfiles() {
    assert_eq!(LIVE_WORKSPACE_RUNFILES, "required");
}

#[test]
fn registry_selection_accepts_canonical_machine_name_and_slug() {
    let registry = CanonicalRegistry::load();
    let by_name = registry
        .select(&SelectionArgs {
            all: false,
            machines: vec!["MeerkatMachine".into()],
            compositions: vec![],
        })
        .expect("selection by name");
    assert_eq!(by_name.machines.len(), 1);
    assert_eq!(
        by_name.machines[0].schema.machine.as_str(),
        "MeerkatMachine"
    );

    let by_slug = registry
        .select(&SelectionArgs {
            all: false,
            machines: vec!["meerkat_machine".into()],
            compositions: vec![],
        })
        .expect("selection by slug");
    assert_eq!(by_slug.machines.len(), 1);
    assert_eq!(
        by_slug.machines[0].schema.machine.as_str(),
        "MeerkatMachine"
    );
}

#[test]
fn registry_selection_rejects_absorbed_machine_slugs() {
    let registry = CanonicalRegistry::load();

    for absorbed in ["runtime_control", "flow_run", "mob_orchestrator"] {
        let result = registry.select(&SelectionArgs {
            all: false,
            machines: vec![absorbed.into()],
            compositions: vec![],
        });
        let err = match result {
            Ok(selection) => panic!(
                "absorbed machine {absorbed} resolved canonically as {:?}",
                selection
                    .machines
                    .iter()
                    .map(|entry| entry.schema.machine.clone())
                    .collect::<Vec<_>>()
            ),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("unknown machine selection"),
            "unexpected error for {absorbed}: {err}"
        );
    }
}

#[test]
fn registry_validation_covers_canonical_machine_and_composition_sets() {
    let registry = CanonicalRegistry::load();
    assert!(registry.validate().is_ok());
}

#[test]
fn flow_frame_loop_store_plan_commit_requirements_cover_all_variants_with_authority_cas() {
    let requirements = canonical_flow_frame_loop_store_plan_commit_requirements();
    let mut by_variant = BTreeMap::new();
    for requirement in requirements {
        let previous = by_variant.insert(
            requirement.variant.as_str(),
            requirement.operation.store_methods(),
        );
        assert!(
            previous.is_none(),
            "duplicate commit requirement for {}",
            requirement.variant.as_str()
        );
        let methods = requirement.operation.store_methods();
        assert_eq!(
            methods.len(),
            1,
            "expected exactly one authority CAS method for {}",
            requirement.variant.as_str()
        );
        assert!(
            methods[0].starts_with("cas_") && methods[0].ends_with("_with_authority"),
            "expected authority MobRunStore CAS method for {}, got {}",
            requirement.variant.as_str(),
            methods[0]
        );
    }

    let expected = [
        (
            FlowFrameLoopStorePlanVariant::InsertFrame,
            "cas_frame_state_with_authority",
        ),
        (
            FlowFrameLoopStorePlanVariant::FrameState,
            "cas_frame_state_with_authority",
        ),
        (
            FlowFrameLoopStorePlanVariant::CompleteStepAndRecordOutput,
            "cas_complete_step_and_record_output_with_authority",
        ),
        (
            FlowFrameLoopStorePlanVariant::GrantNodeSlot,
            "cas_grant_node_slot_with_authority",
        ),
        (
            FlowFrameLoopStorePlanVariant::StartLoop,
            "cas_start_loop_with_authority",
        ),
        (
            FlowFrameLoopStorePlanVariant::GrantBodyFrameStart,
            "cas_grant_body_frame_start_with_authority",
        ),
        (
            FlowFrameLoopStorePlanVariant::RunStateOnly,
            "cas_flow_state_with_authority",
        ),
        (
            FlowFrameLoopStorePlanVariant::SealFrame,
            "cas_frame_state_with_authority",
        ),
        (
            FlowFrameLoopStorePlanVariant::CompleteBodyFrame,
            "cas_complete_body_frame_with_authority",
        ),
        (
            FlowFrameLoopStorePlanVariant::LoopRequestBodyFrame,
            "cas_loop_request_body_frame_with_authority",
        ),
        (
            FlowFrameLoopStorePlanVariant::CompleteLoop,
            "cas_complete_loop_with_authority",
        ),
    ];

    assert_eq!(by_variant.len(), expected.len());
    for (variant, method) in expected {
        assert_eq!(
            by_variant.get(variant.as_str()).copied(),
            Some([method]),
            "unexpected commit method for {}",
            variant.as_str()
        );
    }
}

#[test]
fn codegen_writes_canonical_machine_and_composition_authority_modules() {
    let registry = CanonicalRegistry::load();
    let selection = registry
        .select(&SelectionArgs {
            all: false,
            machines: vec!["meerkat_machine".into(), "mob_machine".into()],
            compositions: vec!["meerkat_mob_seam".into()],
        })
        .expect("selection");
    let dir = tempdir().expect("tempdir");

    machine_codegen_at_root(dir.path(), &selection).expect("generate outputs");

    let meerkat_model =
        fs::read_to_string(dir.path().join("specs/machines/meerkat_machine/model.tla"))
            .expect("meerkat model");
    assert!(meerkat_model.contains("Generated semantic machine model for MeerkatMachine."));

    let mob_model = fs::read_to_string(dir.path().join("specs/machines/mob_machine/model.tla"))
        .expect("mob model");
    assert!(mob_model.contains("Generated semantic machine model for MobMachine."));
    assert!(!mob_model.contains("MeerkatIdValues"));

    let seam_model = fs::read_to_string(
        dir.path()
            .join("specs/compositions/meerkat_mob_seam/model.tla"),
    )
    .expect("seam model");
    assert!(seam_model.contains("Generated composition model for meerkat_mob_seam."));
    assert!(seam_model.contains("binding_request_reaches_meerkat"));
    assert!(seam_model.contains("work_request_reaches_meerkat"));
    assert!(seam_model.contains("EntryPacketAdmissible(packet) =="));
    assert!(seam_model.contains("RejectPendingEntryInput =="));
}

#[test]
fn drift_check_reports_missing_and_stale_generated_files_for_canonical_selection() {
    let registry = CanonicalRegistry::load();
    let selection = registry
        .select(&SelectionArgs {
            all: false,
            machines: vec!["meerkat_machine".into()],
            compositions: vec!["meerkat_mob_seam".into()],
        })
        .expect("selection");
    let dir = tempdir().expect("tempdir");

    let missing = collect_drift_mismatches(dir.path(), &selection).expect("missing mismatches");
    assert!(!missing.is_empty());

    machine_codegen_at_root(dir.path(), &selection).expect("generate outputs");
    let clean = collect_drift_mismatches(dir.path(), &selection).expect("clean mismatches");
    assert!(clean.is_empty());

    let machine_path = dir.path().join("specs/machines/meerkat_machine/model.tla");
    fs::write(&machine_path, "stale output").expect("mutate machine model");
    let stale = collect_drift_mismatches(dir.path(), &selection).expect("stale mismatches");
    assert_eq!(stale.len(), 1);
    assert!(stale[0].contains(machine_path.to_string_lossy().as_ref()));
}

#[test]
fn authority_language_check_reports_stale_terms() {
    let dir = tempdir().expect("tempdir");
    let docs = dir.path().join("docs/reference");
    fs::create_dir_all(&docs).expect("create docs dir");
    let file = docs.join("note.md");
    fs::write(&file, "schema.yaml and PureHandKernel are stale").expect("write file");

    let mismatches = collect_authority_language_mismatches(dir.path()).expect("mismatches");
    assert_eq!(mismatches.len(), 3);
    assert!(mismatches.iter().any(|item| item.contains("schema.yaml")));
    assert!(
        mismatches
            .iter()
            .any(|item| item.contains("PureHandKernel"))
    );
    assert!(mismatches.iter().any(|item| item.contains("PureHand")));
}

#[test]
fn authority_language_check_reports_live_docs_outside_retired_architecture_scope() {
    let dir = tempdir().expect("tempdir");
    let docs = dir.path().join("docs/guides");
    fs::create_dir_all(&docs).expect("create docs dir");
    let file = docs.join("runtime.md");
    fs::write(&file, "PureHandKernel is stale").expect("write file");

    let mismatches = collect_authority_language_mismatches(dir.path()).expect("mismatches");
    assert!(
        mismatches
            .iter()
            .any(|item| item.contains("docs/guides/runtime.md")),
        "expected stale authority language in live docs/guides to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn peer_response_terminal_projection_ratchet_rejects_runtime_boundary_projection() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-runtime/src");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("input.rs"),
        r#"
fn bad() {
    let _ = PeerConversationProjection::ResponseTerminal { fact };
    let _ = "[SYSTEM NOTICE][PEER_RESPONSE_TERMINAL]";
    let _ = peer_response_terminal_context_key("peer", "req");
}

#[cfg(test)]
mod tests {
    fn allowed_test_fixture() {
        let _ = "[SYSTEM NOTICE][PEER_RESPONSE_TERMINAL]";
    }
}
"#,
    )
    .expect("write boundary file");

    let mismatches = collect_peer_response_terminal_projection_mismatches(dir.path())
        .expect("peer response terminal projection mismatches");
    assert_eq!(
        mismatches.len(),
        3,
        "expected production boundary projection violations only, got {mismatches:#?}"
    );
    assert!(
        mismatches.iter().all(|mismatch| {
            mismatch.contains("meerkat-runtime/src/input.rs")
                && !mismatch.contains("allowed_test_fixture")
        }),
        "expected mismatches to point at production boundary lines, got {mismatches:#?}"
    );
}

#[test]
fn peer_response_terminal_projection_ratchet_rejects_core_string_identity_bus() {
    let dir = tempdir().expect("tempdir");
    let core = dir.path().join("meerkat-core/src");
    fs::create_dir_all(&core).expect("create core dir");
    fs::write(
        core.join("handles.rs"),
        r"
pub struct PeerResponseTerminalSource {
    pub transport_identity: Option<String>,
    pub route_identity: String,
    pub display_identity: String,
}

pub struct PeerResponseTerminalFact {
    pub correlation_id: String,
}

impl PeerResponseTerminalFact {
    pub fn context_key(&self) -> String {
        peer_response_terminal_context_key(&self.source.route_identity, &self.correlation_id)
    }
}
",
    )
    .expect("write core handles file");

    let mismatches = collect_peer_response_terminal_projection_mismatches(dir.path())
        .expect("peer response terminal projection mismatches");
    assert!(
        mismatches
            .iter()
            .any(|mismatch| mismatch.contains("transport_identity")),
        "expected transport identity string bus to be rejected, got {mismatches:#?}"
    );
    assert!(
        mismatches
            .iter()
            .any(|mismatch| mismatch.contains("route_identity")),
        "expected route identity string bus to be rejected, got {mismatches:#?}"
    );
    assert!(
        mismatches
            .iter()
            .any(|mismatch| mismatch.contains("display_identity")),
        "expected display identity string bus to be rejected, got {mismatches:#?}"
    );
    assert!(
        mismatches
            .iter()
            .any(|mismatch| mismatch.contains("correlation_id")),
        "expected correlation id string bus to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn peer_response_terminal_projection_ratchet_rejects_shell_peer_name_identity_bus() {
    let dir = tempdir().expect("tempdir");
    let contracts = dir.path().join("meerkat-contracts/src/wire");
    let rpc = dir.path().join("meerkat-rpc/src");
    fs::create_dir_all(&contracts).expect("create contracts dir");
    fs::create_dir_all(rpc.join("handlers")).expect("create rpc handlers dir");
    fs::write(
        contracts.join("runtime.rs"),
        r"
pub struct SessionPeerResponseTerminalParams {
    pub peer_name: PeerName,
}
",
    )
    .expect("write contract boundary file");
    fs::write(
        rpc.join("session_runtime.rs"),
        r"
fn bad(peer_name: PeerName) {
    let route = PeerResponseTerminalRouteIdentity::parse(peer_name.as_str().to_string());
    let display = PeerResponseTerminalDisplayIdentity::parse(peer_name.as_str().to_string());
    let _ = peer_response_terminal_input(&peer_name, request_id, status, result);
}
",
    )
    .expect("write rpc boundary file");

    let mismatches = collect_peer_response_terminal_projection_mismatches(dir.path())
        .expect("peer response terminal projection mismatches");
    assert!(
        mismatches
            .iter()
            .any(|mismatch| mismatch.contains("pub peer_name: PeerName")),
        "expected contract peer_name identity bus to be rejected, got {mismatches:#?}"
    );
    assert!(
        mismatches
            .iter()
            .any(|mismatch| mismatch.contains("RouteIdentity")),
        "expected route identity projection from peer_name to be rejected, got {mismatches:#?}"
    );
    assert!(
        mismatches
            .iter()
            .any(|mismatch| mismatch.contains("DisplayIdentity")),
        "expected display identity projection from peer_name to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn canonical_machine_inventory_matches_docs_and_artifact_bundle() {
    require_live_workspace_runfiles();
    let root = repo_root().expect("repo root");
    let mismatches =
        collect_machine_inventory_mismatches(&root).expect("machine inventory mismatches");
    assert!(
        mismatches.is_empty(),
        "expected canonical machine inventory to match docs and artifact bundle, got {mismatches:#?}"
    );
}

#[test]
fn row22_kernel_public_api_contract_rejects_legacy_exports() {
    require_live_workspace_runfiles();
    let root = repo_root().expect("repo root");
    let kernel_lib = root.join("meerkat-machine-kernels/src/lib.rs");
    let contents = fs::read_to_string(&kernel_lib).expect("read kernel lib");

    assert!(
        !contents.contains("pub use runtime::{"),
        "row-22 public API contract should not re-export the legacy runtime surface from {}:\n{contents}",
        kernel_lib.display()
    );
}

#[test]
fn kernel_generated_inventory_is_canonical_five_only() {
    require_live_workspace_runfiles();
    let root = repo_root().expect("repo root");
    let generated_mod = root.join("meerkat-machine-kernels/src/generated/mod.rs");
    let contents = fs::read_to_string(&generated_mod).expect("read generated mod");

    for required in [
        "auth",
        "meerkat",
        "mob",
        "occurrence_lifecycle",
        "schedule_lifecycle",
    ] {
        assert!(
            contents.contains(&format!("pub mod {required};")),
            "canonical kernel inventory must include `{required}` in {}:\n{contents}",
            generated_mod.display()
        );
    }

    for forbidden in ["flow_run", "flow_frame", "loop_iteration"] {
        assert!(
            !contents.contains(&format!("pub mod {forbidden};")),
            "canonical kernel inventory must not export compat module `{forbidden}` from {}:\n{contents}",
            generated_mod.display()
        );
    }
}

#[test]
fn generated_kernel_boundary_rejects_production_owner_machine_body() {
    let dir = tempdir().expect("tempdir");
    let generated = dir
        .path()
        .join("meerkat-machine-kernels/src/generated/meerkat.rs");
    fs::create_dir_all(generated.parent().expect("generated parent")).expect("create generated");
    fs::write(&generated, "// generated kernel exists").expect("write generated kernel");

    let owner = dir
        .path()
        .join("meerkat-runtime/src/meerkat_machine/dsl.rs");
    fs::create_dir_all(owner.parent().expect("owner parent")).expect("create owner");
    fs::write(
        &owner,
        "meerkat_machine_dsl::machine! { machine MeerkatMachine {} }",
    )
    .expect("write production owner body");

    let mismatches =
        collect_generated_kernel_boundary_mismatches(dir.path()).expect("boundary mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MeerkatMachine")
                && mismatch.contains("meerkat-runtime/src/meerkat_machine/dsl.rs")
        }),
        "expected production owner machine! body to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn generated_kernel_boundary_rejects_retired_generated_source_and_bridge_paths() {
    let dir = tempdir().expect("tempdir");
    for retired in [
        "meerkat-machine-kernels/src/compat_generated.rs",
        "meerkat-machine-kernels/src/generated/flow_run.rs",
        "meerkat-mob/src/generated/flow_frame_loop_driver.rs",
        "meerkat-mob/src/runtime/loop_iteration_authority.rs",
    ] {
        let path = dir.path().join(retired);
        fs::create_dir_all(path.parent().expect("retired parent")).expect("create parent");
        fs::write(&path, "// retired generated source").expect("write retired source");
    }

    let mismatches =
        collect_generated_kernel_boundary_mismatches(dir.path()).expect("boundary mismatches");
    for retired in [
        "meerkat-machine-kernels/src/compat_generated.rs",
        "meerkat-machine-kernels/src/generated/flow_run.rs",
        "meerkat-mob/src/generated/flow_frame_loop_driver.rs",
        "meerkat-mob/src/runtime/loop_iteration_authority.rs",
    ] {
        assert!(
            mismatches.iter().any(|mismatch| mismatch.contains(retired)),
            "expected retired generated/bridge path {retired} to be rejected, got {mismatches:#?}"
        );
    }
}

#[test]
fn generated_kernel_boundary_accepts_canonical_generated_module_without_owner_body() {
    let dir = tempdir().expect("tempdir");
    let generated = dir
        .path()
        .join("meerkat-machine-kernels/src/generated/meerkat.rs");
    fs::create_dir_all(generated.parent().expect("generated parent")).expect("create generated");
    fs::write(&generated, "// generated MeerkatMachine kernel").expect("write generated kernel");

    let owner = dir
        .path()
        .join("meerkat-runtime/src/meerkat_machine/dsl.rs");
    fs::create_dir_all(owner.parent().expect("owner parent")).expect("create owner");
    fs::write(
        &owner,
        "pub use meerkat_machine_kernels::generated::meerkat::schema;",
    )
    .expect("write owner adapter");

    let mismatches =
        collect_generated_kernel_boundary_mismatches(dir.path()).expect("boundary mismatches");
    assert!(
        mismatches.is_empty(),
        "expected canonical generated module plus body-free owner adapter to be accepted, got {mismatches:#?}"
    );
}

#[test]
fn phase1_production_body_audit_accepts_only_declared_carry_forward_bodies() {
    require_live_workspace_runfiles();
    let root = repo_root().expect("repo root");
    let mismatches =
        collect_phase1_production_body_mismatches(&root).expect("production body mismatches");
    assert!(
        mismatches.is_empty(),
        "expected live canonical machine! bodies outside the catalog to be explicit Phase 1 carry-forward debt, got {mismatches:#?}"
    );
}

#[test]
fn phase1_production_body_audit_rejects_new_canonical_body_outside_catalog() {
    let dir = tempdir().expect("tempdir");

    for body in PHASE1_CARRY_FORWARD_PRODUCTION_BODIES {
        let path = dir.path().join(body.path);
        fs::create_dir_all(path.parent().expect("carry-forward parent")).expect("create parent");
        fs::write(
            &path,
            format!("machine! {{ machine {} {{}} }}", body.machine),
        )
        .expect("write carry-forward body");
    }

    let new_body = dir.path().join("meerkat-runtime/src/new_machine.rs");
    fs::create_dir_all(new_body.parent().expect("new body parent")).expect("create parent");
    fs::write(&new_body, "machine! { machine MeerkatMachine {} }").expect("write new body");

    let mismatches =
        collect_phase1_production_body_mismatches(dir.path()).expect("production body mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MeerkatMachine")
                && mismatch.contains("meerkat-runtime/src/new_machine.rs")
        }),
        "expected new canonical body to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn phase1_production_body_audit_rejects_spaced_and_qualified_machine_macros() {
    let dir = tempdir().expect("tempdir");

    for body in PHASE1_CARRY_FORWARD_PRODUCTION_BODIES {
        let path = dir.path().join(body.path);
        fs::create_dir_all(path.parent().expect("carry-forward parent")).expect("create parent");
        fs::write(
            &path,
            format!("machine! {{ machine {} {{}} }}", body.machine),
        )
        .expect("write carry-forward body");
    }

    let spaced_body = dir.path().join("meerkat-runtime/src/spaced_machine.rs");
    fs::create_dir_all(spaced_body.parent().expect("spaced body parent")).expect("create parent");
    fs::write(&spaced_body, "machine ! { machine MeerkatMachine {} }").expect("write spaced body");

    let qualified_body = dir.path().join("meerkat-mob/src/qualified_machine.rs");
    fs::create_dir_all(qualified_body.parent().expect("qualified body parent"))
        .expect("create parent");
    fs::write(
        &qualified_body,
        "meerkat_machine_dsl::machine! { machine MobMachine {} }",
    )
    .expect("write qualified body");

    let alias_body = dir.path().join("meerkat-runtime/src/aliased_machine.rs");
    fs::create_dir_all(alias_body.parent().expect("aliased body parent")).expect("create parent");
    fs::write(
        &alias_body,
        "use meerkat_machine_dsl::machine as local_machine;\nlocal_machine! { machine MeerkatMachine {} }",
    )
    .expect("write aliased body");

    let mismatches =
        collect_phase1_production_body_mismatches(dir.path()).expect("production body mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MeerkatMachine")
                && mismatch.contains("meerkat-runtime/src/spaced_machine.rs")
        }),
        "expected spaced canonical body to be rejected, got {mismatches:#?}"
    );
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobMachine")
                && mismatch.contains("meerkat-mob/src/qualified_machine.rs")
        }),
        "expected qualified canonical body to be rejected, got {mismatches:#?}"
    );
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MeerkatMachine")
                && mismatch.contains("meerkat-runtime/src/aliased_machine.rs")
        }),
        "expected aliased canonical body to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn phase1_production_body_audit_rejects_machine_body_hidden_in_wrapper_macro() {
    let dir = tempdir().expect("tempdir");
    let wrapped_body = dir.path().join("meerkat-runtime/src/wrapped_machine.rs");
    fs::create_dir_all(wrapped_body.parent().expect("wrapped body parent")).expect("create parent");
    fs::write(
        &wrapped_body,
        r"
macro_rules! prod_body {
    () => {
        meerkat_machine_dsl::machine! { machine MeerkatMachine {} }
    };
}
prod_body!();
",
    )
    .expect("write wrapped body");

    let mismatches =
        collect_phase1_production_body_mismatches(dir.path()).expect("production body mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MeerkatMachine")
                && mismatch.contains("meerkat-runtime/src/wrapped_machine.rs")
        }),
        "expected wrapper macro canonical body to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn compat_kernel_modules_and_flow_runtime_mini_machines_are_deleted() {
    require_live_workspace_runfiles();
    let root = repo_root().expect("repo root");
    for forbidden in [
        "meerkat-machine-kernels/src/compat_generated.rs",
        "meerkat-machine-kernels/src/generated/flow_run.rs",
        "meerkat-machine-kernels/src/generated/flow_frame.rs",
        "meerkat-machine-kernels/src/generated/loop_iteration.rs",
        "meerkat-mob/src/generated/flow_run.rs",
        "meerkat-mob/src/generated/flow_frame.rs",
        "meerkat-mob/src/generated/loop_iteration.rs",
        "meerkat-mob/src/runtime/flow_run_kernel.rs",
        "meerkat-mob/src/runtime/flow_frame_kernel.rs",
        "meerkat-mob/src/runtime/loop_iteration_authority.rs",
        "meerkat-mob/src/generated/flow_frame_loop_driver.rs",
    ] {
        let path = root.join(forbidden);
        assert!(
            !path.exists(),
            "strict row-22 end-state requires deleting {forbidden}, but it still exists"
        );
    }
}

#[test]
fn flow_runtime_until_feedback_does_not_use_loop_iteration_authority_bridge() {
    require_live_workspace_runfiles();
    let root = repo_root().expect("repo root");
    let flow_engine = root.join("meerkat-mob/src/runtime/flow_frame_engine.rs");
    let contents = std::fs::read_to_string(&flow_engine)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", flow_engine.display()));

    for forbidden in [
        "loop_iteration_authority",
        "protocol_flow_loop_until_evaluation",
        "submit_until_condition_met",
        "submit_until_condition_failed",
    ] {
        assert!(
            !contents.contains(forbidden),
            "flow until feedback must route through MobMachine, not `{forbidden}`, in {}",
            flow_engine.display()
        );
    }

    let schema_src = root.join("meerkat-machine-schema/src");
    let mut stack = vec![schema_src];
    while let Some(path) = stack.pop() {
        let metadata = fs::metadata(&path)
            .unwrap_or_else(|error| panic!("failed to stat {}: {error}", path.display()));
        if metadata.is_dir() {
            for entry in fs::read_dir(&path)
                .unwrap_or_else(|error| panic!("failed to read dir {}: {error}", path.display()))
            {
                stack.push(
                    entry
                        .unwrap_or_else(|error| {
                            panic!("failed to read dir entry in {}: {error}", path.display())
                        })
                        .path(),
                );
            }
            continue;
        }
        if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
            continue;
        }

        let contents = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        for forbidden in [
            "flow_loop_until_evaluation",
            "loop_iteration_authority",
            "protocol_flow_loop_until_evaluation",
        ] {
            assert!(
                !contents.contains(forbidden),
                "machine schema source must not declare deleted loop-until bridge `{forbidden}` in {}",
                path.display()
            );
        }
    }
}

#[test]
fn live_flow_runtime_reducer_transitions_are_mob_machine_command_gated() {
    require_live_workspace_runfiles();
    let root = repo_root().expect("repo root");
    let mismatches = collect_direct_flow_reducer_transition_mismatches(&root)
        .expect("flow reducer transition mismatches");
    assert!(
        mismatches.is_empty(),
        "flow_run/flow_frame/loop_iteration reducers must only transition behind the MobMachine command gate, got {mismatches:#?}"
    );
}

#[test]
fn live_mob_runtime_catalog_commands_fail_close_on_matching_machine_inputs() {
    require_live_workspace_runfiles();
    let root = repo_root().expect("repo root");
    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(&root)
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.is_empty(),
        "catalog-classified Mob runtime commands must fail-close on their matching MobMachine inputs, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_reducer_transition_ratchet_rejects_direct_runtime_calls() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    let file = runtime.join("bad.rs");
    fs::write(
        &file,
        "fn bad() { let _ = flow_run::transition(&state, input, &ctx); }",
    )
    .expect("write direct reducer call");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow reducer transition mismatches");
    assert!(
        mismatches
            .iter()
            .any(|mismatch| mismatch.contains("meerkat-mob/src/runtime/bad.rs:1")),
        "expected direct runtime reducer call to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_reducer_transition_ratchet_rejects_direct_reducer_input_construction() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    let file = runtime.join("bad_input.rs");
    fs::write(
        &file,
        "fn bad() { let _ = flow_frame::Input::SealFrame(flow_frame::inputs::SealFrame {}); }",
    )
    .expect("write direct reducer input");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow reducer transition mismatches");
    assert!(
        mismatches
            .iter()
            .any(|mismatch| mismatch.contains("meerkat-mob/src/runtime/bad_input.rs:1")),
        "expected direct runtime reducer input to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_reducer_transition_ratchet_rejects_aliases_and_projection_writes() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("aliased.rs"),
        "use crate::run::flow_run as fr;\nuse crate::run::flow_run::transition as run_transition;\nuse crate::run::flow_frame::{transition as frame_transition, Input as FrameInput};\nuse crate::run::flow_frame::*;\nuse crate::run::loop_iteration::{transition, Input};\nfn bad() { let _ = fr::transition(&state, input, &ctx); let _ = run_transition(&state, input, &ctx); let _ = frame_transition(&state, input, &ctx); let _ = FrameInput::SealFrame(payload); let _ = transition(&state, input, &ctx); let _ = Input::CancelLoop(payload); }",
    )
    .expect("write aliased reducer use");
    fs::write(
        runtime.join("projection_write.rs"),
        "async fn bad(mut run: MobRun, mut frame: FrameSnapshot, store: Store) { run.flow_state.phase = flow_run::Phase::Running; run.flow_state.active_node_count = 99; frame.kernel_state.node_status.insert(node, NodeRunStatus::Ready); frame.kernel_state.ready_queue.push(node); let _ = store.cas_flow_state(&id, &old, &run.flow_state).await; }",
    )
    .expect("write projection write");
    fs::write(
        runtime.join("compound_projection_write.rs"),
        "async fn bad(store: Store) { let _ = store.cas_grant_node_slot(a).await; let _ = store.cas_start_loop(b).await; let _ = store.cas_grant_body_frame_start(c).await; let _ = store.cas_complete_body_frame(d).await; let _ = store.cas_loop_request_body_frame(e).await; let _ = store.cas_complete_loop(f).await; }",
    )
    .expect("write compound projection write");
    fs::write(
        runtime.join("projection_helper_wrapper.rs"),
        "async fn persist_body_frame(store: Store) { let _ = store.cas_grant_body_frame_start(c).await; }\nasync fn bad(store: Store) { persist_body_frame(store).await; }",
    )
    .expect("write projection helper wrapper");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow reducer transition mismatches");
    assert!(
        mismatches
            .iter()
            .any(|mismatch| mismatch
                .contains("direct live-flow reducer transition `fr::transition(")),
        "expected module alias reducer call to be rejected, got {mismatches:#?}"
    );
    assert!(
        mismatches.iter().any(|mismatch| mismatch
            .contains("direct live-flow reducer transition alias `frame_transition`")),
        "expected imported transition alias to be rejected, got {mismatches:#?}"
    );
    assert!(
        mismatches.iter().any(|mismatch| mismatch
            .contains("direct live-flow reducer transition alias `run_transition`")),
        "expected direct imported transition alias to be rejected, got {mismatches:#?}"
    );
    assert!(
        mismatches
            .iter()
            .any(|mismatch| mismatch.contains("direct live-flow reducer input alias `FrameInput`")),
        "expected imported input alias to be rejected, got {mismatches:#?}"
    );
    assert!(
        mismatches
            .iter()
            .any(|mismatch| mismatch
                .contains("direct live-flow reducer transition alias `transition`")),
        "expected bare imported transition to be rejected, got {mismatches:#?}"
    );
    assert!(
        mismatches
            .iter()
            .any(|mismatch| mismatch.contains("direct live-flow reducer input alias `Input`")),
        "expected bare imported input to be rejected, got {mismatches:#?}"
    );
    assert!(
        mismatches.iter().any(|mismatch| mismatch
            .contains("direct live-flow projection mutation `.flow_state.phase`")),
        "expected direct flow_state mutation to be rejected, got {mismatches:#?}"
    );
    assert!(
        mismatches.iter().any(|mismatch| mismatch
            .contains("direct live-flow projection mutation `.flow_state.active_node_count`")),
        "expected direct active_node_count mutation to be rejected, got {mismatches:#?}"
    );
    assert!(
        mismatches.iter().any(|mismatch| mismatch
            .contains("direct live-flow projection mutation `.kernel_state.node_status`")),
        "expected direct frame node_status mutation to be rejected, got {mismatches:#?}"
    );
    assert!(
        mismatches.iter().any(|mismatch| mismatch
            .contains("direct live-flow projection mutation `.kernel_state.ready_queue`")),
        "expected direct frame ready_queue mutation to be rejected, got {mismatches:#?}"
    );
    assert!(
        mismatches
            .iter()
            .any(|mismatch| mismatch
                .contains("direct live-flow projection mutation `.cas_flow_state(")),
        "expected direct cas_flow_state write to be rejected, got {mismatches:#?}"
    );
    for token in [
        ".cas_grant_node_slot(",
        ".cas_start_loop(",
        ".cas_grant_body_frame_start(",
        ".cas_complete_body_frame(",
        ".cas_loop_request_body_frame(",
        ".cas_complete_loop(",
    ] {
        assert!(
            mismatches.iter().any(|mismatch| mismatch.contains(token)),
            "expected compound projection CAS write {token} to be rejected, got {mismatches:#?}"
        );
    }
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/projection_helper_wrapper.rs")
                && mismatch.contains(".cas_grant_body_frame_start(")
        }),
        "expected helper wrapper around compound projection CAS to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_indexed_projection_field_writes() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
fn bad(run: &mut MobRun, frame: &mut FrameSnapshot, frame_id: FrameId, node_id: NodeId) {
    run.flow_state.ready_frames[0] = frame_id;
    frame.kernel_state.ready_queue[0] = node_id;
}
",
    )
    .expect("write indexed projection field writes");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".flow_state.ready_frames")
        }),
        "expected indexed flow_state ready_frames write to be rejected, got {mismatches:#?}"
    );
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".kernel_state.ready_queue")
        }),
        "expected indexed kernel_state ready_queue write to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_missing_and_warning_only_gates() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::CreateRunSeed, "create_run_seed")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    let task_id = self.task_board_service.create_task().await?;
    if let Err(error) = self.apply_dsl_input(MobMachineInput::TaskCreate { task_id }, "handle_task_create") {
        tracing::warn!(%error, "TaskCreate DSL input rejected; DSL and event-sourced task board may diverge");
    }
    Ok(task_id)
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.cancel_runtime().await
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            self.spawn_policy.set(policy).await;
            let _ = reply_tx.send(());
        }
        _ => {}
    }
}
"#,
    )
    .expect("write actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    for input in [
        "RunFlow",
        "CancelFlow",
        "TaskCreate",
        "SetSpawnPolicy",
        "ForceCancel",
    ] {
        assert!(
            mismatches.iter().any(|mismatch| {
                mismatch.contains(&format!("MobCommand::{input}"))
                    && mismatch.contains(&format!("MobMachineInput::{input}"))
            }),
            "expected missing/fail-open {input} command gate to be rejected, got {mismatches:#?}"
        );
    }
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_accepts_fail_closed_gates() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    let prepared = self.prepare_dsl_input(MobMachineInput::RunFlow, "run_flow_input")?;
    self.create_pending_run().await?;
    self.commit_prepared_dsl_input(prepared);
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow_input")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    let task_id = TaskId::new();
    let prepared = self.prepare_command_admission(
        MobMachineInput::TaskCreate { task_id },
        MobState::Running,
        "handle_task_create",
    )?;
    self.task_board_service.create_task().await?;
    self.commit_prepared_dsl_input(prepared);
    Ok(task_id)
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel_input")?;
    self.cancel_runtime().await
}

async fn dispatch(&mut self, command: MobCommand) -> Result<(), MobError> {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            self.spawn_policy.set(policy).await;
            let _ = reply_tx.send(());
            Ok(())
        }
        _ => Ok(()),
    }
}
"#,
    )
    .expect("write actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.is_empty(),
        "expected fail-closed catalog command gates to be accepted, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_comment_spoofed_gates() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    // self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    // self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    // self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    // self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            // self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            self.spawn_policy.set(policy).await;
            let _ = reply_tx.send(());
        }
        _ => {}
    }
}
"#,
    )
    .expect("write comment-spoofed actor gates");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    for input in [
        "RunFlow",
        "CancelFlow",
        "TaskCreate",
        "SetSpawnPolicy",
        "ForceCancel",
    ] {
        assert!(
            mismatches.iter().any(|mismatch| {
                mismatch.contains(&format!("MobCommand::{input}"))
                    && mismatch.contains(&format!("MobMachineInput::{input}"))
            }),
            "expected comment-spoofed {input} command gate to be rejected, got {mismatches:#?}"
        );
    }
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_ignored_map_err_gates() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn handle_set_spawn_policy(&mut self) -> Result<(), MobError> {
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            let result = self
                .apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")
                .map_err(|error| MobError::Internal(format!("{error}")));
            self.spawn_policy.set(policy).await;
            let _ = reply_tx.send(result);
        }
        _ => {}
    }
}
"#,
    )
    .expect("write ignored map_err actor gate");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::SetSpawnPolicy")
                && mismatch.contains("MobMachineInput::SetSpawnPolicy")
        }),
        "expected ignored SetSpawnPolicy map_err gate to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_uncontrolled_is_ok_checks() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn handle_set_spawn_policy(&mut self) -> Result<(), MobError> {
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            let result = self
                .apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")
                .map_err(|error| MobError::Internal(format!("{error}")));
            let _ = result.is_ok();
            self.spawn_policy.set(policy).await;
            let _ = reply_tx.send(result);
        }
        _ => {}
    }
}
"#,
    )
    .expect("write uncontrolled is_ok actor gate");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::SetSpawnPolicy")
                && mismatch.contains("MobMachineInput::SetSpawnPolicy")
        }),
        "expected uncontrolled SetSpawnPolicy is_ok check to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_branch_local_is_ok_bypass() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            let result = self
                .apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")
                .map_err(|error| MobError::Internal(format!("{error}")));
            if result.is_ok() {
                self.unrelated.set(policy.clone()).await;
            }
            self.spawn_policy.set(policy).await;
            let _ = reply_tx.send(result);
        }
        _ => {}
    }
}
"#,
    )
    .expect("write branch-local is_ok actor gate");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::SetSpawnPolicy")
                && mismatch.contains("MobMachineInput::SetSpawnPolicy")
        }),
        "expected side effect outside SetSpawnPolicy is_ok branch to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_accepts_helper_side_effect_inside_is_ok_branch() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn apply_spawn_policy(&mut self, policy: SpawnPolicy) -> Result<(), MobError> {
    self.spawn_policy.set(policy).await;
    Ok(())
}

async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.apply_dsl_input(MobMachineInput::TaskCreate { task_id }, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            let result =
                self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy");
            if result.is_ok() {
                self.apply_spawn_policy(policy.clone()).await?;
            }
            let _ = reply_tx.send(result);
        }
        _ => {}
    }
}
"#,
    )
    .expect("write helper side effect inside is_ok branch actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.is_empty(),
        "expected helper side effect inside SetSpawnPolicy is_ok branch to be accepted, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_ufcs_set_before_gate() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.apply_dsl_input(MobMachineInput::TaskCreate { task_id }, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            SpawnPolicyService::set(self.spawn_policy.as_ref(), policy.clone()).await?;
            let result =
                self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy");
            let _ = reply_tx.send(result);
        }
        _ => {}
    }
}
"#,
    )
    .expect("write UFCS set before gate actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::SetSpawnPolicy")
                && mismatch.contains("MobMachineInput::SetSpawnPolicy")
        }),
        "expected UFCS set side effect before SetSpawnPolicy gate to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_ufcs_set_on_associated_cloned_service_alias_before_gate()
 {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.apply_dsl_input(MobMachineInput::TaskCreate { task_id }, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            let service = Arc::clone(&self.spawn_policy);
            SpawnPolicyService::set(service.as_ref(), policy.clone()).await?;
            let result =
                self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy");
            let _ = reply_tx.send(result);
        }
        _ => {}
    }
}
"#,
    )
    .expect("write UFCS set on cloned service alias before gate actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::SetSpawnPolicy")
                && mismatch.contains("MobMachineInput::SetSpawnPolicy")
        }),
        "expected UFCS set on associated cloned service alias before SetSpawnPolicy gate to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_method_set_on_associated_cloned_service_alias_before_gate()
 {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.apply_dsl_input(MobMachineInput::TaskCreate { task_id }, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            let service = Arc::clone(&self.spawn_policy);
            service.set(policy.clone()).await?;
            let result =
                self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy");
            let _ = reply_tx.send(result);
        }
        _ => {}
    }
}
"#,
    )
    .expect("write method set on cloned service alias before gate actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::SetSpawnPolicy")
                && mismatch.contains("MobMachineInput::SetSpawnPolicy")
        }),
        "expected method set on associated cloned service alias before SetSpawnPolicy gate to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_fake_gate_receivers() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            fake.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            self.spawn_policy.set(policy).await;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write fake receiver actor gate");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::SetSpawnPolicy")
                && mismatch.contains("MobMachineInput::SetSpawnPolicy")
        }),
        "expected fake SetSpawnPolicy gate receiver to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_conditional_gate_leaks() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand, should_gate: bool) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            if should_gate {
                self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            }
            self.spawn_policy.set(policy).await;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write conditional gate actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::SetSpawnPolicy")
                && mismatch.contains("MobMachineInput::SetSpawnPolicy")
        }),
        "expected conditionally gated SetSpawnPolicy side effect to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_shadowed_pending_gate_results() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            let result = self
                .apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")
                .map_err(|error| MobError::Internal(format!("{error}")));
            let result = Ok(());
            if result.is_ok() {
                self.spawn_policy.set(policy).await;
            }
            let _ = reply_tx.send(result);
        }
        _ => {}
    }
}
"#,
    )
    .expect("write shadowed pending gate actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::SetSpawnPolicy")
                && mismatch.contains("MobMachineInput::SetSpawnPolicy")
        }),
        "expected shadowed pending SetSpawnPolicy gate result to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_reassigned_pending_gate_results() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            let mut result = self
                .apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")
                .map_err(|error| MobError::Internal(format!("{error}")));
            result = Ok(());
            if result.is_ok() {
                self.spawn_policy.set(policy).await;
            }
            let _ = reply_tx.send(result);
        }
        _ => {}
    }
}
"#,
    )
    .expect("write reassigned pending gate actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::SetSpawnPolicy")
                && mismatch.contains("MobMachineInput::SetSpawnPolicy")
        }),
        "expected reassigned pending SetSpawnPolicy gate result to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_reassigned_gate_inputs() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            let mut input = MobMachineInput::SetSpawnPolicy;
            input = MobMachineInput::RunFlow;
            let result = self
                .apply_dsl_input(input, "set_spawn_policy")
                .map_err(|error| MobError::Internal(format!("{error}")));
            if result.is_ok() {
                self.spawn_policy.set(policy).await;
            }
            let _ = reply_tx.send(result);
        }
        _ => {}
    }
}
"#,
    )
    .expect("write reassigned gate input actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::SetSpawnPolicy")
                && mismatch.contains("MobMachineInput::SetSpawnPolicy")
        }),
        "expected reassigned SetSpawnPolicy gate input to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_if_let_shadowed_pending_gate_results() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand, maybe_result: Option<Result<(), MobError>>) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            let result = self
                .apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")
                .map_err(|error| MobError::Internal(format!("{error}")));
            if let Some(result) = maybe_result {
                if result.is_ok() {
                    self.spawn_policy.set(policy).await;
                }
                let _ = reply_tx.send(result);
            }
        }
        _ => {}
    }
}
"#,
    )
    .expect("write if-let shadowed pending gate actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::SetSpawnPolicy")
                && mismatch.contains("MobMachineInput::SetSpawnPolicy")
        }),
        "expected if-let shadowed pending SetSpawnPolicy gate result to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_unpropagated_gate_result_success_reply() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            let result = self
                .apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")
                .map_err(|error| MobError::Internal(format!("{error}")));
            if result.is_ok() {
                self.spawn_policy.set(policy).await;
            }
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write unpropagated gate result success reply actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::SetSpawnPolicy")
                && mismatch.contains("MobMachineInput::SetSpawnPolicy")
        }),
        "expected SetSpawnPolicy gate result not propagated to reply to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_gate_result_sent_to_unrelated_channel() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand, audit_tx: ReplyTx<Result<(), MobError>>) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            let result = self
                .apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")
                .map_err(|error| MobError::Internal(format!("{error}")));
            if result.is_ok() {
                self.spawn_policy.set(policy).await;
            }
            let _ = audit_tx.send(result);
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write unrelated reply channel actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::SetSpawnPolicy")
                && mismatch.contains("MobMachineInput::SetSpawnPolicy")
        }),
        "expected SetSpawnPolicy gate result sent to unrelated channel to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_shadowed_reply_tx_propagation() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand, audit_tx: ReplyTx<Result<(), MobError>>) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            let result = self
                .apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")
                .map_err(|error| MobError::Internal(format!("{error}")));
            if result.is_ok() {
                self.spawn_policy.set(policy).await;
            }
            let reply_tx = audit_tx;
            let _ = reply_tx.send(result);
        }
        _ => {}
    }
}
"#,
    )
    .expect("write shadowed reply channel actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::SetSpawnPolicy")
                && mismatch.contains("MobMachineInput::SetSpawnPolicy")
        }),
        "expected SetSpawnPolicy gate result sent through shadowed reply_tx to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_accepts_reply_tx_alias_propagation() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.apply_dsl_input(MobMachineInput::TaskCreate { task_id }, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            let result = self
                .apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")
                .map_err(MobError::from);
            if result.is_ok() {
                self.spawn_policy.set(policy).await;
            }
            let tx = reply_tx;
            let _ = tx.send(result);
        }
        _ => {}
    }
}
"#,
    )
    .expect("write reply_tx alias propagation actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.is_empty(),
        "expected SetSpawnPolicy reply sender alias propagation to be accepted, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_deferred_side_effect_proofs() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            let result = self
                .apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")
                .map_err(|error| MobError::Internal(format!("{error}")));
            let _deferred = async {
                if result.is_ok() {
                    self.spawn_policy.set(policy.clone()).await;
                }
            };
            let _ = reply_tx.send(result);
        }
        _ => {}
    }
}
"#,
    )
    .expect("write deferred side-effect proof actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::SetSpawnPolicy")
                && mismatch.contains("MobMachineInput::SetSpawnPolicy")
        }),
        "expected SetSpawnPolicy side effect hidden in deferred async block to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_ungated_command_arm_side_effects() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::RunFlow { reply_tx, .. } => {
            self.create_pending_run().await?;
            let _ = reply_tx.send(Ok(RunId::new()));
        }
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            self.spawn_policy.set(policy).await;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write ungated command arm actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::RunFlow")
                && mismatch.contains("MobMachineInput::RunFlow")
        }),
        "expected ungated RunFlow command-arm side effect to be rejected even when the handler gates, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_secondary_dispatch_arm_masking_real_arm() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn run(&mut self, command: MobCommand) {
    match command {
        MobCommand::RunFlow { .. } => {
            self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
        }
        _ => {}
    }
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::RunFlow { reply_tx, .. } => {
            let result = self.handle_run_flow().await;
            let _ = reply_tx.send(result);
        }
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            self.spawn_policy.set(policy).await;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write secondary dispatch masking actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::RunFlow")
                && mismatch.contains("MobMachineInput::RunFlow")
        }),
        "expected gated secondary RunFlow arm not to mask ungated real dispatch arm, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_imported_variant_ungated_arms() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
use crate::mob_machine::MobCommand::*;

async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        RunFlow { reply_tx, .. } => {
            self.create_pending_run().await?;
            let _ = reply_tx.send(Ok(RunId::new()));
        }
        SetSpawnPolicy { policy, reply_tx } => {
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            self.spawn_policy.set(policy).await;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write imported variant actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::RunFlow")
                && mismatch.contains("MobMachineInput::RunFlow")
        }),
        "expected imported RunFlow command arm to be checked independently, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_or_pattern_variant_gate_aliasing() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::CancelFlow { .. } | MobCommand::ForceCancel { .. } => {
            self.apply_dsl_input(MobMachineInput::CancelFlow, "cancel_flow")?;
        }
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            self.spawn_policy.set(policy).await;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write or-pattern command arm actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::ForceCancel")
                && mismatch.contains("MobMachineInput::ForceCancel")
        }),
        "expected shared CancelFlow | ForceCancel arm not to borrow CancelFlow gate proof for ForceCancel, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_block_local_variant_alias_arms() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    use crate::machines::mob_machine::MobCommand::RunFlow as Run;

    match command {
        Run { reply_tx, .. } => {
            self.create_pending_run().await?;
            let _ = reply_tx.send(Ok(RunId::new()));
        }
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            self.spawn_policy.set(policy).await;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write block-local variant alias actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::RunFlow")
                && mismatch.contains("MobMachineInput::RunFlow")
        }),
        "expected block-local RunFlow variant alias arm to be checked independently, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_variant_alias_chain_arms() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
use crate::machines::mob_machine::MobCommand::RunFlow as CanonicalRun;
use CanonicalRun as Run;

async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        Run { reply_tx, .. } => {
            self.create_pending_run().await?;
            let _ = reply_tx.send(Ok(RunId::new()));
        }
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            self.spawn_policy.set(policy).await;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write variant alias chain actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::RunFlow")
                && mismatch.contains("MobMachineInput::RunFlow")
        }),
        "expected RunFlow variant alias chain arm to be checked independently, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_at_pattern_command_arms() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        run @ MobCommand::RunFlow { reply_tx, .. } => {
            self.create_pending_run().await?;
            let _ = reply_tx.send(Ok(RunId::new()));
            let _ = run;
        }
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            self.spawn_policy.set(policy).await;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write at-pattern command arm actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::RunFlow")
                && mismatch.contains("MobMachineInput::RunFlow")
        }),
        "expected RunFlow @ pattern arm to be checked independently, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_guarded_command_arm_spoofing() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::RunFlow { .. } if false => {
            self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
        }
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            self.spawn_policy.set(policy).await;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write guarded command arm actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::RunFlow")
                && mismatch.contains("MobMachineInput::RunFlow")
        }),
        "expected guarded/dead RunFlow arm not to mask ungated RunFlow handler, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_shadowed_gate_input_alias() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
use crate::mob_machine::MobMachineInput::SetSpawnPolicy as Gate;

async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            use crate::mob_machine::MobMachineInput::RunFlow as Gate;

            let result = self.apply_dsl_input(Gate, "set_spawn_policy").map_err(Into::into);
            if result.is_ok() {
                self.spawn_policy.set(policy).await;
            }
            let _ = reply_tx.send(result);
        }
        _ => {}
    }
}
"#,
    )
    .expect("write shadowed gate input alias actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::SetSpawnPolicy")
                && mismatch.contains("MobMachineInput::SetSpawnPolicy")
        }),
        "expected block-local Gate alias shadowing to require SetSpawnPolicy authority, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_nested_shadowed_gate_input_alias() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
use crate::mob_machine::MobMachineInput::SetSpawnPolicy as Gate;

async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            let result = {
                use crate::mob_machine::MobMachineInput::RunFlow as Gate;
                self.apply_dsl_input(Gate, "set_spawn_policy").map_err(Into::into)
            };
            if result.is_ok() {
                self.spawn_policy.set(policy).await;
            }
            let _ = reply_tx.send(result);
        }
        _ => {}
    }
}
"#,
    )
    .expect("write nested shadowed gate input alias actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::SetSpawnPolicy")
                && mismatch.contains("MobMachineInput::SetSpawnPolicy")
        }),
        "expected nested block-local Gate alias shadowing to require SetSpawnPolicy authority, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_mutably_replaced_gate_input() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            let mut input = MobMachineInput::SetSpawnPolicy;
            std::mem::replace(&mut input, MobMachineInput::RunFlow);
            let result = self.apply_dsl_input(input, "set_spawn_policy").map_err(Into::into);
            if result.is_ok() {
                self.spawn_policy.set(policy).await;
            }
            let _ = reply_tx.send(result);
        }
        _ => {}
    }
}
"#,
    )
    .expect("write mutably replaced gate input actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::SetSpawnPolicy")
                && mismatch.contains("MobMachineInput::SetSpawnPolicy")
        }),
        "expected mutable input replacement to invalidate SetSpawnPolicy gate proof, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_autoref_mutated_gate_input() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            let mut input = MobMachineInput::SetSpawnPolicy;
            input.clone_from(&MobMachineInput::RunFlow);
            let result = self.apply_dsl_input(input, "set_spawn_policy").map_err(Into::into);
            if result.is_ok() {
                self.spawn_policy.set(policy).await;
            }
            let _ = reply_tx.send(result);
        }
        _ => {}
    }
}
"#,
    )
    .expect("write autoref-mutated gate input actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::SetSpawnPolicy")
                && mismatch.contains("MobMachineInput::SetSpawnPolicy")
        }),
        "expected autoref input mutation to invalidate SetSpawnPolicy gate proof, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_nested_block_autoref_mutated_gate_input() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            let mut input = MobMachineInput::SetSpawnPolicy;
            {
                input.clone_from(&MobMachineInput::RunFlow);
            }
            let result = self.apply_dsl_input(input, "set_spawn_policy").map_err(Into::into);
            if result.is_ok() {
                self.spawn_policy.set(policy).await;
            }
            let _ = reply_tx.send(result);
        }
        _ => {}
    }
}
"#,
    )
    .expect("write nested-block autoref-mutated gate input actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::SetSpawnPolicy")
                && mismatch.contains("MobMachineInput::SetSpawnPolicy")
        }),
        "expected nested block autoref input mutation to invalidate SetSpawnPolicy gate proof, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_loop_autoref_mutated_gate_input() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand, should_replace: bool) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            let mut input = MobMachineInput::SetSpawnPolicy;
            while should_replace {
                input.clone_from(&MobMachineInput::RunFlow);
                break;
            }
            let result = self.apply_dsl_input(input, "set_spawn_policy").map_err(Into::into);
            if result.is_ok() {
                self.spawn_policy.set(policy).await;
            }
            let _ = reply_tx.send(result);
        }
        _ => {}
    }
}
"#,
    )
    .expect("write loop autoref-mutated gate input actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::SetSpawnPolicy")
                && mismatch.contains("MobMachineInput::SetSpawnPolicy")
        }),
        "expected loop autoref input mutation to invalidate SetSpawnPolicy gate proof, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_mutably_replaced_gate_result() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            let mut result = self
                .apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")
                .map_err(Into::into);
            std::mem::replace(&mut result, Ok(()));
            if result.is_ok() {
                self.spawn_policy.set(policy).await;
            }
            let _ = reply_tx.send(result);
        }
        _ => {}
    }
}
"#,
    )
    .expect("write mutably replaced gate result actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::SetSpawnPolicy")
                && mismatch.contains("MobMachineInput::SetSpawnPolicy")
        }),
        "expected mutable result replacement to invalidate SetSpawnPolicy gate proof, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_mutably_replaced_reply_tx() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(
    &mut self,
    command: MobCommand,
    audit_tx: ReplyTx<Result<(), MobError>>,
) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            let result = self
                .apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")
                .map_err(Into::into);
            if result.is_ok() {
                self.spawn_policy.set(policy).await;
            }
            std::mem::replace(&mut reply_tx, audit_tx);
            let _ = reply_tx.send(result);
        }
        _ => {}
    }
}
"#,
    )
    .expect("write mutably replaced reply_tx actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::SetSpawnPolicy")
                && mismatch.contains("MobMachineInput::SetSpawnPolicy")
        }),
        "expected mutable reply_tx replacement to invalidate propagated reply proof, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_fail_open_gate_result_wrappers() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            let result = self
                .apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")
                .or_else(|_| Ok(()));
            if result.is_ok() {
                self.spawn_policy.set(policy).await;
            }
            let _ = reply_tx.send(result);
        }
        _ => {}
    }
}
"#,
    )
    .expect("write fail-open gate result wrapper actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::SetSpawnPolicy")
                && mismatch.contains("MobMachineInput::SetSpawnPolicy")
        }),
        "expected fail-open result wrapper not to count as a fail-closed gate, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_fail_open_try_gate_wrappers() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) -> Result<(), MobError> {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")
                .or_else(|_| Ok(()))?;
            self.spawn_policy.set(policy).await;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
    Ok(())
}
"#,
    )
    .expect("write fail-open try gate wrapper actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::SetSpawnPolicy")
                && mismatch.contains("MobMachineInput::SetSpawnPolicy")
        }),
        "expected fail-open try wrapper not to count as a fail-closed gate, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_ufcs_fail_open_try_gate_wrappers() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) -> Result<(), MobError> {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            Result::or_else(
                self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy"),
                |_| Ok(()),
            )?;
            self.spawn_policy.set(policy).await;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
    Ok(())
}
"#,
    )
    .expect("write UFCS fail-open try gate wrapper actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::SetSpawnPolicy")
                && mismatch.contains("MobMachineInput::SetSpawnPolicy")
        }),
        "expected UFCS fail-open try wrapper not to count as a fail-closed gate, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_catch_all_side_effect_arms() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn handle_set_spawn_policy(&mut self) -> Result<(), MobError> {
    self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        other => {
            self.create_pending_run().await?;
            let _ = other;
        }
    }
}
"#,
    )
    .expect("write catch-all side-effect actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::RunFlow")
                && mismatch.contains("MobMachineInput::RunFlow")
        }),
        "expected catch-all command side effect not to be hidden by handler fallback, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_fake_command_and_input_owners() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
mod fake {
    enum MobCommand {
        SetSpawnPolicy { policy: SpawnPolicy, reply_tx: ReplyTx<Result<(), MobError>> },
    }

    enum MobMachineInput {
        SetSpawnPolicy,
    }
}

async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: fake::MobCommand) {
    match command {
        fake::MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            let result = self
                .apply_dsl_input(fake::MobMachineInput::SetSpawnPolicy, "set_spawn_policy")
                .map_err(Into::into);
            if result.is_ok() {
                self.spawn_policy.set(policy).await;
            }
            let _ = reply_tx.send(result);
        }
    }
}
"#,
    )
    .expect("write fake command and input owners actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::SetSpawnPolicy")
                && mismatch.contains("MobMachineInput::SetSpawnPolicy")
        }),
        "expected fake MobCommand/MobMachineInput owners not to satisfy canonical command gate, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_fake_mob_machine_path_owners() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
mod fake {
    pub mod mob_machine {
        pub enum MobCommand {
            SetSpawnPolicy { policy: SpawnPolicy, reply_tx: ReplyTx<Result<(), MobError>> },
        }

        pub enum MobMachineInput {
            SetSpawnPolicy,
        }
    }
}

async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: fake::mob_machine::MobCommand) {
    match command {
        fake::mob_machine::MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            let result = self
                .apply_dsl_input(fake::mob_machine::MobMachineInput::SetSpawnPolicy, "set_spawn_policy")
                .map_err(Into::into);
            if result.is_ok() {
                self.spawn_policy.set(policy).await;
            }
            let _ = reply_tx.send(result);
        }
    }
}
"#,
    )
    .expect("write fake mob_machine command and input owners actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::SetSpawnPolicy")
                && mismatch.contains("MobMachineInput::SetSpawnPolicy")
        }),
        "expected fake mob_machine owner path not to satisfy canonical command gate, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_crate_qualified_fake_mob_machine_path_owners() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
mod fake {
    pub mod mob_machine {
        pub enum MobCommand {
            SetSpawnPolicy { policy: SpawnPolicy, reply_tx: ReplyTx<Result<(), MobError>> },
        }

        pub enum MobMachineInput {
            SetSpawnPolicy,
        }
    }
}

async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: crate::fake::mob_machine::MobCommand) {
    match command {
        crate::fake::mob_machine::MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            let result = self
                .apply_dsl_input(crate::fake::mob_machine::MobMachineInput::SetSpawnPolicy, "set_spawn_policy")
                .map_err(Into::into);
            if result.is_ok() {
                self.spawn_policy.set(policy).await;
            }
            let _ = reply_tx.send(result);
        }
    }
}
"#,
    )
    .expect("write crate-qualified fake mob_machine command and input owners actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::SetSpawnPolicy")
                && mismatch.contains("MobMachineInput::SetSpawnPolicy")
        }),
        "expected crate-qualified fake mob_machine owner path not to satisfy canonical command gate, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_shadowed_command_scrutinee() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand, fake: MobCommand) {
    let command = fake;
    match command {
        MobCommand::RunFlow { .. } => {
            self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
        }
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            self.spawn_policy.set(policy).await;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write shadowed command scrutinee actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::RunFlow")
                && mismatch.contains("MobMachineInput::RunFlow")
        }),
        "expected shadowed command scrutinee arm not to mask ungated RunFlow handler, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_command_scrutinee_aliases() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn handle_set_spawn_policy(&mut self) -> Result<(), MobError> {
    self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    let cmd = command;
    match cmd {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            self.spawn_policy.set(policy).await;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write command scrutinee alias actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::SetSpawnPolicy")
                && mismatch.contains("MobMachineInput::SetSpawnPolicy")
        }),
        "expected command scrutinee alias arm to require its own gate proof, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_reassigned_command_scrutinee_aliases() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn handle_set_spawn_policy(&mut self) -> Result<(), MobError> {
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand, fake: MobCommand) {
    let mut cmd = command;
    cmd = fake;
    match cmd {
        MobCommand::SetSpawnPolicy { reply_tx, .. } => {
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write reassigned command scrutinee alias actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::SetSpawnPolicy")
                && mismatch.contains("MobMachineInput::SetSpawnPolicy")
        }),
        "expected reassigned command scrutinee alias arm to require its own gate proof, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_same_name_handler_spoofing() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::RunFlow { reply_tx, .. } => {
            let result = self.handle_run_flow().await;
            let _ = reply_tx.send(result);
        }
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            self.spawn_policy.set(policy).await;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}

mod fake {
    async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
        self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
        Ok(RunId::new())
    }
}
"#,
    )
    .expect("write same-name handler spoof actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::RunFlow")
                && mismatch.contains("MobMachineInput::RunFlow")
        }),
        "expected nested same-name handler not to satisfy RunFlow propagation, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_fake_command_arm_sources() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn preview_fake_command(&mut self, fake: FakeCommand) {
    match fake {
        MobCommand::RunFlow { .. } => {
            self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
        }
        _ => {}
    }
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            self.spawn_policy.set(policy).await;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write fake command-arm source actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::RunFlow")
                && mismatch.contains("MobMachineInput::RunFlow")
        }),
        "expected fake RunFlow command-arm source to be ignored and the ungated handler rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_glob_imported_fake_command_and_input_owners() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
use crate::machines::mob_machine as mob_dsl;

mod fake {
    pub enum MobCommand {
        SetSpawnPolicy {
            policy: SpawnPolicy,
            reply_tx: ReplyTx<Result<(), MobError>>,
        },
    }

    pub enum MobMachineInput {
        SetSpawnPolicy,
    }
}

use fake::*;

async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(mob_dsl::MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(mob_dsl::MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(mob_dsl::MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(mob_dsl::MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(mob_dsl::MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            self.spawn_policy.set(policy).await;
            let _ = reply_tx.send(Ok(()));
        }
    }
}
"#,
    )
    .expect("write glob-imported fake command/input owners actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::SetSpawnPolicy")
                && mismatch.contains("MobMachineInput::SetSpawnPolicy")
        }),
        "expected glob-imported fake SetSpawnPolicy command/input owners to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_dispatch_side_effect_before_command_match() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    self.create_pending_run().await?;
    match command {
        MobCommand::RunFlow { reply_tx, .. } => {
            self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
            let _ = reply_tx.send(Ok(RunId::new()));
        }
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            self.spawn_policy.set(policy).await;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write dispatch side effect before command match actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::RunFlow")
                && mismatch.contains("MobMachineInput::RunFlow")
        }),
        "expected pre-match RunFlow side effect not to be hidden by a gated arm, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_dispatch_helper_side_effect_before_command_match()
 {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn pre_match_run_effect(&mut self) -> Result<(), MobError> {
    self.create_pending_run().await?;
    Ok(())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    let _ = self.pre_match_run_effect().await;
    match command {
        MobCommand::RunFlow { reply_tx, .. } => {
            self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
            let _ = reply_tx.send(Ok(RunId::new()));
        }
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            self.spawn_policy.set(policy).await;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write dispatch helper side effect before command match actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::RunFlow")
                && mismatch.contains("MobMachineInput::RunFlow")
        }),
        "expected pre-match helper RunFlow side effect not to be hidden by a gated arm, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_command_rx_helper_side_effect_before_command_match()
 {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn pre_match_run_effect(&mut self) -> Result<(), MobError> {
    self.create_pending_run().await?;
    Ok(())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

impl MobActor {
    async fn run(&mut self, command_rx: mpsc::Receiver<MobCommand>) -> Result<(), MobError> {
        let command = command_rx.recv().await?;
        let _ = self.pre_match_run_effect().await;
        match command {
            MobCommand::RunFlow { reply_tx, .. } => {
                self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
                let _ = reply_tx.send(Ok(RunId::new()));
            }
            MobCommand::SetSpawnPolicy { policy, reply_tx } => {
                self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
                self.spawn_policy.set(policy).await;
                let _ = reply_tx.send(Ok(()));
            }
            _ => {}
        }
        Ok(())
    }
}
"#,
    )
    .expect("write command_rx helper side effect before command match actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::RunFlow")
                && mismatch.contains("MobMachineInput::RunFlow")
        }),
        "expected command_rx pre-match helper RunFlow side effect not to be hidden by a gated arm, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_fake_actor_handler_fallback() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
impl FakeActor {
    async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
        self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
        Ok(RunId::new())
    }
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            self.spawn_policy.set(policy).await;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write fake actor handler fallback actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::RunFlow")
                && mismatch.contains("MobMachineInput::RunFlow")
        }),
        "expected non-MobActor handle_run_flow not to satisfy RunFlow fallback, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_private_catalog_command_input_owners() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
use crate::machines::mob_machine as mob_dsl;
use crate::mob_machine::{MobCommand, MobMachineInput};

async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(mob_dsl::MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(mob_dsl::MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(mob_dsl::MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(mob_dsl::MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(mob_dsl::MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            self.spawn_policy.set(policy).await;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write private catalog owner command/input actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::SetSpawnPolicy")
                && mismatch.contains("MobMachineInput::SetSpawnPolicy")
        }),
        "expected private crate::mob_machine command/input owners not to satisfy runtime command gate, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_ignored_helper_gates() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn run_flow_gate(&mut self) -> Result<(), MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(())
}

async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    let _ = self.run_flow_gate().await;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::RunFlow { reply_tx, .. } => {
            let result = self.handle_run_flow().await;
            let _ = reply_tx.send(result);
        }
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            self.spawn_policy.set(policy).await;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write ignored helper gate actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::RunFlow")
                && mismatch.contains("MobMachineInput::RunFlow")
        }),
        "expected ignored helper RunFlow gate to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_loop_gate_leaks() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand, should_gate: bool) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            while should_gate {
                self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
                break;
            }
            self.spawn_policy.set(policy).await;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write loop gate leak actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::SetSpawnPolicy")
                && mismatch.contains("MobMachineInput::SetSpawnPolicy")
        }),
        "expected loop-local SetSpawnPolicy gate to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_loop_side_effect_before_gate() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand, should_apply: bool) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            while should_apply {
                self.spawn_policy.set(policy.clone()).await;
                break;
            }
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write loop side effect before gate actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::SetSpawnPolicy")
                && mismatch.contains("MobMachineInput::SetSpawnPolicy")
        }),
        "expected loop-local SetSpawnPolicy side effect before gate to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_wrong_input_gate_before_side_effect() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.apply_dsl_input(MobMachineInput::TaskCreate { task_id }, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
            self.spawn_policy.set(policy.clone()).await;
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write wrong input gate before side effect actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::SetSpawnPolicy")
                && mismatch.contains("MobMachineInput::SetSpawnPolicy")
        }),
        "expected SetSpawnPolicy side effect after wrong input gate to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_loop_pending_gate_result_leaks() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.apply_dsl_input(MobMachineInput::TaskCreate { task_id }, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand, should_gate: bool) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            let mut result = Ok(());
            while should_gate {
                result = self
                    .apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")
                    .map_err(|error| MobError::Internal(format!("{error}")));
                break;
            }
            if result.is_ok() {
                self.spawn_policy.set(policy.clone()).await;
            }
            let _ = reply_tx.send(result);
        }
        _ => {}
    }
}
"#,
    )
    .expect("write loop pending gate result leak actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::SetSpawnPolicy")
                && mismatch.contains("MobMachineInput::SetSpawnPolicy")
        }),
        "expected loop-local SetSpawnPolicy pending gate result not to authorize side effects, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_bare_shadowed_mob_machine_input() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
enum MobMachineInput {
    SetSpawnPolicy,
}

async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(crate::mob_machine::MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(crate::mob_machine::MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.apply_dsl_input(crate::mob_machine::MobMachineInput::TaskCreate { task_id }, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.apply_dsl_input(crate::mob_machine::MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: crate::mob_machine::MobCommand) {
    match command {
        crate::mob_machine::MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            let result = self
                .apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")
                .map_err(|error| MobError::Internal(format!("{error}")));
            if result.is_ok() {
                self.spawn_policy.set(policy.clone()).await;
            }
            let _ = reply_tx.send(result);
        }
        _ => {}
    }
}
"#,
    )
    .expect("write bare shadowed MobMachineInput actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::SetSpawnPolicy")
                && mismatch.contains("MobMachineInput::SetSpawnPolicy")
        }),
        "expected bare shadowed MobMachineInput gate not to authorize SetSpawnPolicy, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_bare_shadowed_mob_command_arm() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
enum MobCommand {
    SetSpawnPolicy { policy: SpawnPolicy, reply_tx: ReplyTx<Result<(), MobError>> },
}

async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(crate::mob_machine::MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(crate::mob_machine::MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.apply_dsl_input(crate::mob_machine::MobMachineInput::TaskCreate { task_id }, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.apply_dsl_input(crate::mob_machine::MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: crate::mob_machine::MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            let result = self
                .apply_dsl_input(crate::mob_machine::MobMachineInput::SetSpawnPolicy, "set_spawn_policy")
                .map_err(|error| MobError::Internal(format!("{error}")));
            if result.is_ok() {
                self.spawn_policy.set(policy.clone()).await;
            }
            let _ = reply_tx.send(result);
        }
        _ => {}
    }
}
"#,
    )
    .expect("write bare shadowed MobCommand actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::SetSpawnPolicy")
                && mismatch.contains("MobMachineInput::SetSpawnPolicy")
        }),
        "expected bare shadowed MobCommand arm not to satisfy SetSpawnPolicy command gate, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_task_create_wrapper_before_gate() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self, draft: NewTask) -> Result<TaskId, MobError> {
    let task = self.create_task(draft).await?;
    self.apply_dsl_input(MobMachineInput::TaskCreate { task_id: task.id }, "task_create")?;
    Ok(task.id)
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            self.spawn_policy.set(policy.clone()).await;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write task create wrapper before gate actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::TaskCreate")
                && mismatch.contains("MobMachineInput::TaskCreate")
        }),
        "expected TaskCreate create_task side effect before gate to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_task_service_create_with_id_before_gate() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self, task_id: TaskId) -> Result<TaskId, MobError> {
    self.task_board_service
        .create_task_with_id(task_id.clone(), subject, description, blocked_by)
        .await?;
    self.apply_dsl_input(MobMachineInput::TaskCreate { task_id: task_id.clone() }, "task_create")?;
    Ok(task_id)
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            self.spawn_policy.set(policy.clone()).await;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write task service create_task_with_id before gate actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::TaskCreate")
                && mismatch.contains("MobMachineInput::TaskCreate")
        }),
        "expected TaskCreate service create_task_with_id side effect before gate to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_aliased_task_service_create_with_id_before_gate()
 {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self, task_id: TaskId) -> Result<TaskId, MobError> {
    let task_board = self.task_board_service.clone();
    task_board
        .create_task_with_id(task_id.clone(), subject, description, blocked_by)
        .await?;
    self.apply_dsl_input(MobMachineInput::TaskCreate { task_id: task_id.clone() }, "task_create")?;
    Ok(task_id)
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            self.spawn_policy.set(policy.clone()).await;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write aliased task service create_task_with_id before gate actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::TaskCreate")
                && mismatch.contains("MobMachineInput::TaskCreate")
        }),
        "expected aliased TaskCreate create_task_with_id side effect before gate to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_force_cancel_interrupt_member_before_gate() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.apply_dsl_input(MobMachineInput::TaskCreate { task_id }, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self, member_ref: MemberRef) -> Result<(), MobError> {
    self.provisioner.interrupt_member(&member_ref).await?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            self.spawn_policy.set(policy.clone()).await;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write force cancel interrupt member before gate actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::ForceCancel")
                && mismatch.contains("MobMachineInput::ForceCancel")
        }),
        "expected ForceCancel interrupt_member side effect before gate to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_qualified_mob_command_arm_before_gate() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.apply_dsl_input(MobMachineInput::TaskCreate { task_id }, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn handle_set_spawn_policy(&mut self) -> Result<(), MobError> {
    self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
    Ok(())
}

async fn dispatch(&mut self, command: super::state::MobCommand) {
    match command {
        super::state::MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            self.spawn_policy.set(policy.clone()).await;
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write qualified MobCommand arm before gate actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::SetSpawnPolicy")
                && mismatch.contains("MobMachineInput::SetSpawnPolicy")
        }),
        "expected qualified MobCommand arm side effect before gate to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_ufcs_task_create_before_gate() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self, task_id: TaskId) -> Result<TaskId, MobError> {
    TaskBoardService::create_task_with_id(
        &self.task_board_service,
        task_id.clone(),
        subject,
        description,
        blocked_by,
    )
    .await?;
    self.apply_dsl_input(MobMachineInput::TaskCreate { task_id: task_id.clone() }, "task_create")?;
    Ok(task_id)
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            self.spawn_policy.set(policy.clone()).await;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write UFCS task create before gate actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::TaskCreate")
                && mismatch.contains("MobMachineInput::TaskCreate")
        }),
        "expected UFCS TaskCreate side effect before gate to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_ufcs_cancel_before_gate() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self, cancel_token: CancellationToken) -> Result<(), MobError> {
    CancellationToken::cancel(&cancel_token);
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.apply_dsl_input(MobMachineInput::TaskCreate { task_id }, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            self.spawn_policy.set(policy.clone()).await;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write UFCS cancel before gate actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::CancelFlow")
                && mismatch.contains("MobMachineInput::CancelFlow")
        }),
        "expected UFCS CancelFlow side effect before gate to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_ordered_helper_side_effect_before_gate() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn a_task_create_gate(&mut self, task_id: TaskId) -> Result<(), MobError> {
    self.apply_dsl_input(MobMachineInput::TaskCreate { task_id }, "task_create")?;
    Ok(())
}

async fn z_task_create_side_effect(&mut self, task_id: TaskId) -> Result<(), MobError> {
    self.task_board_service
        .create_task_with_id(task_id.clone(), subject, description, blocked_by)
        .await?;
    Ok(())
}

async fn handle_task_create(&mut self, task_id: TaskId) -> Result<TaskId, MobError> {
    self.z_task_create_side_effect(task_id.clone()).await?;
    self.a_task_create_gate(task_id.clone()).await?;
    Ok(task_id)
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            self.spawn_policy.set(policy.clone()).await;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write ordered helper side effect before gate actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::TaskCreate")
                && mismatch.contains("MobMachineInput::TaskCreate")
        }),
        "expected helper side effect before later gate helper to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_ignored_helper_side_effect_before_gate() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn a_task_create_gate(&mut self, task_id: TaskId) -> Result<(), MobError> {
    self.apply_dsl_input(MobMachineInput::TaskCreate { task_id }, "task_create")?;
    Ok(())
}

async fn z_task_create_side_effect(&mut self, task_id: TaskId) -> Result<(), MobError> {
    self.task_board_service
        .create_task_with_id(task_id.clone(), subject, description, blocked_by)
        .await?;
    Ok(())
}

async fn handle_task_create(&mut self, task_id: TaskId) -> Result<TaskId, MobError> {
    let _ = self.z_task_create_side_effect(task_id.clone()).await;
    self.a_task_create_gate(task_id.clone()).await?;
    Ok(task_id)
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            self.spawn_policy.set(policy.clone()).await;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write ignored helper side effect before gate actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::TaskCreate")
                && mismatch.contains("MobMachineInput::TaskCreate")
        }),
        "expected ignored helper side effect before later gate helper to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_fake_mob_run_run_flow_input() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
mod fake {
    pub struct MobRun;

    impl MobRun {
        pub fn run_flow_input(_run_id: &RunId, _config: &FlowRunConfig) -> Result<MobMachineInput, MobError> {
            Ok(MobMachineInput::CancelFlow)
        }
    }
}

async fn handle_run_flow(
    &mut self,
    flow_id: FlowId,
    activation_params: serde_json::Value,
) -> Result<RunId, MobError> {
    let run_id = RunId::new();
    let config = FlowRunConfig::from_definition(flow_id, &self.definition)?;
    let run_flow = fake::MobRun::run_flow_input(&run_id, &config)?;
    let prepared_run_flow = self.prepare_dsl_input(run_flow.clone(), "run_flow")?;
    self.create_pending_run(
        run_id.clone(),
        &config,
        &prepared_run_flow.authority.state,
        activation_params.clone(),
        vec![run_flow],
    )
    .await?;
    self.commit_prepared_dsl_input(prepared_run_flow);
    Ok(run_id)
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.apply_dsl_input(MobMachineInput::TaskCreate { task_id }, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            self.spawn_policy.set(policy.clone()).await;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write fake MobRun run_flow_input actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::RunFlow")
                && mismatch.contains("MobMachineInput::RunFlow")
        }),
        "expected fake MobRun::run_flow_input provenance to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_glob_imported_fake_mob_run_run_flow_input() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
mod fake {
    pub struct MobRun;

    impl MobRun {
        pub fn run_flow_input(_run_id: &RunId, _config: &FlowRunConfig) -> Result<MobMachineInput, MobError> {
            Ok(MobMachineInput::CancelFlow)
        }
    }
}

use fake::*;

async fn handle_run_flow(
    &mut self,
    flow_id: FlowId,
    activation_params: serde_json::Value,
) -> Result<RunId, MobError> {
    let run_id = RunId::new();
    let config = FlowRunConfig::from_definition(flow_id, &self.definition)?;
    let run_flow = MobRun::run_flow_input(&run_id, &config)?;
    let prepared_run_flow = self.prepare_dsl_input(run_flow.clone(), "run_flow")?;
    self.create_pending_run(
        run_id.clone(),
        &config,
        &prepared_run_flow.authority.state,
        activation_params.clone(),
        vec![run_flow],
    )
    .await?;
    self.commit_prepared_dsl_input(prepared_run_flow);
    Ok(run_id)
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.apply_dsl_input(MobMachineInput::TaskCreate { task_id }, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            self.spawn_policy.set(policy.clone()).await;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write glob imported fake MobRun run_flow_input actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::RunFlow")
                && mismatch.contains("MobMachineInput::RunFlow")
        }),
        "expected glob-imported fake MobRun::run_flow_input provenance to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_run_flow_tracker_insert_before_gate() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    let run_id = RunId::new();
    let cancel_token = tokio_util::sync::CancellationToken::new();
    self.run_cancel_tokens.insert(run_id.clone(), (cancel_token.clone(), flow_id.clone()));
    self.flow_streams.lock().await.insert(run_id.clone(), scoped_event_tx);
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(run_id)
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.apply_dsl_input(MobMachineInput::TaskCreate { task_id }, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            self.spawn_policy.set(policy.clone()).await;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write run flow tracker inserts before gate actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::RunFlow")
                && mismatch.contains("MobMachineInput::RunFlow")
        }),
        "expected RunFlow tracker inserts before gate to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_run_flow_task_spawn_before_gate() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    let run_id = RunId::new();
    let handle = tokio::spawn(async move {
        let _ = run_id.clone();
    });
    self.run_tasks.insert(run_id.clone(), handle);
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(run_id)
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.apply_dsl_input(MobMachineInput::TaskCreate { task_id }, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            self.spawn_policy.set(policy.clone()).await;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write run flow task spawn before gate actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::RunFlow")
                && mismatch.contains("MobMachineInput::RunFlow")
        }),
        "expected RunFlow task spawn before gate to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_cancel_flow_cleanup_before_gate() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self, run_id: RunId) -> Result<(), MobError> {
    self.flow_streams.lock().await.remove(&run_id);
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.apply_dsl_input(MobMachineInput::TaskCreate { task_id }, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            self.spawn_policy.set(policy.clone()).await;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write cancel flow cleanup before gate actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::CancelFlow")
                && mismatch.contains("MobMachineInput::CancelFlow")
        }),
        "expected CancelFlow cleanup before gate to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_cancel_flow_token_cancel_before_gate() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self, run_id: RunId) -> Result<(), MobError> {
    let Some((cancel_token, _flow_id)) = self
        .run_cancel_tokens
        .get(&run_id)
        .map(|(token, flow_id)| (token.clone(), flow_id.clone()))
    else {
        return Ok(());
    };
    cancel_token.cancel();
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.apply_dsl_input(MobMachineInput::TaskCreate { task_id }, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            self.spawn_policy.set(policy.clone()).await;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write cancel flow token cancel before gate actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::CancelFlow")
                && mismatch.contains("MobMachineInput::CancelFlow")
        }),
        "expected CancelFlow token cancel before gate to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_cancel_flow_engine_cancel_before_gate() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self, run_id: RunId) -> Result<(), MobError> {
    let flow_engine = self.flow_engine.clone();
    flow_engine.cancel_unfinished_steps(&run_id).await?;
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.apply_dsl_input(MobMachineInput::TaskCreate { task_id }, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            self.spawn_policy.set(policy.clone()).await;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write cancel flow engine cancel before gate actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::CancelFlow")
                && mismatch.contains("MobMachineInput::CancelFlow")
        }),
        "expected CancelFlow engine cancel before gate to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_uncalled_mob_command_helpers() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn preview_command(&mut self, command: MobCommand) {
    match command {
        MobCommand::RunFlow { .. } => {
            self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
        }
        _ => {}
    }
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            self.spawn_policy.set(policy).await;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write uncalled MobCommand helper actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::RunFlow")
                && mismatch.contains("MobMachineInput::RunFlow")
        }),
        "expected uncalled MobCommand helper arm to be ignored and the ungated RunFlow handler rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_uncalled_run_command_helpers() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn run(&mut self, command: MobCommand) {
    match command {
        MobCommand::RunFlow { .. } => {
            self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
        }
        _ => {}
    }
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            self.spawn_policy.set(policy).await;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write uncalled run MobCommand helper actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::RunFlow")
                && mismatch.contains("MobMachineInput::RunFlow")
        }),
        "expected uncalled run(command: MobCommand) helper arm to be ignored and the ungated RunFlow handler rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_command_arm_ignored_handler_results() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::RunFlow { reply_tx, .. } => {
            let _ = self.handle_run_flow().await;
            let _ = reply_tx.send(Ok(RunId::new()));
        }
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            self.spawn_policy.set(policy).await;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write ignored handler result command arm actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::RunFlow")
                && mismatch.contains("MobMachineInput::RunFlow")
        }),
        "expected command arm that ignores the gated handler result to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_conditional_handler_result_propagation() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand, should_propagate: bool) {
    match command {
        MobCommand::RunFlow { reply_tx, .. } => {
            if should_propagate {
                let result = self.handle_run_flow().await;
                let _ = reply_tx.send(result);
            }
            let _ = reply_tx.send(Ok(RunId::new()));
        }
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            self.spawn_policy.set(policy).await;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write conditional handler result propagation actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::RunFlow")
                && mismatch.contains("MobMachineInput::RunFlow")
        }),
        "expected conditional RunFlow handler-result propagation to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_conditional_gate_only_handlers() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self, should_gate: bool) -> Result<RunId, MobError> {
    if should_gate {
        self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
    }
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            self.spawn_policy.set(policy).await;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write conditional gate-only handler actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::RunFlow")
                && mismatch.contains("MobMachineInput::RunFlow")
        }),
        "expected conditional-only RunFlow gate to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn mob_runtime_catalog_command_gate_ratchet_rejects_deferred_gate_only_handlers() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
    let gate = || -> Result<(), MobError> {
        self.apply_dsl_input(MobMachineInput::RunFlow, "run_flow")?;
        Ok(())
    };
    let _ = gate;
    Ok(RunId::new())
}

async fn handle_cancel_flow(&mut self) -> Result<(), MobError> {
    self.apply_command_admission(MobMachineInput::CancelFlow, MobState::Running, "cancel_flow")?;
    Ok(())
}

async fn handle_task_create(&mut self) -> Result<TaskId, MobError> {
    self.prepare_command_admission(MobMachineInput::TaskCreate { task_id }, MobState::Running, "task_create")?;
    Ok(TaskId::new())
}

async fn handle_force_cancel(&mut self) -> Result<(), MobError> {
    self.preview_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    self.apply_dsl_input(MobMachineInput::ForceCancel, "force_cancel")?;
    Ok(())
}

async fn dispatch(&mut self, command: MobCommand) {
    match command {
        MobCommand::SetSpawnPolicy { policy, reply_tx } => {
            self.apply_dsl_input(MobMachineInput::SetSpawnPolicy, "set_spawn_policy")?;
            self.spawn_policy.set(policy).await;
            let _ = reply_tx.send(Ok(()));
        }
        _ => {}
    }
}
"#,
    )
    .expect("write deferred gate-only handler actor");

    let mismatches = collect_mob_runtime_catalog_command_gate_mismatches(dir.path())
        .expect("catalog command gate mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("MobCommand::RunFlow")
                && mismatch.contains("MobMachineInput::RunFlow")
        }),
        "expected deferred-only RunFlow gate to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_reducer_transition_ratchet_accepts_typed_authority_wrappers() {
    let dir = tempdir().expect("tempdir");
    let run = dir.path().join("meerkat-mob/src/run.rs");
    fs::create_dir_all(run.parent().expect("run parent")).expect("create run dir");
    fs::write(
        &run,
        r"
pub(crate) fn apply_mob_machine_flow_run_command(
    state: &flow_run::State,
    command: MobMachineFlowRunCommand,
    authority: MobMachineFlowAuthorityToken,
) -> Result<flow_run::Outcome, MobError> {
    authority.require(MobMachineFlowAuthorityKind::FlowRun(command.kind()))?;
    flow_run::transition(state, command.into_input(), &flow_run::EmptyContext)
}

impl MobMachineFlowRunCommand {
    fn into_input(self) -> flow_run::Input {
        match self {
            Self::StartRun(payload) => flow_run::Input::StartRun(payload),
        }
    }
}
",
    )
    .expect("write typed wrapper");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow reducer transition mismatches");
    assert!(
        mismatches.is_empty(),
        "expected typed MobMachine authority wrapper and command-to-input adapter to be accepted, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_reducer_transition_ratchet_accepts_typed_authority_wrappers_with_module_declarations()
 {
    let dir = tempdir().expect("tempdir");
    let run = dir.path().join("meerkat-mob/src/run.rs");
    fs::create_dir_all(run.parent().expect("run parent")).expect("create run dir");
    fs::write(
        &run,
        r"
pub mod flow_frame;
pub mod flow_run;
pub mod loop_iteration;

pub(crate) fn apply_mob_machine_flow_run_command(
    state: &flow_run::State,
    command: MobMachineFlowRunCommand,
    authority: MobMachineFlowAuthorityToken,
) -> Result<flow_run::Outcome, MobError> {
    authority.require(MobMachineFlowAuthorityKind::FlowRun(command.kind()))?;
    flow_run::transition(state, command.into_input(), &flow_run::EmptyContext)
}

impl MobMachineFlowRunCommand {
    fn into_input(self) -> flow_run::Input {
        match self {
            Self::StartRun(payload) => flow_run::Input::StartRun(payload),
        }
    }
}
",
    )
    .expect("write typed wrapper with module declarations");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow reducer transition mismatches");
    assert!(
        mismatches.is_empty(),
        "expected typed MobMachine authority wrapper with reducer module declarations to be accepted, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_reducer_transition_ratchet_rejects_comment_spoofed_authority_wrapper() {
    let dir = tempdir().expect("tempdir");
    let run = dir.path().join("meerkat-mob/src/run.rs");
    fs::create_dir_all(run.parent().expect("run parent")).expect("create run dir");
    fs::write(
        &run,
        r"
pub(crate) fn apply_mob_machine_flow_run_command(
    state: &flow_run::State,
    command: MobMachineFlowRunCommand,
    authority: MobMachineFlowAuthorityToken,
) -> Result<flow_run::Outcome, MobError> {
    let _ = authority;
    // authority.require(MobMachineFlowAuthorityKind::FlowRun(command.kind()))?;
    flow_run::transition(state, command.into_input(), &flow_run::EmptyContext)
}

impl MobMachineFlowRunCommand {
    fn into_input(self) -> flow_run::Input {
        match self {
            Self::StartRun(payload) => flow_run::Input::StartRun(payload),
        }
    }
}
",
    )
    .expect("write comment-spoofed authority wrapper");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow reducer transition mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/run.rs")
                && mismatch.contains("flow_run::transition(")
        }),
        "expected comment-spoofed authority wrapper to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_reducer_transition_ratchet_rejects_nested_fake_authority_wrapper() {
    let dir = tempdir().expect("tempdir");
    let run = dir.path().join("meerkat-mob/src/run.rs");
    fs::create_dir_all(run.parent().expect("run parent")).expect("create run dir");
    fs::write(
        &run,
        r"
mod spoof {
    struct MobMachineFlowAuthorityToken;
    struct MobMachineFlowRunCommand;

    impl MobMachineFlowAuthorityToken {
        fn require(&self, _kind: MobMachineFlowAuthorityKind) -> Result<(), MobError> {
            Ok(())
        }
    }

    impl MobMachineFlowRunCommand {
        fn kind(&self) -> FlowRunReducerCommandKind {
            FlowRunReducerCommandKind::StartRun
        }

        fn into_input(self) -> flow_run::Input {
            flow_run::Input::StartRun(flow_run::inputs::StartRun {})
        }
    }

    pub(crate) fn apply_mob_machine_flow_run_command(
        state: &flow_run::State,
        command: MobMachineFlowRunCommand,
        authority: MobMachineFlowAuthorityToken,
    ) -> Result<flow_run::Outcome, MobError> {
        authority.require(MobMachineFlowAuthorityKind::FlowRun(command.kind()))?;
        flow_run::transition(state, command.into_input(), &flow_run::EmptyContext)
    }
}
",
    )
    .expect("write nested fake authority wrapper");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow reducer transition mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/run.rs")
                && mismatch.contains("flow_run::transition(")
        }),
        "expected nested fake reducer authority wrapper to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_reducer_transition_ratchet_rejects_fake_reducer_module_inside_authority_wrapper()
 {
    let dir = tempdir().expect("tempdir");
    let run = dir.path().join("meerkat-mob/src/run.rs");
    fs::create_dir_all(run.parent().expect("run parent")).expect("create run dir");
    fs::write(
        &run,
        r"
mod fake {
    pub mod flow_run {
        pub struct State;
        pub struct EmptyContext;
        pub enum Input {
            StartRun,
        }
        pub fn transition(
            _state: &State,
            _input: Input,
            _ctx: &EmptyContext,
        ) -> Result<Outcome, MobError> {
            Ok(Outcome)
        }
        pub struct Outcome;
    }
}

pub(crate) fn apply_mob_machine_flow_run_command(
    state: &fake::flow_run::State,
    command: MobMachineFlowRunCommand,
    authority: MobMachineFlowAuthorityToken,
) -> Result<fake::flow_run::Outcome, MobError> {
    authority.require(MobMachineFlowAuthorityKind::FlowRun(command.kind()))?;
    fake::flow_run::transition(state, command.into_input(), &fake::flow_run::EmptyContext)
}

impl MobMachineFlowRunCommand {
    fn into_input(self) -> fake::flow_run::Input {
        fake::flow_run::Input::StartRun
    }
}
",
    )
    .expect("write fake reducer module inside authority wrapper");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow reducer transition mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/run.rs")
                && mismatch.contains("fake::flow_run::transition(")
        }),
        "expected fake reducer module inside typed wrapper to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_reducer_transition_ratchet_rejects_block_local_aliases() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("block_alias.rs"),
        r"
fn bad(state: &flow_run::State, input: flow_run::Input) -> Result<flow_run::Outcome, MobError> {
    use crate::run::flow_run as fr;
    fr::transition(state, input, &flow_run::EmptyContext)
}
",
    )
    .expect("write block-local alias reducer use");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow reducer transition mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/block_alias.rs")
                && mismatch.contains("fr::transition(")
        }),
        "expected block-local reducer alias to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_reducer_transition_ratchet_rejects_parenthesized_transition_callees() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("paren_callee.rs"),
        r"
fn bad(state: &flow_run::State, input: flow_run::Input) -> Result<flow_run::Outcome, MobError> {
    (flow_run::transition)(state, input, &flow_run::EmptyContext)
}
",
    )
    .expect("write parenthesized reducer callee");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow reducer transition mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/paren_callee.rs")
                && mismatch.contains("flow_run::transition(")
        }),
        "expected parenthesized reducer callee to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_reducer_transition_ratchet_rejects_wrapped_authority_parameters() {
    let dir = tempdir().expect("tempdir");
    let run = dir.path().join("meerkat-mob/src/run.rs");
    fs::create_dir_all(run.parent().expect("run parent")).expect("create run dir");
    fs::write(
        &run,
        r"
pub(crate) fn apply_mob_machine_flow_run_command(
    state: &flow_run::State,
    command: Wrapper<MobMachineFlowRunCommand>,
    authority: Wrapper<MobMachineFlowAuthorityToken>,
) -> Result<flow_run::Outcome, MobError> {
    authority.require(MobMachineFlowAuthorityKind::FlowRun(command.kind()))?;
    flow_run::transition(state, command.into_input(), &flow_run::EmptyContext)
}
",
    )
    .expect("write wrapped authority parameters");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow reducer transition mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/run.rs")
                && mismatch.contains("flow_run::transition(")
        }),
        "expected wrapped reducer authority/command parameters to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_reducer_transition_ratchet_rejects_fake_authority_kind_constructors() {
    let dir = tempdir().expect("tempdir");
    let run = dir.path().join("meerkat-mob/src/run.rs");
    fs::create_dir_all(run.parent().expect("run parent")).expect("create run dir");
    fs::write(
        &run,
        r"
pub(crate) fn apply_mob_machine_flow_run_command(
    state: &flow_run::State,
    command: MobMachineFlowRunCommand,
    authority: MobMachineFlowAuthorityToken,
    hardcoded_kind: FlowRunReducerCommandKind,
) -> Result<flow_run::Outcome, MobError> {
    authority.require(MobMachineFlowAuthorityKind::FlowRun(fake_kind(command.kind(), hardcoded_kind)))?;
    flow_run::transition(state, command.into_input(), &flow_run::EmptyContext)
}

impl MobMachineFlowRunCommand {
    fn into_input(self) -> flow_run::Input {
        match self {
            Self::StartRun(payload) => flow_run::Input::StartRun(payload),
        }
    }
}
",
    )
    .expect("write fake authority kind constructor");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow reducer transition mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/run.rs")
                && mismatch.contains("flow_run::transition(")
        }),
        "expected fake reducer authority kind constructor to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_reducer_transition_ratchet_rejects_fake_authority_kind_owner_path() {
    let dir = tempdir().expect("tempdir");
    let run = dir.path().join("meerkat-mob/src/run.rs");
    fs::create_dir_all(run.parent().expect("run parent")).expect("create run dir");
    fs::write(
        &run,
        r"
mod fake {
    pub(crate) enum MobMachineFlowAuthorityKind {
        FlowRun(FlowRunReducerCommandKind),
    }
}

pub(crate) fn apply_mob_machine_flow_run_command(
    state: &flow_run::State,
    command: MobMachineFlowRunCommand,
    authority: MobMachineFlowAuthorityToken,
) -> Result<flow_run::Outcome, MobError> {
    authority.require(fake::MobMachineFlowAuthorityKind::FlowRun(command.kind()))?;
    flow_run::transition(state, command.into_input(), &flow_run::EmptyContext)
}

impl MobMachineFlowRunCommand {
    fn into_input(self) -> flow_run::Input {
        match self {
            Self::StartRun(payload) => flow_run::Input::StartRun(payload),
        }
    }
}
",
    )
    .expect("write fake authority kind owner path");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow reducer transition mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/run.rs")
                && mismatch.contains("flow_run::transition(")
        }),
        "expected fake reducer authority kind owner path to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_reducer_transition_ratchet_rejects_ambiguous_glob_authority_kind_owner() {
    let dir = tempdir().expect("tempdir");
    let run = dir.path().join("meerkat-mob/src/run.rs");
    fs::create_dir_all(run.parent().expect("run parent")).expect("create run dir");
    fs::write(
        &run,
        r"
mod fake {
    pub(crate) enum MobMachineFlowAuthorityKind {
        FlowRun(FlowRunReducerCommandKind),
    }
}
use fake::*;

pub(crate) fn apply_mob_machine_flow_run_command(
    state: &flow_run::State,
    command: MobMachineFlowRunCommand,
    authority: MobMachineFlowAuthorityToken,
) -> Result<flow_run::Outcome, MobError> {
    authority.require(MobMachineFlowAuthorityKind::FlowRun(command.kind()))?;
    flow_run::transition(state, command.into_input(), &flow_run::EmptyContext)
}

impl MobMachineFlowRunCommand {
    fn into_input(self) -> flow_run::Input {
        match self {
            Self::StartRun(payload) => flow_run::Input::StartRun(payload),
        }
    }
}
",
    )
    .expect("write ambiguous glob authority kind owner");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow reducer transition mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/run.rs")
                && mismatch.contains("flow_run::transition(")
        }),
        "expected ambiguous glob reducer authority kind owner to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_reducer_transition_ratchet_rejects_ignored_authority_require() {
    let dir = tempdir().expect("tempdir");
    let run = dir.path().join("meerkat-mob/src/run.rs");
    fs::create_dir_all(run.parent().expect("run parent")).expect("create run dir");
    fs::write(
        &run,
        r"
pub(crate) fn apply_mob_machine_flow_run_command(
    state: &flow_run::State,
    command: MobMachineFlowRunCommand,
    authority: MobMachineFlowAuthorityToken,
) -> Result<flow_run::Outcome, MobError> {
    let _ = authority.require(MobMachineFlowAuthorityKind::FlowRun(command.kind()));
    flow_run::transition(state, command.into_input(), &flow_run::EmptyContext)
}

impl MobMachineFlowRunCommand {
    fn into_input(self) -> flow_run::Input {
        match self {
            Self::StartRun(payload) => flow_run::Input::StartRun(payload),
        }
    }
}
",
    )
    .expect("write ignored authority wrapper");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow reducer transition mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/run.rs")
                && mismatch.contains("flow_run::transition(")
        }),
        "expected ignored authority requirement to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_reducer_transition_ratchet_rejects_nested_result_authority_require() {
    let dir = tempdir().expect("tempdir");
    let run = dir.path().join("meerkat-mob/src/run.rs");
    fs::create_dir_all(run.parent().expect("run parent")).expect("create run dir");
    fs::write(
        &run,
        r"
pub(crate) fn apply_mob_machine_flow_run_command(
    state: &flow_run::State,
    command: MobMachineFlowRunCommand,
    authority: MobMachineFlowAuthorityToken,
) -> Result<flow_run::Outcome, MobError> {
    Ok(authority.require(MobMachineFlowAuthorityKind::FlowRun(command.kind())))?;
    flow_run::transition(state, command.into_input(), &flow_run::EmptyContext)
}

impl MobMachineFlowRunCommand {
    fn into_input(self) -> flow_run::Input {
        match self {
            Self::StartRun(payload) => flow_run::Input::StartRun(payload),
        }
    }
}
",
    )
    .expect("write nested result authority wrapper");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow reducer transition mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/run.rs")
                && mismatch.contains("flow_run::transition(")
        }),
        "expected nested Result authority requirement not propagated by ? to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_reducer_transition_ratchet_rejects_fake_authority_receiver() {
    let dir = tempdir().expect("tempdir");
    let run = dir.path().join("meerkat-mob/src/run.rs");
    fs::create_dir_all(run.parent().expect("run parent")).expect("create run dir");
    fs::write(
        &run,
        r"
pub(crate) fn apply_mob_machine_flow_run_command(
    state: &flow_run::State,
    command: MobMachineFlowRunCommand,
    authority: MobMachineFlowAuthorityToken,
    fake: FakeAuthority,
) -> Result<flow_run::Outcome, MobError> {
    let _ = authority;
    fake.require(MobMachineFlowAuthorityKind::FlowRun(command.kind()))?;
    flow_run::transition(state, command.into_input(), &flow_run::EmptyContext)
}

impl MobMachineFlowRunCommand {
    fn into_input(self) -> flow_run::Input {
        match self {
            Self::StartRun(payload) => flow_run::Input::StartRun(payload),
        }
    }
}
",
    )
    .expect("write fake authority receiver wrapper");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow reducer transition mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/run.rs")
                && mismatch.contains("flow_run::transition(")
        }),
        "expected fake authority receiver to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_reducer_transition_ratchet_rejects_conditional_authority_require() {
    let dir = tempdir().expect("tempdir");
    let run = dir.path().join("meerkat-mob/src/run.rs");
    fs::create_dir_all(run.parent().expect("run parent")).expect("create run dir");
    fs::write(
        &run,
        r"
pub(crate) fn apply_mob_machine_flow_run_command(
    state: &flow_run::State,
    command: MobMachineFlowRunCommand,
    authority: MobMachineFlowAuthorityToken,
    should_require: bool,
) -> Result<flow_run::Outcome, MobError> {
    if should_require {
        authority.require(MobMachineFlowAuthorityKind::FlowRun(command.kind()))?;
    }
    flow_run::transition(state, command.into_input(), &flow_run::EmptyContext)
}

impl MobMachineFlowRunCommand {
    fn into_input(self) -> flow_run::Input {
        match self {
            Self::StartRun(payload) => flow_run::Input::StartRun(payload),
        }
    }
}
",
    )
    .expect("write conditional authority wrapper");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow reducer transition mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/run.rs")
                && mismatch.contains("flow_run::transition(")
        }),
        "expected conditional authority requirement to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_reducer_transition_ratchet_rejects_shadowed_authority_bindings() {
    let dir = tempdir().expect("tempdir");
    let run = dir.path().join("meerkat-mob/src/run.rs");
    fs::create_dir_all(run.parent().expect("run parent")).expect("create run dir");
    fs::write(
        &run,
        r"
pub(crate) fn apply_mob_machine_flow_run_command(
    state: &flow_run::State,
    command: MobMachineFlowRunCommand,
    authority: MobMachineFlowAuthorityToken,
    fake_command: FakeCommand,
    fake_authority: FakeAuthority,
) -> Result<flow_run::Outcome, MobError> {
    let authority = fake_authority;
    let command = fake_command;
    authority.require(MobMachineFlowAuthorityKind::FlowRun(command.kind()))?;
    flow_run::transition(state, command.into_input(), &flow_run::EmptyContext)
}

impl MobMachineFlowRunCommand {
    fn into_input(self) -> flow_run::Input {
        match self {
            Self::StartRun(payload) => flow_run::Input::StartRun(payload),
        }
    }
}
",
    )
    .expect("write shadowed authority bindings");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow reducer transition mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/run.rs")
                && mismatch.contains("flow_run::transition(")
        }),
        "expected shadowed reducer authority/command bindings to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_reducer_transition_ratchet_rejects_split_command_authority() {
    let dir = tempdir().expect("tempdir");
    let run = dir.path().join("meerkat-mob/src/run.rs");
    fs::create_dir_all(run.parent().expect("run parent")).expect("create run dir");
    fs::write(
        &run,
        r"
pub(crate) fn apply_mob_machine_flow_run_command(
    state: &flow_run::State,
    required_command: MobMachineFlowRunCommand,
    transitioned_command: MobMachineFlowRunCommand,
    authority: MobMachineFlowAuthorityToken,
) -> Result<flow_run::Outcome, MobError> {
    authority.require(MobMachineFlowAuthorityKind::FlowRun(required_command.kind()))?;
    flow_run::transition(state, transitioned_command.into_input(), &flow_run::EmptyContext)
}

impl MobMachineFlowRunCommand {
    fn into_input(self) -> flow_run::Input {
        match self {
            Self::StartRun(payload) => flow_run::Input::StartRun(payload),
        }
    }
}
",
    )
    .expect("write split command authority");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow reducer transition mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/run.rs")
                && mismatch.contains("flow_run::transition(")
        }),
        "expected transition input from a different command than the required command to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_reducer_transition_ratchet_rejects_deferred_authority_require() {
    let dir = tempdir().expect("tempdir");
    let run = dir.path().join("meerkat-mob/src/run.rs");
    fs::create_dir_all(run.parent().expect("run parent")).expect("create run dir");
    fs::write(
        &run,
        r"
pub(crate) fn apply_mob_machine_flow_run_command(
    state: &flow_run::State,
    command: MobMachineFlowRunCommand,
    authority: MobMachineFlowAuthorityToken,
) -> Result<flow_run::Outcome, MobError> {
    let require = || -> Result<(), MobError> {
        authority.require(MobMachineFlowAuthorityKind::FlowRun(command.kind()))?;
        Ok(())
    };
    let _ = require;
    flow_run::transition(state, command.into_input(), &flow_run::EmptyContext)
}

impl MobMachineFlowRunCommand {
    fn into_input(self) -> flow_run::Input {
        match self {
            Self::StartRun(payload) => flow_run::Input::StartRun(payload),
        }
    }
}
",
    )
    .expect("write deferred authority requirement");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow reducer transition mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/run.rs")
                && mismatch.contains("flow_run::transition(")
        }),
        "expected deferred reducer authority requirement to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_reducer_transition_ratchet_rejects_nested_helper_authority_require() {
    let dir = tempdir().expect("tempdir");
    let run = dir.path().join("meerkat-mob/src/run.rs");
    fs::create_dir_all(run.parent().expect("run parent")).expect("create run dir");
    fs::write(
        &run,
        r"
pub(crate) fn apply_mob_machine_flow_run_command(
    state: &flow_run::State,
    command: MobMachineFlowRunCommand,
    authority: MobMachineFlowAuthorityToken,
) -> Result<flow_run::Outcome, MobError> {
    fn never_called(
        command: MobMachineFlowRunCommand,
        authority: MobMachineFlowAuthorityToken,
    ) -> Result<(), MobError> {
        authority.require(MobMachineFlowAuthorityKind::FlowRun(command.kind()))?;
        Ok(())
    }
    let _ = never_called;
    flow_run::transition(state, command.into_input(), &flow_run::EmptyContext)
}

impl MobMachineFlowRunCommand {
    fn into_input(self) -> flow_run::Input {
        match self {
            Self::StartRun(payload) => flow_run::Input::StartRun(payload),
        }
    }
}
",
    )
    .expect("write nested helper authority requirement");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow reducer transition mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/run.rs")
                && mismatch.contains("flow_run::transition(")
        }),
        "expected nested uncalled reducer authority helper to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_reducer_transition_ratchet_rejects_short_circuit_authority_require() {
    let dir = tempdir().expect("tempdir");
    let run = dir.path().join("meerkat-mob/src/run.rs");
    fs::create_dir_all(run.parent().expect("run parent")).expect("create run dir");
    fs::write(
        &run,
        r"
pub(crate) fn apply_mob_machine_flow_run_command(
    state: &flow_run::State,
    command: MobMachineFlowRunCommand,
    authority: MobMachineFlowAuthorityToken,
    should_require: bool,
) -> Result<flow_run::Outcome, MobError> {
    let _ = should_require && {
        authority.require(MobMachineFlowAuthorityKind::FlowRun(command.kind()))?;
        true
    };
    flow_run::transition(state, command.into_input(), &flow_run::EmptyContext)
}

impl MobMachineFlowRunCommand {
    fn into_input(self) -> flow_run::Input {
        match self {
            Self::StartRun(payload) => flow_run::Input::StartRun(payload),
        }
    }
}
",
    )
    .expect("write short-circuit authority requirement");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow reducer transition mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/run.rs")
                && mismatch.contains("flow_run::transition(")
        }),
        "expected short-circuit reducer authority requirement to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_reducer_transition_ratchet_rejects_same_line_late_authority_require() {
    let dir = tempdir().expect("tempdir");
    let run = dir.path().join("meerkat-mob/src/run.rs");
    fs::create_dir_all(run.parent().expect("run parent")).expect("create run dir");
    fs::write(
        &run,
        r"
pub(crate) fn apply_mob_machine_flow_run_command(
    state: &flow_run::State,
    command: MobMachineFlowRunCommand,
    authority: MobMachineFlowAuthorityToken,
) -> Result<flow_run::Outcome, MobError> {
    let outcome = flow_run::transition(state, command.clone().into_input(), &flow_run::EmptyContext); authority.require(MobMachineFlowAuthorityKind::FlowRun(command.kind()))?; outcome
}

impl MobMachineFlowRunCommand {
    fn into_input(self) -> flow_run::Input {
        match self {
            Self::StartRun(payload) => flow_run::Input::StartRun(payload),
        }
    }
}
",
    )
    .expect("write same-line late authority requirement");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow reducer transition mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/run.rs")
                && mismatch.contains("flow_run::transition(")
        }),
        "expected same-line authority requirement after transition to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_reducer_transition_ratchet_rejects_untyped_transition_input() {
    let dir = tempdir().expect("tempdir");
    let run = dir.path().join("meerkat-mob/src/run.rs");
    fs::create_dir_all(run.parent().expect("run parent")).expect("create run dir");
    fs::write(
        &run,
        r"
pub(crate) fn apply_mob_machine_flow_run_command(
    state: &flow_run::State,
    command: MobMachineFlowRunCommand,
    authority: MobMachineFlowAuthorityToken,
    input: flow_run::Input,
) -> Result<flow_run::Outcome, MobError> {
    authority.require(MobMachineFlowAuthorityKind::FlowRun(command.kind()))?;
    flow_run::transition(state, input, &flow_run::EmptyContext)
}
",
    )
    .expect("write untyped transition input wrapper");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow reducer transition mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/run.rs")
                && mismatch.contains("flow_run::transition(")
        }),
        "expected untyped transition input to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_accepts_typed_outcome_commits() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
use crate::run::apply_mob_machine_flow_run_command;

async fn good(run_store: Arc<dyn crate::store::MobRunStore>, run_id: &RunId, machine_state: &MobMachineState, run: MobRun, command: MobMachineFlowRunCommand, authority: MobMachineFlowAuthorityToken) -> Result<(), MobError> {
    let authority_input = command.authority_input(run_id);
    let outcome = apply_mob_machine_flow_run_command(
        &run.flow_state,
        &machine_state,
        run_id,
        command,
        authority,
    )?;
    run_store
        .cas_flow_state_with_authority(run_id, &run.flow_state, &outcome.next_state, vec![authority_input])
        .await?;
    Ok(())
}
",
    )
    .expect("write typed outcome commit");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.is_empty(),
        "expected CAS of a typed MobMachine flow outcome to be accepted, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_raw_cas_after_typed_outcome() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
use crate::run::apply_mob_machine_flow_run_command;

async fn bad(run_store: Arc<dyn crate::store::MobRunStore>, run_id: &RunId, machine_state: &MobMachineState, run: MobRun, command: MobMachineFlowRunCommand, authority: MobMachineFlowAuthorityToken) -> Result<(), MobError> {
    let outcome = apply_mob_machine_flow_run_command(
        &run.flow_state,
        &machine_state,
        run_id,
        command,
        authority,
    )?;
    run_store
        .cas_flow_state(run_id, &run.flow_state, &outcome.next_state)
        .await?;
    Ok(())
}
",
    )
    .expect("write raw typed outcome commit");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected raw CAS of a typed MobMachine flow outcome to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_fake_receiver_with_typed_authority_cas() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
use crate::run::apply_mob_machine_flow_run_command;

async fn bad(fake_store: FakeRunStore, run_id: &RunId, machine_state: &MobMachineState, run: MobRun, command: MobMachineFlowRunCommand, authority: MobMachineFlowAuthorityToken) -> Result<(), MobError> {
    let authority_input = command.authority_input(run_id);
    let outcome = apply_mob_machine_flow_run_command(
        &run.flow_state,
        &machine_state,
        run_id,
        command,
        authority,
    )?;
    fake_store
        .cas_flow_state_with_authority(run_id, &run.flow_state, &outcome.next_state, vec![authority_input])
        .await?;
    Ok(())
}
",
    )
    .expect("write fake receiver typed authority commit");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".cas_flow_state_with_authority(")
        }),
        "expected typed authority CAS on a fake receiver to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_chained_fake_receiver_from_typed_run_store() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
use crate::run::apply_mob_machine_flow_run_command;

async fn bad(run_store: Arc<dyn crate::store::MobRunStore>, run_id: &RunId, machine_state: &MobMachineState, run: MobRun, command: MobMachineFlowRunCommand, authority: MobMachineFlowAuthorityToken) -> Result<(), MobError> {
    let authority_input = command.authority_input(run_id);
    let outcome = apply_mob_machine_flow_run_command(
        &run.flow_state,
        &machine_state,
        run_id,
        command,
        authority,
    )?;
    run_store
        .fake()
        .cas_flow_state_with_authority(run_id, &run.flow_state, &outcome.next_state, vec![authority_input])
        .await?;
    Ok(())
}
",
    )
    .expect("write chained fake receiver typed authority commit");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".cas_flow_state_with_authority(")
        }),
        "expected typed authority CAS through a chained fake receiver to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_fake_wrapper_parameter_containing_mob_run_store() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
use crate::run::apply_mob_machine_flow_run_command;

struct FakeStore<T>(T);

async fn bad(run_store: FakeStore<Arc<dyn crate::store::MobRunStore>>, run_id: &RunId, machine_state: &MobMachineState, run: MobRun, command: MobMachineFlowRunCommand, authority: MobMachineFlowAuthorityToken) -> Result<(), MobError> {
    let authority_input = command.authority_input(run_id);
    let outcome = apply_mob_machine_flow_run_command(
        &run.flow_state,
        &machine_state,
        run_id,
        command,
        authority,
    )?;
    run_store
        .cas_flow_state_with_authority(run_id, &run.flow_state, &outcome.next_state, vec![authority_input])
        .await?;
    Ok(())
}
",
    )
    .expect("write fake wrapper parameter typed authority commit");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".cas_flow_state_with_authority(")
        }),
        "expected typed authority CAS on a fake wrapper receiver to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_top_level_fake_apply_outcomes() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
fn apply_mob_machine_flow_run_command(
    _state: &flow_run::State,
    _command: MobMachineFlowRunCommand,
    _authority: MobMachineFlowAuthorityToken,
) -> Result<FakeOutcome, MobError> {
    Ok(FakeOutcome {
        next_state: flow_run::State::default(),
    })
}

async fn bad(run_store: Store, run_id: &RunId, run: MobRun, command: MobMachineFlowRunCommand, authority: MobMachineFlowAuthorityToken) -> Result<(), MobError> {
    let outcome = apply_mob_machine_flow_run_command(&run.flow_state, command, authority)?;
    run_store
        .cas_flow_state(run_id, &run.flow_state, &outcome.next_state)
        .await?;
    Ok(())
}
",
    )
    .expect("write top-level fake apply outcome");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected top-level fake flow-apply outcome to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_fake_run_module_apply_outcomes() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
async fn bad(run_store: Store, run_id: &RunId, run: MobRun, command: MobMachineFlowRunCommand, authority: MobMachineFlowAuthorityToken) -> Result<(), MobError> {
    let outcome = crate::fake::run::apply_mob_machine_flow_run_command(&run.flow_state, command, authority)?;
    run_store
        .cas_flow_state(run_id, &run.flow_state, &outcome.next_state)
        .await?;
    Ok(())
}
",
    )
    .expect("write fake run module apply outcome");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected fake crate::fake::run flow-apply outcome to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_bare_run_module_fake_apply_outcomes() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
mod run {
    pub fn apply_mob_machine_flow_run_command(
        _state: &flow_run::State,
        _command: MobMachineFlowRunCommand,
        _authority: MobMachineFlowAuthorityToken,
    ) -> Result<FakeOutcome, MobError> {
        Ok(FakeOutcome {
            next_state: flow_run::State::default(),
        })
    }
}

async fn bad(run_store: Store, run_id: &RunId, run: MobRun, command: MobMachineFlowRunCommand, authority: MobMachineFlowAuthorityToken) -> Result<(), MobError> {
    let outcome = run::apply_mob_machine_flow_run_command(&run.flow_state, command, authority)?;
    run_store
        .cas_flow_state(run_id, &run.flow_state, &outcome.next_state)
        .await?;
    Ok(())
}
",
    )
    .expect("write bare run:: fake apply outcome");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected fake bare run:: flow-apply outcome to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_inner_scope_canonical_apply_alias_leak() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
async fn bad(run_store: Store, run_id: &RunId, run: MobRun, command: MobMachineFlowRunCommand, authority: MobMachineFlowAuthorityToken) -> Result<(), MobError> {
    {
        use crate::run::apply_mob_machine_flow_run_command as apply;
        let _ = ();
    }
    let outcome = apply(&run.flow_state, command, authority)?;
    run_store
        .cas_flow_state(run_id, &run.flow_state, &outcome.next_state)
        .await?;
    Ok(())
}
",
    )
    .expect("write inner-scope canonical apply alias leak");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected inner-scope canonical flow-apply alias leak to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_nested_module_canonical_apply_alias_leak() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
mod leak {
    use crate::run::apply_mob_machine_flow_run_command;
}

fn apply_mob_machine_flow_run_command(
    _state: &flow_run::State,
    _command: MobMachineFlowRunCommand,
    _authority: MobMachineFlowAuthorityToken,
) -> Result<FakeOutcome, MobError> {
    Ok(FakeOutcome {
        next_state: flow_run::State::default(),
    })
}

async fn bad(run_store: Store, run_id: &RunId, run: MobRun, command: MobMachineFlowRunCommand, authority: MobMachineFlowAuthorityToken) -> Result<(), MobError> {
    let outcome = apply_mob_machine_flow_run_command(&run.flow_state, command, authority)?;
    run_store
        .cas_flow_state(run_id, &run.flow_state, &outcome.next_state)
        .await?;
    Ok(())
}
",
    )
    .expect("write nested module canonical apply alias leak");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected nested module canonical flow-apply alias not to authorize sibling fake apply, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_parent_alias_leaking_into_child_module() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
use crate::run::apply_mob_machine_flow_run_command as apply;

mod inner {
    fn apply(
        _state: &flow_run::State,
        _command: MobMachineFlowRunCommand,
        _authority: MobMachineFlowAuthorityToken,
    ) -> Result<FakeOutcome, MobError> {
        Ok(FakeOutcome {
            next_state: flow_run::State::default(),
        })
    }

    async fn bad(run_store: Store, run_id: &RunId, run: MobRun, command: MobMachineFlowRunCommand, authority: MobMachineFlowAuthorityToken) -> Result<(), MobError> {
        let outcome = apply(&run.flow_state, command, authority)?;
        run_store
            .cas_flow_state(run_id, &run.flow_state, &outcome.next_state)
            .await?;
        Ok(())
    }
}
",
    )
    .expect("write parent alias leaking into child module");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected parent canonical apply alias not to authorize child-module fake apply, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_self_run_module_fake_apply_outcomes() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
mod run {
    pub fn apply_mob_machine_flow_run_command(
        _state: &flow_run::State,
        _command: MobMachineFlowRunCommand,
        _authority: MobMachineFlowAuthorityToken,
    ) -> Result<FakeOutcome, MobError> {
        Ok(FakeOutcome {
            next_state: flow_run::State::default(),
        })
    }
}

async fn bad(run_store: Store, run_id: &RunId, run: MobRun, command: MobMachineFlowRunCommand, authority: MobMachineFlowAuthorityToken) -> Result<(), MobError> {
    let outcome = self::run::apply_mob_machine_flow_run_command(&run.flow_state, command, authority)?;
    run_store
        .cas_flow_state(run_id, &run.flow_state, &outcome.next_state)
        .await?;
    Ok(())
}
",
    )
    .expect("write self::run fake apply outcome");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected fake self::run flow-apply outcome to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_reassigned_typed_outcomes() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
use crate::run::apply_mob_machine_flow_run_command;

async fn bad(run_store: Store, run_id: &RunId, run: MobRun, command: MobMachineFlowRunCommand, authority: MobMachineFlowAuthorityToken) -> Result<(), MobError> {
    let mut outcome = apply_mob_machine_flow_run_command(&run.flow_state, command, authority)?;
    outcome = FakeOutcome {
        next_state: flow_run::State::default(),
    };
    run_store
        .cas_flow_state(run_id, &run.flow_state, &outcome.next_state)
        .await?;
    Ok(())
}
",
    )
    .expect("write reassigned typed outcome");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected reassigned typed flow outcome to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_mutated_typed_outcome_next_state() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
use crate::run::apply_mob_machine_flow_run_command;

async fn bad(run_store: Store, run_id: &RunId, run: MobRun, command: MobMachineFlowRunCommand, authority: MobMachineFlowAuthorityToken) -> Result<(), MobError> {
    let mut outcome = apply_mob_machine_flow_run_command(&run.flow_state, command, authority)?;
    outcome.next_state = flow_run::State::default();
    run_store
        .cas_flow_state(run_id, &run.flow_state, &outcome.next_state)
        .await?;
    Ok(())
}
",
    )
    .expect("write mutated typed outcome next_state");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected typed flow outcome with mutated next_state to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_method_mutated_typed_outcome_next_state() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
use crate::run::apply_mob_machine_flow_run_command;

async fn bad(run_store: Store, run_id: &RunId, run: MobRun, command: MobMachineFlowRunCommand, authority: MobMachineFlowAuthorityToken, frame_id: FrameId) -> Result<(), MobError> {
    let mut outcome = apply_mob_machine_flow_run_command(&run.flow_state, command, authority)?;
    outcome.next_state.ready_frames.push(frame_id);
    run_store
        .cas_flow_state(run_id, &run.flow_state, &outcome.next_state)
        .await?;
    Ok(())
}
",
    )
    .expect("write method-mutated typed outcome next_state");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected typed flow outcome with method-mutated next_state to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_method_mutated_typed_next_state_alias() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
use crate::run::apply_mob_machine_flow_run_command;

async fn bad(run_store: Store, run_id: &RunId, run: MobRun, command: MobMachineFlowRunCommand, authority: MobMachineFlowAuthorityToken, frame_id: FrameId) -> Result<(), MobError> {
    let mut next_state = apply_mob_machine_flow_run_command(&run.flow_state, command, authority)?.next_state;
    next_state.ready_frames.push(frame_id);
    run_store
        .cas_flow_state(run_id, &run.flow_state, &next_state)
        .await?;
    Ok(())
}
",
    )
    .expect("write method-mutated typed next_state alias");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected method-mutated typed next_state alias to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_unlisted_method_mutated_typed_next_state() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
use crate::run::apply_mob_machine_flow_run_command;

async fn bad(run_store: Store, run_id: &RunId, run: MobRun, command: MobMachineFlowRunCommand, authority: MobMachineFlowAuthorityToken) -> Result<(), MobError> {
    let mut next_state = apply_mob_machine_flow_run_command(&run.flow_state, command, authority)?.next_state;
    next_state.ready_frames.truncate(0);
    run_store
        .cas_flow_state(run_id, &run.flow_state, &next_state)
        .await?;
    Ok(())
}
",
    )
    .expect("write unlisted method-mutated typed next_state");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected unlisted method-mutated typed next_state to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_inner_shadowed_next_state_alias() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
use crate::run::apply_mob_machine_flow_run_command;

async fn bad(run_store: Store, run_id: &RunId, run: MobRun, command: MobMachineFlowRunCommand, authority: MobMachineFlowAuthorityToken) -> Result<(), MobError> {
    let next_state = flow_run::State::default();
    {
        let next_state = apply_mob_machine_flow_run_command(&run.flow_state, command, authority)?.next_state;
        let _ = next_state;
    }
    run_store
        .cas_flow_state(run_id, &run.flow_state, &next_state)
        .await?;
    Ok(())
}
",
    )
    .expect("write inner shadowed next-state alias");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected inner shadowed typed next_state alias not to authorize outer CAS, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_with_authority_cas_without_authority_inputs() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
use crate::run::apply_mob_machine_flow_run_command;

async fn bad(run_store: Store, run_id: &RunId, run: MobRun, command: MobMachineFlowRunCommand, authority: MobMachineFlowAuthorityToken) -> Result<(), MobError> {
    let outcome = apply_mob_machine_flow_run_command(&run.flow_state, command.clone(), authority)?;
    run_store
        .cas_flow_state_with_authority(run_id, &run.flow_state, &outcome.next_state, vec![])
        .await?;
    Ok(())
}
",
    )
    .expect("write empty with-authority CAS commit");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".cas_flow_state_with_authority(")
        }),
        "expected with-authority CAS without matching authority inputs to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_with_authority_cas_with_mismatched_authority_input()
{
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
use crate::run::apply_mob_machine_flow_run_command;

async fn bad(run_store: Store, run_id: &RunId, run: MobRun, command: MobMachineFlowRunCommand, other_command: MobMachineFlowRunCommand, authority: MobMachineFlowAuthorityToken) -> Result<(), MobError> {
    let outcome = apply_mob_machine_flow_run_command(&run.flow_state, command.clone(), authority)?;
    let authority_input = other_command.authority_input(run_id);
    run_store
        .cas_flow_state_with_authority(
            run_id,
            &run.flow_state,
            &outcome.next_state,
            vec![authority_input],
        )
        .await?;
    Ok(())
}
",
    )
    .expect("write mismatched with-authority CAS commit");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".cas_flow_state_with_authority(")
        }),
        "expected with-authority CAS with mismatched authority input to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_with_authority_cas_with_wrong_run_id_input() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
use crate::run::apply_mob_machine_flow_run_command;

async fn bad(run_store: Store, run_id: &RunId, other_run_id: &RunId, run: MobRun, command: MobMachineFlowRunCommand, authority: MobMachineFlowAuthorityToken) -> Result<(), MobError> {
    let outcome = apply_mob_machine_flow_run_command(&run.flow_state, command.clone(), authority)?;
    let authority_input = command.authority_input(other_run_id);
    run_store
        .cas_flow_state_with_authority(
            run_id,
            &run.flow_state,
            &outcome.next_state,
            vec![authority_input],
        )
        .await?;
    Ok(())
}
",
    )
    .expect("write wrong-run-id with-authority CAS commit");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".cas_flow_state_with_authority(")
        }),
        "expected authority input identity to match the CAS run_id, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_apply_run_id_mismatched_with_cas_identity() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
use crate::run::apply_mob_machine_flow_run_command;

async fn bad(
    run_store: Store,
    run_id: &RunId,
    other_run_id: &RunId,
    machine_state: &MobMachineState,
    run: MobRun,
    command: MobMachineFlowRunCommand,
    authority: MobMachineFlowAuthorityToken,
) -> Result<(), MobError> {
    let outcome = apply_mob_machine_flow_run_command(
        &run.flow_state,
        machine_state,
        other_run_id,
        command,
        authority,
    )?;
    run_store
        .cas_flow_state(run_id, &run.flow_state, &outcome.next_state)
        .await?;
    Ok(())
}
",
    )
    .expect("write apply-run-id mismatched CAS identity");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected typed flow outcome computed with another RunId to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_apply_run_id_from_local_identity() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
use crate::run::apply_mob_machine_flow_run_command;

async fn bad(
    run_store: Store,
    run_id: &RunId,
    machine_state: &MobMachineState,
    run: MobRun,
    command: MobMachineFlowRunCommand,
    authority: MobMachineFlowAuthorityToken,
) -> Result<(), MobError> {
    let forged_run_id = RunId::new();
    let outcome = apply_mob_machine_flow_run_command(
        &run.flow_state,
        machine_state,
        &forged_run_id,
        command,
        authority,
    )?;
    run_store
        .cas_flow_state(run_id, &run.flow_state, &outcome.next_state)
        .await?;
    Ok(())
}
",
    )
    .expect("write local apply-run-id CAS identity");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected typed flow outcome computed with local RunId to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_block_local_run_id_shadowed_apply() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
use crate::run::apply_mob_machine_flow_run_command;

async fn bad(
    run_store: Store,
    run_id: &RunId,
    machine_state: &MobMachineState,
    run: MobRun,
    command: MobMachineFlowRunCommand,
    authority: MobMachineFlowAuthorityToken,
) -> Result<(), MobError> {
    let outcome = {
        let run_id = RunId::new();
        apply_mob_machine_flow_run_command(
            &run.flow_state,
            machine_state,
            &run_id,
            command,
            authority,
        )?
    };
    run_store
        .cas_flow_state(run_id, &run.flow_state, &outcome.next_state)
        .await?;
    Ok(())
}
",
    )
    .expect("write block-local run_id shadowed apply");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected block-local RunId shadowed flow outcome to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_block_local_command_shadowed_apply() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
use crate::run::apply_mob_machine_flow_run_command;

async fn bad(
    run_store: Store,
    run_id: &RunId,
    machine_state: &MobMachineState,
    run: MobRun,
    command: MobMachineFlowRunCommand,
    other_command: MobMachineFlowRunCommand,
    authority: MobMachineFlowAuthorityToken,
) -> Result<(), MobError> {
    let outcome = {
        let command = other_command;
        apply_mob_machine_flow_run_command(
            &run.flow_state,
            machine_state,
            run_id,
            command,
            authority,
        )?
    };
    let authority_input = command.authority_input(run_id);
    run_store
        .cas_flow_state_with_authority(
            run_id,
            &run.flow_state,
            &outcome.next_state,
            vec![authority_input],
        )
        .await?;
    Ok(())
}
",
    )
    .expect("write block-local command shadowed apply");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".cas_flow_state_with_authority(")
        }),
        "expected block-local command shadowed flow outcome to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_match_shadowed_typed_outcome() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
use crate::run::apply_mob_machine_flow_run_command;

async fn bad(
    run_store: Store,
    run_id: &RunId,
    machine_state: &MobMachineState,
    run: MobRun,
    command: MobMachineFlowRunCommand,
    authority: MobMachineFlowAuthorityToken,
    fake: FakeOutcomeHolder,
) -> Result<(), MobError> {
    let outcome = apply_mob_machine_flow_run_command(
        &run.flow_state,
        machine_state,
        run_id,
        command,
        authority,
    )?;
    match fake {
        FakeOutcomeHolder { outcome } => {
            run_store
                .cas_flow_state(run_id, &run.flow_state, &outcome.next_state)
                .await?;
        }
    }
    Ok(())
}
",
    )
    .expect("write match-shadowed typed outcome");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected match-shadowed typed outcome to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_fake_local_command_token_constructors() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
use crate::run::apply_mob_machine_flow_run_command;

mod fake {
    pub enum MobMachineFlowRunCommand {
        StartRun(flow_run::inputs::StartRun),
    }

    impl MobMachineFlowRunCommand {
        fn authority_input(&self, _run_id: &RunId) -> MobMachineInput {
            MobMachineInput::RunFlow
        }
    }

    pub struct MobMachineFlowAuthorityToken;

    impl MobMachineFlowAuthorityToken {
        fn from_accepted_mob_machine_input(
            _input: &MobMachineInput,
        ) -> Result<Self, MobError> {
            Ok(Self)
        }
    }
}

async fn bad(
    run_store: Store,
    run_id: &RunId,
    machine_state: &MobMachineState,
    run: MobRun,
    accepted_input: MobMachineInput,
) -> Result<(), MobError> {
    let command = fake::MobMachineFlowRunCommand::StartRun(flow_run::inputs::StartRun {});
    let authority = fake::MobMachineFlowAuthorityToken::from_accepted_mob_machine_input(
        &accepted_input,
    )?;
    let outcome = apply_mob_machine_flow_run_command(
        &run.flow_state,
        machine_state,
        run_id,
        command,
        authority,
    )?;
    let authority_input = command.authority_input(run_id);
    run_store
        .cas_flow_state_with_authority(
            run_id,
            &run.flow_state,
            &outcome.next_state,
            vec![authority_input],
        )
        .await?;
    Ok(())
}
",
    )
    .expect("write fake local command/token constructors");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".cas_flow_state_with_authority(")
        }),
        "expected fake local command/token constructors not to authorize flow CAS, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_if_let_shadowed_authority_input() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
use crate::run::apply_mob_machine_flow_run_command;

async fn bad(
    run_store: Store,
    run_id: &RunId,
    machine_state: &MobMachineState,
    run: MobRun,
    command: MobMachineFlowRunCommand,
    authority: MobMachineFlowAuthorityToken,
    fake_input: Option<MobMachineInput>,
) -> Result<(), MobError> {
    let outcome = apply_mob_machine_flow_run_command(
        &run.flow_state,
        machine_state,
        run_id,
        command.clone(),
        authority,
    )?;
    let authority_input = command.authority_input(run_id);
    if let Some(authority_input) = fake_input {
        run_store
            .cas_flow_state_with_authority(
                run_id,
                &run.flow_state,
                &outcome.next_state,
                vec![authority_input],
            )
            .await?;
    }
    Ok(())
}
",
    )
    .expect("write if-let-shadowed authority input");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".cas_flow_state_with_authority(")
        }),
        "expected if-let-shadowed authority input to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_mutably_replaced_authority_input() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
use crate::run::apply_mob_machine_flow_run_command;

async fn bad(run_store: Store, run_id: &RunId, run: MobRun, command: MobMachineFlowRunCommand, other_command: MobMachineFlowRunCommand, authority: MobMachineFlowAuthorityToken) -> Result<(), MobError> {
    let outcome = apply_mob_machine_flow_run_command(&run.flow_state, command.clone(), authority)?;
    let mut authority_input = command.authority_input(run_id);
    std::mem::replace(&mut authority_input, other_command.authority_input(run_id));
    run_store
        .cas_flow_state_with_authority(
            run_id,
            &run.flow_state,
            &outcome.next_state,
            vec![authority_input],
        )
        .await?;
    Ok(())
}
",
    )
    .expect("write mutably replaced authority input CAS commit");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".cas_flow_state_with_authority(")
        }),
        "expected mutable authority input replacement to invalidate CAS authority proof, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_mutably_replaced_next_state_alias() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
use crate::run::apply_mob_machine_flow_run_command;

async fn bad(run_store: Store, run_id: &RunId, run: MobRun, command: MobMachineFlowRunCommand, authority: MobMachineFlowAuthorityToken) -> Result<(), MobError> {
    let outcome = apply_mob_machine_flow_run_command(&run.flow_state, command.clone(), authority)?;
    let authority_input = command.authority_input(run_id);
    let mut next_state = outcome.next_state.clone();
    std::mem::replace(&mut next_state, flow_run::State::default());
    run_store
        .cas_flow_state_with_authority(
            run_id,
            &run.flow_state,
            &next_state,
            vec![authority_input],
        )
        .await?;
    Ok(())
}
",
    )
    .expect("write mutably replaced next-state alias CAS commit");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".cas_flow_state_with_authority(")
        }),
        "expected mutable next-state alias replacement to invalidate CAS state proof, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_rebound_command_authority_input() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
use crate::run::apply_mob_machine_flow_run_command;

async fn bad(run_store: Store, run_id: &RunId, run: MobRun, command: MobMachineFlowRunCommand, replacement: MobMachineFlowRunCommand, authority: MobMachineFlowAuthorityToken) -> Result<(), MobError> {
    let outcome = apply_mob_machine_flow_run_command(&run.flow_state, command.clone(), authority)?;
    let command = replacement;
    let authority_input = command.authority_input(run_id);
    run_store
        .cas_flow_state_with_authority(
            run_id,
            &run.flow_state,
            &outcome.next_state,
            vec![authority_input],
        )
        .await?;
    Ok(())
}
",
    )
    .expect("write rebound command authority input CAS commit");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".cas_flow_state_with_authority(")
        }),
        "expected authority input derived from rebound command name to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_reassigned_command_parameter_authority_input() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
use crate::run::apply_mob_machine_flow_run_command;

async fn bad(run_store: Store, run_id: &RunId, run: MobRun, mut command: MobMachineFlowRunCommand, replacement: MobMachineFlowRunCommand, authority: MobMachineFlowAuthorityToken) -> Result<(), MobError> {
    let outcome = apply_mob_machine_flow_run_command(&run.flow_state, command.clone(), authority)?;
    command = replacement;
    let authority_input = command.authority_input(run_id);
    run_store
        .cas_flow_state_with_authority(
            run_id,
            &run.flow_state,
            &outcome.next_state,
            vec![authority_input],
        )
        .await?;
    Ok(())
}
",
    )
    .expect("write reassigned command parameter authority input CAS commit");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".cas_flow_state_with_authority(")
        }),
        "expected authority input derived from reassigned command parameter to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_mixed_authority_input_vector() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
use crate::run::apply_mob_machine_flow_run_command;

async fn bad(run_store: Store, run_id: &RunId, run: MobRun, command: MobMachineFlowRunCommand, other_command: MobMachineFlowRunCommand, authority: MobMachineFlowAuthorityToken) -> Result<(), MobError> {
    let outcome = apply_mob_machine_flow_run_command(&run.flow_state, command.clone(), authority)?;
    let authority_input = command.authority_input(run_id);
    let unrelated_input = other_command.authority_input(run_id);
    run_store
        .cas_flow_state_with_authority(
            run_id,
            &run.flow_state,
            &outcome.next_state,
            vec![authority_input, unrelated_input],
        )
        .await?;
    Ok(())
}
",
    )
    .expect("write mixed authority input CAS commit");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".cas_flow_state_with_authority(")
        }),
        "expected with-authority CAS containing unrelated authority input to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_unrelated_with_authority_cas_next_state() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
async fn bad(run_store: Store, run_id: &RunId, run: MobRun) -> Result<(), MobError> {
    let unrelated_next_state = flow_run::State::default();
    run_store
        .cas_flow_state_with_authority(run_id, &run.flow_state, &unrelated_next_state, vec![])
        .await?;
    Ok(())
}
",
    )
    .expect("write unrelated with-authority CAS commit");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".cas_flow_state_with_authority(")
        }),
        "expected with-authority CAS not tied to typed outcome.next_state to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_unrelated_cas_next_state() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
async fn bad(run_store: Store, run_id: &RunId, run: MobRun, command: MobMachineFlowRunCommand, authority: MobMachineFlowAuthorityToken) -> Result<(), MobError> {
    let outcome = apply_mob_machine_flow_run_command(&run.flow_state, command, authority)?;
    let unrelated_next_state = flow_run::State::default();
    run_store
        .cas_flow_state(run_id, &run.flow_state, &unrelated_next_state)
        .await?;
    let _ = outcome;
    Ok(())
}
",
    )
    .expect("write unrelated CAS commit");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected CAS not tied to typed outcome.next_state to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_typed_outcome_in_wrong_cas_argument() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
async fn bad(run_store: Store, run_id: &RunId, run: MobRun, command: MobMachineFlowRunCommand, authority: MobMachineFlowAuthorityToken) -> Result<(), MobError> {
    let outcome = apply_mob_machine_flow_run_command(&run.flow_state, command, authority)?;
    let unrelated_next_state = flow_run::State::default();
    run_store
        .cas_flow_state(run_id, &outcome.next_state, &unrelated_next_state)
        .await?;
    Ok(())
}
",
    )
    .expect("write wrong-position CAS commit");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected CAS with typed outcome in expected-state slot to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_fake_outcome_wrappers() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
async fn bad(run_store: Store, run_id: &RunId, run: MobRun, command: MobMachineFlowRunCommand, authority: MobMachineFlowAuthorityToken) -> Result<(), MobError> {
    let fake = {
        let _ = apply_mob_machine_flow_run_command(&run.flow_state, command, authority)?;
        FakeOutcome {
            next_state: flow_run::State::default(),
        }
    };
    run_store
        .cas_flow_state(run_id, &run.flow_state, &fake.next_state)
        .await?;
    Ok(())
}
",
    )
    .expect("write fake outcome commit");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected fake outcome wrapper to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_shadowed_outcome_names() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
async fn bad(run_store: Store, run_id: &RunId, run: MobRun, command: MobMachineFlowRunCommand, authority: MobMachineFlowAuthorityToken) -> Result<(), MobError> {
    let outcome = FakeOutcome {
        next_state: flow_run::State::default(),
    };
    {
        let outcome = apply_mob_machine_flow_run_command(&run.flow_state, command, authority)?;
        let _ = outcome;
    }
    run_store
        .cas_flow_state(run_id, &run.flow_state, &outcome.next_state)
        .await?;
    Ok(())
}
",
    )
    .expect("write shadowed outcome commit");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected shadowed outcome name to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_local_fake_apply_outcomes() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
async fn bad(run_store: Store, run_id: &RunId, run: MobRun, command: MobMachineFlowRunCommand, authority: MobMachineFlowAuthorityToken) -> Result<(), MobError> {
    fn apply_mob_machine_flow_run_command(
        _state: &flow_run::State,
        _command: MobMachineFlowRunCommand,
        _authority: MobMachineFlowAuthorityToken,
    ) -> Result<FakeOutcome, MobError> {
        Ok(FakeOutcome {
            next_state: flow_run::State::default(),
        })
    }

    let outcome = apply_mob_machine_flow_run_command(&run.flow_state, command, authority)?;
    run_store
        .cas_flow_state(run_id, &run.flow_state, &outcome.next_state)
        .await?;
    Ok(())
}
",
    )
    .expect("write local fake apply outcome");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected local fake flow-apply outcome to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_ufcs_cas_writes() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
async fn bad(run_store: Store, run_id: &RunId, run: MobRun) -> Result<(), MobError> {
    let unrelated_next_state = flow_run::State::default();
    MobRunStore::cas_flow_state(&run_store, run_id, &run.flow_state, &unrelated_next_state).await?;
    Ok(())
}
",
    )
    .expect("write UFCS CAS commit");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected UFCS CAS write to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_local_same_name_cas_function() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
use crate::run::apply_mob_machine_flow_run_command;

async fn cas_flow_state_with_authority(
    _run_id: &RunId,
    _expected: &flow_run::State,
    _next: &flow_run::State,
    _authority_inputs: Vec<MobMachineInput>,
) -> Result<bool, MobError> {
    Ok(true)
}

async fn bad(
    run_id: &RunId,
    run: MobRun,
    command: MobMachineFlowRunCommand,
    authority: MobMachineFlowAuthorityToken,
) -> Result<(), MobError> {
    let outcome = apply_mob_machine_flow_run_command(&run.flow_state, command.clone(), authority)?;
    let authority_inputs = command.authority_input(run_id);
    cas_flow_state_with_authority(
        run_id,
        &run.flow_state,
        &outcome.next_state,
        authority_inputs,
    )
    .await?;
    Ok(())
}
",
    )
    .expect("write local same-name CAS function");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".cas_flow_state_with_authority(")
        }),
        "expected local same-name CAS function not to authorize projection commit, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_transitive_state_alias_mutation() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
fn bad(run: &mut MobRun) {
    let state = &mut run.flow_state;
    let alias = state;
    alias.phase = flow_run::Phase::Running;
}
",
    )
    .expect("write transitive state alias mutation");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".flow_state.phase")
        }),
        "expected transitive flow-state alias mutation to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_assigned_state_alias_mutation() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
fn bad(run: &mut MobRun) {
    let state;
    state = &mut run.flow_state;
    state.phase = flow_run::Phase::Running;
}
",
    )
    .expect("write assigned state alias mutation");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".flow_state.phase")
        }),
        "expected assigned flow-state alias mutation to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_destructured_state_alias_mutation() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
fn bad(run: &mut MobRun) {
    let state = &mut run.flow_state;
    let (alias,) = (state,);
    alias.phase = flow_run::Phase::Running;
}
",
    )
    .expect("write destructured state alias mutation");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".flow_state.phase")
        }),
        "expected destructured flow-state alias mutation to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_struct_destructured_state_alias_mutation() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
fn bad(run: &mut MobRun) {
    let Wrapper { alias } = Wrapper {
        alias: &mut run.flow_state,
    };
    alias.phase = flow_run::Phase::Running;
}
",
    )
    .expect("write struct-destructured state alias mutation");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".flow_state.phase")
        }),
        "expected struct-destructured flow-state alias mutation to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_slice_destructured_state_alias_mutation() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
fn bad(run: &mut MobRun) {
    let [alias] = [&mut run.flow_state];
    alias.phase = flow_run::Phase::Running;
}
",
    )
    .expect("write slice-destructured state alias mutation");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".flow_state.phase")
        }),
        "expected slice-destructured flow-state alias mutation to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_state_alias_mutation() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
fn bad(run: &mut MobRun) {
    let state = &mut run.flow_state;
    state.phase = flow_run::Phase::Running;
}
",
    )
    .expect("write state alias mutation");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".flow_state.phase")
        }),
        "expected aliased flow-state mutation to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_deref_state_alias_mutation() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
fn bad(mut run: MobRun) {
    let state = &mut run.flow_state;
    (*state).phase = flow_run::Phase::Running;
}
",
    )
    .expect("write deref state alias mutation");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".flow_state.phase")
        }),
        "expected dereferenced flow_state alias mutation to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_mutable_reference_to_flow_state() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
fn bad(run: &mut MobRun) {
    let _old = std::mem::replace(&mut run.flow_state, flow_run::State::default());
}
",
    )
    .expect("write mutable reference to flow state");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs") && mismatch.contains(".flow_state")
        }),
        "expected mutable reference to flow_state to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_whole_flow_state_assignment() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
fn bad(run: &mut MobRun) {
    run.flow_state = flow_run::State::default();
}
",
    )
    .expect("write whole flow state assignment");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs") && mismatch.contains(".flow_state")
        }),
        "expected whole flow_state assignment to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_whole_flow_state_mutator() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
fn bad(run: &mut MobRun) {
    run.flow_state.clone_from(&flow_run::State::default());
}
",
    )
    .expect("write whole flow state mutator");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs") && mismatch.contains(".flow_state")
        }),
        "expected whole flow_state mutator to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_flow_state_entry_mutator() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
fn bad(run: &mut MobRun, step_id: StepId) {
    run.flow_state
        .step_status
        .entry(step_id)
        .or_insert(Some(StepRunStatus::Running));
}
",
    )
    .expect("write flow state entry mutator");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".flow_state.step_status")
        }),
        "expected flow_state entry mutator to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_flow_state_get_mut_write_access() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
fn bad(run: &mut MobRun, step_id: &StepId) {
    if let Some(status) = run.flow_state.step_status.get_mut(step_id) {
        *status = Some(StepRunStatus::Running);
    }
}
",
    )
    .expect("write flow state get_mut write access");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".flow_state.step_status")
        }),
        "expected flow_state get_mut write access to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_flow_state_values_mut_write_access() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
fn bad(run: &mut MobRun) {
    for status in run.flow_state.step_status.values_mut() {
        *status = StepRunStatus::Running;
    }
}
",
    )
    .expect("write flow state values_mut write access");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".flow_state.step_status")
        }),
        "expected flow_state values_mut write access to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_flow_state_range_mut_write_access() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
fn bad(run: &mut MobRun) {
    for (_, status) in run.flow_state.step_status.range_mut(..) {
        *status = StepRunStatus::Running;
    }
}
",
    )
    .expect("write flow state range_mut write access");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".flow_state.step_status")
        }),
        "expected flow_state range_mut write access to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_flow_state_first_entry_write_access() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
fn bad(run: &mut MobRun) {
    if let Some(mut entry) = run.flow_state.step_status.first_entry() {
        entry.insert(StepRunStatus::Running);
    }
}
",
    )
    .expect("write flow state first_entry write access");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".flow_state.step_status")
        }),
        "expected flow_state first_entry write access to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_flow_state_take_write_access() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
fn bad(run: &mut MobRun, step_id: &StepId) {
    let _ = run.flow_state.ready_frame_membership.take(step_id);
}
",
    )
    .expect("write flow state take write access");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".flow_state.ready_frame_membership")
        }),
        "expected flow_state take write access to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_flow_state_replace_write_access() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
fn bad(run: &mut MobRun, step_id: StepId) {
    let _ = run.flow_state.ready_queue.replace(step_id);
}
",
    )
    .expect("write flow state replace write access");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs") && mismatch.contains(".flow_state")
        }),
        "expected flow_state replace write access to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_flow_state_pop_entry_write_access() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
fn bad(run: &mut MobRun) {
    let _ = run.flow_state.step_status.pop_first();
}
",
    )
    .expect("write flow state pop_first write access");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".flow_state.step_status")
        }),
        "expected flow_state pop_first write access to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_match_destructured_state_alias_mutation() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
fn bad(run: &mut MobRun) {
    match run {
        MobRun { flow_state: state, .. } => {
            state.phase = flow_run::Phase::Running;
        }
    }
}
",
    )
    .expect("write match owner struct destructured state alias mutation");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".flow_state.phase")
        }),
        "expected match-destructured flow_state alias mutation to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_let_else_diverge_projection_mutation() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
fn bad(run: &mut MobRun, maybe: Option<StepId>) {
    let Some(step_id) = maybe else {
        run.flow_state.phase = flow_run::Phase::Running;
        return;
    };
    let _ = step_id;
}
",
    )
    .expect("write let-else diverge projection mutation");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".flow_state.phase")
        }),
        "expected let-else diverge projection mutation to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_owner_struct_destructured_state_alias_mutation() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
fn bad(run: &mut MobRun) {
    let MobRun { flow_state: state, .. } = run;
    state.phase = flow_run::Phase::Running;
}
",
    )
    .expect("write owner struct destructured state alias mutation");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".flow_state.phase")
        }),
        "expected owner-struct destructured flow_state alias mutation to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_aliased_owner_struct_destructured_state_alias_mutation()
 {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
use crate::run::MobRun as Run;

fn bad(run: &mut Run) {
    let Run { flow_state: state, .. } = run;
    state.phase = flow_run::Phase::Running;
}
",
    )
    .expect("write aliased owner struct destructured state alias mutation");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/flow.rs")
                && mismatch.contains(".flow_state.phase")
        }),
        "expected aliased owner-struct destructured flow_state alias mutation to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_accepts_actor_store_plan_commit() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
impl MobActor {
    async fn commit_flow_frame_store_plan_in_actor(
        &mut self,
        run_id: &RunId,
        plan: FlowFrameLoopStorePlan,
    ) -> Result<bool, MobError> {
        if !self
            .flow_frame_store_plan_expected_matches(run_id, &plan)
            .await?
        {
            return Ok(false);
        }
        let prepared =
            self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
        let authority_inputs = plan.machine_inputs().to_vec();
        let won = match &plan {
            FlowFrameLoopStorePlan::InsertFrame {
                frame_id,
                initial_frame,
                ..
            } => self
                .run_store
                .cas_frame_state_with_authority(
                    run_id,
                    frame_id,
                    None,
                    initial_frame.clone(),
                    authority_inputs.clone(),
                )
                .await?,
            FlowFrameLoopStorePlan::FrameState {
                frame_id,
                expected_frame,
                next_frame,
                ..
            } => self
                .run_store
                .cas_frame_state_with_authority(
                    run_id,
                    frame_id,
                    Some(expected_frame),
                    next_frame.clone(),
                    authority_inputs.clone(),
                )
                .await?,
            FlowFrameLoopStorePlan::CompleteStepAndRecordOutput {
                frame_id,
                expected_frame,
                next_frame,
                step_output_key,
                step_output,
                loop_context,
                ..
            } => self
                .run_store
                .cas_complete_step_and_record_output_with_authority(
                    run_id,
                    frame_id,
                    expected_frame,
                    next_frame.clone(),
                    step_output_key.clone(),
                    step_output.clone(),
                    loop_context
                        .as_ref()
                        .map(|(loop_id, iteration)| (loop_id, *iteration)),
                    authority_inputs.clone(),
                )
                .await?,
            FlowFrameLoopStorePlan::GrantNodeSlot {
                expected_run_state,
                next_run_state,
                frame_id,
                expected_frame,
                next_frame,
                ..
            } => self
                .run_store
                .cas_grant_node_slot_with_authority(
                    run_id,
                    expected_run_state,
                    next_run_state.clone(),
                    frame_id,
                    expected_frame,
                    next_frame.clone(),
                    authority_inputs.clone(),
                )
                .await?,
            FlowFrameLoopStorePlan::StartLoop {
                loop_instance_id,
                expected_run_state,
                next_run_state,
                frame_id,
                expected_frame,
                next_frame,
                initial_loop,
                ..
            } => self
                .run_store
                .cas_start_loop_with_authority(
                    run_id,
                    loop_instance_id,
                    expected_run_state,
                    next_run_state.clone(),
                    frame_id,
                    expected_frame,
                    next_frame.clone(),
                    initial_loop.clone(),
                    authority_inputs.clone(),
                )
                .await?,
            FlowFrameLoopStorePlan::GrantBodyFrameStart {
                loop_instance_id,
                expected_loop,
                next_loop,
                frame_id,
                initial_frame,
                ledger_entry,
                expected_run_state,
                next_run_state,
                ..
            } => self
                .run_store
                .cas_grant_body_frame_start_with_authority(
                    run_id,
                    loop_instance_id,
                    expected_loop,
                    next_loop.clone(),
                    frame_id,
                    initial_frame.clone(),
                    ledger_entry.clone(),
                    expected_run_state,
                    next_run_state.clone(),
                    authority_inputs.clone(),
                )
                .await?,
            FlowFrameLoopStorePlan::RunStateOnly {
                expected_run_state,
                next_run_state,
                ..
            } => self
                .run_store
                .cas_flow_state_with_authority(
                    run_id,
                    expected_run_state,
                    next_run_state,
                    authority_inputs.clone(),
                )
                .await?,
            FlowFrameLoopStorePlan::SealFrame {
                frame_id,
                expected_frame,
                next_frame,
                ..
            } => self
                .run_store
                .cas_frame_state_with_authority(
                    run_id,
                    frame_id,
                    Some(expected_frame),
                    next_frame.clone(),
                    authority_inputs.clone(),
                )
                .await?,
            FlowFrameLoopStorePlan::CompleteBodyFrame {
                loop_instance_id,
                expected_loop,
                next_loop,
                frame_id,
                expected_frame,
                next_frame,
                expected_run_state,
                next_run_state,
                ..
            } => self
                .run_store
                .cas_complete_body_frame_with_authority(
                    run_id,
                    loop_instance_id,
                    expected_loop,
                    next_loop.clone(),
                    frame_id,
                    expected_frame,
                    next_frame.clone(),
                    expected_run_state,
                    next_run_state.clone(),
                    authority_inputs.clone(),
                )
                .await?,
            FlowFrameLoopStorePlan::LoopRequestBodyFrame {
                loop_instance_id,
                expected_loop,
                next_loop,
                expected_run_state,
                next_run_state,
                ..
            } => self
                .run_store
                .cas_loop_request_body_frame_with_authority(
                    run_id,
                    loop_instance_id,
                    expected_loop,
                    next_loop.clone(),
                    expected_run_state,
                    next_run_state.clone(),
                    authority_inputs.clone(),
                )
                .await?,
            FlowFrameLoopStorePlan::CompleteLoop {
                loop_instance_id,
                expected_loop,
                next_loop,
                frame_id,
                expected_frame,
                next_frame,
                expected_run_state,
                next_run_state,
                ..
            } => self
                .run_store
                .cas_complete_loop_with_authority(
                    run_id,
                    loop_instance_id,
                    expected_loop,
                    next_loop.clone(),
                    frame_id,
                    expected_frame,
                    next_frame.clone(),
                    expected_run_state,
                    next_run_state.clone(),
                    authority_inputs.clone(),
                )
                .await?,
            _ => false,
        };
        if won {
            self.commit_prepared_dsl_input(prepared);
        }
        Ok(won)
    }
}
"#,
    )
    .expect("write actor store plan commit");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.is_empty(),
        "expected actor-gated store plan commits to be accepted, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_wrong_run_id_arg() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
impl MobActor {
    async fn commit_flow_frame_store_plan_in_actor(
        &mut self,
        run_id: &RunId,
        other_run_id: &RunId,
        plan: FlowFrameLoopStorePlan,
    ) -> Result<bool, MobError> {
        let prepared =
            self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
        let won = match &plan {
            FlowFrameLoopStorePlan::RunStateOnly {
                expected_run_state,
                next_run_state,
                ..
            } => self
                .run_store
                .cas_flow_state(other_run_id, expected_run_state, next_run_state)
                .await?,
            _ => false,
        };
        if won {
            self.commit_prepared_dsl_input(prepared);
        }
        Ok(won)
    }
}
"#,
    )
    .expect("write actor store plan wrong run_id arg");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected actor store-plan CAS with wrong run_id arg to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_swapped_run_state_args() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
impl MobActor {
    async fn commit_flow_frame_store_plan_in_actor(
        &mut self,
        run_id: &RunId,
        plan: FlowFrameLoopStorePlan,
    ) -> Result<bool, MobError> {
        let prepared =
            self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
        let won = match &plan {
            FlowFrameLoopStorePlan::RunStateOnly {
                expected_run_state,
                next_run_state,
                ..
            } => self
                .run_store
                .cas_flow_state(run_id, next_run_state, expected_run_state)
                .await?,
            _ => false,
        };
        if won {
            self.commit_prepared_dsl_input(prepared);
        }
        Ok(won)
    }
}
"#,
    )
    .expect("write actor store plan swapped run state args");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected actor store-plan CAS with swapped expected/next run states to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_wrong_cas_for_variant() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
impl MobActor {
    async fn commit_flow_frame_store_plan_in_actor(
        &mut self,
        run_id: &RunId,
        plan: FlowFrameLoopStorePlan,
    ) -> Result<bool, MobError> {
        let prepared =
            self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
        let won = match &plan {
            FlowFrameLoopStorePlan::GrantNodeSlot {
                expected_run_state,
                next_run_state,
                frame_id,
                expected_frame,
                next_frame,
                ..
            } => self
                .run_store
                .cas_flow_state(run_id, expected_run_state, next_run_state)
                .await?,
            _ => false,
        };
        if won {
            self.commit_prepared_dsl_input(prepared);
        }
        Ok(won)
    }
}
"#,
    )
    .expect("write actor store plan wrong CAS for variant");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected actor store-plan CAS that drops GrantNodeSlot frame fields to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_discarded_loop_context() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
impl MobActor {
    async fn commit_flow_frame_store_plan_in_actor(
        &mut self,
        run_id: &RunId,
        plan: FlowFrameLoopStorePlan,
    ) -> Result<bool, MobError> {
        let prepared =
            self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
        let authority_inputs = plan.machine_inputs().to_vec();
        let won = match &plan {
            FlowFrameLoopStorePlan::CompleteStepAndRecordOutput {
                frame_id,
                expected_frame,
                next_frame,
                step_output_key,
                step_output,
                loop_context,
                ..
            } => self
                .run_store
                .cas_complete_step_and_record_output_with_authority(
                    run_id,
                    frame_id,
                    expected_frame,
                    next_frame.clone(),
                    step_output_key.clone(),
                    step_output.clone(),
                    None,
                    authority_inputs,
                )
                .await?,
            _ => false,
        };
        if won {
            self.commit_prepared_dsl_input(prepared);
        }
        Ok(won)
    }
}
"#,
    )
    .expect("write actor store plan discarded loop context");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_complete_step_and_record_output_with_authority(")
        }),
        "expected actor store-plan CAS that discards loop_context to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_same_shape_wrong_cas_variant() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
impl MobActor {
    async fn commit_flow_frame_store_plan_in_actor(
        &mut self,
        run_id: &RunId,
        plan: FlowFrameLoopStorePlan,
    ) -> Result<bool, MobError> {
        let prepared =
            self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
        let authority_inputs = plan.machine_inputs().to_vec();
        let won = match &plan {
            FlowFrameLoopStorePlan::CompleteBodyFrame {
                loop_instance_id,
                expected_loop,
                next_loop,
                frame_id,
                expected_frame,
                next_frame,
                expected_run_state,
                next_run_state,
                ..
            } => self
                .run_store
                .cas_complete_loop_with_authority(
                    run_id,
                    loop_instance_id,
                    expected_loop,
                    next_loop.clone(),
                    frame_id,
                    expected_frame,
                    next_frame.clone(),
                    expected_run_state,
                    next_run_state.clone(),
                    authority_inputs,
                )
                .await?,
            _ => false,
        };
        if won {
            self.commit_prepared_dsl_input(prepared);
        }
        Ok(won)
    }
}
"#,
    )
    .expect("write actor store plan same-shape wrong CAS variant");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_complete_loop_with_authority(")
        }),
        "expected same-shape store-plan variant to require its canonical CAS method, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_or_pattern_variant_union() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
impl MobActor {
    async fn commit_flow_frame_store_plan_in_actor(
        &mut self,
        run_id: &RunId,
        plan: FlowFrameLoopStorePlan,
    ) -> Result<bool, MobError> {
        let prepared =
            self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
        let won = match &plan {
            FlowFrameLoopStorePlan::GrantNodeSlot {
                expected_run_state,
                next_run_state,
                frame_id,
                expected_frame,
                next_frame,
                ..
            }
            | FlowFrameLoopStorePlan::StartLoop {
                expected_run_state,
                next_run_state,
                frame_id,
                expected_frame,
                next_frame,
                ..
            } => self
                .run_store
                .cas_grant_node_slot(
                    run_id,
                    expected_run_state,
                    next_run_state.clone(),
                    frame_id,
                    expected_frame,
                    next_frame.clone(),
                )
                .await?,
            _ => false,
        };
        if won {
            self.commit_prepared_dsl_input(prepared);
        }
        Ok(won)
    }
}
"#,
    )
    .expect("write actor store plan or-pattern variant union");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_grant_node_slot(")
        }),
        "expected store-plan or-pattern variant union to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_reassigned_plan_parameter() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
impl MobActor {
    async fn commit_flow_frame_store_plan_in_actor(
        &mut self,
        run_id: &RunId,
        mut plan: FlowFrameLoopStorePlan,
    ) -> Result<bool, MobError> {
        let prepared =
            self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
        plan = replacement_plan();
        let won = match &plan {
            FlowFrameLoopStorePlan::RunStateOnly {
                expected_run_state,
                next_run_state,
                ..
            } => self
                .run_store
                .cas_flow_state(run_id, expected_run_state, next_run_state)
                .await?,
            _ => false,
        };
        if won {
            self.commit_prepared_dsl_input(prepared);
        }
        Ok(won)
    }
}
"#,
    )
    .expect("write actor store plan reassigned plan parameter");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected actor store-plan CAS with reassigned plan parameter to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_previously_mutated_plan_parameter()
{
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
impl MobActor {
    async fn commit_flow_frame_store_plan_in_actor(
        &mut self,
        run_id: &RunId,
        mut plan: FlowFrameLoopStorePlan,
    ) -> Result<bool, MobError> {
        std::mem::replace(&mut plan, replacement_plan());
        let prepared =
            self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
        let won = match &plan {
            FlowFrameLoopStorePlan::RunStateOnly {
                expected_run_state,
                next_run_state,
                ..
            } => self
                .run_store
                .cas_flow_state(run_id, expected_run_state, next_run_state)
                .await?,
            _ => false,
        };
        if won {
            self.commit_prepared_dsl_input(prepared);
        }
        Ok(won)
    }
}
"#,
    )
    .expect("write actor store plan mutated plan parameter before prepare");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected actor store-plan CAS with previously mutated plan parameter to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_shadowed_plan_parameter() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
impl MobActor {
    async fn commit_flow_frame_store_plan_in_actor(
        &mut self,
        run_id: &RunId,
        plan: FlowFrameLoopStorePlan,
        fake_plan: FlowFrameLoopStorePlan,
    ) -> Result<bool, MobError> {
        let plan = fake_plan;
        let prepared =
            self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
        let won = match &plan {
            FlowFrameLoopStorePlan::RunStateOnly {
                expected_run_state,
                next_run_state,
                ..
            } => self
                .run_store
                .cas_flow_state(run_id, expected_run_state, next_run_state)
                .await?,
            _ => false,
        };
        if won {
            self.commit_prepared_dsl_input(prepared);
        }
        Ok(won)
    }
}
"#,
    )
    .expect("write actor store plan shadowed plan parameter");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected actor store-plan CAS using a shadowed plan parameter to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_fake_plan_parameter() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
impl MobActor {
    async fn commit_flow_frame_store_plan_in_actor(
        &mut self,
        run_id: &RunId,
        plan: FlowFrameLoopStorePlan,
        fake_plan: FlowFrameLoopStorePlan,
    ) -> Result<bool, MobError> {
        let prepared =
            self.prepare_dsl_inputs(fake_plan.machine_inputs(), "flow_frame_loop_store_plan")?;
        let won = match &fake_plan {
            FlowFrameLoopStorePlan::RunStateOnly {
                expected_run_state,
                next_run_state,
                ..
            } => self
                .run_store
                .cas_flow_state(run_id, expected_run_state, next_run_state)
                .await?,
            _ => false,
        };
        if won {
            self.commit_prepared_dsl_input(prepared);
        }
        Ok(won)
    }
}
"#,
    )
    .expect("write actor store plan fake plan parameter");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected actor store-plan CAS using a fake plan parameter to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_nested_prepared_reassignment() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
impl MobActor {
    async fn commit_flow_frame_store_plan_in_actor(
        &mut self,
        run_id: &RunId,
        plan: FlowFrameLoopStorePlan,
    ) -> Result<bool, MobError> {
        let mut prepared =
            self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
        {
            let fake_prepared =
                self.prepare_dsl_inputs(Vec::new(), "flow_frame_loop_store_plan")?;
            prepared = fake_prepared;
        }
        let won = match &plan {
            FlowFrameLoopStorePlan::RunStateOnly {
                expected_run_state,
                next_run_state,
                ..
            } => self
                .run_store
                .cas_flow_state(run_id, expected_run_state, next_run_state)
                .await?,
            _ => false,
        };
        if won {
            self.commit_prepared_dsl_input(prepared);
        }
        Ok(won)
    }
}
"#,
    )
    .expect("write actor store plan nested prepared reassignment");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected actor store-plan CAS after nested prepared reassignment to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_nested_match_result_reassignment()
{
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
impl MobActor {
    async fn commit_flow_frame_store_plan_in_actor(
        &mut self,
        run_id: &RunId,
        plan: FlowFrameLoopStorePlan,
        should_commit: bool,
    ) -> Result<bool, MobError> {
        let prepared =
            self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
        let mut won = match &plan {
            FlowFrameLoopStorePlan::RunStateOnly {
                expected_run_state,
                next_run_state,
                ..
            } => self
                .run_store
                .cas_flow_state(run_id, expected_run_state, next_run_state)
                .await?,
            _ => false,
        };
        {
            won = should_commit;
        }
        if won {
            self.commit_prepared_dsl_input(prepared);
        }
        Ok(won)
    }
}
"#,
    )
    .expect("write actor store plan nested match result reassignment");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected actor store-plan CAS after nested match-result reassignment to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_fake_cas_receiver() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
impl MobActor {
    async fn commit_flow_frame_store_plan_in_actor(
        &mut self,
        run_id: &RunId,
        plan: FlowFrameLoopStorePlan,
    ) -> Result<bool, MobError> {
        let prepared =
            self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
        let fake_store = FakeRunStore::default();
        let won = match &plan {
            FlowFrameLoopStorePlan::RunStateOnly {
                expected_run_state,
                next_run_state,
                ..
            } => fake_store
                .cas_flow_state(run_id, expected_run_state, next_run_state)
                .await?,
            _ => false,
        };
        if won {
            self.commit_prepared_dsl_input(prepared);
        }
        Ok(won)
    }
}
"#,
    )
    .expect("write actor store plan fake cas receiver");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected actor store-plan CAS on fake receiver to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_shadowed_prepared_commit_after_cas()
 {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
impl MobActor {
    async fn commit_flow_frame_store_plan_in_actor(
        &mut self,
        run_id: &RunId,
        plan: FlowFrameLoopStorePlan,
    ) -> Result<bool, MobError> {
        let prepared =
            self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
        let won = match &plan {
            FlowFrameLoopStorePlan::RunStateOnly {
                expected_run_state,
                next_run_state,
                ..
            } => self
                .run_store
                .cas_flow_state(run_id, expected_run_state, next_run_state)
                .await?,
            _ => false,
        };
        if won {
            let prepared =
                self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
            self.commit_prepared_dsl_input(prepared);
        }
        Ok(won)
    }
}
"#,
    )
    .expect("write actor store plan shadowed prepared commit after cas");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected actor store-plan CAS committed with a post-CAS shadowed prepared input to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_mutated_prepared_field() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
impl MobActor {
    async fn commit_flow_frame_store_plan_in_actor(
        &mut self,
        run_id: &RunId,
        plan: FlowFrameLoopStorePlan,
    ) -> Result<bool, MobError> {
        let mut prepared =
            self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
        prepared.effects.clear();
        let won = match &plan {
            FlowFrameLoopStorePlan::RunStateOnly {
                expected_run_state,
                next_run_state,
                ..
            } => self
                .run_store
                .cas_flow_state(run_id, expected_run_state, next_run_state)
                .await?,
            _ => false,
        };
        if won {
            self.commit_prepared_dsl_input(prepared);
        }
        Ok(won)
    }
}
"#,
    )
    .expect("write actor store plan mutated prepared field");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected actor store-plan CAS committed with mutated prepared field to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_mutable_prepared_field_reference()
{
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
impl MobActor {
    async fn commit_flow_frame_store_plan_in_actor(
        &mut self,
        run_id: &RunId,
        plan: FlowFrameLoopStorePlan,
        fake_authority: MobMachineAuthority,
    ) -> Result<bool, MobError> {
        let mut prepared =
            self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
        let _old = std::mem::replace(&mut prepared.authority, fake_authority);
        let won = match &plan {
            FlowFrameLoopStorePlan::RunStateOnly {
                expected_run_state,
                next_run_state,
                ..
            } => self
                .run_store
                .cas_flow_state(run_id, expected_run_state, next_run_state)
                .await?,
            _ => false,
        };
        if won {
            self.commit_prepared_dsl_input(prepared);
        }
        Ok(won)
    }
}
"#,
    )
    .expect("write actor store plan mutable prepared field reference");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected actor store-plan CAS committed with mutable prepared field reference to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_prepared_as_mut_slice() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
impl MobActor {
    async fn commit_flow_frame_store_plan_in_actor(
        &mut self,
        run_id: &RunId,
        plan: FlowFrameLoopStorePlan,
        replacement_effect: PreparedDslEffect,
    ) -> Result<bool, MobError> {
        let mut prepared =
            self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
        prepared.effects.as_mut_slice()[0] = replacement_effect;
        let won = match &plan {
            FlowFrameLoopStorePlan::RunStateOnly {
                expected_run_state,
                next_run_state,
                ..
            } => self
                .run_store
                .cas_flow_state(run_id, expected_run_state, next_run_state)
                .await?,
            _ => false,
        };
        if won {
            self.commit_prepared_dsl_input(prepared);
        }
        Ok(won)
    }
}
"#,
    )
    .expect("write actor store plan prepared as_mut_slice mutation");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected actor store-plan CAS with as_mut_slice-mutated prepared input to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_shadowed_won_guard_after_cas() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
impl MobActor {
    async fn commit_flow_frame_store_plan_in_actor(
        &mut self,
        run_id: &RunId,
        plan: FlowFrameLoopStorePlan,
    ) -> Result<bool, MobError> {
        let prepared =
            self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
        let won = match &plan {
            FlowFrameLoopStorePlan::RunStateOnly {
                expected_run_state,
                next_run_state,
                ..
            } => self
                .run_store
                .cas_flow_state(run_id, expected_run_state, next_run_state)
                .await?,
            _ => false,
        };
        {
            let won = match &plan {
                FlowFrameLoopStorePlan::RunStateOnly { .. } => true,
                _ => false,
            };
            if won {
                self.commit_prepared_dsl_input(prepared);
            }
        }
        Ok(won)
    }
}
"#,
    )
    .expect("write actor store plan shadowed won guard after cas");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected actor store-plan CAS committed under a post-CAS shadowed won guard to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_fake_plan_type_owner() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
mod fake {
    pub enum FlowFrameLoopStorePlan {
        RunStateOnly {
            expected_run_state: flow_run::State,
            next_run_state: flow_run::State,
        },
    }
}

impl MobActor {
    async fn commit_flow_frame_store_plan_in_actor(
        &mut self,
        run_id: &RunId,
        plan: fake::FlowFrameLoopStorePlan,
    ) -> Result<bool, MobError> {
        let prepared =
            self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
        let won = match &plan {
            fake::FlowFrameLoopStorePlan::RunStateOnly {
                expected_run_state,
                next_run_state,
            } => self
                .run_store
                .cas_flow_state(run_id, expected_run_state, next_run_state)
                .await?,
        };
        if won {
            self.commit_prepared_dsl_input(prepared);
        }
        Ok(won)
    }
}
"#,
    )
    .expect("write actor store plan fake plan type owner");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected actor store-plan CAS with fake FlowFrameLoopStorePlan owner to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_bare_shadowed_plan_type_owner() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
pub enum FlowFrameLoopStorePlan {
    RunStateOnly {
        expected_run_state: flow_run::State,
        next_run_state: flow_run::State,
    },
}

impl FlowFrameLoopStorePlan {
    fn machine_inputs(&self) -> Vec<MobMachineInput> {
        Vec::new()
    }
}

impl MobActor {
    async fn commit_flow_frame_store_plan_in_actor(
        &mut self,
        run_id: &RunId,
        plan: FlowFrameLoopStorePlan,
    ) -> Result<bool, MobError> {
        let prepared =
            self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
        let won = match &plan {
            FlowFrameLoopStorePlan::RunStateOnly {
                expected_run_state,
                next_run_state,
            } => self
                .run_store
                .cas_flow_state(run_id, expected_run_state, next_run_state)
                .await?,
        };
        if won {
            self.commit_prepared_dsl_input(prepared);
        }
        Ok(won)
    }
}
"#,
    )
    .expect("write actor store plan bare shadowed plan type owner");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected actor store-plan CAS with bare shadowed FlowFrameLoopStorePlan owner to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_ambiguous_glob_plan_type_owner() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
mod fake {
    pub enum FlowFrameLoopStorePlan {
        RunStateOnly {
            expected_run_state: flow_run::State,
            next_run_state: flow_run::State,
        },
    }

    impl FlowFrameLoopStorePlan {
        pub fn machine_inputs(&self) -> Vec<MobMachineInput> {
            Vec::new()
        }
    }
}

use fake::*;

impl MobActor {
    async fn commit_flow_frame_store_plan_in_actor(
        &mut self,
        run_id: &RunId,
        plan: FlowFrameLoopStorePlan,
    ) -> Result<bool, MobError> {
        let prepared =
            self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
        let won = match &plan {
            FlowFrameLoopStorePlan::RunStateOnly {
                expected_run_state,
                next_run_state,
            } => self
                .run_store
                .cas_flow_state(run_id, expected_run_state, next_run_state)
                .await?,
        };
        if won {
            self.commit_prepared_dsl_input(prepared);
        }
        Ok(won)
    }
}
"#,
    )
    .expect("write actor store plan ambiguous-glob plan type owner");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected actor store-plan CAS with ambiguous-glob FlowFrameLoopStorePlan owner to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_fake_run_id_type_owner() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
mod fake {
    pub struct RunId;
}

impl MobActor {
    async fn commit_flow_frame_store_plan_in_actor(
        &mut self,
        run_id: &fake::RunId,
        plan: FlowFrameLoopStorePlan,
    ) -> Result<bool, MobError> {
        let prepared =
            self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
        let won = match &plan {
            FlowFrameLoopStorePlan::RunStateOnly {
                expected_run_state,
                next_run_state,
                ..
            } => self
                .run_store
                .cas_flow_state(run_id, expected_run_state, next_run_state)
                .await?,
            _ => false,
        };
        if won {
            self.commit_prepared_dsl_input(prepared);
        }
        Ok(won)
    }
}
"#,
    )
    .expect("write actor store plan fake run id type owner");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected actor store-plan CAS with fake RunId owner to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_ambiguous_glob_run_id_type_owner()
{
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
mod fake {
    pub struct RunId;
}

use fake::*;

impl MobActor {
    async fn commit_flow_frame_store_plan_in_actor(
        &mut self,
        run_id: &RunId,
        plan: FlowFrameLoopStorePlan,
    ) -> Result<bool, MobError> {
        let prepared =
            self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
        let won = match &plan {
            FlowFrameLoopStorePlan::RunStateOnly {
                expected_run_state,
                next_run_state,
                ..
            } => self
                .run_store
                .cas_flow_state(run_id, expected_run_state, next_run_state)
                .await?,
            _ => false,
        };
        if won {
            self.commit_prepared_dsl_input(prepared);
        }
        Ok(won)
    }
}
"#,
    )
    .expect("write actor store plan ambiguous-glob RunId type owner");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected actor store-plan CAS with ambiguous-glob RunId owner to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_bare_shadowed_run_id_type_owner() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
pub struct RunId;

impl MobActor {
    async fn commit_flow_frame_store_plan_in_actor(
        &mut self,
        run_id: &RunId,
        plan: FlowFrameLoopStorePlan,
    ) -> Result<bool, MobError> {
        let prepared =
            self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
        let won = match &plan {
            FlowFrameLoopStorePlan::RunStateOnly {
                expected_run_state,
                next_run_state,
                ..
            } => self
                .run_store
                .cas_flow_state(run_id, expected_run_state, next_run_state)
                .await?,
            _ => false,
        };
        if won {
            self.commit_prepared_dsl_input(prepared);
        }
        Ok(won)
    }
}
"#,
    )
    .expect("write actor store plan bare shadowed run id type owner");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected actor store-plan CAS with bare shadowed RunId owner to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_root_free_function_spoof() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn commit_flow_frame_store_plan_in_actor(
    &mut self,
    run_id: &RunId,
    plan: FlowFrameLoopStorePlan,
) -> Result<bool, MobError> {
    let prepared =
        self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
    let won = match &plan {
        FlowFrameLoopStorePlan::RunStateOnly {
            expected_run_state,
            next_run_state,
            ..
        } => self
            .run_store
            .cas_flow_state(run_id, expected_run_state, next_run_state)
            .await?,
        _ => false,
    };
    if won {
        self.commit_prepared_dsl_input(prepared);
    }
    Ok(won)
}
"#,
    )
    .expect("write root free actor store plan spoof");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected root free function not to authorize actor store-plan CAS, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_commit_without_freshness_check() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
impl MobActor {
    async fn commit_flow_frame_store_plan_in_actor(
        &mut self,
        run_id: &RunId,
        plan: FlowFrameLoopStorePlan,
    ) -> Result<bool, MobError> {
        let prepared =
            self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
        let authority_inputs = plan.machine_inputs().to_vec();
        let won = match &plan {
            FlowFrameLoopStorePlan::RunStateOnly {
                expected_run_state,
                next_run_state,
                ..
            } => self
                .run_store
                .cas_flow_state_with_authority(
                    run_id,
                    expected_run_state,
                    next_run_state,
                    authority_inputs,
                )
                .await?,
            _ => false,
        };
        if won {
            self.commit_prepared_dsl_input(prepared);
        }
        Ok(won)
    }
}
"#,
    )
    .expect("write actor store plan commit without freshness check");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains("FlowFrameLoopStorePlan::RunStateOnly")
        }),
        "expected actor store-plan commit without freshness check to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_arm_true_without_cas() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
impl MobActor {
    async fn commit_flow_frame_store_plan_in_actor(
        &mut self,
        run_id: &RunId,
        plan: FlowFrameLoopStorePlan,
    ) -> Result<bool, MobError> {
        let prepared =
            self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
        let won = match &plan {
            FlowFrameLoopStorePlan::RunStateOnly {
                expected_run_state: _,
                next_run_state: _,
                ..
            } => true,
            _ => false,
        };
        if won {
            self.commit_prepared_dsl_input(prepared);
        }
        Ok(won)
    }
}
"#,
    )
    .expect("write actor store plan true without CAS");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains("FlowFrameLoopStorePlan::RunStateOnly")
        }),
        "expected actor store-plan arm returning true without CAS to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_branch_true_without_cas() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
impl MobActor {
    async fn commit_flow_frame_store_plan_in_actor(
        &mut self,
        run_id: &RunId,
        plan: FlowFrameLoopStorePlan,
    ) -> Result<bool, MobError> {
        let prepared =
            self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
        let authority_inputs = plan.machine_inputs().to_vec();
        let won = match &plan {
            FlowFrameLoopStorePlan::RunStateOnly {
                expected_run_state,
                next_run_state,
                ..
            } => {
                if bypass() {
                    true
                } else {
                    self
                        .run_store
                        .cas_flow_state_with_authority(
                            run_id,
                            expected_run_state,
                            next_run_state,
                            authority_inputs.clone(),
                        )
                        .await?
                }
            }
            _ => false,
        };
        if won {
            self.commit_prepared_dsl_input(prepared);
        }
        Ok(won)
    }
}
"#,
    )
    .expect("write actor store plan branch true without CAS");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains("FlowFrameLoopStorePlan::RunStateOnly")
        }),
        "expected actor store-plan branch returning true without CAS to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_insert_frame_non_none_expected() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
impl MobActor {
    async fn commit_flow_frame_store_plan_in_actor(
        &mut self,
        run_id: &RunId,
        plan: FlowFrameLoopStorePlan,
    ) -> Result<bool, MobError> {
        let prepared =
            self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
        let authority_inputs = plan.machine_inputs().to_vec();
        let won = match &plan {
            FlowFrameLoopStorePlan::InsertFrame {
                frame_id,
                initial_frame,
                ..
            } => self
                .run_store
                .cas_frame_state_with_authority(
                    run_id,
                    frame_id,
                    Some(initial_frame),
                    initial_frame.clone(),
                    authority_inputs.clone(),
                )
                .await?,
            _ => false,
        };
        if won {
            self.commit_prepared_dsl_input(prepared);
        }
        Ok(won)
    }
}
"#,
    )
    .expect("write actor store plan InsertFrame non-None expected");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_frame_state_with_authority(")
        }),
        "expected actor store-plan InsertFrame CAS with non-None expected frame to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_insert_frame_fake_none_expected() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
impl MobActor {
    async fn commit_flow_frame_store_plan_in_actor(
        &mut self,
        run_id: &RunId,
        plan: FlowFrameLoopStorePlan,
    ) -> Result<bool, MobError> {
        if !self
            .flow_frame_store_plan_expected_matches(run_id, &plan)
            .await?
        {
            return Ok(false);
        }
        let prepared =
            self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
        let authority_inputs = plan.machine_inputs().to_vec();
        let won = match &plan {
            FlowFrameLoopStorePlan::InsertFrame {
                frame_id,
                initial_frame,
                ..
            } => self
                .run_store
                .cas_frame_state_with_authority(
                    run_id,
                    frame_id,
                    fake::None,
                    initial_frame.clone(),
                    authority_inputs,
                )
                .await?,
            _ => false,
        };
        if won {
            self.commit_prepared_dsl_input(prepared);
        }
        Ok(won)
    }
}
"#,
    )
    .expect("write actor store plan insert frame fake None expected");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains("FlowFrameLoopStorePlan::InsertFrame")
        }),
        "expected actor store-plan InsertFrame CAS with fake None expected arg to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_cas_before_prepare() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
impl MobActor {
async fn commit_flow_frame_store_plan_in_actor(
    &mut self,
    run_id: &RunId,
    plan: FlowFrameLoopStorePlan,
) -> Result<bool, MobError> {
    let won = match &plan {
        FlowFrameLoopStorePlan::RunStateOnly {
            expected_run_state,
            next_run_state,
            ..
        } => self
            .run_store
            .cas_flow_state(run_id, expected_run_state, next_run_state)
            .await?,
        _ => false,
    };
    let prepared =
        self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
    if won {
        self.commit_prepared_dsl_input(prepared);
    }
    Ok(won)
}
}
"#,
    )
    .expect("write actor store plan commit before prepare");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected actor store-plan CAS before prepared MobMachine input authority to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_commit_before_cas() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
impl MobActor {
async fn commit_flow_frame_store_plan_in_actor(
    &mut self,
    run_id: &RunId,
    plan: FlowFrameLoopStorePlan,
) -> Result<bool, MobError> {
    let prepared =
        self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
    self.commit_prepared_dsl_input(prepared);
    let won = match &plan {
        FlowFrameLoopStorePlan::RunStateOnly {
            expected_run_state,
            next_run_state,
            ..
        } => self
            .run_store
            .cas_flow_state(run_id, expected_run_state, next_run_state)
            .await?,
        _ => false,
    };
    Ok(won)
}
}
"#,
    )
    .expect("write actor store plan commit before cas");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected actor store-plan CAS after an already-committed prepared input to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_unrelated_raw_cas() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
impl MobActor {
async fn commit_flow_frame_store_plan_in_actor(
    &mut self,
    run_id: &RunId,
    plan: FlowFrameLoopStorePlan,
    expected_run_state: &flow_run::State,
    next_run_state: &flow_run::State,
) -> Result<bool, MobError> {
    let prepared =
        self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
    let won = self
        .run_store
        .cas_flow_state(run_id, expected_run_state, next_run_state)
        .await?;
    if won {
        self.commit_prepared_dsl_input(prepared);
    }
    Ok(won)
}
}
"#,
    )
    .expect("write actor store plan unrelated raw cas");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected actor store-plan raw CAS unrelated to the plan match to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_unrelated_raw_cas_inside_match() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
impl MobActor {
async fn commit_flow_frame_store_plan_in_actor(
    &mut self,
    run_id: &RunId,
    plan: FlowFrameLoopStorePlan,
    expected_run_state: &flow_run::State,
    next_run_state: &flow_run::State,
) -> Result<bool, MobError> {
    let prepared =
        self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
    let won = match &plan {
        FlowFrameLoopStorePlan::RunStateOnly {
            expected_run_state: _plan_expected_run_state,
            next_run_state: _plan_next_run_state,
            ..
        } => self
            .run_store
            .cas_flow_state(run_id, expected_run_state, next_run_state)
            .await?,
        _ => false,
    };
    if won {
        self.commit_prepared_dsl_input(prepared);
    }
    Ok(won)
}
}
"#,
    )
    .expect("write actor store plan unrelated raw cas inside match");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected actor store-plan raw CAS unrelated to the matched plan arm to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_shadowed_compound_cas_args() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn commit_flow_frame_store_plan_in_actor(
    &mut self,
    run_id: &RunId,
    plan: FlowFrameLoopStorePlan,
) -> Result<bool, MobError> {
    let prepared =
        self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
    let won = match &plan {
        FlowFrameLoopStorePlan::GrantNodeSlot {
            expected_run_state,
            next_run_state,
            frame_id,
            expected_frame,
            next_frame,
            ..
        } => {
            let next_run_state = flow_run::State::default();
            self
                .run_store
                .cas_grant_node_slot(
                    run_id,
                    expected_run_state,
                    next_run_state,
                    frame_id,
                    expected_frame,
                    next_frame.clone(),
                )
                .await?
        }
        _ => false,
    };
    if won {
        self.commit_prepared_dsl_input(prepared);
    }
    Ok(won)
}
"#,
    )
    .expect("write actor store plan shadowed compound cas args");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_grant_node_slot(")
        }),
        "expected actor store-plan compound CAS with shadowed plan state arg to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_unrelated_compound_cas_id_arg() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn commit_flow_frame_store_plan_in_actor(
    &mut self,
    run_id: &RunId,
    plan: FlowFrameLoopStorePlan,
    unrelated_frame_id: &FrameId,
) -> Result<bool, MobError> {
    let prepared =
        self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
    let authority_inputs = plan.machine_inputs().to_vec();
    let won = match &plan {
        FlowFrameLoopStorePlan::GrantNodeSlot {
            expected_run_state,
            next_run_state,
            frame_id: _frame_id,
            expected_frame,
            next_frame,
            ..
        } => self
            .run_store
            .cas_grant_node_slot_with_authority(
                run_id,
                expected_run_state,
                next_run_state.clone(),
                unrelated_frame_id,
                expected_frame,
                next_frame.clone(),
                authority_inputs,
            )
            .await?,
        _ => false,
    };
    if won {
        self.commit_prepared_dsl_input(prepared);
    }
    Ok(won)
}
"#,
    )
    .expect("write actor store plan unrelated compound CAS id arg");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_grant_node_slot_with_authority(")
        }),
        "expected actor store-plan compound CAS with unrelated frame id arg to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_unrelated_map_cas_arg() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn commit_flow_frame_store_plan_in_actor(
    &mut self,
    run_id: &RunId,
    plan: FlowFrameLoopStorePlan,
    unrelated_loop_id: &LoopId,
) -> Result<bool, MobError> {
    let prepared =
        self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
    let authority_inputs = plan.machine_inputs().to_vec();
    let won = match &plan {
        FlowFrameLoopStorePlan::CompleteStepAndRecordOutput {
            frame_id,
            expected_frame,
            next_frame,
            step_output_key,
            step_output,
            loop_context,
            ..
        } => self
            .run_store
            .cas_complete_step_and_record_output_with_authority(
                run_id,
                frame_id,
                expected_frame,
                next_frame.clone(),
                step_output_key.clone(),
                step_output.clone(),
                loop_context.as_ref().map(|_| (unrelated_loop_id, 0)),
                authority_inputs,
            )
            .await?,
        _ => false,
    };
    if won {
        self.commit_prepared_dsl_input(prepared);
    }
    Ok(won)
}
"#,
    )
    .expect("write actor store plan unrelated map CAS arg");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_complete_step_and_record_output_with_authority(")
        }),
        "expected actor store-plan CAS with unrelated map output to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_deferred_cas() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn commit_flow_frame_store_plan_in_actor(
    &mut self,
    run_id: &RunId,
    plan: FlowFrameLoopStorePlan,
) -> Result<bool, MobError> {
    let prepared =
        self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
    let won = match &plan {
        FlowFrameLoopStorePlan::RunStateOnly {
            expected_run_state,
            next_run_state,
            ..
        } => {
            let _deferred = || {
                self.run_store.cas_flow_state(run_id, expected_run_state, next_run_state)
            };
            false
        }
        _ => false,
    };
    if won {
        self.commit_prepared_dsl_input(prepared);
    }
    Ok(won)
}
"#,
    )
    .expect("write actor store plan deferred CAS");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected actor store-plan CAS hidden in deferred closure to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_deferred_commit_proof() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
impl MobActor {
    async fn commit_flow_frame_store_plan_in_actor(
        &mut self,
        run_id: &RunId,
        plan: FlowFrameLoopStorePlan,
    ) -> Result<bool, MobError> {
        let prepared =
            self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
        let won = match &plan {
            FlowFrameLoopStorePlan::RunStateOnly {
                expected_run_state,
                next_run_state,
                ..
            } => self
                .run_store
                .cas_flow_state(run_id, expected_run_state, next_run_state)
                .await?,
            _ => false,
        };
        if won {
            let _deferred = || {
                self.commit_prepared_dsl_input(prepared);
            };
        }
        Ok(won)
    }
}
"#,
    )
    .expect("write actor store plan deferred commit proof");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected actor store-plan commit hidden in a deferred closure not to authorize the CAS, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_unreachable_commit_proof() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
impl MobActor {
    async fn commit_flow_frame_store_plan_in_actor(
        &mut self,
        run_id: &RunId,
        plan: FlowFrameLoopStorePlan,
    ) -> Result<bool, MobError> {
        let prepared =
            self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
        let won = match &plan {
            FlowFrameLoopStorePlan::RunStateOnly {
                expected_run_state,
                next_run_state,
                ..
            } => self
                .run_store
                .cas_flow_state(run_id, expected_run_state, next_run_state)
                .await?,
            _ => false,
        };
        if won {
            return Ok(won);
            self.commit_prepared_dsl_input(prepared);
        }
        Ok(won)
    }
}
"#,
    )
    .expect("write actor store plan unreachable commit proof");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected unreachable actor store-plan commit not to authorize the CAS, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_commit_without_won_guard() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn commit_flow_frame_store_plan_in_actor(
    &mut self,
    run_id: &RunId,
    plan: FlowFrameLoopStorePlan,
    should_commit: bool,
) -> Result<bool, MobError> {
    let prepared =
        self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
    let won = match &plan {
        FlowFrameLoopStorePlan::RunStateOnly {
            expected_run_state,
            next_run_state,
            ..
        } => self
            .run_store
            .cas_flow_state(run_id, expected_run_state, next_run_state)
            .await?,
        _ => false,
    };
    if should_commit {
        self.commit_prepared_dsl_input(prepared);
    }
    Ok(won)
}
"#,
    )
    .expect("write actor store plan commit without won guard");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected actor store-plan commit not guarded by CAS result to reject the CAS authorization, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_cas_not_feeding_won() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn commit_flow_frame_store_plan_in_actor(
    &mut self,
    run_id: &RunId,
    plan: FlowFrameLoopStorePlan,
) -> Result<bool, MobError> {
    let prepared =
        self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
    let won = match &plan {
        FlowFrameLoopStorePlan::RunStateOnly {
            expected_run_state,
            next_run_state,
            ..
        } => {
            self
                .run_store
                .cas_flow_state(run_id, expected_run_state, next_run_state)
                .await?;
            false
        }
        _ => false,
    };
    if won {
        self.commit_prepared_dsl_input(prepared);
    }
    Ok(won)
}
"#,
    )
    .expect("write actor store plan cas not feeding won");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected actor store-plan CAS whose result does not feed won to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_tuple_tail_discards_cas_result() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn commit_flow_frame_store_plan_in_actor(
    &mut self,
    run_id: &RunId,
    plan: FlowFrameLoopStorePlan,
) -> Result<bool, MobError> {
    let prepared =
        self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
    let won = match &plan {
        FlowFrameLoopStorePlan::RunStateOnly {
            expected_run_state,
            next_run_state,
            ..
        } => (self
            .run_store
            .cas_flow_state(run_id, expected_run_state, next_run_state)
            .await?, false).1,
        _ => false,
    };
    if won {
        self.commit_prepared_dsl_input(prepared);
    }
    Ok(won)
}
"#,
    )
    .expect("write actor store plan tuple tail discarding cas result");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected actor store-plan CAS discarded by tuple-field tail expression to be rejected, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_shadowed_won_commit_guard() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn commit_flow_frame_store_plan_in_actor(
    &mut self,
    run_id: &RunId,
    plan: FlowFrameLoopStorePlan,
    should_commit: bool,
) -> Result<bool, MobError> {
    let prepared =
        self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
    let won = match &plan {
        FlowFrameLoopStorePlan::RunStateOnly {
            expected_run_state,
            next_run_state,
            ..
        } => self
            .run_store
            .cas_flow_state(run_id, expected_run_state, next_run_state)
            .await?,
        _ => false,
    };
    let won = should_commit;
    if won {
        self.commit_prepared_dsl_input(prepared);
    }
    Ok(won)
}
"#,
    )
    .expect("write actor store plan shadowed won commit guard");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected actor store-plan commit guarded by a shadowed CAS result name to reject the CAS authorization, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_reassigned_won_commit_guard() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn commit_flow_frame_store_plan_in_actor(
    &mut self,
    run_id: &RunId,
    plan: FlowFrameLoopStorePlan,
    should_commit: bool,
) -> Result<bool, MobError> {
    let prepared =
        self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
    let mut won = match &plan {
        FlowFrameLoopStorePlan::RunStateOnly {
            expected_run_state,
            next_run_state,
            ..
        } => self
            .run_store
            .cas_flow_state(run_id, expected_run_state, next_run_state)
            .await?,
        _ => false,
    };
    won = should_commit;
    if won {
        self.commit_prepared_dsl_input(prepared);
    }
    Ok(won)
}
"#,
    )
    .expect("write actor store plan reassigned won commit guard");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected actor store-plan commit guarded by reassigned CAS result name to reject the CAS authorization, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_destructured_won_commit_guard() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
impl MobActor {
    async fn commit_flow_frame_store_plan_in_actor(
        &mut self,
        run_id: &RunId,
        plan: FlowFrameLoopStorePlan,
        should_commit: bool,
    ) -> Result<bool, MobError> {
        let prepared =
            self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
        let won = match &plan {
            FlowFrameLoopStorePlan::RunStateOnly {
                expected_run_state,
                next_run_state,
                ..
            } => self
                .run_store
                .cas_flow_state(run_id, expected_run_state, next_run_state)
                .await?,
            _ => false,
        };
        let (won,) = (should_commit,);
        if won {
            self.commit_prepared_dsl_input(prepared);
        }
        Ok(won)
    }
}
"#,
    )
    .expect("write actor store plan destructured won commit guard");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected actor store-plan commit guarded by destructured CAS result name to reject the CAS authorization, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_autoref_mutated_won_guard() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
impl MobActor {
    async fn commit_flow_frame_store_plan_in_actor(
        &mut self,
        run_id: &RunId,
        plan: FlowFrameLoopStorePlan,
        should_commit: bool,
    ) -> Result<bool, MobError> {
        let prepared =
            self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
        let mut won = match &plan {
            FlowFrameLoopStorePlan::RunStateOnly {
                expected_run_state,
                next_run_state,
                ..
            } => self
                .run_store
                .cas_flow_state(run_id, expected_run_state, next_run_state)
                .await?,
            _ => false,
        };
        won.clone_from(&should_commit);
        if won {
            self.commit_prepared_dsl_input(prepared);
        }
        Ok(won)
    }
}
"#,
    )
    .expect("write actor store plan autoref-mutated won guard");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected actor store-plan commit guarded by autoref-mutated CAS result name to reject the CAS authorization, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_shadowed_prepared_commit() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn commit_flow_frame_store_plan_in_actor(
    &mut self,
    run_id: &RunId,
    plan: FlowFrameLoopStorePlan,
    fake_prepared: PreparedDslInput,
) -> Result<bool, MobError> {
    let prepared =
        self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
    let won = match &plan {
        FlowFrameLoopStorePlan::RunStateOnly {
            expected_run_state,
            next_run_state,
            ..
        } => self
            .run_store
            .cas_flow_state(run_id, expected_run_state, next_run_state)
            .await?,
        _ => false,
    };
    if won {
        let prepared = fake_prepared;
        self.commit_prepared_dsl_input(prepared);
    }
    Ok(won)
}
"#,
    )
    .expect("write actor store plan shadowed prepared commit");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected actor store-plan commit using shadowed prepared input to reject the CAS authorization, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_reassigned_prepared_commit() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn commit_flow_frame_store_plan_in_actor(
    &mut self,
    run_id: &RunId,
    plan: FlowFrameLoopStorePlan,
    fake_prepared: PreparedDslInput,
) -> Result<bool, MobError> {
    let mut prepared =
        self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
    prepared = fake_prepared;
    let won = match &plan {
        FlowFrameLoopStorePlan::RunStateOnly {
            expected_run_state,
            next_run_state,
            ..
        } => self
            .run_store
            .cas_flow_state(run_id, expected_run_state, next_run_state)
            .await?,
        _ => false,
    };
    if won {
        self.commit_prepared_dsl_input(prepared);
    }
    Ok(won)
}
"#,
    )
    .expect("write actor store plan reassigned prepared commit");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected actor store-plan commit using reassigned prepared input to reject the CAS authorization, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_destructured_prepared_commit() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
impl MobActor {
    async fn commit_flow_frame_store_plan_in_actor(
        &mut self,
        run_id: &RunId,
        plan: FlowFrameLoopStorePlan,
        fake_prepared: PreparedDslInput,
    ) -> Result<bool, MobError> {
        let prepared =
            self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
        let (prepared,) = (fake_prepared,);
        let won = match &plan {
            FlowFrameLoopStorePlan::RunStateOnly {
                expected_run_state,
                next_run_state,
                ..
            } => self
                .run_store
                .cas_flow_state(run_id, expected_run_state, next_run_state)
                .await?,
            _ => false,
        };
        if won {
            self.commit_prepared_dsl_input(prepared);
        }
        Ok(won)
    }
}
"#,
    )
    .expect("write actor store plan destructured prepared commit");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected actor store-plan commit using destructured prepared input to reject the CAS authorization, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_fake_receiver_commit() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn commit_flow_frame_store_plan_in_actor(
    &mut self,
    run_id: &RunId,
    plan: FlowFrameLoopStorePlan,
    fake: FakeActor,
) -> Result<bool, MobError> {
    let prepared =
        self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
    let won = match &plan {
        FlowFrameLoopStorePlan::RunStateOnly {
            expected_run_state,
            next_run_state,
            ..
        } => self
            .run_store
            .cas_flow_state(run_id, expected_run_state, next_run_state)
            .await?,
        _ => false,
    };
    if won {
        fake.commit_prepared_dsl_input(prepared);
    }
    Ok(won)
}
"#,
    )
    .expect("write actor store plan fake receiver commit");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected actor store-plan commit on fake receiver to reject the CAS authorization, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_non_actor_impl_spoof() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
struct FakeActor {
    run_store: Store,
}

impl FakeActor {
    async fn commit_flow_frame_store_plan_in_actor(
        &mut self,
        run_id: &RunId,
        plan: FlowFrameLoopStorePlan,
    ) -> Result<bool, MobError> {
        let prepared =
            self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
        let won = match &plan {
            FlowFrameLoopStorePlan::RunStateOnly {
                expected_run_state,
                next_run_state,
                ..
            } => self
                .run_store
                .cas_flow_state(run_id, expected_run_state, next_run_state)
                .await?,
            _ => false,
        };
        if won {
            self.commit_prepared_dsl_input(prepared);
        }
        Ok(won)
    }
}
"#,
    )
    .expect("write fake actor store plan impl spoof");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected non-MobActor impl not to authorize actor store-plan CAS, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_root_qualified_mob_actor_spoof() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
mod fake {
    pub struct MobActor {
        run_store: Store,
    }
}

impl fake::MobActor {
    async fn commit_flow_frame_store_plan_in_actor(
        &mut self,
        run_id: &RunId,
        plan: FlowFrameLoopStorePlan,
    ) -> Result<bool, MobError> {
        let prepared =
            self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
        let won = match &plan {
            FlowFrameLoopStorePlan::RunStateOnly {
                expected_run_state,
                next_run_state,
                ..
            } => self
                .run_store
                .cas_flow_state(run_id, expected_run_state, next_run_state)
                .await?,
            _ => false,
        };
        if won {
            self.commit_prepared_dsl_input(prepared);
        }
        Ok(won)
    }
}
"#,
    )
    .expect("write root-qualified fake MobActor store plan impl spoof");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected root-qualified fake MobActor impl not to authorize actor store-plan CAS, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_mutated_authority_inputs() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
async fn commit_flow_frame_store_plan_in_actor(
    &mut self,
    run_id: &RunId,
    plan: FlowFrameLoopStorePlan,
) -> Result<bool, MobError> {
    let prepared =
        self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
    let mut authority_inputs = plan.machine_inputs().to_vec();
    authority_inputs.clear();
    let won = match &plan {
        FlowFrameLoopStorePlan::RunStateOnly {
            expected_run_state,
            next_run_state,
            ..
        } => self
            .run_store
            .cas_flow_state_with_authority(
                run_id,
                expected_run_state,
                next_run_state,
                authority_inputs,
            )
            .await?,
        _ => false,
    };
    if won {
        self.commit_prepared_dsl_input(prepared);
    }
    Ok(won)
}
"#,
    )
    .expect("write actor store plan mutated authority inputs");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state_with_authority(")
        }),
        "expected mutated actor store-plan authority inputs to reject the CAS authorization, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_split_off_authority_inputs() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
impl MobActor {
    async fn commit_flow_frame_store_plan_in_actor(
        &mut self,
        run_id: &RunId,
        plan: FlowFrameLoopStorePlan,
    ) -> Result<bool, MobError> {
        let prepared =
            self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
        let mut authority_inputs = plan.machine_inputs().to_vec();
        let _removed = authority_inputs.split_off(0);
        let won = match &plan {
            FlowFrameLoopStorePlan::RunStateOnly {
                expected_run_state,
                next_run_state,
                ..
            } => self
                .run_store
                .cas_flow_state_with_authority(
                    run_id,
                    expected_run_state,
                    next_run_state,
                    authority_inputs,
                )
                .await?,
            _ => false,
        };
        if won {
            self.commit_prepared_dsl_input(prepared);
        }
        Ok(won)
    }
}
"#,
    )
    .expect("write actor store plan split_off-mutated authority inputs");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state_with_authority(")
        }),
        "expected split_off actor store-plan authority input mutation to reject the CAS authorization, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_destructured_authority_inputs() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
impl MobActor {
    async fn commit_flow_frame_store_plan_in_actor(
        &mut self,
        run_id: &RunId,
        plan: FlowFrameLoopStorePlan,
        fake_authority_inputs: Vec<MobMachineInput>,
    ) -> Result<bool, MobError> {
        let prepared =
            self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
        let authority_inputs = plan.machine_inputs().to_vec();
        let (authority_inputs,) = (fake_authority_inputs,);
        let won = match &plan {
            FlowFrameLoopStorePlan::RunStateOnly {
                expected_run_state,
                next_run_state,
                ..
            } => self
                .run_store
                .cas_flow_state_with_authority(
                    run_id,
                    expected_run_state,
                    next_run_state,
                    authority_inputs,
                )
                .await?,
            _ => false,
        };
        if won {
            self.commit_prepared_dsl_input(prepared);
        }
        Ok(won)
    }
}
"#,
    )
    .expect("write actor store plan destructured authority inputs");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state_with_authority(")
        }),
        "expected destructured actor store-plan authority input shadow to reject the CAS authorization, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_index_mutated_authority_inputs() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
impl MobActor {
    async fn commit_flow_frame_store_plan_in_actor(
        &mut self,
        run_id: &RunId,
        plan: FlowFrameLoopStorePlan,
        unrelated_input: MobMachineInput,
    ) -> Result<bool, MobError> {
        let prepared =
            self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
        let mut authority_inputs = plan.machine_inputs().to_vec();
        authority_inputs[0] = unrelated_input;
        let won = match &plan {
            FlowFrameLoopStorePlan::RunStateOnly {
                expected_run_state,
                next_run_state,
                ..
            } => self
                .run_store
                .cas_flow_state_with_authority(
                    run_id,
                    expected_run_state,
                    next_run_state,
                    authority_inputs,
                )
                .await?,
            _ => false,
        };
        if won {
            self.commit_prepared_dsl_input(prepared);
        }
        Ok(won)
    }
}
"#,
    )
    .expect("write actor store plan index-mutated authority inputs");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state_with_authority(")
        }),
        "expected indexed actor store-plan authority input mutation to reject the CAS authorization, got {mismatches:#?}"
    );
}

#[test]
fn live_flow_runtime_projection_ratchet_rejects_actor_store_plan_nested_mob_actor_spoof() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("actor.rs"),
        r#"
mod fake {
    struct MobActor {
        run_store: Store,
    }

    impl MobActor {
        async fn commit_flow_frame_store_plan_in_actor(
            &mut self,
            run_id: &RunId,
            plan: FlowFrameLoopStorePlan,
        ) -> Result<bool, MobError> {
            let prepared =
                self.prepare_dsl_inputs(plan.machine_inputs(), "flow_frame_loop_store_plan")?;
            let won = match &plan {
                FlowFrameLoopStorePlan::RunStateOnly {
                    expected_run_state,
                    next_run_state,
                    ..
                } => self
                    .run_store
                    .cas_flow_state(run_id, expected_run_state, next_run_state)
                    .await?,
                _ => false,
            };
            if won {
                self.commit_prepared_dsl_input(prepared);
            }
            Ok(won)
        }
    }
}
"#,
    )
    .expect("write nested fake MobActor store plan impl spoof");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.iter().any(|mismatch| {
            mismatch.contains("meerkat-mob/src/runtime/actor.rs")
                && mismatch.contains(".cas_flow_state(")
        }),
        "expected nested fake MobActor impl not to authorize actor store-plan CAS, got {mismatches:#?}"
    );
}

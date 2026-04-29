#![allow(clippy::expect_used, clippy::panic)]

use std::fs;

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
fn live_flow_runtime_projection_ratchet_accepts_typed_outcome_commits() {
    let dir = tempdir().expect("tempdir");
    let runtime = dir.path().join("meerkat-mob/src/runtime");
    fs::create_dir_all(&runtime).expect("create runtime dir");
    fs::write(
        runtime.join("flow.rs"),
        r"
async fn good(run_store: Store, run_id: &RunId, run: MobRun, command: MobMachineFlowRunCommand, authority: MobMachineFlowAuthorityToken) -> Result<(), MobError> {
    let outcome = apply_mob_machine_flow_run_command(&run.flow_state, command, authority)?;
    run_store
        .cas_flow_state(run_id, &run.flow_state, &outcome.next_state)
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
fn live_flow_runtime_projection_ratchet_accepts_actor_store_plan_commit() {
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
    .expect("write actor store plan commit");

    let mismatches = collect_direct_flow_reducer_transition_mismatches(dir.path())
        .expect("flow projection mutation mismatches");
    assert!(
        mismatches.is_empty(),
        "expected actor-gated store plan commits to be accepted, got {mismatches:#?}"
    );
}

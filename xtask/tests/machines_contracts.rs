#![allow(clippy::expect_used, clippy::panic)]

use std::fs;

use tempfile::tempdir;
use xtask::machines::*;

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
    let docs = dir.path().join("docs/architecture/0.5");
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
fn canonical_machine_inventory_matches_docs_and_artifact_bundle() {
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
fn compat_kernel_modules_and_flow_runtime_mini_machines_are_deleted() {
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

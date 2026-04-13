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
    assert_eq!(by_name.machines[0].schema.machine, "MeerkatMachine");

    let by_slug = registry
        .select(&SelectionArgs {
            all: false,
            machines: vec!["meerkat_machine".into()],
            compositions: vec![],
        })
        .expect("selection by slug");
    assert_eq!(by_slug.machines.len(), 1);
    assert_eq!(by_slug.machines[0].schema.machine, "MeerkatMachine");
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
    assert!(seam_model.contains("work_completed_reaches_mob"));
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

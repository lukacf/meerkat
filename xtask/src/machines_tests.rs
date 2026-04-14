#![allow(clippy::expect_used, clippy::panic)]

use std::fs;

use super::*;
use tempfile::tempdir;

#[test]
fn snake_case_handles_kernel_machine_names_and_composition_names() {
    assert_eq!(machine_slug("MeerkatMachine"), "meerkat_machine");
    assert_eq!(machine_slug("MobMachine"), "mob_machine");
    assert_eq!(composition_slug("meerkat_mob_seam"), "meerkat_mob_seam");
    assert_eq!(to_snake_case("Mob Machine"), "mob_machine");
}

#[test]
fn output_paths_land_under_canonical_specs_dirs() {
    let root = repo_root().expect("repo root");
    assert_eq!(
        machine_model_path(&root, "meerkat_machine"),
        root.join("specs/machines/meerkat_machine/model.tla")
    );
    assert_eq!(
        composition_model_path(&root, "meerkat_mob_seam"),
        root.join("specs/compositions/meerkat_mob_seam/model.tla")
    );
}

#[test]
fn owner_tests_are_registered_only_for_remaining_canonical_surfaces() {
    let meerkat = owner_test_specs_for_machine("meerkat_machine");
    assert_eq!(meerkat.len(), 6);
    assert!(
        meerkat
            .iter()
            .all(|spec| spec.package == "meerkat-integration-tests")
    );

    let mob = owner_test_specs_for_machine("mob_machine");
    assert_eq!(mob.len(), 1);
    assert!(mob.iter().all(|spec| spec.package == "meerkat-mob"));
}

#[test]
#[ignore = "Phase 1 red-ok machine workflow E2E"]
fn machine_workflow_red_ok_detects_missing_and_stale_generated_artifacts() {
    let registry = CanonicalRegistry::load();
    let selection = registry
        .select(&SelectionArgs {
            all: false,
            machines: vec!["meerkat_machine".into()],
            compositions: vec!["meerkat_mob_seam".into()],
        })
        .expect("selection should resolve canonical workflow artifacts");
    let dir = tempdir().expect("tempdir");

    let missing = collect_drift_mismatches(dir.path(), &selection).expect("missing drift");
    assert!(
        !missing.is_empty(),
        "fresh temp roots should surface missing machine workflow artifacts"
    );

    machine_codegen_at_root(dir.path(), &selection).expect("generate workflow artifacts");

    let clean = collect_drift_mismatches(dir.path(), &selection).expect("clean drift");
    assert!(
        clean.is_empty(),
        "generated workflow artifacts should satisfy the anti-drift contract: {clean:#?}"
    );

    let machine_model = dir.path().join("specs/machines/meerkat_machine/model.tla");
    let composition_model = dir
        .path()
        .join("specs/compositions/meerkat_mob_seam/model.tla");
    assert!(machine_model.exists(), "machine model should be generated");
    assert!(
        composition_model.exists(),
        "composition model should be generated"
    );

    fs::write(&machine_model, "---- MODULE stale ----\n").expect("write stale model");
    let stale = collect_drift_mismatches(dir.path(), &selection).expect("stale drift");
    assert!(
        !stale.is_empty(),
        "editing a generated artifact should be caught by anti-drift checks"
    );
}

#![allow(clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::fs;
use std::process::Stdio;

use meerkat_machine_schema::{
    runtime_control_machine, runtime_ingress_machine, runtime_pipeline_composition,
    turn_execution_machine,
};
use tempfile::tempdir;
use xtask::machines::*;
use xtask::public_contracts::{
    assert_directory_contents_match, collect_public_contract_propagation_mismatches,
    repo_root as public_contracts_repo_root,
};

fn changed_paths(paths: &[&str]) -> BTreeSet<String> {
    paths.iter().map(|path| (*path).to_string()).collect()
}

#[test]
fn registry_selection_accepts_machine_name_and_slug() {
    let registry = CanonicalRegistry::load();
    let by_name = registry
        .select(&SelectionArgs {
            all: false,
            machines: vec!["RuntimeControlMachine".into()],
            compositions: vec![],
        })
        .expect("selection by name");
    assert_eq!(by_name.machines.len(), 1);
    assert_eq!(by_name.machines[0].schema.machine, "RuntimeControlMachine");

    let by_slug = registry
        .select(&SelectionArgs {
            all: false,
            machines: vec!["runtime_control".into()],
            compositions: vec![],
        })
        .expect("selection by slug");
    assert_eq!(by_slug.machines.len(), 1);
    assert_eq!(by_slug.machines[0].schema.machine, "RuntimeControlMachine");
}

#[test]
fn registry_validation_covers_machine_and_composition_sets() {
    let registry = CanonicalRegistry::load();
    assert!(registry.validate().is_ok());
}

#[test]
fn codegen_writes_machine_and_composition_authority_modules() {
    let registry = CanonicalRegistry::load();
    let selection = registry
        .select(&SelectionArgs {
            all: false,
            machines: vec!["runtime_control".into()],
            compositions: vec!["runtime_pipeline".into()],
        })
        .expect("selection");
    let dir = tempdir().expect("tempdir");

    machine_codegen_at_root(dir.path(), &selection).expect("generate outputs");

    let machine = fs::read_to_string(dir.path().join("specs/machines/runtime_control/model.tla"))
        .expect("machine model");
    assert!(machine.starts_with("---- MODULE model ----"));
    assert!(machine.contains("Generated semantic machine model for RuntimeControlMachine."));
    let machine_mapping =
        fs::read_to_string(dir.path().join("specs/machines/runtime_control/mapping.md"))
            .expect("machine mapping");
    assert!(machine_mapping.contains("## Generated Coverage"));
    assert!(machine_mapping.contains("`BeginRunFromIdle`"));

    let composition = fs::read_to_string(
        dir.path()
            .join("specs/compositions/runtime_pipeline/model.tla"),
    )
    .expect("composition model");
    assert!(composition.starts_with("---- MODULE model ----"));
    assert!(composition.contains("Generated composition model for runtime_pipeline."));
    assert!(composition.contains("effect_packet.effect_id = input_packet.effect_id"));
    assert!(composition.contains("RouteDeliveryKind("));
    let composition_ci = fs::read_to_string(
        dir.path()
            .join("specs/compositions/runtime_pipeline/ci.cfg"),
    )
    .expect("composition ci");
    assert!(composition_ci.contains("begin_run_requires_staged_drain"));
    assert!(!composition_ci.contains("control_preempts_ordinary_work"));
    let composition_contract = fs::read_to_string(
        dir.path()
            .join("specs/compositions/runtime_pipeline/contract.md"),
    )
    .expect("composition contract");
    assert!(composition_contract.contains("## Structural Requirements"));
    assert!(composition_contract.contains("## Behavioral Invariants"));
    let composition_mapping = fs::read_to_string(
        dir.path()
            .join("specs/compositions/runtime_pipeline/mapping.md"),
    )
    .expect("composition mapping");
    assert!(composition_mapping.contains("## Generated Coverage"));
    assert!(composition_mapping.contains("`staged_run_notifies_control`"));
}

#[test]
fn codegen_respects_composition_domain_cardinality_overrides() {
    let registry = CanonicalRegistry::load();
    let selection = registry
        .select(&SelectionArgs {
            all: false,
            machines: vec![],
            compositions: vec!["mob_bundle".into()],
        })
        .expect("selection");
    let dir = tempdir().expect("tempdir");

    machine_codegen_at_root(dir.path(), &selection).expect("generate outputs");

    let deep_cfg = fs::read_to_string(dir.path().join("specs/compositions/mob_bundle/deep.cfg"))
        .expect("mob deep cfg");
    assert!(deep_cfg.contains("StringValues = {\"alpha\"}"));
    assert!(deep_cfg.contains("RunIdValues = {\"runid_1\"}"));
    assert!(!deep_cfg.contains("\"beta\""));
    assert!(!deep_cfg.contains("\"runid_2\""));
}

#[test]
fn codegen_applies_minimum_witness_state_limits() {
    let registry = CanonicalRegistry::load();
    let selection = registry
        .select(&SelectionArgs {
            all: false,
            machines: vec![],
            compositions: vec!["mob_bundle".into()],
        })
        .expect("selection");
    let dir = tempdir().expect("tempdir");

    machine_codegen_at_root(dir.path(), &selection).expect("generate outputs");

    let model = fs::read_to_string(dir.path().join("specs/compositions/mob_bundle/model.tla"))
        .expect("mob model");
    assert!(
        model
            .contains("WitnessStateConstraint_mob_flow_success_path == /\\ model_step_count <= 35")
    );
    assert!(model.contains("Cardinality(delivered_routes) <= 14"));
    assert!(model.contains("Cardinality(emitted_effects) <= 21"));
}

#[test]
fn witness_cfg_includes_required_named_literals_for_path() {
    let registry = CanonicalRegistry::load();
    let selection = registry
        .select(&SelectionArgs {
            all: false,
            machines: vec![],
            compositions: vec!["mob_bundle".into()],
        })
        .expect("selection");
    let dir = tempdir().expect("tempdir");

    machine_codegen_at_root(dir.path(), &selection).expect("generate outputs");

    let cfg = fs::read_to_string(
        dir.path()
            .join("specs/compositions/mob_bundle/witness-mob_flow_success_path.cfg"),
    )
    .expect("mob witness cfg");
    assert!(cfg.contains("StepIdValues = {\"step_1\"}"));
    assert!(cfg.contains("WorkIdValues = {\"step_1\"}"));
    assert!(cfg.contains("RunIdValues = {\"runid_1\"}"));
}

#[test]
fn scheduler_witnesses_seed_initial_competing_inputs() {
    let registry = CanonicalRegistry::load();
    let selection = registry
        .select(&SelectionArgs {
            all: false,
            machines: vec![],
            compositions: vec!["runtime_pipeline".into()],
        })
        .expect("selection");
    let dir = tempdir().expect("tempdir");

    machine_codegen_at_root(dir.path(), &selection).expect("generate outputs");

    let model = fs::read_to_string(
        dir.path()
            .join("specs/compositions/runtime_pipeline/model.tla"),
    )
    .expect("runtime pipeline model");
    assert!(model.contains("WitnessInit_control_preemption =="));
    assert!(model.contains(
        "pending_inputs = <<[machine |-> \"runtime_control\", variant |-> \"Initialize\""
    ));
    assert!(model.contains("[machine |-> \"runtime_ingress\", variant |-> \"AdmitQueued\""));
    assert!(model.contains(
        "witness_current_script_input = [machine |-> \"runtime_ingress\", variant |-> \"AdmitQueued\""
    ));
    assert!(model.contains("witness_remaining_script_inputs = <<>>"));
}

#[test]
fn external_tool_witness_uses_boundary_applied_causal_order() {
    let registry = CanonicalRegistry::load();
    let selection = registry
        .select(&SelectionArgs {
            all: false,
            machines: vec![],
            compositions: vec!["external_tool_bundle".into()],
        })
        .expect("selection");
    let dir = tempdir().expect("tempdir");

    machine_codegen_at_root(dir.path(), &selection).expect("generate outputs");

    let model = fs::read_to_string(
        dir.path()
            .join("specs/compositions/external_tool_bundle/model.tla"),
    )
    .expect("external tool model");
    // Causal chain: LlmReturnedTerminal → BoundaryComplete → ApplyBoundaryAdd → ExternalToolDeltaReceivedIdle
    assert!(model.contains(
        "earlier.machine = \"turn_execution\" /\\ earlier.transition = \"LlmReturnedTerminal\" /\\ later.machine = \"turn_execution\" /\\ later.transition = \"BoundaryComplete\""
    ));
    assert!(model.contains(
        "earlier.machine = \"turn_execution\" /\\ earlier.transition = \"BoundaryComplete\" /\\ later.machine = \"external_tool_surface\" /\\ later.transition = \"ApplyBoundaryAdd\""
    ));
    assert!(model.contains(
        "earlier.machine = \"external_tool_surface\" /\\ earlier.transition = \"ApplyBoundaryAdd\" /\\ later.machine = \"runtime_control\" /\\ later.transition = \"ExternalToolDeltaReceivedIdle\""
    ));
    assert!(model.contains(
        "WitnessTransitionObserved_surface_add_notifies_control_turn_execution_LlmReturnedTerminal"
    ));
    assert!(model.contains(
        "WitnessTransitionObserved_turn_boundary_reaches_surface_turn_execution_LlmReturnedTerminal"
    ));
    assert!(model.contains(
        "WitnessTransitionObserved_turn_boundary_reaches_surface_runtime_control_Initialize"
    ));
    assert!(model.contains(
        "WitnessTransitionObserved_turn_boundary_reaches_surface_runtime_control_ExternalToolDeltaReceivedIdle"
    ));
}

#[test]
fn drift_check_reports_missing_and_stale_generated_files() {
    let registry = CanonicalRegistry::load();
    let selection = registry
        .select(&SelectionArgs {
            all: false,
            machines: vec!["runtime_control".into()],
            compositions: vec!["runtime_pipeline".into()],
        })
        .expect("selection");
    let dir = tempdir().expect("tempdir");

    let missing = collect_drift_mismatches(dir.path(), &selection).expect("missing mismatches");
    let mut expected_paths = vec![
        machine_model_path(dir.path(), "runtime_control"),
        machine_ci_path(dir.path(), "runtime_control"),
        machine_deep_path(dir.path(), "runtime_control"),
        machine_contract_path(dir.path(), "runtime_control"),
        machine_mapping_path(dir.path(), "runtime_control"),
        generated_kernel_module_path(dir.path(), "runtime_control"),
        generated_kernel_mod_path(dir.path()),
        composition_model_path(dir.path(), "runtime_pipeline"),
        composition_ci_path(dir.path(), "runtime_pipeline"),
        composition_deep_path(dir.path(), "runtime_pipeline"),
        composition_contract_path(dir.path(), "runtime_pipeline"),
        composition_mapping_path(dir.path(), "runtime_pipeline"),
    ];
    expected_paths.extend(
        selection.compositions[0]
            .schema
            .witnesses
            .iter()
            .map(|witness| composition_witness_path(dir.path(), "runtime_pipeline", &witness.name)),
    );

    assert_eq!(missing.len(), expected_paths.len());
    for path in expected_paths {
        assert!(
            missing
                .iter()
                .any(|item| item.contains(path.to_string_lossy().as_ref())),
            "expected missing artifact {}",
            path.display()
        );
    }

    machine_codegen_at_root(dir.path(), &selection).expect("generate outputs");
    let clean = collect_drift_mismatches(dir.path(), &selection).expect("clean mismatches");
    assert!(clean.is_empty());

    let machine_path = dir.path().join("specs/machines/runtime_control/model.tla");
    fs::write(&machine_path, "stale output").expect("mutate machine model");
    let stale = collect_drift_mismatches(dir.path(), &selection).expect("stale mismatches");
    assert_eq!(stale.len(), 1);
    assert!(stale[0].contains(machine_path.to_string_lossy().as_ref()));
}

#[test]
fn drift_check_reports_stale_mapping_file() {
    let registry = CanonicalRegistry::load();
    let selection = registry
        .select(&SelectionArgs {
            all: false,
            machines: vec!["runtime_control".into()],
            compositions: vec!["runtime_pipeline".into()],
        })
        .expect("selection");
    let dir = tempdir().expect("tempdir");

    machine_codegen_at_root(dir.path(), &selection).expect("generate outputs");
    let machine_mapping_path = machine_mapping_path(dir.path(), "runtime_control");
    fs::write(
        &machine_mapping_path,
        "# RuntimeControlMachine Mapping Note\n\nmanual only",
    )
    .expect("mutate machine mapping");

    let mismatches = collect_drift_mismatches(dir.path(), &selection).expect("mismatches");
    assert_eq!(mismatches.len(), 1);
    assert!(mismatches[0].contains(machine_mapping_path.to_string_lossy().as_ref()));

    machine_codegen_at_root(dir.path(), &selection).expect("regenerate outputs");
    let composition_mapping_path = composition_mapping_path(dir.path(), "runtime_pipeline");
    fs::write(
        &composition_mapping_path,
        "# runtime_pipeline Mapping Note\n\nmanual only",
    )
    .expect("mutate composition mapping");

    let mismatches = collect_drift_mismatches(dir.path(), &selection).expect("mismatches");
    assert_eq!(mismatches.len(), 1);
    assert!(mismatches[0].contains(composition_mapping_path.to_string_lossy().as_ref()));
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
#[ignore]
fn machine_authority_contract_requires_verification_and_companion_docs() {
    let registry = CanonicalRegistry::load();
    let selection = registry
        .select(&SelectionArgs {
            all: false,
            machines: vec!["runtime_control".into()],
            compositions: vec![],
        })
        .expect("selection");

    let dir = tempdir().expect("tempdir");
    machine_codegen_at_root(dir.path(), &selection).expect("generate outputs");
    let clean = collect_drift_mismatches(dir.path(), &selection).expect("clean mismatches");
    assert!(clean.is_empty(), "{clean:#?}");

    let kernel_path = generated_kernel_module_path(dir.path(), "runtime_control");
    let mapping_path = machine_mapping_path(dir.path(), "runtime_control");
    fs::write(&kernel_path, "stale kernel").expect("mutate generated kernel");
    fs::write(
        &mapping_path,
        "# RuntimeControlMachine Mapping Note\n\nmanual only",
    )
    .expect("mutate mapping doc");

    let mismatches = collect_drift_mismatches(dir.path(), &selection).expect("mismatches");
    assert!(
        mismatches
            .iter()
            .any(|item| item.contains(kernel_path.to_string_lossy().as_ref()))
    );
    assert!(
        mismatches
            .iter()
            .any(|item| item.contains(mapping_path.to_string_lossy().as_ref()))
    );

    machine_codegen_at_root(dir.path(), &selection).expect("regenerate outputs");
    let clean = collect_drift_mismatches(dir.path(), &selection).expect("clean mismatches");
    assert!(
        clean.is_empty(),
        "regenerated outputs should satisfy the anti-drift contract: {clean:#?}"
    );
}

#[test]
fn generated_kernel_boundary_rejects_parallel_owner_module_files() {
    let dir = tempdir().expect("tempdir");
    let generated = generated_kernel_module_path(dir.path(), "runtime_control");
    let owner_file = owner_module_file(dir.path(), "meerkat-runtime", "machines::runtime_control")
        .expect("owner_module_file");

    fs::create_dir_all(generated.parent().expect("generated parent")).expect("generated dir");
    fs::create_dir_all(owner_file.parent().expect("owner parent")).expect("owner dir");
    fs::write(&generated, "// generated kernel\n").expect("generated kernel");
    fs::write(&owner_file, "// parallel shell owner\n").expect("owner file");

    let mismatches =
        collect_generated_kernel_boundary_mismatches(dir.path()).expect("boundary mismatches");
    assert!(
        mismatches
            .iter()
            .any(|item| item.contains(owner_file.to_string_lossy().as_ref())),
        "expected owner file mismatch, got {mismatches:#?}"
    );
}

#[test]
fn generated_kernel_boundary_rejects_parallel_owner_module_dirs() {
    let dir = tempdir().expect("tempdir");
    let generated = generated_kernel_module_path(dir.path(), "mob_lifecycle");
    let owner_mod =
        owner_module_dir(dir.path(), "meerkat-mob", "machines::mob_lifecycle").join("mod.rs");

    fs::create_dir_all(generated.parent().expect("generated parent")).expect("generated dir");
    fs::create_dir_all(owner_mod.parent().expect("owner parent")).expect("owner dir");
    fs::write(&generated, "// generated kernel\n").expect("generated kernel");
    fs::write(&owner_mod, "// parallel shell owner module\n").expect("owner module");

    let mismatches =
        collect_generated_kernel_boundary_mismatches(dir.path()).expect("boundary mismatches");
    assert!(
        mismatches
            .iter()
            .any(|item| item.contains(owner_mod.to_string_lossy().as_ref())),
        "expected owner module mismatch, got {mismatches:#?}"
    );
}

#[test]
fn generated_kernel_boundary_accepts_generated_only_layout() {
    let root = repo_root().expect("repo root");
    let mismatches =
        collect_generated_kernel_boundary_mismatches(&root).expect("boundary mismatches");
    assert!(
        mismatches.is_empty(),
        "expected generated-only machine owner layout, got {mismatches:#?}"
    );
}

#[test]
fn coverage_anchor_check_reports_missing_paths() {
    let registry = CanonicalRegistry::load();
    let selection = registry
        .select(&SelectionArgs {
            all: false,
            machines: vec!["runtime_control".into()],
            compositions: vec!["runtime_pipeline".into()],
        })
        .expect("selection");

    let dir = tempdir().expect("tempdir");
    let mismatches = collect_coverage_anchor_mismatches(dir.path(), &selection);
    assert!(!mismatches.is_empty());
}

#[test]
fn parses_tlc_coverage_lines() {
    let parsed = parse_tlc_coverage_line(
        "<BeginRunFromIdle line 12, col 1 to line 24, col 13 of module model>: 7:19",
    )
    .expect("coverage line");
    assert_eq!(parsed.0, "BeginRunFromIdle");
    assert_eq!(parsed.1.truth_hits, 7);
    assert_eq!(parsed.1.evaluations, 19);
}

#[test]
fn machine_deep_coverage_rejects_zero_hit_transition() {
    let schema = runtime_control_machine();
    let coverage = parse_tlc_coverage(
        "<Initialize line 1, col 1 to line 3, col 1 of module model>: 1:1\n\
         <BeginRunFromIdle line 4, col 1 to line 6, col 1 of module model>: 0:0\n",
    );
    let err = ensure_machine_transition_coverage(&schema, &coverage).expect_err("zero-hit");
    assert!(err.to_string().contains("BeginRunFromIdle"));
}

#[test]
fn canonical_registry_rejects_missing_composition_witness_coverage() {
    let runtime_control = runtime_control_machine();
    let runtime_ingress = runtime_ingress_machine();
    let turn_execution = turn_execution_machine();
    let mut schema = runtime_pipeline_composition();
    for witness in &mut schema.witnesses {
        witness.expected_routes.clear();
        witness.expected_scheduler_rules.clear();
    }

    let err = schema
        .validate_against(&[&runtime_control, &runtime_ingress, &turn_execution])
        .expect_err("missing witness coverage");
    assert!(
        err.to_string()
            .contains("is not covered by any composition witness")
    );
}

#[test]
fn composition_deep_coverage_accepts_hits_from_witness_runs() {
    let schema = runtime_pipeline_composition();
    let mut aggregated = parse_tlc_coverage("");
    let witness = parse_tlc_coverage(
        "<RouteCoverage_admitted_work_enters_ingress line 1, col 1 to line 1, col 10 of module model>: 1:2\n\
         <RouteCoverage_staged_run_notifies_control line 1, col 1 to line 1, col 10 of module model>: 1:3\n\
         <RouteCoverage_control_starts_execution line 1, col 1 to line 1, col 10 of module model>: 1:2\n\
         <RouteCoverage_execution_boundary_updates_ingress line 1, col 1 to line 1, col 10 of module model>: 1:1\n\
         <RouteCoverage_execution_completion_updates_ingress line 1, col 1 to line 1, col 10 of module model>: 1:1\n\
         <RouteCoverage_execution_completion_notifies_control line 1, col 1 to line 1, col 10 of module model>: 1:1\n\
         <RouteCoverage_execution_failure_updates_ingress line 1, col 1 to line 1, col 10 of module model>: 1:1\n\
         <RouteCoverage_execution_failure_notifies_control line 1, col 1 to line 1, col 10 of module model>: 1:1\n\
         <RouteCoverage_execution_cancel_updates_ingress line 1, col 1 to line 1, col 10 of module model>: 1:1\n\
         <RouteCoverage_execution_cancel_notifies_control line 1, col 1 to line 1, col 10 of module model>: 1:1\n\
         <SchedulerCoverage_PreemptWhenReady_control_plane_ordinary_ingress line 1, col 1 to line 1, col 10 of module model>: 1:4\n",
    );
    merge_tlc_coverage(&mut aggregated, Some(&witness));

    ensure_composition_coverage(
        &schema,
        &aggregated,
        &std::collections::BTreeSet::new(),
        &std::collections::BTreeSet::new(),
    )
    .expect("witness coverage should count");
}

#[test]
fn accepts_tlc_success_sentinel_even_with_nonzero_exit_status() {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        use std::process::ExitStatus;

        let status = ExitStatus::from_raw(13 << 8);
        assert!(tlc_run_succeeded(
            &status,
            "Model checking completed. No error has been found.\nEnd of statistics."
        ));
    }
}

#[test]
fn rejects_tlc_nonzero_status_without_success_sentinel() {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        use std::process::ExitStatus;

        let status = ExitStatus::from_raw(13 << 8);
        assert!(!tlc_run_succeeded(
            &status,
            "Error: The behavior up to this point is:\nstate 1"
        ));
    }
}

#[test]
#[ignore]
fn public_contract_slice_requires_regenerated_bindings_docs_and_changelog() {
    let mismatches = collect_public_contract_propagation_mismatches(&changed_paths(&[
        "meerkat-contracts/src/wire/event.rs",
    ]));

    assert_eq!(mismatches.len(), 5);
    assert!(
        mismatches
            .iter()
            .any(|item| item.contains("artifacts/schemas/"))
    );
    assert!(
        mismatches
            .iter()
            .any(|item| item.contains("sdks/python/meerkat/generated/"))
    );
    assert!(
        mismatches
            .iter()
            .any(|item| item.contains("sdks/typescript/src/generated/"))
    );
    assert!(
        mismatches
            .iter()
            .any(|item| item.contains("docs/api, docs/sdks, docs/rust, or examples/"))
    );
    assert!(mismatches.iter().any(|item| item.contains("CHANGELOG.md")));
}

#[test]
#[ignore]
fn public_contract_slice_accepts_full_companion_update() {
    let mismatches = collect_public_contract_propagation_mismatches(&changed_paths(&[
        "meerkat-contracts/src/wire/event.rs",
        "artifacts/schemas/events.json",
        "sdks/python/meerkat/generated/types.py",
        "sdks/typescript/src/generated/types.ts",
        "docs/api/rpc.mdx",
        "CHANGELOG.md",
    ]));

    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

#[test]
#[ignore]
fn generated_sdk_bindings_match_committed_outputs() {
    let root = public_contracts_repo_root().expect("repo root");
    let temp = tempdir().expect("tempdir");
    let output_root = temp.path().join("generated");

    let status = std::process::Command::new("python3")
        .arg("tools/sdk-codegen/generate.py")
        .arg("--output-root")
        .arg(&output_root)
        .current_dir(&root)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .status()
        .expect("run sdk codegen");
    assert!(status.success(), "sdk codegen should succeed");

    assert_directory_contents_match(
        &root.join("sdks/python/meerkat/generated"),
        &output_root.join("sdks/python/meerkat/generated"),
    )
    .expect("python bindings should be fresh");
    assert_directory_contents_match(
        &root.join("sdks/typescript/src/generated"),
        &output_root.join("sdks/typescript/src/generated"),
    )
    .expect("typescript bindings should be fresh");
}

#[test]
#[ignore]
fn repo_policy_docs_and_changelog_keep_propagation_anchors() {
    let root = public_contracts_repo_root().expect("repo root");
    let gaps =
        std::fs::read_to_string(root.join("docs/architecture/0.5/rct/gaps-and-contradictions.md"))
            .expect("read gaps and contradictions");
    assert!(gaps.contains("The anti-drift rule is:"));
    assert!(gaps.contains("The public-change propagation rule is:"));

    let changelog = std::fs::read_to_string(root.join("CHANGELOG.md")).expect("read changelog");
    assert!(changelog.contains("## [Unreleased]"));
}

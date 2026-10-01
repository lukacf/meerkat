#![allow(clippy::expect_used, clippy::panic)]

#[cfg(feature = "machine-authority")]
use std::collections::BTreeSet;
use std::fs;

use super::*;
#[cfg(feature = "machine-authority")]
use meerkat_machine_schema::{
    EnumSchema, InitSchema, MachineSchema, RustBinding, SemanticCoverageEntry, StateSchema,
    TransitionSchema, VariantSchema,
};
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

#[cfg(all(unix, feature = "machine-authority"))]
#[test]
fn tlc_success_uses_process_status_not_stdout_folklore() {
    use std::os::unix::process::ExitStatusExt;

    let failed = std::process::ExitStatus::from_raw(1 << 8);
    assert!(
        !tlc_run_succeeded(&failed),
        "failed TLC process status must not be overridden by parsed stdout"
    );

    let succeeded = std::process::ExitStatus::from_raw(0);
    assert!(tlc_run_succeeded(&succeeded));
}

#[test]
fn owner_tests_are_registered_only_for_remaining_canonical_surfaces() {
    let meerkat = owner_test_specs_for_machine("meerkat_machine");
    assert_eq!(meerkat.len(), 4);
    assert!(
        meerkat
            .iter()
            .all(|spec| spec.package == "meerkat-integration-tests")
    );

    let mob = owner_test_specs_for_machine("mob_machine");
    assert_eq!(mob.len(), 1);
    assert!(mob.iter().all(|spec| spec.package == "meerkat-mob"));
}

#[cfg(feature = "machine-authority")]
#[test]
fn semantic_coverage_rejects_all_anchor_all_scenario_entries() {
    let anchors = BTreeSet::from(["runtime", "schema"]);
    let scenarios = BTreeSet::from(["happy", "failure"]);
    let err = validate_semantic_entries(
        "machine TestMachine",
        "transition",
        &["Apply".to_string()],
        &[SemanticCoverageEntry {
            name: "Apply".to_string(),
            anchor_ids: vec!["runtime".to_string(), "schema".to_string()],
            scenario_ids: vec!["happy".to_string(), "failure".to_string()],
        }],
        &anchors,
        &scenarios,
    )
    .expect_err("all-anchor/all-scenario coverage should be rejected");

    assert!(
        err.to_string().contains("not tautological"),
        "unexpected error: {err:#}"
    );
}

#[test]
fn machine_workflow_red_ok_detects_missing_and_stale_generated_artifacts() {
    let registry = CanonicalRegistry::load();
    let selection = registry
        .select(&SelectionArgs {
            all: false,
            machines: vec!["meerkat_machine".to_string()],
            compositions: vec!["meerkat_mob_seam".to_string()],
        })
        .expect("selection should resolve representative canonical workflow artifacts");
    let dir = tempdir().expect("tempdir");
    let selected_machine_dir = dir
        .path()
        .join("specs/machines/meerkat_machine")
        .display()
        .to_string();
    let selected_composition_dir = dir
        .path()
        .join("specs/compositions/meerkat_mob_seam")
        .display()
        .to_string();

    let missing = collect_drift_mismatches(dir.path(), &selection).expect("missing drift");
    assert!(
        !missing.is_empty(),
        "fresh temp roots should surface missing machine workflow artifacts"
    );
    materialize_missing_coverage_anchors(&missing).expect("materialize coverage anchors");

    machine_codegen_at_root(dir.path(), &selection).expect("generate workflow artifacts");

    let mut clean = collect_drift_mismatches(dir.path(), &selection).expect("clean drift");
    clean.retain(|mismatch| {
        selected_workflow_mismatch(
            mismatch,
            &[&selected_machine_dir, &selected_composition_dir],
        ) && !mismatch.starts_with("production owner audit path for ")
            && !mismatch.starts_with("production owner schema relation for ")
            && !mismatch.starts_with("MobCommand::")
    });
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
    let stale = stale
        .into_iter()
        .filter(|mismatch| {
            selected_workflow_mismatch(
                mismatch,
                &[&selected_machine_dir, &selected_composition_dir],
            )
        })
        .collect::<Vec<_>>();
    assert!(
        stale.iter().any(|mismatch| mismatch.contains(
            machine_model
                .to_str()
                .expect("machine model path should be UTF-8")
        )),
        "editing a selected generated artifact should be caught by anti-drift checks: {stale:#?}"
    );
}

fn selected_workflow_mismatch(mismatch: &str, selected_dirs: &[&str]) -> bool {
    selected_dirs
        .iter()
        .any(|selected_dir| mismatch.contains(*selected_dir))
}

#[cfg(feature = "machine-authority")]
#[test]
fn skipped_composition_tlc_still_requires_ci_structural_invariants() {
    let schema = meerkat_machine_schema::canonical_composition_schemas()
        .into_iter()
        .find(|schema| schema.name.as_str() == "adaptive_mob_bundle")
        .expect("adaptive composition exists");
    let slug = composition_slug(&schema.name);
    let dir = tempdir().expect("tempdir");
    let composition_dir = dir.path().join("specs/compositions").join(&slug);
    fs::create_dir_all(&composition_dir).expect("create composition dir");

    fs::write(
        composition_dir.join("ci.cfg"),
        "SPECIFICATION Spec\nINVARIANTS\n  TRUE\n",
    )
    .expect("write incomplete cfg");
    let err = ensure_composition_ci_structural_invariants(dir.path(), &slug, &schema)
        .expect_err("missing structural invariants should fail closed");
    assert!(
        err.to_string().contains("omits structural invariants"),
        "unexpected error: {err:#}"
    );

    fs::write(
        composition_dir.join("ci.cfg"),
        render_composition_ci_cfg(&schema, false),
    )
    .expect("write generated cfg");
    ensure_composition_ci_structural_invariants(dir.path(), &slug, &schema)
        .expect("generated ci.cfg should include structural invariants");
}

#[cfg(feature = "machine-authority")]
fn canonical_witness(
    composition: &str,
    witness: &str,
) -> meerkat_machine_schema::CompositionWitness {
    meerkat_machine_schema::canonical_composition_schemas()
        .into_iter()
        .find(|schema| schema.name.as_str() == composition)
        .expect("composition exists")
        .witnesses
        .into_iter()
        .find(|candidate| candidate.name.as_str() == witness)
        .expect("witness exists")
}

#[cfg(feature = "machine-authority")]
fn witness_coverage_output(witness: &str, stutter_counts: Option<&str>) -> String {
    let mut output = String::from(
        "<WitnessInit_x line 1, col 1 to line 1, col 9 of module model>: 3:3\n\
         <DeliverQueuedRoute line 2, col 1 to line 2, col 18 of module model>: 2:6\n",
    );
    if let Some(counts) = stutter_counts {
        output.push_str(&format!(
            "<{} line 3, col 1 to line 3, col 40 of module model>: {counts}\n",
            witness_satisfied_stutter_operator_name(witness)
        ));
    }
    output
}

#[cfg(feature = "machine-authority")]
#[test]
fn witness_exit_zero_without_completion_fails_closed() {
    let witness = canonical_witness("job_runtime_delivery", "runtime_delivery_first_commit");
    let name = witness.name.as_str();

    // Satisfied stutter generated states: the script completed.
    let completed = parse_tlc_coverage(&witness_coverage_output(name, Some("0:27")));
    ensure_witness_completed("job_runtime_delivery", &witness, &completed)
        .expect("a completed witness passes");

    // Truncated or stuck run: TLC can exit 0 but the stutter never fires.
    let truncated = parse_tlc_coverage(&witness_coverage_output(name, Some("0:0")));
    let err = ensure_witness_completed("job_runtime_delivery", &witness, &truncated)
        .expect_err("an incomplete witness must fail");
    assert!(err.to_string().contains("never completed"), "{err:#}");

    // No coverage line for the completion action: fail closed.
    let missing = parse_tlc_coverage(&witness_coverage_output(name, None));
    let err = ensure_witness_completed("job_runtime_delivery", &witness, &missing)
        .expect_err("missing completion coverage must fail");
    assert!(err.to_string().contains("cannot be proven"), "{err:#}");
}

#[cfg(feature = "machine-authority")]
#[test]
fn vacuous_witness_fails_even_when_tlc_reports_activity() {
    // The canonical catalog no longer carries vacuous witnesses (#1362
    // scripted or removed every one), so build one: a real witness with its
    // script and every expectation cleared, exactly the old
    // `witness(name, &[])` placeholder shape.
    let mut witness = canonical_witness("schedule_bundle", "pause_resume_without_revision");
    witness.preload_inputs.clear();
    witness.expected_routes.clear();
    witness.expected_scheduler_rules.clear();
    witness.expected_states.clear();
    witness.expected_transitions.clear();
    witness.expected_transition_order.clear();
    assert!(witness_declares_no_conditions(&witness));
    let coverage = parse_tlc_coverage(&witness_coverage_output(witness.name.as_str(), Some("1:1")));
    let err = ensure_witness_completed("schedule_bundle", &witness, &coverage)
        .expect_err("a witness that declares nothing must fail");
    assert!(err.to_string().contains("vacuous"), "{err:#}");
}

#[cfg(feature = "machine-authority")]
#[test]
fn canonical_composition_witnesses_are_not_vacuous() {
    // A witness with no script and no expectations "passes" TLC while proving
    // nothing; the harness fails it. Keep the catalog free of them so a
    // placeholder cannot reappear unnoticed in a lane that skips TLC.
    let vacuous = meerkat_machine_schema::canonical_composition_schemas()
        .into_iter()
        .flat_map(|schema| {
            let name = schema.name.to_string();
            schema
                .witnesses
                .into_iter()
                .filter(witness_declares_no_conditions)
                .map(move |witness| format!("{name}/{}", witness.name))
        })
        .collect::<Vec<_>>();
    assert!(
        vacuous.is_empty(),
        "vacuous canonical witnesses: {vacuous:?}"
    );
}

#[cfg(feature = "machine-authority")]
#[test]
fn witness_completion_operator_names_match_checked_in_models() {
    let root = repo_root().expect("repo root");
    for schema in meerkat_machine_schema::canonical_composition_schemas() {
        if schema.witnesses.is_empty() {
            continue;
        }
        let slug = composition_slug(&schema.name);
        let model = fs::read_to_string(composition_model_path(&root, &slug)).expect("read model");
        for witness in &schema.witnesses {
            let operator = witness_satisfied_stutter_operator_name(witness.name.as_str());
            let body = model
                .split(&format!("\n{operator} ==\n"))
                .nth(1)
                .and_then(|tail| tail.split("\n\n").next())
                .unwrap_or_else(|| panic!("{slug}: checked-in model defines no {operator}"));
            // The harness's vacuity rule must agree with codegen: a witness
            // that declares nothing renders an always-disabled stutter.
            assert_eq!(
                body.trim() == "/\\ FALSE",
                witness_declares_no_conditions(witness),
                "{slug}: {operator} body disagrees with the vacuity rule:\n{body}"
            );
        }
    }
}

#[cfg(all(unix, feature = "machine-authority"))]
#[test]
fn capped_tlc_run_is_killed_and_reported_incomplete() {
    let started = std::time::Instant::now();
    let run = run_tlc_with_cap(
        std::process::Command::new("sleep").arg("30"),
        std::time::Duration::from_secs(1),
    )
    .expect("spawn sleep");
    assert!(
        run.status.is_none(),
        "a run past its cap has no exit status"
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(20),
        "the cap must kill the child, not wait for it"
    );

    let finished = run_tlc_with_cap(
        std::process::Command::new("sh").args(["-c", "echo done; exit 11"]),
        std::time::Duration::from_secs(30),
    )
    .expect("spawn sh");
    assert_eq!(finished.status.and_then(|status| status.code()), Some(11));
    assert_eq!(String::from_utf8_lossy(&finished.stdout).trim(), "done");

    let incomplete = TlcRunError::Incomplete {
        slug: "meerkat_mob_seam".into(),
        config: "ci.cfg".into(),
        cap_secs: 900,
    };
    assert!(incomplete.to_string().contains("TLC INCOMPLETE"));
    assert!(incomplete.to_string().contains("not a pass"));
}

#[cfg(feature = "machine-authority")]
#[test]
fn deep_coverage_credits_transitions_tlc_reports_as_next_disjuncts() {
    // work_attention_lifecycle shape: PauseActive binds a state-dependent set,
    // so TLC reports it as a Next disjunct, while ClassifyEligibilityActive is
    // split and named directly.
    let model = [
        "Next ==",
        "    \\/ \\E expected_revision \\in {revision} : \\E until_utc_ms \\in OptionU64Values : PauseActive(expected_revision, until_utc_ms)",
        "    \\/ \\E expected_revision \\in {revision} : ResumePaused(expected_revision)",
    ]
    .join("\n");
    let output = "\
<ClassifyEligibilityActive line 137, col 1 to line 137, col 37 of module model>: 27:111
<Next line 1, col 1 to line 1, col 4 of module model (2 8 2 127)>: 4:148
<Next line 1, col 1 to line 1, col 4 of module model (3 8 3 76)>: 0:0
";
    let coverage = parse_tlc_coverage_with_model(output, &model);
    let counts = |name: &str| coverage.counts_by_operator.get(name).map(|c| c.evaluations);
    assert_eq!(counts("ClassifyEligibilityActive"), Some(111));
    assert_eq!(
        counts("PauseActive"),
        Some(148),
        "a fired disjunct is credited"
    );
    assert_eq!(
        counts("ResumePaused"),
        Some(0),
        "an unfired disjunct stays zero-hit"
    );
    // Without the model the disjunct counts are not attributed at all.
    assert_eq!(
        parse_tlc_coverage(output)
            .counts_by_operator
            .get("PauseActive")
            .map(|c| c.evaluations),
        None
    );
}

#[cfg(feature = "machine-authority")]
#[test]
fn tlc_lane_script_heap_floor_matches_the_xtask_floor() {
    let root = repo_root().expect("repo root");
    let script = fs::read_to_string(root.join("crates/xtask/tests/machine_verify_all_tlc_test.sh"))
        .expect("read lane script");
    let expected = format!("\nmin_heap_mb_per_job={MIN_HEAP_MB_PER_PARALLEL_JOB}\n");
    assert!(
        script.contains(&expected),
        "the lane script's audit heap floor must equal MIN_HEAP_MB_PER_PARALLEL_JOB ({MIN_HEAP_MB_PER_PARALLEL_JOB} MiB)"
    );
}

#[cfg(feature = "machine-authority")]
#[test]
fn tlc_lane_budget_split_never_exceeds_the_total_worker_budget() {
    let budget = |workers: usize| TlcRunBudget {
        workers,
        cap: std::time::Duration::from_secs(900),
        gc_threads: None,
        heap_mb: None,
    };
    // A 4-worker budget (or less) runs sequentially with the JVM defaults,
    // exactly as before.
    for workers in [1, 2, 4, 7] {
        let (concurrency, job) = budget(workers).split_for_jobs_with_heap(30, None);
        assert_eq!(concurrency, 1, "workers={workers}");
        assert_eq!(job.workers, workers);
        assert!(job.gc_threads.is_none() && job.heap_mb.is_none());
    }
    for (workers, jobs) in [(8, 30), (10, 30), (16, 30), (192, 35), (192, 3)] {
        let (concurrency, job) = budget(workers).split_for_jobs_with_heap(jobs, None);
        assert!(concurrency >= 1 && concurrency <= jobs, "workers={workers}");
        assert!(
            concurrency * job.workers <= workers,
            "workers={workers}: {concurrency} x {} exceeds the budget",
            job.workers
        );
        assert!(job.workers >= MIN_WORKERS_PER_PARALLEL_JOB.min(workers));
        if concurrency > 1 {
            assert_eq!(
                job.gc_threads,
                Some(job.workers),
                "GC threads follow workers"
            );
        }
    }
    assert_eq!(budget(8).split_for_jobs_with_heap(30, None).0, 2);
    assert_eq!(budget(192).split_for_jobs_with_heap(3, None).0, 3);

    // The heap budget also bounds concurrency: every concurrent JVM gets at
    // least MIN_HEAP_MB_PER_PARALLEL_JOB, so a small machine runs fewer jobs.
    let floor = MIN_HEAP_MB_PER_PARALLEL_JOB;
    let (concurrency, job) = budget(16).split_for_jobs_with_heap(30, Some(2 * floor));
    assert_eq!(concurrency, 2);
    assert_eq!(job.heap_mb, Some(floor));
    let (concurrency, job) = budget(16).split_for_jobs_with_heap(30, Some(floor / 2));
    assert_eq!(
        concurrency, 1,
        "below one floor share the lane runs sequentially"
    );
    assert_eq!(
        job.heap_mb,
        Some(floor / 2),
        "a lone JVM gets the whole budget"
    );
    assert!(
        job.gc_threads.is_none(),
        "a lone JVM keeps the default GC threads"
    );
    let (concurrency, job) = budget(192).split_for_jobs_with_heap(35, Some(100 * floor));
    assert_eq!(
        concurrency, MAX_CONCURRENT_TLC_JOBS,
        "a large budget runs at most MAX_CONCURRENT_TLC_JOBS JVMs, each with more workers"
    );
    assert_eq!(job.workers, 192 / MAX_CONCURRENT_TLC_JOBS);
    assert!(job.heap_mb.is_some_and(|mb| mb >= floor));
}

#[cfg(feature = "machine-authority")]
#[test]
fn tlc_lane_jobs_report_in_lane_order_and_failures_do_not_cancel_others() {
    let jobs = (0..6).map(LaneJob::Machine).collect::<Vec<_>>();
    let results = run_lane_jobs(&jobs, 3, |job| {
        let LaneJob::Machine(index) = *job else {
            unreachable!("machine jobs only")
        };
        // Earlier jobs finish later, so completion order is reversed.
        std::thread::sleep(std::time::Duration::from_millis(30 * (6 - index as u64)));
        LaneJobResult {
            output: format!("job {index}\n"),
            coverage: if index == 1 {
                Err(anyhow::anyhow!("job 1 failed"))
            } else {
                Ok(None)
            },
        }
    });
    let outputs = results
        .iter()
        .map(|r| r.output.as_str())
        .collect::<String>();
    assert_eq!(outputs, "job 0\njob 1\njob 2\njob 3\njob 4\njob 5\n");
    assert!(results[1].coverage.is_err());
    assert_eq!(results.iter().filter(|r| r.coverage.is_ok()).count(), 5);
}

#[cfg(feature = "machine-authority")]
#[test]
fn witness_model_pruning_keeps_exactly_the_reachable_definitions() {
    let module = [
        "---- MODULE model ----",
        "EXTENDS TLC, Naturals",
        "CONSTANTS StringValues, SampleValues",
        "VARIABLES x, y",
        "vars == << x, y >>",
        "Helper(a) == a + 1",
        "Unused == 42",
        "RECURSIVE Walk(_)",
        "Walk(s) == IF s = {} THEN 0 ELSE Walk(s \\ {CHOOSE e \\in s : TRUE})",
        "RECURSIVE Orphan(_)",
        "Orphan(s) == Orphan(s)",
        "Big == {",
        "  Helper(1),",
        "  Walk({})",
        "}",
        "Init == x = 0 /\\ y = 0",
        "Step == x' = Helper(x) /\\ y' = Big",
        "Spec == Init /\\ [][Step]_vars",
        "SampleValuesWitness == {1}",
        "Inv == x >= 0",
        "Never == Unused = 0",
        "Assumed == TRUE",
        "ASSUME Assumed",
        "THEOREM Spec => []Never",
        "====",
    ]
    .join("\n");
    let cfg = "SPECIFICATION Spec\nCONSTANTS\n  StringValues = {\"a\"}\n  SampleValues <- SampleValuesWitness\nINVARIANTS\n  Inv\n";
    let pruned = prune_tla_module_for_cfg(&module, cfg);
    for kept in [
        "---- MODULE model ----",
        "EXTENDS TLC, Naturals",
        "VARIABLES x, y",
        "vars ==",
        "Helper(a) ==",
        "RECURSIVE Walk(_)",
        "Walk(s) ==",
        "  Walk({})",
        "}",
        "Spec ==",
        "SampleValuesWitness ==",
        "Inv ==",
        "Assumed ==",
        "ASSUME Assumed",
        "====",
    ] {
        assert!(
            pruned.contains(kept),
            "pruned module lost `{kept}`:\n{pruned}"
        );
    }
    for dropped in ["Unused ==", "Never ==", "THEOREM", "Orphan"] {
        assert!(
            !pruned.contains(dropped),
            "pruned module kept `{dropped}`:\n{pruned}"
        );
    }
}

#[cfg(feature = "machine-authority")]
#[test]
fn witness_model_pruning_keeps_every_cfg_root_of_every_canonical_witness() {
    let root = repo_root().expect("repo root");
    for schema in meerkat_machine_schema::canonical_composition_schemas() {
        if schema.witnesses.is_empty() {
            continue;
        }
        let slug = composition_slug(&schema.name);
        let model = fs::read_to_string(composition_model_path(&root, &slug)).expect("read model");
        for witness in &schema.witnesses {
            let cfg_path = composition_witness_path(&root, &slug, witness.name.as_str());
            let cfg = fs::read_to_string(&cfg_path).expect("read witness cfg");
            let pruned = prune_tla_module_for_cfg(&model, &cfg);
            assert!(pruned.len() <= model.len());
            assert!(
                pruned.trim_end().ends_with("===="),
                "{slug}/{}",
                witness.name
            );
            for line in cfg.lines().map(str::trim) {
                let operator = line.split("<-").nth(1).map(str::trim).unwrap_or(line);
                if operator.is_empty()
                    || operator.contains(' ')
                    || operator
                        .chars()
                        .next()
                        .is_none_or(|c| !c.is_ascii_alphabetic())
                {
                    continue;
                }
                if model.contains(&format!("\n{operator} ==")) {
                    assert!(
                        pruned.contains(&format!("\n{operator} ==")),
                        "{slug}/{}: pruning dropped cfg root {operator}",
                        witness.name
                    );
                }
            }
        }
    }
}

fn materialize_missing_coverage_anchors(mismatches: &[String]) -> anyhow::Result<()> {
    for mismatch in mismatches {
        let Some((_, rest)) = mismatch.split_once("coverage anchor ") else {
            continue;
        };
        let Some((path, _)) = rest.split_once(" for ") else {
            continue;
        };
        let path = std::path::Path::new(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, "// coverage anchor fixture\n")?;
    }
    Ok(())
}

#[cfg(feature = "machine-authority")]
#[test]
fn tlc_dot_parser_reads_snapshot_nodes_and_action_labels() {
    let dot = r#"
strict digraph DiskGraph {
nodesep=0.35;
subgraph cluster_graph {
color="white";
1 [label="phase = \"Idle\"\nmodel_step_count = 0",style = filled]
1 -> 2 [label="StepUp",color="black",fontcolor="black"];
2 [label="phase = \"Running\"\nmodel_step_count = 1"];
2 -> 1 [label="StepDown",color="black",fontcolor="black"];
}
}
"#;

    let graph = parse_tlc_dot_graph(dot).expect("parse DOT graph");
    assert_eq!(graph.states.len(), 2);
    assert_eq!(graph.edges.len(), 2);
    assert_eq!(graph.initial_states.len(), 1);
    assert_eq!(graph.states[0].phase.as_deref(), Some("Idle"));
}

#[cfg(feature = "machine-authority")]
#[test]
fn parse_tlc_graph_stats_extracts_generated_distinct_and_depth() {
    let stats = parse_tlc_graph_stats(
        r"
Verifying machine MeerkatMachine
123 states generated, 45 distinct states found, 0 states left on queue.
The depth of the complete state graph search is 9.
",
    );

    assert_eq!(stats.generated_states, Some(123));
    assert_eq!(stats.distinct_states, Some(45));
    assert_eq!(stats.depth, Some(9));
}

#[cfg(feature = "machine-authority")]
#[test]
fn hopcroft_refinement_merges_terminally_equivalent_states_without_observation() {
    let dot = r#"
strict digraph DiskGraph {
nodesep=0.35;
subgraph cluster_graph {
color="white";
1 [label="phase = \"Idle\"\nmodel_step_count = 0",style = filled]
1 -> 2 [label="Tick",color="black",fontcolor="black"];
1 -> 3 [label="Tick",color="black",fontcolor="black"];
2 [label="phase = \"LeftTerminal\"\nmodel_step_count = 1"];
3 [label="phase = \"RightTerminal\"\nmodel_step_count = 1"];
}
}
"#;

    let graph = parse_tlc_dot_graph(dot).expect("parse DOT graph");
    let quotient = hopcroft_partition_refinement(&graph, HopcroftObservation::None);
    assert_eq!(quotient.blocks.len(), 2);

    let terminal_blocks = quotient
        .blocks
        .iter()
        .find(|members| members.len() == 2)
        .expect("terminal states merged");
    let phases = terminal_blocks
        .iter()
        .filter_map(|state_idx| graph.states[*state_idx].phase.as_deref())
        .collect::<Vec<_>>();
    assert!(phases.contains(&"LeftTerminal"));
    assert!(phases.contains(&"RightTerminal"));
}

#[cfg(feature = "machine-authority")]
#[test]
fn phase_observation_prevents_cross_phase_merges() {
    let dot = r#"
strict digraph DiskGraph {
nodesep=0.35;
subgraph cluster_graph {
color="white";
1 [label="phase = \"Idle\"\nmodel_step_count = 0",style = filled]
2 [label="phase = \"Running\"\nmodel_step_count = 0"];
}
}
"#;

    let graph = parse_tlc_dot_graph(dot).expect("parse DOT graph");
    let quotient = hopcroft_partition_refinement(&graph, HopcroftObservation::Phase);
    assert_eq!(quotient.blocks.len(), 2);
}

#[cfg(feature = "machine-authority")]
#[test]
fn field_observation_modes_support_only_and_all_except() {
    let dot = r#"
strict digraph DiskGraph {
nodesep=0.35;
subgraph cluster_graph {
color="white";
1 [label="/\\ phase = \"Idle\"\n/\\ x = 0\n/\\ y = 7\n/\\ model_step_count = 0",style = filled]
2 [label="/\\ phase = \"Idle\"\n/\\ x = 1\n/\\ y = 7\n/\\ model_step_count = 0"];
}
}
"#;

    let graph = parse_tlc_dot_graph(dot).expect("parse DOT graph");
    let only_x = hopcroft_partition_refinement_with_spec(
        &graph,
        &HopcroftObservationSpec::Fields(BTreeSet::from([String::from("x")])),
    );
    let all_except_x = hopcroft_partition_refinement_with_spec(
        &graph,
        &HopcroftObservationSpec::AllExceptFields(BTreeSet::from([String::from("x")])),
    );

    assert_eq!(
        only_x.blocks.len(),
        2,
        "x alone should distinguish the states"
    );
    assert_eq!(
        all_except_x.blocks.len(),
        1,
        "removing x should collapse the states because y is identical"
    );
}

#[cfg(feature = "machine-authority")]
#[test]
fn largest_mixed_block_projection_reports_distinct_field_partitions() {
    let dot = r#"
strict digraph DiskGraph {
nodesep=0.35;
subgraph cluster_graph {
color="white";
1 [label="/\\ phase = \"Idle\"\n/\\ x = 0\n/\\ y = \"left\"\n/\\ model_step_count = 0",style = filled]
2 [label="/\\ phase = \"Running\"\n/\\ x = 0\n/\\ y = \"right\"\n/\\ model_step_count = 0"];
3 [label="/\\ phase = \"Stopped\"\n/\\ x = 1\n/\\ y = \"right\"\n/\\ model_step_count = 0"];
4 [label="/\\ phase = \"Retired\"\n/\\ x = 1\n/\\ y = \"right\"\n/\\ model_step_count = 0"];
}
}
"#;

    let graph = parse_tlc_dot_graph(dot).expect("parse DOT graph");
    let quotient = hopcroft_partition_refinement(&graph, HopcroftObservation::None);
    let mixed_phase_blocks = quotient
        .blocks
        .iter()
        .enumerate()
        .filter_map(|(block_id, members)| {
            let summary = summarize_hopcroft_block(block_id, members, &graph, &quotient);
            (summary.phases.len() > 1).then_some(summary)
        })
        .collect::<Vec<_>>();

    let projection = summarize_largest_mixed_phase_block_field_projection(
        &graph,
        &quotient,
        &mixed_phase_blocks,
    )
    .expect("mixed block projection");

    assert_eq!(projection.size, 4);
    assert_eq!(projection.field_count, 2);
    assert_eq!(projection.distinct_tuples, 3);
    assert_eq!(projection.phase_overlay_tuple_count, 1);
    assert_eq!(projection.max_phases_per_tuple, 2);

    let x = projection
        .fields
        .iter()
        .find(|field| field.field == "x")
        .expect("x projection");
    assert_eq!(x.distinct_values, 2);
    assert_eq!(x.largest_bucket_size, 2);
    assert_eq!(x.top_values.len(), 2);

    let y = projection
        .fields
        .iter()
        .find(|field| field.field == "y")
        .expect("y projection");
    assert_eq!(y.distinct_values, 2);
    assert_eq!(y.largest_bucket_size, 3);
    assert_eq!(y.top_values[0].count, 3);
}

#[cfg(feature = "machine-authority")]
#[test]
fn schema_input_rows_classify_same_left_only_and_different_surfaces() {
    use meerkat_machine_schema::TriggerMatch;
    use meerkat_machine_schema::identity::{
        EnumVariantId, FieldId, InputVariantId, MachineId, PhaseId, TransitionId,
    };

    fn vid(s: &str) -> EnumVariantId {
        EnumVariantId::parse(s).expect("valid enum variant slug")
    }
    fn pid(s: &str) -> PhaseId {
        PhaseId::parse(s).expect("valid phase slug")
    }
    fn tid(s: &str) -> TransitionId {
        TransitionId::parse(s).expect("valid transition slug")
    }
    fn ivid(s: &str) -> InputVariantId {
        InputVariantId::parse(s).expect("valid input variant slug")
    }
    let _: Option<FieldId> = None;

    let schema = MachineSchema {
        machine: MachineId::parse("TestMachine").expect("valid machine slug"),
        version: 1,
        rust: RustBinding {
            crate_name: "test".into(),
            module: "test".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "Phase".into(),
                variants: vec![
                    VariantSchema {
                        name: vid("Idle"),
                        fields: vec![],
                    },
                    VariantSchema {
                        name: vid("Attached"),
                        fields: vec![],
                    },
                    VariantSchema {
                        name: vid("Running"),
                        fields: vec![],
                    },
                    VariantSchema {
                        name: vid("Stopped"),
                        fields: vec![],
                    },
                ],
            },
            fields: vec![],
            init: InitSchema {
                phase: pid("Idle"),
                fields: vec![],
            },
            terminal_phases: vec![pid("Stopped")],
        },
        inputs: EnumSchema {
            name: "Input".into(),
            variants: vec![
                VariantSchema {
                    name: vid("Ping"),
                    fields: vec![],
                },
                VariantSchema {
                    name: vid("Start"),
                    fields: vec![],
                },
                VariantSchema {
                    name: vid("Retire"),
                    fields: vec![],
                },
            ],
        },
        surface_only_inputs: vec![],
        runtime_internal_inputs: vec![],
        tlc_representative_inputs: vec![],
        signals: EnumSchema {
            name: "Signal".into(),
            variants: vec![],
        },
        effects: EnumSchema {
            name: "Effect".into(),
            variants: vec![],
        },
        helpers: vec![],
        derived: vec![],
        invariants: vec![],
        transitions: vec![
            TransitionSchema {
                name: tid("PingIdle"),
                from: vec![pid("Idle")],
                on: TriggerMatch::Input {
                    variant: ivid("Ping"),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![],
                to: pid("Idle"),
                emit: vec![],
            },
            TransitionSchema {
                name: tid("PingAttached"),
                from: vec![pid("Attached")],
                on: TriggerMatch::Input {
                    variant: ivid("Ping"),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![],
                to: pid("Idle"),
                emit: vec![],
            },
            TransitionSchema {
                name: tid("StartIdle"),
                from: vec![pid("Idle")],
                on: TriggerMatch::Input {
                    variant: ivid("Start"),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![],
                to: pid("Running"),
                emit: vec![],
            },
            TransitionSchema {
                name: tid("RetireIdle"),
                from: vec![pid("Idle")],
                on: TriggerMatch::Input {
                    variant: ivid("Retire"),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![],
                to: pid("Idle"),
                emit: vec![],
            },
            TransitionSchema {
                name: tid("RetireAttached"),
                from: vec![pid("Attached")],
                on: TriggerMatch::Input {
                    variant: ivid("Retire"),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![],
                to: pid("Stopped"),
                emit: vec![],
            },
        ],
        command_plans: vec![],
        effect_dispositions: vec![],
        ci_step_limit: None,
        deep_domain_overrides: Default::default(),
        named_types: vec![],
    };

    let rows = schema_input_rows_for_pair(&schema, "Idle", "Attached");
    let by_input = rows
        .into_iter()
        .map(|row| (row.input_variant.clone(), row))
        .collect::<std::collections::BTreeMap<_, _>>();

    assert!(matches!(
        by_input["Ping"].classification,
        HopcroftSchemaInputClassification::SameSurface
    ));
    assert!(matches!(
        by_input["Start"].classification,
        HopcroftSchemaInputClassification::LeftOnly
    ));
    assert!(matches!(
        by_input["Retire"].classification,
        HopcroftSchemaInputClassification::DifferentSurface
    ));
}

#[cfg(feature = "machine-authority")]
fn peer_terminal_projection_mismatches(source: &str) -> Vec<String> {
    let parsed = syn::parse_file(source).expect("parse projection fixture");
    let mut visitor =
        PeerResponseTerminalProjectionVisitor::new("crates/meerkat-runtime/src/accept.rs");
    visitor.visit_file(&parsed);
    visitor.mismatches
}

#[cfg(feature = "machine-authority")]
#[test]
fn peer_terminal_projection_flags_construction_string_and_call_via_ast() {
    let flagged = peer_terminal_projection_mismatches(
        r#"
        fn build() {
            let _proj = PeerConversationProjection::ResponseTerminal { id: 1 };
            let _key = peer_response_terminal_context_key(route, correlation);
            let _notice = "[SYSTEM NOTICE][PEER_RESPONSE_TERMINAL] done";
        }
    "#,
    );
    assert_eq!(
        flagged.len(),
        3,
        "all three banned shapes must flag: {flagged:#?}"
    );
}

#[cfg(feature = "machine-authority")]
#[test]
fn peer_terminal_projection_ignores_names_in_comments_and_unrelated_calls() {
    // The banned names appear only in a comment and in an unrelated call/path;
    // the AST carries no banned construction/call/string, so nothing flags.
    let clean = peer_terminal_projection_mismatches(
        r"
        fn build() {
            // PeerConversationProjection::ResponseTerminal is gone; context key too
            let _ = some_other_module::peer_response_terminal_context_key_helper_doc();
            let _ = PeerConversationProjection::Streaming { id: 1 };
        }
    ",
    );
    assert!(
        clean.is_empty(),
        "comment/unrelated mentions must not flag: {clean:#?}"
    );
}

#[cfg(feature = "machine-authority")]
fn peer_terminal_shell_mismatches(source: &str) -> Vec<String> {
    let parsed = syn::parse_file(source).expect("parse shell fixture");
    let mut visitor =
        PeerResponseTerminalShellVisitor::new("crates/meerkat-rpc/src/session_runtime.rs");
    visitor.visit_file(&parsed);
    visitor.mismatches
}

#[cfg(feature = "machine-authority")]
#[test]
fn peer_terminal_shell_flags_pub_identity_bus_and_peer_name_projection() {
    let flagged = peer_terminal_shell_mismatches(
        r"
        pub struct ShellBus {
            pub peer_name: PeerName,
        }
        fn project(peer_name: &PeerName) {
            let _ = PeerResponseTerminalRouteIdentity::parse(peer_name);
            let _ = peer_response_terminal_input(&peer_name, status);
        }
    ",
    );
    assert_eq!(
        flagged.len(),
        3,
        "field + two projections must flag: {flagged:#?}"
    );
}

#[cfg(feature = "machine-authority")]
#[test]
fn peer_terminal_shell_allows_enum_variant_carrier_and_typed_facts() {
    // An enum-variant `peer_name: PeerName` carrier is an allowed shape, and a
    // call that passes typed facts (not `peer_name`) is not a projection.
    let clean = peer_terminal_shell_mismatches(
        r"
        pub enum WirePersistedInput {
            PeerMessage { peer_name: PeerName },
        }
        fn admit(peer_id: PeerId, request_id: PeerCorrelationId) {
            let _ = peer_response_terminal_input(peer_id, None, request_id, status, result);
        }
    ",
    );
    assert!(
        clean.is_empty(),
        "enum carrier + typed-fact call must not flag: {clean:#?}"
    );
}

// ─── AST-level Mob catalog command gate (replaces window/token scanning) ─────

#[cfg(feature = "machine-authority")]
fn mob_gate_check(source: &str, spec: MobCatalogCommandGateSpec) -> bool {
    let parsed = parse_production_file(source).expect("test source parses");
    mob_catalog_command_gate_is_fail_closed(&parsed, spec)
}

#[cfg(feature = "machine-authority")]
#[test]
fn mob_catalog_gate_accepts_try_propagated_admission() {
    let gated = mob_gate_check(
        r#"
        impl Actor {
            async fn handle_cancel_flow(&mut self, run_id: RunId) -> Result<(), MobError> {
                self.apply_command_admission(
                    mob_dsl::MobMachineInput::CancelFlow {
                        run_id: mob_dsl::RunId::from(run_id.to_string()),
                    },
                    MobState::Running,
                    "cancel_flow",
                )?;
                Ok(())
            }
        }
    "#,
        MobCatalogCommandGateSpec {
            input: "CancelFlow",
            scope: MobCatalogCommandGateScope::Function("handle_cancel_flow"),
        },
    );
    assert!(gated, "`?`-propagated admission must satisfy the gate");
}

#[cfg(feature = "machine-authority")]
#[test]
fn mob_catalog_gate_accepts_map_err_chained_admission_in_command_arm() {
    let gated = mob_gate_check(
        r#"
        impl Actor {
            async fn run(&mut self) {
                match command {
                    MobCommand::SetSpawnPolicy { policy, reply_tx } => {
                        let enabled = policy.is_some();
                        let result = self
                            .apply_dsl_input(
                                mob_dsl::MobMachineInput::SetSpawnPolicy { enabled },
                                "set_spawn_policy",
                            )
                            .map_err(|error| MobError::Internal(error.to_string()));
                        let _ = reply_tx.send(result);
                    }
                    _ => {}
                }
            }
        }
    "#,
        MobCatalogCommandGateSpec {
            input: "SetSpawnPolicy",
            scope: MobCatalogCommandGateScope::CommandArm("SetSpawnPolicy"),
        },
    );
    assert!(
        gated,
        "map_err-propagated admission in the arm must satisfy the gate"
    );
}

#[cfg(feature = "machine-authority")]
#[test]
fn mob_catalog_gate_accepts_macro_asserted_variable_input() {
    // handle_run_flow shape: the input is built elsewhere, structurally
    // asserted to be the variant via `debug_assert!(matches!(..))`, and the
    // asserted local is what flows into the gate seam.
    let gated = mob_gate_check(
        r#"
        impl Actor {
            async fn handle_run_flow(&mut self) -> Result<RunId, MobError> {
                let run_flow = MobRun::run_flow_input(&run_id, &config)?;
                debug_assert!(matches!(run_flow, mob_dsl::MobMachineInput::RunFlow { .. }));
                let prepared = self
                    .prepare_dsl_input(run_flow.clone(), "run_flow")
                    .map_err(|_| self.invalid_transition_to(MobState::Running))?;
                self.commit_prepared_dsl_input(prepared)?;
                Ok(run_id)
            }
        }
    "#,
        MobCatalogCommandGateSpec {
            input: "RunFlow",
            scope: MobCatalogCommandGateScope::Function("handle_run_flow"),
        },
    );
    assert!(
        gated,
        "macro-asserted local flowing into the gate seam must satisfy the gate"
    );
}

#[cfg(feature = "machine-authority")]
#[test]
fn mob_catalog_gate_rejects_discarded_admission_result() {
    let gated = mob_gate_check(
        r#"
        impl Actor {
            async fn handle_force_cancel(&mut self, id: MeerkatId) -> Result<(), MobError> {
                let _ = self.apply_dsl_input(
                    mob_dsl::MobMachineInput::ForceCancel { agent_identity: id },
                    "force_cancel",
                );
                self.do_the_cancel(id).await?;
                Ok(())
            }
        }
    "#,
        MobCatalogCommandGateSpec {
            input: "ForceCancel",
            scope: MobCatalogCommandGateScope::Function("handle_force_cancel"),
        },
    );
    assert!(!gated, "a discarded admission result must fail the gate");
}

#[cfg(feature = "machine-authority")]
#[test]
fn mob_catalog_gate_rejects_unrelated_try_in_scope() {
    // The retired window scanner accepted any `?` within ±N lines of the
    // input token. The AST gate requires the `?` (or map_err) to sit on the
    // admission call itself.
    let gated = mob_gate_check(
        r#"
        impl Actor {
            async fn handle_cancel_flow(&mut self, run_id: RunId) -> Result<(), MobError> {
                let outcome = self.apply_dsl_input(
                    mob_dsl::MobMachineInput::CancelFlow { run_id },
                    "cancel_flow",
                );
                self.unrelated_io().await?;
                tracing::debug!(?outcome, "admission outcome ignored");
                Ok(())
            }
        }
    "#,
        MobCatalogCommandGateSpec {
            input: "CancelFlow",
            scope: MobCatalogCommandGateScope::Function("handle_cancel_flow"),
        },
    );
    assert!(
        !gated,
        "a `?` on an unrelated call must not satisfy the gate"
    );
}

#[cfg(feature = "machine-authority")]
#[test]
fn mob_catalog_gate_rejects_missing_scope() {
    let gated = mob_gate_check(
        r"
        impl Actor {
            async fn some_other_fn(&mut self) {}
        }
    ",
        MobCatalogCommandGateSpec {
            input: "RunFlow",
            scope: MobCatalogCommandGateScope::Function("handle_run_flow"),
        },
    );
    assert!(!gated, "a missing scope must fail closed");
}

// machines_test_support.rs stands in for machines.rs without the
// machine-authority feature and includes this file too; the merge helper
// lives only in machines.rs.
#[cfg(feature = "machine-authority")]
#[test]
fn jdk_java_options_mirror_the_tlc_stack_flag_for_the_launcher_main_thread() {
    // The launcher sizes TLC's main thread from JDK_JAVA_OPTIONS, not from
    // JAVA_TOOL_OPTIONS, so the merged stack flag must appear there too.
    assert_eq!(
        super::merge_jdk_java_options("", "-Xss256m -XX:+UseParallelGC"),
        "-Xss256m"
    );
    // An explicit caller stack policy governs both layers.
    assert_eq!(
        super::merge_jdk_java_options("", "-Dmeerkat.sentinel=true -Xss8m"),
        "-Xss8m"
    );
    // Existing launcher options are preserved and never duplicated.
    assert_eq!(
        super::merge_jdk_java_options("-Dlauncher.flag=1", "-Xss256m"),
        "-Xss256m -Dlauncher.flag=1"
    );
    assert_eq!(
        super::merge_jdk_java_options("-Xss1g", "-Xss256m"),
        "-Xss1g"
    );
    // No stack flag anywhere still yields the canonical default.
    assert_eq!(
        super::merge_jdk_java_options("", "-XX:+UseParallelGC"),
        "-Xss256m"
    );
}

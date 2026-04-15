#![allow(clippy::expect_used, clippy::panic)]

#[cfg(feature = "machine-authority")]
use std::collections::BTreeSet;
use std::fs;

use super::*;
#[cfg(feature = "machine-authority")]
use meerkat_machine_schema::{
    EnumSchema, InitSchema, InputMatch, MachineSchema, RustBinding, StateSchema, TransitionSchema,
    TriggerKind, VariantSchema,
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
    let schema = MachineSchema {
        machine: "TestMachine".into(),
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
                        name: "Idle".into(),
                        fields: vec![],
                    },
                    VariantSchema {
                        name: "Attached".into(),
                        fields: vec![],
                    },
                    VariantSchema {
                        name: "Running".into(),
                        fields: vec![],
                    },
                    VariantSchema {
                        name: "Stopped".into(),
                        fields: vec![],
                    },
                ],
            },
            fields: vec![],
            init: InitSchema {
                phase: "Idle".into(),
                fields: vec![],
            },
            terminal_phases: vec!["Stopped".into()],
        },
        inputs: EnumSchema {
            name: "Input".into(),
            variants: vec![
                VariantSchema {
                    name: "Ping".into(),
                    fields: vec![],
                },
                VariantSchema {
                    name: "Start".into(),
                    fields: vec![],
                },
                VariantSchema {
                    name: "Retire".into(),
                    fields: vec![],
                },
            ],
        },
        surface_only_inputs: vec![],
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
                name: "PingIdle".into(),
                from: vec!["Idle".into()],
                on: InputMatch {
                    kind: TriggerKind::Input,
                    variant: "Ping".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![],
                to: "Idle".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "PingAttached".into(),
                from: vec!["Attached".into()],
                on: InputMatch {
                    kind: TriggerKind::Input,
                    variant: "Ping".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![],
                to: "Idle".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "StartIdle".into(),
                from: vec!["Idle".into()],
                on: InputMatch {
                    kind: TriggerKind::Input,
                    variant: "Start".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![],
                to: "Running".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "RetireIdle".into(),
                from: vec!["Idle".into()],
                on: InputMatch {
                    kind: TriggerKind::Input,
                    variant: "Retire".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![],
                to: "Idle".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "RetireAttached".into(),
                from: vec!["Attached".into()],
                on: InputMatch {
                    kind: TriggerKind::Input,
                    variant: "Retire".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![],
                to: "Stopped".into(),
                emit: vec![],
            },
        ],
        effect_dispositions: vec![],
        ci_step_limit: None,
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

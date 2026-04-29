use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    io::Write as _,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, ValueEnum};
use meerkat_machine_codegen::{
    composition_route_coverage_operator_name, composition_scheduler_coverage_operator_name,
    composition_witness_cfg_name, merge_mapping_document, render_composition_ci_cfg,
    render_composition_contract_markdown, render_composition_driver,
    render_composition_mapping_coverage, render_composition_semantic_model,
    render_composition_witness_cfg, render_generated_kernel_mod, render_machine_ci_cfg,
    render_machine_contract_markdown, render_machine_kernel_module,
    render_machine_mapping_coverage, render_machine_semantic_model,
};
use meerkat_machine_schema::{
    CompositionCoverageManifest, CompositionSchema, MachineCoverageManifest, MachineSchema,
    SchedulerRule, SemanticCoverageEntry, TriggerKind, canonical_composition_coverage_manifests,
    canonical_composition_schemas, canonical_machine_coverage_manifests, canonical_machine_schemas,
};
use proc_macro2::TokenTree;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Phase1CarryForwardProductionBody {
    pub machine: &'static str,
    pub path: &'static str,
}

pub const PHASE1_CARRY_FORWARD_PRODUCTION_BODIES: &[Phase1CarryForwardProductionBody] = &[];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProductionMachineOwnerPath {
    pub machine: &'static str,
    pub path: &'static str,
}

pub const PRODUCTION_MACHINE_OWNER_PATHS: &[ProductionMachineOwnerPath] = &[
    ProductionMachineOwnerPath {
        machine: "MeerkatMachine",
        path: "meerkat-runtime/src/meerkat_machine/dsl.rs",
    },
    ProductionMachineOwnerPath {
        machine: "AuthMachine",
        path: "meerkat-runtime/src/auth_machine/dsl.rs",
    },
    ProductionMachineOwnerPath {
        machine: "MobMachine",
        path: "meerkat-mob/src/machines/mob_machine.rs",
    },
    ProductionMachineOwnerPath {
        machine: "ScheduleLifecycleMachine",
        path: "meerkat-schedule/src/machines/schedule_lifecycle.rs",
    },
    ProductionMachineOwnerPath {
        machine: "OccurrenceLifecycleMachine",
        path: "meerkat-schedule/src/machines/occurrence_lifecycle.rs",
    },
];

#[derive(Debug, Clone, Args)]
pub struct SelectionArgs {
    /// Operate on every registered machine and composition.
    #[arg(long)]
    pub all: bool,
    /// Restrict work to one or more machine names or machine slugs.
    #[arg(long = "machine")]
    pub machines: Vec<String>,
    /// Restrict work to one or more composition names.
    #[arg(long = "composition")]
    pub compositions: Vec<String>,
}

#[derive(Debug, Clone, Args)]
pub struct VerifyArgs {
    #[command(flatten)]
    selection: SelectionArgs,
    /// Validate the canonical registry only and skip TLC execution.
    #[arg(long)]
    skip_tlc: bool,
    /// TLC config profile to run.
    #[arg(long, value_enum, default_value_t = VerifyProfile::Ci)]
    profile: VerifyProfile,
    /// TLC worker count. Defaults to local core count or TLC_WORKERS.
    #[arg(long)]
    workers: Option<usize>,
}

#[derive(Debug, Clone, Args)]
pub struct HopcroftArgs {
    #[command(flatten)]
    selection: SelectionArgs,
    /// TLC config profile to dump.
    #[arg(long, value_enum, default_value_t = VerifyProfile::Ci)]
    profile: VerifyProfile,
    /// TLC worker count. Defaults to local core count or TLC_WORKERS.
    #[arg(long)]
    workers: Option<usize>,
    /// Seed the initial quotient partition with a state observation signature.
    #[arg(long, value_enum, default_value_t = HopcroftObservation::None)]
    observation: HopcroftObservation,
    /// Persist DOT dumps, TLC logs, and JSON summaries under this directory.
    #[arg(long)]
    artifact_dir: Option<PathBuf>,
    /// Emit the full audit map: field-ablation summaries plus mixed-phase pair audits.
    #[arg(long)]
    audit_map: bool,
    /// Reuse an existing `graph.dot` under `--artifact-dir` instead of rerunning TLC.
    #[arg(long)]
    reuse_existing_dump: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum VerifyProfile {
    Ci,
    Deep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HopcroftObservation {
    /// Pure behavior quotient seeded with a single initial partition.
    None,
    /// Preserve the `phase` snapshot as part of the observation signature.
    Phase,
    /// Preserve the full snapshot (minus `model_step_count`) as the observation signature.
    Full,
}

pub fn machine_codegen(args: SelectionArgs) -> Result<()> {
    let registry = CanonicalRegistry::load();
    registry.validate()?;

    let selection = registry.select(&args)?;
    let root = repo_root()?;
    println!(
        "machine-codegen: {} machine(s), {} composition(s)",
        selection.machines.len(),
        selection.compositions.len()
    );
    machine_codegen_at_root(&root, &selection)
}

pub fn machine_verify(args: VerifyArgs) -> Result<()> {
    let registry = CanonicalRegistry::load();
    registry.validate()?;

    let selection = registry.select(&args.selection)?;
    let root = repo_root()?;
    let workers = resolve_tlc_workers(args.workers)?;
    println!(
        "machine-verify ({:?}): {} machine(s), {} composition(s), tlc={}",
        args.profile,
        selection.machines.len(),
        selection.compositions.len(),
        !args.skip_tlc
    );
    machine_verify_at_root(&root, &selection, !args.skip_tlc, args.profile, workers)
}

pub fn machine_hopcroft(args: HopcroftArgs) -> Result<()> {
    let registry = CanonicalRegistry::load();
    registry.validate()?;

    let selection = registry.select(&args.selection)?;
    let root = repo_root()?;
    ensure_no_drift(&root, &selection)?;

    if !args.reuse_existing_dump && which::which("tlc").is_err() {
        bail!("tlc not on PATH; machine-hopcroft requires the TLC CLI");
    }

    let workers = resolve_tlc_workers(args.workers)?;
    println!(
        "machine-hopcroft ({:?}, {:?}): {} machine(s), {} composition(s)",
        args.profile,
        args.observation,
        selection.machines.len(),
        selection.compositions.len()
    );

    let artifact_dir = args
        .artifact_dir
        .as_deref()
        .map(|path| {
            let resolved = if path.is_absolute() {
                path.to_path_buf()
            } else {
                root.join(path)
            };
            fs::create_dir_all(&resolved)
                .with_context(|| format!("create artifact dir {}", resolved.display()))?;
            Ok::<PathBuf, anyhow::Error>(resolved)
        })
        .transpose()?;

    let mut items = Vec::new();
    for machine in &selection.machines {
        let dir = machine_dir(&root, &machine.slug);
        let artifact_subdir = artifact_dir.as_deref().map(|base| base.join(&machine.slug));
        items.push(run_hopcroft_for_target(
            &root,
            HopcroftTarget {
                kind: "machine",
                display_name: machine.schema.machine.as_str(),
                slug: &machine.slug,
                dir: &dir,
                machine_schema: Some(&machine.schema),
            },
            args.profile,
            workers,
            args.observation,
            args.audit_map,
            args.reuse_existing_dump,
            artifact_subdir.as_deref(),
        )?);
    }

    for composition in &selection.compositions {
        let dir = composition_dir(&root, &composition.slug);
        let artifact_subdir = artifact_dir
            .as_deref()
            .map(|base| base.join(&composition.slug));
        items.push(run_hopcroft_for_target(
            &root,
            HopcroftTarget {
                kind: "composition",
                display_name: composition.schema.name.as_str(),
                slug: &composition.slug,
                dir: &dir,
                machine_schema: None,
            },
            args.profile,
            workers,
            args.observation,
            args.audit_map,
            args.reuse_existing_dump,
            artifact_subdir.as_deref(),
        )?);
    }

    if let Some(artifact_dir) = artifact_dir {
        let summary_path = artifact_dir.join("summary.json");
        let root_summary = HopcroftRunSummary {
            observation: args.observation,
            profile: verify_profile_name(args.profile).into(),
            items,
        };
        fs::write(
            &summary_path,
            serde_json::to_vec_pretty(&root_summary).context("serialize hopcroft summary")?,
        )
        .with_context(|| format!("write {}", summary_path.display()))?;
        println!("wrote {}", summary_path.display());
    }

    Ok(())
}

pub fn machine_check_drift(args: SelectionArgs) -> Result<()> {
    let registry = CanonicalRegistry::load();
    registry.validate()?;

    let selection = registry.select(&args)?;
    let root = repo_root()?;
    println!(
        "machine-check-drift: checking {} machine(s), {} composition(s)",
        selection.machines.len(),
        selection.compositions.len()
    );
    let mut mismatches = collect_drift_mismatches(&root, &selection)?;
    mismatches.extend(collect_coverage_anchor_mismatches(&root, &selection));
    mismatches.extend(collect_machine_inventory_mismatches(&root)?);
    mismatches.extend(collect_generated_kernel_boundary_mismatches(&root)?);
    mismatches.extend(collect_phase1_production_body_mismatches(&root)?);
    mismatches.extend(collect_authority_language_mismatches(&root)?);
    mismatches.extend(collect_stale_cfg_mismatches(&root)?);
    mismatches.extend(collect_direct_flow_reducer_transition_mismatches(&root)?);
    mismatches.extend(collect_mob_runtime_catalog_command_gate_mismatches(&root)?);

    if !mismatches.is_empty() {
        bail!(
            "machine authority drift detected:\n{}",
            mismatches
                .into_iter()
                .map(|item| format!("- {item}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    println!("machine authority artifacts are up to date");
    Ok(())
}

pub fn machine_codegen_at_root(root: &Path, selection: &Selection) -> Result<()> {
    let registry = CanonicalRegistry::load();
    prune_stale_generated_kernel_modules(root, &registry)?;
    write_generated(
        &generated_kernel_mod_path(root),
        &render_generated_kernel_mod(&registry.machines),
    )?;

    for machine in &selection.machines {
        remove_legacy_authority_path(&machine_authority_path(root, &machine.slug))?;
        write_generated(
            &machine_model_path(root, &machine.slug),
            &render_machine_semantic_model(&machine.schema),
        )?;
        write_generated(
            &machine_ci_path(root, &machine.slug),
            &render_machine_ci_cfg(&machine.schema, false),
        )?;
        write_generated(
            &machine_deep_path(root, &machine.slug),
            &render_machine_ci_cfg(&machine.schema, true),
        )?;
        write_generated(
            &machine_contract_path(root, &machine.slug),
            &render_machine_contract_markdown(&machine.schema, &machine.coverage),
        )?;
        println!(
            "generated {}",
            machine_model_path(root, &machine.slug).display()
        );

        let mapping_path = machine_mapping_path(root, &machine.slug);
        let existing = fs::read_to_string(&mapping_path).ok();
        let merged = merge_mapping_document(
            existing.as_deref(),
            machine.schema.machine.as_str(),
            &render_machine_mapping_coverage(&machine.schema, &machine.coverage),
        );
        write_generated(&mapping_path, &merged)?;
        println!("updated {}", mapping_path.display());

        let generated_slug = generated_kernel_module_slug(&machine.schema.machine);
        write_generated(
            &generated_kernel_module_path(root, &generated_slug),
            &render_machine_kernel_module(&machine.schema),
        )?;
        println!(
            "generated {}",
            generated_kernel_module_path(root, &generated_slug).display()
        );
    }

    for compat in compat_generated_kernel_schemas() {
        let generated_slug = generated_kernel_module_slug(&compat.machine);
        write_generated(
            &mob_generated_machine_module_path(root, &generated_slug),
            &render_machine_kernel_module(&compat),
        )?;
        println!(
            "generated {}",
            mob_generated_machine_module_path(root, &generated_slug).display()
        );
    }

    for composition in &selection.compositions {
        remove_legacy_authority_path(&composition_authority_path(root, &composition.slug))?;
        write_generated(
            &composition_model_path(root, &composition.slug),
            &render_composition_semantic_model(&composition.schema),
        )?;
        write_generated(
            &composition_ci_path(root, &composition.slug),
            &render_composition_ci_cfg(&composition.schema, false),
        )?;
        write_generated(
            &composition_deep_path(root, &composition.slug),
            &render_composition_ci_cfg(&composition.schema, true),
        )?;
        for witness in &composition.schema.witnesses {
            write_generated(
                &composition_witness_path(root, &composition.slug, witness.name.as_str()),
                &render_composition_witness_cfg(&composition.schema, witness),
            )?;
        }
        write_generated(
            &composition_contract_path(root, &composition.slug),
            &render_composition_contract_markdown(&composition.schema, &composition.coverage),
        )?;
        println!(
            "generated {}",
            composition_model_path(root, &composition.slug).display()
        );

        let mapping_path = composition_mapping_path(root, &composition.slug);
        let existing = fs::read_to_string(&mapping_path).ok();
        let merged = merge_mapping_document(
            existing.as_deref(),
            composition.schema.name.as_str(),
            &render_composition_mapping_coverage(&composition.schema, &composition.coverage),
        );
        write_generated(&mapping_path, &merged)?;
        println!("updated {}", mapping_path.display());

        if let Some(driver_code) = render_composition_driver(&composition.schema) {
            let driver_path = composition_driver_path(root, &composition.schema)?;
            write_generated(&driver_path, &driver_code)?;
            println!("generated {}", driver_path.display());
        }
    }

    Ok(())
}

fn machine_verify_at_root(
    root: &Path,
    selection: &Selection,
    run_tlc: bool,
    profile: VerifyProfile,
    workers: usize,
) -> Result<()> {
    ensure_no_drift(root, selection)?;

    for machine in &selection.machines {
        println!("machine: {}", machine.schema.machine);
        if run_tlc {
            let coverage = maybe_run_tlc_in_dir(
                &machine_dir(root, &machine.slug),
                &machine.slug,
                profile,
                workers,
            )?;
            if matches!(profile, VerifyProfile::Deep)
                && let Some(coverage) = coverage
            {
                ensure_machine_transition_coverage(&machine.schema, &coverage)?;
            }
        }
    }

    for composition in &selection.compositions {
        println!("composition: {}", composition.schema.name);
        if run_tlc {
            let main_coverage = maybe_run_tlc_in_dir(
                &composition_dir(root, &composition.slug),
                &composition.slug,
                profile,
                workers,
            )?;
            if matches!(profile, VerifyProfile::Deep) {
                let mut aggregated_coverage = main_coverage.unwrap_or_default();
                let mut witness_covered_routes = BTreeSet::new();
                let mut witness_covered_scheduler_rules = BTreeSet::new();
                for witness in &composition.schema.witnesses {
                    let witness_coverage = maybe_run_tlc_in_dir_with_config(
                        &composition_dir(root, &composition.slug),
                        &composition.slug,
                        &composition_witness_cfg_name(&witness.name),
                        profile,
                        workers,
                    )?;
                    merge_tlc_coverage(&mut aggregated_coverage, witness_coverage.as_ref());
                    witness_covered_routes.extend(
                        witness
                            .expected_routes
                            .iter()
                            .map(|r| r.as_str().to_owned()),
                    );
                    witness_covered_scheduler_rules.extend(
                        witness
                            .expected_scheduler_rules
                            .iter()
                            .map(composition_scheduler_coverage_operator_name),
                    );
                }
                ensure_composition_coverage(
                    &composition.schema,
                    &aggregated_coverage,
                    &witness_covered_routes,
                    &witness_covered_scheduler_rules,
                )?;
            }
        }
    }

    run_generated_kernel_tests(root)?;
    for machine in &selection.machines {
        run_machine_owner_tests(root, machine)?;
    }

    Ok(())
}

fn ensure_no_drift(root: &Path, selection: &Selection) -> Result<()> {
    let mut mismatches = collect_drift_mismatches(root, selection)?;
    mismatches.extend(collect_coverage_anchor_mismatches(root, selection));
    mismatches.extend(collect_machine_inventory_mismatches(root)?);
    mismatches.extend(collect_generated_kernel_boundary_mismatches(root)?);
    mismatches.extend(collect_phase1_production_body_mismatches(root)?);
    mismatches.extend(collect_authority_language_mismatches(root)?);
    mismatches.extend(collect_stale_cfg_mismatches(root)?);
    mismatches.extend(collect_direct_flow_reducer_transition_mismatches(root)?);
    mismatches.extend(collect_mob_runtime_catalog_command_gate_mismatches(root)?);

    if !mismatches.is_empty() {
        bail!(
            "machine authority drift detected:\n{}",
            mismatches
                .into_iter()
                .map(|item| format!("- {item}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    Ok(())
}

pub fn collect_drift_mismatches(root: &Path, selection: &Selection) -> Result<Vec<String>> {
    let mut mismatches = Vec::new();
    let registry = CanonicalRegistry::load();
    let kernel_export_schemas = generated_kernel_export_schemas(&registry);

    for machine in &selection.machines {
        collect_legacy_authority_mismatch(
            &machine_authority_path(root, &machine.slug),
            &mut mismatches,
        );
        compare_generated(
            &machine_model_path(root, &machine.slug),
            &render_machine_semantic_model(&machine.schema),
            &mut mismatches,
        )?;
        compare_generated(
            &machine_ci_path(root, &machine.slug),
            &render_machine_ci_cfg(&machine.schema, false),
            &mut mismatches,
        )?;
        compare_generated(
            &machine_deep_path(root, &machine.slug),
            &render_machine_ci_cfg(&machine.schema, true),
            &mut mismatches,
        )?;
        compare_generated(
            &machine_contract_path(root, &machine.slug),
            &render_machine_contract_markdown(&machine.schema, &machine.coverage),
            &mut mismatches,
        )?;

        let mapping_path = machine_mapping_path(root, &machine.slug);
        let mapping_expected = expected_mapping_document(
            &mapping_path,
            machine.schema.machine.as_str(),
            &render_machine_mapping_coverage(&machine.schema, &machine.coverage),
        )?;
        compare_generated(&mapping_path, &mapping_expected, &mut mismatches)?;
        let generated_slug = generated_kernel_module_slug(&machine.schema.machine);
        compare_generated(
            &generated_kernel_module_path(root, &generated_slug),
            &render_machine_kernel_module(&machine.schema),
            &mut mismatches,
        )?;
    }

    compare_generated(
        &generated_kernel_mod_path(root),
        &render_generated_kernel_mod(&kernel_export_schemas),
        &mut mismatches,
    )?;
    for compat in compat_generated_kernel_schemas() {
        let generated_slug = generated_kernel_module_slug(&compat.machine);
        compare_generated(
            &mob_generated_machine_module_path(root, &generated_slug),
            &render_machine_kernel_module(&compat),
            &mut mismatches,
        )?;
    }

    for composition in &selection.compositions {
        collect_legacy_authority_mismatch(
            &composition_authority_path(root, &composition.slug),
            &mut mismatches,
        );
        compare_generated(
            &composition_model_path(root, &composition.slug),
            &render_composition_semantic_model(&composition.schema),
            &mut mismatches,
        )?;
        compare_generated(
            &composition_ci_path(root, &composition.slug),
            &render_composition_ci_cfg(&composition.schema, false),
            &mut mismatches,
        )?;
        compare_generated(
            &composition_deep_path(root, &composition.slug),
            &render_composition_ci_cfg(&composition.schema, true),
            &mut mismatches,
        )?;
        for witness in &composition.schema.witnesses {
            compare_generated(
                &composition_witness_path(root, &composition.slug, witness.name.as_str()),
                &render_composition_witness_cfg(&composition.schema, witness),
                &mut mismatches,
            )?;
        }
        compare_generated(
            &composition_contract_path(root, &composition.slug),
            &render_composition_contract_markdown(&composition.schema, &composition.coverage),
            &mut mismatches,
        )?;

        let mapping_path = composition_mapping_path(root, &composition.slug);
        let mapping_expected = expected_mapping_document(
            &mapping_path,
            composition.schema.name.as_str(),
            &render_composition_mapping_coverage(&composition.schema, &composition.coverage),
        )?;
        compare_generated(&mapping_path, &mapping_expected, &mut mismatches)?;
        if let Some(driver_code) = render_composition_driver(&composition.schema) {
            compare_generated(
                &composition_driver_path(root, &composition.schema)?,
                &driver_code,
                &mut mismatches,
            )?;
        }
    }

    Ok(mismatches)
}

pub fn collect_authority_language_mismatches(root: &Path) -> Result<Vec<String>> {
    let mut mismatches = Vec::new();
    let banned = ["schema.yaml", "PureHandKernel", "PureHand"];

    for path in authority_language_paths(root)? {
        let contents = fs::read_to_string(&path)
            .with_context(|| format!("read authority-language file {}", path.display()))?;

        for token in banned {
            if contents.contains(token) {
                mismatches.push(format!(
                    "stale authority language `{token}` present in {}",
                    path.display()
                ));
            }
        }
    }

    Ok(mismatches)
}

pub fn collect_direct_flow_reducer_transition_mismatches(root: &Path) -> Result<Vec<String>> {
    let mut mismatches = Vec::new();
    let forbidden_modules = ["flow_run", "flow_frame", "loop_iteration"];
    let forbidden_flow_projection_fields = [
        "phase",
        "step_status",
        "output_recorded",
        "step_condition_results",
        "target_counts",
        "target_success_counts",
        "target_terminal_failure_counts",
        "target_retry_counts",
        "failure_count",
        "consecutive_failure_count",
        "ready_frames",
        "ready_frame_membership",
        "pending_body_frame_loops",
        "pending_body_frame_loop_membership",
        "active_node_count",
        "active_frame_count",
        "last_granted_frame",
        "last_granted_loop",
        "max_active_nodes",
        "max_active_frames",
        "max_frame_depth",
    ];
    let forbidden_frame_projection_fields = [
        "phase",
        "last_admitted_node",
        "node_status",
        "ready_queue",
        "output_recorded",
        "node_condition_results",
    ];
    let forbidden_projection_cas_writes = [
        ".cas_flow_state(",
        ".cas_run_snapshot(",
        ".cas_frame_state(",
        ".cas_complete_step_and_record_output(",
        ".cas_loop_state(",
        ".cas_grant_node_slot(",
        ".cas_start_loop(",
        ".cas_grant_body_frame_start(",
        ".cas_complete_body_frame(",
        ".cas_loop_request_body_frame(",
        ".cas_complete_loop(",
    ];

    for path in production_rust_source_paths(root)? {
        let rel = relative_slash_path(root, &path)?;
        let contents = fs::read_to_string(&path).with_context(|| {
            format!("read flow reducer transition candidate {}", path.display())
        })?;
        let lines = contents.lines().collect::<Vec<_>>();
        let test_only_lines = flow_reducer_test_only_lines(&rel, &lines);
        let mut module_aliases = BTreeMap::new();
        let mut bare_transition_aliases = BTreeMap::new();
        let mut bare_input_aliases = BTreeMap::new();

        for module in forbidden_modules {
            module_aliases.insert(module.to_string(), module.to_string());
        }

        for line in contents.lines() {
            let compact = line.split_whitespace().collect::<Vec<_>>().join(" ");
            for module in forbidden_modules {
                if let Some(alias) = reducer_module_alias(&compact, module) {
                    module_aliases.insert(alias, module.to_string());
                }
                if compact.contains(&format!("{module}::transition"))
                    || compact.contains(&format!("{module}::Input"))
                    || compact.contains(&format!("{module}::{{"))
                {
                    collect_reducer_bare_aliases(
                        &compact,
                        module,
                        &mut bare_transition_aliases,
                        &mut bare_input_aliases,
                    );
                }
            }
        }

        for (line_index, line) in lines.iter().enumerate() {
            if test_only_lines[line_index] {
                continue;
            }
            for alias in module_aliases.keys() {
                let module = module_aliases
                    .get(alias)
                    .map(String::as_str)
                    .unwrap_or(alias);
                for token in [format!("{alias}::transition("), format!("{alias}::Input::")] {
                    if line.contains(&token)
                        && !flow_reducer_direct_use_is_structurally_allowed(
                            &rel, &lines, line_index, module, &token,
                        )
                    {
                        mismatches.push(format!(
                            "direct live-flow reducer transition `{token}` is not MobMachine-command gated: {rel}:{}",
                            line_index + 1
                        ));
                    }
                }
            }
            for (alias, module) in &bare_transition_aliases {
                if line.contains(&format!("{alias}("))
                    && !flow_reducer_direct_use_is_structurally_allowed(
                        &rel, &lines, line_index, module, alias,
                    )
                {
                    mismatches.push(format!(
                        "direct live-flow reducer transition alias `{alias}` is not MobMachine-command gated: {rel}:{}",
                        line_index + 1
                    ));
                }
            }
            for (alias, module) in &bare_input_aliases {
                let token = format!("{alias}::");
                if line.contains(&token)
                    && !flow_reducer_direct_use_is_structurally_allowed(
                        &rel, &lines, line_index, module, &token,
                    )
                {
                    mismatches.push(format!(
                        "direct live-flow reducer input alias `{alias}` is not MobMachine-command gated: {rel}:{}",
                        line_index + 1
                    ));
                }
            }
            for field in forbidden_flow_projection_fields {
                let token = format!(".flow_state.{field}");
                if projection_field_is_directly_written(line, &token) {
                    mismatches.push(format!(
                        "direct live-flow projection mutation `{token}` is not MobMachine-command gated: {rel}:{}",
                        line_index + 1
                    ));
                }
            }
            for field in forbidden_frame_projection_fields {
                let token = format!(".kernel_state.{field}");
                if projection_field_is_directly_written(line, &token) {
                    mismatches.push(format!(
                        "direct live-flow projection mutation `{token}` is not MobMachine-command gated: {rel}:{}",
                        line_index + 1
                    ));
                }
            }
            for token in forbidden_projection_cas_writes {
                if line.contains(token)
                    && !flow_reducer_projection_commit_is_structurally_allowed(
                        &rel, &lines, line_index,
                    )
                {
                    mismatches.push(format!(
                        "direct live-flow projection mutation `{token}` is not MobMachine-command gated: {rel}:{}",
                        line_index + 1
                    ));
                }
            }
        }
    }

    Ok(mismatches)
}

pub fn collect_mob_runtime_catalog_command_gate_mismatches(root: &Path) -> Result<Vec<String>> {
    let actor_path = root.join("meerkat-mob/src/runtime/actor.rs");
    if !actor_path.exists() {
        return Ok(Vec::new());
    }

    let contents = fs::read_to_string(&actor_path)
        .with_context(|| format!("read Mob runtime actor {}", actor_path.display()))?;
    let lines = contents.lines().collect::<Vec<_>>();
    let rel = relative_slash_path(root, &actor_path)?;
    let critical_inputs = [
        MobCatalogCommandGateSpec {
            input: "RunFlow",
            scope: MobCatalogCommandGateScope::Function("handle_run_flow"),
        },
        MobCatalogCommandGateSpec {
            input: "CancelFlow",
            scope: MobCatalogCommandGateScope::Function("handle_cancel_flow"),
        },
        MobCatalogCommandGateSpec {
            input: "TaskCreate",
            scope: MobCatalogCommandGateScope::Function("handle_task_create"),
        },
        MobCatalogCommandGateSpec {
            input: "SetSpawnPolicy",
            scope: MobCatalogCommandGateScope::CommandArm("SetSpawnPolicy"),
        },
        MobCatalogCommandGateSpec {
            input: "ForceCancel",
            scope: MobCatalogCommandGateScope::Function("handle_force_cancel"),
        },
    ];
    let mut mismatches = Vec::new();

    for spec in critical_inputs {
        let scope = spec.scope.resolve(&lines);
        let gated = scope.is_some_and(|(start, end)| {
            mob_catalog_input_has_fail_closed_gate(&lines[start..=end], spec.input)
        });
        if !gated {
            mismatches.push(format!(
                "MobCommand::{} is catalog-classified but does not fail-close on MobMachineInput::{} in {}",
                spec.input, spec.input, rel
            ));
        }
    }

    Ok(mismatches)
}

#[derive(Debug, Clone, Copy)]
struct MobCatalogCommandGateSpec {
    input: &'static str,
    scope: MobCatalogCommandGateScope,
}

#[derive(Debug, Clone, Copy)]
enum MobCatalogCommandGateScope {
    Function(&'static str),
    CommandArm(&'static str),
}

impl MobCatalogCommandGateScope {
    fn resolve(self, lines: &[&str]) -> Option<(usize, usize)> {
        match self {
            Self::Function(name) => function_scope_bounds(lines, name),
            Self::CommandArm(variant) => command_arm_scope_bounds(lines, variant),
        }
    }
}

fn reducer_module_alias(line: &str, module: &str) -> Option<String> {
    let marker = format!("{module} as ");
    let index = line.find(&marker)?;
    Some(
        line[index + marker.len()..]
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
            .collect(),
    )
    .filter(|alias: &String| !alias.is_empty())
}

fn collect_reducer_bare_aliases(
    line: &str,
    module: &str,
    transition_aliases: &mut BTreeMap<String, String>,
    input_aliases: &mut BTreeMap<String, String>,
) {
    if let Some((_, after)) = line.split_once(&format!("{module}::transition")) {
        if let Some(alias) = after
            .trim_start()
            .strip_prefix("as ")
            .map(|value| {
                value
                    .chars()
                    .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
                    .collect::<String>()
            })
            .filter(|alias| !alias.is_empty())
        {
            transition_aliases.insert(alias, module.to_string());
        } else {
            transition_aliases.insert("transition".to_string(), module.to_string());
        }
    }
    if let Some((_, after)) = line.split_once(&format!("{module}::Input")) {
        if let Some(alias) = after
            .trim_start()
            .strip_prefix("as ")
            .map(|value| {
                value
                    .chars()
                    .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
                    .collect::<String>()
            })
            .filter(|alias| !alias.is_empty())
        {
            input_aliases.insert(alias, module.to_string());
        } else {
            input_aliases.insert("Input".to_string(), module.to_string());
        }
    }
    if line.contains(&format!("{module}::*")) {
        transition_aliases.insert("transition".to_string(), module.to_string());
        input_aliases.insert("Input".to_string(), module.to_string());
    }
    if let Some((_, imports)) = line.split_once(&format!("{module}::{{"))
        && let Some((imports, _)) = imports.split_once('}')
    {
        for item in imports.split(',').map(str::trim) {
            if let Some(alias) = item.strip_prefix("transition as ") {
                let alias = alias.trim();
                if !alias.is_empty() {
                    transition_aliases.insert(alias.to_string(), module.to_string());
                }
            } else if item == "transition" {
                transition_aliases.insert("transition".to_string(), module.to_string());
            } else if let Some(alias) = item.strip_prefix("Input as ") {
                let alias = alias.trim();
                if !alias.is_empty() {
                    input_aliases.insert(alias.to_string(), module.to_string());
                }
            } else if item == "Input" {
                input_aliases.insert("Input".to_string(), module.to_string());
            }
        }
    }
}

fn projection_field_is_directly_written(line: &str, token: &str) -> bool {
    let Some(index) = line.find(token) else {
        return false;
    };
    let tail = &line[index + token.len()..];
    let tail = tail.trim_start();
    (tail.starts_with('=') && !tail.starts_with("==") && !tail.starts_with("=>"))
        || tail.starts_with("+=")
        || tail.starts_with("-=")
        || tail.starts_with(".insert(")
        || tail.starts_with(".remove(")
        || tail.starts_with(".clear(")
        || tail.starts_with(".push(")
        || tail.starts_with(".retain(")
}

fn mob_catalog_input_has_fail_closed_gate(lines: &[&str], input: &str) -> bool {
    let token = format!("MobMachineInput::{input}");
    lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.contains(&token))
        .any(|(line_index, _)| mob_catalog_input_occurrence_is_fail_closed(lines, line_index))
}

fn mob_catalog_input_occurrence_is_fail_closed(lines: &[&str], line_index: usize) -> bool {
    let start = line_index.saturating_sub(4);
    let end = (line_index + 18).min(lines.len());
    let window = lines[start..end].join("\n");
    let compact = window.split_whitespace().collect::<Vec<_>>().join(" ");

    if compact.contains("let _ = self.apply_dsl_input") || compact.contains("may diverge") {
        return false;
    }

    compact.contains("probe_mob_machine_input(")
        || (compact.contains("apply_dsl_input(")
            && (compact.contains('?')
                || compact.contains("return Err")
                || compact.contains("continue;")
                || compact.contains(".map_err(")))
        || (compact.contains("prepare_dsl_input(")
            && (compact.contains('?')
                || compact.contains("return Err")
                || compact.contains("continue;")
                || compact.contains(".map_err(")))
        || (compact.contains("apply_command_admission(")
            && (compact.contains('?')
                || compact.contains("return Err")
                || compact.contains("continue;")
                || compact.contains(".map_err(")))
        || (compact.contains("prepare_command_admission(")
            && (compact.contains('?')
                || compact.contains("return Err")
                || compact.contains("continue;")
                || compact.contains(".map_err(")))
}

fn function_scope_bounds(lines: &[&str], function_name: &str) -> Option<(usize, usize)> {
    let needle = format!("fn{function_name}(");
    let start = lines.iter().position(|line| {
        let compact = line.split_whitespace().collect::<String>();
        compact.contains(&needle)
    })?;
    brace_scope_bounds(lines, start)
}

fn command_arm_scope_bounds(lines: &[&str], variant: &str) -> Option<(usize, usize)> {
    let needle = format!("MobCommand::{variant}");
    let start = lines.iter().position(|line| line.contains(&needle))?;
    brace_scope_bounds(lines, start)
}

fn brace_scope_bounds(lines: &[&str], start: usize) -> Option<(usize, usize)> {
    let mut depth = 0usize;
    let mut opened = false;
    for (index, line) in lines.iter().enumerate().skip(start) {
        for ch in line.chars() {
            match ch {
                '{' => {
                    depth += 1;
                    opened = true;
                }
                '}' if depth > 0 => depth -= 1,
                _ => {}
            }
        }
        if opened && depth == 0 {
            return Some((start, index));
        }
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FlowReducerFamily {
    FlowRun,
    FlowFrame,
    LoopIteration,
}

impl FlowReducerFamily {
    fn from_module(module: &str) -> Option<Self> {
        match module {
            "flow_run" => Some(Self::FlowRun),
            "flow_frame" => Some(Self::FlowFrame),
            "loop_iteration" => Some(Self::LoopIteration),
            _ => None,
        }
    }

    fn module(self) -> &'static str {
        match self {
            Self::FlowRun => "flow_run",
            Self::FlowFrame => "flow_frame",
            Self::LoopIteration => "loop_iteration",
        }
    }

    fn apply_function(self) -> &'static str {
        match self {
            Self::FlowRun => "apply_mob_machine_flow_run_command",
            Self::FlowFrame => "apply_mob_machine_flow_frame_command",
            Self::LoopIteration => "apply_mob_machine_loop_iteration_command",
        }
    }

    fn command_type(self) -> &'static str {
        match self {
            Self::FlowRun => "MobMachineFlowRunCommand",
            Self::FlowFrame => "MobMachineFlowFrameCommand",
            Self::LoopIteration => "MobMachineLoopIterationCommand",
        }
    }

    fn authority_kind(self) -> &'static str {
        match self {
            Self::FlowRun => "MobMachineFlowAuthorityKind::FlowRun",
            Self::FlowFrame => "MobMachineFlowAuthorityKind::FlowFrame",
            Self::LoopIteration => "MobMachineFlowAuthorityKind::LoopIteration",
        }
    }
}

fn flow_reducer_test_only_lines(path: &str, lines: &[&str]) -> Vec<bool> {
    if path.ends_with("/tests.rs") {
        return vec![true; lines.len()];
    }

    let mut test_only = Vec::with_capacity(lines.len());
    let mut in_test_module = false;
    for (index, line) in lines.iter().enumerate() {
        if index > 0 && lines[index - 1].trim() == "#[cfg(test)]" && line.contains("mod tests") {
            in_test_module = true;
        }
        test_only.push(in_test_module);
    }
    test_only
}

fn flow_reducer_direct_use_is_structurally_allowed(
    path: &str,
    lines: &[&str],
    line_index: usize,
    module: &str,
    token: &str,
) -> bool {
    if path != "meerkat-mob/src/run.rs" {
        return false;
    }
    let Some(family) = FlowReducerFamily::from_module(module) else {
        return false;
    };

    let Some(function_start) = nearest_function_start(lines, line_index) else {
        return false;
    };
    let function_name = function_name_from_signature(lines[function_start]);

    if function_name.as_deref() == Some(family.apply_function())
        && token.contains("transition")
        && lines[function_start..=line_index]
            .iter()
            .any(|line| line.contains(&format!("authority.require({}", family.authority_kind())))
    {
        return true;
    }

    let function_signature = lines[function_start].split_whitespace().collect::<String>();
    let expected_input_return = format!("->{}::Input", family.module());
    function_name.as_deref() == Some("into_input")
        && nearest_impl_start(lines, function_start)
            .is_some_and(|impl_start| lines[impl_start].contains(family.command_type()))
        && function_signature.contains(expected_input_return.as_str())
        && token.contains("Input::")
}

fn flow_reducer_projection_commit_is_structurally_allowed(
    path: &str,
    lines: &[&str],
    line_index: usize,
) -> bool {
    let Some(function_start) = nearest_function_start(lines, line_index) else {
        return false;
    };
    let function_name = function_name_from_signature(lines[function_start]);
    let scope = &lines[function_start..=line_index];

    match path {
        "meerkat-mob/src/runtime/flow.rs" => {
            let signature = compact_function_signature(lines, function_start);
            let call_window = lines[line_index..(line_index + 8).min(lines.len())].join(" ");
            let has_authority_setup = scope
                .iter()
                .any(|line| line.contains("project_machine_input("))
                && scope
                    .iter()
                    .any(|line| line.contains("from_accepted_mob_machine_input("));
            let has_typed_authority_args = signature.contains("MobMachineFlowRunCommand")
                && signature.contains("MobMachineFlowAuthorityToken");
            let has_typed_authority_token_and_command_source = signature
                .contains("MobMachineFlowAuthorityToken")
                && (signature.contains("MobMachineFlowRunCommand")
                    || scope
                        .iter()
                        .any(|line| line.contains("MobMachineFlowRunCommand::")));
            scope
                .iter()
                .any(|line| line.contains("apply_mob_machine_flow_run_command("))
                && (has_authority_setup
                    || has_typed_authority_args
                    || has_typed_authority_token_and_command_source)
                && (call_window.contains("outcome.next_state")
                    || call_window.contains("next_state"))
        }
        "meerkat-mob/src/runtime/actor.rs" => {
            if function_name.as_deref() != Some("commit_flow_frame_store_plan_in_actor") {
                return false;
            }
            let function_commits_prepared_inputs = lines
                .iter()
                .skip(function_start)
                .take(420)
                .any(|line| line.contains("commit_prepared_dsl_input(prepared)"));
            let function_prepares_machine_inputs = lines
                .iter()
                .skip(function_start)
                .take(420)
                .any(|line| line.contains("prepare_dsl_inputs(plan.machine_inputs()"));
            if !function_prepares_machine_inputs || !function_commits_prepared_inputs {
                return false;
            }
            let window_start = line_index.saturating_sub(32);
            let local_scope = &lines[window_start..=line_index];
            let call_window = lines[line_index..(line_index + 12).min(lines.len())].join(" ");
            (call_window.contains(".cas_flow_state(")
                && call_window.contains("next_run_state")
                && local_scope
                    .iter()
                    .any(|line| line.contains("FlowFrameLoopStorePlan::RunStateOnly")))
                || (call_window.contains(".cas_frame_state(")
                    && call_window.contains("next_frame")
                    && local_scope
                        .iter()
                        .any(|line| line.contains("FlowFrameLoopStorePlan::FrameState")))
                || (call_window.contains(".cas_frame_state(")
                    && call_window.contains("initial_frame")
                    && local_scope
                        .iter()
                        .any(|line| line.contains("FlowFrameLoopStorePlan::InsertFrame")))
                || (call_window.contains(".cas_frame_state(")
                    && call_window.contains("next_frame")
                    && local_scope
                        .iter()
                        .any(|line| line.contains("FlowFrameLoopStorePlan::SealFrame")))
                || (call_window.contains(".cas_complete_step_and_record_output(")
                    && local_scope.iter().any(|line| {
                        line.contains("FlowFrameLoopStorePlan::CompleteStepAndRecordOutput")
                    }))
                || (call_window.contains(".cas_grant_node_slot(")
                    && local_scope
                        .iter()
                        .any(|line| line.contains("FlowFrameLoopStorePlan::GrantNodeSlot")))
                || (call_window.contains(".cas_start_loop(")
                    && local_scope
                        .iter()
                        .any(|line| line.contains("FlowFrameLoopStorePlan::StartLoop")))
                || (call_window.contains(".cas_grant_body_frame_start(")
                    && local_scope
                        .iter()
                        .any(|line| line.contains("FlowFrameLoopStorePlan::GrantBodyFrameStart")))
                || (call_window.contains(".cas_complete_body_frame(")
                    && local_scope
                        .iter()
                        .any(|line| line.contains("FlowFrameLoopStorePlan::CompleteBodyFrame")))
                || (call_window.contains(".cas_loop_request_body_frame(")
                    && local_scope
                        .iter()
                        .any(|line| line.contains("FlowFrameLoopStorePlan::LoopRequestBodyFrame")))
                || (call_window.contains(".cas_complete_loop(")
                    && local_scope
                        .iter()
                        .any(|line| line.contains("FlowFrameLoopStorePlan::CompleteLoop")))
        }
        "meerkat-mob/src/runtime/flow_frame_engine.rs" => false,
        _ => false,
    }
}

fn nearest_function_start(lines: &[&str], line_index: usize) -> Option<usize> {
    (0..=line_index).rev().find(|index| {
        let compact = lines[*index].split_whitespace().collect::<String>();
        compact.starts_with("fn")
            || compact.starts_with("pubfn")
            || compact.starts_with("pub(crate)fn")
            || compact.starts_with("pub(super)fn")
            || compact.starts_with("asyncfn")
            || compact.starts_with("pubasyncfn")
            || compact.starts_with("pub(crate)asyncfn")
            || compact.starts_with("pub(super)asyncfn")
    })
}

fn nearest_impl_start(lines: &[&str], line_index: usize) -> Option<usize> {
    (0..=line_index).rev().find(|index| {
        let compact = lines[*index].split_whitespace().collect::<String>();
        compact.starts_with("impl")
    })
}

fn compact_function_signature(lines: &[&str], function_start: usize) -> String {
    let mut signature = String::new();
    for line in &lines[function_start..] {
        for part in line.split_whitespace() {
            signature.push_str(part);
        }
        if line.contains('{') {
            break;
        }
    }
    signature
}

fn function_name_from_signature(line: &str) -> Option<String> {
    let compact = line.split_whitespace().collect::<String>();
    let after_fn = compact.split_once("fn")?.1;
    let name = after_fn
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
        .collect::<String>();
    (!name.is_empty()).then_some(name)
}

pub fn collect_generated_kernel_boundary_mismatches(root: &Path) -> Result<Vec<String>> {
    let registry = CanonicalRegistry::load();
    let mut mismatches = Vec::new();
    let expected_generated = expected_generated_kernel_modules(&registry);

    for retired in retired_generated_source_paths() {
        let retired_path = root.join(retired);
        if retired_path.exists() {
            mismatches.push(format!(
                "retired generated-source/bridge path must be removed {retired}"
            ));
        }
    }

    for machine in &registry.machines {
        let slug = generated_kernel_module_slug(&machine.machine);
        let generated_kernel = generated_kernel_module_path(root, &slug);
        if !generated_kernel.exists() {
            continue;
        }

        let owner_paths = PRODUCTION_MACHINE_OWNER_PATHS
            .iter()
            .filter(|owner| owner.machine == machine.machine.as_str())
            .collect::<Vec<_>>();
        if owner_paths.is_empty() {
            mismatches.push(format!(
                "missing production owner audit path for {} while generated kernel {} exists",
                machine.machine,
                generated_kernel.display()
            ));
            continue;
        }

        for owner in owner_paths {
            let owner_path = root.join(owner.path);
            if !owner_path.exists() {
                mismatches.push(format!(
                    "production owner audit path for {} is missing: {}",
                    machine.machine,
                    owner_path.display()
                ));
                continue;
            }
            let source = fs::read_to_string(&owner_path)
                .with_context(|| format!("read production owner {}", owner_path.display()))?;
            if !source.contains("machine") {
                continue;
            }
            let parsed = syn::parse_file(&source)
                .with_context(|| format!("parse production owner {}", owner_path.display()))?;
            for declared_machine in machine_macro_names(&parsed) {
                if declared_machine == machine.machine.as_str() {
                    mismatches.push(format!(
                        "production owner module must not define canonical machine! body after generated-kernel cutover: {} in {}",
                        machine.machine,
                        owner.path
                    ));
                }
            }
        }
    }

    let generated_mod = generated_kernel_mod_path(root);
    let generated_dir = generated_mod
        .parent()
        .context("generated kernel mod parent")?;
    if generated_dir.exists() {
        for entry in fs::read_dir(generated_dir)
            .with_context(|| format!("read {}", generated_dir.display()))?
        {
            let entry = entry.with_context(|| {
                format!("scan generated kernel dir {}", generated_dir.display())
            })?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if path.file_name().and_then(|name| name.to_str()) == Some("mod.rs") {
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            if !expected_generated.contains(stem) {
                mismatches.push(format!(
                    "stale generated kernel module must be removed {}",
                    path.display()
                ));
            }
        }
    }

    Ok(mismatches)
}

fn retired_generated_source_paths() -> &'static [&'static str] {
    &[
        "meerkat-machine-kernels/src/compat_generated.rs",
        "meerkat-machine-kernels/src/generated/flow_run.rs",
        "meerkat-machine-kernels/src/generated/flow_frame.rs",
        "meerkat-machine-kernels/src/generated/loop_iteration.rs",
        "meerkat-mob/src/generated/flow_run.rs",
        "meerkat-mob/src/generated/flow_frame.rs",
        "meerkat-mob/src/generated/loop_iteration.rs",
        "meerkat-mob/src/generated/flow_frame_loop_driver.rs",
        "meerkat-mob/src/runtime/flow_run_kernel.rs",
        "meerkat-mob/src/runtime/flow_frame_kernel.rs",
        "meerkat-mob/src/runtime/loop_iteration_authority.rs",
    ]
}

pub fn collect_phase1_production_body_mismatches(root: &Path) -> Result<Vec<String>> {
    let registry = CanonicalRegistry::load();
    let canonical_machines: BTreeSet<String> = registry
        .machines
        .iter()
        .map(|machine| machine.machine.as_str().to_owned())
        .collect();
    let carry_forward: BTreeSet<(&'static str, &'static str)> =
        PHASE1_CARRY_FORWARD_PRODUCTION_BODIES
            .iter()
            .map(|body| (body.path, body.machine))
            .collect();
    let mut seen_carry_forward = BTreeSet::new();
    let mut mismatches = Vec::new();

    for path in production_rust_source_paths(root)? {
        let rel = relative_slash_path(root, &path)?;
        let source = fs::read_to_string(&path)
            .with_context(|| format!("read machine body candidate {}", path.display()))?;
        if !source.contains("machine") {
            continue;
        }
        let parsed = syn::parse_file(&source)
            .with_context(|| format!("parse machine body candidate {}", path.display()))?;

        for machine in machine_macro_names(&parsed) {
            if !canonical_machines.contains(&machine) {
                continue;
            }
            if is_catalog_dsl_path(&rel) {
                continue;
            }
            if is_expected_generated_kernel_body(&rel, &machine) {
                continue;
            }
            if carry_forward.contains(&(rel.as_str(), machine.as_str())) {
                seen_carry_forward.insert((rel.clone(), machine));
                continue;
            }
            mismatches.push(format!(
                "canonical machine! body outside catalog is not Phase 1 carry-forward debt: {machine} in {rel}"
            ));
        }
    }

    for body in PHASE1_CARRY_FORWARD_PRODUCTION_BODIES {
        if !seen_carry_forward.contains(&(body.path.to_owned(), body.machine.to_owned())) {
            mismatches.push(format!(
                "Phase 1 carry-forward production body is not present as declared: {} in {}",
                body.machine, body.path
            ));
        }
    }

    Ok(mismatches)
}

pub fn collect_coverage_anchor_mismatches(root: &Path, selection: &Selection) -> Vec<String> {
    let mut mismatches = Vec::new();

    for machine in &selection.machines {
        for anchor in &machine.coverage.code_anchors {
            let path = root.join(&anchor.path);
            if !path.exists() {
                mismatches.push(format!(
                    "missing machine coverage anchor {} for {}",
                    path.display(),
                    machine.schema.machine
                ));
            }
        }
    }

    for composition in &selection.compositions {
        for anchor in &composition.coverage.code_anchors {
            let path = root.join(&anchor.path);
            if !path.exists() {
                mismatches.push(format!(
                    "missing composition coverage anchor {} for {}",
                    path.display(),
                    composition.schema.name
                ));
            }
        }
    }

    mismatches
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MachineOwnerInventoryRow {
    machine: String,
    final_mode: String,
    owner_crate: String,
}

pub fn collect_machine_inventory_mismatches(root: &Path) -> Result<Vec<String>> {
    let registry = CanonicalRegistry::load();

    // The doc-based owner/mode inventories were in docs/architecture/0.5/ which
    // has been removed (the machine schema catalog is the sole source of truth).
    // Skip those cross-reference checks when the docs don't exist.
    let impl_plan_path = root.join("docs/architecture/0.5/meerkat_0_5_implementation_plan.md");
    let strategy_path =
        root.join("docs/architecture/0.5/meerkat_machine_formalization_strategy.md");
    let owner_inventory = if impl_plan_path.exists() {
        parse_owner_inventory(&fs::read_to_string(&impl_plan_path)?)?
    } else {
        BTreeMap::new()
    };
    let final_mode_inventory = if strategy_path.exists() {
        parse_final_mode_inventory(&fs::read_to_string(&strategy_path)?)?
    } else {
        BTreeMap::new()
    };

    let mut mismatches = Vec::new();
    let registry_names: BTreeSet<String> = registry
        .machines
        .iter()
        .map(|machine| machine.machine.as_str().to_owned())
        .collect();

    // Only cross-reference against doc inventories when the docs exist.
    if !owner_inventory.is_empty() {
        let owner_names: BTreeSet<String> = owner_inventory.keys().cloned().collect();
        for machine in registry_names.difference(&owner_names) {
            mismatches.push(format!(
                "canonical machine {machine} missing from implementation plan owner map"
            ));
        }
        for machine in owner_names.difference(&registry_names) {
            mismatches.push(format!(
                "untracked machine {machine} present in implementation plan owner map"
            ));
        }
    }
    if !final_mode_inventory.is_empty() {
        let final_mode_names: BTreeSet<String> = final_mode_inventory.keys().cloned().collect();
        for machine in registry_names.difference(&final_mode_names) {
            mismatches.push(format!(
                "canonical machine {machine} missing from formalization strategy final mode table"
            ));
        }
        for machine in final_mode_names.difference(&registry_names) {
            mismatches.push(format!(
                "untracked machine {machine} present in formalization strategy final mode table"
            ));
        }
    }

    let specs_machine_root = root.join("specs/machines");
    let actual_machine_dirs = canonical_machine_dirs(&specs_machine_root)?;
    let expected_machine_dirs: BTreeSet<String> = registry
        .machines
        .iter()
        .map(|machine| machine_slug(&machine.machine))
        .collect();
    for slug in expected_machine_dirs.difference(&actual_machine_dirs) {
        mismatches.push(format!(
            "missing canonical machine artifact directory {}",
            specs_machine_root.join(slug).display()
        ));
    }
    for slug in actual_machine_dirs.difference(&expected_machine_dirs) {
        mismatches.push(format!(
            "untracked canonical machine directory {}",
            specs_machine_root.join(slug).display()
        ));
    }

    let specs_composition_root = root.join("specs/compositions");
    let actual_composition_dirs = canonical_machine_dirs(&specs_composition_root)?;
    let expected_composition_dirs: BTreeSet<String> = registry
        .compositions
        .iter()
        .map(|c| composition_slug(&c.name))
        .collect();
    for slug in expected_composition_dirs.difference(&actual_composition_dirs) {
        mismatches.push(format!(
            "missing canonical composition artifact directory {}",
            specs_composition_root.join(slug).display()
        ));
    }
    for slug in actual_composition_dirs.difference(&expected_composition_dirs) {
        mismatches.push(format!(
            "untracked canonical composition directory {}",
            specs_composition_root.join(slug).display()
        ));
    }

    for machine in &registry.machines {
        let Some(owner_row) = owner_inventory.get(machine.machine.as_str()) else {
            continue;
        };
        let Some(required_final_mode) = final_mode_inventory.get(machine.machine.as_str()) else {
            continue;
        };
        if owner_row.owner_crate != machine.rust.crate_name {
            mismatches.push(format!(
                "owner inventory mismatch for {}: docs say {}, registry says {}",
                machine.machine, owner_row.owner_crate, machine.rust.crate_name
            ));
        }
        if owner_row.final_mode != *required_final_mode {
            mismatches.push(format!(
                "final mode inventory mismatch for {}: owner map says {}, final mode table says {}",
                machine.machine, owner_row.final_mode, required_final_mode
            ));
        }
        if required_final_mode != "SchemaKernel" {
            mismatches.push(format!(
                "invalid final mode {} for {}: canonical 0.5 machine inventories must converge on SchemaKernel",
                required_final_mode, machine.machine
            ));
        }

        let slug = machine_slug(&machine.machine);
        let generated_slug = generated_kernel_module_slug(&machine.machine);
        for artifact_path in [
            machine_contract_path(root, &slug),
            machine_model_path(root, &slug),
            machine_ci_path(root, &slug),
            machine_deep_path(root, &slug),
            machine_mapping_path(root, &slug),
            generated_kernel_module_path(root, &generated_slug),
        ] {
            if !artifact_path.exists() {
                mismatches.push(format!(
                    "missing canonical machine artifact {}",
                    artifact_path.display()
                ));
            }
        }
    }

    Ok(mismatches)
}

fn canonical_machine_dirs(root: &Path) -> Result<BTreeSet<String>> {
    let mut dirs = BTreeSet::new();
    if !root.exists() {
        return Ok(dirs);
    }

    for entry in fs::read_dir(root).with_context(|| format!("read {}", root.display()))? {
        let entry = entry.with_context(|| format!("iterate {}", root.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("read file type {}", path.display()))?;
        if !file_type.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        dirs.insert(name.to_owned());
    }

    Ok(dirs)
}

fn parse_owner_inventory(contents: &str) -> Result<BTreeMap<String, MachineOwnerInventoryRow>> {
    let mut rows = BTreeMap::new();
    for row in parse_markdown_table_rows(contents, "## Canonical Machine Owner Map")? {
        if row.len() < 3 {
            bail!("owner map row must contain machine, final mode, and owner crate");
        }
        let entry = MachineOwnerInventoryRow {
            machine: row[0].clone(),
            final_mode: row[1].clone(),
            owner_crate: row[2].clone(),
        };
        if rows.insert(entry.machine.clone(), entry).is_some() {
            bail!("duplicate machine in canonical owner map");
        }
    }
    Ok(rows)
}

fn parse_final_mode_inventory(contents: &str) -> Result<BTreeMap<String, String>> {
    let mut rows = BTreeMap::new();
    for row in parse_markdown_table_rows(contents, "## Final 0.5 Machine Modes")? {
        if row.len() < 2 {
            bail!("final mode row must contain machine and required final mode");
        }
        if rows.insert(row[0].clone(), row[1].clone()).is_some() {
            bail!("duplicate machine in final mode inventory");
        }
    }
    Ok(rows)
}

fn parse_markdown_table_rows(contents: &str, heading: &str) -> Result<Vec<Vec<String>>> {
    let mut in_section = false;
    let mut rows = Vec::new();

    for line in contents.lines() {
        let trimmed = line.trim();

        if !in_section {
            if trimmed == heading {
                in_section = true;
            }
            continue;
        }

        if trimmed.starts_with("## ") && !rows.is_empty() {
            break;
        }
        if !trimmed.starts_with('|') {
            if !rows.is_empty() {
                break;
            }
            continue;
        }

        let columns: Vec<String> = trimmed
            .trim_matches('|')
            .split('|')
            .map(|column| column.trim().replace('`', ""))
            .collect();
        if columns.is_empty() {
            continue;
        }
        if columns[0] == "Machine" {
            continue;
        }
        if columns.iter().all(|column| {
            !column.is_empty() && column.chars().all(|ch| ch == '-' || ch == ':' || ch == ' ')
        }) {
            continue;
        }
        rows.push(columns);
    }

    if rows.is_empty() {
        bail!("failed to parse markdown table under {heading}");
    }

    Ok(rows)
}

fn authority_language_paths(root: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();

    collect_text_paths(&root.join("docs"), &mut paths)?;

    for path in [
        root.join("specs/machines/README.md"),
        root.join("specs/compositions/README.md"),
    ] {
        if path.exists() {
            paths.push(path);
        }
    }

    Ok(paths)
}

pub fn owner_module_dir(root: &Path, crate_name: &str, module: &str) -> PathBuf {
    let mut path = root.join(crate_name).join("src");
    let mut segments = module.split("::").peekable();
    while let Some(segment) = segments.next() {
        if segments.peek().is_none() {
            break;
        }
        path.push(segment);
    }
    path
}

pub fn owner_module_file(root: &Path, crate_name: &str, module: &str) -> Result<PathBuf> {
    let mut path = owner_module_dir(root, crate_name, module);
    let leaf = module
        .rsplit("::")
        .next()
        .ok_or_else(|| anyhow!("Rust module path always has a leaf segment"))?;
    path.push(format!("{leaf}.rs"));
    Ok(path)
}

fn collect_text_paths(dir: &Path, paths: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }

    for entry in fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = entry.with_context(|| format!("iterate {}", dir.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("read file type {}", path.display()))?;

        if file_type.is_dir() {
            collect_text_paths(&path, paths)?;
            continue;
        }

        let is_text = matches!(
            path.extension().and_then(|ext| ext.to_str()),
            Some("md" | "mdx" | "yaml" | "yml" | "txt")
        );
        if is_text {
            paths.push(path);
        }
    }

    Ok(())
}

fn production_rust_source_paths(root: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(root).with_context(|| format!("read {}", root.display()))? {
        let entry = entry.with_context(|| format!("iterate {}", root.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("read file type {}", path.display()))?;
        if !file_type.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name == "meerkat" || name.starts_with("meerkat-") {
            collect_rust_source_paths(&path.join("src"), &mut paths)?;
        }
    }
    Ok(paths)
}

fn collect_rust_source_paths(dir: &Path, paths: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }

    for entry in fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = entry.with_context(|| format!("iterate {}", dir.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("read file type {}", path.display()))?;

        if file_type.is_dir() {
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if should_skip_rust_scan_dir(name) {
                continue;
            }
            collect_rust_source_paths(&path, paths)?;
            continue;
        }

        if file_type.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            paths.push(path);
        }
    }

    Ok(())
}

fn should_skip_rust_scan_dir(name: &str) -> bool {
    name.starts_with('.')
        || matches!(
            name,
            "target"
                | "bazel-bin"
                | "bazel-meerkat"
                | "bazel-out"
                | "bazel-testlogs"
                | "node_modules"
        )
}

fn relative_slash_path(root: &Path, path: &Path) -> Result<String> {
    Ok(path
        .strip_prefix(root)
        .with_context(|| format!("strip {} from {}", root.display(), path.display()))?
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/"))
}

fn is_catalog_dsl_path(path: &str) -> bool {
    path.starts_with("meerkat-machine-schema/src/catalog/dsl/")
}

fn is_expected_generated_kernel_body(path: &str, machine: &str) -> bool {
    path == format!(
        "meerkat-machine-kernels/src/generated/{}.rs",
        generated_kernel_module_slug(machine)
    )
}

fn machine_macro_names(parsed: &syn::File) -> Vec<String> {
    let mut names = Vec::new();
    let mut aliases = BTreeSet::new();
    collect_machine_macro_aliases_from_items(&parsed.items, &mut aliases);
    collect_machine_macro_names_from_items(&parsed.items, &aliases, &mut names);
    names
}

fn collect_machine_macro_names_from_items(
    items: &[syn::Item],
    aliases: &BTreeSet<String>,
    names: &mut Vec<String>,
) {
    for item in items {
        match item {
            syn::Item::Macro(item_macro)
                if macro_path_ends_with_machine(&item_macro.mac.path)
                    || macro_path_ends_with_machine_alias(&item_macro.mac.path, aliases) =>
            {
                if let Some(name) = machine_name_from_tokens(item_macro.mac.tokens.clone()) {
                    names.push(name);
                }
            }
            syn::Item::Macro(item_macro)
                if macro_path_ends_with_macro_rules(&item_macro.mac.path) =>
            {
                for name in machine_names_from_wrapper_macro_tokens(item_macro.mac.tokens.clone()) {
                    names.push(name);
                }
            }
            syn::Item::Mod(item_mod) => {
                if let Some((_, nested_items)) = &item_mod.content {
                    collect_machine_macro_names_from_items(nested_items, aliases, names);
                }
            }
            _ => {}
        }
    }
}

fn collect_machine_macro_aliases_from_items(items: &[syn::Item], aliases: &mut BTreeSet<String>) {
    for item in items {
        match item {
            syn::Item::Use(item_use) => {
                collect_machine_macro_aliases_from_use_tree(&item_use.tree, &[], aliases);
            }
            syn::Item::Mod(item_mod) => {
                if let Some((_, nested_items)) = &item_mod.content {
                    collect_machine_macro_aliases_from_items(nested_items, aliases);
                }
            }
            _ => {}
        }
    }
}

fn collect_machine_macro_aliases_from_use_tree(
    tree: &syn::UseTree,
    prefix: &[String],
    aliases: &mut BTreeSet<String>,
) {
    match tree {
        syn::UseTree::Path(path) => {
            let mut next_prefix = prefix.to_vec();
            next_prefix.push(path.ident.to_string());
            collect_machine_macro_aliases_from_use_tree(&path.tree, &next_prefix, aliases);
        }
        syn::UseTree::Name(name)
            if is_meerkat_machine_dsl_prefix(prefix) && name.ident == "machine" =>
        {
            aliases.insert(name.ident.to_string());
        }
        syn::UseTree::Rename(rename)
            if is_meerkat_machine_dsl_prefix(prefix) && rename.ident == "machine" =>
        {
            aliases.insert(rename.rename.to_string());
        }
        syn::UseTree::Group(group) => {
            for item in &group.items {
                collect_machine_macro_aliases_from_use_tree(item, prefix, aliases);
            }
        }
        _ => {}
    }
}

fn is_meerkat_machine_dsl_prefix(prefix: &[String]) -> bool {
    prefix.len() == 1 && prefix[0] == "meerkat_machine_dsl"
}

fn macro_path_ends_with_machine(path: &syn::Path) -> bool {
    path.segments
        .last()
        .is_some_and(|segment| segment.ident == "machine")
}

fn macro_path_ends_with_machine_alias(path: &syn::Path, aliases: &BTreeSet<String>) -> bool {
    path.segments
        .last()
        .is_some_and(|segment| aliases.contains(&segment.ident.to_string()))
}

fn macro_path_ends_with_macro_rules(path: &syn::Path) -> bool {
    path.segments
        .last()
        .is_some_and(|segment| segment.ident == "macro_rules")
}

fn machine_names_from_wrapper_macro_tokens(tokens: proc_macro2::TokenStream) -> Vec<String> {
    let mut names = Vec::new();
    let mut iter = tokens.into_iter().peekable();
    while let Some(token) = iter.next() {
        match token {
            TokenTree::Ident(ident) if ident == "machine" => {
                if let Some(TokenTree::Ident(machine_name)) = iter.next() {
                    names.push(machine_name.to_string());
                }
            }
            TokenTree::Group(group) => {
                names.extend(machine_names_from_wrapper_macro_tokens(group.stream()));
            }
            _ => {}
        }
    }
    names
}

fn machine_name_from_tokens(tokens: proc_macro2::TokenStream) -> Option<String> {
    let mut after_machine_keyword = false;
    for token in tokens {
        match token {
            TokenTree::Ident(ident) if after_machine_keyword => return Some(ident.to_string()),
            TokenTree::Ident(ident) if ident == "machine" => after_machine_keyword = true,
            _ => {}
        }
    }
    None
}

pub struct CanonicalRegistry {
    machines: Vec<MachineSchema>,
    compositions: Vec<CompositionSchema>,
    machine_coverages: Vec<MachineCoverageManifest>,
    composition_coverages: Vec<CompositionCoverageManifest>,
}

impl CanonicalRegistry {
    pub fn load() -> Self {
        Self {
            machines: canonical_machine_schemas(),
            compositions: canonical_composition_schemas(),
            machine_coverages: canonical_machine_coverage_manifests(),
            composition_coverages: canonical_composition_coverage_manifests(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        let by_name = self.machine_map();
        self.validate_coverages()?;

        for machine in &self.machines {
            machine
                .validate()
                .with_context(|| format!("validate machine {}", machine.machine))?;
        }

        for composition in &self.compositions {
            composition
                .validate()
                .with_context(|| format!("validate composition {}", composition.name))?;

            let machine_refs = composition
                .machines
                .iter()
                .map(|instance| {
                    by_name
                        .get(instance.machine_name.as_str())
                        .copied()
                        .ok_or_else(|| {
                            anyhow!("unknown machine in composition: {}", instance.machine_name)
                        })
                })
                .collect::<Result<Vec<_>>>()?;

            composition
                .validate_against(&machine_refs)
                .with_context(|| format!("cross-validate composition {}", composition.name))?;
        }

        Ok(())
    }

    fn validate_coverages(&self) -> Result<()> {
        let machine_names = self
            .machines
            .iter()
            .map(|schema| schema.machine.as_str())
            .collect::<BTreeSet<_>>();

        for schema in &self.machines {
            let manifest = self
                .machine_coverages
                .iter()
                .find(|item| item.machine == schema.machine)
                .ok_or_else(|| {
                    anyhow!("missing machine coverage manifest for {}", schema.machine)
                })?;
            if manifest.code_anchors.is_empty() {
                bail!(
                    "machine coverage manifest {} has no code anchors",
                    manifest.machine
                );
            }
            if manifest.scenarios.is_empty() {
                bail!(
                    "machine coverage manifest {} has no scenarios",
                    manifest.machine
                );
            }
            validate_machine_semantic_coverage(schema, manifest)?;
        }

        for manifest in &self.machine_coverages {
            if !machine_names.contains(manifest.machine.as_str()) {
                bail!(
                    "machine coverage manifest {} does not match a canonical machine",
                    manifest.machine
                );
            }
        }

        let composition_names = self
            .compositions
            .iter()
            .map(|schema| schema.name.as_str())
            .collect::<BTreeSet<_>>();

        for schema in &self.compositions {
            let manifest = self
                .composition_coverages
                .iter()
                .find(|item| item.composition == schema.name)
                .ok_or_else(|| {
                    anyhow!("missing composition coverage manifest for {}", schema.name)
                })?;
            if manifest.code_anchors.is_empty() {
                bail!(
                    "composition coverage manifest {} has no code anchors",
                    manifest.composition
                );
            }
            if manifest.scenarios.is_empty() {
                bail!(
                    "composition coverage manifest {} has no scenarios",
                    manifest.composition
                );
            }
            validate_composition_semantic_coverage(schema, manifest)?;
        }

        for manifest in &self.composition_coverages {
            if !composition_names.contains(manifest.composition.as_str()) {
                bail!(
                    "composition coverage manifest {} does not match a canonical composition",
                    manifest.composition
                );
            }
        }

        Ok(())
    }

    fn machine_map(&self) -> BTreeMap<&str, &MachineSchema> {
        self.machines
            .iter()
            .map(|schema| (schema.machine.as_str(), schema))
            .collect()
    }

    pub fn select(&self, args: &SelectionArgs) -> Result<Selection> {
        if !args.all && args.machines.is_empty() && args.compositions.is_empty() {
            bail!("select --all or provide at least one --machine/--composition");
        }

        let machine_entries = self
            .machines
            .iter()
            .map(|schema| {
                let coverage = self
                    .machine_coverages
                    .iter()
                    .find(|item| item.machine == schema.machine)
                    .ok_or_else(|| {
                        anyhow!("missing validated machine coverage for {}", schema.machine)
                    })?
                    .clone();
                Ok(MachineEntry {
                    slug: machine_slug(&schema.machine),
                    schema: schema.clone(),
                    coverage,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        let composition_entries = self
            .compositions
            .iter()
            .map(|schema| {
                let coverage = self
                    .composition_coverages
                    .iter()
                    .find(|item| item.composition == schema.name)
                    .ok_or_else(|| {
                        anyhow!("missing validated composition coverage for {}", schema.name)
                    })?
                    .clone();
                Ok(CompositionEntry {
                    slug: composition_slug(&schema.name),
                    schema: schema.clone(),
                    coverage,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        let machines = if args.all {
            machine_entries
        } else {
            select_machines(&machine_entries, &args.machines)?
        };

        let compositions = if args.all {
            composition_entries
        } else {
            select_compositions(&composition_entries, &args.compositions)?
        };

        Ok(Selection {
            machines,
            compositions,
        })
    }
}

fn validate_machine_semantic_coverage(
    schema: &MachineSchema,
    manifest: &MachineCoverageManifest,
) -> Result<()> {
    let anchor_ids = manifest
        .code_anchors
        .iter()
        .map(|anchor| anchor.id.as_str())
        .collect::<BTreeSet<_>>();
    let scenario_ids = manifest
        .scenarios
        .iter()
        .map(|scenario| scenario.id.as_str())
        .collect::<BTreeSet<_>>();

    validate_semantic_entries(
        &format!("machine {}", schema.machine),
        "transition",
        &schema
            .transitions
            .iter()
            .map(|transition| transition.name.as_str().to_owned())
            .collect::<Vec<_>>(),
        &manifest.transition_coverage,
        &anchor_ids,
        &scenario_ids,
    )?;
    validate_semantic_entries(
        &format!("machine {}", schema.machine),
        "effect",
        &schema
            .effects
            .variants
            .iter()
            .map(|variant| variant.name.as_str().to_owned())
            .collect::<Vec<_>>(),
        &manifest.effect_coverage,
        &anchor_ids,
        &scenario_ids,
    )?;
    validate_semantic_entries(
        &format!("machine {}", schema.machine),
        "invariant",
        &schema
            .invariants
            .iter()
            .map(|invariant| invariant.name.as_str().to_owned())
            .collect::<Vec<_>>(),
        &manifest.invariant_coverage,
        &anchor_ids,
        &scenario_ids,
    )?;

    Ok(())
}

fn validate_composition_semantic_coverage(
    schema: &CompositionSchema,
    manifest: &CompositionCoverageManifest,
) -> Result<()> {
    let anchor_ids = manifest
        .code_anchors
        .iter()
        .map(|anchor| anchor.id.as_str())
        .collect::<BTreeSet<_>>();
    let scenario_ids = manifest
        .scenarios
        .iter()
        .map(|scenario| scenario.id.as_str())
        .collect::<BTreeSet<_>>();

    validate_semantic_entries(
        &format!("composition {}", schema.name),
        "route",
        &schema
            .routes
            .iter()
            .map(|route| route.name.as_str().to_owned())
            .collect::<Vec<_>>(),
        &manifest.route_coverage,
        &anchor_ids,
        &scenario_ids,
    )?;
    validate_semantic_entries(
        &format!("composition {}", schema.name),
        "scheduler rule",
        &schema
            .scheduler_rules
            .iter()
            .map(scheduler_rule_name)
            .collect::<Vec<_>>(),
        &manifest.scheduler_rule_coverage,
        &anchor_ids,
        &scenario_ids,
    )?;
    validate_semantic_entries(
        &format!("composition {}", schema.name),
        "invariant",
        &schema
            .invariants
            .iter()
            .map(|invariant| invariant.name.clone())
            .collect::<Vec<_>>(),
        &manifest.invariant_coverage,
        &anchor_ids,
        &scenario_ids,
    )?;

    Ok(())
}

fn validate_semantic_entries(
    owner: &str,
    item_kind: &str,
    expected_names: &[String],
    entries: &[SemanticCoverageEntry],
    anchor_ids: &BTreeSet<&str>,
    scenario_ids: &BTreeSet<&str>,
) -> Result<()> {
    let expected = expected_names
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let seen = entries
        .iter()
        .map(|entry| entry.name.as_str())
        .collect::<BTreeSet<_>>();

    for name in &expected {
        if !seen.contains(name) {
            bail!("{owner} missing semantic coverage entry for {item_kind} `{name}`");
        }
    }

    for entry in entries {
        if !expected.contains(entry.name.as_str()) {
            bail!(
                "{owner} semantic coverage entry `{}` does not match a declared {item_kind}",
                entry.name
            );
        }
        if entry.anchor_ids.is_empty() {
            bail!(
                "{owner} semantic coverage entry `{}` has no code-anchor mappings",
                entry.name
            );
        }
        if entry.scenario_ids.is_empty() {
            bail!(
                "{owner} semantic coverage entry `{}` has no scenario mappings",
                entry.name
            );
        }
        if anchor_ids.len() > 1
            && scenario_ids.len() > 1
            && entry.anchor_ids.len() == anchor_ids.len()
            && entry.scenario_ids.len() == scenario_ids.len()
        {
            bail!(
                "{owner} semantic coverage entry `{}` maps to every code anchor and every scenario; coverage must be semantic, not tautological",
                entry.name
            );
        }

        for anchor_id in &entry.anchor_ids {
            if !anchor_ids.contains(anchor_id.as_str()) {
                bail!(
                    "{owner} semantic coverage entry `{}` references unknown code anchor `{anchor_id}`",
                    entry.name
                );
            }
        }

        for scenario_id in &entry.scenario_ids {
            if !scenario_ids.contains(scenario_id.as_str()) {
                bail!(
                    "{owner} semantic coverage entry `{}` references unknown scenario `{scenario_id}`",
                    entry.name
                );
            }
        }
    }

    Ok(())
}

fn scheduler_rule_name(rule: &SchedulerRule) -> String {
    match rule {
        SchedulerRule::PreemptWhenReady { higher, lower } => {
            format!("PreemptWhenReady({higher}, {lower})")
        }
    }
}

pub struct Selection {
    pub machines: Vec<MachineEntry>,
    pub compositions: Vec<CompositionEntry>,
}

#[derive(Clone)]
pub struct MachineEntry {
    pub slug: String,
    pub schema: MachineSchema,
    pub coverage: MachineCoverageManifest,
}

#[derive(Clone)]
pub struct CompositionEntry {
    pub slug: String,
    pub schema: CompositionSchema,
    pub coverage: CompositionCoverageManifest,
}

fn select_machines(entries: &[MachineEntry], requested: &[String]) -> Result<Vec<MachineEntry>> {
    if requested.is_empty() {
        return Ok(Vec::new());
    }

    requested
        .iter()
        .map(|wanted| {
            entries
                .iter()
                .find(|entry| {
                    entry.schema.machine.as_str() == wanted.as_str()
                        || entry.slug == *wanted
                        || legacy_machine_slug(&entry.schema.machine) == Some(wanted.as_str())
                        || entry.schema.machine.as_str().strip_suffix("Machine")
                            == Some(wanted.as_str())
                })
                .cloned()
                .ok_or_else(|| anyhow!("unknown machine selection `{wanted}`"))
        })
        .collect()
}

fn select_compositions(
    entries: &[CompositionEntry],
    requested: &[String],
) -> Result<Vec<CompositionEntry>> {
    if requested.is_empty() {
        return Ok(Vec::new());
    }

    requested
        .iter()
        .map(|wanted| {
            entries
                .iter()
                .find(|entry| {
                    entry.schema.name.as_str() == wanted.as_str() || entry.slug == *wanted
                })
                .cloned()
                .ok_or_else(|| anyhow!("unknown composition selection `{wanted}`"))
        })
        .collect()
}

#[derive(Debug, Default, Clone)]
pub struct TlcCoverageSummary {
    counts_by_operator: BTreeMap<String, TlcCoverageCounts>,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct TlcCoverageCounts {
    pub truth_hits: u64,
    pub evaluations: u64,
}

pub fn merge_tlc_coverage(target: &mut TlcCoverageSummary, other: Option<&TlcCoverageSummary>) {
    let Some(other) = other else {
        return;
    };

    for (operator, counts) in &other.counts_by_operator {
        target
            .counts_by_operator
            .entry(operator.clone())
            .and_modify(|existing| {
                existing.truth_hits = existing.truth_hits.max(counts.truth_hits);
                existing.evaluations = existing.evaluations.max(counts.evaluations);
            })
            .or_insert(*counts);
    }
}

fn maybe_run_tlc_in_dir(
    dir: &Path,
    slug: &str,
    profile: VerifyProfile,
    workers: usize,
) -> Result<Option<TlcCoverageSummary>> {
    let config_name = match profile {
        VerifyProfile::Ci => "ci.cfg",
        VerifyProfile::Deep => "deep.cfg",
    };
    maybe_run_tlc_in_dir_with_config(dir, slug, config_name, profile, workers)
}

fn maybe_run_tlc_in_dir_with_config(
    dir: &Path,
    slug: &str,
    config_name: &str,
    profile: VerifyProfile,
    workers: usize,
) -> Result<Option<TlcCoverageSummary>> {
    let model = dir.join("model.tla");
    let config = dir.join(config_name);

    if !model.exists() || !config.exists() {
        bail!(
            "missing checked-in model/config for {slug} at {} / {}",
            model.display(),
            config.display()
        );
    }

    if which::which("tlc").is_err() {
        bail!("tlc not on PATH; machine-verify requires the TLC CLI");
    }

    let root = repo_root()?;
    let metadir = verification_metadir(slug, profile)?;
    fs::create_dir_all(&metadir)
        .with_context(|| format!("create TLC metadir {}", metadir.display()))?;

    let mut cmd = Command::new("tlc");
    cmd.arg("-workers")
        .arg(workers.to_string())
        .args(match profile {
            VerifyProfile::Ci => Vec::new(),
            VerifyProfile::Deep => vec!["-coverage".to_string(), "1".to_string()],
        })
        .arg("-metadir")
        .arg(&metadir)
        .arg("-config")
        .arg(&config)
        .arg(&model)
        .current_dir(&root)
        .env("JAVA_TOOL_OPTIONS", merged_java_tool_options());

    let output = cmd
        .output()
        .with_context(|| format!("run tlc for {slug}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    print!("{stdout}");
    eprint!("{stderr}");

    let combined = format!("{stdout}\n{stderr}");

    if let Err(err) = fs::remove_dir_all(&metadir) {
        eprintln!(
            "warning: failed to remove TLC metadir {}: {err:#}",
            metadir.display()
        );
    }

    if !tlc_run_succeeded(&output.status) {
        bail!("tlc failed for {slug} ({config_name})");
    }

    let coverage = if matches!(profile, VerifyProfile::Deep) {
        Some(parse_tlc_coverage(&combined))
    } else {
        None
    };

    Ok(coverage)
}

pub fn tlc_run_succeeded(status: &ExitStatus) -> bool {
    status.success()
}

pub fn ensure_machine_transition_coverage(
    schema: &MachineSchema,
    coverage: &TlcCoverageSummary,
) -> Result<()> {
    let zero_hit = schema
        .transitions
        .iter()
        .filter_map(|transition| {
            let evaluations = coverage
                .counts_by_operator
                .get(transition.name.as_str())
                .map(|counts| counts.evaluations)
                .unwrap_or(0);
            (evaluations == 0).then(|| transition.name.as_str().to_owned())
        })
        .collect::<Vec<_>>();

    if zero_hit.is_empty() {
        return Ok(());
    }

    bail!(
        "deep TLC coverage for {} left zero-hit transitions:\n{}",
        schema.machine,
        zero_hit
            .into_iter()
            .map(|name| format!("- {name}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

pub fn ensure_composition_coverage(
    schema: &CompositionSchema,
    coverage: &TlcCoverageSummary,
    witness_covered_routes: &BTreeSet<String>,
    witness_covered_scheduler_rules: &BTreeSet<String>,
) -> Result<()> {
    let zero_hit_routes = schema
        .routes
        .iter()
        .filter_map(|route| {
            let operator = composition_route_coverage_operator_name(&route.name);
            let evaluations = coverage
                .counts_by_operator
                .get(&operator)
                .map(|counts| counts.evaluations)
                .unwrap_or(0);
            (evaluations == 0 && !witness_covered_routes.contains(route.name.as_str()))
                .then(|| route.name.as_str().to_owned())
        })
        .collect::<Vec<_>>();

    if !zero_hit_routes.is_empty() {
        bail!(
            "deep TLC coverage for composition {} left zero-hit routes:\n{}",
            schema.name,
            zero_hit_routes
                .into_iter()
                .map(|name| format!("- {name}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    let zero_hit_scheduler_rules = schema
        .scheduler_rules
        .iter()
        .filter_map(|rule| {
            let operator = composition_scheduler_coverage_operator_name(rule);
            let evaluations = coverage
                .counts_by_operator
                .get(&operator)
                .map(|counts| counts.evaluations)
                .unwrap_or(0);
            (evaluations == 0 && !witness_covered_scheduler_rules.contains(&operator))
                .then(|| format!("{rule:?}"))
        })
        .collect::<Vec<_>>();

    if zero_hit_scheduler_rules.is_empty() {
        Ok(())
    } else {
        bail!(
            "deep TLC coverage for composition {} left zero-hit scheduler rules:\n{}",
            schema.name,
            zero_hit_scheduler_rules
                .into_iter()
                .map(|rule| format!("- {rule}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    }
}

pub fn parse_tlc_coverage(output: &str) -> TlcCoverageSummary {
    let mut summary = TlcCoverageSummary::default();

    for line in output.lines() {
        if let Some((operator, counts)) = parse_tlc_coverage_line(line) {
            summary
                .counts_by_operator
                .entry(operator)
                .and_modify(|existing| {
                    existing.truth_hits = existing.truth_hits.max(counts.truth_hits);
                    existing.evaluations = existing.evaluations.max(counts.evaluations);
                })
                .or_insert(counts);
        }
    }

    summary
}

pub fn parse_tlc_coverage_line(line: &str) -> Option<(String, TlcCoverageCounts)> {
    let line = line.trim();
    if !line.starts_with('<') {
        return None;
    }

    let name_end = line.find(" line ")?;
    let operator = line.get(1..name_end)?.trim().to_string();
    let counts = line.split(">: ").nth(1)?;
    let mut parts = counts.split(':');
    let truth_hits = parts.next()?.trim().parse::<u64>().ok()?;
    let evaluations = parts.next()?.trim().parse::<u64>().ok()?;
    Some((
        operator,
        TlcCoverageCounts {
            truth_hits,
            evaluations,
        },
    ))
}

fn run_generated_kernel_tests(root: &Path) -> Result<()> {
    let mut cmd = repo_cargo_command(root);
    cmd.arg("test")
        .arg("-p")
        .arg("meerkat-machine-kernels")
        .arg("--lib")
        .arg("--")
        .arg("--test-threads=1")
        .current_dir(root);

    let status = cmd.status().context("run generated machine kernel tests")?;
    if !status.success() {
        bail!("generated machine kernel tests failed");
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OwnerTestSpec {
    package: &'static str,
    target: &'static str,
    filter: &'static str,
}

fn owner_test_specs_for_machine(slug: &str) -> &'static [OwnerTestSpec] {
    const MEERKAT: &[OwnerTestSpec] = &[
        OwnerTestSpec {
            package: "meerkat-integration-tests",
            target: "peer_directory_reachability_kernel",
            filter: "peer_directory_reachability_kernel_initializes_with_typed_signal",
        },
        OwnerTestSpec {
            package: "meerkat-integration-tests",
            target: "peer_directory_reachability_kernel",
            filter: "peer_directory_reachability_kernel_fields_removed_from_state",
        },
        OwnerTestSpec {
            package: "meerkat-integration-tests",
            target: "session_turn_admission_kernel",
            filter: "session_turn_admission_kernel_attached_state_reached",
        },
        OwnerTestSpec {
            package: "meerkat-integration-tests",
            target: "session_turn_admission_kernel",
            filter: "session_turn_admission_kernel_interrupt_allowed_while_attached",
        },
        OwnerTestSpec {
            package: "meerkat-integration-tests",
            target: "session_tool_visibility_kernel",
            filter: "session_tool_visibility_kernel_publishes_committed_set_from_attached",
        },
        OwnerTestSpec {
            package: "meerkat-integration-tests",
            target: "session_tool_visibility_kernel",
            filter: "session_tool_visibility_kernel_stages_deferred_requests_without_touching_active_state",
        },
    ];
    const MOB: &[OwnerTestSpec] = &[OwnerTestSpec {
        package: "meerkat-mob",
        target: "lib",
        filter: "runtime::tests::test_cancel_fallback_uses_direct_pending_to_terminal_cas_attempts",
    }];

    match slug {
        "meerkat_machine" => MEERKAT,
        "mob_machine" => MOB,
        _ => &[],
    }
}

fn run_machine_owner_tests(root: &Path, machine: &MachineEntry) -> Result<()> {
    for spec in owner_test_specs_for_machine(&machine.slug) {
        println!(
            "owner-test: {} -> {}::{}/{}",
            machine.schema.machine, spec.package, spec.target, spec.filter
        );
        let mut cmd = repo_cargo_command(root);
        cmd.arg("test").arg("-p").arg(spec.package).arg(spec.filter);
        if spec.target == "lib" {
            cmd.arg("--lib");
        } else {
            cmd.arg("--test").arg(spec.target);
        }
        cmd.arg("--")
            .arg("--exact")
            .arg("--test-threads=1")
            .current_dir(root);

        let status = cmd.status().with_context(|| {
            format!(
                "run owner test {}::{}/{}",
                spec.package, spec.target, spec.filter
            )
        })?;
        if !status.success() {
            bail!(
                "owner test failed for {}: {}::{}/{}",
                machine.schema.machine,
                spec.package,
                spec.target,
                spec.filter
            );
        }
    }

    Ok(())
}

fn repo_cargo_command(root: &Path) -> Command {
    Command::new(root.join("scripts/repo-cargo"))
}

fn verification_metadir(slug: &str, profile: VerifyProfile) -> Result<PathBuf> {
    let epoch_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let metadir = env::temp_dir().join("meerkat-machine-verify").join(format!(
        "{slug}-{}-{}-{epoch_ms}",
        verify_profile_name(profile),
        std::process::id()
    ));
    if metadir.exists() {
        fs::remove_dir_all(&metadir)
            .with_context(|| format!("remove stale TLC metadir {}", metadir.display()))?;
    }
    Ok(metadir)
}

fn resolve_tlc_workers(explicit: Option<usize>) -> Result<usize> {
    if let Some(workers) = explicit {
        return Ok(workers.max(1));
    }

    if let Ok(value) = env::var("TLC_WORKERS") {
        let parsed = value
            .parse::<usize>()
            .with_context(|| format!("parse TLC_WORKERS={value}"))?;
        return Ok(parsed.max(1));
    }

    Ok(std::thread::available_parallelism()
        .map(std::num::NonZero::get)
        .unwrap_or(1)
        .max(1))
}

fn verify_profile_name(profile: VerifyProfile) -> &'static str {
    match profile {
        VerifyProfile::Ci => "ci",
        VerifyProfile::Deep => "deep",
    }
}

fn merged_java_tool_options() -> String {
    let throughput_gc = "-XX:+UseParallelGC";
    let stack_size = "-Xss16m";
    let existing = env::var("JAVA_TOOL_OPTIONS").unwrap_or_default();
    let mut flags = existing
        .split_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>();

    if !flags.iter().any(|flag| flag == throughput_gc) {
        flags.insert(0, throughput_gc.into());
    }
    if !flags.iter().any(|flag| flag.starts_with("-Xss")) {
        flags.insert(0, stack_size.into());
    }

    flags.join(" ")
}

fn write_generated(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create output dir {}", parent.display()))?;
    }
    let contents = normalize_generated_contents(path, contents)?;
    fs::write(path, contents).with_context(|| format!("write {}", path.display()))
}

fn compare_generated(path: &Path, expected: &str, mismatches: &mut Vec<String>) -> Result<()> {
    let expected = normalize_generated_contents(path, expected)?;
    match fs::read_to_string(path) {
        Ok(actual) if actual == expected => Ok(()),
        Ok(_) => {
            mismatches.push(format!("stale generated artifact {}", path.display()));
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            mismatches.push(format!("missing generated artifact {}", path.display()));
            Ok(())
        }
        Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }
}

fn normalize_generated_contents(path: &Path, contents: &str) -> Result<String> {
    if path.extension().is_some_and(|ext| ext == "rs") {
        rustfmt_source(contents)
    } else {
        Ok(contents.to_owned())
    }
}

fn rustfmt_source(source: &str) -> Result<String> {
    let rustfmt = std::env::var_os("RUSTFMT").unwrap_or_else(|| "rustfmt".into());
    let mut child = Command::new(rustfmt)
        .args(["--edition", "2024", "--emit", "stdout"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawn rustfmt for machine codegen")?;

    {
        let stdin = child
            .stdin
            .as_mut()
            .context("open rustfmt stdin for machine codegen")?;
        stdin
            .write_all(source.as_bytes())
            .context("write generated machine source to rustfmt")?;
    }

    let output = child
        .wait_with_output()
        .context("wait for rustfmt during machine codegen")?;
    if !output.status.success() {
        bail!(
            "rustfmt failed for generated machine code: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    String::from_utf8(output.stdout).context("decode rustfmt output as utf-8")
}

fn collect_legacy_authority_mismatch(path: &Path, mismatches: &mut Vec<String>) {
    if path.exists() {
        mismatches.push(format!(
            "legacy parallel authority artifact must be removed {}",
            path.display()
        ));
    }
}

fn remove_legacy_authority_path(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_file(path)
            .with_context(|| format!("remove legacy authority artifact {}", path.display()))?;
    }
    if let Some(parent) = path.parent() {
        remove_dir_if_empty(parent)?;
    }
    Ok(())
}

fn remove_dir_if_empty(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    if fs::read_dir(path)
        .with_context(|| format!("read {}", path.display()))?
        .next()
        .is_none()
    {
        fs::remove_dir(path).with_context(|| format!("remove empty dir {}", path.display()))?;
    }
    Ok(())
}

pub fn repo_root() -> Result<PathBuf> {
    if let Some(root) = bazel_runfiles_workspace_root() {
        return Ok(root);
    }
    if let Some(root) = std::env::var_os("MEERKAT_MACHINE_ROOT") {
        return Ok(PathBuf::from(root));
    }

    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow!("failed to resolve repo root from xtask manifest dir"))
}

fn bazel_runfiles_workspace_root() -> Option<PathBuf> {
    let workspace = std::env::var("TEST_WORKSPACE").ok()?;
    for base in [
        std::env::var_os("TEST_SRCDIR"),
        std::env::var_os("RUNFILES_DIR"),
    ] {
        let Some(base) = base else {
            continue;
        };
        let candidate = PathBuf::from(base).join(&workspace);
        if candidate.join("Cargo.toml").exists() {
            return Some(candidate);
        }
    }
    None
}

fn machine_authority_path(root: &Path, slug: &str) -> PathBuf {
    root.join("specs")
        .join("machines")
        .join(slug)
        .join("generated")
        .join("authority.tla")
}

fn composition_authority_path(root: &Path, slug: &str) -> PathBuf {
    root.join("specs")
        .join("compositions")
        .join(slug)
        .join("generated")
        .join("authority.tla")
}

pub fn machine_model_path(root: &Path, slug: &str) -> PathBuf {
    machine_dir(root, slug).join("model.tla")
}

pub fn machine_ci_path(root: &Path, slug: &str) -> PathBuf {
    machine_dir(root, slug).join("ci.cfg")
}

pub fn machine_deep_path(root: &Path, slug: &str) -> PathBuf {
    machine_dir(root, slug).join("deep.cfg")
}

pub fn machine_contract_path(root: &Path, slug: &str) -> PathBuf {
    machine_dir(root, slug).join("contract.md")
}

fn machine_dir(root: &Path, slug: &str) -> PathBuf {
    root.join("specs").join("machines").join(slug)
}

pub fn composition_model_path(root: &Path, slug: &str) -> PathBuf {
    composition_dir(root, slug).join("model.tla")
}

pub fn composition_ci_path(root: &Path, slug: &str) -> PathBuf {
    composition_dir(root, slug).join("ci.cfg")
}

pub fn composition_deep_path(root: &Path, slug: &str) -> PathBuf {
    composition_dir(root, slug).join("deep.cfg")
}

pub fn composition_witness_path(root: &Path, slug: &str, witness: &str) -> PathBuf {
    composition_dir(root, slug).join(composition_witness_cfg_name(witness))
}

pub fn composition_contract_path(root: &Path, slug: &str) -> PathBuf {
    composition_dir(root, slug).join("contract.md")
}

fn composition_driver_path(root: &Path, schema: &CompositionSchema) -> Result<PathBuf> {
    let driver = schema.driver.as_ref().ok_or_else(|| {
        anyhow!(
            "composition {} has no generated driver binding",
            schema.name
        )
    })?;
    Ok(root.join(&driver.rust.module_path))
}

fn composition_dir(root: &Path, slug: &str) -> PathBuf {
    root.join("specs").join("compositions").join(slug)
}

pub fn machine_mapping_path(root: &Path, slug: &str) -> PathBuf {
    root.join("specs")
        .join("machines")
        .join(slug)
        .join("mapping.md")
}

fn generated_kernel_root(root: &Path) -> PathBuf {
    root.join("meerkat-machine-kernels")
        .join("src")
        .join("generated")
}

pub fn generated_kernel_module_path(root: &Path, slug: &str) -> PathBuf {
    generated_kernel_root(root).join(format!("{slug}.rs"))
}

pub fn generated_kernel_mod_path(root: &Path) -> PathBuf {
    generated_kernel_root(root).join("mod.rs")
}

pub fn composition_mapping_path(root: &Path, slug: &str) -> PathBuf {
    root.join("specs")
        .join("compositions")
        .join(slug)
        .join("mapping.md")
}

fn expected_mapping_document(path: &Path, title: &str, generated: &str) -> Result<String> {
    let existing = fs::read_to_string(path).ok();
    Ok(merge_mapping_document(
        existing.as_deref(),
        title,
        generated,
    ))
}

fn generated_kernel_export_schemas(registry: &CanonicalRegistry) -> Vec<MachineSchema> {
    registry.machines.clone()
}

fn compat_generated_kernel_schemas() -> Vec<MachineSchema> {
    Vec::new()
}

fn expected_generated_kernel_modules(registry: &CanonicalRegistry) -> BTreeSet<String> {
    registry
        .machines
        .iter()
        .map(|schema| generated_kernel_module_slug(&schema.machine))
        .collect::<BTreeSet<_>>()
}

fn mob_generated_machine_module_path(root: &Path, slug: &str) -> PathBuf {
    root.join(format!("meerkat-mob/src/generated/{slug}.rs"))
}

fn prune_stale_generated_kernel_modules(root: &Path, registry: &CanonicalRegistry) -> Result<()> {
    let generated_mod = generated_kernel_mod_path(root);
    let generated_dir = generated_mod
        .parent()
        .context("generated kernel mod parent")?;
    if !generated_dir.exists() {
        return Ok(());
    }

    let expected = expected_generated_kernel_modules(registry);
    for entry in
        fs::read_dir(generated_dir).with_context(|| format!("read {}", generated_dir.display()))?
    {
        let entry = entry
            .with_context(|| format!("scan generated kernel dir {}", generated_dir.display()))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.file_name().and_then(|name| name.to_str()) == Some("mod.rs") {
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if expected.contains(stem) {
            continue;
        }
        fs::remove_file(&path)
            .with_context(|| format!("remove stale generated kernel module {}", path.display()))?;
    }

    Ok(())
}

pub fn collect_stale_cfg_mismatches(root: &Path) -> Result<Vec<String>> {
    let registry = CanonicalRegistry::load();
    let mut mismatches = Vec::new();

    let valid_machine_cfgs: BTreeSet<&str> = ["ci.cfg", "deep.cfg", "audit.cfg"]
        .iter()
        .copied()
        .collect();
    let specs_machine_root = root.join("specs/machines");
    if specs_machine_root.exists() {
        for machine in &registry.machines {
            let slug = machine_slug(&machine.machine);
            let machine_dir = specs_machine_root.join(&slug);
            if !machine_dir.exists() {
                continue;
            }
            for entry in fs::read_dir(&machine_dir)
                .with_context(|| format!("read {}", machine_dir.display()))?
            {
                let entry = entry.with_context(|| format!("iterate {}", machine_dir.display()))?;
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("cfg") {
                    continue;
                }
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if !valid_machine_cfgs.contains(name) {
                    mismatches.push(format!("stale cfg {}", path.display()));
                }
            }
        }
    }

    let specs_composition_root = root.join("specs/compositions");
    if specs_composition_root.exists() {
        for composition in &registry.compositions {
            let slug = composition_slug(&composition.name);
            let comp_dir = specs_composition_root.join(&slug);
            if !comp_dir.exists() {
                continue;
            }
            let mut valid_comp_cfgs: BTreeSet<String> = BTreeSet::new();
            valid_comp_cfgs.insert("ci.cfg".into());
            valid_comp_cfgs.insert("deep.cfg".into());
            valid_comp_cfgs.insert("audit.cfg".into());
            for witness in &composition.witnesses {
                valid_comp_cfgs.insert(composition_witness_cfg_name(&witness.name));
            }
            for entry in
                fs::read_dir(&comp_dir).with_context(|| format!("read {}", comp_dir.display()))?
            {
                let entry = entry.with_context(|| format!("iterate {}", comp_dir.display()))?;
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("cfg") {
                    continue;
                }
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if !valid_comp_cfgs.contains(name) {
                    mismatches.push(format!("stale cfg {}", path.display()));
                }
            }
        }
    }

    Ok(mismatches)
}

#[derive(Debug)]
struct HopcroftTarget<'a> {
    kind: &'static str,
    display_name: &'a str,
    slug: &'a str,
    dir: &'a Path,
    machine_schema: Option<&'a MachineSchema>,
}

#[derive(Debug, Serialize)]
struct HopcroftRunSummary {
    observation: HopcroftObservation,
    profile: String,
    items: Vec<HopcroftSummary>,
}

#[derive(Debug, Serialize)]
struct HopcroftSummary {
    kind: String,
    name: String,
    slug: String,
    observation: HopcroftObservation,
    reachable_states: usize,
    edge_count: usize,
    quotient_states: usize,
    reduction_states: usize,
    reduction_percent: f64,
    initial_blocks: Vec<usize>,
    mixed_phase_blocks: Vec<HopcroftBlockSummary>,
    mixed_phase_pairs: Vec<HopcroftPhasePairSummary>,
    field_audit: Option<HopcroftFieldAuditSummary>,
    largest_mixed_phase_block_field_projection: Option<HopcroftBlockFieldProjectionSummary>,
    largest_blocks: Vec<HopcroftBlockSummary>,
    tlc: TlcGraphStats,
}

#[derive(Debug, Serialize)]
struct HopcroftBlockSummary {
    block: usize,
    size: usize,
    phases: BTreeMap<String, usize>,
    representative_state_id: String,
    representative_phase: Option<String>,
    outgoing: BTreeMap<String, Vec<usize>>,
}

#[derive(Debug, Serialize)]
struct HopcroftPhasePairSummary {
    left_phase: String,
    right_phase: String,
    blocks: Vec<usize>,
    total_block_members: usize,
    field_difference_counts: Vec<HopcroftFieldDifferenceCount>,
    schema_input_counts: HopcroftSchemaInputCounts,
    schema_input_rows: Vec<HopcroftSchemaInputRow>,
    sample_block_witnesses: Vec<HopcroftStatePairWitness>,
}

#[derive(Debug, Serialize)]
struct HopcroftFieldAuditSummary {
    field_count: usize,
    fields: Vec<HopcroftFieldImpactSummary>,
}

#[derive(Debug, Serialize)]
struct HopcroftFieldImpactSummary {
    field: String,
    only_quotient_states: usize,
    only_reduction_states: usize,
    only_reduction_percent: f64,
    all_except_quotient_states: usize,
    all_except_reduction_states: usize,
    all_except_reduction_percent: f64,
    all_except_collapsed_states: usize,
}

#[derive(Debug, Serialize)]
struct HopcroftBlockFieldProjectionSummary {
    block: usize,
    size: usize,
    phases: BTreeMap<String, usize>,
    field_count: usize,
    distinct_tuples: usize,
    phase_overlay_tuple_count: usize,
    max_phases_per_tuple: usize,
    fields: Vec<HopcroftBlockFieldProjectionFieldSummary>,
}

#[derive(Debug, Serialize)]
struct HopcroftBlockFieldProjectionFieldSummary {
    field: String,
    distinct_values: usize,
    largest_bucket_size: usize,
    largest_bucket_percent: f64,
    omitted_value_count: usize,
    top_values: Vec<HopcroftBlockFieldValueSummary>,
}

#[derive(Debug, Serialize)]
struct HopcroftBlockFieldValueSummary {
    value: String,
    count: usize,
    phases: BTreeMap<String, usize>,
}

#[derive(Debug, Serialize)]
struct HopcroftFieldDifferenceCount {
    field: String,
    differing_blocks: usize,
    equal_blocks: usize,
}

#[derive(Debug, Default, Serialize)]
struct HopcroftSchemaInputCounts {
    same_surface: usize,
    different_surface: usize,
    left_only: usize,
    right_only: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum HopcroftSchemaInputClassification {
    SameSurface,
    DifferentSurface,
    LeftOnly,
    RightOnly,
}

#[derive(Debug, Serialize)]
struct HopcroftSchemaInputRow {
    input_variant: String,
    classification: HopcroftSchemaInputClassification,
    left: Vec<HopcroftSchemaTransitionSummary>,
    right: Vec<HopcroftSchemaTransitionSummary>,
}

#[derive(Debug, Serialize)]
struct HopcroftSchemaTransitionSummary {
    transition: String,
    to_phase: String,
    binding_names: Vec<String>,
    guard_names: Vec<String>,
    update_count: usize,
    effect_variants: Vec<String>,
}

#[derive(Debug, Serialize)]
struct HopcroftStatePairWitness {
    block: usize,
    left_state_id: String,
    right_state_id: String,
    shared_input_witnesses: BTreeMap<String, Vec<String>>,
    differing_fields: BTreeMap<String, HopcroftFieldPairValues>,
}

#[derive(Debug, Serialize)]
struct HopcroftFieldPairValues {
    left_value: String,
    right_value: String,
}

#[derive(Debug, Default, Serialize)]
struct TlcGraphStats {
    generated_states: Option<u64>,
    distinct_states: Option<u64>,
    depth: Option<u64>,
}

#[derive(Debug)]
struct TlcDotGraph {
    states: Vec<TlcDotState>,
    edges: Vec<TlcDotEdge>,
    outgoing: Vec<Vec<usize>>,
    initial_states: Vec<usize>,
}

#[derive(Debug)]
struct TlcDotState {
    id: String,
    phase: Option<String>,
    snapshot_fields: BTreeMap<String, String>,
    is_initial: bool,
}

#[derive(Debug)]
struct TlcDotEdge {
    to: usize,
    label: String,
}

#[derive(Debug)]
struct TlcDotDump {
    dot: String,
    tlc_output: String,
}

#[derive(Debug)]
struct HopcroftQuotient {
    partition_by_state: Vec<usize>,
    blocks: Vec<Vec<usize>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum HopcroftObservationSpec {
    Builtin(HopcroftObservation),
    Fields(BTreeSet<String>),
    AllExceptFields(BTreeSet<String>),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct HopcroftStateSignature {
    observation: String,
    outgoing: Vec<(String, Vec<usize>)>,
}

#[derive(Debug, Default)]
struct HopcroftPhasePairAccumulator {
    blocks: BTreeSet<usize>,
    total_block_members: usize,
    witness_pairs: Vec<HopcroftWitnessPair>,
}

#[derive(Debug, Clone, Copy)]
struct HopcroftWitnessPair {
    block: usize,
    left_state_idx: usize,
    right_state_idx: usize,
}

#[allow(clippy::too_many_arguments)]
fn run_hopcroft_for_target(
    root: &Path,
    target: HopcroftTarget<'_>,
    profile: VerifyProfile,
    workers: usize,
    observation: HopcroftObservation,
    audit_map: bool,
    reuse_existing_dump: bool,
    artifact_dir: Option<&Path>,
) -> Result<HopcroftSummary> {
    let artifact_dir = artifact_dir.map(|path| {
        fs::create_dir_all(path).with_context(|| format!("create {}", path.display()))?;
        Ok::<&Path, anyhow::Error>(path)
    });
    let artifact_dir = artifact_dir.transpose()?;

    let dump = dump_tlc_dot_for_target(
        root,
        &target,
        profile,
        workers,
        artifact_dir,
        reuse_existing_dump,
    )?;
    let graph = parse_tlc_dot_graph(&dump.dot)
        .with_context(|| format!("parse TLC DOT dump for {}", target.display_name))?;
    let quotient = hopcroft_partition_refinement(&graph, observation);
    let summary = summarize_hopcroft_target(
        target,
        observation,
        audit_map,
        &graph,
        &quotient,
        &dump.tlc_output,
    );

    print_hopcroft_summary(&summary);

    if let Some(artifact_dir) = artifact_dir {
        let summary_path = artifact_dir.join("summary.json");
        fs::write(
            &summary_path,
            serde_json::to_vec_pretty(&summary).context("serialize hopcroft item summary")?,
        )
        .with_context(|| format!("write {}", summary_path.display()))?;
        println!("  wrote {}", summary_path.display());
    }

    Ok(summary)
}

fn dump_tlc_dot_for_target(
    root: &Path,
    target: &HopcroftTarget<'_>,
    profile: VerifyProfile,
    workers: usize,
    artifact_dir: Option<&Path>,
    reuse_existing_dump: bool,
) -> Result<TlcDotDump> {
    let config_name = match profile {
        VerifyProfile::Ci => "ci.cfg",
        VerifyProfile::Deep => "deep.cfg",
    };
    let model = target.dir.join("model.tla");
    let config = target.dir.join(config_name);
    if !model.exists() || !config.exists() {
        bail!(
            "missing checked-in model/config for {} at {} / {}",
            target.display_name,
            model.display(),
            config.display()
        );
    }

    let metadir = verification_metadir(&format!("{}-hopcroft", target.slug), profile)?;
    fs::create_dir_all(&metadir)
        .with_context(|| format!("create TLC metadir {}", metadir.display()))?;

    let owned_artifact_dir = artifact_dir.is_none().then(|| {
        env::temp_dir()
            .join("meerkat-machine-hopcroft")
            .join(format!(
                "{}-{}-{}",
                target.slug,
                verify_profile_name(profile),
                std::process::id()
            ))
    });
    let artifact_dir = match artifact_dir {
        Some(path) => path,
        None => owned_artifact_dir
            .as_deref()
            .context("expected temp hopcroft artifact dir to be created")?,
    };
    fs::create_dir_all(artifact_dir)
        .with_context(|| format!("create artifact dir {}", artifact_dir.display()))?;

    let dump_path = artifact_dir.join("graph.dot");
    let log_path = artifact_dir.join("tlc.log");

    if reuse_existing_dump {
        let dot = fs::read_to_string(&dump_path).with_context(|| {
            format!(
                "read existing TLC DOT dump for {} from {}",
                target.display_name,
                dump_path.display()
            )
        })?;
        let tlc_output = fs::read_to_string(&log_path).unwrap_or_default();
        return Ok(TlcDotDump { dot, tlc_output });
    }

    let mut cmd = Command::new("tlc");
    cmd.arg("-workers")
        .arg(workers.to_string())
        .arg("-dump")
        // Plain DOT already carries the state labels we parse for observation
        // and field-audit work. Adding `snapshot` causes the Meerkat export to
        // stall after writing a partial graph on the truthful 59k-state model.
        .arg("dot,actionlabels")
        .arg(&dump_path)
        .arg("-metadir")
        .arg(&metadir)
        .arg("-config")
        .arg(&config)
        .arg(&model)
        .current_dir(root)
        .env("JAVA_TOOL_OPTIONS", merged_java_tool_options());

    let output = cmd
        .output()
        .with_context(|| format!("run tlc hopcroft dump for {}", target.display_name))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}\n{stderr}");
    fs::write(&log_path, &combined).with_context(|| format!("write {}", log_path.display()))?;

    if let Err(err) = fs::remove_dir_all(&metadir) {
        eprintln!(
            "warning: failed to remove TLC metadir {}: {err:#}",
            metadir.display()
        );
    }

    if !tlc_run_succeeded(&output.status) {
        bail!("tlc hopcroft dump failed for {}", target.display_name);
    }

    let dot = fs::read_to_string(&dump_path)
        .with_context(|| format!("read TLC DOT dump {}", dump_path.display()))?;

    if owned_artifact_dir.is_some()
        && let Err(err) = fs::remove_dir_all(artifact_dir)
    {
        eprintln!(
            "warning: failed to clean hopcroft temp artifacts {}: {err:#}",
            artifact_dir.display()
        );
    }

    Ok(TlcDotDump {
        dot,
        tlc_output: combined,
    })
}

fn parse_tlc_dot_graph(contents: &str) -> Result<TlcDotGraph> {
    let mut nodes = BTreeMap::<String, TlcDotState>::new();
    let mut edges = Vec::<(String, String, String)>::new();

    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty()
            || line == "{"
            || line == "}"
            || line.starts_with("strict digraph")
            || line.starts_with("subgraph")
            || line.starts_with("nodesep=")
            || line.starts_with("color=")
            || line.starts_with("{rank")
        {
            continue;
        }

        if let Some((from, to, label)) = parse_tlc_dot_edge_line(line)? {
            edges.push((from, to, label));
            continue;
        }

        if let Some(state) = parse_tlc_dot_node_line(line)? {
            nodes.insert(state.id.clone(), state);
        }
    }

    if nodes.is_empty() {
        bail!("no TLC graph nodes found in DOT dump");
    }

    let mut states = nodes.into_values().collect::<Vec<_>>();
    states.sort_by(|left, right| left.id.cmp(&right.id));

    let id_to_index = states
        .iter()
        .enumerate()
        .map(|(idx, state)| (state.id.clone(), idx))
        .collect::<BTreeMap<_, _>>();

    let mut parsed_edges = Vec::new();
    let mut outgoing = vec![Vec::new(); states.len()];
    for (from, to, label) in edges {
        let Some(&from_idx) = id_to_index.get(&from) else {
            bail!("DOT edge references unknown source node {from}");
        };
        let Some(&to_idx) = id_to_index.get(&to) else {
            bail!("DOT edge references unknown target node {to}");
        };
        outgoing[from_idx].push(parsed_edges.len());
        parsed_edges.push(TlcDotEdge { to: to_idx, label });
    }

    let initial_states = states
        .iter()
        .enumerate()
        .filter_map(|(idx, state)| state.is_initial.then_some(idx))
        .collect::<Vec<_>>();

    Ok(TlcDotGraph {
        states,
        edges: parsed_edges,
        outgoing,
        initial_states,
    })
}

fn parse_tlc_dot_node_line(line: &str) -> Result<Option<TlcDotState>> {
    let Some(bracket_start) = line.find('[') else {
        return Ok(None);
    };
    let Some(bracket_end) = line.rfind(']') else {
        return Ok(None);
    };

    let id = line[..bracket_start]
        .trim()
        .trim_end_matches(';')
        .to_owned();
    if id.is_empty() {
        return Ok(None);
    }
    let attrs = &line[(bracket_start + 1)..bracket_end];
    let label = parse_tlc_dot_attribute(attrs, "label").unwrap_or_default();
    let snapshot_fields = parse_snapshot_fields(&label);
    let phase = snapshot_fields.get("phase").map(|value| {
        value
            .strip_prefix('"')
            .and_then(|trimmed| trimmed.strip_suffix('"'))
            .unwrap_or(value)
            .to_owned()
    });
    let is_initial = attrs.contains("style = filled") || attrs.contains("style=filled");

    Ok(Some(TlcDotState {
        id,
        phase,
        snapshot_fields,
        is_initial,
    }))
}

fn parse_tlc_dot_edge_line(line: &str) -> Result<Option<(String, String, String)>> {
    let Some(arrow) = line.find("->") else {
        return Ok(None);
    };
    let Some(bracket_start) = line.find('[') else {
        return Ok(None);
    };
    if arrow > bracket_start {
        return Ok(None);
    }
    let Some(bracket_end) = line.rfind(']') else {
        return Ok(None);
    };

    let from = line[..arrow].trim().trim_end_matches(';').to_owned();
    let to = line[(arrow + 2)..bracket_start]
        .trim()
        .trim_end_matches(';')
        .to_owned();
    let attrs = &line[(bracket_start + 1)..bracket_end];
    let label = parse_tlc_dot_attribute(attrs, "label").unwrap_or_else(|| "<unlabeled>".into());
    Ok(Some((from, to, label)))
}

fn parse_tlc_dot_attribute(attrs: &str, name: &str) -> Option<String> {
    let mut search_from = 0;
    while let Some(relative) = attrs[search_from..].find(name) {
        let start = search_from + relative;
        let mut cursor = start + name.len();
        while matches!(attrs.as_bytes().get(cursor), Some(b' ' | b'\t')) {
            cursor += 1;
        }
        if attrs.as_bytes().get(cursor).copied() != Some(b'=') {
            search_from = start + name.len();
            continue;
        }
        cursor += 1;
        while matches!(attrs.as_bytes().get(cursor), Some(b' ' | b'\t')) {
            cursor += 1;
        }
        if attrs.as_bytes().get(cursor).copied() != Some(b'"') {
            return None;
        }
        return parse_tlc_dot_quoted_string(&attrs[cursor..]).map(|(value, _)| value);
    }
    None
}

fn parse_tlc_dot_quoted_string(input: &str) -> Option<(String, usize)> {
    if !input.starts_with('"') {
        return None;
    }

    let mut out = String::new();
    let mut escaped = false;
    for (offset, ch) in input[1..].char_indices() {
        if escaped {
            match ch {
                'n' | 'l' => out.push('\n'),
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                other => out.push(other),
            }
            escaped = false;
            continue;
        }

        match ch {
            '\\' => escaped = true,
            '"' => return Some((out, offset + 2)),
            other => out.push(other),
        }
    }

    None
}

fn parse_snapshot_fields(label: &str) -> BTreeMap<String, String> {
    let mut fields = BTreeMap::new();
    for line in label.lines() {
        let line = line.trim_start();
        let line = line
            .strip_prefix("/\\ ")
            .or_else(|| line.strip_prefix("/\\"))
            .unwrap_or(line);
        if let Some((field, value)) = line.split_once(" = ") {
            fields.insert(field.trim().to_owned(), value.trim().to_owned());
        }
    }
    fields
}

fn hopcroft_partition_refinement(
    graph: &TlcDotGraph,
    observation: HopcroftObservation,
) -> HopcroftQuotient {
    hopcroft_partition_refinement_with_spec(graph, &HopcroftObservationSpec::Builtin(observation))
}

fn hopcroft_partition_refinement_with_spec(
    graph: &TlcDotGraph,
    observation: &HopcroftObservationSpec,
) -> HopcroftQuotient {
    let mut partition = seed_hopcroft_partition_with_spec(graph, observation);

    loop {
        let signatures = graph
            .states
            .iter()
            .enumerate()
            .map(|(state_idx, state)| {
                let mut by_label = BTreeMap::<String, BTreeSet<usize>>::new();
                for &edge_idx in &graph.outgoing[state_idx] {
                    let edge = &graph.edges[edge_idx];
                    by_label
                        .entry(edge.label.clone())
                        .or_default()
                        .insert(partition[edge.to]);
                }

                HopcroftStateSignature {
                    observation: hopcroft_observation_key_with_spec(state, observation),
                    outgoing: by_label
                        .into_iter()
                        .map(|(label, targets)| (label, targets.into_iter().collect()))
                        .collect(),
                }
            })
            .collect::<Vec<_>>();

        let mut groups = BTreeMap::<HopcroftStateSignature, Vec<usize>>::new();
        for (state_idx, signature) in signatures.into_iter().enumerate() {
            groups.entry(signature).or_default().push(state_idx);
        }

        let mut blocks = groups.into_values().collect::<Vec<_>>();
        blocks.sort_by_key(|members| members.first().copied().unwrap_or(usize::MAX));

        let mut next_partition = vec![0; graph.states.len()];
        for (block_id, members) in blocks.iter().enumerate() {
            for &state_idx in members {
                next_partition[state_idx] = block_id;
            }
        }

        if next_partition == partition {
            return HopcroftQuotient {
                partition_by_state: next_partition,
                blocks,
            };
        }

        partition = next_partition;
    }
}

fn seed_hopcroft_partition_with_spec(
    graph: &TlcDotGraph,
    observation: &HopcroftObservationSpec,
) -> Vec<usize> {
    let mut groups = BTreeMap::<String, Vec<usize>>::new();
    for (state_idx, state) in graph.states.iter().enumerate() {
        groups
            .entry(hopcroft_observation_key_with_spec(state, observation))
            .or_default()
            .push(state_idx);
    }

    let mut blocks = groups.into_values().collect::<Vec<_>>();
    blocks.sort_by_key(|members| members.first().copied().unwrap_or(usize::MAX));

    let mut partition = vec![0; graph.states.len()];
    for (block_id, members) in blocks.iter().enumerate() {
        for &state_idx in members {
            partition[state_idx] = block_id;
        }
    }
    partition
}

fn hopcroft_observation_key_with_spec(
    state: &TlcDotState,
    observation: &HopcroftObservationSpec,
) -> String {
    match observation {
        HopcroftObservationSpec::Builtin(HopcroftObservation::None) => String::new(),
        HopcroftObservationSpec::Builtin(HopcroftObservation::Phase) => {
            state.phase.clone().unwrap_or_else(|| "<unknown>".into())
        }
        HopcroftObservationSpec::Builtin(HopcroftObservation::Full) => state
            .snapshot_fields
            .iter()
            .filter(|(field, _)| field.as_str() != "model_step_count")
            .map(|(field, value)| format!("{field} = {value}"))
            .collect::<Vec<_>>()
            .join("\n"),
        HopcroftObservationSpec::Fields(fields) => state
            .snapshot_fields
            .iter()
            .filter(|(field, _)| fields.contains(*field) && is_extended_state_field(field))
            .map(|(field, value)| format!("{field} = {value}"))
            .collect::<Vec<_>>()
            .join("\n"),
        HopcroftObservationSpec::AllExceptFields(excluded) => state
            .snapshot_fields
            .iter()
            .filter(|(field, _)| {
                is_extended_state_field(field) && !excluded.contains((*field).as_str())
            })
            .map(|(field, value)| format!("{field} = {value}"))
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn is_extended_state_field(field: &str) -> bool {
    field != "phase" && field != "model_step_count"
}

fn summarize_hopcroft_target(
    target: HopcroftTarget<'_>,
    observation: HopcroftObservation,
    audit_map: bool,
    graph: &TlcDotGraph,
    quotient: &HopcroftQuotient,
    tlc_output: &str,
) -> HopcroftSummary {
    let mixed_phase_blocks = quotient
        .blocks
        .iter()
        .enumerate()
        .filter_map(|(block_id, members)| {
            let summary = summarize_hopcroft_block(block_id, members, graph, quotient);
            (summary.phases.len() > 1).then_some(summary)
        })
        .collect::<Vec<_>>();

    let mut largest_blocks = quotient
        .blocks
        .iter()
        .enumerate()
        .map(|(block_id, members)| summarize_hopcroft_block(block_id, members, graph, quotient))
        .collect::<Vec<_>>();
    largest_blocks.sort_by(|left, right| {
        right
            .size
            .cmp(&left.size)
            .then_with(|| left.block.cmp(&right.block))
    });
    largest_blocks.truncate(10);

    let initial_blocks = graph
        .initial_states
        .iter()
        .map(|state_idx| quotient.partition_by_state[*state_idx])
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();

    let reduction_states = graph.states.len().saturating_sub(quotient.blocks.len());
    let reduction_percent = if graph.states.is_empty() {
        0.0
    } else {
        ((reduction_states as f64) / (graph.states.len() as f64)) * 100.0
    };
    let mixed_phase_pairs =
        summarize_hopcroft_phase_pairs(graph, quotient, target.machine_schema, audit_map);
    let field_audit = if audit_map {
        summarize_hopcroft_field_audit(graph)
    } else {
        None
    };
    let largest_mixed_phase_block_field_projection =
        summarize_largest_mixed_phase_block_field_projection(graph, quotient, &mixed_phase_blocks);

    HopcroftSummary {
        kind: target.kind.into(),
        name: target.display_name.into(),
        slug: target.slug.into(),
        observation,
        reachable_states: graph.states.len(),
        edge_count: graph.edges.len(),
        quotient_states: quotient.blocks.len(),
        reduction_states,
        reduction_percent,
        initial_blocks,
        mixed_phase_blocks,
        mixed_phase_pairs,
        field_audit,
        largest_mixed_phase_block_field_projection,
        largest_blocks,
        tlc: parse_tlc_graph_stats(tlc_output),
    }
}

fn summarize_hopcroft_phase_pairs(
    graph: &TlcDotGraph,
    quotient: &HopcroftQuotient,
    machine_schema: Option<&MachineSchema>,
    audit_map: bool,
) -> Vec<HopcroftPhasePairSummary> {
    let mut pairs = BTreeMap::<(String, String), HopcroftPhasePairAccumulator>::new();

    for (block_id, members) in quotient.blocks.iter().enumerate() {
        let mut representatives = BTreeMap::<String, usize>::new();
        for &state_idx in members {
            if let Some(phase) = graph.states[state_idx].phase.clone() {
                representatives.entry(phase).or_insert(state_idx);
            }
        }

        let phases = representatives.keys().cloned().collect::<Vec<_>>();
        for (idx, left) in phases.iter().enumerate() {
            for right in phases.iter().skip(idx + 1) {
                let Some(&left_state_idx) = representatives.get(left) else {
                    continue;
                };
                let Some(&right_state_idx) = representatives.get(right) else {
                    continue;
                };
                let key = (left.clone(), right.clone());
                let entry = pairs.entry(key).or_default();
                entry.blocks.insert(block_id);
                entry.total_block_members += members.len();
                if audit_map {
                    entry.witness_pairs.push(HopcroftWitnessPair {
                        block: block_id,
                        left_state_idx,
                        right_state_idx,
                    });
                }
            }
        }
    }

    let transition_input_variants = machine_schema.map(transition_input_variant_map);
    let mut summaries = pairs
        .into_iter()
        .map(|((left_phase, right_phase), pair)| {
            let witness_pairs = pair.witness_pairs;
            let (field_difference_counts, sample_block_witnesses) = if audit_map {
                (
                    summarize_phase_pair_field_differences(graph, &witness_pairs),
                    summarize_phase_pair_sample_witnesses(
                        graph,
                        &witness_pairs,
                        transition_input_variants.as_ref(),
                    ),
                )
            } else {
                (Vec::new(), Vec::new())
            };
            let schema_input_rows = if audit_map {
                machine_schema
                    .map(|schema| schema_input_rows_for_pair(schema, &left_phase, &right_phase))
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            let schema_input_counts = summarize_schema_input_counts(&schema_input_rows);

            HopcroftPhasePairSummary {
                left_phase,
                right_phase,
                blocks: pair.blocks.into_iter().collect(),
                total_block_members: pair.total_block_members,
                field_difference_counts,
                schema_input_counts,
                schema_input_rows,
                sample_block_witnesses,
            }
        })
        .collect::<Vec<_>>();
    summaries.sort_by(|left, right| {
        right
            .total_block_members
            .cmp(&left.total_block_members)
            .then_with(|| left.left_phase.cmp(&right.left_phase))
            .then_with(|| left.right_phase.cmp(&right.right_phase))
    });
    summaries
}

fn summarize_hopcroft_field_audit(graph: &TlcDotGraph) -> Option<HopcroftFieldAuditSummary> {
    let fields = graph_field_names(graph);
    if fields.is_empty() {
        return None;
    }

    let mut summaries = Vec::new();
    for field in fields {
        let only_spec = HopcroftObservationSpec::Fields(BTreeSet::from([field.clone()]));
        let only_quotient = hopcroft_partition_refinement_with_spec(graph, &only_spec);
        let only_reduction_states = graph
            .states
            .len()
            .saturating_sub(only_quotient.blocks.len());
        let only_reduction_percent = reduction_percent(only_reduction_states, graph.states.len());

        let all_except_spec =
            HopcroftObservationSpec::AllExceptFields(BTreeSet::from([field.clone()]));
        let all_except_quotient = hopcroft_partition_refinement_with_spec(graph, &all_except_spec);
        let all_except_reduction_states = graph
            .states
            .len()
            .saturating_sub(all_except_quotient.blocks.len());
        let all_except_reduction_percent =
            reduction_percent(all_except_reduction_states, graph.states.len());

        summaries.push(HopcroftFieldImpactSummary {
            all_except_collapsed_states: all_except_reduction_states,
            field,
            only_quotient_states: only_quotient.blocks.len(),
            only_reduction_states,
            only_reduction_percent,
            all_except_quotient_states: all_except_quotient.blocks.len(),
            all_except_reduction_states,
            all_except_reduction_percent,
        });
    }

    summaries.sort_by(|left, right| {
        right
            .all_except_collapsed_states
            .cmp(&left.all_except_collapsed_states)
            .then_with(|| right.only_quotient_states.cmp(&left.only_quotient_states))
            .then_with(|| left.field.cmp(&right.field))
    });

    Some(HopcroftFieldAuditSummary {
        field_count: summaries.len(),
        fields: summaries,
    })
}

fn summarize_largest_mixed_phase_block_field_projection(
    graph: &TlcDotGraph,
    quotient: &HopcroftQuotient,
    mixed_phase_blocks: &[HopcroftBlockSummary],
) -> Option<HopcroftBlockFieldProjectionSummary> {
    let largest = mixed_phase_blocks.iter().max_by(|left, right| {
        left.size
            .cmp(&right.size)
            .then_with(|| right.block.cmp(&left.block))
    })?;
    let members = quotient.blocks.get(largest.block)?;
    let fields = members
        .iter()
        .flat_map(|state_idx| graph.states[*state_idx].snapshot_fields.keys().cloned())
        .filter(|field| is_extended_state_field(field))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();

    let tuple_phase_map = members
        .iter()
        .map(|state_idx| {
            let state = &graph.states[*state_idx];
            let tuple = fields
                .iter()
                .map(|field| snapshot_field_value(state, field))
                .collect::<Vec<_>>();
            let phase = state.phase.clone().unwrap_or_else(|| "<missing>".into());
            (tuple, phase)
        })
        .fold(
            BTreeMap::<Vec<String>, BTreeSet<String>>::new(),
            |mut acc, (tuple, phase)| {
                acc.entry(tuple).or_default().insert(phase);
                acc
            },
        );
    let distinct_tuples = tuple_phase_map.len();
    let phase_overlay_tuple_count = tuple_phase_map
        .values()
        .filter(|phases| phases.len() > 1)
        .count();
    let max_phases_per_tuple = tuple_phase_map
        .values()
        .map(BTreeSet::len)
        .max()
        .unwrap_or_default();

    let mut summaries = Vec::new();
    for field in &fields {
        let mut buckets = BTreeMap::<String, (usize, BTreeMap<String, usize>)>::new();
        for &state_idx in members {
            let state = &graph.states[state_idx];
            let value = snapshot_field_value(state, field);
            let entry = buckets.entry(value).or_default();
            entry.0 += 1;
            if let Some(phase) = &state.phase {
                *entry.1.entry(phase.clone()).or_default() += 1;
            }
        }

        let distinct_values = buckets.len();
        let largest_bucket_size = buckets
            .values()
            .map(|(count, _)| *count)
            .max()
            .unwrap_or_default();
        let mut top_values = buckets
            .into_iter()
            .map(|(value, (count, phases))| HopcroftBlockFieldValueSummary {
                value,
                count,
                phases,
            })
            .collect::<Vec<_>>();
        top_values.sort_by(|left, right| {
            right
                .count
                .cmp(&left.count)
                .then_with(|| left.value.cmp(&right.value))
        });
        let omitted_value_count = distinct_values.saturating_sub(top_values.len().min(8));
        top_values.truncate(8);

        summaries.push(HopcroftBlockFieldProjectionFieldSummary {
            field: field.clone(),
            distinct_values,
            largest_bucket_size,
            largest_bucket_percent: reduction_percent(largest_bucket_size, members.len()),
            omitted_value_count,
            top_values,
        });
    }

    summaries.sort_by(|left, right| {
        right
            .distinct_values
            .cmp(&left.distinct_values)
            .then_with(|| {
                left.largest_bucket_percent
                    .partial_cmp(&right.largest_bucket_percent)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| left.field.cmp(&right.field))
    });

    Some(HopcroftBlockFieldProjectionSummary {
        block: largest.block,
        size: largest.size,
        phases: largest.phases.clone(),
        field_count: fields.len(),
        distinct_tuples,
        phase_overlay_tuple_count,
        max_phases_per_tuple,
        fields: summaries,
    })
}

fn graph_field_names(graph: &TlcDotGraph) -> Vec<String> {
    graph
        .states
        .iter()
        .flat_map(|state| state.snapshot_fields.keys().cloned())
        .filter(|field| is_extended_state_field(field))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn snapshot_field_value(state: &TlcDotState, field: &str) -> String {
    state
        .snapshot_fields
        .get(field)
        .cloned()
        .unwrap_or_else(|| "<missing>".into())
}

fn reduction_percent(reduction_states: usize, reachable_states: usize) -> f64 {
    if reachable_states == 0 {
        0.0
    } else {
        ((reduction_states as f64) / (reachable_states as f64)) * 100.0
    }
}

fn summarize_phase_pair_field_differences(
    graph: &TlcDotGraph,
    witness_pairs: &[HopcroftWitnessPair],
) -> Vec<HopcroftFieldDifferenceCount> {
    let mut counts = BTreeMap::<String, (usize, usize)>::new();

    for witness in witness_pairs {
        let left = &graph.states[witness.left_state_idx];
        let right = &graph.states[witness.right_state_idx];
        let fields = left
            .snapshot_fields
            .keys()
            .chain(right.snapshot_fields.keys())
            .filter(|field| is_extended_state_field(field))
            .cloned()
            .collect::<BTreeSet<_>>();

        for field in fields {
            let left_value = left
                .snapshot_fields
                .get(&field)
                .map(String::as_str)
                .unwrap_or("<missing>");
            let right_value = right
                .snapshot_fields
                .get(&field)
                .map(String::as_str)
                .unwrap_or("<missing>");
            let entry = counts.entry(field).or_insert((0, 0));
            if left_value == right_value {
                entry.1 += 1;
            } else {
                entry.0 += 1;
            }
        }
    }

    let mut summaries = counts
        .into_iter()
        .map(
            |(field, (differing_blocks, equal_blocks))| HopcroftFieldDifferenceCount {
                field,
                differing_blocks,
                equal_blocks,
            },
        )
        .collect::<Vec<_>>();
    summaries.sort_by(|left, right| {
        right
            .differing_blocks
            .cmp(&left.differing_blocks)
            .then_with(|| right.equal_blocks.cmp(&left.equal_blocks))
            .then_with(|| left.field.cmp(&right.field))
    });
    summaries
}

fn summarize_phase_pair_sample_witnesses(
    graph: &TlcDotGraph,
    witness_pairs: &[HopcroftWitnessPair],
    transition_input_variants: Option<&BTreeMap<String, String>>,
) -> Vec<HopcroftStatePairWitness> {
    witness_pairs
        .iter()
        .take(4)
        .map(|witness| {
            let left = &graph.states[witness.left_state_idx];
            let right = &graph.states[witness.right_state_idx];
            let differing_fields = left
                .snapshot_fields
                .keys()
                .chain(right.snapshot_fields.keys())
                .filter(|field| is_extended_state_field(field))
                .cloned()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .filter_map(|field| {
                    let left_value = left
                        .snapshot_fields
                        .get(&field)
                        .cloned()
                        .unwrap_or_else(|| "<missing>".into());
                    let right_value = right
                        .snapshot_fields
                        .get(&field)
                        .cloned()
                        .unwrap_or_else(|| "<missing>".into());
                    (left_value != right_value).then_some((
                        field,
                        HopcroftFieldPairValues {
                            left_value,
                            right_value,
                        },
                    ))
                })
                .collect::<BTreeMap<_, _>>();

            HopcroftStatePairWitness {
                block: witness.block,
                left_state_id: left.id.clone(),
                right_state_id: right.id.clone(),
                shared_input_witnesses: transition_input_variants
                    .map(|index| {
                        shared_input_witnesses_for_state(graph, witness.left_state_idx, index)
                    })
                    .unwrap_or_default(),
                differing_fields,
            }
        })
        .collect()
}

fn shared_input_witnesses_for_state(
    graph: &TlcDotGraph,
    state_idx: usize,
    transition_input_variants: &BTreeMap<String, String>,
) -> BTreeMap<String, Vec<String>> {
    let mut inputs = BTreeMap::<String, BTreeSet<String>>::new();
    for &edge_idx in &graph.outgoing[state_idx] {
        let edge = &graph.edges[edge_idx];
        let Some(input_variant) = transition_input_variants.get(&edge.label) else {
            continue;
        };
        inputs
            .entry(input_variant.clone())
            .or_default()
            .insert(edge.label.clone());
    }

    inputs
        .into_iter()
        .map(|(input_variant, transitions)| (input_variant, transitions.into_iter().collect()))
        .collect()
}

fn transition_input_variant_map(schema: &MachineSchema) -> BTreeMap<String, String> {
    schema
        .transitions
        .iter()
        .filter(|transition| transition.on.kind() == TriggerKind::Input)
        .map(|transition| {
            (
                transition.name.as_str().to_owned(),
                transition.on.variant_str().to_owned(),
            )
        })
        .collect()
}

fn schema_input_rows_for_pair(
    schema: &MachineSchema,
    left_phase: &str,
    right_phase: &str,
) -> Vec<HopcroftSchemaInputRow> {
    let mut rows = Vec::new();

    for input_variant in &schema.inputs.variants {
        let left = schema_transition_summaries_for_phase_input(
            schema,
            left_phase,
            input_variant.name.as_str(),
        );
        let right = schema_transition_summaries_for_phase_input(
            schema,
            right_phase,
            input_variant.name.as_str(),
        );
        if left.is_empty() && right.is_empty() {
            continue;
        }

        rows.push(HopcroftSchemaInputRow {
            input_variant: input_variant.name.as_str().to_owned(),
            classification: classify_schema_input_row(&left, &right),
            left,
            right,
        });
    }

    rows
}

fn schema_transition_summaries_for_phase_input(
    schema: &MachineSchema,
    phase: &str,
    input_variant: &str,
) -> Vec<HopcroftSchemaTransitionSummary> {
    let mut summaries = schema
        .transitions
        .iter()
        .filter(|transition| {
            transition.on.kind() == TriggerKind::Input
                && transition.on.variant_str() == input_variant
                && transition.from.iter().any(|from| from.as_str() == phase)
        })
        .map(|transition| HopcroftSchemaTransitionSummary {
            transition: transition.name.as_str().to_owned(),
            to_phase: transition.to.as_str().to_owned(),
            binding_names: transition
                .on
                .bindings()
                .iter()
                .map(|b| b.as_str().to_owned())
                .collect(),
            guard_names: transition
                .guards
                .iter()
                .map(|guard| guard.name.as_str().to_owned())
                .collect(),
            update_count: transition.updates.len(),
            effect_variants: transition
                .emit
                .iter()
                .map(|effect| effect.variant.as_str().to_owned())
                .collect(),
        })
        .collect::<Vec<_>>();

    summaries.sort_by(|left, right| left.transition.cmp(&right.transition));
    summaries
}

fn classify_schema_input_row(
    left: &[HopcroftSchemaTransitionSummary],
    right: &[HopcroftSchemaTransitionSummary],
) -> HopcroftSchemaInputClassification {
    if left.is_empty() && !right.is_empty() {
        return HopcroftSchemaInputClassification::RightOnly;
    }
    if !left.is_empty() && right.is_empty() {
        return HopcroftSchemaInputClassification::LeftOnly;
    }

    let left_surface = left
        .iter()
        .map(|summary| {
            (
                summary.to_phase.clone(),
                summary.binding_names.clone(),
                summary.guard_names.clone(),
                summary.update_count,
                summary.effect_variants.clone(),
            )
        })
        .collect::<BTreeSet<_>>();
    let right_surface = right
        .iter()
        .map(|summary| {
            (
                summary.to_phase.clone(),
                summary.binding_names.clone(),
                summary.guard_names.clone(),
                summary.update_count,
                summary.effect_variants.clone(),
            )
        })
        .collect::<BTreeSet<_>>();

    if left_surface == right_surface {
        HopcroftSchemaInputClassification::SameSurface
    } else {
        HopcroftSchemaInputClassification::DifferentSurface
    }
}

fn summarize_schema_input_counts(rows: &[HopcroftSchemaInputRow]) -> HopcroftSchemaInputCounts {
    let mut counts = HopcroftSchemaInputCounts::default();
    for row in rows {
        match row.classification {
            HopcroftSchemaInputClassification::SameSurface => counts.same_surface += 1,
            HopcroftSchemaInputClassification::DifferentSurface => counts.different_surface += 1,
            HopcroftSchemaInputClassification::LeftOnly => counts.left_only += 1,
            HopcroftSchemaInputClassification::RightOnly => counts.right_only += 1,
        }
    }
    counts
}

fn summarize_hopcroft_block(
    block_id: usize,
    members: &[usize],
    graph: &TlcDotGraph,
    quotient: &HopcroftQuotient,
) -> HopcroftBlockSummary {
    let phases = members
        .iter()
        .filter_map(|state_idx| graph.states[*state_idx].phase.clone())
        .fold(BTreeMap::<String, usize>::new(), |mut acc, phase| {
            *acc.entry(phase).or_default() += 1;
            acc
        });

    let representative = members.first().copied().unwrap_or(0);
    let outgoing = graph.outgoing[representative]
        .iter()
        .fold(
            BTreeMap::<String, BTreeSet<usize>>::new(),
            |mut acc, edge_idx| {
                let edge = &graph.edges[*edge_idx];
                acc.entry(edge.label.clone())
                    .or_default()
                    .insert(quotient.partition_by_state[edge.to]);
                acc
            },
        )
        .into_iter()
        .map(|(label, targets)| (label, targets.into_iter().collect()))
        .collect::<BTreeMap<_, _>>();

    HopcroftBlockSummary {
        block: block_id,
        size: members.len(),
        phases,
        representative_state_id: graph.states[representative].id.clone(),
        representative_phase: graph.states[representative].phase.clone(),
        outgoing,
    }
}

fn parse_tlc_graph_stats(output: &str) -> TlcGraphStats {
    let mut stats = TlcGraphStats::default();

    for line in output.lines() {
        let line = line.trim();
        if line.contains("states generated") && line.contains("distinct states found") {
            let fragments = line.split(',').collect::<Vec<_>>();
            if let Some(generated) = fragments
                .first()
                .and_then(|fragment| fragment.split_whitespace().next())
                .and_then(|value| value.parse::<u64>().ok())
            {
                stats.generated_states = Some(generated);
            }
            if let Some(distinct) = fragments
                .get(1)
                .and_then(|fragment| fragment.split_whitespace().next())
                .and_then(|value| value.parse::<u64>().ok())
            {
                stats.distinct_states = Some(distinct);
            }
        }

        if let Some(prefix) = line.strip_prefix("The depth of the complete state graph search is ")
        {
            let depth = prefix.trim_end_matches('.').trim();
            if let Ok(parsed) = depth.parse::<u64>() {
                stats.depth = Some(parsed);
            }
        }
    }

    stats
}

fn print_hopcroft_summary(summary: &HopcroftSummary) {
    println!("{}: {}", summary.kind, summary.name);
    println!(
        "  reachable={} edges={} quotient={} reduced={} ({:.1}%)",
        summary.reachable_states,
        summary.edge_count,
        summary.quotient_states,
        summary.reduction_states,
        summary.reduction_percent
    );
    if let Some(generated) = summary.tlc.generated_states {
        println!(
            "  tlc generated={} distinct={:?} depth={:?}",
            generated, summary.tlc.distinct_states, summary.tlc.depth
        );
    }
    if summary.mixed_phase_blocks.is_empty() {
        println!("  phase-mixed blocks: none");
    } else {
        println!("  phase-mixed blocks:");
        for block in summary.mixed_phase_blocks.iter().take(8) {
            println!(
                "    - block {} size {} phases {:?}",
                block.block, block.size, block.phases
            );
        }
        println!("  mixed-phase pairs:");
        for pair in summary.mixed_phase_pairs.iter().take(8) {
            println!(
                "    - {} <-> {} across blocks {:?} ({} states, schema same={} diff={} left={} right={})",
                pair.left_phase,
                pair.right_phase,
                pair.blocks,
                pair.total_block_members,
                pair.schema_input_counts.same_surface,
                pair.schema_input_counts.different_surface,
                pair.schema_input_counts.left_only,
                pair.schema_input_counts.right_only
            );
        }
    }
    if let Some(field_audit) = &summary.field_audit {
        println!("  field audit:");
        for field in field_audit.fields.iter().take(8) {
            println!(
                "    - {}: only={} | all_except={} (collapse_without={} / {:.1}%)",
                field.field,
                field.only_quotient_states,
                field.all_except_quotient_states,
                field.all_except_collapsed_states,
                field.all_except_reduction_percent
            );
        }
    }
    if let Some(block_projection) = &summary.largest_mixed_phase_block_field_projection {
        println!(
            "  largest mixed-block field projection: block {} size {} tuples={} phase_overlay={} max_phases_per_tuple={} fields={}",
            block_projection.block,
            block_projection.size,
            block_projection.distinct_tuples,
            block_projection.phase_overlay_tuple_count,
            block_projection.max_phases_per_tuple,
            block_projection.field_count
        );
        for field in block_projection.fields.iter().take(8) {
            let top_values = field
                .top_values
                .iter()
                .take(3)
                .map(|value| {
                    format!(
                        "{} ({})",
                        summarize_snapshot_value_for_cli(&value.value),
                        value.count
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            println!(
                "    - {}: distinct={} largest_bucket={} ({:.1}%) top=[{}{}]",
                field.field,
                field.distinct_values,
                field.largest_bucket_size,
                field.largest_bucket_percent,
                top_values,
                if field.omitted_value_count == 0 {
                    String::new()
                } else {
                    format!(", +{} more", field.omitted_value_count)
                }
            );
        }
    }
    println!("  largest blocks:");
    for block in summary.largest_blocks.iter().take(5) {
        println!(
            "    - block {} size {} phase {:?}",
            block.block, block.size, block.representative_phase
        );
    }
}

fn summarize_snapshot_value_for_cli(value: &str) -> String {
    let compact = value.replace('\n', " ");
    const LIMIT: usize = 40;
    if compact.chars().count() > LIMIT {
        let truncated = compact.chars().take(LIMIT - 1).collect::<String>();
        format!("{truncated}…")
    } else {
        compact
    }
}

fn generated_kernel_module_slug(machine_name: impl AsRef<str>) -> String {
    let machine_name = machine_name.as_ref();
    match machine_name {
        "MeerkatMachine" => "meerkat".into(),
        "MobMachine" => "mob".into(),
        _ => to_snake_case(machine_name.strip_suffix("Machine").unwrap_or(machine_name)),
    }
}

pub fn machine_slug(machine_name: impl AsRef<str>) -> String {
    let machine_name = machine_name.as_ref();
    match machine_name {
        "MeerkatMachine" => return "meerkat_machine".into(),
        "MobMachine" => return "mob_machine".into(),
        _ => {}
    }
    let trimmed = machine_name.strip_suffix("Machine").unwrap_or(machine_name);
    to_snake_case(trimmed)
}

fn legacy_machine_slug(machine_name: impl AsRef<str>) -> Option<&'static str> {
    match machine_name.as_ref() {
        "MeerkatMachine" => Some("meerkat"),
        "MobMachine" => Some("mob"),
        _ => None,
    }
}

pub fn composition_slug(name: impl AsRef<str>) -> String {
    to_snake_case(name.as_ref())
}

pub fn to_snake_case(value: &str) -> String {
    let mut out = String::new();
    let mut previous_is_sep = true;

    for ch in value.chars() {
        if ch == '_' || ch == '-' || ch == ' ' {
            if !previous_is_sep {
                out.push('_');
                previous_is_sep = true;
            }
            continue;
        }

        if ch.is_ascii_uppercase() {
            if !out.is_empty() && !previous_is_sep {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
            previous_is_sep = false;
        } else {
            out.push(ch.to_ascii_lowercase());
            previous_is_sep = false;
        }
    }

    out.trim_matches('_').to_owned()
}

#[cfg(test)]
#[path = "machines_tests.rs"]
mod tests;

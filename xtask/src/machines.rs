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
use meerkat_machine_schema::catalog::dsl::mob_machine::{MobMachineInput, MobMachineInputVariant};
use meerkat_machine_schema::{
    CompositionCoverageManifest, CompositionSchema, MachineCoverageManifest, MachineSchema,
    SchedulerRule, SemanticCoverageEntry, TriggerKind, canonical_composition_coverage_manifests,
    canonical_composition_schemas, canonical_machine_coverage_manifests, canonical_machine_schemas,
};
use meerkat_mob::canonical_mob_machine_command_gate_requirements;
use meerkat_mob::runtime::flow_frame_engine::canonical_flow_frame_loop_store_plan_commit_requirements;
use proc_macro2::TokenTree;
use serde::Serialize;
use syn::parse::Parser;
use syn::visit::{self, Visit};

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
    mismatches.extend(collect_peer_response_terminal_projection_mismatches(&root)?);
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
    mismatches.extend(collect_peer_response_terminal_projection_mismatches(root)?);
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

pub fn collect_peer_response_terminal_projection_mismatches(root: &Path) -> Result<Vec<String>> {
    let mut mismatches = Vec::new();
    let boundary_files = [
        "meerkat-runtime/src/accept.rs",
        "meerkat-runtime/src/input.rs",
        "meerkat-runtime/src/runtime_loop.rs",
    ];
    let banned = [
        (
            "PeerConversationProjection::ResponseTerminal",
            "direct terminal projection construction",
        ),
        (
            "[SYSTEM NOTICE][PEER_RESPONSE_TERMINAL]",
            "handwritten terminal render text",
        ),
        (
            "peer_response_terminal_context_key(",
            "handwritten terminal context key projection",
        ),
    ];

    for rel in boundary_files {
        let path = root.join(rel);
        if !path.exists() {
            continue;
        }
        let contents = fs::read_to_string(&path).with_context(|| {
            format!(
                "read peer-response terminal boundary file {}",
                path.display()
            )
        })?;
        let production = contents
            .split("\n#[cfg(test)]")
            .next()
            .unwrap_or(contents.as_str());

        for (line_idx, line) in production.lines().enumerate() {
            for (token, reason) in banned {
                if line.contains(token) {
                    mismatches.push(format!(
                        "{rel}:{}: {reason} `{token}` must route through typed core peer-response terminal facts",
                        line_idx + 1
                    ));
                }
            }
        }
    }

    let core_projection_owner = root.join("meerkat-core/src/handles.rs");
    if core_projection_owner.exists() {
        let rel = "meerkat-core/src/handles.rs";
        let contents = fs::read_to_string(&core_projection_owner).with_context(|| {
            format!(
                "read peer-response terminal core projection owner {}",
                core_projection_owner.display()
            )
        })?;
        let production = contents
            .split("\n#[cfg(test)]")
            .next()
            .unwrap_or(contents.as_str());
        let core_banned = [
            (
                "transport_identity: Option<String>",
                "transport identity string bus",
            ),
            ("route_identity: String", "route identity string bus"),
            ("display_identity: String", "display identity string bus"),
            ("correlation_id: String", "correlation id string bus"),
            (
                "peer_response_terminal_context_key(&self.source.route_identity, &self.correlation_id)",
                "untyped terminal context key projection",
            ),
        ];

        for (line_idx, line) in production.lines().enumerate() {
            for (token, reason) in core_banned {
                if line.contains(token) {
                    mismatches.push(format!(
                        "{rel}:{}: {reason} `{token}` must use distinct typed peer-response terminal facts",
                        line_idx + 1
                    ));
                }
            }
        }
    }

    let shell_boundary_files = [
        "meerkat-contracts/src/wire/runtime.rs",
        "meerkat-rest/src/lib.rs",
        "meerkat-rpc/src/handlers/event.rs",
        "meerkat-rpc/src/session_runtime.rs",
    ];
    let shell_banned = [
        (
            "pub peer_name: PeerName",
            "terminal shell peer_name identity bus",
        ),
        (
            "peer_name: meerkat_core::comms::PeerName",
            "terminal shell peer_name identity bus",
        ),
        (
            "PeerResponseTerminalRouteIdentity::parse(peer_name",
            "terminal route identity projected from peer_name",
        ),
        (
            "PeerResponseTerminalDisplayIdentity::parse(peer_name",
            "terminal display identity projected from peer_name",
        ),
        (
            "peer_response_terminal_input(&peer_name",
            "terminal input projected from peer_name",
        ),
    ];

    for rel in shell_boundary_files {
        let path = root.join(rel);
        if !path.exists() {
            continue;
        }
        let contents = fs::read_to_string(&path).with_context(|| {
            format!(
                "read peer-response terminal shell boundary file {}",
                path.display()
            )
        })?;
        let production = contents
            .split("\n#[cfg(test)]")
            .next()
            .unwrap_or(contents.as_str());

        for (line_idx, line) in production.lines().enumerate() {
            for (token, reason) in shell_banned {
                if line.contains(token) {
                    mismatches.push(format!(
                        "{rel}:{}: {reason} `{token}` must transport typed route/display/correlation facts",
                        line_idx + 1
                    ));
                }
            }
        }
    }

    Ok(mismatches)
}

pub fn collect_direct_flow_reducer_transition_mismatches(root: &Path) -> Result<Vec<String>> {
    let mut mismatches = Vec::new();
    let facts = FlowProjectionGovernanceFacts::load(root)?;

    for path in production_rust_source_paths(root)? {
        let rel = relative_slash_path(root, &path)?;
        if rel.ends_with("/tests.rs") {
            continue;
        }
        let contents = fs::read_to_string(&path).with_context(|| {
            format!("read flow reducer transition candidate {}", path.display())
        })?;
        let parsed = syn::parse_file(&contents).with_context(|| {
            format!("parse flow reducer transition candidate {}", path.display())
        })?;
        let aliases = RustUseAliases::from_file(&parsed);
        let mut visitor = FlowGovernanceVisitor::new(&rel, &aliases, &facts);
        visitor.visit_file(&parsed);
        mismatches.extend(visitor.mismatches);
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
    let parsed = syn::parse_file(&contents)
        .with_context(|| format!("parse Mob runtime actor {}", actor_path.display()))?;
    let rel = relative_slash_path(root, &actor_path)?;
    let actor = MobRuntimeActorAst::from_file(&parsed);
    let mut mismatches = Vec::new();

    for requirement in canonical_mob_machine_command_gate_requirements() {
        let command = requirement.command.as_str();
        let input = requirement.input.input_variant();
        if !actor.command_fail_closes_on(command, input) {
            let input = requirement.input.as_str();
            mismatches.push(format!(
                "MobCommand::{command} is catalog-classified but does not fail-close on MobMachineInput::{input} in {rel}",
            ));
        }
    }

    Ok(mismatches)
}

#[derive(Debug, Clone, Default)]
struct RustUseAliases {
    aliases: BTreeMap<String, Vec<Vec<String>>>,
    shadowed_roots: BTreeSet<String>,
    ambiguous_glob_import: bool,
}

impl RustUseAliases {
    fn from_file(file: &syn::File) -> Self {
        let mut aliases = Self::default();
        aliases.collect_from_items(&file.items);
        aliases
    }

    fn collect_from_items(&mut self, items: &[syn::Item]) {
        for item in items {
            match item {
                syn::Item::Use(item_use) => self.collect_use_tree(Vec::new(), &item_use.tree),
                _ => self.collect_item_shadow(item),
            }
        }
    }

    fn with_item_aliases(&self, items: &[syn::Item]) -> Self {
        let mut aliases = self.clone();
        aliases.collect_from_items(items);
        aliases
    }

    fn with_block_aliases(&self, block: &syn::Block) -> Self {
        let mut aliases = self.clone();
        let mut collector = BlockUseAliasCollector {
            aliases: &mut aliases,
        };
        collector.visit_block(block);
        aliases
    }

    fn collect_use_tree(&mut self, mut prefix: Vec<String>, tree: &syn::UseTree) {
        match tree {
            syn::UseTree::Path(path) => {
                prefix.push(path.ident.to_string());
                self.collect_use_tree(prefix, &path.tree);
            }
            syn::UseTree::Name(name) => {
                let mut full_path = prefix;
                full_path.push(name.ident.to_string());
                self.aliases.insert(name.ident.to_string(), vec![full_path]);
            }
            syn::UseTree::Rename(rename) => {
                let mut full_path = prefix;
                full_path.push(rename.ident.to_string());
                self.aliases
                    .insert(rename.rename.to_string(), vec![full_path]);
            }
            syn::UseTree::Glob(_) => {
                if let Some(family) = FlowReducerFamily::from_canonical_module_path(&prefix) {
                    self.aliases.insert(
                        "transition".to_string(),
                        vec![vec![family.module().to_string(), "transition".to_string()]],
                    );
                    self.aliases.insert(
                        "Input".to_string(),
                        vec![vec![family.module().to_string(), "Input".to_string()]],
                    );
                } else if !glob_prefix_is_known_canonical_import_surface(&prefix) {
                    self.ambiguous_glob_import = true;
                }
            }
            syn::UseTree::Group(group) => {
                for tree in &group.items {
                    self.collect_use_tree(prefix.clone(), tree);
                }
            }
        }
    }

    fn collect_item_shadow(&mut self, item: &syn::Item) {
        let ident = match item {
            syn::Item::Const(item) => Some(&item.ident),
            syn::Item::Enum(item) => Some(&item.ident),
            syn::Item::Mod(item)
                if item.content.is_some()
                    || FlowReducerFamily::from_module(&item.ident.to_string()).is_none() =>
            {
                Some(&item.ident)
            }
            syn::Item::Mod(_) => None,
            syn::Item::Static(item) => Some(&item.ident),
            syn::Item::Struct(item) => Some(&item.ident),
            syn::Item::Trait(item) => Some(&item.ident),
            syn::Item::TraitAlias(item) => Some(&item.ident),
            syn::Item::Type(item) => Some(&item.ident),
            syn::Item::Union(item) => Some(&item.ident),
            _ => None,
        };
        if let Some(ident) = ident {
            self.shadowed_roots.insert(ident.to_string());
        }
    }

    fn path_has_shadowed_bare_root(&self, path: &syn::Path) -> bool {
        path.leading_colon.is_none()
            && path
                .segments
                .first()
                .is_some_and(|segment| self.shadowed_roots.contains(&segment.ident.to_string()))
    }

    fn path_has_unresolved_bare_root(&self, path: &syn::Path, root: &str) -> bool {
        path.leading_colon.is_none()
            && path
                .segments
                .first()
                .is_some_and(|segment| segment.ident == root)
            && !self.aliases.contains_key(root)
    }

    fn path_is_unresolved_bare_ident(&self, path: &syn::Path, ident: &str) -> bool {
        path.leading_colon.is_none()
            && path.segments.len() == 1
            && path
                .segments
                .first()
                .is_some_and(|segment| segment.ident == ident)
            && !self.aliases.contains_key(ident)
    }

    fn resolve_path(&self, path: &syn::Path) -> Vec<Vec<String>> {
        self.resolve_segments(path_segments(path), &mut BTreeSet::new())
    }

    fn resolve_segments(
        &self,
        segments: Vec<String>,
        seen: &mut BTreeSet<String>,
    ) -> Vec<Vec<String>> {
        if let Some(first) = segments.first()
            && let Some(alias_paths) = self.aliases.get(first)
            && seen.insert(first.clone())
        {
            let resolved_paths = alias_paths
                .iter()
                .flat_map(|alias_path| {
                    let mut resolved = alias_path.clone();
                    resolved.extend(segments.iter().skip(1).cloned());
                    self.resolve_segments(resolved, seen)
                })
                .collect::<Vec<_>>();
            seen.remove(first);
            if !resolved_paths.is_empty() {
                return resolved_paths;
            }
        }
        vec![segments]
    }
}

fn glob_prefix_is_known_canonical_import_surface(prefix: &[String]) -> bool {
    prefix.iter().map(String::as_str).eq(["super"])
        || prefix.iter().map(String::as_str).eq(["crate", "run"])
        || prefix.iter().map(String::as_str).eq(["crate", "runtime"])
        || prefix
            .iter()
            .map(String::as_str)
            .eq(["crate", "machines", "mob_machine", "MobCommand"])
        || prefix.iter().map(String::as_str).eq([
            "crate",
            "machines",
            "mob_machine",
            "MobMachineInput",
        ])
        || prefix
            .iter()
            .map(String::as_str)
            .eq(["super", "state", "MobCommand"])
}

struct BlockUseAliasCollector<'a> {
    aliases: &'a mut RustUseAliases,
}

impl<'ast> Visit<'ast> for BlockUseAliasCollector<'_> {
    fn visit_block(&mut self, node: &'ast syn::Block) {
        for stmt in &node.stmts {
            if let syn::Stmt::Item(item) = stmt {
                match item {
                    syn::Item::Use(item_use) => self.visit_item_use(item_use),
                    _ => self.aliases.collect_item_shadow(item),
                }
            }
        }
    }

    fn visit_item_use(&mut self, node: &'ast syn::ItemUse) {
        self.aliases.collect_use_tree(Vec::new(), &node.tree);
    }

    fn visit_item_mod(&mut self, _node: &'ast syn::ItemMod) {}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
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

    fn from_any_path_segments(segments: &[String]) -> Option<Self> {
        segments
            .iter()
            .rev()
            .find_map(|segment| Self::from_module(segment))
    }

    fn from_canonical_module_path(segments: &[String]) -> Option<Self> {
        if segments.len() == 1 {
            return segments
                .first()
                .and_then(|segment| Self::from_module(segment));
        }
        if segments.len() == 3
            && segments.first().is_some_and(|segment| segment == "crate")
            && segments.get(1).is_some_and(|segment| segment == "run")
        {
            return segments
                .get(2)
                .and_then(|segment| Self::from_module(segment));
        }
        None
    }

    fn from_transition_path(segments: &[String]) -> Option<(Self, bool)> {
        if segments
            .last()
            .is_none_or(|segment| segment != "transition")
        {
            return None;
        }
        let family = Self::from_any_path_segments(segments)?;
        let canonical =
            Self::from_canonical_module_path(&segments[..segments.len().saturating_sub(1)])
                .is_some_and(|canonical_family| canonical_family == family);
        Some((family, canonical))
    }

    fn from_input_path(segments: &[String]) -> Option<(Self, bool)> {
        for input_index in 1..segments.len() {
            if segments[input_index] == "Input"
                && let Some(family) = Self::from_module(&segments[input_index - 1])
            {
                let canonical = Self::from_canonical_module_path(&segments[..input_index])
                    .is_some_and(|canonical_family| canonical_family == family);
                return Some((family, canonical));
            }
        }
        None
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
}

#[derive(Debug, Clone)]
struct FlowProjectionGovernanceFacts {
    flow_state_fields: BTreeSet<String>,
    frame_state_fields: BTreeSet<String>,
    cas_methods: BTreeMap<String, ProjectionCasMethodFacts>,
    store_plan_variant_fields: BTreeMap<String, BTreeSet<String>>,
    store_plan_variant_commit_methods: BTreeMap<String, BTreeSet<String>>,
}

#[derive(Debug, Clone, Default)]
struct ProjectionCasMethodFacts {
    next_flow_state_arg_indices: BTreeSet<usize>,
    plan_state_arg_indices: BTreeSet<usize>,
    authority_input_arg_indices: BTreeSet<usize>,
    authority_identity_arg_indices: BTreeSet<usize>,
    plan_owned_arg_indices: BTreeSet<usize>,
    arg_names: BTreeMap<usize, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct SourcePosition {
    line: usize,
    column: usize,
}

#[derive(Debug, Clone)]
struct PreparedCommitFact {
    position: SourcePosition,
    guard_var: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct ActorPlanArmFacts {
    variants: BTreeSet<String>,
    bindings: BTreeSet<String>,
    field_bindings: BTreeMap<String, BTreeSet<String>>,
}

#[derive(Debug, Clone, Copy, Default)]
struct ActorPlanResultProof {
    authorized_commit_seen: bool,
    unauthorized_true_possible: bool,
}

impl ActorPlanResultProof {
    fn authorized() -> Self {
        Self {
            authorized_commit_seen: true,
            unauthorized_true_possible: false,
        }
    }

    fn static_false() -> Self {
        Self {
            authorized_commit_seen: false,
            unauthorized_true_possible: false,
        }
    }

    fn unauthorized() -> Self {
        Self {
            authorized_commit_seen: false,
            unauthorized_true_possible: true,
        }
    }

    fn combine(self, other: Self) -> Self {
        Self {
            authorized_commit_seen: self.authorized_commit_seen || other.authorized_commit_seen,
            unauthorized_true_possible: self.unauthorized_true_possible
                || other.unauthorized_true_possible,
        }
    }

    fn proves_authorized_commit(self) -> bool {
        self.authorized_commit_seen && !self.unauthorized_true_possible
    }
}

impl FlowProjectionGovernanceFacts {
    fn load(root: &Path) -> Result<Self> {
        let owner_root = if root.join("meerkat-mob/src/run/flow_run.rs").exists() {
            root.to_path_buf()
        } else {
            repo_root()?
        };

        let flow_state_fields =
            state_struct_fields(&owner_root.join("meerkat-mob/src/run/flow_run.rs"))?;
        let frame_state_fields =
            state_struct_fields(&owner_root.join("meerkat-mob/src/run/flow_frame.rs"))?;
        let cas_methods =
            mob_run_store_cas_methods(&owner_root.join("meerkat-mob/src/store/mod.rs"))?;
        let store_plan_variant_fields = flow_frame_loop_store_plan_variant_fields(
            &owner_root.join("meerkat-mob/src/runtime/flow_frame_engine.rs"),
        )?;
        let store_plan_variant_commit_methods =
            canonical_flow_frame_loop_store_plan_commit_requirements()
                .iter()
                .map(|requirement| {
                    (
                        requirement.variant.as_str().to_string(),
                        requirement
                            .operation
                            .store_methods()
                            .into_iter()
                            .map(ToOwned::to_owned)
                            .collect::<BTreeSet<_>>(),
                    )
                })
                .collect();

        Ok(Self {
            flow_state_fields,
            frame_state_fields,
            cas_methods,
            store_plan_variant_fields,
            store_plan_variant_commit_methods,
        })
    }
}

#[derive(Debug, Clone, Default)]
struct FlowFunctionContext {
    name: String,
    impl_type: Option<String>,
    impl_type_path: Option<Vec<String>>,
    module_depth: usize,
    return_path: Option<Vec<String>>,
    parameter_type_idents: BTreeSet<String>,
    parameter_names_by_type: BTreeMap<String, BTreeSet<String>>,
    authority_requirements: BTreeMap<FlowReducerFamily, BTreeMap<String, SourcePosition>>,
    calls: BTreeSet<String>,
    plan_freshness_check_position: Option<SourcePosition>,
    prepared_plan_input_vars: BTreeMap<String, SourcePosition>,
    committed_prepared_input_vars: BTreeMap<String, PreparedCommitFact>,
}

impl FlowFunctionContext {
    fn from_signature(
        sig: &syn::Signature,
        block: &syn::Block,
        impl_type_path: Option<Vec<String>>,
        module_depth: usize,
        rel: &str,
        aliases: &RustUseAliases,
    ) -> Self {
        let impl_type = impl_type_path
            .as_ref()
            .and_then(|path| path.last())
            .cloned();
        let mut context = Self {
            name: sig.ident.to_string(),
            impl_type,
            impl_type_path,
            module_depth,
            return_path: signature_return_path(sig),
            ..Self::default()
        };
        for input in &sig.inputs {
            if let syn::FnArg::Typed(pat_type) = input {
                if type_is_exact_canonical_mob_run_store_receiver(&pat_type.ty, aliases)
                    && let Some(name) = pat_ident_name(&pat_type.pat)
                {
                    context
                        .parameter_type_idents
                        .insert("MobRunStore".to_string());
                    context
                        .parameter_names_by_type
                        .entry("MobRunStore".to_string())
                        .or_default()
                        .insert(name);
                }
                if let Some(type_ident) =
                    parameter_type_key(&pat_type.ty, aliases, rel, module_depth)
                {
                    context.parameter_type_idents.insert(type_ident.clone());
                    if let Some(name) = pat_ident_name(&pat_type.pat) {
                        context
                            .parameter_names_by_type
                            .entry(type_ident)
                            .or_default()
                            .insert(name);
                    }
                }
            }
        }
        let authority_parameter_names = context
            .parameter_names_by_type
            .get("MobMachineFlowAuthorityToken")
            .cloned()
            .unwrap_or_default();
        let plan_parameter_names = context
            .parameter_names_by_type
            .get("FlowFrameLoopStorePlan")
            .cloned()
            .unwrap_or_default();
        let run_id_parameter_names = context
            .parameter_names_by_type
            .get("RunId")
            .cloned()
            .unwrap_or_default();
        let mut command_parameter_names = BTreeMap::new();
        for family in [
            FlowReducerFamily::FlowRun,
            FlowReducerFamily::FlowFrame,
            FlowReducerFamily::LoopIteration,
        ] {
            if let Some(names) = context.parameter_names_by_type.get(family.command_type()) {
                command_parameter_names.insert(family, names.clone());
            }
        }
        let mut facts = FlowBodyFactCollector::new(
            aliases,
            authority_parameter_names,
            command_parameter_names,
            run_id_parameter_names,
            plan_parameter_names,
        );
        facts.visit_block(block);
        context.authority_requirements = facts.authority_requirements;
        context.calls = facts.calls;
        context.plan_freshness_check_position = facts.plan_freshness_check_position;
        context.prepared_plan_input_vars = facts.prepared_plan_input_vars;
        context.committed_prepared_input_vars = facts.committed_prepared_input_vars;
        context
    }
}

struct FlowBodyFactCollector<'a> {
    aliases: &'a RustUseAliases,
    authority_parameter_names: BTreeSet<String>,
    command_parameter_names: BTreeMap<FlowReducerFamily, BTreeSet<String>>,
    run_id_parameter_names: BTreeSet<String>,
    plan_parameter_names: BTreeSet<String>,
    tracked_parameter_names: BTreeSet<String>,
    shadowed_name_scope_stack: Vec<BTreeSet<String>>,
    commit_guard_stack: Vec<Option<String>>,
    invalidated_plan_parameter_names: BTreeSet<String>,
    plan_match_result_vars: BTreeMap<String, SourcePosition>,
    fail_closed_depth: usize,
    conditional_depth: usize,
    authority_requirements: BTreeMap<FlowReducerFamily, BTreeMap<String, SourcePosition>>,
    calls: BTreeSet<String>,
    plan_freshness_check_position: Option<SourcePosition>,
    prepared_plan_input_vars: BTreeMap<String, SourcePosition>,
    committed_prepared_input_vars: BTreeMap<String, PreparedCommitFact>,
}

fn restore_shadowed_block_facts<T: Clone>(
    facts: &mut BTreeMap<String, T>,
    before: &BTreeMap<String, T>,
    block_bound_names: &BTreeSet<String>,
) {
    for name in block_bound_names {
        if let Some(previous) = before.get(name) {
            facts.insert(name.clone(), previous.clone());
        } else {
            facts.remove(name);
        }
    }
}

fn restore_shadowed_commit_facts(
    facts: &mut BTreeMap<String, PreparedCommitFact>,
    before: &BTreeMap<String, PreparedCommitFact>,
    block_bound_names: &BTreeSet<String>,
) {
    let mut affected = block_bound_names.clone();
    affected.extend(
        facts
            .iter()
            .filter(|(_, fact)| {
                fact.guard_var
                    .as_ref()
                    .is_some_and(|guard| block_bound_names.contains(guard))
            })
            .map(|(name, _)| name.clone()),
    );
    for name in affected {
        if let Some(previous) = before.get(&name) {
            facts.insert(name, previous.clone());
        } else {
            facts.remove(&name);
        }
    }
}

impl<'a> FlowBodyFactCollector<'a> {
    fn new(
        aliases: &'a RustUseAliases,
        authority_parameter_names: BTreeSet<String>,
        command_parameter_names: BTreeMap<FlowReducerFamily, BTreeSet<String>>,
        run_id_parameter_names: BTreeSet<String>,
        plan_parameter_names: BTreeSet<String>,
    ) -> Self {
        let tracked_parameter_names = authority_parameter_names
            .iter()
            .chain(command_parameter_names.values().flatten())
            .chain(&run_id_parameter_names)
            .chain(&plan_parameter_names)
            .cloned()
            .collect();
        Self {
            aliases,
            authority_parameter_names,
            command_parameter_names,
            run_id_parameter_names,
            plan_parameter_names,
            tracked_parameter_names,
            shadowed_name_scope_stack: Vec::new(),
            commit_guard_stack: Vec::new(),
            invalidated_plan_parameter_names: BTreeSet::new(),
            plan_match_result_vars: BTreeMap::new(),
            fail_closed_depth: 0,
            conditional_depth: 0,
            authority_requirements: BTreeMap::new(),
            calls: BTreeSet::new(),
            plan_freshness_check_position: None,
            prepared_plan_input_vars: BTreeMap::new(),
            committed_prepared_input_vars: BTreeMap::new(),
        }
    }

    fn with_fail_closed<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        self.fail_closed_depth += 1;
        let result = f(self);
        self.fail_closed_depth -= 1;
        result
    }

    fn with_conditional<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        self.conditional_depth += 1;
        let result = f(self);
        self.conditional_depth -= 1;
        result
    }

    fn name_is_shadowed(&self, name: &str) -> bool {
        self.shadowed_name_scope_stack
            .iter()
            .rev()
            .any(|scope| scope.contains(name))
    }

    fn visible_parameter_name(&self, name: &str) -> bool {
        self.tracked_parameter_names.contains(name) && !self.name_is_shadowed(name)
    }

    fn visible_command_parameter_names(&self, family: FlowReducerFamily) -> BTreeSet<String> {
        self.command_parameter_names
            .get(&family)
            .into_iter()
            .flat_map(|names| names.iter())
            .filter(|name| self.visible_parameter_name(name))
            .cloned()
            .collect()
    }

    fn visible_plan_parameter_names(&self) -> BTreeSet<String> {
        let names = self
            .plan_parameter_names
            .iter()
            .filter(|name| {
                self.visible_parameter_name(name)
                    && !self.invalidated_plan_parameter_names.contains(*name)
            })
            .cloned()
            .collect::<BTreeSet<_>>();
        if names.len() == 1 {
            names
        } else {
            BTreeSet::new()
        }
    }

    fn visible_run_id_parameter_names(&self) -> BTreeSet<String> {
        let names = self
            .run_id_parameter_names
            .iter()
            .filter(|name| self.visible_parameter_name(name))
            .cloned()
            .collect::<BTreeSet<_>>();
        if names.len() == 1 {
            names
        } else {
            BTreeSet::new()
        }
    }

    fn expr_is_authority_parameter(&self, expr: &syn::Expr) -> bool {
        expr_path_single_ident(expr).as_ref().is_some_and(|ident| {
            self.authority_parameter_names.contains(ident) && self.visible_parameter_name(ident)
        })
    }

    fn expr_is_plan_scrutinee(&self, expr: &syn::Expr) -> bool {
        expr_is_plan_scrutinee(expr, &self.visible_plan_parameter_names())
    }

    fn expr_is_plan_machine_inputs_prepare_call(&self, expr: &syn::Expr) -> bool {
        expr_is_plan_machine_inputs_prepare_call(expr, &self.visible_plan_parameter_names())
    }

    fn expr_is_plan_freshness_failure_guard(&self, expr: &syn::Expr) -> bool {
        expr_is_plan_freshness_failure_guard(
            expr,
            &self.visible_run_id_parameter_names(),
            &self.visible_plan_parameter_names(),
        )
    }

    fn authority_kind_command_parameter(
        &self,
        family: FlowReducerFamily,
        expr: &syn::Expr,
    ) -> Option<String> {
        let command_vars = self.visible_command_parameter_names(family);
        flow_authority_kind_command_parameter(expr, family, &command_vars, self.aliases)
    }

    fn bind_local_names(&mut self, pat: &syn::Pat) {
        let Some(scope) = self.shadowed_name_scope_stack.last_mut() else {
            return;
        };
        scope.extend(pat_bound_idents(pat));
    }

    fn invalidate_plan_proof_var(&mut self, name: &str) {
        if self.plan_parameter_names.contains(name) && self.visible_parameter_name(name) {
            self.invalidated_plan_parameter_names
                .insert(name.to_string());
            self.prepared_plan_input_vars.clear();
            self.plan_match_result_vars.clear();
            self.committed_prepared_input_vars.clear();
            return;
        }
        self.prepared_plan_input_vars.remove(name);
        self.plan_match_result_vars.remove(name);
        self.committed_prepared_input_vars
            .retain(|prepared, commit| {
                prepared != name && commit.guard_var.as_deref() != Some(name)
            });
    }
}

impl<'ast> Visit<'ast> for FlowBodyFactCollector<'_> {
    fn visit_block(&mut self, node: &'ast syn::Block) {
        let invalidated_plan_parameter_names_before = self.invalidated_plan_parameter_names.clone();
        let plan_match_result_vars_before = self.plan_match_result_vars.clone();
        let prepared_plan_input_vars_before = self.prepared_plan_input_vars.clone();
        let committed_prepared_input_vars_before = self.committed_prepared_input_vars.clone();
        let nested_block = !self.shadowed_name_scope_stack.is_empty();
        let mut scope = block_item_function_names(node);
        scope.retain(|name| self.tracked_parameter_names.contains(name));
        self.shadowed_name_scope_stack.push(scope);
        for stmt in &node.stmts {
            self.visit_stmt(stmt);
            if stmt_unconditionally_exits_block(stmt) {
                break;
            }
        }
        let block_bound_names = self.shadowed_name_scope_stack.pop().unwrap_or_default();
        if nested_block {
            restore_shadowed_block_facts(
                &mut self.plan_match_result_vars,
                &plan_match_result_vars_before,
                &block_bound_names,
            );
            restore_shadowed_block_facts(
                &mut self.prepared_plan_input_vars,
                &prepared_plan_input_vars_before,
                &block_bound_names,
            );
            restore_shadowed_commit_facts(
                &mut self.committed_prepared_input_vars,
                &committed_prepared_input_vars_before,
                &block_bound_names,
            );
            for name in &block_bound_names {
                if invalidated_plan_parameter_names_before.contains(name) {
                    self.invalidated_plan_parameter_names.insert(name.clone());
                } else {
                    self.invalidated_plan_parameter_names.remove(name);
                }
            }
        } else {
            self.plan_match_result_vars = plan_match_result_vars_before;
        }
    }

    fn visit_local(&mut self, node: &'ast syn::Local) {
        if let Some(init) = &node.init {
            self.visit_expr(&init.expr);
            if let Some((_, diverge)) = &init.diverge {
                self.visit_expr(diverge);
            }
        }
        let bound_names = pat_bound_idents(&node.pat);
        for name in &bound_names {
            self.invalidate_plan_proof_var(name);
        }
        if let syn::Pat::Ident(ident) = &node.pat
            && let Some(init) = &node.init
        {
            let name = ident.ident.to_string();
            if self.expr_is_plan_machine_inputs_prepare_call(&init.expr) {
                self.prepared_plan_input_vars
                    .insert(name.clone(), source_position(node));
            }
            if matches!(&*init.expr, syn::Expr::Match(expr_match) if self.expr_is_plan_scrutinee(&expr_match.expr))
            {
                self.plan_match_result_vars
                    .insert(name, source_position(node));
            }
        }
        self.bind_local_names(&node.pat);
    }

    fn visit_item_fn(&mut self, _node: &'ast syn::ItemFn) {}

    fn visit_item_impl(&mut self, _node: &'ast syn::ItemImpl) {}

    fn visit_expr_try(&mut self, node: &'ast syn::ExprTry) {
        if expr_is_ok_constructor_call(&node.expr)
            || expr_contains_fail_open_result_method(&node.expr)
        {
            self.visit_expr(&node.expr);
        } else {
            self.with_fail_closed(|this| this.visit_expr(&node.expr));
        }
    }

    fn visit_expr_closure(&mut self, node: &'ast syn::ExprClosure) {
        self.with_conditional(|this| visit::visit_expr_closure(this, node));
    }

    fn visit_expr_async(&mut self, node: &'ast syn::ExprAsync) {
        self.with_conditional(|this| visit::visit_expr_async(this, node));
    }

    fn visit_expr_loop(&mut self, node: &'ast syn::ExprLoop) {
        self.with_conditional(|this| visit::visit_expr_loop(this, node));
    }

    fn visit_expr_while(&mut self, node: &'ast syn::ExprWhile) {
        self.with_conditional(|this| visit::visit_expr_while(this, node));
    }

    fn visit_expr_for_loop(&mut self, node: &'ast syn::ExprForLoop) {
        self.with_conditional(|this| visit::visit_expr_for_loop(this, node));
    }

    fn visit_expr_if(&mut self, node: &'ast syn::ExprIf) {
        if self.conditional_depth == 0
            && node.else_branch.is_none()
            && self.expr_is_plan_freshness_failure_guard(&node.cond)
            && block_returns_ok_false(&node.then_branch)
        {
            self.plan_freshness_check_position
                .get_or_insert_with(|| source_position(node));
        }
        self.visit_expr(&node.cond);
        let commit_guard = expr_path_single_ident(&node.cond)
            .filter(|name| self.plan_match_result_vars.contains_key(name));
        self.commit_guard_stack.push(commit_guard);
        self.with_conditional(|this| this.visit_block(&node.then_branch));
        let _ = self.commit_guard_stack.pop();
        if let Some((_, else_branch)) = &node.else_branch {
            self.commit_guard_stack.push(None);
            self.with_conditional(|this| this.visit_expr(else_branch));
            let _ = self.commit_guard_stack.pop();
        }
    }

    fn visit_expr_match(&mut self, node: &'ast syn::ExprMatch) {
        self.visit_expr(&node.expr);
        for arm in &node.arms {
            if let Some((_, guard)) = &arm.guard {
                self.visit_expr(guard);
            }
            self.with_conditional(|this| this.visit_expr(&arm.body));
        }
    }

    fn visit_expr_assign(&mut self, node: &'ast syn::ExprAssign) {
        self.visit_expr(&node.right);
        if let Some(name) = expr_path_single_ident(&node.left) {
            self.invalidate_plan_proof_var(&name);
            if self.expr_is_plan_machine_inputs_prepare_call(&node.right) {
                self.prepared_plan_input_vars
                    .insert(name.clone(), source_position(node));
            }
            if matches!(&*node.right, syn::Expr::Match(expr_match) if self.expr_is_plan_scrutinee(&expr_match.expr))
            {
                self.plan_match_result_vars
                    .insert(name, source_position(node));
            }
        } else {
            if let Some(name) = expr_root_ident(&node.left) {
                self.invalidate_plan_proof_var(&name);
            }
            self.visit_expr(&node.left);
        }
    }

    fn visit_expr_reference(&mut self, node: &'ast syn::ExprReference) {
        if node.mutability.is_some()
            && let Some(name) = expr_root_ident(&node.expr)
        {
            self.invalidate_plan_proof_var(&name);
        }
        visit::visit_expr_reference(self, node);
    }

    fn visit_expr_binary(&mut self, node: &'ast syn::ExprBinary) {
        if bin_op_is_assign(&node.op)
            && let Some(name) = expr_root_ident(&node.left)
        {
            self.invalidate_plan_proof_var(&name);
        }
        if matches!(node.op, syn::BinOp::And(_) | syn::BinOp::Or(_)) {
            self.with_conditional(|this| {
                this.visit_expr(&node.left);
                this.visit_expr(&node.right);
            });
        } else {
            visit::visit_expr_binary(self, node);
        }
    }

    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if let syn::Expr::Path(func) = &*node.func
            && let Some(name) = func.path.segments.last()
        {
            self.calls.insert(name.ident.to_string());
        }
        visit::visit_expr_call(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        let method = node.method.to_string();
        if flow_provenance_mutator_method(&method)
            && let Some(name) = expr_root_ident(&node.receiver)
        {
            self.invalidate_plan_proof_var(&name);
        }
        if method == "require"
            && self.fail_closed_depth > 0
            && self.conditional_depth == 0
            && self.expr_is_authority_parameter(&node.receiver)
            && let Some(arg) = node.args.first()
            && let Some(family) = flow_authority_kind_argument(arg, self.aliases)
            && let Some(command_var) = self.authority_kind_command_parameter(family, arg)
        {
            self.authority_requirements
                .entry(family)
                .or_default()
                .entry(command_var)
                .or_insert_with(|| source_position(node));
        }
        if method == "commit_prepared_dsl_input"
            && expr_is_self_receiver(&node.receiver)
            && self.conditional_depth == 1
            && let Some(prepared_var) = node.args.first().and_then(expr_path_single_ident)
            && self.prepared_plan_input_vars.contains_key(&prepared_var)
        {
            self.committed_prepared_input_vars
                .entry(prepared_var)
                .or_insert_with(|| PreparedCommitFact {
                    position: source_position(node),
                    guard_var: self.commit_guard_stack.last().cloned().flatten(),
                });
        }
        self.calls.insert(method);
        visit::visit_expr_method_call(self, node);
    }
}

struct FlowGovernanceVisitor<'a> {
    rel: &'a str,
    aliases: &'a RustUseAliases,
    facts: &'a FlowProjectionGovernanceFacts,
    impl_stack: Vec<Option<Vec<String>>>,
    module_depth: usize,
    function_stack: Vec<FlowFunctionContext>,
    alias_stack: Vec<RustUseAliases>,
    projection_state_alias_stack: Vec<BTreeMap<String, Option<String>>>,
    typed_flow_outcome_scope_stack: Vec<BTreeMap<String, bool>>,
    typed_flow_outcome_command_scope_stack: Vec<BTreeMap<String, Option<String>>>,
    typed_flow_outcome_identity_scope_stack: Vec<BTreeMap<String, Option<String>>>,
    typed_flow_next_state_scope_stack: Vec<BTreeMap<String, bool>>,
    typed_flow_next_state_command_scope_stack: Vec<BTreeMap<String, Option<String>>>,
    typed_flow_next_state_identity_scope_stack: Vec<BTreeMap<String, Option<String>>>,
    flow_run_command_scope_stack: Vec<BTreeMap<String, bool>>,
    flow_authority_token_scope_stack: Vec<BTreeMap<String, bool>>,
    flow_authority_input_scope_stack: Vec<BTreeMap<String, Option<String>>>,
    flow_authority_input_identity_scope_stack: Vec<BTreeMap<String, Option<String>>>,
    actor_plan_authority_input_scope_stack: Vec<BTreeMap<String, bool>>,
    shadowed_name_scope_stack: Vec<BTreeSet<String>>,
    deferred_expr_depth: usize,
    actor_plan_binding_scope_stack: Vec<ActorPlanArmFacts>,
    actor_plan_match_result_stack: Vec<Option<String>>,
    actor_plan_result_expr_depth: usize,
    actor_plan_authorized_commit_stack: Vec<bool>,
    mismatches: Vec<String>,
}

impl<'a> FlowGovernanceVisitor<'a> {
    fn new(
        rel: &'a str,
        aliases: &'a RustUseAliases,
        facts: &'a FlowProjectionGovernanceFacts,
    ) -> Self {
        Self {
            rel,
            aliases,
            facts,
            impl_stack: Vec::new(),
            module_depth: 0,
            function_stack: Vec::new(),
            alias_stack: Vec::new(),
            projection_state_alias_stack: Vec::new(),
            typed_flow_outcome_scope_stack: Vec::new(),
            typed_flow_outcome_command_scope_stack: Vec::new(),
            typed_flow_outcome_identity_scope_stack: Vec::new(),
            typed_flow_next_state_scope_stack: Vec::new(),
            typed_flow_next_state_command_scope_stack: Vec::new(),
            typed_flow_next_state_identity_scope_stack: Vec::new(),
            flow_run_command_scope_stack: Vec::new(),
            flow_authority_token_scope_stack: Vec::new(),
            flow_authority_input_scope_stack: Vec::new(),
            flow_authority_input_identity_scope_stack: Vec::new(),
            actor_plan_authority_input_scope_stack: Vec::new(),
            shadowed_name_scope_stack: Vec::new(),
            deferred_expr_depth: 0,
            actor_plan_binding_scope_stack: Vec::new(),
            actor_plan_match_result_stack: Vec::new(),
            actor_plan_result_expr_depth: 0,
            actor_plan_authorized_commit_stack: Vec::new(),
            mismatches: Vec::new(),
        }
    }

    fn current_function(&self) -> Option<&FlowFunctionContext> {
        self.function_stack.last()
    }

    fn current_aliases(&self) -> &RustUseAliases {
        self.alias_stack.last().unwrap_or(self.aliases)
    }

    fn name_is_shadowed(&self, name: &str) -> bool {
        self.shadowed_name_scope_stack
            .iter()
            .rev()
            .any(|scope| scope.contains(name))
    }

    fn visible_parameter_names(&self, type_ident: &str) -> BTreeSet<String> {
        self.current_function()
            .and_then(|function| function.parameter_names_by_type.get(type_ident))
            .into_iter()
            .flat_map(|names| names.iter())
            .filter(|name| !self.name_is_shadowed(name))
            .cloned()
            .collect()
    }

    fn has_visible_parameter_type(&self, type_ident: &str) -> bool {
        !self.visible_parameter_names(type_ident).is_empty()
    }

    fn visible_projection_state_alias(&self, alias: &str) -> Option<&str> {
        for scope in self.projection_state_alias_stack.iter().rev() {
            if let Some(value) = scope.get(alias) {
                return value.as_deref();
            }
        }
        None
    }

    fn visible_typed_flow_outcome_vars(&self) -> BTreeSet<String> {
        let mut resolved = BTreeMap::new();
        for scope in self.typed_flow_outcome_scope_stack.iter().rev() {
            for (name, is_typed) in scope {
                resolved.entry(name.clone()).or_insert(*is_typed);
            }
        }
        resolved
            .into_iter()
            .filter_map(|(name, is_typed)| is_typed.then_some(name))
            .collect()
    }

    fn visible_typed_flow_outcome_command_vars(&self) -> BTreeMap<String, String> {
        let mut resolved: BTreeMap<String, Option<String>> = BTreeMap::new();
        for scope in self.typed_flow_outcome_command_scope_stack.iter().rev() {
            for (name, command) in scope {
                resolved
                    .entry(name.clone())
                    .or_insert_with(|| command.clone());
            }
        }
        resolved
            .into_iter()
            .filter_map(|(name, command)| command.map(|command| (name, command)))
            .collect()
    }

    fn visible_typed_flow_outcome_identity_vars(&self) -> BTreeMap<String, String> {
        let mut resolved: BTreeMap<String, Option<String>> = BTreeMap::new();
        for scope in self.typed_flow_outcome_identity_scope_stack.iter().rev() {
            for (name, identity) in scope {
                resolved
                    .entry(name.clone())
                    .or_insert_with(|| identity.clone());
            }
        }
        resolved
            .into_iter()
            .filter_map(|(name, identity)| identity.map(|identity| (name, identity)))
            .collect()
    }

    fn visible_typed_flow_next_state_vars(&self) -> BTreeSet<String> {
        let mut resolved = BTreeMap::new();
        for scope in self.typed_flow_next_state_scope_stack.iter().rev() {
            for (name, is_typed) in scope {
                resolved.entry(name.clone()).or_insert(*is_typed);
            }
        }
        resolved
            .into_iter()
            .filter_map(|(name, is_typed)| is_typed.then_some(name))
            .collect()
    }

    fn visible_typed_flow_next_state_command_vars(&self) -> BTreeMap<String, String> {
        let mut resolved: BTreeMap<String, Option<String>> = BTreeMap::new();
        for scope in self.typed_flow_next_state_command_scope_stack.iter().rev() {
            for (name, command) in scope {
                resolved
                    .entry(name.clone())
                    .or_insert_with(|| command.clone());
            }
        }
        resolved
            .into_iter()
            .filter_map(|(name, command)| command.map(|command| (name, command)))
            .collect()
    }

    fn visible_typed_flow_next_state_identity_vars(&self) -> BTreeMap<String, String> {
        let mut resolved: BTreeMap<String, Option<String>> = BTreeMap::new();
        for scope in self.typed_flow_next_state_identity_scope_stack.iter().rev() {
            for (name, identity) in scope {
                resolved
                    .entry(name.clone())
                    .or_insert_with(|| identity.clone());
            }
        }
        resolved
            .into_iter()
            .filter_map(|(name, identity)| identity.map(|identity| (name, identity)))
            .collect()
    }

    fn visible_flow_run_command_vars(&self) -> BTreeSet<String> {
        let mut vars = self.visible_parameter_names("MobMachineFlowRunCommand");
        let mut resolved = BTreeMap::new();
        for scope in self.flow_run_command_scope_stack.iter().rev() {
            for (name, is_typed) in scope {
                resolved.entry(name.clone()).or_insert(*is_typed);
            }
        }
        for (name, is_typed) in resolved {
            if is_typed {
                vars.insert(name);
            } else {
                vars.remove(&name);
            }
        }
        vars
    }

    fn visible_flow_authority_token_vars(&self) -> BTreeSet<String> {
        let mut vars = self.visible_parameter_names("MobMachineFlowAuthorityToken");
        let mut resolved = BTreeMap::new();
        for scope in self.flow_authority_token_scope_stack.iter().rev() {
            for (name, is_typed) in scope {
                resolved.entry(name.clone()).or_insert(*is_typed);
            }
        }
        for (name, is_typed) in resolved {
            if is_typed {
                vars.insert(name);
            } else {
                vars.remove(&name);
            }
        }
        vars
    }

    fn visible_flow_authority_input_command_vars(&self) -> BTreeMap<String, String> {
        let mut resolved: BTreeMap<String, Option<String>> = BTreeMap::new();
        for scope in self.flow_authority_input_scope_stack.iter().rev() {
            for (name, command) in scope {
                resolved
                    .entry(name.clone())
                    .or_insert_with(|| command.clone());
            }
        }
        resolved
            .into_iter()
            .filter_map(|(name, command)| command.map(|command| (name, command)))
            .collect()
    }

    fn visible_flow_authority_input_identity_vars(&self) -> BTreeMap<String, String> {
        let mut resolved: BTreeMap<String, Option<String>> = BTreeMap::new();
        for scope in self.flow_authority_input_identity_scope_stack.iter().rev() {
            for (name, identity) in scope {
                resolved
                    .entry(name.clone())
                    .or_insert_with(|| identity.clone());
            }
        }
        resolved
            .into_iter()
            .filter_map(|(name, identity)| identity.map(|identity| (name, identity)))
            .collect()
    }

    fn visible_actor_plan_authority_input_vars(&self) -> BTreeSet<String> {
        let mut resolved = BTreeMap::new();
        for scope in self.actor_plan_authority_input_scope_stack.iter().rev() {
            for (name, is_plan_authority) in scope {
                resolved.entry(name.clone()).or_insert(*is_plan_authority);
            }
        }
        resolved
            .into_iter()
            .filter_map(|(name, is_plan_authority)| is_plan_authority.then_some(name))
            .collect()
    }

    fn visible_actor_plan_parameter_vars(&self) -> BTreeSet<String> {
        let names = self.visible_parameter_names("FlowFrameLoopStorePlan");
        if names.len() == 1 {
            names
        } else {
            BTreeSet::new()
        }
    }

    fn expr_is_actor_plan_scrutinee(&self, expr: &syn::Expr) -> bool {
        expr_is_plan_scrutinee(expr, &self.visible_actor_plan_parameter_vars())
    }

    fn expr_is_actor_plan_machine_inputs_value(
        &self,
        expr: &syn::Expr,
        plan_authority_input_vars: &BTreeSet<String>,
    ) -> bool {
        expr_is_plan_machine_inputs_value(
            expr,
            plan_authority_input_vars,
            &self.visible_actor_plan_parameter_vars(),
        )
    }

    fn set_typed_flow_outcome_var(&mut self, name: &str, is_typed: bool) {
        for scope in self.typed_flow_outcome_scope_stack.iter_mut().rev() {
            if scope.contains_key(name) {
                scope.insert(name.to_string(), is_typed);
                return;
            }
        }
        if let Some(scope) = self.typed_flow_outcome_scope_stack.last_mut() {
            scope.insert(name.to_string(), is_typed);
        }
    }

    fn set_typed_flow_outcome_command_var(&mut self, name: &str, command: Option<String>) {
        for scope in self.typed_flow_outcome_command_scope_stack.iter_mut().rev() {
            if scope.contains_key(name) {
                scope.insert(name.to_string(), command);
                return;
            }
        }
        if let Some(scope) = self.typed_flow_outcome_command_scope_stack.last_mut() {
            scope.insert(name.to_string(), command);
        }
    }

    fn set_typed_flow_outcome_identity_var(&mut self, name: &str, identity: Option<String>) {
        for scope in self
            .typed_flow_outcome_identity_scope_stack
            .iter_mut()
            .rev()
        {
            if scope.contains_key(name) {
                scope.insert(name.to_string(), identity);
                return;
            }
        }
        if let Some(scope) = self.typed_flow_outcome_identity_scope_stack.last_mut() {
            scope.insert(name.to_string(), identity);
        }
    }

    fn set_typed_flow_next_state_var(&mut self, name: &str, is_typed: bool) {
        for scope in self.typed_flow_next_state_scope_stack.iter_mut().rev() {
            if scope.contains_key(name) {
                scope.insert(name.to_string(), is_typed);
                return;
            }
        }
        if let Some(scope) = self.typed_flow_next_state_scope_stack.last_mut() {
            scope.insert(name.to_string(), is_typed);
        }
    }

    fn set_typed_flow_next_state_command_var(&mut self, name: &str, command: Option<String>) {
        for scope in self
            .typed_flow_next_state_command_scope_stack
            .iter_mut()
            .rev()
        {
            if scope.contains_key(name) {
                scope.insert(name.to_string(), command);
                return;
            }
        }
        if let Some(scope) = self.typed_flow_next_state_command_scope_stack.last_mut() {
            scope.insert(name.to_string(), command);
        }
    }

    fn set_typed_flow_next_state_identity_var(&mut self, name: &str, identity: Option<String>) {
        for scope in self
            .typed_flow_next_state_identity_scope_stack
            .iter_mut()
            .rev()
        {
            if scope.contains_key(name) {
                scope.insert(name.to_string(), identity);
                return;
            }
        }
        if let Some(scope) = self.typed_flow_next_state_identity_scope_stack.last_mut() {
            scope.insert(name.to_string(), identity);
        }
    }

    fn set_flow_run_command_var(&mut self, name: &str, is_typed: bool) {
        for scope in self.flow_run_command_scope_stack.iter_mut().rev() {
            if scope.contains_key(name) {
                scope.insert(name.to_string(), is_typed);
                return;
            }
        }
        if let Some(scope) = self.flow_run_command_scope_stack.last_mut() {
            scope.insert(name.to_string(), is_typed);
        }
    }

    fn set_flow_authority_token_var(&mut self, name: &str, is_typed: bool) {
        for scope in self.flow_authority_token_scope_stack.iter_mut().rev() {
            if scope.contains_key(name) {
                scope.insert(name.to_string(), is_typed);
                return;
            }
        }
        if let Some(scope) = self.flow_authority_token_scope_stack.last_mut() {
            scope.insert(name.to_string(), is_typed);
        }
    }

    fn set_flow_authority_input_var(&mut self, name: &str, command: Option<String>) {
        for scope in self.flow_authority_input_scope_stack.iter_mut().rev() {
            if scope.contains_key(name) {
                scope.insert(name.to_string(), command);
                return;
            }
        }
        if let Some(scope) = self.flow_authority_input_scope_stack.last_mut() {
            scope.insert(name.to_string(), command);
        }
    }

    fn set_flow_authority_input_identity_var(&mut self, name: &str, identity: Option<String>) {
        for scope in self
            .flow_authority_input_identity_scope_stack
            .iter_mut()
            .rev()
        {
            if scope.contains_key(name) {
                scope.insert(name.to_string(), identity);
                return;
            }
        }
        if let Some(scope) = self.flow_authority_input_identity_scope_stack.last_mut() {
            scope.insert(name.to_string(), identity);
        }
    }

    fn set_actor_plan_authority_input_var(&mut self, name: &str, is_plan_authority: bool) {
        for scope in self.actor_plan_authority_input_scope_stack.iter_mut().rev() {
            if scope.contains_key(name) {
                scope.insert(name.to_string(), is_plan_authority);
                return;
            }
        }
        if let Some(scope) = self.actor_plan_authority_input_scope_stack.last_mut() {
            scope.insert(name.to_string(), is_plan_authority);
        }
    }

    fn invalidate_typed_flow_outcome_path(&mut self, expr: &syn::Expr) {
        if let Some(name) = expr_field_chain(expr).first().cloned()
            && self.visible_typed_flow_outcome_vars().contains(&name)
        {
            self.set_typed_flow_outcome_var(&name, false);
            self.set_typed_flow_outcome_command_var(&name, None);
            self.set_typed_flow_outcome_identity_var(&name, None);
        }
    }

    fn invalidate_typed_flow_next_state_path(&mut self, expr: &syn::Expr) {
        if let Some(name) = expr_field_chain(expr).first().cloned()
            && self.visible_typed_flow_next_state_vars().contains(&name)
        {
            self.set_typed_flow_next_state_var(&name, false);
            self.set_typed_flow_next_state_command_var(&name, None);
            self.set_typed_flow_next_state_identity_var(&name, None);
        }
    }

    fn invalidate_flow_provenance_var(&mut self, name: &str) {
        self.set_typed_flow_outcome_var(name, false);
        self.set_typed_flow_outcome_command_var(name, None);
        self.set_typed_flow_outcome_identity_var(name, None);
        self.set_typed_flow_next_state_var(name, false);
        self.set_typed_flow_next_state_command_var(name, None);
        self.set_typed_flow_next_state_identity_var(name, None);
        self.set_flow_run_command_var(name, false);
        self.set_flow_authority_token_var(name, false);
        self.set_flow_authority_input_var(name, None);
        self.set_flow_authority_input_identity_var(name, None);
        self.set_actor_plan_authority_input_var(name, false);
        self.set_projection_state_alias_var(name, None);
    }

    fn set_projection_state_alias_var(&mut self, name: &str, alias: Option<String>) {
        for scope in self.projection_state_alias_stack.iter_mut().rev() {
            if scope.contains_key(name) {
                scope.insert(name.to_string(), alias);
                return;
            }
        }
        if let Some(scope) = self.projection_state_alias_stack.last_mut() {
            scope.insert(name.to_string(), alias);
        }
    }

    fn current_actor_plan_arm(&self) -> Option<&ActorPlanArmFacts> {
        self.actor_plan_binding_scope_stack.last()
    }

    fn current_actor_plan_match_result_var(&self) -> Option<&str> {
        self.actor_plan_match_result_stack
            .last()
            .and_then(|name| name.as_deref())
    }

    fn mark_actor_plan_authorized_commit(&mut self) {
        if let Some(seen) = self.actor_plan_authorized_commit_stack.last_mut() {
            *seen = true;
        }
    }

    fn with_actor_plan_commit_marker<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> (R, bool) {
        self.actor_plan_authorized_commit_stack.push(false);
        let result = f(self);
        let seen = self
            .actor_plan_authorized_commit_stack
            .pop()
            .unwrap_or(false);
        (result, seen)
    }

    fn with_actor_plan_result_expr<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        self.actor_plan_result_expr_depth += 1;
        let result = f(self);
        self.actor_plan_result_expr_depth -= 1;
        result
    }

    fn visit_actor_plan_result_stmt(&mut self, stmt: &syn::Stmt) -> ActorPlanResultProof {
        match stmt {
            syn::Stmt::Expr(expr, None) => self.visit_actor_plan_result_expr(expr),
            _ => {
                self.visit_stmt(stmt);
                ActorPlanResultProof::unauthorized()
            }
        }
    }

    fn visit_actor_plan_result_expr(&mut self, expr: &syn::Expr) -> ActorPlanResultProof {
        match expr {
            syn::Expr::Block(block) => {
                let local_aliases = self.current_aliases().with_block_aliases(&block.block);
                self.alias_stack.push(local_aliases);
                self.typed_flow_outcome_scope_stack.push(BTreeMap::new());
                self.typed_flow_outcome_command_scope_stack
                    .push(BTreeMap::new());
                self.typed_flow_outcome_identity_scope_stack
                    .push(BTreeMap::new());
                self.typed_flow_next_state_scope_stack.push(BTreeMap::new());
                self.typed_flow_next_state_command_scope_stack
                    .push(BTreeMap::new());
                self.typed_flow_next_state_identity_scope_stack
                    .push(BTreeMap::new());
                self.flow_run_command_scope_stack.push(BTreeMap::new());
                self.flow_authority_token_scope_stack.push(BTreeMap::new());
                self.flow_authority_input_scope_stack.push(BTreeMap::new());
                self.flow_authority_input_identity_scope_stack
                    .push(BTreeMap::new());
                self.actor_plan_authority_input_scope_stack
                    .push(BTreeMap::new());
                self.projection_state_alias_stack.push(BTreeMap::new());
                self.shadowed_name_scope_stack
                    .push(block_item_function_names(&block.block));
                let mut proof = ActorPlanResultProof::unauthorized();
                for (index, stmt) in block.block.stmts.iter().enumerate() {
                    if index + 1 == block.block.stmts.len() {
                        proof = self.visit_actor_plan_result_stmt(stmt);
                    } else {
                        self.visit_stmt(stmt);
                    }
                }
                let _ = self.shadowed_name_scope_stack.pop();
                let _ = self.projection_state_alias_stack.pop();
                let _ = self.actor_plan_authority_input_scope_stack.pop();
                let _ = self.flow_authority_input_identity_scope_stack.pop();
                let _ = self.flow_authority_input_scope_stack.pop();
                let _ = self.flow_authority_token_scope_stack.pop();
                let _ = self.flow_run_command_scope_stack.pop();
                let _ = self.typed_flow_next_state_identity_scope_stack.pop();
                let _ = self.typed_flow_next_state_command_scope_stack.pop();
                let _ = self.typed_flow_next_state_scope_stack.pop();
                let _ = self.typed_flow_outcome_identity_scope_stack.pop();
                let _ = self.typed_flow_outcome_command_scope_stack.pop();
                let _ = self.typed_flow_outcome_scope_stack.pop();
                let _ = self.alias_stack.pop();
                proof
            }
            syn::Expr::If(expr_if) => {
                self.visit_expr(&expr_if.cond);
                let then_expr = syn::Expr::Block(syn::ExprBlock {
                    attrs: Vec::new(),
                    label: None,
                    block: expr_if.then_branch.clone(),
                });
                let then_proof = if let syn::Expr::Let(expr_let) = &*expr_if.cond {
                    self.with_pattern_bindings(&expr_let.pat, &expr_let.expr, |this| {
                        this.visit_actor_plan_result_expr(&then_expr)
                    })
                } else {
                    self.visit_actor_plan_result_expr(&then_expr)
                };
                let else_proof = if let Some((_, else_branch)) = &expr_if.else_branch {
                    self.visit_actor_plan_result_expr(else_branch)
                } else {
                    ActorPlanResultProof::unauthorized()
                };
                then_proof.combine(else_proof)
            }
            syn::Expr::Match(expr_match) => {
                self.visit_expr(&expr_match.expr);
                let mut proof = ActorPlanResultProof::static_false();
                for arm in &expr_match.arms {
                    let arm_proof =
                        self.with_pattern_bindings(&arm.pat, &expr_match.expr, |this| {
                            if let Some((_, guard)) = &arm.guard {
                                this.visit_expr(guard);
                            }
                            this.visit_actor_plan_result_expr(&arm.body)
                        });
                    proof = proof.combine(arm_proof);
                }
                proof
            }
            syn::Expr::Try(try_expr) => self.visit_actor_plan_result_expr(&try_expr.expr),
            syn::Expr::Await(await_expr) => self.visit_actor_plan_result_expr(&await_expr.base),
            syn::Expr::Reference(reference) => self.visit_actor_plan_result_expr(&reference.expr),
            syn::Expr::Paren(paren) => self.visit_actor_plan_result_expr(&paren.expr),
            syn::Expr::Group(group) => self.visit_actor_plan_result_expr(&group.expr),
            syn::Expr::MethodCall(call) if call.method == "map_err" => {
                let proof = self.visit_actor_plan_result_expr(&call.receiver);
                for arg in &call.args {
                    self.visit_expr(arg);
                }
                proof
            }
            syn::Expr::MethodCall(call)
                if self
                    .facts
                    .cas_methods
                    .contains_key(&call.method.to_string()) =>
            {
                let ((), seen) = self.with_actor_plan_commit_marker(|this| {
                    this.with_actor_plan_result_expr(|this| {
                        this.visit_expr(expr);
                    });
                });
                if seen {
                    ActorPlanResultProof::authorized()
                } else {
                    ActorPlanResultProof::unauthorized()
                }
            }
            syn::Expr::Call(call)
                if expr_as_path(&call.func)
                    .and_then(|path| path.segments.last())
                    .is_some_and(|segment| {
                        self.facts
                            .cas_methods
                            .contains_key(&segment.ident.to_string())
                    }) =>
            {
                let ((), seen) = self.with_actor_plan_commit_marker(|this| {
                    this.with_actor_plan_result_expr(|this| {
                        this.visit_expr(expr);
                    });
                });
                if seen {
                    ActorPlanResultProof::authorized()
                } else {
                    ActorPlanResultProof::unauthorized()
                }
            }
            _ if expr_static_bool_value(expr) == Some(false) => {
                ActorPlanResultProof::static_false()
            }
            _ if expr_static_bool_value(expr) == Some(true) => {
                self.visit_expr(expr);
                ActorPlanResultProof::unauthorized()
            }
            _ => {
                self.visit_expr(expr);
                ActorPlanResultProof::unauthorized()
            }
        }
    }

    fn bind_local_names(&mut self, pat: &syn::Pat) {
        let names = pat_bound_idents(pat);
        if let Some(scope) = self.shadowed_name_scope_stack.last_mut() {
            scope.extend(names.iter().cloned());
        }
        if let Some(scope) = self.typed_flow_outcome_scope_stack.last_mut() {
            for name in &names {
                scope.entry(name.clone()).or_insert(false);
            }
        }
        if let Some(scope) = self.typed_flow_outcome_command_scope_stack.last_mut() {
            for name in &names {
                scope.entry(name.clone()).or_insert(None);
            }
        }
        if let Some(scope) = self.typed_flow_outcome_identity_scope_stack.last_mut() {
            for name in &names {
                scope.entry(name.clone()).or_insert(None);
            }
        }
        if let Some(scope) = self.typed_flow_next_state_scope_stack.last_mut() {
            for name in &names {
                scope.entry(name.clone()).or_insert(false);
            }
        }
        if let Some(scope) = self.typed_flow_next_state_command_scope_stack.last_mut() {
            for name in &names {
                scope.entry(name.clone()).or_insert(None);
            }
        }
        if let Some(scope) = self.typed_flow_next_state_identity_scope_stack.last_mut() {
            for name in &names {
                scope.entry(name.clone()).or_insert(None);
            }
        }
        if let Some(scope) = self.flow_run_command_scope_stack.last_mut() {
            for name in &names {
                scope.entry(name.clone()).or_insert(false);
            }
        }
        if let Some(scope) = self.flow_authority_token_scope_stack.last_mut() {
            for name in &names {
                scope.entry(name.clone()).or_insert(false);
            }
        }
        if let Some(scope) = self.flow_authority_input_scope_stack.last_mut() {
            for name in &names {
                scope.entry(name.clone()).or_insert(None);
            }
        }
        if let Some(scope) = self.flow_authority_input_identity_scope_stack.last_mut() {
            for name in &names {
                scope.entry(name.clone()).or_insert(None);
            }
        }
        if let Some(scope) = self.actor_plan_authority_input_scope_stack.last_mut() {
            for name in &names {
                scope.entry(name.clone()).or_insert(false);
            }
        }
        if let Some(scope) = self.projection_state_alias_stack.last_mut() {
            for name in names {
                scope.entry(name).or_insert(None);
            }
        }
    }

    fn with_deferred_expr<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        self.deferred_expr_depth += 1;
        let result = f(self);
        self.deferred_expr_depth -= 1;
        result
    }

    fn record_direct_use(
        &mut self,
        kind: FlowDirectUse,
        line: usize,
        position: SourcePosition,
        source_path: &syn::Path,
        call: Option<&syn::ExprCall>,
    ) {
        if self.direct_use_allowed(kind, position, call) {
            return;
        }
        let source_segments = path_segments(source_path);
        let source_label = source_segments.join("::");
        match kind {
            FlowDirectUse::Transition { .. } if source_segments.len() == 1 => {
                self.mismatches.push(format!(
                    "direct live-flow reducer transition alias `{source_label}` is not MobMachine-command gated: {}:{line}",
                    self.rel
                ));
            }
            FlowDirectUse::Transition { .. } => {
                self.mismatches.push(format!(
                    "direct live-flow reducer transition `{source_label}(` is not MobMachine-command gated: {}:{line}",
                    self.rel
                ));
            }
            FlowDirectUse::Input { .. }
                if source_segments
                    .first()
                    .is_some_and(|segment| FlowReducerFamily::from_module(segment).is_none()) =>
            {
                let alias = source_segments
                    .first()
                    .map(String::as_str)
                    .unwrap_or(source_label.as_str());
                self.mismatches.push(format!(
                    "direct live-flow reducer input alias `{alias}` is not MobMachine-command gated: {}:{line}",
                    self.rel
                ));
            }
            FlowDirectUse::Input { .. } => {
                self.mismatches.push(format!(
                    "direct live-flow reducer input `{source_label}` is not MobMachine-command gated: {}:{line}",
                    self.rel
                ));
            }
        }
    }

    fn direct_use_allowed(
        &self,
        kind: FlowDirectUse,
        position: SourcePosition,
        call: Option<&syn::ExprCall>,
    ) -> bool {
        if self.rel != "meerkat-mob/src/run.rs" {
            return false;
        }
        let Some(function) = self.current_function() else {
            return false;
        };
        if self.deferred_expr_depth > 0 {
            return false;
        }
        match kind {
            FlowDirectUse::Transition { canonical, .. }
            | FlowDirectUse::Input { canonical, .. }
                if !canonical =>
            {
                return false;
            }
            _ => {}
        }
        if function.module_depth != 0 {
            return false;
        }
        match kind {
            FlowDirectUse::Transition { family, .. } => {
                let command_vars = if family == FlowReducerFamily::FlowRun {
                    self.visible_flow_run_command_vars()
                } else {
                    self.visible_parameter_names(family.command_type())
                };
                let Some(command_var) =
                    call.and_then(|call| transition_call_command_input_var(call, &command_vars))
                else {
                    return false;
                };
                function.name == family.apply_function()
                    && self.has_visible_parameter_type(family.command_type())
                    && self.has_visible_parameter_type("MobMachineFlowAuthorityToken")
                    && function
                        .authority_requirements
                        .get(&family)
                        .and_then(|requirements| requirements.get(&command_var))
                        .is_some_and(|required_position| *required_position < position)
            }
            FlowDirectUse::Input { family, .. } => {
                function.name == "into_input"
                    && function.impl_type.as_deref() == Some(family.command_type())
                    && function
                        .return_path
                        .as_ref()
                        .is_some_and(|path| path_has_tail(path, &[family.module(), "Input"]))
            }
        }
    }

    fn actor_store_plan_commit_allowed(
        &self,
        node: &syn::ExprMethodCall,
        cas_facts: &ProjectionCasMethodFacts,
    ) -> bool {
        if self.deferred_expr_depth > 0 {
            return false;
        }
        if self.actor_plan_result_expr_depth == 0 {
            return false;
        }
        let Some(function) = self.current_function() else {
            return false;
        };
        if function.name != "commit_flow_frame_store_plan_in_actor" {
            return false;
        }
        if function.module_depth != 0 {
            return false;
        }
        if !function
            .impl_type_path
            .as_deref()
            .is_some_and(path_is_root_mob_actor_impl)
        {
            return false;
        }
        if !expr_is_self_run_store_receiver(&node.receiver) {
            return false;
        }
        let Some(plan_arm) = self.current_actor_plan_arm() else {
            return false;
        };
        let plan_bindings = &plan_arm.bindings;
        if !cas_facts.authority_identity_arg_indices.is_empty() {
            let run_id_parameters = self.visible_parameter_names("RunId");
            let Some(authority_run_id) = run_id_parameters.iter().next() else {
                return false;
            };
            if plan_arm.bindings.contains(authority_run_id)
                || run_id_parameters.len() != 1
                || !cas_facts
                    .authority_identity_arg_indices
                    .iter()
                    .all(|index| {
                        node.args.iter().nth(*index).is_some_and(|arg| {
                            expr_identity_argument_name(arg)
                                .as_ref()
                                .is_some_and(|name| name == authority_run_id)
                        })
                    })
            {
                return false;
            }
        }
        if cas_facts.plan_state_arg_indices.is_empty()
            || !cas_facts.plan_state_arg_indices.iter().all(|index| {
                node.args.iter().nth(*index).is_some_and(|arg| {
                    expr_variable_name(arg, plan_bindings)
                        .as_ref()
                        .is_some_and(|name| !self.name_is_shadowed(name))
                })
            })
        {
            return false;
        }
        let plan_authority_inputs = self.visible_actor_plan_authority_input_vars();
        if !cas_facts.plan_owned_arg_indices.iter().all(|index| {
            node.args.iter().nth(*index).is_some_and(|arg| {
                self.expr_is_actor_plan_owned_arg(arg, plan_bindings, &plan_authority_inputs)
            })
        }) {
            return false;
        }
        if !self.actor_plan_variant_commit_method_allowed(node, plan_arm) {
            return false;
        }
        if !self.actor_plan_cas_args_match_variant_fields(node, cas_facts, plan_arm) {
            return false;
        }
        if !self.actor_plan_variant_fields_are_consumed(node, cas_facts, plan_arm) {
            return false;
        }
        let Some(match_result_var) = self.current_actor_plan_match_result_var() else {
            return false;
        };
        let position = source_position(node);
        let Some(freshness_position) = function.plan_freshness_check_position else {
            return false;
        };
        function
            .prepared_plan_input_vars
            .iter()
            .any(|(var, prepared_position)| {
                freshness_position < *prepared_position
                    && *prepared_position < position
                    && function
                        .committed_prepared_input_vars
                        .get(var)
                        .is_some_and(|commit| {
                            commit.position > position
                                && commit.guard_var.as_deref() == Some(match_result_var)
                        })
            })
    }

    fn expr_is_actor_plan_owned_arg(
        &self,
        expr: &syn::Expr,
        plan_bindings: &BTreeSet<String>,
        plan_authority_inputs: &BTreeSet<String>,
    ) -> bool {
        if expr_is_none_constructor(expr, self.current_aliases()) {
            return true;
        }
        if expr_variable_name(expr, plan_bindings)
            .as_ref()
            .is_some_and(|name| !self.name_is_shadowed(name))
        {
            return true;
        }
        if expr_variable_name(expr, plan_authority_inputs).is_some() {
            return true;
        }
        if self.expr_is_actor_plan_machine_inputs_value(expr, plan_authority_inputs) {
            return true;
        }
        match expr {
            syn::Expr::Call(call) => {
                expr_as_path(&call.func).is_some_and(|path| {
                    path.segments
                        .last()
                        .is_some_and(|segment| segment.ident == "Some")
                }) && call.args.iter().all(|arg| {
                    self.expr_is_actor_plan_owned_arg(arg, plan_bindings, plan_authority_inputs)
                })
            }
            syn::Expr::Tuple(tuple) => tuple.elems.iter().all(|elem| {
                self.expr_is_actor_plan_owned_arg(elem, plan_bindings, plan_authority_inputs)
            }),
            syn::Expr::MethodCall(call) if call.method == "map" => {
                if !self.expr_is_actor_plan_owned_arg(
                    &call.receiver,
                    plan_bindings,
                    plan_authority_inputs,
                ) {
                    return false;
                }
                let Some(syn::Expr::Closure(closure)) = call.args.first() else {
                    return false;
                };
                let mut closure_plan_bindings = plan_bindings.clone();
                for input in &closure.inputs {
                    closure_plan_bindings.extend(pat_bound_idents(input));
                }
                self.expr_is_actor_plan_owned_arg(
                    &closure.body,
                    &closure_plan_bindings,
                    plan_authority_inputs,
                )
            }
            syn::Expr::MethodCall(call)
                if matches!(
                    call.method.to_string().as_str(),
                    "clone" | "as_ref" | "to_vec" | "cloned" | "copied"
                ) =>
            {
                self.expr_is_actor_plan_owned_arg(
                    &call.receiver,
                    plan_bindings,
                    plan_authority_inputs,
                )
            }
            syn::Expr::Unary(unary) if matches!(unary.op, syn::UnOp::Deref(_)) => {
                self.expr_is_actor_plan_owned_arg(&unary.expr, plan_bindings, plan_authority_inputs)
            }
            syn::Expr::Reference(reference) => self.expr_is_actor_plan_owned_arg(
                &reference.expr,
                plan_bindings,
                plan_authority_inputs,
            ),
            syn::Expr::Paren(paren) => {
                self.expr_is_actor_plan_owned_arg(&paren.expr, plan_bindings, plan_authority_inputs)
            }
            syn::Expr::Group(group) => {
                self.expr_is_actor_plan_owned_arg(&group.expr, plan_bindings, plan_authority_inputs)
            }
            syn::Expr::Try(try_expr) => self.expr_is_actor_plan_owned_arg(
                &try_expr.expr,
                plan_bindings,
                plan_authority_inputs,
            ),
            syn::Expr::Await(await_expr) => self.expr_is_actor_plan_owned_arg(
                &await_expr.base,
                plan_bindings,
                plan_authority_inputs,
            ),
            _ => false,
        }
    }

    fn actor_plan_variant_fields_are_consumed(
        &self,
        node: &syn::ExprMethodCall,
        cas_facts: &ProjectionCasMethodFacts,
        plan_arm: &ActorPlanArmFacts,
    ) -> bool {
        let Some(variant) = actor_plan_single_variant(plan_arm) else {
            return false;
        };
        let Some(required_fields) = self.facts.store_plan_variant_fields.get(&variant) else {
            return false;
        };
        let consumed = cas_facts
            .plan_owned_arg_indices
            .iter()
            .filter_map(|index| node.args.iter().nth(*index))
            .flat_map(expr_root_idents_through_calls)
            .filter(|name| !self.name_is_shadowed(name))
            .collect::<BTreeSet<_>>();

        required_fields.iter().all(|field| {
            plan_arm
                .field_bindings
                .get(field)
                .is_some_and(|bindings| bindings.iter().any(|binding| consumed.contains(binding)))
        })
    }

    fn actor_plan_variant_commit_method_allowed(
        &self,
        node: &syn::ExprMethodCall,
        plan_arm: &ActorPlanArmFacts,
    ) -> bool {
        let Some(variant) = actor_plan_single_variant(plan_arm) else {
            return false;
        };
        self.facts
            .store_plan_variant_commit_methods
            .get(&variant)
            .is_some_and(|methods| methods.contains(&node.method.to_string()))
    }

    fn actor_plan_cas_args_match_variant_fields(
        &self,
        node: &syn::ExprMethodCall,
        cas_facts: &ProjectionCasMethodFacts,
        plan_arm: &ActorPlanArmFacts,
    ) -> bool {
        let Some(variant) = actor_plan_single_variant(plan_arm) else {
            return false;
        };
        let Some(required_fields) = self.facts.store_plan_variant_fields.get(&variant) else {
            return false;
        };
        if variant == "InsertFrame"
            && let Some((expected_index, _)) = cas_facts
                .arg_names
                .iter()
                .find(|(_, arg_name)| arg_name.as_str() == "expected")
            && !node
                .args
                .iter()
                .nth(*expected_index)
                .is_some_and(|arg| expr_is_none_constructor(arg, self.current_aliases()))
        {
            return false;
        }
        cas_facts
            .arg_names
            .iter()
            .filter(|(_, arg_name)| {
                arg_name.as_str() != "run_id" && arg_name.as_str() != "authority_inputs"
            })
            .all(|(index, arg_name)| {
                let Some(field) = actor_plan_field_for_cas_arg(arg_name.as_str(), required_fields)
                else {
                    return true;
                };
                let Some(arg) = node.args.iter().nth(*index) else {
                    return false;
                };
                let consumed = expr_root_idents_through_calls(arg)
                    .into_iter()
                    .filter(|name| !self.name_is_shadowed(name))
                    .collect::<BTreeSet<_>>();
                plan_arm.field_bindings.get(&field).is_some_and(|bindings| {
                    bindings.iter().any(|binding| consumed.contains(binding))
                })
            })
    }

    fn path_resolves_to_canonical_flow_run_apply(&self, path: &syn::Path) -> bool {
        let segments = path_segments(path);
        if segments.len() == 1
            && segments
                .first()
                .is_some_and(|name| self.name_is_shadowed(name))
        {
            return false;
        }
        self.current_aliases()
            .resolve_path(path)
            .into_iter()
            .any(|resolved| resolved_path_is_canonical_flow_run_apply(&resolved))
    }

    fn block_shadows_flow_apply_command_provenance(&self, block: &syn::Block) -> bool {
        let mut protected = self.visible_flow_run_command_vars();
        protected.extend(self.visible_flow_authority_token_vars());
        block_shadows_names(block, &protected)
    }

    fn block_shadows_flow_apply_identity_provenance(&self, block: &syn::Block) -> bool {
        block_shadows_names(block, &self.visible_parameter_names("RunId"))
    }

    fn expr_canonical_flow_run_apply_command_var(&self, expr: &syn::Expr) -> Option<String> {
        match expr {
            syn::Expr::Call(call) => {
                let path = expr_as_path(&call.func)?;
                if !self.path_resolves_to_canonical_flow_run_apply(path) {
                    return None;
                }
                let command_vars = self.visible_flow_run_command_vars();
                let authority_vars = self.visible_flow_authority_token_vars();
                if authority_vars.is_empty()
                    || !call
                        .args
                        .iter()
                        .any(|arg| expr_variable_name(arg, &authority_vars).is_some())
                {
                    return None;
                }
                call.args
                    .iter()
                    .find_map(|arg| expr_variable_name(arg, &command_vars))
            }
            syn::Expr::MethodCall(call) if call.method == "map_err" => {
                self.expr_canonical_flow_run_apply_command_var(&call.receiver)
            }
            syn::Expr::Try(try_expr) => {
                self.expr_canonical_flow_run_apply_command_var(&try_expr.expr)
            }
            syn::Expr::Await(await_expr) => {
                self.expr_canonical_flow_run_apply_command_var(&await_expr.base)
            }
            syn::Expr::Reference(reference) => {
                self.expr_canonical_flow_run_apply_command_var(&reference.expr)
            }
            syn::Expr::Paren(paren) => self.expr_canonical_flow_run_apply_command_var(&paren.expr),
            syn::Expr::Group(group) => self.expr_canonical_flow_run_apply_command_var(&group.expr),
            syn::Expr::Block(block) => {
                if self.block_shadows_flow_apply_command_provenance(&block.block) {
                    return None;
                }
                block
                    .block
                    .stmts
                    .last()
                    .and_then(|stmt| self.stmt_canonical_flow_run_apply_command_var(stmt))
            }
            _ => None,
        }
    }

    fn expr_canonical_flow_run_apply_identity_var(&self, expr: &syn::Expr) -> Option<String> {
        match expr {
            syn::Expr::Call(call) => {
                let path = expr_as_path(&call.func)?;
                if !self.path_resolves_to_canonical_flow_run_apply(path) {
                    return None;
                }
                let run_id_vars = self.visible_parameter_names("RunId");
                call.args.iter().find_map(|arg| {
                    expr_identity_argument_name(arg).filter(|name| run_id_vars.contains(name))
                })
            }
            syn::Expr::MethodCall(call) if call.method == "map_err" => {
                self.expr_canonical_flow_run_apply_identity_var(&call.receiver)
            }
            syn::Expr::Try(try_expr) => {
                self.expr_canonical_flow_run_apply_identity_var(&try_expr.expr)
            }
            syn::Expr::Await(await_expr) => {
                self.expr_canonical_flow_run_apply_identity_var(&await_expr.base)
            }
            syn::Expr::Reference(reference) => {
                self.expr_canonical_flow_run_apply_identity_var(&reference.expr)
            }
            syn::Expr::Paren(paren) => self.expr_canonical_flow_run_apply_identity_var(&paren.expr),
            syn::Expr::Group(group) => self.expr_canonical_flow_run_apply_identity_var(&group.expr),
            syn::Expr::Block(block) => {
                if self.block_shadows_flow_apply_identity_provenance(&block.block) {
                    return None;
                }
                block
                    .block
                    .stmts
                    .last()
                    .and_then(|stmt| self.stmt_canonical_flow_run_apply_identity_var(stmt))
            }
            _ => None,
        }
    }

    fn expr_canonical_flow_run_apply_next_state_command_var(
        &self,
        expr: &syn::Expr,
    ) -> Option<String> {
        match expr {
            syn::Expr::Field(field) => {
                if matches!(&field.member, syn::Member::Named(ident) if ident == "next_state") {
                    self.expr_canonical_flow_run_apply_command_var(&field.base)
                } else {
                    None
                }
            }
            syn::Expr::MethodCall(call) if call.method == "clone" => {
                self.expr_canonical_flow_run_apply_next_state_command_var(&call.receiver)
            }
            syn::Expr::Try(try_expr) => {
                self.expr_canonical_flow_run_apply_next_state_command_var(&try_expr.expr)
            }
            syn::Expr::Await(await_expr) => {
                self.expr_canonical_flow_run_apply_next_state_command_var(&await_expr.base)
            }
            syn::Expr::Reference(reference) => {
                self.expr_canonical_flow_run_apply_next_state_command_var(&reference.expr)
            }
            syn::Expr::Paren(paren) => {
                self.expr_canonical_flow_run_apply_next_state_command_var(&paren.expr)
            }
            syn::Expr::Group(group) => {
                self.expr_canonical_flow_run_apply_next_state_command_var(&group.expr)
            }
            _ => None,
        }
    }

    fn expr_canonical_flow_run_apply_next_state_identity_var(
        &self,
        expr: &syn::Expr,
    ) -> Option<String> {
        match expr {
            syn::Expr::Field(field) => {
                if matches!(&field.member, syn::Member::Named(ident) if ident == "next_state") {
                    self.expr_canonical_flow_run_apply_identity_var(&field.base)
                } else {
                    None
                }
            }
            syn::Expr::MethodCall(call) if call.method == "clone" => {
                self.expr_canonical_flow_run_apply_next_state_identity_var(&call.receiver)
            }
            syn::Expr::Try(try_expr) => {
                self.expr_canonical_flow_run_apply_next_state_identity_var(&try_expr.expr)
            }
            syn::Expr::Await(await_expr) => {
                self.expr_canonical_flow_run_apply_next_state_identity_var(&await_expr.base)
            }
            syn::Expr::Reference(reference) => {
                self.expr_canonical_flow_run_apply_next_state_identity_var(&reference.expr)
            }
            syn::Expr::Paren(paren) => {
                self.expr_canonical_flow_run_apply_next_state_identity_var(&paren.expr)
            }
            syn::Expr::Group(group) => {
                self.expr_canonical_flow_run_apply_next_state_identity_var(&group.expr)
            }
            _ => None,
        }
    }

    fn stmt_canonical_flow_run_apply_command_var(&self, stmt: &syn::Stmt) -> Option<String> {
        match stmt {
            syn::Stmt::Expr(expr, None) => self.expr_canonical_flow_run_apply_command_var(expr),
            syn::Stmt::Local(_) | syn::Stmt::Expr(_, Some(_)) => None,
            syn::Stmt::Item(_) | syn::Stmt::Macro(_) => None,
        }
    }

    fn stmt_canonical_flow_run_apply_identity_var(&self, stmt: &syn::Stmt) -> Option<String> {
        match stmt {
            syn::Stmt::Expr(expr, None) => self.expr_canonical_flow_run_apply_identity_var(expr),
            syn::Stmt::Local(_) | syn::Stmt::Expr(_, Some(_)) => None,
            syn::Stmt::Item(_) | syn::Stmt::Macro(_) => None,
        }
    }

    fn flow_runtime_cas_receiver_allowed(&self, receiver: &syn::Expr) -> bool {
        match self.rel {
            "meerkat-mob/src/runtime/flow.rs" | "meerkat-mob/src/runtime/builder.rs" => {
                expr_is_self_run_store_receiver(receiver)
                    || self.expr_is_visible_mob_run_store_parameter_receiver(receiver)
            }
            _ => false,
        }
    }

    fn expr_is_visible_mob_run_store_parameter_receiver(&self, receiver: &syn::Expr) -> bool {
        match receiver {
            syn::Expr::Path(_) | syn::Expr::Paren(_) | syn::Expr::Group(_) => {
                expr_path_single_ident(receiver)
                    .as_ref()
                    .is_some_and(|name| self.visible_parameter_names("MobRunStore").contains(name))
            }
            syn::Expr::Reference(reference) => {
                self.expr_is_visible_mob_run_store_parameter_receiver(&reference.expr)
            }
            syn::Expr::Unary(unary) if matches!(unary.op, syn::UnOp::Deref(_)) => {
                self.expr_is_visible_mob_run_store_parameter_receiver(&unary.expr)
            }
            _ => false,
        }
    }

    fn projection_commit_allowed(&self, node: &syn::ExprMethodCall) -> bool {
        if self.current_function().is_none() {
            return false;
        }
        if self.deferred_expr_depth > 0 {
            return false;
        }
        let method = node.method.to_string();
        let Some(cas_facts) = self.facts.cas_methods.get(&method) else {
            return false;
        };
        match self.rel {
            "meerkat-mob/src/runtime/flow.rs" | "meerkat-mob/src/runtime/builder.rs" => {
                if cas_facts.authority_input_arg_indices.is_empty()
                    || !self.flow_runtime_cas_receiver_allowed(&node.receiver)
                {
                    return false;
                }
                let typed_identity_var = method_call_typed_next_state_identity_var(
                    node,
                    cas_facts,
                    &self.visible_typed_flow_outcome_identity_vars(),
                    &self.visible_typed_flow_next_state_identity_vars(),
                );
                method_call_typed_next_state_command_var(
                    node,
                    cas_facts,
                    &self.visible_typed_flow_outcome_command_vars(),
                    &self.visible_typed_flow_next_state_command_vars(),
                )
                .as_ref()
                .is_some_and(|command| {
                    method_call_uses_matching_typed_flow_identity(
                        node,
                        cas_facts,
                        typed_identity_var.as_deref(),
                    ) && method_call_uses_matching_flow_authority_input(
                        node,
                        cas_facts,
                        command,
                        &self.visible_flow_authority_input_command_vars(),
                        &self.visible_flow_authority_input_identity_vars(),
                        &self.visible_flow_run_command_vars(),
                    )
                })
            }
            "meerkat-mob/src/runtime/actor.rs" => {
                if cas_facts.authority_input_arg_indices.is_empty() {
                    return false;
                }
                self.actor_store_plan_commit_allowed(node, cas_facts)
            }
            _ => false,
        }
    }

    fn projection_call_allowed(&self, node: &syn::ExprCall) -> bool {
        if self.current_function().is_none() {
            return false;
        }
        if self.deferred_expr_depth > 0 {
            return false;
        }
        let Some(path) = expr_as_path(&node.func) else {
            return false;
        };
        let Some(method) = path
            .segments
            .last()
            .map(|segment| segment.ident.to_string())
        else {
            return false;
        };
        let Some(cas_facts) = self.facts.cas_methods.get(&method) else {
            return false;
        };
        if !path_is_canonical_mob_run_store_cas_call(path, self.current_aliases(), &method) {
            return false;
        }
        if cas_facts.authority_input_arg_indices.is_empty() {
            return false;
        }
        if path.segments.len() > 1
            && !node
                .args
                .first()
                .is_some_and(|receiver| self.flow_runtime_cas_receiver_allowed(receiver))
        {
            return false;
        }
        matches!(self.rel, "meerkat-mob/src/runtime/flow.rs")
            && self.has_visible_parameter_type("MobMachineFlowRunCommand")
            && self.has_visible_parameter_type("MobMachineFlowAuthorityToken")
            && {
                let typed_identity_var = call_typed_next_state_identity_var(
                    node,
                    path,
                    cas_facts,
                    &self.visible_typed_flow_outcome_identity_vars(),
                    &self.visible_typed_flow_next_state_identity_vars(),
                );
                call_typed_next_state_command_var(
                    node,
                    path,
                    cas_facts,
                    &self.visible_typed_flow_outcome_command_vars(),
                    &self.visible_typed_flow_next_state_command_vars(),
                )
                .as_ref()
                .is_some_and(|command| {
                    call_uses_matching_typed_flow_identity(
                        node,
                        path,
                        cas_facts,
                        typed_identity_var.as_deref(),
                    ) && call_uses_matching_flow_authority_input(
                        node,
                        path,
                        cas_facts,
                        command,
                        &self.visible_flow_authority_input_command_vars(),
                        &self.visible_flow_authority_input_identity_vars(),
                        &self.visible_flow_run_command_vars(),
                    )
                })
            }
    }

    fn direct_use_from_path(&self, path: &syn::Path) -> Option<FlowDirectUse> {
        let shadowed_root = self.current_aliases().path_has_shadowed_bare_root(path);
        self.current_aliases()
            .resolve_path(path)
            .into_iter()
            .find_map(|segments| {
                FlowReducerFamily::from_transition_path(&segments)
                    .map(|(family, canonical)| FlowDirectUse::Transition {
                        family,
                        canonical: canonical && !shadowed_root,
                    })
                    .or_else(|| {
                        FlowReducerFamily::from_input_path(&segments).map(|(family, canonical)| {
                            FlowDirectUse::Input {
                                family,
                                canonical: canonical && !shadowed_root,
                            }
                        })
                    })
            })
    }

    fn projection_field_token(&self, expr: &syn::Expr) -> Option<String> {
        let chain = expr_field_chain(expr);
        projection_field_token_from_chain(&chain, "flow_state", &self.facts.flow_state_fields)
            .or_else(|| {
                projection_field_token_from_chain(
                    &chain,
                    "kernel_state",
                    &self.facts.frame_state_fields,
                )
            })
            .or_else(|| {
                let alias = chain.first()?;
                let field = chain.get(1)?;
                match self.visible_projection_state_alias(alias) {
                    Some("flow_state") if self.facts.flow_state_fields.contains(field) => {
                        Some(format!(".flow_state.{field}"))
                    }
                    Some("kernel_state") if self.facts.frame_state_fields.contains(field) => {
                        Some(format!(".kernel_state.{field}"))
                    }
                    _ => None,
                }
            })
    }

    fn projection_state_token(&self, expr: &syn::Expr) -> Option<String> {
        let chain = expr_field_chain(expr);
        projection_state_token_from_chain(&chain)
            .map(ToOwned::to_owned)
            .or_else(|| {
                if chain.len() == 1 {
                    chain
                        .first()
                        .and_then(|alias| self.visible_projection_state_alias(alias))
                        .map(|state| format!(".{state}"))
                } else {
                    None
                }
            })
    }

    fn projection_write_token(&self, expr: &syn::Expr) -> Option<String> {
        self.projection_field_token(expr).or_else(|| {
            self.whole_projection_state_write_forbidden()
                .then(|| self.projection_state_token(expr))
                .flatten()
        })
    }

    fn whole_projection_state_write_forbidden(&self) -> bool {
        self.rel.starts_with("meerkat-mob/src/runtime/")
    }

    fn projection_state_field_from_struct_pattern(
        &self,
        pat_struct: &syn::PatStruct,
        field: &str,
    ) -> Option<&'static str> {
        let resolved_paths = self.current_aliases().resolve_path(&pat_struct.path);
        match field {
            "flow_state"
                if resolved_paths
                    .iter()
                    .any(|path| path_is_canonical_mob_run_projection_owner(path)) =>
            {
                Some("flow_state")
            }
            "kernel_state"
                if resolved_paths
                    .iter()
                    .any(|path| path_is_canonical_kernel_snapshot_projection_owner(path)) =>
            {
                Some("kernel_state")
            }
            _ => None,
        }
    }

    fn projection_state_alias_from_expr(&self, expr: &syn::Expr) -> Option<String> {
        if let Some(field) = projection_state_alias_from_expr(expr) {
            return Some(field.to_string());
        }
        let chain = expr_field_chain(expr);
        if chain.len() == 1 {
            chain
                .first()
                .and_then(|alias| self.visible_projection_state_alias(alias))
                .map(ToOwned::to_owned)
        } else {
            None
        }
    }

    fn collect_projection_alias_bindings(
        &self,
        pat: &syn::Pat,
        expr: &syn::Expr,
        bindings: &mut BTreeMap<String, Option<String>>,
    ) {
        match (pat, expr) {
            (syn::Pat::Ident(ident), expr) => {
                bindings.insert(
                    ident.ident.to_string(),
                    self.projection_state_alias_from_expr(expr),
                );
            }
            (syn::Pat::Tuple(pat_tuple), syn::Expr::Tuple(expr_tuple)) => {
                for (pat, expr) in pat_tuple.elems.iter().zip(&expr_tuple.elems) {
                    self.collect_projection_alias_bindings(pat, expr, bindings);
                }
            }
            (syn::Pat::TupleStruct(pat_tuple), syn::Expr::Tuple(expr_tuple)) => {
                for (pat, expr) in pat_tuple.elems.iter().zip(&expr_tuple.elems) {
                    self.collect_projection_alias_bindings(pat, expr, bindings);
                }
            }
            (syn::Pat::Struct(pat_struct), syn::Expr::Struct(expr_struct)) => {
                let expr_fields = expr_struct
                    .fields
                    .iter()
                    .map(|field| (member_key(&field.member), &field.expr))
                    .collect::<BTreeMap<_, _>>();
                for field in &pat_struct.fields {
                    let key = member_key(&field.member);
                    if let Some(expr) = expr_fields.get(&key) {
                        self.collect_projection_alias_bindings(&field.pat, expr, bindings);
                    }
                }
            }
            (syn::Pat::Struct(pat_struct), _) => {
                for field in &pat_struct.fields {
                    let key = member_key(&field.member);
                    let Some(state_field) =
                        self.projection_state_field_from_struct_pattern(pat_struct, &key)
                    else {
                        continue;
                    };
                    for name in pat_bound_idents(&field.pat) {
                        bindings.insert(name, Some(state_field.to_string()));
                    }
                }
            }
            (syn::Pat::Slice(pat_slice), syn::Expr::Array(expr_array)) => {
                for (pat, expr) in pat_slice.elems.iter().zip(&expr_array.elems) {
                    self.collect_projection_alias_bindings(pat, expr, bindings);
                }
            }
            (syn::Pat::Paren(paren), expr) => {
                self.collect_projection_alias_bindings(&paren.pat, expr, bindings);
            }
            (syn::Pat::Reference(reference), expr) => {
                self.collect_projection_alias_bindings(&reference.pat, expr, bindings);
            }
            (syn::Pat::Type(ty), expr) => {
                self.collect_projection_alias_bindings(&ty.pat, expr, bindings);
            }
            (pat, syn::Expr::Reference(reference)) => {
                self.collect_projection_alias_bindings(pat, &reference.expr, bindings);
            }
            (pat, syn::Expr::Paren(paren)) => {
                self.collect_projection_alias_bindings(pat, &paren.expr, bindings);
            }
            (pat, syn::Expr::Group(group)) => {
                self.collect_projection_alias_bindings(pat, &group.expr, bindings);
            }
            _ => {}
        }
    }

    fn push_pattern_scope(&mut self) {
        self.typed_flow_outcome_scope_stack.push(BTreeMap::new());
        self.typed_flow_outcome_command_scope_stack
            .push(BTreeMap::new());
        self.typed_flow_outcome_identity_scope_stack
            .push(BTreeMap::new());
        self.typed_flow_next_state_scope_stack.push(BTreeMap::new());
        self.typed_flow_next_state_command_scope_stack
            .push(BTreeMap::new());
        self.typed_flow_next_state_identity_scope_stack
            .push(BTreeMap::new());
        self.flow_run_command_scope_stack.push(BTreeMap::new());
        self.flow_authority_token_scope_stack.push(BTreeMap::new());
        self.flow_authority_input_scope_stack.push(BTreeMap::new());
        self.flow_authority_input_identity_scope_stack
            .push(BTreeMap::new());
        self.actor_plan_authority_input_scope_stack
            .push(BTreeMap::new());
        self.projection_state_alias_stack.push(BTreeMap::new());
        self.shadowed_name_scope_stack.push(BTreeSet::new());
    }

    fn pop_pattern_scope(&mut self) {
        let _ = self.shadowed_name_scope_stack.pop();
        let _ = self.projection_state_alias_stack.pop();
        let _ = self.actor_plan_authority_input_scope_stack.pop();
        let _ = self.flow_authority_input_identity_scope_stack.pop();
        let _ = self.flow_authority_input_scope_stack.pop();
        let _ = self.flow_authority_token_scope_stack.pop();
        let _ = self.flow_run_command_scope_stack.pop();
        let _ = self.typed_flow_next_state_identity_scope_stack.pop();
        let _ = self.typed_flow_next_state_command_scope_stack.pop();
        let _ = self.typed_flow_next_state_scope_stack.pop();
        let _ = self.typed_flow_outcome_identity_scope_stack.pop();
        let _ = self.typed_flow_outcome_command_scope_stack.pop();
        let _ = self.typed_flow_outcome_scope_stack.pop();
    }

    fn with_pattern_bindings<R>(
        &mut self,
        pat: &syn::Pat,
        expr: &syn::Expr,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.push_pattern_scope();
        self.bind_local_names(pat);
        let mut alias_bindings = BTreeMap::new();
        self.collect_projection_alias_bindings(pat, expr, &mut alias_bindings);
        if let Some(aliases) = self.projection_state_alias_stack.last_mut() {
            aliases.extend(alias_bindings);
        }
        let result = f(self);
        self.pop_pattern_scope();
        result
    }
}

impl<'ast> Visit<'ast> for FlowGovernanceVisitor<'_> {
    fn visit_block(&mut self, node: &'ast syn::Block) {
        let local_aliases = self.current_aliases().with_block_aliases(node);
        self.alias_stack.push(local_aliases);
        self.typed_flow_outcome_scope_stack.push(BTreeMap::new());
        self.typed_flow_outcome_command_scope_stack
            .push(BTreeMap::new());
        self.typed_flow_outcome_identity_scope_stack
            .push(BTreeMap::new());
        self.typed_flow_next_state_scope_stack.push(BTreeMap::new());
        self.typed_flow_next_state_command_scope_stack
            .push(BTreeMap::new());
        self.typed_flow_next_state_identity_scope_stack
            .push(BTreeMap::new());
        self.flow_run_command_scope_stack.push(BTreeMap::new());
        self.flow_authority_token_scope_stack.push(BTreeMap::new());
        self.flow_authority_input_scope_stack.push(BTreeMap::new());
        self.flow_authority_input_identity_scope_stack
            .push(BTreeMap::new());
        self.actor_plan_authority_input_scope_stack
            .push(BTreeMap::new());
        self.projection_state_alias_stack.push(BTreeMap::new());
        self.shadowed_name_scope_stack
            .push(block_item_function_names(node));
        for stmt in &node.stmts {
            self.visit_stmt(stmt);
        }
        let _ = self.shadowed_name_scope_stack.pop();
        let _ = self.projection_state_alias_stack.pop();
        let _ = self.actor_plan_authority_input_scope_stack.pop();
        let _ = self.flow_authority_input_identity_scope_stack.pop();
        let _ = self.flow_authority_input_scope_stack.pop();
        let _ = self.flow_authority_token_scope_stack.pop();
        let _ = self.flow_run_command_scope_stack.pop();
        let _ = self.typed_flow_next_state_identity_scope_stack.pop();
        let _ = self.typed_flow_next_state_command_scope_stack.pop();
        let _ = self.typed_flow_next_state_scope_stack.pop();
        let _ = self.typed_flow_outcome_identity_scope_stack.pop();
        let _ = self.typed_flow_outcome_command_scope_stack.pop();
        let _ = self.typed_flow_outcome_scope_stack.pop();
        let _ = self.alias_stack.pop();
    }

    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        if attrs_cfg_test(&node.attrs) {
            return;
        }
        if let Some((_, items)) = &node.content {
            let local_aliases = RustUseAliases::default().with_item_aliases(items);
            self.alias_stack.push(local_aliases);
            self.module_depth += 1;
            visit::visit_item_mod(self, node);
            self.module_depth -= 1;
            let _ = self.alias_stack.pop();
        } else {
            visit::visit_item_mod(self, node);
        }
    }

    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        self.impl_stack.push(type_path_segments(&node.self_ty));
        visit::visit_item_impl(self, node);
        let _ = self.impl_stack.pop();
    }

    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        if attrs_cfg_test(&node.attrs) {
            return;
        }
        let local_aliases = self.current_aliases().with_block_aliases(&node.block);
        let context = FlowFunctionContext::from_signature(
            &node.sig,
            &node.block,
            None,
            self.module_depth,
            self.rel,
            &local_aliases,
        );
        self.alias_stack.push(local_aliases);
        self.function_stack.push(context);
        visit::visit_item_fn(self, node);
        let _ = self.function_stack.pop();
        let _ = self.alias_stack.pop();
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        if attrs_cfg_test(&node.attrs) {
            return;
        }
        let local_aliases = self.current_aliases().with_block_aliases(&node.block);
        let context = FlowFunctionContext::from_signature(
            &node.sig,
            &node.block,
            self.impl_stack.last().cloned().flatten(),
            self.module_depth,
            self.rel,
            &local_aliases,
        );
        self.alias_stack.push(local_aliases);
        self.function_stack.push(context);
        visit::visit_impl_item_fn(self, node);
        let _ = self.function_stack.pop();
        let _ = self.alias_stack.pop();
    }

    fn visit_local(&mut self, node: &'ast syn::Local) {
        if let Some(init) = &node.init {
            let plan_match_result_var = match (&node.pat, &*init.expr) {
                (syn::Pat::Ident(ident), syn::Expr::Match(expr_match))
                    if self.rel == "meerkat-mob/src/runtime/actor.rs"
                        && self.current_function().is_some_and(|function| {
                            function.name == "commit_flow_frame_store_plan_in_actor"
                        })
                        && self.expr_is_actor_plan_scrutinee(&expr_match.expr) =>
                {
                    Some(ident.ident.to_string())
                }
                _ => None,
            };
            self.actor_plan_match_result_stack
                .push(plan_match_result_var);
            self.visit_expr(&init.expr);
            if let Some((_, diverge)) = &init.diverge {
                self.visit_expr(diverge);
            }
            let _ = self.actor_plan_match_result_stack.pop();
        }
        let bound_names = pat_bound_idents(&node.pat);
        let simple_initialized_ident =
            matches!(&node.pat, syn::Pat::Ident(_)) && node.init.is_some();
        if !simple_initialized_ident {
            for name in &bound_names {
                self.invalidate_flow_provenance_var(name);
            }
        }
        let mut alias_bindings = BTreeMap::new();
        if let Some(init) = &node.init {
            self.collect_projection_alias_bindings(&node.pat, &init.expr, &mut alias_bindings);
        }
        if let Some(aliases) = self.projection_state_alias_stack.last_mut() {
            aliases.extend(alias_bindings);
        }
        if let syn::Pat::Ident(ident) = &node.pat
            && let Some(init) = &node.init
        {
            let typed_outcome_command_var =
                self.expr_canonical_flow_run_apply_command_var(&init.expr);
            let typed_outcome_identity_var =
                self.expr_canonical_flow_run_apply_identity_var(&init.expr);
            let name = ident.ident.to_string();
            let typed_next_state_command_var =
                self.expr_canonical_flow_run_apply_next_state_command_var(&init.expr);
            let typed_next_state_identity_var =
                self.expr_canonical_flow_run_apply_next_state_identity_var(&init.expr);
            let is_flow_run_command =
                expr_returns_mob_machine_flow_run_command(&init.expr, self.current_aliases());
            let is_flow_authority_token =
                expr_returns_mob_machine_flow_authority_token(&init.expr, self.current_aliases());
            let visible_flow_authority_input_command_vars =
                self.visible_flow_authority_input_command_vars();
            let visible_flow_authority_input_identity_vars =
                self.visible_flow_authority_input_identity_vars();
            let flow_authority_input_command_var = expr_flow_authority_input_command_var(
                &init.expr,
                &self.visible_flow_run_command_vars(),
                &visible_flow_authority_input_command_vars,
            );
            let flow_authority_input_identity_var = expr_flow_authority_input_identity_var(
                &init.expr,
                &visible_flow_authority_input_identity_vars,
            );
            let is_actor_plan_authority_input =
                self.expr_is_actor_plan_machine_inputs_value(&init.expr, &BTreeSet::new());
            if let Some(scope) = self.typed_flow_outcome_scope_stack.last_mut() {
                scope.insert(name.clone(), typed_outcome_command_var.is_some());
            }
            if let Some(scope) = self.typed_flow_outcome_command_scope_stack.last_mut() {
                scope.insert(name.clone(), typed_outcome_command_var);
            }
            if let Some(scope) = self.typed_flow_outcome_identity_scope_stack.last_mut() {
                scope.insert(name.clone(), typed_outcome_identity_var);
            }
            if let Some(scope) = self.typed_flow_next_state_scope_stack.last_mut() {
                scope.insert(name.clone(), typed_next_state_command_var.is_some());
            }
            if let Some(scope) = self.typed_flow_next_state_command_scope_stack.last_mut() {
                scope.insert(name.clone(), typed_next_state_command_var);
            }
            if let Some(scope) = self.typed_flow_next_state_identity_scope_stack.last_mut() {
                scope.insert(name.clone(), typed_next_state_identity_var);
            }
            if let Some(scope) = self.flow_run_command_scope_stack.last_mut() {
                scope.insert(name.clone(), is_flow_run_command);
            }
            if let Some(scope) = self.flow_authority_token_scope_stack.last_mut() {
                scope.insert(name.clone(), is_flow_authority_token);
            }
            if let Some(scope) = self.flow_authority_input_scope_stack.last_mut() {
                scope.insert(name.clone(), flow_authority_input_command_var);
            }
            if let Some(scope) = self.flow_authority_input_identity_scope_stack.last_mut() {
                scope.insert(name.clone(), flow_authority_input_identity_var);
            }
            if let Some(scope) = self.actor_plan_authority_input_scope_stack.last_mut() {
                scope.insert(name, is_actor_plan_authority_input);
            }
        }
        self.bind_local_names(&node.pat);
    }

    fn visit_expr_closure(&mut self, node: &'ast syn::ExprClosure) {
        self.with_deferred_expr(|this| visit::visit_expr_closure(this, node));
    }

    fn visit_expr_async(&mut self, node: &'ast syn::ExprAsync) {
        self.with_deferred_expr(|this| visit::visit_expr_async(this, node));
    }

    fn visit_expr_match(&mut self, node: &'ast syn::ExprMatch) {
        let is_actor_plan_match = self.rel == "meerkat-mob/src/runtime/actor.rs"
            && self
                .current_function()
                .is_some_and(|function| function.name == "commit_flow_frame_store_plan_in_actor")
            && self.expr_is_actor_plan_scrutinee(&node.expr);
        if is_actor_plan_match {
            self.visit_expr(&node.expr);
            for arm in &node.arms {
                if let Some((_, guard)) = &arm.guard {
                    self.visit_expr(guard);
                }
                let plan_arm = actor_plan_arm_facts_from_pat(&arm.pat, self.current_aliases());
                let plan_variant = actor_plan_single_variant(&plan_arm);
                self.actor_plan_binding_scope_stack.push(plan_arm);
                let result_proof = self.visit_actor_plan_result_expr(&arm.body);
                let _ = self.actor_plan_binding_scope_stack.pop();
                if let Some(variant) = plan_variant
                    && !result_proof.proves_authorized_commit()
                {
                    self.mismatches.push(format!(
                        "FlowFrameLoopStorePlan::{variant} arm does not use canonical MobRunStore CAS: {}:{}",
                        self.rel,
                        line_number(&arm.pat)
                    ));
                }
            }
        } else {
            self.visit_expr(&node.expr);
            for arm in &node.arms {
                self.with_pattern_bindings(&arm.pat, &node.expr, |this| {
                    if let Some((_, guard)) = &arm.guard {
                        this.visit_expr(guard);
                    }
                    this.visit_expr(&arm.body);
                });
            }
        }
    }

    fn visit_expr_if(&mut self, node: &'ast syn::ExprIf) {
        self.visit_expr(&node.cond);
        if let syn::Expr::Let(expr_let) = &*node.cond {
            self.with_pattern_bindings(&expr_let.pat, &expr_let.expr, |this| {
                this.visit_block(&node.then_branch);
            });
        } else {
            self.visit_block(&node.then_branch);
        }
        if let Some((_, else_branch)) = &node.else_branch {
            self.visit_expr(else_branch);
        }
    }

    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if let Some(path) = expr_as_path(&node.func) {
            if let Some(kind) = self.direct_use_from_path(path) {
                self.record_direct_use(
                    kind,
                    line_number(node),
                    source_position(node),
                    path,
                    Some(node),
                );
            }
            if let Some(method) = path
                .segments
                .last()
                .map(|segment| segment.ident.to_string())
                && self.facts.cas_methods.contains_key(&method)
                && !self.projection_call_allowed(node)
            {
                self.mismatches.push(format!(
                    "direct live-flow projection mutation `.{method}(` is not MobMachine-command gated: {}:{}",
                    self.rel,
                    line_number(node)
                ));
            }
        } else {
            self.visit_expr(&node.func);
        }
        for arg in &node.args {
            self.visit_expr(arg);
        }
    }

    fn visit_expr_struct(&mut self, node: &'ast syn::ExprStruct) {
        if let Some(kind) = self.direct_use_from_path(&node.path) {
            self.record_direct_use(
                kind,
                line_number(node),
                source_position(node),
                &node.path,
                None,
            );
        }
        visit::visit_expr_struct(self, node);
    }

    fn visit_expr_path(&mut self, node: &'ast syn::ExprPath) {
        if let Some(kind) = self.direct_use_from_path(&node.path) {
            self.record_direct_use(
                kind,
                line_number(node),
                source_position(node),
                &node.path,
                None,
            );
        }
    }

    fn visit_expr_assign(&mut self, node: &'ast syn::ExprAssign) {
        if let Some(token) = self.projection_write_token(&node.left) {
            self.mismatches.push(format!(
                "direct live-flow projection mutation `{token}` is not MobMachine-command gated: {}:{}",
                self.rel,
                line_number(node)
            ));
        }
        if let Some(name) = expr_path_single_ident(&node.left) {
            let typed_outcome_command_var =
                self.expr_canonical_flow_run_apply_command_var(&node.right);
            let typed_outcome_identity_var =
                self.expr_canonical_flow_run_apply_identity_var(&node.right);
            self.set_typed_flow_outcome_var(&name, typed_outcome_command_var.is_some());
            self.set_typed_flow_outcome_command_var(&name, typed_outcome_command_var);
            self.set_typed_flow_outcome_identity_var(&name, typed_outcome_identity_var);
            let typed_next_state_command_var =
                self.expr_canonical_flow_run_apply_next_state_command_var(&node.right);
            let typed_next_state_identity_var =
                self.expr_canonical_flow_run_apply_next_state_identity_var(&node.right);
            self.set_typed_flow_next_state_var(&name, typed_next_state_command_var.is_some());
            self.set_typed_flow_next_state_command_var(&name, typed_next_state_command_var);
            self.set_typed_flow_next_state_identity_var(&name, typed_next_state_identity_var);
            let visible_flow_authority_input_command_vars =
                self.visible_flow_authority_input_command_vars();
            let visible_flow_authority_input_identity_vars =
                self.visible_flow_authority_input_identity_vars();
            let flow_authority_input_command_var = expr_flow_authority_input_command_var(
                &node.right,
                &self.visible_flow_run_command_vars(),
                &visible_flow_authority_input_command_vars,
            );
            let flow_authority_input_identity_var = expr_flow_authority_input_identity_var(
                &node.right,
                &visible_flow_authority_input_identity_vars,
            );
            self.set_flow_run_command_var(
                &name,
                expr_returns_mob_machine_flow_run_command(&node.right, self.current_aliases()),
            );
            self.set_flow_authority_token_var(
                &name,
                expr_returns_mob_machine_flow_authority_token(&node.right, self.current_aliases()),
            );
            self.set_flow_authority_input_var(&name, flow_authority_input_command_var);
            self.set_flow_authority_input_identity_var(&name, flow_authority_input_identity_var);
            self.set_actor_plan_authority_input_var(
                &name,
                self.expr_is_actor_plan_machine_inputs_value(
                    &node.right,
                    &self.visible_actor_plan_authority_input_vars(),
                ),
            );
            let alias = self.projection_state_alias_from_expr(&node.right);
            self.set_projection_state_alias_var(&name, alias);
        } else {
            if let Some(name) = expr_root_ident(&node.left) {
                self.invalidate_flow_provenance_var(&name);
            }
            self.invalidate_typed_flow_outcome_path(&node.left);
            self.invalidate_typed_flow_next_state_path(&node.left);
        }
        visit::visit_expr_assign(self, node);
    }

    fn visit_expr_binary(&mut self, node: &'ast syn::ExprBinary) {
        if bin_op_is_assign(&node.op)
            && let Some(token) = self.projection_write_token(&node.left)
        {
            self.mismatches.push(format!(
                "direct live-flow projection mutation `{token}` is not MobMachine-command gated: {}:{}",
                self.rel,
                line_number(node)
            ));
        }
        if bin_op_is_assign(&node.op)
            && let Some(name) = expr_path_single_ident(&node.left)
        {
            self.invalidate_flow_provenance_var(&name);
        }
        if bin_op_is_assign(&node.op) {
            if let Some(name) = expr_root_ident(&node.left) {
                self.invalidate_flow_provenance_var(&name);
            }
            self.invalidate_typed_flow_outcome_path(&node.left);
            self.invalidate_typed_flow_next_state_path(&node.left);
        }
        visit::visit_expr_binary(self, node);
    }

    fn visit_expr_reference(&mut self, node: &'ast syn::ExprReference) {
        if node.mutability.is_some() {
            if let Some(token) = self.projection_write_token(&node.expr) {
                self.mismatches.push(format!(
                    "direct live-flow projection mutation `{token}` is not MobMachine-command gated: {}:{}",
                    self.rel,
                    line_number(node)
                ));
            }
            if let Some(name) = expr_root_ident(&node.expr) {
                self.invalidate_flow_provenance_var(&name);
            }
        }
        visit::visit_expr_reference(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        let method = node.method.to_string();
        if projection_mutator_method(&method)
            && let Some(token) = self.projection_write_token(&node.receiver)
        {
            self.mismatches.push(format!(
                "direct live-flow projection mutation `{token}` is not MobMachine-command gated: {}:{}",
                self.rel,
                line_number(node)
            ));
        }
        if !projection_method_preserves_typed_flow_state(&method) {
            self.invalidate_typed_flow_outcome_path(&node.receiver);
            self.invalidate_typed_flow_next_state_path(&node.receiver);
        }
        if flow_provenance_mutator_method(&method)
            && let Some(name) = expr_root_ident(&node.receiver)
        {
            self.invalidate_flow_provenance_var(&name);
        }
        if self.facts.cas_methods.contains_key(&method) {
            if self.projection_commit_allowed(node) {
                self.mark_actor_plan_authorized_commit();
            } else {
                self.mismatches.push(format!(
                    "direct live-flow projection mutation `.{method}(` is not MobMachine-command gated: {}:{}",
                    self.rel,
                    line_number(node)
                ));
            }
        }
        visit::visit_expr_method_call(self, node);
    }
}

#[derive(Debug, Clone, Copy)]
enum FlowDirectUse {
    Transition {
        family: FlowReducerFamily,
        canonical: bool,
    },
    Input {
        family: FlowReducerFamily,
        canonical: bool,
    },
}

#[derive(Debug, Clone, Default)]
struct MobRuntimeActorAst {
    functions: BTreeMap<String, Vec<MobGateSummary>>,
    command_arms: BTreeMap<String, Vec<MobGateSummary>>,
    catch_all_arms: Vec<MobGateSummary>,
    dispatch_contexts: Vec<MobGateSummary>,
}

impl MobRuntimeActorAst {
    fn from_file(file: &syn::File) -> Self {
        let aliases = RustUseAliases::from_file(file);
        let mut visitor = MobRuntimeActorVisitor::new(&aliases);
        visitor.visit_file(file);
        Self {
            functions: visitor.functions,
            command_arms: visitor.command_arms,
            catch_all_arms: visitor.catch_all_arms,
            dispatch_contexts: visitor.dispatch_contexts,
        }
    }

    fn command_fail_closes_on(&self, command: &str, input: MobMachineInputVariant) -> bool {
        let visited = BTreeSet::new();
        let handler = format!("handle_{}", camel_to_snake(command));
        if self
            .dispatch_contexts
            .iter()
            .filter(|summary| summary.has_command_effect())
            .any(|summary| {
                let mut visited = BTreeSet::new();
                !self.summary_fail_closes_on(summary, input, &mut visited, true)
            })
        {
            return false;
        }
        if let Some(summaries) = self.command_arms.get(command) {
            return summaries.iter().all(|summary| {
                let mut visited = BTreeSet::new();
                self.summary_fail_closes_on(summary, input, &mut visited, true)
            });
        }
        let catch_all_summaries = self
            .catch_all_arms
            .iter()
            .filter(|summary| summary.has_command_effect())
            .collect::<Vec<_>>();
        if !catch_all_summaries.is_empty() {
            return catch_all_summaries.iter().all(|summary| {
                let mut visited = BTreeSet::new();
                self.summary_fail_closes_on(summary, input, &mut visited, true)
            });
        }
        self.functions.get(&handler).is_some_and(|summaries| {
            summaries.iter().all(|summary| {
                let mut branch_visited = visited.clone();
                self.summary_fail_closes_on(summary, input, &mut branch_visited, false)
            })
        })
    }

    fn summary_fail_closes_on(
        &self,
        summary: &MobGateSummary,
        input: MobMachineInputVariant,
        visited: &mut BTreeSet<String>,
        command_arm: bool,
    ) -> bool {
        if self
            .summary_gate_eval(summary, input, visited, false)
            .is_some_and(|eval| eval.fail_closes)
        {
            return true;
        }
        summary_direct_fail_closes_on(summary, input, command_arm)
    }

    fn summary_gate_eval(
        &self,
        summary: &MobGateSummary,
        input: MobMachineInputVariant,
        visited: &mut BTreeSet<String>,
        inherited_gate: bool,
    ) -> Option<MobGateEval> {
        let mut eval = MobGateEval {
            gated: inherited_gate,
            ..MobGateEval::default()
        };
        for event in &summary.events {
            match event {
                MobGateEvent::Gate(gated_input) if *gated_input == input => {
                    eval.gated = true;
                }
                MobGateEvent::SideEffect => {
                    eval.has_side_effect = true;
                    if !eval.gated {
                        eval.has_ungated_side_effect = true;
                    }
                }
                MobGateEvent::FailClosedCall(call) | MobGateEvent::PropagatedCall(call) => {
                    if !visited.insert(call.clone()) {
                        continue;
                    }
                    let Some(called) = self.functions.get(call) else {
                        continue;
                    };
                    let mut called_fail_closes = true;
                    let mut called_has_side_effect = false;
                    for summary in called {
                        let mut branch_visited = visited.clone();
                        let called_eval = self.summary_gate_eval(
                            summary,
                            input,
                            &mut branch_visited,
                            eval.gated,
                        )?;
                        if called_eval.has_ungated_side_effect {
                            return Some(MobGateEval {
                                has_side_effect: true,
                                has_ungated_side_effect: true,
                                ..eval
                            });
                        }
                        called_fail_closes &= called_eval.fail_closes;
                        called_has_side_effect |= called_eval.has_side_effect;
                    }
                    if called_has_side_effect {
                        eval.has_side_effect = true;
                    }
                    if called_fail_closes {
                        eval.gated = true;
                    }
                }
                MobGateEvent::HelperCall { call, gated_inputs } => {
                    let mut helper_visited = visited.clone();
                    if !helper_visited.insert(call.clone()) {
                        continue;
                    }
                    let Some(called) = self.functions.get(call) else {
                        continue;
                    };
                    for summary in called {
                        let mut branch_visited = helper_visited.clone();
                        let called_eval = self.summary_gate_eval(
                            summary,
                            input,
                            &mut branch_visited,
                            eval.gated || gated_inputs.contains(&input),
                        )?;
                        if called_eval.has_ungated_side_effect {
                            return Some(MobGateEval {
                                has_side_effect: true,
                                has_ungated_side_effect: true,
                                ..eval
                            });
                        }
                        if called_eval.has_side_effect {
                            eval.has_side_effect = true;
                        }
                    }
                }
                _ => {}
            }
        }
        if summary.has_ungated_side_effect && !inherited_gate {
            eval.has_side_effect = true;
            eval.has_ungated_side_effect = true;
        }
        eval.fail_closes = (eval.gated
            || (eval.has_side_effect && summary.reply_gated_inputs.contains(&input)))
            && !eval.has_ungated_side_effect;
        Some(eval)
    }
}

fn summary_direct_fail_closes_on(
    summary: &MobGateSummary,
    input: MobMachineInputVariant,
    command_arm: bool,
) -> bool {
    if summary.has_ungated_side_effect {
        return false;
    }
    if !summary.has_side_effect
        && summary.gated_inputs.contains(&input)
        && summary.fail_closed_calls.is_empty()
        && summary.propagated_calls.is_empty()
    {
        return true;
    }
    if summary.has_side_effect && command_arm {
        summary.side_effect_gated_inputs.contains(&input)
            && (summary.reply_gated_inputs.contains(&input)
                || summary.gated_inputs.contains(&input))
    } else {
        summary.has_side_effect && summary.side_effect_gated_inputs.contains(&input)
    }
}

#[derive(Debug, Clone, Default)]
struct MobGateEval {
    gated: bool,
    has_side_effect: bool,
    has_ungated_side_effect: bool,
    fail_closes: bool,
}

#[derive(Debug, Clone, Default)]
struct MobGateSummary {
    gated_inputs: BTreeSet<MobMachineInputVariant>,
    side_effect_gated_inputs: BTreeSet<MobMachineInputVariant>,
    reply_gated_inputs: BTreeSet<MobMachineInputVariant>,
    has_side_effect: bool,
    has_ungated_side_effect: bool,
    fail_closed_calls: BTreeSet<String>,
    propagated_calls: BTreeSet<String>,
    events: Vec<MobGateEvent>,
}

impl MobGateSummary {
    fn has_command_effect(&self) -> bool {
        self.has_side_effect
            || self.has_ungated_side_effect
            || !self.side_effect_gated_inputs.is_empty()
            || !self.fail_closed_calls.is_empty()
            || !self.propagated_calls.is_empty()
            || self
                .events
                .iter()
                .any(|event| matches!(event, MobGateEvent::HelperCall { .. }))
    }
}

#[derive(Debug, Clone)]
enum MobGateEvent {
    Gate(MobMachineInputVariant),
    SideEffect,
    FailClosedCall(String),
    HelperCall {
        call: String,
        gated_inputs: BTreeSet<MobMachineInputVariant>,
    },
    PropagatedCall(String),
}

fn retain_loop_preserved_gate_map<T: Clone + Eq>(
    facts: &mut BTreeMap<String, T>,
    before: &BTreeMap<String, T>,
) {
    let after = facts.clone();
    facts.clear();
    for (name, value) in before {
        if after
            .get(name)
            .is_some_and(|after_value| after_value == value)
        {
            facts.insert(name.clone(), value.clone());
        }
    }
}

fn retain_loop_preserved_gate_set(facts: &mut BTreeSet<String>, before: &BTreeSet<String>) {
    facts.retain(|name| before.contains(name));
}

struct MobRuntimeActorVisitor<'a> {
    aliases: &'a RustUseAliases,
    alias_stack: Vec<RustUseAliases>,
    functions: BTreeMap<String, Vec<MobGateSummary>>,
    command_arms: BTreeMap<String, Vec<MobGateSummary>>,
    catch_all_arms: Vec<MobGateSummary>,
    dispatch_contexts: Vec<MobGateSummary>,
    mob_command_value_scope_stack: Vec<BTreeMap<String, bool>>,
    mob_command_source_scope_stack: Vec<BTreeMap<String, bool>>,
    mob_command_buffer_scope_stack: Vec<BTreeMap<String, bool>>,
    mob_command_dispatch_scope_stack: Vec<bool>,
    impl_self_type_stack: Vec<Option<Vec<String>>>,
}

impl<'a> MobRuntimeActorVisitor<'a> {
    fn new(aliases: &'a RustUseAliases) -> Self {
        Self {
            aliases,
            alias_stack: Vec::new(),
            functions: BTreeMap::new(),
            command_arms: BTreeMap::new(),
            catch_all_arms: Vec::new(),
            dispatch_contexts: Vec::new(),
            mob_command_value_scope_stack: Vec::new(),
            mob_command_source_scope_stack: Vec::new(),
            mob_command_buffer_scope_stack: Vec::new(),
            mob_command_dispatch_scope_stack: Vec::new(),
            impl_self_type_stack: Vec::new(),
        }
    }

    fn current_aliases(&self) -> &RustUseAliases {
        self.alias_stack.last().unwrap_or(self.aliases)
    }

    fn visible_scope_names(stack: &[BTreeMap<String, bool>]) -> BTreeSet<String> {
        let mut resolved = BTreeMap::new();
        for scope in stack.iter().rev() {
            for (name, visible) in scope {
                resolved.entry(name.clone()).or_insert(*visible);
            }
        }
        resolved
            .into_iter()
            .filter_map(|(name, visible)| visible.then_some(name))
            .collect()
    }

    fn visible_mob_command_values(&self) -> BTreeSet<String> {
        Self::visible_scope_names(&self.mob_command_value_scope_stack)
    }

    fn visible_mob_command_sources(&self) -> BTreeSet<String> {
        Self::visible_scope_names(&self.mob_command_source_scope_stack)
    }

    fn visible_mob_command_buffers(&self) -> BTreeSet<String> {
        Self::visible_scope_names(&self.mob_command_buffer_scope_stack)
    }

    fn expr_is_visible_mob_command_value(&self, expr: &syn::Expr) -> bool {
        expr_path_single_ident(expr)
            .as_ref()
            .is_some_and(|ident| self.visible_mob_command_values().contains(ident))
    }

    fn set_mob_command_value_var(&mut self, name: &str, visible: bool) {
        for scope in self.mob_command_value_scope_stack.iter_mut().rev() {
            if scope.contains_key(name) {
                scope.insert(name.to_string(), visible);
                return;
            }
        }
        if let Some(scope) = self.mob_command_value_scope_stack.last_mut() {
            scope.insert(name.to_string(), visible);
        }
    }

    fn set_mob_command_source_var(&mut self, name: &str, visible: bool) {
        for scope in self.mob_command_source_scope_stack.iter_mut().rev() {
            if scope.contains_key(name) {
                scope.insert(name.to_string(), visible);
                return;
            }
        }
        if let Some(scope) = self.mob_command_source_scope_stack.last_mut() {
            scope.insert(name.to_string(), visible);
        }
    }

    fn set_mob_command_buffer_var(&mut self, name: &str, visible: bool) {
        for scope in self.mob_command_buffer_scope_stack.iter_mut().rev() {
            if scope.contains_key(name) {
                scope.insert(name.to_string(), visible);
                return;
            }
        }
        if let Some(scope) = self.mob_command_buffer_scope_stack.last_mut() {
            scope.insert(name.to_string(), visible);
        }
    }

    fn invalidate_mob_command_provenance_var(&mut self, name: &str) {
        self.set_mob_command_value_var(name, false);
        self.set_mob_command_source_var(name, false);
        self.set_mob_command_buffer_var(name, false);
    }

    fn expr_is_mob_command_scrutinee(&self, expr: &syn::Expr) -> bool {
        if !self
            .mob_command_dispatch_scope_stack
            .last()
            .copied()
            .unwrap_or(false)
        {
            return false;
        }
        expr_path_single_ident(expr)
            .as_ref()
            .is_some_and(|ident| self.visible_mob_command_values().contains(ident))
    }

    fn expr_uses_mob_command_source(&self, expr: &syn::Expr) -> bool {
        match expr {
            syn::Expr::MethodCall(call) => {
                let receiver = expr_path_single_ident(&call.receiver);
                (call.method == "recv"
                    && receiver
                        .as_ref()
                        .is_some_and(|ident| self.visible_mob_command_sources().contains(ident)))
                    || (call.method == "pop_front"
                        && receiver.as_ref().is_some_and(|ident| {
                            self.visible_mob_command_buffers().contains(ident)
                        }))
                    || self.expr_uses_mob_command_source(&call.receiver)
                    || call
                        .args
                        .iter()
                        .any(|arg| self.expr_uses_mob_command_source(arg))
            }
            syn::Expr::Call(call) => call
                .args
                .iter()
                .any(|arg| self.expr_uses_mob_command_source(arg)),
            syn::Expr::If(expr_if) => {
                self.expr_uses_mob_command_source(&expr_if.cond)
                    || expr_if
                        .then_branch
                        .stmts
                        .iter()
                        .any(|stmt| self.stmt_uses_mob_command_source(stmt))
                    || expr_if
                        .else_branch
                        .as_ref()
                        .is_some_and(|(_, expr)| self.expr_uses_mob_command_source(expr))
            }
            syn::Expr::Let(expr_let) => self.expr_uses_mob_command_source(&expr_let.expr),
            syn::Expr::Block(block) => block
                .block
                .stmts
                .iter()
                .any(|stmt| self.stmt_uses_mob_command_source(stmt)),
            syn::Expr::Try(try_expr) => self.expr_uses_mob_command_source(&try_expr.expr),
            syn::Expr::Await(await_expr) => self.expr_uses_mob_command_source(&await_expr.base),
            syn::Expr::Reference(reference) => self.expr_uses_mob_command_source(&reference.expr),
            syn::Expr::Paren(paren) => self.expr_uses_mob_command_source(&paren.expr),
            syn::Expr::Group(group) => self.expr_uses_mob_command_source(&group.expr),
            _ => false,
        }
    }

    fn stmt_uses_mob_command_source(&self, stmt: &syn::Stmt) -> bool {
        match stmt {
            syn::Stmt::Local(local) => local
                .init
                .as_ref()
                .is_some_and(|init| self.expr_uses_mob_command_source(&init.expr)),
            syn::Stmt::Expr(expr, _) => self.expr_uses_mob_command_source(expr),
            syn::Stmt::Item(_) | syn::Stmt::Macro(_) => false,
        }
    }

    fn push_mob_command_signature_scope(&mut self, sig: &syn::Signature) {
        let mut values = BTreeMap::new();
        let mut sources = BTreeMap::new();
        let function_name = sig.ident.to_string();
        for input in &sig.inputs {
            let syn::FnArg::Typed(pat_type) = input else {
                continue;
            };
            let Some(name) = pat_ident_name(&pat_type.pat) else {
                continue;
            };
            if type_is_canonical_mob_command(&pat_type.ty, self.current_aliases()) {
                let is_dispatch_command = function_name == "dispatch" && name == "command";
                values.insert(name, is_dispatch_command);
            } else if type_contains_canonical_mob_command(&pat_type.ty, self.current_aliases()) {
                sources.insert(name, true);
            }
        }
        let command_values = values.values().any(|visible| *visible);
        let command_sources = sources.values().any(|visible| *visible);
        self.mob_command_value_scope_stack.push(values);
        self.mob_command_source_scope_stack.push(sources);
        self.mob_command_buffer_scope_stack.push(BTreeMap::new());
        self.mob_command_dispatch_scope_stack
            .push(command_sources || (command_values && function_name == "dispatch"));
    }

    fn pop_mob_command_signature_scope(&mut self) {
        let _ = self.mob_command_dispatch_scope_stack.pop();
        let _ = self.mob_command_buffer_scope_stack.pop();
        let _ = self.mob_command_source_scope_stack.pop();
        let _ = self.mob_command_value_scope_stack.pop();
    }

    fn dispatch_command_parameter_names(&self, sig: &syn::Signature) -> BTreeSet<String> {
        sig.inputs
            .iter()
            .filter_map(|input| match input {
                syn::FnArg::Typed(pat_type) => {
                    let name = pat_ident_name(&pat_type.pat)?;
                    (sig.ident == "dispatch"
                        && name == "command"
                        && type_is_canonical_mob_command(&pat_type.ty, self.current_aliases()))
                    .then_some(name)
                }
                syn::FnArg::Receiver(_) => None,
            })
            .collect()
    }

    fn dispatch_command_source_parameter_names(&self, sig: &syn::Signature) -> BTreeSet<String> {
        sig.inputs
            .iter()
            .filter_map(|input| match input {
                syn::FnArg::Typed(pat_type) => {
                    let name = pat_ident_name(&pat_type.pat)?;
                    type_contains_canonical_mob_command(&pat_type.ty, self.current_aliases())
                        .then_some(name)
                }
                syn::FnArg::Receiver(_) => None,
            })
            .collect()
    }
}

impl<'ast> Visit<'ast> for MobRuntimeActorVisitor<'_> {
    fn visit_item_mod(&mut self, _node: &'ast syn::ItemMod) {}

    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        if attrs_cfg_test(&node.attrs) {
            return;
        }
        self.impl_self_type_stack
            .push(type_path_segments(&node.self_ty));
        visit::visit_item_impl(self, node);
        let _ = self.impl_self_type_stack.pop();
    }

    fn visit_block(&mut self, node: &'ast syn::Block) {
        let local_aliases = self.current_aliases().with_block_aliases(node);
        self.alias_stack.push(local_aliases);
        self.mob_command_value_scope_stack.push(BTreeMap::new());
        self.mob_command_source_scope_stack.push(BTreeMap::new());
        self.mob_command_buffer_scope_stack.push(BTreeMap::new());
        visit::visit_block(self, node);
        let _ = self.mob_command_buffer_scope_stack.pop();
        let _ = self.mob_command_source_scope_stack.pop();
        let _ = self.mob_command_value_scope_stack.pop();
        let _ = self.alias_stack.pop();
    }

    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        if attrs_cfg_test(&node.attrs) {
            return;
        }
        let summary = MobGateAnalyzer::analyze(&node.block, self.current_aliases());
        self.functions
            .entry(node.sig.ident.to_string())
            .or_default()
            .push(summary);
        let dispatch_command_vars = self.dispatch_command_parameter_names(&node.sig);
        let dispatch_command_source_vars = self.dispatch_command_source_parameter_names(&node.sig);
        if !dispatch_command_vars.is_empty() || !dispatch_command_source_vars.is_empty() {
            self.dispatch_contexts
                .push(MobGateAnalyzer::analyze_dispatch_context(
                    &node.block,
                    self.current_aliases(),
                    dispatch_command_vars,
                    dispatch_command_source_vars,
                ));
        }
        self.push_mob_command_signature_scope(&node.sig);
        visit::visit_item_fn(self, node);
        self.pop_mob_command_signature_scope();
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        if attrs_cfg_test(&node.attrs) {
            return;
        }
        let is_mob_actor_impl = self
            .impl_self_type_stack
            .last()
            .and_then(|path| path.as_deref())
            .is_some_and(path_is_root_mob_actor_impl);
        if !is_mob_actor_impl {
            return;
        }
        let summary = MobGateAnalyzer::analyze(&node.block, self.current_aliases());
        self.functions
            .entry(node.sig.ident.to_string())
            .or_default()
            .push(summary);
        let dispatch_command_vars = self.dispatch_command_parameter_names(&node.sig);
        let dispatch_command_source_vars = self.dispatch_command_source_parameter_names(&node.sig);
        if !dispatch_command_vars.is_empty() || !dispatch_command_source_vars.is_empty() {
            self.dispatch_contexts
                .push(MobGateAnalyzer::analyze_dispatch_context(
                    &node.block,
                    self.current_aliases(),
                    dispatch_command_vars,
                    dispatch_command_source_vars,
                ));
        }
        self.push_mob_command_signature_scope(&node.sig);
        visit::visit_impl_item_fn(self, node);
        self.pop_mob_command_signature_scope();
    }

    fn visit_local(&mut self, node: &'ast syn::Local) {
        if let Some(init) = &node.init
            && let Some((_, diverge)) = &init.diverge
        {
            self.visit_expr(diverge);
        }
        let bound_names = pat_bound_idents(&node.pat);
        if let syn::Pat::Ident(ident) = &node.pat
            && let Some(init) = &node.init
        {
            let name = ident.ident.to_string();
            let is_command_value = self.expr_uses_mob_command_source(&init.expr)
                || self.expr_is_visible_mob_command_value(&init.expr);
            if let Some(scope) = self.mob_command_value_scope_stack.last_mut() {
                scope.insert(name.clone(), is_command_value);
            }
            if let Some(scope) = self.mob_command_source_scope_stack.last_mut() {
                scope.insert(name.clone(), false);
            }
            let is_command_buffer = expr_initializes_mob_command_buffer(&init.expr)
                && !self.visible_mob_command_sources().is_empty();
            if let Some(scope) = self.mob_command_buffer_scope_stack.last_mut() {
                scope.insert(name, is_command_buffer);
            }
        } else {
            for name in &bound_names {
                if let Some(scope) = self.mob_command_value_scope_stack.last_mut() {
                    scope.insert(name.clone(), false);
                }
                if let Some(scope) = self.mob_command_source_scope_stack.last_mut() {
                    scope.insert(name.clone(), false);
                }
                if let Some(scope) = self.mob_command_buffer_scope_stack.last_mut() {
                    scope.insert(name.clone(), false);
                }
            }
        }
        visit::visit_local(self, node);
    }

    fn visit_expr_match(&mut self, node: &'ast syn::ExprMatch) {
        if self.expr_is_mob_command_scrutinee(&node.expr) {
            let command_variants = canonical_gated_mob_command_variants();
            for arm in &node.arms {
                let variants = mob_command_variants_from_pat(
                    &arm.pat,
                    self.current_aliases(),
                    &command_variants,
                );
                if !variants.is_empty() {
                    let summary = if arm.guard.is_some() {
                        MobGateSummary {
                            has_ungated_side_effect: true,
                            ..MobGateSummary::default()
                        }
                    } else {
                        MobGateAnalyzer::analyze_command_arm_expr(
                            &arm.body,
                            self.current_aliases(),
                            reply_tx_vars_from_pat(&arm.pat),
                        )
                    };
                    for variant in variants {
                        self.command_arms
                            .entry(variant)
                            .or_default()
                            .push(summary.clone());
                    }
                } else if pat_is_catch_all_command_arm(&arm.pat) {
                    let summary = if arm.guard.is_some() {
                        MobGateSummary {
                            has_ungated_side_effect: true,
                            ..MobGateSummary::default()
                        }
                    } else {
                        MobGateAnalyzer::analyze_command_arm_expr(
                            &arm.body,
                            self.current_aliases(),
                            reply_tx_vars_from_pat(&arm.pat),
                        )
                    };
                    self.catch_all_arms.push(summary);
                }
            }
        }
        visit::visit_expr_match(self, node);
    }

    fn visit_expr_assign(&mut self, node: &'ast syn::ExprAssign) {
        self.visit_expr(&node.right);
        if let Some(name) = expr_path_single_ident(&node.left) {
            self.set_mob_command_value_var(
                &name,
                self.expr_uses_mob_command_source(&node.right)
                    || self.expr_is_visible_mob_command_value(&node.right),
            );
            self.set_mob_command_source_var(&name, false);
            self.set_mob_command_buffer_var(
                &name,
                expr_initializes_mob_command_buffer(&node.right)
                    && !self.visible_mob_command_sources().is_empty(),
            );
        } else if let Some(name) = expr_root_ident(&node.left) {
            self.invalidate_mob_command_provenance_var(&name);
            self.visit_expr(&node.left);
        } else {
            self.visit_expr(&node.left);
        }
    }

    fn visit_expr_binary(&mut self, node: &'ast syn::ExprBinary) {
        if bin_op_is_assign(&node.op)
            && let Some(name) = expr_root_ident(&node.left)
        {
            self.invalidate_mob_command_provenance_var(&name);
        }
        visit::visit_expr_binary(self, node);
    }

    fn visit_expr_reference(&mut self, node: &'ast syn::ExprReference) {
        if node.mutability.is_some()
            && let Some(name) = expr_root_ident(&node.expr)
        {
            self.invalidate_mob_command_provenance_var(&name);
        }
        visit::visit_expr_reference(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        let method = node.method.to_string();
        if gate_provenance_mutator_method(&method)
            && let Some(name) = expr_root_ident(&node.receiver)
        {
            self.invalidate_mob_command_provenance_var(&name);
        }
        visit::visit_expr_method_call(self, node);
    }
}

struct MobGateAnalyzer {
    aliases: RustUseAliases,
    input_vars: BTreeMap<String, MobMachineInputVariant>,
    pending_gate_result_vars: BTreeMap<String, MobMachineInputVariant>,
    self_call_result_vars: BTreeMap<String, String>,
    reply_tx_vars: BTreeSet<String>,
    skipped_command_match_vars: BTreeSet<String>,
    skipped_command_source_vars: BTreeSet<String>,
    skipped_command_buffer_vars: BTreeSet<String>,
    command_effects_require_skipped_source: bool,
    skipped_command_effects_armed: bool,
    side_effect_receiver_vars: BTreeSet<String>,
    active_gated_inputs: BTreeSet<MobMachineInputVariant>,
    statement_gated_inputs: BTreeSet<MobMachineInputVariant>,
    fail_closed_depth: usize,
    summary: MobGateSummary,
}

impl MobGateAnalyzer {
    fn analyze(block: &syn::Block, aliases: &RustUseAliases) -> MobGateSummary {
        let local_aliases = aliases.with_block_aliases(block);
        let mut analyzer = Self::new(local_aliases);
        analyzer.visit_block(block);
        analyzer.summary
    }

    fn analyze_dispatch_context(
        block: &syn::Block,
        aliases: &RustUseAliases,
        command_vars: BTreeSet<String>,
        command_source_vars: BTreeSet<String>,
    ) -> MobGateSummary {
        let local_aliases = aliases.with_block_aliases(block);
        let mut analyzer = Self::new(local_aliases);
        analyzer.command_effects_require_skipped_source =
            command_vars.is_empty() && !command_source_vars.is_empty();
        analyzer.skipped_command_effects_armed = !command_vars.is_empty();
        analyzer.skipped_command_match_vars = command_vars;
        analyzer.skipped_command_source_vars = command_source_vars;
        analyzer.visit_block(block);
        analyzer.summary
    }

    fn analyze_command_arm_expr(
        expr: &syn::Expr,
        aliases: &RustUseAliases,
        reply_tx_vars: BTreeSet<String>,
    ) -> MobGateSummary {
        Self::analyze_expr_with_reply_vars(expr, aliases, reply_tx_vars)
    }

    fn analyze_expr_with_reply_vars(
        expr: &syn::Expr,
        aliases: &RustUseAliases,
        reply_tx_vars: BTreeSet<String>,
    ) -> MobGateSummary {
        let local_aliases = match expr {
            syn::Expr::Block(block) => aliases.with_block_aliases(&block.block),
            _ => aliases.clone(),
        };
        let mut analyzer = Self::new(local_aliases);
        analyzer.reply_tx_vars = reply_tx_vars;
        analyzer.visit_expr(expr);
        analyzer.summary
    }

    fn new(aliases: RustUseAliases) -> Self {
        Self {
            aliases,
            input_vars: BTreeMap::new(),
            pending_gate_result_vars: BTreeMap::new(),
            self_call_result_vars: BTreeMap::new(),
            reply_tx_vars: BTreeSet::new(),
            skipped_command_match_vars: BTreeSet::new(),
            skipped_command_source_vars: BTreeSet::new(),
            skipped_command_buffer_vars: BTreeSet::new(),
            command_effects_require_skipped_source: false,
            skipped_command_effects_armed: true,
            side_effect_receiver_vars: BTreeSet::new(),
            active_gated_inputs: BTreeSet::new(),
            statement_gated_inputs: BTreeSet::new(),
            fail_closed_depth: 0,
            summary: MobGateSummary::default(),
        }
    }

    fn with_fail_closed<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        self.fail_closed_depth += 1;
        let result = f(self);
        self.fail_closed_depth -= 1;
        result
    }

    fn command_effects_enabled(&self) -> bool {
        !self.command_effects_require_skipped_source || self.skipped_command_effects_armed
    }

    fn record_gate_call(&mut self, input: MobMachineInputVariant) {
        if !self.command_effects_enabled() {
            return;
        }
        if self.fail_closed_depth > 0 {
            self.summary.gated_inputs.insert(input);
            self.statement_gated_inputs.insert(input);
            self.summary.events.push(MobGateEvent::Gate(input));
        }
    }

    fn record_mob_side_effect(&mut self) {
        if !self.command_effects_enabled() {
            return;
        }
        self.summary.has_side_effect = true;
        self.summary.events.push(MobGateEvent::SideEffect);
        if self.active_gated_inputs.is_empty() {
            self.summary.has_ungated_side_effect = true;
        } else {
            self.summary
                .side_effect_gated_inputs
                .extend(self.active_gated_inputs.iter().copied());
        }
    }

    fn pending_gate_input_from_condition(
        &self,
        expr: &syn::Expr,
    ) -> Option<MobMachineInputVariant> {
        match expr {
            syn::Expr::MethodCall(call) if call.method == "is_ok" => {
                expr_path_single_ident(&call.receiver)
                    .and_then(|var| self.pending_gate_result_vars.get(&var).copied())
            }
            syn::Expr::Paren(paren) => self.pending_gate_input_from_condition(&paren.expr),
            syn::Expr::Group(group) => self.pending_gate_input_from_condition(&group.expr),
            syn::Expr::Reference(reference) => {
                self.pending_gate_input_from_condition(&reference.expr)
            }
            _ => None,
        }
    }

    fn with_isolated_statement_gates<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let statement_before = self.statement_gated_inputs.clone();
        let summary_gated_before = self.summary.gated_inputs.clone();
        let fail_closed_before = self.summary.fail_closed_calls.clone();
        let propagated_before = self.summary.propagated_calls.clone();
        let reply_gated_before = self.summary.reply_gated_inputs.clone();
        let events_before = self.summary.events.clone();
        self.statement_gated_inputs.clear();
        let result = f(self);
        let retained_helper_events = self
            .summary
            .events
            .iter()
            .skip(events_before.len())
            .filter(|event| matches!(event, MobGateEvent::HelperCall { .. }))
            .cloned()
            .collect::<Vec<_>>();
        self.summary.gated_inputs = summary_gated_before;
        self.summary.fail_closed_calls = fail_closed_before;
        self.summary.propagated_calls = propagated_before;
        self.summary.reply_gated_inputs = reply_gated_before;
        self.summary.events = events_before;
        self.summary.events.extend(retained_helper_events);
        self.statement_gated_inputs = statement_before;
        result
    }

    fn with_isolated_gate_state<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let input_vars_before = self.input_vars.clone();
        let pending_gate_result_vars_before = self.pending_gate_result_vars.clone();
        let self_call_result_vars_before = self.self_call_result_vars.clone();
        let reply_tx_vars_before = self.reply_tx_vars.clone();
        let skipped_command_match_vars_before = self.skipped_command_match_vars.clone();
        let skipped_command_source_vars_before = self.skipped_command_source_vars.clone();
        let skipped_command_buffer_vars_before = self.skipped_command_buffer_vars.clone();
        let skipped_command_effects_armed_before = self.skipped_command_effects_armed;
        let side_effect_receiver_vars_before = self.side_effect_receiver_vars.clone();
        let active_before = self.active_gated_inputs.clone();
        let statement_before = self.statement_gated_inputs.clone();
        let summary_gated_before = self.summary.gated_inputs.clone();
        let side_effect_gated_before = self.summary.side_effect_gated_inputs.clone();
        let has_side_effect_before = self.summary.has_side_effect;
        let has_ungated_side_effect_before = self.summary.has_ungated_side_effect;
        let fail_closed_before = self.summary.fail_closed_calls.clone();
        let propagated_before = self.summary.propagated_calls.clone();
        let reply_gated_before = self.summary.reply_gated_inputs.clone();
        let events_before = self.summary.events.clone();
        self.statement_gated_inputs.clear();
        let result = f(self);
        self.summary.gated_inputs = summary_gated_before;
        self.summary.side_effect_gated_inputs = side_effect_gated_before;
        self.summary.has_side_effect = has_side_effect_before;
        self.summary.has_ungated_side_effect = has_ungated_side_effect_before;
        self.summary.fail_closed_calls = fail_closed_before;
        self.summary.propagated_calls = propagated_before;
        self.summary.reply_gated_inputs = reply_gated_before;
        self.summary.events = events_before;
        self.active_gated_inputs = active_before;
        self.statement_gated_inputs = statement_before;
        self.input_vars = input_vars_before;
        self.pending_gate_result_vars = pending_gate_result_vars_before;
        self.self_call_result_vars = self_call_result_vars_before;
        self.reply_tx_vars = reply_tx_vars_before;
        self.skipped_command_match_vars = skipped_command_match_vars_before;
        self.skipped_command_source_vars = skipped_command_source_vars_before;
        self.skipped_command_buffer_vars = skipped_command_buffer_vars_before;
        self.skipped_command_effects_armed = skipped_command_effects_armed_before;
        self.side_effect_receiver_vars = side_effect_receiver_vars_before;
        result
    }

    fn with_isolated_loop_gate_state<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let input_vars_before = self.input_vars.clone();
        let pending_gate_result_vars_before = self.pending_gate_result_vars.clone();
        let self_call_result_vars_before = self.self_call_result_vars.clone();
        let reply_tx_vars_before = self.reply_tx_vars.clone();
        let skipped_command_match_vars_before = self.skipped_command_match_vars.clone();
        let skipped_command_source_vars_before = self.skipped_command_source_vars.clone();
        let skipped_command_buffer_vars_before = self.skipped_command_buffer_vars.clone();
        let skipped_command_effects_armed_before = self.skipped_command_effects_armed;
        let side_effect_receiver_vars_before = self.side_effect_receiver_vars.clone();
        let active_before = self.active_gated_inputs.clone();
        let statement_before = self.statement_gated_inputs.clone();
        let summary_gated_before = self.summary.gated_inputs.clone();
        let fail_closed_before = self.summary.fail_closed_calls.clone();
        let propagated_before = self.summary.propagated_calls.clone();
        let reply_gated_before = self.summary.reply_gated_inputs.clone();
        let events_before = self.summary.events.clone();
        self.statement_gated_inputs.clear();
        let result = f(self);
        self.summary.gated_inputs = summary_gated_before;
        self.summary.fail_closed_calls = fail_closed_before;
        self.summary.propagated_calls = propagated_before;
        self.summary.reply_gated_inputs = reply_gated_before;
        self.summary.events = events_before;
        self.active_gated_inputs = active_before;
        self.statement_gated_inputs = statement_before;
        retain_loop_preserved_gate_map(&mut self.input_vars, &input_vars_before);
        retain_loop_preserved_gate_map(
            &mut self.pending_gate_result_vars,
            &pending_gate_result_vars_before,
        );
        retain_loop_preserved_gate_map(
            &mut self.self_call_result_vars,
            &self_call_result_vars_before,
        );
        retain_loop_preserved_gate_set(&mut self.reply_tx_vars, &reply_tx_vars_before);
        retain_loop_preserved_gate_set(
            &mut self.skipped_command_match_vars,
            &skipped_command_match_vars_before,
        );
        retain_loop_preserved_gate_set(
            &mut self.skipped_command_source_vars,
            &skipped_command_source_vars_before,
        );
        retain_loop_preserved_gate_set(
            &mut self.skipped_command_buffer_vars,
            &skipped_command_buffer_vars_before,
        );
        self.skipped_command_effects_armed = skipped_command_effects_armed_before;
        retain_loop_preserved_gate_set(
            &mut self.side_effect_receiver_vars,
            &side_effect_receiver_vars_before,
        );
        result
    }

    fn with_shadowed_gate_vars<R>(
        &mut self,
        names: &BTreeSet<String>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        if names.is_empty() {
            return f(self);
        }
        let input_vars_before = self.input_vars.clone();
        let pending_gate_result_vars_before = self.pending_gate_result_vars.clone();
        let self_call_result_vars_before = self.self_call_result_vars.clone();
        let reply_tx_vars_before = self.reply_tx_vars.clone();
        let skipped_command_match_vars_before = self.skipped_command_match_vars.clone();
        let skipped_command_source_vars_before = self.skipped_command_source_vars.clone();
        let skipped_command_buffer_vars_before = self.skipped_command_buffer_vars.clone();
        let side_effect_receiver_vars_before = self.side_effect_receiver_vars.clone();
        for name in names {
            self.input_vars.remove(name);
            self.pending_gate_result_vars.remove(name);
            self.self_call_result_vars.remove(name);
            self.reply_tx_vars.remove(name);
            self.skipped_command_match_vars.remove(name);
            self.skipped_command_source_vars.remove(name);
            self.skipped_command_buffer_vars.remove(name);
            self.side_effect_receiver_vars.remove(name);
        }
        let result = f(self);
        self.input_vars = input_vars_before;
        self.pending_gate_result_vars = pending_gate_result_vars_before;
        self.self_call_result_vars = self_call_result_vars_before;
        self.reply_tx_vars = reply_tx_vars_before;
        self.skipped_command_match_vars = skipped_command_match_vars_before;
        self.skipped_command_source_vars = skipped_command_source_vars_before;
        self.skipped_command_buffer_vars = skipped_command_buffer_vars_before;
        self.side_effect_receiver_vars = side_effect_receiver_vars_before;
        result
    }

    fn invalidate_gate_var(&mut self, name: &str) {
        self.input_vars.remove(name);
        self.pending_gate_result_vars.remove(name);
        self.self_call_result_vars.remove(name);
        self.reply_tx_vars.remove(name);
        self.skipped_command_match_vars.remove(name);
        self.skipped_command_source_vars.remove(name);
        self.skipped_command_buffer_vars.remove(name);
        self.side_effect_receiver_vars.remove(name);
    }

    fn expr_is_mob_side_effect_receiver_source(&self, expr: &syn::Expr) -> bool {
        expr_root_ident_through_method_calls(expr)
            .as_ref()
            .is_some_and(|root| self.side_effect_receiver_vars.contains(root))
            || expr_is_self_mob_side_effect_field(expr)
            || self.expr_contains_mob_side_effect_receiver_source(expr)
    }

    fn expr_contains_mob_side_effect_receiver_source(&self, expr: &syn::Expr) -> bool {
        match expr {
            syn::Expr::Call(call) => call
                .args
                .iter()
                .any(|arg| self.expr_is_mob_side_effect_receiver_source(arg)),
            syn::Expr::MethodCall(call) => {
                self.expr_is_mob_side_effect_receiver_source(&call.receiver)
                    || call
                        .args
                        .iter()
                        .any(|arg| self.expr_is_mob_side_effect_receiver_source(arg))
            }
            syn::Expr::Try(try_expr) => {
                self.expr_is_mob_side_effect_receiver_source(&try_expr.expr)
            }
            syn::Expr::Await(await_expr) => {
                self.expr_is_mob_side_effect_receiver_source(&await_expr.base)
            }
            syn::Expr::Reference(reference) => {
                self.expr_is_mob_side_effect_receiver_source(&reference.expr)
            }
            syn::Expr::Paren(paren) => self.expr_is_mob_side_effect_receiver_source(&paren.expr),
            syn::Expr::Group(group) => self.expr_is_mob_side_effect_receiver_source(&group.expr),
            _ => false,
        }
    }

    fn expr_uses_skipped_command_source(&self, expr: &syn::Expr) -> bool {
        match expr {
            syn::Expr::MethodCall(call) => {
                let receiver = expr_path_single_ident(&call.receiver);
                (call.method == "recv"
                    && receiver
                        .as_ref()
                        .is_some_and(|ident| self.skipped_command_source_vars.contains(ident)))
                    || (call.method == "pop_front"
                        && receiver
                            .as_ref()
                            .is_some_and(|ident| self.skipped_command_buffer_vars.contains(ident)))
                    || self.expr_uses_skipped_command_source(&call.receiver)
                    || call
                        .args
                        .iter()
                        .any(|arg| self.expr_uses_skipped_command_source(arg))
            }
            syn::Expr::Call(call) => call
                .args
                .iter()
                .any(|arg| self.expr_uses_skipped_command_source(arg)),
            syn::Expr::If(expr_if) => {
                self.expr_uses_skipped_command_source(&expr_if.cond)
                    || expr_if
                        .then_branch
                        .stmts
                        .iter()
                        .any(|stmt| self.stmt_uses_skipped_command_source(stmt))
                    || expr_if
                        .else_branch
                        .as_ref()
                        .is_some_and(|(_, expr)| self.expr_uses_skipped_command_source(expr))
            }
            syn::Expr::Let(expr_let) => self.expr_uses_skipped_command_source(&expr_let.expr),
            syn::Expr::Block(block) => block
                .block
                .stmts
                .iter()
                .any(|stmt| self.stmt_uses_skipped_command_source(stmt)),
            syn::Expr::Try(try_expr) => self.expr_uses_skipped_command_source(&try_expr.expr),
            syn::Expr::Await(await_expr) => self.expr_uses_skipped_command_source(&await_expr.base),
            syn::Expr::Reference(reference) => {
                self.expr_uses_skipped_command_source(&reference.expr)
            }
            syn::Expr::Paren(paren) => self.expr_uses_skipped_command_source(&paren.expr),
            syn::Expr::Group(group) => self.expr_uses_skipped_command_source(&group.expr),
            _ => false,
        }
    }

    fn stmt_uses_skipped_command_source(&self, stmt: &syn::Stmt) -> bool {
        match stmt {
            syn::Stmt::Local(local) => local
                .init
                .as_ref()
                .is_some_and(|init| self.expr_uses_skipped_command_source(&init.expr)),
            syn::Stmt::Expr(expr, _) => self.expr_uses_skipped_command_source(expr),
            syn::Stmt::Item(_) | syn::Stmt::Macro(_) => false,
        }
    }
}

impl<'ast> Visit<'ast> for MobGateAnalyzer {
    fn visit_block(&mut self, node: &'ast syn::Block) {
        let aliases_before = self.aliases.clone();
        self.aliases = self.aliases.with_block_aliases(node);
        let local_names = block_local_bound_names(node);
        let input_vars_before = self.input_vars.clone();
        let pending_gate_result_vars_before = self.pending_gate_result_vars.clone();
        let self_call_result_vars_before = self.self_call_result_vars.clone();
        let reply_tx_vars_before = self.reply_tx_vars.clone();
        let skipped_command_match_vars_before = self.skipped_command_match_vars.clone();
        let skipped_command_source_vars_before = self.skipped_command_source_vars.clone();
        let skipped_command_buffer_vars_before = self.skipped_command_buffer_vars.clone();
        let side_effect_receiver_vars_before = self.side_effect_receiver_vars.clone();
        for stmt in &node.stmts {
            self.statement_gated_inputs.clear();
            self.visit_stmt(stmt);
            self.active_gated_inputs
                .extend(self.statement_gated_inputs.iter().copied());
        }
        self.statement_gated_inputs.clear();
        for name in &local_names {
            if let Some(input) = input_vars_before.get(name) {
                self.input_vars.insert(name.clone(), *input);
            } else {
                self.input_vars.remove(name);
            }
            if let Some(input) = pending_gate_result_vars_before.get(name) {
                self.pending_gate_result_vars.insert(name.clone(), *input);
            } else {
                self.pending_gate_result_vars.remove(name);
            }
            if let Some(method) = self_call_result_vars_before.get(name) {
                self.self_call_result_vars
                    .insert(name.clone(), method.clone());
            } else {
                self.self_call_result_vars.remove(name);
            }
            if reply_tx_vars_before.contains(name) {
                self.reply_tx_vars.insert(name.clone());
            } else {
                self.reply_tx_vars.remove(name);
            }
            if skipped_command_match_vars_before.contains(name) {
                self.skipped_command_match_vars.insert(name.clone());
            } else {
                self.skipped_command_match_vars.remove(name);
            }
            if skipped_command_source_vars_before.contains(name) {
                self.skipped_command_source_vars.insert(name.clone());
            } else {
                self.skipped_command_source_vars.remove(name);
            }
            if skipped_command_buffer_vars_before.contains(name) {
                self.skipped_command_buffer_vars.insert(name.clone());
            } else {
                self.skipped_command_buffer_vars.remove(name);
            }
            if side_effect_receiver_vars_before.contains(name) {
                self.side_effect_receiver_vars.insert(name.clone());
            } else {
                self.side_effect_receiver_vars.remove(name);
            }
        }
        self.aliases = aliases_before;
    }

    fn visit_local(&mut self, node: &'ast syn::Local) {
        if let Some(init) = &node.init {
            self.visit_expr(&init.expr);
            if let Some((_, diverge)) = &init.diverge {
                self.visit_expr(diverge);
            }
        }
        let bound_names = pat_bound_idents(&node.pat);
        if let syn::Pat::Ident(ident) = &node.pat
            && let Some(init) = &node.init
        {
            let name = ident.ident.to_string();
            let is_reply_tx_alias = expr_path_single_ident(&init.expr)
                .as_ref()
                .is_some_and(|source| self.reply_tx_vars.contains(source));
            if is_reply_tx_alias {
                self.reply_tx_vars.insert(name.clone());
            } else {
                self.reply_tx_vars.remove(&name);
            }
            if let Some(input) =
                mob_input_variant_from_expr(&init.expr, &self.input_vars, &self.aliases)
            {
                self.input_vars.insert(name.clone(), input);
            } else {
                self.input_vars.remove(&name);
            }
            if let Some(input) =
                mob_gate_input_from_expr(&init.expr, &self.input_vars, &self.aliases)
            {
                self.pending_gate_result_vars.insert(name.clone(), input);
            } else {
                self.pending_gate_result_vars.remove(&name);
            }
            if let Some(method) = self_method_call_name_from_expr(&init.expr) {
                self.self_call_result_vars.insert(name.clone(), method);
            } else {
                self.self_call_result_vars.remove(&name);
            }
            let uses_skipped_command_source = self.expr_uses_skipped_command_source(&init.expr);
            if uses_skipped_command_source {
                self.skipped_command_effects_armed = true;
            }
            let is_skipped_command_source_alias = expr_path_single_ident(&init.expr)
                .as_ref()
                .is_some_and(|source| self.skipped_command_source_vars.contains(source));
            if is_skipped_command_source_alias {
                self.skipped_command_source_vars.insert(name.clone());
            } else {
                self.skipped_command_source_vars.remove(&name);
            }
            let is_skipped_command_buffer = expr_initializes_mob_command_buffer(&init.expr)
                && !self.skipped_command_source_vars.is_empty();
            if is_skipped_command_buffer {
                self.skipped_command_buffer_vars.insert(name.clone());
            } else {
                self.skipped_command_buffer_vars.remove(&name);
            }
            if uses_skipped_command_source
                || expr_path_single_ident(&init.expr)
                    .as_ref()
                    .is_some_and(|source| self.skipped_command_match_vars.contains(source))
            {
                self.skipped_command_match_vars.insert(name.clone());
            } else {
                self.skipped_command_match_vars.remove(&name);
            }
            if self.expr_is_mob_side_effect_receiver_source(&init.expr) {
                self.side_effect_receiver_vars.insert(name);
            } else {
                self.side_effect_receiver_vars.remove(&name);
            }
            return;
        }
        for name in bound_names {
            self.input_vars.remove(&name);
            self.pending_gate_result_vars.remove(&name);
            self.self_call_result_vars.remove(&name);
            self.reply_tx_vars.remove(&name);
            self.skipped_command_match_vars.remove(&name);
            self.skipped_command_source_vars.remove(&name);
            self.skipped_command_buffer_vars.remove(&name);
            self.side_effect_receiver_vars.remove(&name);
        }
    }

    fn visit_expr_try(&mut self, node: &'ast syn::ExprTry) {
        if expr_is_ok_constructor_call(&node.expr)
            || expr_contains_fail_open_result_method(&node.expr)
        {
            self.visit_expr(&node.expr);
        } else {
            self.with_fail_closed(|this| this.visit_expr(&node.expr));
        }
    }

    fn visit_expr_if(&mut self, node: &'ast syn::ExprIf) {
        self.visit_expr(&node.cond);
        let shadowed_cond_names = match &*node.cond {
            syn::Expr::Let(expr_let) => pat_bound_idents(&expr_let.pat),
            _ => BTreeSet::new(),
        };
        let active_before = self.active_gated_inputs.clone();
        if let Some(input) = self.pending_gate_input_from_condition(&node.cond) {
            self.active_gated_inputs.insert(input);
            self.with_isolated_statement_gates(|this| {
                this.with_shadowed_gate_vars(&shadowed_cond_names, |this| {
                    this.with_fail_closed(|this| this.visit_block(&node.then_branch));
                });
            });
        } else {
            self.with_isolated_statement_gates(|this| {
                this.with_shadowed_gate_vars(&shadowed_cond_names, |this| {
                    this.visit_block(&node.then_branch);
                });
            });
        }
        self.active_gated_inputs = active_before.clone();
        if let Some((_, else_branch)) = &node.else_branch {
            self.with_isolated_statement_gates(|this| this.visit_expr(else_branch));
            self.active_gated_inputs = active_before;
        }
    }

    fn visit_expr_assign(&mut self, node: &'ast syn::ExprAssign) {
        self.visit_expr(&node.right);
        if let Some(name) = expr_path_single_ident(&node.left) {
            let is_reply_tx_alias = expr_path_single_ident(&node.right)
                .as_ref()
                .is_some_and(|source| self.reply_tx_vars.contains(source));
            if is_reply_tx_alias {
                self.reply_tx_vars.insert(name.clone());
            } else {
                self.reply_tx_vars.remove(&name);
            }
            if let Some(input) =
                mob_input_variant_from_expr(&node.right, &self.input_vars, &self.aliases)
            {
                self.input_vars.insert(name.clone(), input);
            } else {
                self.input_vars.remove(&name);
            }
            if let Some(input) =
                mob_gate_input_from_expr(&node.right, &self.input_vars, &self.aliases)
            {
                self.pending_gate_result_vars.insert(name.clone(), input);
            } else {
                self.pending_gate_result_vars.remove(&name);
            }
            if let Some(method) = self_method_call_name_from_expr(&node.right) {
                self.self_call_result_vars.insert(name.clone(), method);
            } else {
                self.self_call_result_vars.remove(&name);
            }
            let uses_skipped_command_source = self.expr_uses_skipped_command_source(&node.right);
            if uses_skipped_command_source {
                self.skipped_command_effects_armed = true;
            }
            let is_skipped_command_source_alias = expr_path_single_ident(&node.right)
                .as_ref()
                .is_some_and(|source| self.skipped_command_source_vars.contains(source));
            if is_skipped_command_source_alias {
                self.skipped_command_source_vars.insert(name.clone());
            } else {
                self.skipped_command_source_vars.remove(&name);
            }
            let is_skipped_command_buffer = expr_initializes_mob_command_buffer(&node.right)
                && !self.skipped_command_source_vars.is_empty();
            if is_skipped_command_buffer {
                self.skipped_command_buffer_vars.insert(name.clone());
            } else {
                self.skipped_command_buffer_vars.remove(&name);
            }
            if uses_skipped_command_source
                || expr_path_single_ident(&node.right)
                    .as_ref()
                    .is_some_and(|source| self.skipped_command_match_vars.contains(source))
            {
                self.skipped_command_match_vars.insert(name.clone());
            } else {
                self.skipped_command_match_vars.remove(&name);
            }
            if self.expr_is_mob_side_effect_receiver_source(&node.right) {
                self.side_effect_receiver_vars.insert(name);
            } else {
                self.side_effect_receiver_vars.remove(&name);
            }
        } else {
            self.visit_expr(&node.left);
        }
    }

    fn visit_expr_binary(&mut self, node: &'ast syn::ExprBinary) {
        if bin_op_is_assign(&node.op)
            && let Some(name) = expr_root_ident(&node.left)
        {
            self.invalidate_gate_var(&name);
        }
        visit::visit_expr_binary(self, node);
    }

    fn visit_expr_reference(&mut self, node: &'ast syn::ExprReference) {
        if node.mutability.is_some()
            && let Some(name) = expr_path_single_ident(&node.expr)
        {
            self.invalidate_gate_var(&name);
        }
        visit::visit_expr_reference(self, node);
    }

    fn visit_expr_match(&mut self, node: &'ast syn::ExprMatch) {
        self.visit_expr(&node.expr);
        let uses_skipped_command_source = self.expr_uses_skipped_command_source(&node.expr);
        if uses_skipped_command_source {
            self.skipped_command_effects_armed = true;
        }
        if uses_skipped_command_source
            || expr_path_single_ident(&node.expr)
                .as_ref()
                .is_some_and(|ident| self.skipped_command_match_vars.contains(ident))
        {
            return;
        }
        let active_before = self.active_gated_inputs.clone();
        for arm in &node.arms {
            self.active_gated_inputs = active_before.clone();
            let shadowed_names = pat_bound_idents(&arm.pat);
            self.with_shadowed_gate_vars(&shadowed_names, |this| {
                if let Some((_, guard)) = &arm.guard {
                    this.visit_expr(guard);
                }
                this.with_isolated_statement_gates(|this| this.visit_expr(&arm.body));
            });
        }
        self.active_gated_inputs = active_before;
    }

    fn visit_expr_closure(&mut self, node: &'ast syn::ExprClosure) {
        self.with_isolated_gate_state(|this| visit::visit_expr_closure(this, node));
    }

    fn visit_expr_async(&mut self, node: &'ast syn::ExprAsync) {
        self.with_isolated_gate_state(|this| visit::visit_expr_async(this, node));
    }

    fn visit_expr_loop(&mut self, node: &'ast syn::ExprLoop) {
        self.with_isolated_loop_gate_state(|this| visit::visit_expr_loop(this, node));
    }

    fn visit_expr_while(&mut self, node: &'ast syn::ExprWhile) {
        self.with_isolated_loop_gate_state(|this| visit::visit_expr_while(this, node));
    }

    fn visit_expr_for_loop(&mut self, node: &'ast syn::ExprForLoop) {
        self.with_isolated_loop_gate_state(|this| visit::visit_expr_for_loop(this, node));
    }

    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if let syn::Expr::Path(func) = &*node.func
            && let Some(name) = func
                .path
                .segments
                .last()
                .map(|segment| segment.ident.to_string())
            && self.fail_closed_depth > 0
            && self.command_effects_enabled()
        {
            self.summary.fail_closed_calls.insert(name.clone());
            self.summary.events.push(MobGateEvent::FailClosedCall(name));
        }
        if mob_side_effect_path_call(node, &self.side_effect_receiver_vars) {
            self.record_mob_side_effect();
        }
        visit::visit_expr_call(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        let method = node.method.to_string();
        if gate_provenance_mutator_method(&method)
            && let Some(name) = expr_root_ident(&node.receiver)
        {
            self.invalidate_gate_var(&name);
        }
        if method == "map_err" {
            if self.fail_closed_depth > 0 {
                self.with_fail_closed(|this| this.visit_expr(&node.receiver));
            } else {
                self.visit_expr(&node.receiver);
            }
            for arg in &node.args {
                self.visit_expr(arg);
            }
            return;
        }
        if expr_is_self_receiver(&node.receiver) {
            if self.fail_closed_depth > 0 && self.command_effects_enabled() {
                self.summary.fail_closed_calls.insert(method.clone());
                self.summary
                    .events
                    .push(MobGateEvent::FailClosedCall(method.clone()));
            }
            if self.command_effects_enabled() {
                self.summary.events.push(MobGateEvent::HelperCall {
                    call: method.clone(),
                    gated_inputs: self.active_gated_inputs.clone(),
                });
            }
        }
        if method == "send"
            && expr_path_single_ident(&node.receiver)
                .as_ref()
                .is_some_and(|receiver| self.reply_tx_vars.contains(receiver))
            && let Some(result_var) = node.args.first().and_then(expr_path_single_ident)
            && let Some(input) = self.pending_gate_result_vars.get(&result_var).copied()
            && self.command_effects_enabled()
        {
            self.summary.reply_gated_inputs.insert(input);
        }
        if method == "send"
            && expr_path_single_ident(&node.receiver)
                .as_ref()
                .is_some_and(|receiver| self.reply_tx_vars.contains(receiver))
            && let Some(result_var) = node.args.first().and_then(expr_path_single_ident)
            && let Some(call) = self.self_call_result_vars.get(&result_var)
            && self.command_effects_enabled()
        {
            self.summary.propagated_calls.insert(call.clone());
            self.summary
                .events
                .push(MobGateEvent::PropagatedCall(call.clone()));
        }
        if mob_side_effect_call(node, &self.side_effect_receiver_vars) {
            self.record_mob_side_effect();
        }
        if let Some(input) = mob_gate_method_input(node, &self.input_vars, &self.aliases) {
            self.record_gate_call(input);
        }
        visit::visit_expr_method_call(self, node);
    }
}

fn attrs_cfg_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        if !attr.path().is_ident("cfg") {
            return false;
        }
        let mut found = false;
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("test") {
                found = true;
            }
            Ok(())
        });
        found
    })
}

fn path_segments(path: &syn::Path) -> Vec<String> {
    path.segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect()
}

fn path_has_tail(path: &[String], tail: &[&str]) -> bool {
    path.len() >= tail.len()
        && path[path.len() - tail.len()..]
            .iter()
            .map(String::as_str)
            .eq(tail.iter().copied())
}

fn path_is_canonical_mob_machine_type(path: &[String], owner: &str) -> bool {
    path.iter().map(String::as_str).eq([owner])
        || path
            .iter()
            .map(String::as_str)
            .eq(["crate", "machines", "mob_machine", owner])
}

fn path_is_canonical_mob_command_type(path: &[String]) -> bool {
    path_is_canonical_mob_machine_type(path, "MobCommand")
        || path
            .iter()
            .map(String::as_str)
            .eq(["super", "state", "MobCommand"])
        || path
            .iter()
            .map(String::as_str)
            .eq(["crate", "runtime", "state", "MobCommand"])
}

fn path_is_canonical_mob_machine_variant(path: &[String], owner: &str, variant: &str) -> bool {
    (path.len() == 2
        && path.first().is_some_and(|segment| segment == owner)
        && path.get(1).is_some_and(|segment| segment == variant))
        || path
            .iter()
            .map(String::as_str)
            .eq(["crate", "machines", "mob_machine", owner, variant])
}

fn path_is_canonical_mob_command_variant(path: &[String], variant: &str) -> bool {
    path_is_canonical_mob_machine_variant(path, "MobCommand", variant)
        || path
            .iter()
            .map(String::as_str)
            .eq(["super", "state", "MobCommand", variant])
        || path
            .iter()
            .map(String::as_str)
            .eq(["crate", "runtime", "state", "MobCommand", variant])
}

fn path_is_root_mob_actor_impl(path: &[String]) -> bool {
    path.iter().map(String::as_str).eq(["MobActor"])
}

fn expr_is_self_run_store_receiver(expr: &syn::Expr) -> bool {
    expr_field_chain(expr)
        .iter()
        .map(String::as_str)
        .eq(["self", "run_store"])
}

fn expr_static_bool_value(expr: &syn::Expr) -> Option<bool> {
    match expr {
        syn::Expr::Lit(lit) => {
            if let syn::Lit::Bool(value) = &lit.lit {
                Some(value.value)
            } else {
                None
            }
        }
        syn::Expr::Paren(paren) => expr_static_bool_value(&paren.expr),
        syn::Expr::Group(group) => expr_static_bool_value(&group.expr),
        _ => None,
    }
}

fn resolved_path_is_canonical_flow_run_apply(path: &[String]) -> bool {
    path.iter()
        .map(String::as_str)
        .eq(["crate", "run", "apply_mob_machine_flow_run_command"])
}

fn path_is_canonical_mob_run_store_cas_call(
    path: &syn::Path,
    aliases: &RustUseAliases,
    method: &str,
) -> bool {
    if aliases.path_has_shadowed_bare_root(path)
        || (aliases.ambiguous_glob_import
            && aliases.path_has_unresolved_bare_root(path, "MobRunStore"))
    {
        return false;
    }
    aliases.resolve_path(path).into_iter().any(|segments| {
        segments
            .iter()
            .map(String::as_str)
            .eq(["MobRunStore", method])
            || segments
                .iter()
                .map(String::as_str)
                .eq(["crate", "store", "MobRunStore", method])
    })
}

fn member_key(member: &syn::Member) -> String {
    match member {
        syn::Member::Named(ident) => ident.to_string(),
        syn::Member::Unnamed(index) => index.index.to_string(),
    }
}

fn line_number(node: &impl syn::spanned::Spanned) -> usize {
    node.span().start().line
}

fn source_position(node: &impl syn::spanned::Spanned) -> SourcePosition {
    let start = node.span().start();
    SourcePosition {
        line: start.line,
        column: start.column,
    }
}

fn expr_as_path(expr: &syn::Expr) -> Option<&syn::Path> {
    match expr {
        syn::Expr::Path(path) => Some(&path.path),
        syn::Expr::Paren(paren) => expr_as_path(&paren.expr),
        syn::Expr::Group(group) => expr_as_path(&group.expr),
        _ => None,
    }
}

fn expr_path_single_ident(expr: &syn::Expr) -> Option<String> {
    let path = expr_as_path(expr)?;
    if path.segments.len() == 1 {
        path.segments
            .first()
            .map(|segment| segment.ident.to_string())
    } else {
        None
    }
}

fn expr_root_ident(expr: &syn::Expr) -> Option<String> {
    match expr {
        syn::Expr::Path(_) => expr_path_single_ident(expr),
        syn::Expr::Field(field) => expr_root_ident(&field.base),
        syn::Expr::Index(index) => expr_root_ident(&index.expr),
        syn::Expr::Reference(reference) => expr_root_ident(&reference.expr),
        syn::Expr::Unary(unary) if matches!(unary.op, syn::UnOp::Deref(_)) => {
            expr_root_ident(&unary.expr)
        }
        syn::Expr::Paren(paren) => expr_root_ident(&paren.expr),
        syn::Expr::Group(group) => expr_root_ident(&group.expr),
        syn::Expr::Await(await_expr) => expr_root_ident(&await_expr.base),
        _ => None,
    }
}

fn expr_identity_argument_name(expr: &syn::Expr) -> Option<String> {
    match expr {
        syn::Expr::Path(_) => expr_path_single_ident(expr),
        syn::Expr::MethodCall(call)
            if matches!(call.method.to_string().as_str(), "clone" | "as_ref") =>
        {
            expr_identity_argument_name(&call.receiver)
        }
        syn::Expr::Reference(reference) => expr_identity_argument_name(&reference.expr),
        syn::Expr::Paren(paren) => expr_identity_argument_name(&paren.expr),
        syn::Expr::Group(group) => expr_identity_argument_name(&group.expr),
        _ => None,
    }
}

fn expr_is_plan_scrutinee(expr: &syn::Expr, plan_vars: &BTreeSet<String>) -> bool {
    expr_variable_name(expr, plan_vars).is_some()
}

fn expr_initializes_mob_command_buffer(expr: &syn::Expr) -> bool {
    match expr {
        syn::Expr::Call(call) => expr_as_path(&call.func)
            .is_some_and(|path| path_has_tail(&path_segments(path), &["VecDeque", "new"])),
        syn::Expr::Paren(paren) => expr_initializes_mob_command_buffer(&paren.expr),
        syn::Expr::Group(group) => expr_initializes_mob_command_buffer(&group.expr),
        _ => false,
    }
}

fn state_struct_fields(path: &Path) -> Result<BTreeSet<String>> {
    if !path.exists() {
        return Ok(BTreeSet::new());
    }
    let source = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let parsed = syn::parse_file(&source).with_context(|| format!("parse {}", path.display()))?;
    Ok(parsed
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Struct(item_struct) if item_struct.ident == "State" => {
                Some(named_field_idents(&item_struct.fields))
            }
            _ => None,
        })
        .unwrap_or_default())
}

fn named_field_idents(fields: &syn::Fields) -> BTreeSet<String> {
    match fields {
        syn::Fields::Named(fields) => fields
            .named
            .iter()
            .filter_map(|field| field.ident.as_ref().map(ToString::to_string))
            .collect(),
        syn::Fields::Unnamed(_) | syn::Fields::Unit => BTreeSet::new(),
    }
}

fn flow_frame_loop_store_plan_variant_fields(
    path: &Path,
) -> Result<BTreeMap<String, BTreeSet<String>>> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let source = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let parsed = syn::parse_file(&source).with_context(|| format!("parse {}", path.display()))?;
    let mut variants = BTreeMap::new();
    for item in parsed.items {
        if let syn::Item::Enum(item_enum) = item
            && item_enum.ident == "FlowFrameLoopStorePlan"
        {
            for variant in item_enum.variants {
                let mut fields = named_field_idents(&variant.fields);
                fields.remove("machine_inputs");
                variants.insert(variant.ident.to_string(), fields);
            }
        }
    }
    Ok(variants)
}

fn mob_run_store_cas_methods(path: &Path) -> Result<BTreeMap<String, ProjectionCasMethodFacts>> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let source = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let parsed = syn::parse_file(&source).with_context(|| format!("parse {}", path.display()))?;
    let mut methods = BTreeMap::new();
    for item in parsed.items {
        if let syn::Item::Trait(item_trait) = item
            && item_trait.ident == "MobRunStore"
        {
            for trait_item in item_trait.items {
                if let syn::TraitItem::Fn(method) = trait_item {
                    let method_name = method.sig.ident.to_string();
                    if method_name.starts_with("cas_") {
                        methods.insert(method_name, projection_cas_method_facts(&method.sig));
                    }
                }
            }
        }
    }
    Ok(methods)
}

fn projection_cas_method_facts(sig: &syn::Signature) -> ProjectionCasMethodFacts {
    let mut facts = ProjectionCasMethodFacts::default();
    let mut method_arg_index = 0;
    for input in &sig.inputs {
        match input {
            syn::FnArg::Receiver(_) => {}
            syn::FnArg::Typed(pat_type) => {
                let arg_name = pat_ident_name(&pat_type.pat);
                if let Some(arg_name) = &arg_name {
                    facts.arg_names.insert(method_arg_index, arg_name.clone());
                }
                if projection_cas_arg_is_next_flow_state(pat_type) {
                    facts.next_flow_state_arg_indices.insert(method_arg_index);
                }
                if projection_cas_arg_is_plan_state(pat_type) {
                    facts.plan_state_arg_indices.insert(method_arg_index);
                }
                if projection_cas_arg_is_authority_input(pat_type) {
                    facts.authority_input_arg_indices.insert(method_arg_index);
                }
                if arg_name.as_deref() == Some("run_id") {
                    facts
                        .authority_identity_arg_indices
                        .insert(method_arg_index);
                }
                if arg_name.as_deref() != Some("run_id") {
                    facts.plan_owned_arg_indices.insert(method_arg_index);
                }
                method_arg_index += 1;
            }
        }
    }
    facts
}

fn projection_cas_arg_is_next_flow_state(pat_type: &syn::PatType) -> bool {
    pat_ident_name(&pat_type.pat)
        .is_some_and(|name| name == "next" || name == "next_flow_state" || name == "next_run_state")
        && type_path_segments(&pat_type.ty)
            .is_some_and(|segments| path_has_tail(&segments, &["flow_run", "State"]))
}

fn projection_cas_arg_is_plan_state(pat_type: &syn::PatType) -> bool {
    pat_ident_name(&pat_type.pat).is_some_and(|name| {
        name == "next"
            || name.starts_with("next_")
            || name.starts_with("initial_")
            || name == "next_flow_state"
            || name == "next_run_state"
    })
}

fn projection_cas_arg_is_authority_input(pat_type: &syn::PatType) -> bool {
    pat_ident_name(&pat_type.pat).is_some_and(|name| name == "authority_inputs")
        && type_contains_canonical_mob_machine_input(&pat_type.ty)
}

fn signature_return_path(sig: &syn::Signature) -> Option<Vec<String>> {
    match &sig.output {
        syn::ReturnType::Default => None,
        syn::ReturnType::Type(_, ty) => type_path_segments(ty),
    }
}

fn type_path_segments(ty: &syn::Type) -> Option<Vec<String>> {
    match ty {
        syn::Type::Path(path) => Some(path_segments(&path.path)),
        syn::Type::Reference(reference) => type_path_segments(&reference.elem),
        syn::Type::Paren(paren) => type_path_segments(&paren.elem),
        syn::Type::Group(group) => type_path_segments(&group.elem),
        _ => None,
    }
}

fn type_tail_ident(ty: &syn::Type) -> Option<String> {
    type_path_segments(ty).and_then(|segments| segments.last().cloned())
}

fn parameter_type_key(
    ty: &syn::Type,
    aliases: &RustUseAliases,
    rel: &str,
    module_depth: usize,
) -> Option<String> {
    let tail = type_tail_ident(ty)?;
    match tail.as_str() {
        "MobMachineFlowRunCommand"
        | "MobMachineFlowFrameCommand"
        | "MobMachineLoopIterationCommand"
        | "MobMachineFlowAuthorityToken" => {
            type_is_canonical_mob_run_owner_type(ty, aliases, rel, module_depth, tail.as_str())
                .then_some(tail)
        }
        "FlowFrameLoopStorePlan" => {
            type_is_canonical_flow_frame_loop_store_plan(ty, aliases).then_some(tail)
        }
        "RunId" => type_is_canonical_mob_run_id(ty, aliases).then_some(tail),
        _ => Some(tail),
    }
}

fn type_is_canonical_mob_run_owner_type(
    ty: &syn::Type,
    aliases: &RustUseAliases,
    rel: &str,
    module_depth: usize,
    owner: &str,
) -> bool {
    match ty {
        syn::Type::Path(path) => {
            let segments = path_segments(&path.path);
            if path.path.leading_colon.is_none()
                && segments.len() == 1
                && segments.first().is_some_and(|segment| segment == owner)
                && rel == "meerkat-mob/src/run.rs"
                && module_depth == 0
            {
                return true;
            }
            if aliases.path_has_shadowed_bare_root(&path.path) {
                return false;
            }
            if aliases.ambiguous_glob_import
                && aliases.path_has_unresolved_bare_root(&path.path, owner)
            {
                return false;
            }
            aliases
                .resolve_path(&path.path)
                .into_iter()
                .any(|segments| path_is_canonical_mob_run_owner_type(&segments, owner))
        }
        syn::Type::Reference(reference) => {
            type_is_canonical_mob_run_owner_type(&reference.elem, aliases, rel, module_depth, owner)
        }
        syn::Type::Paren(paren) => {
            type_is_canonical_mob_run_owner_type(&paren.elem, aliases, rel, module_depth, owner)
        }
        syn::Type::Group(group) => {
            type_is_canonical_mob_run_owner_type(&group.elem, aliases, rel, module_depth, owner)
        }
        _ => false,
    }
}

fn path_is_canonical_mob_run_owner_type(path: &[String], owner: &str) -> bool {
    path.iter().map(String::as_str).eq([owner])
        || path.iter().map(String::as_str).eq(["crate", "run", owner])
}

fn type_resolved_paths_for_owner(
    ty: &syn::Type,
    aliases: &RustUseAliases,
    owner: Option<&str>,
) -> Vec<Vec<String>> {
    match ty {
        syn::Type::Path(path) => {
            if aliases.path_has_shadowed_bare_root(&path.path)
                || owner.is_some_and(|owner| {
                    aliases.ambiguous_glob_import
                        && aliases.path_has_unresolved_bare_root(&path.path, owner)
                })
            {
                Vec::new()
            } else {
                aliases.resolve_path(&path.path)
            }
        }
        syn::Type::Reference(reference) => {
            type_resolved_paths_for_owner(&reference.elem, aliases, owner)
        }
        syn::Type::Paren(paren) => type_resolved_paths_for_owner(&paren.elem, aliases, owner),
        syn::Type::Group(group) => type_resolved_paths_for_owner(&group.elem, aliases, owner),
        _ => Vec::new(),
    }
}

fn type_is_canonical_flow_frame_loop_store_plan(ty: &syn::Type, aliases: &RustUseAliases) -> bool {
    type_resolved_paths_for_owner(ty, aliases, Some("FlowFrameLoopStorePlan"))
        .into_iter()
        .any(|segments| path_is_canonical_flow_frame_loop_store_plan_type(&segments))
}

fn path_is_canonical_flow_frame_loop_store_plan_type(path: &[String]) -> bool {
    path.iter()
        .map(String::as_str)
        .eq(["FlowFrameLoopStorePlan"])
        || path.iter().map(String::as_str).eq([
            "super",
            "flow_frame_engine",
            "FlowFrameLoopStorePlan",
        ])
        || path.iter().map(String::as_str).eq([
            "crate",
            "runtime",
            "flow_frame_engine",
            "FlowFrameLoopStorePlan",
        ])
}

fn path_is_canonical_flow_frame_loop_store_plan_variant(path: &[String], variant: &str) -> bool {
    (path.len() == 2
        && path
            .first()
            .is_some_and(|segment| segment == "FlowFrameLoopStorePlan")
        && path.get(1).is_some_and(|segment| segment == variant))
        || path.iter().map(String::as_str).eq([
            "super",
            "flow_frame_engine",
            "FlowFrameLoopStorePlan",
            variant,
        ])
        || path.iter().map(String::as_str).eq([
            "crate",
            "runtime",
            "flow_frame_engine",
            "FlowFrameLoopStorePlan",
            variant,
        ])
}

fn type_is_exact_canonical_mob_run_store_receiver(
    ty: &syn::Type,
    aliases: &RustUseAliases,
) -> bool {
    match ty {
        syn::Type::Path(path) => {
            type_path_is_canonical_mob_run_store_type(path, aliases)
                || type_path_is_arc_canonical_mob_run_store_trait_object(path, aliases)
        }
        syn::Type::TraitObject(trait_object) => {
            trait_object_is_canonical_mob_run_store(trait_object, aliases)
        }
        syn::Type::Reference(reference) => {
            type_is_exact_canonical_mob_run_store_receiver(&reference.elem, aliases)
        }
        syn::Type::Paren(paren) => {
            type_is_exact_canonical_mob_run_store_receiver(&paren.elem, aliases)
        }
        syn::Type::Group(group) => {
            type_is_exact_canonical_mob_run_store_receiver(&group.elem, aliases)
        }
        _ => false,
    }
}

fn type_path_is_canonical_mob_run_store_type(
    path: &syn::TypePath,
    aliases: &RustUseAliases,
) -> bool {
    !(type_path_has_generic_arguments(path)
        || aliases.path_has_shadowed_bare_root(&path.path)
        || (aliases.ambiguous_glob_import
            && aliases.path_has_unresolved_bare_root(&path.path, "MobRunStore")))
        && aliases
            .resolve_path(&path.path)
            .into_iter()
            .any(|segments| path_is_canonical_mob_run_store_type(&segments))
}

fn type_path_is_arc_canonical_mob_run_store_trait_object(
    path: &syn::TypePath,
    aliases: &RustUseAliases,
) -> bool {
    if !type_path_is_allowed_arc(&path.path, aliases) {
        return false;
    }
    let Some(segment) = path.path.segments.last() else {
        return false;
    };
    let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return false;
    };
    let mut type_args = arguments.args.iter().filter_map(|arg| {
        if let syn::GenericArgument::Type(ty) = arg {
            Some(ty)
        } else {
            None
        }
    });
    let Some(syn::Type::TraitObject(trait_object)) = type_args.next() else {
        return false;
    };
    type_args.next().is_none() && trait_object_is_canonical_mob_run_store(trait_object, aliases)
}

fn type_path_has_generic_arguments(path: &syn::TypePath) -> bool {
    path.path
        .segments
        .iter()
        .any(|segment| !matches!(segment.arguments, syn::PathArguments::None))
}

fn type_path_is_allowed_arc(path: &syn::Path, aliases: &RustUseAliases) -> bool {
    if aliases.path_has_shadowed_bare_root(path) {
        return false;
    }
    aliases.resolve_path(path).into_iter().any(|segments| {
        segments.iter().map(String::as_str).eq(["Arc"])
            || segments
                .iter()
                .map(String::as_str)
                .eq(["std", "sync", "Arc"])
    })
}

fn trait_object_is_canonical_mob_run_store(
    trait_object: &syn::TypeTraitObject,
    aliases: &RustUseAliases,
) -> bool {
    trait_object.bounds.iter().any(|bound| {
        if let syn::TypeParamBound::Trait(trait_bound) = bound {
            !(aliases.path_has_shadowed_bare_root(&trait_bound.path)
                || (aliases.ambiguous_glob_import
                    && aliases.path_has_unresolved_bare_root(&trait_bound.path, "MobRunStore")))
                && aliases
                    .resolve_path(&trait_bound.path)
                    .into_iter()
                    .any(|segments| path_is_canonical_mob_run_store_type(&segments))
        } else {
            false
        }
    })
}

fn path_is_canonical_mob_run_store_type(path: &[String]) -> bool {
    path.iter().map(String::as_str).eq(["MobRunStore"])
        || path
            .iter()
            .map(String::as_str)
            .eq(["crate", "store", "MobRunStore"])
        || path
            .iter()
            .map(String::as_str)
            .eq(["super", "store", "MobRunStore"])
}

fn actor_plan_arm_facts_from_pat(pat: &syn::Pat, aliases: &RustUseAliases) -> ActorPlanArmFacts {
    let mut facts = ActorPlanArmFacts {
        bindings: pat_bound_idents(pat),
        ..ActorPlanArmFacts::default()
    };
    collect_actor_plan_arm_facts(pat, aliases, &mut facts);
    facts
}

fn actor_plan_single_variant(plan_arm: &ActorPlanArmFacts) -> Option<String> {
    (plan_arm.variants.len() == 1)
        .then(|| plan_arm.variants.iter().next().cloned())
        .flatten()
}

fn actor_plan_field_for_cas_arg(
    arg_name: &str,
    variant_fields: &BTreeSet<String>,
) -> Option<String> {
    match arg_name {
        "expected" if variant_fields.contains("expected_frame") => Some("expected_frame"),
        "expected" if variant_fields.contains("expected_run_state") => Some("expected_run_state"),
        "next" if variant_fields.contains("next_frame") => Some("next_frame"),
        "next" if variant_fields.contains("next_run_state") => Some("next_run_state"),
        "next" if variant_fields.contains("initial_frame") => Some("initial_frame"),
        field if variant_fields.contains(field) => Some(field),
        _ => None,
    }
    .map(ToOwned::to_owned)
}

fn collect_actor_plan_arm_facts(
    pat: &syn::Pat,
    aliases: &RustUseAliases,
    facts: &mut ActorPlanArmFacts,
) {
    match pat {
        syn::Pat::Struct(pat_struct) => {
            let variant = aliases
                .resolve_path(&pat_struct.path)
                .into_iter()
                .find_map(|path| {
                    path.last().and_then(|variant| {
                        path_is_canonical_flow_frame_loop_store_plan_variant(&path, variant)
                            .then(|| variant.clone())
                    })
                });
            if let Some(variant) = variant {
                facts.variants.insert(variant);
            }
            for field in &pat_struct.fields {
                if let syn::Member::Named(ident) = &field.member {
                    let field_name = ident.to_string();
                    if field_name != "machine_inputs" {
                        let bindings = pat_bound_idents(&field.pat);
                        if !bindings.is_empty() {
                            facts
                                .field_bindings
                                .entry(field_name)
                                .or_default()
                                .extend(bindings);
                        }
                    }
                }
            }
        }
        syn::Pat::Reference(reference) => {
            collect_actor_plan_arm_facts(&reference.pat, aliases, facts);
        }
        syn::Pat::Paren(paren) => {
            collect_actor_plan_arm_facts(&paren.pat, aliases, facts);
        }
        syn::Pat::Or(or_pat) => {
            for case in &or_pat.cases {
                collect_actor_plan_arm_facts(case, aliases, facts);
            }
        }
        syn::Pat::Type(ty) => {
            collect_actor_plan_arm_facts(&ty.pat, aliases, facts);
        }
        _ => {}
    }
}

fn type_is_canonical_mob_run_id(ty: &syn::Type, aliases: &RustUseAliases) -> bool {
    type_resolved_paths_for_owner(ty, aliases, Some("RunId"))
        .into_iter()
        .any(|segments| path_is_canonical_mob_run_id_type(&segments))
}

fn path_is_canonical_mob_run_id_type(path: &[String]) -> bool {
    path.iter().map(String::as_str).eq(["RunId"])
        || path.iter().map(String::as_str).eq(["crate", "RunId"])
        || path
            .iter()
            .map(String::as_str)
            .eq(["crate", "ids", "RunId"])
        || path.iter().map(String::as_str).eq(["super", "RunId"])
        || path
            .iter()
            .map(String::as_str)
            .eq(["super", "ids", "RunId"])
}

fn type_is_canonical_mob_command(ty: &syn::Type, aliases: &RustUseAliases) -> bool {
    match ty {
        syn::Type::Path(path) => {
            !(aliases.path_has_shadowed_bare_root(&path.path)
                || (aliases.ambiguous_glob_import
                    && aliases.path_has_unresolved_bare_root(&path.path, "MobCommand")))
                && aliases
                    .resolve_path(&path.path)
                    .into_iter()
                    .any(|segments| path_is_canonical_mob_command_type(&segments))
        }
        syn::Type::Reference(reference) => type_is_canonical_mob_command(&reference.elem, aliases),
        syn::Type::Paren(paren) => type_is_canonical_mob_command(&paren.elem, aliases),
        syn::Type::Group(group) => type_is_canonical_mob_command(&group.elem, aliases),
        _ => false,
    }
}

fn type_contains_canonical_mob_command(ty: &syn::Type, aliases: &RustUseAliases) -> bool {
    match ty {
        syn::Type::Path(path) => {
            (!(aliases.path_has_shadowed_bare_root(&path.path)
                || (aliases.ambiguous_glob_import
                    && aliases.path_has_unresolved_bare_root(&path.path, "MobCommand")))
                && aliases
                    .resolve_path(&path.path)
                    .into_iter()
                    .any(|segments| path_is_canonical_mob_command_type(&segments)))
                || path.path.segments.iter().any(|segment| {
                    if let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments {
                        arguments.args.iter().any(|arg| match arg {
                            syn::GenericArgument::Type(ty) => {
                                type_contains_canonical_mob_command(ty, aliases)
                            }
                            _ => false,
                        })
                    } else {
                        false
                    }
                })
        }
        syn::Type::Reference(reference) => {
            type_contains_canonical_mob_command(&reference.elem, aliases)
        }
        syn::Type::Paren(paren) => type_contains_canonical_mob_command(&paren.elem, aliases),
        syn::Type::Group(group) => type_contains_canonical_mob_command(&group.elem, aliases),
        _ => false,
    }
}

fn type_contains_canonical_mob_machine_input(ty: &syn::Type) -> bool {
    match ty {
        syn::Type::Path(path) => {
            let segments = path_segments(&path.path);
            path_is_canonical_mob_machine_type(&segments, "MobMachineInput")
                || segments
                    .iter()
                    .map(String::as_str)
                    .eq(["mob_dsl", "MobMachineInput"])
                || path.path.segments.iter().any(|segment| {
                    if let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments {
                        arguments.args.iter().any(|arg| match arg {
                            syn::GenericArgument::Type(ty) => {
                                type_contains_canonical_mob_machine_input(ty)
                            }
                            _ => false,
                        })
                    } else {
                        false
                    }
                })
        }
        syn::Type::Reference(reference) => {
            type_contains_canonical_mob_machine_input(&reference.elem)
        }
        syn::Type::Paren(paren) => type_contains_canonical_mob_machine_input(&paren.elem),
        syn::Type::Group(group) => type_contains_canonical_mob_machine_input(&group.elem),
        _ => false,
    }
}

fn pat_ident_name(pat: &syn::Pat) -> Option<String> {
    match pat {
        syn::Pat::Ident(ident) => Some(ident.ident.to_string()),
        syn::Pat::Reference(reference) => pat_ident_name(&reference.pat),
        syn::Pat::Paren(paren) => pat_ident_name(&paren.pat),
        _ => None,
    }
}

fn pat_bound_idents(pat: &syn::Pat) -> BTreeSet<String> {
    let mut idents = BTreeSet::new();
    collect_pat_bound_idents(pat, &mut idents);
    idents
}

fn collect_pat_bound_idents(pat: &syn::Pat, idents: &mut BTreeSet<String>) {
    match pat {
        syn::Pat::Ident(ident) => {
            idents.insert(ident.ident.to_string());
            if let Some((_, subpat)) = &ident.subpat {
                collect_pat_bound_idents(subpat, idents);
            }
        }
        syn::Pat::Or(or) => {
            for case in &or.cases {
                collect_pat_bound_idents(case, idents);
            }
        }
        syn::Pat::Paren(paren) => collect_pat_bound_idents(&paren.pat, idents),
        syn::Pat::Reference(reference) => collect_pat_bound_idents(&reference.pat, idents),
        syn::Pat::Slice(slice) => {
            for elem in &slice.elems {
                collect_pat_bound_idents(elem, idents);
            }
        }
        syn::Pat::Struct(strukt) => {
            for field in &strukt.fields {
                collect_pat_bound_idents(&field.pat, idents);
            }
        }
        syn::Pat::Tuple(tuple) => {
            for elem in &tuple.elems {
                collect_pat_bound_idents(elem, idents);
            }
        }
        syn::Pat::TupleStruct(tuple) => {
            for elem in &tuple.elems {
                collect_pat_bound_idents(elem, idents);
            }
        }
        syn::Pat::Type(ty) => collect_pat_bound_idents(&ty.pat, idents),
        syn::Pat::Const(_)
        | syn::Pat::Lit(_)
        | syn::Pat::Macro(_)
        | syn::Pat::Path(_)
        | syn::Pat::Range(_)
        | syn::Pat::Rest(_)
        | syn::Pat::Verbatim(_)
        | syn::Pat::Wild(_) => {}
        _ => {}
    }
}

fn block_item_function_names(block: &syn::Block) -> BTreeSet<String> {
    block
        .stmts
        .iter()
        .filter_map(|stmt| match stmt {
            syn::Stmt::Item(syn::Item::Fn(item_fn)) => Some(item_fn.sig.ident.to_string()),
            _ => None,
        })
        .collect()
}

fn block_local_bound_names(block: &syn::Block) -> BTreeSet<String> {
    let mut names = block_item_function_names(block);
    for stmt in &block.stmts {
        if let syn::Stmt::Local(local) = stmt {
            names.extend(pat_bound_idents(&local.pat));
        }
    }
    names
}

fn block_shadows_names(block: &syn::Block, names: &BTreeSet<String>) -> bool {
    block_local_bound_names(block)
        .iter()
        .any(|name| names.contains(name))
}

fn flow_authority_kind_argument(
    expr: &syn::Expr,
    aliases: &RustUseAliases,
) -> Option<FlowReducerFamily> {
    match expr {
        syn::Expr::Call(call) => {
            let syn::Expr::Path(func) = &*call.func else {
                return None;
            };
            if aliases.path_has_shadowed_bare_root(&func.path)
                || (aliases.ambiguous_glob_import
                    && aliases
                        .path_has_unresolved_bare_root(&func.path, "MobMachineFlowAuthorityKind"))
            {
                return None;
            }
            aliases
                .resolve_path(&func.path)
                .into_iter()
                .find_map(|path| flow_authority_kind_family_from_path(&path))
        }
        syn::Expr::Paren(paren) => flow_authority_kind_argument(&paren.expr, aliases),
        syn::Expr::Group(group) => flow_authority_kind_argument(&group.expr, aliases),
        _ => None,
    }
}

fn flow_authority_kind_family_from_path(path: &[String]) -> Option<FlowReducerFamily> {
    let (variant, owner_path) = path.split_last()?;
    let canonical_owner = owner_path
        .iter()
        .map(String::as_str)
        .eq(["MobMachineFlowAuthorityKind"])
        || owner_path.iter().map(String::as_str).eq([
            "crate",
            "run",
            "MobMachineFlowAuthorityKind",
        ])
        || owner_path
            .iter()
            .map(String::as_str)
            .eq(["super", "MobMachineFlowAuthorityKind"]);
    if !canonical_owner {
        return None;
    }
    FlowReducerFamily::from_module(match variant.as_str() {
        "FlowRun" => "flow_run",
        "FlowFrame" => "flow_frame",
        "LoopIteration" => "loop_iteration",
        _ => return None,
    })
}

fn flow_authority_kind_command_parameter(
    expr: &syn::Expr,
    family: FlowReducerFamily,
    command_vars: &BTreeSet<String>,
    aliases: &RustUseAliases,
) -> Option<String> {
    match expr {
        syn::Expr::Call(call) => {
            if flow_authority_kind_argument(expr, aliases) == Some(family) && call.args.len() == 1 {
                call.args
                    .first()
                    .and_then(|arg| expr_command_kind_call_var(arg, command_vars))
            } else {
                None
            }
        }
        syn::Expr::Try(try_expr) => {
            flow_authority_kind_command_parameter(&try_expr.expr, family, command_vars, aliases)
        }
        syn::Expr::Reference(reference) => {
            flow_authority_kind_command_parameter(&reference.expr, family, command_vars, aliases)
        }
        syn::Expr::Paren(paren) => {
            flow_authority_kind_command_parameter(&paren.expr, family, command_vars, aliases)
        }
        syn::Expr::Group(group) => {
            flow_authority_kind_command_parameter(&group.expr, family, command_vars, aliases)
        }
        _ => None,
    }
}

fn expr_command_kind_call_var(expr: &syn::Expr, command_vars: &BTreeSet<String>) -> Option<String> {
    match expr {
        syn::Expr::MethodCall(call) if call.method == "kind" && call.args.is_empty() => {
            expr_variable_name(&call.receiver, command_vars)
        }
        syn::Expr::Reference(reference) => {
            expr_command_kind_call_var(&reference.expr, command_vars)
        }
        syn::Expr::Paren(paren) => expr_command_kind_call_var(&paren.expr, command_vars),
        syn::Expr::Group(group) => expr_command_kind_call_var(&group.expr, command_vars),
        _ => None,
    }
}

fn expr_variable_name(expr: &syn::Expr, names: &BTreeSet<String>) -> Option<String> {
    match expr {
        syn::Expr::Path(_) => expr_path_single_ident(expr).filter(|ident| names.contains(ident)),
        syn::Expr::MethodCall(call) if call.method == "clone" && call.args.is_empty() => {
            expr_variable_name(&call.receiver, names)
        }
        syn::Expr::Reference(reference) => expr_variable_name(&reference.expr, names),
        syn::Expr::Paren(paren) => expr_variable_name(&paren.expr, names),
        syn::Expr::Group(group) => expr_variable_name(&group.expr, names),
        _ => None,
    }
}

fn transition_call_command_input_var(
    call: &syn::ExprCall,
    command_vars: &BTreeSet<String>,
) -> Option<String> {
    call.args
        .iter()
        .find_map(|arg| expr_command_input_var(arg, command_vars))
}

fn expr_command_input_var(expr: &syn::Expr, command_vars: &BTreeSet<String>) -> Option<String> {
    match expr {
        syn::Expr::MethodCall(call) if call.method == "into_input" => {
            expr_variable_name(&call.receiver, command_vars)
        }
        syn::Expr::MethodCall(call) if call.method == "clone" => {
            expr_command_input_var(&call.receiver, command_vars)
        }
        syn::Expr::Try(try_expr) => expr_command_input_var(&try_expr.expr, command_vars),
        syn::Expr::Reference(reference) => expr_command_input_var(&reference.expr, command_vars),
        syn::Expr::Paren(paren) => expr_command_input_var(&paren.expr, command_vars),
        syn::Expr::Group(group) => expr_command_input_var(&group.expr, command_vars),
        _ => None,
    }
}

fn expr_is_ok_constructor_call(expr: &syn::Expr) -> bool {
    match expr {
        syn::Expr::Call(call) => expr_as_path(&call.func).is_some_and(|path| {
            path.segments
                .last()
                .is_some_and(|segment| segment.ident == "Ok")
        }),
        syn::Expr::Paren(paren) => expr_is_ok_constructor_call(&paren.expr),
        syn::Expr::Group(group) => expr_is_ok_constructor_call(&group.expr),
        _ => false,
    }
}

fn result_method_can_swallow_error(method: &str) -> bool {
    matches!(
        method,
        "or" | "or_else"
            | "unwrap_or"
            | "unwrap_or_else"
            | "unwrap_or_default"
            | "ok"
            | "err"
            | "map_or"
            | "map_or_else"
    )
}

fn expr_contains_fail_open_result_method(expr: &syn::Expr) -> bool {
    match expr {
        syn::Expr::MethodCall(call) => {
            result_method_can_swallow_error(&call.method.to_string())
                || expr_contains_fail_open_result_method(&call.receiver)
                || call.args.iter().any(expr_contains_fail_open_result_method)
        }
        syn::Expr::Call(call) => {
            expr_as_path(&call.func)
                .and_then(|path| path.segments.last())
                .is_some_and(|segment| result_method_can_swallow_error(&segment.ident.to_string()))
                || call.args.iter().any(expr_contains_fail_open_result_method)
        }
        syn::Expr::Closure(closure) => expr_contains_fail_open_result_method(&closure.body),
        syn::Expr::Try(try_expr) => expr_contains_fail_open_result_method(&try_expr.expr),
        syn::Expr::Await(await_expr) => expr_contains_fail_open_result_method(&await_expr.base),
        syn::Expr::Reference(reference) => expr_contains_fail_open_result_method(&reference.expr),
        syn::Expr::Paren(paren) => expr_contains_fail_open_result_method(&paren.expr),
        syn::Expr::Group(group) => expr_contains_fail_open_result_method(&group.expr),
        syn::Expr::Block(block) => block
            .block
            .stmts
            .iter()
            .any(stmt_contains_fail_open_result_method),
        syn::Expr::If(expr_if) => {
            expr_contains_fail_open_result_method(&expr_if.cond)
                || expr_if
                    .then_branch
                    .stmts
                    .iter()
                    .any(stmt_contains_fail_open_result_method)
                || expr_if
                    .else_branch
                    .as_ref()
                    .is_some_and(|(_, expr)| expr_contains_fail_open_result_method(expr))
        }
        syn::Expr::Match(expr_match) => {
            expr_contains_fail_open_result_method(&expr_match.expr)
                || expr_match.arms.iter().any(|arm| {
                    arm.guard
                        .as_ref()
                        .is_some_and(|(_, guard)| expr_contains_fail_open_result_method(guard))
                        || expr_contains_fail_open_result_method(&arm.body)
                })
        }
        _ => false,
    }
}

fn stmt_contains_fail_open_result_method(stmt: &syn::Stmt) -> bool {
    match stmt {
        syn::Stmt::Local(local) => local
            .init
            .as_ref()
            .is_some_and(|init| expr_contains_fail_open_result_method(&init.expr)),
        syn::Stmt::Expr(expr, _) => expr_contains_fail_open_result_method(expr),
        syn::Stmt::Item(_) | syn::Stmt::Macro(_) => false,
    }
}

fn stmt_unconditionally_exits_block(stmt: &syn::Stmt) -> bool {
    match stmt {
        syn::Stmt::Expr(expr, _) => expr_unconditionally_exits_block(expr),
        syn::Stmt::Local(_) | syn::Stmt::Item(_) | syn::Stmt::Macro(_) => false,
    }
}

fn expr_unconditionally_exits_block(expr: &syn::Expr) -> bool {
    match expr {
        syn::Expr::Return(_) | syn::Expr::Break(_) | syn::Expr::Continue(_) => true,
        syn::Expr::Group(group) => expr_unconditionally_exits_block(&group.expr),
        syn::Expr::Paren(paren) => expr_unconditionally_exits_block(&paren.expr),
        _ => false,
    }
}

fn path_is_canonical_mob_run_owner_constructor(
    path: &syn::Path,
    aliases: &RustUseAliases,
    owner: &str,
    constructor: impl Fn(&str) -> bool,
) -> bool {
    if aliases.path_has_shadowed_bare_root(path)
        || (aliases.ambiguous_glob_import && aliases.path_has_unresolved_bare_root(path, owner))
    {
        return false;
    }
    aliases.resolve_path(path).into_iter().any(|segments| {
        let Some(last) = segments.last() else {
            return false;
        };
        constructor(last)
            && path_is_canonical_mob_run_owner_type(
                &segments[..segments.len().saturating_sub(1)],
                owner,
            )
    })
}

fn expr_returns_mob_machine_flow_run_command(expr: &syn::Expr, aliases: &RustUseAliases) -> bool {
    match expr {
        syn::Expr::Call(call) => expr_as_path(&call.func).is_some_and(|path| {
            path_is_canonical_mob_run_owner_constructor(
                path,
                aliases,
                "MobMachineFlowRunCommand",
                |constructor| constructor != "from",
            )
        }),
        syn::Expr::MethodCall(call) if call.method == "clone" => {
            expr_returns_mob_machine_flow_run_command(&call.receiver, aliases)
        }
        syn::Expr::Try(try_expr) => {
            expr_returns_mob_machine_flow_run_command(&try_expr.expr, aliases)
        }
        syn::Expr::Await(await_expr) => {
            expr_returns_mob_machine_flow_run_command(&await_expr.base, aliases)
        }
        syn::Expr::Reference(reference) => {
            expr_returns_mob_machine_flow_run_command(&reference.expr, aliases)
        }
        syn::Expr::Paren(paren) => expr_returns_mob_machine_flow_run_command(&paren.expr, aliases),
        syn::Expr::Group(group) => expr_returns_mob_machine_flow_run_command(&group.expr, aliases),
        _ => false,
    }
}

fn expr_returns_mob_machine_flow_authority_token(
    expr: &syn::Expr,
    aliases: &RustUseAliases,
) -> bool {
    match expr {
        syn::Expr::Call(call) => expr_as_path(&call.func).is_some_and(|path| {
            path_is_canonical_mob_run_owner_constructor(
                path,
                aliases,
                "MobMachineFlowAuthorityToken",
                |constructor| constructor == "from_accepted_mob_machine_input",
            )
        }),
        syn::Expr::MethodCall(call) if call.method == "clone" => {
            expr_returns_mob_machine_flow_authority_token(&call.receiver, aliases)
        }
        syn::Expr::Try(try_expr) => {
            expr_returns_mob_machine_flow_authority_token(&try_expr.expr, aliases)
        }
        syn::Expr::Await(await_expr) => {
            expr_returns_mob_machine_flow_authority_token(&await_expr.base, aliases)
        }
        syn::Expr::Reference(reference) => {
            expr_returns_mob_machine_flow_authority_token(&reference.expr, aliases)
        }
        syn::Expr::Paren(paren) => {
            expr_returns_mob_machine_flow_authority_token(&paren.expr, aliases)
        }
        syn::Expr::Group(group) => {
            expr_returns_mob_machine_flow_authority_token(&group.expr, aliases)
        }
        _ => false,
    }
}

fn expr_flow_authority_input_command_var(
    expr: &syn::Expr,
    command_vars: &BTreeSet<String>,
    authority_input_command_vars: &BTreeMap<String, String>,
) -> Option<String> {
    match expr {
        syn::Expr::Path(_) => expr_path_single_ident(expr)
            .and_then(|name| authority_input_command_vars.get(&name).cloned()),
        syn::Expr::MethodCall(call) if call.method == "authority_input" => {
            expr_variable_name(&call.receiver, command_vars)
        }
        syn::Expr::MethodCall(call) if call.method == "clone" => {
            expr_flow_authority_input_command_var(
                &call.receiver,
                command_vars,
                authority_input_command_vars,
            )
        }
        syn::Expr::Try(try_expr) => expr_flow_authority_input_command_var(
            &try_expr.expr,
            command_vars,
            authority_input_command_vars,
        ),
        syn::Expr::Await(await_expr) => expr_flow_authority_input_command_var(
            &await_expr.base,
            command_vars,
            authority_input_command_vars,
        ),
        syn::Expr::Reference(reference) => expr_flow_authority_input_command_var(
            &reference.expr,
            command_vars,
            authority_input_command_vars,
        ),
        syn::Expr::Paren(paren) => expr_flow_authority_input_command_var(
            &paren.expr,
            command_vars,
            authority_input_command_vars,
        ),
        syn::Expr::Group(group) => expr_flow_authority_input_command_var(
            &group.expr,
            command_vars,
            authority_input_command_vars,
        ),
        _ => None,
    }
}

fn expr_flow_authority_input_identity_var(
    expr: &syn::Expr,
    authority_input_identity_vars: &BTreeMap<String, String>,
) -> Option<String> {
    match expr {
        syn::Expr::Path(_) => expr_path_single_ident(expr)
            .and_then(|name| authority_input_identity_vars.get(&name).cloned()),
        syn::Expr::MethodCall(call) if call.method == "authority_input" => {
            call.args.first().and_then(expr_identity_argument_name)
        }
        syn::Expr::MethodCall(call) if call.method == "clone" => {
            expr_flow_authority_input_identity_var(&call.receiver, authority_input_identity_vars)
        }
        syn::Expr::Try(try_expr) => {
            expr_flow_authority_input_identity_var(&try_expr.expr, authority_input_identity_vars)
        }
        syn::Expr::Await(await_expr) => {
            expr_flow_authority_input_identity_var(&await_expr.base, authority_input_identity_vars)
        }
        syn::Expr::Reference(reference) => {
            expr_flow_authority_input_identity_var(&reference.expr, authority_input_identity_vars)
        }
        syn::Expr::Paren(paren) => {
            expr_flow_authority_input_identity_var(&paren.expr, authority_input_identity_vars)
        }
        syn::Expr::Group(group) => {
            expr_flow_authority_input_identity_var(&group.expr, authority_input_identity_vars)
        }
        _ => None,
    }
}

fn expr_is_plan_machine_inputs_prepare_call(
    expr: &syn::Expr,
    plan_vars: &BTreeSet<String>,
) -> bool {
    match expr {
        syn::Expr::MethodCall(call) => {
            call.method == "prepare_dsl_inputs"
                && expr_is_self_receiver(&call.receiver)
                && call
                    .args
                    .first()
                    .is_some_and(|arg| expr_is_plan_machine_inputs_call(arg, plan_vars))
        }
        syn::Expr::Try(try_expr) => {
            expr_is_plan_machine_inputs_prepare_call(&try_expr.expr, plan_vars)
        }
        syn::Expr::Await(await_expr) => {
            expr_is_plan_machine_inputs_prepare_call(&await_expr.base, plan_vars)
        }
        syn::Expr::Reference(reference) => {
            expr_is_plan_machine_inputs_prepare_call(&reference.expr, plan_vars)
        }
        syn::Expr::Paren(paren) => expr_is_plan_machine_inputs_prepare_call(&paren.expr, plan_vars),
        syn::Expr::Group(group) => expr_is_plan_machine_inputs_prepare_call(&group.expr, plan_vars),
        _ => false,
    }
}

fn expr_is_plan_machine_inputs_value(
    expr: &syn::Expr,
    plan_authority_input_vars: &BTreeSet<String>,
    plan_vars: &BTreeSet<String>,
) -> bool {
    if expr_variable_name(expr, plan_authority_input_vars).is_some() {
        return true;
    }
    match expr {
        syn::Expr::MethodCall(call)
            if matches!(call.method.to_string().as_str(), "to_vec" | "clone") =>
        {
            expr_is_plan_machine_inputs_value(&call.receiver, plan_authority_input_vars, plan_vars)
        }
        syn::Expr::MethodCall(call) => {
            call.method == "machine_inputs" && expr_is_plan_scrutinee(&call.receiver, plan_vars)
        }
        syn::Expr::Try(try_expr) => {
            expr_is_plan_machine_inputs_value(&try_expr.expr, plan_authority_input_vars, plan_vars)
        }
        syn::Expr::Await(await_expr) => expr_is_plan_machine_inputs_value(
            &await_expr.base,
            plan_authority_input_vars,
            plan_vars,
        ),
        syn::Expr::Reference(reference) => {
            expr_is_plan_machine_inputs_value(&reference.expr, plan_authority_input_vars, plan_vars)
        }
        syn::Expr::Paren(paren) => {
            expr_is_plan_machine_inputs_value(&paren.expr, plan_authority_input_vars, plan_vars)
        }
        syn::Expr::Group(group) => {
            expr_is_plan_machine_inputs_value(&group.expr, plan_authority_input_vars, plan_vars)
        }
        _ => false,
    }
}

fn expr_is_none_constructor(expr: &syn::Expr, aliases: &RustUseAliases) -> bool {
    match expr {
        syn::Expr::Path(path) => {
            !(aliases.path_has_shadowed_bare_root(&path.path)
                || (aliases.ambiguous_glob_import
                    && aliases.path_has_unresolved_bare_root(&path.path, "None")))
                && aliases
                    .resolve_path(&path.path)
                    .into_iter()
                    .any(|segments| path_is_canonical_none_constructor(&segments))
        }
        syn::Expr::Paren(paren) => expr_is_none_constructor(&paren.expr, aliases),
        syn::Expr::Group(group) => expr_is_none_constructor(&group.expr, aliases),
        _ => false,
    }
}

fn path_is_canonical_none_constructor(path: &[String]) -> bool {
    path.iter().map(String::as_str).eq(["None"])
        || path.iter().map(String::as_str).eq(["Option", "None"])
        || path
            .iter()
            .map(String::as_str)
            .eq(["std", "option", "Option", "None"])
        || path
            .iter()
            .map(String::as_str)
            .eq(["core", "option", "Option", "None"])
}

fn expr_is_plan_freshness_failure_guard(
    expr: &syn::Expr,
    run_id_vars: &BTreeSet<String>,
    plan_vars: &BTreeSet<String>,
) -> bool {
    match expr {
        syn::Expr::Unary(unary) if matches!(unary.op, syn::UnOp::Not(_)) => {
            expr_is_plan_freshness_check(&unary.expr, run_id_vars, plan_vars)
        }
        syn::Expr::Paren(paren) => {
            expr_is_plan_freshness_failure_guard(&paren.expr, run_id_vars, plan_vars)
        }
        syn::Expr::Group(group) => {
            expr_is_plan_freshness_failure_guard(&group.expr, run_id_vars, plan_vars)
        }
        _ => false,
    }
}

fn expr_is_plan_freshness_check(
    expr: &syn::Expr,
    run_id_vars: &BTreeSet<String>,
    plan_vars: &BTreeSet<String>,
) -> bool {
    match expr {
        syn::Expr::MethodCall(call) => {
            call.method == "flow_frame_store_plan_expected_matches"
                && expr_is_self_receiver(&call.receiver)
                && call.args.len() == 2
                && call
                    .args
                    .first()
                    .is_some_and(|arg| expr_variable_name(arg, run_id_vars).is_some())
                && call
                    .args
                    .iter()
                    .nth(1)
                    .is_some_and(|arg| expr_variable_name(arg, plan_vars).is_some())
        }
        syn::Expr::Try(try_expr) => {
            expr_is_plan_freshness_check(&try_expr.expr, run_id_vars, plan_vars)
        }
        syn::Expr::Await(await_expr) => {
            expr_is_plan_freshness_check(&await_expr.base, run_id_vars, plan_vars)
        }
        syn::Expr::Reference(reference) => {
            expr_is_plan_freshness_check(&reference.expr, run_id_vars, plan_vars)
        }
        syn::Expr::Paren(paren) => {
            expr_is_plan_freshness_check(&paren.expr, run_id_vars, plan_vars)
        }
        syn::Expr::Group(group) => {
            expr_is_plan_freshness_check(&group.expr, run_id_vars, plan_vars)
        }
        _ => false,
    }
}

fn block_returns_ok_false(block: &syn::Block) -> bool {
    block.stmts.iter().any(stmt_returns_ok_false)
}

fn stmt_returns_ok_false(stmt: &syn::Stmt) -> bool {
    match stmt {
        syn::Stmt::Expr(expr, _) => expr_returns_ok_false(expr),
        syn::Stmt::Local(_) | syn::Stmt::Item(_) | syn::Stmt::Macro(_) => false,
    }
}

fn expr_returns_ok_false(expr: &syn::Expr) -> bool {
    match expr {
        syn::Expr::Return(ret) => ret.expr.as_deref().is_some_and(expr_is_ok_false),
        syn::Expr::Paren(paren) => expr_returns_ok_false(&paren.expr),
        syn::Expr::Group(group) => expr_returns_ok_false(&group.expr),
        _ => false,
    }
}

fn expr_is_ok_false(expr: &syn::Expr) -> bool {
    match expr {
        syn::Expr::Call(call) => {
            expr_as_path(&call.func)
                .and_then(|path| path.segments.last())
                .is_some_and(|segment| segment.ident == "Ok")
                && call
                    .args
                    .first()
                    .is_some_and(|arg| expr_static_bool_value(arg) == Some(false))
                && call.args.len() == 1
        }
        syn::Expr::Paren(paren) => expr_is_ok_false(&paren.expr),
        syn::Expr::Group(group) => expr_is_ok_false(&group.expr),
        _ => false,
    }
}

fn expr_is_plan_machine_inputs_call(expr: &syn::Expr, plan_vars: &BTreeSet<String>) -> bool {
    match expr {
        syn::Expr::MethodCall(call) => {
            call.method == "machine_inputs" && expr_is_plan_scrutinee(&call.receiver, plan_vars)
        }
        syn::Expr::Reference(reference) => {
            expr_is_plan_machine_inputs_call(&reference.expr, plan_vars)
        }
        syn::Expr::Paren(paren) => expr_is_plan_machine_inputs_call(&paren.expr, plan_vars),
        syn::Expr::Group(group) => expr_is_plan_machine_inputs_call(&group.expr, plan_vars),
        _ => false,
    }
}

fn expr_field_chain(expr: &syn::Expr) -> Vec<String> {
    match expr {
        syn::Expr::Field(field) => {
            let mut chain = expr_field_chain(&field.base);
            if let syn::Member::Named(ident) = &field.member {
                chain.push(ident.to_string());
            }
            chain
        }
        syn::Expr::Path(path) => path
            .path
            .segments
            .last()
            .map(|segment| vec![segment.ident.to_string()])
            .unwrap_or_default(),
        syn::Expr::Index(index) => expr_field_chain(&index.expr),
        syn::Expr::Reference(reference) => expr_field_chain(&reference.expr),
        syn::Expr::Unary(unary) if matches!(unary.op, syn::UnOp::Deref(_)) => {
            expr_field_chain(&unary.expr)
        }
        syn::Expr::Paren(paren) => expr_field_chain(&paren.expr),
        syn::Expr::Group(group) => expr_field_chain(&group.expr),
        syn::Expr::Await(await_expr) => expr_field_chain(&await_expr.base),
        _ => Vec::new(),
    }
}

fn projection_state_alias_from_expr(expr: &syn::Expr) -> Option<&'static str> {
    let chain = expr_field_chain(expr);
    if chain.iter().any(|segment| segment == "flow_state") {
        Some("flow_state")
    } else if chain.iter().any(|segment| segment == "kernel_state") {
        Some("kernel_state")
    } else {
        None
    }
}

fn projection_state_token_from_chain(chain: &[String]) -> Option<&'static str> {
    if chain.iter().any(|segment| segment == "flow_state") {
        Some(".flow_state")
    } else if chain.iter().any(|segment| segment == "kernel_state") {
        Some(".kernel_state")
    } else {
        None
    }
}

fn path_is_canonical_mob_run_projection_owner(path: &[String]) -> bool {
    path.iter().map(String::as_str).eq(["MobRun"])
        || path
            .iter()
            .map(String::as_str)
            .eq(["crate", "run", "MobRun"])
}

fn path_is_canonical_kernel_snapshot_projection_owner(path: &[String]) -> bool {
    path.iter().map(String::as_str).eq(["FrameSnapshot"])
        || path.iter().map(String::as_str).eq(["LoopSnapshot"])
        || path
            .iter()
            .map(String::as_str)
            .eq(["crate", "run", "FrameSnapshot"])
        || path
            .iter()
            .map(String::as_str)
            .eq(["crate", "run", "LoopSnapshot"])
}

fn projection_field_token_from_chain(
    chain: &[String],
    state_field: &str,
    governed_fields: &BTreeSet<String>,
) -> Option<String> {
    chain.windows(2).find_map(|window| {
        if window[0] == state_field && governed_fields.contains(&window[1]) {
            Some(format!(".{state_field}.{}", window[1]))
        } else {
            None
        }
    })
}

fn method_call_typed_next_state_command_var(
    call: &syn::ExprMethodCall,
    facts: &ProjectionCasMethodFacts,
    typed_outcome_command_vars: &BTreeMap<String, String>,
    typed_next_state_command_vars: &BTreeMap<String, String>,
) -> Option<String> {
    facts.next_flow_state_arg_indices.iter().find_map(|index| {
        call.args.iter().nth(*index).and_then(|arg| {
            expr_typed_outcome_next_state_command_var(
                arg,
                typed_outcome_command_vars,
                typed_next_state_command_vars,
            )
        })
    })
}

fn method_call_typed_next_state_identity_var(
    call: &syn::ExprMethodCall,
    facts: &ProjectionCasMethodFacts,
    typed_outcome_identity_vars: &BTreeMap<String, String>,
    typed_next_state_identity_vars: &BTreeMap<String, String>,
) -> Option<String> {
    facts.next_flow_state_arg_indices.iter().find_map(|index| {
        call.args.iter().nth(*index).and_then(|arg| {
            expr_typed_outcome_next_state_identity_var(
                arg,
                typed_outcome_identity_vars,
                typed_next_state_identity_vars,
            )
        })
    })
}

fn call_typed_next_state_command_var(
    call: &syn::ExprCall,
    path: &syn::Path,
    facts: &ProjectionCasMethodFacts,
    typed_outcome_command_vars: &BTreeMap<String, String>,
    typed_next_state_command_vars: &BTreeMap<String, String>,
) -> Option<String> {
    let receiver_offset = usize::from(path.segments.len() > 1);
    facts.next_flow_state_arg_indices.iter().find_map(|index| {
        call.args
            .iter()
            .nth(index + receiver_offset)
            .and_then(|arg| {
                expr_typed_outcome_next_state_command_var(
                    arg,
                    typed_outcome_command_vars,
                    typed_next_state_command_vars,
                )
            })
    })
}

fn call_typed_next_state_identity_var(
    call: &syn::ExprCall,
    path: &syn::Path,
    facts: &ProjectionCasMethodFacts,
    typed_outcome_identity_vars: &BTreeMap<String, String>,
    typed_next_state_identity_vars: &BTreeMap<String, String>,
) -> Option<String> {
    let receiver_offset = usize::from(path.segments.len() > 1);
    facts.next_flow_state_arg_indices.iter().find_map(|index| {
        call.args
            .iter()
            .nth(index + receiver_offset)
            .and_then(|arg| {
                expr_typed_outcome_next_state_identity_var(
                    arg,
                    typed_outcome_identity_vars,
                    typed_next_state_identity_vars,
                )
            })
    })
}

fn expr_typed_outcome_next_state_command_var(
    expr: &syn::Expr,
    typed_outcome_command_vars: &BTreeMap<String, String>,
    typed_next_state_command_vars: &BTreeMap<String, String>,
) -> Option<String> {
    match expr {
        syn::Expr::Path(_) => expr_path_single_ident(expr)
            .and_then(|name| typed_next_state_command_vars.get(&name).cloned()),
        syn::Expr::Field(_) => {
            let chain = expr_field_chain(expr);
            chain
                .first()
                .and_then(|name| typed_outcome_command_vars.get(name))
                .filter(|_| chain.last().is_some_and(|field| field == "next_state"))
                .cloned()
        }
        syn::Expr::MethodCall(call) if call.method == "clone" => {
            expr_typed_outcome_next_state_command_var(
                &call.receiver,
                typed_outcome_command_vars,
                typed_next_state_command_vars,
            )
        }
        syn::Expr::Try(try_expr) => expr_typed_outcome_next_state_command_var(
            &try_expr.expr,
            typed_outcome_command_vars,
            typed_next_state_command_vars,
        ),
        syn::Expr::Await(await_expr) => expr_typed_outcome_next_state_command_var(
            &await_expr.base,
            typed_outcome_command_vars,
            typed_next_state_command_vars,
        ),
        syn::Expr::Reference(reference) => expr_typed_outcome_next_state_command_var(
            &reference.expr,
            typed_outcome_command_vars,
            typed_next_state_command_vars,
        ),
        syn::Expr::Paren(paren) => expr_typed_outcome_next_state_command_var(
            &paren.expr,
            typed_outcome_command_vars,
            typed_next_state_command_vars,
        ),
        syn::Expr::Group(group) => expr_typed_outcome_next_state_command_var(
            &group.expr,
            typed_outcome_command_vars,
            typed_next_state_command_vars,
        ),
        _ => None,
    }
}

fn expr_typed_outcome_next_state_identity_var(
    expr: &syn::Expr,
    typed_outcome_identity_vars: &BTreeMap<String, String>,
    typed_next_state_identity_vars: &BTreeMap<String, String>,
) -> Option<String> {
    match expr {
        syn::Expr::Path(_) => expr_path_single_ident(expr)
            .and_then(|name| typed_next_state_identity_vars.get(&name).cloned()),
        syn::Expr::Field(_) => {
            let chain = expr_field_chain(expr);
            chain
                .first()
                .and_then(|name| typed_outcome_identity_vars.get(name))
                .filter(|_| chain.last().is_some_and(|field| field == "next_state"))
                .cloned()
        }
        syn::Expr::MethodCall(call) if call.method == "clone" => {
            expr_typed_outcome_next_state_identity_var(
                &call.receiver,
                typed_outcome_identity_vars,
                typed_next_state_identity_vars,
            )
        }
        syn::Expr::Try(try_expr) => expr_typed_outcome_next_state_identity_var(
            &try_expr.expr,
            typed_outcome_identity_vars,
            typed_next_state_identity_vars,
        ),
        syn::Expr::Await(await_expr) => expr_typed_outcome_next_state_identity_var(
            &await_expr.base,
            typed_outcome_identity_vars,
            typed_next_state_identity_vars,
        ),
        syn::Expr::Reference(reference) => expr_typed_outcome_next_state_identity_var(
            &reference.expr,
            typed_outcome_identity_vars,
            typed_next_state_identity_vars,
        ),
        syn::Expr::Paren(paren) => expr_typed_outcome_next_state_identity_var(
            &paren.expr,
            typed_outcome_identity_vars,
            typed_next_state_identity_vars,
        ),
        syn::Expr::Group(group) => expr_typed_outcome_next_state_identity_var(
            &group.expr,
            typed_outcome_identity_vars,
            typed_next_state_identity_vars,
        ),
        _ => None,
    }
}

fn method_call_uses_matching_flow_authority_input(
    call: &syn::ExprMethodCall,
    facts: &ProjectionCasMethodFacts,
    command_var: &str,
    authority_input_command_vars: &BTreeMap<String, String>,
    authority_input_identity_vars: &BTreeMap<String, String>,
    command_vars: &BTreeSet<String>,
) -> bool {
    let Some(required_identity_vars) =
        method_call_authority_identity_vars(call, &facts.authority_identity_arg_indices)
    else {
        return false;
    };
    !facts.authority_input_arg_indices.is_empty()
        && facts.authority_input_arg_indices.iter().any(|index| {
            call.args.iter().nth(*index).is_some_and(|arg| {
                expr_contains_flow_authority_input_for_command(
                    arg,
                    command_var,
                    authority_input_command_vars,
                    authority_input_identity_vars,
                    &required_identity_vars,
                    command_vars,
                )
            })
        })
}

fn method_call_uses_matching_typed_flow_identity(
    call: &syn::ExprMethodCall,
    facts: &ProjectionCasMethodFacts,
    typed_identity_var: Option<&str>,
) -> bool {
    let Some(typed_identity_var) = typed_identity_var else {
        return false;
    };
    let Some(required_identity_vars) =
        method_call_authority_identity_vars(call, &facts.authority_identity_arg_indices)
    else {
        return false;
    };
    required_identity_vars.contains(typed_identity_var)
}

fn call_uses_matching_flow_authority_input(
    call: &syn::ExprCall,
    path: &syn::Path,
    facts: &ProjectionCasMethodFacts,
    command_var: &str,
    authority_input_command_vars: &BTreeMap<String, String>,
    authority_input_identity_vars: &BTreeMap<String, String>,
    command_vars: &BTreeSet<String>,
) -> bool {
    let receiver_offset = usize::from(path.segments.len() > 1);
    let Some(required_identity_vars) =
        call_authority_identity_vars(call, receiver_offset, &facts.authority_identity_arg_indices)
    else {
        return false;
    };
    !facts.authority_input_arg_indices.is_empty()
        && facts.authority_input_arg_indices.iter().any(|index| {
            call.args
                .iter()
                .nth(index + receiver_offset)
                .is_some_and(|arg| {
                    expr_contains_flow_authority_input_for_command(
                        arg,
                        command_var,
                        authority_input_command_vars,
                        authority_input_identity_vars,
                        &required_identity_vars,
                        command_vars,
                    )
                })
        })
}

fn call_uses_matching_typed_flow_identity(
    call: &syn::ExprCall,
    path: &syn::Path,
    facts: &ProjectionCasMethodFacts,
    typed_identity_var: Option<&str>,
) -> bool {
    let Some(typed_identity_var) = typed_identity_var else {
        return false;
    };
    let receiver_offset = usize::from(path.segments.len() > 1);
    let Some(required_identity_vars) =
        call_authority_identity_vars(call, receiver_offset, &facts.authority_identity_arg_indices)
    else {
        return false;
    };
    required_identity_vars.contains(typed_identity_var)
}

fn method_call_authority_identity_vars(
    call: &syn::ExprMethodCall,
    indices: &BTreeSet<usize>,
) -> Option<BTreeSet<String>> {
    let identities = indices
        .iter()
        .filter_map(|index| {
            call.args
                .iter()
                .nth(*index)
                .and_then(expr_identity_argument_name)
        })
        .collect::<BTreeSet<_>>();
    (!identities.is_empty() || indices.is_empty()).then_some(identities)
}

fn call_authority_identity_vars(
    call: &syn::ExprCall,
    receiver_offset: usize,
    indices: &BTreeSet<usize>,
) -> Option<BTreeSet<String>> {
    let identities = indices
        .iter()
        .filter_map(|index| {
            call.args
                .iter()
                .nth(index + receiver_offset)
                .and_then(expr_identity_argument_name)
        })
        .collect::<BTreeSet<_>>();
    (!identities.is_empty() || indices.is_empty()).then_some(identities)
}

fn expr_contains_flow_authority_input_for_command(
    expr: &syn::Expr,
    command_var: &str,
    authority_input_command_vars: &BTreeMap<String, String>,
    authority_input_identity_vars: &BTreeMap<String, String>,
    required_identity_vars: &BTreeSet<String>,
    command_vars: &BTreeSet<String>,
) -> bool {
    match expr {
        syn::Expr::Path(_) => expr_path_single_ident(expr).is_some_and(|name| {
            authority_input_command_vars
                .get(&name)
                .is_some_and(|command| command == command_var)
                && authority_input_identity_matches(
                    authority_input_identity_vars.get(&name),
                    required_identity_vars,
                )
        }),
        syn::Expr::MethodCall(call) if call.method == "authority_input" => {
            expr_variable_name(&call.receiver, command_vars).as_deref() == Some(command_var)
                && authority_input_identity_matches(
                    call.args
                        .first()
                        .and_then(expr_identity_argument_name)
                        .as_ref(),
                    required_identity_vars,
                )
        }
        syn::Expr::MethodCall(call) if call.method == "clone" => {
            expr_contains_flow_authority_input_for_command(
                &call.receiver,
                command_var,
                authority_input_command_vars,
                authority_input_identity_vars,
                required_identity_vars,
                command_vars,
            )
        }
        syn::Expr::Array(array) => {
            !array.elems.is_empty()
                && array.elems.iter().all(|elem| {
                    expr_contains_flow_authority_input_for_command(
                        elem,
                        command_var,
                        authority_input_command_vars,
                        authority_input_identity_vars,
                        required_identity_vars,
                        command_vars,
                    )
                })
        }
        syn::Expr::Macro(mac) if mac.mac.path.is_ident("vec") => {
            let parser = syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated;
            parser
                .parse2(mac.mac.tokens.clone())
                .ok()
                .is_some_and(|exprs| {
                    !exprs.is_empty()
                        && exprs.iter().all(|expr| {
                            expr_contains_flow_authority_input_for_command(
                                expr,
                                command_var,
                                authority_input_command_vars,
                                authority_input_identity_vars,
                                required_identity_vars,
                                command_vars,
                            )
                        })
                })
        }
        syn::Expr::Reference(reference) => expr_contains_flow_authority_input_for_command(
            &reference.expr,
            command_var,
            authority_input_command_vars,
            authority_input_identity_vars,
            required_identity_vars,
            command_vars,
        ),
        syn::Expr::Paren(paren) => expr_contains_flow_authority_input_for_command(
            &paren.expr,
            command_var,
            authority_input_command_vars,
            authority_input_identity_vars,
            required_identity_vars,
            command_vars,
        ),
        syn::Expr::Group(group) => expr_contains_flow_authority_input_for_command(
            &group.expr,
            command_var,
            authority_input_command_vars,
            authority_input_identity_vars,
            required_identity_vars,
            command_vars,
        ),
        _ => false,
    }
}

fn authority_input_identity_matches(
    identity: Option<&String>,
    required_identity_vars: &BTreeSet<String>,
) -> bool {
    required_identity_vars.is_empty()
        || identity.is_some_and(|identity| required_identity_vars.contains(identity))
}

fn projection_mutator_method(method: &str) -> bool {
    matches!(
        method,
        "append"
            | "clear"
            | "clone_from"
            | "as_mut"
            | "as_mut_slice"
            | "back_mut"
            | "chunks_exact_mut"
            | "chunks_mut"
            | "dedup"
            | "dedup_by"
            | "dedup_by_key"
            | "drain"
            | "entry"
            | "extend"
            | "first_entry"
            | "first_mut"
            | "front_mut"
            | "get_mut"
            | "insert"
            | "iter_mut"
            | "last_entry"
            | "last_mut"
            | "pop"
            | "pop_first"
            | "pop_last"
            | "push"
            | "range_mut"
            | "rchunks_exact_mut"
            | "rchunks_mut"
            | "replace"
            | "remove"
            | "retain"
            | "retain_mut"
            | "resize"
            | "resize_with"
            | "reverse"
            | "rotate_left"
            | "rotate_right"
            | "sort"
            | "sort_by"
            | "sort_by_cached_key"
            | "sort_by_key"
            | "splice"
            | "split_at_mut"
            | "split_first_mut"
            | "split_last_mut"
            | "split_off"
            | "swap"
            | "swap_remove"
            | "take"
            | "truncate"
            | "values_mut"
    )
}

fn flow_provenance_mutator_method(method: &str) -> bool {
    projection_mutator_method(method) || matches!(method, "clone_from")
}

fn gate_provenance_mutator_method(method: &str) -> bool {
    flow_provenance_mutator_method(method)
}

fn projection_method_preserves_typed_flow_state(method: &str) -> bool {
    matches!(method, "clone")
}

fn bin_op_is_assign(op: &syn::BinOp) -> bool {
    matches!(
        op,
        syn::BinOp::AddAssign(_)
            | syn::BinOp::SubAssign(_)
            | syn::BinOp::MulAssign(_)
            | syn::BinOp::DivAssign(_)
            | syn::BinOp::RemAssign(_)
            | syn::BinOp::BitXorAssign(_)
            | syn::BinOp::BitAndAssign(_)
            | syn::BinOp::BitOrAssign(_)
            | syn::BinOp::ShlAssign(_)
            | syn::BinOp::ShrAssign(_)
    )
}

fn canonical_gated_mob_command_variants() -> BTreeSet<String> {
    canonical_mob_machine_command_gate_requirements()
        .into_iter()
        .map(|requirement| requirement.command.as_str().to_string())
        .collect()
}

fn reply_tx_vars_from_pat(pat: &syn::Pat) -> BTreeSet<String> {
    match pat {
        syn::Pat::Struct(strukt) => strukt
            .fields
            .iter()
            .filter(|field| member_key(&field.member) == "reply_tx")
            .flat_map(|field| pat_bound_idents(&field.pat))
            .collect(),
        syn::Pat::Or(or) => or.cases.iter().flat_map(reply_tx_vars_from_pat).collect(),
        syn::Pat::Reference(reference) => reply_tx_vars_from_pat(&reference.pat),
        syn::Pat::Paren(paren) => reply_tx_vars_from_pat(&paren.pat),
        syn::Pat::Ident(ident) if ident.ident == "reply_tx" => {
            BTreeSet::from([ident.ident.to_string()])
        }
        _ => BTreeSet::new(),
    }
}

fn pat_is_catch_all_command_arm(pat: &syn::Pat) -> bool {
    match pat {
        syn::Pat::Wild(_) => true,
        syn::Pat::Ident(ident) => ident.subpat.is_none(),
        syn::Pat::Reference(reference) => pat_is_catch_all_command_arm(&reference.pat),
        syn::Pat::Paren(paren) => pat_is_catch_all_command_arm(&paren.pat),
        syn::Pat::Type(ty) => pat_is_catch_all_command_arm(&ty.pat),
        _ => false,
    }
}

fn mob_command_variants_from_pat(
    pat: &syn::Pat,
    aliases: &RustUseAliases,
    command_variants: &BTreeSet<String>,
) -> BTreeSet<String> {
    match pat {
        syn::Pat::Struct(pat) => {
            mob_command_variant_from_path(&pat.path, aliases, command_variants)
                .into_iter()
                .collect()
        }
        syn::Pat::TupleStruct(pat) => {
            mob_command_variant_from_path(&pat.path, aliases, command_variants)
                .into_iter()
                .collect()
        }
        syn::Pat::Path(pat) => mob_command_variant_from_path(&pat.path, aliases, command_variants)
            .into_iter()
            .collect(),
        syn::Pat::Or(or) => or
            .cases
            .iter()
            .flat_map(|pat| mob_command_variants_from_pat(pat, aliases, command_variants))
            .collect(),
        syn::Pat::Ident(ident) => ident
            .subpat
            .as_ref()
            .map(|(_, pat)| mob_command_variants_from_pat(pat, aliases, command_variants))
            .unwrap_or_default(),
        syn::Pat::Reference(reference) => {
            mob_command_variants_from_pat(&reference.pat, aliases, command_variants)
        }
        syn::Pat::Paren(paren) => {
            mob_command_variants_from_pat(&paren.pat, aliases, command_variants)
        }
        syn::Pat::Type(ty) => mob_command_variants_from_pat(&ty.pat, aliases, command_variants),
        _ => BTreeSet::new(),
    }
}

fn mob_command_variant_from_path(
    path: &syn::Path,
    aliases: &RustUseAliases,
    command_variants: &BTreeSet<String>,
) -> Option<String> {
    if aliases.path_has_shadowed_bare_root(path) {
        return None;
    }
    let segments = path_segments(path);
    if aliases.ambiguous_glob_import && aliases.path_has_unresolved_bare_root(path, "MobCommand") {
        return None;
    }
    if segments.len() == 1
        && let Some(variant) = segments.first()
        && command_variants.contains(variant)
    {
        if aliases.ambiguous_glob_import && aliases.path_is_unresolved_bare_ident(path, variant) {
            return None;
        }
        return Some(variant.clone());
    }
    aliases.resolve_path(path).into_iter().find_map(|segments| {
        canonical_mob_command_variant_from_segments(&segments, command_variants)
    })
}

fn canonical_mob_command_variant_from_segments(
    segments: &[String],
    command_variants: &BTreeSet<String>,
) -> Option<String> {
    command_variants.iter().find_map(|variant| {
        path_is_canonical_mob_command_variant(segments, variant.as_str()).then(|| variant.clone())
    })
}

fn mob_gate_call_function(name: &str) -> bool {
    matches!(
        name,
        "apply_dsl_input"
            | "prepare_dsl_input"
            | "preview_dsl_input"
            | "apply_command_admission"
            | "prepare_command_admission"
            | "probe_mob_machine_input"
    )
}

fn mob_side_effect_call(call: &syn::ExprMethodCall, receiver_aliases: &BTreeSet<String>) -> bool {
    let method = call.method.to_string();
    let receiver_root = expr_root_ident_through_method_calls(&call.receiver);
    let receiver_is_actor_owned = receiver_root
        .as_deref()
        .is_some_and(|root| root == "self" || receiver_aliases.contains(root));
    match method.as_str() {
        "cancel" | "abort" => true,
        "insert" | "set" | "remove" => receiver_is_actor_owned,
        "cancel_unfinished_steps" | "terminalize_canceled" | "terminalize_failed" => true,
        "create_pending_run"
        | "create_task"
        | "create_task_with_id"
        | "interrupt_member"
        | "terminalize_canceled_in_actor"
        | "apply_dsl_signal" => receiver_is_actor_owned,
        _ => false,
    }
}

fn mob_side_effect_path_call(call: &syn::ExprCall, receiver_aliases: &BTreeSet<String>) -> bool {
    let Some(path) = expr_as_path(&call.func) else {
        return false;
    };
    let Some(function) = path
        .segments
        .last()
        .map(|segment| segment.ident.to_string())
    else {
        return false;
    };
    match function.as_str() {
        "cancel" | "abort" => true,
        "spawn" => path_is_tokio_spawn(path),
        "set" | "remove" => call
            .args
            .first()
            .and_then(expr_root_ident_through_method_calls)
            .as_deref()
            .is_some_and(|root| root == "self" || receiver_aliases.contains(root)),
        "cancel_unfinished_steps" | "terminalize_canceled" | "terminalize_failed" => true,
        "create_pending_run"
        | "create_task"
        | "create_task_with_id"
        | "interrupt_member"
        | "terminalize_canceled_in_actor"
        | "apply_dsl_signal" => call
            .args
            .first()
            .and_then(expr_root_ident_through_method_calls)
            .as_deref()
            .is_some_and(|root| root == "self" || receiver_aliases.contains(root)),
        _ => false,
    }
}

fn path_is_tokio_spawn(path: &syn::Path) -> bool {
    let segments = path_segments(path);
    segments.iter().map(String::as_str).eq(["tokio", "spawn"])
        || segments
            .iter()
            .map(String::as_str)
            .eq(["crate", "tokio", "spawn"])
}

fn expr_root_ident_through_method_calls(expr: &syn::Expr) -> Option<String> {
    match expr {
        syn::Expr::Path(_) => expr_path_single_ident(expr),
        syn::Expr::Field(field) => expr_root_ident_through_method_calls(&field.base),
        syn::Expr::Index(index) => expr_root_ident_through_method_calls(&index.expr),
        syn::Expr::MethodCall(call) => expr_root_ident_through_method_calls(&call.receiver),
        syn::Expr::Reference(reference) => expr_root_ident_through_method_calls(&reference.expr),
        syn::Expr::Unary(unary) if matches!(unary.op, syn::UnOp::Deref(_)) => {
            expr_root_ident_through_method_calls(&unary.expr)
        }
        syn::Expr::Paren(paren) => expr_root_ident_through_method_calls(&paren.expr),
        syn::Expr::Group(group) => expr_root_ident_through_method_calls(&group.expr),
        syn::Expr::Await(await_expr) => expr_root_ident_through_method_calls(&await_expr.base),
        _ => None,
    }
}

fn expr_field_chain_through_method_calls(expr: &syn::Expr) -> Vec<String> {
    match expr {
        syn::Expr::Field(field) => {
            let mut chain = expr_field_chain_through_method_calls(&field.base);
            if let syn::Member::Named(ident) = &field.member {
                chain.push(ident.to_string());
            }
            chain
        }
        syn::Expr::Path(_) => expr_field_chain(expr),
        syn::Expr::Index(index) => expr_field_chain_through_method_calls(&index.expr),
        syn::Expr::MethodCall(call) => expr_field_chain_through_method_calls(&call.receiver),
        syn::Expr::Reference(reference) => expr_field_chain_through_method_calls(&reference.expr),
        syn::Expr::Unary(unary) if matches!(unary.op, syn::UnOp::Deref(_)) => {
            expr_field_chain_through_method_calls(&unary.expr)
        }
        syn::Expr::Paren(paren) => expr_field_chain_through_method_calls(&paren.expr),
        syn::Expr::Group(group) => expr_field_chain_through_method_calls(&group.expr),
        syn::Expr::Await(await_expr) => expr_field_chain_through_method_calls(&await_expr.base),
        _ => Vec::new(),
    }
}

fn expr_is_self_mob_side_effect_field(expr: &syn::Expr) -> bool {
    let chain = expr_field_chain_through_method_calls(expr);
    chain.first().is_some_and(|segment| segment == "self")
        && chain.get(1).is_some_and(|segment| {
            matches!(
                segment.as_str(),
                "spawn_policy" | "task_board_service" | "provisioner"
            )
        })
}

fn expr_root_idents_through_calls(expr: &syn::Expr) -> BTreeSet<String> {
    let mut roots = BTreeSet::new();
    collect_expr_root_idents_through_calls(expr, &mut roots);
    roots
}

fn collect_expr_root_idents_through_calls(expr: &syn::Expr, roots: &mut BTreeSet<String>) {
    if let Some(root) = expr_root_ident_through_method_calls(expr) {
        roots.insert(root);
    }
    match expr {
        syn::Expr::Array(array) => {
            for elem in &array.elems {
                collect_expr_root_idents_through_calls(elem, roots);
            }
        }
        syn::Expr::Call(call) => {
            for arg in &call.args {
                collect_expr_root_idents_through_calls(arg, roots);
            }
        }
        syn::Expr::MethodCall(call) => {
            collect_expr_root_idents_through_calls(&call.receiver, roots);
            for arg in &call.args {
                collect_expr_root_idents_through_calls(arg, roots);
            }
        }
        syn::Expr::Tuple(tuple) => {
            for elem in &tuple.elems {
                collect_expr_root_idents_through_calls(elem, roots);
            }
        }
        syn::Expr::Reference(reference) => {
            collect_expr_root_idents_through_calls(&reference.expr, roots);
        }
        syn::Expr::Paren(paren) => collect_expr_root_idents_through_calls(&paren.expr, roots),
        syn::Expr::Group(group) => collect_expr_root_idents_through_calls(&group.expr, roots),
        syn::Expr::Try(try_expr) => collect_expr_root_idents_through_calls(&try_expr.expr, roots),
        syn::Expr::Await(await_expr) => {
            collect_expr_root_idents_through_calls(&await_expr.base, roots);
        }
        syn::Expr::Unary(unary) if matches!(unary.op, syn::UnOp::Deref(_)) => {
            collect_expr_root_idents_through_calls(&unary.expr, roots);
        }
        _ => {}
    }
}

fn self_method_call_name_from_expr(expr: &syn::Expr) -> Option<String> {
    match expr {
        syn::Expr::MethodCall(call) if expr_is_self_receiver(&call.receiver) => {
            Some(call.method.to_string())
        }
        syn::Expr::MethodCall(call) if call.method == "map_err" => {
            self_method_call_name_from_expr(&call.receiver)
        }
        syn::Expr::Try(try_expr) => self_method_call_name_from_expr(&try_expr.expr),
        syn::Expr::Await(await_expr) => self_method_call_name_from_expr(&await_expr.base),
        syn::Expr::Reference(reference) => self_method_call_name_from_expr(&reference.expr),
        syn::Expr::Paren(paren) => self_method_call_name_from_expr(&paren.expr),
        syn::Expr::Group(group) => self_method_call_name_from_expr(&group.expr),
        syn::Expr::Match(expr_match) => {
            let calls = expr_match
                .arms
                .iter()
                .filter_map(|arm| self_method_call_name_from_expr(&arm.body))
                .collect::<BTreeSet<_>>();
            if calls.len() == 1 {
                calls.into_iter().next()
            } else {
                None
            }
        }
        _ => None,
    }
}

fn mob_gate_method_input(
    call: &syn::ExprMethodCall,
    input_vars: &BTreeMap<String, MobMachineInputVariant>,
    aliases: &RustUseAliases,
) -> Option<MobMachineInputVariant> {
    if !mob_gate_call_function(&call.method.to_string()) || !expr_is_self_receiver(&call.receiver) {
        return None;
    }
    call.args
        .first()
        .and_then(|arg| mob_input_variant_from_expr(arg, input_vars, aliases))
}

fn expr_is_self_receiver(expr: &syn::Expr) -> bool {
    expr_path_single_ident(expr).is_some_and(|ident| ident == "self")
}

fn mob_gate_input_from_expr(
    expr: &syn::Expr,
    input_vars: &BTreeMap<String, MobMachineInputVariant>,
    aliases: &RustUseAliases,
) -> Option<MobMachineInputVariant> {
    match expr {
        syn::Expr::MethodCall(call) => {
            let method = call.method.to_string();
            if method == "map_err" {
                return mob_gate_input_from_expr(&call.receiver, input_vars, aliases);
            }
            if let Some(input) = mob_gate_method_input(call, input_vars, aliases) {
                return Some(input);
            }
            None
        }
        syn::Expr::Call(_) => None,
        syn::Expr::Try(try_expr) => mob_gate_input_from_expr(&try_expr.expr, input_vars, aliases),
        syn::Expr::Await(await_expr) => {
            mob_gate_input_from_expr(&await_expr.base, input_vars, aliases)
        }
        syn::Expr::Reference(reference) => {
            mob_gate_input_from_expr(&reference.expr, input_vars, aliases)
        }
        syn::Expr::Paren(paren) => mob_gate_input_from_expr(&paren.expr, input_vars, aliases),
        syn::Expr::Group(group) => mob_gate_input_from_expr(&group.expr, input_vars, aliases),
        _ => None,
    }
}

fn mob_input_variant_from_expr(
    expr: &syn::Expr,
    input_vars: &BTreeMap<String, MobMachineInputVariant>,
    aliases: &RustUseAliases,
) -> Option<MobMachineInputVariant> {
    match expr {
        syn::Expr::Struct(expr) => mob_input_variant_from_path(&expr.path, aliases),
        syn::Expr::Path(expr) => {
            let segments = path_segments(&expr.path);
            if segments.len() == 1
                && let Some(input) = input_vars.get(&segments[0])
            {
                return Some(*input);
            }
            mob_input_variant_from_path(&expr.path, aliases)
        }
        syn::Expr::Call(call) => {
            let syn::Expr::Path(func) = &*call.func else {
                return None;
            };
            if path_is_canonical_mob_run_flow_input_constructor(&func.path, aliases) {
                return Some(MobMachineInputVariant::RunFlow);
            }
            mob_input_variant_from_path(&func.path, aliases)
        }
        syn::Expr::MethodCall(call) if call.method == "clone" => {
            mob_input_variant_from_expr(&call.receiver, input_vars, aliases)
        }
        syn::Expr::Try(try_expr) => {
            mob_input_variant_from_expr(&try_expr.expr, input_vars, aliases)
        }
        syn::Expr::Reference(reference) => {
            mob_input_variant_from_expr(&reference.expr, input_vars, aliases)
        }
        syn::Expr::Paren(paren) => mob_input_variant_from_expr(&paren.expr, input_vars, aliases),
        syn::Expr::Group(group) => mob_input_variant_from_expr(&group.expr, input_vars, aliases),
        _ => None,
    }
}

fn path_is_canonical_mob_run_flow_input_constructor(
    path: &syn::Path,
    aliases: &RustUseAliases,
) -> bool {
    if aliases.path_has_shadowed_bare_root(path) {
        return false;
    }
    if aliases.ambiguous_glob_import && aliases.path_has_unresolved_bare_root(path, "MobRun") {
        return false;
    }
    aliases.resolve_path(path).into_iter().any(|segments| {
        segments
            .last()
            .is_some_and(|method| method == "run_flow_input")
            && path_is_canonical_mob_run_projection_owner(
                &segments[..segments.len().saturating_sub(1)],
            )
    })
}

fn mob_input_variant_from_path(
    path: &syn::Path,
    aliases: &RustUseAliases,
) -> Option<MobMachineInputVariant> {
    if aliases.path_has_shadowed_bare_root(path) {
        return None;
    }
    if aliases.ambiguous_glob_import
        && aliases.path_has_unresolved_bare_root(path, "MobMachineInput")
    {
        return None;
    }
    aliases
        .resolve_path(path)
        .into_iter()
        .find_map(|segments| canonical_mob_input_variant_from_segments(&segments))
}

fn canonical_mob_input_variant_from_segments(
    segments: &[String],
) -> Option<MobMachineInputVariant> {
    MobMachineInput::variant_manifest()
        .iter()
        .copied()
        .find(|variant| {
            let variant = format!("{variant:?}");
            path_is_canonical_mob_machine_variant(segments, "MobMachineInput", variant.as_str())
        })
}

fn camel_to_snake(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for (index, ch) in input.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if index > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
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

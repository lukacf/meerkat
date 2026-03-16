use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, ValueEnum};
use meerkat_machine_codegen::{
    composition_route_coverage_operator_name, composition_scheduler_coverage_operator_name,
    composition_witness_cfg_name, merge_mapping_document, render_composition_ci_cfg,
    render_composition_contract_markdown, render_composition_mapping_coverage,
    render_composition_semantic_model, render_composition_witness_cfg, render_generated_kernel_mod,
    render_machine_ci_cfg, render_machine_contract_markdown, render_machine_kernel_module,
    render_machine_mapping_coverage, render_machine_semantic_model,
};
use meerkat_machine_schema::{
    CompositionCoverageManifest, CompositionSchema, MachineCoverageManifest, MachineSchema,
    SchedulerRule, SemanticCoverageEntry, canonical_composition_coverage_manifests,
    canonical_composition_schemas, canonical_machine_coverage_manifests, canonical_machine_schemas,
};

#[derive(Debug, Clone, Args)]
pub struct SelectionArgs {
    /// Operate on every registered machine and composition.
    #[arg(long)]
    all: bool,
    /// Restrict work to one or more machine names or machine slugs.
    #[arg(long = "machine")]
    machines: Vec<String>,
    /// Restrict work to one or more composition names.
    #[arg(long = "composition")]
    compositions: Vec<String>,
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

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum VerifyProfile {
    Ci,
    Deep,
}

pub fn machine_codegen(args: SelectionArgs) -> Result<()> {
    let registry = CanonicalRegistry::load();
    registry.validate()?;

    let selection = registry.select(&args)?;
    let root = repo_root()?;
    machine_codegen_at_root(&root, &selection)
}

pub fn machine_verify(args: VerifyArgs) -> Result<()> {
    let registry = CanonicalRegistry::load();
    registry.validate()?;

    let selection = registry.select(&args.selection)?;
    let root = repo_root()?;
    let workers = resolve_tlc_workers(args.workers)?;
    machine_verify_at_root(&root, &selection, !args.skip_tlc, args.profile, workers)
}

pub fn machine_check_drift(args: SelectionArgs) -> Result<()> {
    let registry = CanonicalRegistry::load();
    registry.validate()?;

    let selection = registry.select(&args)?;
    let root = repo_root()?;
    let mut mismatches = collect_drift_mismatches(&root, &selection)?;
    mismatches.extend(collect_coverage_anchor_mismatches(&root, &selection));
    mismatches.extend(collect_authority_language_mismatches(&root)?);

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

fn machine_codegen_at_root(root: &Path, selection: &Selection) -> Result<()> {
    let registry = CanonicalRegistry::load();
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
            &machine.schema.machine,
            &render_machine_mapping_coverage(&machine.schema, &machine.coverage),
        );
        write_generated(&mapping_path, &merged)?;
        println!("updated {}", mapping_path.display());

        write_generated(
            &generated_kernel_module_path(root, &machine.slug),
            &render_machine_kernel_module(&machine.schema),
        )?;
        println!(
            "generated {}",
            generated_kernel_module_path(root, &machine.slug).display()
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
                &composition_witness_path(root, &composition.slug, &witness.name),
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
            &composition.schema.name,
            &render_composition_mapping_coverage(&composition.schema, &composition.coverage),
        );
        write_generated(&mapping_path, &merged)?;
        println!("updated {}", mapping_path.display());
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
            if matches!(profile, VerifyProfile::Deep) {
                if let Some(coverage) = coverage {
                    ensure_machine_transition_coverage(&machine.schema, &coverage)?;
                }
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
                    witness_covered_routes.extend(witness.expected_routes.iter().cloned());
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

    Ok(())
}

fn ensure_no_drift(root: &Path, selection: &Selection) -> Result<()> {
    let mut mismatches = collect_drift_mismatches(root, selection)?;
    mismatches.extend(collect_coverage_anchor_mismatches(root, selection));
    mismatches.extend(collect_authority_language_mismatches(root)?);

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

fn collect_drift_mismatches(root: &Path, selection: &Selection) -> Result<Vec<String>> {
    let mut mismatches = Vec::new();
    let registry = CanonicalRegistry::load();

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
            &machine.schema.machine,
            &render_machine_mapping_coverage(&machine.schema, &machine.coverage),
        )?;
        compare_generated(&mapping_path, &mapping_expected, &mut mismatches)?;
        compare_generated(
            &generated_kernel_module_path(root, &machine.slug),
            &render_machine_kernel_module(&machine.schema),
            &mut mismatches,
        )?;
    }

    compare_generated(
        &generated_kernel_mod_path(root),
        &render_generated_kernel_mod(&registry.machines),
        &mut mismatches,
    )?;

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
                &composition_witness_path(root, &composition.slug, &witness.name),
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
            &composition.schema.name,
            &render_composition_mapping_coverage(&composition.schema, &composition.coverage),
        )?;
        compare_generated(&mapping_path, &mapping_expected, &mut mismatches)?;
    }

    Ok(mismatches)
}

fn collect_authority_language_mismatches(root: &Path) -> Result<Vec<String>> {
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

fn collect_coverage_anchor_mismatches(root: &Path, selection: &Selection) -> Vec<String> {
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

fn authority_language_paths(root: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();

    collect_text_paths(&root.join("docs/architecture/0.5"), &mut paths)?;
    collect_text_paths(&root.join("docs/architecture/0.5/rct"), &mut paths)?;

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

struct CanonicalRegistry {
    machines: Vec<MachineSchema>,
    compositions: Vec<CompositionSchema>,
    machine_coverages: Vec<MachineCoverageManifest>,
    composition_coverages: Vec<CompositionCoverageManifest>,
}

impl CanonicalRegistry {
    fn load() -> Self {
        Self {
            machines: canonical_machine_schemas(),
            compositions: canonical_composition_schemas(),
            machine_coverages: canonical_machine_coverage_manifests(),
            composition_coverages: canonical_composition_coverage_manifests(),
        }
    }

    fn validate(&self) -> Result<()> {
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
            .map(|schema| (schema.machine.as_str(), ()))
            .collect::<BTreeMap<_, _>>();

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
            if !machine_names.contains_key(manifest.machine.as_str()) {
                bail!(
                    "machine coverage manifest {} does not match a canonical machine",
                    manifest.machine
                );
            }
        }

        let composition_names = self
            .compositions
            .iter()
            .map(|schema| (schema.name.as_str(), ()))
            .collect::<BTreeMap<_, _>>();

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
            if !composition_names.contains_key(manifest.composition.as_str()) {
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

    fn select(&self, args: &SelectionArgs) -> Result<Selection> {
        if !args.all && args.machines.is_empty() && args.compositions.is_empty() {
            bail!("select --all or provide at least one --machine/--composition");
        }

        let machine_entries = self
            .machines
            .iter()
            .map(|schema| MachineEntry {
                slug: machine_slug(&schema.machine),
                schema: schema.clone(),
                coverage: self
                    .machine_coverages
                    .iter()
                    .find(|item| item.machine == schema.machine)
                    .expect("validated machine coverage")
                    .clone(),
            })
            .collect::<Vec<_>>();

        let composition_entries = self
            .compositions
            .iter()
            .map(|schema| CompositionEntry {
                slug: composition_slug(&schema.name),
                schema: schema.clone(),
                coverage: self
                    .composition_coverages
                    .iter()
                    .find(|item| item.composition == schema.name)
                    .expect("validated composition coverage")
                    .clone(),
            })
            .collect::<Vec<_>>();

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
        .map(|anchor| (anchor.id.as_str(), ()))
        .collect::<BTreeMap<_, _>>();
    let scenario_ids = manifest
        .scenarios
        .iter()
        .map(|scenario| (scenario.id.as_str(), ()))
        .collect::<BTreeMap<_, _>>();

    validate_semantic_entries(
        &format!("machine {}", schema.machine),
        "transition",
        &schema
            .transitions
            .iter()
            .map(|transition| transition.name.clone())
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
            .map(|variant| variant.name.clone())
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
            .map(|invariant| invariant.name.clone())
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
        .map(|anchor| (anchor.id.as_str(), ()))
        .collect::<BTreeMap<_, _>>();
    let scenario_ids = manifest
        .scenarios
        .iter()
        .map(|scenario| (scenario.id.as_str(), ()))
        .collect::<BTreeMap<_, _>>();

    validate_semantic_entries(
        &format!("composition {}", schema.name),
        "route",
        &schema
            .routes
            .iter()
            .map(|route| route.name.clone())
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
    anchor_ids: &BTreeMap<&str, ()>,
    scenario_ids: &BTreeMap<&str, ()>,
) -> Result<()> {
    let expected = expected_names
        .iter()
        .map(|name| (name.as_str(), ()))
        .collect::<BTreeMap<_, _>>();
    let seen = entries
        .iter()
        .map(|entry| (entry.name.as_str(), ()))
        .collect::<BTreeMap<_, _>>();

    for name in expected.keys() {
        if !seen.contains_key(name) {
            bail!("{owner} missing semantic coverage entry for {item_kind} `{name}`");
        }
    }

    for entry in entries {
        if !expected.contains_key(entry.name.as_str()) {
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

        for anchor_id in &entry.anchor_ids {
            if !anchor_ids.contains_key(anchor_id.as_str()) {
                bail!(
                    "{owner} semantic coverage entry `{}` references unknown code anchor `{anchor_id}`",
                    entry.name
                );
            }
        }

        for scenario_id in &entry.scenario_ids {
            if !scenario_ids.contains_key(scenario_id.as_str()) {
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

struct Selection {
    machines: Vec<MachineEntry>,
    compositions: Vec<CompositionEntry>,
}

#[derive(Clone)]
struct MachineEntry {
    slug: String,
    schema: MachineSchema,
    coverage: MachineCoverageManifest,
}

#[derive(Clone)]
struct CompositionEntry {
    slug: String,
    schema: CompositionSchema,
    coverage: CompositionCoverageManifest,
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
                .cloned()
                .find(|entry| {
                    entry.schema.machine == *wanted
                        || entry.slug == *wanted
                        || entry.schema.machine.strip_suffix("Machine") == Some(wanted.as_str())
                })
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
                .cloned()
                .find(|entry| entry.schema.name == *wanted || entry.slug == *wanted)
                .ok_or_else(|| anyhow!("unknown composition selection `{wanted}`"))
        })
        .collect()
}

#[derive(Debug, Default, Clone)]
struct TlcCoverageSummary {
    counts_by_operator: BTreeMap<String, TlcCoverageCounts>,
}

#[derive(Debug, Default, Clone, Copy)]
struct TlcCoverageCounts {
    truth_hits: u64,
    evaluations: u64,
}

fn merge_tlc_coverage(target: &mut TlcCoverageSummary, other: Option<&TlcCoverageSummary>) {
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
        println!("  tlc skipped for {slug} (no checked-in model/config)");
        return Ok(None);
    }

    if which::which("tlc").is_err() {
        println!("  tlc skipped for {slug} (tlc not on PATH)");
        return Ok(None);
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

    if !tlc_run_succeeded(&output.status, &combined) {
        bail!("tlc failed for {slug} ({config_name})");
    }

    let coverage = if matches!(profile, VerifyProfile::Deep) {
        Some(parse_tlc_coverage(&combined))
    } else {
        None
    };

    Ok(coverage)
}

fn tlc_run_succeeded(status: &ExitStatus, combined_output: &str) -> bool {
    if status.success() {
        return true;
    }

    combined_output.contains("Model checking completed. No error has been found.")
        && !combined_output.contains("Error: TLC threw an unexpected exception")
        && !combined_output.contains("Error: The behavior up to this point is")
        && !combined_output.contains("is violated by the initial state")
        && !combined_output.contains("is violated.")
}

fn ensure_machine_transition_coverage(
    schema: &MachineSchema,
    coverage: &TlcCoverageSummary,
) -> Result<()> {
    let zero_hit = schema
        .transitions
        .iter()
        .filter_map(|transition| {
            let evaluations = coverage
                .counts_by_operator
                .get(&transition.name)
                .map(|counts| counts.evaluations)
                .unwrap_or(0);
            (evaluations == 0).then(|| transition.name.clone())
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

fn ensure_composition_coverage(
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
            (evaluations == 0 && !witness_covered_routes.contains(&route.name))
                .then(|| route.name.clone())
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

fn parse_tlc_coverage(output: &str) -> TlcCoverageSummary {
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

fn parse_tlc_coverage_line(line: &str) -> Option<(String, TlcCoverageCounts)> {
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
    let mut cmd = Command::new("cargo");
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
        .map(|count| count.get())
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
    match env::var("JAVA_TOOL_OPTIONS") {
        Ok(existing)
            if existing
                .split_whitespace()
                .any(|flag| flag == throughput_gc) =>
        {
            existing
        }
        Ok(existing) if existing.trim().is_empty() => throughput_gc.into(),
        Ok(existing) => format!("{throughput_gc} {existing}"),
        Err(_) => throughput_gc.into(),
    }
}

fn write_generated(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create output dir {}", parent.display()))?;
    }
    fs::write(path, contents).with_context(|| format!("write {}", path.display()))
}

fn compare_generated(path: &Path, expected: &str, mismatches: &mut Vec<String>) -> Result<()> {
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

fn repo_root() -> Result<PathBuf> {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow!("failed to resolve repo root from xtask manifest dir"))
}

#[cfg(test)]
fn machine_output_path(root: &Path, slug: &str) -> PathBuf {
    machine_model_path(root, slug)
}

fn machine_authority_path(root: &Path, slug: &str) -> PathBuf {
    root.join("specs")
        .join("machines")
        .join(slug)
        .join("generated")
        .join("authority.tla")
}

#[cfg(test)]
fn composition_output_path(root: &Path, slug: &str) -> PathBuf {
    composition_model_path(root, slug)
}

fn composition_authority_path(root: &Path, slug: &str) -> PathBuf {
    root.join("specs")
        .join("compositions")
        .join(slug)
        .join("generated")
        .join("authority.tla")
}

fn machine_model_path(root: &Path, slug: &str) -> PathBuf {
    machine_dir(root, slug).join("model.tla")
}

fn machine_ci_path(root: &Path, slug: &str) -> PathBuf {
    machine_dir(root, slug).join("ci.cfg")
}

fn machine_deep_path(root: &Path, slug: &str) -> PathBuf {
    machine_dir(root, slug).join("deep.cfg")
}

fn machine_contract_path(root: &Path, slug: &str) -> PathBuf {
    machine_dir(root, slug).join("contract.md")
}

fn machine_dir(root: &Path, slug: &str) -> PathBuf {
    root.join("specs").join("machines").join(slug)
}

fn composition_model_path(root: &Path, slug: &str) -> PathBuf {
    composition_dir(root, slug).join("model.tla")
}

fn composition_ci_path(root: &Path, slug: &str) -> PathBuf {
    composition_dir(root, slug).join("ci.cfg")
}

fn composition_deep_path(root: &Path, slug: &str) -> PathBuf {
    composition_dir(root, slug).join("deep.cfg")
}

fn composition_witness_path(root: &Path, slug: &str, witness: &str) -> PathBuf {
    composition_dir(root, slug).join(composition_witness_cfg_name(witness))
}

fn composition_contract_path(root: &Path, slug: &str) -> PathBuf {
    composition_dir(root, slug).join("contract.md")
}

fn composition_dir(root: &Path, slug: &str) -> PathBuf {
    root.join("specs").join("compositions").join(slug)
}

fn machine_mapping_path(root: &Path, slug: &str) -> PathBuf {
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

fn generated_kernel_module_path(root: &Path, slug: &str) -> PathBuf {
    generated_kernel_root(root).join(format!("{slug}.rs"))
}

fn generated_kernel_mod_path(root: &Path) -> PathBuf {
    generated_kernel_root(root).join("mod.rs")
}

fn composition_mapping_path(root: &Path, slug: &str) -> PathBuf {
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

fn machine_slug(machine_name: &str) -> String {
    let trimmed = machine_name.strip_suffix("Machine").unwrap_or(machine_name);
    to_snake_case(trimmed)
}

fn composition_slug(name: &str) -> String {
    to_snake_case(name)
}

fn to_snake_case(value: &str) -> String {
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
mod tests {
    use std::fs;

    use meerkat_machine_schema::{
        runtime_control_machine, runtime_ingress_machine, runtime_pipeline_composition,
        turn_execution_machine,
    };

    use super::{
        CanonicalRegistry, SelectionArgs, collect_coverage_anchor_mismatches,
        collect_drift_mismatches, composition_mapping_path, composition_output_path,
        composition_slug, ensure_composition_coverage, ensure_machine_transition_coverage,
        machine_codegen_at_root, machine_mapping_path, machine_output_path, machine_slug,
        merge_tlc_coverage, parse_tlc_coverage, parse_tlc_coverage_line, repo_root,
        tlc_run_succeeded, to_snake_case,
    };
    use tempfile::tempdir;

    #[test]
    fn snake_case_handles_machine_names_and_existing_snake_case() {
        assert_eq!(machine_slug("RuntimeControlMachine"), "runtime_control");
        assert_eq!(
            machine_slug("ExternalToolSurfaceMachine"),
            "external_tool_surface"
        );
        assert_eq!(composition_slug("runtime_pipeline"), "runtime_pipeline");
        assert_eq!(to_snake_case("Mob-Orchestrator"), "mob_orchestrator");
    }

    #[test]
    fn output_paths_land_under_specs() {
        let root = repo_root().expect("repo root");
        assert_eq!(
            machine_output_path(&root, "runtime_control"),
            root.join("specs/machines/runtime_control/model.tla")
        );
        assert_eq!(
            composition_output_path(&root, "runtime_pipeline"),
            root.join("specs/compositions/runtime_pipeline/model.tla")
        );
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

        let machine =
            fs::read_to_string(dir.path().join("specs/machines/runtime_control/model.tla"))
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

        let deep_cfg =
            fs::read_to_string(dir.path().join("specs/compositions/mob_bundle/deep.cfg"))
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

        let model =
            fs::read_to_string(dir.path().join("specs/compositions/mob_bundle/model.tla"))
                .expect("mob model");
        assert!(model.contains("WitnessStateConstraint_mob_flow_success_path == /\\ model_step_count <= 26"));
        assert!(model.contains("Cardinality(delivered_routes) <= 7"));
        assert!(model.contains("Cardinality(emitted_effects) <= 14"));
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
        assert!(cfg.contains("CandidateIdValues = {\"step_1\"}"));
        assert!(cfg.contains("InputKindValues = {\"WorkInput\"}"));
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
        assert!(model.contains("pending_inputs = <<[machine |-> \"runtime_control\", variant |-> \"Initialize\""));
        assert!(model.contains("[machine |-> \"runtime_ingress\", variant |-> \"AdmitQueued\""));
        assert!(model.contains("wake |-> TRUE"));
        assert!(model.contains("process |-> TRUE"));
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
        assert!(model.contains(
            "earlier.machine = \"turn_execution\" /\\ earlier.transition = \"LlmReturnedTerminal\" /\\ later.machine = \"external_tool_surface\" /\\ later.transition = \"ApplyBoundaryAdd\""
        ));
        assert!(model.contains(
            "earlier.machine = \"external_tool_surface\" /\\ earlier.transition = \"ApplyBoundaryAdd\" /\\ later.machine = \"turn_execution\" /\\ later.transition = \"BoundaryComplete\""
        ));
        assert!(model.contains(
            "earlier.machine = \"runtime_control\" /\\ earlier.transition = \"ExternalToolDeltaReceivedIdle\" /\\ later.machine = \"turn_execution\" /\\ later.transition = \"BoundaryComplete\""
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
        assert_eq!(missing.len(), 16);
        assert!(
            missing
                .iter()
                .any(|item| item.contains("specs/machines/runtime_control/model.tla"))
        );
        assert!(
            missing
                .iter()
                .any(|item| item.contains("specs/machines/runtime_control/ci.cfg"))
        );
        assert!(
            missing
                .iter()
                .any(|item| item.contains("specs/machines/runtime_control/deep.cfg"))
        );
        assert!(
            missing
                .iter()
                .any(|item| item.contains("specs/machines/runtime_control/contract.md"))
        );
        assert!(
            missing
                .iter()
                .any(|item| item.contains("specs/machines/runtime_control/mapping.md"))
        );
        assert!(
            missing
                .iter()
                .any(|item| item.contains("specs/compositions/runtime_pipeline/model.tla"))
        );
        assert!(
            missing
                .iter()
                .any(|item| item.contains("specs/compositions/runtime_pipeline/ci.cfg"))
        );
        assert!(
            missing
                .iter()
                .any(|item| item.contains("specs/compositions/runtime_pipeline/deep.cfg"))
        );
        assert!(missing.iter().any(|item| {
            item.contains("specs/compositions/runtime_pipeline/witness-success_path.cfg")
        }));
        assert!(missing.iter().any(|item| {
            item.contains("specs/compositions/runtime_pipeline/witness-failure_path.cfg")
        }));
        assert!(missing.iter().any(|item| {
            item.contains("specs/compositions/runtime_pipeline/witness-cancel_path.cfg")
        }));
        assert!(missing.iter().any(|item| {
            item.contains("specs/compositions/runtime_pipeline/witness-control_preemption.cfg")
        }));
        assert!(
            missing
                .iter()
                .any(|item| item.contains("specs/compositions/runtime_pipeline/contract.md"))
        );
        assert!(
            missing
                .iter()
                .any(|item| item.contains("specs/compositions/runtime_pipeline/mapping.md"))
        );
        assert!(
            missing
                .iter()
                .any(|item| item
                    .contains("meerkat-machine-kernels/src/generated/runtime_control.rs"))
        );
        assert!(
            missing
                .iter()
                .any(|item| item.contains("meerkat-machine-kernels/src/generated/mod.rs"))
        );

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

        let mismatches =
            super::collect_authority_language_mismatches(dir.path()).expect("mismatches");
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
            "<RouteCoverage_staged_run_notifies_control line 1, col 1 to line 1, col 10 of module model>: 1:3\n\
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
}

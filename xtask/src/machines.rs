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

pub fn machine_codegen_at_root(root: &Path, selection: &Selection) -> Result<()> {
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

pub fn collect_drift_mismatches(root: &Path, selection: &Selection) -> Result<Vec<String>> {
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

pub fn collect_generated_kernel_boundary_mismatches(root: &Path) -> Result<Vec<String>> {
    let registry = CanonicalRegistry::load();
    let mut mismatches = Vec::new();

    for machine in &registry.machines {
        let slug = machine_slug(&machine.machine);
        let generated_kernel = generated_kernel_module_path(root, &slug);
        if !generated_kernel.exists() {
            continue;
        }

        let owner_file = owner_module_file(root, &machine.rust.crate_name, &machine.rust.module)?;
        let leaf = machine
            .rust
            .module
            .rsplit("::")
            .next()
            .unwrap_or(&machine.rust.module);
        let owner_mod = owner_module_dir(root, &machine.rust.crate_name, &machine.rust.module)
            .join(leaf)
            .join("mod.rs");

        if owner_file.exists() {
            mismatches.push(format!(
                "parallel owner module must be removed for {}: generated kernel {} exists but shell owner file {} still defines machine authority",
                machine.machine,
                generated_kernel.display(),
                owner_file.display()
            ));
        }

        if owner_mod.exists() {
            mismatches.push(format!(
                "parallel owner module must be removed for {}: generated kernel {} exists but shell owner module {} still defines machine authority",
                machine.machine,
                generated_kernel.display(),
                owner_mod.display()
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
    let owner_inventory = parse_owner_inventory(
        &fs::read_to_string(root.join("docs/architecture/0.5/meerkat_0_5_implementation_plan.md"))
            .context("read canonical machine owner map")?,
    )?;
    let final_mode_inventory = parse_final_mode_inventory(
        &fs::read_to_string(
            root.join("docs/architecture/0.5/meerkat_machine_formalization_strategy.md"),
        )
        .context("read final machine mode inventory")?,
    )?;

    let mut mismatches = Vec::new();
    let registry_names: BTreeSet<String> = registry
        .machines
        .iter()
        .map(|machine| machine.machine.clone())
        .collect();
    let owner_names: BTreeSet<String> = owner_inventory.keys().cloned().collect();
    let final_mode_names: BTreeSet<String> = final_mode_inventory.keys().cloned().collect();

    for machine in registry_names.difference(&owner_names) {
        mismatches.push(format!(
            "canonical machine {machine} missing from docs/architecture/0.5/meerkat_0_5_implementation_plan.md owner map"
        ));
    }
    for machine in owner_names.difference(&registry_names) {
        mismatches.push(format!(
            "untracked machine {machine} present in docs/architecture/0.5/meerkat_0_5_implementation_plan.md owner map"
        ));
    }
    for machine in registry_names.difference(&final_mode_names) {
        mismatches.push(format!(
            "canonical machine {machine} missing from docs/architecture/0.5/meerkat_machine_formalization_strategy.md final mode table"
        ));
    }
    for machine in final_mode_names.difference(&registry_names) {
        mismatches.push(format!(
            "untracked machine {machine} present in docs/architecture/0.5/meerkat_machine_formalization_strategy.md final mode table"
        ));
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

    for machine in &registry.machines {
        let Some(owner_row) = owner_inventory.get(&machine.machine) else {
            continue;
        };
        let Some(required_final_mode) = final_mode_inventory.get(&machine.machine) else {
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
        for artifact_path in [
            machine_contract_path(root, &slug),
            machine_model_path(root, &slug),
            machine_ci_path(root, &slug),
            machine_deep_path(root, &slug),
            machine_mapping_path(root, &slug),
            generated_kernel_module_path(root, &slug),
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
                    entry.schema.machine == *wanted
                        || entry.slug == *wanted
                        || entry.schema.machine.strip_suffix("Machine") == Some(wanted.as_str())
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
                .find(|entry| entry.schema.name == *wanted || entry.slug == *wanted)
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

pub fn tlc_run_succeeded(status: &ExitStatus, combined_output: &str) -> bool {
    if status.success() {
        return true;
    }

    combined_output.contains("Model checking completed. No error has been found.")
        && !combined_output.contains("Error: TLC threw an unexpected exception")
        && !combined_output.contains("Error: The behavior up to this point is")
        && !combined_output.contains("is violated by the initial state")
        && !combined_output.contains("is violated.")
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
    const PEER_COMMS: &[OwnerTestSpec] = &[
        OwnerTestSpec {
            package: "meerkat-comms",
            target: "peer_comms_kernel",
            filter: "peer_comms_kernel_preserves_reservation_and_trust_snapshot_for_trusted_requests",
        },
        OwnerTestSpec {
            package: "meerkat-comms",
            target: "peer_comms_kernel",
            filter: "peer_comms_kernel_classifies_inline_terminal_without_child_lifecycle_leakage",
        },
    ];
    const TURN_EXECUTION: &[OwnerTestSpec] = &[
        OwnerTestSpec {
            package: "meerkat-core",
            target: "turn_execution_kernel",
            filter: "turn_execution_kernel_tool_loop_yields_back_to_llm_after_boundary",
        },
        OwnerTestSpec {
            package: "meerkat-core",
            target: "turn_execution_kernel",
            filter: "turn_execution_kernel_immediate_context_completes_without_llm_loop",
        },
        OwnerTestSpec {
            package: "meerkat-core",
            target: "turn_execution_kernel",
            filter: "turn_execution_kernel_cancel_and_failure_paths_emit_terminal_effects",
        },
    ];
    const EXTERNAL_TOOL_SURFACE: &[OwnerTestSpec] = &[
        OwnerTestSpec {
            package: "meerkat-mcp",
            target: "external_tool_surface_kernel",
            filter: "external_tool_surface_kernel_add_and_reload_emit_canonical_deltas",
        },
        OwnerTestSpec {
            package: "meerkat-mcp",
            target: "external_tool_surface_kernel",
            filter: "external_tool_surface_kernel_remove_drain_completion_and_forced_finalize_emit_deltas",
        },
    ];
    const FLOW_RUN: &[OwnerTestSpec] = &[OwnerTestSpec {
        package: "meerkat-mob",
        target: "flow_run_kernel",
        filter: "flow_run_kernel_persists_pending_and_terminal_truth_for_machine_verify",
    }];
    const MOB_ORCHESTRATOR: &[OwnerTestSpec] = &[OwnerTestSpec {
        package: "meerkat-mob",
        target: "mob_orchestrator_kernel",
        filter: "mob_orchestrator_kernel_tracks_binding_pending_spawn_and_resume_semantics_for_machine_verify",
    }];

    match slug {
        "peer_comms" => PEER_COMMS,
        "turn_execution" => TURN_EXECUTION,
        "external_tool_surface" => EXTERNAL_TOOL_SURFACE,
        "flow_run" => FLOW_RUN,
        "mob_orchestrator" => MOB_ORCHESTRATOR,
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
        cmd.arg("test")
            .arg("-p")
            .arg(spec.package)
            .arg("--test")
            .arg(spec.target)
            .arg(spec.filter)
            .arg("--")
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
    let mut child = Command::new("rustfmt")
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
    if let Some(root) = std::env::var_os("MEERKAT_MACHINE_ROOT") {
        return Ok(PathBuf::from(root));
    }

    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow!("failed to resolve repo root from xtask manifest dir"))
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

pub fn machine_slug(machine_name: &str) -> String {
    let trimmed = machine_name.strip_suffix("Machine").unwrap_or(machine_name);
    to_snake_case(trimmed)
}

pub fn composition_slug(name: &str) -> String {
    to_snake_case(name)
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

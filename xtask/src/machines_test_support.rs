use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, ValueEnum};
use meerkat_machine_schema::{
    CompositionSchema, MachineSchema, canonical_composition_schemas, canonical_machine_schemas,
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

pub fn machine_codegen(_args: SelectionArgs) -> Result<()> {
    bail!("machine-codegen is not available through the xtask lib test harness")
}

pub fn machine_verify(_args: VerifyArgs) -> Result<()> {
    bail!("machine-verify is not available through the xtask lib test harness")
}

pub fn machine_check_drift(_args: SelectionArgs) -> Result<()> {
    bail!("machine-check-drift is not available through the xtask lib test harness")
}

pub struct Selection {
    pub machines: Vec<MachineEntry>,
    pub compositions: Vec<CompositionEntry>,
}

#[derive(Clone)]
pub struct MachineEntry {
    slug: String,
    pub schema: MachineSchema,
}

#[derive(Clone)]
pub struct CompositionEntry {
    slug: String,
    pub schema: CompositionSchema,
}

pub struct CanonicalRegistry {
    machines: Vec<MachineSchema>,
    compositions: Vec<CompositionSchema>,
}

impl CanonicalRegistry {
    pub fn load() -> Self {
        Self {
            machines: canonical_machine_schemas(),
            compositions: canonical_composition_schemas(),
        }
    }

    pub fn select(&self, args: &SelectionArgs) -> Result<Selection> {
        if !args.all && args.machines.is_empty() && args.compositions.is_empty() {
            bail!("select --all or provide at least one --machine/--composition");
        }

        let machine_entries = self
            .machines
            .iter()
            .map(|schema| MachineEntry {
                slug: machine_slug(&schema.machine),
                schema: schema.clone(),
            })
            .collect::<Vec<_>>();

        let composition_entries = self
            .compositions
            .iter()
            .map(|schema| CompositionEntry {
                slug: composition_slug(&schema.name),
                schema: schema.clone(),
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
                        || legacy_machine_slug(&entry.schema.machine) == Some(wanted.as_str())
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

pub fn repo_root() -> Result<PathBuf> {
    if let Some(root) = env::var_os("MEERKAT_MACHINE_ROOT") {
        return Ok(PathBuf::from(root));
    }

    workspace_root()
}

pub fn machine_model_path(root: &Path, slug: &str) -> PathBuf {
    machine_dir(root, slug).join("model.tla")
}

pub fn composition_model_path(root: &Path, slug: &str) -> PathBuf {
    composition_dir(root, slug).join("model.tla")
}

pub fn machine_codegen_at_root(root: &Path, selection: &Selection) -> Result<()> {
    run_xtask(root, "machine-codegen", selection).map(|_| ())
}

pub fn collect_drift_mismatches(root: &Path, selection: &Selection) -> Result<Vec<String>> {
    let output = run_xtask(root, "machine-check-drift", selection)?;
    if output.status.success() {
        return Ok(Vec::new());
    }

    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let mismatches = combined
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix("- "))
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();

    if mismatches.is_empty() {
        bail!("machine-check-drift failed without parsable mismatches:\n{combined}");
    }

    Ok(mismatches)
}

fn run_xtask(root: &Path, subcommand: &str, selection: &Selection) -> Result<std::process::Output> {
    let mut cmd = Command::new("cargo");
    cmd.arg("run")
        .arg("-p")
        .arg("xtask")
        .arg("--features")
        .arg("machine-authority")
        .arg("--")
        .arg(subcommand)
        .env("MEERKAT_MACHINE_ROOT", root)
        .current_dir(workspace_root()?);

    for machine in &selection.machines {
        cmd.arg("--machine").arg(&machine.slug);
    }
    for composition in &selection.compositions {
        cmd.arg("--composition").arg(&composition.slug);
    }

    let output = cmd.output().with_context(|| {
        format!(
            "run cargo xtask {subcommand} for repo root override {}",
            root.display()
        )
    })?;

    if output.status.success() || subcommand == "machine-check-drift" {
        return Ok(output);
    }

    bail!(
        "cargo xtask {subcommand} failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    )
}

fn workspace_root() -> Result<PathBuf> {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow!("failed to resolve repo root from xtask manifest dir"))
}

fn machine_dir(root: &Path, slug: &str) -> PathBuf {
    root.join("specs").join("machines").join(slug)
}

fn composition_dir(root: &Path, slug: &str) -> PathBuf {
    root.join("specs").join("compositions").join(slug)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
struct OwnerTestSpec {
    package: &'static str,
    target: &'static str,
    filter: &'static str,
}

#[allow(dead_code)]
fn owner_test_specs_for_machine(slug: &str) -> &'static [OwnerTestSpec] {
    const MEERKAT: &[OwnerTestSpec] = &[
        OwnerTestSpec {
            package: "meerkat-integration-tests",
            target: "peer_directory_reachability_kernel",
            filter: "peer_directory_reachability_kernel_reconcile_replaces_directory_snapshot",
        },
        OwnerTestSpec {
            package: "meerkat-integration-tests",
            target: "peer_directory_reachability_kernel",
            filter: "peer_directory_reachability_kernel_records_send_failures_for_resolved_peers",
        },
        OwnerTestSpec {
            package: "meerkat-integration-tests",
            target: "session_turn_admission_kernel",
            filter: "session_turn_admission_kernel_gracefully_drains_running_shutdown",
        },
        OwnerTestSpec {
            package: "meerkat-integration-tests",
            target: "session_turn_admission_kernel",
            filter: "session_turn_admission_kernel_interrupt_only_wakes_running_turns",
        },
        OwnerTestSpec {
            package: "meerkat-integration-tests",
            target: "session_tool_visibility_kernel",
            filter: "session_tool_visibility_kernel_promotes_staged_filter_at_boundary",
        },
        OwnerTestSpec {
            package: "meerkat-integration-tests",
            target: "session_tool_visibility_kernel",
            filter: "session_tool_visibility_kernel_stages_deferred_requests_without_touching_active_state",
        },
    ];
    const MOB: &[OwnerTestSpec] = &[OwnerTestSpec {
        package: "meerkat-mob",
        target: "flow_run_kernel",
        filter: "flow_run_kernel_persists_pending_and_terminal_truth_for_machine_verify",
    }];

    match slug {
        "meerkat_machine" => MEERKAT,
        "mob_machine" => MOB,
        _ => &[],
    }
}

pub fn machine_slug(machine_name: &str) -> String {
    match machine_name {
        "MeerkatMachine" => return "meerkat_machine".into(),
        "MobMachine" => return "mob_machine".into(),
        _ => {}
    }
    let trimmed = machine_name.strip_suffix("Machine").unwrap_or(machine_name);
    to_snake_case(trimmed)
}

fn legacy_machine_slug(machine_name: &str) -> Option<&'static str> {
    match machine_name {
        "MeerkatMachine" => Some("meerkat"),
        "MobMachine" => Some("mob"),
        _ => None,
    }
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

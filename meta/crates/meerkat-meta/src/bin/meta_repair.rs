//! Supported operator repair of predecessor composition manifests.
//!
//! Composition manifests (`--composition-manifests <path> [--apply]`): a
//! separate mode for a store created by the predecessor (pre-0.9) release,
//! whose mob composition manifest (`*.composition.json` beside the mob
//! storage) still spells its creator field `created_by_mobkit`. `<path>` is
//! one manifest, or a state directory whose top-level manifests are all
//! inspected. Dry run by default: each manifest is reported `current`,
//! `predecessor` or `refused` (with the reason; nothing changed). `--apply`
//! renames the field of each `predecessor` manifest in place (reported
//! `rewritten`), keeping every other byte, the recorded definition included;
//! a manifest whose rename changed something before failing (replaced but
//! not yet durable, or a stage left behind) is reported `failed`, with the
//! reason. Predecessor adoption (`meta_gateway storage-adopt-predecessor`)
//! does the same as one of its steps; this mode serves a store adopted
//! before that step existed, and a manifest outside a state directory. Stop
//! every process using the store first. Exit code 0 when no manifest was
//! refused or failed, 1 otherwise.
//!
//! Removed in 0.9.4: the session transcript modes (`--db`, `--session`,
//! `--all-sessions`, `--keep-newest`, `--truncate-tool-results` and
//! `--drop-tool-results-older-than`). They rewrote transcripts in Meta's
//! 0.9.3 continuity projection, which 0.9.4 no longer reads: Core owns every
//! member's session. A 0.9.3 transcript that still needs repair is repaired
//! with the 0.9.3 meta-repair before the directory moves with `meta_gateway
//! storage-migrate-native`; the removed flags refuse with both pointers.

/// The flags of the session transcript modes removed in 0.9.4.
const REMOVED_TRANSCRIPT_FLAGS: [&str; 6] = [
    "--db",
    "--session",
    "--all-sessions",
    "--keep-newest",
    "--truncate-tool-results",
    "--drop-tool-results-older-than",
];

fn main() {
    match run() {
        Ok(exit) => std::process::exit(exit),
        Err(error) => {
            eprintln!("meta-repair: {error}");
            std::process::exit(2);
        }
    }
}

fn run() -> Result<i32, Box<dyn std::error::Error>> {
    let mut composition_manifests: Option<String> = None;
    let mut apply = false;
    let mut report_path: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--composition-manifests" => {
                composition_manifests = Some(
                    args.next()
                        .ok_or("--composition-manifests requires a manifest or a directory")?,
                );
            }
            "--apply" => apply = true,
            "--report" => report_path = args.next(),
            removed if REMOVED_TRANSCRIPT_FLAGS.contains(&removed) => {
                return Err(format!(
                    "{removed} was removed in 0.9.4 with the session transcript repair modes: \
                     0.9.4 no longer reads the 0.9.3 continuity projection they rewrote. To \
                     repair a 0.9.3 transcript before migrating, use the 0.9.3 meta-repair; then \
                     move the 0.9.3 state directory with `meta_gateway storage-migrate-native`. \
                     meta-repair keeps --composition-manifests"
                )
                .into());
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }
    let target = composition_manifests
        .ok_or("--composition-manifests <manifest or state directory> is required")?;
    repair_composition_manifests(&target, apply, report_path.as_deref())
}

/// One manifest's line in the composition manifest report.
#[derive(Debug, serde::Serialize)]
struct ManifestReport {
    path: String,
    /// `current`, `predecessor`, `rewritten`, `refused` (nothing changed) or
    /// `failed` (something changed: see the detail).
    outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

#[derive(Debug, serde::Serialize)]
struct ManifestRepairReport {
    mode: &'static str,
    applied: bool,
    manifests: Vec<ManifestReport>,
    refused: usize,
    failed: usize,
}

/// The `--composition-manifests` mode: inspect, and with `apply` rename, the
/// predecessor creator field of one manifest or of every top-level manifest
/// in a directory.
fn repair_composition_manifests(
    target: &str,
    apply: bool,
    report_path: Option<&str>,
) -> Result<i32, Box<dyn std::error::Error>> {
    let target = std::path::Path::new(target);
    let manifests = if target.is_dir() {
        meerkat_meta::storage_predecessor::composition_manifests_in(target)?
    } else {
        vec![target.to_path_buf()]
    };
    if manifests.is_empty() {
        eprintln!(
            "meta-repair: no composition manifest (*.composition.json) in {}",
            target.display()
        );
    }
    let mode = if apply {
        meerkat_meta::MigrateMode::Apply
    } else {
        meerkat_meta::MigrateMode::DryRun
    };
    let mut refused = 0usize;
    let mut failed = 0usize;
    let mut reports = Vec::with_capacity(manifests.len());
    for manifest in manifests {
        let path = manifest.display().to_string();
        match meerkat_meta::storage_predecessor::repair_composition_manifest(&manifest, mode) {
            Ok(outcome) => {
                let outcome = serde_json::to_value(outcome)?
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                eprintln!("meta-repair: {path}: {outcome}");
                reports.push(ManifestReport {
                    path,
                    outcome,
                    detail: None,
                });
            }
            Err(error) => {
                let counter = match error {
                    meerkat_meta::storage_predecessor::ManifestRepairError::Refused(_) => {
                        &mut refused
                    }
                    meerkat_meta::storage_predecessor::ManifestRepairError::Failed(_) => {
                        &mut failed
                    }
                };
                *counter += 1;
                let outcome = error.outcome();
                eprintln!(
                    "meta-repair: {path} {}: {error}",
                    outcome.to_ascii_uppercase()
                );
                reports.push(ManifestReport {
                    path,
                    outcome: outcome.to_string(),
                    detail: Some(error.detail().to_string()),
                });
            }
        }
    }
    let rendered = serde_json::to_string_pretty(&ManifestRepairReport {
        mode: "composition_manifests",
        applied: apply,
        manifests: reports,
        refused,
        failed,
    })?;
    println!("{rendered}");
    if let Some(path) = report_path {
        std::fs::write(path, &rendered)?;
        eprintln!("meta-repair: report written to {path}");
    }
    Ok(i32::from(refused + failed != 0))
}

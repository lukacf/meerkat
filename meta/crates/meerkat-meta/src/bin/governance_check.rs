#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args,
    clippy::collapsible_if,
    clippy::redundant_clone,
    clippy::needless_raw_string_hashes,
    clippy::single_match,
    clippy::redundant_closure_for_method_calls,
    clippy::redundant_pattern_matching,
    clippy::ignored_unit_patterns,
    clippy::clone_on_copy,
    clippy::manual_assert,
    clippy::unwrap_in_result,
    clippy::useless_vec
)]
//! Phase 0 binary — validates governance policies before runtime startup.

use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    process,
};

use meerkat_meta::validate_governance_contracts;

fn read_required(path: &PathBuf) -> Result<String, String> {
    fs::read_to_string(path).map_err(|err| format!("failed to read {}: {err}", path.display()))
}

fn read_traceability(root: &Path) -> Result<String, String> {
    for candidate in [
        root.join(".rct/traceability.yaml"),
        root.join("docs/rct/traceability.yaml"),
        root.join(".rct/traceability.md"),
        root.join("docs/rct/traceability.md"),
    ] {
        match fs::read_to_string(&candidate) {
            Ok(contents) => return Ok(contents),
            Err(err) if err.kind() == ErrorKind::NotFound => continue,
            Err(err) => return Err(format!("failed to read {}: {err}", candidate.display())),
        }
    }

    Err(format!(
        "failed to read traceability artifact from any supported path under {}",
        root.display()
    ))
}

const USAGE: &str = "usage: governance_check --root <DIR>  (validates <DIR>/.rct/{spec,plan,checklist}.yaml and its traceability document)";

/// The governance root, named explicitly. RCT documents are local working
/// state that this repository does not track, so there is no default root.
fn project_root() -> Result<PathBuf, String> {
    let mut args = std::env::args().skip(1);
    match (args.next().as_deref(), args.next(), args.next()) {
        (Some("--root"), Some(root), None) => Ok(PathBuf::from(root)),
        _ => Err(USAGE.to_string()),
    }
}

fn main() {
    let root = match project_root() {
        Ok(root) => root,
        Err(usage) => {
            eprintln!("{usage}");
            process::exit(2);
        }
    };
    let spec_path = root.join(".rct/spec.yaml");
    let plan_path = root.join(".rct/plan.yaml");
    let checklist_path = root.join(".rct/checklist.yaml");

    let spec = match read_required(&spec_path) {
        Ok(value) => value,
        Err(err) => {
            eprintln!("{err}");
            process::exit(1);
        }
    };
    let plan = match read_required(&plan_path) {
        Ok(value) => value,
        Err(err) => {
            eprintln!("{err}");
            process::exit(1);
        }
    };
    let checklist = match read_required(&checklist_path) {
        Ok(value) => value,
        Err(err) => {
            eprintln!("{err}");
            process::exit(1);
        }
    };
    let traceability = match read_traceability(&root) {
        Ok(value) => value,
        Err(err) => {
            eprintln!("{err}");
            process::exit(1);
        }
    };

    if let Err(err) = validate_governance_contracts(&spec, &plan, &checklist, &traceability) {
        eprintln!("governance validation failed: {err}");
        process::exit(1);
    }

    println!("governance validation passed");
}

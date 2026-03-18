use anyhow::{Context, Result, bail};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

const PUBLIC_CONTRACT_TRIGGER_PREFIXES: &[&str] = &["meerkat-contracts/src/", "artifacts/schemas/"];
const PUBLIC_DOC_PREFIXES: &[&str] = &["docs/api/", "docs/sdks/", "docs/rust/", "examples/"];
const PYTHON_BINDINGS_PREFIX: &str = "sdks/python/meerkat/generated/";
const TYPESCRIPT_BINDINGS_PREFIX: &str = "sdks/typescript/src/generated/";
const CHANGELOG_PATH: &str = "CHANGELOG.md";

pub fn collect_public_contract_propagation_mismatches(
    changed_paths: &BTreeSet<String>,
) -> Vec<String> {
    let public_contract_changed = changed_paths
        .iter()
        .any(|path| starts_with_any(path, PUBLIC_CONTRACT_TRIGGER_PREFIXES));
    if !public_contract_changed {
        return Vec::new();
    }

    let mut mismatches = Vec::new();
    if !changed_paths
        .iter()
        .any(|path| path.starts_with("artifacts/schemas/"))
    {
        mismatches.push(
            "public contract slice is missing regenerated schema artifacts under artifacts/schemas/"
                .to_string(),
        );
    }
    if !changed_paths
        .iter()
        .any(|path| path.starts_with(PYTHON_BINDINGS_PREFIX))
    {
        mismatches.push(
            "public contract slice is missing regenerated Python bindings under sdks/python/meerkat/generated/"
                .to_string(),
        );
    }
    if !changed_paths
        .iter()
        .any(|path| path.starts_with(TYPESCRIPT_BINDINGS_PREFIX))
    {
        mismatches.push(
            "public contract slice is missing regenerated TypeScript bindings under sdks/typescript/src/generated/"
                .to_string(),
        );
    }
    if !changed_paths
        .iter()
        .any(|path| starts_with_any(path, PUBLIC_DOC_PREFIXES))
    {
        mismatches.push(
            "public contract slice is missing affected docs/examples under docs/api, docs/sdks, docs/rust, or examples/"
                .to_string(),
        );
    }
    if !changed_paths.contains(CHANGELOG_PATH) {
        mismatches.push("public contract slice is missing CHANGELOG.md".to_string());
    }

    mismatches
}

fn starts_with_any(path: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|prefix| path.starts_with(prefix))
}

pub fn repo_root() -> Result<PathBuf> {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow::anyhow!("failed to resolve repo root from xtask manifest dir"))
}

fn collect_relative_files(root: &Path, base: &Path, files: &mut BTreeSet<PathBuf>) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }

    for entry in fs::read_dir(root).with_context(|| format!("read {}", root.display()))? {
        let entry = entry.with_context(|| format!("iterate {}", root.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("read file type {}", path.display()))?;
        if file_type.is_dir() {
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name == "__pycache__")
            {
                continue;
            }
            collect_relative_files(&path, base, files)?;
        } else if file_type.is_file() {
            if path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext == "pyc")
            {
                continue;
            }
            let relative = path.strip_prefix(base).with_context(|| {
                format!("strip prefix {} from {}", base.display(), path.display())
            })?;
            files.insert(relative.to_path_buf());
        }
    }

    Ok(())
}

pub fn assert_directory_contents_match(expected_root: &Path, actual_root: &Path) -> Result<()> {
    let mut expected_files = BTreeSet::new();
    let mut actual_files = BTreeSet::new();
    collect_relative_files(expected_root, expected_root, &mut expected_files)?;
    collect_relative_files(actual_root, actual_root, &mut actual_files)?;

    if expected_files != actual_files {
        bail!(
            "generated file sets differ:\nexpected: {expected_files:#?}\nactual: {actual_files:#?}"
        );
    }

    for relative in expected_files {
        let expected_path = expected_root.join(&relative);
        let actual_path = actual_root.join(&relative);
        let expected = fs::read_to_string(&expected_path)
            .with_context(|| format!("read {}", expected_path.display()))?;
        let actual = fs::read_to_string(&actual_path)
            .with_context(|| format!("read {}", actual_path.display()))?;
        if expected != actual {
            bail!(
                "generated file {} is stale; regenerate bindings to match canonical schemas",
                expected_path.display()
            );
        }
    }

    Ok(())
}

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
    if let Some(root) = bazel_runfiles_workspace_root() {
        return Ok(root);
    }
    if let Some(root) = std::env::var_os("WORKSPACE_ROOT") {
        return Ok(PathBuf::from(root));
    }
    if let Some(root) = std::env::var_os("MEERKAT_WORKSPACE_ROOT") {
        return Ok(PathBuf::from(root));
    }
    if let Ok(current_dir) = std::env::current_dir()
        && let Some(root) = workspace_root_containing(&current_dir)
    {
        return Ok(root);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow::anyhow!("failed to resolve repo root from xtask manifest dir"))
}

/// Nearest ancestor of `start` (inclusive) whose `Cargo.toml` declares the
/// `[workspace]` table.
///
/// A member crate's own `Cargo.toml` is not evidence of the repo root: cargo
/// runs test binaries with the current directory set to the package root, so
/// `cargo test -p xtask` starts in `xtask/`, and accepting any manifest there
/// pointed `repo_root()` at the crate instead of the workspace.
fn workspace_root_containing(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|dir| manifest_declares_workspace(dir))
        .map(Path::to_path_buf)
}

fn manifest_declares_workspace(dir: &Path) -> bool {
    fs::read_to_string(dir.join("Cargo.toml"))
        .is_ok_and(|manifest| manifest.lines().any(|line| line.trim() == "[workspace]"))
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

#[cfg(test)]
#[allow(clippy::expect_used)]
mod repo_root_tests {
    use super::workspace_root_containing;
    use std::fs;

    #[test]
    fn member_crate_directory_resolves_to_the_enclosing_workspace_root() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"member\"]\n",
        )
        .expect("write workspace manifest");
        let member = root.join("member");
        fs::create_dir_all(member.join("src")).expect("create member");
        fs::write(
            member.join("Cargo.toml"),
            "[package]\nname = \"member\"\nversion = \"0.0.0\"\n",
        )
        .expect("write member manifest");

        let resolved = workspace_root_containing(&member.join("src"))
            .expect("nested directory resolves to workspace root");
        assert_eq!(resolved, root);
        assert_eq!(
            workspace_root_containing(&member).as_deref(),
            Some(root),
            "a member crate manifest must not be mistaken for the workspace root"
        );
    }

    #[test]
    fn crate_without_an_enclosing_workspace_does_not_resolve() {
        let temp = tempfile::tempdir().expect("tempdir");
        let lone = temp.path().join("lone");
        fs::create_dir_all(&lone).expect("create crate dir");
        fs::write(
            lone.join("Cargo.toml"),
            "[package]\nname = \"lone\"\nversion = \"0.0.0\"\n",
        )
        .expect("write crate manifest");

        assert_eq!(workspace_root_containing(&lone), None);
    }
}

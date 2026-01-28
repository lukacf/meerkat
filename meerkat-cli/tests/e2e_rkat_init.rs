use std::path::PathBuf;
use std::process::Command;

use tempfile::TempDir;

fn rkat_binary_path() -> Option<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent()?;
    let debug = workspace_root.join("target/debug/rkat");
    if debug.exists() {
        return Some(debug);
    }
    let release = workspace_root.join("target/release/rkat");
    if release.exists() {
        return Some(release);
    }
    None
}

#[test]
fn e2e_rkat_init_snapshot() {
    let Some(rkat) = rkat_binary_path() else {
        eprintln!("Skipping: rkat binary not found (build with cargo build -p meerkat-cli)");
        return;
    };

    let temp_dir = TempDir::new().expect("temp dir");
    let project_dir = temp_dir.path().join("project");
    std::fs::create_dir_all(&project_dir).expect("create project dir");

    let output = Command::new(&rkat)
        .current_dir(&project_dir)
        .env("HOME", temp_dir.path())
        .args(["init"])
        .output()
        .expect("run rkat init");

    assert!(
        output.status.success(),
        "rkat init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let global_path = temp_dir.path().join(".rkat").join("config.toml");
    let project_path = project_dir.join(".rkat").join("config.toml");
    assert!(global_path.exists(), "global config should exist");
    assert!(project_path.exists(), "project config should exist");

    let global_contents = std::fs::read_to_string(&global_path).expect("read global config");
    let project_contents = std::fs::read_to_string(&project_path).expect("read project config");

    assert_eq!(
        global_contents, project_contents,
        "project config should match global template"
    );
}

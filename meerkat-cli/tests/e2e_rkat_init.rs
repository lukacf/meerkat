use std::path::PathBuf;
use tempfile::TempDir;
use tokio::process::Command;

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

#[tokio::test]
async fn e2e_rkat_init_snapshot() -> Result<(), Box<dyn std::error::Error>> {
    let rkat = rkat_binary_path().ok_or("rkat binary not found")?;

    let temp_dir = TempDir::new()?;
    let project_dir = temp_dir.path().join("my-project");
    std::fs::create_dir_all(&project_dir)?;

    let output = Command::new(&rkat)
        .current_dir(&project_dir)
        .env("HOME", temp_dir.path())
        .args(["init"])
        .output()
        .await?;

    assert!(output.status.success());

    // Verify .rkat directory and config.toml were created
    let project_path = project_dir.join(".rkat/config.toml");
    let global_path = temp_dir.path().join(".rkat/config.toml");

    assert!(project_path.exists(), "project config should exist");
    assert!(global_path.exists(), "global config should exist");

    let global_contents = std::fs::read_to_string(&global_path)?;
    let project_contents = std::fs::read_to_string(&project_path)?;

    // VERIFY: MIG-001 - verbatim copy
    assert_eq!(global_contents, project_contents);

    Ok(())
}

#![cfg(feature = "integration-real-tests")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::path::PathBuf;
use tokio::process::Command;
use tokio::time::{Duration, timeout};

use tempfile::TempDir;

fn rkat_binary_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("CARGO_BIN_EXE_rkat") {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path.canonicalize().unwrap_or(path));
        }
    }

    if let Some(target_dir) = std::env::var_os("CARGO_TARGET_DIR") {
        let target_dir = PathBuf::from(target_dir);
        let debug = target_dir.join("debug/rkat");
        if debug.exists() {
            return Some(debug);
        }
        let release = target_dir.join("release/rkat");
        if release.exists() {
            return Some(release);
        }
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent()?;
    let codex_debug = workspace_root.join("target-codex/debug/rkat");
    if codex_debug.exists() {
        return Some(codex_debug);
    }
    let codex_release = workspace_root.join("target-codex/release/rkat");
    if codex_release.exists() {
        return Some(codex_release);
    }
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

fn assert_atif_trajectory(raw: &str) -> serde_json::Value {
    let trajectory: serde_json::Value =
        serde_json::from_str(raw).expect("trajectory file is valid JSON");
    assert_eq!(
        trajectory["schema_version"].as_str(),
        Some("ATIF-v1.7"),
        "trajectory carries the ATIF schema version"
    );
    let steps = trajectory["steps"]
        .as_array()
        .expect("trajectory has a steps array");
    assert!(
        steps
            .iter()
            .any(|step| step["source"].as_str() == Some("user")),
        "trajectory contains the user turn"
    );
    assert!(
        steps
            .iter()
            .any(|step| step["source"].as_str() == Some("agent")),
        "trajectory contains an agent turn"
    );
    trajectory
}

struct RunFixture {
    rkat: PathBuf,
    home_dir: PathBuf,
    data_dir: PathBuf,
    project_dir: PathBuf,
}

impl RunFixture {
    /// One deterministic turn against the test client shim; --output json also
    /// guards the pure-JSON stdout contract. Returns (session_id, session_ref).
    async fn run_turn(
        &self,
        extra_args: &[&str],
    ) -> Result<(String, String), Box<dyn std::error::Error>> {
        let mut args = vec![
            "run",
            "Say the word 'ok' and nothing else.",
            "--yolo",
            "--output",
            "json",
        ];
        args.extend_from_slice(extra_args);
        let run_output = timeout(
            Duration::from_secs(120),
            Command::new(&self.rkat)
                .current_dir(&self.project_dir)
                .env("HOME", &self.home_dir)
                .env("XDG_DATA_HOME", &self.data_dir)
                .env("RKAT_TEST_CLIENT", "1")
                .args(&args)
                .output(),
        )
        .await??;
        assert!(
            run_output.status.success(),
            "rkat run {extra_args:?} failed: {}",
            String::from_utf8_lossy(&run_output.stderr)
        );
        let run_stdout = String::from_utf8_lossy(&run_output.stdout);
        let parsed: serde_json::Value = serde_json::from_str(run_stdout.trim())
            .map_err(|error| format!("run stdout is not pure JSON ({error}): {run_stdout}"))?;
        let session_id = parsed["session_id"]
            .as_str()
            .ok_or("session_id missing in run response")?
            .to_string();
        let session_ref = parsed["session_ref"]
            .as_str()
            .ok_or("session_ref missing in run response")?
            .to_string();
        Ok((session_id, session_ref))
    }

    /// Resolve the realm trajectory directory used by the auto-export.
    async fn trajectories_dir(
        &self,
        session_ref: &str,
    ) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let realm_id = session_ref
            .split_once(':')
            .map(|(realm_part, _)| realm_part)
            .ok_or("session_ref missing realm prefix")?;
        let realm_show = Command::new(&self.rkat)
            .current_dir(&self.project_dir)
            .env("HOME", &self.home_dir)
            .env("XDG_DATA_HOME", &self.data_dir)
            .args(["realm", "show", realm_id])
            .output()
            .await?;
        assert!(
            realm_show.status.success(),
            "rkat realm show failed: {}",
            String::from_utf8_lossy(&realm_show.stderr)
        );
        let realm_show_stdout = String::from_utf8_lossy(&realm_show.stdout);
        let state_root = realm_show_stdout
            .lines()
            .find_map(|line| line.strip_prefix("state_root: "))
            .map(PathBuf::from)
            .ok_or_else(|| {
                format!("state_root missing in realm show output: {realm_show_stdout}")
            })?;
        Ok(state_root.join(realm_id).join("trajectories"))
    }
}

#[tokio::test]
#[ignore = "lane:e2e-system"]
async fn integration_real_cli_export_atif() -> Result<(), Box<dyn std::error::Error>> {
    let Some(rkat) = rkat_binary_path() else {
        eprintln!("Skipping: missing rkat binary (build with cargo build -p meerkat-cli)");
        return Ok(());
    };

    let temp_dir = TempDir::new()?;
    let project_dir = temp_dir.path().join("project");
    tokio::fs::create_dir_all(project_dir.join(".rkat")).await?;
    let data_dir = temp_dir.path().join("data");
    tokio::fs::create_dir_all(&data_dir).await?;
    let fixture = RunFixture {
        rkat,
        home_dir: temp_dir.path().to_path_buf(),
        data_dir,
        project_dir,
    };

    // Negative arm: without --export-atif no trajectory is written (default off).
    let (plain_session_id, plain_session_ref) = fixture.run_turn(&[]).await?;
    let trajectories_dir = fixture.trajectories_dir(&plain_session_ref).await?;
    let plain_trajectory_path = trajectories_dir.join(format!("{plain_session_id}.json"));
    assert!(
        !plain_trajectory_path.exists(),
        "run without --export-atif must not write a trajectory, found {}",
        plain_trajectory_path.display()
    );

    // Flagged arm: --export-atif persists a valid trajectory for the session.
    let (flagged_session_id, _) = fixture.run_turn(&["--export-atif"]).await?;
    let flagged_trajectory_path = trajectories_dir.join(format!("{flagged_session_id}.json"));
    let auto_exported = tokio::fs::read_to_string(&flagged_trajectory_path)
        .await
        .map_err(|error| {
            format!(
                "auto-exported trajectory missing at {}: {error}",
                flagged_trajectory_path.display()
            )
        })?;
    let auto_trajectory = assert_atif_trajectory(&auto_exported);
    assert_eq!(
        auto_trajectory["session_id"].as_str(),
        Some(flagged_session_id.as_str()),
        "auto-exported trajectory names its session"
    );

    // Explicit export works for a session that was never auto-exported.
    let export_path = temp_dir.path().join("exported-trajectory.json");
    let export_output = timeout(
        Duration::from_secs(60),
        Command::new(&fixture.rkat)
            .current_dir(&fixture.project_dir)
            .env("HOME", &fixture.home_dir)
            .env("XDG_DATA_HOME", &fixture.data_dir)
            .args([
                "session",
                "export-atif",
                &plain_session_id,
                "--output",
                export_path.to_str().ok_or("export path is not UTF-8")?,
            ])
            .output(),
    )
    .await??;
    assert!(
        export_output.status.success(),
        "rkat session export-atif failed: {}",
        String::from_utf8_lossy(&export_output.stderr)
    );
    let export_stdout = String::from_utf8_lossy(&export_output.stdout);
    assert!(
        export_stdout.contains("Wrote ATIF trajectory to"),
        "export announces its destination: {export_stdout}"
    );
    let exported = tokio::fs::read_to_string(&export_path).await?;
    let trajectory = assert_atif_trajectory(&exported);
    assert_eq!(
        trajectory["session_id"].as_str(),
        Some(plain_session_id.as_str()),
        "trajectory names the exported session"
    );

    Ok(())
}

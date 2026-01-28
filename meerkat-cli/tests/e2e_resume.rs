use std::path::PathBuf;
use tokio::process::Command;
use tokio::time::{Duration, timeout};

use meerkat_core::{Config, SessionId};
use meerkat_store::{JsonlStore, SessionStore};
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

fn skip_if_no_prereqs() -> bool {
    let mut missing = Vec::new();

    if rkat_binary_path().is_none() {
        missing.push("rkat binary (build with cargo build -p meerkat-cli)");
    }

    if missing.is_empty() {
        return false;
    }

    eprintln!("Skipping: missing {}", missing.join(" and "));
    true
}

#[tokio::test]
async fn e2e_cli_resume_tools() {
    if skip_if_no_prereqs() {
        return;
    }

    let temp_dir = TempDir::new().expect("temp dir");
    let project_dir = temp_dir.path().join("project");
    std::fs::create_dir_all(project_dir.join(".rkat")).expect("create .rkat");

    let mut config = Config::default();
    config.agent.max_tokens_per_turn = 128;
    let config_toml = toml::to_string_pretty(&config).expect("serialize config");
    std::fs::write(project_dir.join(".rkat/config.toml"), config_toml).expect("write config");

    let data_dir = temp_dir.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("create data dir");
    let original_home = std::env::var_os("HOME");
    let original_xdg = std::env::var_os("XDG_DATA_HOME");
    unsafe {
        std::env::set_var("HOME", temp_dir.path());
        std::env::set_var("XDG_DATA_HOME", &data_dir);
    }

    let rkat = rkat_binary_path().expect("rkat binary");
    let output = timeout(
        Duration::from_secs(120),
        Command::new(&rkat)
            .current_dir(&project_dir)
            .env("HOME", temp_dir.path())
            .env("XDG_DATA_HOME", &data_dir)
            .env("RKAT_TEST_CLIENT", "1")
            .args([
                "run",
                "Say the word 'ok' and nothing else.",
                "--output",
                "json",
                "--enable-builtins",
            ])
            .output(),
    )
    .await
    .expect("rkat run timed out")
    .expect("run rkat");

    assert!(
        output.status.success(),
        "rkat run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).expect("parse run json");
    let session_id = parsed["session_id"]
        .as_str()
        .expect("session_id")
        .to_string();

    let store_dir = dirs::data_dir()
        .unwrap_or_else(|| temp_dir.path().to_path_buf())
        .join("meerkat")
        .join("sessions");
    let store = JsonlStore::new(store_dir);
    store.init().await.expect("init store");
    let session = store
        .load(&SessionId::parse(&session_id).expect("session id"))
        .await
        .expect("load session")
        .expect("session exists");
    let metadata = session.session_metadata().expect("metadata");
    assert!(metadata.tooling.builtins, "builtins should be recorded");

    let original_model = metadata.model.clone();
    let original_max_tokens = metadata.max_tokens;
    let original_tooling = metadata.tooling.clone();
    let original_provider = metadata.provider;

    let mut config_alt = config.clone();
    config_alt.agent.model = "gpt-4o-mini".to_string();
    config_alt.agent.max_tokens_per_turn = 7;
    let config_toml = toml::to_string_pretty(&config_alt).expect("serialize config");
    std::fs::write(project_dir.join(".rkat/config.toml"), config_toml).expect("write config");

    let output = timeout(
        Duration::from_secs(120),
        Command::new(&rkat)
            .current_dir(&project_dir)
            .env("HOME", temp_dir.path())
            .env("XDG_DATA_HOME", &data_dir)
            .env("RKAT_TEST_CLIENT", "1")
            .args(["resume", &session_id, "Continue."])
            .output(),
    )
    .await
    .expect("rkat resume timed out")
    .expect("resume rkat");

    assert!(
        output.status.success(),
        "rkat resume failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let session = store
        .load(&SessionId::parse(&session_id).expect("session id"))
        .await
        .expect("load session")
        .expect("session exists");
    let metadata = session.session_metadata().expect("metadata");

    assert_eq!(metadata.model, original_model, "model should persist");
    assert_eq!(
        metadata.max_tokens, original_max_tokens,
        "max_tokens should persist"
    );
    assert_eq!(
        metadata.provider, original_provider,
        "provider should persist"
    );
    assert_eq!(metadata.tooling.builtins, original_tooling.builtins);
    assert_eq!(metadata.tooling.shell, original_tooling.shell);
    assert_eq!(metadata.tooling.comms, original_tooling.comms);
    assert_eq!(metadata.tooling.subagents, original_tooling.subagents);

    unsafe {
        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        } else {
            std::env::remove_var("HOME");
        }
        if let Some(xdg) = original_xdg {
            std::env::set_var("XDG_DATA_HOME", xdg);
        } else {
            std::env::remove_var("XDG_DATA_HOME");
        }
    }
}

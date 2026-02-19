#![cfg(feature = "integration-real-tests")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use meerkat::Config;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tempfile::TempDir;
use tokio::process::Command;
use tokio::time::{Duration, timeout};

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

fn rkat_binary_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("CARGO_BIN_EXE_rkat") {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path);
        }
    }

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

fn first_env(names: &[&str]) -> Option<String> {
    for name in names {
        if let Ok(value) = std::env::var(name)
            && !value.is_empty()
        {
            return Some(value);
        }
    }
    None
}

fn anthropic_api_key() -> Option<String> {
    first_env(&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"])
}

fn smoke_model() -> String {
    std::env::var("SMOKE_MODEL").unwrap_or_else(|_| "claude-sonnet-4-5".to_string())
}

fn skip_if_no_api_prereqs() -> bool {
    if rkat_binary_path().is_none() {
        eprintln!("Skipping: rkat binary not found (build with `cargo build -p meerkat-cli`)");
        return true;
    }

    if anthropic_api_key().is_none() {
        eprintln!(
            "Skipping: no Anthropic API key (set ANTHROPIC_API_KEY or RKAT_ANTHROPIC_API_KEY)",
        );
        return true;
    }

    false
}

fn unique_mob_id(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let seq = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{}-{seq}", nanos % 1_000_000_000)
}

struct SmokeHarness {
    _temp_dir: TempDir,
    project_dir: PathBuf,
    data_dir: PathBuf,
    rkat: PathBuf,
    api_key: String,
}

impl SmokeHarness {
    async fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let temp_dir = TempDir::new()?;
        let project_dir = temp_dir.path().join("project");
        let data_dir = temp_dir.path().join("data");

        tokio::fs::create_dir_all(project_dir.join(".rkat")).await?;
        tokio::fs::create_dir_all(&data_dir).await?;

        let rkat = rkat_binary_path().ok_or("rkat binary not found")?;
        let api_key = anthropic_api_key().ok_or("anthropic api key missing")?;

        let mut config = Config::default();
        config.agent.max_tokens_per_turn = 320;
        config.agent.model = smoke_model();
        let config_toml = toml::to_string_pretty(&config)?;
        tokio::fs::write(project_dir.join(".rkat/config.toml"), config_toml).await?;

        Ok(Self {
            _temp_dir: temp_dir,
            project_dir,
            data_dir,
            rkat,
            api_key,
        })
    }

    async fn run_output(
        &self,
        args: &[&str],
    ) -> Result<std::process::Output, Box<dyn std::error::Error>> {
        let output = timeout(
            Duration::from_secs(240),
            Command::new(&self.rkat)
                .current_dir(&self.project_dir)
                .args(args)
                .env("HOME", &self.project_dir)
                .env("XDG_DATA_HOME", &self.data_dir)
                .env("ANTHROPIC_API_KEY", &self.api_key)
                .env("RKAT_ANTHROPIC_API_KEY", &self.api_key)
                .output(),
        )
        .await??;

        Ok(output)
    }

    async fn run_ok(&self, args: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
        let output = self.run_output(args).await?;
        if !output.status.success() {
            return Err(format!(
                "command failed (exit {:?}): rkat {}\nstdout:\n{}\nstderr:\n{}",
                output.status.code(),
                args.join(" "),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    async fn run_ok_owned(&self, args: Vec<String>) -> Result<String, Box<dyn std::error::Error>> {
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        self.run_ok(&refs).await
    }

    async fn mob_create_prefab(&self, prefab: &str) -> Result<String, Box<dyn std::error::Error>> {
        let out = self.run_ok(&["mob", "create", "--prefab", prefab]).await?;
        let mob_id = out
            .lines()
            .find(|line| !line.trim().is_empty())
            .ok_or("mob create output missing id")?
            .trim()
            .to_string();
        Ok(mob_id)
    }

    async fn mob_create_definition(
        &self,
        definition_path: &Path,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let out = self
            .run_ok_owned(vec![
                "mob".to_string(),
                "create".to_string(),
                "--definition".to_string(),
                definition_path.display().to_string(),
            ])
            .await?;
        let mob_id = out
            .lines()
            .find(|line| !line.trim().is_empty())
            .ok_or("mob create output missing id")?
            .trim()
            .to_string();
        Ok(mob_id)
    }

    async fn mob_spawn(
        &self,
        mob_id: &str,
        profile: &str,
        meerkat_id: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.run_ok_owned(vec![
            "mob".to_string(),
            "spawn".to_string(),
            mob_id.to_string(),
            profile.to_string(),
            meerkat_id.to_string(),
        ])
        .await?;
        Ok(())
    }

    async fn mob_wire(
        &self,
        mob_id: &str,
        a: &str,
        b: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.run_ok_owned(vec![
            "mob".to_string(),
            "wire".to_string(),
            mob_id.to_string(),
            a.to_string(),
            b.to_string(),
        ])
        .await?;
        Ok(())
    }

    async fn mob_unwire(
        &self,
        mob_id: &str,
        a: &str,
        b: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.run_ok_owned(vec![
            "mob".to_string(),
            "unwire".to_string(),
            mob_id.to_string(),
            a.to_string(),
            b.to_string(),
        ])
        .await?;
        Ok(())
    }

    async fn mob_retire(
        &self,
        mob_id: &str,
        meerkat_id: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.run_ok_owned(vec![
            "mob".to_string(),
            "retire".to_string(),
            mob_id.to_string(),
            meerkat_id.to_string(),
        ])
        .await?;
        Ok(())
    }

    async fn mob_turn(
        &self,
        mob_id: &str,
        meerkat_id: &str,
        message: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.run_ok_owned(vec![
            "mob".to_string(),
            "turn".to_string(),
            mob_id.to_string(),
            meerkat_id.to_string(),
            message.to_string(),
        ])
        .await?;
        Ok(())
    }

    async fn mob_status(&self, mob_id: &str) -> Result<String, Box<dyn std::error::Error>> {
        self.run_ok_owned(vec![
            "mob".to_string(),
            "status".to_string(),
            mob_id.to_string(),
        ])
        .await
    }

    async fn mob_stop(&self, mob_id: &str) -> Result<(), Box<dyn std::error::Error>> {
        self.run_ok_owned(vec![
            "mob".to_string(),
            "stop".to_string(),
            mob_id.to_string(),
        ])
        .await?;
        Ok(())
    }

    async fn mob_resume(&self, mob_id: &str) -> Result<(), Box<dyn std::error::Error>> {
        self.run_ok_owned(vec![
            "mob".to_string(),
            "resume".to_string(),
            mob_id.to_string(),
        ])
        .await?;
        Ok(())
    }

    async fn mob_complete(&self, mob_id: &str) -> Result<(), Box<dyn std::error::Error>> {
        self.run_ok_owned(vec![
            "mob".to_string(),
            "complete".to_string(),
            mob_id.to_string(),
        ])
        .await?;
        Ok(())
    }

    async fn mob_destroy(&self, mob_id: &str) -> Result<(), Box<dyn std::error::Error>> {
        self.run_ok_owned(vec![
            "mob".to_string(),
            "destroy".to_string(),
            mob_id.to_string(),
        ])
        .await?;
        Ok(())
    }

    async fn mob_list(&self) -> Result<String, Box<dyn std::error::Error>> {
        self.run_ok(&["mob", "list"]).await
    }

    async fn run_json(&self, prompt: &str) -> Result<Value, Box<dyn std::error::Error>> {
        let stdout = self
            .run_ok_owned(vec![
                "run".to_string(),
                prompt.to_string(),
                "--output".to_string(),
                "json".to_string(),
                "--enable-builtins".to_string(),
            ])
            .await?;
        Ok(serde_json::from_str(&stdout)?)
    }

    async fn resume_text(
        &self,
        session_id: &str,
        prompt: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        self.run_ok_owned(vec![
            "resume".to_string(),
            session_id.to_string(),
            prompt.to_string(),
        ])
        .await
    }

    async fn write_definition(
        &self,
        file_name: &str,
        content: &str,
    ) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let path = self.project_dir.join(file_name);
        tokio::fs::write(&path, content).await?;
        Ok(path)
    }

    async fn registry_json(&self) -> Result<Value, Box<dyn std::error::Error>> {
        let realm_id = self.run_ok(&["realm", "current"]).await?;
        let show = self
            .run_ok_owned(vec![
                "realm".to_string(),
                "show".to_string(),
                realm_id.clone(),
            ])
            .await?;
        let state_root = show
            .lines()
            .find_map(|line| line.strip_prefix("state_root: "))
            .ok_or("realm show output missing state_root")?
            .trim()
            .to_string();

        let paths = meerkat_store::realm_paths_in(Path::new(&state_root), &realm_id);
        let registry_path = paths.root.join("mob_registry.json");
        let content = tokio::fs::read_to_string(&registry_path).await?;
        Ok(serde_json::from_str(&content)?)
    }
}

fn list_has_status(list_output: &str, mob_id: &str, status: &str) -> bool {
    list_output.lines().any(|line| {
        let mut parts = line.split('\t');
        parts.next() == Some(mob_id) && parts.next() == Some(status)
    })
}

fn event_count(registry: &Value, mob_id: &str, event_type: &str) -> usize {
    registry["mobs"][mob_id]["events"]
        .as_array()
        .map(|events| {
            events
                .iter()
                .filter(|event| event["kind"]["type"].as_str() == Some(event_type))
                .count()
        })
        .unwrap_or(0)
}

fn lead_worker_definition(mob_id: &str, auto_wire_orchestrator: bool, worker_mesh: bool) -> String {
    let model = smoke_model();
    let role_wiring = if worker_mesh {
        "\n[[wiring.role_wiring]]\na = \"worker\"\nb = \"worker\"\n"
    } else {
        ""
    };

    format!(
        r#"[mob]
id = "{mob_id}"
orchestrator = "lead"

[profiles.lead]
model = "{model}"
skills = ["orchestrator"]
peer_description = "Lead orchestrator"
external_addressable = true

[profiles.lead.tools]
builtins = true
comms = true
mob = true
mob_tasks = true

[profiles.worker]
model = "{model}"
skills = ["worker"]
peer_description = "Worker"
external_addressable = false

[profiles.worker.tools]
builtins = true
shell = true
comms = true
mob_tasks = true

[wiring]
auto_wire_orchestrator = {auto_wire_orchestrator}
{role_wiring}
[skills.orchestrator]
source = "inline"
content = "You coordinate work and keep responses concise."

[skills.worker]
source = "inline"
content = "Execute assigned tasks and report concrete outcomes."
"#
    )
}

fn hierarchical_definition(mob_id: &str) -> String {
    let model = smoke_model();
    format!(
        r#"[mob]
id = "{mob_id}"
orchestrator = "lead"

[profiles.lead]
model = "{model}"
skills = ["orchestrator"]
peer_description = "Top-level orchestrator"
external_addressable = true

[profiles.lead.tools]
builtins = true
comms = true
mob = true
mob_tasks = true

[profiles.supervisor]
model = "{model}"
skills = ["supervisor"]
peer_description = "Middle manager"
external_addressable = true

[profiles.supervisor.tools]
builtins = true
comms = true
mob = true
mob_tasks = true

[profiles.worker]
model = "{model}"
skills = ["worker"]
peer_description = "Execution worker"
external_addressable = false

[profiles.worker.tools]
builtins = true
shell = true
comms = true
mob_tasks = true

[wiring]
auto_wire_orchestrator = false

[skills.orchestrator]
source = "inline"
content = "Coordinate supervisors and maintain global state."

[skills.supervisor]
source = "inline"
content = "Coordinate a subset of workers and escalate blockers."

[skills.worker]
source = "inline"
content = "Execute delegated actions and report results."
"#
    )
}

#[tokio::test]
#[ignore = "integration-real: live API"]
async fn e2e_smoke_01_basic_run_resume_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
    if skip_if_no_api_prereqs() {
        return Ok(());
    }

    let harness = SmokeHarness::new().await?;

    let token = "SMOKE_TOKEN_01";
    let run = harness
        .run_json(&format!(
            "Remember exactly this token: {token}. Reply with 'stored'."
        ))
        .await?;

    let session_id = run["session_id"]
        .as_str()
        .ok_or("run response missing session_id")?
        .to_string();

    let resumed = harness
        .resume_text(
            &session_id,
            "What token did I ask you to remember? Reply with just the token.",
        )
        .await?
        .to_lowercase();

    assert!(
        resumed.contains(&token.to_lowercase()),
        "resume response should include token; got: {resumed}"
    );

    Ok(())
}

#[tokio::test]
#[ignore = "integration-real: live API"]
async fn e2e_smoke_02_prefab_lifecycle_and_events() -> Result<(), Box<dyn std::error::Error>> {
    if skip_if_no_api_prereqs() {
        return Ok(());
    }

    let harness = SmokeHarness::new().await?;
    let mob_id = harness.mob_create_prefab("coding_swarm").await?;

    harness.mob_spawn(&mob_id, "lead", "lead-1").await?;
    harness.mob_spawn(&mob_id, "worker", "worker-1").await?;
    harness.mob_wire(&mob_id, "lead-1", "worker-1").await?;
    harness
        .mob_turn(
            &mob_id,
            "lead-1",
            "Acknowledge the worker is connected and say ready.",
        )
        .await?;

    assert_eq!(harness.mob_status(&mob_id).await?, "Running");

    let registry = harness.registry_json().await?;
    assert!(event_count(&registry, &mob_id, "meerkat_spawned") >= 2);
    assert!(event_count(&registry, &mob_id, "peers_wired") >= 1);

    harness.mob_retire(&mob_id, "worker-1").await?;
    assert!(event_count(&harness.registry_json().await?, &mob_id, "meerkat_retired") >= 1);

    harness.mob_destroy(&mob_id).await?;
    let list = harness.mob_list().await?;
    assert!(
        !list.lines().any(|line| line.starts_with(&mob_id)),
        "destroyed mob should not appear in mob list"
    );

    Ok(())
}

#[tokio::test]
#[ignore = "integration-real: live API"]
async fn e2e_smoke_03_star_topology_swarm() -> Result<(), Box<dyn std::error::Error>> {
    if skip_if_no_api_prereqs() {
        return Ok(());
    }

    let harness = SmokeHarness::new().await?;
    let mob_id = unique_mob_id("star-swarm");
    let def = lead_worker_definition(&mob_id, false, false);
    let def_path = harness.write_definition("star.toml", &def).await?;

    let created = harness.mob_create_definition(&def_path).await?;
    assert_eq!(created, mob_id);

    harness.mob_spawn(&mob_id, "lead", "lead-1").await?;
    for i in 1..=5 {
        harness
            .mob_spawn(&mob_id, "worker", &format!("w-{i}"))
            .await?;
        harness
            .mob_wire(&mob_id, "lead-1", &format!("w-{i}"))
            .await?;
    }

    harness
        .mob_turn(
            &mob_id,
            "lead-1",
            "You have a star topology; coordinate a brief swarm plan.",
        )
        .await?;

    let registry = harness.registry_json().await?;
    assert!(event_count(&registry, &mob_id, "meerkat_spawned") >= 6);
    assert!(event_count(&registry, &mob_id, "peers_wired") >= 5);

    harness.mob_complete(&mob_id).await?;
    assert_eq!(harness.mob_status(&mob_id).await?, "Completed");
    assert!(event_count(&harness.registry_json().await?, &mob_id, "mob_completed") >= 1);

    harness.mob_destroy(&mob_id).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "integration-real: live API"]
async fn e2e_smoke_04_ring_round_robin_topology() -> Result<(), Box<dyn std::error::Error>> {
    if skip_if_no_api_prereqs() {
        return Ok(());
    }

    let harness = SmokeHarness::new().await?;
    let mob_id = unique_mob_id("ring");
    let def = lead_worker_definition(&mob_id, false, false);
    let def_path = harness.write_definition("ring.toml", &def).await?;

    harness.mob_create_definition(&def_path).await?;
    harness.mob_spawn(&mob_id, "lead", "lead-1").await?;

    for worker in ["w-1", "w-2", "w-3", "w-4"] {
        harness.mob_spawn(&mob_id, "worker", worker).await?;
    }

    for (a, b) in [
        ("w-1", "w-2"),
        ("w-2", "w-3"),
        ("w-3", "w-4"),
        ("w-4", "w-1"),
    ] {
        harness.mob_wire(&mob_id, a, b).await?;
    }

    harness
        .mob_turn(
            &mob_id,
            "lead-1",
            "Acknowledge the ring topology and assign a round-robin sequence.",
        )
        .await?;

    harness.mob_unwire(&mob_id, "w-4", "w-1").await?;
    harness.mob_wire(&mob_id, "w-1", "w-3").await?;

    let registry = harness.registry_json().await?;
    assert!(event_count(&registry, &mob_id, "peers_wired") >= 5);
    assert!(event_count(&registry, &mob_id, "peers_unwired") >= 1);

    harness.mob_destroy(&mob_id).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "integration-real: live API"]
async fn e2e_smoke_05_hierarchical_three_tier_topology() -> Result<(), Box<dyn std::error::Error>> {
    if skip_if_no_api_prereqs() {
        return Ok(());
    }

    let harness = SmokeHarness::new().await?;
    let mob_id = unique_mob_id("hier");
    let def_path = harness
        .write_definition("hier.toml", &hierarchical_definition(&mob_id))
        .await?;

    harness.mob_create_definition(&def_path).await?;
    harness.mob_spawn(&mob_id, "lead", "lead-1").await?;
    harness.mob_spawn(&mob_id, "supervisor", "sup-1").await?;
    harness.mob_spawn(&mob_id, "supervisor", "sup-2").await?;

    for worker in ["w-1", "w-2", "w-3", "w-4"] {
        harness.mob_spawn(&mob_id, "worker", worker).await?;
    }

    for (a, b) in [
        ("lead-1", "sup-1"),
        ("lead-1", "sup-2"),
        ("sup-1", "w-1"),
        ("sup-1", "w-2"),
        ("sup-2", "w-3"),
        ("sup-2", "w-4"),
    ] {
        harness.mob_wire(&mob_id, a, b).await?;
    }

    harness
        .mob_turn(
            &mob_id,
            "lead-1",
            "Confirm hierarchical routing through supervisors.",
        )
        .await?;
    harness
        .mob_turn(&mob_id, "sup-1", "Summarize your worker lane.")
        .await?;

    harness.mob_stop(&mob_id).await?;
    assert_eq!(harness.mob_status(&mob_id).await?, "Stopped");

    harness.mob_resume(&mob_id).await?;
    assert_eq!(harness.mob_status(&mob_id).await?, "Running");

    let registry = harness.registry_json().await?;
    assert!(event_count(&registry, &mob_id, "meerkat_spawned") >= 7);
    assert!(event_count(&registry, &mob_id, "peers_wired") >= 6);

    harness.mob_destroy(&mob_id).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "integration-real: live API"]
async fn e2e_smoke_06_mesh_swarm_auto_wiring() -> Result<(), Box<dyn std::error::Error>> {
    if skip_if_no_api_prereqs() {
        return Ok(());
    }

    let harness = SmokeHarness::new().await?;
    let mob_id = unique_mob_id("mesh");
    let def = lead_worker_definition(&mob_id, true, true);
    let def_path = harness.write_definition("mesh.toml", &def).await?;

    harness.mob_create_definition(&def_path).await?;
    harness.mob_spawn(&mob_id, "lead", "lead-1").await?;
    harness.mob_spawn(&mob_id, "worker", "w-1").await?;
    harness.mob_spawn(&mob_id, "worker", "w-2").await?;
    harness.mob_spawn(&mob_id, "worker", "w-3").await?;

    harness
        .mob_turn(
            &mob_id,
            "lead-1",
            "Acknowledge mesh-like worker connectivity and proceed.",
        )
        .await?;

    let registry = harness.registry_json().await?;
    assert!(
        event_count(&registry, &mob_id, "peers_wired") >= 5,
        "expected auto/role wiring to emit multiple peer wire events"
    );

    harness.mob_complete(&mob_id).await?;
    assert_eq!(harness.mob_status(&mob_id).await?, "Completed");

    harness.mob_destroy(&mob_id).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "integration-real: live API"]
async fn e2e_smoke_07_dual_mob_isolation() -> Result<(), Box<dyn std::error::Error>> {
    if skip_if_no_api_prereqs() {
        return Ok(());
    }

    let harness = SmokeHarness::new().await?;

    let mob_a = unique_mob_id("iso-a");
    let mob_b = unique_mob_id("iso-b");

    let def_a = harness
        .write_definition("iso-a.toml", &lead_worker_definition(&mob_a, false, false))
        .await?;
    let def_b = harness
        .write_definition("iso-b.toml", &lead_worker_definition(&mob_b, false, false))
        .await?;

    harness.mob_create_definition(&def_a).await?;
    harness.mob_create_definition(&def_b).await?;

    for mob in [&mob_a, &mob_b] {
        harness.mob_spawn(mob, "lead", "lead-1").await?;
        harness.mob_spawn(mob, "worker", "worker-1").await?;
        harness.mob_wire(mob, "lead-1", "worker-1").await?;
        harness
            .mob_turn(mob, "lead-1", "Confirm your local worker assignment.")
            .await?;
    }

    let list = harness.mob_list().await?;
    assert!(list_has_status(&list, &mob_a, "Running"));
    assert!(list_has_status(&list, &mob_b, "Running"));

    let registry = harness.registry_json().await?;
    assert!(event_count(&registry, &mob_a, "meerkat_spawned") >= 2);
    assert!(event_count(&registry, &mob_b, "meerkat_spawned") >= 2);

    harness.mob_destroy(&mob_a).await?;
    harness.mob_destroy(&mob_b).await?;

    Ok(())
}

#[tokio::test]
#[ignore = "integration-real: live API"]
async fn e2e_smoke_08_kitchen_sink_lifecycle_mutations() -> Result<(), Box<dyn std::error::Error>> {
    if skip_if_no_api_prereqs() {
        return Ok(());
    }

    let harness = SmokeHarness::new().await?;
    let mob_id = unique_mob_id("kitchen");
    let def_path = harness
        .write_definition("kitchen.toml", &hierarchical_definition(&mob_id))
        .await?;

    harness.mob_create_definition(&def_path).await?;
    harness.mob_spawn(&mob_id, "lead", "lead-1").await?;
    harness.mob_spawn(&mob_id, "supervisor", "sup-1").await?;
    harness.mob_spawn(&mob_id, "worker", "w-1").await?;
    harness.mob_spawn(&mob_id, "worker", "w-2").await?;
    harness.mob_spawn(&mob_id, "worker", "w-3").await?;

    harness.mob_wire(&mob_id, "lead-1", "sup-1").await?;
    harness.mob_wire(&mob_id, "sup-1", "w-1").await?;
    harness.mob_wire(&mob_id, "sup-1", "w-2").await?;
    harness.mob_wire(&mob_id, "sup-1", "w-3").await?;

    harness.mob_unwire(&mob_id, "sup-1", "w-3").await?;
    harness.mob_retire(&mob_id, "w-2").await?;
    harness.mob_spawn(&mob_id, "worker", "w-4").await?;
    harness.mob_wire(&mob_id, "sup-1", "w-4").await?;

    harness
        .mob_turn(
            &mob_id,
            "lead-1",
            "Re-plan after topology mutation and worker retirement.",
        )
        .await?;

    harness.mob_stop(&mob_id).await?;
    harness.mob_resume(&mob_id).await?;
    harness.mob_complete(&mob_id).await?;

    let registry = harness.registry_json().await?;
    assert!(event_count(&registry, &mob_id, "meerkat_spawned") >= 6);
    assert!(event_count(&registry, &mob_id, "meerkat_retired") >= 1);
    assert!(event_count(&registry, &mob_id, "peers_unwired") >= 1);
    assert!(event_count(&registry, &mob_id, "mob_completed") >= 1);

    harness.mob_destroy(&mob_id).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "integration-real: live API"]
async fn e2e_smoke_09_cross_surface_capabilities_config_run_mob()
-> Result<(), Box<dyn std::error::Error>> {
    if skip_if_no_api_prereqs() {
        return Ok(());
    }

    let harness = SmokeHarness::new().await?;

    let capabilities = harness.run_ok(&["capabilities"]).await?;
    let caps: Value = serde_json::from_str(&capabilities)?;
    assert!(caps.get("contract_version").is_some());

    let config_out = harness.run_ok(&["config", "get"]).await?;
    assert!(!config_out.trim().is_empty());

    let run = harness
        .run_json("Reply with exactly: CROSS_SURFACE_OK")
        .await?;
    let session_id = run["session_id"]
        .as_str()
        .ok_or("run response missing session_id")?
        .to_string();

    let mob_id = harness.mob_create_prefab("pipeline").await?;
    harness.mob_spawn(&mob_id, "lead", "lead-1").await?;
    harness.mob_spawn(&mob_id, "worker", "worker-1").await?;
    harness.mob_wire(&mob_id, "lead-1", "worker-1").await?;
    harness
        .mob_turn(
            &mob_id,
            "lead-1",
            "Confirm pipeline orchestration readiness.",
        )
        .await?;

    let resumed = harness
        .resume_text(&session_id, "Repeat the phrase CROSS_SURFACE_OK")
        .await?
        .to_lowercase();
    assert!(resumed.contains("cross_surface_ok"));

    harness.mob_destroy(&mob_id).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "integration-real: live API"]
async fn e2e_smoke_10_topology_migration_star_to_ring() -> Result<(), Box<dyn std::error::Error>> {
    if skip_if_no_api_prereqs() {
        return Ok(());
    }

    let harness = SmokeHarness::new().await?;
    let mob_id = unique_mob_id("migrate");
    let def_path = harness
        .write_definition(
            "migrate.toml",
            &lead_worker_definition(&mob_id, false, false),
        )
        .await?;

    harness.mob_create_definition(&def_path).await?;
    harness.mob_spawn(&mob_id, "lead", "lead-1").await?;

    for worker in ["w-1", "w-2", "w-3", "w-4", "w-5"] {
        harness.mob_spawn(&mob_id, "worker", worker).await?;
        harness.mob_wire(&mob_id, "lead-1", worker).await?;
    }

    harness
        .mob_turn(
            &mob_id,
            "lead-1",
            "You are in hub-and-spoke mode; acknowledge.",
        )
        .await?;

    for worker in ["w-1", "w-2", "w-3", "w-4", "w-5"] {
        harness.mob_unwire(&mob_id, "lead-1", worker).await?;
    }

    for (a, b) in [
        ("w-1", "w-2"),
        ("w-2", "w-3"),
        ("w-3", "w-4"),
        ("w-4", "w-5"),
        ("w-5", "w-1"),
    ] {
        harness.mob_wire(&mob_id, a, b).await?;
    }

    harness
        .mob_turn(
            &mob_id,
            "lead-1",
            "You are now in ring mode; produce a concise migration confirmation.",
        )
        .await?;

    let registry = harness.registry_json().await?;
    assert!(event_count(&registry, &mob_id, "peers_wired") >= 10);
    assert!(event_count(&registry, &mob_id, "peers_unwired") >= 5);

    harness.mob_destroy(&mob_id).await?;
    Ok(())
}

#![cfg(feature = "integration-real-tests")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use meerkat::Config;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tempfile::TempDir;
use tokio::process::Command;
use tokio::time::{Duration, Instant, interval};

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

fn smoke_timeout_secs() -> u64 {
    std::env::var("SMOKE_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .map(|value| value.min(120))
        .unwrap_or(120)
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
    verbose: bool,
    provider: Option<String>,
    model: String,
}

impl SmokeHarness {
    async fn read_stream(
        mut stream: impl tokio::io::AsyncRead + Unpin,
        prefix: &'static str,
        verbose: bool,
    ) -> std::io::Result<Vec<u8>> {
        use tokio::io::AsyncReadExt;

        let mut captured = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let read = stream.read(&mut buf).await?;
            if read == 0 {
                break;
            }
            captured.extend_from_slice(&buf[..read]);
            if verbose {
                eprintln!("{prefix} {}", String::from_utf8_lossy(&buf[..read]));
            }
        }
        Ok(captured)
    }

    async fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let temp_dir = TempDir::new()?;
        let project_dir = temp_dir.path().join("project");
        let data_dir = temp_dir.path().join("data");

        tokio::fs::create_dir_all(project_dir.join(".rkat")).await?;
        tokio::fs::create_dir_all(&data_dir).await?;

        let rkat = rkat_binary_path().ok_or("rkat binary not found")?;
        let api_key = anthropic_api_key().ok_or("anthropic api key missing")?;
        let verbose = std::env::var("SMOKE_VERBOSE")
            .ok()
            .map(|value| {
                let value = value.to_ascii_lowercase();
                matches!(value.as_str(), "1" | "true" | "yes" | "on")
            })
            .unwrap_or(false);
        let provider = std::env::var("SMOKE_PROVIDER")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());

        let model = smoke_model();
        let mut config = Config::default();
        config.agent.max_tokens_per_turn = 320;
        config.agent.model = model.clone();
        let config_toml = toml::to_string_pretty(&config)?;
        tokio::fs::write(project_dir.join(".rkat/config.toml"), config_toml).await?;

        Ok(Self {
            _temp_dir: temp_dir,
            project_dir,
            data_dir,
            rkat,
            api_key,
            verbose,
            provider,
            model,
        })
    }

    async fn run_output(
        &self,
        args: &[&str],
    ) -> Result<std::process::Output, Box<dyn std::error::Error>> {
        let rendered_args = args.join(" ");
        let timeout_secs = smoke_timeout_secs();
        if self.verbose {
            eprintln!("[smoke] launching: rkat {rendered_args}");
            eprintln!(
                "[smoke] context: model={} provider={} timeout={}s cwd={}",
                self.model,
                self.provider.as_deref().unwrap_or("default"),
                timeout_secs,
                self.project_dir.display()
            );
        }
        let mut command = Command::new(&self.rkat);
        command
            .current_dir(&self.project_dir)
            .env("HOME", &self.project_dir)
            .env("XDG_DATA_HOME", &self.data_dir)
            .env("ANTHROPIC_API_KEY", &self.api_key)
            .env("RKAT_ANTHROPIC_API_KEY", &self.api_key)
            .env("RKAT_MOB_DEBUG", if self.verbose { "1" } else { "0" })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command.args(args);
        let mut child = command.spawn()?;
        let stdout = child.stdout.take().ok_or("failed to capture stdout")?;
        let stderr = child.stderr.take().ok_or("failed to capture stderr")?;
        let stdout_task = tokio::spawn(Self::read_stream(stdout, "[smoke][stdout+]", self.verbose));
        let stderr_task = tokio::spawn(Self::read_stream(stderr, "[smoke][stderr+]", self.verbose));

        let started = Instant::now();
        let mut heartbeat = interval(Duration::from_secs(10));
        let mut deadline = Box::pin(tokio::time::sleep(Duration::from_secs(timeout_secs)));
        let status = loop {
            tokio::select! {
                waited = child.wait() => {
                    break waited?;
                }
                _ = heartbeat.tick() => {
                    if self.verbose {
                        eprintln!(
                            "[smoke] still running after {:.1}s: rkat {rendered_args}",
                            started.elapsed().as_secs_f32()
                        );
                    }
                }
                _ = &mut deadline => {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    let stdout = stdout_task.await??;
                    let stderr = stderr_task.await??;
                    return Err(format!(
                        "timeout after {}s: rkat {}\npartial stdout:\n{}\npartial stderr:\n{}",
                        timeout_secs,
                        rendered_args,
                        String::from_utf8_lossy(&stdout),
                        String::from_utf8_lossy(&stderr)
                    ).into());
                }
            }
        };

        let stdout = stdout_task.await??;
        let stderr = stderr_task.await??;

        if self.verbose {
            eprintln!(
                "[smoke] completed in {:.2}s with exit {:?}: rkat {}",
                started.elapsed().as_secs_f32(),
                status.code(),
                rendered_args
            );
        }

        Ok(std::process::Output {
            status,
            stdout,
            stderr,
        })
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

    async fn run_output_owned(
        &self,
        args: Vec<String>,
    ) -> Result<std::process::Output, Box<dyn std::error::Error>> {
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        self.run_output(&refs).await
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
        let mut args = vec![
            "run".to_string(),
            prompt.to_string(),
            "--model".to_string(),
            self.model.clone(),
            "--output".to_string(),
            "json".to_string(),
            "--enable-builtins".to_string(),
        ];
        if let Some(provider) = &self.provider {
            args.push("--provider".to_string());
            args.push(provider.clone());
        }
        if self.verbose {
            args.push("--verbose".to_string());
        }
        let stdout = self.run_ok_owned(args).await?;
        Ok(serde_json::from_str(&stdout)?)
    }

    async fn run_json_with_schema(
        &self,
        prompt: &str,
        schema: &Value,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        self.run_json_with_schema_capture(prompt, schema, false)
            .await
            .map(|(value, _)| value)
    }

    async fn run_json_with_schema_capture(
        &self,
        prompt: &str,
        schema: &Value,
        force_verbose: bool,
    ) -> Result<(Value, String), Box<dyn std::error::Error>> {
        let schema_inline = serde_json::to_string(schema)?;
        let mut args = vec![
            "run".to_string(),
            prompt.to_string(),
            "--model".to_string(),
            self.model.clone(),
            "--output".to_string(),
            "json".to_string(),
            "--output-schema".to_string(),
            schema_inline,
            "--enable-builtins".to_string(),
        ];
        if let Some(provider) = &self.provider {
            args.push("--provider".to_string());
            args.push(provider.clone());
        }
        if self.verbose || force_verbose {
            args.push("--verbose".to_string());
        }
        let output = self.run_output_owned(args).await?;
        if !output.status.success() {
            return Err(format!(
                "command failed (exit {:?}): run with schema\nstdout:\n{}\nstderr:\n{}",
                output.status.code(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Ok((serde_json::from_str(&stdout)?, stderr))
    }

    async fn resume_text(
        &self,
        session_id: &str,
        prompt: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let mut args = vec!["resume".to_string()];
        if self.verbose {
            args.push("--verbose".to_string());
        }
        args.push(session_id.to_string());
        args.push(prompt.to_string());
        self.run_ok_owned(args).await
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
        let registry_path = self.registry_path().await?;
        let content = tokio::fs::read_to_string(&registry_path).await?;
        Ok(serde_json::from_str(&content)?)
    }

    async fn registry_path(&self) -> Result<PathBuf, Box<dyn std::error::Error>> {
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
        Ok(paths.root.join("mob_registry.json"))
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

fn parse_tool_latencies(stderr: &str) -> Vec<(String, u64)> {
    stderr
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            let first = trimmed.chars().next()?;
            if first != '✓' && first != '✗' {
                return None;
            }
            let rest = trimmed.get(first.len_utf8()..)?.trim_start();
            let (name, tail) = rest.split_once(' ')?;
            let open = tail.find('(')?;
            let close = tail[open + 1..].find("ms)")?;
            let millis = tail[open + 1..open + 1 + close].parse::<u64>().ok()?;
            Some((name.to_string(), millis))
        })
        .collect()
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
    let mob_id = unique_mob_id("tool-kitchen");
    let model = smoke_model();
    let definition = json!({
        "id": mob_id,
        "orchestrator": {"profile": "lead"},
        "profiles": {
            "lead": {
                "model": model,
                "skills": [],
                "tools": {
                    "builtins": true,
                    "shell": false,
                    "comms": true,
                    "memory": false,
                    "mob": true,
                    "mob_tasks": true,
                    "mcp": [],
                    "rust_bundles": []
                },
                "peer_description": "Lead orchestrator",
                "external_addressable": true
            },
            "worker": {
                "model": model,
                "skills": [],
                "tools": {
                    "builtins": true,
                    "shell": true,
                    "comms": true,
                    "memory": false,
                    "mob": false,
                    "mob_tasks": true,
                    "mcp": [],
                    "rust_bundles": []
                },
                "peer_description": "Worker",
                "external_addressable": false
            }
        },
        "mcp_servers": {},
        "wiring": {
            "auto_wire_orchestrator": false,
            "role_wiring": []
        },
        "skills": {}
    });
    let definition_json = serde_json::to_string(&definition)?;

    let create_schema = json!({
        "type": "object",
        "properties": {
            "mob_id": {"type": "string"}
        },
        "required": ["mob_id"],
        "additionalProperties": false
    });
    let create_run = harness.run_json_with_schema(
            &format!(
                "Use mob_* tools only (no simulation). Call mob_create with arguments {{\"definition\": {}}}. Return structured output with field mob_id only.",
                definition_json
            ),
            &create_schema,
        ).await?;
    assert!(
        create_run["tool_calls"].as_u64().unwrap_or(0) >= 1,
        "expected mob_create tool call; got: {}",
        create_run["tool_calls"]
    );
    let create_result = create_run["structured_output"]
        .as_object()
        .ok_or("create run structured_output missing object")?;
    let returned_mob_id = create_result
        .get("mob_id")
        .and_then(Value::as_str)
        .ok_or("create run missing mob_id")?;
    assert_eq!(returned_mob_id, mob_id);

    let session_id = create_run["session_id"]
        .as_str()
        .ok_or("create run missing session_id")?
        .to_string();

    async fn quick(
        harness: &SmokeHarness,
        session_id: &str,
        prompt: String,
    ) -> Result<(), Box<dyn std::error::Error>> {
        harness.resume_text(session_id, &prompt).await?;
        Ok(())
    }

    quick(
        &harness,
        &session_id,
        format!(
        "Use mob_* tools only. For mob '{}', call mob_spawn with profile='lead' and meerkat_id='lead-1'. Reply exactly OK.",
        returned_mob_id
    ),
    )
    .await?;
    quick(
        &harness,
        &session_id,
        format!(
        "Use mob_* tools only. For mob '{}', call mob_spawn with profile='worker' and meerkat_id='worker-1'. Reply exactly OK.",
        returned_mob_id
    ),
    )
    .await?;
    quick(
        &harness,
        &session_id,
        format!(
        "Use mob_* tools only. For mob '{}', call mob_spawn with profile='worker' and meerkat_id='worker-2'. Reply exactly OK.",
        returned_mob_id
    ),
    )
    .await?;
    quick(
        &harness,
        &session_id,
        format!(
        "Use mob_* tools only. For mob '{}', call mob_wire with a='lead-1' and b='worker-1'. Reply exactly OK.",
        returned_mob_id
    ),
    )
    .await?;
    quick(
        &harness,
        &session_id,
        format!(
        "Use mob_* tools only. For mob '{}', call mob_wire with a='lead-1' and b='worker-2'. Reply exactly OK.",
        returned_mob_id
    ),
    )
    .await?;
    quick(
        &harness,
        &session_id,
        format!(
        "Use mob_* tools only. For mob '{}', call mob_unwire with a='lead-1' and b='worker-2'. Reply exactly OK.",
        returned_mob_id
    ),
    )
    .await?;
    quick(
        &harness,
        &session_id,
        format!(
        "Use mob_* tools only. For mob '{}', call mob_retire with meerkat_id='worker-2'. Reply exactly OK.",
        returned_mob_id
    ),
    )
    .await?;
    quick(
        &harness,
        &session_id,
        format!(
        "Use mob_* tools only. For mob '{}', call mob_spawn with profile='worker' and meerkat_id='worker-3'. Reply exactly OK.",
        returned_mob_id
    ),
    )
    .await?;
    quick(
        &harness,
        &session_id,
        format!(
        "Use mob_* tools only. For mob '{}', call mob_wire with a='lead-1' and b='worker-3'. Reply exactly OK.",
        returned_mob_id
    ),
    )
    .await?;
    quick(
        &harness,
        &session_id,
        format!(
            "Use mob_* tools only. For mob '{}', call mob_stop. Reply exactly OK.",
            returned_mob_id
        ),
    )
    .await?;
    quick(
        &harness,
        &session_id,
        format!(
            "Use mob_* tools only. For mob '{}', call mob_resume. Reply exactly OK.",
            returned_mob_id
        ),
    )
    .await?;
    quick(
        &harness,
        &session_id,
        format!(
            "Use mob_* tools only. For mob '{}', call mob_complete. Reply exactly OK.",
            returned_mob_id
        ),
    )
    .await?;

    let summary = harness
        .resume_text(
            &session_id,
            &format!(
                "Use mob_* tools only (no simulation) for existing mob '{}':\n\
                 1) mob_status\n\
                 2) mob_events with after_cursor=0 and limit=200\n\
                 3) mob_destroy\n\
                 Respond with exactly four lines:\n\
                 FINAL:<final_status>\n\
                 EVENTS:<comma-separated unique event kind.type values>\n\
                 KINDS:<comma-separated member_ref.kind values from meerkat_spawned events>\n\
                 DESTROYED:true",
                returned_mob_id
            ),
        )
        .await?;
    let final_status = summary
        .lines()
        .find_map(|line| line.strip_prefix("FINAL:"))
        .map(str::trim)
        .ok_or("summary response missing FINAL line")?;
    assert_eq!(final_status, "Completed");
    let event_types = summary
        .lines()
        .find_map(|line| line.strip_prefix("EVENTS:"))
        .map(str::trim)
        .ok_or("summary response missing EVENTS line")?
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    assert!(
        event_types
            .iter()
            .any(|value| value.contains("meerkat_spawned")),
        "expected spawned event in summary: {event_types:?}"
    );
    assert!(
        event_types
            .iter()
            .any(|value| value.contains("meerkat_retired")),
        "expected retired event in summary: {event_types:?}"
    );
    assert!(
        event_types
            .iter()
            .any(|value| value.contains("peers_unwired")),
        "expected unwired event in summary: {event_types:?}"
    );
    assert!(
        event_types
            .iter()
            .any(|value| value.contains("mob_completed")),
        "expected completed event in summary: {event_types:?}"
    );
    let spawned_kinds = summary
        .lines()
        .find_map(|line| line.strip_prefix("KINDS:"))
        .map(str::trim)
        .ok_or("summary response missing KINDS line")?
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    assert!(
        !spawned_kinds.is_empty() && spawned_kinds.iter().all(|kind| *kind == "session"),
        "default backend for run+mob should remain subagent(session); got {spawned_kinds:?}"
    );
    assert!(
        summary.lines().any(|line| line.trim() == "DESTROYED:true"),
        "summary response missing DESTROYED:true line: {summary}"
    );
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

#[tokio::test]
#[ignore = "integration-real: live API"]
async fn e2e_smoke_11_external_backend_lifecycle() -> Result<(), Box<dyn std::error::Error>> {
    if skip_if_no_api_prereqs() {
        return Ok(());
    }

    let harness = SmokeHarness::new().await?;
    let mob_id = unique_mob_id("ext-lifecycle");
    let model = smoke_model();
    let definition = json!({
        "id": mob_id,
        "orchestrator": {"profile": "lead"},
        "profiles": {
            "lead": {
                "model": model,
                "skills": [],
                "tools": {"builtins": true, "shell": false, "comms": true, "memory": false, "mob": true, "mob_tasks": true, "mcp": [], "rust_bundles": []},
                "peer_description": "Lead",
                "external_addressable": true
            },
            "worker": {
                "model": model,
                "skills": [],
                "tools": {"builtins": true, "shell": false, "comms": true, "memory": false, "mob": false, "mob_tasks": true, "mcp": [], "rust_bundles": []},
                "peer_description": "Worker",
                "external_addressable": true
            }
        },
        "backend": {
            "default": "external",
            "external": {"address_base": "https://backend.example.invalid/mesh"}
        },
        "mcp_servers": {},
        "wiring": {"auto_wire_orchestrator": false, "role_wiring": []},
        "skills": {}
    });
    let schema = json!({
        "type": "object",
        "properties": {
            "mob_id": {"type": "string"},
            "member_kinds": {"type": "array", "items": {"type": "string"}},
            "member_addresses": {"type": "array", "items": {"type": "string"}},
            "final_status": {"type": "string"},
            "destroyed": {"type": "boolean"}
        },
        "required": ["mob_id", "member_kinds", "member_addresses", "final_status", "destroyed"],
        "additionalProperties": false
    });
    let run = harness
        .run_json_with_schema(
            &format!(
                "Use mob_* tools only. Execute: \
                 1) mob_create with definition={} \
                 2) mob_spawn lead-1 on lead (no backend arg) \
                 3) mob_spawn worker-1 on worker (no backend arg) \
                 4) mob_list_meerkats \
                 5) mob_wire lead-1 worker-1 \
                 6) mob_external_turn lead-1 message='external ping' \
                 7) mob_complete \
                 8) mob_status \
                 9) mob_destroy. \
                 Return structured output with: mob_id, member_kinds from list_meerkats[*].member_ref.kind, member_addresses from list_meerkats[*].member_ref.address (when present), final_status from mob_status, destroyed=true.",
                serde_json::to_string(&definition)?
            ),
            &schema,
        )
        .await?;

    let out = run["structured_output"]
        .as_object()
        .ok_or("structured_output missing")?;
    assert_eq!(
        out.get("mob_id").and_then(Value::as_str),
        Some(mob_id.as_str())
    );
    let kinds = out
        .get("member_kinds")
        .and_then(Value::as_array)
        .ok_or("member_kinds missing")?
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    assert!(
        kinds.iter().all(|kind| *kind == "backend_peer"),
        "external default should emit backend peers, got {kinds:?}"
    );
    let addresses = out
        .get("member_addresses")
        .and_then(Value::as_array)
        .ok_or("member_addresses missing")?
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    assert!(
        !addresses.is_empty()
            && addresses
                .iter()
                .all(|addr| addr.starts_with("https://backend.example.invalid/mesh/")),
        "external members must use configured backend address base: {addresses:?}"
    );
    assert_eq!(
        out.get("final_status").and_then(Value::as_str),
        Some("Completed")
    );
    assert_eq!(out.get("destroyed"), Some(&Value::Bool(true)));
    Ok(())
}

#[tokio::test]
#[ignore = "integration-real: live API"]
async fn e2e_smoke_12_mixed_backend_and_verbose_observability()
-> Result<(), Box<dyn std::error::Error>> {
    if skip_if_no_api_prereqs() {
        return Ok(());
    }

    let harness = SmokeHarness::new().await?;
    let mob_id = unique_mob_id("mixed-topology");
    let model = smoke_model();
    let definition = json!({
        "id": mob_id,
        "orchestrator": {"profile": "lead"},
        "profiles": {
            "lead": {
                "model": model,
                "skills": [],
                "tools": {"builtins": true, "shell": false, "comms": true, "memory": false, "mob": true, "mob_tasks": true, "mcp": [], "rust_bundles": []},
                "peer_description": "Lead",
                "external_addressable": true
            },
            "worker": {
                "model": model,
                "skills": [],
                "tools": {"builtins": true, "shell": false, "comms": true, "memory": false, "mob": false, "mob_tasks": true, "mcp": [], "rust_bundles": []},
                "peer_description": "Worker",
                "external_addressable": true
            }
        },
        "backend": {
            "default": "subagent",
            "external": {"address_base": "https://backend.example.invalid/mesh"}
        },
        "mcp_servers": {},
        "wiring": {"auto_wire_orchestrator": false, "role_wiring": []},
        "skills": {}
    });
    let schema = json!({
        "type": "object",
        "properties": {
            "mob_id": {"type": "string"},
            "sub_kind": {"type": "string"},
            "ext_kind": {"type": "string"},
            "ext_address": {"type": "string"},
            "destroyed": {"type": "boolean"}
        },
        "required": ["mob_id", "sub_kind", "ext_kind", "ext_address", "destroyed"],
        "additionalProperties": false
    });
    let (run, stderr) = harness
        .run_json_with_schema_capture(
            &format!(
                "Use mob_* tools only. Execute exactly: \
                 1) mob_create with definition={} \
                 2) mob_spawn worker-sub on worker without backend arg (capture returned member_ref.kind as sub_kind) \
                 3) mob_spawn worker-ext on worker with backend='external' (capture member_ref.kind as ext_kind and member_ref.address as ext_address) \
                 4) mob_wire worker-sub worker-ext \
                 5) mob_events after_cursor=0 limit=200 \
                 6) mob_destroy \
                 Return structured output: mob_id, sub_kind, ext_kind, ext_address, destroyed=true.",
                serde_json::to_string(&definition)?
            ),
            &schema,
            true,
        )
        .await?;
    let out = run["structured_output"]
        .as_object()
        .ok_or("structured_output missing")?;
    assert_eq!(
        out.get("mob_id").and_then(Value::as_str),
        Some(mob_id.as_str())
    );
    assert_eq!(out.get("sub_kind").and_then(Value::as_str), Some("session"));
    assert_eq!(
        out.get("ext_kind").and_then(Value::as_str),
        Some("backend_peer")
    );
    let ext_addr = out
        .get("ext_address")
        .and_then(Value::as_str)
        .ok_or("ext_address missing")?;
    assert!(
        ext_addr.starts_with("https://backend.example.invalid/mesh/"),
        "mixed backend should retain external address identity: {ext_addr}"
    );
    assert_eq!(out.get("destroyed"), Some(&Value::Bool(true)));

    assert!(
        stderr.contains("mob_spawn"),
        "verbose output should show mob tool interactions"
    );
    assert!(
        stderr.contains("\"backend\":\"external\""),
        "verbose output should include backend-specific spawn arguments"
    );
    let latencies = parse_tool_latencies(&stderr);
    assert!(
        !latencies.is_empty(),
        "verbose output should include tool durations for latency diagnosis"
    );
    let mut slowest = latencies.clone();
    slowest.sort_by_key(|(_, ms)| std::cmp::Reverse(*ms));
    let summary = slowest
        .iter()
        .take(5)
        .map(|(name, ms)| format!("{name}:{ms}ms"))
        .collect::<Vec<_>>()
        .join("\n");

    let artifact_dir = std::env::var("SMOKE_ARTIFACT_DIR")
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(|| harness.project_dir.join(".rkat"));
    tokio::fs::create_dir_all(&artifact_dir).await?;
    let artifact_path = artifact_dir.join("phase3_mixed_verbose_observability.log");
    tokio::fs::write(
        &artifact_path,
        format!("slowest_tools:\n{summary}\n\nfull_verbose_output:\n{stderr}",),
    )
    .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "integration-real: live API"]
async fn e2e_smoke_13_legacy_resume_event_shape_compat() -> Result<(), Box<dyn std::error::Error>> {
    if skip_if_no_api_prereqs() {
        return Ok(());
    }

    let harness = SmokeHarness::new().await?;
    let mob_id = unique_mob_id("legacy-resume");
    let def_path = harness
        .write_definition(
            "legacy-resume.toml",
            &lead_worker_definition(&mob_id, false, false),
        )
        .await?;
    harness.mob_create_definition(&def_path).await?;
    harness.mob_spawn(&mob_id, "worker", "w-1").await?;
    harness.mob_stop(&mob_id).await?;

    let registry_path = harness.registry_path().await?;
    let mut registry: Value =
        serde_json::from_str(&tokio::fs::read_to_string(&registry_path).await?)?;
    let events = registry["mobs"][mob_id.as_str()]["events"]
        .as_array_mut()
        .ok_or("registry events missing")?;
    for event in events.iter_mut() {
        if event["kind"]["type"].as_str() != Some("meerkat_spawned") {
            continue;
        }
        let sid = event["kind"]["member_ref"]["session_id"].clone();
        if sid.is_null() {
            continue;
        }
        if let Some(kind) = event["kind"].as_object_mut() {
            kind.remove("member_ref");
            kind.insert("session_id".to_string(), sid);
        }
    }
    tokio::fs::write(&registry_path, serde_json::to_string_pretty(&registry)?).await?;

    harness.mob_resume(&mob_id).await?;
    assert_eq!(harness.mob_status(&mob_id).await?, "Running");
    harness.mob_destroy(&mob_id).await?;
    Ok(())
}

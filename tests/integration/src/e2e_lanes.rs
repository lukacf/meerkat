use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};

use tokio::process::Command;
use tokio::time::{Duration, timeout};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Lane {
    System,
    Live,
    Smoke,
}

#[derive(Clone, Copy)]
enum OutputPolicy {
    CargoTest,
    Pytest,
    NodeTest,
    ExitOnly,
}

#[derive(Clone, Copy)]
enum CommandSpec {
    CargoTest {
        package: &'static str,
        test_target: &'static str,
        test_name: &'static str,
        features: &'static [&'static str],
        all_features: bool,
    },
    Pytest {
        test_file: &'static str,
        test_name: &'static str,
    },
    NodeTest {
        test_file: &'static str,
        test_name: &'static str,
    },
    Raw {
        argv: &'static [&'static str],
        output_policy: OutputPolicy,
    },
}

#[derive(Clone, Copy)]
struct CommandEntry {
    spec: &'static Spec,
    command: &'static [&'static str],
}

#[derive(Clone, Copy)]
#[allow(dead_code)]
struct Spec {
    id: Option<u16>,
    lane: Lane,
    title: &'static str,
    timeout_secs: u64,
    required_env: &'static [&'static [&'static str]],
    required_bins: &'static [&'static str],
    cwd: &'static str,
    env: &'static [(&'static str, &'static str)],
    cargo_bin_env: &'static [&'static str],
    pre_commands: &'static [&'static [&'static str]],
    command: CommandSpec,
}

struct CompletedCommand {
    output: String,
}

#[derive(Clone, Debug)]
enum PreCommandState {
    Running,
    Done,
    Failed(String),
}

pub async fn run_catalog_scenario(id: u16) -> Result<(), String> {
    let Some(spec) = scenario_spec(id) else {
        return Err(format!("unknown live/smoke scenario id {id}"));
    };
    run_spec(spec).await
}

pub async fn run_named_suite(name: &str) -> Result<(), String> {
    let Some(spec) = suite_spec(name) else {
        return Err(format!("unknown lane suite '{name}'"));
    };
    run_spec(spec).await
}

fn run_label(spec: &Spec) -> String {
    match spec.id {
        Some(id) => format!("{id:02} {}", spec.title),
        None => spec.title.to_string(),
    }
}

async fn run_spec(spec: &'static Spec) -> Result<(), String> {
    if let Some(message) = prereq_failure(spec) {
        if strict_prereqs_enabled() {
            return Err(format!("{}: {message}", run_label(spec)));
        }
        eprintln!("skipping {}: {message}", run_label(spec));
        return Ok(());
    }

    let cwd = workspace_root().join(spec.cwd);
    let env_overrides = scenario_env(spec)?;
    let (pre_commands, command, output_policy) = build_commands(spec);

    for pre_command in pre_commands {
        run_pre_command(
            CommandEntry {
                spec,
                command: pre_command,
            },
            &cwd,
            &env_overrides,
        )
        .await?;
    }

    let completed = run_command(command, &cwd, &env_overrides, spec.timeout_secs).await?;
    if let Some(problem) = analyze_success_output(output_policy, &completed.output) {
        return Err(format!(
            "{}: {} ({problem})",
            run_label(spec),
            command_display(command)
        ));
    }

    Ok(())
}

#[allow(clippy::await_holding_lock)] // Lock is explicitly dropped before await
async fn run_pre_command(
    entry: CommandEntry,
    cwd: &Path,
    env_overrides: &[(String, String)],
) -> Result<(), String> {
    let env_signature = env_overrides
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("\n");
    let key = format!(
        "{}::{}::{}",
        cwd.display(),
        command_display(entry.command),
        env_signature
    );
    loop {
        let mut completed = completed_pre_commands().lock().unwrap();
        match completed.get(&key) {
            Some(PreCommandState::Done) => return Ok(()),
            Some(PreCommandState::Failed(error)) => return Err(error.clone()),
            Some(PreCommandState::Running) => {
                drop(completed);
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            None => {
                completed.insert(key.clone(), PreCommandState::Running);
                break;
            }
        }
    }

    let result = run_command(entry.command, cwd, env_overrides, entry.spec.timeout_secs)
        .await
        .map(|_| ());
    let mut completed = completed_pre_commands().lock().unwrap();
    match &result {
        Ok(()) => {
            completed.insert(key, PreCommandState::Done);
        }
        Err(error) => {
            completed.insert(key, PreCommandState::Failed(error.clone()));
        }
    }
    result
}

async fn run_command(
    command: &[&str],
    cwd: &Path,
    env_overrides: &[(String, String)],
    timeout_secs: u64,
) -> Result<CompletedCommand, String> {
    let argv = normalize_command(command);
    let mut child = Command::new(&argv[0]);
    child
        .args(&argv[1..])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in env_overrides {
        child.env(key, value);
    }

    let output = timeout(Duration::from_secs(timeout_secs), child.output())
        .await
        .map_err(|_| {
            format!(
                "command timed out after {timeout_secs}s: {}",
                argv.join(" ")
            )
        })?
        .map_err(|error| format!("failed to run {}: {error}", argv.join(" ")))?;

    let combined = combine_output(&output.stdout, &output.stderr);
    if !output.status.success() {
        return Err(format!(
            "command failed (exit {:?}): {}\n{}",
            output.status.code(),
            argv.join(" "),
            combined
        ));
    }

    Ok(CompletedCommand { output: combined })
}

fn build_commands(
    spec: &'static Spec,
) -> (
    Vec<&'static [&'static str]>,
    &'static [&'static str],
    OutputPolicy,
) {
    let output_policy = match spec.command {
        CommandSpec::CargoTest { .. } => OutputPolicy::CargoTest,
        CommandSpec::Pytest { .. } => OutputPolicy::Pytest,
        CommandSpec::NodeTest { .. } => OutputPolicy::NodeTest,
        CommandSpec::Raw { output_policy, .. } => output_policy,
    };

    let command = match spec.command {
        CommandSpec::CargoTest {
            package,
            test_target,
            test_name,
            features,
            all_features,
        } => {
            let mut command = vec!["cargo", "test", "-p", package];
            if all_features {
                command.push("--all-features");
            }
            if !features.is_empty() {
                command.push("--features");
                command.push(Box::leak(features.join(",").into_boxed_str()));
            }
            command.extend([
                "--test",
                test_target,
                test_name,
                "--",
                "--ignored",
                "--nocapture",
            ]);
            Box::leak(command.into_boxed_slice())
        }
        CommandSpec::Pytest {
            test_file,
            test_name,
        } => Box::leak(
            vec![
                compatible_python_bin().unwrap_or("python3"),
                "-m",
                "pytest",
                "-v",
                Box::leak(format!("{test_file}::{test_name}").into_boxed_str()),
            ]
            .into_boxed_slice(),
        ),
        CommandSpec::NodeTest {
            test_file,
            test_name,
        } => Box::leak(
            vec![
                "node",
                "--test",
                "--test-name-pattern",
                test_name,
                test_file,
            ]
            .into_boxed_slice(),
        ),
        CommandSpec::Raw { argv, .. } => argv,
    };

    (spec.pre_commands.to_vec(), command, output_policy)
}

fn normalize_command(command: &[&str]) -> Vec<String> {
    let repo_root = workspace_root().display().to_string();
    let cargo_target_dir = cargo_target_dir()
        .unwrap_or_else(|_| workspace_root().join("target"))
        .display()
        .to_string();
    let mut argv = command
        .iter()
        .map(|part| {
            part.replace("{repo_root}", &repo_root)
                .replace("{cargo_target_dir}", &cargo_target_dir)
        })
        .collect::<Vec<_>>();
    if argv.first().map(|part| part.as_str()) == Some("cargo") {
        argv[0] = repo_cargo().display().to_string();
    }
    argv
}

fn combine_output(stdout: &[u8], stderr: &[u8]) -> String {
    let stdout = String::from_utf8_lossy(stdout);
    let stderr = String::from_utf8_lossy(stderr);
    match (stdout.trim(), stderr.trim()) {
        ("", "") => String::new(),
        (_, "") => stdout.into_owned(),
        ("", _) => stderr.into_owned(),
        _ => format!("{stdout}\n--- stderr ---\n{stderr}"),
    }
}

fn analyze_success_output(output_policy: OutputPolicy, output: &str) -> Option<&'static str> {
    let lower = output.to_ascii_lowercase();
    match output_policy {
        OutputPolicy::CargoTest => {
            if lower.contains("running 0 tests") {
                return Some("cargo executed 0 tests");
            }
            let Some(summary) = lower.lines().find(|line| line.contains("test result:")) else {
                return Some("cargo output did not include a test summary");
            };
            if summary.contains(" 0 passed;") {
                return Some("cargo reported 0 passed tests");
            }
            None
        }
        OutputPolicy::Pytest => {
            if lower.contains("no tests ran") {
                return Some("pytest executed 0 tests");
            }
            if lower.contains(" skipped") || lower.contains(" deselected") {
                return Some("pytest skipped or deselected the requested scenario");
            }
            if !lower.contains(" passed") {
                return Some("pytest output did not report a passed test");
            }
            None
        }
        OutputPolicy::NodeTest => {
            let has_zero_tests =
                lower.contains("# tests 0") || lower.lines().any(|line| line.contains("tests 0"));
            if has_zero_tests {
                return Some("node executed 0 tests");
            }
            let has_pass = lower.lines().any(|line| {
                line.starts_with("# pass ")
                    || line.starts_with("ℹ pass ")
                    || line.contains(" pass 1")
                    || line.contains(" pass 2")
                    || line.contains(" pass 3")
                    || line.contains(" pass 4")
                    || line.contains(" pass 5")
            });
            if !has_pass {
                return Some("node output did not report a passed test");
            }
            None
        }
        OutputPolicy::ExitOnly => None,
    }
}

fn prereq_failure(spec: &Spec) -> Option<String> {
    let mut failures = Vec::new();
    for group in spec.required_env {
        if group
            .iter()
            .all(|name| std::env::var(name).map_or(true, |value| value.is_empty()))
        {
            let message = if group.len() == 1 {
                format!("missing env {}", group[0])
            } else {
                format!("missing one of {}", group.join(" / "))
            };
            failures.push(message);
        }
    }
    for binary in spec.required_bins {
        if *binary == "python3" {
            if compatible_python_bin().is_none() {
                failures.push("missing compatible python >=3.10".to_string());
            }
            continue;
        }
        if *binary == "cargo" {
            if !repo_cargo().exists() {
                failures.push(format!(
                    "missing repo cargo wrapper {}",
                    repo_cargo().display()
                ));
            }
            continue;
        }
        if std::process::Command::new(binary)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_err()
        {
            failures.push(format!("missing binary {binary}"));
        }
    }

    if failures.is_empty() {
        None
    } else {
        Some(failures.join("; "))
    }
}

fn scenario_env(spec: &Spec) -> Result<Vec<(String, String)>, String> {
    let mut env = BTreeMap::new();
    let cargo_target_dir = cargo_target_dir()?;
    for (key, value) in spec.env {
        env.insert(
            (*key).to_string(),
            expand_template(value, &cargo_target_dir),
        );
    }
    for binary in spec.cargo_bin_env {
        env.insert(
            format!("CARGO_BIN_EXE_{binary}"),
            cargo_target_dir
                .join("debug")
                .join(platform_binary_name(binary))
                .display()
                .to_string(),
        );
    }
    if matches!(spec.command, CommandSpec::Pytest { .. })
        && let Some(python) = compatible_python_bin()
    {
        env.insert("MEERKAT_PYTHON_BIN".to_string(), python.to_string());
    }
    Ok(env.into_iter().collect())
}

fn expand_template(value: &str, cargo_target_dir: &Path) -> String {
    value
        .replace("{repo_root}", &workspace_root().display().to_string())
        .replace(
            "{cargo_target_dir}",
            &cargo_target_dir.display().to_string(),
        )
}

fn platform_binary_name(name: &str) -> String {
    if cfg!(windows) && !name.ends_with(".exe") {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

fn workspace_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

fn repo_cargo() -> &'static Path {
    static REPO_CARGO: OnceLock<PathBuf> = OnceLock::new();
    REPO_CARGO
        .get_or_init(|| workspace_root().join("scripts").join("repo-cargo"))
        .as_path()
}

fn compatible_python_bin() -> Option<&'static str> {
    static PYTHON_BIN: OnceLock<Option<String>> = OnceLock::new();
    PYTHON_BIN
        .get_or_init(|| {
            for candidate in ["python3.11", "python3.10", "python3", "python"] {
                let output = std::process::Command::new(candidate)
                    .args([
                        "-c",
                        "import sys; print(f'{sys.version_info[0]}.{sys.version_info[1]}')",
                    ])
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .output();
                let Ok(output) = output else {
                    continue;
                };
                if !output.status.success() {
                    continue;
                }
                let version = String::from_utf8_lossy(&output.stdout);
                let mut parts = version.trim().split('.');
                let major = parts.next().and_then(|part| part.parse::<u32>().ok());
                let minor = parts.next().and_then(|part| part.parse::<u32>().ok());
                if matches!((major, minor), (Some(major), Some(minor)) if (major, minor) >= (3, 10))
                {
                    return Some(candidate.to_string());
                }
            }
            None
        })
        .as_deref()
}

fn cargo_target_dir() -> Result<PathBuf, String> {
    if let Ok(value) = std::env::var("CARGO_TARGET_DIR") {
        return Ok(PathBuf::from(value));
    }

    static TARGET_DIR: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    TARGET_DIR
        .get_or_init(|| {
            let output = std::process::Command::new(repo_cargo())
                .arg("--print-env")
                .output()
                .map_err(|error| format!("failed to query repo-cargo env: {error}"))?;
            if !output.status.success() {
                return Err(format!(
                    "repo-cargo --print-env failed: {}",
                    combine_output(&output.stdout, &output.stderr)
                ));
            }
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                if let Some(value) = line.strip_prefix("CARGO_TARGET_DIR=") {
                    return Ok(PathBuf::from(value));
                }
            }
            Err("repo-cargo --print-env did not report CARGO_TARGET_DIR".to_string())
        })
        .clone()
}

fn strict_prereqs_enabled() -> bool {
    matches!(
        std::env::var("MEERKAT_STRICT_E2E_PREREQS").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes") | Ok("YES")
    )
}

fn completed_pre_commands() -> &'static Mutex<HashMap<String, PreCommandState>> {
    static PRE_COMMANDS: OnceLock<Mutex<HashMap<String, PreCommandState>>> = OnceLock::new();
    PRE_COMMANDS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn command_display(command: &[&str]) -> String {
    normalize_command(command).join(" ")
}

fn scenario_spec(id: u16) -> Option<&'static Spec> {
    match id {
        15 => Some(&Spec {
            id: Some(15),
            lane: Lane::Live,
            title: "RPC full lifecycle and recall",
            timeout_secs: 900,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-rpc",
                test_target: "live_smoke_rpc",
                test_name: "e2e_scenario_15_full_rpc_conversation_flow",
                features: &[],
                all_features: false,
            },
        }),
        16 => Some(&Spec {
            id: Some(16),
            lane: Lane::Smoke,
            title: "RPC kitchen sink",
            timeout_secs: 1200,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-rpc",
                test_target: "live_smoke_rpc",
                test_name: "e2e_scenario_16_kitchen_sink",
                features: &[],
                all_features: false,
            },
        }),
        17 => Some(&Spec {
            id: Some(17),
            lane: Lane::Live,
            title: "RPC multi-turn event streaming",
            timeout_secs: 1200,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-rpc",
                test_target: "live_smoke_rpc",
                test_name: "e2e_scenario_17_multi_turn_event_streaming",
                features: &[],
                all_features: false,
            },
        }),
        18 => Some(&Spec {
            id: Some(18),
            lane: Lane::Live,
            title: "RPC config, capabilities, and errors",
            timeout_secs: 900,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-rpc",
                test_target: "live_rpc_regression",
                test_name: "e2e_scenario_18_config_capabilities_errors",
                features: &[],
                all_features: false,
            },
        }),
        19 => Some(&Spec {
            id: Some(19),
            lane: Lane::Live,
            title: "RPC context injection recall",
            timeout_secs: 900,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-rpc",
                test_target: "live_rpc_regression",
                test_name: "e2e_scenario_19_inject_context_recall",
                features: &[],
                all_features: false,
            },
        }),
        20 => Some(&Spec {
            id: Some(20),
            lane: Lane::Live,
            title: "RPC dedicated event stream roundtrip",
            timeout_secs: 900,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-rpc",
                test_target: "live_rpc_regression",
                test_name: "e2e_scenario_20_streaming_events",
                features: &[],
                all_features: false,
            },
        }),
        21 => Some(&Spec {
            id: Some(21),
            lane: Lane::Live,
            title: "REST runtime accept input roundtrip",
            timeout_secs: 900,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-rest",
                test_target: "live_rest_matrix",
                test_name: "e2e_scenario_21_rest_runtime_accept_input_roundtrip",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        22 => Some(&Spec {
            id: Some(22),
            lane: Lane::Live,
            title: "REST runtime reset and retire semantics",
            timeout_secs: 900,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-rest",
                test_target: "live_rest_matrix",
                test_name: "e2e_scenario_22_rest_runtime_reset_and_retire_drain_staged_inputs",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        23 => Some(&Spec {
            id: Some(23),
            lane: Lane::Live,
            title: "REST SSE follow-up event stream",
            timeout_secs: 900,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-rest",
                test_target: "live_rest_matrix",
                test_name: "e2e_scenario_23_rest_sse_events_follow_continue_turn",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        24 => Some(&Spec {
            id: Some(24),
            lane: Lane::Live,
            title: "REST config, capabilities, health, and skills",
            timeout_secs: 600,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-rest",
                test_target: "live_rest_matrix",
                test_name: "e2e_scenario_24_rest_config_capabilities_health_and_skills",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        25 => Some(&Spec {
            id: Some(25),
            lane: Lane::Live,
            title: "REST reload and resume on same realm root",
            timeout_secs: 900,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-rest",
                test_target: "live_rest_matrix",
                test_name: "e2e_scenario_25_rest_reload_and_resume_on_same_realm_root",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        26 => Some(&Spec {
            id: Some(26),
            lane: Lane::Live,
            title: "CLI run and resume persistence",
            timeout_secs: 900,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "rkat",
                test_target: "live_smoke_cli",
                test_name: "e2e_scenario_26_cli_run_resume_persistence",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        27 => Some(&Spec {
            id: Some(27),
            lane: Lane::Smoke,
            title: "CLI shell and structured output",
            timeout_secs: 1200,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "rkat",
                test_target: "live_smoke_cli",
                test_name: "e2e_scenario_27_cli_shell_and_structured_output",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        28 => Some(&Spec {
            id: Some(28),
            lane: Lane::Smoke,
            title: "CLI signed mobpack deploy",
            timeout_secs: 1200,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "rkat",
                test_target: "cli_mobpack_live_smoke",
                test_name: "e2e_scenario_28_cli_mobpack_deploy_signed_strict_live",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        29 => Some(&Spec {
            id: Some(29),
            lane: Lane::Live,
            title: "CLI mob member turn probe",
            timeout_secs: 1500,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "rkat",
                test_target: "cli_mobpack_live_smoke",
                test_name: "e2e_scenario_29_cli_mob_rpc_member_turn_probe",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        30 => Some(&Spec {
            id: Some(30),
            lane: Lane::Smoke,
            title: "CLI mob flow probe",
            timeout_secs: 1800,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "rkat",
                test_target: "cli_mobpack_live_smoke",
                test_name: "e2e_scenario_30_cli_mob_rpc_flow_probe",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        31 => Some(&Spec {
            id: Some(31),
            lane: Lane::Live,
            title: "MCP stdio run and resume lifecycle",
            timeout_secs: 1200,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &["rkat-mcp"],
            pre_commands: &[&[
                "cargo",
                "build",
                "-p",
                "meerkat-mcp-server",
                "--bin",
                "rkat-mcp",
            ]],
            command: CommandSpec::CargoTest {
                package: "meerkat-mcp-server",
                test_target: "live_mcp_matrix",
                test_name: "e2e_scenario_31_mcp_stdio_run_resume_lifecycle",
                features: &[],
                all_features: false,
            },
        }),
        32 => Some(&Spec {
            id: Some(32),
            lane: Lane::Live,
            title: "MCP stdio config, capabilities, and skills",
            timeout_secs: 900,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &["rkat-mcp"],
            pre_commands: &[&[
                "cargo",
                "build",
                "-p",
                "meerkat-mcp-server",
                "--bin",
                "rkat-mcp",
            ]],
            command: CommandSpec::CargoTest {
                package: "meerkat-mcp-server",
                test_target: "live_mcp_matrix",
                test_name: "e2e_scenario_32_mcp_stdio_config_capabilities_and_skills",
                features: &[],
                all_features: false,
            },
        }),
        33 => Some(&Spec {
            id: Some(33),
            lane: Lane::Live,
            title: "MCP stdio event stream roundtrip",
            timeout_secs: 1200,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &["rkat-mcp"],
            pre_commands: &[&[
                "cargo",
                "build",
                "-p",
                "meerkat-mcp-server",
                "--bin",
                "rkat-mcp",
            ]],
            command: CommandSpec::CargoTest {
                package: "meerkat-mcp-server",
                test_target: "live_mcp_matrix",
                test_name: "e2e_scenario_33_mcp_stdio_event_stream_read_roundtrip",
                features: &[],
                all_features: false,
            },
        }),
        34 => Some(&Spec {
            id: Some(34),
            lane: Lane::Live,
            title: "MCP streamable HTTP run and resume lifecycle",
            timeout_secs: 1200,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-mcp-server",
                test_target: "live_mcp_matrix",
                test_name: "e2e_scenario_34_mcp_streamable_http_run_resume_lifecycle",
                features: &[],
                all_features: false,
            },
        }),
        35 => Some(&Spec {
            id: Some(35),
            lane: Lane::Live,
            title: "MCP streamable HTTP config, capabilities, and skills",
            timeout_secs: 900,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-mcp-server",
                test_target: "live_mcp_matrix",
                test_name: "e2e_scenario_35_mcp_streamable_http_config_capabilities_and_skills",
                features: &[],
                all_features: false,
            },
        }),
        36 => Some(&Spec {
            id: Some(36),
            lane: Lane::Live,
            title: "MCP streamable HTTP event stream and archive",
            timeout_secs: 1200,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-mcp-server",
                test_target: "live_mcp_matrix",
                test_name: "e2e_scenario_36_mcp_streamable_http_event_stream_and_archive_roundtrip",
                features: &[],
                all_features: false,
            },
        }),
        37 => Some(&Spec {
            id: Some(37),
            lane: Lane::Live,
            title: "Python SDK full lifecycle and capabilities",
            timeout_secs: 1200,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["python3", "cargo"],
            cwd: "sdks/python",
            env: &[("MEERKAT_BIN_PATH", "{cargo_target_dir}/debug/rkat-rpc")],
            cargo_bin_env: &[],
            pre_commands: &[
                &[
                    "/bin/sh",
                    "-c",
                    "${MEERKAT_PYTHON_BIN:-python3} -c 'import pytest, pytest_asyncio' >/dev/null 2>&1 || ${MEERKAT_PYTHON_BIN:-python3} -m pip install -e \".[dev]\"",
                ],
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rpc",
                    "--bin",
                    "rkat-rpc",
                    "--features",
                    "mob",
                ],
            ],
            command: CommandSpec::Pytest {
                test_file: "tests/test_e2e_smoke.py",
                test_name: "test_smoke_scenario_37_full_lifecycle_and_capabilities",
            },
        }),
        38 => Some(&Spec {
            id: Some(38),
            lane: Lane::Live,
            title: "Python SDK context injection and streaming",
            timeout_secs: 1200,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["python3", "cargo"],
            cwd: "sdks/python",
            env: &[("MEERKAT_BIN_PATH", "{cargo_target_dir}/debug/rkat-rpc")],
            cargo_bin_env: &[],
            pre_commands: &[
                &[
                    "/bin/sh",
                    "-c",
                    "${MEERKAT_PYTHON_BIN:-python3} -c 'import pytest, pytest_asyncio' >/dev/null 2>&1 || ${MEERKAT_PYTHON_BIN:-python3} -m pip install -e \".[dev]\"",
                ],
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rpc",
                    "--bin",
                    "rkat-rpc",
                    "--features",
                    "mob",
                ],
            ],
            command: CommandSpec::Pytest {
                test_file: "tests/test_e2e_smoke.py",
                test_name: "test_smoke_scenario_38_inject_context_and_streaming",
            },
        }),
        39 => Some(&Spec {
            id: Some(39),
            lane: Lane::Live,
            title: "Python SDK persistent reconnect and runtime accept",
            timeout_secs: 1500,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["python3", "cargo"],
            cwd: "sdks/python",
            env: &[("MEERKAT_BIN_PATH", "{cargo_target_dir}/debug/rkat-rpc")],
            cargo_bin_env: &[],
            pre_commands: &[
                &[
                    "/bin/sh",
                    "-c",
                    "${MEERKAT_PYTHON_BIN:-python3} -c 'import pytest, pytest_asyncio' >/dev/null 2>&1 || ${MEERKAT_PYTHON_BIN:-python3} -m pip install -e \".[dev]\"",
                ],
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rpc",
                    "--bin",
                    "rkat-rpc",
                    "--features",
                    "mob",
                ],
            ],
            command: CommandSpec::Pytest {
                test_file: "tests/test_e2e_smoke.py",
                test_name: "test_smoke_scenario_39_persistent_reconnect_and_runtime_accept",
            },
        }),
        40 => Some(&Spec {
            id: Some(40),
            lane: Lane::Smoke,
            title: "Python SDK mixed-provider swarm probe",
            timeout_secs: 1800,
            required_env: &[
                &["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"],
                &["RKAT_OPENAI_API_KEY", "OPENAI_API_KEY"],
            ],
            required_bins: &["python3", "cargo"],
            cwd: "sdks/python",
            env: &[("MEERKAT_BIN_PATH", "{cargo_target_dir}/debug/rkat-rpc")],
            cargo_bin_env: &[],
            pre_commands: &[
                &[
                    "/bin/sh",
                    "-c",
                    "${MEERKAT_PYTHON_BIN:-python3} -c 'import pytest, pytest_asyncio' >/dev/null 2>&1 || ${MEERKAT_PYTHON_BIN:-python3} -m pip install -e \".[dev]\"",
                ],
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rpc",
                    "--bin",
                    "rkat-rpc",
                    "--features",
                    "mob",
                ],
            ],
            command: CommandSpec::Pytest {
                test_file: "tests/test_e2e_smoke.py",
                test_name: "test_smoke_scenario_40_mixed_provider_swarm_probe",
            },
        }),
        41 => Some(&Spec {
            id: Some(41),
            lane: Lane::Live,
            title: "TypeScript SDK full lifecycle and recall",
            timeout_secs: 1500,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["node", "npm", "cargo"],
            cwd: "sdks/typescript",
            env: &[("MEERKAT_BIN_PATH", "{cargo_target_dir}/debug/rkat-rpc")],
            cargo_bin_env: &[],
            pre_commands: &[
                &["/bin/sh", "-c", "test -d node_modules || npm install"],
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rpc",
                    "--bin",
                    "rkat-rpc",
                    "--features",
                    "mob",
                ],
                &["npm", "run", "build"],
            ],
            command: CommandSpec::NodeTest {
                test_file: "tests/e2e_smoke.test.mjs",
                test_name: "Scenario 41",
            },
        }),
        42 => Some(&Spec {
            id: Some(42),
            lane: Lane::Live,
            title: "TypeScript SDK deferred context and streaming",
            timeout_secs: 1500,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["node", "npm", "cargo"],
            cwd: "sdks/typescript",
            env: &[("MEERKAT_BIN_PATH", "{cargo_target_dir}/debug/rkat-rpc")],
            cargo_bin_env: &[],
            pre_commands: &[
                &["/bin/sh", "-c", "test -d node_modules || npm install"],
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rpc",
                    "--bin",
                    "rkat-rpc",
                    "--features",
                    "mob",
                ],
                &["npm", "run", "build"],
            ],
            command: CommandSpec::NodeTest {
                test_file: "tests/e2e_smoke.test.mjs",
                test_name: "Scenario 42",
            },
        }),
        43 => Some(&Spec {
            id: Some(43),
            lane: Lane::Live,
            title: "TypeScript SDK persistent reconnect and resume",
            timeout_secs: 1500,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["node", "npm", "cargo"],
            cwd: "sdks/typescript",
            env: &[("MEERKAT_BIN_PATH", "{cargo_target_dir}/debug/rkat-rpc")],
            cargo_bin_env: &[],
            pre_commands: &[
                &["/bin/sh", "-c", "test -d node_modules || npm install"],
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rpc",
                    "--bin",
                    "rkat-rpc",
                    "--features",
                    "mob",
                ],
                &["npm", "run", "build"],
            ],
            command: CommandSpec::NodeTest {
                test_file: "tests/e2e_smoke.test.mjs",
                test_name: "Scenario 43",
            },
        }),
        44 => Some(&Spec {
            id: Some(44),
            lane: Lane::Smoke,
            title: "TypeScript SDK mixed-provider swarm probe",
            timeout_secs: 1800,
            required_env: &[
                &["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"],
                &["RKAT_OPENAI_API_KEY", "OPENAI_API_KEY"],
            ],
            required_bins: &["node", "npm", "cargo"],
            cwd: "sdks/typescript",
            env: &[("MEERKAT_BIN_PATH", "{cargo_target_dir}/debug/rkat-rpc")],
            cargo_bin_env: &[],
            pre_commands: &[
                &["/bin/sh", "-c", "test -d node_modules || npm install"],
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rpc",
                    "--bin",
                    "rkat-rpc",
                    "--features",
                    "mob",
                ],
                &["npm", "run", "build"],
            ],
            command: CommandSpec::NodeTest {
                test_file: "tests/e2e_smoke.test.mjs",
                test_name: "Scenario 44",
            },
        }),
        45 => Some(&Spec {
            id: Some(45),
            lane: Lane::Live,
            title: "Browser raw session lifecycle",
            timeout_secs: 1800,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["node", "npm"],
            cwd: "tests/live_smoke/browser",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[
                &["/bin/sh", "-c", "test -d node_modules || npm ci"],
                &[
                    "/bin/sh",
                    "-c",
                    "test -d ../../../sdks/web/node_modules || npm --prefix ../../../sdks/web install",
                ],
                &["npm", "--prefix", "../../../sdks/web", "run", "build"],
                &["npx", "playwright", "install", "chromium"],
            ],
            command: CommandSpec::Raw {
                argv: &[
                    "npm",
                    "run",
                    "smoke",
                    "--",
                    "--scenario",
                    "BROWSER-RAW-SESSION-001",
                ],
                output_policy: OutputPolicy::ExitOnly,
            },
        }),
        46 => Some(&Spec {
            id: Some(46),
            lane: Lane::Live,
            title: "Browser raw session recall",
            timeout_secs: 1800,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["node", "npm"],
            cwd: "tests/live_smoke/browser",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[
                &["/bin/sh", "-c", "test -d node_modules || npm ci"],
                &[
                    "/bin/sh",
                    "-c",
                    "test -d ../../../sdks/web/node_modules || npm --prefix ../../../sdks/web install",
                ],
                &["npm", "--prefix", "../../../sdks/web", "run", "build"],
                &["npx", "playwright", "install", "chromium"],
            ],
            command: CommandSpec::Raw {
                argv: &[
                    "npm",
                    "run",
                    "smoke",
                    "--",
                    "--scenario",
                    "BROWSER-RAW-RECALL-002",
                ],
                output_policy: OutputPolicy::ExitOnly,
            },
        }),
        47 => Some(&Spec {
            id: Some(47),
            lane: Lane::Smoke,
            title: "Browser mobpack session flow",
            timeout_secs: 1800,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["node", "npm"],
            cwd: "tests/live_smoke/browser",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[
                &["/bin/sh", "-c", "test -d node_modules || npm ci"],
                &[
                    "/bin/sh",
                    "-c",
                    "test -d ../../../sdks/web/node_modules || npm --prefix ../../../sdks/web install",
                ],
                &["npm", "--prefix", "../../../sdks/web", "run", "build"],
                &["npx", "playwright", "install", "chromium"],
            ],
            command: CommandSpec::Raw {
                argv: &[
                    "npm",
                    "run",
                    "smoke",
                    "--",
                    "--scenario",
                    "BROWSER-MOBPACK-SESSION-003",
                ],
                output_policy: OutputPolicy::ExitOnly,
            },
        }),
        48 => Some(&Spec {
            id: Some(48),
            lane: Lane::Smoke,
            title: "Browser raw mob lifecycle",
            timeout_secs: 1800,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["node", "npm"],
            cwd: "tests/live_smoke/browser",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[
                &["/bin/sh", "-c", "test -d node_modules || npm ci"],
                &[
                    "/bin/sh",
                    "-c",
                    "test -d ../../../sdks/web/node_modules || npm --prefix ../../../sdks/web install",
                ],
                &["npm", "--prefix", "../../../sdks/web", "run", "build"],
                &["npx", "playwright", "install", "chromium"],
            ],
            command: CommandSpec::Raw {
                argv: &[
                    "npm",
                    "run",
                    "smoke",
                    "--",
                    "--scenario",
                    "BROWSER-RAW-MOB-004",
                ],
                output_policy: OutputPolicy::ExitOnly,
            },
        }),
        49 => Some(&Spec {
            id: Some(49),
            lane: Lane::Smoke,
            title: "Cross-surface RPC to REST shared-realm roundtrip",
            timeout_secs: 1800,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-integration-tests",
                test_target: "smoke_shared_realm",
                test_name: "e2e_scenario_49_rpc_rest_shared_realm_roundtrip",
                features: &[],
                all_features: false,
            },
        }),
        50 => Some(&Spec {
            id: Some(50),
            lane: Lane::Smoke,
            title: "Cross-surface REST to CLI shared-realm roundtrip",
            timeout_secs: 1800,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-integration-tests",
                test_target: "smoke_shared_realm",
                test_name: "e2e_scenario_50_rest_cli_shared_realm_roundtrip",
                features: &[],
                all_features: false,
            },
        }),
        51 => Some(&Spec {
            id: Some(51),
            lane: Lane::Smoke,
            title: "Cross-surface RPC to MCP shared-realm parity",
            timeout_secs: 1800,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-integration-tests",
                test_target: "smoke_shared_realm",
                test_name: "e2e_scenario_51_rpc_mcp_shared_realm_roundtrip",
                features: &[],
                all_features: false,
            },
        }),
        52 => Some(&Spec {
            id: Some(52),
            lane: Lane::Smoke,
            title: "Cross-surface CLI to RPC to CLI continuity",
            timeout_secs: 1800,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-integration-tests",
                test_target: "smoke_shared_realm",
                test_name: "e2e_scenario_52_cli_rpc_shared_realm_roundtrip",
                features: &[],
                all_features: false,
            },
        }),
        53 => Some(&Spec {
            id: Some(53),
            lane: Lane::Smoke,
            title: "Cross-surface CLI to REST to CLI continuity",
            timeout_secs: 1800,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-integration-tests",
                test_target: "smoke_shared_realm",
                test_name: "e2e_scenario_53_cli_rest_shared_realm_roundtrip",
                features: &[],
                all_features: false,
            },
        }),
        54 => Some(&Spec {
            id: Some(54),
            lane: Lane::Smoke,
            title: "Cross-surface shared-realm mob visibility",
            timeout_secs: 1800,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[
                (
                    "RKAT_TEST_BIN_RKAT",
                    "{cargo_target_dir}/e2e-bins/scenario-54/rkat",
                ),
                (
                    "RKAT_TEST_BIN_RKAT_RPC",
                    "{cargo_target_dir}/e2e-bins/scenario-54/rkat-rpc",
                ),
            ],
            cargo_bin_env: &[],
            pre_commands: &[
                &["cargo", "build", "-p", "rkat", "--bin", "rkat"],
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rpc",
                    "--bin",
                    "rkat-rpc",
                    "--features",
                    "mob",
                ],
                &["mkdir", "-p", "{cargo_target_dir}/e2e-bins/scenario-54"],
                &[
                    "cp",
                    "{cargo_target_dir}/debug/rkat",
                    "{cargo_target_dir}/e2e-bins/scenario-54/rkat",
                ],
                &[
                    "cp",
                    "{cargo_target_dir}/debug/rkat-rpc",
                    "{cargo_target_dir}/e2e-bins/scenario-54/rkat-rpc",
                ],
            ],
            command: CommandSpec::CargoTest {
                package: "meerkat-integration-tests",
                test_target: "smoke_shared_realm",
                test_name: "e2e_scenario_54_shared_realm_mob_sessions_visible_to_cli",
                features: &[],
                all_features: false,
            },
        }),
        55 => Some(&Spec {
            id: Some(55),
            lane: Lane::Smoke,
            title: "RPC+REST callback peer storm resume",
            timeout_secs: 1800,
            required_env: &[
                &["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"],
                &["RKAT_OPENAI_API_KEY", "OPENAI_API_KEY"],
            ],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[
                (
                    "RKAT_TEST_BIN_RKAT",
                    "{cargo_target_dir}/e2e-bins/scenario-55/rkat",
                ),
                (
                    "RKAT_TEST_BIN_RKAT_RPC",
                    "{cargo_target_dir}/e2e-bins/scenario-55/rkat-rpc",
                ),
                (
                    "RKAT_TEST_BIN_RKAT_REST",
                    "{cargo_target_dir}/e2e-bins/scenario-55/rkat-rest",
                ),
            ],
            cargo_bin_env: &[],
            pre_commands: &[
                &["cargo", "build", "-p", "rkat", "--bin", "rkat"],
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rpc",
                    "--bin",
                    "rkat-rpc",
                    "--features",
                    "mob",
                ],
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rest",
                    "--bin",
                    "rkat-rest",
                    "--features",
                    "mob",
                ],
                &["mkdir", "-p", "{cargo_target_dir}/e2e-bins/scenario-55"],
                &[
                    "cp",
                    "{cargo_target_dir}/debug/rkat",
                    "{cargo_target_dir}/e2e-bins/scenario-55/rkat",
                ],
                &[
                    "cp",
                    "{cargo_target_dir}/debug/rkat-rpc",
                    "{cargo_target_dir}/e2e-bins/scenario-55/rkat-rpc",
                ],
                &[
                    "cp",
                    "{cargo_target_dir}/debug/rkat-rest",
                    "{cargo_target_dir}/e2e-bins/scenario-55/rkat-rest",
                ],
            ],
            command: CommandSpec::CargoTest {
                package: "meerkat-integration-tests",
                test_target: "smoke_shared_realm",
                test_name: "e2e_scenario_55_rpc_rest_callback_peer_storm_resume",
                features: &[],
                all_features: false,
            },
        }),
        56 => Some(&Spec {
            id: Some(56),
            lane: Lane::System,
            title: "RPC+REST explicit mob registry restores",
            timeout_secs: 300,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[
                (
                    "RKAT_TEST_BIN_RKAT_RPC",
                    "{cargo_target_dir}/e2e-bins/scenario-56/rkat-rpc",
                ),
                (
                    "RKAT_TEST_BIN_RKAT_REST",
                    "{cargo_target_dir}/e2e-bins/scenario-56/rkat-rest",
                ),
            ],
            cargo_bin_env: &[],
            pre_commands: &[
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rpc",
                    "--bin",
                    "rkat-rpc",
                    "--features",
                    "mob",
                ],
                &[
                    "cargo",
                    "build",
                    "-p",
                    "meerkat-rest",
                    "--bin",
                    "rkat-rest",
                    "--features",
                    "mob",
                ],
                &["mkdir", "-p", "{cargo_target_dir}/e2e-bins/scenario-56"],
                &[
                    "cp",
                    "{cargo_target_dir}/debug/rkat-rpc",
                    "{cargo_target_dir}/e2e-bins/scenario-56/rkat-rpc",
                ],
                &[
                    "cp",
                    "{cargo_target_dir}/debug/rkat-rest",
                    "{cargo_target_dir}/e2e-bins/scenario-56/rkat-rest",
                ],
            ],
            command: CommandSpec::CargoTest {
                package: "meerkat-integration-tests",
                test_target: "smoke_shared_realm",
                test_name: "rpc_rest_explicit_mob_registry_restores_without_live_api",
                features: &[],
                all_features: false,
            },
        }),
        _ => None,
    }
}

fn suite_spec(name: &str) -> Option<&'static Spec> {
    match name {
        "cli-init-snapshot" => Some(&Spec {
            id: None,
            lane: Lane::System,
            title: "CLI init snapshot",
            timeout_secs: 300,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "rkat",
                test_target: "system_cli_init",
                test_name: "integration_real_rkat_init_snapshot",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        "cli-resume-tools" => Some(&Spec {
            id: None,
            lane: Lane::System,
            title: "CLI resume tools",
            timeout_secs: 300,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "rkat",
                test_target: "system_cli_resume",
                test_name: "integration_real_cli_resume_tools",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        "rest-resume-metadata" => Some(&Spec {
            id: None,
            lane: Lane::System,
            title: "REST resume metadata",
            timeout_secs: 300,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-rest",
                test_target: "system_rest_resume",
                test_name: "integration_real_rest_resume_metadata",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        "cli-capabilities-and-config" => Some(&Spec {
            id: None,
            lane: Lane::System,
            title: "CLI capabilities and config",
            timeout_secs: 300,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "rkat",
                test_target: "live_smoke_cli",
                test_name: "e2e_scenario_28_cli_capabilities_and_config",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        "cli-mobpack-pack-inspect-validate" => Some(&Spec {
            id: None,
            lane: Lane::System,
            title: "CLI mobpack pack/inspect/validate",
            timeout_secs: 300,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "rkat",
                test_target: "cli_mobpack_live_smoke",
                test_name: "e2e_smoke_mobpack_pack_inspect_validate",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        "cli-wasm-surface-gate" => Some(&Spec {
            id: None,
            lane: Lane::System,
            title: "CLI wasm surface gate",
            timeout_secs: 300,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "rkat",
                test_target: "cli_mobpack_live_smoke",
                test_name: "e2e_smoke_wasm_surface_gate",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        "cli-wasm-forbidden-capability" => Some(&Spec {
            id: None,
            lane: Lane::System,
            title: "CLI wasm forbidden capability rejection",
            timeout_secs: 300,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "rkat",
                test_target: "cli_mobpack_live_smoke",
                test_name: "e2e_smoke_wasm_forbidden_capability_rejected",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        "cli-mob-rpc-state-machine-probe" => Some(&Spec {
            id: None,
            lane: Lane::Live,
            title: "CLI mob RPC state machine probe",
            timeout_secs: 600,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "rkat",
                test_target: "cli_mobpack_live_smoke",
                test_name: "e2e_cli_mob_rpc_state_machine_probe",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        "sqlite-shared-realm-rpc-rest-rpc" => Some(&Spec {
            id: None,
            lane: Lane::System,
            title: "SQLite shared realm RPC -> REST -> RPC",
            timeout_secs: 600,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[
                (
                    "RKAT_TEST_BIN_RKAT_RPC",
                    "{cargo_target_dir}/e2e-bins/system-rpc-rest-rpc/rkat-rpc",
                ),
                (
                    "RKAT_TEST_BIN_RKAT_REST",
                    "{cargo_target_dir}/e2e-bins/system-rpc-rest-rpc/rkat-rest",
                ),
            ],
            cargo_bin_env: &[],
            pre_commands: &[
                &["cargo", "build", "-p", "meerkat-rpc", "--bin", "rkat-rpc"],
                &["cargo", "build", "-p", "meerkat-rest", "--bin", "rkat-rest"],
                &[
                    "mkdir",
                    "-p",
                    "{cargo_target_dir}/e2e-bins/system-rpc-rest-rpc",
                ],
                &[
                    "cp",
                    "{cargo_target_dir}/debug/rkat-rpc",
                    "{cargo_target_dir}/e2e-bins/system-rpc-rest-rpc/rkat-rpc",
                ],
                &[
                    "cp",
                    "{cargo_target_dir}/debug/rkat-rest",
                    "{cargo_target_dir}/e2e-bins/system-rpc-rest-rpc/rkat-rest",
                ],
            ],
            command: CommandSpec::CargoTest {
                package: "meerkat-integration-tests",
                test_target: "system_shared_realm",
                test_name: "rpc_rest_rpc_default_sqlite_shared_realm_roundtrip",
                features: &[],
                all_features: false,
            },
        }),
        "sqlite-shared-realm-cli-rpc-cli" => Some(&Spec {
            id: None,
            lane: Lane::System,
            title: "SQLite shared realm CLI -> RPC -> CLI",
            timeout_secs: 600,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[
                (
                    "RKAT_TEST_BIN_RKAT",
                    "{cargo_target_dir}/e2e-bins/system-cli-rpc-cli/rkat",
                ),
                (
                    "RKAT_TEST_BIN_RKAT_RPC",
                    "{cargo_target_dir}/e2e-bins/system-cli-rpc-cli/rkat-rpc",
                ),
            ],
            cargo_bin_env: &[],
            pre_commands: &[
                &["cargo", "build", "-p", "rkat", "--bin", "rkat"],
                &["cargo", "build", "-p", "meerkat-rpc", "--bin", "rkat-rpc"],
                &[
                    "mkdir",
                    "-p",
                    "{cargo_target_dir}/e2e-bins/system-cli-rpc-cli",
                ],
                &[
                    "cp",
                    "{cargo_target_dir}/debug/rkat",
                    "{cargo_target_dir}/e2e-bins/system-cli-rpc-cli/rkat",
                ],
                &[
                    "cp",
                    "{cargo_target_dir}/debug/rkat-rpc",
                    "{cargo_target_dir}/e2e-bins/system-cli-rpc-cli/rkat-rpc",
                ],
            ],
            command: CommandSpec::CargoTest {
                package: "meerkat-integration-tests",
                test_target: "system_shared_realm",
                test_name: "cli_rpc_cli_default_sqlite_shared_realm_roundtrip",
                features: &[],
                all_features: false,
            },
        }),
        "sqlite-shared-realm-cli-rest-cli" => Some(&Spec {
            id: None,
            lane: Lane::System,
            title: "SQLite shared realm CLI -> REST -> CLI",
            timeout_secs: 600,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[
                (
                    "RKAT_TEST_BIN_RKAT",
                    "{cargo_target_dir}/e2e-bins/system-cli-rest-cli/rkat",
                ),
                (
                    "RKAT_TEST_BIN_RKAT_REST",
                    "{cargo_target_dir}/e2e-bins/system-cli-rest-cli/rkat-rest",
                ),
            ],
            cargo_bin_env: &[],
            pre_commands: &[
                &["cargo", "build", "-p", "rkat", "--bin", "rkat"],
                &["cargo", "build", "-p", "meerkat-rest", "--bin", "rkat-rest"],
                &[
                    "mkdir",
                    "-p",
                    "{cargo_target_dir}/e2e-bins/system-cli-rest-cli",
                ],
                &[
                    "cp",
                    "{cargo_target_dir}/debug/rkat",
                    "{cargo_target_dir}/e2e-bins/system-cli-rest-cli/rkat",
                ],
                &[
                    "cp",
                    "{cargo_target_dir}/debug/rkat-rest",
                    "{cargo_target_dir}/e2e-bins/system-cli-rest-cli/rkat-rest",
                ],
            ],
            command: CommandSpec::CargoTest {
                package: "meerkat-integration-tests",
                test_target: "system_shared_realm",
                test_name: "cli_rest_cli_default_sqlite_shared_realm_roundtrip",
                features: &[],
                all_features: false,
            },
        }),
        "surface-rkat-help" => Some(&Spec {
            id: None,
            lane: Lane::System,
            title: "Surface smoke: rkat --help",
            timeout_secs: 300,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::Raw {
                argv: &[
                    "cargo",
                    "run",
                    "-p",
                    "rkat",
                    "--no-default-features",
                    "--features",
                    "session-store",
                    "--bin",
                    "rkat",
                    "--",
                    "--help",
                ],
                output_policy: OutputPolicy::ExitOnly,
            },
        }),
        "surface-rkat-tests" => Some(&Spec {
            id: None,
            lane: Lane::System,
            title: "Surface smoke: rkat no-default-features tests",
            timeout_secs: 600,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::Raw {
                argv: &[
                    "cargo",
                    "nextest",
                    "run",
                    "-p",
                    "rkat",
                    "--no-default-features",
                    "--features",
                    "session-store,mcp",
                    "--no-capture",
                ],
                output_policy: OutputPolicy::ExitOnly,
            },
        }),
        "surface-rkat-rpc-help" => Some(&Spec {
            id: None,
            lane: Lane::System,
            title: "Surface smoke: rkat-rpc --help",
            timeout_secs: 300,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::Raw {
                argv: &[
                    "cargo",
                    "run",
                    "-p",
                    "meerkat-rpc",
                    "--no-default-features",
                    "--bin",
                    "rkat-rpc",
                    "--",
                    "--help",
                ],
                output_policy: OutputPolicy::ExitOnly,
            },
        }),
        "surface-rkat-rest-help" => Some(&Spec {
            id: None,
            lane: Lane::System,
            title: "Surface smoke: rkat-rest --help",
            timeout_secs: 300,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::Raw {
                argv: &[
                    "cargo",
                    "run",
                    "-p",
                    "meerkat-rest",
                    "--no-default-features",
                    "--bin",
                    "rkat-rest",
                    "--",
                    "--help",
                ],
                output_policy: OutputPolicy::ExitOnly,
            },
        }),
        "surface-rkat-mcp-help" => Some(&Spec {
            id: None,
            lane: Lane::System,
            title: "Surface smoke: rkat-mcp --help",
            timeout_secs: 300,
            required_env: &[],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::Raw {
                argv: &[
                    "cargo",
                    "run",
                    "-p",
                    "meerkat-mcp-server",
                    "--no-default-features",
                    "--bin",
                    "rkat-mcp",
                    "--",
                    "--help",
                ],
                output_policy: OutputPolicy::ExitOnly,
            },
        }),
        "agent-mob-tools" => Some(&Spec {
            id: None,
            lane: Lane::Live,
            title: "Agent mob tools live suite",
            timeout_secs: 1800,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::Raw {
                argv: &[
                    "cargo",
                    "test",
                    "-p",
                    "meerkat-integration-tests",
                    "--features",
                    "integration-real-tests",
                    "--test",
                    "live_mob_tools",
                    "--",
                    "--ignored",
                    "--nocapture",
                ],
                output_policy: OutputPolicy::CargoTest,
            },
        }),
        "cli-structured-output" => Some(&Spec {
            id: None,
            lane: Lane::Live,
            title: "CLI structured output live regression",
            timeout_secs: 1200,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "rkat",
                test_target: "live_smoke_cli",
                test_name: "e2e_cli_structured_output",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        "rpc-mob-callback-tools" => Some(&Spec {
            id: None,
            lane: Lane::Smoke,
            title: "RPC mob deferred callback tools smoke",
            timeout_secs: 1500,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-rpc",
                test_target: "live_smoke_rpc",
                test_name: "e2e_scenario_21_mob_callback_tools",
                features: &[],
                all_features: false,
            },
        }),
        "rpc-transport-backpressure" => Some(&Spec {
            id: None,
            lane: Lane::Smoke,
            title: "RPC transport backpressure smoke",
            timeout_secs: 1500,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-rpc",
                test_target: "live_smoke_rpc",
                test_name: "e2e_scenario_22_transport_backpressure",
                features: &[],
                all_features: false,
            },
        }),
        "rpc-dynamic-tool-pickup" => Some(&Spec {
            id: None,
            lane: Lane::Smoke,
            title: "RPC deferred dynamic tool pickup smoke",
            timeout_secs: 1500,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-rpc",
                test_target: "live_smoke_rpc",
                test_name: "e2e_scenario_23_late_register_on_existing_member",
                features: &[],
                all_features: false,
            },
        }),
        "rpc-deferred-catalog-session" => Some(&Spec {
            id: None,
            lane: Lane::Smoke,
            title: "RPC deferred catalog direct-session smoke",
            timeout_secs: 1500,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-rpc",
                test_target: "live_smoke_rpc",
                test_name: "e2e_direct_session_deferred_callback_tool_flow",
                features: &[],
                all_features: false,
            },
        }),
        "cli-background-job-active-turn" => Some(&Spec {
            id: None,
            lane: Lane::Smoke,
            title: "CLI background job active-turn smoke",
            timeout_secs: 1500,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "rkat",
                test_target: "live_smoke_cli",
                test_name: "e2e_001_background_job_active_turn_completion",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        "cli-background-job-idle-keepalive" => Some(&Spec {
            id: None,
            lane: Lane::Smoke,
            title: "CLI background job idle keepalive smoke",
            timeout_secs: 1500,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "rkat",
                test_target: "live_smoke_cli",
                test_name: "e2e_002_background_job_idle_keepalive_completion",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        "mob-live-smoke" => Some(&Spec {
            id: None,
            lane: Lane::Smoke,
            title: "Mob partial resume collaborative joke smoke",
            timeout_secs: 1200,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-mob",
                test_target: "smoke_mob_resume",
                test_name: "e2e_smoke_mob_partial_resume_collaborative_joke",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        "mob-flow-runtime" => Some(&Spec {
            id: None,
            lane: Lane::Smoke,
            title: "Mob flow runtime smoke suite",
            timeout_secs: 2400,
            required_env: &[&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"]],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::Raw {
                argv: &[
                    "cargo",
                    "test",
                    "-p",
                    "meerkat-mob",
                    "--features",
                    "integration-real-tests",
                    "--test",
                    "smoke_mob_flow_runtime",
                    "--",
                    "--ignored",
                    "--test-threads=1",
                    "--nocapture",
                ],
                output_policy: OutputPolicy::CargoTest,
            },
        }),
        "mob-pictionary" => Some(&Spec {
            id: None,
            lane: Lane::Smoke,
            title: "Mob pictionary multimodal smoke",
            timeout_secs: 1800,
            required_env: &[
                &["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"],
                &["RKAT_OPENAI_API_KEY", "OPENAI_API_KEY"],
                &["RKAT_GEMINI_API_KEY", "GEMINI_API_KEY"],
            ],
            required_bins: &["cargo"],
            cwd: ".",
            env: &[],
            cargo_bin_env: &[],
            pre_commands: &[],
            command: CommandSpec::CargoTest {
                package: "meerkat-mob",
                test_target: "smoke_mob_pictionary",
                test_name: "e2e_pictionary_multimodal_comms_stress",
                features: &["integration-real-tests"],
                all_features: false,
            },
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{Lane, scenario_spec};

    #[test]
    fn numbered_catalog_covers_matrix_ids() {
        let ids = (15..=56)
            .filter(|id| scenario_spec(*id).is_some())
            .collect::<Vec<_>>();
        assert_eq!(ids, (15..=56).collect::<Vec<_>>());
        assert_eq!(ids.len(), 42);
    }

    #[test]
    fn live_and_smoke_counts_match_expected_split() {
        let live = (15..=56)
            .filter(|id| scenario_spec(*id).map(|spec| spec.lane) == Some(Lane::Live))
            .count();
        let smoke = (15..=56)
            .filter(|id| scenario_spec(*id).map(|spec| spec.lane) == Some(Lane::Smoke))
            .count();
        let system = (15..=56)
            .filter(|id| scenario_spec(*id).map(|spec| spec.lane) == Some(Lane::System))
            .count();
        assert_eq!(live, 26);
        assert_eq!(smoke, 15);
        assert_eq!(system, 1);
    }
}

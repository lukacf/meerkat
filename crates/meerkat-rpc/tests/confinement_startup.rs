//! OS process confinement on the rkat-rpc surface: the stock binary resolves
//! the composed `[confinement]` and its `--confinement*` flags at startup.
//!
//! The observables are the real binary's. The workspace preset without
//! `--context-root` refuses to start instead of confining nothing. A
//! requirement in the user document reaches the server's startup report even
//! without a `[realm.global]` section: a requirement no platform backend
//! enforces (descendant termination) is reported as unenforceable on every
//! OS, and the server still serves. The operator's `--confinement
//! trusted-host` replaces the documents, so nothing is reported.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::disallowed_methods
)]

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tempfile::TempDir;

const INITIALIZE_REQUEST: &str =
    "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n";

/// A requirement no platform backend enforces (descendant termination).
const UNENFORCEABLE_CONFINEMENT: &str = r#"[confinement.all]
mode = "custom"
working_directory = "/"
requirement = { baseline = "command_runtime_v1", read = { kind = "paths", paths = [] }, write = { kind = "paths", paths = [] }, deny_read = [], deny_write = [], network = { kind = "denied" }, unix_connect = [], require_descendant_termination = true }
"#;

/// The startup report of an installed requirement, and its warning for a
/// launch kind this host cannot confine.
const INSTALLED_REPORT: &str = "confinement: requested custom";
const UNENFORCEABLE_REPORT: &str = "cannot be enforced on this host";
/// The report's informational line naming the host's protected inputs.
const PROTECTED_INPUTS_REPORT: &str = "kept from every confined process: the realm state at";

/// Last-resort bound for a server that neither exits nor answers, as in
/// `realm_config_ingress.rs`: well above the observed startup latency under
/// a fully parallel crate run and below the CI unit profile's ceiling.
const EXIT_DEADLINE: Duration = Duration::from_secs(200);

/// The crate's own binary, from the runtime environment only: Cargo,
/// nextest (including an archived test run on another machine) and the
/// generated Bazel targets set it there. A compile-time path would be stale
/// in an archive. A test without it fails rather than passing without having
/// run.
fn rkat_rpc_binary() -> PathBuf {
    let advertised = std::env::var_os("CARGO_BIN_EXE_rkat-rpc")
        .map(PathBuf::from)
        .expect("CARGO_BIN_EXE_rkat-rpc is set at runtime by cargo, nextest and the generated Bazel targets");
    assert!(
        advertised.exists(),
        "the rkat-rpc binary {} does not exist",
        advertised.display()
    );
    advertised
}

struct Run {
    /// Whether the server answered `initialize`.
    answered: bool,
    status: ExitStatus,
    stderr: String,
}

/// Run `rkat-rpc` with `args` in `cwd`, hand it one `initialize` request,
/// and close its stdin once it answers, so a booted server shuts down on EOF.
/// A server that refuses to start exits without answering.
fn run_rkat_rpc(binary: &Path, cwd: &Path, args: &[&str]) -> Run {
    run_rkat_rpc_with_requests(binary, cwd, args, &[]).0
}

/// [`run_rkat_rpc`], handing the server `requests` (ids 2, 3, ...) once it
/// answered `initialize` and closing its stdin once it answered every one.
/// Returns each answered request's response by id.
fn run_rkat_rpc_with_requests(
    binary: &Path,
    cwd: &Path,
    args: &[&str],
    requests: &[(&str, serde_json::Value)],
) -> (Run, BTreeMap<u64, serde_json::Value>) {
    let mut command = Command::new(binary);
    command.current_dir(cwd).args(args);
    run_rpc_command(command, requests)
}

/// Run the server `command` starts as [`run_rkat_rpc_with_requests`] does.
fn run_rpc_command(
    mut command: Command,
    requests: &[(&str, serde_json::Value)],
) -> (Run, BTreeMap<u64, serde_json::Value>) {
    let mut child = command
        .env_remove("RUST_LOG")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("rkat-rpc should spawn");

    // A refusal may already have closed the pipe; that write failure is the
    // expected shape, not a test defect.
    let mut stdin = child.stdin.take();
    if let Some(pipe) = stdin.as_mut() {
        let _ = pipe.write_all(INITIALIZE_REQUEST.as_bytes());
        let _ = pipe.flush();
    }

    let mut stderr_pipe = child.stderr.take().expect("stderr pipe");
    let stderr_reader = std::thread::spawn(move || {
        let mut buffer = String::new();
        let _ = stderr_pipe.read_to_string(&mut buffer);
        buffer
    });
    let stdout_pipe = child.stdout.take().expect("stdout pipe");
    let (stdout_tx, stdout_rx) = mpsc::channel::<String>();
    let stdout_reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout_pipe).lines() {
            let Ok(line) = line else { break };
            if stdout_tx.send(line).is_err() {
                break;
            }
        }
    });

    let deadline = Instant::now() + EXIT_DEADLINE;
    let mut answered = false;
    let mut responses: BTreeMap<u64, serde_json::Value> = BTreeMap::new();
    let last_id = 1 + requests.len() as u64;
    let status = loop {
        while let Ok(line) = stdout_rx.try_recv() {
            let Ok(message) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            let Some(id) = message.get("id").and_then(serde_json::Value::as_u64) else {
                continue;
            };
            responses.insert(id, message);
            if id == 1 && !answered {
                answered = true;
                if let Some(pipe) = stdin.as_mut() {
                    for (offset, (method, params)) in requests.iter().enumerate() {
                        let request = serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": 2 + offset as u64,
                            "method": method,
                            "params": params,
                        });
                        let _ = pipe.write_all(format!("{request}\n").as_bytes());
                    }
                    let _ = pipe.flush();
                }
            }
            if answered && (1..=last_id).all(|id| responses.contains_key(&id)) {
                // EOF on stdin shuts the booted server down.
                drop(stdin.take());
            }
        }
        if let Some(status) = child.try_wait().expect("try_wait should succeed") {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let stderr = stderr_reader
                .join()
                .expect("stderr reader thread should finish");
            panic!(
                "rkat-rpc neither exited nor shut down after answering within \
                 {EXIT_DEADLINE:?} (answered: {answered}). stderr:\n{stderr}"
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let stderr = stderr_reader
        .join()
        .expect("stderr reader thread should finish");
    stdout_reader
        .join()
        .expect("stdout reader thread should finish");
    (
        Run {
            answered,
            status,
            stderr,
        },
        responses,
    )
}

/// The scope arguments of one isolated run: a memory realm under `temp`,
/// with `temp/user` as the user config root.
fn scope_args<'a>(realm: &'a str, state_root: &'a Path, user_root: &'a Path) -> Vec<&'a str> {
    vec![
        "--realm",
        realm,
        "--realm-backend",
        "memory",
        "--state-root",
        state_root.to_str().expect("utf-8 state root"),
        "--user-config-root",
        user_root.to_str().expect("utf-8 user root"),
    ]
}

#[test]
fn workspace_confinement_without_a_context_root_refuses_rkat_rpc_startup() {
    let binary = rkat_rpc_binary();
    let temp = TempDir::new().expect("temp dir");
    let state_root = temp.path().join("state");
    let user_root = temp.path().join("user");
    std::fs::create_dir_all(&user_root).expect("user root should initialize");

    let mut args = scope_args("rpc-confinement-unrooted", &state_root, &user_root);
    args.extend(["--confinement", "workspace"]);
    let run = run_rkat_rpc(&binary, temp.path(), &args);

    assert!(
        !run.answered && !run.status.success(),
        "rkat-rpc must refuse workspace confinement without a workspace root instead of \
         serving unconfined; exit={:?} stderr:\n{}",
        run.status,
        run.stderr
    );
    assert!(
        run.stderr.contains("--context-root"),
        "the refusal names its remedy; stderr:\n{}",
        run.stderr
    );
}

/// A start whose confined processes could change the realm state is refused
/// before the server serves, naming the input and its remedy.
#[test]
fn confinement_that_could_change_the_realm_state_refuses_rkat_rpc_startup() {
    let binary = rkat_rpc_binary();
    let temp = TempDir::new().expect("temp dir");
    let base = temp.path().canonicalize().expect("canonical temp");
    let project = base.join("project");
    let state = base.join("state");
    let user_root = base.join("user");
    std::fs::create_dir_all(&project).expect("project root should initialize");
    std::fs::create_dir_all(&state).expect("state root should initialize");
    std::fs::create_dir_all(user_root.join(".rkat")).expect("user root should initialize");
    std::fs::write(
        user_root.join(".rkat").join("config.toml"),
        format!(
            "[confinement.all]\nmode = \"custom\"\nworking_directory = \"{state}\"\nrequirement = {{ baseline = \"command_runtime_v1\", read = {{ kind = \"paths\", paths = [{{ kind = \"subtree\", path = \"{state}\" }}] }}, write = {{ kind = \"paths\", paths = [{{ kind = \"subtree\", path = \"{state}\" }}] }}, deny_read = [], deny_write = [], network = {{ kind = \"denied\" }}, unix_connect = [], require_descendant_termination = false }}\n",
            state = state.display()
        ),
    )
    .expect("user document should be written");

    let mut args = scope_args("rpc-confinement-state-writer", &state, &user_root);
    args.extend([
        "--context-root",
        project.to_str().expect("utf-8 project root"),
    ]);
    let run = run_rkat_rpc(&binary, &project, &args);

    assert!(
        !run.answered && !run.status.success(),
        "rkat-rpc must refuse a start whose confined processes could change its state; \
         exit={:?} stderr:\n{}",
        run.status,
        run.stderr
    );
    assert!(
        run.stderr.contains("the realm state") && run.stderr.contains("--state-root"),
        "the refusal names the input and its remedy; stderr:\n{}",
        run.stderr
    );
}

#[test]
fn the_user_documents_confinement_reaches_the_rkat_rpc_startup_report() {
    let binary = rkat_rpc_binary();
    let temp = TempDir::new().expect("temp dir");
    let project = temp.path().join("project");
    let state_root = temp.path().join("state");
    let user_root = temp.path().join("user");
    std::fs::create_dir_all(&project).expect("project root should initialize");
    std::fs::create_dir_all(user_root.join(".rkat")).expect("user root should initialize");
    std::fs::write(
        user_root.join(".rkat").join("config.toml"),
        UNENFORCEABLE_CONFINEMENT,
    )
    .expect("user document should be written");
    let project_arg = project.to_str().expect("utf-8 project root");

    let mut args = scope_args("rpc-confinement-reported", &state_root, &user_root);
    args.extend(["--context-root", project_arg]);
    let configured = run_rkat_rpc(&binary, &project, &args);
    assert!(
        configured.answered,
        "a confined server still serves; exit={:?} stderr:\n{}",
        configured.status, configured.stderr
    );
    assert!(
        configured.stderr.contains(INSTALLED_REPORT)
            && configured.stderr.contains(UNENFORCEABLE_REPORT),
        "the user document's requirement reaches the startup report; stderr:\n{}",
        configured.stderr
    );
    assert!(
        configured.stderr.contains(PROTECTED_INPUTS_REPORT),
        "the report's informational lines reach the default log too; stderr:\n{}",
        configured.stderr
    );

    let mut args = scope_args("rpc-confinement-flagged", &state_root, &user_root);
    args.extend([
        "--context-root",
        project_arg,
        "--confinement",
        "trusted-host",
    ]);
    let flagged = run_rkat_rpc(&binary, &project, &args);
    assert!(
        flagged.answered,
        "the trusted host serves; exit={:?} stderr:\n{}",
        flagged.status, flagged.stderr
    );
    assert!(
        !flagged.stderr.contains(INSTALLED_REPORT),
        "the operator's flag replaces the documents; stderr:\n{}",
        flagged.stderr
    );
}

/// A `config/patch` or `config/set` cannot cut the served realm from the
/// parent whose `[confinement]` it composes: the real server refuses the
/// write with the confinement discriminator and the realm document keeps its
/// bytes. A write that keeps the edge commits.
#[test]
fn rkat_rpc_config_writes_cannot_drop_inherited_confinement() {
    let binary = rkat_rpc_binary();
    let temp = TempDir::new().expect("temp dir");
    let state_root = temp.path().join("state");
    let user_root = temp.path().join("user");
    std::fs::create_dir_all(&user_root).expect("user root should initialize");
    let team_text = "[realm.team]\nparent = \"org\"\n";
    let org_text = format!("[realm.org]\n\n{UNENFORCEABLE_CONFINEMENT}");
    for (realm, text) in [("org", org_text.as_str()), ("team", team_text)] {
        let document = state_root.join(realm).join("config.toml");
        std::fs::create_dir_all(document.parent().expect("realm dir"))
            .expect("realm dir should initialize");
        std::fs::write(&document, text).expect("realm document should be written");
    }

    let args = scope_args("team", &state_root, &user_root);
    let (run, responses) = run_rkat_rpc_with_requests(
        &binary,
        temp.path(),
        &args,
        &[
            (
                "config/patch",
                serde_json::json!({ "patch": { "realm": { "team": { "parent": null } } } }),
            ),
            (
                "config/patch",
                serde_json::json!({ "patch": { "realm": { "team": null } } }),
            ),
            (
                "config/patch",
                serde_json::json!({ "patch": { "max_tokens": 2048 } }),
            ),
        ],
    );
    assert!(
        run.answered,
        "the team realm serves; exit={:?} stderr:\n{}",
        run.status, run.stderr
    );
    for id in [2, 3] {
        let error = &responses[&id]["error"];
        assert_eq!(
            error["data"],
            serde_json::json!({ "type": "confinement_administration_required" }),
            "request {id}: {}",
            responses[&id]
        );
    }
    assert!(
        responses[&4].get("error").is_none(),
        "a patch that keeps the edge commits: {}",
        responses[&4]
    );
    let committed = std::fs::read_to_string(state_root.join("team").join("config.toml"))
        .expect("team document");
    assert!(committed.contains("parent = \"org\""), "{committed}");
    assert!(committed.contains("max_tokens = 2048"), "{committed}");
}

/// A `config/patch` cannot change where a credential command's token goes:
/// the real server refuses a write that moves the command's binding to
/// another backend, or changes the backend it names, with the confinement
/// discriminator, and the realm document keeps its routes. A patch that
/// keeps them commits.
#[test]
fn rkat_rpc_config_writes_cannot_redirect_a_credential_command_token() {
    let binary = rkat_rpc_binary();
    let temp = TempDir::new().expect("temp dir");
    let state_root = temp.path().join("state");
    let user_root = temp.path().join("user");
    std::fs::create_dir_all(&user_root).expect("user root should initialize");
    let document = state_root.join("team").join("config.toml");
    std::fs::create_dir_all(document.parent().expect("realm dir"))
        .expect("realm dir should initialize");
    std::fs::write(
        &document,
        r#"[realm.team]

[realm.team.backend.vendor]
provider = "openai"
backend_kind = "openai_api"

[realm.team.backend.elsewhere]
provider = "openai"
backend_kind = "openai_api"
base_url = "https://elsewhere.example/v1"

[realm.team.auth.helper]
provider = "openai"
auth_method = "api_key"
source = { kind = "command", program = "/usr/bin/helper" }

[realm.team.binding.main]
backend_profile = "vendor"
auth_profile = "helper"
"#,
    )
    .expect("realm document should be written");

    let args = scope_args("team", &state_root, &user_root);
    let (run, responses) = run_rkat_rpc_with_requests(
        &binary,
        temp.path(),
        &args,
        &[
            (
                "config/patch",
                serde_json::json!({ "patch": { "realm": { "team": { "binding": { "main": { "backend_profile": "elsewhere" } } } } } }),
            ),
            (
                "config/patch",
                serde_json::json!({ "patch": { "realm": { "team": { "backend": { "vendor": { "base_url": "https://attacker.example/v1" } } } } } }),
            ),
            (
                "config/patch",
                serde_json::json!({ "patch": { "max_tokens": 2048 } }),
            ),
        ],
    );
    assert!(
        run.answered,
        "the team realm serves; exit={:?} stderr:\n{}",
        run.status, run.stderr
    );
    for id in [2, 3] {
        let error = &responses[&id]["error"];
        assert_eq!(
            error["data"],
            serde_json::json!({ "type": "confinement_administration_required" }),
            "request {id}: {}",
            responses[&id]
        );
    }
    assert!(
        responses[&4].get("error").is_none(),
        "a patch that keeps the routes commits: {}",
        responses[&4]
    );
    let committed = std::fs::read_to_string(&document).expect("team document");
    assert!(
        committed.contains("backend_profile = \"vendor\"") && !committed.contains("attacker"),
        "{committed}"
    );
    assert!(committed.contains("max_tokens = 2048"), "{committed}");
}

/// Without a confinement flag that names a relative path the server never
/// reads its working directory, so it serves from one that was removed.
#[cfg(unix)]
#[test]
fn rkat_rpc_starts_without_a_working_directory_when_no_flag_needs_it() {
    let binary = rkat_rpc_binary();
    let temp = TempDir::new().expect("temp dir");
    let state_root = temp.path().join("state");
    let user_root = temp.path().join("user");
    let removed = temp.path().join("removed");
    for dir in [&user_root, &removed] {
        std::fs::create_dir_all(dir).expect("directory should initialize");
    }

    // The shell removes its working directory, then becomes the server.
    let mut command = Command::new("/bin/sh");
    command
        .arg("-c")
        .arg("cd \"$1\" && rmdir \"$1\" && shift && exec \"$@\"")
        .arg("sh")
        .arg(&removed)
        .arg(&binary)
        .args(scope_args("rpc-removed-cwd", &state_root, &user_root));
    let (run, _) = run_rpc_command(command, &[]);
    assert!(
        run.answered,
        "a server without confinement flags serves from a removed working directory; \
         exit={:?} stderr:\n{}",
        run.status, run.stderr
    );
}

/// A start whose confined processes could change the host's credential store
/// (the tokens and keys its providers, MCP servers and connectors use) is
/// refused before the server serves, naming the store.
#[test]
fn confinement_that_could_change_the_credential_store_refuses_rkat_rpc_startup() {
    let binary = rkat_rpc_binary();
    let temp = TempDir::new().expect("temp dir");
    let base = temp.path().canonicalize().expect("canonical temp");
    // The user's config directory, and with it the default credential store,
    // lives under the only write grant.
    let home = base.join("home");
    let state = base.join("state");
    let user_root = base.join("user");
    for dir in [home.join(".config"), state.clone(), user_root.join(".rkat")] {
        std::fs::create_dir_all(&dir).expect("directory should initialize");
    }
    std::fs::write(
        user_root.join(".rkat").join("config.toml"),
        format!(
            "[confinement.all]\nmode = \"custom\"\nworking_directory = \"{home}\"\nrequirement = {{ baseline = \"command_runtime_v1\", read = {{ kind = \"paths\", paths = [{{ kind = \"subtree\", path = \"{home}\" }}] }}, write = {{ kind = \"paths\", paths = [{{ kind = \"subtree\", path = \"{home}\" }}] }}, deny_read = [], deny_write = [], network = {{ kind = \"denied\" }}, unix_connect = [], require_descendant_termination = false }}\n",
            home = home.display()
        ),
    )
    .expect("user document should be written");

    let mut command = Command::new(&binary);
    command
        .current_dir(&base)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .args(scope_args("rpc-credential-writer", &state, &user_root));
    let (run, _) = run_rpc_command(command, &[]);

    assert!(
        !run.answered && !run.status.success(),
        "rkat-rpc must refuse a start whose confined processes could change its credential \
         store; exit={:?} stderr:\n{}",
        run.status,
        run.stderr
    );
    assert!(
        run.stderr.contains("the credential store"),
        "the refusal names the input; stderr:\n{}",
        run.stderr
    );
}

/// A `config/patch` cannot cut the served realm from the parent whose
/// `[governance]` it composes: the real server refuses the write with the
/// governance discriminator and the realm document keeps its bytes. A write
/// that keeps the edge commits.
#[test]
fn rkat_rpc_config_writes_cannot_drop_inherited_governance() {
    let binary = rkat_rpc_binary();
    let temp = TempDir::new().expect("temp dir");
    let state_root = temp.path().join("state");
    let user_root = temp.path().join("user");
    std::fs::create_dir_all(&user_root).expect("user root should initialize");
    let team_text = "[realm.team]\nparent = \"org\"\n";
    let org_text = "[realm.org]\n\n[[governance.review.rules]]\ntarget = { kind = \"mcp_server\", server = \"mail\" }\ntier = \"r2\"\n";
    for (realm, text) in [("org", org_text), ("team", team_text)] {
        let document = state_root.join(realm).join("config.toml");
        std::fs::create_dir_all(document.parent().expect("realm dir"))
            .expect("realm dir should initialize");
        std::fs::write(&document, text).expect("realm document should be written");
    }

    let args = scope_args("team", &state_root, &user_root);
    let (run, responses) = run_rkat_rpc_with_requests(
        &binary,
        temp.path(),
        &args,
        &[
            (
                "config/patch",
                serde_json::json!({ "patch": { "realm": { "team": { "parent": null } } } }),
            ),
            (
                "config/patch",
                serde_json::json!({ "patch": { "realm": { "team": null } } }),
            ),
            (
                "config/patch",
                serde_json::json!({ "patch": { "max_tokens": 2048 } }),
            ),
        ],
    );
    assert!(
        run.answered,
        "the team realm serves; exit={:?} stderr:\n{}",
        run.status, run.stderr
    );
    for id in [2, 3] {
        let error = &responses[&id]["error"];
        assert_eq!(
            error["data"],
            serde_json::json!({ "type": "governance_administration_required" }),
            "request {id}: {}",
            responses[&id]
        );
    }
    assert!(
        responses[&4].get("error").is_none(),
        "a patch that keeps the edge commits: {}",
        responses[&4]
    );
    let committed = std::fs::read_to_string(state_root.join("team").join("config.toml"))
        .expect("team document");
    assert!(committed.contains("parent = \"org\""), "{committed}");
    assert!(committed.contains("max_tokens = 2048"), "{committed}");
}

/// An MCP auth request addresses only servers the host installed: on a
/// server whose confined processes can write the project `mcp.toml`, a
/// server only that file defines is refused as not configured, naming the
/// refusal, so a planted entry cannot point a login at a server of its
/// choosing.
#[test]
fn rkat_rpc_mcp_auth_refuses_a_server_only_an_untrusted_mcp_toml_defines() {
    let binary = rkat_rpc_binary();
    let temp = TempDir::new().expect("temp dir");
    let base = temp.path().canonicalize().expect("canonical temp");
    let project = base.join("project");
    let home = base.join("home");
    let state = base.join("state");
    let user_root = base.join("user");
    for dir in [
        project.join(".rkat"),
        home.join(".config"),
        state.clone(),
        user_root.join(".rkat"),
    ] {
        std::fs::create_dir_all(&dir).expect("directory should initialize");
    }
    std::fs::write(
        project.join(".rkat").join("mcp.toml"),
        "[[servers]]\nname = \"planted\"\nurl = \"https://planted.example/mcp\"\n",
    )
    .expect("project mcp.toml should be written");
    std::fs::write(
        user_root.join(".rkat").join("config.toml"),
        format!(
            "[confinement.all]\nmode = \"custom\"\nworking_directory = \"{dir}\"\nrequirement = {{ baseline = \"command_runtime_v1\", read = {{ kind = \"paths\", paths = [{{ kind = \"subtree\", path = \"{dir}\" }}] }}, write = {{ kind = \"paths\", paths = [{{ kind = \"subtree\", path = \"{dir}\" }}] }}, deny_read = [], deny_write = [], network = {{ kind = \"denied\" }}, unix_connect = [], require_descendant_termination = false }}\n",
            dir = project.display()
        ),
    )
    .expect("user document should be written");

    let mut command = Command::new(&binary);
    command
        .current_dir(&base)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .args(scope_args("rpc-untrusted-mcp", &state, &user_root))
        .args([
            "--context-root",
            project.to_str().expect("utf-8 project root"),
        ]);
    let (run, responses) = run_rpc_command(
        command,
        &[(
            "auth/status/get",
            serde_json::json!({
                "mcp": {
                    "server_name": "planted",
                    "server_url": "https://planted.example/mcp",
                }
            }),
        )],
    );
    assert!(
        run.answered,
        "the server serves; exit={:?} stderr:\n{}",
        run.status, run.stderr
    );
    let error = &responses[&2]["error"];
    assert_eq!(
        error["data"]["reason"], "mcp_server_not_configured",
        "{}",
        responses[&2]
    );
    let refusal =
        meerkat_core::confinement::ConfinementRefusal::UntrustedConfigurationSource.to_string();
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|text| text.contains(&refusal)),
        "{}",
        responses[&2]
    );
}

/// The governed JSONL route resolves the realm's default binding to build
/// its controller client at startup. A credential command it names runs as
/// the host, outside confinement, so a host that confines its launches
/// refuses one whose program a confined process could change: the governed
/// controller factory installs the same confinement as the connection
/// factory, so startup is refused instead of running the command.
#[cfg(feature = "local-authorization")]
#[tokio::test(flavor = "multi_thread")]
async fn a_governed_route_credential_command_under_confinement_is_refused() {
    use std::os::unix::fs::PermissionsExt;

    let binary = rkat_rpc_binary();
    let temp = TempDir::new().expect("temp dir");
    let base = temp.path().canonicalize().expect("canonical temp");
    let state = base.join("state");
    let user_root = base.join("user");
    let home = base.join("home");
    let writable = base.join("writable");
    for dir in [&state, &user_root, &home, &writable, &home.join(".config")] {
        std::fs::create_dir_all(dir).expect("directory should initialize");
    }
    // The realm's default-binding credential is a command whose program
    // lives under the only write grant.
    let helper = writable.join("helper.sh");
    std::fs::write(&helper, "#!/bin/sh\nprintf 'sk-governed-token'\n").expect("helper");
    std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).expect("chmod");

    const REALM: &str = "governed-cmd";
    drop(
        meerkat::open_realm_persistence_with_provider(
            &meerkat::DiskStorageProvider,
            &state,
            REALM,
            Some(meerkat_store::RealmBackend::Sqlite),
            None,
            None,
        )
        .await
        .expect("materialize the realm"),
    );
    let realm = meerkat_core::connection::RealmId::parse(REALM).expect("realm id");
    meerkat::governed_realm::stock::govern_stock_realm(
        &state,
        REALM,
        &meerkat::governed_realm::stock::StockPrincipals::for_local_operator(&realm)
            .expect("local operator principals"),
    )
    .await
    .expect("govern the realm");

    let document = state.join(REALM).join("config.toml");
    std::fs::write(
        &document,
        format!(
            "[confinement.all]\nmode = \"custom\"\nworking_directory = \"{home}\"\nrequirement = {{ baseline = \"command_runtime_v1\", read = {{ kind = \"paths\", paths = [{{ kind = \"subtree\", path = \"{writable}\" }}] }}, write = {{ kind = \"paths\", paths = [{{ kind = \"subtree\", path = \"{writable}\" }}] }}, deny_read = [], deny_write = [], network = {{ kind = \"denied\" }}, unix_connect = [], require_descendant_termination = false }}\n\n[realm.{REALM}]\ndefault_binding = \"ctl\"\n\n[realm.{REALM}.backend.api]\nprovider = \"openai\"\nbackend_kind = \"openai_api\"\n\n[realm.{REALM}.auth.cmd]\nprovider = \"openai\"\nauth_method = \"api_key\"\nsource = {{ kind = \"command\", program = \"{helper}\" }}\n\n[realm.{REALM}.binding.ctl]\nbackend_profile = \"api\"\nauth_profile = \"cmd\"\ndefault_model = \"gpt-5.4\"\nprovider_default = true\n",
            home = home.display(),
            writable = writable.display(),
            helper = helper.display(),
        ),
    )
    .expect("realm document should be written");

    let mut command = Command::new(&binary);
    command
        .current_dir(&base)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .args([
            "--realm",
            REALM,
            "--realm-backend",
            "sqlite",
            "--state-root",
            state.to_str().expect("utf-8 state root"),
            "--user-config-root",
            user_root.to_str().expect("utf-8 user root"),
        ]);
    let (run, _) = run_rpc_command(command, &[]);

    assert!(
        !run.answered && !run.status.success(),
        "the governed controller refuses a credential command its confinement flags; exit={:?} \
         stderr:\n{}",
        run.status,
        run.stderr
    );
    assert!(
        run.stderr.contains("credential command") || run.stderr.to_lowercase().contains("confined"),
        "the refusal names the credential command; stderr:\n{}",
        run.stderr
    );
}

/// The governed JSONL route also builds its configured reviewer through the
/// host's confinement. With the controller's own credential trusted (an
/// inline secret), a reviewer binding whose credential is a command under a
/// write grant is refused, so startup fails: the reviewer factory carries
/// the same confinement as the controller and connection factories.
#[cfg(feature = "local-authorization")]
#[tokio::test(flavor = "multi_thread")]
async fn a_governed_reviewer_credential_command_under_confinement_is_refused() {
    use std::os::unix::fs::PermissionsExt;

    let binary = rkat_rpc_binary();
    let temp = TempDir::new().expect("temp dir");
    let base = temp.path().canonicalize().expect("canonical temp");
    let state = base.join("state");
    let user_root = base.join("user");
    let home = base.join("home");
    let writable = base.join("writable");
    for dir in [&state, &user_root, &home, &writable, &home.join(".config")] {
        std::fs::create_dir_all(dir).expect("directory should initialize");
    }
    let helper = writable.join("helper.sh");
    std::fs::write(&helper, "#!/bin/sh\nprintf 'sk-reviewer-token'\n").expect("helper");
    std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).expect("chmod");

    const REALM: &str = "governed-reviewer";
    drop(
        meerkat::open_realm_persistence_with_provider(
            &meerkat::DiskStorageProvider,
            &state,
            REALM,
            Some(meerkat_store::RealmBackend::Sqlite),
            None,
            None,
        )
        .await
        .expect("materialize the realm"),
    );
    let realm = meerkat_core::connection::RealmId::parse(REALM).expect("realm id");
    meerkat::governed_realm::stock::govern_stock_realm(
        &state,
        REALM,
        &meerkat::governed_realm::stock::StockPrincipals::for_local_operator(&realm)
            .expect("local operator principals"),
    )
    .await
    .expect("govern the realm");

    // The controller's own credential is a trusted inline secret, so startup
    // reaches the reviewer; only the reviewer's binding is a command under
    // the write grant.
    let document = state.join(REALM).join("config.toml");
    std::fs::write(
        &document,
        format!(
            "[confinement.all]\nmode = \"custom\"\nworking_directory = \"{home}\"\nrequirement = {{ baseline = \"command_runtime_v1\", read = {{ kind = \"paths\", paths = [{{ kind = \"subtree\", path = \"{writable}\" }}] }}, write = {{ kind = \"paths\", paths = [{{ kind = \"subtree\", path = \"{writable}\" }}] }}, deny_read = [], deny_write = [], network = {{ kind = \"denied\" }}, unix_connect = [], require_descendant_termination = false }}\n\n[[governance.review.rules]]\ntarget = {{ kind = \"mcp_server\", server = \"mail\" }}\ntier = \"r2\"\n\n[governance.review.reviewer]\nmax_output_tokens = 64\nidentity = {{ model = \"gpt-5.4\", provider = \"openai\", auth_binding = {{ realm = \"{REALM}\", binding = \"rev\" }} }}\n\n[realm.{REALM}]\ndefault_binding = \"ctl\"\n\n[realm.{REALM}.backend.api]\nprovider = \"openai\"\nbackend_kind = \"openai_api\"\n\n[realm.{REALM}.auth.inline]\nprovider = \"openai\"\nauth_method = \"api_key\"\nsource = {{ kind = \"inline_secret\", secret = \"sk-controller-inline\" }}\n\n[realm.{REALM}.auth.cmd]\nprovider = \"openai\"\nauth_method = \"api_key\"\nsource = {{ kind = \"command\", program = \"{helper}\" }}\n\n[realm.{REALM}.binding.ctl]\nbackend_profile = \"api\"\nauth_profile = \"inline\"\ndefault_model = \"gpt-5.4\"\nprovider_default = true\n\n[realm.{REALM}.binding.rev]\nbackend_profile = \"api\"\nauth_profile = \"cmd\"\ndefault_model = \"gpt-5.4\"\n",
            home = home.display(),
            writable = writable.display(),
            helper = helper.display(),
        ),
    )
    .expect("realm document should be written");

    let mut command = Command::new(&binary);
    command
        .current_dir(&base)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .args([
            "--realm",
            REALM,
            "--realm-backend",
            "sqlite",
            "--state-root",
            state.to_str().expect("utf-8 state root"),
            "--user-config-root",
            user_root.to_str().expect("utf-8 user root"),
        ]);
    let (run, _) = run_rpc_command(command, &[]);

    assert!(
        !run.answered && !run.status.success(),
        "the governed reviewer refuses a credential command its confinement flags; exit={:?} \
         stderr:\n{}",
        run.status,
        run.stderr
    );
    assert!(
        run.stderr.contains("credential command") || run.stderr.to_lowercase().contains("confined"),
        "the refusal names the credential command; stderr:\n{}",
        run.stderr
    );
}

// ---- The in-process file tools and the server's own documents ----

/// A scripted OpenAI-compatible chat-completions server on loopback: each
/// new conversation is answered with one `apply_patch` call for the next
/// queued patch, and the request carrying a tool result with text. Returns
/// its port and the tool results it was sent.
fn scripted_file_tool_model(
    patches: Vec<String>,
) -> (u16, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    use std::sync::{Arc, Mutex};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind scripted model");
    let port = listener
        .local_addr()
        .expect("scripted model address")
        .port();
    let queue = Arc::new(Mutex::new(patches));
    let results = Arc::new(Mutex::new(Vec::new()));
    let served_results = Arc::clone(&results);
    std::thread::spawn(move || {
        for socket in listener.incoming().flatten() {
            let queue = Arc::clone(&queue);
            let results = Arc::clone(&served_results);
            std::thread::spawn(move || {
                let mut socket = socket;
                let mut buffer = Vec::new();
                let mut chunk = [0u8; 8192];
                let header_end = loop {
                    if let Some(position) =
                        buffer.windows(4).position(|window| window == b"\r\n\r\n")
                    {
                        break position + 4;
                    }
                    match socket.read(&mut chunk) {
                        Ok(0) | Err(_) => return,
                        Ok(read) => buffer.extend_from_slice(&chunk[..read]),
                    }
                };
                let headers = String::from_utf8_lossy(&buffer[..header_end]).to_ascii_lowercase();
                let length = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .and_then(|value| value.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                while buffer.len() < header_end + length {
                    match socket.read(&mut chunk) {
                        Ok(0) | Err(_) => return,
                        Ok(read) => buffer.extend_from_slice(&chunk[..read]),
                    }
                }
                let request: serde_json::Value =
                    serde_json::from_slice(&buffer[header_end..header_end + length])
                        .unwrap_or_default();
                let last = request["messages"]
                    .as_array()
                    .and_then(|messages| messages.last())
                    .cloned()
                    .unwrap_or_default();
                let chunk_of = |value: serde_json::Value| format!("data: {value}\n\n");
                let mut body = String::new();
                let patch = if last["role"].as_str() == Some("tool") {
                    results.lock().unwrap().push(match &last["content"] {
                        serde_json::Value::String(text) => text.clone(),
                        other => other.to_string(),
                    });
                    None
                } else {
                    queue.lock().unwrap().pop()
                };
                match patch {
                    Some(patch) => {
                        let arguments = serde_json::json!({ "patch": patch }).to_string();
                        body.push_str(&chunk_of(serde_json::json!({
                            "choices": [{
                                "delta": { "tool_calls": [{
                                    "index": 0,
                                    "id": "call_1",
                                    "type": "function",
                                    "function": { "name": "apply_patch", "arguments": arguments }
                                }] },
                                "finish_reason": null
                            }]
                        })));
                        body.push_str(&chunk_of(serde_json::json!({
                            "choices": [{ "delta": {}, "finish_reason": "tool_calls" }]
                        })));
                    }
                    None => {
                        body.push_str(&chunk_of(serde_json::json!({
                            "choices": [{ "delta": { "content": "done" }, "finish_reason": null }]
                        })));
                        body.push_str(&chunk_of(serde_json::json!({
                            "choices": [{ "delta": {}, "finish_reason": "stop" }]
                        })));
                    }
                }
                body.push_str(&chunk_of(serde_json::json!({
                    "choices": [],
                    "usage": { "prompt_tokens": 10, "completion_tokens": 2 }
                })));
                body.push_str("data: [DONE]\n\n");
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes());
                let _ = socket.shutdown(std::net::Shutdown::Both);
            });
        }
    });
    (port, results)
}

/// The stock server declares its own inputs to its agents' in-process file
/// tools. Started without `--context-root`, the tools are rooted in the
/// realm directory: the realm's config document is refused there, and so is
/// the skills directory the server reads from that root (`.rkat/skills`),
/// here a link to a directory beside it. An ordinary file is still written.
#[cfg(unix)]
#[test]
fn rkat_rpc_file_tools_never_write_the_servers_own_documents() {
    const REALM: &str = "file-tools";
    const MODEL: &str = "scripted-file-tools";
    let binary = rkat_rpc_binary();
    let temp = TempDir::new().expect("temp dir");
    let base = temp.path().canonicalize().expect("canonical temp dir");
    let home = base.join("home");
    let state = base.join("realms");
    let user_root = base.join("user");
    let realm_dir = state.join(REALM);
    for dir in [
        &home,
        &user_root,
        &realm_dir.join(".rkat"),
        &realm_dir.join("shared-skills"),
    ] {
        std::fs::create_dir_all(dir).expect("fixture dir");
    }
    std::os::unix::fs::symlink("../shared-skills", realm_dir.join(".rkat/skills"))
        .expect("link the skills directory");

    let add = |path: &str| {
        format!("*** Begin Patch\n*** Add File: {path}\n+written by the model\n*** End Patch")
    };
    let (port, tool_results) = scripted_file_tool_model(vec![
        "*** Begin Patch\n*** Update File: config.toml\n@@\n+# appended by the model\n*** End Patch"
            .to_string(),
        add("shared-skills/planted/SKILL.md"),
        add("out.txt"),
    ]);
    let document = format!(
        r#"[self_hosted.servers.scripted]
base_url = "http://127.0.0.1:{port}/v1"

[self_hosted.models."{MODEL}"]
server = "scripted"
remote_model = "{MODEL}"

[realm."{REALM}"]
default_binding = "scripted"

[realm."{REALM}".backend.scripted]
provider = "self_hosted"
backend_kind = "self_hosted"
server = "scripted"

[realm."{REALM}".auth.scripted]
provider = "self_hosted"
auth_method = "none"
source = {{ kind = "platform_default" }}

[realm."{REALM}".binding.scripted]
backend_profile = "scripted"
auth_profile = "scripted"
"#
    );
    let realm_document = realm_dir.join("config.toml");
    std::fs::write(&realm_document, &document).expect("realm document");

    let mut command = Command::new(&binary);
    for name in [
        "RKAT_ANTHROPIC_API_KEY",
        "ANTHROPIC_API_KEY",
        "RKAT_OPENAI_API_KEY",
        "OPENAI_API_KEY",
        "RKAT_GEMINI_API_KEY",
        "GEMINI_API_KEY",
        "RKAT_TEST_CLIENT",
    ] {
        command.env_remove(name);
    }
    command
        .current_dir(&base)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".data"))
        .args([
            "--realm",
            REALM,
            "--state-root",
            state.to_str().expect("utf-8 state root"),
            "--user-config-root",
            user_root.to_str().expect("utf-8 user root"),
        ]);
    let create = serde_json::json!({
        "prompt": "Apply the patch.",
        "model": MODEL,
        "provider": "self_hosted",
        "enable_shell": false,
    });
    let (run, responses) = run_rpc_command(
        command,
        &[
            ("session/create", create.clone()),
            ("session/create", create.clone()),
            ("session/create", create),
        ],
    );
    assert!(
        run.answered && run.status.success(),
        "rkat-rpc serves and shuts down; exit={:?} stderr:\n{}",
        run.status,
        run.stderr
    );
    for id in 2..=4 {
        assert!(
            responses[&id].get("result").is_some(),
            "session/create {id} ran its turn: {}",
            responses[&id]
        );
    }
    let tool_results = tool_results.lock().unwrap().clone();
    assert_eq!(
        tool_results.len(),
        3,
        "three apply_patch calls: {tool_results:#?}"
    );
    assert_eq!(
        std::fs::read_to_string(&realm_document).expect("realm document"),
        document,
        "the file tool changed the realm config document: {tool_results:#?}"
    );
    assert!(
        !realm_dir.join("shared-skills/planted").exists(),
        "the file tool wrote a skill through the linked skills directory: {tool_results:#?}"
    );
    assert_eq!(
        std::fs::read_to_string(realm_dir.join("out.txt")).expect("the ordinary file"),
        "written by the model\n",
        "{tool_results:#?}"
    );
    assert_eq!(
        tool_results
            .iter()
            .filter(|result| result.contains("engine's own directory"))
            .count(),
        2,
        "two calls were refused as writes into the server's own files: {tool_results:#?}"
    );
}

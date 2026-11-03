//! The composed gateway hands its host the installed identity view between
//! installing the identity context and bootstrapping it: after `bind_runtime`,
//! before the identity bootstrap registers persisted owners, consumes the
//! pending activation or restores the roster. A refusal there fails init
//! before any of those, and the no-identity path never calls it. Each case is a separate gateway process
//! over real stdio, answering the SDK roster callbacks itself. On both paths
//! the Ready carries the runtime's bootstrap completion, which the host
//! publishes inside its publication boundary.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::io::{BufRead, BufReader, Write};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use meerkat_meta::UnifiedRuntime;
use meerkat_meta::rpc_gateway::{
    GatewayBootstrapReady, GatewayHostCloseReason, GatewayHostComposition, GatewayHostError,
};
use meerkat_runtime::meerkat_machine::NativeGrantWorkConfiguration;
use serde_json::{Value, json};

/// Set (to `bind`, `refuse` or `plain`) only in the gateway host process;
/// `RECORD` names its record file.
const HOST_PROCESS: &str = "META_GATEWAY_IDENTITY_BIND_HOST";
const RECORD: &str = "META_GATEWAY_IDENTITY_BIND_RECORD";
const TEST: &str = "the_host_binds_the_installed_identity_view_before_bootstrap";
const BACKSTOP: Duration = Duration::from_secs(90);

/// Progress phases the identity bootstrap reports; none may precede the bind.
const BOOTSTRAP_PHASES: [&str; 4] = ["register_owners", "prewarm", "activate", "restore"];

struct BindingHost {
    refuse: bool,
    record: std::path::PathBuf,
}

impl BindingHost {
    fn append(&self, line: &str) {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.record)
            .expect("record");
        writeln!(file, "{line}").expect("append");
    }
}

#[async_trait::async_trait]
impl GatewayHostComposition for BindingHost {
    async fn prepare_bootstrap(
        &self,
        spec: meerkat_meta::MobBootstrapSpec,
    ) -> Result<meerkat_meta::MobBootstrapSpec, GatewayHostError> {
        Ok(spec)
    }

    fn native_grant_work_configuration(
        &self,
    ) -> Result<Option<NativeGrantWorkConfiguration>, GatewayHostError> {
        Ok(None)
    }

    async fn bind_runtime(&self, runtime: &mut UnifiedRuntime) -> Result<(), GatewayHostError> {
        self.append(&format!(
            "bind_runtime identity={}",
            runtime.identity_runtime().is_some()
        ));
        Ok(())
    }

    async fn bind_identity_runtime(
        &self,
        runtime: &mut UnifiedRuntime,
    ) -> Result<(), GatewayHostError> {
        self.append(&format!(
            "bind_identity identity={}",
            runtime.identity_runtime().is_some()
        ));
        if self.refuse {
            Err(GatewayHostError::Unavailable)
        } else {
            Ok(())
        }
    }

    fn publish_bootstrap_ready(
        &self,
        mut publication: GatewayBootstrapReady<'_>,
    ) -> Result<(), GatewayHostError> {
        let completion = match publication.take_runtime_completion() {
            Some(completion) => match completion.publish(|| ()) {
                Ok(()) => "published",
                Err(_) => "refused",
            },
            None => "absent",
        };
        self.append(&format!("ready completion={completion}"));
        publication.publish();
        Ok(())
    }

    fn begin_close(&self, _reason: GatewayHostCloseReason) -> Result<(), GatewayHostError> {
        Ok(())
    }

    async fn join_close(&self) -> Result<(), GatewayHostError> {
        Ok(())
    }

    async fn wait_closed(&self) {
        std::future::pending::<()>().await;
    }
}

/// Run one gateway host process through its init; returns the record lines,
/// every message seen, and whether a bootstrap phase was reported before the
/// identity bind was recorded.
fn run(mode: &str) -> (Vec<String>, Vec<Value>, bool) {
    let dir = tempfile::tempdir().unwrap();
    let record = dir.path().join("record");
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", TEST, "--nocapture"])
        .env(HOST_PROCESS, mode)
        .env(RECORD, &record)
        .env("OPENAI_API_KEY", "sk-regression-test")
        .env("XDG_STATE_HOME", dir.path().join("xdg-state"))
        .current_dir(dir.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("gateway host process");
    let stdout = child.stdout.take().expect("stdout");
    let (tx, lines) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut stdin = child.stdin.take().expect("stdin");
    let mut send = |value: Value| {
        writeln!(stdin, "{value}").expect("write");
        stdin.flush().expect("flush");
    };
    let mut params = json!({
        "init_protocol": "accepted_then_settled",
        "init_id": format!("identity-bind-{mode}"),
        "mob_config": "[mob]\nid = \"gateway-identity-bind\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n",
        "persistent_state": dir.path().join("state"),
    });
    if mode != "plain" {
        params["has_roster_provider"] = json!(true);
    }
    send(json!({"jsonrpc": "2.0", "id": "init", "method": "meta/init", "params": params}));

    let started = Instant::now();
    let mut seen = Vec::new();
    let mut bootstrap_before_bind = false;
    loop {
        let remaining = BACKSTOP.saturating_sub(started.elapsed());
        let line = match lines.recv_timeout(remaining) {
            Ok(line) => line,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                panic!("{mode}: the gateway ended unsettled; seen {seen:?}")
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                panic!("{mode}: no settlement; seen {seen:?}")
            }
        };
        let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        if let (Some(method), Some(id)) = (
            message.get("method").and_then(Value::as_str),
            message.get("id").cloned(),
        ) {
            let reply = if method == "callback/roster_provider/roster" {
                json!({ "jsonrpc": "2.0", "id": id, "result": [] })
            } else {
                json!({ "jsonrpc": "2.0", "id": id,
                    "error": { "code": -32601, "message": format!("no handler for {method}") } })
            };
            send(reply);
        }
        if message.get("method").and_then(Value::as_str) == Some("meta/init_progress") {
            let phase = message["params"]["phase"].as_str().unwrap_or_default();
            if BOOTSTRAP_PHASES.contains(&phase) {
                let recorded = std::fs::read_to_string(&record).unwrap_or_default();
                if !recorded.contains("bind_identity identity=true") {
                    bootstrap_before_bind = true;
                }
            }
        }
        let settled = message.get("method").and_then(Value::as_str) == Some("meta/init_settled");
        seen.push(message);
        if settled {
            break;
        }
    }
    drop(stdin);
    let deadline = Instant::now() + BACKSTOP;
    while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let exited = child.try_wait().unwrap().is_some();
    if !exited {
        let _ = child.kill();
    }
    let _ = child.wait();
    assert!(exited, "{mode}: the gateway ends after its init");
    let recorded = std::fs::read_to_string(&record).unwrap_or_default();
    (
        recorded.lines().map(str::to_string).collect(),
        seen,
        bootstrap_before_bind,
    )
}

/// The record's binding lines, without how the run returned.
fn binds(lines: &[String]) -> Vec<&str> {
    lines
        .iter()
        .map(String::as_str)
        .filter(|line| line.starts_with("bind_"))
        .collect()
}

fn phases(seen: &[Value]) -> Vec<String> {
    seen.iter()
        .filter(|m| m.get("method").and_then(Value::as_str) == Some("meta/init_progress"))
        .map(|m| {
            m["params"]["phase"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

#[test]
fn the_host_binds_the_installed_identity_view_before_bootstrap() {
    if let Some(mode) = std::env::var_os(HOST_PROCESS) {
        let host = Arc::new(BindingHost {
            refuse: mode == "refuse",
            record: std::env::var_os(RECORD).expect("record path").into(),
        });
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        let returned = runtime.block_on(meerkat_meta::rpc_gateway::run_persistent_with_host(
            None,
            Arc::clone(&host) as Arc<dyn GatewayHostComposition>,
        ));
        // Record how the run returned before the process exits. A failure
        // here comes after cleanup handling and the host-close await; it can
        // still carry runtime-cleanup uncertainty (ShutdownIncomplete after
        // the shutdown observation horizon).
        host.append(&format!("returned ok={}", returned.is_ok()));
        std::process::exit(0);
    }

    // The identity view does not exist at the early bind; it does at the
    // identity bind, which precedes every identity bootstrap phase.
    let (lines, seen, bootstrap_before_bind) = run("bind");
    assert_eq!(
        binds(&lines),
        ["bind_runtime identity=false", "bind_identity identity=true"],
        "{seen:?}"
    );
    assert!(!bootstrap_before_bind, "bound before bootstrap: {seen:?}");
    assert!(
        phases(&seen).iter().any(|phase| phase == "restore"),
        "the bound context was then bootstrapped: {:?}",
        phases(&seen)
    );
    assert_eq!(seen.last().unwrap()["params"]["outcome"], "ready");
    assert!(
        lines
            .iter()
            .any(|line| line == "ready completion=published"),
        "the identity bootstrap's completion rides the Ready: {lines:?}"
    );

    // A refused identity bind fails init before any bootstrap phase, and the
    // gateway run returns failure after cleanup handling and the host-close
    // await. `ok=false` also covers ShutdownIncomplete after the shutdown
    // observation horizon, so this does not show that cleanup completed or
    // that any storage handle was released.
    let (lines, seen, _) = run("refuse");
    assert_eq!(
        lines,
        [
            "bind_runtime identity=false",
            "bind_identity identity=true",
            "returned ok=false"
        ]
    );
    let bootstrap: Vec<String> = phases(&seen)
        .into_iter()
        .filter(|phase| BOOTSTRAP_PHASES.contains(&phase.as_str()))
        .collect();
    assert!(
        bootstrap.is_empty(),
        "no bootstrap after a refusal: {bootstrap:?}"
    );
    let settled = &seen.last().unwrap()["params"];
    assert_eq!(settled["outcome"], "failed", "{settled}");
    assert!(
        settled
            .to_string()
            .contains(&GatewayHostError::Unavailable.to_string()),
        "the init failure is the host's own error: {settled}"
    );

    // Without an identity roster the identity bind is never called, and the
    // gateway still starts: its init settles ready.
    let (lines, seen, _) = run("plain");
    assert_eq!(binds(&lines), ["bind_runtime identity=false"], "{seen:?}");
    assert_eq!(
        seen.last().unwrap()["params"]["outcome"],
        "ready",
        "{seen:?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line == "ready completion=published"),
        "the activation's completion rides the Ready: {lines:?}"
    );
}

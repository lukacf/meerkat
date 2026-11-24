//! The composed gateway bootstraps an identity runtime on bounded stacks.
//! Embedders run `run_persistent_with_host` on default-sized threads and tokio
//! workers; the workspace `RUST_MIN_STACK` used by the other tests does not
//! reach them. The host process here removes it, runs its caller on an
//! explicit stack and its runtime workers at the same size, and must settle
//! init `ready` and exit cleanly rather than overflow.
//!
//! The bound is this row's, for such an embedder. It is not the gateway
//! binaries' stack: they run their workers at 16 MiB.
//!
//! An optimized build bootstraps on the ordinary 2 MiB. An unoptimized build
//! keeps one stack slot per temporary in each poll frame, and room for a
//! callee's whole future at every call it awaits; it needs more than 2 MiB
//! and is held to 3 MiB here. The deepest point of a start is the identity
//! roster's bootstrap, awaited from `run_persistent_inner`: the control below
//! says what was measured there.
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

/// Set only in the gateway host process, to its stack size in bytes.
const HOST_PROCESS: &str = "META_GATEWAY_BOOTSTRAP_STACK_HOST";
const TEST: &str = "the_gateway_bootstraps_an_identity_runtime_on_bounded_stacks";
const TWO_MIB_CONTROL: &str = "the_unoptimized_gateway_bootstraps_on_two_mib_stacks";
const BACKSTOP: Duration = Duration::from_mins(2);
const STACK: usize = if cfg!(debug_assertions) {
    3 << 20
} else {
    2 << 20
};

struct PlainHost;

#[async_trait::async_trait]
impl GatewayHostComposition for PlainHost {
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

    async fn bind_runtime(&self, _runtime: &mut UnifiedRuntime) -> Result<(), GatewayHostError> {
        Ok(())
    }

    async fn bind_identity_runtime(
        &self,
        _runtime: &mut UnifiedRuntime,
    ) -> Result<(), GatewayHostError> {
        Ok(())
    }

    fn publish_bootstrap_ready(
        &self,
        publication: GatewayBootstrapReady<'_>,
    ) -> Result<(), GatewayHostError> {
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

/// The gateway host process: the caller thread and every runtime worker get
/// exactly `stack` bytes.
fn host_process(stack: usize) -> ! {
    let returned = std::thread::Builder::new()
        .name("gateway-caller".into())
        .stack_size(stack)
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .thread_stack_size(stack)
                .enable_all()
                .build()
                .expect("tokio runtime");
            runtime
                .block_on(meerkat_meta::rpc_gateway::run_persistent_with_host(
                    None,
                    Arc::new(PlainHost) as Arc<dyn GatewayHostComposition>,
                ))
                .is_ok()
        })
        .expect("caller thread")
        .join()
        .expect("caller thread completes");
    eprintln!("gateway returned ok={returned}");
    // The parent requires a clean exit, so a failed run must not exit 0.
    if !returned {
        std::process::exit(1);
    }
    std::process::exit(0);
}

/// In the host process, run the gateway on the stack its parent chose.
fn serve_if_host_process() {
    if let Some(stack) = std::env::var_os(HOST_PROCESS) {
        let stack = stack
            .to_str()
            .and_then(|s| s.parse().ok())
            .expect("stack bytes");
        host_process(stack);
    }
}

#[test]
fn the_gateway_bootstraps_an_identity_runtime_on_bounded_stacks() {
    serve_if_host_process();
    bootstraps_ready_on(TEST, STACK);
}

/// The unoptimized 2 MiB control. Measured on an unoptimized build at the
/// identity roster's bootstrap, the deepest point of a start: the worker
/// stack was about 3.14 MB deep, past this file's 3 MiB by less than 32 KB.
/// `run_persistent_inner`'s own poll frame was about 2.05 MB of it: about
/// 0.73 MB for the bootstrap spec, which every call of its builder chain
/// returns by value into a slot of its own, and about 0.39 MB for the future
/// of an init's abort at a shutdown request, kept whole at each of its seven
/// call sites. The roster bootstrap above that frame was about 1.08 MB
/// (`apply_open_declared_roster` about 246 KB,
/// `bootstrap_installed_identity_first_context_inner` about 223 KB, the rest
/// in frames of 15 to 60 KB). The abort's future is on the heap since, which
/// takes its slots out of that frame: measured again, the frame is about
/// 1.71 MB (0.34 MB less) and a start settles on about 2.85 MB of worker
/// stack. The spec's slots and the other callees' futures stay in it; an
/// unoptimized embedder sets an explicit worker stack.
#[test]
#[ignore = "an unoptimized build needs more than 2 MiB; see the doc comment"]
fn the_unoptimized_gateway_bootstraps_on_two_mib_stacks() {
    serve_if_host_process();
    bootstraps_ready_on(TWO_MIB_CONTROL, 2 << 20);
}

/// Run the gateway host as a child process of this test binary on `stack`
/// bytes and require init `ready` and a clean exit.
fn bootstraps_ready_on(test: &str, stack: usize) {
    let dir = tempfile::tempdir().unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--include-ignored", "--nocapture"])
        .env(HOST_PROCESS, stack.to_string())
        .env_remove("RUST_MIN_STACK")
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
    send(
        json!({"jsonrpc": "2.0", "id": "init", "method": "meta/init", "params": {
            "init_protocol": "accepted_then_settled",
            "init_id": "bootstrap-stack",
            "mob_config": "[mob]\nid = \"gateway-bootstrap-stack\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n",
            "persistent_state": dir.path().join("state"),
            "has_roster_provider": true,
        }}),
    );

    let started = Instant::now();
    let mut seen = Vec::new();
    let settled = loop {
        let remaining = BACKSTOP.saturating_sub(started.elapsed());
        let line = match lines.recv_timeout(remaining) {
            Ok(line) => line,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let status = child.wait().unwrap();
                panic!("the gateway ended unsettled ({status}); seen {seen:?}")
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let _ = child.kill();
                panic!("no settlement; seen {seen:?}")
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
        if message.get("method").and_then(Value::as_str) == Some("meta/init_settled") {
            break message;
        }
        seen.push(message);
    };
    assert_eq!(
        settled["params"]["outcome"], "ready",
        "{settled}; seen {seen:?}"
    );

    drop(stdin);
    let deadline = Instant::now() + BACKSTOP;
    while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let Some(status) = child.try_wait().unwrap() else {
        let _ = child.kill();
        panic!("the gateway did not end after its stdin closed");
    };
    assert!(
        status.success(),
        "the gateway run did not return Ok or its process ended abnormally: {status}"
    );
}

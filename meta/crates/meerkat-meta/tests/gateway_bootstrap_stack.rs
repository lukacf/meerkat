//! The composed gateway bootstraps an identity runtime on bounded stacks.
//! Embedders run `run_persistent_with_host` on default-sized threads and tokio
//! workers; the workspace `RUST_MIN_STACK` used by the other tests does not
//! reach them. The host process here removes it, runs its caller on an
//! explicit stack and its runtime workers at the same size, and must settle
//! init `ready` and exit cleanly rather than overflow.
//!
//! An optimized build bootstraps on the ordinary 2 MiB. An unoptimized build
//! keeps one stack slot per temporary in each poll frame; with the runtime
//! construction futures heap-placed it needs a little over 2 MiB, against more
//! than 4 MiB before, so it is held to 3 MiB here.
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

/// The unoptimized 2 MiB control. Measured on the heap-placed tree, the
/// overflowing worker stack is about 2.09 MB: `run_persistent_inner`'s own
/// poll frame (about 793 KB of locals and temporaries; its future is 35 KB),
/// the native mob-mcp restore chain (`restore_from_persistent_storage` about
/// 529 KB, `reobserve_held_child_mobs` about 320 KB,
/// `scavenge_orphaned_bridge_session_scoped_mobs_inner` about 183 KB) and the
/// remaining bootstrap chain (about 200 KB). Heap placement cannot shrink
/// those frames; an unoptimized embedder sets an explicit worker stack.
#[test]
#[ignore = "an unoptimized build needs a little over 2 MiB; see the doc comment"]
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

//! The actual native gateway owns the opaque readiness token. Exercise its
//! no-identity branch with the real installed Toolkit host in a child process.
use super::*;
use meerkat_core::service::SessionQuery;
use meerkat_mob::MobSessionService;
use meerkat_mobkit::{
    MobBootstrapSpec, UnifiedRuntime,
    rpc_gateway::{
        GatewayBootstrapReady, GatewayFactoryConstruction, GatewayHostCloseReason,
        GatewayHostComposition, GatewayHostError, GatewayMcpComposition, GatewayRunError,
        PreparedApplicationRequest, PreparedGatewayApplication,
    },
};
use meerkat_runtime::service_ext::SessionServiceRuntimeExt;
use serde::{Deserialize, Serialize};
use toolkit_store::ToolkitStore;
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, Command, Stdio},
    sync::{Mutex, mpsc},
    time::{Duration, Instant},
};

const CHILD_REPORT: &str = "TOOLKIT_GATEWAY_IDENTITY_NEGATIVE_REPORT";
const TEST: &str = "installed_host::tests::gateway_identity::native_no_identity_path_refuses_toolkit_ready_before_input_or_publication";
const BACKSTOP: Duration = Duration::from_secs(90);

#[derive(Default, Serialize, Deserialize, Debug)]
struct Trace {
    prepared: bool,
    runtime_bound: bool,
    identity_calls: usize,
    ready_calls: usize,
    identity_at_ready: bool,
    selection_current_at_ready: bool,
    ready_refused_unavailable: bool,
    close_joined: bool,
}

/// Observe calls made by native. Every hook delegates unchanged to the same
/// Toolkit host, including the opaque publication and optional factory hooks.
/// This wrapper neither supplies an identity nor changes a returned result.
struct ObservedHost {
    inner: ToolkitGatewayHost,
    owner: Arc<NativeHostLifetime>,
    originals: Arc<PrivateOriginals>,
    trace: Mutex<Trace>,
    service: Mutex<Option<Arc<dyn MobSessionService>>>,
}

#[async_trait::async_trait]
impl GatewayHostComposition for ObservedHost {
    async fn prepare_bootstrap(&self, spec: MobBootstrapSpec) -> Result<MobBootstrapSpec, GatewayHostError> {
        let result = self.inner.prepare_bootstrap(spec).await;
        self.trace.lock().unwrap().prepared = result.is_ok();
        result
    }
    fn native_grant_work_configuration(&self) -> Result<Option<meerkat_runtime::meerkat_machine::NativeGrantWorkConfiguration>, GatewayHostError> {
        self.inner.native_grant_work_configuration()
    }
    async fn bind_runtime(&self, runtime: &mut UnifiedRuntime) -> Result<(), GatewayHostError> {
        let result = self.inner.bind_runtime(runtime).await;
        if result.is_ok() {
            *self.service.lock().unwrap() = runtime.mob_runtime().session_service().cloned();
        }
        self.trace.lock().unwrap().runtime_bound = result.is_ok();
        result
    }
    async fn bind_identity_runtime(&self, runtime: &mut UnifiedRuntime) -> Result<(), GatewayHostError> {
        self.trace.lock().unwrap().identity_calls += 1;
        self.inner.bind_identity_runtime(runtime).await
    }
    fn publish_bootstrap_ready(&self, publication: GatewayBootstrapReady<'_>) -> Result<(), GatewayHostError> {
        let identity_present = self.originals.bound_identity_runtime().is_ok();
        let selection_current = self.owner.validate_current_selection().is_ok();
        let result = self.inner.publish_bootstrap_ready(publication);
        let mut trace = self.trace.lock().unwrap();
        trace.ready_calls += 1;
        trace.identity_at_ready = identity_present;
        trace.selection_current_at_ready = selection_current;
        trace.ready_refused_unavailable = matches!(result, Err(GatewayHostError::Unavailable));
        result
    }
    fn begin_close(&self, reason: GatewayHostCloseReason) -> Result<(), GatewayHostError> {
        self.inner.begin_close(reason)
    }
    async fn join_close(&self) -> Result<(), GatewayHostError> {
        let result = self.inner.join_close().await;
        self.trace.lock().unwrap().close_joined = result.is_ok();
        result
    }
    async fn wait_closed(&self) { self.inner.wait_closed().await; }
    async fn prepared_application(&self, request: Option<&PreparedApplicationRequest>) -> Result<Option<PreparedGatewayApplication>, GatewayHostError> {
        self.inner.prepared_application(request).await
    }
    fn compose_member_factory(&self, construction: &GatewayFactoryConstruction<'_>) -> Result<Option<GatewayMcpComposition>, GatewayHostError> {
        self.inner.compose_member_factory(construction)
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct Report {
    trace: Trace,
    returned_host_unavailable: bool,
    sessions: Option<usize>,
    inputs: Option<usize>,
    source_rows: Option<usize>,
    grant_bytes_unchanged: bool,
}

async fn run_child(report_path: &Path) {
    let (_directory, mut selection) = fixture();
    provision_fixture(&mut selection);
    let (owner, installed) = InstalledToolkitHost::open(selection.clone(), NativeOwnerStartup::Restore)
        .await.unwrap();
    let directory = state_directory(&selection, &installed.native, NativeOwnerStartup::Restore).unwrap();
    let grant_bytes = fs::read(directory.join("grants.jsonl")).unwrap();
    let originals = installed.originals.clone();
    let store = installed.store.clone();
    let scope = installed.source_scope.clone();
    let (_signal, closed) = tokio::sync::watch::channel(false);
    let owner = Arc::new(owner);
    let host = Arc::new(ObservedHost {
        inner: installed.compose(owner.clone(), PrivateHostClosed(closed)), owner,
        originals: originals.clone(), trace: Mutex::new(Trace::default()), service: Mutex::new(None),
    });
    let result = meerkat_mobkit::rpc_gateway::run_persistent_with_host(None, host.clone()).await;
    // The same actual service and machine remain available for observation.
    // Missing observations fail this control instead of being reported as zero.
    let service = host.service.lock().unwrap().clone();
    let sessions = match service {
        Some(service) => service.list(SessionQuery::default()).await.ok(),
        None => None,
    };
    let inputs = match (&sessions, originals.machine()) {
        (Some(sessions), Ok(machine)) => {
            let mut count = Some(0);
            for session in sessions {
                count = match (count, machine.meerkat_machine_spine_snapshot(&session.session_id).await) {
                    (Some(count), Some(snapshot)) => Some(count + snapshot.ledger.input_count),
                    _ => None,
                };
            }
            count
        }
        _ => None,
    };
    let source_rows = store.list_records(&scope, None, 1).await.ok().map(|rows| rows.len());
    let report = Report {
        trace: std::mem::take(&mut *host.trace.lock().unwrap()),
        returned_host_unavailable: matches!(result, Err(GatewayRunError::Host(GatewayHostError::Unavailable))),
        sessions: sessions.as_ref().map(Vec::len), inputs, source_rows,
        grant_bytes_unchanged: fs::read(directory.join("grants.jsonl")).unwrap() == grant_bytes,
    };
    fs::write(report_path, serde_json::to_vec(&report).unwrap()).unwrap();
}

struct ChildGuard {
    process: Child,
    reader: Option<std::thread::JoinHandle<()>>,
}
impl Drop for ChildGuard {
    fn drop(&mut self) {
        if !matches!(self.process.try_wait(), Ok(Some(_))) { let _ = self.process.kill(); }
        let _ = self.process.wait();
        if let Some(reader) = self.reader.take() { let _ = reader.join(); }
    }
}

#[test]
fn native_no_identity_path_refuses_toolkit_ready_before_input_or_publication() {
    if let Some(path) = std::env::var_os(CHILD_REPORT) {
        let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
        runtime.block_on(run_child(Path::new(&path)));
        // The native driver has returned after cleanup. Its stdin helper can
        // remain blocked until EOF, as in the native gateway process entry.
        runtime.shutdown_background();
        std::process::exit(0);
    }
    let directory = tempfile::tempdir().unwrap();
    let report_path = directory.path().join("report.json");
    let stderr = fs::File::create(directory.path().join("stderr.log")).unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    if cfg!(debug_assertions) {
        // This unoptimized native gateway needs the measured 8 MiB child stack.
        // Keep release stack qualification separate from this debug fixture.
        command.env("RUST_MIN_STACK", "8388608");
    }
    let mut child = ChildGuard { process: command
        .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
        .env(CHILD_REPORT, &report_path)
        .env("OPENAI_API_KEY", "sk-no-request-identity-fixture")
        .env("XDG_STATE_HOME", directory.path().join("xdg-state"))
        .current_dir(directory.path())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(stderr)
        .spawn().unwrap(), reader: None };
    let stdout = child.process.stdout.take().unwrap();
    let (tx, lines) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if tx.send(line).is_err() { break; }
        }
    });
    child.reader = Some(reader);
    let mut stdin = child.process.stdin.take().unwrap();
    let request = serde_json::json!({
        "jsonrpc": "2.0", "id": "init", "method": "meta/init",
        "params": {
            "init_protocol": "accepted_then_settled", "init_id": "toolkit-no-identity",
            "mob_config": "[mob]\nid = \"factory\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n",
            "persistent_state": directory.path().join("state")
        }
    });
    writeln!(stdin, "{request}").unwrap();
    stdin.flush().unwrap();
    let deadline = Instant::now() + BACKSTOP;
    let mut messages = Vec::new();
    let mut timed_out = false;
    loop {
        match lines.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(Ok(line)) => {
                if let Ok(message) = serde_json::from_str::<serde_json::Value>(line.trim()) { messages.push(message); }
            }
            Ok(Err(_)) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => { timed_out = true; break; }
        }
    }
    drop(stdin);
    while child.process.try_wait().unwrap().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    if child.process.try_wait().unwrap().is_none() {
        timed_out = true;
        let _ = child.process.kill();
    }
    let status = child.process.wait().unwrap();
    child.reader.take().unwrap().join().unwrap();
    assert!(!timed_out, "native gateway did not settle within the test backstop");
    assert!(status.success(),
        "native gateway fixture failed; no readiness-refusal claim; status: {status:?}; stderr: {:?}; messages: {messages:?}",
        fs::read_to_string(directory.path().join("stderr.log")));
    let report: Report = serde_json::from_slice(&fs::read(report_path).unwrap()).unwrap();
    assert!(report.trace.prepared && report.trace.runtime_bound, "{report:?}");
    assert_eq!(report.trace.identity_calls, 0, "native must skip this hook without identity context");
    assert_eq!(report.trace.ready_calls, 1, "must reach the actual opaque readiness offer");
    assert!(!report.trace.identity_at_ready, "{report:?}");
    assert!(report.trace.selection_current_at_ready, "an unrelated closed/stale owner must not satisfy the refusal oracle");
    assert!(report.trace.ready_refused_unavailable && report.returned_host_unavailable, "{report:?}");
    assert!(report.trace.close_joined, "actual Toolkit close must settle");
    assert_eq!((report.sessions, report.inputs, report.source_rows), (Some(0), Some(0), Some(0)), "{report:?}");
    assert!(report.grant_bytes_unchanged, "no native grant issuance during this failed init");
    assert!(!messages.iter().any(|m| m["method"] == "meta/init_settled" && m["params"]["outcome"] == "ready"),
        "a refused opaque publication cannot emit Ready");
    assert!(!messages.iter().any(|m| m["id"] == "init" && m["result"]["http_base_url"].is_string()),
        "a refused opaque publication cannot expose a ready HTTP endpoint");
}

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
use serde::{Deserialize, Serialize};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, Command, Stdio},
    sync::{Mutex, mpsc},
    time::{Duration, Instant},
};
use toolkit_store::ToolkitStore;

mod collector;

const CHILD_REPORT: &str = "TOOLKIT_GATEWAY_IDENTITY_NEGATIVE_REPORT";
const TEST: &str = "installed_host::tests::gateway_identity::native_no_identity_path_refuses_toolkit_ready_before_input_or_publication";
const BACKSTOP: Duration = Duration::from_secs(90);

#[derive(Default, Serialize, Deserialize, Debug)]
struct Trace {
    prepared: bool,
    operation_review_calls: usize,
    operation_review_exact: bool,
    operation_review_closed: bool,
    collector_completed: bool,
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
    inner: Arc<ToolkitGatewayHost>,
    expected_review: Option<Arc<meerkat_core::approval::review::BoundOperationReview>>,
    collector: Option<Arc<collector::Fixture>>,
    owner: Arc<NativeHostLifetime>,
    originals: Arc<PrivateOriginals>,
    trace: Mutex<Trace>,
    service: Mutex<Option<Arc<dyn MobSessionService>>>,
}

#[async_trait::async_trait]
impl GatewayHostComposition for ObservedHost {
    fn operation_review(
        &self,
        construction: &GatewayFactoryConstruction<'_>,
    ) -> Result<Option<Arc<meerkat_core::approval::review::BoundOperationReview>>, GatewayHostError>
    {
        // Only native creates this offer. Delegate the exact same borrow.
        let selected = self.inner.operation_review(construction)?;
        let mut trace = self.trace.lock().unwrap();
        trace.operation_review_calls += 1;
        trace.operation_review_exact = match (&self.expected_review, &selected) {
            (None, None) => true,
            (Some(expected), Some(actual)) => Arc::ptr_eq(expected, actual),
            _ => false,
        };
        if !trace.operation_review_exact {
            return Err(GatewayHostError::Denied);
        }
        Ok(selected)
    }

    async fn prepare_bootstrap(
        &self,
        spec: MobBootstrapSpec,
    ) -> Result<MobBootstrapSpec, GatewayHostError> {
        let result = self.inner.prepare_bootstrap(spec).await;
        self.trace.lock().unwrap().prepared = result.is_ok();
        result
    }
    fn native_grant_work_configuration(
        &self,
    ) -> Result<
        Option<meerkat_runtime::meerkat_machine::NativeGrantWorkConfiguration>,
        GatewayHostError,
    > {
        self.inner.native_grant_work_configuration()
    }
    async fn bind_runtime(&self, runtime: &mut UnifiedRuntime) -> Result<(), GatewayHostError> {
        eprintln!("collector_fixture runtime.bind.begin");
        let result = self.inner.bind_runtime(runtime).await;
        eprintln!("collector_fixture runtime.bind.end");
        if result.is_ok() {
            *self.service.lock().unwrap() = runtime.mob_runtime().session_service().cloned();
        }
        self.trace.lock().unwrap().runtime_bound = result.is_ok();
        if result.is_ok()
            && let Some(fixture) = &self.collector
        {
            if !fixture.exercise(self).await {
                return Err(GatewayHostError::Denied);
            }
            self.trace.lock().unwrap().collector_completed = true;
        }
        result
    }
    async fn bind_identity_runtime(
        &self,
        runtime: &mut UnifiedRuntime,
    ) -> Result<(), GatewayHostError> {
        self.trace.lock().unwrap().identity_calls += 1;
        self.inner.bind_identity_runtime(runtime).await
    }
    fn publish_bootstrap_ready(
        &self,
        publication: GatewayBootstrapReady<'_>,
    ) -> Result<(), GatewayHostError> {
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
        eprintln!("collector_fixture host.join_close.begin");
        let result = self.inner.join_close().await;
        eprintln!("collector_fixture host.join_close.end");
        self.trace.lock().unwrap().close_joined = result.is_ok();
        result
    }
    async fn wait_closed(&self) {
        self.inner.wait_closed().await;
    }
    async fn prepared_application(
        &self,
        request: Option<&PreparedApplicationRequest>,
    ) -> Result<Option<PreparedGatewayApplication>, GatewayHostError> {
        self.inner.prepared_application(request).await
    }
    fn compose_member_factory(
        &self,
        construction: &GatewayFactoryConstruction<'_>,
    ) -> Result<Option<GatewayMcpComposition>, GatewayHostError> {
        let result = self.inner.compose_member_factory(construction)?;
        if let Some(fixture) = &self.collector
            && !fixture.capture(self, construction)
        {
            return Err(GatewayHostError::Denied);
        }
        Ok(result)
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct Report {
    trace: Trace,
    returned_host_unavailable: bool,
    sessions: Option<usize>,
    inputs: Option<usize>,
    source_rows: Option<usize>,
    source_rows_unchanged: Option<bool>,
    grant_bytes_unchanged: bool,
}

struct NoOperationReview;
#[async_trait::async_trait]
impl meerkat_core::approval::review::OperationReviewer for NoOperationReview {
    async fn review(
        &self,
        _: &meerkat_core::approval::review::ReviewCandidate<'_>,
    ) -> Result<
        meerkat_core::approval::review::ReviewVerdict,
        meerkat_core::approval::review::ReviewerFailure,
    > {
        panic!("factory selection must not issue or decide a review")
    }
}

async fn run_child(report_path: &Path, collector: bool) {
    run_child_selected(report_path, collector, false).await
}
async fn run_child_selected(report_path: &Path, collector: bool, review: bool) {
    let (_directory, mut selection) = fixture();
    let collector = if collector {
        use crate::installed_host::operator_setup::{OperatorOwnerStartup, OperatorSetup};
        OperatorSetup::open(
            selection.clone(),
            OperatorOwnerStartup::fresh("collector-public-test", 1).unwrap(),
        )
        .unwrap()
        .finish()
        .await
        .unwrap();
        Some(Arc::new(
            collector::Fixture::new(report_path.parent().unwrap()).await,
        ))
    } else {
        provision_fixture(&mut selection);
        None
    };
    let (owner, installed) =
        InstalledToolkitHost::open(selection.clone(), NativeOwnerStartup::Restore)
            .await
            .unwrap();
    let directory =
        state_directory(&selection, &installed.native, NativeOwnerStartup::Restore).unwrap();
    let grant_bytes = fs::read(directory.join("grants.jsonl")).unwrap();
    let originals = installed.originals.clone();
    let store = installed.store.clone();
    let scope = installed.source_scope.clone();
    // Host preparation already admits its canonical service-policy bootstrap.
    // Two rows suffice to reject any extra row in this one-slot baseline.
    let source_before = store.list_records(&scope, None, 2).await.ok();
    if collector.is_none() {
        let rows = source_before
            .as_ref()
            .expect("observe prepared source-store baseline");
        assert_eq!(
            rows.len(),
            1,
            "only the prepared policy bootstrap may preexist"
        );
        let row = &rows[0];
        let schema = "toolkit.application-service-tool-policy";
        let policy = installed.tool_policy.compiled();
        let identity = canonical_json(&(schema, &policy.provider_id, &policy.policy_id)).unwrap();
        let bootstrap = String::from_utf8(policy.canonical_json().unwrap()).unwrap();
        assert_eq!(row.key.scope(), scope);
        assert_eq!(
            row.key.id,
            format!("service-tool-policy-{}", digest(&identity))
        );
        assert_eq!(
            (row.revision, row.data.schema.as_str(), row.data.version),
            (1, schema, 1)
        );
        assert_eq!(row.content_digest, row.data.digest().unwrap());
        assert_eq!(
            row.data.body,
            serde_json::json!({
                "bootstrap": bootstrap, "current": bootstrap, "decision": null, "pending": null,
            })
        );
    }
    let (_signal, closed) = tokio::sync::watch::channel(false);
    let owner = Arc::new(owner);
    let host = installed.compose(owner.clone(), PrivateHostClosed(closed));
    let expected_review = review.then(|| {
        Arc::new(meerkat_core::approval::review::BoundOperationReview::new(
            Arc::new(NoOperationReview),
            meerkat_core::ApprovalService::new(),
            Duration::from_secs(30),
        ))
    });
    let host = if let Some(review) = &expected_review {
        let review = review.clone();
        host.with_operation_review(|actual| {
            assert!(
                std::ptr::eq(actual, owner.as_ref()),
                "composition must supply its own lifetime"
            );
            Ok(Arc::new(move || Ok(review.clone())))
        })
        .unwrap()
    } else {
        host
    };
    let host = Arc::new(ObservedHost {
        inner: Arc::new(host),
        expected_review,
        owner,
        collector: collector.clone(),
        originals: originals.clone(),
        trace: Mutex::new(Trace::default()),
        service: Mutex::new(None),
    });
    eprintln!("collector_fixture gateway.run.begin");
    let result = meerkat_mobkit::rpc_gateway::run_persistent_with_host(None, host.clone()).await;
    eprintln!("collector_fixture gateway.run.end");
    if let Some(collector) = collector {
        eprintln!("collector_fixture collector.finish.begin");
        collector.finish().await;
        eprintln!("collector_fixture collector.finish.end");
    }
    host.trace.lock().unwrap().operation_review_closed = matches!(
        host.inner.selected_operation_review(),
        Err(GatewayHostError::Denied)
    );
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
                count = match (
                    count,
                    machine
                        .meerkat_machine_spine_snapshot(&session.session_id)
                        .await,
                ) {
                    (Some(count), Some(snapshot)) => Some(count + snapshot.ledger.input_count),
                    _ => None,
                };
            }
            count
        }
        _ => None,
    };
    let source_after = store.list_records(&scope, None, 2).await.ok();
    let source_rows = source_after.as_ref().map(Vec::len);
    let source_rows_unchanged = source_before
        .as_ref()
        .zip(source_after.as_ref())
        .map(|(before, after)| before == after);
    let report = Report {
        trace: std::mem::take(&mut *host.trace.lock().unwrap()),
        returned_host_unavailable: matches!(
            result,
            Err(GatewayRunError::Host(GatewayHostError::Unavailable))
        ),
        sessions: sessions.as_ref().map(Vec::len),
        inputs,
        source_rows,
        source_rows_unchanged,
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
        if !matches!(self.process.try_wait(), Ok(Some(_))) {
            let _ = self.process.kill();
        }
        let _ = self.process.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

#[test]
fn native_no_identity_path_refuses_toolkit_ready_before_input_or_publication() {
    if let Some(path) = std::env::var_os(CHILD_REPORT) {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(run_child(Path::new(&path), false));
        // The native driver has returned after cleanup. Its stdin helper can
        // remain blocked until EOF, as in the native gateway process entry.
        runtime.shutdown_background();
        std::process::exit(0);
    }
    let (report, messages) = child_report(TEST);
    assert_eq!(
        report.trace.operation_review_calls, 1,
        "actual native factory callback"
    );
    assert!(
        report.trace.operation_review_exact && report.trace.operation_review_closed,
        "{report:?}"
    );
    assert!(
        report.trace.prepared && report.trace.runtime_bound,
        "{report:?}"
    );
    assert_eq!(
        report.trace.identity_calls, 0,
        "native must skip this hook without identity context"
    );
    assert_eq!(
        report.trace.ready_calls, 1,
        "must reach the actual opaque readiness offer"
    );
    assert!(!report.trace.identity_at_ready, "{report:?}");
    assert!(
        report.trace.selection_current_at_ready,
        "an unrelated closed/stale owner must not satisfy the refusal oracle"
    );
    assert!(
        report.trace.ready_refused_unavailable && report.returned_host_unavailable,
        "{report:?}"
    );
    assert!(
        report.trace.close_joined,
        "actual Toolkit close must settle"
    );
    assert_eq!(
        (report.sessions, report.inputs),
        (Some(0), Some(0)),
        "{report:?}"
    );
    assert_eq!(
        report.source_rows_unchanged,
        Some(true),
        "no source-store row may be added, removed or changed by refused bootstrap: {report:?}"
    );
    assert!(
        report.grant_bytes_unchanged,
        "no native grant issuance during this failed init"
    );
    assert!(
        !messages
            .iter()
            .any(|m| m["method"] == "meta/init_settled" && m["params"]["outcome"] == "ready"),
        "a refused opaque publication cannot emit Ready"
    );
    assert!(
        !messages
            .iter()
            .any(|m| m["id"] == "init" && m["result"]["http_base_url"].is_string()),
        "a refused opaque publication cannot expose a ready HTTP endpoint"
    );
}

#[test]
fn native_installed_review_offer_preserves_exact_selection_and_closes_without_entry() {
    const REVIEW_TEST: &str = "installed_host::tests::gateway_identity::native_installed_review_offer_preserves_exact_selection_and_closes_without_entry";
    if let Some(path) = std::env::var_os(CHILD_REPORT) {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(run_child_selected(Path::new(&path), false, true));
        runtime.shutdown_background();
        std::process::exit(0);
    }
    let (report, messages) = child_report(REVIEW_TEST);
    assert_eq!(
        report.trace.operation_review_calls, 1,
        "actual native factory offer"
    );
    assert!(
        report.trace.operation_review_exact && report.trace.operation_review_closed,
        "{report:?}"
    );
    assert!(
        report.trace.prepared && report.trace.runtime_bound && report.trace.close_joined,
        "{report:?}"
    );
    assert!(
        report.trace.selection_current_at_ready
            && report.trace.ready_refused_unavailable
            && report.returned_host_unavailable,
        "the existing no-identity refusal remains unchanged: {report:?}"
    );
    assert_eq!(
        (report.sessions, report.inputs),
        (Some(0), Some(0)),
        "{report:?}"
    );
    assert_eq!(report.source_rows_unchanged, Some(true));
    assert!(report.grant_bytes_unchanged);
    assert!(
        !messages
            .iter()
            .any(|m| m["method"] == "meta/init_settled" && m["params"]["outcome"] == "ready")
    );
}

// Same emitted test binary and normal init; no Cargo or product build.
fn child_report(test: &str) -> (Report, Vec<serde_json::Value>) {
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir_all(directory.path().join("home")).unwrap();
    fs::create_dir_all(directory.path().join("config")).unwrap();
    let report_path = directory.path().join("report.json");
    let stderr = fs::File::create(directory.path().join("stderr.log")).unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    if cfg!(debug_assertions) {
        // This unoptimized native gateway needs the measured 8 MiB child stack.
        // Keep release stack qualification separate from this debug fixture.
        command.env("RUST_MIN_STACK", "8388608");
    }
    let mut child = ChildGuard {
        process: command
            .args(["--exact", test, "--nocapture", "--test-threads=1"])
            .env(CHILD_REPORT, &report_path)
            .env("OPENAI_API_KEY", "sk-no-request-identity-fixture")
            .env("XDG_STATE_HOME", directory.path().join("xdg-state"))
            .env("HOME", directory.path().join("home"))
            .env("XDG_CONFIG_HOME", directory.path().join("config"))
            .current_dir(directory.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr)
            .spawn()
            .unwrap(),
        reader: None,
    };
    let stdout = child.process.stdout.take().unwrap();
    let (tx, lines) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if tx.send(line).is_err() {
                break;
            }
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
                if let Ok(message) = serde_json::from_str::<serde_json::Value>(line.trim()) {
                    messages.push(message);
                }
            }
            Ok(Err(_)) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                timed_out = true;
                break;
            }
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
    assert!(
        !timed_out,
        "native gateway did not settle within the test backstop; stderr: {:?}",
        fs::read_to_string(directory.path().join("stderr.log"))
    );
    assert!(
        status.success(),
        "native gateway fixture failed; no readiness-refusal claim; status: {status:?}; stderr: {:?}; messages: {messages:?}",
        fs::read_to_string(directory.path().join("stderr.log"))
    );
    let report: Report = serde_json::from_slice(&fs::read(report_path).unwrap()).unwrap();
    (report, messages)
}

//! Actual Core requests and completion records, with synthetic stdlib stdio.
//! Registration uses the existing compiler test port after real native binding.
//! This does not install gateway guards, run production Python or prove reset.
use super::*;
use crate::application_setup::compilation::tests::{
    LocalRoundRegistrationFixture, local_round_registration_fixture,
};
use async_trait::async_trait;
use meerkat_core::mcp_attachment::McpAttachmentRefusal;
use meerkat_core::{ExternalToolDeltaPhase, McpServerConfig, SessionId};
use meerkat_mcp::{
    McpAttemptCancellation, McpAttemptGuard, McpAttemptGuardBounds, McpAttemptGuardFactory,
    McpAttemptGuardRequest, McpAttemptGuards, McpAttemptOpenOutcome, McpAttemptProcessEnd,
    McpConstructionOwner, McpGuardCompletion, McpGuardCompletionRecord, McpGuardStage, McpRouter,
    McpRouterShutdownReport, McpStdioLaunchProfile,
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

const NATIVE: &str = r#"import json, os, pathlib, select, sys
root = pathlib.Path(sys.argv[1]); pid = os.getpid()
mode = (root/'native-mode').read_text()
(root/(str(pid)+'.native-started')).write_text(json.dumps({'pid':pid}))
try:
    for line in sys.stdin:
        row=json.loads(line); method=row.get('method')
        if 'id' not in row: continue
        if method == 'initialize':
            if mode == 'initialize_failure': sys.exit(2)
            if mode == 'initialize_hold':
                (root/(str(pid)+'.native-initializing')).write_text('held')
                while True:
                    readable, _, _ = select.select([sys.stdin], [], [], 0.01)
                    if readable:
                        assert sys.stdin.buffer.read(1) == b''
                        sys.exit(0)
            result={'protocolVersion':row['params']['protocolVersion'],'capabilities':{'tools':{}},'serverInfo':{'name':'local-round-fixture','version':'0'}}
        elif method == 'tools/list':
            result={'tools':[{'name':'setup_selected_check','description':'inert transport','inputSchema':{'type':'object'}}]}
        elif method == 'tools/call':
            result={'content':[{'type':'text','text':'inert'}]}
        else: raise AssertionError(method)
        print(json.dumps({'jsonrpc':'2.0','id':row['id'],'result':result}),flush=True)
finally:
    (root/(str(pid)+'.native-ended')).write_text(json.dumps({'pid':pid}))
if mode == 'abnormal_exit': sys.exit(2)
"#;

struct Probe {
    inner: Arc<LocalRoundAttempts>,
    requests: Mutex<Vec<McpAttemptGuardRequest>>,
    hold_first: tokio::sync::watch::Sender<bool>,
    hold_second: tokio::sync::watch::Sender<bool>,
    held: AtomicBool,
}
#[async_trait]
impl McpAttemptGuardFactory for Probe {
    async fn begin(
        &self,
        request: McpAttemptGuardRequest,
        cancel: McpAttemptCancellation,
    ) -> Result<Option<Box<dyn McpAttemptGuard>>, McpAttachmentRefusal> {
        let ordinal = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(request.clone());
            requests.len()
        };
        let guard = self.inner.begin(request, cancel).await?;
        if ordinal == 1 && guard.is_some() && *self.hold_first.borrow() {
            self.held.store(true, Ordering::SeqCst);
            let mut hold = self.hold_first.subscribe();
            let _ = hold.wait_for(|value| !*value).await;
        }
        if ordinal == 2 && guard.is_some() && *self.hold_second.borrow() {
            // Core retains this real late return, including the offer Child.
            // The test releases it in all-exit cleanup, without aborting begin.
            self.held.store(true, Ordering::SeqCst);
            let mut hold = self.hold_second.subscribe();
            let _ = hold.wait_for(|value| !*value).await;
        }
        Ok(guard)
    }
}

struct NativeCase {
    protocol: Case,
    fixture: LocalRoundRegistrationFixture,
    attempts: Arc<LocalRoundAttempts>,
    probe: Arc<Probe>,
    guards: McpAttemptGuards,
    routers: Vec<Option<McpRouter>>,
    closing: Vec<JoinHandle<McpRouterShutdownReport>>,
    reports: Vec<McpRouterShutdownReport>,
    records: Vec<McpGuardCompletionRecord>,
    notices: Vec<meerkat_core::ExternalToolDelta>,
}
impl NativeCase {
    async fn new(protocol_mode: &str, native_mode: &str, late: bool) -> Self {
        let mut protocol = Case::new(protocol_mode);
        fs::write(protocol.output.join("native-mode"), native_mode).unwrap();
        let mut owner = protocol.owner.take().unwrap();
        let python = std::env::var("TOOLKIT_SHARED_TEST_PYTHON").unwrap();
        let handoff = serde_json::to_vec(&json!({"mcpServers":{"toolkit-setup":{
            "command":python,"args":["-I","-B","-c",NATIVE,protocol.output],
        }}}))
        .unwrap();
        retain(&owner.artifacts.join("prepare.stdout"), &handoff).unwrap();
        owner.handoff_sha256 = digest(&handoff);
        let prepared = PreparedResources {
            connection: crate::setup::PreparedSetupConnection::from_preparer_output(&handoff)
                .unwrap(),
            artifact_directory: owner.artifacts.clone(),
        };
        let (fixture, attempts) = local_round_registration_fixture(
            &protocol._directory.path().canonicalize().unwrap(),
            prepared,
            owner,
        )
        .await;
        let probe = Arc::new(Probe {
            inner: attempts.clone(),
            requests: Mutex::new(Vec::new()),
            hold_first: tokio::sync::watch::channel(false).0,
            hold_second: tokio::sync::watch::channel(late).0,
            held: AtomicBool::new(false),
        });
        let guards = McpAttemptGuards::new(
            probe.clone(),
            McpAttemptGuardBounds::new(
                Duration::from_millis(250),
                Duration::from_secs(2),
                Duration::from_secs(2),
            )
            .unwrap(),
        );
        Self {
            protocol,
            fixture,
            attempts,
            probe,
            guards,
            routers: Vec::new(),
            closing: Vec::new(),
            reports: Vec::new(),
            records: Vec::new(),
            notices: Vec::new(),
        }
    }
    fn route(&mut self, session: &SessionId) -> usize {
        let router = McpRouter::new_with_surface_handle_and_stdio_profile(
            Arc::new(meerkat_runtime::RuntimeExternalToolSurfaceHandle::ephemeral()),
            McpStdioLaunchProfile::trusted_host(),
        )
        .with_attempt_guards(
            self.guards.clone(),
            McpConstructionOwner::new(session.clone(), Some(self.fixture.member().clone())),
        );
        self.routers.push(Some(router));
        self.routers.len() - 1
    }
    async fn add(&mut self, route: usize, expected: ExternalToolDeltaPhase) {
        self.add_config(route, self.fixture.server().clone(), expected)
            .await;
    }
    async fn add_config(
        &mut self,
        route: usize,
        config: McpServerConfig,
        expected: ExternalToolDeltaPhase,
    ) {
        let name = config.name.clone();
        let router = self.routers[route].as_mut().unwrap();
        router.stage_add(config).unwrap();
        router.apply_staged().await.unwrap();
        let notices = &mut self.notices;
        tokio::time::timeout(BOUND, async {
            loop {
                let update = router.take_external_updates();
                let matched = update
                    .notices
                    .iter()
                    .any(|n| n.target == name && n.phase == expected);
                let wrong = update.notices.iter().any(|n| {
                    n.target == name
                        && matches!(
                            n.phase,
                            ExternalToolDeltaPhase::Applied
                                | ExternalToolDeltaPhase::Failed
                                | ExternalToolDeltaPhase::Forced
                        )
                        && n.phase != expected
                });
                if !update.notices.is_empty() {
                    eprintln!("local_round_native_notices {:?}", update.notices);
                }
                notices.extend(update.notices);
                assert!(!wrong, "wrong terminal notice: {notices:?}");
                if matched {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("actual native terminal notice");
    }
    async fn close(&mut self, route: usize) -> usize {
        let router = self.routers[route].take().unwrap();
        self.closing
            .push(tokio::spawn(router.shutdown_with_report()));
        let report = tokio::time::timeout(BOUND, self.closing.last_mut().unwrap())
            .await
            .expect("same retained native shutdown finishes")
            .expect("native shutdown task joins");
        self.closing.pop();
        self.reports.push(report);
        self.reports.len() - 1
    }
    fn assert_settled(&self, report: usize) {
        let report = &self.reports[report];
        assert!(
            report.is_settled() && report.guards_completed(),
            "{report:?}"
        );
        assert!(
            report
                .guards()
                .iter()
                .any(|r| r.stage == McpGuardStage::Settle
                    && r.completion == McpGuardCompletion::Completed
                    && r.disposition.as_ref().is_some_and(|d| matches!(
                        d.open,
                        McpAttemptOpenOutcome::Installed { .. }
                    ) && matches!(
                        d.process,
                        McpAttemptProcessEnd::Closed(_)
                    ))),
            "{report:?}"
        );
    }
    fn native_pids(&self) -> Vec<i32> {
        fs::read_dir(&self.protocol.output)
            .unwrap()
            .filter_map(|e| {
                let path = e.unwrap().path();
                if path.extension().and_then(|v| v.to_str()) != Some("native-started") {
                    return None;
                }
                let row: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
                Some(i32::try_from(row["pid"].as_i64().unwrap()).unwrap())
            })
            .collect()
    }
    async fn cleanup(&mut self) -> Vec<String> {
        let mut errors = Vec::new();
        self.probe.hold_first.send_replace(false);
        self.probe.hold_second.send_replace(false);
        fs::write(self.protocol.output.join("release"), "all-exit release").unwrap();
        for router in &mut self.routers {
            if let Some(router) = router.take() {
                self.closing
                    .push(tokio::spawn(router.shutdown_with_report()));
            }
        }
        // Retain and await exactly the same shutdown tasks after a test timeout.
        for task in self.closing.drain(..) {
            match task.await {
                Ok(report) => self.reports.push(report),
                Err(error) => errors.push(error.to_string()),
            }
        }
        loop {
            let batch = self
                .guards
                .close_and_join(tokio::time::Instant::now() + BOUND)
                .await;
            let pending = batch.iter().any(|r| {
                matches!(
                    r.completion,
                    McpGuardCompletion::Held | McpGuardCompletion::TimedOut
                )
            });
            self.records.extend(batch);
            if let Some(owner) = self.attempts.take_closed_owner() {
                self.protocol.owner = Some(owner);
            }
            if !pending && self.protocol.owner.is_some() {
                break;
            }
            // The case's outer executor owns the deadline. Never discard an
            // active Core task or its offer merely because a bounded join ends.
        }
        self.protocol.cleanup().await;
        for report in &self.reports {
            if !report.is_settled() {
                errors.push(format!("unsettled native process: {report:?}"));
            }
        }
        errors.extend(self.fixture.cleanup().await);
        for pid in self.native_pids().into_iter().chain(self.protocol.pids()) {
            if nix::sys::wait::waitpid(
                nix::unistd::Pid::from_raw(pid),
                Some(nix::sys::wait::WaitPidFlag::WNOHANG),
            ) != Err(nix::errno::Errno::ECHILD)
            {
                errors.push(format!("child {pid} was not reaped"));
            }
        }
        errors
    }
}
async fn run(
    protocol: &str,
    native: &str,
    late: bool,
    body: impl for<'a> FnOnce(&'a mut NativeCase) -> Pin<Box<dyn Future<Output = ()> + 'a>>,
) {
    let mut case = NativeCase::new(protocol, native, late).await;
    let result = AssertUnwindSafe(body(&mut case)).catch_unwind().await;
    let cleanup = AssertUnwindSafe(async {
        let failures = case.cleanup().await;
        assert!(failures.is_empty(), "cleanup failures: {failures:?}");
    }).catch_unwind().await;
    // Check cleanup on every exit; its panic hook reports any secondary failure
    // before the original test failure is resumed.
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
    if let Err(panic) = cleanup {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
async fn native_initial_close_then_exact_successor_blocks_overlap_and_joins_offer() {
    run("ready", "normal", false, |case| {
        Box::pin(async move {
            let session = case.fixture.register().await;
            let wrong = case.route(&session);
            let mut changed = case.fixture.server().clone();
            match &mut changed.transport {
                meerkat_core::mcp_config::McpTransportConfig::Stdio(config) => {
                    config.args.push("unselected".into())
                }
                _ => panic!("ordinary fixture is stdio"),
            }
            case.add_config(wrong, changed, ExternalToolDeltaPhase::Failed)
                .await;
            case.close(wrong).await;
            assert!(
                case.native_pids().is_empty(),
                "same-name descriptor drift never reaches spawn"
            );
            let first = case.route(&session);
            case.add(first, ExternalToolDeltaPhase::Applied).await;
            assert_eq!(case.native_pids().len(), 1);
            assert!(
                case.protocol.pids().is_empty(),
                "initial native transport needs no offer"
            );
            let overlap = case.route(&session);
            case.add(overlap, ExternalToolDeltaPhase::Failed).await;
            assert_eq!(
                case.native_pids().len(),
                1,
                "live guard blocks physical overlap"
            );
            case.close(overlap).await;
            let closed = case.close(first).await;
            case.assert_settled(closed);
            case.fixture.retire(&session).await;
            let next_session = case.fixture.register().await;
            assert_ne!(next_session, session);
            let next = case.route(&next_session);
            case.add(next, ExternalToolDeltaPhase::Applied).await;
            assert_eq!(case.native_pids().len(), 2);
            assert_eq!(case.protocol.pids().len(), 1);
            // No finish before native process end: the protocol child is still ours.
            assert!(
                !case
                    .protocol
                    .output
                    .join(format!("{}.ended", case.protocol.pids()[0]))
                    .exists()
            );
            let closed = case.close(next).await;
            case.assert_settled(closed);
            assert!(
                case.protocol
                    .output
                    .join(format!("{}.ended", case.protocol.pids()[0]))
                    .exists()
            );
            let owner = case
                .attempts
                .take_closed_owner()
                .expect("same owner returned after native finish");
            case.protocol.owner = Some(owner);
            let owner = case.protocol.owner.as_ref().unwrap();
            assert!(!owner.poisoned && !owner.has_pending_round());
            assert_eq!(owner.resources.root, case.protocol.selected_root);
        })
    })
    .await;
}

#[tokio::test]
async fn native_initial_failure_and_abnormal_close_poison_pristine_owner() {
    for mode in ["initialize_failure", "abnormal_exit"] {
        run("ready", mode, false, |case| {
            Box::pin(async move {
                let session = case.fixture.register().await;
                let first = case.route(&session);
                let expected = if fs::read_to_string(case.protocol.output.join("native-mode"))
                    .unwrap()
                    == "initialize_failure"
                {
                    ExternalToolDeltaPhase::Failed
                } else {
                    ExternalToolDeltaPhase::Applied
                };
                case.add(first, expected).await;
                case.close(first).await;
                assert!(case.protocol.pids().is_empty());
                let later = case.route(&session);
                case.add(later, ExternalToolDeltaPhase::Failed).await;
                assert_eq!(
                    case.native_pids().len(),
                    1,
                    "failed initial attempt cannot rearm by absence of an offer child"
                );
                case.close(later).await;
                let owner = case.attempts.take_closed_owner().unwrap();
                case.protocol.owner = Some(owner);
                let owner = case.protocol.owner.as_ref().unwrap();
                assert!(owner.poisoned && !owner.has_pending_round());
            })
        })
        .await;
    }
}

#[tokio::test]
async fn native_cancelled_successor_begin_joins_offer_without_second_transport() {
    run("hold", "normal", false, |case| {
        Box::pin(async move {
            let session = case.fixture.register().await;
            let first = case.route(&session);
            case.add(first, ExternalToolDeltaPhase::Applied).await;
            let closed = case.close(first).await;
            case.assert_settled(closed);
            let next = case.route(&session);
            case.add(next, ExternalToolDeltaPhase::Failed).await;
            case.close(next).await;
            assert_eq!(case.native_pids().len(), 1);
            assert_eq!(
                case.protocol.pids().len(),
                1,
                "cancelled begin really entered the offer process"
            );
            let owner = case
                .attempts
                .take_closed_owner()
                .expect("cancelled retained begin returned the owner");
            case.protocol.owner = Some(owner);
            let owner = case.protocol.owner.as_ref().unwrap();
            assert!(owner.poisoned && !owner.has_pending_round());
        })
    })
    .await;
}

#[tokio::test]
async fn native_late_ready_guard_is_cancelled_and_returned_without_rearming() {
    run("ready", "normal", true, |case| {
        Box::pin(async move {
            let session = case.fixture.register().await;
            let first = case.route(&session);
            case.add(first, ExternalToolDeltaPhase::Applied).await;
            let closed = case.close(first).await;
            case.assert_settled(closed);
            let next = case.route(&session);
            case.add(next, ExternalToolDeltaPhase::Failed).await;
            assert!(
                case.probe.held.load(Ordering::SeqCst),
                "real ready guard is retained inside Core begin"
            );
            assert_eq!(case.native_pids().len(), 1);
            assert_eq!(case.protocol.pids().len(), 1);
            let premature = case.attempts.take_closed_owner();
            let returned_early = premature.is_some();
            if let Some(owner) = premature {
                case.protocol.owner = Some(owner);
            }
            assert!(!returned_early, "fence does not steal active custody");
            case.probe.hold_second.send_replace(false);
            case.close(next).await;
            let owner = case
                .attempts
                .take_closed_owner()
                .expect("same late guard was cancelled and joined");
            case.protocol.owner = Some(owner);
            let owner = case.protocol.owner.as_ref().unwrap();
            assert!(owner.poisoned && !owner.has_pending_round());
        })
    })
    .await;
}

mod gateway_cleanup;
mod initial_cancel;

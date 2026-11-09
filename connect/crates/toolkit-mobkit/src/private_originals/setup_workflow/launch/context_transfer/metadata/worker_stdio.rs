//! Real stdio init/roster/Ready/shutdown over the existing installed recovery
//! owner. Envelopes and Uncertain outcomes are controlled setup; native inputs
//! are actually admitted and retired. Empty roster is not session resumption.
use super::super::{delivery::historical::tests as history_fixture, restore_tests as fixture};
use super::*;
use crate::{
    host_gateway::ToolkitGatewayHost,
    installed_host::acquisition::{
        InstalledNativeContinuation, InstalledSetupDefaults, InstalledSetupLimits,
        InstalledSetupSelection, installed_workflow_context_selector,
        recovery::AcquisitionRecovery,
    },
    private_originals::recovery_probe::MetadataRecoveryProbe,
};
use futures::FutureExt;
use meerkat_core::SessionId;
use meerkat_mobkit::{
    MobBootstrapSpec, UnifiedRuntime,
    rpc_gateway::{
        GatewayBootstrapReady, GatewayFactoryConstruction, GatewayHostCloseReason,
        GatewayHostComposition, GatewayHostError, GatewayMcpComposition,
        PreparedApplicationRequest, PreparedGatewayApplication,
    },
};
use meerkat_runtime::{RuntimeStore, meerkat_machine::NativeGrantWorkConfiguration};
use meerkat_toolkit::{authority::Ceiling, setup::WorkAdmissionStepOptions};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    panic::AssertUnwindSafe,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use toolkit_store::ToolkitStore;

mod live_interest;

const CHILD: &str = "TOOLKIT_CONTEXT_RECOVERY_STDIO_CHILD";
const TEST: &str = concat!(
    module_path!(),
    "::actual_stdio_ready_runs_retained_metadata_page_and_joins_shutdown"
);
const WAIT: Duration = Duration::from_secs(60);

struct Host {
    gateway: ToolkitGatewayHost,
    continuation: Arc<InstalledNativeContinuation>,
    ready: AtomicBool,
    closing: AtomicBool,
    model: Option<Arc<dyn meerkat_llm_core::LlmClient>>,
    tool_probe: Option<Arc<crate::installed_host::acquisition::ContextProbe>>,
    definition: Option<meerkat_mob::MobDefinition>,
}
#[async_trait::async_trait]
impl GatewayHostComposition for Host {
    fn host_app_routes(
        &self,
        viewer: meerkat_mobkit::http_console::ConsoleViewerResolver,
    ) -> Result<Option<axum::Router>, GatewayHostError> {
        self.gateway.host_app_routes(viewer)
    }
    fn compose_member_factory(
        &self,
        construction: &GatewayFactoryConstruction<'_>,
    ) -> Result<Option<GatewayMcpComposition>, GatewayHostError> {
        self.gateway.compose_member_factory(construction)
    }
    async fn prepared_application(
        &self,
        request: Option<&PreparedApplicationRequest>,
    ) -> Result<Option<PreparedGatewayApplication>, GatewayHostError> {
        self.gateway.prepared_application(request).await
    }

    async fn prepare_bootstrap(
        &self,
        mut spec: MobBootstrapSpec,
    ) -> Result<MobBootstrapSpec, GatewayHostError> {
        if let Some(definition) = &self.definition {
            assert_eq!(
                &spec.definition, definition,
                "normal init must select the exact seed definition"
            );
            spec.options.default_llm_client = self.model.clone();
            spec = spec.with_child_mob_storage(meerkat_mobkit::ChildMobStorage::ProcessBound);
            let probe = self
                .tool_probe
                .clone()
                .expect("same genuine seed tool surface");
            spec = spec.with_default_external_tools_provider(Arc::new(move || {
                Some(probe.clone() as Arc<dyn meerkat_core::AgentToolDispatcher>)
            }));
        }
        // This is the actual source installed before restoration. The public
        // method refuses a spec lacking Meta's own runtime-delivery owner.
        let source = meerkat_mob_mcp::HostJobSource::new(
            [self.continuation.namespace().to_owned()],
            self.continuation.clone(),
        )
        .map_err(|_| GatewayHostError::Denied)?;
        let spec = spec
            .with_host_job_source(source)
            .map_err(|_| GatewayHostError::Unavailable)?;
        self.gateway.prepare_bootstrap(spec).await
    }
    fn native_grant_work_configuration(
        &self,
    ) -> Result<Option<NativeGrantWorkConfiguration>, GatewayHostError> {
        self.gateway.native_grant_work_configuration()
    }
    async fn bind_runtime(&self, runtime: &mut UnifiedRuntime) -> Result<(), GatewayHostError> {
        self.gateway.bind_runtime(runtime).await?;
        self.continuation
            .bind_native(runtime.mob_runtime())
            .map_err(|_| GatewayHostError::Unavailable)
    }
    async fn bind_identity_runtime(
        &self,
        runtime: &mut UnifiedRuntime,
    ) -> Result<(), GatewayHostError> {
        self.gateway.bind_identity_runtime(runtime).await
    }
    fn publish_bootstrap_ready(
        &self,
        ready: GatewayBootstrapReady<'_>,
    ) -> Result<(), GatewayHostError> {
        // Only the native driver constructs this token. Toolkit publishes it
        // under its existing close boundary, then starts its retained worker.
        self.gateway.publish_bootstrap_ready(ready)?;
        self.ready.store(true, Ordering::SeqCst);
        Ok(())
    }
    fn begin_close(&self, reason: GatewayHostCloseReason) -> Result<(), GatewayHostError> {
        self.gateway.begin_close(reason)?;
        self.closing.store(true, Ordering::SeqCst);
        Ok(())
    }
    async fn join_close(&self) -> Result<(), GatewayHostError> {
        self.gateway.join_close().await
    }
    async fn wait_closed(&self) {
        self.gateway.wait_closed().await
    }
}

async fn until(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(WAIT, async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("bounded actual owner observation");
}
async fn persisted(directory: &Path, sessions: &[SessionId]) -> Value {
    use meerkat::SessionStore;
    let layout = meerkat_mobkit::storage_layout::MetaStorageLayout::standalone(
        directory.to_path_buf(),
        directory.join("gateway-home"),
    );
    // The gateway's head-canonical pair on runtime.sqlite: the runtime store
    // holds each session's inputs and catalog, and the session store on the
    // same file holds its committed transcript.
    let runtime =
        meerkat_runtime::store::SqliteRuntimeStore::new_head_canonical(layout.runtime_db())
            .unwrap();
    let committed = meerkat::SqliteSessionStore::open(layout.runtime_db()).unwrap();
    let mut records = Vec::new();
    for session in sessions {
        let inputs = runtime
            .load_input_states_strict(&meerkat_runtime::LogicalRuntimeId::for_session(session))
            .await
            .unwrap();
        // Read the actual committed transcript of the original session.
        let original = committed
            .load(session)
            .await
            .unwrap()
            .expect("original transcript remains in its selected session store");
        assert_eq!(original.id(), session);
        records.push(json!({"inputs": inputs.iter().map(|row| json!({
            "id": row.state.input_id, "history": row.state.history,
            "authority": row.state.authority_contributors, "key": row.state.idempotency_key,
            "body_retained": row.state.persisted_input.is_some(), "phase": row.seed.phase,
            "admission_sequence": row.seed.admission_sequence, "attempts": row.seed.attempt_count,
        })).collect::<Vec<_>>(), "session": original}));
    }
    let catalog = runtime
        .list_runtime_session_catalog_entries(meerkat_core::SessionFilter {
            limit: Some(64),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(
        catalog.len() < 64,
        "complete bounded runtime session census"
    );
    let mut ids = catalog
        .iter()
        .map(|row| row.session_id().to_string())
        .collect::<Vec<_>>();
    ids.sort();
    json!({"records": records, "runtime_sessions": ids})
}
// Controlled unselected source records exercise the real canonical cursor.
// No filler is submitted, given a workflow marker or made into native history.
async fn fill_between(
    c: &fixture::Fixture,
    low: &toolkit_store::StoreKey,
    high: &toolkit_store::StoreKey,
) {
    assert!(low < high);
    let mut retained = 0;
    for index in 0..100_000 {
        let mut event =
            crate::original_ingress::tests::private_event(&format!("metadata-page-filler-{index}"));
        event.route = c.f.original_reference().route.clone();
        let reference = crate::private_originals::PrivateOriginalRef {
            route: event.route.clone(),
            original: event.original.clone(),
            event_sha256: digest(&crate::private_originals::canonical_json(&event).unwrap()),
        };
        let key = c.f.source.key(&reference).unwrap();
        if low < &key && &key < high {
            assert!(c.f.source.retain_event(event).await.unwrap() == reference);
            let row = c.f.store.get(&key).await.unwrap().unwrap();
            let record = c.f.source.decode_stored(&row).unwrap();
            assert!(
                record.planning_source_evidence.is_none() && record.final_association.is_none()
            );
            retained += 1;
            if retained == 32 {
                break;
            }
        }
    }
    assert_eq!(retained, 32, "bounded deterministic filler selection");
    let page =
        c.f.source
            .workflow_close_recovery_page(None, 32)
            .await
            .unwrap();
    assert!(
        page.sources.iter().any(|row| &row.key == low),
        "held source is in actual first page"
    );
    assert!(
        page.sources.iter().all(|row| &row.key != high),
        "actual second source is beyond first page"
    );
    assert!(page.next_after_id.as_ref().is_some_and(|id| id < &high.id));
}
struct Release(Arc<MetadataRecoveryProbe>);
impl Drop for Release {
    fn drop(&mut self) {
        self.0.release();
    }
}

async fn child() {
    let c = fixture::Fixture::persistent_configured_waiter_for_gateway().await;
    let lifecycle = AcquisitionRecovery::default();
    let old_source = Arc::downgrade(&c.f.source);
    let old_machine = Arc::downgrade(&c.f.machine);
    let seeded = AssertUnwindSafe(async {
        let data = fixture::prepare(&c).await;
        let first = history_fixture::restored(&c, &data, &lifecycle).await;
        let (_, a_admitted) = history_fixture::admitted_but_unacknowledged(&c, &first).await;
        let other = super::tests::second_source(&c, &data.planning).await;
        let second = history_fixture::restored(&c, &other, &lifecycle).await;
        let (_, b_admitted) = history_fixture::admitted_but_unacknowledged(&c, &second).await;
        assert_ne!(a_admitted.input_id, b_admitted.input_id);
        let a = data.envelope.membership.source().clone();
        let b = other.envelope.membership.source().clone();
        let (first_source, second_source, first_key, second_key, first_admitted, second_admitted) =
            if a < b {
                (
                    a,
                    b,
                    data.envelope.membership.delivery_effect().unwrap(),
                    other.envelope.membership.delivery_effect().unwrap(),
                    a_admitted,
                    b_admitted,
                )
            } else {
                (
                    b,
                    a,
                    other.envelope.membership.delivery_effect().unwrap(),
                    data.envelope.membership.delivery_effect().unwrap(),
                    b_admitted,
                    a_admitted,
                )
            };
        let filler_rows_before = fixture::native_rows(&c).await;
        let filler_requests_before = c.f.model_requests();
        fill_between(&c, &first_source, &second_source).await;
        assert_eq!(fixture::native_rows(&c).await, filler_rows_before);
        assert_eq!(c.f.model_requests(), filler_requests_before);
        for membership in [
            &data.envelope.membership,
            &data.sibling,
            &other.envelope.membership,
        ] {
            data.planning.depart_waiter(membership).await.unwrap();
        }
        let runtime = c.f.source.bound_identity_runtime().unwrap();
        let identity = runtime
            .owned_identity_for_member_alias(&c.terms.lead.member)
            .await
            .unwrap();
        runtime.retire(&identity).await.unwrap();
        assert!(
            c.f.machine
                .current_session_registration_witness(&c.lead_session)
                .await
                .is_none()
        );
        let retired = c
            .history
            .as_ref()
            .unwrap()
            .load_input_states_strict(&meerkat_runtime::LogicalRuntimeId::for_session(
                &c.lead_session,
            ))
            .await
            .unwrap();
        for input in [&first_admitted.input_id, &second_admitted.input_id] {
            let row = retired
                .iter()
                .find(|row| &row.state.input_id == input)
                .expect("actual admitted row");
            assert!(
                row.state.persisted_input.is_none(),
                "query must use genuinely retired native history"
            );
        }
        let first_before = c.f.store.get_effect(&first_key).await.unwrap().unwrap();
        let second_before = c.f.store.get_effect(&second_key).await.unwrap().unwrap();
        assert_eq!(first_before.status, EffectStatus::Uncertain);
        assert_eq!(second_before.status, EffectStatus::Uncertain);
        (
            c.durable_state(),
            first_source,
            first_key,
            second_key,
            first_before,
            second_before,
            first_admitted,
            second_admitted,
        )
    })
    .catch_unwind()
    .await;
    lifecycle.stop();
    let transfer_join = c.f.source.join_workflow_context_transfers(false).await;
    let closed = c.close().await;
    let cleanup = std::panic::catch_unwind(|| {
        assert!(
            transfer_join.is_ok() && closed.is_ok(),
            "initial owners joined: {transfer_join:?}/{closed:?}"
        )
    });
    drop(c);
    let (
        saved,
        first_source,
        first_key,
        second_key,
        first_before,
        second_before,
        first_admitted,
        second_admitted,
    ) = match seeded {
        Ok(value) => {
            if let Err(panic) = cleanup {
                std::panic::resume_unwind(panic);
            }
            value
        }
        Err(panic) => std::panic::resume_unwind(panic),
    };
    assert!(old_source.upgrade().is_none() && old_machine.upgrade().is_none());
    let mut reopened = saved.reopen_stdio_host().await;
    let directory = reopened.directory.clone();
    let sessions = saved.sessions().to_vec();
    let before = persisted(directory.path(), &sessions).await;
    let originals = reopened.originals.clone();
    let probe = Arc::new(MetadataRecoveryProbe::new(first_source));
    assert!(originals.metadata_probe.set(probe.clone()).is_ok());
    let _release = Release(probe.clone());
    let (host, _private_open) = recovery_host(&mut reopened, &saved);
    let host = Arc::new(host);
    assert!(!host.ready.load(Ordering::SeqCst));
    assert_eq!(probe.reads.load(Ordering::SeqCst), 0);
    // Parent supplies these exact paths through normal init. No rows are copied
    // into the gateway databases after admission, and no fallback is selected.
    println!(
        "{}",
        json!({"fixture": "prepared", "state": directory.path()})
    );
    let observed_host = host.clone();
    let observed_probe = probe.clone();
    let store = reopened.store.clone();
    let observer = tokio::spawn(async move {
        let _release = Release(observed_probe.clone());
        let checked = AssertUnwindSafe(async {
            until(|| observed_host.ready.load(Ordering::SeqCst)).await;
            until(|| {
                observed_probe.reads.load(Ordering::SeqCst) == 1
                    && observed_probe.polls.load(Ordering::SeqCst) >= 2
            })
            .await;
            let applied = tokio::time::timeout(WAIT, async {
                loop {
                    let effect = store.get_effect(&second_key).await.unwrap().unwrap();
                    if effect.status == EffectStatus::Succeeded {
                        break effect;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("second actual selected source progresses under the real worker");
            assert_eq!(applied.attempt, second_before.attempt);
            assert_eq!(applied.intent, second_before.intent);
            let Some(EffectOutcome::Succeeded { receipt }) = applied.outcome else {
                panic!("Applied")
            };
            assert_eq!(
                receipt.body["input"],
                serde_json::to_value(second_admitted.input_id).unwrap()
            );
            assert_eq!(
                receipt.body["session"],
                serde_json::to_value(second_admitted.session_id).unwrap()
            );
            assert_eq!(
                store.get_effect(&first_key).await.unwrap().unwrap(),
                first_before
            );
            until(|| observed_probe.wraps.load(Ordering::SeqCst) >= 2).await;
            assert!(observed_probe.pages.load(Ordering::SeqCst) > 1);
            assert_eq!(
                observed_probe.reads.load(Ordering::SeqCst),
                1,
                "cursor wrap cannot double-enter the physically retained source"
            );
            // Hold the exact post-poll boundary before canonical page entry.
            // The real parent sends shutdown only after this owner is held.
            observed_probe.arm_page_boundary();
            until(|| observed_probe.page_boundary.load(Ordering::SeqCst)).await;
            let pages = observed_probe.pages.load(Ordering::SeqCst);
            println!(
                "{}",
                json!({"fixture": "fairness", "controlled_fillers": 32})
            );
            until(|| observed_host.closing.load(Ordering::SeqCst)).await;
            assert!(!observed_probe.drain.load(Ordering::SeqCst));
            assert_eq!(observed_probe.pages.load(Ordering::SeqCst), pages);
            observed_probe.release_page_boundary();
            until(|| observed_probe.drain.load(Ordering::SeqCst)).await;
            assert_eq!(
                observed_probe.pages.load(Ordering::SeqCst),
                pages,
                "stop after retained poll must prevent a new canonical read before drain"
            );
            assert!(
                tokio::time::timeout(Duration::from_millis(600), observed_host.join_close())
                    .await
                    .is_err(),
                "cancelled join observer cannot discard the real entered metadata future"
            );
            assert_eq!(observed_probe.pages.load(Ordering::SeqCst), pages);
            assert_eq!(observed_probe.reads.load(Ordering::SeqCst), 1);
            observed_probe.release();
            tokio::time::timeout(WAIT, observed_host.join_close())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                store.get_effect(&first_key).await.unwrap().unwrap(),
                first_before,
                "stopped pre-query source remains Uncertain, without resubmit authority"
            );
            assert_eq!(observed_probe.pages.load(Ordering::SeqCst), pages);
            assert_eq!(observed_probe.reads.load(Ordering::SeqCst), 1);
            let _ = first_admitted; // Its exact row is in the unchanged native census.
        })
        .catch_unwind()
        .await;
        if checked.is_err() {
            let _ = observed_host.begin_close(GatewayHostCloseReason::Shutdown);
        }
        checked
    });
    let outcome = meerkat_mobkit::rpc_gateway::run_persistent_with_host(None, host.clone()).await;
    probe.release();
    let checked = observer.await.expect("retained verifier task joins");
    let cleanup = AssertUnwindSafe(async {
        assert!(outcome.is_ok(), "actual stdio native cleanup: {outcome:?}");
        host.join_close().await.unwrap();
        assert_eq!(persisted(directory.path(), &sessions).await, before,
            "same originally selected runtime inputs and original transcript, no replay or new input");
        assert!(originals.workflow_context_transfers().unwrap().is_empty());
        assert!(originals.retained.lock().unwrap().is_empty());
    }).catch_unwind().await;
    if let Err(panic) = checked {
        std::panic::resume_unwind(panic);
    }
    if let Err(panic) = cleanup {
        std::panic::resume_unwind(panic);
    }
    println!("{}", json!({"fixture": "joined"}));
}

#[test]
fn actual_stdio_ready_runs_retained_metadata_page_and_joins_shutdown() {
    if std::env::var_os(CHILD).is_some() {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .unwrap()
            .block_on(child());
        return;
    }
    run_parent(TEST);
}
fn run_parent(test: &str) {
    use std::{
        process::{Command, Stdio},
        sync::mpsc,
    };
    let dir = tempfile::tempdir().unwrap();
    // libtest selectors omit the crate prefix from module_path!().
    let exact = test.split_once("::").unwrap().1;
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", exact, "--nocapture"])
        .env(CHILD, "1")
        .env("OPENAI_API_KEY", "synthetic-unused-stdio-fixture")
        .env("XDG_STATE_HOME", dir.path().join("xdg-state"))
        .current_dir(dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let (sender, lines) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if sender.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let errors = std::thread::spawn(move || {
        let mut text = String::new();
        BufReader::new(stderr).read_to_string(&mut text).unwrap();
        text
    });
    let mut stdin = child.stdin.take().unwrap();
    let mut send = |value: Value| {
        writeln!(stdin, "{value}").unwrap();
        stdin.flush().unwrap();
    };
    let mut ready = false;
    let mut fair = false;
    let mut joined = false;
    let mut shutdown = false;
    let mut shutdown_settled = false;
    let mut roster_calls = 0;
    let mut roster = json!([]);
    let mut seen = Vec::new();
    let body = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let deadline = Instant::now() + Duration::from_secs(180);
        while !joined {
            let line = lines
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("bounded real child protocol");
            let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            if message["fixture"] == "prepared" {
                roster = message.get("roster").cloned().unwrap_or_else(|| json!([]));
                let config = message.get("mob_config").and_then(Value::as_str).unwrap_or(
                    "[mob]\nid = \"factory\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n",
                );
                send(
                    json!({"jsonrpc":"2.0", "id":"init", "method":"meta/init", "params": {
                        "init_protocol":"accepted_then_settled", "init_id":"toolkit-metadata-worker",
                        "mob_config":config,
                        "persistent_state":message["state"], "has_roster_provider":true,
                        "runtime_options":{"demo_llm":true}
                    }}),
                );
            }
            if message["method"] == "callback/roster_provider/roster" {
                roster_calls += 1;
                send(json!({"jsonrpc":"2.0", "id":message["id"], "result":roster}));
            } else if message.get("method").is_some() && message.get("id").is_some() {
                send(
                    json!({"jsonrpc":"2.0", "id":message["id"], "error":{"code":-32601,"message":"no extra fixture callback"}}),
                );
            }
            if message["method"] == "meta/init_settled" {
                assert_eq!(message["params"]["outcome"], "ready", "{seen:?}");
                ready = true;
            }
            if message["fixture"] == "fairness" {
                fair = true;
            }
            if ready && fair && !shutdown {
                send(
                    json!({"jsonrpc":"2.0", "id":"shutdown", "method":"meta/shutdown", "params":{}}),
                );
                shutdown = true;
            }
            if message["id"] == "shutdown" {
                assert_eq!(message["result"]["shutdown"], true);
                assert_eq!(message["result"]["runtime_cleanup_completed"], true);
                assert_eq!(message["result"]["host_cleanup_completed"], true);
                shutdown_settled = true;
            }
            joined |= message["fixture"] == "joined";
            seen.push(message);
        }
        assert!(ready && fair && shutdown && shutdown_settled && roster_calls > 0);
    }));
    drop(stdin);
    let deadline = Instant::now() + WAIT;
    while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let exited = child.try_wait().unwrap().is_some();
    if !exited {
        let _ = child.kill();
    }
    let status = child.wait().unwrap();
    let read_join = reader.join();
    let error_join = errors.join();
    let cleanup = std::panic::catch_unwind(AssertUnwindSafe(|| {
        assert!(read_join.is_ok(), "stdout reader joined");
        let errors = error_join.expect("stderr reader joined");
        assert!(
            exited && status.success(),
            "real child settled successfully: {status}; {errors}; {seen:?}"
        );
    }));
    if let Err(panic) = body {
        std::panic::resume_unwind(panic);
    }
    if let Err(panic) = cleanup {
        std::panic::resume_unwind(panic);
    }
}

fn recovery_host(
    reopened: &mut crate::installed_host::controller_first_input_fixture::DedicatedHost,
    saved: &crate::private_originals::governed_submit::tests::first_input_tests::dedicated::DurableDedicated,
) -> (Host, tokio::sync::watch::Sender<bool>) {
    let graph = WorkGraphService::with_scope(
        Arc::new(
            meerkat_workgraph::SqliteWorkGraphStore::open(
                reopened.source_path.with_extension("dedicated-work.sqlite"),
            )
            .unwrap(),
        ),
        reopened.source_scope.realm.clone(),
        meerkat_workgraph::WorkNamespace::new("dedicated-workflow").unwrap(),
    );
    let backend = SetupWorkBackend::new(
        graph,
        "dedicated-sqlite".into(),
        Id::new("one").unwrap(),
        &[meerkat_core::DurabilityDeclaration::durable(
            "workgraph",
            meerkat_core::DurabilityResolution::Persistent,
        )],
    )
    .unwrap();
    assert_eq!(backend.pin(), &saved.terms().backend);
    let native = reopened
        .lifetime
        .selected_policy()
        .native_host
        .as_ref()
        .unwrap();
    let lead = native
        .members
        .iter()
        .find(|row| row.member == saved.terms().lead)
        .unwrap();
    let route = native
        .routes
        .iter()
        .find(|route| route.id == saved.lead_source().route)
        .unwrap();
    assert!(
        route
            .source_binding(reopened.lifetime.selected_policy())
            .unwrap()
            .0
            == &lead.member
    );
    // Same explicit helper profile selected by the configured fixture. It has
    // no operation targets and creates no grants or source custody.
    let helper = InstalledSetupSelection { catalog_capability: None, report_capability: None,
        member: lead.member.clone(), ingress_actor: native.grant_administrator.clone(),
        logical_executor: route.logical_executor.clone(), represented_subject: route.represented_subject.clone(),
        controller_parent: route.controller_parent.clone().unwrap(),
        operation_basis: meerkat_authorization_contracts::work_association::WorkAuthorityBasis::TargetedGrantLineages {
            base: None, targets: vec![],
        },
    };
    let audiences = [Id::new("donor").unwrap()].into();
    let selector = installed_workflow_context_selector(
        native.realm.clone(),
        Id::new("one").unwrap(),
        native
            .members
            .iter()
            .map(|row| row.member.clone())
            .collect(),
        audiences,
    );
    let installed = reopened.installed.take().unwrap();
    let continuation = installed
        .native_continuation([Id::new("donor").unwrap()].into())
        .unwrap();
    let installed = installed
        .with_default_acquisition(InstalledSetupDefaults {
            helper,
            proposal_ceiling: Ceiling {
                actions: Default::default(),
                services: Default::default(),
                frontends: Default::default(),
                output_audiences: [Id::new("donor").unwrap()].into(),
            },
            continuation: continuation.clone(),
            work_backend: backend,
            references: None,
            limits: InstalledSetupLimits::default(),
            work_step: WorkAdmissionStepOptions {
                worker: Id::new("metadata-stdio").unwrap(),
                lease_ms: 30_000,
                owner_timeout_ms: 10_000,
                settlement_budget_ms: 5_000,
            },
        })
        .unwrap()
        .with_workflow_context_recovery(selector)
        .unwrap();
    let (_private_open, private_closed) = tokio::sync::watch::channel(false);
    let host = Host {
        gateway: installed.compose(
            reopened.lifetime.clone(),
            crate::private_host::PrivateHostClosed(private_closed),
        ),
        continuation,
        ready: AtomicBool::new(false),
        closing: AtomicBool::new(false),
        model: None,
        tool_probe: None,
        definition: None,
    };
    (host, _private_open)
}

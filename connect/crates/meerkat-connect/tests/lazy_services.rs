//! Lazy activation through the actual public composition and native owners.
//! The tiny stdio peer has no accounts, install operations or worker children.

use std::{
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use futures_util::{FutureExt, Stream};
use meerkat::{LlmClient, LlmError, LlmEvent, LlmRequest};
use meerkat_connect::{
    ConnectSession, RealmOptions, SessionOptions,
    approval::{ApprovalPresenter, LocalApprovals},
    paired_host_tool::PairedHostFrontend,
    service_preparation::{LazySetup, ServicePreparationPresenter, TOOLKIT_PREPARE_SERVICES},
    setup::PreparedSetupConnection,
};
use meerkat_core::{
    AgentToolDispatcher, Config, Message, Provider, SessionId, ToolError, ToolExecutionPolicy,
    ToolMutationClass,
    agent::ToolDispatchContext,
    ops::{ToolAccessPolicy, ToolDispatchOutcome},
    tool_execution_policy::ExecutionPolicyGatedDispatcher,
    types::ToolCallView,
};
use meerkat_runtime::completion::CompletionOutcome;
use nix::{
    errno::Errno,
    sys::signal::{Signal, kill, killpg},
    unistd::Pid,
};
use serde_json::{Value, json};
use tokio::time::{sleep, timeout};

const BOUND: Duration = Duration::from_secs(8);
type Gated = ExecutionPolicyGatedDispatcher<dyn AgentToolDispatcher>;

struct Gate {
    _directory: tempfile::TempDir,
    approvals: LocalApprovals,
    _decisions: ApprovalPresenter,
    dispatcher: Arc<Gated>,
    raw: Arc<dyn AgentToolDispatcher>,
    context: ToolDispatchContext,
}
impl Gate {
    fn new(lazy: &LazySetup, policy: Option<ToolAccessPolicy>) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let (approvals, decisions) = LocalApprovals::create_paired_host(directory.path()).unwrap();
        let mut input = SessionOptions::default();
        input.build.tool_access_policy = policy;
        let mut options = lazy.session_options(input).unwrap();
        assert!(options.build.mcp_servers.is_empty());
        approvals.attach_lazy_setup(&mut options, lazy).unwrap();
        let session = SessionId::new();
        approvals.bind_session(&session).unwrap();
        let raw = options.build.external_tools.unwrap();
        let dispatcher = Arc::new(
            ExecutionPolicyGatedDispatcher::new(
                raw.clone(),
                ToolExecutionPolicy::resolve(options.build.tool_access_policy.unwrap()).unwrap(),
            )
            .with_dispatch_admission(options.build.tool_dispatch_admission.unwrap()),
        );
        Self {
            _directory: directory,
            approvals,
            _decisions: decisions,
            dispatcher,
            raw,
            context: ToolDispatchContext::default().with_runtime_identity(session, None),
        }
    }
    async fn call(
        &self,
        id: &str,
        name: &str,
        args: Value,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        call(self.dispatcher.as_ref(), &self.context, id, name, args).await
    }
}
async fn call(
    dispatcher: &impl AgentToolDispatcher,
    context: &ToolDispatchContext,
    id: &str,
    name: &str,
    args: Value,
) -> Result<ToolDispatchOutcome, ToolError> {
    let args = serde_json::value::to_raw_value(&args).unwrap();
    dispatcher
        .dispatch_with_context(
            ToolCallView {
                id,
                name,
                args: &args,
            },
            context,
        )
        .await
}
async fn no_request(presenter: &mut ServicePreparationPresenter) {
    assert!(
        timeout(Duration::from_millis(20), presenter.next())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn recipe_discovery_is_read_only_before_activation_and_keeps_caller_session_policy() {
    use toolkit_package::{LIST_RECIPES, READ_RECIPE};
    let (paired_host, mut hosts) = PairedHostFrontend::deferred();
    let (lazy, mut preparation) = LazySetup::new(paired_host);
    let allowed = Gate::new(&lazy, Some(ToolAccessPolicy::ReadOnly));
    let page = allowed.call("recipes", LIST_RECIPES, json!({"limit":1})).await.unwrap();
    assert!(page.async_ops.is_empty() && page.session_effects.is_empty());
    let page: Value = serde_json::from_str(&page.result.text_content()).unwrap();
    let first = &page["entries"][0];
    let exact = json!({"identity":first["identity"],"digest":first["digest"]});
    let entry = allowed.call("recipe", READ_RECIPE, exact.clone()).await.unwrap();
    assert!(entry.async_ops.is_empty() && entry.session_effects.is_empty());
    let catalog = toolkit_package::bundled_recipe_catalog().unwrap();
    let identity = serde_json::from_value(first["identity"].clone()).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&entry.result.text_content()).unwrap(),
        serde_json::to_value(catalog.get(&identity).unwrap()).unwrap());
    let denied = Gate::new(&lazy, Some(ToolAccessPolicy::DenyList(
        [LIST_RECIPES, READ_RECIPE].into_iter().collect(),
    )));
    let calls = [(LIST_RECIPES, json!({"limit":1})), (READ_RECIPE, exact)];
    for (name, args) in &calls {
        assert_eq!(allowed.raw.tool_mutation_class(name), ToolMutationClass::ReadOnly);
        assert!(matches!(denied.call("denied", name, args.clone()).await, Err(ToolError::AccessDenied { .. })));
        let wrong = ToolDispatchContext::default().with_runtime_identity(SessionId::new(), None);
        assert!(matches!(call(allowed.dispatcher.as_ref(), &wrong, "wrong-session", name, args.clone()).await,
            Err(ToolError::AccessDenied { .. })));
    }
    allowed.approvals.retire();
    for (name, args) in calls {
        assert!(matches!(allowed.call("closed", name, args).await, Err(ToolError::AccessDenied { .. })));
    }
    assert!(allowed.raw.tool_catalog_capabilities().exact_catalog);
    for name in ["setup_help", "setup_install", "find_and_install"] {
        assert!(!allowed.raw.tools().iter().any(|tool| tool.name == name));
    }
    no_request(&mut preparation).await;
    assert!(!preparation.requires_shutdown());
    assert!(timeout(Duration::from_millis(20), hosts.next()).await.is_err());
}

#[tokio::test]
async fn public_document_refusal_keeps_native_caller_policy_before_any_connection() {
    let name = "read_public_document";
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let args = json!({"url":format!("https://{}/guide?lang=en#part", listener.local_addr().unwrap())});
    let (paired_host, mut hosts) = PairedHostFrontend::deferred();
    let (lazy, mut preparation) = LazySetup::new(paired_host);
    let allowed = Gate::new(&lazy, Some(ToolAccessPolicy::ReadOnly));
    assert_eq!(allowed.raw.tool_mutation_class(name), ToolMutationClass::ReadOnly);
    let result = allowed.call("public-read", name, args.clone()).await.unwrap();
    assert!(result.async_ops.is_empty() && result.session_effects.is_empty());
    let result: Value = serde_json::from_str(&result.result.text_content()).unwrap();
    assert_eq!(result["error"], "destination_refused");
    assert!(result.get("text").is_none());
    let denied = Gate::new(&lazy, Some(ToolAccessPolicy::DenyList([name].into_iter().collect())));
    assert!(matches!(denied.call("denied", name, args.clone()).await, Err(ToolError::AccessDenied { .. })));
    for context in [
        ToolDispatchContext::default(),
        ToolDispatchContext::default().with_runtime_identity(SessionId::new(), None),
    ] {
        assert!(matches!(call(allowed.dispatcher.as_ref(), &context, "other", name, args.clone()).await,
            Err(ToolError::AccessDenied { .. })));
    }
    allowed.approvals.retire();
    assert!(matches!(allowed.call("closed", name, args).await, Err(ToolError::AccessDenied { .. })));
    assert!(timeout(Duration::from_millis(20), listener.accept()).await.is_err());
    no_request(&mut preparation).await;
    assert!(!preparation.requires_shutdown());
    assert!(timeout(Duration::from_millis(20), hosts.next()).await.is_err());
}

#[tokio::test]
async fn lazy_activation_preserves_policy_session_and_closed_arguments() {
    let (paired_host, _hosts) = PairedHostFrontend::deferred();
    let (lazy, mut presenter) = LazySetup::new(paired_host);
    let denied = Gate::new(&lazy, Some(ToolAccessPolicy::ReadOnly));
    let capabilities = denied.raw.tool_catalog_capabilities();
    assert!(capabilities.exact_catalog);
    assert!(capabilities.may_require_catalog_control_plane);
    let visible: Vec<_> = denied
        .raw
        .tools()
        .iter()
        .map(|tool| tool.name.clone())
        .collect();
    let catalog: Vec<_> = denied
        .raw
        .tool_catalog()
        .iter()
        .map(|entry| entry.tool.name.clone())
        .collect();
    assert_eq!(
        catalog, visible,
        "all initial tools are inline; no catalog entry may be hidden or invented"
    );
    assert_eq!(
        denied.raw.tool_mutation_class(TOOLKIT_PREPARE_SERVICES),
        ToolMutationClass::Mutating
    );
    assert!(
        !denied
            .raw
            .tools()
            .iter()
            .any(|tool| tool.name == "setup_help")
    );
    assert!(
        denied
            .call("readonly", TOOLKIT_PREPARE_SERVICES, json!({}))
            .await
            .is_err()
    );
    no_request(&mut presenter).await;
    assert!(
        !presenter.requires_shutdown(),
        "policy refusal must precede activation"
    );

    let allowed = Gate::new(&lazy, None);
    let wrong = ToolDispatchContext::default().with_runtime_identity(SessionId::new(), None);
    assert!(
        call(
            allowed.dispatcher.as_ref(),
            &wrong,
            "wrong-session",
            TOOLKIT_PREPARE_SERVICES,
            json!({})
        )
        .await
        .is_err()
    );
    for (index, args) in [
        json!({"command":"/not-authorized"}),
        json!({"account":"other"}),
        json!([]),
        Value::Null,
    ]
    .into_iter()
    .enumerate()
    {
        assert!(
            allowed
                .call(&format!("bad-{index}"), TOOLKIT_PREPARE_SERVICES, args)
                .await
                .is_err()
        );
    }
    for name in [
        "setup_help",
        "setup_install",
        "tool_catalog_search",
        "tool_catalog_load",
    ] {
        assert!(
            allowed
                .call("not-activated", name, json!({}))
                .await
                .is_err()
        );
    }
    no_request(&mut presenter).await;
    assert!(!presenter.requires_shutdown());
    assert!(!allowed.approvals.has_uncertain_dispatch());
}

#[tokio::test]
async fn pending_activation_rejects_duplicates_and_both_drop_directions() {
    for drop_waiter in [true, false] {
        let (paired_host, _hosts) = PairedHostFrontend::deferred();
        let (lazy, mut presenter) = LazySetup::new(paired_host);
        let gate = Gate::new(&lazy, None);
        let mut activation = Box::pin(gate.call("activate", TOOLKIT_PREPARE_SERVICES, json!({})));
        assert!(activation.as_mut().now_or_never().is_none());
        let mut request = timeout(BOUND, presenter.next()).await.unwrap().unwrap();
        assert!(
            gate.call("duplicate", TOOLKIT_PREPARE_SERVICES, json!({}))
                .await
                .is_err()
        );
        if drop_waiter {
            drop(activation);
            timeout(BOUND, request.closed()).await.unwrap();
            drop(request);
        } else {
            drop(request);
            assert!(timeout(BOUND, activation).await.unwrap().is_err());
        }
        assert!(presenter.requires_shutdown());
        assert!(
            gate.call("retry", TOOLKIT_PREPARE_SERVICES, json!({}))
                .await
                .is_err()
        );
        for name in ["setup_help", "tool_catalog_search", "tool_catalog_load"] {
            assert!(gate.call("late-service", name, json!({})).await.is_err());
        }
        no_request(&mut presenter).await;
    }
}

#[tokio::test]
async fn prepared_transfer_cannot_outlive_either_unacknowledged_party() {
    for drop_waiter in [true, false] {
        let (paired_host, _hosts) = PairedHostFrontend::deferred();
        let (lazy, mut presenter) = LazySetup::new(paired_host);
        let gate = Gate::new(&lazy, None);
        let peer = Peer::new();
        let mut activation = Box::pin(gate.call("activate", TOOLKIT_PREPARE_SERVICES, json!({})));
        assert!(activation.as_mut().now_or_never().is_none());
        let request = timeout(BOUND, presenter.next()).await.unwrap().unwrap();
        let mut handoff = Box::pin(request.attach_prepared(peer.connection()));
        // Sends typed preparation, but the native activation is deliberately
        // unpolled. No stage/add has happened and no timer is used to claim it.
        assert!(handoff.as_mut().now_or_never().is_none());
        if drop_waiter {
            drop(activation);
            assert!(timeout(BOUND, handoff).await.unwrap().is_err());
        } else {
            drop(handoff);
            assert!(timeout(BOUND, activation).await.unwrap().is_err());
        }
        assert!(presenter.requires_shutdown());
        assert!(!peer.path("pid.json").exists());
        assert!(
            gate.call("retry", TOOLKIT_PREPARE_SERVICES, json!({}))
                .await
                .is_err()
        );
        assert!(
            gate.call("late-help", "setup_help", json!({}))
                .await
                .is_err()
        );
    }
}

#[derive(Clone, Copy)]
enum Mode {
    Deadline,
    Complete,
    Wait,
}
struct Probe {
    inner: meerkat_client::TestClient,
    calls: AtomicUsize,
    mode: Mode,
}
#[async_trait]
impl LlmClient for Probe {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        self.inner.project_replay_messages(messages)
    }
    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        for name in ["tool_catalog_search", "tool_catalog_load"] {
            assert!(
                request.tools.iter().any(|tool| tool.name == name),
                "native catalog control must be composed before the first late MCP add: {name}"
            );
        }
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            assert!(
                request
                    .tools
                    .iter()
                    .any(|tool| tool.name == TOOLKIT_PREPARE_SERVICES)
            );
            assert!(!request.tools.iter().any(|tool| tool.name == "setup_help"));
            return Box::pin(futures_util::stream::iter([
                Ok(LlmEvent::ToolCallComplete {
                    id: "activate".into(),
                    name: TOOLKIT_PREPARE_SERVICES.into(),
                    args: json!({}),
                    meta: None,
                }),
                Ok(LlmEvent::UsageUpdate {
                    usage: meerkat_core::TurnUsage::host_declared(
                        Provider::OpenAI,
                        &request.model,
                        meerkat_core::Usage::default(),
                    ),
                }),
                Ok(LlmEvent::Done {
                    outcome: meerkat::LlmDoneOutcome::success(meerkat::StopReason::ToolUse),
                }),
            ]));
        }
        let result = request
            .messages
            .iter()
            .filter_map(|message| match message {
                Message::ToolResults { results, .. } => Some(results),
                _ => None,
            })
            .flatten()
            .find(|result| result.tool_use_id == "activate")
            .unwrap();
        assert_eq!(result.is_error, matches!(self.mode, Mode::Deadline));
        if !result.is_error {
            assert_eq!(
                serde_json::from_str::<Value>(&result.text_content()).unwrap(),
                json!({"state":"requested","changed":true,"native_pending_sources":1})
            );
        }
        if matches!(self.mode, Mode::Wait) {
            Box::pin(futures_util::stream::pending())
        } else {
            self.inner.stream(request)
        }
    }
    fn provider(&self) -> Provider {
        Provider::OpenAI
    }
    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}
struct Native {
    session: ConnectSession,
    approvals: LocalApprovals,
    _decisions: ApprovalPresenter,
    raw: Arc<dyn AgentToolDispatcher>,
    probe: Arc<Probe>,
}
async fn native(directory: &Path, lazy: &LazySetup, config: Config, mode: Mode) -> Native {
    let (approvals, decisions) = LocalApprovals::create_paired_host(directory).unwrap();
    let mut options = lazy.session_options(SessionOptions::default()).unwrap();
    approvals.attach_lazy_setup(&mut options, lazy).unwrap();
    let raw = options.build.external_tools.as_ref().unwrap().clone();
    let probe = Arc::new(Probe {
        inner: meerkat_client::TestClient::for_provider(Provider::OpenAI),
        calls: AtomicUsize::new(0),
        mode,
    });
    options.model.model = Some("gpt-5.4".into());
    options.build.llm_client_override = Some(meerkat::encode_llm_client_override_for_service(
        probe.clone(),
    ));
    let storage = RealmOptions {
        context_root: directory.into(),
        state_root: Some(directory.join("state")),
        user_config_root: Some(directory.join("user")),
    }
    .resolve("lazy-fixture")
    .unwrap();
    let session = Box::pin(ConnectSession::open_lazy_setup_with_config(
        storage, config, options, lazy,
    ))
    .await
    .unwrap();
    approvals.bind_session(session.id()).unwrap();
    Native {
        session,
        approvals,
        _decisions: decisions,
        raw,
        probe,
    }
}

#[tokio::test]
async fn configured_native_deadline_refuses_late_prepared_handoff() {
    let directory = tempfile::tempdir().unwrap();
    let (paired_host, _hosts) = PairedHostFrontend::deferred();
    let (lazy, mut presenter) = LazySetup::new(paired_host);
    let mut config = Config::default();
    config.tools.default_timeout = Duration::from_secs(3);
    config
        .tools
        .tool_timeouts
        .insert(TOOLKIT_PREPARE_SERVICES.into(), Duration::from_millis(80));
    let Native {
        session,
        approvals,
        _decisions,
        raw,
        probe,
    } = native(directory.path(), &lazy, config, Mode::Deadline).await;
    let (admitted, completion) = session
        .submit("Prepare supplied services.".into())
        .await
        .unwrap();
    let request = timeout(BOUND, presenter.next()).await.unwrap().unwrap();
    let waiting = completion.wait();
    tokio::pin!(waiting);
    let outcome = timeout(Duration::from_secs(2), &mut waiting).await;
    if outcome.is_err() {
        approvals.retire();
        session.cancel(&admitted).await.unwrap();
        let _ = waiting.await;
        session.close().await.unwrap();
        panic!("configured native 80ms deadline was not preserved");
    }
    assert!(matches!(
        outcome.unwrap().unwrap(),
        CompletionOutcome::Completed(_)
    ));
    let peer = Peer::new();
    assert!(
        timeout(BOUND, request.attach_prepared(peer.connection()))
            .await
            .unwrap()
            .is_err()
    );
    assert!(!peer.path("pid.json").exists());
    assert!(presenter.requires_shutdown());
    assert_eq!(probe.calls.load(Ordering::SeqCst), 2);
    assert!(!raw.tools().iter().any(|tool| tool.name == "setup_help"));
    approvals.retire();
    timeout(BOUND, session.close()).await.unwrap().unwrap();
}

#[tokio::test]
async fn native_close_joins_pending_and_ready_stdio_after_completion_or_cancel() {
    // The close contract is cancel exact input and join first, then native
    // adapter shutdown before unregister. The peer has no worker descendants.
    for (ready, cancel) in [(false, false), (false, true), (true, false)] {
        let directory = tempfile::tempdir().unwrap();
        let peer = Peer::new();
        let (paired_host, _hosts) = PairedHostFrontend::deferred();
        let (lazy, mut presenter) = LazySetup::new(paired_host);
        let Native {
            session,
            approvals,
            _decisions,
            raw,
            probe,
        } = native(
            directory.path(),
            &lazy,
            Config::default(),
            if cancel { Mode::Wait } else { Mode::Complete },
        )
        .await;
        let (admitted, completion) = session
            .submit("Prepare supplied services.".into())
            .await
            .unwrap();
        let request = timeout(BOUND, presenter.next()).await.unwrap().unwrap();
        timeout(BOUND, request.attach_prepared(peer.connection()))
            .await
            .unwrap()
            .unwrap();
        let pid = peer.started().await;
        timeout(BOUND, async {
            while probe.calls.load(Ordering::SeqCst) < 2 {
                sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        if cancel {
            session.cancel(&admitted).await.unwrap();
        }
        let outcome = timeout(BOUND, completion.wait()).await.unwrap().unwrap();
        if cancel {
            assert!(matches!(outcome, CompletionOutcome::Cancelled));
        } else {
            assert!(matches!(outcome, CompletionOutcome::Completed(_)));
        }
        if ready {
            std::fs::write(peer.path("release"), []).unwrap();
            timeout(BOUND, async {
                loop {
                    raw.poll_external_updates().await;
                    if raw
                        .tool_catalog()
                        .iter()
                        .any(|entry| entry.tool.name == "setup_help")
                    {
                        break;
                    }
                    sleep(Duration::from_millis(2)).await;
                }
            })
            .await
            .unwrap();
            assert!(raw.pending_catalog_sources().is_empty());
        } else {
            assert_eq!(raw.pending_catalog_sources().as_ref(), ["toolkit-setup"]);
            assert!(
                !raw.tool_catalog()
                    .iter()
                    .any(|entry| entry.tool.name == "setup_help")
            );
        }
        approvals.retire();
        timeout(BOUND, session.close()).await.unwrap().unwrap();
        assert_eq!(
            kill(Pid::from_raw(pid), None),
            Err(Errno::ESRCH),
            "native owner must reap the peer before test cleanup"
        );
        assert_eq!(killpg(Pid::from_raw(pid), None), Err(Errno::ESRCH));
        assert!(!peer.path("calls").exists());
    }
}

struct Peer {
    directory: tempfile::TempDir,
}
impl Peer {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("peer.py"), PEER).unwrap();
        Self { directory }
    }
    fn path(&self, name: &str) -> PathBuf {
        self.directory.path().join(name)
    }
    fn connection(&self) -> PreparedSetupConnection {
        let python =
            std::env::var_os("CONNECT_FIXTURE_PYTHON").unwrap_or_else(|| "/usr/bin/python3".into());
        let python = PathBuf::from(python);
        assert!(python.is_absolute() && python.is_file());
        PreparedSetupConnection::from_preparer_output(&serde_json::to_vec(&json!({"mcpServers":{"toolkit-setup":{
            "command":"/usr/bin/env","args":["-i",python,"-I",self.path("peer.py"),self.directory.path()]
        }}})).unwrap()).unwrap()
    }
    async fn started(&self) -> i32 {
        timeout(BOUND, async {
            while !self.path("initialize").exists() {
                sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        let ids: Value =
            serde_json::from_slice(&std::fs::read(self.path("pid.json")).unwrap()).unwrap();
        let pid = i32::try_from(ids["pid"].as_i64().unwrap()).unwrap();
        assert_eq!(ids["group"], pid);
        assert!(pid > 1 && pid != i32::try_from(std::process::id()).unwrap());
        pid
    }
}
impl Drop for Peer {
    fn drop(&mut self) {
        // Failure-only cleanup of this fixture's own isolated group. Success
        // assertions above require absence before this guard can intervene.
        if let Ok(bytes) = std::fs::read(self.path("pid.json"))
            && let Ok(ids) = serde_json::from_slice::<Value>(&bytes)
            && let Some(pid) = ids["pid"].as_i64().and_then(|pid| i32::try_from(pid).ok())
            && pid > 1
            && ids["group"] == pid
            && kill(Pid::from_raw(pid), None).is_ok()
        {
            eprintln!("terminating surviving owned synthetic MCP peer during failed test cleanup");
            let _ = killpg(Pid::from_raw(pid), Some(Signal::SIGKILL));
        }
    }
}
const PEER: &str = r#"import json, os, pathlib, sys, time
root = pathlib.Path(sys.argv[1])
(root / 'pid.json').write_text(json.dumps({'pid': os.getpid(), 'group': os.getpgrp()}))
for line in sys.stdin:
    request = json.loads(line)
    if 'id' not in request:
        continue
    method = request.get('method')
    if method == 'initialize':
        (root / 'initialize').write_text('seen')
        deadline = time.monotonic() + 20
        while not (root / 'release').exists():
            if time.monotonic() > deadline:
                sys.exit(19)
            time.sleep(0.002)
        result = {'protocolVersion': request['params']['protocolVersion'], 'capabilities': {'tools': {}}, 'serverInfo': {'name': 'bounded-lazy-fixture', 'version': '1'}}
    elif method == 'tools/list':
        result = {'tools': [{'name': 'setup_help', 'description': 'Synthetic read only', 'inputSchema': {'type': 'object', 'properties': {}}, 'annotations': {'readOnlyHint': True}}]}
    elif method == 'tools/call':
        (root / 'calls').write_text(request['params']['name'])
        result = {'content': [{'type': 'text', 'text': 'bounded synthetic help'}], 'isError': False}
    elif method == 'ping':
        result = {}
    else:
        raise AssertionError('unexpected fixture method')
    print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}), flush=True)
"#;

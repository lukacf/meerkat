//! Public-boundary controls for one host chosen inside an existing conversation.
//! Only fresh PTYs, a synthetic model and loopback PairedHost responses are used.

use std::{
    io::{self, Write},
    os::fd::OwnedFd,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, Method, StatusCode, Uri},
    response::IntoResponse,
};
use futures_util::Stream;
use meerkat::{LlmClient, LlmError, LlmEvent, LlmRequest};
use meerkat_connect::{
    ConnectSession, RealmOptions, SessionOptions,
    approval::{ApprovalPresenter, LocalApprovals},
    paired_host_tool::{PAIRED_HOST_FRONTEND_INVENTORY, PAIRED_HOST_SECRET_CREATE, PairedHostFrontend},
    host_selection::{
        PAIRED_HOST_SELECT_HOST, HostSelectionPresenter, HostSelectionPrompt, present_host_selection,
    },
    setup::{PreparedSetupConnection, paired_host_frontend_session},
    terminal::TerminalInput,
};
use meerkat_core::{
    AgentToolDispatcher, ApprovalDecision, Config, Message, Provider, SessionId, ToolError,
    ToolExecutionPolicy, ToolMutationClass,
    agent::ToolDispatchContext,
    ops::{ToolAccessPolicy, ToolDispatchOutcome},
    tool_execution_policy::ExecutionPolicyGatedDispatcher,
    types::ToolCallView,
};
use meerkat_runtime::completion::CompletionOutcome;
use nix::{
    fcntl::{FcntlArg, fcntl},
    pty::openpty,
    sys::termios::{LocalFlags, Termios, tcgetattr},
    unistd::write,
};
use serde_json::{Value, json};
use tokio::{
    sync::oneshot,
    time::{sleep, timeout},
};

const BOUND: Duration = Duration::from_secs(8);
const CODE: &str = "ABCD-EFGH-JKMN";
const DEVICE: &str = "deferred-private-device-fixture";
const FIRST: &str = "Help me connect a frontend for my family.";
const SECOND: &str = "Keep the same conversation and selected host.";

#[derive(Clone, Default)]
struct Output(Arc<Mutex<Vec<u8>>>);
impl Output {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
struct Pty {
    master: OwnedFd,
    observer: OwnedFd,
    original: Termios,
    flags: i32,
    input: TerminalInput,
}
impl Pty {
    fn new() -> Self {
        let pair = openpty(None, None).unwrap();
        let observer = pair.slave.try_clone().unwrap();
        let original = tcgetattr(&observer).unwrap();
        let flags = fcntl(&observer, FcntlArg::F_GETFL).unwrap();
        Self {
            master: pair.master,
            observer,
            original,
            flags,
            input: TerminalInput::from_owned_fd(pair.slave),
        }
    }
    fn restored(&self) {
        assert_eq!(tcgetattr(&self.observer).unwrap(), self.original);
        assert_eq!(
            fcntl(&self.observer, FcntlArg::F_GETFL).unwrap(),
            self.flags
        );
    }
}
async fn shown(output: &Output, text: &str) {
    timeout(BOUND, async {
        while !output.text().contains(text) {
            sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
}
fn send(master: &OwnedFd, text: &str) {
    assert_eq!(write(master, text.as_bytes()).unwrap(), text.len());
}
async fn human(master: &OwnedFd, observer: &OwnedFd, output: &Output, origin: &str) {
    shown(output, "PairedHost origin: ").await;
    send(master, &format!("{origin}\n"));
    shown(output, "'memory' for this process only").await;
    send(master, "memory\n");
    shown(output, "'terminal' to use a code").await;
    send(master, "terminal\n");
    shown(output, "Enter the 12-symbol host-terminal code (hidden)").await;
    assert!(
        !tcgetattr(observer)
            .unwrap()
            .local_flags
            .contains(LocalFlags::ECHO)
    );
    send(master, &format!("{CODE}\n"));
}
async fn present(request: HostSelectionPrompt, origin: &str) -> bool {
    let mut pty = Pty::new();
    let mut output = Output::default();
    let observed = output.clone();
    let (result, ()) = timeout(BOUND, async {
        tokio::join!(
            present_host_selection(request, &mut pty.input, &mut output, std::future::pending()),
            human(&pty.master, &pty.observer, &observed, origin),
        )
    })
    .await
    .unwrap();
    pty.restored();
    for secret in [CODE, "ABCDEFGHJKMN", DEVICE] {
        assert!(!output.text().contains(secret));
    }
    assert!(
        output
            .text()
            .contains("OS credential store will not be read or written")
    );
    result.unwrap()
}

#[derive(Clone)]
struct HostState {
    origin: String,
    requests: Arc<Mutex<Vec<(String, String)>>>,
}
struct Host {
    state: HostState,
    stop: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}
impl Host {
    async fn new() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let state = HostState {
            origin: format!("http://{}", listener.local_addr().unwrap()),
            requests: Default::default(),
        };
        let router = Router::new()
            .fallback(host_response)
            .with_state(state.clone());
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
        });
        Self {
            state,
            stop: Some(stop),
            task: Some(task),
        }
    }
    fn count(&self) -> usize {
        self.state.requests.lock().unwrap().len()
    }
    async fn finish(mut self) -> Vec<(String, String)> {
        let _ = self.stop.take().unwrap().send(());
        timeout(BOUND, self.task.take().unwrap())
            .await
            .unwrap()
            .unwrap();
        let requests = self.state.requests.lock().unwrap().clone();
        requests
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}
async fn host_response(
    State(state): State<HostState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    assert!(!headers.contains_key("x-paired-host-dev-token"));
    assert!(!headers.contains_key("x-paired-host-harness-token"));
    assert!(body.len() < 8192);
    if method == Method::POST {
        assert_eq!(headers["origin"].to_str().unwrap(), state.origin);
    }
    state
        .requests
        .lock()
        .unwrap()
        .push((method.to_string(), uri.path().to_owned()));
    let (status, cookie, value) = match (method.as_str(), uri.path()) {
        ("POST", "/console/pair/terminal") => {
            assert!(!headers.contains_key("cookie"));
            assert_eq!(
                serde_json::from_slice::<Value>(&body).unwrap(),
                json!({"code":"ABCDEFGHJKMN"})
            );
            (
                StatusCode::OK,
                format!("hc_console_device={DEVICE}; Path=/; HttpOnly; SameSite=Lax"),
                json!({"ok":true}),
            )
        }
        ("GET", "/console/extensions/frontend/inventory") => {
            assert!(headers["cookie"].to_str().unwrap().contains(DEVICE));
            assert_eq!(headers["cache-control"], "no-store");
            (StatusCode::OK, String::new(), inventory())
        }
        ("POST", "/console/extensions/secrets/intents") => {
            assert!(headers["cookie"].to_str().unwrap().contains(DEVICE));
            assert_eq!(
                serde_json::from_slice::<Value>(&body).unwrap(),
                secret_request()
            );
            // A host error can follow a committed effect; the invocation must
            // keep uncertainty and cannot request another pairing or effect.
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                String::new(),
                json!({"error":"internal_error"}),
            )
        }
        _ => panic!("unexpected method or route in bounded PairedHost fixture"),
    };
    let mut response = (status, [("cache-control", "no-store")], axum::Json(value)).into_response();
    if !cookie.is_empty() {
        response
            .headers_mut()
            .insert("set-cookie", cookie.parse().unwrap());
    }
    response
}
fn inventory() -> Value {
    json!({"current":{"activation_id":"214","policy_digest":"sha256:fixture-policy"},"extensions":[],"routing":"sender_identity"})
}
fn secret_request() -> Value {
    json!({"extension_id":"telegram","binding_name":"telegram_bot_token", "base":{"activation_id":"214","policy_digest":"sha256:fixture-policy","ledger_digest":"sha256:fixture-ledger"}})
}

type Gated = ExecutionPolicyGatedDispatcher<dyn AgentToolDispatcher>;
struct Gate {
    _directory: tempfile::TempDir,
    approvals: LocalApprovals,
    decisions: ApprovalPresenter,
    dispatcher: Arc<Gated>,
    raw: Arc<dyn AgentToolDispatcher>,
    context: ToolDispatchContext,
}
impl Gate {
    fn new(binding: &PairedHostFrontend, policy: Option<ToolAccessPolicy>) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let (approvals, decisions) = LocalApprovals::create_paired_host(directory.path()).unwrap();
        let mut input = SessionOptions::default();
        input.build.tool_access_policy = policy;
        let mut options = paired_host_frontend_session(input, binding).unwrap();
        approvals
            .attach_paired_host_direct(&mut options, binding)
            .unwrap();
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
            decisions,
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
    fn start(
        &self,
        id: &'static str,
        name: &'static str,
        args: Value,
    ) -> tokio::task::JoinHandle<Result<ToolDispatchOutcome, ToolError>> {
        let dispatcher = self.dispatcher.clone();
        let context = self.context.clone();
        tokio::spawn(async move { call(dispatcher.as_ref(), &context, id, name, args).await })
    }
    fn bound(&self) -> bool {
        self.raw
            .tools()
            .iter()
            .any(|tool| tool.name == PAIRED_HOST_FRONTEND_INVENTORY)
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
async fn no_prompt(presenter: &mut HostSelectionPresenter) {
    assert!(
        timeout(Duration::from_millis(20), presenter.next())
            .await
            .is_err()
    );
}

#[test]
fn combined_surface_keeps_one_preparer_exact_binding_and_caller_ceiling() {
    let (binding, _prompts) = PairedHostFrontend::deferred();
    let (other, _other_prompts) = PairedHostFrontend::deferred();
    let selected = PreparedSetupConnection::from_preparer_output(
        br#"{"mcpServers":{"toolkit-setup":{"command":"/fixture/not-executed","args":[]}}}"#,
    )
    .unwrap();
    let mut input = SessionOptions::default();
    input.build.tool_access_policy = Some(ToolAccessPolicy::DenyList(
        ["setup_check"].into_iter().collect(),
    ));
    let mut options = selected
        .interactive_session_with_paired_host(input, &binding)
        .unwrap();
    assert_eq!(options.build.mcp_servers.len(), 1);
    assert_eq!(options.build.mcp_servers[0].name, "toolkit-setup");
    let policy =
        ToolExecutionPolicy::resolve(options.build.tool_access_policy.clone().unwrap()).unwrap();
    for name in [
        "setup_help",
        "setup_prepare",
        PAIRED_HOST_SELECT_HOST,
        PAIRED_HOST_SECRET_CREATE,
    ] {
        assert!(policy.permits(name));
    }
    for name in [
        "setup_check",
        "setup_install",
        "setup_selected_install",
        "shell",
        "spawn",
        "schedule",
        "paired_host_activate",
    ] {
        assert!(!policy.permits(name));
    }
    let directory = tempfile::tempdir().unwrap();
    let (approvals, _decisions) = LocalApprovals::create_paired_host(directory.path()).unwrap();
    assert!(
        approvals
            .attach_paired_host_direct(&mut options, &binding)
            .is_err()
    );
    assert!(
        approvals
            .attach_paired_host_interactive(&mut options, &other)
            .is_err()
    );
    approvals
        .attach_paired_host_interactive(&mut options, &binding)
        .unwrap();
    assert!(
        approvals
            .attach_paired_host_interactive(&mut options, &binding)
            .is_err()
    );
}

#[tokio::test]
async fn caller_and_session_admission_precede_protected_input_and_unbound_effects() {
    let (binding, mut prompts) = PairedHostFrontend::deferred();
    let mut denied = Gate::new(&binding, Some(ToolAccessPolicy::ReadOnly));
    assert_eq!(
        denied.raw.tool_mutation_class(PAIRED_HOST_SELECT_HOST),
        ToolMutationClass::Mutating
    );
    assert!(!denied.bound());
    assert!(
        denied
            .call("readonly", PAIRED_HOST_SELECT_HOST, json!({}))
            .await
            .is_err()
    );
    no_prompt(&mut prompts).await;
    let mut allowed = Gate::new(&binding, None);
    let wrong = ToolDispatchContext::default().with_runtime_identity(SessionId::new(), None);
    assert!(
        call(
            allowed.dispatcher.as_ref(),
            &wrong,
            "wrong-session",
            PAIRED_HOST_SELECT_HOST,
            json!({})
        )
        .await
        .is_err()
    );
    assert!(
        allowed
            .call(
                "model-origin",
                PAIRED_HOST_SELECT_HOST,
                json!({"origin":"https://model-selected.invalid"})
            )
            .await
            .is_err()
    );
    assert!(
        allowed
            .call("unbound-effect", PAIRED_HOST_SECRET_CREATE, secret_request())
            .await
            .is_err()
    );
    no_prompt(&mut prompts).await;
    assert!(
        timeout(Duration::from_millis(20), denied.decisions.next())
            .await
            .is_err()
    );
    assert!(
        timeout(Duration::from_millis(20), allowed.decisions.next())
            .await
            .is_err()
    );
    assert!(!allowed.approvals.has_uncertain_dispatch());
}

#[tokio::test]
async fn duplicate_pending_drop_and_explicit_stop_cannot_restart_or_bind() {
    for mode in ["native-drop", "presenter-drop", "human-stop"] {
        let (binding, mut prompts) = PairedHostFrontend::deferred();
        let gate = Gate::new(&binding, None);
        let pending = gate.start("select", PAIRED_HOST_SELECT_HOST, json!({}));
        let request = timeout(BOUND, prompts.next()).await.unwrap().unwrap();
        assert!(
            gate.call("duplicate", PAIRED_HOST_SELECT_HOST, json!({}))
                .await
                .is_err()
        );
        no_prompt(&mut prompts).await;
        if mode == "native-drop" {
            pending.abort();
            assert!(pending.await.unwrap_err().is_cancelled());
            let mut pty = Pty::new();
            let mut output = Output::default();
            assert!(
                timeout(
                    BOUND,
                    present_host_selection(
                        request,
                        &mut pty.input,
                        &mut output,
                        std::future::pending()
                    )
                )
                .await
                .unwrap()
                .is_err()
            );
            pty.restored();
        } else {
            if mode == "presenter-drop" {
                drop(request);
            } else {
                let mut pty = Pty::new();
                let mut output = Output::default();
                assert!(
                    !present_host_selection(request, &mut pty.input, &mut output, async { Ok(()) })
                        .await
                        .unwrap()
                );
                pty.restored();
            }
            assert!(timeout(BOUND, pending).await.unwrap().unwrap().is_err());
        }
        assert!(!gate.bound());
        assert!(
            gate.call("replay", PAIRED_HOST_SELECT_HOST, json!({}))
                .await
                .is_err()
        );
        no_prompt(&mut prompts).await;
    }
}

#[tokio::test]
async fn completed_pairing_cannot_publish_after_either_handoff_future_is_dropped() {
    for drop_native in [true, false] {
        let host = Host::new().await;
        let (binding, mut prompts) = PairedHostFrontend::deferred();
        let gate = Gate::new(&binding, None);
        // Poll the real native gate only until its human prompt is queued. It
        // remains unpolled while confirmed pairing is transferred back to it.
        let mut invocation = Box::pin(gate.call("transfer", PAIRED_HOST_SELECT_HOST, json!({})));
        let request = timeout(BOUND, async {
            tokio::select! {
                prompt = prompts.next() => prompt.unwrap(),
                result = &mut invocation => panic!("selection finished before its presenter: {}", result.is_ok()),
            }
        }).await.unwrap();
        let mut pty = Pty::new();
        let mut output = Output::default();
        let observed = output.clone();
        let mut presentation = Box::pin(present_host_selection(
            request,
            &mut pty.input,
            &mut output,
            std::future::pending(),
        ));
        timeout(BOUND, async {
            tokio::select! {
                _ = async {
                    human(&pty.master, &pty.observer, &observed, &host.state.origin).await;
                    shown(&observed, "Paired for this Connect process.").await;
                } => {},
                result = &mut presentation => panic!("presenter reported publication while receiver was not polled: {}", result.is_ok()),
            }
        }).await.unwrap();
        // Advance the presenter through its handoff wait, but never the native
        // receiver. No second owner or test-only binding hook is involved.
        assert!(
            timeout(Duration::from_millis(20), &mut presentation)
                .await
                .is_err()
        );
        assert_eq!(host.count(), 1);
        assert!(!gate.bound(), "pairing alone must not expose host tools");
        if drop_native {
            drop(invocation);
            assert!(timeout(BOUND, &mut presentation).await.unwrap().is_err());
            drop(presentation);
        } else {
            drop(presentation);
            assert!(timeout(BOUND, &mut invocation).await.unwrap().is_err());
            drop(invocation);
        }
        pty.restored();
        assert!(!gate.bound());
        assert!(
            gate.call("after-drop", PAIRED_HOST_SELECT_HOST, json!({}))
                .await
                .is_err()
        );
        no_prompt(&mut prompts).await;
        assert_eq!(
            host.finish().await,
            [("POST".into(), "/console/pair/terminal".into())]
        );
    }
}

#[tokio::test]
async fn one_selected_host_and_uncertain_effect_do_not_reopen_selection_authority() {
    let host = Host::new().await;
    let other = Host::new().await;
    let (binding, mut prompts) = PairedHostFrontend::deferred();
    let mut gate = Gate::new(&binding, None);
    let selecting = gate.start("select", PAIRED_HOST_SELECT_HOST, json!({}));
    let request = timeout(BOUND, prompts.next()).await.unwrap().unwrap();
    assert!(present(request, &host.state.origin).await);
    let selected = timeout(BOUND, selecting).await.unwrap().unwrap().unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&selected.result.text_content()).unwrap(),
        json!({"state":"selected","changed":true})
    );
    assert!(gate.bound());
    let again = gate
        .call("same-host", PAIRED_HOST_SELECT_HOST, json!({}))
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&again.result.text_content()).unwrap(),
        json!({"state":"selected","changed":false})
    );
    assert!(
        gate.call(
            "replace-host",
            PAIRED_HOST_SELECT_HOST,
            json!({"origin":other.state.origin})
        )
        .await
        .is_err()
    );
    no_prompt(&mut prompts).await;
    let read = gate
        .call("inventory", PAIRED_HOST_FRONTEND_INVENTORY, json!({}))
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&read.result.text_content()).unwrap(),
        inventory()
    );
    assert_eq!(host.count(), 2);
    assert_eq!(other.count(), 0);

    let effect = gate.start("create", PAIRED_HOST_SECRET_CREATE, secret_request());
    let mut approval = timeout(BOUND, gate.decisions.next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(host.count(), 2, "approval must precede the request");
    approval.decide(ApprovalDecision::Approve).unwrap();
    assert!(timeout(BOUND, effect).await.unwrap().unwrap().is_err());
    assert!(gate.approvals.has_uncertain_dispatch());
    for (id, name, args) in [
        ("retry-select", PAIRED_HOST_SELECT_HOST, json!({})),
        ("retry-create", PAIRED_HOST_SECRET_CREATE, secret_request()),
        ("read-unrelated", PAIRED_HOST_FRONTEND_INVENTORY, json!({})),
    ] {
        assert!(gate.call(id, name, args).await.is_err());
    }
    assert_eq!(host.count(), 3);
    assert_eq!(other.count(), 0);
    no_prompt(&mut prompts).await;
    assert!(
        timeout(Duration::from_millis(20), gate.decisions.next())
            .await
            .is_err()
    );
    let audit = std::fs::read_to_string(gate.approvals.store_path()).unwrap();
    for secret in [CODE, "ABCDEFGHJKMN", DEVICE] {
        assert!(!audit.contains(secret));
    }
    assert_eq!(host.finish().await.len(), 3);
    assert!(other.finish().await.is_empty());
}

#[tokio::test]
async fn settled_frontend_moves_pairing_into_fresh_consent_without_sharing_old_dispatchers() {
    let host = Host::new().await;
    let (binding, mut prompts) = PairedHostFrontend::deferred();
    let gate = Gate::new(&binding, None);
    let selecting = gate.start("select-for-continuation", PAIRED_HOST_SELECT_HOST, json!({}));
    let request = timeout(BOUND, prompts.next()).await.unwrap().unwrap();
    assert!(present(request, &host.state.origin).await);
    timeout(BOUND, selecting).await.unwrap().unwrap().unwrap();
    gate.call("before-close", PAIRED_HOST_FRONTEND_INVENTORY, json!({})).await.unwrap();
    assert_eq!(host.count(), 2);
    // The live dispatcher prevents move-only reuse. The original remains
    // usable; no connection, consent or selection clone is manufactured.
    assert!(binding.clone().continue_after_close().is_err());
    let old_dispatcher = gate.raw.clone();
    gate.approvals.retire();
    drop(gate);
    drop(prompts);
    assert!(binding.clone().continue_after_close().is_err());
    drop(old_dispatcher);
    let (fresh, presenter) = binding.continue_after_close().unwrap();
    assert!(presenter.is_none(), "the existing paired host needs no repeated selection or pairing");
    let mut next = Gate::new(&fresh, None);
    next.call("after-close", PAIRED_HOST_FRONTEND_INVENTORY, json!({})).await.unwrap();
    assert_eq!(host.count(), 3, "the original private client still supplies its existing host credentials");
    let effect = next.start("new-consent", PAIRED_HOST_SECRET_CREATE, secret_request());
    let decision = timeout(BOUND, next.decisions.next()).await.unwrap().unwrap();
    assert_eq!(host.count(), 3, "new composition cannot reuse a prior consent");
    next.approvals.retire();
    drop(decision);
    assert!(timeout(BOUND, effect).await.unwrap().unwrap().is_err());
    drop(next); drop(fresh);
    assert_eq!(host.finish().await, [
        ("POST".into(), "/console/pair/terminal".into()),
        ("GET".into(), "/console/extensions/frontend/inventory".into()),
        ("GET".into(), "/console/extensions/frontend/inventory".into()),
    ]);
}

struct Probe {
    inner: meerkat_client::TestClient,
    calls: AtomicUsize,
    requests: Mutex<Vec<Vec<Message>>>,
    timeout_expected: bool,
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
        let encoded = serde_json::to_string(&request.messages).unwrap();
        let definitions = serde_json::to_string(&request.tools).unwrap();
        for secret in [CODE, "ABCDEFGHJKMN", DEVICE] {
            assert!(!encoded.contains(secret) && !definitions.contains(secret));
        }
        self.requests.lock().unwrap().push(request.messages.clone());
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let tool = match call {
            0 => {
                assert!(
                    request
                        .tools
                        .iter()
                        .any(|tool| tool.name == PAIRED_HOST_SELECT_HOST)
                );
                assert!(
                    !request
                        .tools
                        .iter()
                        .any(|tool| tool.name == PAIRED_HOST_FRONTEND_INVENTORY)
                );
                Some(("select", PAIRED_HOST_SELECT_HOST))
            }
            1 => {
                let result = request
                    .messages
                    .iter()
                    .filter_map(|message| match message {
                        Message::ToolResults { results, .. } => Some(results),
                        _ => None,
                    })
                    .flatten()
                    .find(|result| result.tool_use_id == "select")
                    .unwrap();
                assert_eq!(result.is_error, self.timeout_expected);
                assert_eq!(
                    request
                        .tools
                        .iter()
                        .any(|tool| tool.name == PAIRED_HOST_FRONTEND_INVENTORY),
                    !self.timeout_expected
                );
                if self.timeout_expected {
                    None
                } else {
                    Some(("inventory", PAIRED_HOST_FRONTEND_INVENTORY))
                }
            }
            _ => None,
        };
        if let Some((id, name)) = tool {
            Box::pin(futures_util::stream::iter([
                Ok(LlmEvent::ToolCallComplete {
                    id: id.into(),
                    name: name.into(),
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
            ]))
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
async fn native(
    directory: &std::path::Path,
    binding: &PairedHostFrontend,
    config: Config,
    probe: Arc<Probe>,
) -> (ConnectSession, LocalApprovals, ApprovalPresenter) {
    let (approvals, decisions) = LocalApprovals::create_paired_host(directory).unwrap();
    let mut options = paired_host_frontend_session(SessionOptions::default(), binding).unwrap();
    approvals
        .attach_paired_host_direct(&mut options, binding)
        .unwrap();
    options.model.model = Some("gpt-5.4".into());
    options.build.llm_client_override =
        Some(meerkat::encode_llm_client_override_for_service(probe));
    let storage = RealmOptions {
        context_root: directory.into(),
        state_root: Some(directory.join("state")),
        user_config_root: Some(directory.join("user")),
    }
    .resolve("deferred-fixture")
    .unwrap();
    let session = Box::pin(ConnectSession::open_paired_host_with_config(
        storage, config, options, binding,
    ))
    .await
    .unwrap();
    approvals.bind_session(session.id()).unwrap();
    (session, approvals, decisions)
}
fn probe(timeout_expected: bool) -> Arc<Probe> {
    Arc::new(Probe {
        inner: meerkat_client::TestClient::for_provider(Provider::OpenAI),
        calls: AtomicUsize::new(0),
        requests: Mutex::new(Vec::new()),
        timeout_expected,
    })
}

#[tokio::test]
async fn native_turn_refreshes_tools_after_selection_without_replacing_conversation() {
    let directory = tempfile::tempdir().unwrap();
    let host = Host::new().await;
    let (binding, mut prompts) = PairedHostFrontend::deferred();
    let probe = probe(false);
    let (session, approvals, mut decisions) =
        native(directory.path(), &binding, Config::default(), probe.clone()).await;
    let id = session.id().clone();
    let (admitted, completion) = session.submit(FIRST.into()).await.unwrap();
    let waiting = completion.wait();
    tokio::pin!(waiting);
    let interaction = async {
        let request = timeout(BOUND, prompts.next()).await.unwrap().unwrap();
        assert!(present(request, &host.state.origin).await);
    };
    let result = timeout(Duration::from_secs(25), async {
        tokio::join!(&mut waiting, interaction)
    })
    .await;
    if result.is_err() {
        approvals.retire();
        session.cancel(&admitted).await.unwrap();
        let _ = waiting.await;
        session.close().await.unwrap();
        panic!("native deferred interaction exceeded its finite fixture bound");
    }
    assert!(matches!(
        result.unwrap().0.unwrap(),
        CompletionOutcome::Completed(_)
    ));
    let (_, completion) = session.submit(SECOND.into()).await.unwrap();
    assert!(matches!(
        timeout(BOUND, completion.wait()).await.unwrap().unwrap(),
        CompletionOutcome::Completed(_)
    ));
    assert_eq!(session.id(), &id);
    let last = probe.requests.lock().unwrap().last().unwrap().clone();
    let users: Vec<_> = last
        .iter()
        .filter_map(|message| match message {
            Message::User(user) => Some(user.text_content()),
            _ => None,
        })
        .collect();
    assert_eq!(users, [FIRST, SECOND]);
    for secret in [CODE, "ABCDEFGHJKMN", DEVICE, host.state.origin.as_str()] {
        assert!(
            !serde_json::to_string(&*probe.requests.lock().unwrap())
                .unwrap()
                .contains(secret)
        );
    }
    assert!(
        timeout(Duration::from_millis(20), decisions.next())
            .await
            .is_err()
    );
    approvals.retire();
    session.close().await.unwrap();
    assert_eq!(
        host.finish().await,
        [
            ("POST".into(), "/console/pair/terminal".into()),
            (
                "GET".into(),
                "/console/extensions/frontend/inventory".into()
            )
        ]
    );
}

#[tokio::test]
async fn explicit_native_deadline_expires_selection_and_cannot_publish_from_old_prompt() {
    let directory = tempfile::tempdir().unwrap();
    let (binding, mut prompts) = PairedHostFrontend::deferred();
    let mut config = Config::default();
    config.tools.default_timeout = Duration::from_secs(3);
    config
        .tools
        .tool_timeouts
        .insert(PAIRED_HOST_SELECT_HOST.into(), Duration::from_millis(80));
    let probe = probe(true);
    let (session, approvals, mut decisions) =
        native(directory.path(), &binding, config, probe.clone()).await;
    let (admitted, completion) = session.submit(FIRST.into()).await.unwrap();
    let request = timeout(BOUND, prompts.next()).await.unwrap().unwrap();
    let waiting = completion.wait();
    tokio::pin!(waiting);
    let result = timeout(Duration::from_secs(2), &mut waiting).await;
    if result.is_err() {
        approvals.retire();
        session.cancel(&admitted).await.unwrap();
        let _ = waiting.await;
        session.close().await.unwrap();
        panic!("explicit 80ms operator timeout was not preserved");
    }
    assert!(matches!(
        result.unwrap().unwrap(),
        CompletionOutcome::Completed(_)
    ));
    let mut pty = Pty::new();
    let mut output = Output::default();
    assert!(
        timeout(
            BOUND,
            present_host_selection(request, &mut pty.input, &mut output, std::future::pending())
        )
        .await
        .unwrap()
        .is_err()
    );
    pty.restored();
    assert_eq!(probe.calls.load(Ordering::SeqCst), 2);
    assert!(
        timeout(Duration::from_millis(20), decisions.next())
            .await
            .is_err()
    );
    approvals.retire();
    session.close().await.unwrap();
}

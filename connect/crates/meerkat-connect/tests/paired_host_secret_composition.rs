//! Synthetic host and native consent only. No secret values, browser entry,
//! owner confirmation, live feature enabling or provider readiness.
use std::{
    collections::BTreeMap,
    io::{self, Write},
    sync::{Arc, Mutex},
    time::Duration,
};

use meerkat_connect::{
    SessionOptions,
    approval::{ApprovalPresenter, LocalApprovals},
    paired_host::{PairedHostClient, PairingCode},
    paired_host_tool::{
        PAIRED_HOST_FRONTEND_INVENTORY, PAIRED_HOST_FRONTEND_STATUS, PAIRED_HOST_FRONTEND_SUBMIT,
        PAIRED_HOST_SECRET_CANCEL, PAIRED_HOST_SECRET_CREATE, PAIRED_HOST_SECRET_STATUS,
        PairedHostFrontend,
    },
    setup::paired_host_frontend_session,
};
use meerkat_core::{
    AgentToolDispatcher, ApprovalDecision, SessionId, ToolError, ToolExecutionPolicy,
    agent::ToolDispatchContext,
    ops::{ToolAccessPolicy, ToolDispatchOutcome},
    tool_execution_policy::ExecutionPolicyGatedDispatcher,
    types::ToolCallView,
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
    time::timeout,
};

const BOUND: Duration = Duration::from_secs(5);
const INTENTS: &str = "/console/extensions/secrets/intents";
const ID: &str = "esi_aaaaaaaaaaaaaaaaaaaa";
const OTHER: &str = "esi_bbbbbbbbbbbbbbbbbbbb";
const DEVICE: &str = "private-fixture-secret-device";

fn request() -> Value {
    json!({"extension_id":"telegram","binding_name":"telegram_bot_token",
        "base":{"activation_id":"214","policy_digest":"sha256:fixture-policy","ledger_digest":"sha256:fixture-ledger"}})
}
fn observation(state: &str) -> Value {
    let mut value = json!({"intent_id":ID,"state":state,
        "target":{"host":"fixture-host","activation_id":"214","extension_id":"telegram",
            "binding_name":"telegram_bot_token","channel_ids":["telegram:primary"],
            "consumer":"channel:telegram:primary","consumer_verified":true,"required":true},
        "present":false,"requires_overwrite":false,"presenter":"terminal",
        "entry_url":format!("{INTENTS}/{ID}/entry")});
    if state == "awaiting_owner" {
        value["fingerprint"] = json!("a1b2c3d4");
        value["overwrite"] = json!(false);
    }
    value
}

#[derive(Clone, Copy)]
enum Mode {
    Normal,
    LostCreate,
    LostCancel,
    Missing,
    AwaitingOwner,
}

#[derive(Debug)]
struct Request {
    method: String,
    path: String,
    body: Value,
}
struct Host {
    origin: String,
    requests: Arc<Mutex<Vec<Request>>>,
    stop: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}
impl Host {
    async fn new(mode: Mode) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let expected_origin = origin.clone();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        let (stop, mut stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            loop {
                let (mut stream, _) = tokio::select! {
                    _ = &mut stopped => break,
                    accepted = listener.accept() => accepted.unwrap(),
                };
                let (method, path, headers, body) =
                    timeout(BOUND, read_request(&mut stream)).await.unwrap();
                if method == "POST" {
                    assert_eq!(headers["origin"], expected_origin);
                }
                assert!(!headers.contains_key("x-host-dev-token"));
                assert!(!headers.contains_key("x-host-harness-token"));
                if path.starts_with(INTENTS) {
                    assert!(headers["cookie"].contains(DEVICE));
                    if method == "GET" {
                        assert_eq!(headers["cache-control"], "no-store");
                    }
                }
                let (status, cookie, response) = match (method.as_str(), path.as_str()) {
                    ("POST", "/console/pair/request") => (200, "Set-Cookie: paired_host_pair=fixture-pair; Path=/console/pair; HttpOnly; SameSite=Strict\r\n".into(), json!({"ok":true})),
                    ("POST", "/console/pair/confirm") => (200, format!("Set-Cookie: paired_host_device={DEVICE}; Path=/; HttpOnly; SameSite=Lax\r\nSet-Cookie: paired_host_pair=; Max-Age=0; Path=/console/pair\r\n"), json!({"ok":true})),
                    ("POST", INTENTS) => (if matches!(mode,Mode::LostCreate) {0} else {201}, String::new(), observation("awaiting_entry")),
                    ("POST", p) if p == format!("{INTENTS}/{ID}/cancel") => (if matches!(mode,Mode::LostCancel) {0} else {200}, String::new(), observation("cancelled")),
                    ("GET", p) if p == format!("{INTENTS}/{ID}") => {
                        if matches!(mode,Mode::Missing | Mode::LostCancel) {(404,String::new(),json!({"error":"not_found"}))}
                        else {(200,String::new(),observation(if matches!(mode,Mode::AwaitingOwner) {"awaiting_owner"} else {"awaiting_entry"}))}
                    },
                    _ => panic!("unexpected fixture method/route"),
                };
                recorded
                    .lock()
                    .unwrap()
                    .push(Request { method, path, body });
                if status == 0 {
                    continue;
                }
                let body = serde_json::to_vec(&response).unwrap();
                let head = format!(
                    "HTTP/1.1 {status} Fixture\r\nConnection: close\r\nContent-Type: application/json\r\nCache-Control: no-store\r\n{cookie}Content-Length: {}\r\n\r\n",
                    body.len()
                );
                timeout(BOUND, async {
                    stream.write_all(head.as_bytes()).await.unwrap();
                    stream.write_all(&body).await.unwrap();
                    stream.shutdown().await.unwrap();
                })
                .await
                .unwrap();
            }
        });
        Self {
            origin,
            requests,
            stop: Some(stop),
            task: Some(task),
        }
    }
    fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
    async fn binding(&self) -> PairedHostFrontend {
        let mut client = PairedHostClient::new(&self.origin).unwrap();
        client
            .request_pairing("Secret composition fixture")
            .await
            .unwrap();
        client
            .confirm_pairing(PairingCode::new("421735".into()).unwrap())
            .await
            .unwrap();
        PairedHostFrontend::new(client)
    }
    async fn finish(mut self) {
        self.stop.take().unwrap().send(()).unwrap();
        timeout(BOUND, self.task.take().unwrap())
            .await
            .unwrap()
            .unwrap();
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}
async fn read_request(stream: &mut TcpStream) -> (String, String, BTreeMap<String, String>, Value) {
    let mut bytes = Vec::new();
    let boundary = loop {
        if let Some(i) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
            break i + 4;
        }
        let mut buffer = [0; 4096];
        let count = stream.read(&mut buffer).await.unwrap();
        assert!(count > 0 && bytes.len() + count < 128 * 1024);
        bytes.extend_from_slice(&buffer[..count]);
    };
    let mut lines = std::str::from_utf8(&bytes[..boundary])
        .unwrap()
        .split("\r\n");
    let mut first = lines.next().unwrap().split_whitespace();
    let method = first.next().unwrap().into();
    let path = first.next().unwrap().into();
    let headers: BTreeMap<_, _> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    let len: usize = headers
        .get("content-length")
        .map(|v| v.parse().unwrap())
        .unwrap_or(0);
    assert!(len <= 64 * 1024);
    while bytes.len() < boundary + len {
        let mut buffer = [0; 4096];
        let count = stream.read(&mut buffer).await.unwrap();
        assert!(count > 0);
        bytes.extend_from_slice(&buffer[..count]);
    }
    let body = if len == 0 {
        Value::Null
    } else {
        serde_json::from_slice(&bytes[boundary..boundary + len]).unwrap()
    };
    (method, path, headers, body)
}

struct Setup {
    _directory: tempfile::TempDir,
    approvals: LocalApprovals,
    presenter: ApprovalPresenter,
    dispatcher: Arc<ExecutionPolicyGatedDispatcher<dyn AgentToolDispatcher>>,
    raw: Arc<dyn AgentToolDispatcher>,
    context: ToolDispatchContext,
}
impl Setup {
    fn new(binding: &PairedHostFrontend, read_only: bool) -> Self {
        Self::with_policy(
            binding,
            if read_only {
                Some(ToolAccessPolicy::ReadOnly)
            } else {
                None
            },
        )
    }
    fn with_policy(binding: &PairedHostFrontend, policy: Option<ToolAccessPolicy>) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let (approvals, presenter) = LocalApprovals::create_paired_host(directory.path()).unwrap();
        let mut initial = SessionOptions::default();
        initial.build.tool_access_policy = policy;
        let mut options = paired_host_frontend_session(initial, binding).unwrap();
        approvals
            .attach_paired_host_direct(&mut options, binding)
            .unwrap();
        let session = SessionId::new();
        approvals.bind_session(&session).unwrap();
        let raw = options.build.external_tools.unwrap();
        let admission = options.build.tool_dispatch_admission.unwrap();
        let dispatcher = Arc::new(
            ExecutionPolicyGatedDispatcher::new(
                raw.clone(),
                ToolExecutionPolicy::resolve(options.build.tool_access_policy.unwrap()).unwrap(),
            )
            .with_dispatch_admission(admission.clone()),
        );
        Self {
            _directory: directory,
            approvals,
            presenter,
            dispatcher,
            raw,
            context: ToolDispatchContext::default().with_runtime_identity(session, None),
        }
    }
    fn start(
        &self,
        id: &str,
        name: &str,
        args: Value,
    ) -> tokio::task::JoinHandle<Result<ToolDispatchOutcome, ToolError>> {
        let dispatcher = self.dispatcher.clone();
        let context = self.context.clone();
        let id = id.to_owned();
        let name = name.to_owned();
        tokio::spawn(async move { call(&*dispatcher, &context, &id, &name, args).await })
    }
    async fn approved(
        &mut self,
        id: &str,
        name: &str,
        args: Value,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        let task = self.start(id, name, args);
        let mut prompt = timeout(BOUND, self.presenter.next())
            .await
            .unwrap()
            .unwrap();
        prompt.decide(ApprovalDecision::Approve).unwrap();
        timeout(BOUND, task).await.unwrap().unwrap()
    }
}
async fn call(
    dispatcher: &impl AgentToolDispatcher,
    context: &ToolDispatchContext,
    id: &str,
    name: &str,
    args: Value,
) -> Result<ToolDispatchOutcome, ToolError> {
    let raw = serde_json::value::RawValue::from_string(args.to_string()).unwrap();
    dispatcher
        .dispatch_with_context(
            ToolCallView {
                id,
                name,
                args: &raw,
            },
            context,
        )
        .await
}
fn value(outcome: ToolDispatchOutcome) -> Value {
    serde_json::from_str(&outcome.result.text_content()).unwrap()
}
fn assert_private(value: &impl std::fmt::Debug) {
    let text = format!("{value:?}");
    for forbidden in ["/entry", "entry_url", "entry_path", DEVICE] {
        assert!(
            !text.contains(forbidden),
            "private presentation data escaped"
        );
    }
}

#[tokio::test]
async fn exact_approval_create_is_settled_before_private_completion_drain_and_replay_is_refused() {
    let host = Host::new(Mode::Normal).await;
    let binding = host.binding().await;
    let mut notices = binding.take_secret_entry_presenter().unwrap();
    assert!(binding.take_secret_entry_presenter().is_err());
    let mut setup = Setup::new(&binding, false);
    let task = setup.start("create", PAIRED_HOST_SECRET_CREATE, request());
    let mut prompt = timeout(BOUND, setup.presenter.next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(host.count(), 2, "no create before human decision");
    let body = prompt.record().proposed_action.body.as_ref().unwrap();
    assert_eq!(body["host"], host.origin);
    assert_eq!(body["request"], request());
    assert_eq!(body["kind"], "secret_intent_create");
    assert_private(body);
    prompt.decide(ApprovalDecision::Approve).unwrap();
    assert!(prompt.decide(ApprovalDecision::Approve).is_err());
    let created = value(timeout(BOUND, task).await.unwrap().unwrap().unwrap());
    assert_eq!(created["outcome"], "created");
    assert_eq!(created["result"]["intent"]["intent_id"], ID);
    assert_eq!(created["result"]["request"], request());
    assert_eq!(created["entry_presentation"], "queued");
    assert_private(&created);
    assert!(!setup.approvals.has_uncertain_dispatch());
    // Deterministically let completion win before polling the notice receiver.
    let mut terminal = Vec::new();
    notices.present_pending(&mut terminal).unwrap();
    let terminal = String::from_utf8(terminal).unwrap();
    assert!(terminal.contains(&format!("{}{INTENTS}/{ID}/entry", host.origin)));
    assert!(terminal.contains("Pair that browser with the paired host independently"));
    let mut drained = Vec::new();
    notices.present_pending(&mut drained).unwrap();
    assert!(drained.is_empty());
    assert!(matches!(
        call(
            &*setup.dispatcher,
            &setup.context,
            "create",
            PAIRED_HOST_SECRET_CREATE,
            request()
        )
        .await,
        Err(ToolError::AccessDenied { .. })
    ));
    assert_eq!(host.count(), 3);
    let audit = std::fs::read_to_string(setup.approvals.store_path()).unwrap();
    assert_private(&audit);
    {
        let requests = host.requests.lock().unwrap();
        assert_eq!(requests[2].body, request());
        assert_eq!(requests[2].method, "POST");
        assert_eq!(requests[2].path, INTENTS);
    }
    host.finish().await;
}

#[tokio::test]
async fn denial_retirement_and_dropped_waiter_never_post_or_restore_a_decision() {
    for action in 0..3 {
        let host = Host::new(Mode::Normal).await;
        let binding = host.binding().await;
        let mut setup = Setup::new(&binding, false);
        let task = setup.start("refused-create", PAIRED_HOST_SECRET_CREATE, request());
        let mut prompt = timeout(BOUND, setup.presenter.next())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(host.count(), 2);
        match action {
            0 => {
                prompt.decide(ApprovalDecision::Deny).unwrap();
                assert!(timeout(BOUND, task).await.unwrap().unwrap().is_err());
            }
            1 => {
                setup.approvals.retire();
                assert!(prompt.decide(ApprovalDecision::Approve).is_err());
                drop(prompt);
                assert!(timeout(BOUND, task).await.unwrap().unwrap().is_err());
            }
            _ => {
                task.abort();
                assert!(task.await.unwrap_err().is_cancelled());
                assert!(prompt.decide(ApprovalDecision::Approve).is_err());
            }
        }
        assert_eq!(
            host.count(),
            2,
            "no effect after refusal or abandoned consent"
        );
        assert!(!setup.approvals.has_uncertain_dispatch());
        host.finish().await;
    }
}

#[tokio::test]
async fn caller_read_only_and_wrong_session_or_unarmed_dispatch_cannot_write() {
    let host = Host::new(Mode::Normal).await;
    let binding = host.binding().await;
    let mut setup = Setup::new(&binding, true);
    for name in [PAIRED_HOST_SECRET_CREATE, PAIRED_HOST_SECRET_CANCEL] {
        let args = if name == PAIRED_HOST_SECRET_CREATE {
            request()
        } else {
            json!({"intent_id":ID})
        };
        // Native 0.8.51 asks admission before its policy check. Even an
        // approved human decision cannot override the caller's ReadOnly ceiling.
        let id = format!("denied-{name}");
        assert!(matches!(
            setup.approved(&id, name, args.clone()).await,
            Err(ToolError::AccessDenied { .. })
        ));
        assert!(!setup.approvals.has_uncertain_dispatch());
        let raw = serde_json::value::RawValue::from_string(args.to_string()).unwrap();
        assert!(matches!(
            setup
                .raw
                .dispatch(ToolCallView {
                    id: &id,
                    name,
                    args: &raw
                })
                .await,
            Err(ToolError::AccessDenied { .. })
        ));
    }
    let wrong = ToolDispatchContext::default().with_runtime_identity(SessionId::new(), None);
    assert!(matches!(
        call(
            &*setup.dispatcher,
            &wrong,
            "wrong",
            PAIRED_HOST_SECRET_STATUS,
            json!({"intent_id":ID})
        )
        .await,
        Err(ToolError::AccessDenied { .. })
    ));
    assert_eq!(host.count(), 2);
    let status = value(
        call(
            &*setup.dispatcher,
            &setup.context,
            "read",
            PAIRED_HOST_SECRET_STATUS,
            json!({"intent_id":ID}),
        )
        .await
        .unwrap(),
    );
    assert_eq!(status["outcome"], "status");
    assert_private(&status);
    assert_eq!(host.count(), 3);
    host.finish().await;
}

#[tokio::test]
async fn lost_creation_has_no_recovery_target_or_second_effect() {
    let host = Host::new(Mode::LostCreate).await;
    let binding = host.binding().await;
    let mut setup = Setup::new(&binding, false);
    assert!(
        setup
            .approved("lost", PAIRED_HOST_SECRET_CREATE, request())
            .await
            .is_err()
    );
    assert!(setup.approvals.has_uncertain_dispatch());
    assert!(!setup.approvals.permits_uncertain_status_readback());
    for (name, args) in [
        (PAIRED_HOST_SECRET_CREATE, request()),
        (PAIRED_HOST_SECRET_CANCEL, json!({"intent_id":ID})),
        (
            PAIRED_HOST_SECRET_STATUS,
            json!({"intent_id":ID,"show_entry":true}),
        ),
        (
            PAIRED_HOST_FRONTEND_STATUS,
            json!({"change_digest":format!("paired-host-change-v1:sha256:{}","a".repeat(64))}),
        ),
        (PAIRED_HOST_FRONTEND_INVENTORY, json!({})),
    ] {
        assert!(matches!(
            call(
                &*setup.dispatcher,
                &setup.context,
                "no-recovery",
                name,
                args
            )
            .await,
            Err(ToolError::AccessDenied { .. })
        ));
    }
    assert_eq!(
        host.count(),
        3,
        "one lost create, no guessed lookup or retry"
    );
    host.finish().await;
}

#[tokio::test]
async fn lost_cancellation_admits_only_its_exact_intent_and_never_restores_effect_authority() {
    let host = Host::new(Mode::LostCancel).await;
    let binding = host.binding().await;
    let mut setup = Setup::new(&binding, false);
    assert!(
        setup
            .approved("cancel", PAIRED_HOST_SECRET_CANCEL, json!({"intent_id":ID}))
            .await
            .is_err()
    );
    assert!(setup.approvals.has_uncertain_dispatch());
    assert!(setup.approvals.permits_uncertain_status_readback());
    for (name, args) in [
        (PAIRED_HOST_SECRET_STATUS, json!({"intent_id":OTHER})),
        (
            PAIRED_HOST_SECRET_STATUS,
            json!({"intent_id":ID,"origin":host.origin}),
        ),
        (
            PAIRED_HOST_FRONTEND_STATUS,
            json!({"change_digest":format!("paired-host-change-v1:sha256:{}","a".repeat(64))}),
        ),
        (
            PAIRED_HOST_FRONTEND_SUBMIT,
            json!({"change_digest":format!("paired-host-change-v1:sha256:{}","a".repeat(64))}),
        ),
        (PAIRED_HOST_SECRET_CREATE, request()),
        (PAIRED_HOST_SECRET_CANCEL, json!({"intent_id":ID})),
    ] {
        assert!(matches!(
            call(
                &*setup.dispatcher,
                &setup.context,
                "wrong-target",
                name,
                args
            )
            .await,
            Err(ToolError::AccessDenied { .. })
        ));
    }
    assert_eq!(host.count(), 3, "no cross-target or cross-operation HTTP");
    let status = value(
        call(
            &*setup.dispatcher,
            &setup.context,
            "read",
            PAIRED_HOST_SECRET_STATUS,
            json!({"intent_id":ID}),
        )
        .await
        .unwrap(),
    );
    assert_eq!(status, json!({"outcome":"not_found"}));
    assert!(setup.approvals.has_uncertain_dispatch());
    assert!(matches!(
        call(
            &*setup.dispatcher,
            &setup.context,
            "new-cancel",
            PAIRED_HOST_SECRET_CANCEL,
            json!({"intent_id":ID})
        )
        .await,
        Err(ToolError::AccessDenied { .. })
    ));
    assert_eq!(host.count(), 4);
    host.finish().await;
}

struct FailedOutput;
impl Write for FailedOutput {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "fixture output closed",
        ))
    }
    fn flush(&mut self) -> io::Result<()> {
        Err(io::Error::from(io::ErrorKind::BrokenPipe))
    }
}

#[tokio::test]
async fn dropped_presenter_full_queue_and_failed_output_preserve_created_and_known_id() {
    for mode in 0..3 {
        let host = Host::new(Mode::Normal).await;
        let binding = host.binding().await;
        let mut notices = Some(binding.take_secret_entry_presenter().unwrap());
        if mode == 0 {
            drop(notices.take());
        }
        let mut setup = Setup::new(&binding, false);
        if mode == 1 {
            // Explicit prior readback occupies the one-slot presenter queue.
            let read = value(
                call(
                    &*setup.dispatcher,
                    &setup.context,
                    "prior-link",
                    PAIRED_HOST_SECRET_STATUS,
                    json!({"intent_id":ID,"show_entry":true}),
                )
                .await
                .unwrap(),
            );
            assert_eq!(read["entry_presentation"], "queued");
        }
        let result = value(
            setup
                .approved("create", PAIRED_HOST_SECRET_CREATE, request())
                .await
                .unwrap(),
        );
        assert_eq!(result["outcome"], "created");
        assert_eq!(result["result"]["intent"]["intent_id"], ID);
        assert_eq!(
            result["entry_presentation"],
            if mode == 2 { "queued" } else { "unavailable" }
        );
        assert_private(&result);
        assert!(!setup.approvals.has_uncertain_dispatch());
        if mode == 2 {
            let notice = timeout(BOUND, notices.as_mut().unwrap().next())
                .await
                .unwrap()
                .unwrap();
            assert_private(&notice);
            let error = notice.present(&mut FailedOutput).unwrap_err().to_string();
            assert!(error.contains(ID));
            assert!(error.contains("Do not create it again"));
            assert_private(&error);
            assert!(!setup.approvals.has_uncertain_dispatch());
        }
        assert!(matches!(
            call(
                &*setup.dispatcher,
                &setup.context,
                "create",
                PAIRED_HOST_SECRET_CREATE,
                request()
            )
            .await,
            Err(ToolError::AccessDenied { .. })
        ));
        assert_eq!(host.count(), if mode == 1 { 4 } else { 3 });
        assert_eq!(
            host.requests
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r.method == "POST" && r.path == INTENTS)
                .count(),
            1
        );
        host.finish().await;
    }
}

#[tokio::test]
async fn only_explicit_awaiting_entry_readback_redisplays_and_cancel_needs_its_own_approval() {
    for mode in [Mode::Normal, Mode::AwaitingOwner, Mode::Missing] {
        let host = Host::new(mode).await;
        let binding = host.binding().await;
        let mut notices = binding.take_secret_entry_presenter().unwrap();
        let mut setup = Setup::new(&binding, false);
        let plain = value(
            call(
                &*setup.dispatcher,
                &setup.context,
                "plain",
                PAIRED_HOST_SECRET_STATUS,
                json!({"intent_id":ID}),
            )
            .await
            .unwrap(),
        );
        assert!(plain.get("entry_presentation").is_none());
        let mut output = Vec::new();
        notices.present_pending(&mut output).unwrap();
        assert!(output.is_empty());
        let explicit = value(
            call(
                &*setup.dispatcher,
                &setup.context,
                "explicit",
                PAIRED_HOST_SECRET_STATUS,
                json!({"intent_id":ID,"show_entry":true}),
            )
            .await
            .unwrap(),
        );
        assert_private(&explicit);
        notices.present_pending(&mut output).unwrap();
        match mode {
            Mode::Normal => {
                assert_eq!(explicit["entry_presentation"], "queued");
                assert!(!output.is_empty());
            }
            Mode::AwaitingOwner => {
                assert_eq!(explicit["entry_presentation"], "not_awaiting_entry");
                assert!(output.is_empty());
            }
            _ => {
                assert!(explicit.get("entry_presentation").is_none());
                assert!(output.is_empty());
            }
        }
        let before = host.count();
        let task = setup.start("cancel", PAIRED_HOST_SECRET_CANCEL, json!({"intent_id":ID}));
        let mut prompt = timeout(BOUND, setup.presenter.next())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(host.count(), before);
        let body = prompt.record().proposed_action.body.as_ref().unwrap();
        assert_eq!(body["intent_id"], ID);
        assert_eq!(body["host"], host.origin);
        assert_private(body);
        prompt.decide(ApprovalDecision::Approve).unwrap();
        let cancelled = value(timeout(BOUND, task).await.unwrap().unwrap().unwrap());
        assert_eq!(cancelled["outcome"], "cancelled");
        assert_eq!(cancelled["result"]["state"], "cancelled");
        assert_eq!(host.count(), before + 1);
        assert!(!setup.approvals.has_uncertain_dispatch());
        host.finish().await;
    }
}

#[tokio::test]
async fn invalid_create_is_rejected_before_approval_and_valid_work_remains_possible() {
    let host = Host::new(Mode::Normal).await;
    let binding = host.binding().await;
    let mut setup = Setup::new(&binding, false);
    let mut empty_extension = request();
    empty_extension["extension_id"] = json!("");
    let mut empty_binding = request();
    empty_binding["binding_name"] = json!("");
    let mut oversized = request();
    oversized["binding_name"] = json!("a".repeat(8192));
    let malformed_wheel = json!({"binding_name":"telegram_bot_token", "candidate":{
        "change_digest":format!("paired-host-change-v1:sha256:{}", "a".repeat(64)),"wheel_sha256":"not-a-wheel-hash"}});
    let mut secret_field = request();
    secret_field["value"] = json!("forbidden-field-sentinel");
    for args in [
        empty_extension,
        empty_binding,
        oversized,
        malformed_wheel,
        secret_field,
    ] {
        assert!(
            call(
                &*setup.dispatcher,
                &setup.context,
                "invalid",
                PAIRED_HOST_SECRET_CREATE,
                args
            )
            .await
            .is_err()
        );
        assert_eq!(host.count(), 2);
        assert!(!setup.approvals.has_uncertain_dispatch());
        assert!(
            timeout(Duration::from_millis(10), setup.presenter.next())
                .await
                .is_err()
        );
    }
    let audit_before = std::fs::read_to_string(setup.approvals.store_path()).unwrap_or_default();
    assert!(
        !audit_before.contains("invalid"),
        "invalid input must not create an approval record"
    );
    let created = value(
        setup
            .approved("valid", PAIRED_HOST_SECRET_CREATE, request())
            .await
            .unwrap(),
    );
    assert_eq!(created["outcome"], "created");
    assert_eq!(host.count(), 3);
    host.finish().await;
}

#[tokio::test]
async fn candidate_secret_binding_preserves_reviewed_digest_wheel_and_host() {
    let host = Host::new(Mode::Normal).await;
    let binding = host.binding().await;
    let mut setup = Setup::new(&binding, false);
    let proposal = json!({"candidate":{"change_digest":format!("paired-host-change-v1:sha256:{}","c".repeat(64)),
        "wheel_sha256":"d".repeat(64)},"binding_name":"telegram_bot_token"});
    let task = setup.start(
        "candidate-secret",
        PAIRED_HOST_SECRET_CREATE,
        proposal.clone(),
    );
    let mut prompt = timeout(BOUND, setup.presenter.next())
        .await
        .unwrap()
        .unwrap();
    let reviewed = prompt.record().proposed_action.body.as_ref().unwrap();
    assert_eq!(reviewed["host"], host.origin);
    assert_eq!(reviewed["request"], proposal);
    assert_eq!(host.count(), 2);
    prompt.decide(ApprovalDecision::Approve).unwrap();
    let created = value(timeout(BOUND, task).await.unwrap().unwrap().unwrap());
    assert_eq!(created["outcome"], "created");
    assert_eq!(created["result"]["request"], proposal);
    assert_eq!(host.requests.lock().unwrap()[2].body, proposal);
    assert_eq!(host.count(), 3);
    assert_private(&created);
    host.finish().await;
}

#[tokio::test]
async fn known_native_policy_refusal_discards_stale_arm_and_permits_a_fresh_eligible_action() {
    let host = Host::new(Mode::Normal).await;
    let binding = host.binding().await;
    let mut setup = Setup::with_policy(
        &binding,
        Some(ToolAccessPolicy::DenyList(
            [PAIRED_HOST_SECRET_CANCEL].into_iter().collect(),
        )),
    );
    let cancel = json!({"intent_id":ID});
    assert!(matches!(
        setup
            .approved("refused-cancel", PAIRED_HOST_SECRET_CANCEL, cancel.clone())
            .await,
        Err(ToolError::AccessDenied { .. })
    ));
    assert_eq!(host.count(), 2, "native policy prevented the HTTP call");
    assert!(!setup.approvals.has_uncertain_dispatch());
    let args = serde_json::value::RawValue::from_string(cancel.to_string()).unwrap();
    assert!(
        matches!(
            setup
                .raw
                .dispatch(ToolCallView {
                    id: "refused-cancel",
                    name: PAIRED_HOST_SECRET_CANCEL,
                    args: &args
                })
                .await,
            Err(ToolError::AccessDenied { .. })
        ),
        "the known refusal must leave no stale leaf authorization"
    );
    let created = value(
        setup
            .approved("fresh-create", PAIRED_HOST_SECRET_CREATE, request())
            .await
            .unwrap(),
    );
    assert_eq!(created["outcome"], "created");
    assert_eq!(host.count(), 3);
    assert!(!setup.approvals.has_uncertain_dispatch());
    host.finish().await;
}

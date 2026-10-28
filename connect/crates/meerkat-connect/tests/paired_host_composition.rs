//! Native external-tool and loopback HTTP qualification. Ordinary cases use a
//! synthetic model; the explicitly ignored judgment trial uses the native public
//! provider. No real paired host, packaged MCP worker, installation or activation.

use std::{
    collections::BTreeMap,
    io::{self, Write},
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use futures_util::Stream;
use meerkat::{LlmClient, LlmError, LlmEvent, LlmRequest};
use meerkat_connect::{
    ConnectSession, RealmOptions, SessionOptions,
    approval::{LocalApprovals, present_approval},
    paired_host::{PairedHostClient, PairingCode},
    paired_host_tool::{
        PAIRED_HOST_FRONTEND_INVENTORY, PAIRED_HOST_FRONTEND_PREPARE, PAIRED_HOST_FRONTEND_STATUS,
        PAIRED_HOST_FRONTEND_SUBMIT, PairedHostFrontend,
    },
    setup::{PreparedSetupConnection, paired_host_frontend_session},
    terminal::TerminalInput,
};
use meerkat_core::{
    AgentToolDispatcher, ApprovalDecision, Message, Provider, SessionId, ToolExecutionPolicy,
    ToolMutationClass, ToolResult, agent::ToolDispatchContext, ops::ToolAccessPolicy,
    tool_execution_policy::ExecutionPolicyGatedDispatcher, types::ToolCallView,
};
use nix::{
    fcntl::{FcntlArg, fcntl},
    pty::openpty,
    sys::termios::tcgetattr,
    unistd::write,
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Notify, oneshot},
    time::timeout,
};

const CODE: &str = "421735";
const DEVICE: &str = "fixture-private-device-cookie";
const BOUND: Duration = Duration::from_secs(5);

struct Request {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: Value,
}
struct Host {
    origin: String,
    inventory_arrived: Arc<Notify>,
    submission_count: Arc<std::sync::atomic::AtomicUsize>,
    stop: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<Vec<Request>>>,
}

impl Host {
    async fn new(revoked: bool) -> Self {
        Self::with_pending_inventory(revoked, false).await
    }

    async fn with_pending_inventory(revoked: bool, pending_inventory: bool) -> Self {
        Self::with_mode(revoked, pending_inventory, None).await
    }

    async fn with_mode(revoked: bool, pending_inventory: bool, submission: Option<u16>) -> Self {
        Self::with_package_mode(revoked, pending_inventory, submission, false).await
    }

    async fn with_package_mode(
        revoked: bool,
        pending_inventory: bool,
        submission: Option<u16>,
        new_extension: bool,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let (stop, mut stopped) = oneshot::channel();
        let inventory_arrived = Arc::new(Notify::new());
        let arrived = inventory_arrived.clone();
        let submission_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted_submissions = submission_count.clone();
        let task = tokio::spawn(async move {
            let mut requests = Vec::new();
            loop {
                let accepted = tokio::select! {
                    biased;
                    _ = &mut stopped => break,
                    accepted = listener.accept() => accepted.unwrap(),
                };
                let mut stream = accepted.0;
                let request = timeout(BOUND, read_request(&mut stream)).await.unwrap();
                if request.method == "POST"
                    && request.path == "/console/extensions/frontend/change-requests"
                {
                    counted_submissions.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
                if pending_inventory && request.path == "/console/extensions/frontend/inventory" {
                    requests.push(request);
                    arrived.notify_one();
                    // The fixture retains custody until the test has joined
                    // native cancellation and teardown, then explicitly stops it.
                    let _ = stopped.await;
                    break;
                }
                if submission == Some(202)
                    && request.path == "/console/extensions/frontend/change-requests"
                {
                    requests.push(request);
                    arrived.notify_one();
                    let _ = stopped.await;
                    break;
                }
                if submission == Some(0)
                    && request.path == "/console/extensions/frontend/change-requests"
                {
                    requests.push(request);
                    // The host may have committed; deliberately lose the response.
                    drop(stream);
                    continue;
                }
                let (status, headers, body) = match request.path.as_str() {
                    "/console/pair/request" => (200, "Set-Cookie: paired_host_pair=fixture-private-pair-cookie; Path=/console/pair; HttpOnly; SameSite=Strict\r\n".to_owned(), json!({"ok":true})),
                    "/console/pair/confirm" => (200, format!("Set-Cookie: paired_host_device={DEVICE}; Path=/; HttpOnly; SameSite=Lax\r\nSet-Cookie: paired_host_pair=; Max-Age=0; Path=/console/pair\r\n"), json!({"ok":true})),
                    "/console/extensions/frontend/inventory" if revoked => (403, String::new(), json!({"error":"forbidden","code":"console_owner_required"})),
                    "/console/extensions/frontend/inventory" => (200, String::new(), if new_extension { absent_inventory() } else { inventory() }),
                    "/console/extensions/frontend/prepare" if revoked => (403, String::new(), json!({"error":"forbidden","code":"console_owner_required"})),
                    "/console/extensions/frontend/prepare" if request.body.get("new_extension").is_some() => (200, String::new(), new_extension_receipt(request.body["new_extension"].clone())),
                    "/console/extensions/frontend/prepare" if submission == Some(409) => {
                        let mut candidate_receipt = change_receipt(request.body["participants"]["approved_groups"].clone());
                        candidate_receipt["change"]["change_digest"] = change_receipt(candidate()["participants"]["approved_groups"].clone())["change"]["change_digest"].clone();
                        (200, String::new(), candidate_receipt)
                    },
                    "/console/extensions/frontend/prepare" if submission.is_some() => (200, String::new(), change_receipt(request.body["participants"]["approved_groups"].clone())),
                    "/console/extensions/frontend/prepare" => (200, String::new(), receipt()),
                    "/console/extensions/frontend/change-requests" if submission == Some(500) => (500, String::new(),
                        json!({"error":"internal_error","problems":[{"code":"internal_error","path":"","message":"fixture"}]})),
                    "/console/extensions/frontend/change-requests" => (201, String::new(), json!({
                        "request_id":request.body["change_digest"],"state":"submitted",
                        "submitted_at":"2026-10-07T00:00:00+00:00","submitted_by":"paired-device",
                        "change":request.body["change"]})),
                    path if new_extension && submission == Some(201) && path.starts_with("/console/extensions/frontend/change-requests/") =>
                        (200, String::new(), json!({"request_id":path.rsplit('/').next().unwrap(),
                            "kind":"new_extension", "state":"submitted", "submitted_at":"2026-10-07T00:00:00+00:00",
                            "staged_at":null,"built":null,"prerequisites":[{"kind":"build_and_review","state":"pending"}]})),
                    path if submission == Some(201) && path.starts_with("/console/extensions/frontend/change-requests/") =>
                        (200, String::new(), json!({"request_id":path.rsplit('/').next().unwrap(),"state":"submitted","submitted_at":"2026-10-07T00:00:00+00:00"})),
                    path if path.starts_with("/console/extensions/frontend/change-requests/") =>
                        (404, String::new(), json!({"state":"not_found"})),
                    _ => panic!("unexpected host route"),
                };
                let body = serde_json::to_vec(&body).unwrap();
                let head = format!(
                    "HTTP/1.1 {status} Fixture\r\nConnection: close\r\nContent-Type: application/json\r\nCache-Control: no-store\r\n{headers}Content-Length: {}\r\n\r\n",
                    body.len()
                );
                timeout(BOUND, async {
                    stream.write_all(head.as_bytes()).await.unwrap();
                    stream.write_all(&body).await.unwrap();
                    stream.shutdown().await.unwrap();
                })
                .await
                .unwrap();
                requests.push(request);
            }
            requests
        });
        Self {
            origin,
            inventory_arrived,
            submission_count,
            stop: Some(stop),
            task: Some(task),
        }
    }

    async fn paired(&self) -> PairedHostFrontend {
        let mut client = PairedHostClient::new(&self.origin).unwrap();
        client
            .request_pairing("Connect loopback fixture")
            .await
            .unwrap();
        client
            .confirm_pairing(PairingCode::new(CODE.into()).unwrap())
            .await
            .unwrap();
        PairedHostFrontend::new(client)
    }

    async fn finish(mut self) -> Vec<Request> {
        self.stop.take().unwrap().send(()).unwrap();
        timeout(BOUND, self.task.take().unwrap())
            .await
            .unwrap()
            .unwrap()
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

async fn read_request(stream: &mut TcpStream) -> Request {
    let mut bytes = Vec::new();
    let boundary = loop {
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
        let mut buffer = [0; 4096];
        let count = stream.read(&mut buffer).await.unwrap();
        assert!(count > 0 && bytes.len() + count < 128 * 1024);
        bytes.extend_from_slice(&buffer[..count]);
    };
    let head = std::str::from_utf8(&bytes[..boundary]).unwrap();
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap().split_whitespace();
    let method = first.next().unwrap().to_owned();
    assert!(["GET", "POST"].contains(&method.as_str()));
    let path = first.next().unwrap().to_owned();
    let headers: BTreeMap<_, _> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    let length: usize = headers
        .get("content-length")
        .map(|length| length.parse().unwrap())
        .unwrap_or(0);
    assert!(length <= 64 * 1024);
    while bytes.len() < boundary + length {
        let mut buffer = [0; 4096];
        let count = stream.read(&mut buffer).await.unwrap();
        assert!(count > 0);
        bytes.extend_from_slice(&buffer[..count]);
    }
    Request {
        method,
        path,
        headers,
        body: if length == 0 {
            Value::Null
        } else {
            serde_json::from_slice(&bytes[boundary..boundary + length]).unwrap()
        },
    }
}

fn candidate() -> Value {
    json!({"extension_id":"telegram","channel_id":"telegram:primary","ingress":"polling",
        "participants":{"approved_groups":["-1001234567890"]}})
}

fn new_extension_candidate() -> Value {
    // This is synthetic proposal evidence, not a claim that this fixture pin
    // exists or that any real source discovery, package build or install ran.
    json!({"new_extension":{"distribution":"paired-host-telegram","version":"0.2.6",
        "source":{"repo":"https://github.com/fixture/example-telegram.git","tag":"v0.2.6","commit":"a".repeat(40)},
        "channel":{"ingress":"polling","approved_groups":[]}}})
}

fn absent_inventory() -> Value {
    let mut current = inventory();
    current["extensions"] = json!([]);
    current
}

fn new_extension_receipt(proposal: Value) -> Value {
    use sha2::{Digest, Sha256};
    let mut change = json!({"kind":"new_extension", "base":{"activation_id":"210",
        "policy_digest":"sha256:current-fixture","ledger_digest":format!("sha256:{}", "1".repeat(64))},
        "proposal":proposal,"unverified":["manifest","facades","channel_template","secrets"],
        "prerequisites":[{"kind":"build_and_review","state":"pending"}],
        "unified_summary":"Synthetic owner summary: request a pinned Telegram package for build and review.\nNo channel template or secrets have been verified.\n"});
    change["change_digest"] = json!(format!(
        "paired-host-change-v1:sha256:{:x}",
        Sha256::digest(serde_json::to_vec(&change).unwrap())
    ));
    json!({"valid":true,"problems":[],"change":change})
}

fn inventory() -> Value {
    json!({"current":{"activation_id":"210","policy_digest":"sha256:current-fixture"},
        "extensions":[{"extension_id":"telegram","enabled":true,"wheel_present":true,
            "channels":[{"channel_id":"telegram:primary","ingress":["polling","webhook"],
                "secrets":[{"name":"TELEGRAM_BOT_TOKEN","present":false},
                    {"name":"TELEGRAM_WEBHOOK_SECRET","present":false}],
                "approved_groups":{"declared":["-1001234567890"],"discovery":"unavailable"},
                "senders":{"identities":["identity:parent-1"],"informational":true}}]}],
        "routing":"sender_identity"})
}

fn receipt() -> Value {
    json!({"valid":true,"problems":[],
        "missing_prerequisites":[{"kind":"secret","name":"telegram_bot_token"}],
        "current":{"activation_id":"210","policy_digest":"sha256:current-fixture"},
        "candidate":{"channel_row_digest":"sha256:row-fixture",
            "policy_digest":"fixture-policy-v1:sha256:policy-fixture","compile_depth":"policy_bundle"},
        "compiled_is_not_ready":true})
}

fn change_receipt(groups: Value) -> Value {
    use sha2::{Digest, Sha256};
    let mut before = json!({"id":"telegram:primary","channel_type":"telegram","ingress":"polling",
        "sender_id":"sender","provider_account":"household","principal_subject_id":"paired_host",
        "source_event_type":"string","secret_names":[],"dedup_horizon_seconds":3600,
        "features":[],"approved_groups":[],"polling":{"interval_seconds":1},
        "inbound_limits":{"max_text_utf8_bytes":100,"max_caption_utf8_bytes":100,"max_parts":1,
            "max_part_bytes":100,"max_event_bytes":100,"max_channel_stored_bytes":100,"allowed_mime_types":["text/plain"]},
        "outbound_limits":{"max_text_utf8_bytes":100,"max_caption_utf8_bytes":100,
            "max_media_bytes":100,"allowed_media_types":["text/plain"]}});
    let original = before.clone();
    before["approved_groups"] = groups;
    let mut change = json!({"base":{"activation_id":"210","policy_digest":"sha256:current-fixture",
        "ledger_digest":format!("sha256:{}", "1".repeat(64))},
        "ledger_patch":{"path":"host/extensions.toml","extension_id":"telegram",
            "channel_id":"telegram:primary","before":original,"after":before},
        "unified_diff":format!("--- a/host/extensions.toml\n+++ b/host/extensions.toml\n@@ -1 +1 @@\n-approved_groups = []\n+approved_groups = {}\n", before["approved_groups"])});
    let digest = format!(
        "paired-host-change-v1:sha256:{:x}",
        Sha256::digest(serde_json::to_vec(&change).unwrap())
    );
    change["change_digest"] = json!(digest);
    let mut receipt = receipt();
    receipt["change"] = change;
    receipt
}

fn direct(binding: &PairedHostFrontend, caller: Option<ToolAccessPolicy>) -> SessionOptions {
    let mut options = SessionOptions::default();
    options.build.tool_access_policy = caller;
    paired_host_frontend_session(options, binding).unwrap()
}

fn connection() -> PreparedSetupConnection {
    // Structural handoff fixture, never executed. Native tests explicitly omit
    // its launch after checking the production builder's retained composition.
    PreparedSetupConnection::from_preparer_output(
        br#"{"mcpServers":{"toolkit-setup":{"command":"/fixture/not-launched"}}}"#,
    )
    .unwrap()
}

fn composed(binding: &PairedHostFrontend, caller: Option<ToolAccessPolicy>) -> SessionOptions {
    let mut options = SessionOptions::default();
    options.build.tool_access_policy = caller;
    connection()
        .preparation_session_with_paired_host(options, binding)
        .unwrap()
}

#[test]
fn concrete_builder_keeps_preparation_and_caller_ceilings_and_rejects_binding_substitution() {
    let binding = PairedHostFrontend::new(PairedHostClient::new("http://127.0.0.1:1").unwrap());
    let other = PairedHostFrontend::new(PairedHostClient::new("http://127.0.0.1:2").unwrap());
    let mut options = composed(
        &binding,
        Some(ToolAccessPolicy::DenyList(
            ["setup_check"].into_iter().collect(),
        )),
    );
    assert_eq!(options.build.mcp_servers.len(), 1);
    assert_eq!(options.build.mcp_servers[0].name, "toolkit-setup");
    let policy =
        ToolExecutionPolicy::resolve(options.build.tool_access_policy.clone().unwrap()).unwrap();
    for name in [
        "setup_catalog",
        "setup_prepare",
        "setup_help",
        PAIRED_HOST_FRONTEND_PREPARE,
        PAIRED_HOST_FRONTEND_INVENTORY,
    ] {
        assert!(policy.permits(name));
    }
    for name in [
        "setup_check",
        "setup_install",
        "setup_selected_install",
        "paired_host_activate",
        "unexpected",
    ] {
        assert!(!policy.permits(name));
    }
    let directory = tempfile::tempdir().unwrap();
    let (approvals, _presenter) = LocalApprovals::create(directory.path()).unwrap();
    assert!(approvals.attach_preparation(&mut options).is_err());
    assert!(
        approvals
            .attach_paired_host_preparation(&mut options, &other)
            .is_err()
    );
    approvals
        .attach_paired_host_preparation(&mut options, &binding)
        .unwrap();
    assert!(
        approvals
            .attach_paired_host_preparation(&mut options, &binding)
            .is_err()
    );
    assert!(
        connection()
            .preparation_session_with_paired_host(options, &binding)
            .is_err()
    );
    let options = composed(&binding, Some(ToolAccessPolicy::ReadOnly));
    let policy = ToolExecutionPolicy::resolve(options.build.tool_access_policy.unwrap()).unwrap();
    for name in [PAIRED_HOST_FRONTEND_PREPARE, PAIRED_HOST_FRONTEND_INVENTORY] {
        assert!(policy.permits_call(name, ToolMutationClass::ReadOnly));
    }
    assert!(!policy.permits_call("setup_prepare", ToolMutationClass::Unknown));
}

#[tokio::test]
async fn native_public_gate_refuses_narrower_policy_wrong_session_retirement_and_closed_request_without_http()
 {
    let host = Host::new(false).await;
    let binding = host.paired().await;
    let directory = tempfile::tempdir().unwrap();
    let (approvals, mut presenter) = LocalApprovals::create(directory.path()).unwrap();
    let mut options = composed(&binding, None);
    approvals
        .attach_paired_host_preparation(&mut options, &binding)
        .unwrap();
    let session = SessionId::new();
    approvals.bind_session(&session).unwrap();
    let inner = options.build.external_tools.clone().unwrap();
    let gate = options.build.tool_dispatch_admission.clone().unwrap();
    let dispatcher = ExecutionPolicyGatedDispatcher::new(
        inner.clone(),
        ToolExecutionPolicy::resolve(options.build.tool_access_policy.unwrap()).unwrap(),
    )
    .with_dispatch_admission(gate.clone());
    let context = ToolDispatchContext::default().with_runtime_identity(session, None);
    let wrong = ToolDispatchContext::default().with_runtime_identity(SessionId::new(), None);
    let raw = serde_json::value::to_raw_value(&candidate()).unwrap();
    let call = ToolCallView {
        id: "candidate",
        name: PAIRED_HOST_FRONTEND_PREPARE,
        args: &raw,
    };
    assert!(
        dispatcher
            .dispatch_with_context(call, &wrong)
            .await
            .is_err()
    );
    let denied_options = composed(
        &binding,
        Some(ToolAccessPolicy::DenyList(
            [PAIRED_HOST_FRONTEND_PREPARE, PAIRED_HOST_FRONTEND_INVENTORY]
                .into_iter()
                .collect(),
        )),
    );
    let denied = ExecutionPolicyGatedDispatcher::new(
        inner,
        ToolExecutionPolicy::resolve(denied_options.build.tool_access_policy.unwrap()).unwrap(),
    )
    .with_dispatch_admission(gate);
    assert!(denied.dispatch_with_context(call, &context).await.is_err());
    let empty = serde_json::value::to_raw_value(&json!({})).unwrap();
    let inventory_call = ToolCallView {
        id: "inventory",
        name: PAIRED_HOST_FRONTEND_INVENTORY,
        args: &empty,
    };
    assert!(
        dispatcher
            .dispatch_with_context(inventory_call, &wrong)
            .await
            .is_err()
    );
    assert!(
        denied
            .dispatch_with_context(inventory_call, &context)
            .await
            .is_err()
    );
    let injected = serde_json::value::to_raw_value(&json!({"origin":host.origin})).unwrap();
    assert!(
        dispatcher
            .dispatch_with_context(
                ToolCallView {
                    id: "inventory-injected",
                    name: PAIRED_HOST_FRONTEND_INVENTORY,
                    args: &injected,
                },
                &context
            )
            .await
            .is_err()
    );
    for field in ["origin", "authorization", "activate", "target_identity"] {
        let mut invalid = candidate();
        invalid[field] = json!("untrusted");
        let raw = serde_json::value::to_raw_value(&invalid).unwrap();
        assert!(
            dispatcher
                .dispatch_with_context(
                    ToolCallView {
                        id: "invalid",
                        name: PAIRED_HOST_FRONTEND_PREPARE,
                        args: &raw
                    },
                    &context
                )
                .await
                .is_err()
        );
    }
    approvals.retire();
    assert!(
        dispatcher
            .dispatch_with_context(inventory_call, &context)
            .await
            .is_err()
    );
    assert!(
        dispatcher
            .dispatch_with_context(call, &context)
            .await
            .is_err()
    );
    assert!(!approvals.has_uncertain_dispatch());
    assert!(
        timeout(Duration::from_millis(10), presenter.next())
            .await
            .is_err()
    );
    let requests = host.finish().await;
    assert_eq!(requests.len(), 2, "no candidate HTTP after native refusal");
}

struct Probe {
    inner: meerkat_client::TestClient,
    issued: std::sync::atomic::AtomicUsize,
    results: Mutex<BTreeMap<String, ToolResult>>,
    submit: bool,
    new_extension: bool,
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
        let captured = serde_json::to_string(&request.messages).unwrap();
        let offered = serde_json::to_string(&request.tools).unwrap();
        for private in [CODE, DEVICE, "fixture-private-pair-cookie"] {
            assert!(
                !captured.contains(private) && !offered.contains(private),
                "protected pairing value reached the model"
            );
        }
        for message in &request.messages {
            if let Message::ToolResults { results, .. } = message {
                for result in results.iter().filter(|result| {
                    ["inventory", "candidate", "submit", "status"]
                        .contains(&result.tool_use_id.as_str())
                }) {
                    self.results
                        .lock()
                        .unwrap()
                        .insert(result.tool_use_id.clone(), result.clone());
                }
            }
        }
        let (id, name, args) = match self
            .issued
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        {
            0 => ("inventory", PAIRED_HOST_FRONTEND_INVENTORY, json!({})),
            1 => {
                let results = self.results.lock().unwrap();
                let observed = results
                    .get("inventory")
                    .expect("native inventory returned before candidate");
                if observed.is_error {
                    return self.inner.stream(request);
                }
                let observed: Value = serde_json::from_str(&observed.text_content()).unwrap();
                if self.new_extension {
                    assert_eq!(observed, absent_inventory());
                    (
                        "candidate",
                        PAIRED_HOST_FRONTEND_PREPARE,
                        new_extension_candidate(),
                    )
                } else {
                    assert_eq!(observed, inventory());
                    // This deterministic fixture selects only fields it actually
                    // observed. Production selection remains the setup agent's job.
                    let extension = &observed["extensions"][0];
                    let channel = &extension["channels"][0];
                    (
                        "candidate",
                        PAIRED_HOST_FRONTEND_PREPARE,
                        json!({
                            "extension_id":extension["extension_id"],"channel_id":channel["channel_id"],
                            "ingress":channel["ingress"][0],
                            "participants":{"approved_groups":channel["approved_groups"]["declared"]}
                        }),
                    )
                }
            }
            2 if self.submit => {
                let results = self.results.lock().unwrap();
                let result = results.get("candidate").expect("candidate precedes submit");
                assert!(!result.is_error);
                let observed: Value = serde_json::from_str(&result.text_content()).unwrap();
                assert_eq!(observed["valid"], true);
                // Carry only the digest returned by the native preparation tool.
                (
                    "submit",
                    PAIRED_HOST_FRONTEND_SUBMIT,
                    json!({"change_digest":observed["change"]["change_digest"]}),
                )
            }
            3 if self.submit => {
                let results = self.results.lock().unwrap();
                let result = results.get("submit").expect("submission precedes status");
                assert!(!result.is_error);
                let observed: Value = serde_json::from_str(&result.text_content()).unwrap();
                assert_eq!(observed["state"], "submitted");
                assert_eq!(observed["http_status"], 201);
                let prepared: Value =
                    serde_json::from_str(&results["candidate"].text_content()).unwrap();
                assert_eq!(observed["change"], prepared["change"]);
                assert_eq!(observed["request_id"], prepared["change"]["change_digest"]);
                (
                    "status",
                    PAIRED_HOST_FRONTEND_STATUS,
                    json!({"change_digest":observed["request_id"]}),
                )
            }
            _ => return self.inner.stream(request),
        };
        assert!(request.tools.iter().any(|tool| tool.name == name));
        Box::pin(futures_util::stream::iter([
            Ok(LlmEvent::ToolCallComplete {
                id: id.into(),
                name: name.into(),
                args,
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
    }
    fn provider(&self) -> Provider {
        Provider::OpenAI
    }
    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

#[tokio::test]
async fn native_external_inventory_then_candidate_without_mcp_launch_preserves_evidence_and_revocation()
 {
    for revoked in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let host = Host::new(revoked).await;
        let binding = host.paired().await;
        let (approvals, mut presenter) =
            LocalApprovals::create_paired_host(directory.path()).unwrap();
        let mut options = direct(&binding, None);
        approvals
            .attach_paired_host_direct(&mut options, &binding)
            .unwrap();
        assert!(options.build.mcp_servers.is_empty());
        let probe = Arc::new(Probe {
            inner: meerkat_client::TestClient::for_provider(Provider::OpenAI),
            issued: Default::default(),
            results: Mutex::new(BTreeMap::new()),
            submit: false,
            new_extension: false,
        });
        options.model.model = Some("gpt-5.4".into());
        options.build.llm_client_override = Some(meerkat::encode_llm_client_override_for_service(
            probe.clone(),
        ));
        let storage = RealmOptions {
            context_root: directory.path().into(),
            state_root: Some(directory.path().join("state")),
            user_config_root: Some(directory.path().join("user")),
        }
        .resolve("paired-host-candidate")
        .unwrap();
        let mut realm_config = meerkat_core::Config::default();
        realm_config
            .tools
            .mcp_servers
            .push(meerkat_core::McpServerConfig::stdio(
                "existing-service",
                "/fixture/must-not-be-launched",
                Vec::new(),
                Default::default(),
            ));
        let session = Box::pin(ConnectSession::open_with_config(
            storage,
            realm_config,
            options,
        ))
        .await
        .unwrap();
        approvals.bind_session(session.id()).unwrap();
        let (admission, completion) = session
            .submit(
                "Read the captured host's frontend inventory, then check the concrete candidate."
                    .into(),
            )
            .await
            .unwrap();
        let wait = completion.wait();
        tokio::pin!(wait);
        let (finished, outcome) = tokio::select! {
            result = &mut wait => (true, result),
            _ = tokio::time::sleep(Duration::from_secs(20)) => {
                approvals.retire();
                session.cancel(&admission).await.unwrap();
                (false, wait.await)
            }
        };
        approvals.retire();
        session.close().await.unwrap();
        let requests = host.finish().await;
        assert!(
            finished,
            "native fixture timed out after cancellation and close"
        );
        assert!(matches!(
            outcome.unwrap(),
            meerkat_runtime::completion::CompletionOutcome::Completed(_)
        ));
        assert!(!approvals.has_uncertain_dispatch());
        assert!(
            timeout(Duration::from_millis(10), presenter.next())
                .await
                .is_err(),
            "candidate operation must not request extra consent"
        );
        let results = probe.results.lock().unwrap();
        let inventory_result = results
            .get("inventory")
            .expect("real native inventory result");
        let result = if revoked {
            inventory_result
        } else {
            results
                .get("candidate")
                .expect("real native candidate result")
        };
        if revoked {
            assert!(result.is_error);
            assert!(result.text_content().contains("ConsoleOwnerRequired"));
            assert!(
                !results.contains_key("candidate"),
                "no candidate after refused inventory"
            );
        } else {
            assert!(!inventory_result.is_error);
            assert_eq!(
                serde_json::from_str::<Value>(&inventory_result.text_content()).unwrap(),
                inventory()
            );
            assert!(!result.is_error);
            let mut expected = receipt();
            expected["review_presentation"] = json!("unavailable");
            assert_eq!(
                serde_json::from_str::<Value>(&result.text_content()).unwrap(),
                expected
            );
        }
        for private in [CODE, DEVICE, "fixture-private-pair-cookie"] {
            assert!(!result.text_content().contains(private));
        }
        assert_eq!(
            requests.len(),
            if revoked { 3 } else { 4 },
            "one request per native call, no retry"
        );
        assert_eq!(requests[2].method, "GET");
        assert_eq!(requests[2].path, "/console/extensions/frontend/inventory");
        assert_eq!(requests[2].body, Value::Null);
        if !revoked {
            assert_eq!(requests[3].method, "POST");
            assert_eq!(requests[3].path, "/console/extensions/frontend/prepare");
            assert_eq!(requests[3].body, candidate());
            assert!(requests[3].body.get("target_identity").is_none());
        }
        assert_eq!(
            requests[2].headers["cookie"],
            format!("paired_host_device={DEVICE}")
        );
    }
}

#[tokio::test]
async fn native_exact_input_cancellation_during_inventory_joins_without_candidate_or_retry() {
    let directory = tempfile::tempdir().unwrap();
    let host = Host::with_pending_inventory(false, true).await;
    let binding = host.paired().await;
    let (approvals, _presenter) = LocalApprovals::create_paired_host(directory.path()).unwrap();
    let mut options = direct(&binding, None);
    approvals
        .attach_paired_host_direct(&mut options, &binding)
        .unwrap();
    assert!(options.build.mcp_servers.is_empty());
    let probe = Arc::new(Probe {
        inner: meerkat_client::TestClient::for_provider(Provider::OpenAI),
        issued: Default::default(),
        results: Mutex::new(BTreeMap::new()),
        submit: false,
        new_extension: false,
    });
    options.model.model = Some("gpt-5.4".into());
    options.build.llm_client_override = Some(meerkat::encode_llm_client_override_for_service(
        probe.clone(),
    ));
    let storage = RealmOptions {
        context_root: directory.path().into(),
        state_root: Some(directory.path().join("state")),
        user_config_root: Some(directory.path().join("user")),
    }
    .resolve("paired-host-inventory-cancel")
    .unwrap();
    let session = Box::pin(ConnectSession::open_with_config(
        storage,
        meerkat_core::Config::default(),
        options,
    ))
    .await
    .unwrap();
    approvals.bind_session(session.id()).unwrap();
    let (admission, completion) = session
        .submit("Read the captured frontend inventory.".into())
        .await
        .unwrap();
    let arrived = timeout(Duration::from_secs(20), host.inventory_arrived.notified())
        .await
        .is_ok();
    approvals.retire();
    timeout(Duration::from_secs(20), session.cancel(&admission))
        .await
        .unwrap()
        .unwrap();
    let outcome = timeout(Duration::from_secs(20), completion.wait())
        .await
        .unwrap()
        .unwrap();
    timeout(Duration::from_secs(20), session.close())
        .await
        .unwrap()
        .unwrap();
    let requests = host.finish().await;
    assert!(
        arrived,
        "native dispatch did not reach inventory before bounded cancellation"
    );
    assert!(matches!(
        outcome,
        meerkat_runtime::completion::CompletionOutcome::Cancelled
    ));
    assert_eq!(
        requests.len(),
        3,
        "only pairing and the single pending inventory call"
    );
    assert_eq!(requests[2].path, "/console/extensions/frontend/inventory");
    assert_eq!(probe.issued.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(
        !approvals.has_uncertain_dispatch(),
        "read-only inventory did not consume an effect approval"
    );
}

#[derive(Clone, Default)]
struct ReviewOutput(Arc<Mutex<Vec<u8>>>);

impl ReviewOutput {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

impl Write for ReviewOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn native_frontend_submission_waits_for_protected_terminal_approval_then_reads_host_status() {
    native_submission_trajectory(false).await;
}

#[tokio::test]
async fn native_new_package_request_requires_protected_review_of_pinned_source_then_reads_status() {
    native_submission_trajectory(true).await;
}

async fn native_submission_trajectory(new_extension: bool) {
    let directory = tempfile::tempdir().unwrap();
    let host = Host::with_package_mode(false, false, Some(201), new_extension).await;
    let binding = host.paired().await;
    let (approvals, mut presenter) = LocalApprovals::create_paired_host(directory.path()).unwrap();
    let mut options = direct(&binding, None);
    approvals
        .attach_paired_host_direct(&mut options, &binding)
        .unwrap();
    assert!(options.build.mcp_servers.is_empty());
    let probe = Arc::new(Probe {
        inner: meerkat_client::TestClient::for_provider(Provider::OpenAI),
        issued: Default::default(),
        results: Mutex::new(BTreeMap::new()),
        submit: true,
        new_extension,
    });
    options.model.model = Some("gpt-5.4".into());
    options.build.llm_client_override = Some(meerkat::encode_llm_client_override_for_service(
        probe.clone(),
    ));
    let storage = RealmOptions {
        context_root: directory.path().into(),
        state_root: Some(directory.path().join("state")),
        user_config_root: Some(directory.path().join("user")),
    }
    .resolve("paired-host-submit")
    .unwrap();
    let session = Box::pin(ConnectSession::open_with_config(
        storage,
        meerkat_core::Config::default(),
        options,
    ))
    .await
    .unwrap();
    approvals.bind_session(session.id()).unwrap();
    let (admission, completion) = session
        .submit(
            "Check the selected frontend, submit its reviewed change and read its host status."
                .into(),
        )
        .await
        .unwrap();
    let expected_change = if new_extension {
        new_extension_receipt(new_extension_candidate()["new_extension"].clone())["change"].clone()
    } else {
        change_receipt(candidate()["participants"]["approved_groups"].clone())["change"].clone()
    };

    // This terminal belongs only to the fixture. No process stdin or /dev/tty
    // is used, and the real presenter remains its exclusive input owner.
    let pty = openpty(None, None).unwrap();
    let observer = pty.slave.try_clone().unwrap();
    let original_termios = tcgetattr(&observer).unwrap();
    let original_flags = fcntl(&observer, FcntlArg::F_GETFL).unwrap();
    let mut input = TerminalInput::from_owned_fd(pty.slave);
    let mut output = ReviewOutput::default();
    let observed_output = output.clone();
    let interaction = async {
        let prompt = presenter
            .next()
            .await
            .expect("native submit requests a decision");
        assert_eq!(
            prompt.record().owner,
            meerkat_core::ApprovalOwnerRef::Session {
                session_id: session.id().clone()
            }
        );
        let body = prompt.record().proposed_action.body.as_ref().unwrap();
        assert_eq!(body["call_id"], "submit");
        assert_eq!(body["host"], host.origin);
        assert_eq!(body["base"], expected_change["base"]);
        if new_extension {
            assert_eq!(body["kind"], "new_extension");
            assert_eq!(body["proposal"], expected_change["proposal"]);
            assert_eq!(body["unified_summary"], expected_change["unified_summary"]);
            assert!(body.get("unified_diff").is_none());
        } else {
            assert_eq!(body["kind"], "channel_config");
            assert_eq!(body["unified_diff"], expected_change["unified_diff"]);
        }
        assert_eq!(body["change_digest"], expected_change["change_digest"]);
        let approval_id = prompt.record().approval_id.to_string();
        let human = async {
            timeout(BOUND, async {
                while !observed_output
                    .text()
                    .contains(&format!("approve {approval_id}"))
                {
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
            })
            .await
            .unwrap();
            assert_eq!(
                host.submission_count
                    .load(std::sync::atomic::Ordering::SeqCst),
                0
            );
            let old = b"approve an-old-id\n";
            assert_eq!(write(&pty.master, old).unwrap(), old.len());
            timeout(BOUND, async {
                while !observed_output.text().contains("Use the exact approval ID") {
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
            })
            .await
            .unwrap();
            assert_eq!(
                host.submission_count
                    .load(std::sync::atomic::Ordering::SeqCst),
                0
            );
            let decision = format!("approve {approval_id}\n");
            assert_eq!(
                write(&pty.master, decision.as_bytes()).unwrap(),
                decision.len()
            );
        };
        let (presented, ()) = tokio::join!(
            present_approval(prompt, &mut input, &mut output, std::future::pending()),
            human
        );
        presented.unwrap()
    };
    let reviewed = timeout(Duration::from_secs(20), interaction).await;
    if !matches!(reviewed, Ok(true)) {
        approvals.retire();
        session.cancel(&admission).await.unwrap();
    }
    let outcome = timeout(Duration::from_secs(20), completion.wait()).await;
    if outcome.is_err() {
        approvals.retire();
        session.cancel(&admission).await.unwrap();
    }
    approvals.retire();
    session.close().await.unwrap();
    let requests = host.finish().await;
    assert!(
        matches!(reviewed, Ok(true)),
        "native approval did not complete"
    );
    assert!(matches!(
        outcome.unwrap().unwrap(),
        meerkat_runtime::completion::CompletionOutcome::Completed(_)
    ));
    assert!(
        !approvals.has_uncertain_dispatch(),
        "native dispatcher reported the settled result"
    );
    assert_eq!(tcgetattr(&observer).unwrap(), original_termios);
    assert_eq!(fcntl(&observer, FcntlArg::F_GETFL).unwrap(), original_flags);
    let shown = output.text();
    if new_extension {
        assert!(shown.contains(expected_change["unified_summary"].as_str().unwrap()));
        for field in ["distribution", "version"] {
            assert!(shown.contains(expected_change["proposal"][field].as_str().unwrap()));
        }
        for field in ["repo", "tag", "commit"] {
            assert!(
                shown.contains(
                    expected_change["proposal"]["source"][field]
                        .as_str()
                        .unwrap()
                )
            );
        }
        assert!(shown.contains("Kind: new_extension"));
        assert!(shown.contains("Request host-owned build and review"));
        assert!(
            shown.contains("installation, activation and readiness require later host evidence.")
        );
    } else {
        assert!(shown.contains(expected_change["unified_diff"].as_str().unwrap()));
        assert!(shown.contains("This does not activate it."));
    }
    assert!(shown.contains("Decision recorded. Waiting for the native outcome."));

    let results = probe.results.lock().unwrap();
    for id in ["inventory", "candidate", "submit", "status"] {
        assert!(!results.get(id).expect("native tool result").is_error);
    }
    let observed: Value = serde_json::from_str(&results["status"].text_content()).unwrap();
    assert_eq!(observed["state"], "submitted");
    assert_eq!(observed["request_id"], expected_change["change_digest"]);
    assert!(observed.get("activation_id").is_none());
    if new_extension {
        assert_eq!(observed["kind"], "new_extension");
        assert!(observed["built"].is_null());
        assert_eq!(
            observed["prerequisites"],
            json!([{"kind":"build_and_review","state":"pending"}])
        );
    }
    assert_eq!(requests.len(), 6, "pairing plus four native tool calls");
    let posts: Vec<_> = requests
        .iter()
        .filter(|request| {
            request.method == "POST"
                && request.path == "/console/extensions/frontend/change-requests"
        })
        .collect();
    assert_eq!(posts.len(), 1);
    assert_eq!(
        requests[3].body,
        if new_extension {
            new_extension_candidate()
        } else {
            candidate()
        }
    );
    assert_eq!(
        posts[0].body,
        json!({"change_digest":expected_change["change_digest"],"change":expected_change})
    );
    assert_eq!(requests[5].method, "GET");
    assert_eq!(
        requests[5].path,
        format!(
            "/console/extensions/frontend/change-requests/{}",
            expected_change["change_digest"].as_str().unwrap()
        )
    );
}

// This module is deliberately opt-in. Its fixture owns only synthetic host
// data; credentials stay exclusively with the normal native provider owner.
mod real_model_judgment {
    use super::*;
    use std::{
        collections::BTreeSet,
        fs::{self, File, OpenOptions},
        os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    };

    use futures_util::{FutureExt, StreamExt};
    use meerkat::resolve_create_session_model;
    use meerkat_connect::approval::{ApprovalPresenter, ApprovalPrompt};
    use meerkat_core::{AgentEvent, BudgetLimits, Config, EventEnvelope, agent_event_type};
    use meerkat_runtime::completion::CompletionOutcome;

    const TOOLS: [&str; 4] = [
        PAIRED_HOST_FRONTEND_INVENTORY,
        PAIRED_HOST_FRONTEND_PREPARE,
        PAIRED_HOST_FRONTEND_SUBMIT,
        PAIRED_HOST_FRONTEND_STATUS,
    ];
    const PROMPT: &str = "I want my family to talk to the paired host through Telegram. Use polling for the one Telegram group the selected host already declares, with messages going to each sender's own agent. Please work out the available setup and prepare a change for the host operator to review. If credentials are still missing, explain what needs finishing rather than asking for them here; I still want the review request prepared. Let me review and approve the exact change in the terminal before you submit it, then check the request's status and tell me what happened and what remains. Do not activate anything or send messages.";
    const NEW_EXTENSION_PROMPT: &str = "I want my family to talk to the paired host through Telegram, with messages going to each sender's own agent. I have researched this package: distribution example-ext-telegram, version 0.2.8, repository https://github.com/fixture/example-telegram.git, tag v0.2.8, commit 49f53a27a4535f80a357ee13e8150776ed02886b. Use this exact source with polling and no approved groups initially. Please check whether the selected host already has it and prepare the appropriate request for host-owned build and review. Let me review and approve the exact pinned proposal in the terminal before you submit it, then check its status and explain what is confirmed and what still needs the host's work. Do not ask for credentials here, claim installation from a recorded request, or send messages.";

    fn researched_new_extension_candidate() -> Value {
        // Genuine owner-accepted metadata. The host responses below remain
        // synthetic; this trial does not perform discovery or source verification.
        json!({"new_extension":{"distribution":"example-ext-telegram","version":"0.2.8",
            "source":{"repo":"https://github.com/fixture/example-telegram.git","tag":"v0.2.8",
                "commit":"49f53a27a4535f80a357ee13e8150776ed02886b"},
            "channel":{"ingress":"polling","approved_groups":[]}}})
    }

    fn require(condition: bool, message: &'static str) -> io::Result<()> {
        if condition {
            Ok(())
        } else {
            Err(io::Error::other(message))
        }
    }

    struct Trace {
        file: File,
        successful: BTreeSet<String>,
        status: Option<Value>,
        outside_ceiling: bool,
        complete: bool,
    }

    impl Trace {
        fn record(&mut self, value: Value) -> io::Result<()> {
            writeln!(self.file, "{value}")?;
            self.file.flush()
        }

        fn event(&mut self, event: EventEnvelope<AgentEvent>) -> io::Result<()> {
            let details = match &event.payload {
                AgentEvent::ToolCallRequested { id, name, args } => {
                    let allowed = TOOLS.contains(&name.as_str());
                    self.outside_ceiling |= !allowed;
                    json!({"id":id,"name":name,"allowed":allowed,
                        "arguments":if allowed { Some(args.as_value()) } else { None }})
                }
                AgentEvent::ToolResultReceived {
                    id,
                    name,
                    content,
                    is_error,
                } => {
                    let retain = TOOLS.contains(&name.as_str()) && !is_error;
                    let value = if retain {
                        let text = meerkat_core::types::text_content(content);
                        let value: Value = serde_json::from_str(&text).map_err(|_| {
                            io::Error::other("public host tool result was not JSON")
                        })?;
                        self.successful.insert(name.clone());
                        if name == PAIRED_HOST_FRONTEND_STATUS {
                            self.status = Some(value.clone());
                        }
                        Some(value)
                    } else {
                        None
                    };
                    // Successful results contain only this fixture's public
                    // host evidence. Never retain native/provider error text.
                    json!({"id":id,"name":name,"is_error":is_error,"result":value})
                }
                AgentEvent::RunStarted {
                    session_id,
                    identity,
                    ..
                }
                | AgentEvent::RunCompleted {
                    session_id,
                    identity,
                    ..
                } => json!({"session_id":session_id,"identity":identity}),
                AgentEvent::TurnCompleted {
                    stop_reason,
                    usage,
                    assistant_message_id,
                } => json!({"stop_reason":stop_reason,"usage":usage,
                        "assistant_message_id":assistant_message_id}),
                AgentEvent::RunFailed { .. } => json!({"diagnostic_omitted":true}),
                AgentEvent::ServerToolContent { kind, .. } => {
                    self.outside_ceiling = true;
                    json!({"unexpected_provider_tool":true,"kind":kind})
                }
                AgentEvent::StreamTruncated { .. } => {
                    self.complete = false;
                    json!({"evidence_complete":false})
                }
                // No reasoning, uncommitted assistant text, raw transport,
                // auth diagnostics, configuration or full session dump.
                _ => return Ok(()),
            };
            self.record(json!({"kind":"native_event","event_id":event.event_id,
                "seq":event.seq,"event_type":agent_event_type(&event.payload),
                "details":details}))
        }
    }

    async fn approve_exact_change(
        prompt: ApprovalPrompt,
        session: &ConnectSession,
        host: &Host,
        expected: &Value,
    ) -> io::Result<Value> {
        require(
            prompt.record().owner
                == meerkat_core::ApprovalOwnerRef::Session {
                    session_id: session.id().clone(),
                },
            "approval belongs to a different native session",
        )?;
        let body = prompt
            .record()
            .proposed_action
            .body
            .as_ref()
            .ok_or_else(|| io::Error::other("approval lacks its retained body"))?;
        let package = expected["kind"] == "new_extension";
        let exact_review = if package {
            body["kind"] == "new_extension"
                && body["proposal"] == expected["proposal"]
                && body["unified_summary"] == expected["unified_summary"]
                && body.get("unified_diff").is_none()
        } else {
            body["kind"] == "channel_config" && body["unified_diff"] == expected["unified_diff"]
        };
        require(
            body["tool"] == PAIRED_HOST_FRONTEND_SUBMIT
                && body["call_id"].as_str().is_some_and(|id| !id.is_empty())
                && body["host"] == host.origin
                && body["base"] == expected["base"]
                && exact_review
                && body["change_digest"] == expected["change_digest"],
            "proposed action differs from the pre-reviewed synthetic change",
        )?;
        let approval_id = prompt.record().approval_id.to_string();
        let pty =
            openpty(None, None).map_err(|_| io::Error::other("isolated PTY creation failed"))?;
        let observer = pty.slave.try_clone()?;
        let original_termios = tcgetattr(&observer).map_err(io::Error::from)?;
        let original_flags = fcntl(&observer, FcntlArg::F_GETFL).map_err(io::Error::from)?;
        let mut input = TerminalInput::from_owned_fd(pty.slave);
        let mut output = ReviewOutput::default();
        let shown = output.clone();
        let human = async {
            loop {
                if shown.text().contains(&format!("approve {approval_id}")) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
            let shown = shown.text();
            let rendered = if package {
                shown.contains(expected["unified_summary"].as_str().unwrap())
                    && ["repo", "tag", "commit"].into_iter().all(|field| {
                        shown.contains(expected["proposal"]["source"][field].as_str().unwrap())
                    })
                    && ["distribution", "version"]
                        .into_iter()
                        .all(|field| shown.contains(expected["proposal"][field].as_str().unwrap()))
                    && shown.contains(
                        "installation, activation and readiness require later host evidence.",
                    )
            } else {
                shown.contains(expected["unified_diff"].as_str().unwrap())
                    && shown.contains("This does not activate it.")
            };
            require(
                rendered && shown.contains(&host.origin),
                "protected presenter did not show the exact owner review and limit",
            )?;
            require(
                host.submission_count
                    .load(std::sync::atomic::Ordering::SeqCst)
                    == 0,
                "a change-request POST preceded protected approval",
            )?;
            let decision = format!("approve {approval_id}\n");
            require(
                write(&pty.master, decision.as_bytes()).map_err(io::Error::from)? == decision.len(),
                "isolated approval input was incomplete",
            )
        };
        // Only the owned PTY is read. The real protected presenter consumes
        // the fresh ID; neither decision nor code enters the model session.
        let interaction = timeout(BOUND, async {
            tokio::try_join!(
                async {
                    present_approval(prompt, &mut input, &mut output, std::future::pending())
                        .await
                        .map_err(|_| io::Error::other("protected approval failed"))
                },
                human
            )
        })
        .await;
        // timeout/try_join can drop a pending read. Observe synchronous cleanup
        // immediately, including the failure path, before releasing this PTY.
        input.check_cleanup()?;
        require(
            tcgetattr(&observer).map_err(io::Error::from)? == original_termios
                && fcntl(&observer, FcntlArg::F_GETFL).map_err(io::Error::from)? == original_flags,
            "protected terminal state was not restored",
        )?;
        let (accepted, ()) = interaction
            .map_err(|_| io::Error::other("protected approval exceeded its fixture deadline"))??;
        require(accepted, "protected approval was cancelled")?;
        require(
            output
                .text()
                .contains("Decision recorded. Waiting for the native outcome."),
            "protected presenter did not record the decision",
        )?;
        Ok(
            json!({"kind":"protected_approval","approval_id":approval_id,
            "session_id":session.id(),"host":host.origin,"base":expected["base"],
            "change_digest":expected["change_digest"],"unified_diff":expected["unified_diff"],
            "change_kind":if package { "new_extension" } else { "channel_config" },
            "proposal":expected["proposal"],"unified_summary":expected["unified_summary"],
            "decision":"approve","terminal_restored":true}),
        )
    }

    async fn run_turn(
        session: &ConnectSession,
        host: &Host,
        approvals: &LocalApprovals,
        presenter: &mut ApprovalPresenter,
        expected: &Value,
        prompt: &str,
        trace: &mut Trace,
    ) -> io::Result<()> {
        let mut events = session
            .events()
            .await
            .map_err(|_| io::Error::other("native event subscription failed"))?;
        trace.record(json!({"kind":"human_input","text":prompt}))?;
        let (admission, completion) = session
            .submit(prompt.into())
            .await
            .map_err(|_| io::Error::other("native input admission failed; no retry"))?;
        let wait = completion.wait();
        tokio::pin!(wait);
        let deadline = tokio::time::sleep(Duration::from_secs(300));
        tokio::pin!(deadline);
        let interrupt = tokio::signal::ctrl_c();
        tokio::pin!(interrupt);
        let mut first_failure = None;
        let mut cancelled = false;
        let mut stream_open = true;
        let mut approved = false;
        let outcome = loop {
            tokio::select! {
                biased;
                result = &mut wait => break result
                    .map_err(|_| io::Error::other("native completion waiter failed")),
                _ = &mut deadline, if !cancelled => {
                    first_failure = Some(io::Error::other("real-model turn exceeded its deadline"));
                }
                _ = &mut interrupt, if !cancelled => {
                    first_failure = Some(io::Error::other("real-model turn interrupted"));
                }
                prompt = presenter.next(), if !cancelled => {
                    let result = match prompt {
                        Some(prompt) if !approved => approve_exact_change(prompt, session, host, expected).await,
                        _ => Err(io::Error::other("missing or repeated protected approval")),
                    };
                    match result {
                        Ok(record) => {
                            approved = true;
                            if let Err(error) = trace.record(record) { first_failure = Some(error); }
                        }
                        Err(error) => first_failure = Some(error),
                    }
                }
                event = events.next(), if stream_open => match event {
                    Some(event) => {
                        if let Err(error) = trace.event(event)
                            && first_failure.is_none() { first_failure = Some(error); }
                    }
                    None => {
                        stream_open = false;
                        trace.complete = false;
                        if first_failure.is_none() {
                            first_failure = Some(io::Error::other("event stream ended before native completion"));
                        }
                    }
                },
            }
            if first_failure.is_some() && !cancelled {
                cancelled = true;
                approvals.retire();
                let cancellation = session.cancel(&admission).await;
                let _ = trace.record(json!({"kind":"exact_input_cancel",
                    "session_id":session.id(),"native_returned":cancellation.as_ref().ok(),
                    "ok":cancellation.is_ok()}));
                // Join completion even after cancellation fails. Root owns the
                // enclosing process deadline; no dropped close implies exit.
            }
        };
        while let Some(Some(event)) = events.next().now_or_never() {
            if let Err(error) = trace.event(event)
                && first_failure.is_none()
            {
                first_failure = Some(error);
            }
        }
        trace.record(json!({"kind":"native_completion","joined":true,
            "completed":matches!(&outcome, Ok(CompletionOutcome::Completed(_))),
            "protected_approval_recorded":approved,"cancellation_requested":cancelled}))?;
        if let Ok(CompletionOutcome::Completed(result)) = &outcome {
            trace.record(
                json!({"kind":"committed_answer","session_id":result.session_id,
                "text":result.text,"turns":result.turns,"tool_calls":result.tool_calls,
                "run_usage":result.run_usage,"cumulative_usage":result.usage,
                "terminal_cause_kind":result.terminal_cause_kind,"semantic_review":"pending"}),
            )?;
        }
        if let Some(error) = first_failure {
            return Err(error);
        }
        let CompletionOutcome::Completed(result) = outcome? else {
            return Err(io::Error::other(
                "real-model turn did not commit a result; no retry",
            ));
        };
        require(approved, "model did not request protected approval")?;
        require(
            !result.text.trim().is_empty() && result.terminal_cause_kind.is_none(),
            "native result is empty or ended at a terminal limit",
        )?;
        require(
            trace.complete && !trace.outside_ceiling,
            "incomplete evidence or a tool outside the direct frontend ceiling",
        )?;
        require(
            TOOLS.iter().all(|name| trace.successful.contains(*name)),
            "model did not complete all four frontend tools",
        )?;
        let status = trace
            .status
            .as_ref()
            .ok_or_else(|| io::Error::other("model did not receive request status"))?;
        require(
            status["state"] == "submitted"
                && status["request_id"] == expected["change_digest"]
                && (expected["kind"] != "new_extension" || status["kind"] == "new_extension")
                && status.get("activation_id").is_none(),
            "model status was not the correlated pending operator request",
        )
    }

    async fn finish_host(mut host: Host) -> io::Result<Vec<Request>> {
        if let Some(stop) = host.stop.take() {
            let _ = stop.send(());
        }
        let mut task = host
            .task
            .take()
            .ok_or_else(|| io::Error::other("synthetic host task custody is missing"))?;
        match timeout(BOUND * 2, &mut task).await {
            Ok(result) => {
                result.map_err(|_| io::Error::other("synthetic host task failed; joined"))
            }
            Err(_) => {
                task.abort();
                let _ = task.await;
                Err(io::Error::other(
                    "synthetic host task exceeded its deadline; aborted and joined",
                ))
            }
        }
    }

    #[tokio::test]
    #[ignore = "charged real-model judgment; root-selected native OpenAI credential and fresh private outside-checkout cwd required"]
    async fn real_agent_prepares_and_submits_frontend_for_operator_review() -> io::Result<()> {
        let new_extension = match std::env::var("MEERKAT_CONNECT_PAIRED_HOST_TRIAL") {
            Ok(value) if value == "new_extension" => true,
            Ok(value) if value == "channel_config" => false,
            Err(std::env::VarError::NotPresent) => false,
            _ => {
                return Err(io::Error::other(
                    "unsupported finite paired host trial scenario",
                ));
            }
        };
        let proposed = if new_extension {
            researched_new_extension_candidate()
        } else {
            candidate()
        };
        let prompt = if new_extension {
            NEW_EXTENSION_PROMPT
        } else {
            PROMPT
        };
        let current = std::env::current_dir()?;
        require(
            !current.join("Cargo.toml").exists() && !current.join(".git").exists(),
            "run from a fresh private directory outside the checkout",
        )?;
        let artifacts = current.join("paired-host-agent-judgment");
        fs::DirBuilder::new().mode(0o700).create(&artifacts)?;
        let mut trace = Trace {
            file: OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(artifacts.join("observations.jsonl"))?,
            successful: BTreeSet::new(),
            status: None,
            outside_ceiling: false,
            complete: true,
        };
        let mut config = Config {
            max_tokens: Some(4096),
            ..Default::default()
        };
        config.retry.max_retries = 0;
        config.model_fallback.enabled = Some(false);
        config.provider_tools.openai.web_search = false;
        config.provider_tools.anthropic.web_search = false;
        config.provider_tools.gemini.google_search = false;
        let resolved = resolve_create_session_model(&config, Default::default())
            .map_err(|_| io::Error::other("native model resolution failed"))?;
        trace.record(json!({"kind":"scope","pid":std::process::id(),
            "scope":"real_model_native_facade_synthetic_host_not_cli_or_live_host",
            "scenario":if new_extension { "known_pinned_new_extension_request_not_source_discovery" } else { "declared_channel_config" },
            "model":resolved.model,"provider":resolved.provider,
            "selection":"native Config::default; no client/model/provider override",
            "credential_handling":"native only; no value read, output or custom forwarding",
            "max_output_tokens":4096,"max_tokens":120000,"max_tool_calls":12,
            "native_lifetime_seconds":600,"native_turn_seconds":240,
            "presenter_deadline_seconds":300,"retries":0,"fallback":false,
            "provider_native_tools":"disabled","semantic_review":"pending"}))?;
        require(
            resolved.provider == Provider::OpenAI && resolved.model == "gpt-6-astra",
            "native default differs from the reviewed real-model profile; no call made",
        )?;
        let host = Host::with_package_mode(false, false, Some(201), new_extension).await;
        let expected = if new_extension {
            new_extension_receipt(proposed["new_extension"].clone())["change"].clone()
        } else {
            change_receipt(proposed["participants"]["approved_groups"].clone())["change"].clone()
        };
        let exercise = async {
            let binding = host.paired().await;
            let (approvals, mut presenter) = LocalApprovals::create_paired_host(&artifacts)
                .map_err(|_| io::Error::other("local consent owner construction failed"))?;
            let mut options = direct(&binding, None);
            options.build.budget_limits = Some(BudgetLimits {
                max_tokens: Some(120_000), max_tool_calls: Some(12),
                max_duration: Some(Duration::from_secs(600)),
                max_turn_duration: Some(Duration::from_secs(240)),
            });
            approvals.attach_paired_host_direct(&mut options, &binding)
                .map_err(|_| io::Error::other("direct consent composition failed"))?;
            require(options.build.mcp_servers.is_empty() && options.build.llm_client_override.is_none(),
                "trial unexpectedly contains service resources or an injected model")?;
            let storage = RealmOptions {
                context_root: current,
                state_root: Some(artifacts.join("state")),
                user_config_root: Some(artifacts.join("home")),
            }.resolve("paired-host-agent-judgment")
                .map_err(|_| io::Error::other("isolated native realm resolution failed"))?;
            let session = ConnectSession::open_with_config(storage, config, options).await
                .map_err(|_| io::Error::other("native open failed; auth diagnostic omitted"))?;
            let turn = async {
                approvals.bind_session(session.id())
                    .map_err(|_| io::Error::other("native consent session binding failed"))?;
                run_turn(&session, &host, &approvals, &mut presenter, &expected, prompt, &mut trace).await
            }.await;
            approvals.retire();
            let closed = session.close().await;
            trace.record(json!({"kind":"native_close","joined":true,"ok":closed.is_ok(),
                "turn_checks_passed":turn.is_ok(),"uncertain_dispatch":approvals.has_uncertain_dispatch()}))?;
            turn?;
            require(closed.is_ok(), "native close failed; no settlement inferred")?;
            require(!approvals.has_uncertain_dispatch(), "submission outcome remains uncertain")
        }.await;
        // Stop and join the owned HTTP task even if the model or consent path
        // fails. Never export Request.headers or pairing request bodies.
        let returned = finish_host(host).await;
        trace.record(json!({"kind":"synthetic_host_closed","joined":true,
            "ok":returned.is_ok(),"native_trial_passed":exercise.is_ok()}))?;
        let requests = match returned {
            Ok(requests) => requests,
            Err(error) => return exercise.and(Err(error)),
        };
        for request in &requests {
            trace.record(json!({"kind":"synthetic_host_request","method":request.method,
                "path":request.path,"body":if request.path.starts_with("/console/extensions/frontend/") {
                    Some(&request.body)
                } else { None }}))?;
        }
        exercise?;
        let frontend: Vec<_> = requests
            .iter()
            .filter(|request| request.path.starts_with("/console/extensions/frontend/"))
            .collect();
        let submit_path = "/console/extensions/frontend/change-requests";
        let posts: Vec<_> = frontend
            .iter()
            .filter(|request| request.method == "POST" && request.path == submit_path)
            .collect();
        require(
            posts.len() == 1
                && posts[0].body
                    == json!({
            "change_digest":expected["change_digest"],"change":expected}),
            "host did not receive exactly one original approved change",
        )?;
        let position = |method: &str, path: &str| {
            frontend
                .iter()
                .position(|request| request.method == method && request.path == path)
        };
        let inventory = position("GET", "/console/extensions/frontend/inventory");
        let prepare = position("POST", "/console/extensions/frontend/prepare");
        let submit = position("POST", submit_path);
        let status_path = format!(
            "{submit_path}/{}",
            expected["change_digest"].as_str().unwrap()
        );
        let status = position("GET", &status_path);
        require(
            matches!((inventory, prepare, submit, status),
            (Some(i), Some(p), Some(s), Some(r)) if i < p && p < s && s < r),
            "actual host calls did not follow inventory, preparation, approval submission and status",
        )?;
        require(
            frontend
                .iter()
                .filter(|request| request.path == "/console/extensions/frontend/prepare")
                .all(|request| request.body == proposed),
            "model proposed a candidate other than the exact pre-reviewed scenario",
        )?;
        trace.record(json!({"kind":"mechanical_acceptance","passed":true,
            "change_request_posts":posts.len(),"semantic_review":"pending",
            "answer_review":if new_extension {
                "Check known pinned package, pending host-owned build/review, unverified manifest/template/secrets, sender routing and no source-discovery/install/activation/readiness/delivery claim; no keyword grader."
            } else {
                "Check pending operator review, missing selected-mode token, sender routing and no activation/readiness/delivery claim; no keyword grader."
            }}))
    }
}

type DirectDispatcher = ExecutionPolicyGatedDispatcher<dyn AgentToolDispatcher>;

fn attach_direct(
    binding: &PairedHostFrontend,
) -> (
    tempfile::TempDir,
    LocalApprovals,
    meerkat_connect::approval::ApprovalPresenter,
    SessionId,
    Arc<DirectDispatcher>,
) {
    let directory = tempfile::tempdir().unwrap();
    let (approvals, presenter) = LocalApprovals::create_paired_host(directory.path()).unwrap();
    let mut options = direct(binding, None);
    assert!(options.build.mcp_servers.is_empty());
    approvals
        .attach_paired_host_direct(&mut options, binding)
        .unwrap();
    let session = SessionId::new();
    approvals.bind_session(&session).unwrap();
    let dispatcher = Arc::new(
        ExecutionPolicyGatedDispatcher::new(
            options.build.external_tools.unwrap(),
            ToolExecutionPolicy::resolve(options.build.tool_access_policy.unwrap()).unwrap(),
        )
        .with_dispatch_admission(options.build.tool_dispatch_admission.unwrap()),
    );
    (directory, approvals, presenter, session, dispatcher)
}

async fn direct_call(
    dispatcher: &DirectDispatcher,
    session: &SessionId,
    id: &str,
    name: &str,
    args: Value,
) -> Result<meerkat_core::ops::ToolDispatchOutcome, meerkat_core::ToolError> {
    let raw = serde_json::value::to_raw_value(&args).unwrap();
    dispatcher
        .dispatch_with_context(
            ToolCallView {
                id,
                name,
                args: &raw,
            },
            &ToolDispatchContext::default().with_runtime_identity(session.clone(), None),
        )
        .await
}

fn start_call(
    dispatcher: Arc<DirectDispatcher>,
    session: SessionId,
    id: &'static str,
    digest: Value,
) -> tokio::task::JoinHandle<Result<meerkat_core::ops::ToolDispatchOutcome, meerkat_core::ToolError>>
{
    tokio::spawn(async move {
        direct_call(
            dispatcher.as_ref(),
            &session,
            id,
            PAIRED_HOST_FRONTEND_SUBMIT,
            json!({"change_digest":digest}),
        )
        .await
    })
}

#[test]
fn direct_composition_has_no_mcp_preserves_caller_ceiling_and_cannot_substitute_host() {
    let host = PairedHostFrontend::new(PairedHostClient::new("http://127.0.0.1:1").unwrap());
    let other = PairedHostFrontend::new(PairedHostClient::new("http://127.0.0.1:2").unwrap());
    let mut options = direct(&host, Some(ToolAccessPolicy::ReadOnly));
    let policy =
        ToolExecutionPolicy::resolve(options.build.tool_access_policy.clone().unwrap()).unwrap();
    let tools = options.build.external_tools.as_ref().unwrap();
    for name in [
        PAIRED_HOST_FRONTEND_PREPARE,
        PAIRED_HOST_FRONTEND_INVENTORY,
        PAIRED_HOST_FRONTEND_STATUS,
    ] {
        assert!(policy.permits_call(name, tools.tool_mutation_class(name)));
    }
    assert!(!policy.permits_call(
        PAIRED_HOST_FRONTEND_SUBMIT,
        tools.tool_mutation_class(PAIRED_HOST_FRONTEND_SUBMIT)
    ));
    assert!(options.build.mcp_servers.is_empty());
    let directory = tempfile::tempdir().unwrap();
    let (approval, _) = LocalApprovals::create_paired_host(directory.path()).unwrap();
    assert!(
        approval
            .attach_paired_host_direct(&mut options, &other)
            .is_err()
    );
    approval
        .attach_paired_host_direct(&mut options, &host)
        .unwrap();
    let mut extra = SessionOptions::default();
    extra
        .build
        .mcp_servers
        .push(meerkat_core::McpServerConfig::stdio(
            "extra",
            "/fixture/unselected",
            Vec::new(),
            Default::default(),
        ));
    assert!(paired_host_frontend_session(extra, &host).is_err());
    let mut extra = SessionOptions::default();
    extra.build.keep_alive = true;
    assert!(paired_host_frontend_session(extra, &host).is_err());
}

#[tokio::test]
async fn exact_host_change_requires_fresh_terminal_decision_and_survives_later_prepare() {
    let host = Host::with_mode(false, false, Some(201)).await;
    let binding = host.paired().await;
    let (_directory, approvals, mut presenter, session, dispatcher) = attach_direct(&binding);
    let first =
        change_receipt(candidate()["participants"]["approved_groups"].clone())["change"].clone();
    let digest = first["change_digest"].clone();
    // A model-authored digest before prepare, a wrong session and injected body cannot dispatch.
    assert!(
        direct_call(
            dispatcher.as_ref(),
            &session,
            "before-prepare",
            PAIRED_HOST_FRONTEND_SUBMIT,
            json!({"change_digest":digest})
        )
        .await
        .is_err()
    );
    assert!(
        direct_call(
            dispatcher.as_ref(),
            &SessionId::new(),
            "wrong-session",
            PAIRED_HOST_FRONTEND_PREPARE,
            candidate()
        )
        .await
        .is_err()
    );
    direct_call(
        dispatcher.as_ref(),
        &session,
        "prepare-1",
        PAIRED_HOST_FRONTEND_PREPARE,
        candidate(),
    )
    .await
    .unwrap();
    assert!(
        direct_call(
            dispatcher.as_ref(),
            &session,
            "injected-change",
            PAIRED_HOST_FRONTEND_SUBMIT,
            json!({"change_digest":digest,"change":first})
        )
        .await
        .is_err()
    );
    let denied = start_call(
        dispatcher.clone(),
        session.clone(),
        "declined",
        digest.clone(),
    );
    let mut prompt = timeout(BOUND, presenter.next()).await.unwrap().unwrap();
    assert_eq!(
        prompt.record().proposed_action.body.as_ref().unwrap()["host"],
        host.origin
    );
    prompt.decide(ApprovalDecision::Deny).unwrap();
    assert!(timeout(BOUND, denied).await.unwrap().unwrap().is_err());
    assert!(!approvals.has_uncertain_dispatch());

    let accepted = start_call(
        dispatcher.clone(),
        session.clone(),
        "approved",
        digest.clone(),
    );
    let mut prompt = timeout(BOUND, presenter.next()).await.unwrap().unwrap();
    assert_eq!(
        prompt.record().proposed_action.body.as_ref().unwrap()["base"],
        first["base"]
    );
    assert_eq!(
        prompt.record().proposed_action.body.as_ref().unwrap()["unified_diff"],
        first["unified_diff"]
    );
    // Another preparation while the decision is pending must not replace the retained object.
    let mut second = candidate();
    second["participants"]["approved_groups"] = json!(["-100999"]);
    direct_call(
        dispatcher.as_ref(),
        &session,
        "prepare-2",
        PAIRED_HOST_FRONTEND_PREPARE,
        second.clone(),
    )
    .await
    .unwrap();
    prompt.decide(ApprovalDecision::Approve).unwrap();
    assert!(prompt.decide(ApprovalDecision::Approve).is_err());
    timeout(BOUND, accepted).await.unwrap().unwrap().unwrap();
    assert!(!approvals.has_uncertain_dispatch());
    // A consumed call cannot be replayed, even after a known response.
    assert!(
        direct_call(
            dispatcher.as_ref(),
            &session,
            "approved",
            PAIRED_HOST_FRONTEND_SUBMIT,
            json!({"change_digest":digest})
        )
        .await
        .is_err()
    );
    // A different prepared request remains possible after settlement, but needs another decision.
    let second_change =
        change_receipt(second["participants"]["approved_groups"].clone())["change"].clone();
    let next = start_call(
        dispatcher.clone(),
        session.clone(),
        "second-request",
        second_change["change_digest"].clone(),
    );
    let mut next_prompt = timeout(BOUND, presenter.next()).await.unwrap().unwrap();
    assert_eq!(
        next_prompt.record().proposed_action.body.as_ref().unwrap()["change_digest"],
        second_change["change_digest"]
    );
    next_prompt.decide(ApprovalDecision::Approve).unwrap();
    timeout(BOUND, next).await.unwrap().unwrap().unwrap();
    approvals.retire();
    let requests = host.finish().await;
    let posts: Vec<_> = requests
        .iter()
        .filter(|request| request.path == "/console/extensions/frontend/change-requests")
        .collect();
    assert_eq!(
        posts.len(),
        2,
        "no POST for absent, injected, denied or replayed calls"
    );
    assert_eq!(
        posts[0].body,
        json!({"change_digest":digest,"change":first})
    );
    assert_eq!(posts[1].body["change"], second_change);
}

#[tokio::test]
async fn new_package_schema_is_closed_and_prior_status_never_restores_submission_authority() {
    let host = Host::with_package_mode(false, false, Some(201), true).await;
    let binding = host.paired().await;
    let (_directory, approvals, mut presenter, session, dispatcher) = attach_direct(&binding);
    let proposal = new_extension_candidate();
    let change = new_extension_receipt(proposal["new_extension"].clone())["change"].clone();
    let reference = json!({"change_digest":change["change_digest"]});

    // A fresh authenticated instance may observe a prior owner's receipt, but
    // it has no retained preparation and no approval for another submission.
    let observed = direct_call(
        dispatcher.as_ref(),
        &session,
        "prior",
        PAIRED_HOST_FRONTEND_STATUS,
        reference.clone(),
    )
    .await
    .unwrap();
    let observed: Value = serde_json::from_str(&observed.result.text_content()).unwrap();
    assert_eq!(observed["kind"], "new_extension");
    assert_eq!(observed["state"], "submitted");
    assert!(
        direct_call(
            dispatcher.as_ref(),
            &session,
            "unprepared",
            PAIRED_HOST_FRONTEND_SUBMIT,
            reference.clone()
        )
        .await
        .is_err()
    );

    let mut mixed = proposal.clone();
    mixed["extension_id"] = json!("telegram");
    let mut token = proposal.clone();
    token["new_extension"]["token"] = json!("fixture-never-send");
    let mut source = proposal.clone();
    source["new_extension"]["source"]["authorization"] = json!("fixture-never-send");
    let mut channel = proposal.clone();
    channel["new_extension"]["channel"]["target_identity"] = json!("arbitrary-agent");
    for invalid in [mixed, token, source, channel] {
        assert!(
            direct_call(
                dispatcher.as_ref(),
                &session,
                "invalid",
                PAIRED_HOST_FRONTEND_PREPARE,
                invalid
            )
            .await
            .is_err()
        );
    }
    direct_call(
        dispatcher.as_ref(),
        &session,
        "prepared",
        PAIRED_HOST_FRONTEND_PREPARE,
        proposal.clone(),
    )
    .await
    .unwrap();
    assert!(
        direct_call(
            dispatcher.as_ref(),
            &session,
            "injected",
            PAIRED_HOST_FRONTEND_SUBMIT,
            json!({"change_digest":change["change_digest"],"change":change})
        )
        .await
        .is_err()
    );

    // The same prepared package cannot turn a caller's read ceiling into consent.
    let read_only = direct(&binding, Some(ToolAccessPolicy::ReadOnly));
    let read_only = ExecutionPolicyGatedDispatcher::new(
        read_only.build.external_tools.unwrap(),
        ToolExecutionPolicy::resolve(read_only.build.tool_access_policy.unwrap()).unwrap(),
    );
    assert!(
        direct_call(
            &read_only,
            &session,
            "read-only",
            PAIRED_HOST_FRONTEND_SUBMIT,
            reference.clone()
        )
        .await
        .is_err()
    );
    assert!(
        timeout(Duration::from_millis(20), presenter.next())
            .await
            .is_err()
    );
    assert_eq!(
        host.submission_count
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );

    let denied = start_call(
        dispatcher,
        session,
        "declined-package",
        reference["change_digest"].clone(),
    );
    let mut prompt = timeout(BOUND, presenter.next()).await.unwrap().unwrap();
    assert_eq!(
        prompt.record().proposed_action.body.as_ref().unwrap()["proposal"],
        proposal["new_extension"]
    );
    prompt.decide(ApprovalDecision::Deny).unwrap();
    assert!(timeout(BOUND, denied).await.unwrap().unwrap().is_err());
    approvals.retire();
    let requests = host.finish().await;
    assert_eq!(
        requests.len(),
        4,
        "pairing, prior status and one valid prepare only"
    );
    assert!(
        requests
            .iter()
            .all(|request| request.path != "/console/extensions/frontend/change-requests")
    );
    assert_eq!(requests[3].body, proposal);
}

#[tokio::test]
async fn lost_post_response_allows_only_retained_status_and_not_found_stays_unresolved() {
    for (new_extension, response) in [(false, 0), (false, 500), (true, 0), (true, 500)] {
        let host = Host::with_package_mode(false, false, Some(response), new_extension).await;
        let binding = host.paired().await;
        let (_directory, approvals, mut presenter, session, dispatcher) = attach_direct(&binding);
        let proposed = if new_extension {
            new_extension_candidate()
        } else {
            candidate()
        };
        let digest = if new_extension {
            new_extension_receipt(proposed["new_extension"].clone())["change"]["change_digest"]
                .clone()
        } else {
            change_receipt(proposed["participants"]["approved_groups"].clone())["change"]["change_digest"].clone()
        };
        direct_call(
            dispatcher.as_ref(),
            &session,
            "prepare",
            PAIRED_HOST_FRONTEND_PREPARE,
            proposed.clone(),
        )
        .await
        .unwrap();
        let submit = start_call(
            dispatcher.clone(),
            session.clone(),
            "lost-submit",
            digest.clone(),
        );
        let mut prompt = timeout(BOUND, presenter.next()).await.unwrap().unwrap();
        prompt.decide(ApprovalDecision::Approve).unwrap();
        assert!(timeout(BOUND, submit).await.unwrap().unwrap().is_err());
        assert!(approvals.has_uncertain_dispatch());
        for (name, arguments) in [
            (PAIRED_HOST_FRONTEND_SUBMIT, json!({"change_digest":digest})),
            (PAIRED_HOST_FRONTEND_PREPARE, proposed),
            (PAIRED_HOST_FRONTEND_INVENTORY, json!({})),
        ] {
            assert!(
                direct_call(dispatcher.as_ref(), &session, "after-loss", name, arguments)
                    .await
                    .is_err()
            );
        }
        for id in ["read-1", "read-2"] {
            direct_call(
                dispatcher.as_ref(),
                &session,
                id,
                PAIRED_HOST_FRONTEND_STATUS,
                json!({"change_digest":digest}),
            )
            .await
            .unwrap();
            assert!(
                approvals.has_uncertain_dispatch(),
                "readback never restores spent authority"
            );
        }
        assert!(
            timeout(Duration::from_millis(20), presenter.next())
                .await
                .is_err()
        );
        approvals.retire();
        let requests = host.finish().await;
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.path == "/console/extensions/frontend/change-requests")
                .count(),
            1
        );
        assert_eq!(
            requests
                .iter()
                .filter(|request| request
                    .path
                    .starts_with("/console/extensions/frontend/change-requests/"))
                .count(),
            2
        );
    }
}

#[tokio::test]
async fn cancelled_decision_cannot_release_submission_and_another_host_cannot_use_its_reference() {
    let host = Host::with_mode(false, false, Some(201)).await;
    let other = Host::with_mode(false, false, Some(201)).await;
    let binding = host.paired().await;
    let other_binding = other.paired().await;
    let (_directory, approvals, mut presenter, session, dispatcher) = attach_direct(&binding);
    let (_other_dir, other_approvals, mut other_presenter, other_session, other_dispatcher) =
        attach_direct(&other_binding);
    let digest = change_receipt(candidate()["participants"]["approved_groups"].clone())["change"]["change_digest"].clone();
    direct_call(
        dispatcher.as_ref(),
        &session,
        "prepare",
        PAIRED_HOST_FRONTEND_PREPARE,
        candidate(),
    )
    .await
    .unwrap();
    assert!(
        direct_call(
            other_dispatcher.as_ref(),
            &other_session,
            "foreign",
            PAIRED_HOST_FRONTEND_SUBMIT,
            json!({"change_digest":digest})
        )
        .await
        .is_err()
    );
    assert!(
        timeout(Duration::from_millis(20), other_presenter.next())
            .await
            .is_err()
    );
    let pending = start_call(dispatcher.clone(), session.clone(), "cancelled", digest);
    let mut prompt = timeout(BOUND, presenter.next()).await.unwrap().unwrap();
    pending.abort();
    assert!(pending.await.unwrap_err().is_cancelled());
    assert!(prompt.decide(ApprovalDecision::Approve).is_err());
    assert!(!approvals.has_uncertain_dispatch());
    approvals.retire();
    other_approvals.retire();
    for requests in [host.finish().await, other.finish().await] {
        assert!(
            !requests
                .iter()
                .any(|request| request.path == "/console/extensions/frontend/change-requests")
        );
    }
}

#[tokio::test]
async fn cancellation_after_post_admission_remains_uncertain_and_never_resends() {
    let host = Host::with_mode(false, false, Some(202)).await;
    let binding = host.paired().await;
    let (_directory, approvals, mut presenter, session, dispatcher) = attach_direct(&binding);
    let digest = change_receipt(candidate()["participants"]["approved_groups"].clone())["change"]["change_digest"].clone();
    direct_call(
        dispatcher.as_ref(),
        &session,
        "prepare",
        PAIRED_HOST_FRONTEND_PREPARE,
        candidate(),
    )
    .await
    .unwrap();
    let pending = start_call(
        dispatcher.clone(),
        session.clone(),
        "cancel-after-post",
        digest,
    );
    let mut prompt = timeout(BOUND, presenter.next()).await.unwrap().unwrap();
    prompt.decide(ApprovalDecision::Approve).unwrap();
    timeout(BOUND, host.inventory_arrived.notified())
        .await
        .unwrap();
    pending.abort();
    assert!(pending.await.unwrap_err().is_cancelled());
    assert!(approvals.has_uncertain_dispatch());
    approvals.retire();
    assert!(
        approvals.has_uncertain_dispatch(),
        "cancellation cannot claim no effect"
    );
    let requests = host.finish().await;
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.path == "/console/extensions/frontend/change-requests")
            .count(),
        1
    );
}

#[tokio::test]
async fn conflicting_host_payload_under_retained_digest_is_refused() {
    let host = Host::with_mode(false, false, Some(409)).await;
    let binding = host.paired().await;
    let (_directory, approvals, mut presenter, session, dispatcher) = attach_direct(&binding);
    direct_call(
        dispatcher.as_ref(),
        &session,
        "original",
        PAIRED_HOST_FRONTEND_PREPARE,
        candidate(),
    )
    .await
    .unwrap();
    let mut changed = candidate();
    changed["participants"]["approved_groups"] = json!(["-100different"]);
    assert!(
        direct_call(
            dispatcher.as_ref(),
            &session,
            "contradiction",
            PAIRED_HOST_FRONTEND_PREPARE,
            changed
        )
        .await
        .is_err()
    );
    assert!(
        timeout(Duration::from_millis(20), presenter.next())
            .await
            .is_err()
    );
    approvals.retire();
    let requests = host.finish().await;
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.path == "/console/extensions/frontend/prepare")
            .count(),
        2
    );
    assert!(
        !requests
            .iter()
            .any(|request| request.path == "/console/extensions/frontend/change-requests")
    );
}

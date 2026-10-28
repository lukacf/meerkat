//! Prior request readback through fresh native sessions and captured clients.
//! Synthetic loopback hosts only; no recovered approval or live activation.

use std::{
    collections::BTreeMap,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use futures_util::Stream;
use meerkat::{LlmClient, LlmError, LlmEvent, LlmRequest};
use meerkat_connect::{
    ConnectSession, RealmOptions, SessionOptions,
    approval::LocalApprovals,
    paired_host::{PairedHostClient, PairingCode},
    paired_host_tool::{
        PAIRED_HOST_FRONTEND_PREPARE, PAIRED_HOST_FRONTEND_STATUS, PAIRED_HOST_FRONTEND_SUBMIT,
        PairedHostFrontend,
    },
    setup::paired_host_frontend_session,
};
use meerkat_core::{
    AgentToolDispatcher, ApprovalDecision, Message, Provider, SessionId, ToolError,
    ToolExecutionPolicy, ToolResult,
    agent::ToolDispatchContext,
    ops::{ToolAccessPolicy, ToolDispatchOutcome},
    tool_execution_policy::ExecutionPolicyGatedDispatcher,
    types::ToolCallView,
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::timeout,
};

const BOUND: Duration = Duration::from_secs(5);
const CHANGES: &str = "/console/extensions/frontend/change-requests";
const DEVICE: &str = "fixture-private-status-device";

fn digest(character: char) -> String {
    format!(
        "paired-host-change-v1:sha256:{}",
        character.to_string().repeat(64)
    )
}

struct Reply {
    method: &'static str,
    path: String,
    status: u16,
    body: Value,
}

impl Reply {
    fn read(reference: &str, status: u16, body: Value) -> Self {
        Self {
            method: "GET",
            path: format!("{CHANGES}/{reference}"),
            status,
            body,
        }
    }

    fn pairing() -> [Self; 2] {
        ["request", "confirm"].map(|action| Self {
            method: "POST",
            path: format!("/console/pair/{action}"),
            status: 200,
            body: json!({"ok":true}),
        })
    }
}

struct Host {
    origin: String,
    task: Option<tokio::task::JoinHandle<Vec<Value>>>,
}

impl Host {
    async fn new(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let expected_origin = origin.clone();
        let task = tokio::spawn(async move {
            let mut requests = Vec::new();
            for reply in replies {
                let (mut stream, _) = timeout(BOUND, listener.accept()).await.unwrap().unwrap();
                let (method, path, headers, body) =
                    timeout(BOUND, read_request(&mut stream)).await.unwrap();
                assert_eq!(method, reply.method);
                assert_eq!(path, reply.path);
                if !path.starts_with("/console/pair/") {
                    assert_eq!(headers["origin"], expected_origin);
                    assert!(headers["cookie"].contains(&format!("paired_host_device={DEVICE}")));
                    if method == "GET" {
                        assert_eq!(headers["cache-control"], "no-store");
                    } else {
                        assert_eq!(headers["content-type"], "application/json");
                    }
                }
                requests.push(body);
                // Deliberately lose the submitted response, after observing its body.
                if reply.status == 0 {
                    continue;
                }
                let cookie = match path.as_str() {
                    "/console/pair/request" => "Set-Cookie: paired_host_pair=fixture-private-status-pair; Path=/console/pair; HttpOnly; SameSite=Strict\r\n".into(),
                    "/console/pair/confirm" => format!("Set-Cookie: paired_host_device={DEVICE}; Path=/; HttpOnly; SameSite=Lax\r\nSet-Cookie: paired_host_pair=; Max-Age=0; Path=/console/pair\r\n"),
                    _ => String::new(),
                };
                let bytes = serde_json::to_vec(&reply.body).unwrap();
                let head = format!(
                    "HTTP/1.1 {} Fixture\r\nConnection: close\r\nContent-Type: application/json\r\nCache-Control: no-store\r\n{cookie}Content-Length: {}\r\n\r\n",
                    reply.status,
                    bytes.len()
                );
                timeout(BOUND, async {
                    stream.write_all(head.as_bytes()).await.unwrap();
                    stream.write_all(&bytes).await.unwrap();
                    stream.shutdown().await.unwrap();
                })
                .await
                .unwrap();
            }
            requests
        });
        Self {
            origin,
            task: Some(task),
        }
    }

    async fn paired(&self) -> PairedHostFrontend {
        let mut client = PairedHostClient::new(&self.origin).unwrap();
        client
            .request_pairing("Prior request fixture")
            .await
            .unwrap();
        client
            .confirm_pairing(PairingCode::new("421735".into()).unwrap())
            .await
            .unwrap();
        PairedHostFrontend::new(client)
    }

    async fn finish(mut self) -> Vec<Value> {
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

async fn read_request(stream: &mut TcpStream) -> (String, String, BTreeMap<String, String>, Value) {
    let mut bytes = Vec::new();
    let boundary = loop {
        if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            break index + 4;
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
    let method = first.next().unwrap().to_owned();
    let path = first.next().unwrap().to_owned();
    let headers: BTreeMap<_, _> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    let length: usize = headers
        .get("content-length")
        .map(|value| value.parse().unwrap())
        .unwrap_or(0);
    assert!(length <= 64 * 1024);
    while bytes.len() < boundary + length {
        let mut buffer = [0; 4096];
        let count = stream.read(&mut buffer).await.unwrap();
        assert!(count > 0);
        bytes.extend_from_slice(&buffer[..count]);
    }
    let body = if length == 0 {
        Value::Null
    } else {
        serde_json::from_slice(&bytes[boundary..boundary + length]).unwrap()
    };
    (method, path, headers, body)
}

struct ReadPriorRequest {
    inner: meerkat_client::TestClient,
    reference: String,
    issued: AtomicUsize,
    results: Mutex<BTreeMap<String, ToolResult>>,
}

#[async_trait]
impl LlmClient for ReadPriorRequest {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        self.inner.project_replay_messages(messages)
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        let visible = serde_json::to_string(&request.messages).unwrap();
        assert!(
            visible.contains(&self.reference),
            "request reference comes from the person"
        );
        for private in [DEVICE, "421735", "fixture-private-status-pair"] {
            assert!(!visible.contains(private));
        }
        for message in &request.messages {
            if let Message::ToolResults { results, .. } = message {
                for result in results {
                    self.results
                        .lock()
                        .unwrap()
                        .insert(result.tool_use_id.clone(), result.clone());
                }
            }
        }
        let (id, name) = match self.issued.fetch_add(1, Ordering::SeqCst) {
            0 => ("prior-status", PAIRED_HOST_FRONTEND_STATUS),
            // Deliberately try treating readback as submission authority.
            1 => ("unprepared-submit", PAIRED_HOST_FRONTEND_SUBMIT),
            _ => return self.inner.stream(request),
        };
        Box::pin(futures_util::stream::iter([
            Ok(LlmEvent::ToolCallComplete {
                id: id.into(),
                name: name.into(),
                args: json!({"change_digest":self.reference}),
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
async fn fresh_native_sessions_read_prior_requests_without_recovering_submission_authority() {
    let reference = digest('1');
    let prepared = json!({"state":"prepared","base_current":false,"change":{
        "change_digest":reference,
        "base":{"activation_id":"214","policy_digest":"sha256:fixture","ledger_digest":"sha256:fixture"},
        "ledger_patch":{"path":"deployment/extensions.toml","extension_id":"telegram","channel_id":"telegram:primary",
            "before":null,"after":{"id":"telegram:primary","ingress":"polling","approved_groups":[]}},
        "unified_diff":"fixture exact draft diff"}});
    let mut replies = Vec::new();
    for state in ["submitted", "applied", "prepared"] {
        replies.extend(Reply::pairing());
        let mut observed = json!({"request_id":reference,"state":state,"submitted_at":"2026-10-07T00:00:00+00:00"});
        if state == "applied" {
            observed["activation_id"] = json!("fixture-activation-215");
        } else if state == "prepared" {
            observed = prepared.clone();
        }
        replies.push(Reply::read(&reference, 200, observed));
    }
    let host = Host::new(replies).await;
    let directory = tempfile::tempdir().unwrap();
    let mut prior_session = None;
    for (index, state) in ["submitted", "applied", "prepared"].into_iter().enumerate() {
        // Each connection is freshly paired; no retained candidate, approval,
        // request map or previous ConnectSession is carried into the next one.
        let binding = host.paired().await;
        let (approvals, mut presenter) =
            LocalApprovals::create_paired_host(directory.path()).unwrap();
        let mut initial = SessionOptions::default();
        if index == 1 {
            initial.build.tool_access_policy = Some(ToolAccessPolicy::ReadOnly);
        }
        let mut options = paired_host_frontend_session(initial, &binding).unwrap();
        approvals
            .attach_paired_host_direct(&mut options, &binding)
            .unwrap();
        if index == 1 {
            let policy =
                ToolExecutionPolicy::resolve(options.build.tool_access_policy.clone().unwrap())
                    .unwrap();
            let tools = options.build.external_tools.as_ref().unwrap();
            assert!(!policy.permits_call(
                PAIRED_HOST_FRONTEND_SUBMIT,
                tools.tool_mutation_class(PAIRED_HOST_FRONTEND_SUBMIT)
            ));
        }
        let probe = Arc::new(ReadPriorRequest {
            inner: meerkat_client::TestClient::for_provider(Provider::OpenAI),
            reference: reference.clone(),
            issued: AtomicUsize::new(0),
            results: Mutex::new(BTreeMap::new()),
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
        .resolve("prior-paired-host-request")
        .unwrap();
        let session = Box::pin(ConnectSession::open_with_config(
            storage,
            meerkat_core::Config::default(),
            options,
        ))
        .await
        .unwrap();
        if let Some(previous) = &prior_session {
            assert_ne!(session.id(), previous);
        }
        prior_session = Some(session.id().clone());
        approvals.bind_session(session.id()).unwrap();
        let (admission, completion) = session
            .submit(format!(
                "Please check my earlier paired host request {reference}."
            ))
            .await
            .unwrap();
        let outcome = timeout(Duration::from_secs(20), completion.wait()).await;
        if outcome.is_err() {
            approvals.retire();
            session.cancel(&admission).await.unwrap();
        }
        approvals.retire();
        session.close().await.unwrap();
        assert!(matches!(
            outcome.unwrap().unwrap(),
            meerkat_runtime::completion::CompletionOutcome::Completed(_)
        ));
        assert!(!approvals.has_uncertain_dispatch());
        assert!(
            timeout(Duration::from_millis(10), presenter.next())
                .await
                .is_err(),
            "readback cannot create an approval"
        );
        let results = probe.results.lock().unwrap();
        assert!(!results["prior-status"].is_error);
        let observed: Value =
            serde_json::from_str(&results["prior-status"].text_content()).unwrap();
        assert_eq!(observed["state"], state);
        if state == "prepared" {
            assert_eq!(observed, prepared);
            assert!(observed.get("request_id").is_none());
        } else {
            assert_eq!(observed["request_id"], reference);
        }
        assert_eq!(observed.get("activation_id").is_some(), state == "applied");
        assert!(results["unprepared-submit"].is_error);
    }
    assert_eq!(
        host.finish().await.len(),
        9,
        "only three fresh pairings and three GETs"
    );
}

type Dispatcher = ExecutionPolicyGatedDispatcher<dyn AgentToolDispatcher>;

async fn call(
    dispatcher: &Dispatcher,
    session: &SessionId,
    id: &str,
    name: &str,
    args: Value,
) -> Result<ToolDispatchOutcome, ToolError> {
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

#[tokio::test]
async fn new_extension_attempt_readback_keeps_lost_submission_spent_and_status_only() {
    let own = digest('6');
    let proposal = json!({"distribution":"example-ext-signal","version":"0.1.0",
        "source":{"repo":"https://github.com/fixture/example-signal.git","tag":"v0.1.0","commit":"a".repeat(40)},
        "channel":{"ingress":"polling","approved_groups":["fixture-group"]}});
    let change = json!({"change_digest":own,"kind":"new_extension",
        "base":{"activation_id":"214","policy_digest":"fixture-policy","ledger_digest":"fixture-ledger"},
        "proposal":proposal,"unverified":["manifest","facades","channel_template","secrets"],
        "prerequisites":[{"kind":"build_and_review","state":"pending"}],
        "unified_summary":"Synthetic pinned new extension; submitting activates nothing\n"});
    let progress = |attempt| {
        json!({"request_id":own,"submitted_at":"original-request-time",
        "kind":"new_extension","state":"submitted","attempt":attempt,
        "staged_at":null,"built":null,"prerequisites":[{"kind":"build_and_review","state":"pending"}]})
    };
    let mut replies = Reply::pairing().into_iter().collect::<Vec<_>>();
    replies.extend([
        Reply {
            method: "POST",
            path: "/console/extensions/frontend/prepare".into(),
            status: 200,
            body: json!({"valid":true,"problems":[],"change":change}),
        },
        Reply {
            method: "POST",
            path: CHANGES.into(),
            status: 0,
            body: Value::Null,
        },
        Reply::read(&own, 200, progress(2)),
        Reply::read(&own, 200, progress(3)),
    ]);
    let host = Host::new(replies).await;
    let binding = host.paired().await;
    let directory = tempfile::tempdir().unwrap();
    let (approvals, mut presenter) = LocalApprovals::create_paired_host(directory.path()).unwrap();
    let mut options = paired_host_frontend_session(SessionOptions::default(), &binding).unwrap();
    approvals
        .attach_paired_host_direct(&mut options, &binding)
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
    call(
        &dispatcher,
        &session,
        "prepare",
        PAIRED_HOST_FRONTEND_PREPARE,
        json!({"new_extension":proposal}),
    )
    .await
    .unwrap();
    let task_dispatcher = dispatcher.clone();
    let task_session = session.clone();
    let task_digest = own.clone();
    let submission = tokio::spawn(async move {
        call(
            &task_dispatcher,
            &task_session,
            "spent-submit",
            PAIRED_HOST_FRONTEND_SUBMIT,
            json!({"change_digest":task_digest}),
        )
        .await
    });
    let mut prompt = timeout(BOUND, presenter.next()).await.unwrap().unwrap();
    let approved_body = prompt
        .record()
        .proposed_action
        .body
        .as_ref()
        .unwrap()
        .clone();
    assert_eq!(approved_body["change_digest"], own);
    assert_eq!(approved_body["proposal"], proposal);
    assert!(approved_body.get("attempt").is_none());
    prompt.decide(ApprovalDecision::Approve).unwrap();
    assert!(timeout(BOUND, submission).await.unwrap().unwrap().is_err());
    for attempt in [2, 3] {
        let observed = call(
            &dispatcher,
            &session,
            "same-request-status",
            PAIRED_HOST_FRONTEND_STATUS,
            json!({"change_digest":own}),
        )
        .await
        .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&observed.result.text_content()).unwrap(),
            progress(attempt)
        );
        assert!(
            approvals.has_uncertain_dispatch(),
            "newer host observations cannot settle the lost POST"
        );
        assert_eq!(
            prompt.record().proposed_action.body.as_ref().unwrap(),
            &approved_body
        );
        for id in ["spent-submit", "fresh-submit"] {
            assert!(matches!(
                call(
                    &dispatcher,
                    &session,
                    id,
                    PAIRED_HOST_FRONTEND_SUBMIT,
                    json!({"change_digest":own})
                )
                .await,
                Err(ToolError::AccessDenied { .. })
            ));
        }
        assert!(matches!(
            call(
                &dispatcher,
                &session,
                "reprepare",
                PAIRED_HOST_FRONTEND_PREPARE,
                json!({"new_extension":proposal})
            )
            .await,
            Err(ToolError::AccessDenied { .. })
        ));
    }
    assert!(
        timeout(Duration::from_millis(10), presenter.next())
            .await
            .is_err()
    );
    approvals.retire();
    let requests = host.finish().await;
    assert_eq!(
        requests.len(),
        6,
        "pairing, one prepare, one lost POST and two readbacks only"
    );
    assert_eq!(requests[3], json!({"change_digest":own,"change":change}));
    assert!(requests[4..].iter().all(|body| body.is_null()));
}

#[tokio::test]
async fn unrelated_and_unknown_status_cannot_clear_an_uncertain_submission() {
    let own = digest('2');
    let other = digest('3');
    let missing = digest('4');
    let forbidden = digest('5');
    // Raw channel rows are opaque owner data to Connect. This synthetic receipt
    // is sufficient for the immutable transport/approval contract being tested.
    let change = json!({"change_digest":own,
        "base":{"activation_id":"214","policy_digest":"sha256:fixture","ledger_digest":format!("sha256:{}", "1".repeat(64))},
        "ledger_patch":{"path":"deployment/extensions.toml","extension_id":"telegram","channel_id":"telegram:primary",
            "before":{"id":"telegram:primary","ingress":"polling","approved_groups":[]},
            "after":{"id":"telegram:primary","ingress":"polling","approved_groups":["-1001234567890"]}},
        "unified_diff":"--- a/deployment/extensions.toml\n+++ b/deployment/extensions.toml\n@@ -1 +1 @@\n-approved_groups = []\n+approved_groups = [\"-1001234567890\"]\n"});
    let mut replies = Reply::pairing().into_iter().collect::<Vec<_>>();
    replies.extend([
        Reply { method: "POST", path: "/console/extensions/frontend/prepare".into(), status: 200,
            body: json!({"valid":true,"problems":[],"missing_prerequisites":[],"compiled_is_not_ready":true,
                "current":{"activation_id":"214","policy_digest":"sha256:fixture"},
                "candidate":{"channel_row_digest":"sha256:row","policy_digest":"sha256:fixture","compile_depth":"policy_bundle"},"change":change}) },
        Reply { method: "POST", path: CHANGES.into(), status: 0, body: Value::Null },
        Reply::read(&own, 404, json!({"state":"not_found"})),
        Reply::read(&own, 200, json!({"state":"prepared","change":change,"base_current":true})),
    ]);
    let host = Host::new(replies).await;
    let binding = host.paired().await;
    let directory = tempfile::tempdir().unwrap();
    let (approvals, mut presenter) = LocalApprovals::create_paired_host(directory.path()).unwrap();
    let mut options = paired_host_frontend_session(SessionOptions::default(), &binding).unwrap();
    approvals
        .attach_paired_host_direct(&mut options, &binding)
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
    for (identity, arguments) in [
        (SessionId::new(), json!({"change_digest":other})),
        (
            session.clone(),
            json!({"change_digest":other,"origin":"http://127.0.0.1:1"}),
        ),
        (session.clone(), json!({"change_digest":"not-a-request-id"})),
    ] {
        assert!(
            call(
                &dispatcher,
                &identity,
                "invalid-status",
                PAIRED_HOST_FRONTEND_STATUS,
                arguments
            )
            .await
            .is_err()
        );
    }
    let candidate = json!({"extension_id":"telegram","channel_id":"telegram:primary","ingress":"polling",
        "participants":{"approved_groups":["-1001234567890"]}});
    call(
        &dispatcher,
        &session,
        "prepare",
        PAIRED_HOST_FRONTEND_PREPARE,
        candidate.clone(),
    )
    .await
    .unwrap();
    let task_dispatcher = dispatcher.clone();
    let task_session = session.clone();
    let task_digest = own.clone();
    let submission = tokio::spawn(async move {
        call(
            &task_dispatcher,
            &task_session,
            "spent-submit",
            PAIRED_HOST_FRONTEND_SUBMIT,
            json!({"change_digest":task_digest}),
        )
        .await
    });
    let mut prompt = timeout(BOUND, presenter.next()).await.unwrap().unwrap();
    assert_eq!(
        prompt.record().proposed_action.body.as_ref().unwrap()["change_digest"],
        own
    );
    prompt.decide(ApprovalDecision::Approve).unwrap();
    assert!(timeout(BOUND, submission).await.unwrap().unwrap().is_err());
    for reference in [&other, &missing, &forbidden] {
        let observed = call(
            &dispatcher,
            &session,
            "read",
            PAIRED_HOST_FRONTEND_STATUS,
            json!({"change_digest":reference}),
        )
        .await;
        assert!(
            matches!(observed, Err(ToolError::AccessDenied { .. })),
            "an unrelated digest must be refused by admission before host HTTP"
        );
        assert!(
            approvals.has_uncertain_dispatch(),
            "readback never settles the uncertain POST"
        );
        for id in ["spent-submit", "new-submit"] {
            assert!(
                call(
                    &dispatcher,
                    &session,
                    id,
                    PAIRED_HOST_FRONTEND_SUBMIT,
                    json!({"change_digest":own})
                )
                .await
                .is_err()
            );
        }
        assert!(
            call(
                &dispatcher,
                &session,
                "reprepare",
                PAIRED_HOST_FRONTEND_PREPARE,
                candidate.clone()
            )
            .await
            .is_err()
        );
    }
    let observed = call(
        &dispatcher,
        &session,
        "own-status",
        PAIRED_HOST_FRONTEND_STATUS,
        json!({"change_digest":own}),
    )
    .await
    .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&observed.result.text_content()).unwrap(),
        json!({"state":"not_found","submission_outcome":"unresolved","retry_submission":false})
    );
    assert!(
        approvals.has_uncertain_dispatch(),
        "the exact read cannot restore spent authority"
    );
    let observed = call(
        &dispatcher,
        &session,
        "own-prepared-status",
        PAIRED_HOST_FRONTEND_STATUS,
        json!({"change_digest":own}),
    )
    .await
    .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&observed.result.text_content()).unwrap(),
        json!({"state":"prepared","change":change,"base_current":true})
    );
    assert!(
        approvals.has_uncertain_dispatch(),
        "a draft GET cannot settle the lost POST"
    );
    for id in ["spent-submit", "after-draft-submit"] {
        assert!(matches!(
            call(
                &dispatcher,
                &session,
                id,
                PAIRED_HOST_FRONTEND_SUBMIT,
                json!({"change_digest":own})
            )
            .await,
            Err(ToolError::AccessDenied { .. })
        ));
    }
    assert!(matches!(
        call(
            &dispatcher,
            &session,
            "after-draft-prepare",
            PAIRED_HOST_FRONTEND_PREPARE,
            candidate
        )
        .await,
        Err(ToolError::AccessDenied { .. })
    ));
    assert!(matches!(
        call(
            &dispatcher,
            &session,
            "after-draft-unrelated",
            PAIRED_HOST_FRONTEND_STATUS,
            json!({"change_digest":other})
        )
        .await,
        Err(ToolError::AccessDenied { .. })
    ));
    assert!(
        timeout(Duration::from_millis(10), presenter.next())
            .await
            .is_err()
    );
    approvals.retire();
    let requests = host.finish().await;
    assert_eq!(
        requests.len(),
        6,
        "pairing, preparation, one submit and two exact status reads; denied calls add no HTTP"
    );
    assert_eq!(
        requests[3],
        json!({"change_digest":own,"change":change}),
        "one exact approved POST only"
    );
}

//! Synthetic HTTP observations and admitted tool composition only. No live host,
//! repository fetch, package execution, model call or provider authentication.

use meerkat_connect::paired_host::*;
use serde_json::{Value, json};
use std::{collections::BTreeMap, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
    time::timeout,
};

const FIXTURE_BOUND: Duration = Duration::from_secs(5);
const SOURCES: &str = "/console/extensions/frontend/sources";
const REPO: &str = "https://github.com/fixture/example-telegram.git";
const TAG: &str = "v0.2.8+fixture";
const COMMIT: &str = "49f53a27a4535f80a357ee13e8150776ed02886b";
const COOKIE: &str = "synthetic-source-device";

#[derive(Debug)]
struct Request {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    chunked: bool,
    close_without_reply: bool,
}

impl Reply {
    fn json(status: u16, body: Value) -> Self {
        Self::bytes(status, serde_json::to_vec(&body).unwrap())
    }

    fn bytes(status: u16, body: Vec<u8>) -> Self {
        Self {
            status,
            headers: vec![
                ("Content-Type".into(), "application/json".into()),
                ("Cache-Control".into(), "no-store".into()),
            ],
            body,
            chunked: false,
            close_without_reply: false,
        }
    }

    fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    async fn write(self, stream: &mut TcpStream) {
        if self.close_without_reply {
            return;
        }
        let mut head = format!("HTTP/1.1 {} Fixture\r\nConnection: close\r\n", self.status);
        for (name, value) in self.headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        if self.chunked {
            head.push_str("Transfer-Encoding: chunked\r\n\r\n");
        } else {
            head.push_str(&format!("Content-Length: {}\r\n\r\n", self.body.len()));
        }
        // Refused/oversized responses may be dropped before the fixture writes
        // all bytes. A closed peer is expected, not a reason to retry.
        if stream.write_all(head.as_bytes()).await.is_err() {
            return;
        }
        if self.chunked {
            for chunk in self.body.chunks(4096) {
                if stream
                    .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
                    .await
                    .is_err()
                    || stream.write_all(chunk).await.is_err()
                    || stream.write_all(b"\r\n").await.is_err()
                {
                    return;
                }
            }
            let _ = stream.write_all(b"0\r\n\r\n").await;
        } else {
            let _ = stream.write_all(&self.body).await;
        }
        let _ = stream.shutdown().await;
    }
}

struct Fixture {
    origin: String,
    task: Option<JoinHandle<Vec<Request>>>,
}

impl Fixture {
    async fn new(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let mut requests = Vec::new();
            for reply in replies {
                let (mut stream, _) = timeout(FIXTURE_BOUND, listener.accept())
                    .await
                    .unwrap()
                    .unwrap();
                requests.push(
                    timeout(FIXTURE_BOUND, read_request(&mut stream))
                        .await
                        .unwrap(),
                );
                timeout(FIXTURE_BOUND, reply.write(&mut stream))
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

    async fn finish(mut self) -> Vec<Request> {
        timeout(FIXTURE_BOUND, self.task.take().unwrap())
            .await
            .unwrap()
            .unwrap()
    }
}

impl Drop for Fixture {
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
        let mut buffer = [0_u8; 4096];
        let read = stream.read(&mut buffer).await.unwrap();
        assert!(
            read > 0 && bytes.len() + read < 128 * 1024,
            "bounded fixture request"
        );
        bytes.extend_from_slice(&buffer[..read]);
    };
    let head = std::str::from_utf8(&bytes[..boundary]).unwrap();
    let mut lines = head.split("\r\n");
    let first = lines.next().unwrap();
    let mut first = first.split_whitespace();
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
        let mut buffer = [0_u8; 4096];
        let read = stream.read(&mut buffer).await.unwrap();
        assert!(read > 0);
        bytes.extend_from_slice(&buffer[..read]);
    }
    Request {
        method,
        path,
        headers,
        body: bytes[boundary..boundary + length].to_vec(),
    }
}

fn tags() -> Value {
    json!({"repo": REPO, "tags":[{"tag":TAG,"commit":COMMIT}]})
}

fn inspection() -> Value {
    json!({"repo":REPO,"tag":TAG,"commit":COMMIT,
        "distribution":"example-ext-telegram","version":"0.2.8",
        "entry_point":{"group":"example_host.extensions","name":"telegram","object":"example_ext_telegram.extension:TelegramExtension"},
        "declared":{},"unverified":["manifest","facades","channel_template","secrets"],
        "already_declared":false,
        "prepare":{"new_extension":{"distribution":"example-ext-telegram","version":"0.2.8",
            "source":{"repo":REPO,"tag":TAG,"commit":COMMIT},
            "channel":{"ingress":null,"approved_groups":null}}}})
}

fn proposal() -> Value {
    let mut value = inspection()["prepare"]["new_extension"].clone();
    value["channel"] = json!({"ingress":"polling","approved_groups":[]});
    value
}

fn prepared() -> Value {
    json!({"valid":true,"problems":[],"change":{
        "change_digest":format!("paired-host-change-v1:sha256:{}","a".repeat(64)),
        "kind":"new_extension", "base":{"activation_id":"218","policy_digest":"fixture-policy","ledger_digest":"fixture-ledger"},
        "proposal":proposal(),"unverified":["manifest","facades","channel_template","secrets"],
        "prerequisites":[{"kind":"build_and_review","state":"pending"}],
        "unified_summary":"Request host-owned build and review of the fixture's pinned source. Readiness remains unverified."}})
}

// PR457 synthetic owner data. No host fetch, role activation or daemon contact.
fn role_inspection() -> Value {
    let mut body = inspection();
    body["package_path"] = json!("extension");
    body["distribution"] = json!("example-ext-device");
    body["version"] = json!("0.5.1.dev2026100301");
    body["entry_point"] = json!({"group":"example_host.extensions", "name":"device",
        "object":"example_ext_device.extension:DeviceExtension"});
    body["unverified"] = json!([
        "manifest",
        "facades",
        "role_contract",
        "host_bindings",
        "secrets"
    ]);
    body["roles"] = json!({"selections":[
        {"selected_roles":["service_connector"],"daemon":{"voice_mode":"disabled"},
         "required_facades":["daemon_control.v1"],"voice_map_entries":0},
        {"selected_roles":["agent_frontend"],"daemon":{"voice_mode":"gateway"},
         "required_facades":["daemon_control.v1","live.v1","media.v1","notify.v1"],"voice_map_entries":1},
        {"selected_roles":["agent_frontend","service_connector"],"daemon":{"voice_mode":"gateway"},
         "required_facades":["daemon_control.v1","live.v1","media.v1","notify.v1"],"voice_map_entries":1}
    ]});
    let draft = body["prepare"]["new_extension"].as_object_mut().unwrap();
    draft.insert("distribution".into(), json!("example-ext-device"));
    draft.insert("version".into(), json!("0.5.1.dev2026100301"));
    draft.get_mut("source").unwrap()["package_path"] = json!("extension");
    draft.remove("channel");
    draft.insert(
        "roles".into(),
        json!({"selected_roles":null,"daemon":{"voice_mode":null},
                                      "required_facades":null,"voice_map":null}),
    );
    body
}

fn role_proposal(selection: usize) -> Value {
    let inspected = role_inspection();
    let mut proposal = inspected["prepare"]["new_extension"].clone();
    let mut roles = inspected["roles"]["selections"][selection].clone();
    roles.as_object_mut().unwrap().remove("voice_map_entries");
    roles["voice_map"] = if selection == 0 {
        json!({})
    } else {
        json!({"identity:fixture-member":"device:fixture-room"})
    };
    proposal["roles"] = roles;
    proposal
}

#[test]
fn role_source_inspection_preserves_owner_selections_and_nested_package() {
    let body = role_inspection();
    let observed: FrontendSourceInspection = serde_json::from_value(body.clone())
        .expect("the merged owner role inspection is supported");
    assert_eq!(serde_json::to_value(observed).unwrap(), body);
}

#[test]
fn role_change_preserves_each_proposal_and_exact_owner_bytes() {
    for selection in 0..3 {
        let proposal = role_proposal(selection);
        let request: FrontendNewExtensionRequest = serde_json::from_value(proposal.clone())
            .expect("one owner-advertised role proposal is supported");
        assert_eq!(serde_json::to_value(request).unwrap(), proposal);
        let mut change = prepared()["change"].clone();
        change["proposal"] = proposal;
        change["unverified"] = role_inspection()["unverified"].clone();
        let raw = serde_json::to_string_pretty(&change).unwrap();
        let captured: FrontendNewExtensionChange = serde_json::from_str(&raw).unwrap();
        assert_eq!(serde_json::to_string(&captured).unwrap(), raw);
    }
}

#[test]
fn role_built_status_preserves_required_null_template_and_package_path() {
    let body = json!({"request_id":format!("paired-host-change-v1:sha256:{}","a".repeat(64)),
        "submitted_at":"fixture-time","kind":"new_extension","state":"built",
        "staged_at":"fixture-stage","prerequisites":[],
        "built":{"extension_id":"device","distribution":"example-ext-device",
            "version":"0.5.1.dev2026100301","source_commit":COMMIT,"package_path":"extension",
            "wheel_sha256":"b".repeat(64),"release_evidence_sha256":"c".repeat(64),
            "facades":["daemon_control.v1"],"channel_template":null,"secret_names":[]}});
    let observed: FrontendNewExtensionStatus = serde_json::from_str(&body.to_string())
        .expect("role build status includes an explicit null template");
    assert_eq!(serde_json::to_value(observed).unwrap(), body);
}

#[tokio::test]
async fn role_required_is_an_exact_host_problem_instead_of_an_unknown_response() {
    let problem = json!({"code":"roles_required","path":"new_extension.channel",
        "message":"This extension is configured by a role selection, not a channel."});
    let fixture = Fixture::new(vec![Reply::json(
        200,
        json!({"valid":false,"problems":[problem.clone()],"change":null}),
    )])
    .await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    let mut value = proposal();
    value["distribution"] = json!("example-ext-device");
    let request = serde_json::from_value(value).unwrap();
    let result = client.prepare_new_extension(&request).await;
    assert_eq!(fixture.finish().await.len(), 1);
    let FrontendNewExtensionPreparation::Candidate(receipt) = result.unwrap() else {
        panic!("owner candidate refusal expected")
    };
    assert!(!receipt.valid && receipt.change.is_none());
    assert_eq!(serde_json::to_value(&receipt.problems[0]).unwrap(), problem);
}

fn pairing() -> Reply {
    Reply::json(200, json!({"ok":true})).header(
        "Set-Cookie",
        &format!("paired_host_device={COOKIE}; Max-Age=15552000; Path=/; HttpOnly; SameSite=Lax"),
    )
}

fn source_error(category: &str, code: &str, path: &str, message: &str) -> Value {
    json!({"error":category,"problems":[{"code":code,"path":path,"message":message}]})
}

fn assert_safe_unconfirmed(error: &PairedHostError, operation: PairedHostOperation) {
    assert!(
        matches!(error, PairedHostError::Unconfirmed {operation: actual, ..} if *actual == operation)
    );
    assert!(!format!("{error:?} {error}").contains("private-response-marker"));
}

#[tokio::test]
async fn captured_client_reads_exact_pins_and_only_fills_channel_before_existing_prepare() {
    let fixture = Fixture::new(vec![
        pairing(),
        Reply::json(200, tags()),
        Reply::json(200, inspection()),
        Reply::json(200, prepared()),
    ])
    .await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    client
        .confirm_terminal_pairing(TerminalPairingCode::new("ABCD-EFGH-JKMN".into()).unwrap())
        .await
        .unwrap();
    let FrontendSourceRead::Observed(list) = client.frontend_source_tags(REPO).await.unwrap()
    else {
        panic!("tags")
    };
    assert_eq!(list.tags[0].tag, TAG);
    let FrontendSourceRead::Observed(observed) = client
        .inspect_frontend_source(&list.repo, &list.tags[0].tag)
        .await
        .unwrap()
    else {
        panic!("inspection")
    };
    assert_eq!(observed.entry_point.group, "example_host.extensions");
    let request = observed
        .with_channel(FrontendExtensionChannel {
            ingress: FrontendIngress::Polling,
            approved_groups: vec![],
        })
        .unwrap();
    assert_eq!(serde_json::to_value(&request).unwrap(), proposal());
    let FrontendNewExtensionPreparation::Candidate(candidate) =
        client.prepare_new_extension(&request).await.unwrap()
    else {
        panic!("prepare")
    };
    assert_eq!(candidate.change.unwrap().proposal(), &request);
    let origin = fixture.origin.clone();
    let requests = fixture.finish().await;
    assert_eq!(requests.len(), 4);
    for request in &requests[1..3] {
        assert_eq!(request.method, "GET");
        assert_eq!(request.headers["origin"], origin);
        assert_eq!(request.headers["cache-control"], "no-store");
        assert!(request.headers["cookie"].contains(COOKIE));
        assert!(!request.headers.contains_key("x-host-dev-token"));
        assert!(request.body.is_empty());
    }
    let query = |request: &Request| -> BTreeMap<String, String> {
        reqwest::Url::parse(&format!("{origin}{}", request.path))
            .unwrap()
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect()
    };
    assert_eq!(
        query(&requests[1]),
        BTreeMap::from([("repo".into(), REPO.into())])
    );
    assert_eq!(
        query(&requests[2]),
        BTreeMap::from([("repo".into(), REPO.into()), ("tag".into(), TAG.into())])
    );
    assert!(requests[2].path.contains("%2Bfixture"));
    assert_eq!(
        requests[1].path.split('?').next().unwrap(),
        format!("{SOURCES}/tags")
    );
    assert_eq!(
        requests[2].path.split('?').next().unwrap(),
        format!("{SOURCES}/inspect")
    );
    assert_eq!(requests[3].path, "/console/extensions/frontend/prepare");
    assert_eq!(
        serde_json::from_slice::<Value>(&requests[3].body).unwrap(),
        json!({"new_extension":proposal()})
    );
}

#[tokio::test]
async fn existing_native_admission_composes_catalog_source_reads_and_preparation() {
    use meerkat_connect::{
        SessionOptions, approval::LocalApprovals, paired_host_tool::*,
        setup::paired_host_frontend_session,
    };
    use meerkat_core::{
        AgentToolDispatcher, SessionId, ToolExecutionPolicy, agent::ToolDispatchContext,
        ops::ToolAccessPolicy, tool_execution_policy::ExecutionPolicyGatedDispatcher,
        types::ToolCallView,
    };
    let fixture = Fixture::new(vec![
        Reply::json(200, tags()),
        Reply::json(200, inspection()),
        Reply::json(200, prepared()),
    ])
    .await;
    let paired_host = PairedHostFrontend::new(PairedHostClient::new(&fixture.origin).unwrap());
    let directory = tempfile::tempdir().unwrap();
    let (approvals, mut presenter) = LocalApprovals::create_paired_host(directory.path()).unwrap();
    let mut input = SessionOptions::default();
    input.build.tool_access_policy = Some(ToolAccessPolicy::ReadOnly);
    let mut options = paired_host_frontend_session(input, &paired_host).unwrap();
    approvals
        .attach_paired_host_direct(&mut options, &paired_host)
        .unwrap();
    let session = SessionId::new();
    approvals.bind_session(&session).unwrap();
    let dispatcher = ExecutionPolicyGatedDispatcher::new(
        options.build.external_tools.unwrap(),
        ToolExecutionPolicy::resolve(options.build.tool_access_policy.unwrap()).unwrap(),
    )
    .with_dispatch_admission(options.build.tool_dispatch_admission.unwrap());
    let context = ToolDispatchContext::default().with_runtime_identity(session, None);
    let invoke = async |name: &str, args: Value| {
        let raw = serde_json::value::RawValue::from_string(args.to_string()).unwrap();
        let result = dispatcher
            .dispatch_with_context(
                ToolCallView {
                    id: name,
                    name,
                    args: &raw,
                },
                &context,
            )
            .await
            .unwrap();
        serde_json::from_str::<Value>(&result.result.text_content()).unwrap()
    };
    let catalog = invoke(TOOLKIT_FRONTEND_CATALOG, json!({})).await;
    assert!(
        catalog["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry.to_string().contains(REPO))
    );
    let listed = invoke(PAIRED_HOST_FRONTEND_SOURCE_TAGS, json!({"repo":REPO})).await;
    let inspected = invoke(
        PAIRED_HOST_FRONTEND_SOURCE_INSPECT,
        json!({"repo":listed["repo"],"tag":listed["tags"][0]["tag"]}),
    )
    .await;
    let mut body = inspected["prepare"].clone();
    body["new_extension"]["channel"] = json!({"ingress":"polling","approved_groups":[]});
    let prepared = invoke(PAIRED_HOST_FRONTEND_PREPARE, body).await;
    assert_eq!(prepared["change"]["proposal"], proposal());
    assert_eq!(prepared["valid"], true);
    for (name, args) in [
        (
            PAIRED_HOST_FRONTEND_SOURCE_TAGS,
            json!({"repo":REPO,"origin":"https://other.invalid"}),
        ),
        (
            PAIRED_HOST_FRONTEND_SOURCE_INSPECT,
            json!({"repo":REPO,"tag":TAG,"credential":"not-an-accepted-field"}),
        ),
    ] {
        let raw = serde_json::value::RawValue::from_string(args.to_string()).unwrap();
        let error = dispatcher
            .dispatch_with_context(
                ToolCallView {
                    id: name,
                    name,
                    args: &raw,
                },
                &context,
            )
            .await
            .unwrap_err();
        assert!(
            matches!(error,meerkat_core::ToolError::ExecutionFailed {message} if message.starts_with("Supply only the canonical source repository"))
        );
    }
    assert!(
        timeout(Duration::from_millis(10), presenter.next())
            .await
            .is_err(),
        "reads must not request human effect approval"
    );
    approvals.retire();
    let requests = fixture.finish().await;
    assert_eq!(requests.len(), 3);
    assert_eq!(requests.iter().filter(|r| r.method == "POST").count(), 1);
    assert_eq!(requests[2].path, "/console/extensions/frontend/prepare");
}

#[tokio::test]
async fn full_bounded_tag_list_exceeds_old_limit_without_reordering_or_partial_output() {
    let entries: Vec<_> = (0..500)
        .rev()
        .map(|i| json!({"tag":format!("v{i:03}{}","x".repeat(96)),"commit":COMMIT}))
        .collect();
    let body = json!({"repo":REPO,"tags":entries});
    assert!(serde_json::to_vec(&body).unwrap().len() > 64 * 1024);
    let mut too_many = body.clone();
    too_many["tags"]
        .as_array_mut()
        .unwrap()
        .push(json!({"tag":"vextra","commit":COMMIT}));
    let mut duplicate = tags();
    duplicate["tags"]
        .as_array_mut()
        .unwrap()
        .push(json!({"tag":TAG,"commit":COMMIT}));
    let mut wrong_repo = tags();
    wrong_repo["repo"] = json!("https://other.invalid/owner/repo.git");
    let mut wrong_commit = tags();
    wrong_commit["tags"][0]["commit"] = json!("A".repeat(40));
    let fixture = Fixture::new(vec![
        Reply::json(200, body.clone()),
        Reply::json(200, too_many),
        Reply::json(200, duplicate),
        Reply::json(200, wrong_repo),
        Reply::json(200, wrong_commit),
    ])
    .await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    let FrontendSourceRead::Observed(observed) = client.frontend_source_tags(REPO).await.unwrap()
    else {
        panic!("tags")
    };
    assert_eq!(serde_json::to_value(observed).unwrap(), body);
    for _ in 0..4 {
        assert_safe_unconfirmed(
            &client.frontend_source_tags(REPO).await.unwrap_err(),
            PairedHostOperation::FrontendSourceTags,
        );
    }
    assert_eq!(fixture.finish().await.len(), 5);
}

#[tokio::test]
async fn inspection_correlation_and_closed_unverified_shapes_refuse_host_contradictions() {
    let mut bodies = Vec::new();
    for (pointer, value) in [
        ("/repo", json!("https://other.invalid/owner/repo.git")),
        ("/tag", json!("vother")),
        ("/commit", json!("bad")),
        (
            "/prepare/new_extension/source/repo",
            json!("https://other.invalid/owner/repo.git"),
        ),
        ("/prepare/new_extension/source/tag", json!("vother")),
        (
            "/prepare/new_extension/source/commit",
            json!("b".repeat(40)),
        ),
        (
            "/prepare/new_extension/distribution",
            json!("example-ext-other"),
        ),
        ("/prepare/new_extension/version", json!("9.0")),
        ("/prepare/new_extension/channel/ingress", json!("polling")),
        ("/prepare/new_extension/channel/approved_groups", json!([])),
        ("/entry_point/group", json!("other.group")),
        ("/declared", json!({"secrets":["private-response-marker"]})),
        ("/unverified", json!([])),
    ] {
        let mut body = inspection();
        *body.pointer_mut(pointer).unwrap() = value;
        bodies.push(body);
    }
    let mut missing = inspection();
    missing["prepare"]["new_extension"]["channel"]
        .as_object_mut()
        .unwrap()
        .remove("ingress");
    bodies.push(missing);
    let mut extra = inspection();
    extra["execute"] = json!("private-response-marker");
    bodies.push(extra);
    let count = bodies.len();
    let fixture = Fixture::new(bodies.into_iter().map(|b| Reply::json(200, b)).collect()).await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    for _ in 0..count {
        assert_safe_unconfirmed(
            &client.inspect_frontend_source(REPO, TAG).await.unwrap_err(),
            PairedHostOperation::FrontendSourceInspect,
        );
    }
    assert_eq!(fixture.finish().await.len(), count);
}

#[tokio::test]
async fn source_failures_preserve_only_the_exact_static_problem_and_optional_reason() {
    let mut unreachable = source_error(
        "bad_gateway",
        "repo_unreachable",
        "repo",
        "The source repository could not be read with the host's credentials.",
    );
    unreachable["problems"][0]["reason"] = json!("host_credential_invalid");
    let metadata = source_error(
        "unprocessable",
        "metadata_invalid",
        "pyproject.toml:project.entry-points.example_host.extensions",
        "The source's package metadata is not static, valid extension metadata for this host.",
    );
    let no_tags = source_error(
        "not_found",
        "no_release_tags",
        "repo",
        "The source repository has no tags a request can pin.",
    );
    let no_reason = source_error(
        "bad_gateway",
        "repo_unreachable",
        "repo",
        "The source repository could not be read with the host's credentials.",
    );
    let mut extra = no_tags.clone();
    extra["detail"] = json!("private-response-marker");
    let mut raw = unreachable.clone();
    raw["problems"][0]["message"] = json!("private-response-marker");
    let mut wrong_reason = no_tags.clone();
    wrong_reason["problems"][0]["reason"] = json!("timeout");
    let mut null_reason = no_reason.clone();
    null_reason["problems"][0]["reason"] = Value::Null;
    let fixture = Fixture::new(vec![
        Reply::json(502, unreachable),
        Reply::json(422, metadata),
        Reply::json(404, no_tags),
        Reply::json(502, no_reason),
        Reply::json(404, extra),
        Reply::json(502, raw),
        Reply::json(404, wrong_reason),
        Reply::json(502, null_reason),
        Reply::json(
            403,
            json!({"error":"forbidden","code":"console_owner_required"}),
        ),
    ])
    .await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    let FrontendSourceRead::Rejected(failure) = client.frontend_source_tags(REPO).await.unwrap()
    else {
        panic!("failure")
    };
    assert_eq!(
        failure.problems[0].reason,
        Some(FrontendSourceUnavailableReason::HostCredentialInvalid)
    );
    let FrontendSourceRead::Rejected(failure) =
        client.inspect_frontend_source(REPO, TAG).await.unwrap()
    else {
        panic!("metadata")
    };
    assert_eq!(
        failure.problems[0].path,
        "pyproject.toml:project.entry-points.example_host.extensions"
    );
    let FrontendSourceRead::Rejected(failure) = client.frontend_source_tags(REPO).await.unwrap()
    else {
        panic!("tags")
    };
    assert_eq!(failure.error, FrontendSourceRejectionKind::NotFound);
    let FrontendSourceRead::Rejected(failure) = client.frontend_source_tags(REPO).await.unwrap()
    else {
        panic!("no reason")
    };
    assert_eq!(
        failure.problems[0].reason, None,
        "a private not-found must not invent credential classification"
    );
    for _ in 0..4 {
        assert_safe_unconfirmed(
            &client.frontend_source_tags(REPO).await.unwrap_err(),
            PairedHostOperation::FrontendSourceTags,
        );
    }
    assert_eq!(
        client.frontend_source_tags(REPO).await.unwrap_err(),
        PairedHostError::Refused {
            operation: PairedHostOperation::FrontendSourceTags,
            reason: PairedHostRefusal::ConsoleOwnerRequired
        }
    );
    assert_eq!(fixture.finish().await.len(), 9);
}

#[tokio::test]
async fn malformed_transport_bounds_and_loss_never_trigger_a_second_request() {
    let mut no_store = Reply::json(200, tags());
    no_store.headers.retain(|(name, _)| name != "Cache-Control");
    let duplicate = serde_json::to_string(&tags()).unwrap().replacen(
        '{',
        "{\"repo\":\"private-response-marker\",",
        1,
    );
    let mut chunked = Reply::bytes(200, vec![b'x'; 256 * 1024 + 1]);
    chunked.chunked = true;
    let mut lost = Reply::json(200, tags());
    lost.close_without_reply = true;
    let fixture = Fixture::new(vec![
        no_store,
        Reply::bytes(200, duplicate.into_bytes()),
        Reply::bytes(200, vec![b'x'; 256 * 1024 + 1]),
        chunked,
        lost,
    ])
    .await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    for _ in 0..5 {
        assert_safe_unconfirmed(
            &client.frontend_source_tags(REPO).await.unwrap_err(),
            PairedHostOperation::FrontendSourceTags,
        );
    }
    assert_eq!(fixture.finish().await.len(), 5);
}

#[tokio::test]
async fn selected_origin_is_not_a_tool_argument_and_device_cookies_never_cross_origins() {
    let first = Fixture::new(vec![pairing(), Reply::json(200, tags())]).await;
    let second = Fixture::new(vec![
        Reply::json(302, json!({})).header("Location", &format!("{}{SOURCES}/tags", first.origin)),
    ])
    .await;
    let mut paired = PairedHostClient::new(&first.origin).unwrap();
    paired
        .confirm_terminal_pairing(TerminalPairingCode::new("ABCD-EFGH-JKMN".into()).unwrap())
        .await
        .unwrap();
    let mut other = PairedHostClient::new(&second.origin).unwrap();
    let error = other.inspect_frontend_source(REPO, TAG).await.unwrap_err();
    assert_eq!(
        error,
        PairedHostError::Unconfirmed {
            operation: PairedHostOperation::FrontendSourceInspect,
            reason: UnconfirmedReason::Redirect
        }
    );
    assert!(matches!(
        paired.frontend_source_tags(REPO).await.unwrap(),
        FrontendSourceRead::Observed(_)
    ));
    let others = second.finish().await;
    assert_eq!(others.len(), 1);
    assert!(!others[0].headers.contains_key("cookie"));
    assert!(!others[0].headers.contains_key("x-host-dev-token"));
    let own = first.finish().await;
    assert_eq!(own.len(), 2);
    assert!(own[1].headers["cookie"].contains(COOKIE));
}

#[tokio::test]
async fn invalid_source_inputs_are_rejected_before_any_transport() {
    let mut client = PairedHostClient::new("http://127.0.0.1:9").unwrap();
    for repo in [
        "http://github.com/owner/repo.git",
        "https://user:private-response-marker@github.com/owner/repo.git",
        "https://github.com/owner/repo",
        "https://github.com/owner/repo.git?token=private-response-marker",
        "https://github.com/owner/repo.git#fragment",
        "https://GitHub.com/owner/repo.git",
    ] {
        assert_eq!(
            client.frontend_source_tags(repo).await.unwrap_err(),
            PairedHostError::InvalidSourceRepository
        );
    }
    for tag in [
        "",
        "../main",
        "v1.lock",
        "v1..2",
        "refs/tags/v1",
        "v1?secret",
        "v1&tag=other",
    ] {
        assert_eq!(
            client.inspect_frontend_source(REPO, tag).await.unwrap_err(),
            PairedHostError::InvalidSourceTag
        );
    }
}

fn role_prepared(selection: usize) -> Value {
    let mut result = prepared();
    result["change"]["proposal"] = role_proposal(selection);
    result["change"]["unverified"] = role_inspection()["unverified"].clone();
    result
}

fn role_status(selection: usize) -> Value {
    json!({"request_id":prepared()["change"]["change_digest"],"state":"built","kind":"new_extension",
        "submitted_at":"fixture-time","staged_at":"fixture-stage","attempt":2,
        "built":{"extension_id":"device","distribution":role_proposal(selection)["distribution"],"version":role_proposal(selection)["version"],
            "source_commit":COMMIT,"package_path":"extension","wheel_sha256":"b".repeat(64),
            "release_evidence_sha256":"c".repeat(64),"channel_template":null,
            "facades":role_proposal(selection)["roles"]["required_facades"],"secret_names":[]},"prerequisites":[]})
}

fn role_secret_status() -> Value {
    let mut body = role_status(2);
    body["built"]["secret_names"] = json!(["api_key"]);
    body["built"]["role_consumers"] =
        json!({"api_key":{"consumer":"extension","consumer_id":"device"}});
    body["prerequisites"] = json!([{"kind":"secret","name":"api_key","provisioned":false}]);
    body
}

#[tokio::test]
async fn role_consumer_status_preserves_owner_facts_and_legacy_omission() {
    let mut empty = role_status(2);
    empty["built"]["role_consumers"] = json!({});
    let mut legacy_secret = role_secret_status();
    legacy_secret["built"]
        .as_object_mut()
        .unwrap()
        .remove("role_consumers");
    let mut channel = role_status(2);
    channel["built"]["channel_template"] = json!({"id":"telegram:primary"});
    let bodies = [role_secret_status(), empty, legacy_secret, channel];
    let fixture = Fixture::new(
        bodies
            .iter()
            .cloned()
            .map(|b| Reply::json(200, b))
            .collect(),
    )
    .await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    let digest = serde_json::from_value(prepared()["change"]["change_digest"].clone()).unwrap();
    for (index, body) in bodies.iter().enumerate() {
        let FrontendChangeRead::NewExtensionStatus(observed) =
            client.frontend_change_status(&digest).await.unwrap()
        else {
            panic!("expected the exact owner build observation")
        };
        assert_eq!(serde_json::to_value(&observed).unwrap(), *body);
        let request = serde_json::from_value(role_proposal(2)).unwrap();
        assert_eq!(observed.matches_proposal(&request), index != 3);
    }
    let requests = fixture.finish().await;
    assert_eq!(requests.len(), bodies.len());
    assert!(
        requests
            .iter()
            .all(|request| request.method == "GET" && request.body.is_empty())
    );
}

#[tokio::test]
async fn role_consumer_status_refuses_ambiguous_or_contradictory_owner_facts() {
    let mut bodies = Vec::new();
    for value in [
        Value::Null,
        json!([]),
        json!({}),
        json!({"other":{"consumer":"extension","consumer_id":"device"}}),
        json!({"api_key":null}),
        json!({"api_key":{"consumer":"extension"}}),
        json!({"api_key":{"consumer_id":"device"}}),
        json!({"api_key":{"consumer":"channel","consumer_id":"device"}}),
        json!({"api_key":{"consumer":"extension","consumer_id":"other"}}),
        json!({"api_key":{"consumer":"extension","consumer_id":null}}),
        json!({"api_key":{"consumer":"extension","consumer_id":"device","extra":"private-response-marker"}}),
        json!({"api_key":{"consumer":"extension","consumer_id":"device"},"extra":{"consumer":"extension","consumer_id":"device"}}),
    ] {
        let mut body = role_secret_status();
        body["built"]["role_consumers"] = value;
        bodies.push(serde_json::to_vec(&body).unwrap());
    }
    let mut channel = role_secret_status();
    channel["built"]["channel_template"] = json!({"id":"device:primary"});
    bodies.push(serde_json::to_vec(&channel).unwrap());
    let raw = role_secret_status().to_string();
    for replacement in [
        r#""role_consumers":{},"role_consumers":"#,
        r#""role_consumers":{},"role_\u0063onsumers":"#,
    ] {
        bodies.push(
            raw.replacen(r#""role_consumers":"#, replacement, 1)
                .into_bytes(),
        );
    }
    for replacement in [
        r#""api_key":{},"api_key":"#,
        r#""api_key":{},"api_\u006bey":"#,
        r#""consumer":"channel","consumer":"#,
        r#""consumer_id":"other","consumer_\u0069d":"#,
    ] {
        let key = if replacement.starts_with(r#""api"#) {
            r#""api_key":"#
        } else if replacement.starts_with(r#""consumer_id"#) {
            r#""consumer_id":"#
        } else {
            r#""consumer":"#
        };
        bodies.push(raw.replacen(key, replacement, 1).into_bytes());
    }
    let count = bodies.len();
    let fixture = Fixture::new(bodies.into_iter().map(|b| Reply::bytes(200, b)).collect()).await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    let digest = serde_json::from_value(prepared()["change"]["change_digest"].clone()).unwrap();
    for _ in 0..count {
        assert_safe_unconfirmed(
            &client.frontend_change_status(&digest).await.unwrap_err(),
            PairedHostOperation::FrontendChangeStatus,
        );
    }
    let requests = fixture.finish().await;
    assert_eq!(requests.len(), count);
    assert!(
        requests
            .iter()
            .all(|request| request.method == "GET" && request.body.is_empty())
    );
}

#[test]
fn role_configuration_is_closed_and_never_collapses_path_or_selection() {
    for path in [
        "",
        ".",
        "..",
        "a/../b",
        "a..b",
        "/extension",
        "extension/",
        "a//b",
        "a\\b",
        "a?b",
        "a%2fb",
        "_private",
        "é",
        "a/b/c/d/e/f/g/h/i",
    ] {
        assert!(FrontendPackagePath::new(path).is_err(), "{path:?}");
    }
    for path in ["extension", "Packages/Device-1.2", "a/b/c/d/e/f/g/h"] {
        assert_eq!(FrontendPackagePath::new(path).unwrap().as_str(), path);
    }
    assert!(FrontendPackagePath::new("a".repeat(101)).is_err());
    assert!(
        FrontendPackagePath::new(format!("{}/{}/a", "a".repeat(100), "b".repeat(100))).is_err()
    );
    let mut invalid = Vec::new();
    for (pointer, value) in [
        ("/source/package_path", Value::Null),
        ("/roles", Value::Null),
        ("/roles/daemon", Value::Null),
        ("/roles/daemon/voice_mode", Value::Null),
        ("/roles/selected_roles", json!([])),
        ("/roles/required_facades", json!(["bad facade"])),
        ("/roles/voice_map", json!({"identity":""})),
    ] {
        let mut body = role_proposal(1);
        *body.pointer_mut(pointer).unwrap() = value;
        invalid.push(body);
    }
    let mut both = role_proposal(0);
    both["channel"] = proposal()["channel"].clone();
    invalid.push(both);
    let mut neither = role_proposal(0);
    neither.as_object_mut().unwrap().remove("roles");
    invalid.push(neither);
    let mut extra = role_proposal(1);
    extra["roles"]["daemon"]["execute"] = json!(true);
    invalid.push(extra);
    for body in invalid {
        assert!(serde_json::from_value::<FrontendNewExtensionRequest>(body).is_err());
    }
    let raw = serde_json::to_string(&role_proposal(1)).unwrap();
    for duplicate in [
        r#""voice_map":{"identity":"first","identity":"second"}"#,
        r#""voice_map":{"identity":"first","ident\u0069ty":"second"}"#,
    ] {
        let start = raw.find("\"voice_map\":{").unwrap();
        let end = start + raw[start..].find('}').unwrap() + 1;
        let corrupted = format!("{}{}{}", &raw[..start], duplicate, &raw[end..]);
        assert!(serde_json::from_str::<FrontendNewExtensionRequest>(&corrupted).is_err());
    }
    let duplicate_path = raw.replace(
        "\"package_path\":\"extension\"",
        r#""package_path":"other","package_\u0070ath":"extension""#,
    );
    assert!(serde_json::from_str::<FrontendNewExtensionRequest>(&duplicate_path).is_err());
    // Generic Unicode role/map values retain character rather than byte bounds.
    let mut unicode = role_proposal(1);
    unicode["roles"]["selected_roles"] = json!(["é".repeat(64)]);
    unicode["roles"]["daemon"]["voice_mode"] = json!("é".repeat(32));
    unicode["roles"]["voice_map"] = json!({"é".repeat(128):"界".repeat(128)});
    assert_eq!(
        serde_json::to_value(
            serde_json::from_value::<FrontendNewExtensionRequest>(unicode.clone()).unwrap()
        )
        .unwrap(),
        unicode
    );
}

#[test]
fn role_choices_fill_only_the_advertised_contract_and_preserve_facade_order() {
    let observed: FrontendSourceInspection = serde_json::from_value(role_inspection()).unwrap();
    assert_eq!(
        observed
            .with_channel(FrontendExtensionChannel {
                ingress: FrontendIngress::Polling,
                approved_groups: vec![]
            })
            .unwrap_err(),
        PairedHostError::InvalidFrontendConfiguration
    );
    for index in 0..3 {
        let roles: FrontendExtensionRoles =
            serde_json::from_value(role_proposal(index)["roles"].clone()).unwrap();
        let request = observed.with_roles(roles.clone()).unwrap();
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            role_proposal(index)
        );
        let mut mode = roles.clone();
        mode.daemon.voice_mode = "unadvertised".into();
        assert!(observed.with_roles(mode).is_err());
        let mut map = roles.clone();
        map.voice_map.insert("additional".into(), "device".into());
        assert!(observed.with_roles(map).is_err());
        if roles.required_facades.len() > 1 {
            let mut reversed = roles.clone();
            reversed.required_facades.reverse();
            assert!(observed.with_roles(reversed).is_err());
        }
        let status: FrontendNewExtensionStatus =
            serde_json::from_str(&role_status(index).to_string()).unwrap();
        assert!(status.matches_proposal(&request));
        for (pointer, value) in [
            ("/built/package_path", json!("other")),
            ("/built/distribution", json!("other")),
            ("/built/version", json!("9")),
            ("/built/source_commit", json!("e".repeat(40))),
            ("/built/channel_template", json!({})),
            ("/built/facades", json!([])),
        ] {
            let mut body = role_status(index);
            *body.pointer_mut(pointer).unwrap() = value;
            let status: FrontendNewExtensionStatus =
                serde_json::from_str(&body.to_string()).unwrap();
            assert!(!status.matches_proposal(&request), "{pointer}");
        }
        let mut extra = role_status(index);
        extra["built"]["facades"]
            .as_array_mut()
            .unwrap()
            .push(json!("extra.v1"));
        assert!(
            serde_json::from_str::<FrontendNewExtensionStatus>(&extra.to_string())
                .unwrap()
                .matches_proposal(&request)
        );
    }
    let legacy: FrontendSourceInspection = serde_json::from_value(inspection()).unwrap();
    assert!(
        legacy
            .with_roles(serde_json::from_value(role_proposal(0)["roles"].clone()).unwrap())
            .is_err()
    );
}

#[tokio::test]
async fn role_inspection_refuses_crossed_paths_choices_and_placeholders() {
    let mut bodies = Vec::new();
    for (pointer, value) in [
        ("/package_path", json!("other")),
        ("/package_path", Value::Null),
        ("/prepare/new_extension/source/package_path", json!("other")),
        ("/roles", Value::Null),
        ("/roles/selections", json!([])),
        ("/roles/selections/0/voice_map_entries", json!(-1)),
        ("/prepare/new_extension/roles/daemon", Value::Null),
        (
            "/prepare/new_extension/roles/daemon/voice_mode",
            json!("disabled"),
        ),
        ("/unverified", inspection()["unverified"].clone()),
    ] {
        let mut body = role_inspection();
        *body.pointer_mut(pointer).unwrap() = value;
        bodies.push(body);
    }
    let mut missing = role_inspection();
    missing["prepare"]["new_extension"]["roles"]
        .as_object_mut()
        .unwrap()
        .remove("voice_map");
    bodies.push(missing);
    let mut broadened = role_inspection();
    broadened["prepare"]["new_extension"]["channel"] =
        inspection()["prepare"]["new_extension"]["channel"].clone();
    bodies.push(broadened);
    let count = bodies.len();
    let fixture = Fixture::new(
        bodies
            .into_iter()
            .map(|body| Reply::json(200, body))
            .collect(),
    )
    .await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    let path = FrontendPackagePath::new("extension").unwrap();
    for _ in 0..count {
        assert_safe_unconfirmed(
            &client
                .inspect_frontend_package(REPO, TAG, Some(&path))
                .await
                .unwrap_err(),
            PairedHostOperation::FrontendSourceInspect,
        );
    }
    assert_eq!(fixture.finish().await.len(), count);
}

#[tokio::test]
async fn role_source_diagnostics_are_fixed_and_bound_to_the_requested_package() {
    let metadata =
        "The source's package metadata is not static, valid extension metadata for this host.";
    let mismatch =
        "The source commit's build recipe does not build the package at the requested path.";
    let cases = [
        (
            400,
            "invalid_request",
            "invalid_value",
            "package_path",
            "A request field has a value outside its allowed form.",
            true,
        ),
        (
            422,
            "unprocessable",
            "package_path_mismatch",
            "package_path",
            mismatch,
            true,
        ),
        (
            422,
            "unprocessable",
            "metadata_invalid",
            "extension/pyproject.toml:project.name",
            metadata,
            true,
        ),
        (
            422,
            "unprocessable",
            "metadata_invalid",
            "example-host-release-v1.json",
            metadata,
            true,
        ),
        (
            422,
            "unprocessable",
            "not_a_paired_host_extension",
            "extension/pyproject.toml",
            "The source declares no extension entry point for this host.",
            true,
        ),
        (
            422,
            "unprocessable",
            "metadata_invalid",
            "other/pyproject.toml",
            metadata,
            false,
        ),
        (
            422,
            "unprocessable",
            "metadata_invalid",
            "pyproject.toml",
            metadata,
            false,
        ),
        (
            422,
            "unprocessable",
            "metadata_invalid",
            "extension/pyproject.toml:private",
            metadata,
            false,
        ),
        (
            422,
            "unprocessable",
            "metadata_invalid",
            "extension/pyproject.toml",
            "private-response-marker",
            false,
        ),
    ];
    let fixture = Fixture::new(
        cases
            .iter()
            .map(|(status, kind, code, path, message, _)| {
                Reply::json(*status, source_error(kind, code, path, message))
            })
            .collect(),
    )
    .await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    let path = FrontendPackagePath::new("extension").unwrap();
    for (_, _, code, problem_path, message, valid) in cases {
        let result = client
            .inspect_frontend_package(REPO, TAG, Some(&path))
            .await;
        if valid {
            let FrontendSourceRead::Rejected(rejection) = result.unwrap() else {
                panic!("expected fixed diagnostic")
            };
            assert_eq!(rejection.problems[0].code, code);
            assert_eq!(rejection.problems[0].path, problem_path);
            assert_eq!(rejection.problems[0].message, message);
        } else {
            assert_safe_unconfirmed(
                &result.unwrap_err(),
                PairedHostOperation::FrontendSourceInspect,
            );
        }
    }
    let requests = fixture.finish().await;
    assert_eq!(requests.len(), cases.len());
    assert!(requests.iter().all(
        |request| query(request).get("package_path").map(String::as_str) == Some("extension")
    ));
}

#[tokio::test]
async fn role_admitted_inspect_prepare_exact_submit_and_later_status_do_not_replay() {
    use meerkat_connect::{
        SessionOptions, approval::LocalApprovals, paired_host_tool::*,
        setup::paired_host_frontend_session,
    };
    use meerkat_core::{
        AgentToolDispatcher, ApprovalDecision, ExecutionPolicyGatedDispatcher, SessionId,
        ToolExecutionPolicy, agent::ToolDispatchContext, types::ToolCallView,
    };
    use std::sync::Arc;
    for selection in 0..3 {
        let prepared = role_prepared(selection);
        let raw_change = serde_json::to_string_pretty(&prepared["change"]).unwrap();
        let digest = prepared["change"]["change_digest"].as_str().unwrap();
        let path = format!("/console/extensions/frontend/review/{digest}");
        let prepare_reply = format!(
            r#"{{"valid":true,"problems":[],"change":{raw_change},"review_url":"{path}"}}"#
        );
        let submit_reply = format!(
            r#"{{"request_id":"{digest}","state":"submitted","submitted_at":"fixture-time","submitted_by":"paired-device","change":{raw_change}}}"#
        );
        let mut mismatch = role_status(selection);
        mismatch["built"]["facades"] = json!([]);
        let fixture = Fixture::new(vec![
            Reply::json(200, role_inspection()),
            Reply::bytes(200, prepare_reply.into_bytes()),
            Reply::bytes(201, submit_reply.into_bytes()),
            Reply::json(200, role_status(selection)),
            Reply::json(200, mismatch),
        ])
        .await;
        let binding = PairedHostFrontend::new(PairedHostClient::new(&fixture.origin).unwrap());
        let mut private = binding.take_secret_entry_presenter().unwrap();
        let directory = tempfile::tempdir().unwrap();
        let (approvals, mut decisions) =
            LocalApprovals::create_paired_host(directory.path()).unwrap();
        let mut options =
            paired_host_frontend_session(SessionOptions::default(), &binding).unwrap();
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
        let context = ToolDispatchContext::default().with_runtime_identity(session, None);
        let invoke = async |id: &str, name: &str, args: Value| {
            let raw = serde_json::value::RawValue::from_string(args.to_string()).unwrap();
            dispatcher
                .dispatch_with_context(
                    ToolCallView {
                        id,
                        name,
                        args: &raw,
                    },
                    &context,
                )
                .await
        };
        // Invalid model input never reaches the host, including escaped keys.
        for args in [
            r#"{"repo":"https://github.com/a/b.git","tag":"v1","package_path":null}"#,
            r#"{"repo":"https://github.com/a/b.git","tag":"v1","package_path":"other","package_\u0070ath":"extension"}"#,
        ] {
            let raw = serde_json::value::RawValue::from_string(args.into()).unwrap();
            assert!(
                dispatcher
                    .dispatch_with_context(
                        ToolCallView {
                            id: "invalid-path",
                            name: PAIRED_HOST_FRONTEND_SOURCE_INSPECT,
                            args: &raw
                        },
                        &context
                    )
                    .await
                    .is_err()
            );
        }
        let result = invoke(
            "inspect",
            PAIRED_HOST_FRONTEND_SOURCE_INSPECT,
            json!({"repo":REPO,"tag":TAG,"package_path":"extension"}),
        )
        .await
        .unwrap();
        let observed: FrontendSourceInspection =
            serde_json::from_str(&result.result.text_content()).unwrap();
        assert_eq!(serde_json::to_value(&observed).unwrap(), role_inspection());
        let roles = serde_json::from_value::<FrontendExtensionRoles>(
            role_proposal(selection)["roles"].clone(),
        )
        .unwrap();
        let request = observed.with_roles(roles).unwrap();
        let result = invoke(
            "prepare",
            PAIRED_HOST_FRONTEND_PREPARE,
            json!({"new_extension":request}),
        )
        .await
        .unwrap();
        let text = result.result.text_content();
        assert!(
            text.contains(&raw_change),
            "the tool retains the exact owner change bytes"
        );
        let mut review = Vec::new();
        private.present_pending(&mut review).unwrap();
        let review = String::from_utf8(review).unwrap();
        assert!(review.contains(&format!("{}{path}", fixture.origin)));
        let submission = invoke(
            "submit",
            PAIRED_HOST_FRONTEND_SUBMIT,
            json!({"change_digest":digest}),
        );
        tokio::pin!(submission);
        let mut decision = tokio::select! {
            result = &mut submission => panic!("submission ran without protected approval: {result:?}"),
            prompt = timeout(FIXTURE_BOUND,decisions.next()) => prompt.unwrap().unwrap(),
        };
        decision.decide(ApprovalDecision::Approve).unwrap();
        let submitted = timeout(FIXTURE_BOUND, submission).await.unwrap().unwrap();
        assert!(submitted.result.text_content().contains(&raw_change));
        let status = invoke(
            "status",
            PAIRED_HOST_FRONTEND_STATUS,
            json!({"change_digest":digest}),
        )
        .await
        .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&status.result.text_content()).unwrap(),
            role_status(selection)
        );
        let mismatch = invoke(
            "mismatch",
            PAIRED_HOST_FRONTEND_STATUS,
            json!({"change_digest":digest}),
        )
        .await
        .unwrap_err();
        assert!(
            mismatch
                .to_string()
                .contains("does not match the retained package proposal")
        );
        assert!(
            invoke(
                "submit",
                PAIRED_HOST_FRONTEND_SUBMIT,
                json!({"change_digest":digest})
            )
            .await
            .is_err()
        );
        assert!(
            timeout(Duration::from_millis(10), decisions.next())
                .await
                .is_err(),
            "status and a spent call must not create approval"
        );
        // A different explicit call may ask for a new protected decision after
        // confirmed success. Status cannot approve it or reuse the old decision.
        let fresh = invoke(
            "fresh-submit",
            PAIRED_HOST_FRONTEND_SUBMIT,
            json!({"change_digest":digest}),
        );
        tokio::pin!(fresh);
        let mut decision = tokio::select! {
            result = &mut fresh => panic!("fresh submission bypassed its own decision: {result:?}"),
            prompt = timeout(FIXTURE_BOUND, decisions.next()) => prompt.unwrap().unwrap(),
        };
        decision.decide(ApprovalDecision::Deny).unwrap();
        assert!(timeout(FIXTURE_BOUND, fresh).await.unwrap().is_err());
        approvals.retire();
        let requests = fixture.finish().await;
        assert_eq!(
            requests
                .iter()
                .map(|request| request.method.as_str())
                .collect::<Vec<_>>(),
            ["GET", "POST", "POST", "GET", "GET"]
        );
        assert_eq!(
            query(&requests[0]).get("package_path").map(String::as_str),
            Some("extension")
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&requests[1].body).unwrap(),
            json!({"new_extension":role_proposal(selection)})
        );
        let body = std::str::from_utf8(&requests[2].body).unwrap();
        assert_eq!(
            body,
            format!(r#"{{"change_digest":"{digest}","change":{raw_change}}}"#)
        );
    }
}

fn query(request: &Request) -> BTreeMap<String, String> {
    reqwest::Url::parse(&format!("http://fixture.invalid{}", request.path))
        .unwrap()
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect()
}

#[tokio::test]
async fn role_prepare_keeps_exact_owner_pins_and_fixed_errors() {
    let mut mismatches = Vec::new();
    for (pointer, value) in [
        ("/change/proposal/source/package_path", json!("other")),
        (
            "/change/proposal/roles/daemon/voice_mode",
            json!("disabled"),
        ),
        (
            "/change/proposal/roles/voice_map",
            json!({"different":"binding"}),
        ),
    ] {
        let mut changed = role_prepared(1);
        *changed.pointer_mut(pointer).unwrap() = value;
        mismatches.push(changed);
    }
    let messages = [
        (
            "package_unreadable",
            "new_extension.source.package_path",
            "The package at the requested path could not be read from the source commit.",
        ),
        (
            "package_path_mismatch",
            "new_extension.source.package_path",
            "The source commit's build recipe does not build the package at the requested path.",
        ),
        (
            "package_identity_mismatch",
            "new_extension.distribution",
            "The package at the requested path is a different distribution or version.",
        ),
        (
            "roles_unsupported",
            "new_extension.roles",
            "This host has no role configuration for this extension.",
        ),
        (
            "role_unknown",
            "new_extension.roles.selected_roles",
            "The role selection names a role this extension does not define.",
        ),
        (
            "role_mode_incompatible",
            "new_extension.roles.daemon.voice_mode",
            "The daemon voice mode is not the one the selected roles require.",
        ),
        (
            "role_contract_missing",
            "new_extension.roles.required_facades",
            "The required facades are not exactly the selected roles' host contract.",
        ),
        (
            "voice_map_invalid",
            "new_extension.roles.voice_map",
            "The voice map does not fit the selected roles.",
        ),
        (
            "roles_incompatible",
            "new_extension.roles",
            "The role owner refuses this role configuration.",
        ),
        (
            "not_a_paired_host_extension",
            "new_extension.source.package_path",
            "The source declares no extension entry point for this host.",
        ),
        (
            "metadata_invalid",
            "new_extension.source.package_path",
            "The source's package metadata is not static, valid extension metadata for this host.",
        ),
    ];
    let failures:Vec<_> = messages.iter().map(|(code,path,message)|json!({"valid":false,"problems":[{"code":code,"path":path,"message":message}],"change":null})).collect();
    let mut forged = failures[0].clone();
    forged["problems"][0]["message"] = json!("private-response-marker");
    let fixture = Fixture::new(
        mismatches
            .into_iter()
            .chain(failures.clone())
            .chain([forged])
            .map(|body| Reply::json(200, body))
            .collect(),
    )
    .await;
    let mut client = PairedHostClient::new(&fixture.origin).unwrap();
    let proposal: FrontendNewExtensionRequest = serde_json::from_value(role_proposal(1)).unwrap();
    for _ in 0..3 {
        assert_eq!(
            client.prepare_new_extension(&proposal).await.unwrap_err(),
            PairedHostError::NewExtensionProposalMismatch
        );
    }
    for expected in failures {
        let FrontendNewExtensionPreparation::Candidate(receipt) =
            client.prepare_new_extension(&proposal).await.unwrap()
        else {
            panic!("candidate diagnostic")
        };
        assert_eq!(serde_json::to_value(receipt).unwrap(), expected);
    }
    assert_safe_unconfirmed(
        &client.prepare_new_extension(&proposal).await.unwrap_err(),
        PairedHostOperation::PrepareCandidate,
    );
    assert_eq!(fixture.finish().await.len(), messages.len() + 4);
}

#[tokio::test]
async fn role_tool_arguments_refuse_duplicate_decoded_keys_before_transport() {
    use meerkat_connect::paired_host_tool::{
        PAIRED_HOST_FRONTEND_PREPARE, PairedHostPreparationTools,
    };
    use meerkat_core::types::ToolCallView;
    let tools =
        PairedHostPreparationTools::new(PairedHostClient::new("http://127.0.0.1:9").unwrap())
            .unwrap();
    let raw = json!({"new_extension":role_proposal(1)}).to_string();
    for duplicate in [
        r#""voice_map":{"identity":"first","identity":"second"}"#,
        r#""voice_map":{"identity":"first","ident\u0069ty":"second"}"#,
    ] {
        let start = raw.find("\"voice_map\":{").unwrap();
        let end = start + raw[start..].find('}').unwrap() + 1;
        let corrupted = format!("{}{}{}", &raw[..start], duplicate, &raw[end..]);
        let args = serde_json::value::RawValue::from_string(corrupted).unwrap();
        let error = tools
            .dispatcher()
            .dispatch(ToolCallView {
                id: "duplicate",
                name: PAIRED_HOST_FRONTEND_PREPARE,
                args: &args,
            })
            .await
            .unwrap_err();
        assert!(
            matches!(error, meerkat_core::ToolError::ExecutionFailed { message } if message == "The paired host candidate request has an invalid or unknown field; use the offered closed schema")
        );
    }
}

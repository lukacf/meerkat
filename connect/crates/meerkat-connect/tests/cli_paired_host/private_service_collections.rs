//! Actual installed protocol7 collection acceptance. Only the model is scripted;
//! native installation, approvals, grants, Ready, MCP and public results are real.
use super::*;
use super::{private_services as prior, profile_denial, public_service as public};
use futures_util::FutureExt;
#[allow(
    dead_code,
    reason = "shared with the private service example host, which uses the rest of the contract"
)]
#[path = "../support/private_service_collection_contract.rs"]
pub(super) mod host_contract;
use std::os::unix::fs::OpenOptionsExt;
const INSTALL: &str = "Install the supplied Agno Hacker News integration for get_top_hackernews_stories and get_user_details. Ask for public service access to exactly those operations for rkat-agent until revoked. Finish setup without calling either service.";
const LIST_CALL: &str = "collection-first-stories";
const USER_CALL: &str = "collection-second-author";
const DONE: &str = "The selected native tool call completed. Collection assessment belongs to the retained trusted program, not this model answer.";
#[derive(Clone, Copy)]
enum Mode {
    Complete,
    CancelSecond,
    OutputLostSecond,
    ElapsedExpiry,
}
const EXPIRY_LIFETIME: Duration = Duration::from_secs(120);
fn wall_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}
fn selected() -> Value {
    json!({"family":"agno","profile":"hackernews","operations":[host_contract::OPERATION,host_contract::USER_OPERATION],"values":{"timeout":7}})
}
fn instruction(request: &Request) -> Option<(String, Value)> {
    request.body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .filter(|row| row["role"] == "user")
        .filter_map(|row| row["content"].as_str())
        .filter_map(|text| serde_json::from_str::<Value>(text).ok())
        .find(|value| {
            value.as_object().is_some_and(|m| m.len() == 3)
                && value["instruction"].is_string()
                && matches!(
                    value["tool"].as_str(),
                    Some(host_contract::OPERATION | host_contract::USER_OPERATION)
                )
        })
        .map(|value| {
            (
                value["tool"].as_str().unwrap().into(),
                value["arguments"].clone(),
            )
        })
}
fn response(request: &Request, phase: usize, lazy_add: bool) -> String {
    let body = &request.body;
    if let Some((operation, arguments)) = instruction(request) {
        let id = if operation == host_contract::OPERATION {
            LIST_CALL
        } else {
            USER_CALL
        };
        let results = body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["role"] == "tool" && row["tool_call_id"] == id)
            .collect::<Vec<_>>();
        if results.is_empty() {
            assert!(
                body["tools"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|tool| tool["function"]["name"] == operation)
            );
            if operation == host_contract::OPERATION {
                assert_eq!(arguments, json!({"num_stories":3}));
            } else {
                assert!(arguments["username"].is_string());
                assert_eq!(arguments.as_object().unwrap().len(), 1);
            }
            return deferred_call_response(id, &operation, arguments);
        }
        assert_eq!(results.len(), 1);
        let value = host_contract::decode_pair(results[0]["content"].as_str().unwrap()).unwrap();
        if operation == host_contract::OPERATION {
            host_contract::author_from_stories(&value).unwrap();
        } else {
            assert!(host_contract::exact_user(
                &value,
                arguments["username"].as_str().unwrap()
            ));
        }
        return public::completed(DONE);
    }
    let continuation = |request: &Request, job: &str, raw: &str| {
        assert_eq!(raw, host_contract::OPERATION);
        let body = &request.body;
        let metadata = tool_result(body, "current-operation-names");
        for operation in [host_contract::OPERATION, host_contract::USER_OPERATION] {
            assert!(
                metadata["operations"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|row| row["id"] == operation && row["tool"] == operation)
            );
        }
        assert_eq!(metadata["authentication"], json!({"kind":"none"}));
        assert_eq!(
            tool_result(body, "checked-install")["credential_environment_references"],
            json!([])
        );
        if profile_denial::optional_result(body, "review-public-access").is_none() {
            return deferred_call_response(
                "review-public-access",
                "toolkit_enable_public_service",
                json!({"job_id":job,
                "operations":[host_contract::OPERATION,host_contract::USER_OPERATION],"agent":"rkat-agent","expires_in_ms":null}),
            );
        }
        let reviewed = tool_result(body, "review-public-access");
        assert_eq!(reviewed["state"], "pending");
        assert_eq!(
            reviewed["reason"],
            "reviewed_finalization_waiting_for_setup_close"
        );
        assert_eq!(reviewed["access_reviewed"], true);
        assert_eq!(reviewed["grant_issued"], false);
        public::completed(public::SETUP_DONE)
    };
    if lazy_add {
        profile_denial::private_package_response(
            request,
            phase,
            INSTALL,
            selected(),
            host_contract::OPERATION,
            true,
            continuation,
        )
    } else {
        // The protected next-service startup already retained the actual
        // adapter. Keep all package/install checks and private-entry behavior;
        // only its explicit preparation expectation differs from service 1.
        profile_denial::package_response_scoped(
            request,
            phase,
            INSTALL,
            selected(),
            host_contract::OPERATION,
            false,
            "",
            continuation,
        )
    }
}
// A gate holds only the fixture model response. Both native calls are admitted
// by the real owner, and their tool results still come from the public service.
async fn start_provider() -> (
    Server,
    tokio::sync::mpsc::Receiver<(String, oneshot::Sender<()>)>,
) {
    start_provider_with_gate_bound(BOUND).await
}
async fn start_provider_with_gate_bound(
    gate_bound: Duration,
) -> (
    Server,
    tokio::sync::mpsc::Receiver<(String, oneshot::Sender<()>)>,
) {
    start_provider_with_preparation(
        gate_bound,
        Arc::new(std::sync::atomic::AtomicBool::new(true)),
    )
    .await
}
async fn start_provider_with_preparation(
    gate_bound: Duration,
    lazy_add: Arc<std::sync::atomic::AtomicBool>,
) -> (
    Server,
    tokio::sync::mpsc::Receiver<(String, oneshot::Sender<()>)>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::<Request>::new()));
    let captured = requests.clone();
    let (stop, mut stopped) = oneshot::channel();
    let (seen, observed) = tokio::sync::mpsc::channel(2);
    let task = tokio::spawn(async move {
        let mut setup_phase = 0;
        loop {
            let mut stream = tokio::select! {biased;_= &mut stopped=>break,accepted=listener.accept()=>accepted.unwrap().0};
            let received = timeout(BOUND, request(&mut stream)).await.unwrap();
            {
                let mut rows = captured.lock().unwrap();
                assert!(rows.len() < 128, "bounded two-service fixture requests");
                rows.push(Request {
                    method: received.method.clone(),
                    path: received.path.clone(),
                    headers: received.headers.clone(),
                    body: received.body.clone(),
                });
            }
            assert_eq!(received.method, "POST");
            assert_eq!(received.path, "/v1/chat/completions");
            assert_eq!(received.headers["authorization"], format!("Bearer {TOKEN}"));
            assert_eq!(received.body["model"], "cli-fixture-remote");
            if let Some((operation, _)) = instruction(&received) {
                let call_id = if operation == host_contract::OPERATION {
                    LIST_CALL
                } else {
                    USER_CALL
                };
                if !received.body["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|row| row["role"] == "tool" && row["tool_call_id"] == call_id)
                {
                    let (release, released) = oneshot::channel();
                    seen.send((operation, release)).await.unwrap();
                    tokio::select! {biased;_= &mut stopped=>break,joined=timeout(gate_bound,released)=>joined.unwrap().expect("retained parent release")};
                }
            }
            // Each real setup has a fresh native conversation. Collection
            // continuations do not consume the setup's bounded fixture phases.
            if instruction(&received).is_none()
                && profile_denial::optional_result(&received.body, "activate-services").is_none()
            {
                setup_phase = 0;
            }
            let body = response(
                &received,
                setup_phase,
                lazy_add.load(std::sync::atomic::Ordering::Acquire),
            );
            if instruction(&received).is_none() {
                setup_phase += 1;
            }
            let running = received.body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .rev()
                .find(|row| row["role"] == "tool")
                .and_then(|row| serde_json::from_str::<Value>(row["content"].as_str()?).ok())
                .is_some_and(|v| v["job"]["status"] == "running");
            sleep(Duration::from_secs(if running { 5 } else { 1 })).await;
            let wire = format!(
                "HTTP/1.1 200 Fixture\r\nConnection: close\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            timeout(BOUND, async {
                stream.write_all(wire.as_bytes()).await.unwrap();
                stream.shutdown().await.unwrap();
            })
            .await
            .unwrap();
        }
    });
    (
        Server {
            origin,
            requests,
            stop: Some(stop),
            task: Some(task),
        },
        observed,
    )
}
fn digest(value: &Value) {
    assert!(value.as_str().is_some_and(|value| {
        value.len() == 64
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }));
}
fn process_rows(rows: &[public::ObservedProcess]) -> Value {
    json!(
        rows.iter()
            .map(|row| json!({"pid":row.pid,"parent":row.parent,"group":row.group}))
            .collect::<Vec<_>>()
    )
}
fn start(provider: &str, lifetime: Option<Duration>) -> StdioCli {
    let prefix = PathBuf::from(
        std::env::var_os("CONNECT_REAL_LIBRARY_PREFIX").expect("reviewed supplied resource prefix"),
    );
    let binary = PathBuf::from(
        std::env::var_os("CONNECT_SERVICE_COLLECTION_HOST")
            .expect("exact emitted acceptance-only host"),
    );
    let expected =
        std::env::var("CONNECT_SERVICE_COLLECTION_HOST_SHA256").expect("emitted host digest");
    assert!(prefix.is_absolute() && binary.is_absolute());
    assert_eq!(binary.parent(), Some(prefix.join("bin").as_path()));
    assert!(fs::symlink_metadata(&binary).unwrap().is_file());
    assert!(
        expected.len() == 64
            && expected
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
    assert_eq!(file_digest(&binary), expected);
    let Some(pin) = option_env!("MEERKAT_CONNECT_RESOURCES_SHA256") else {
        panic!("matching test/host resource pin");
    };
    assert_eq!(
        file_digest(&prefix.join("libexec/meerkat-connect/resources.json")),
        pin
    );
    match lifetime {
        Some(lifetime) => StdioCli::start_selected_with_expiring_collection_host(
            provider,
            binary,
            40,
            Duration::from_secs(300),
            lifetime,
        ),
        None => StdioCli::start_selected_with_collection_host(
            provider,
            binary,
            40,
            Duration::from_secs(300),
        ),
    }
}
async fn initialize(cli: &mut StdioCli, provider: &mut Server) -> (String, String) {
    let binding =
        host_contract::Launch::read(&cli.root.path().join("trusted-collection-host.json")).unwrap();
    let Some(pin) = option_env!("MEERKAT_CONNECT_RESOURCES_SHA256") else {
        panic!("matching test/host resource pin");
    };
    cli.request("h:init", "connect/initialize", binding.initialize(pin));
    let reply = provider.with_profile_provider(cli.response("h:init")).await;
    assert!(
        reply.get("error").is_none(),
        "initialization refused: {reply}"
    );
    assert_eq!(reply["result"]["protocol_version"], 7);
    assert_eq!(
        reply["result"]["capabilities"]["service_collections"]["selection"],
        "trusted_process_sources"
    );
    assert_eq!(
        reply["result"]["capabilities"]["service_collections"]["slack_inbound"],
        "unsupported"
    );
    assert!(provider.requests.lock().unwrap().is_empty());
    (
        reply["result"]["scope"].as_str().unwrap().into(),
        reply["result"]["session_id"].as_str().unwrap().into(),
    )
}
async fn observed(cli: &mut StdioCli, provider: &mut Server, kind: &str, state: &str) -> Value {
    loop {
        assert!(
            cli.rows.len() < 4096,
            "bounded protocol observation without the required event"
        );
        if let Some(row) = cli
            .rows
            .iter()
            .find(|row| row["params"]["kind"] == kind && row["params"]["state"] == state)
        {
            return row["params"].clone();
        }
        provider.with_profile_provider(cli.next()).await;
    }
}
async fn reach_controller_review(
    cli: &mut StdioCli,
    provider: &mut Server,
    scope: &str,
    session: &str,
    helpers: &mut Vec<public::ObservedProcess>,
) -> (Value, String) {
    cli.request(
        "h:work",
        "connect/send",
        json!({"scope":scope,"text":INSTALL}),
    );
    let install = provider.with_profile_provider(cli.prompt("approval")).await;
    assert_eq!(
        install["params"]["prompt"]["body"]["kind"],
        "service_software_install"
    );
    assert_eq!(install["params"]["prompt"]["body"]["selection"], selected());
    cli.answer(
        install["id"].as_str().unwrap(),
        json!({"kind":"approval","decision":"approve"}),
    );
    let access = provider.with_profile_provider(cli.prompt("approval")).await;
    helpers.extend(public::observed_descendants(cli.child.as_ref().unwrap().id()).await);
    let body = &access["params"]["prompt"]["body"];
    assert_eq!(body["phase"], "public_service_access");
    assert_eq!(body["realm"], REALM);
    assert_eq!(body["authentication"], json!({"kind":"none"}));
    assert_eq!(
        body["grant"]["operations"],
        json!([host_contract::OPERATION, host_contract::USER_OPERATION])
    );
    assert_eq!(body["grant"]["agent"], "rkat-agent");
    assert_eq!(body["grant"]["expires_in_ms"], Value::Null);
    let job = body["job_id"].as_str().unwrap().to_owned();
    cli.answer(
        access["id"].as_str().unwrap(),
        json!({"kind":"approval","decision":"approve"}),
    );
    let work = provider.with_profile_provider(cli.response("h:work")).await;
    assert_eq!(work["result"]["completion"], "completed");
    assert_eq!(work["result"]["state"]["can_send"], false);
    let governance = provider.with_profile_provider(cli.prompt("approval")).await;
    let body = &governance["params"]["prompt"]["body"];
    assert_eq!(body["change"], "govern_existing_realm");
    assert_eq!(body["session_id"], session);
    assert_eq!(body["job_id"], job);
    assert!(
        cli.rows
            .iter()
            .any(|row| row["params"]["kind"] == "service_finalization"
                && row["params"]["state"] == "native_closed")
    );
    cli.answer(
        governance["id"].as_str().unwrap(),
        json!({"kind":"approval","decision":"approve"}),
    );
    let controller = provider.with_profile_provider(cli.prompt("approval")).await;
    let body = &controller["params"]["prompt"]["body"];
    assert_eq!(body["session_id"], session);
    assert_eq!(body["job_id"], job);
    assert_eq!(body["realm"], REALM);
    assert_eq!(
        body["permissions"],
        json!(["exact_controller_route", "native_tool_catalog"])
    );
    assert!(
        body["controller"]["key"]
            .as_str()
            .is_some_and(|key| !key.is_empty())
    );
    assert!(
        body["lifetime"]
            .as_str()
            .unwrap()
            .contains("Until explicitly revoked")
    );
    assert!(
        !cli.rows
            .iter()
            .any(|row| row["params"]["kind"] == "service_finalization"
                && row["params"]["state"] == "observed")
    );
    (controller, job)
}
fn write_json(path: &Path, value: &Value) {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(&serde_json::to_vec_pretty(value).unwrap())
        .unwrap();
    file.sync_all().unwrap();
}
// Failure cleanup first asks the real pipe owner to close by EOF and retains
// its exact Child. Exceptional forced cleanup is a sticky test failure, never
// native success. Kill only the child's original isolated process group.
async fn cleanup_child(cli: &mut StdioCli) -> bool {
    let Some(child) = cli.child.as_mut() else {
        return true;
    };
    child.stdin.take();
    let orderly = timeout(Duration::from_secs(300), async {
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .is_ok();
    if !orderly {
        let group = Pid::from_raw(child.id() as i32);
        let _ = killpg(group, Signal::SIGTERM);
        sleep(Duration::from_millis(500)).await;
        let _ = killpg(group, Signal::SIGKILL);
        // An observation timeout is not a reap. Retain the actual child until
        // the direct owner is terminal even on this exceptional failure path.
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                _ => sleep(Duration::from_millis(10)).await,
            }
        }
    }
    let child = cli.child.take().unwrap();
    let result = child.wait_with_output().unwrap();
    fs::write(
        cli.root.path().join("failure-cleanup.stderr"),
        result.stderr,
    )
    .unwrap();
    orderly
}
async fn cleanup_provider(provider: &mut Server) -> bool {
    if let Some(stop) = provider.stop.take() {
        let _ = stop.send(());
    }
    let Some(mut task) = provider.task.take() else {
        return false;
    };
    match timeout(BOUND, &mut task).await {
        Ok(Ok(())) => true,
        Ok(Err(_)) => false,
        Err(_) => {
            task.abort();
            let _ = task.await;
            false
        }
    }
}
fn arguments_digest(value: &Value) -> String {
    format!("{:x}", Sha256::digest(serde_json::to_vec(value).unwrap()))
}
fn verify_report(
    report: &Value,
    scope: &str,
    selected: (&str, &str, &str),
    server: &str,
    first_body: &Value,
    complete: bool,
) -> Value {
    let (source, session, job) = selected;
    assert!(!scope.is_empty());
    assert_eq!(report["source_id"], source);
    assert!(
        report["attempt_id"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
    );
    for (field, expected) in [
        ("application_id", host_contract::APP),
        ("instance_id", host_contract::INSTANCE),
        ("viewer_id", host_contract::VIEWER),
        ("login_scope_id", host_contract::LOGIN),
        ("origin", host_contract::ORIGIN),
        ("realm", REALM),
        ("session_id", session),
        ("job_id", job),
        ("family", "agno"),
        ("profile", "hackernews"),
        ("agent", "rkat-agent"),
    ] {
        assert_eq!(report[field], expected, "exact report {field}");
    }
    assert_eq!(
        report["reusable_source_coverage"],
        "unqualified_credential_continuity"
    );
    assert_eq!(report["total_calls"], 2);
    assert!(report["total_result_bytes"].as_u64().unwrap() > 0);
    assert_eq!(report["requirements"].as_array().unwrap().len(), 1);
    let row = &report["requirements"][0];
    assert_eq!(row["required_operation_id"], host_contract::REQUIRED);
    assert_eq!(row["resource"], host_contract::resource());
    assert_eq!(
        row["raw_operation_ceiling"],
        json!([host_contract::OPERATION, host_contract::USER_OPERATION])
    );
    let calls = row["calls"].as_array().unwrap();
    assert_eq!(calls.len(), 2);
    let stories = host_contract::decode_pair(tool_content(first_body, LIST_CALL)).unwrap();
    let author = host_contract::author_from_stories(&stories).unwrap();
    for (index, (op, args, call_id)) in [
        (
            host_contract::OPERATION,
            json!({"num_stories":3}),
            LIST_CALL,
        ),
        (
            host_contract::USER_OPERATION,
            json!({"username":author}),
            USER_CALL,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let call = &calls[index];
        assert_eq!(call["source_id"], source);
        assert_eq!(call["required_operation_id"], host_contract::REQUIRED);
        assert_eq!(call["resource"], host_contract::resource());
        assert_eq!(call["ordinal"], index + 1);
        assert_eq!(call["raw_operation"], op);
        assert_eq!(call["arguments_sha256"], arguments_digest(&args));
        let native = &call["native"];
        assert_eq!(native["profile_id"], host_contract::REQUIRED);
        assert_eq!(native["operation"], op);
        assert_eq!(native["session_id"], session);
        assert_eq!(native["job_id"], job);
        assert_eq!(native["server"], server);
        assert_eq!(native["resource"], host_contract::resource());
        assert_eq!(native["native_close_joined"], true);
        for (field, expected) in [
            ("application_id", host_contract::APP),
            ("instance_id", host_contract::INSTANCE),
            ("viewer_id", host_contract::VIEWER),
            ("login_scope_id", host_contract::LOGIN),
            ("origin", host_contract::ORIGIN),
            ("realm", REALM),
            ("agent", "rkat-agent"),
        ] {
            assert_eq!(native[field], expected, "exact call observation {field}");
        }
        if index == 0 || complete {
            assert_eq!(native["status"], "tool_completed");
            assert_eq!(native["dispatch_outcome"], "committed");
            assert_eq!(native["tool_call_id"], call_id);
            assert_eq!(native["native_terminal_committed"], true);
            assert!(native["input_id"].is_string() && native["run_id"].is_string());
            digest(&native["result_sha256"]);
            digest(&native["installed_connection_sha256"]);
        } else {
            assert_eq!(native["status"], "cancelled");
            assert_eq!(native["native_terminal_committed"], false);
            // Actual Accepted input retained by probe_turn before its wait.
            // The held model response is only timing, never an input receipt.
            assert!(native["input_id"].as_str().is_some_and(|id| !id.is_empty()));
            assert_ne!(native["input_id"], calls[0]["native"]["input_id"]);
            assert!(native["run_id"].is_null() && native["tool_call_id"].is_null());
            assert!(native["dispatch_outcome"].is_null() && native["result_sha256"].is_null());
        }
    }
    if complete {
        assert_eq!(report["status"], "historical_checks_completed");
        assert_eq!(row["assessment"], "verified");
        assert_eq!(report["observed_selection"], "rechecked_after_last_close");
        assert!(report["native_own_config_generation"].as_u64().is_some());
        assert_ne!(
            calls[0]["native"]["input_id"],
            calls[1]["native"]["input_id"]
        );
        assert_ne!(calls[0]["native"]["run_id"], calls[1]["native"]["run_id"]);
        assert_eq!(
            calls[0]["native"]["installed_connection_sha256"],
            calls[1]["native"]["installed_connection_sha256"]
        );
    } else {
        assert_eq!(report["status"], "cancelled");
        assert!(row["assessment"].is_null());
    }
    json!({"stories":stories,"selected_author":author})
}
fn tool_content<'a>(body: &'a Value, id: &str) -> &'a str {
    let results = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["role"] == "tool" && row["tool_call_id"] == id)
        .collect::<Vec<_>>();
    assert_eq!(results.len(), 1);
    results[0]["content"].as_str().unwrap()
}
async fn run(mode: Mode) {
    let label = match mode {
        Mode::Complete => "result-dependent-public-author",
        Mode::CancelSecond => "cancel-second-native-input",
        Mode::OutputLostSecond => "output-loss-second-native-input",
        Mode::ElapsedExpiry => "elapsed-expiry-entered-native-input",
    };
    let output = PathBuf::from(
        std::env::var_os("CONNECT_SERVICE_COLLECTION_OUTPUT").expect("fresh evidence parent"),
    )
    .join(label);
    assert!(output.is_absolute() && !output.exists());
    fs::create_dir(&output).unwrap();
    let elapsed_expiry = matches!(mode, Mode::ElapsedExpiry);
    let (mut provider, mut gates) = start_provider_with_gate_bound(if elapsed_expiry {
        EXPIRY_LIFETIME + BOUND
    } else {
        BOUND
    })
    .await;
    let mut held_gate = None;
    let mut cli = None;
    let mut helpers = Vec::new();
    let mut first_helpers = Vec::new();
    let mut second_helpers = Vec::new();
    let mut observation = Value::Null;
    let checked=std::panic::AssertUnwindSafe(async {
        cli=Some(start(&provider.origin,elapsed_expiry.then_some(EXPIRY_LIFETIME)));let cli=cli.as_mut().unwrap();
        let original_pid=cli.child.as_ref().unwrap().id();
        let launch_path=cli.root.path().join("trusted-collection-host.json");
        let launch_bytes=fs::read(&launch_path).unwrap();
        let deadline=serde_json::from_slice::<Value>(&launch_bytes).unwrap()["expires_at_ms"].as_u64().unwrap();
        let (scope,session)=initialize(cli,&mut provider).await;
        let (controller,job)=reach_controller_review(cli,&mut provider,&scope,&session,&mut helpers).await;
        let setup_body=provider.requests.lock().unwrap().last().unwrap().body.clone();prior::effect_counts(&setup_body,true,true);
        assert!(!setup_body["messages"].as_array().unwrap().iter().flat_map(|m|m["tool_calls"].as_array().into_iter().flatten())
            .any(|call|call["function"]["name"]==host_contract::USER_OPERATION));
        let descriptor=public::reviewed_descriptor(&setup_body,"","hackernews",host_contract::OPERATION);
        cli.answer(controller["id"].as_str().unwrap(),json!({"kind":"approval","decision":"approve"}));
        let commissioning=observed(cli,&mut provider,"service_commissioning","observed").await;
        let facts=&commissioning["observation"];assert_eq!(facts["state"],"completed");assert_eq!(facts["custody_released"],true);
        assert_eq!(facts["controller_disposal"],"drained");assert_eq!(facts["issuance"],"committed");
        assert_eq!(facts["grants"].as_array().unwrap().len(),2);
        assert!(facts["grants"].as_array().unwrap().iter().all(|g|g["state"]=="committed"&&g["issued"].is_string()));
        let finalization=observed(cli,&mut provider,"service_finalization","observed").await;
        assert_eq!(finalization["observation"]["facts"]["persistence"],"saved");
        assert_eq!(finalization["observation"]["facts"]["grant"]["state"],"selected");
        assert_eq!(finalization["observation"]["service_readiness"],"not_observed");
        let available=observed(cli,&mut provider,"service_collection_state","available").await;
        assert_eq!(available["session_id"],session);assert_eq!(available["job_id"],job);
        let count=provider.requests.lock().unwrap().len();
        for (id,params) in [("h:foreign-job",json!({"scope":scope,"job_id":"wrong","source_id":host_contract::SOURCE})),
            ("h:foreign-source",json!({"scope":scope,"job_id":job,"source_id":"foreign"}))] {
            cli.request(id,"connect/collect",params);
            assert_eq!(provider.with_profile_provider(cli.response(id)).await["error"]["data"]["code"],"invalid_binding");
        }
        assert_eq!(provider.requests.lock().unwrap().len(),count);
        cli.request("h:collection","connect/collect",json!({"scope":scope,"job_id":job,"source_id":host_contract::SOURCE}));
        let (first,release)=provider.with_profile_provider(timeout(BOUND,gates.recv())).await.unwrap().unwrap();held_gate=Some(release);
        assert_eq!(first,host_contract::OPERATION);first_helpers=public::observed_descendants(cli.child.as_ref().unwrap().id()).await;
        assert!(!first_helpers.iter().any(|row|helpers.iter().any(|old|old.pid==row.pid)));
        held_gate.take().unwrap().send(()).expect("first actual model response still held");
        let (second,release)=provider.with_profile_provider(timeout(BOUND,gates.recv())).await.unwrap().unwrap();held_gate=Some(release);
        assert_eq!(second,host_contract::USER_OPERATION);
        second_helpers=public::observed_descendants(cli.child.as_ref().unwrap().id()).await;
        assert!(!second_helpers.iter().any(|row|helpers.iter().chain(&first_helpers).any(|old|old.pid==row.pid)));
        public::assert_observed_helpers_gone(&first_helpers).await;
        let held_since=std::time::Instant::now();let entered_before_ms=wall_ms();
        if elapsed_expiry {
            assert!(entered_before_ms.checked_add(1000).unwrap()<deadline,"native second input must enter at least one second before original expiry");
            assert!(!held_gate.as_ref().unwrap().is_closed(),"actual provider response gate must still be held");
            assert!(!cli.rows.iter().any(|row|row["id"]=="h:collection"),"no collection result before the deadline");
        }
        let second_body=provider.requests.lock().unwrap().last().unwrap().body.clone();
        let stories=host_contract::decode_pair(tool_content(&second_body,LIST_CALL)).unwrap();let author=host_contract::author_from_stories(&stories).unwrap();
        let (operation,arguments)=instruction(&Request {method:"POST".into(),path:"/v1/chat/completions".into(),headers:Default::default(),body:second_body.clone()}).unwrap();
        assert_eq!(operation,host_contract::USER_OPERATION);assert_eq!(arguments,json!({"username":author}));
        let pending_count=provider.requests.lock().unwrap().len();
        cli.request("h:while-active","connect/collect",json!({"scope":scope,"job_id":job,"source_id":host_contract::SOURCE}));
        assert_eq!(provider.with_profile_provider(cli.response("h:while-active")).await["error"]["data"]["code"],"collection_unavailable");
        assert_eq!(provider.requests.lock().unwrap().len(),pending_count);
        let quiet_wait_started_ms=wall_ms();
        if elapsed_expiry {
            assert!(quiet_wait_started_ms.checked_add(1000).unwrap()<deadline,"last parent frame must precede original expiry");
        }
        match mode {
            Mode::Complete=>{
                held_gate.take().unwrap().send(()).expect("second actual model response still held");
                let result=provider.with_profile_provider(cli.response("h:collection")).await;
                assert_eq!(result["result"]["scope"],scope);assert_eq!(result["result"]["collection_request_id"],"h:collection");
                let public_values=verify_report(&result["result"]["report"],&scope,(host_contract::SOURCE,&session,&job),&descriptor.name,&second_body,true);
                let final_body=provider.requests.lock().unwrap().last().unwrap().body.clone();
                let user=host_contract::decode_pair(tool_content(&final_body,USER_CALL)).unwrap();assert!(host_contract::exact_user(&user,&author));
                let count=provider.requests.lock().unwrap().len();
                cli.request("h:repeat","connect/collect",json!({"scope":scope,"job_id":job,"source_id":host_contract::SOURCE}));
                assert_eq!(provider.with_profile_provider(cli.response("h:repeat")).await["error"]["data"]["code"],"collection_already_attempted");
                cli.request("h:retire-collection","connect/cancel",json!({"scope":scope,"send_request_id":"h:collection"}));
                assert_eq!(provider.with_profile_provider(cli.response("h:retire-collection")).await["result"]["status"],"not_current");
                cli.request("h:after","connect/collect",json!({"scope":scope,"job_id":job,"source_id":"after-retirement"}));
                assert_eq!(provider.with_profile_provider(cli.response("h:after")).await["error"]["data"]["code"],"collection_unavailable");
                assert_eq!(provider.requests.lock().unwrap().len(),count);prior::close(cli,&mut provider,&scope).await;
                observation=json!({"collection":result,"public_values":public_values,"user":user});
            },
            Mode::CancelSecond|Mode::OutputLostSecond|Mode::ElapsedExpiry=>{
                if !matches!(mode,Mode::OutputLostSecond) {
                    if matches!(mode,Mode::CancelSecond) {
                        cli.request("h:stop","connect/cancel",json!({"scope":scope,"send_request_id":"h:collection"}));
                        assert_eq!(provider.with_profile_provider(cli.response("h:stop")).await["result"]["status"],"cancellation_requested");
                    }
                    // Expiry sends no cancel, retire, close, EOF or clock mutation.
                    // The original native deadline must settle the held input.
                    let result=provider.with_profile_provider(cli.response("h:collection")).await;
                    assert_eq!(result["result"]["collection_request_id"],"h:collection");assert_eq!(result["result"]["scope"],scope);
                    verify_report(&result["result"]["report"],&scope,(host_contract::SOURCE,&session,&job),&descriptor.name,&second_body,false);
                    observation=json!({"delivered_collection":result});
                    if elapsed_expiry {
                        let settled_at_ms=wall_ms();let held_for_ms=held_since.elapsed().as_millis();
                        assert!(settled_at_ms>=deadline,"native result cannot stand in for elapsed expiry");
                        assert!(settled_at_ms<deadline+15_000,"native expiry must settle within 15 seconds of the original deadline");
                        assert!(held_for_ms+5>=u128::from(deadline-entered_before_ms),"wall-clock jump is not elapsed expiry");
                        assert!(!held_gate.as_ref().unwrap().is_closed(),"gate release or watchdog is not expiry proof");
                        assert_eq!(fs::read(&launch_path).unwrap(),launch_bytes,"original launch expiry remains immutable");
                        assert_eq!(cli.child.as_ref().unwrap().id(),original_pid);
                        let before=provider.requests.lock().unwrap().len();
                        cli.request("h:after-elapsed-expiry","connect/collect",json!({"scope":scope,"job_id":job,"source_id":"after-retirement"}));
                        let denied=provider.with_profile_provider(cli.response("h:after-elapsed-expiry")).await;
                        assert_eq!(denied["error"]["data"]["code"],"collection_unavailable");
                        assert_eq!(provider.requests.lock().unwrap().len(),before,"no new native input after expiry");
                        observation["elapsed_expiry"]=json!({"original_deadline_ms":deadline,"entered_before_ms":entered_before_ms,
                            "quiet_wait_started_ms":quiet_wait_started_ms,"settled_at_ms":settled_at_ms,"held_for_ms":held_for_ms,"original_pid":original_pid,
                            "gate_released":false,"subsequent_admission":denied,
                            "scope":"entered_native_input_waiting_for_model_response","second_raw_tool_entered":false});
                    }
                    cli.request("h:close","connect/close",json!({"target":{"scope":scope}}));
                    assert_eq!(provider.with_profile_provider(cli.response("h:close")).await["error"]["data"]["code"],"shutdown_unconfirmed");
                } else {
                    // Close the actual parent read endpoint. A subsequent reply
                    // forces the real native writer to observe broken output.
                    drop(cli.stdout.take().unwrap());
                    cli.request("h:output-trigger","connect/history",json!({"scope":scope}));
                    observation=json!({"delivered_collection":null,"output_read_endpoint_closed":true});
                }
                assert!(!provider.with_profile_provider(cli.finish_output()).await.status.success());
                let (_,returned)=read_bounded_json(&cli.root.path().join("returned-collection-failure.json"),8*1024*1024);
                assert_eq!(returned["delivery"],"not_claimed");assert_eq!(returned["original_error_retained"],true);
                assert_eq!(returned["reports"].as_array().unwrap().len(),1);
                verify_report(&returned["reports"][0],&scope,(host_contract::SOURCE,&session,&job),&descriptor.name,&second_body,false);
                if elapsed_expiry {
                    assert_eq!(returned["reports"][0],observation["delivered_collection"]["result"]["report"]);
                    observation["elapsed_expiry"]["original_process_joined"]=json!(true);
                    observation["elapsed_expiry"]["joined_at_ms"]=json!(wall_ms());
                }
                observation["returned_rust_failure"]=returned;
                // The held second model response was never released, so no
                // get_user_details result can be invented or credited.
                assert!(!provider.requests.lock().unwrap().iter().any(|request|request.body["messages"].as_array().unwrap()
                    .iter().any(|row|row["role"]=="tool"&&row["tool_call_id"]==USER_CALL)));
            },
        }
        observation["commissioning"]=commissioning;observation["finalization"]=finalization;
        observation["preparation"]=prior::preparation(cli);observation["reusable_source_coverage"]=json!("unqualified_credential_continuity");
        let positions=[("service_commissioning","observed"),("service_finalization","observed"),("service_collection_state","available")].map(|(kind,state)|
            cli.rows.iter().position(|row|row["params"]["kind"]==kind&&row["params"]["state"]==state).unwrap());
        assert!(positions[0]<positions[1]&&positions[1]<positions[2]);
    }).catch_unwind().await;
    let child_cleanup = std::panic::AssertUnwindSafe(async {
        match cli.as_mut() {
            Some(cli) => cleanup_child(cli).await,
            None => true,
        }
    })
    .catch_unwind()
    .await;
    let provider_cleanup = cleanup_provider(&mut provider).await;
    drop(held_gate);
    let child_cleanup = matches!(child_cleanup, Ok(true));
    let setup_cleanup =
        std::panic::AssertUnwindSafe(public::assert_observed_helpers_gone(&helpers))
            .catch_unwind()
            .await
            .is_ok();
    let first_cleanup =
        std::panic::AssertUnwindSafe(public::assert_observed_helpers_gone(&first_helpers))
            .catch_unwind()
            .await
            .is_ok();
    let second_cleanup =
        std::panic::AssertUnwindSafe(public::assert_observed_helpers_gone(&second_helpers))
            .catch_unwind()
            .await
            .is_ok();
    let helper_cleanup = setup_cleanup && first_cleanup && second_cleanup;
    let evidence = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let retained = cli.as_mut().map(|cli| {
            let root = std::mem::replace(&mut cli.root, tempfile::tempdir().unwrap()).keep();
            write_json(&output.join("retained-root.json"), &json!({"root":root}));
            write_json(&output.join("private-frames.json"), &json!(cli.rows));
            json!({"root":root,"host":cli.binary,"host_sha256":file_digest(&cli.binary)})
        });
        let model_requests = provider
            .requests
            .lock()
            .unwrap()
            .iter()
            .map(|request| json!({"method":request.method,"path":request.path,"body":request.body}))
            .collect::<Vec<_>>();
        write_json(&output.join("model-requests.json"), &json!(model_requests));
        write_json(
            &output.join("OBSERVATIONS.json"),
            &json!({"status":if checked.is_ok()&&child_cleanup&&provider_cleanup&&helper_cleanup{"passed"}else{"failed"},
            "retained":retained,"observations":observation,"child_cleanup_orderly":child_cleanup,"fixture_provider_joined":provider_cleanup,
            "setup_helpers":process_rows(&helpers),"first_helpers":process_rows(&first_helpers),"second_helpers":process_rows(&second_helpers),
            "setup_helpers_gone":setup_cleanup,"first_helpers_gone":first_cleanup,"second_helpers_gone":second_cleanup,
            "resource_manifest_sha256":option_env!("MEERKAT_CONNECT_RESOURCES_SHA256"),"full_goal_ready":false}),
        );
    }));
    if evidence.is_err() {
        eprintln!("acceptance evidence write failed; original body failure remains primary");
    }
    if let Err(panic) = checked {
        std::panic::resume_unwind(panic);
    }
    if let Err(panic) = evidence {
        std::panic::resume_unwind(panic);
    }
    assert!(
        child_cleanup && provider_cleanup && helper_cleanup,
        "actual installed collection owners did not join normally"
    );
}
#[tokio::test]
#[ignore = "exact emitted v7 host and matching real library; live public Hacker News result-dependent calls"]
async fn actual_v7_installed_first_native_result_drives_second_call() {
    run(Mode::Complete).await;
}
#[tokio::test]
#[ignore = "exact emitted v7 host and matching real library; live first public call, cancel second native input"]
async fn actual_v7_installed_cancel_second_input_preserves_first_committed_row() {
    run(Mode::CancelSecond).await;
}
#[tokio::test]
#[ignore = "exact emitted v7 host and matching real library; live first public call, actual output pipe loss"]
async fn actual_v7_installed_output_loss_joins_and_retains_partial_rust_report() {
    run(Mode::OutputLostSecond).await;
}

#[path = "private_service_collections/phases.rs"]
mod phases;

#[tokio::test]
#[ignore = "exact emitted v7 host and real library; original 120s expiry across a gated entered native input"]
async fn actual_v7_installed_elapsed_expiry_cancels_entered_native_input_and_retains_report() {
    run(Mode::ElapsedExpiry).await;
}

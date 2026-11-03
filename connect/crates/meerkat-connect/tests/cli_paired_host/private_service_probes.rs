//! Full installed same-binding protocol6 acceptance with real supplied resources.
//! The scripted model chooses calls; packaged installation, native review/grants,
//! reopening, MCP dispatch and public Hacker News responses are not fabricated.
use super::*;
use super::{private_services as prior, profile_denial, public_service as public};
use futures_util::FutureExt;
#[allow(
    dead_code,
    reason = "shared with the private service example host, which uses the rest of the contract"
)]
#[path = "../support/private_service_probe_contract.rs"]
pub(super) mod host_contract;
use std::os::unix::fs::OpenOptionsExt;
const PROBE_CALL: &str = "same-binding-public-read";
const DONE: &str =
    "The exact native public read returned three stories. This does not qualify another resource.";

fn is_probe(request: &Request) -> bool {
    let body = &request.body;
    let selected = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .filter(|row| row["role"] == "user")
        .filter_map(|row| row["content"].as_str())
        .filter_map(|text| serde_json::from_str::<Value>(text).ok())
        .find(|value| {
            value.as_object().is_some_and(|map| map.len() == 3)
                && value["tool"] == public::OPERATION
                && value["arguments"] == json!({"num_stories":3})
                && value["instruction"].is_string()
        });
    selected.is_some()
}
fn response(request: &Request, phase: usize) -> String {
    let body = &request.body;
    if is_probe(request) {
        let results = body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["role"] == "tool" && row["tool_call_id"] == PROBE_CALL)
            .collect::<Vec<_>>();
        if results.is_empty() {
            assert!(
                body["tools"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|tool| tool["function"]["name"] == public::OPERATION),
                "real narrowed MCP operation must be offered"
            );
            return deferred_call_response(PROBE_CALL, public::OPERATION, json!({"num_stories":3}));
        }
        assert_eq!(results.len(), 1);
        public::public_stories(results[0]["content"].as_str().unwrap()).unwrap();
        return public::completed(DONE);
    }
    // Reuse the actual package-owned prepare/check/install/readback producer.
    // The protocol3 absence assertions in prior::response are intentionally
    // not reused: protocol6 legitimately exposes the reviewed v5 service owner.
    profile_denial::private_package_response(
        request,
        phase,
        prior::INSTALL,
        prior::selected(),
        public::OPERATION,
        true,
        public::public_access,
    )
}
// Hold the actual reopened model request, after native construction and MCP
// attachment, while the parent observes its real helper identities. Releasing
// this gate changes only fixture model latency, never the native tool result.
async fn start_provider() -> (Server, oneshot::Receiver<()>, oneshot::Sender<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::<Request>::new()));
    let captured = requests.clone();
    let (stop, mut stopped) = oneshot::channel();
    let (seen, observed) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let task = tokio::spawn(async move {
        let mut gate = Some((seen, released));
        loop {
            let mut stream = tokio::select! {biased;_= &mut stopped=>break,
            accepted=listener.accept()=>accepted.unwrap().0};
            let received = timeout(BOUND, request(&mut stream)).await.unwrap();
            let phase = {
                let mut rows = captured.lock().unwrap();
                let phase = rows.len();
                rows.push(Request {
                    method: received.method.clone(),
                    path: received.path.clone(),
                    headers: received.headers.clone(),
                    body: received.body.clone(),
                });
                phase
            };
            assert_eq!(received.method, "POST");
            assert_eq!(received.path, "/v1/chat/completions");
            assert_eq!(received.headers["authorization"], format!("Bearer {TOKEN}"));
            assert_eq!(received.body["model"], "cli-fixture-remote");
            if is_probe(&received)
                && let Some((seen, released)) = gate.take()
            {
                seen.send(())
                    .expect("retained parent observes native probe construction");
                tokio::select! {biased;_= &mut stopped=>break,
                joined=timeout(BOUND,released)=>joined.unwrap().expect("parent releases model response")};
            }
            let body = response(&received, phase);
            let running = received.body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .rev()
                .find(|row| row["role"] == "tool")
                .and_then(|row| serde_json::from_str::<Value>(row["content"].as_str()?).ok())
                .is_some_and(|value| value["job"]["status"] == "running");
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
        release,
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
fn start(provider: &str) -> StdioCli {
    let prefix = PathBuf::from(
        std::env::var_os("CONNECT_REAL_LIBRARY_PREFIX").expect("reviewed supplied resource prefix"),
    );
    let binary = PathBuf::from(
        std::env::var_os("CONNECT_SERVICE_PROBE_HOST").expect("exact emitted acceptance-only host"),
    );
    let expected = std::env::var("CONNECT_SERVICE_PROBE_HOST_SHA256").expect("emitted host digest");
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
    StdioCli::start_selected_with_probe_host(provider, binary, 40, Duration::from_secs(300), true)
}
async fn initialize(cli: &mut StdioCli, provider: &mut Server) -> (String, String) {
    let binding =
        host_contract::Launch::read(&cli.root.path().join("trusted-probe-host.json")).unwrap();
    let Some(pin) = option_env!("MEERKAT_CONNECT_RESOURCES_SHA256") else {
        panic!("matching test/host resource pin");
    };
    cli.request("h:init", "connect/initialize", binding.initialize(pin));
    let reply = provider.with_profile_provider(cli.response("h:init")).await;
    assert!(
        reply.get("error").is_none(),
        "initialization refused: {reply}"
    );
    assert_eq!(reply["result"]["protocol_version"], 6);
    assert_eq!(
        reply["result"]["capabilities"]["service_probes"]["selection"],
        "trusted_process_profiles"
    );
    assert_eq!(
        reply["result"]["capabilities"]["service_probes"]["slack_inbound"],
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
        json!({"scope":scope,"text":prior::INSTALL}),
    );
    let install = provider.with_profile_provider(cli.prompt("approval")).await;
    assert_eq!(
        install["params"]["prompt"]["body"]["kind"],
        "service_software_install"
    );
    assert_eq!(
        install["params"]["prompt"]["body"]["selection"],
        prior::selected()
    );
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
    assert_eq!(body["grant"]["operations"], json!([public::OPERATION]));
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
async fn run(retire_at_review: bool) {
    let output = PathBuf::from(
        std::env::var_os("CONNECT_SERVICE_PROBE_OUTPUT").expect("fresh evidence parent"),
    )
    .join(if retire_at_review {
        "retire-controller-review"
    } else {
        "same-binding-public-read"
    });
    assert!(output.is_absolute() && !output.exists());
    fs::create_dir(&output).unwrap();
    let (mut provider, probe_seen, probe_release) = start_provider().await;
    let mut cli = None;
    let mut helpers = Vec::new();
    let mut probe_helpers = Vec::new();
    let mut observation = Value::Null;
    let checked=std::panic::AssertUnwindSafe(async {
        cli=Some(start(&provider.origin));let cli=cli.as_mut().unwrap();
        let (scope,session)=initialize(cli,&mut provider).await;
        let (controller,job)=reach_controller_review(cli,&mut provider,&scope,&session,&mut helpers).await;
        let setup_body=provider.requests.lock().unwrap().last().unwrap().body.clone();
        prior::effect_counts(&setup_body,true,true);
        let descriptor=public::reviewed_descriptor(&setup_body,"","hackernews",public::OPERATION);
        if retire_at_review {
            let count=provider.requests.lock().unwrap().len();
            cli.request("h:cancel-review","connect/cancel",json!({"scope":scope,"send_request_id":"h:work"}));
            let cancelled=provider.with_profile_provider(cli.response("h:cancel-review")).await;
            assert_eq!(cancelled["result"]["status"],"not_current");
            let commissioning=observed(cli,&mut provider,"service_commissioning","observed").await;
            let facts=&commissioning["observation"];
            assert_eq!(facts["state"],"incomplete");assert_eq!(facts["governance"],"committed");
            assert_eq!(facts["grants"],json!([]));assert_eq!(facts["custody_released"],true);
            assert_eq!(facts["controller_disposal"],"drained");assert_eq!(facts["issuance"],"not_entered");
            cli.request("h:refused-probe","connect/probe",json!({"scope":scope,"job_id":job,"profile_id":"public-read"}));
            assert_eq!(provider.with_profile_provider(cli.response("h:refused-probe")).await["error"]["data"]["code"],"probe_unavailable");
            cli.request("h:close","connect/close",json!({"target":{"scope":scope}}));
            let close=provider.with_profile_provider(cli.response("h:close")).await;
            assert_eq!(close["error"]["data"]["code"],"shutdown_unconfirmed");
            assert!(!provider.with_profile_provider(cli.finish_output()).await.status.success());
            assert_eq!(provider.requests.lock().unwrap().len(),count);
            assert!(!cli.rows.iter().any(|row|row["params"]["kind"]=="service_finalization"&&row["params"]["state"]=="observed"));
            let own=fs::read_to_string(cli.state.join(REALM).join("config.toml")).unwrap();
            assert!(meerkat_core::Config::from_persisted_toml(&own).unwrap().0.tools.mcp_servers.is_empty());
            observation=json!({"commissioning":commissioning,"probe":"refused","service_finalization":"not_entered","close":close,
                "saved_governance":"retained_not_rolled_back"});
        } else {
            cli.answer(controller["id"].as_str().unwrap(),json!({"kind":"approval","decision":"approve"}));
            let commissioning=observed(cli,&mut provider,"service_commissioning","observed").await;
            let facts=&commissioning["observation"];
            assert_eq!(facts["state"],"completed");assert_eq!(facts["custody_released"],true);
            assert_eq!(facts["controller_disposal"],"drained");assert_eq!(facts["issuance"],"committed");
            assert_eq!(facts["grants"].as_array().unwrap().len(),2);
            assert!(facts["grants"].as_array().unwrap().iter().all(|g|g["state"]=="committed"&&g["issued"].is_string()));
            let finalization=observed(cli,&mut provider,"service_finalization","observed").await;
            assert_eq!(finalization["observation"]["facts"]["persistence"],"saved");
            assert_eq!(finalization["observation"]["facts"]["grant"]["state"],"selected");
            assert_eq!(finalization["observation"]["service_readiness"],"not_observed");
            let available=observed(cli,&mut provider,"service_probe_state","available").await;
            assert_eq!(available["state"],"available");assert_eq!(available["session_id"],session);assert_eq!(available["job_id"],job);
            let before=provider.requests.lock().unwrap().len();
            cli.request("h:foreign-job","connect/probe",json!({"scope":scope,"job_id":"wrong","profile_id":"public-read"}));
            assert_eq!(provider.with_profile_provider(cli.response("h:foreign-job")).await["error"]["data"]["code"],"invalid_binding");
            assert_eq!(provider.requests.lock().unwrap().len(),before);
            cli.request("h:probe","connect/probe",json!({"scope":scope,"job_id":job,"profile_id":"public-read"}));
            provider.with_profile_provider(timeout(Duration::from_secs(300),probe_seen)).await.unwrap().unwrap();
            probe_helpers=public::observed_descendants(cli.child.as_ref().unwrap().id()).await;
            assert!(!probe_helpers.iter().any(|row|helpers.iter().any(|old|old.pid==row.pid)),
                "the closed setup helper is not the fresh probe helper");
            probe_release.send(()).expect("actual held probe model response remains owned");
            let probed=provider.with_profile_provider(cli.response("h:probe")).await;
            assert_eq!(probed["result"]["scope"],scope);assert_eq!(probed["result"]["probe_request_id"],"h:probe");
            let report=&probed["result"]["report"];
            assert_eq!(report["status"],"tool_completed");assert_eq!(report["dispatch_outcome"],"committed");
            assert_eq!(report["native_terminal_committed"],true);assert_eq!(report["native_close_joined"],true);
            assert_eq!(report["session_id"],session);assert_eq!(report["job_id"],job);assert_eq!(report["realm"],REALM);
            assert_eq!(report["server"],descriptor.name);assert_eq!(report["operation"],public::OPERATION);
            assert_eq!(report["tool_call_id"],PROBE_CALL);assert!(report["input_id"].is_string()&&report["run_id"].is_string());
            assert_eq!(report["application_id"],host_contract::APP);assert_eq!(report["viewer_id"],host_contract::VIEWER);
            assert_eq!(report["instance_id"],host_contract::INSTANCE);assert_eq!(report["login_scope_id"],host_contract::LOGIN);
            assert_eq!(report["origin"],host_contract::ORIGIN);assert_eq!(report["profile_id"],"public-read");
            assert_eq!(report["agent"],"rkat-agent");digest(&report["result_sha256"]);digest(&report["installed_connection_sha256"]);
            assert_eq!(report["resource"],json!({"service":"hackernews","operation":public::OPERATION,"count":3}));
            let actual=provider.requests.lock().unwrap().last().unwrap().body.clone();
            let returned=actual["messages"].as_array().unwrap().iter().filter(|m|m["role"]=="tool"&&m["tool_call_id"]==PROBE_CALL).collect::<Vec<_>>();
            assert_eq!(returned.len(),1);let stories=public::public_stories(returned[0]["content"].as_str().unwrap()).unwrap();
            let count=provider.requests.lock().unwrap().len();
            cli.request("h:repeat","connect/probe",json!({"scope":scope,"job_id":job,"profile_id":"public-read"}));
            assert_eq!(provider.with_profile_provider(cli.response("h:repeat")).await["error"]["data"]["code"],"probe_already_attempted");
            cli.request("h:cancel-probe","connect/cancel",json!({"scope":scope,"send_request_id":"h:probe"}));
            assert_eq!(provider.with_profile_provider(cli.response("h:cancel-probe")).await["result"]["status"],"not_current");
            cli.request("h:after","connect/probe",json!({"scope":scope,"job_id":job,"profile_id":"after-retirement"}));
            assert_eq!(provider.with_profile_provider(cli.response("h:after")).await["error"]["data"]["code"],"probe_unavailable");
            assert_eq!(provider.requests.lock().unwrap().len(),count);
            prior::close(cli,&mut provider,&scope).await;
            observation=json!({"commissioning":commissioning,"finalization":finalization,"probe":probed,
                "public_stories":stories,"capability_verified":false,"ghe_or_slack_qualification":false});
        }
        if !retire_at_review {
            let event_positions=[("service_commissioning","observed"),("service_finalization","observed"),("service_probe_state","available")].map(|(kind,state)|
                cli.rows.iter().position(|row|row["params"]["kind"]==kind&&row["params"]["state"]==state
                    &&(kind!="service_probe_state"||(row["params"]["session_id"]==session&&row["params"]["job_id"]==job))).unwrap());
            assert!(event_positions[0]<event_positions[1]&&event_positions[1]<event_positions[2]);
        }
        observation["preparation"]=prior::preparation(cli);
    }).catch_unwind().await;
    let child_cleanup = std::panic::AssertUnwindSafe(async {
        match cli.as_mut() {
            Some(cli) => cleanup_child(cli).await,
            None => true,
        }
    })
    .catch_unwind()
    .await;
    // A child-cleanup assertion cannot skip the distinct fixture provider join.
    let provider_cleanup = cleanup_provider(&mut provider).await;
    let child_cleanup = matches!(child_cleanup, Ok(true));
    // Run the exact transitive identity/group observation on every exit,
    // including assertion failures. Direct child reap is not helper cleanup.
    let setup_cleanup =
        std::panic::AssertUnwindSafe(public::assert_observed_helpers_gone(&helpers))
            .catch_unwind()
            .await
            .is_ok();
    let probe_cleanup =
        std::panic::AssertUnwindSafe(public::assert_observed_helpers_gone(&probe_helpers))
            .catch_unwind()
            .await
            .is_ok();
    let helper_cleanup = setup_cleanup && probe_cleanup;
    // Evidence failures cannot replace the original checked failure. Retain
    // the actual root before fallible output writes; never delete its evidence
    // merely because writing the convenient observation summary failed.
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
            "setup_helpers":process_rows(&helpers),"probe_helpers":process_rows(&probe_helpers),
            "setup_helpers_gone":setup_cleanup,"probe_helpers_gone":probe_cleanup,"observed_helpers_gone":helper_cleanup,
            "resource_manifest_sha256":option_env!("MEERKAT_CONNECT_RESOURCES_SHA256"),"full_goal_ready":false}),
        );
    }));
    if evidence.is_err() {
        eprintln!(
            "acceptance evidence write failed; original body failure, if any, remains primary"
        );
    }
    if let Err(panic) = checked {
        std::panic::resume_unwind(panic);
    }
    if let Err(panic) = evidence {
        std::panic::resume_unwind(panic);
    }

    assert!(
        child_cleanup && provider_cleanup && helper_cleanup,
        "actual acceptance owners did not join normally"
    );
}
#[tokio::test]
#[ignore = "exact emitted trusted test host and matching real library; live public Hacker News read"]
async fn actual_v6_packaged_install_commission_same_session_probe_and_retire() {
    run(false).await;
}
#[tokio::test]
#[ignore = "exact emitted trusted test host and matching real library; actual installation, no service call"]
async fn actual_v6_packaged_install_cancel_controller_review_joins_without_finalization() {
    run(true).await;
}

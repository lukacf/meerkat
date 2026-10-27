//! Copied CLI acceptance through actual supplied package owners. The loopback
//! model proposes fixed nonsecret inputs; it never supplies installation facts.
use super::*;

const PROMPT: &str = "Prepare and install the supplied Slack auth-test integration once, then request client profile setup. If I deny that profile, read one supplied recipe and finish this same request without signing in or invoking Slack.";
const DONE: &str = "Client profile setup was denied. The recipe was read in the same request. Software installation alone does not establish account access or service readiness.";
const PROFILE_CALL: &str = "denied-client-profile";
const PROFILE: &str = "cli-denied-profile";
const PACKAGE_TOOLS: &[&str] = &[
    "setup_prepare", "setup_help", "setup_check", "setup_install", "setup_job", "setup_reconcile",
];

fn selection() -> Value {
    json!({"family":"services","profile":"slack","operations":["auth_test"],
        "values":{"token_env":"CONNECT_UNUSED_SLACK_TOKEN","timeout_seconds":2,"max_response_bytes":65536}})
}

pub(super) fn optional_result(body: &Value, id: &str) -> Option<Value> {
    body["messages"].as_array().unwrap().iter()
        .find(|message| message["role"] == "tool" && message["tool_call_id"] == id)
        .map(|message| serde_json::from_str(message["content"].as_str().unwrap()).unwrap())
}

fn latest_job(body: &Value, id: &str, first: &Value) -> Value {
    body["messages"].as_array().unwrap().iter().rev()
        .filter(|message| message["role"] == "tool")
        .filter_map(|message| serde_json::from_str::<Value>(message["content"].as_str().unwrap()).ok())
        .find(|value| value["job"]["id"] == id)
        .unwrap_or_else(|| first.clone())
}

fn response(request: &Request, phase: usize) -> String {
    package_response(request, phase, PROMPT, selection(), "auth_test", true, profile_response)
}

pub(super) fn package_response(
    request: &Request, phase: usize, prompt: &str, selected: Value,
    operation: &str, native_entry: bool, continuation: fn(&Request, &str, &str) -> String,
) -> String {
    package_response_scoped(request, phase, prompt, selected, operation, native_entry, "", continuation)
}

// A new native dispatcher must issue its own real calls. Prefix only call IDs;
// never rewrite retained tool results or treat an earlier helper as current.
pub(super) fn package_response_scoped<F>(
    request: &Request, phase: usize, prompt: &str, selected: Value,
    operation: &str, native_entry: bool, prefix: &str, continuation: F,
) -> String
where F: FnOnce(&Request, &str, &str) -> String,
{
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/v1/chat/completions");
    assert_eq!(request.headers["authorization"], format!("Bearer {TOKEN}"));
    assert_eq!(request.body["model"], "cli-fixture-remote");
    let phase_limit = if prefix.is_empty() { 28 } else { 60 };
    assert!(phase < phase_limit,
        "bounded real package calls across two setup rounds; no preparation/install retry");
    let body = &request.body;
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.iter().filter(|m| m["role"] == "user" && m["content"] == prompt).count(), 1);
    for secret in [TOKEN, CODE, DEVICE, PAIR] { assert!(!body.to_string().contains(secret)); }
    let names = body["tools"].as_array().unwrap().iter()
        .filter_map(|tool| tool["function"]["name"].as_str()).collect::<Vec<_>>();
    let id = |suffix: &str| format!("{prefix}{suffix}");
    let call = |suffix: &str, name: &str, args: Value| {
        assert!(names.contains(&name), "required production tool {name} is absent: {names:?}");
        deferred_call_response(&id(suffix), name, args)
    };
    if optional_result(body, &id("activate-services")).is_none() {
        return call("activate-services", "toolkit_prepare_services", json!({}));
    }
    let activation = tool_result(body, &id("activate-services"));
    assert_eq!(activation["state"], "requested");
    assert_eq!(activation["changed"], false,
        "native setup already owns the adapter; service preparation must not perform a second handoff (call-ID prefix: {prefix:?})");
    if PACKAGE_TOOLS.iter().any(|name| !names.contains(name)) {
        let searches = messages.iter().filter(|m| m["role"] == "tool"
            && m["tool_call_id"].as_str().is_some_and(|value| value.starts_with(&id("profile-catalog-search-"))))
            .collect::<Vec<_>>();
        assert!(searches.len() < 10, "native catalog did not expose the supplied setup owner");
        let ready = searches.last().is_some_and(|message| {
            let found: Value = serde_json::from_str(message["content"].as_str().unwrap()).unwrap();
            assert_eq!(found["catalog_exact"], true);
            PACKAGE_TOOLS.iter().all(|name| found["results"].as_array().is_some_and(|rows|
                rows.iter().any(|row| row["name"] == *name
                    && matches!(row["visibility_status"].as_str(), Some("loaded" | "deferred")))))
                && !found["pending_sources"].as_array().is_some_and(|pending| !pending.is_empty())
        });
        return if ready {
            call(&format!("profile-catalog-load-{phase}"), "tool_catalog_load", json!({"names":PACKAGE_TOOLS}))
        } else {
            call(&format!("profile-catalog-search-{phase}"), "tool_catalog_search", json!({"query":"setup","limit":50}))
        };
    }
    if optional_result(body, &id("prepare-family")).is_none() {
        return call("prepare-family", "setup_prepare", json!({"family":selected["family"]}));
    }
    let prepared = tool_result(body, &id("prepare-family"));
    let prepare_id = prepared["job"]["id"].as_str().expect("actual preparation job");
    let prepared = latest_job(body, prepare_id, &prepared);
    match prepared["job"]["status"].as_str() {
        Some("running") => return call(&format!("prepare-job-{phase}"), "setup_job", json!({"job_id":prepare_id})),
        Some("succeeded") => {},
        other => panic!("real family preparation refused before profile review: {other:?}, {prepared}"),
    }
    if optional_result(body, &id("profile-help")).is_none() {
        return call("profile-help", "setup_help", json!({"family":selected["family"],"profile":selected["profile"]}));
    }
    let help = tool_result(body, &id("profile-help"));
    assert!(help["operations"].as_array().unwrap().iter().any(|row| row["id"] == operation));
    if optional_result(body, &id("checked-install")).is_none() {
        return call("checked-install", "setup_check", selected.clone());
    }
    let checked = tool_result(body, &id("checked-install"));
    assert_eq!(checked["scope"], "configuration_only", "real package check must succeed: {checked}");
    if optional_result(body, &id("install-once")).is_none() {
        let mut args = selected.clone();
        args["validation_id"] = checked["validation_id"].clone();
        args["configuration_sha256"] = checked["configuration_sha256"].clone();
        return call("install-once", "setup_install", args);
    }
    let installed = tool_result(body, &id("install-once"));
    let job_id = installed["job"]["id"].as_str().expect("actual approved installation job");
    let observed = latest_job(body, job_id, &installed);
    match observed["job"]["status"].as_str() {
        Some("running") => return call(&format!("install-job-{phase}"), "setup_job", json!({"job_id":job_id})),
        Some("succeeded") => {},
        other => panic!("real install refused before profile review: {other:?}, {observed}"),
    }
    // Always observe the exact successful job through the real readback route,
    // even if its initial response had already settled.
    if optional_result(body, &id("installed-job")).is_none() {
        return call("installed-job", "setup_job", json!({"job_id":job_id}));
    }
    if optional_result(body, &id("reconciled-install")).is_none() {
        return call("reconciled-install", "setup_reconcile", json!({"job_id":job_id}));
    }
    let reconciled = tool_result(body, &id("reconciled-install"));
    assert_eq!(reconciled["observation"]["job_id"], job_id);
    assert_eq!(reconciled["observation"]["status"], "matched", "actual pinned verifier must match: {reconciled}");
    if native_entry {
        assert!(reconciled["observation"]["result"]["native_connector"].is_object(),
            "the real installed package must supply its native connector entry");
    }
    // CheckedInstalls retains publisher raw names only after the check exists.
    if optional_result(body, &id("current-operation-names")).is_none() {
        return call("current-operation-names", "setup_help", json!({"family":selected["family"],"profile":selected["profile"]}));
    }
    let metadata = tool_result(body, &id("current-operation-names"));
    let raw = metadata["operations"].as_array().unwrap().iter()
        .find(|row| row["id"] == operation).unwrap()["tool"].as_str().expect("publisher-owned raw operation name");
    continuation(request, job_id, raw)
}

fn profile_response(request: &Request, job_id: &str, raw: &str) -> String {
    let body = &request.body;
    let names = body["tools"].as_array().unwrap().iter()
        .filter_map(|tool| tool["function"]["name"].as_str()).collect::<Vec<_>>();
    let call = |id: &str, name: &str, args: Value| {
        assert!(names.contains(&name), "required production tool {name} is absent: {names:?}");
        deferred_call_response(id, name, args)
    };
    if optional_result(body, PROFILE_CALL).is_none() {
        return call(PROFILE_CALL, "toolkit_connect_service_account", json!({
            "job_id":job_id,"connector":{"slot_id":"denied-slack","issuer":"https://issuer.invalid",
                "client":"profile-denial-client","resource":"https://slack.com","scopes":["test:read"],
                "strategy_id":"oidc-userinfo-v1","account_selection":null,"client_auth_profile":PROFILE},
            "operations":{raw:{"required_scopes":["test:read"],"destination_origin":"https://slack.com"}},
            "agent":"profile-denial-test","expires_in_ms":60000}));
    }
    assert_eq!(tool_result(body, PROFILE_CALL), json!({"state":"denied","phase":"client_profile","grant_issued":false}),
        "profile refusal must reach the actual native model call ID, not cancel its parent");
    let catalog = toolkit_package::bundled_recipe_catalog().unwrap();
    let recipe = catalog.entries().next().unwrap();
    if optional_result(body, "recipe-after-profile-denial").is_none() {
        return call("recipe-after-profile-denial", toolkit_package::READ_RECIPE,
            json!({"identity":recipe.package.identity,"digest":recipe.digest}));
    }
    assert_eq!(tool_result(body, "recipe-after-profile-denial"), serde_json::to_value(recipe).unwrap());
    let event = json!({"choices":[{"delta":{"content":DONE},"finish_reason":"stop"}]});
    format!("data: {event}\n\ndata: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":100,\"completion_tokens\":20,\"total_tokens\":120}}}}\n\ndata: [DONE]\n\n")
}

fn recorded_response(captured: &Mutex<Vec<Request>>, received: &Request) -> String {
    recorded_package_response(captured, received, response)
}

fn recorded_package_response(
    captured: &Mutex<Vec<Request>>, received: &Request, respond: fn(&Request, usize) -> String,
) -> String {
    let phase = {
        let mut requests = captured.lock().unwrap();
        let phase = requests.len();
        requests.push(Request {
            method: received.method.clone(), path: received.path.clone(),
            headers: received.headers.clone(), body: received.body.clone(),
        });
        phase
    };
    // Preserve the actual request even when its assertion fails. The record
    // mutex is released before response checks, so the evidence stays readable.
    respond(received, phase)
}

impl Server {
    pub(super) async fn with_profile_provider<T>(&mut self, wait: impl std::future::Future<Output = T>) -> T {
        tokio::select! { biased;
            result = self.task.as_mut().expect("live profile provider") => {
                drop(self.task.take());
                match result {
                    Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
                    Err(_) => panic!("profile provider task was cancelled during CLI wait"),
                    Ok(()) => panic!("profile provider stopped during CLI wait"),
                }
            }
            value = wait => value,
        }
    }

    async fn profile_denial() -> Self {
        Self::package_provider(response).await
    }

    pub(super) async fn package_provider(respond: fn(&Request, usize) -> String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::<Request>::new()));
        let captured = requests.clone();
        let (stop, mut stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            loop {
                let mut stream = tokio::select! { biased;
                    _ = &mut stopped => break,
                    accepted = listener.accept() => accepted.unwrap().0,
                };
                let received = timeout(BOUND, request(&mut stream)).await.unwrap();
                let body = recorded_package_response(&captured, &received, respond);
                let last = received.body["messages"].as_array().unwrap().iter().rev()
                    .find(|m| m["role"] == "tool");
                let running = last.and_then(|m| serde_json::from_str::<Value>(m["content"].as_str()?).ok())
                    .is_some_and(|value| value["job"]["status"] == "running");
                // Model latency lets the existing package job progress. The
                // next call still observes that exact job; it never resubmits.
                sleep(Duration::from_secs(if running { 5 } else { 1 })).await;
                let wire = format!("HTTP/1.1 200 Fixture\r\nConnection: close\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{body}", body.len());
                timeout(BOUND, async {
                    stream.write_all(wire.as_bytes()).await.unwrap();
                    stream.shutdown().await.unwrap();
                }).await.unwrap();
            }
        });
        Self { origin, requests, stop:Some(stop), task:Some(task) }
    }
}

pub(super) async fn run() {
    use meerkat_runtime::{LogicalRuntimeId, completion::CompletionOutcome,
        input_state::{InputLifecycleState, InputTerminalOutcome}};
    let prefix = PathBuf::from(std::env::var_os("CONNECT_REAL_LIBRARY_PREFIX")
        .expect("existing isolated supplied library and exact pin-built executable"));
    assert!(prefix.is_absolute());
    let mut provider = Server::profile_denial().await;
    let mut cli = Cli::spawn_supplied(None, &provider.origin, &prefix);
    let root = cli._root.as_ref().unwrap().path().to_owned();
    let storage = meerkat_connect::RealmOptions { context_root:root.join("outside-checkout"),
        state_root:Some(cli.state.clone()),user_config_root:Some(root.join("home")) }.resolve(REALM).unwrap();
    let realm_paths = meerkat_store::realm_paths_in(&storage.locator.state_root, REALM);
    let mut factory = meerkat::AgentFactory::new(realm_paths.root.clone())
        .context_root(storage.layout.invocation_context()).runtime_root(realm_paths.root)
        .builtins(false).shell(false);
    if let Some(home) = storage.layout.user_home_root() { factory = factory.user_config_root(home); }
    let persistence = factory.resolution_provider_auth_persistence().unwrap().unwrap();
    let secrets_before = timeout(BOUND, persistence.token_store().list()).await.unwrap().unwrap();
    assert!(secrets_before.is_empty());
    let config = fs::read(cli.state.join(REALM).join("config.toml")).unwrap();
    provider.with_profile_provider(cli.shown("New realm: ",0)).await; cli.send(&format!("{REALM}\n"));
    provider.with_profile_provider(cli.approve_setup_helper()).await;
    provider.with_profile_provider(cli.enter_conversation()).await;
    assert!(provider.requests.lock().unwrap().is_empty());
    let conversation_start = cli.transcript.len();
    cli.send(&format!("{PROMPT}\n"));
    provider.with_profile_provider(cli.shown("Type 'approve ",conversation_start)).await;
    assert!(cli.transcript[conversation_start..].contains("service_software_install"), "real checked software review required: {}",cli.transcript);
    let id=cli.transcript[conversation_start..].split("Type 'approve ").last().unwrap().split('\'').next().unwrap().to_owned();
    let software_review_end=cli.transcript.len(); cli.send(&format!("approve {id}\n"));
    provider.with_profile_provider(cli.shown("Client authentication",software_review_end)).await; cli.send("public\n");
    provider.with_profile_provider(cli.shown("Advanced settings?",software_review_end)).await; cli.send("\n");
    provider.with_profile_provider(cli.shown("Type 'approve ",software_review_end)).await;
    assert!(cli.transcript[software_review_end..].contains("connector_client_configuration"));
    let id=cli.transcript.split("Type 'approve ").last().unwrap().split('\'').next().unwrap().to_owned();
    cli.send(&format!("deny {id}\n"));
    provider.with_profile_provider(cli.shown(DONE,software_review_end)).await; provider.with_profile_provider(cli.shown("You: ",software_review_end)).await;
    cli.send("/quit\n"); assert!(provider.with_profile_provider(cli.finish()).await.success(),"{}",cli.transcript); provider.finish().await;
    assert_eq!(cli.transcript.matches("Setup session: ").count(),1);
    assert!(!cli.transcript.contains("The native setup turn failed."));
    for forbidden in ["Open this private sign-in URL", "Connector client secret (hidden)", "Sign in for this installed integration", "Review installed account access"] {
        assert!(!cli.transcript.contains(forbidden),"unexpected authentication path: {forbidden}");
    }
    cli.assert_reviewed_setup(&config);
    assert_eq!(timeout(BOUND,persistence.token_store().list()).await.unwrap().unwrap(),secrets_before);
    let requests=provider.requests.lock().unwrap(); let last=&requests.last().unwrap().body;
    let calls=last["messages"].as_array().unwrap().iter().filter(|m|m["role"]=="assistant")
        .flat_map(|m|m["tool_calls"].as_array().into_iter().flatten()).collect::<Vec<_>>();
    for name in ["toolkit_prepare_services","setup_prepare","setup_check","setup_install","setup_reconcile","toolkit_connect_service_account",toolkit_package::READ_RECIPE] {
        assert_eq!(calls.iter().filter(|c|c["function"]["name"]==name).count(),1,"exactly one {name}");
    }
    assert_eq!(tool_result(last,PROFILE_CALL),json!({"state":"denied","phase":"client_profile","grant_issued":false}));
    let session_id=meerkat_core::SessionId::parse(cli.transcript.split("Setup session: ").nth(1).unwrap().split_whitespace().next().unwrap()).unwrap();
    drop(requests);
    let (_,persistence)=timeout(BOUND,meerkat::storage_provider::open_realm_persistence_with_layout(
        storage.layout,REALM,None,Some(meerkat_store::RealmOrigin::Explicit))).await.unwrap().unwrap();
    let session=timeout(BOUND,persistence.session_store().load(&session_id)).await.unwrap().unwrap().unwrap();
    let history=serde_json::to_string(session.messages()).unwrap();
    assert!(history.contains(PROMPT) && history.contains(DONE) && history.contains(PROFILE_CALL));
    let inputs=timeout(BOUND,persistence.runtime_store().load_input_states_strict(&LogicalRuntimeId::for_session(&session_id))).await.unwrap().unwrap();
    assert_eq!(inputs.len(),1,"protected answers must not create another native input");
    let input=&inputs[0]; assert_eq!(input.seed.phase,InputLifecycleState::Consumed);
    assert_eq!(input.seed.terminal_outcome,Some(InputTerminalOutcome::Consumed));
    assert_eq!(input.seed.attempt_count,1); assert_eq!(input.state.recovery_count,0);
    let stored=serde_json::to_value(input).unwrap();
    let outcome:CompletionOutcome=serde_json::from_value(stored["terminal_completion"]["outcome"].clone()).unwrap();
    assert!(matches!(outcome,CompletionOutcome::Completed(_)));
}

#[test]
fn profile_provider_assertion_retains_request_without_poisoning_capture() {
    let captured = Mutex::new(Vec::new());
    let received = Request {
        method: "GET".into(), path: "/fixture-invalid-method".into(),
        headers: BTreeMap::from([("fixture".into(), "recorded".into())]),
        body: json!({"fixture":"must survive the response assertion"}),
    };
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(||
        recorded_response(&captured, &received)));
    assert!(failure.is_err());
    let requests = captured.lock().expect("assertion must not poison request capture");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, received.method);
    assert_eq!(requests[0].path, received.path);
    assert_eq!(requests[0].headers, received.headers);
    assert_eq!(requests[0].body, received.body);
}

#[tokio::test]
async fn profile_provider_failure_interrupts_wait_with_original_panic_and_unwind_cleanup() {
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Cleanup(Arc<AtomicBool>);
    impl Drop for Cleanup {
        fn drop(&mut self) { self.0.store(true, Ordering::SeqCst); }
    }
    let cleaned = Arc::new(AtomicBool::new(false));
    let observed = cleaned.clone();
    let waiter = tokio::spawn(async move {
        let _cleanup = Cleanup(observed);
        let mut provider = Server {
            origin: String::new(), requests: Arc::new(Mutex::new(Vec::new())), stop: None,
            task: Some(tokio::spawn(async { panic!("profile provider fixture failure"); })),
        };
        provider.with_profile_provider(std::future::pending::<()>()).await;
    });
    let error = timeout(BOUND, waiter).await.expect("failed provider must interrupt pending wait")
        .unwrap_err();
    assert!(error.is_panic());
    let payload = error.into_panic();
    assert_eq!(payload.downcast_ref::<&str>().copied(), Some("profile provider fixture failure"));
    assert!(cleaned.load(Ordering::SeqCst));
}

#[tokio::test]
async fn profile_provider_successful_wait_preserves_normal_stop_and_join() {
    let (stop, stopped) = oneshot::channel();
    let mut provider = Server {
        origin: String::new(), requests: Arc::new(Mutex::new(Vec::new())), stop: Some(stop),
        task: Some(tokio::spawn(async { stopped.await.unwrap(); })),
    };
    assert_eq!(provider.with_profile_provider(async { 7 }).await, 7);
    assert!(!provider.task.as_ref().unwrap().is_finished());
    provider.finish().await;
    assert!(provider.task.is_none() && provider.stop.is_none());
}

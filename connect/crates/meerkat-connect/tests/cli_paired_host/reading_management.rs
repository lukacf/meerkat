//! Two actual CLI processes manage one persistent native Reading application.
//! The control pipe observes/release barriers; only the protected CLI tools mutate it.
use super::*;
use super::profile_denial::optional_result;

const SETUP: &str = "Enable my Reading application's private text DM for my Telegram human ID 123456789. I acknowledge exclusive polling. Use the application's advertised owner and route, and prepare it for my private approval.";
const PAUSE: &str = "Pause my Reading frontend using its advertised management action. Retain queued input and drain already accepted work according to its displayed policy.";
const RESUME: &str = "Resume my Reading frontend using its advertised management action and retained queued input.";
const DISABLE: &str = "Disable my Reading frontend using its advertised action. Stop new input and replies, drain entered work, and retain honest native close status.";
const SETTLE: &str = "Read my disabled Reading frontend's status until its native component close is settled. Do not prepare, approve or submit another change.";
const REMOVE: &str = "Remove only my Reading frontend configuration using its advertised action after native close has settled. Preserve unrelated agents and credentials.";
const INSPECT: &str = "Inspect my removed Reading frontend configuration without making a change.";
const INSPECT_AGAIN: &str = "Inspect the removed Reading frontend once more without making a change.";
const PROMPTS: [&str; 8] = [SETUP, PAUSE, RESUME, DISABLE, SETTLE, REMOVE, INSPECT, INSPECT_AGAIN];
const ANSWERS: [&str; 8] = [
    "Reading configuration committed at revision 1. Activation and delivery remain separately observed.",
    "Reading is paused at revision 2 under its advertised pending-work policy.",
    "Reading is resumed at revision 3 under its advertised pending-work policy.",
    "Reading disable is committed at revision 4. Native close settlement remains separately observed.",
    "Reading's native component close is observed settled. No second disable was submitted.",
    "Only Reading frontend configuration was removed at revision 5 after native close settled.",
    "Reading configuration remains removed, with its native close receipt retained.",
    "The second observation still reports removed configuration and retained native close.",
];

fn pending_policy() -> Value {
    json!({"paused_input":"retain","paused_observation":"continue","accepted_work":"drain",
        "entered_admission":"join-with-uncertainty","paused_replies":"drain",
        "disabled_pending":"stop","entered_provider":"join-with-uncertainty"})
}

fn answer(text: &str) -> String {
    format!("data: {}\n\ndata: [DONE]\n\n",
        json!({"choices":[{"delta":{"content":text},"finish_reason":"stop"}]}))
}

fn response(received: &Request) -> String {
    assert_eq!(received.method, "POST");
    assert_eq!(received.path, "/v1/chat/completions");
    assert_eq!(received.headers["authorization"], format!("Bearer {TOKEN}"));
    assert_eq!(received.body["model"], "cli-fixture-remote");
    assert!(!received.headers.contains_key("cookie"));
    let body = &received.body;
    let messages = body["messages"].as_array().unwrap();
    // Native system notices remain intact. Only actual ordinary business inputs
    // choose a management action; notice text never supplies an action or grant.
    let users: Vec<_> = messages.iter().filter(|m| m["role"] == "user")
        .filter_map(|m| m["content"].as_str()).filter(|s| PROMPTS.contains(s)).collect();
    let current = *users.last().expect("actual Reading user instruction is missing");
    let phase = PROMPTS.iter().position(|p| *p == current).unwrap();
    assert!(users == if phase == 0 { vec![SETUP] } else { PROMPTS[1..=phase].to_vec() },
        "Reading business instructions were duplicated, replayed or reordered");
    let names: Vec<_> = body["tools"].as_array().unwrap().iter()
        .filter_map(|t| t["function"]["name"].as_str()).collect();
    // Ordinary NativeSetup keeps the unselected, protected PairedHost chooser.
    // Its offer does not pair a host or authorize a PairedHost change.
    assert!(!names.iter().any(|n| (n.starts_with("paired_host_") && *n != "paired_host_select_host")
        || *n == "setup_install"),
        "standalone frontend management unexpectedly exposes another setup route");
    let id = |suffix: &str| format!("reading-{phase}-{suffix}");
    let call = |suffix: &str, name: &str, args: Value| {
        assert!(names.contains(&name), "required Reading management tool is not offered");
        deferred_call_response(&id(suffix), name, args)
    };
    let host_id = format!("reading-{}-host", if phase == 0 { 0 } else { 1 });
    if optional_result(body, &host_id).is_none() {
        assert!(phase <= 1, "management lost its actual selected host");
        return call("host", "forge_select_host", json!({}));
    }
    let selected = tool_result(body, &host_id);
    assert_eq!(selected["frontend_management"], "declared");
    let inventories: Vec<_> = messages.iter().filter(|m| m["role"] == "tool")
        .filter(|m| m["tool_call_id"].as_str().is_some_and(|v| v.starts_with(&id("inventory-"))))
        .collect();
    assert!(inventories.len() <= 24, "bounded status observation did not settle");
    if inventories.is_empty() {
        return call("inventory-0", "forge_frontend_inspect", json!({}));
    }
    let inventory: Value = serde_json::from_str(inventories.last().unwrap()["content"].as_str().unwrap()).unwrap();
    let observation: Value = serde_json::from_str(inventory["observation_json"].as_str()
        .expect("generic application inventory must retain its exact JSON projection")).unwrap();
    let source = &observation;
    assert_eq!(source["storage"], "available");
    if phase == 4 {
        assert_eq!(source["management"]["state"], "disabled");
        assert_eq!(source["revision"], 4);
        if source["native_close"]["state"] != "settled" {
            assert!(inventories.len() < 24, "approved disable did not reach native settlement");
            return call(&format!("inventory-{}", inventories.len()), "forge_frontend_inspect", json!({}));
        }
        assert!(source["native_close"]["observation"]["failure"].is_null());
        return answer(ANSWERS[phase]);
    }
    if phase >= 6 {
        assert_eq!(source["revision"], 5);
        assert_eq!(source["management"]["state"], "removed");
        assert!(source["configuration"].is_null());
        assert_eq!(source["native_close"]["state"], "settled");
        return answer(ANSWERS[phase]);
    }
    assert_eq!(inventories.len(), 1, "a mutation must use one actual reviewed inventory");
    let expected_revision = [0, 1, 2, 3, 0, 4][phase];
    assert_eq!(source["revision"], expected_revision);
    let prepared_args = if phase == 0 {
        assert_eq!(source["participants"].as_array().unwrap().len(), 1);
        assert_eq!(source["routes"].as_array().unwrap().len(), 1);
        assert_eq!(source["routes"][0]["participant_id"], source["participants"][0]["id"]);
        assert_eq!(source["routes"][0]["destination_rule"], "same-authorized-human-private-dm");
        json!({"configuration":{"component_id":source["component"]["id"],
            "profile":source["component"]["profile"],"participant_id":source["participants"][0]["id"],
            "route_id":source["routes"][0]["id"],"telegram_user_id":123456789,
            "telegram_dm_chat_id":123456789,"native_identity":source["routes"][0]["native_identity"],
            "bot_token":source["component"]["private_reference"],"exclusive_polling_acknowledged":true},
            "expected_revision":source["revision"]})
    } else {
        let action = match phase { 1 => "pause", 2 => "resume", 3 => "disable", 5 => "remove", _ => unreachable!() };
        let offered: Vec<_> = source["management_actions"].as_array().unwrap().iter()
            .filter(|row| row["action"] == action && row["available"] == true).collect();
        assert_eq!(offered.len(), 1, "requested action is not uniquely offered by this application");
        assert_eq!(offered[0]["pending_work_policy"], pending_policy());
        // Exact application-owned envelope, not syntax injected by the fixture pipe.
        offered[0]["prepare"].clone()
    };
    if optional_result(body, &id("prepare")).is_none() {
        return call("prepare", "forge_frontend_prepare", prepared_args);
    }
    let prepared = tool_result(body, &id("prepare"));
    assert_eq!(prepared["approval_required"], true);
    let proposal: Value = serde_json::from_str(prepared["proposal_json"].as_str()
        .expect("generic application proposal must retain its exact JSON projection")).unwrap();
    assert!(proposal["configuration"] == prepared_args["configuration"],
        "actual Reading proposal changed the observed requested configuration");
    assert_eq!(proposal["expected_revision"], prepared_args["expected_revision"]);
    assert_eq!(proposal["pending_work_policy"], pending_policy());
    if optional_result(body, &id("submit")).is_none() {
        return call("submit", "forge_frontend_submit", json!({"change_id":proposal["change_id"]}));
    }
    let returned = tool_result(body, &id("submit"));
    assert!(returned["submission_outcome"] == json!({"change_id": proposal["change_id"], "disposition": "known"}),
        "actual Reading submit lacks its exact negotiated known command disposition");
    assert_eq!(returned["recovery_only"], false);
    let result: Value = serde_json::from_str(returned["observation_json"].as_str()
        .expect("generic application submission must retain its exact JSON projection")).unwrap();
    assert_eq!(result["status"], "committed");
    assert_eq!(result["revision"], expected_revision + 1);
    assert_eq!(result["pending_work_policy"], pending_policy());
    assert_eq!(result["management_state"],
        match phase { 0 | 2 => "enabled", 1 => "paused", 3 => "disabled", 5 => "removed", _ => unreachable!() });
    answer(ANSWERS[phase])
}

async fn start_provider(evidence: &Path, private_origin: String) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (stop, mut stopped) = oneshot::channel();
    let requests = Arc::new(Mutex::new(Vec::<Request>::new()));
    let observed = requests.clone();
    let path = evidence.join("cli-model-requests.json");
    let task = tokio::spawn(async move {
        loop {
            let mut stream = tokio::select! {biased; _=&mut stopped=>break, accepted=listener.accept()=>accepted.unwrap().0};
            let received = timeout(BOUND, request(&mut stream)).await.unwrap();
            for secret in [forge_reading_owner::NATIVE_PASSWORD, forge_reading_owner::NATIVE_TOKEN,
                           private_origin.as_str(), TOKEN, DEVICE, PAIR, CODE] {
                assert!(!received.body.to_string().contains(secret), "protected value entered CLI model context");
            }
            // Retain actual requests before the oracle, including any failed request.
            let should_wait = received.body["messages"].as_array().unwrap().iter()
                .rev().find(|m| m["role"] == "user" && m["content"].as_str().is_some_and(|s| PROMPTS.contains(&s)))
                .is_some_and(|m| m["content"] == SETTLE);
            let body = {
                let mut rows = observed.lock().unwrap();
                assert!(rows.len() < 80, "bounded actual CLI model requests");
                rows.push(received);
                let wire: Vec<_> = rows.iter().map(|row| json!({"method":row.method,"path":row.path,"body":row.body})).collect();
                fs::write(&path, serde_json::to_vec_pretty(&wire).unwrap()).unwrap();
                response(rows.last().unwrap())
            };
            if should_wait { sleep(Duration::from_secs(1)).await; }
            let reply = format!("HTTP/1.1 200 Fixture\r\nConnection: close\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{body}", body.len());
            timeout(BOUND, async {
                stream.write_all(reply.as_bytes()).await.unwrap();
                stream.shutdown().await.unwrap();
            }).await.unwrap();
        }
    });
    Server { origin, requests, stop: Some(stop), task: Some(task) }
}

async fn start_cli(provider: &Server, prefix: &Path) -> Cli {
    // None selects standalone Forge. Some(origin) would select PairedHost instead.
    let mut cli = Cli::spawn_supplied(None, &provider.origin, prefix);
    cli.shown("New realm: ", 0).await;
    cli.send(&format!("{REALM}\n"));
    cli.approve_setup_helper().await;
    cli.enter_conversation().await;
    cli
}

async fn interaction(cli: &mut Cli, phase: usize, origin: Option<&str>) {
    let from = cli.transcript.len();
    cli.send(&format!("{}\n", PROMPTS[phase]));
    if let Some(origin) = origin {
        cli.shown("Forge Console origin", from).await;
        cli.send(&format!("{origin}\n"));
        cli.shown("Forge Console password (hidden)", from).await;
        assert!(!tcgetattr(&cli.observer).unwrap().local_flags.intersects(LocalFlags::ECHO | LocalFlags::ICANON));
        cli.send(&format!("{}\n", forge_reading_owner::NATIVE_PASSWORD));
    }
    if [0, 1, 2, 3, 5].contains(&phase) {
        cli.shown("Type 'approve ", from).await;
        let approval = cli.transcript[from..].split("Type 'approve ").nth(1).unwrap()
            .split('\'').next().unwrap().to_owned();
        assert!(cli.transcript[from..].contains("pending_work_policy"), "complete app policy was not displayed");
        cli.send("approve wrong-id\n");
        cli.shown("Use the exact approval ID", from).await;
        cli.send(&format!("approve {approval}\n"));
    }
    cli.shown(ANSWERS[phase], from).await;
    let end = cli.transcript[from..].find(ANSWERS[phase]).unwrap() + from;
    cli.shown("You: ", end).await;
    for secret in [forge_reading_owner::NATIVE_PASSWORD, forge_reading_owner::NATIVE_TOKEN] {
        assert!(!cli.transcript.contains(secret), "private Reading value appeared in CLI output");
    }
}

#[tokio::test]
#[ignore = "requires exact CONNECT_READING_NATIVE_INPUT, its SHA256, and emitted CONNECT_LAZY_SERVICES_FIXTURE_PREFIX"]
async fn actual_cli_manages_the_same_native_reading_owner() {
    let prefix = PathBuf::from(std::env::var("CONNECT_LAZY_SERVICES_FIXTURE_PREFIX").unwrap());
    let mut owner = forge_reading_owner::NativeOwner::start().await;
    let mut provider = start_provider(&owner.root, owner.origin.clone()).await;
    let mut setup = start_cli(&provider, &prefix).await;
    interaction(&mut setup, 0, Some(&owner.origin)).await;
    let setup_pid = setup.child.id();
    setup.send("/quit\n");
    assert!(setup.finish().await.success(), "initial CLI did not exit normally");
    fs::write(owner.root.join("setup-cli.txt"), &setup.transcript).unwrap();
    owner.release("setup-client-exited", Some((setup_pid, 0)));

    owner.phase("pause").await; // Actual input 101 completed and replied after setup exit.
    let mut management = start_cli(&provider, &prefix).await;
    interaction(&mut management, 1, Some(&owner.origin)).await;
    owner.release("pause", None);
    for (phase, barrier) in [(2, "resume"), (3, "disable"), (4, "observe-settled"),
                             (5, "remove"), (6, "inspect"), (7, "inspect")] {
        owner.phase(barrier).await;
        interaction(&mut management, phase, None).await;
        owner.release(barrier, None);
    }
    owner.phase("management-client-exited").await;
    let management_pid = management.child.id();
    management.send("/quit\n");
    assert!(management.finish().await.success(), "management CLI did not exit normally");
    fs::write(owner.root.join("management-cli.txt"), &management.transcript).unwrap();
    owner.release("management-client-exited", Some((management_pid, 0)));
    let result = owner.finish().await;
    provider.finish().await;
    let actual = result["management_route_observations"].as_array().unwrap();
    let submits: Vec<_> = actual.iter().filter(|row| row["operation"] == "submit").collect();
    assert_eq!(submits.len(), 5);
    for (row, action) in submits.iter().zip([None, Some("pause"), Some("resume"), Some("disable"), Some("remove")]) {
        assert_eq!(row["approved"], true);
        assert_eq!(row["configuration"]["action"].as_str(), action);
        assert_eq!(row["result"]["status"], "committed");
    }
    assert_eq!(result["admissions"].as_array().unwrap().len(), 3);
    assert!(result["management"]["unrelated_identity_before"] == result["management"]["unrelated_identity_after"],
        "Reading management changed the unrelated native identity");
    assert_eq!(result["management"]["shared_private_field_preserved"], true);
    let service = &result["service_role"];
    assert_eq!(service["selection"]["name"], "collection_snapshot");
    assert_eq!(service["arguments"], json!({}));
    assert_eq!(service["application_service_calls"], 2);
    assert_eq!(service["frontend_state"], "removed");
    for retained in ["same_instance", "same_store", "same_database", "same_export"] {
        assert_eq!(service[retained], true, "Reading service ownership changed");
    }
    assert_eq!(service["before_frontend_setup"]["books"].as_array().unwrap().len(), 1);
    assert_eq!(service["after_frontend_removal"], service["before_frontend_setup"]);
    // The ordinary exported service handler reads its actual retained Store.
    // This proves application service-role survival, not native tool dispatch.
    // Actual ticket, same-DM provider receipts, stopped work and close receipt
    // are validated by the one persistent owner, not manufactured by this pipe.
    fs::write(owner.root.join("cli-management-result.json"), serde_json::to_vec_pretty(&json!({
        "status":"passed","scope":"actual CLI management with controlled native/provider Reading owner",
        "setup_cli_pid":setup_pid,"management_cli_pid":management_pid,
        "same_owner_survived":true,"approved_submissions":5,
        "service_role":"same application export and Store callable after frontend removal; not native tool dispatch",
        "reading_result":"native/result.json","model_requests":"cli-model-requests.json"})).unwrap()).unwrap();
}

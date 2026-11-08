//! Actual private Services route using the existing supplied CLI and model peer.
//! Smoke proves packaged preparation/help and close. The separate no-auth case
//! installs Hacker News, finalizes native access and reads it in ordinary rkat.
use super::*;
use super::{profile_denial::private_package_response, public_service as public};
use std::os::unix::fs::OpenOptionsExt;

const SMOKE: &str = "Prepare the supplied Agno library and describe the Hacker News operation. Do not install or request access.";
pub(super) const INSTALL: &str = "Install the supplied Agno Hacker News integration for get_top_hackernews_stories, ask for public service access for rkat-agent until revoked, and finish without calling the service.";
const SMOKE_DONE: &str = "The supplied helper prepared Agno and returned Hacker News help. Nothing was installed or granted.";

pub(super) fn selected() -> Value {
    json!({"family":"agno","profile":"hackernews","operations":[public::OPERATION],"values":{"timeout":7}})
}
fn response(request: &Request, phase: usize) -> String {
    if request.body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| message["role"] == "user" && message["content"] == public::RUN_PROMPT)
    {
        return public::response(request, phase);
    }
    let tools = request.body["tools"].as_array().unwrap();
    for name in [
        "toolkit_connect_service_account",
        "toolkit_enable_secret_service",
        "forge_select_host",
    ] {
        assert!(
            !tools.iter().any(|tool| tool["function"]["name"] == name),
            "unsupplied owner {name}"
        );
    }
    let smoke = request.body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| message["role"] == "user" && message["content"] == SMOKE);
    private_package_response(
        request,
        phase,
        if smoke { SMOKE } else { INSTALL },
        selected(),
        public::OPERATION,
        !smoke,
        |request, job, operation| {
            if smoke {
                public::completed(SMOKE_DONE)
            } else {
                public::public_access(request, job, operation)
            }
        },
    )
}

async fn initialize(cli: &mut StdioCli, provider: &mut Server, adapter: &str) -> (String, String) {
    let Some(pin) = option_env!("MEERKAT_CONNECT_RESOURCES_SHA256") else {
        panic!("resource-pinned test build");
    };
    cli.request("h:init", "connect/initialize", json!({"protocol_version":3,"adapter":adapter,
        "application":{"application_id":"private-service-acceptance","instance_id":"one-owned-process",
            "origin":"http://127.0.0.1:12345"},
        "viewer":{"principal_id":"fixture-human","login_scope_id":"fixture-login",
            "expires_at_ms":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64 + 900_000},
        "realm":{"realm_id":REALM,"state_root":cli.state,"context_root":cli.context,"user_config_root":cli.home},
        "model":"cli-fixture","services":{"resource_manifest_sha256":pin,
            "preparation":"permitted"}}));
    let initialized = provider.with_profile_provider(cli.response("h:init")).await;
    assert!(
        initialized.get("error").is_none(),
        "private initialization: {initialized}"
    );
    assert_eq!(
        initialized["result"]["capabilities"]["service_preparation"],
        true
    );
    let tools = initialized["result"]["capabilities"]["tools"]
        .as_array()
        .unwrap();
    for name in ["toolkit_prepare_services", "toolkit_enable_public_service"] {
        assert_eq!(tools.iter().filter(|value| **value == name).count(), 1);
    }
    assert!(
        provider.requests.lock().unwrap().is_empty(),
        "initialization is not a model input"
    );
    (
        initialized["result"]["scope"].as_str().unwrap().into(),
        initialized["result"]["session_id"].as_str().unwrap().into(),
    )
}

pub(super) async fn close(cli: &mut StdioCli, provider: &mut Server, scope: &str) {
    cli.request(
        "h:retire",
        "connect/retire",
        json!({"target":{"scope":scope},"retirement_id":"logout","reason":"logout"}),
    );
    let retired = provider
        .with_profile_provider(cli.response("h:retire"))
        .await;
    assert_eq!(retired["result"]["entered_work"], "settled", "{retired}");
    cli.request(
        "h:late",
        "connect/send",
        json!({"scope":scope,"text":"must not enter"}),
    );
    assert_eq!(
        provider.with_profile_provider(cli.response("h:late")).await["error"]["data"]["code"],
        "binding_retired"
    );
    cli.request(
        "h:close",
        "connect/close",
        json!({"target":{"scope":scope}}),
    );
    let closed = provider
        .with_profile_provider(cli.response("h:close"))
        .await;
    assert_eq!(closed["result"]["resources_closed"], true, "{closed}");
    assert!(
        provider.with_profile_provider(cli.finish()).await.success(),
        "same native/guard cleanup must succeed"
    );
    let retire_index = cli
        .rows
        .iter()
        .position(|row| row["id"] == "h:retire")
        .unwrap();
    let close_index = cli
        .rows
        .iter()
        .position(|row| row["id"] == "h:close")
        .unwrap();
    assert!(
        retire_index < close_index,
        "stdin remained open through both joined acknowledgements"
    );
}

pub(super) fn preparation(cli: &StdioCli) -> Value {
    let artifacts = preparation_artifacts(&cli.state);
    assert_eq!(artifacts.len(), 1, "exactly one real preparation, no retry");
    let (bytes, handoff) = read_bounded_json(&artifacts[0].join("prepare.stdout"), 256 * 1024);
    let (_, selected) =
        read_bounded_json(&artifacts[0].join("prepared/product-mcp.json"), 256 * 1024);
    assert_eq!(
        handoff, selected,
        "actual prepared command handed to native"
    );
    let session = artifacts[0].join("prepared/local-session/session.json");
    let (_, owned) = read_bounded_json(&session, 256 * 1024);
    json!({"handoff_sha256":format!("{:x}",Sha256::digest(bytes)),
        "local_session_sha256":file_digest(&session),"local_session_binding":owned["binding"],
        "helper_exit_evidence":"native_normal_close_and_same_attempt_guard_join"})
}

pub(super) fn effect_counts(body: &Value, installed: bool, access_returned: bool) {
    let calls: Vec<_> = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|message| message["tool_calls"].as_array().into_iter().flatten())
        .collect();
    for name in ["toolkit_prepare_services", "setup_prepare"] {
        assert_eq!(
            calls
                .iter()
                .filter(|call| call["function"]["name"] == name)
                .count(),
            1,
            "one {name}"
        );
    }
    for name in ["setup_check", "setup_install"] {
        assert_eq!(
            calls
                .iter()
                .filter(|call| call["function"]["name"] == name)
                .count(),
            usize::from(installed),
            "{name}"
        );
    }
    assert_eq!(
        calls
            .iter()
            .filter(|call| call["function"]["name"] == "toolkit_enable_public_service")
            .count(),
        usize::from(access_returned)
    );
    assert!(
        !calls
            .iter()
            .any(|call| call["function"]["name"] == public::OPERATION),
        "setup cannot claim actual service use"
    );
}

#[tokio::test]
#[ignore = "supplied exact pin-built CLI/library; actual packaged preparation, no accounts"]
async fn actual_stdio_services_supplied_library_prepare_help_retire() {
    for adapter in ["services", "forge_services"] {
        let mut provider = Server::package_provider(response).await;
        let mut cli = StdioCli::start_supplied(&provider.origin);
        let root = cli.root.path().to_owned();
        let config = fs::read(cli.state.join(REALM).join("config.toml")).unwrap();
        let (scope, session) = initialize(&mut cli, &mut provider, adapter).await;
        cli.request(
            "h:work",
            "connect/send",
            json!({"scope":scope,"text":SMOKE}),
        );
        let completed = provider.with_profile_provider(cli.response("h:work")).await;
        assert_eq!(
            completed["result"]["completion"], "completed",
            "{completed}"
        );
        assert!(!cli.rows.iter().any(|row| row["method"] == "connect/prompt"));
        let helpers = public::observed_descendants(cli.child.as_ref().unwrap().id()).await;
        close(&mut cli, &mut provider, &scope).await;
        public::assert_observed_helpers_gone(&helpers).await;
        provider.finish().await;
        assert_eq!(
            fs::read(cli.state.join(REALM).join("config.toml")).unwrap(),
            config
        );
        let requests = provider.requests.lock().unwrap();
        effect_counts(&requests.last().unwrap().body, false, false);
        assert!(
            !cli.rows
                .iter()
                .any(|row| row["params"]["kind"] == "service_finalization")
        );
        let observation = preparation(&cli);
        let artifacts = preparation_artifacts(&cli.state);
        let (_, empty) = read_bounded_json(
            &artifacts[0].join("prepared/local-session/empty-close.json"),
            256 * 1024,
        );
        assert_eq!(
            empty["round"]["checkpoints"],
            json!([]),
            "corroborating ordinary Controller close only"
        );
        drop(requests);
        let cli_sha = file_digest(&cli.binary);
        drop(cli);
        assert!(
            !root.exists(),
            "isolated smoke realm removed after joined owners"
        );
        println!(
            "{}",
            json!({"case":"private_services_route_smoke","adapter":adapter,"session":session,
            "cli_sha256":cli_sha,"preparation":observation,"temporary_realm_removed":true,
            "installation":false,"service_use":false,"interrupted_effect_coverage":false})
        );
    }
}

#[tokio::test]
#[ignore = "supplied exact CLI/library and rkat; installs temp realm and reads public Hacker News"]
async fn actual_stdio_services_public_install_close_and_fresh_rkat() {
    let binary =
        PathBuf::from(std::env::var_os("CONNECT_FINAL_RKAT").expect("selected coherent rkat"));
    let expected = std::env::var("CONNECT_FINAL_RKAT_SHA256").expect("selected rkat hash");
    assert_eq!(expected.len(), 64);
    assert!(
        expected
            .bytes()
            .all(|value| value.is_ascii_digit() || (b'a'..=b'f').contains(&value))
    );
    let mut provider = Server::package_provider(response).await;
    let mut cli = StdioCli::start_supplied(&provider.origin);
    let root = cli.root.path().to_owned();
    let (scope, setup_session) = initialize(&mut cli, &mut provider, "services").await;
    cli.request(
        "h:work",
        "connect/send",
        json!({"scope":scope,"text":INSTALL}),
    );
    let software = provider.with_profile_provider(cli.prompt("approval")).await;
    assert_eq!(
        software["params"]["prompt"]["body"]["kind"],
        "service_software_install"
    );
    assert_eq!(
        software["params"]["prompt"]["body"]["selection"],
        selected()
    );
    cli.answer(
        software["id"].as_str().unwrap(),
        json!({"kind":"approval","decision":"approve"}),
    );
    let access = provider.with_profile_provider(cli.prompt("approval")).await;
    assert_eq!(
        access["params"]["prompt"]["body"]["phase"],
        "public_service_access"
    );
    assert_eq!(
        access["params"]["prompt"]["body"]["authentication"],
        json!({"kind":"none"})
    );
    assert_eq!(access["params"]["prompt"]["body"]["realm"], REALM);
    assert_eq!(
        access["params"]["prompt"]["body"]["grant"]["operations"],
        json!([public::OPERATION])
    );
    assert_eq!(
        access["params"]["prompt"]["body"]["grant"]["agent"],
        "rkat-agent"
    );
    assert_eq!(
        access["params"]["prompt"]["body"]["grant"]["expires_in_ms"],
        Value::Null
    );
    let helpers = public::observed_descendants(cli.child.as_ref().unwrap().id()).await;
    cli.answer(
        access["id"].as_str().unwrap(),
        json!({"kind":"approval","decision":"approve"}),
    );
    let completed = provider.with_profile_provider(cli.response("h:work")).await;
    assert_eq!(
        completed["result"]["completion"], "completed",
        "{completed}"
    );
    assert_eq!(completed["result"]["state"]["can_send"], false);
    let observation = loop {
        if let Some(row) = cli.rows.iter().find(|row| {
            row["params"]["kind"] == "service_finalization" && row["params"]["state"] == "observed"
        }) {
            break row["params"]["observation"].clone();
        }
        let row = provider.with_profile_provider(cli.next()).await;
        if row["params"]["kind"] == "service_finalization" {
            assert!(
                matches!(
                    row["params"]["state"].as_str(),
                    Some("native_closed" | "observed")
                ),
                "{row}"
            );
        }
    };
    assert_eq!(observation["facts"]["persistence"], "saved");
    assert_eq!(observation["facts"]["grant"]["state"], "selected");
    assert_eq!(observation["service_readiness"], "not_observed");
    assert_eq!(observation["automatic_reentry_blocked"], false);
    let setup_body = provider
        .requests
        .lock()
        .unwrap()
        .last()
        .unwrap()
        .body
        .clone();
    effect_counts(&setup_body, true, true);
    let descriptor = public::reviewed_descriptor(&setup_body, "", "hackernews", public::OPERATION);
    assert_eq!(observation["server"], descriptor.name);
    assert_eq!(
        observation["source_job"],
        tool_result(&setup_body, "reconciled-install")["observation"]["job_id"]
    );
    close(&mut cli, &mut provider, &scope).await;
    public::assert_observed_helpers_gone(&helpers).await;
    let prepared = preparation(&cli);
    let config = fs::read(cli.state.join(REALM).join("config.toml")).unwrap();
    let parsed = meerkat_core::Config::from_persisted_toml(std::str::from_utf8(&config).unwrap())
        .unwrap()
        .0;
    assert_eq!(parsed.tools.mcp_servers, vec![descriptor]);
    let setup_requests = provider.requests.lock().unwrap().len();
    let output_path = root.join("fresh-rkat.stdout.json");
    let output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&output_path)
        .unwrap();
    let mut command = public::fresh_rkat_command(&binary, &expected, &root, &cli.state);
    command
        .stdin(Stdio::null())
        .stdout(output)
        .stderr(Stdio::piped());
    cli.child = Some(command.spawn().unwrap());
    let result = provider.with_profile_provider(cli.finish_output()).await;
    assert!(result.stderr.len() <= 1024 * 1024);
    fs::write(root.join("fresh-rkat.stderr"), &result.stderr).unwrap();
    assert!(
        result.status.success(),
        "fresh ordinary rkat failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    provider.finish().await;
    assert_eq!(
        fs::read(cli.state.join(REALM).join("config.toml")).unwrap(),
        config
    );
    let requests = provider.requests.lock().unwrap();
    assert_eq!(
        requests.len(),
        setup_requests + 4,
        "exact search/load/public-read sequence"
    );
    let body = &requests.last().unwrap().body;
    let service_result = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["role"] == "tool" && message["tool_call_id"] == public::STORY_CALL)
        .unwrap();
    let stories = public::public_stories(service_result["content"].as_str().unwrap()).unwrap();
    let (_, outcome) = read_bounded_json(&output_path, 1024 * 1024);
    assert_eq!(outcome["tool_calls"], 3);
    assert_eq!(outcome["text"], public::story_answer(&stories));
    assert_ne!(outcome["session_id"], setup_session);
    assert_eq!(
        outcome["session_ref"],
        format!("{REALM}:{}", outcome["session_id"].as_str().unwrap())
    );
    let evidence = json!({"case":"private_services_public_install_use","cli_sha256":file_digest(&cli.binary),
        "rkat_sha256":expected,"setup_session":setup_session,"fresh_session":outcome["session_id"],
        "preparation":prepared,"native_finalization":observation,"public_stories":stories,
        "temporary_realm_removed":true,"interrupted_effect_coverage":false});
    drop(requests);
    drop(cli);
    assert!(
        !root.exists(),
        "isolated installations and native realm grants removed after owner shutdown"
    );
    println!("{evidence}");
}

// These cases interrupt a real request-bearing packaged owner, after installation
// and reconcile but before the protected access decision. They do not simulate
// an installer crash or prove the outcome of an already-entered grant.
async fn pending_access(cli: &mut StdioCli, provider: &mut Server, scope: &str) -> Value {
    cli.request(
        "h:work",
        "connect/send",
        json!({"scope":scope,"text":INSTALL}),
    );
    let software = provider.with_profile_provider(cli.prompt("approval")).await;
    assert_eq!(
        software["params"]["prompt"]["body"]["kind"],
        "service_software_install"
    );
    assert_eq!(
        software["params"]["prompt"]["body"]["selection"],
        selected()
    );
    cli.answer(
        software["id"].as_str().unwrap(),
        json!({"kind":"approval","decision":"approve"}),
    );
    let access = provider.with_profile_provider(cli.prompt("approval")).await;
    let body = &access["params"]["prompt"]["body"];
    assert_eq!(body["phase"], "public_service_access");
    assert_eq!(body["authentication"], json!({"kind":"none"}));
    assert_eq!(body["realm"], REALM);
    assert_eq!(body["grant"]["operations"], json!([public::OPERATION]));
    assert_eq!(body["grant"]["agent"], "rkat-agent");
    assert_eq!(body["grant"]["expires_in_ms"], Value::Null);
    let request = provider
        .requests
        .lock()
        .unwrap()
        .last()
        .unwrap()
        .body
        .clone();
    effect_counts(&request, true, false);
    let job = tool_result(&request, "reconciled-install");
    assert_eq!(job["observation"]["status"], "matched");
    public::reviewed_descriptor(&request, "", "hackernews", public::OPERATION);
    assert!(
        !cli.rows
            .iter()
            .any(|row| row["id"] == "h:work" && row.get("method").is_none())
    );
    job
}

// Scoped ps reads only the already-observed fixture process. The start time,
// process group and full selected argv are compared again before every signal;
// a reused PID must never receive a fixture cleanup signal.
fn process_start_and_args(process: public::ObservedProcess) -> Result<(String, bool), String> {
    let mut child = Command::new("/bin/ps")
        .args([
            "-ww",
            "-p",
            &process.pid.to_string(),
            "-o",
            "pgid=,stat=,lstart=,args=",
        ])
        .env_clear()
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let until = std::time::Instant::now() + Duration::from_millis(250);
    while child.try_wait().map_err(|e| e.to_string())?.is_none() {
        if std::time::Instant::now() >= until {
            let _ = child.kill();
            let _ = child.wait();
            return Err("bounded exact helper identity read timed out".into());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if !output.status.success() || output.stdout.len() > 16 * 1024 || !output.stderr.is_empty() {
        return Err("exact helper identity unavailable".into());
    }
    let identity = String::from_utf8(output.stdout).map_err(|e| e.to_string())?;
    let identity = identity.trim();
    let (group, rest) = identity
        .split_once(char::is_whitespace)
        .ok_or("missing helper identity")?;
    if group.parse::<u32>().ok() != Some(process.group) || rest.lines().count() != 1 {
        return Err("helper process group changed".into());
    }
    let (state, rest) = rest
        .trim()
        .split_once(char::is_whitespace)
        .ok_or("missing helper start time")?;
    Ok((rest.trim().to_owned(), state.starts_with('T')))
}

struct HelperFault {
    selected: usize,
    identities: Vec<(public::ObservedProcess, String)>,
    stopped: bool,
    cleanup: bool,
}
impl HelperFault {
    fn bind(cli: &StdioCli, helpers: &[public::ObservedProcess]) -> Self {
        let artifacts = preparation_artifacts(&cli.state);
        assert_eq!(artifacts.len(), 1);
        let (_, handoff) = read_bounded_json(&artifacts[0].join("prepare.stdout"), 256 * 1024);
        let (_, retained) =
            read_bounded_json(&artifacts[0].join("prepared/product-mcp.json"), 256 * 1024);
        assert_eq!(handoff, retained);
        let server = &handoff["mcpServers"]["toolkit-setup"];
        assert_eq!(handoff["mcpServers"].as_object().unwrap().len(), 1);
        assert_eq!(server["command"], "/usr/bin/env");
        let args: Vec<_> = server["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(
            &args[..3],
            ["-i", "PATH=/usr/bin:/bin", "PYTHONDONTWRITEBYTECODE=1"]
        );
        assert_eq!(&args[4..6], ["-I", "-B"]);
        assert_eq!(Path::new(args[6]).file_name().unwrap(), "local_session.py");
        assert_eq!(args.iter().filter(|arg| **arg == "--state-root").count(), 1);
        let state = args.iter().position(|arg| *arg == "--state-root").unwrap();
        assert_eq!(
            Path::new(args[state + 1]),
            fs::canonicalize(artifacts[0].join("prepared/local-session")).unwrap()
        );
        let argv = args[3..].join(" ");
        assert!(argv.len() < 8192);
        let identities: Vec<_> = helpers
            .iter()
            .map(|process| {
                (
                    *process,
                    process_start_and_args(*process)
                        .expect("live fixture helper identity")
                        .0,
                )
            })
            .collect();
        let matches: Vec<_> = identities
            .iter()
            .enumerate()
            .filter(|(_, (process, identity))| {
                process.parent == cli.child.as_ref().unwrap().id()
                    && process.group == process.pid
                    && identity.ends_with(&format!(" {argv}"))
            })
            .map(|(index, _)| index)
            .collect();
        assert_eq!(
            matches.len(),
            1,
            "one direct native child must match the exact selected launcher"
        );
        Self {
            selected: matches[0],
            identities,
            stopped: false,
            cleanup: true,
        }
    }
    fn signal(&self, signal: Signal) -> Result<(), String> {
        let (process, identity) = &self.identities[self.selected];
        if process_start_and_args(*process)?.0 != *identity {
            return Err("helper start identity changed".into());
        }
        nix::sys::signal::kill(Pid::from_raw(process.pid.try_into().unwrap()), signal)
            .map_err(|e| e.to_string())
    }
    fn stop(&mut self) {
        // Set first so unwinding after an uncertain signal still attempts resume.
        self.stopped = true;
        self.signal(Signal::SIGSTOP)
            .expect("stop the selected packaged helper");
        let (process, identity) = &self.identities[self.selected];
        let (current, stopped) =
            process_start_and_args(*process).expect("stopped helper observation");
        assert_eq!(current, *identity);
        assert!(
            stopped,
            "actual helper must be stopped before the retirement probe"
        );
    }
    fn resume(&mut self) {
        self.signal(Signal::SIGCONT)
            .expect("resume the same selected packaged helper");
        self.stopped = false;
    }
}
impl Drop for HelperFault {
    fn drop(&mut self) {
        // Resume also on timeout/assertion, before StdioCli's failure cleanup.
        if self.stopped {
            let _ = self.signal(Signal::SIGCONT);
        }
        if self.cleanup {
            for (process, identity) in &self.identities {
                if process_start_and_args(*process).is_ok_and(|(current, _)| current == *identity) {
                    let _ = killpg(
                        Pid::from_raw(process.group.try_into().unwrap()),
                        Signal::SIGKILL,
                    );
                }
            }
        }
    }
}

fn retained_install(cli: &StdioCli, job: &Value) -> Vec<(PathBuf, Vec<u8>)> {
    let artifacts = preparation_artifacts(&cli.state);
    assert_eq!(artifacts.len(), 1);
    // Native preparation canonicalizes the isolated state root. A macOS /var
    // alias must not be mistaken for a different owner or checkpoint workspace.
    let root = fs::canonicalize(artifacts[0].join("prepared/local-session")).unwrap();
    let session = root.join("session.json");
    let (session_raw, record) = read_bounded_json(&session, 256 * 1024);
    let workspace = record["workspace"].as_str().unwrap();
    assert!(workspace.starts_with("work-") && Path::new(workspace).components().count() == 1);
    let workspace = root.join(workspace);
    let checkpoint = if record["schema_version"] == 1 {
        root.join("recovery.json")
    } else {
        workspace.join("local-recovery.json")
    };
    let (checkpoint_raw, selected) = read_bounded_json(&checkpoint, 256 * 1024);
    assert_eq!(selected["workspace"], workspace.to_str().unwrap());
    let entries = selected["requests"].as_array().unwrap();
    assert_eq!(
        entries.len(),
        1,
        "one actual installation request, no replay"
    );
    let id = entries[0]["id"].as_str().unwrap();
    assert_eq!(job["observation"]["job_id"], id);
    assert!(
        id.bytes()
            .all(|v| v.is_ascii_alphanumeric() || v == b'-' || v == b'_')
    );
    let request = workspace.join("requests").join(format!("{id}.json"));
    let (request_raw, request_value) = read_bounded_json(&request, 512 * 1024);
    assert_eq!(
        entries[0]["sha256"],
        format!("{:x}", Sha256::digest(&request_raw))
    );
    assert_eq!(request_value["job_id"], id);
    assert_eq!(
        request_value["kind"],
        "toolkit.consumer_setup.configured_install_request"
    );
    vec![
        (session, session_raw),
        (checkpoint, checkpoint_raw),
        (request, request_raw),
    ]
}

async fn interrupt_pending_access(abnormal: bool) {
    let mut provider = Server::package_provider(response).await;
    let mut cli = StdioCli::start_supplied(&provider.origin);
    let root = cli.root.path().to_owned();
    let config = fs::read(cli.state.join(REALM).join("config.toml")).unwrap();
    let (scope, session) = initialize(&mut cli, &mut provider, "services").await;
    let job = pending_access(&mut cli, &mut provider, &scope).await;
    let requests = provider.requests.lock().unwrap().len();
    let helpers = public::observed_descendants(cli.child.as_ref().unwrap().id()).await;
    let mut fault = HelperFault::bind(&cli, &helpers);
    let retained = retained_install(&cli, &job);
    if abnormal {
        fault
            .signal(Signal::SIGKILL)
            .expect("fail the exact packaged helper");
    } else {
        fault.stop();
    }
    cli.request(
        "h:retire",
        "connect/retire",
        json!({"target":{"scope":scope},"retirement_id":"logout","reason":"logout"}),
    );
    if abnormal {
        // Keep both requests in the input before a failed native turn can
        // independently begin full close. Neither response may claim success.
        cli.request(
            "h:close",
            "connect/close",
            json!({"target":{"scope":scope}}),
        );
    }
    if !abnormal {
        // One bounded handshake, not a sleep/absence oracle. Core's initialized
        // helper EOF grace is 5s; always resume before its TERM escalation.
        let probe = timeout(
            Duration::from_secs(1),
            provider.with_profile_provider(async {
                let work = cli.response("h:work").await;
                assert!(work.get("result").is_some() || work.get("error").is_some());
                cli.request(
                    "h:late",
                    "connect/send",
                    json!({"scope":scope,"text":"must not enter"}),
                );
                let late = cli.response("h:late").await;
                assert_eq!(late["error"]["data"]["code"], "binding_retired");
                assert!(
                    !cli.rows
                        .iter()
                        .any(|row| row["id"] == "h:retire" && row.get("method").is_none()),
                    "retirement cannot acknowledge while the exact helper cannot close"
                );
            }),
        )
        .await;
        fault.resume();
        probe.expect("retirement fence observed before helper close escalation");
    }
    let retired = provider
        .with_profile_provider(cli.response("h:retire"))
        .await;
    if !abnormal {
        cli.request(
            "h:close",
            "connect/close",
            json!({"target":{"scope":scope}}),
        );
    }
    let closed = provider
        .with_profile_provider(cli.response("h:close"))
        .await;
    let output = provider.with_profile_provider(cli.finish_output()).await;
    assert!(output.stderr.len() <= 64 * 1024);
    let stderr = std::str::from_utf8(&output.stderr).unwrap();
    if abnormal {
        for row in [&retired, &closed] {
            assert_eq!(
                row["error"]["data"]["code"], "shutdown_unconfirmed",
                "{row}"
            );
            assert!(row.get("result").is_none());
        }
        assert!(!output.status.success());
        assert!(
            stderr.contains("Private setup cleanup is unconfirmed; the same owner is retained"),
            "preserve the actual abnormal close owner diagnostic: {stderr}"
        );
    } else {
        assert_eq!(retired["result"]["entered_work"], "settled", "{retired}");
        assert_eq!(closed["result"]["resources_closed"], true, "{closed}");
        assert!(
            output.status.success() && stderr.is_empty(),
            "normal helper close: {stderr}"
        );
    }
    public::assert_observed_helpers_gone(&helpers).await;
    fault.cleanup = false;
    provider.finish().await;
    assert_eq!(
        provider.requests.lock().unwrap().len(),
        requests,
        "no post-retirement model work"
    );
    assert_eq!(
        fs::read(cli.state.join(REALM).join("config.toml")).unwrap(),
        config
    );
    assert!(
        !cli.rows
            .iter()
            .any(|row| row["params"]["kind"] == "service_finalization"),
        "an unanswered access review cannot enter the finalizer"
    );
    assert_eq!(
        cli.rows
            .iter()
            .filter(|row| row["id"] == "h:work" && row.get("method").is_none())
            .count(),
        1,
        "retain the actual entered native request's result alongside cleanup"
    );
    for (path, raw) in &retained {
        assert_eq!(
            fs::read(path).unwrap(),
            *raw,
            "request-bearing evidence retained through close"
        );
    }
    let (helper, identity) = &fault.identities[fault.selected];
    let evidence = json!({"case":if abnormal {"private_services_failed_packaged_helper"} else {"private_services_retire_held_packaged_helper"},
        "cli_sha256":file_digest(&cli.binary),"session":session,"source_job":job["observation"]["job_id"],
        "helper":{"pid":helper.pid,"group":helper.group,"start_and_selected_argv_sha256":format!("{:x}",Sha256::digest(identity.as_bytes())),
            "signal":if abnormal {"SIGKILL"} else {"SIGSTOP_then_SIGCONT"}},
        "installed_reconciled":true,"access_review_answered":false,"configuration_unchanged":true,
        "finalizer_entered":false,"exhaustive_grant_store_observation":false,"retirement":retired,"close":closed,
        "native_stderr":stderr,"same_request_retained_sha256":retained.iter().map(|(_,raw)|format!("{:x}",Sha256::digest(raw))).collect::<Vec<_>>(),
        "observed_helper_groups_gone":true,"temporary_realm_removed":true,"mid_installer_interruption_coverage":false});
    drop(fault);
    drop(cli);
    assert!(
        !root.exists(),
        "explicit fixture cleanup follows retained close observation"
    );
    println!("{evidence}");
}

#[tokio::test]
#[ignore = "supplied exact CLI/library; real installed request and held packaged helper, no accounts"]
async fn actual_stdio_services_retire_waits_for_packaged_helper() {
    interrupt_pending_access(false).await;
}

#[tokio::test]
#[ignore = "supplied exact CLI/library; kills exact packaged helper after install, no accounts"]
async fn actual_stdio_services_failed_packaged_helper_refuses_settlement() {
    interrupt_pending_access(true).await;
}

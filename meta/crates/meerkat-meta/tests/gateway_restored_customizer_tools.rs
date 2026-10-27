//! A persistent gateway's mob can be restored while the gateway's bootstrap
//! builds it: a mob log left Running revives every member inside meerkat's
//! own resume, before the identity bootstrap publishes any member's
//! customizer tools. As on the library builder, each such member holds its
//! run starts (`HostRunStartHoldReason::ToolsNotPublished`) until the identity
//! runtime publishes its tools and releases the hold, so it starts no run (a
//! restored queued input, a kickoff, a peer message) without them. A
//! process-local gateway restores nothing at its bootstrap: its member is
//! built with its tools and nothing is held. Each boot is a separate gateway
//! process over real stdio, answering the SDK callbacks itself; the first
//! persistent boot ends with a kill, so its mob log stays Running.
//!
//! The holds come from one roster per boot, read before the mob build from
//! the definition the mob runs, and the identity bootstrap takes that same
//! roster: a roster callback that fails before it does not leave a member
//! unheld, and a candidate restart that runs the stored definition holds the
//! members that definition's roster names, and never releases them. A
//! restored member the roster no
//! longer lists stays held until the roster pass retires it, on an eager and
//! a lazy restart. Reading that roster before the build keeps what the
//! roster provider is handed (the booted definition) and the init failures a
//! refused composition answers with.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use meerkat_meta::UnifiedRuntime;
use meerkat_meta::rpc_gateway::{
    GatewayBootstrapReady, GatewayHostCloseReason, GatewayHostComposition, GatewayHostError,
};
use meerkat_mob::MobSessionService;
use meerkat_runtime::meerkat_machine::NativeGrantWorkConfiguration;
use serde_json::{Value, json};

/// Set only in the gateway host process; `RECORD` names its record file.
const HOST_PROCESS: &str = "META_GATEWAY_RESTORED_TOOLS_HOST";
const RECORD: &str = "META_GATEWAY_RESTORED_TOOLS_RECORD";
const PERSISTENT_TEST: &str =
    "a_member_restored_at_a_persistent_gateway_bootstrap_holds_its_runs_until_its_tools_publish";
const PROCESS_LOCAL_TEST: &str = "a_process_local_gateway_bootstrap_holds_no_member";
const FAILED_ROSTER_CALL_TEST: &str =
    "a_restart_whose_first_roster_callback_fails_still_holds_its_restored_member";
const CANDIDATE_TEST: &str =
    "a_candidate_restart_holds_the_member_the_roster_of_its_stored_definition_names";
const DROPPED_TEST: &str =
    "a_restored_member_the_roster_no_longer_lists_stays_held_until_it_is_retired";
const LAZY_DROPPED_TEST: &str =
    "a_lazy_restart_holds_a_restored_member_the_roster_no_longer_lists_until_it_is_retired";
const BOOTED_DEFINITION_TEST: &str =
    "the_boot_roster_is_read_from_the_booted_definition_on_a_fresh_launch_and_a_resume";
const REFUSED_COMPOSITION_TEST: &str =
    "a_restart_refused_for_its_composition_keeps_the_refusal_code_and_message";
const BACKSTOP: Duration = Duration::from_mins(3);
/// Mob members need comms; `turn_driven` keeps the member from starting
/// turns of its own.
const MOB_CONFIG: &str = r#"
[mob]
id = "gateway-restored-customizer-tools"

[profiles.worker]
model = "gpt-5.5"
runtime_mode = "turn_driven"

[profiles.worker.tools]
comms = true
"#;
/// The same mob with its one profile renamed: a definition that differs
/// from the one [`MOB_CONFIG`] stores.
const RENAMED_PROFILE_MOB_CONFIG: &str = r#"
[mob]
id = "gateway-restored-customizer-tools"

[profiles.helper]
model = "gpt-5.5"
runtime_mode = "turn_driven"

[profiles.helper.tools]
comms = true
"#;
/// [`MOB_CONFIG`] with provider parameters on its profile: the mob build
/// normalizes such a definition (it declares the parameters as a resume
/// override) before it creates the mob.
const PROVIDER_PARAMS_MOB_CONFIG: &str = r#"
[mob]
id = "gateway-restored-customizer-tools"

[profiles.worker]
model = "gpt-5.5"
runtime_mode = "turn_driven"

[profiles.worker.provider_params]
max_output_tokens = 1024

[profiles.worker.tools]
comms = true
"#;
/// The one roster member, and the tool the SDK customizer gives it.
const IDENTITY: &str = "keeper";
const TOOL: &str = "lookup";
/// A second member that a later roster no longer lists.
const DROPPED: &str = "dropped";

fn append(record: &Path, line: &str) {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(record)
        .expect("record");
    writeln!(file, "{line}").expect("append");
}

/// Whether meerkat holds each session's run starts, in session order
/// (`None` for a session its runtime does not register).
async fn run_starts_held(service: &Arc<dyn MobSessionService>) -> Vec<Option<bool>> {
    let Some(machine) = service
        .acquire_runtime_adapter(None)
        .expect("acquire the gateway's runtime adapter")
    else {
        return Vec::new();
    };
    let mut sessions = service
        .list(meerkat_core::service::SessionQuery::default())
        .await
        .expect("sessions")
        .into_iter()
        .map(|summary| summary.session_id)
        .collect::<Vec<_>>();
    sessions.sort_by_key(ToString::to_string);
    let mut held = Vec::new();
    for session in &sessions {
        held.push(machine.run_starts_held_for_test(session).await);
    }
    held
}

/// Whether meerkat holds the run starts of each member of the mob, as
/// `member=held` in member order (`None` for a member whose session its
/// runtime does not register).
async fn members_held(
    handle: &meerkat_mob::MobHandle,
    service: &Arc<dyn MobSessionService>,
) -> String {
    let machine = service
        .acquire_runtime_adapter(None)
        .expect("acquire the gateway's runtime adapter");
    let mut members = Vec::new();
    for entry in handle.list_all_members().await {
        let held = match (machine.as_ref(), entry.bridge_session_id()) {
            (Some(machine), Some(session)) => machine.run_starts_held_for_test(session).await,
            _ => None,
        };
        members.push(format!("{}={held:?}", entry.agent_identity));
    }
    members.sort();
    members.join(" ")
}

/// Records whether meerkat holds the run starts of the members it restored,
/// once when the gateway binds the runtime (after the mob build, before the
/// identity bootstrap) and once after the Ready publication (after the
/// identity bootstrap), and each member the identity runtime retires.
struct HoldProbeHost {
    record: PathBuf,
    service: Mutex<Option<Arc<dyn MobSessionService>>>,
    handle: Mutex<Option<meerkat_mob::MobHandle>>,
}

#[async_trait::async_trait]
impl GatewayHostComposition for HoldProbeHost {
    async fn prepare_bootstrap(
        &self,
        spec: meerkat_meta::MobBootstrapSpec,
    ) -> Result<meerkat_meta::MobBootstrapSpec, GatewayHostError> {
        Ok(spec)
    }

    fn native_grant_work_configuration(
        &self,
    ) -> Result<Option<NativeGrantWorkConfiguration>, GatewayHostError> {
        Ok(None)
    }

    async fn bind_runtime(&self, runtime: &mut UnifiedRuntime) -> Result<(), GatewayHostError> {
        let service = runtime
            .mob_runtime()
            .session_service()
            .cloned()
            .expect("the gateway's session service");
        let handle = runtime.mob_handle();
        let held = run_starts_held(&service).await;
        let members = members_held(&handle, &service).await;
        append(&self.record, &format!("bind members={members}"));
        append(&self.record, &format!("bind held={held:?}"));
        *self.service.lock().unwrap() = Some(service);
        *self.handle.lock().unwrap() = Some(handle);
        Ok(())
    }

    async fn identity_member_retired(
        &self,
        retired: &meerkat_meta::identity_first::RetiredIdentityMember,
    ) -> Result<(), GatewayHostError> {
        append(
            &self.record,
            &format!(
                "retired {} cause={}",
                retired.identity,
                retired.cause.as_str()
            ),
        );
        Ok(())
    }

    fn publish_bootstrap_ready(
        &self,
        publication: GatewayBootstrapReady<'_>,
    ) -> Result<(), GatewayHostError> {
        publication.publish();
        let service = self.service.lock().unwrap().clone();
        let handle = self.handle.lock().unwrap().clone();
        let record = self.record.clone();
        tokio::spawn(async move {
            let (held, members) = match (service, handle) {
                (Some(service), Some(handle)) => (
                    run_starts_held(&service).await,
                    members_held(&handle, &service).await,
                ),
                _ => (Vec::new(), String::new()),
            };
            // The driver waits for the `ready held=` line, so it is last.
            append(&record, &format!("ready members={members}"));
            append(&record, &format!("ready held={held:?}"));
        });
        Ok(())
    }

    fn begin_close(&self, _reason: GatewayHostCloseReason) -> Result<(), GatewayHostError> {
        Ok(())
    }

    async fn join_close(&self) -> Result<(), GatewayHostError> {
        Ok(())
    }

    async fn wait_closed(&self) {
        std::future::pending::<()>().await;
    }
}

/// In the gateway host process, run the gateway with the probing host and
/// exit.
fn serve_if_host_process() {
    if std::env::var_os(HOST_PROCESS).is_none() {
        return;
    }
    let host = Arc::new(HoldProbeHost {
        record: std::env::var_os(RECORD).expect("record path").into(),
        service: Mutex::new(None),
        handle: Mutex::new(None),
    });
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let _ = runtime.block_on(meerkat_meta::rpc_gateway::run_persistent_with_host(
        None, host,
    ));
    std::process::exit(0);
}

/// The roster a boot's SDK side answers.
#[derive(Clone, Copy)]
enum Roster {
    /// These identities, each on the `worker` profile.
    Listed(&'static [&'static str]),
    /// One identity per profile of the definition the callback's context
    /// names, `{profile}-member`, as a roster derived from the booted
    /// definition is.
    FromDefinition,
}

/// Whether a boot speaks for the stored composition
/// (`runtime_options.mob_composition`).
#[derive(Clone, Copy)]
enum Composition {
    Authoritative,
    /// A candidate launch: refused when its definition differs from the
    /// stored one.
    Candidate,
    /// A candidate launch that boots the stored definition when the
    /// supplied one differs.
    CandidateRunningTheStored,
}

/// What one boot launches, and what its SDK side answers.
#[derive(Clone, Copy)]
struct Launch {
    /// The mob definition the boot supplies.
    mob_config: &'static str,
    roster: Roster,
    /// How many of the boot's first roster callbacks answer an error.
    failed_roster_calls: usize,
    composition: Composition,
    /// Lazy identity materialization instead of the eager default.
    lazy: bool,
    /// A request sent once the init settled ready (its roster callbacks are
    /// recorded apart from the init's).
    after_ready: Option<&'static str>,
}

impl Default for Launch {
    fn default() -> Self {
        Self {
            mob_config: MOB_CONFIG,
            roster: Roster::Listed(&[IDENTITY]),
            failed_roster_calls: 0,
            composition: Composition::Authoritative,
            lazy: false,
            after_ready: None,
        }
    }
}

/// The SDK side of the gateway's callback bridge: the roster `launch`
/// names, and a customizer that gives each member one callback tool.
/// `roster_calls` counts the boot's roster callbacks so far.
fn answer_callback(
    method: &str,
    params: &Value,
    launch: &Launch,
    roster_calls: &mut usize,
) -> Result<Value, Value> {
    match method {
        "callback/roster_provider/roster" => {
            *roster_calls += 1;
            if *roster_calls <= launch.failed_roster_calls {
                return Err(json!({ "code": -32000, "message": "the roster is unavailable" }));
            }
            Ok(Value::Array(match launch.roster {
                Roster::Listed(identities) => identities
                    .iter()
                    .map(|identity| json!({ "identity": identity, "profile": "worker" }))
                    .collect(),
                Roster::FromDefinition => params["context"]["mob_definition"]["profiles"]
                    .as_object()
                    .unwrap_or_else(|| panic!("the roster context names no profiles: {params}"))
                    .keys()
                    .map(|profile| {
                        json!({ "identity": format!("{profile}-member"), "profile": profile })
                    })
                    .collect(),
            }))
        }
        "callback/agent_customizer/customize_build" => {
            let mut draft = params["draft"].clone();
            draft["external_tools"] = json!([{
                "name": TOOL,
                "description": "Look a value up",
                "input_schema": { "type": "object" },
            }]);
            Ok(draft)
        }
        "callback/agent_customizer/after_create" => Ok(Value::Null),
        other => Err(json!({ "code": -32601, "message": format!("no handler for {other}") })),
    }
}

/// How a boot's gateway process ends once its init settled ready.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ending {
    /// Close its stdin: the gateway shuts down and stops its mob.
    Close,
    /// Kill it: its mob log stays Running.
    Kill,
}

/// One boot: its record lines, its settled outcome, the SDK customizer
/// builds it asked for, and the context of each roster callback.
struct Boot {
    lines: Vec<String>,
    outcome: String,
    /// The `meta/init_settled` params.
    settled: Value,
    customizer_builds: usize,
    /// The roster callbacks' contexts during the init, in order.
    init_rosters: Vec<Value>,
    /// The roster callbacks' contexts while the `after_ready` request ran.
    later_rosters: Vec<Value>,
}

impl Boot {
    fn line(&self, prefix: &str) -> &str {
        self.lines
            .iter()
            .map(String::as_str)
            .find(|line| line.starts_with(prefix))
            .unwrap_or_else(|| panic!("a {prefix:?} line is recorded: {:?}", self.lines))
    }
}

/// Run one gateway host process of `test` in `dir` over `state` (none: a
/// process-local launch) through its init, wait for the host's Ready probe,
/// and end it as `ending` says. The boots of one test share the comms
/// identity keys (kept under `XDG_STATE_HOME`, beside their directories), as
/// the restarts of one installation do.
fn boot(test: &str, dir: &Path, state: Option<&Path>, ending: Ending, launch: &Launch) -> Boot {
    std::fs::create_dir_all(dir).expect("boot dir");
    let record = dir.join("record");
    let stderr_path = dir.join("stderr");
    let xdg_state = dir
        .parent()
        .expect("a boot directory sits in its test's directory")
        .join("xdg-state");
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture"])
        .env(HOST_PROCESS, "1")
        .env(RECORD, &record)
        .env("OPENAI_API_KEY", "sk-regression-test")
        .env("XDG_STATE_HOME", xdg_state)
        .current_dir(dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::fs::File::create(&stderr_path).expect("stderr file"))
        .spawn()
        .expect("gateway host process");
    let stdout = child.stdout.take().expect("stdout");
    let (tx, lines) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut stdin = child.stdin.take().expect("stdin");
    let mut send = |value: Value| {
        writeln!(stdin, "{value}").expect("write");
        stdin.flush().expect("flush");
    };
    let mut runtime_options = json!({ "demo_llm": true });
    match launch.composition {
        Composition::Authoritative => {}
        Composition::Candidate => {
            runtime_options["mob_composition"] = json!({ "authority": "candidate" });
        }
        Composition::CandidateRunningTheStored => {
            runtime_options["mob_composition"] =
                json!({ "authority": "candidate", "candidate_definition": "stored" });
        }
    }
    if launch.lazy {
        runtime_options["identity_bootstrap_mode"] = json!({ "mode": "lazy_materialize" });
    }
    let mut params = json!({
        "init_protocol": "accepted_then_settled",
        "init_id": "restored-customizer-tools",
        "mob_config": launch.mob_config,
        "has_roster_provider": true,
        "has_agent_customizer": true,
        "runtime_options": runtime_options,
    });
    if let Some(state) = state {
        params["persistent_state"] = json!(state);
    }
    send(json!({"jsonrpc": "2.0", "id": "init", "method": "meta/init", "params": params}));

    let started = Instant::now();
    let mut seen = Vec::new();
    let mut customizer_builds = 0;
    let mut roster_calls = 0;
    let mut rosters = Vec::new();
    // The next message from the gateway, answering each callback request on
    // the way; `None` once the backstop passed.
    let mut next = |customizer_builds: &mut usize,
                    rosters: &mut Vec<Value>,
                    send: &mut dyn FnMut(Value)|
     -> Option<Value> {
        loop {
            let line = lines
                .recv_timeout(BACKSTOP.saturating_sub(started.elapsed()))
                .ok()?;
            // The test harness of the host process may print before the
            // first message on the same line.
            let Some(message) = line
                .find('{')
                .and_then(|start| serde_json::from_str::<Value>(line[start..].trim()).ok())
            else {
                continue;
            };
            if let (Some(method), Some(id)) = (
                message.get("method").and_then(Value::as_str),
                message.get("id").cloned(),
            ) {
                match method {
                    "callback/agent_customizer/customize_build" => *customizer_builds += 1,
                    "callback/roster_provider/roster" => {
                        rosters.push(message["params"]["context"].clone());
                    }
                    _ => {}
                }
                send(
                    match answer_callback(method, &message["params"], launch, &mut roster_calls) {
                        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                        Err(error) => json!({ "jsonrpc": "2.0", "id": id, "error": error }),
                    },
                );
            }
            return Some(message);
        }
    };
    let settled = loop {
        let Some(message) = next(&mut customizer_builds, &mut rosters, &mut send) else {
            let _ = child.kill();
            let _ = child.wait();
            eprint!(
                "{}",
                std::fs::read_to_string(&stderr_path).unwrap_or_default()
            );
            panic!("the init did not settle; seen {seen:?}");
        };
        if message.get("method").and_then(Value::as_str) == Some("meta/init_settled") {
            break message["params"].clone();
        }
        seen.push(message);
    };
    let outcome = settled["outcome"].as_str().unwrap_or_default().to_string();
    let init_rosters = std::mem::take(&mut rosters);
    // The host's Ready probe runs after the publication.
    let deadline = Instant::now() + BACKSTOP;
    while outcome == "ready"
        && !std::fs::read_to_string(&record)
            .unwrap_or_default()
            .contains("ready held=")
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    if let Some(method) = launch.after_ready.filter(|_| outcome == "ready") {
        send(json!({ "jsonrpc": "2.0", "id": "after", "method": method, "params": {} }));
        loop {
            let message = next(&mut customizer_builds, &mut rosters, &mut send)
                .unwrap_or_else(|| panic!("{method} was not answered"));
            if message["id"] == "after" {
                assert!(
                    message.get("result").is_some(),
                    "{method} answered: {message}"
                );
                break;
            }
        }
    }
    let later_rosters = rosters;
    match ending {
        Ending::Kill => {
            let _ = child.kill();
            let _ = child.wait();
        }
        Ending::Close => {
            drop(stdin);
            let deadline = Instant::now() + BACKSTOP;
            while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(50));
            }
            if child.try_wait().unwrap().is_none() {
                let _ = child.kill();
            }
            let _ = child.wait();
        }
    }
    // The host process's log rides this test's own output.
    eprint!(
        "{}",
        std::fs::read_to_string(&stderr_path).unwrap_or_default()
    );
    let recorded = std::fs::read_to_string(&record).unwrap_or_default();
    Boot {
        lines: recorded.lines().map(str::to_string).collect(),
        outcome,
        settled,
        customizer_builds,
        init_rosters,
        later_rosters,
    }
}

/// A persistent gateway killed while its mob ran left the mob's log Running,
/// so the next boot's mob build revives the member inside meerkat's own
/// resume, before the identity bootstrap publishes its customizer tools.
/// Meerkat holds the revived member's run starts when the gateway binds the
/// runtime; the identity bootstrap publishes the tools (one SDK customizer
/// build) and releases the hold before Ready.
#[test]
fn a_member_restored_at_a_persistent_gateway_bootstrap_holds_its_runs_until_its_tools_publish() {
    serve_if_host_process();
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");

    let first = boot(
        PERSISTENT_TEST,
        &dir.path().join("boot-1"),
        Some(&state),
        Ending::Kill,
        &Launch::default(),
    );
    assert_eq!(first.outcome, "ready", "the first boot settles ready");
    assert!(
        first.customizer_builds >= 1,
        "the first boot builds the roster member through the SDK customizer"
    );
    assert_eq!(
        first.line("ready held="),
        "ready held=[Some(false)]",
        "the first boot's member runs unheld"
    );

    let second = boot(
        PERSISTENT_TEST,
        &dir.path().join("boot-2"),
        Some(&state),
        Ending::Close,
        &Launch::default(),
    );
    assert_eq!(second.outcome, "ready", "the restart settles ready");
    assert_eq!(
        second.line("bind held="),
        "bind held=[Some(true)]",
        "the member meerkat revived during the mob build holds its run starts until its \
         customizer tools publish"
    );
    assert!(
        second.customizer_builds >= 1,
        "the identity bootstrap publishes the member's tools through the SDK customizer"
    );
    assert_eq!(
        second.line("ready held="),
        "ready held=[Some(false)]",
        "the publication released the hold before Ready"
    );
}

/// A process-local gateway keeps its mob in memory, so its bootstrap
/// restores no member: nothing is held when it binds the runtime, and its
/// member, built by the identity bootstrap with its tools, runs unheld.
#[test]
fn a_process_local_gateway_bootstrap_holds_no_member() {
    serve_if_host_process();
    let dir = tempfile::tempdir().unwrap();
    let run = boot(
        PROCESS_LOCAL_TEST,
        &dir.path().join("boot"),
        None,
        Ending::Close,
        &Launch::default(),
    );
    assert_eq!(run.outcome, "ready", "the boot settles ready");
    assert_eq!(
        run.line("bind held="),
        "bind held=[]",
        "the bootstrap restored no member"
    );
    assert!(
        run.customizer_builds >= 1,
        "the identity bootstrap builds the member through the SDK customizer"
    );
    assert_eq!(
        run.line("ready held="),
        "ready held=[Some(false)]",
        "the member runs unheld"
    );
}

/// The restart's first roster callback fails and every later one answers.
/// The holds and the identity bootstrap use one roster, read before the mob
/// build and failing closed, so a failure before it cannot leave a member the
/// build revives unheld while the bootstrap still goes ahead with a later
/// answer.
#[test]
fn a_restart_whose_first_roster_callback_fails_still_holds_its_restored_member() {
    serve_if_host_process();
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");

    let first = boot(
        FAILED_ROSTER_CALL_TEST,
        &dir.path().join("boot-1"),
        Some(&state),
        Ending::Kill,
        &Launch::default(),
    );
    assert_eq!(first.outcome, "ready", "the first boot settles ready");

    let second = boot(
        FAILED_ROSTER_CALL_TEST,
        &dir.path().join("boot-2"),
        Some(&state),
        Ending::Close,
        &Launch {
            failed_roster_calls: 1,
            ..Launch::default()
        },
    );
    assert_eq!(second.outcome, "ready", "the restart settles ready");
    assert_eq!(
        second.line("bind members="),
        format!("bind members={IDENTITY}=Some(true)"),
        "the member meerkat revived during the mob build holds its run starts, although the \
         restart's first roster callback failed"
    );
    assert_eq!(
        second.line("ready members="),
        format!("ready members={IDENTITY}=Some(false)"),
        "the identity bootstrap published its tools and released the hold before Ready"
    );
}

/// A candidate restart that supplies a definition differing from the stored
/// one runs the stored definition (`candidate_definition = "stored"`). The
/// roster is derived from the definition its callback is handed: the holds
/// come from the roster of the definition the mob runs, the same roster the
/// identity bootstrap restores, so the member the build revives is held. A
/// candidate releases no hold: the identity bootstrap still publishes the
/// member's customizer tools, and the member stays held through Ready, so
/// it starts no run on the candidate.
#[test]
fn a_candidate_restart_holds_the_member_the_roster_of_its_stored_definition_names() {
    serve_if_host_process();
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");

    let first = boot(
        CANDIDATE_TEST,
        &dir.path().join("boot-1"),
        Some(&state),
        Ending::Kill,
        &Launch {
            roster: Roster::FromDefinition,
            ..Launch::default()
        },
    );
    assert_eq!(first.outcome, "ready", "the first boot settles ready");
    assert_eq!(
        first.line("ready members="),
        "ready members=worker-member=Some(false)",
        "the first boot's roster names the member of the stored definition's profile"
    );

    let second = boot(
        CANDIDATE_TEST,
        &dir.path().join("boot-2"),
        Some(&state),
        Ending::Close,
        &Launch {
            mob_config: RENAMED_PROFILE_MOB_CONFIG,
            roster: Roster::FromDefinition,
            composition: Composition::CandidateRunningTheStored,
            ..Launch::default()
        },
    );
    assert_eq!(
        second.outcome, "ready",
        "the candidate restart settles ready"
    );
    assert_eq!(
        second.line("bind members="),
        "bind members=worker-member=Some(true)",
        "the member of the stored definition, which the candidate runs, holds its run starts \
         from the mob build"
    );
    assert!(
        second.customizer_builds >= 1,
        "the identity bootstrap publishes the member's tools through the SDK customizer"
    );
    assert_eq!(
        second.line("ready members="),
        "ready members=worker-member=Some(true)",
        "a candidate releases no hold: the member stays held after its tools publish"
    );
}

/// The restart's roster no longer lists a member the first boot ran. The
/// mob build revives it all the same, and nothing publishes its customizer
/// tools: it holds its run starts from the build until the identity
/// bootstrap's roster pass retires it, so it never starts a run without
/// them.
#[test]
fn a_restored_member_the_roster_no_longer_lists_stays_held_until_it_is_retired() {
    serve_if_host_process();
    a_dropped_member_stays_held_until_it_is_retired(DROPPED_TEST, false);
}

/// The same on a lazy restart: its roster pass retires the members the
/// roster no longer lists as an eager one does, so the hold ends there too.
#[test]
fn a_lazy_restart_holds_a_restored_member_the_roster_no_longer_lists_until_it_is_retired() {
    serve_if_host_process();
    a_dropped_member_stays_held_until_it_is_retired(LAZY_DROPPED_TEST, true);
}

fn a_dropped_member_stays_held_until_it_is_retired(test: &str, lazy: bool) {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");

    let first = boot(
        test,
        &dir.path().join("boot-1"),
        Some(&state),
        Ending::Kill,
        &Launch {
            roster: Roster::Listed(&[IDENTITY, DROPPED]),
            ..Launch::default()
        },
    );
    assert_eq!(first.outcome, "ready", "the first boot settles ready");
    assert_eq!(
        first.line("ready members="),
        format!("ready members={DROPPED}=Some(false) {IDENTITY}=Some(false)"),
        "the first boot runs both members unheld"
    );

    let second = boot(
        test,
        &dir.path().join("boot-2"),
        Some(&state),
        Ending::Close,
        &Launch {
            lazy,
            ..Launch::default()
        },
    );
    assert_eq!(second.outcome, "ready", "the restart settles ready");
    assert_eq!(
        second.line("bind members="),
        format!("bind members={DROPPED}=Some(true) {IDENTITY}=Some(true)"),
        "both members meerkat revived during the mob build hold their run starts, the one the \
         roster no longer lists as well"
    );
    assert_eq!(
        second.line(&format!("retired {DROPPED} ")),
        format!("retired {DROPPED} cause=unlisted"),
        "the roster pass retires the member the roster no longer lists"
    );
    assert_eq!(
        second.line("ready members="),
        format!("ready members={IDENTITY}=Some(false)"),
        "the listed member's hold is released before Ready, and the dropped member is gone"
    );
}

/// The roster provider is handed the definition the mob runs, in the form
/// the mob holds it: the boot's roster, now read before the mob build, gets
/// the same definition a later roster read gets from the running mob, on a
/// fresh launch (the definition as the build normalizes it, here with its
/// provider parameters declared as a resume override) and on a resume (the
/// stored one). The early refusal read, made before the mob store is read,
/// is handed the supplied definition in that same form, so every roster read
/// of one boot sees one definition, as a roster provider may require.
#[test]
fn the_boot_roster_is_read_from_the_booted_definition_on_a_fresh_launch_and_a_resume() {
    serve_if_host_process();
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    let launch = Launch {
        mob_config: PROVIDER_PARAMS_MOB_CONFIG,
        after_ready: Some("meta/topology/query"),
        ..Launch::default()
    };
    for (boot_name, what) in [("boot-1", "a fresh launch"), ("boot-2", "a resume")] {
        let run = boot(
            BOOTED_DEFINITION_TEST,
            &dir.path().join(boot_name),
            Some(&state),
            Ending::Close,
            &launch,
        );
        assert_eq!(run.outcome, "ready", "{what} settles ready");
        assert_eq!(
            run.init_rosters.len(),
            2,
            "{what}: the early refusal read and the boot roster: {:?}",
            run.init_rosters
        );
        let booted = &run
            .later_rosters
            .last()
            .unwrap_or_else(|| panic!("{what}: the topology query read the roster"))["mob_definition"];
        assert_eq!(
            &run.init_rosters[1]["mob_definition"], booted,
            "{what}: the boot roster is read from the definition the mob runs"
        );
        assert_eq!(
            &run.init_rosters[0]["mob_definition"], booted,
            "{what}: the early refusal read is handed the definition in the same form"
        );
    }
}

/// A restart whose composition is refused is now judged before the mob
/// build, when the boot roster's definition is read. The init still fails
/// with the composition refusal's own code, kind and message, as when the
/// build refused it: authoritatively with a changed definition, and as a
/// candidate that must match the stored one.
#[test]
fn a_restart_refused_for_its_composition_keeps_the_refusal_code_and_message() {
    serve_if_host_process();
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");

    let first = boot(
        REFUSED_COMPOSITION_TEST,
        &dir.path().join("boot-1"),
        Some(&state),
        Ending::Close,
        &Launch::default(),
    );
    assert_eq!(first.outcome, "ready", "the first boot settles ready");

    for (boot_name, composition, kind) in [
        ("boot-2", Composition::Authoritative, "divergent"),
        ("boot-3", Composition::Candidate, "candidate_divergent"),
    ] {
        let refused = boot(
            REFUSED_COMPOSITION_TEST,
            &dir.path().join(boot_name),
            Some(&state),
            Ending::Close,
            &Launch {
                mob_config: RENAMED_PROFILE_MOB_CONFIG,
                composition,
                ..Launch::default()
            },
        );
        let settled = &refused.settled;
        assert_eq!(refused.outcome, "failed", "{settled}");
        assert_eq!(
            settled["code"],
            json!(meerkat_meta::COMPOSITION_PROVENANCE_CODE),
            "{settled}"
        );
        assert_eq!(settled["data"]["kind"], json!(kind), "{settled}");
        assert!(
            settled["message"].as_str().is_some_and(|message| message
                .starts_with("Runtime bootstrap failed: failed to bootstrap mob runtime: ")),
            "{settled}"
        );
        assert_eq!(settled["durable_effects"], "possible", "{settled}");
    }
}

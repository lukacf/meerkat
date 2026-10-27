//! A host-composed gateway tells its host about each retired incarnation of a
//! durable identity's member, once, through
//! `GatewayHostComposition::identity_member_retired`: an explicit retire
//! (`meta/retire`), a roster pass that retires an identity the roster no
//! longer lists (`meta/reconcile_identity`), and the identity bootstrap's own
//! retirement of a member the roster dropped while the gateway was down. A
//! host error leaves the retire standing and reaches the SDK as the
//! `retired_member_release_failed` error event. Each case runs separate
//! gateway processes over real stdio, answering the SDK callbacks itself.
//! The gateway composes its session bridge with the runtime machine, so a
//! retired live member names the registration of its own session.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use meerkat_meta::UnifiedRuntime;
use meerkat_meta::identity_first::RetiredIdentityMember;
use meerkat_meta::rpc_gateway::{
    GatewayBootstrapReady, GatewayHostCloseReason, GatewayHostComposition, GatewayHostError,
};
use meerkat_runtime::meerkat_machine::NativeGrantWorkConfiguration;
use serde_json::{Value, json};

/// Set (to the case's mode) only in the gateway host process; `RECORD` names
/// its record file.
const HOST_PROCESS: &str = "META_GATEWAY_RETIRED_MEMBER_HOST";
const RECORD: &str = "META_GATEWAY_RETIRED_MEMBER_RECORD";
const BACKSTOP: Duration = Duration::from_mins(3);
/// Mob members need comms; `turn_driven` keeps the members from starting
/// turns of their own.
const MOB_CONFIG: &str = r#"
[mob]
id = "gateway-retired-member"

[profiles.worker]
model = "gpt-5.5"
runtime_mode = "turn_driven"

[profiles.worker.tools]
comms = true
"#;
/// The roster member that stays listed, and the one the roster drops.
const KEEPER: &str = "keeper";
const LEAVER: &str = "leaver";
/// The record line the driver writes once the init settled ready.
const SETTLED: &str = "driver settled ready";
/// The record line the driver writes once its explicit retire answered.
const RETIRED: &str = "driver retire answered";
const RETIRE_THEN_UNLIST_TEST: &str =
    "a_host_hears_an_explicit_and_an_unlisted_retirement_once_each";
const BOOT_TEST: &str = "the_identity_bootstraps_retirement_reaches_the_host";
const ERROR_TEST: &str = "a_host_error_keeps_the_retire_and_reaches_the_sdk";

fn append(record: &Path, line: &str) {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(record)
        .expect("record");
    writeln!(file, "{line}").expect("append");
}

/// A host that records each retired incarnation it is told about and, in
/// `fail` mode, answers an error for it.
struct RetiringHost {
    fail: bool,
    record: PathBuf,
}

#[async_trait::async_trait]
impl GatewayHostComposition for RetiringHost {
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

    async fn bind_runtime(&self, _runtime: &mut UnifiedRuntime) -> Result<(), GatewayHostError> {
        Ok(())
    }

    async fn identity_member_retired(
        &self,
        retired: &RetiredIdentityMember,
    ) -> Result<(), GatewayHostError> {
        append(
            &self.record,
            &format!(
                "retired identity={} cause={} generation={} runtime={} session={} registration={}",
                retired.identity,
                retired.cause.as_str(),
                retired.generation,
                retired.agent_runtime_id,
                if retired.session_id.is_some() {
                    "some"
                } else {
                    "none"
                },
                // The registration of the incarnation's own session.
                match &retired.registration {
                    Some(registration)
                        if retired.session_id.as_ref() == Some(registration.session_id()) =>
                    {
                        "own"
                    }
                    Some(_) => "other",
                    None => "none",
                },
            ),
        );
        if self.fail {
            Err(GatewayHostError::Unavailable)
        } else {
            Ok(())
        }
    }

    fn publish_bootstrap_ready(
        &self,
        publication: GatewayBootstrapReady<'_>,
    ) -> Result<(), GatewayHostError> {
        publication.publish();
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

/// In the gateway host process, run the gateway with the case's host, record
/// how the run returned, and exit.
fn serve_if_host_process() {
    let Some(mode) = std::env::var_os(HOST_PROCESS) else {
        return;
    };
    let record: PathBuf = std::env::var_os(RECORD).expect("record path").into();
    let fail = match mode.to_str().expect("mode") {
        "record" => false,
        "fail" => true,
        other => panic!("unknown host mode {other}"),
    };
    let host: Arc<dyn GatewayHostComposition> = Arc::new(RetiringHost {
        fail,
        record: record.clone(),
    });
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let returned = runtime.block_on(meerkat_meta::rpc_gateway::run_persistent_with_host(
        None, host,
    ));
    append(&record, &format!("returned ok={}", returned.is_ok()));
    std::process::exit(0);
}

/// What the driver does once the init settled ready, in order.
enum Step {
    /// Answer later roster reads with these identities.
    Roster(&'static [&'static str]),
    /// Send this request and wait for its response.
    Request {
        id: &'static str,
        method: &'static str,
        params: Value,
    },
    /// Write this line to the record.
    Mark(&'static str),
}

/// One gateway host process, driven through its init and then its steps.
struct Run {
    /// The record: the host's lines and the driver's marks.
    lines: Vec<String>,
    /// Every message the gateway wrote.
    seen: Vec<Value>,
    /// The responses to the steps' requests, by request id.
    responses: BTreeMap<String, Value>,
    /// The params of every `meta/on_error` notification.
    errors: Vec<Value>,
}

impl Run {
    fn outcome(&self) -> &str {
        self.seen
            .iter()
            .find(|message| {
                message.get("method").and_then(Value::as_str) == Some("meta/init_settled")
            })
            .unwrap_or_else(|| panic!("the init settles: {:?}", self.seen))["params"]["outcome"]
            .as_str()
            .unwrap_or_default()
    }

    fn retired(&self) -> Vec<&str> {
        self.lines
            .iter()
            .map(String::as_str)
            .filter(|line| line.starts_with("retired "))
            .collect()
    }

    fn position(&self, line: &str) -> usize {
        self.lines
            .iter()
            .position(|recorded| recorded == line)
            .unwrap_or_else(|| panic!("{line:?} is recorded: {:?}", self.lines))
    }

    fn position_starting(&self, prefix: &str) -> usize {
        self.lines
            .iter()
            .position(|recorded| recorded.starts_with(prefix))
            .unwrap_or_else(|| panic!("a line starting {prefix:?} is recorded: {:?}", self.lines))
    }

    fn succeeded(&self, id: &str) -> &Value {
        let response = self
            .responses
            .get(id)
            .unwrap_or_else(|| panic!("{id} answered: {:?}", self.seen));
        assert!(
            response.get("error").is_none() && response.get("result").is_some(),
            "{id} succeeds: {response}"
        );
        &response["result"]
    }
}

/// The SDK roster: each identity on the worker profile.
fn roster_answer(identities: &[&str]) -> Value {
    Value::Array(
        identities
            .iter()
            .map(|identity| json!({ "identity": identity, "profile": "worker" }))
            .collect(),
    )
}

/// Drive the `mode` host as a child process of `test` over the state in
/// `dir`, with an identity roster of `roster`, through `steps`. With
/// `until_error` the run also waits for a `meta/on_error` notification after
/// its last step.
fn drive(
    test: &str,
    mode: &str,
    dir: &Path,
    roster: &'static [&'static str],
    steps: Vec<Step>,
    until_error: bool,
) -> Run {
    let record = dir.join("record");
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture"])
        .env(HOST_PROCESS, mode)
        .env(RECORD, &record)
        .env("OPENAI_API_KEY", "sk-regression-test")
        .env("XDG_STATE_HOME", dir.join("xdg-state"))
        .current_dir(dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
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
    send(
        json!({"jsonrpc": "2.0", "id": "init", "method": "meta/init", "params": {
            "init_protocol": "accepted_then_settled",
            "init_id": format!("retired-member-{mode}"),
            "mob_config": MOB_CONFIG,
            "persistent_state": dir.join("state"),
            "runtime_options": { "demo_llm": true },
            "has_roster_provider": true,
        }}),
    );

    let started = Instant::now();
    let mut roster = roster;
    let mut steps = steps.into_iter();
    let mut pending: Option<&'static str> = None;
    let mut steps_done = false;
    let mut seen = Vec::new();
    let mut responses = BTreeMap::new();
    let mut errors = Vec::new();
    loop {
        if steps_done && (!until_error || !errors.is_empty()) {
            break;
        }
        let remaining = BACKSTOP.saturating_sub(started.elapsed());
        let line = match lines.recv_timeout(remaining) {
            Ok(line) => line,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                panic!("{mode}: the gateway ended before the run did; seen {seen:?}")
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                panic!("{mode}: the run did not finish; seen {seen:?}")
            }
        };
        // The test harness of the host process may print before the first
        // message on the same line.
        let Some(message) = line
            .find('{')
            .and_then(|start| serde_json::from_str::<Value>(line[start..].trim()).ok())
        else {
            continue;
        };
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_string);
        let mut advance = false;
        match (method.as_deref(), message.get("id").cloned()) {
            (Some("callback/roster_provider/roster"), Some(id)) => {
                send(json!({ "jsonrpc": "2.0", "id": id, "result": roster_answer(roster) }));
            }
            (Some(other), Some(id)) if other.starts_with("callback/") => {
                send(json!({ "jsonrpc": "2.0", "id": id,
                    "error": { "code": -32601, "message": format!("no handler for {other}") } }));
            }
            (Some("meta/init_settled"), _) => {
                if message["params"]["outcome"] == "ready" {
                    append(&record, SETTLED);
                    advance = true;
                } else {
                    seen.push(message);
                    break;
                }
            }
            (Some("meta/on_error"), None) => errors.push(message["params"].clone()),
            (None, Some(Value::String(id))) if pending == Some(id.as_str()) => {
                responses.insert(id, message.clone());
                pending = None;
                advance = true;
            }
            _ => {}
        }
        seen.push(message);
        while advance {
            match steps.next() {
                Some(Step::Roster(identities)) => roster = identities,
                Some(Step::Mark(line)) => append(&record, line),
                Some(Step::Request { id, method, params }) => {
                    send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
                    pending = Some(id);
                    advance = false;
                }
                None => {
                    steps_done = true;
                    advance = false;
                }
            }
        }
    }
    drop(stdin);
    let deadline = Instant::now() + BACKSTOP;
    while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let exited = child.try_wait().unwrap().is_some();
    if !exited {
        let _ = child.kill();
    }
    let _ = child.wait();
    assert!(exited, "{mode}: the gateway ends after its run");
    let recorded = std::fs::read_to_string(&record).unwrap_or_default();
    Run {
        lines: recorded.lines().map(str::to_string).collect(),
        seen,
        responses,
        errors,
    }
}

/// An explicit retire reaches the host once, with cause `retire`, before the
/// retire answers; a later roster pass that no longer lists an identity
/// reaches it once, with cause `unlisted`, before the pass answers; each names
/// the registration of the retired incarnation's own session. The pass
/// that brings the explicitly retired identity back (it is still listed)
/// tells the host nothing, and the host hears of nothing else.
#[test]
fn a_host_hears_an_explicit_and_an_unlisted_retirement_once_each() {
    serve_if_host_process();
    let dir = tempfile::tempdir().unwrap();
    let run = drive(
        RETIRE_THEN_UNLIST_TEST,
        "record",
        dir.path(),
        &[KEEPER, LEAVER],
        vec![
            Step::Request {
                id: "retire",
                method: "meta/retire",
                params: json!({ "identity": KEEPER }),
            },
            Step::Mark(RETIRED),
            Step::Roster(&[KEEPER]),
            Step::Request {
                id: "reconcile",
                method: "meta/reconcile_identity",
                params: json!({}),
            },
        ],
        false,
    );
    assert_eq!(run.outcome(), "ready", "{:?}", run.seen);
    run.succeeded("retire");
    run.succeeded("reconcile");
    let retired = run.retired();
    assert_eq!(
        retired.len(),
        2,
        "two retirements, once each: {:?}",
        run.lines
    );
    assert!(
        retired[0].starts_with(&format!(
            "retired identity={KEEPER} cause=retire generation="
        )),
        "{:?}",
        run.lines
    );
    assert!(
        retired[1].starts_with(&format!(
            "retired identity={LEAVER} cause=unlisted generation="
        )),
        "{:?}",
        run.lines
    );
    for line in &retired {
        assert!(
            line.ends_with(" session=some registration=own"),
            "the gateway names the registration of the retired incarnation's session: {line}"
        );
    }
    let settled = run.position(SETTLED);
    let answered = run.position(RETIRED);
    let explicit = run.position_starting(&format!("retired identity={KEEPER} "));
    let unlisted = run.position_starting(&format!("retired identity={LEAVER} "));
    assert!(
        settled < explicit && explicit < answered && answered < unlisted,
        "the explicit retirement before its answer, the unlisted one after it: {:?}",
        run.lines
    );
    assert!(run.errors.is_empty(), "no error event: {:?}", run.errors);
}

/// A member the roster dropped while the gateway was down is retired by the
/// next gateway's identity bootstrap, and the host hears of it during the
/// init: the gateway installs the hook before the bootstrap.
#[test]
fn the_identity_bootstraps_retirement_reaches_the_host() {
    serve_if_host_process();
    let dir = tempfile::tempdir().unwrap();
    let first = drive(
        BOOT_TEST,
        "record",
        dir.path(),
        &[KEEPER, LEAVER],
        vec![],
        false,
    );
    assert_eq!(first.outcome(), "ready", "{:?}", first.seen);
    assert!(first.retired().is_empty(), "{:?}", first.lines);

    let second = drive(BOOT_TEST, "record", dir.path(), &[KEEPER], vec![], false);
    assert_eq!(second.outcome(), "ready", "{:?}", second.seen);
    let retired = second.retired();
    assert_eq!(retired.len(), 1, "one retirement: {:?}", second.lines);
    assert!(
        retired[0].starts_with(&format!(
            "retired identity={LEAVER} cause=unlisted generation="
        )),
        "{:?}",
        second.lines
    );
    let settled: Vec<usize> = second
        .lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.as_str() == SETTLED)
        .map(|(at, _)| at)
        .collect();
    assert_eq!(settled.len(), 2, "both inits settled: {:?}", second.lines);
    let unlisted = second.position_starting("retired ");
    assert!(
        settled[0] < unlisted && unlisted < settled[1],
        "the second init's bootstrap retired it before that init settled: {:?}",
        second.lines
    );
}

/// A host error leaves the retirement standing: the retire answers, the
/// member is retired, and the SDK receives one `retired_member_release_failed`
/// error event naming the incarnation, its cause and the host's error.
#[test]
fn a_host_error_keeps_the_retire_and_reaches_the_sdk() {
    serve_if_host_process();
    let dir = tempfile::tempdir().unwrap();
    let run = drive(
        ERROR_TEST,
        "fail",
        dir.path(),
        &[KEEPER, LEAVER],
        vec![
            Step::Request {
                id: "retire",
                method: "meta/retire",
                params: json!({ "identity": KEEPER }),
            },
            Step::Request {
                id: "status",
                method: "meta/status_identity",
                params: json!({ "identity": KEEPER }),
            },
        ],
        true,
    );
    assert_eq!(run.outcome(), "ready", "{:?}", run.seen);
    run.succeeded("retire");
    let status = run.succeeded("status");
    assert_eq!(
        status["state"], "dormant",
        "the member is retired: {status}"
    );
    let retired = run.retired();
    assert_eq!(retired.len(), 1, "{:?}", run.lines);
    assert!(
        retired[0].starts_with(&format!(
            "retired identity={KEEPER} cause=retire generation="
        )),
        "{:?}",
        run.lines
    );
    let released: Vec<&Value> = run
        .errors
        .iter()
        .filter(|event| event["category"] == "retired_member_release_failed")
        .collect();
    assert_eq!(released.len(), 1, "one error event: {:?}", run.errors);
    let event = released[0];
    assert_eq!(event["identity"], KEEPER, "{event}");
    assert_eq!(event["cause"], "retire", "{event}");
    assert!(event["agent_runtime_id"].is_string(), "{event}");
    assert!(event["session_id"].is_string(), "{event}");
    assert!(
        event["error"]
            .as_str()
            .is_some_and(|error| error.contains(&GatewayHostError::Unavailable.to_string())),
        "the host's own error: {event}"
    );
}

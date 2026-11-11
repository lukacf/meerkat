#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::uninlined_format_args
)]

//! `meta/identity/*`: the changes and reads of a mob's long-term agents,
//! through a real gateway.
//!
//! Each row is a `rpc_gateway` process on a pipe, on a persistent state
//! directory, with an identity roster the row answers as the SDK host and
//! the launch's identity policy in `runtime_options.identity_creation`. The
//! row speaks JSON-RPC as the stdio host (the operator, on the host surface
//! `rpc`) and reads back what it changed through the family's own reads and
//! through Meta's status of the agent.

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

/// A configured agent with a plain name.
const HOST_A: &str = "host-a";
/// A configured agent whose name the member id encoding rewrites.
const DESK: &str = "desk:front";
/// A configured agent of the profile no rule of the rows' policy names.
const REVIEWER: &str = "reviewer-a";

const MOB_CONFIG: &str = r#"
[mob]
id = "meta-identity-methods-test"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.worker.tools]
comms = true

[profiles.reviewer]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.reviewer.tools]
comms = true
"#;

/// The JSON-RPC code of every refusal of the family.
const REFUSED: i64 = -32058;
const LINE_WAIT: Duration = Duration::from_mins(3);

/// The family's methods, in the order `meta/capabilities` lists them.
const METHODS: [&str; 13] = [
    "meta/identity/adopt",
    "meta/identity/get",
    "meta/identity/list",
    "meta/identity/operation",
    "meta/identity/create",
    "meta/identity/update",
    "meta/identity/archive",
    "meta/identity/unarchive",
    "meta/identity/remove",
    "meta/identity/withdraw",
    "meta/identity/changes_hold",
    "meta/identity/directory",
    "meta/identity/plan",
];

/// The rows' policy: the stdio host may make every change of an agent of
/// the profile `worker`, at once. Every rule selects by target profile, so
/// nothing is said of an agent of the profile `reviewer`: its changes get
/// the default, deny.
fn worker_policy() -> Value {
    json!({
        "enabled": true,
        "approval": {"rules": [{
            "change": ["create", "descriptive", "capability", "peers", "profile", "archive",
                       "remove", "adopt"],
            "requester": {"kind": "host", "surface": "rpc"},
            "target": {"profile": ["worker"]},
            "outcome": {"kind": "allow"},
        }]},
        "peers": {"default": "allow"},
    })
}

/// One `rpc_gateway` process on a pipe. Requests go to its stdin; a reader
/// thread hands every stdout line to the row, and the row answers the
/// gateway's roster callback as the SDK host.
struct Gateway {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<String>,
    next_id: u64,
}

impl Gateway {
    fn start(scratch: &Path) -> Self {
        let stderr = File::create(scratch.join(format!(
            "gateway-{}.stderr",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        )))
        .expect("gateway stderr file");
        let mut child = Command::new(env!("CARGO_BIN_EXE_rpc_gateway"))
            .arg("--persistent")
            .env("OPENAI_API_KEY", "sk-regression-test")
            .env("XDG_STATE_HOME", scratch.join("xdg-state"))
            .env("XDG_CONFIG_HOME", scratch.join("xdg-config"))
            .current_dir(scratch)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(stderr))
            .spawn()
            .expect("spawn rpc_gateway");
        let stdin = child.stdin.take().expect("gateway stdin");
        let stdout = child.stdout.take().expect("gateway stdout");
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            stdin,
            lines,
            next_id: 0,
        }
    }

    fn send(&mut self, message: &Value) {
        writeln!(self.stdin, "{message}").expect("write");
        self.stdin.flush().expect("flush");
    }

    /// Send `method` and read stdout until its answer, answering the
    /// gateway's roster callback on the way.
    fn rpc(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = format!("identity-{}", self.next_id);
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let line = self
                .lines
                .recv_timeout(LINE_WAIT)
                .unwrap_or_else(|_| panic!("no answer to {method}"));
            let Some(message) = line
                .find('{')
                .and_then(|start| serde_json::from_str::<Value>(line[start..].trim()).ok())
            else {
                continue;
            };
            if message.get("id") == Some(&json!(id)) && message.get("method").is_none() {
                return message;
            }
            if let (Some(callback), Some(callback_id)) = (
                message.get("method").and_then(Value::as_str),
                message.get("id").cloned(),
            ) {
                let reply = match callback {
                    "callback/roster_provider/roster" => json!({
                        "jsonrpc": "2.0",
                        "id": callback_id,
                        "result": [
                            {"identity": HOST_A, "profile": "worker",
                             "display_name": "Host A",
                             "additional_instructions": ["Keep the ledger."],
                             "labels": {"desk": "east"}},
                            {"identity": DESK, "profile": "worker"},
                            {"identity": REVIEWER, "profile": "reviewer"},
                        ],
                    }),
                    other => json!({
                        "jsonrpc": "2.0",
                        "id": callback_id,
                        "error": {"code": -32601, "message": format!("no handler for {other}")},
                    }),
                };
                self.send(&reply);
            }
        }
    }

    /// `meta/init` on `state` with the identity roster and, when the row
    /// gives one, the identity policy.
    fn init(&mut self, state: &Path, policy: Option<Value>) -> Value {
        let mut runtime_options = json!({"demo_llm": true});
        if let Some(policy) = policy {
            runtime_options["identity_creation"] = policy;
        }
        self.rpc(
            "meta/init",
            json!({
                "mob_config": MOB_CONFIG,
                "persistent_state": state,
                "has_roster_provider": true,
                "runtime_options": runtime_options,
            }),
        )
    }

    /// A gateway launched on `scratch` with `policy`, its init accepted.
    fn launched(scratch: &Path, policy: Option<Value>) -> Self {
        let mut gateway = Self::start(scratch);
        let init = gateway.init(&scratch.join("state"), policy);
        assert!(
            init["error"].is_null() && init["result"]["contract_version"].is_string(),
            "the launch is accepted: {init:#}"
        );
        gateway
    }

    /// The result of `method`, which must answer one.
    fn ok(&mut self, method: &str, params: Value) -> Value {
        let answer = self.rpc(method, params.clone());
        assert!(
            answer["error"].is_null(),
            "{method} {params} answers a result: {answer:#}"
        );
        answer["result"].clone()
    }

    /// The refusal of `method`: the `data` of the family's one error.
    fn refused(&mut self, method: &str, params: Value) -> Value {
        let answer = self.rpc(method, params.clone());
        assert_eq!(
            answer["error"]["code"],
            json!(REFUSED),
            "{method} {params} is refused with the family's code: {answer:#}"
        );
        let data = answer["error"]["data"].clone();
        assert_eq!(
            answer["error"]["message"], data["detail"],
            "the error's message is the refusal's sentence: {answer:#}"
        );
        assert!(
            data["retryable"].is_boolean() && data["detail"].is_string(),
            "a refusal says whether it is retryable and what to do: {answer:#}"
        );
        data
    }

    fn get(&mut self, name: &str) -> Value {
        self.ok("meta/identity/get", json!({"name": name}))
    }

    /// The agents a list names, by name.
    fn listed(&mut self, params: Value) -> Vec<Value> {
        self.ok("meta/identity/list", params)["agents"]
            .as_array()
            .expect("a list names agents")
            .clone()
    }

    fn status(&mut self, name: &str) -> Value {
        self.ok("meta/status_identity", json!({"identity": name}))
    }

    /// One tracked turn of `name`, waited until it completes.
    fn turn(&mut self, name: &str, text: &str) {
        let sent = self.ok(
            "meta/send",
            json!({"identity": name, "content": text, "track_turn": true}),
        );
        let ticket = sent["turn"]["ticket"]
            .as_str()
            .unwrap_or_else(|| panic!("an untracked turn: {sent}"))
            .to_string();
        let waited = self.ok(
            "meta/wait_for_turn",
            json!({"identity": name, "ticket": ticket, "timeout_ms": 90_000}),
        );
        assert_eq!(
            waited["state"],
            json!("completed"),
            "the turn {text:?} completes: {waited}"
        );
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn named<'a>(agents: &'a [Value], name: &str) -> &'a Value {
    agents
        .iter()
        .find(|agent| agent["name"] == json!(name))
        .unwrap_or_else(|| panic!("the list names {name}: {agents:?}"))
}

/// A launch without an identity policy serves no method of the family and
/// lists none: every one answers method not found, and the configured
/// agent is as it was.
#[test]
fn a_launch_without_an_identity_policy_serves_no_identity_method() {
    let scratch = tempfile::tempdir().unwrap();
    let mut gateway = Gateway::launched(scratch.path(), None);
    let capabilities = gateway.ok("meta/capabilities", json!({}));
    let advertised = capabilities["methods"].as_array().expect("methods");
    for method in METHODS {
        assert!(
            !advertised.contains(&json!(method)),
            "{method} is not advertised without a policy"
        );
        let answer = gateway.rpc(method, json!({}));
        assert_eq!(
            answer["error"]["code"],
            json!(-32601),
            "{method}: {answer:#}"
        );
    }
    let status = gateway.status(HOST_A);
    assert_eq!(status["identity"], json!(HOST_A), "{status:#}");
}

/// The launch's identity policy reaches the family: the gateway lists the
/// family's thirteen methods, and its reads answer the agents the host
/// configures.
#[test]
fn the_launch_policy_reaches_the_family_and_its_methods_are_advertised() {
    let scratch = tempfile::tempdir().unwrap();
    let mut gateway = Gateway::launched(scratch.path(), Some(worker_policy()));
    let capabilities = gateway.ok("meta/capabilities", json!({}));
    let advertised = capabilities["methods"].as_array().expect("methods");
    for method in METHODS {
        assert!(
            advertised.contains(&json!(method)),
            "{method} is advertised"
        );
    }
    let agents = gateway.listed(json!({}));
    let names: Vec<&str> = agents
        .iter()
        .filter_map(|agent| agent["name"].as_str())
        .collect();
    assert_eq!(names, [DESK, HOST_A, REVIEWER], "in name order: {agents:?}");
}

/// A list and a get read a configured agent the way they read every agent:
/// its name, display name, profile, spec digest, readiness, and in full its
/// instructions, context and labels, with no word of where its record
/// lives. No read wakes it: it is dormant before and after every read.
#[test]
fn get_and_list_read_a_configured_agent_and_wake_nothing() {
    let scratch = tempfile::tempdir().unwrap();
    let mut gateway = Gateway::launched(scratch.path(), Some(worker_policy()));
    let before = gateway.listed(json!({}));
    let host_a = named(&before, HOST_A).clone();
    assert_eq!(host_a["display_name"], json!("Host A"), "{host_a:#}");
    assert_eq!(host_a["profile"], json!("worker"));
    assert_eq!(host_a["archived"], json!(false));
    assert_eq!(host_a["readiness"], json!("dormant"), "{host_a:#}");
    let digest = host_a["spec_digest"].as_str().expect("a spec digest");
    assert!(digest.starts_with("sha256:"), "{digest}");
    for absent in [
        "provenance",
        "spec_revision",
        "open_operations",
        "origin",
        "custody",
    ] {
        assert!(host_a.get(absent).is_none(), "{absent}: {host_a:#}");
    }

    let record = gateway.get(HOST_A);
    assert_eq!(record["name"], json!(HOST_A), "{record:#}");
    assert_eq!(
        record["spec_digest"],
        json!(digest),
        "one digest: {record:#}"
    );
    assert_eq!(record["instructions"], json!(["Keep the ledger."]));
    assert_eq!(record["labels"], json!({"desk": "east"}));
    assert_eq!(record["peers"], json!([]));
    assert!(record.get("context").is_none(), "{record:#}");
    assert!(record.get("spec").is_none(), "no internal spec: {record:#}");

    // The filters of a list, and the other reads.
    let workers = gateway.listed(json!({"profile": "worker"}));
    assert_eq!(workers.len(), 2, "{workers:?}");
    let after_desk = gateway.listed(json!({"after": DESK}));
    assert_eq!(after_desk.len(), 2, "{after_desk:?}");
    let directory = gateway.ok("meta/identity/directory", json!({}));
    let profiles: Vec<&str> = directory["profiles"]
        .as_array()
        .expect("profiles")
        .iter()
        .filter_map(|profile| profile["profile"].as_str())
        .collect();
    assert_eq!(profiles, ["reviewer", "worker"], "{directory:#}");
    let unknown = gateway.ok(
        "meta/identity/operation",
        json!({"request_id": "never-asked"}),
    );
    assert_eq!(unknown["outcome"], json!("unknown"), "{unknown:#}");
    assert!(
        unknown["operation_id"]
            .as_str()
            .is_some_and(|id| id.starts_with("host.") && id.ends_with(".never-asked")),
        "{unknown:#}"
    );

    let after = gateway.listed(json!({}));
    for name in [HOST_A, DESK, REVIEWER] {
        assert_eq!(
            named(&after, name)["readiness"],
            json!("dormant"),
            "no read woke {name}: {after:?}"
        );
    }
    let missing = gateway.refused("meta/identity/get", json!({"name": "nobody"}));
    assert_eq!(missing["kind"], json!("identity_unknown"), "{missing:#}");
    assert_eq!(missing["field"], json!("name"), "{missing:#}");
}

/// An adoption releases a configured agent from configuration and keeps
/// everything it is: its member, its generation, its session and its
/// history. The same request answers the same outcome again; the operation
/// reads back by the caller's request id and by the outcome's operation id;
/// and the agent's record now says how it entered.
#[test]
fn an_adopt_keeps_the_agents_member_session_and_history_and_replays() {
    let scratch = tempfile::tempdir().unwrap();
    let mut gateway = Gateway::launched(scratch.path(), Some(worker_policy()));
    gateway.turn(HOST_A, "remember the number 41");
    let before = gateway.status(HOST_A);
    let session = before["session_id"]
        .as_str()
        .expect("a session")
        .to_string();
    let record = gateway.get(HOST_A);
    assert_eq!(
        record["readiness"],
        json!("live"),
        "a turn woke it: {record:#}"
    );
    let digest = record["spec_digest"].clone();

    let request = json!({"request_id": "adopt-1", "name": HOST_A, "expected_spec_digest": digest});
    let adopted = gateway.ok("meta/identity/adopt", request.clone());
    assert_eq!(adopted["outcome"], json!("done"), "{adopted:#}");
    let agent = &adopted["agents"][0];
    assert_eq!(agent["name"], json!(HOST_A), "{adopted:#}");
    assert_eq!(agent["state"], json!("committed"), "{adopted:#}");
    assert_eq!(agent["spec_digest"], digest, "an adoption moves no digest");
    assert_eq!(agent["spec_revision"], json!(1), "{adopted:#}");
    assert_eq!(agent["session_id"], json!(session), "{adopted:#}");
    let operation_id = adopted["operation_id"].as_str().expect("an operation id");
    assert!(operation_id.ends_with(".adopt-1"), "{operation_id}");

    let after = gateway.status(HOST_A);
    for kept in ["session_id", "generation", "agent_runtime_id", "member_id"] {
        assert_eq!(after[kept], before[kept], "the adoption keeps {kept}");
    }
    let record = gateway.get(HOST_A);
    assert_eq!(record["spec_digest"], digest, "{record:#}");
    assert_eq!(record["spec_revision"], json!(1), "{record:#}");
    assert_eq!(record["instructions"], json!(["Keep the ledger."]));
    assert_eq!(
        record["provenance"]["entered"],
        json!("adopted"),
        "{record:#}"
    );
    assert_eq!(record["provenance"]["requester_kind"], json!("host"));
    assert_eq!(record["provenance"]["requester"], json!("rpc"));
    assert_eq!(record["provenance"]["consent"], json!({"form": "allowed"}));
    // The conversation goes on where it was.
    gateway.turn(HOST_A, "which number did I give you?");
    assert_eq!(gateway.status(HOST_A)["session_id"], json!(session));

    // The same request again: the same outcome, and nothing new.
    let again = gateway.ok("meta/identity/adopt", request);
    assert_eq!(again, adopted, "a retry replays");
    for read in [
        json!({"request_id": "adopt-1"}),
        json!({"operation_id": operation_id}),
    ] {
        let outcome = gateway.ok("meta/identity/operation", read.clone());
        assert_eq!(outcome, adopted, "{read}");
    }
    // Another request under the same id is refused, and so is another
    // adoption of the agent: there is nothing left to adopt.
    let reused = gateway.refused(
        "meta/identity/adopt",
        json!({"request_id": "adopt-1", "name": DESK, "expected_spec_digest": digest}),
    );
    assert_eq!(reused["kind"], json!("operation_reused"), "{reused:#}");
    let nothing = gateway.refused(
        "meta/identity/adopt",
        json!({"request_id": "adopt-2", "name": HOST_A, "expected_spec_digest": digest}),
    );
    assert_eq!(nothing["kind"], json!("not_adoptable"), "{nothing:#}");
    assert_eq!(nothing["reason"], json!("nothing_to_adopt"), "{nothing:#}");
}

/// One spec digest on every surface: the digest a list and a get name, the
/// digest an outcome names and the digest a `stale_spec` refusal names
/// agree for an agent whose name the member id encoding rewrites, before
/// and after its adoption. The adoption of a dormant agent wakes nothing.
#[test]
fn one_spec_digest_names_an_agent_before_and_after_its_adoption() {
    let scratch = tempfile::tempdir().unwrap();
    let mut gateway = Gateway::launched(scratch.path(), Some(worker_policy()));
    let listed = gateway.listed(json!({}));
    let digest = named(&listed, DESK)["spec_digest"].clone();
    assert_eq!(gateway.get(DESK)["spec_digest"], digest);
    let stale = "sha256:0000000000000000000000000000000000000000000000000000000000000000";
    let refused = gateway.refused(
        "meta/identity/adopt",
        json!({"request_id": "desk-stale", "name": DESK, "expected_spec_digest": stale}),
    );
    assert_eq!(refused["kind"], json!("stale_spec"), "{refused:#}");
    assert_eq!(
        refused["field"],
        json!("expected_spec_digest"),
        "{refused:#}"
    );
    assert_eq!(refused["current_digest"], digest, "{refused:#}");
    assert_eq!(refused["retryable"], json!(false), "{refused:#}");

    let adopted = gateway.ok(
        "meta/identity/adopt",
        json!({"request_id": "desk-1", "name": DESK, "expected_spec_digest": digest}),
    );
    assert_eq!(adopted["agents"][0]["name"], json!(DESK), "{adopted:#}");
    assert_eq!(adopted["agents"][0]["spec_digest"], digest, "{adopted:#}");
    let listed = gateway.listed(json!({}));
    let desk = named(&listed, DESK);
    assert_eq!(desk["spec_digest"], digest, "{desk:#}");
    assert_eq!(
        desk["readiness"],
        json!("dormant"),
        "nothing woke: {desk:#}"
    );
    assert_eq!(gateway.get(DESK)["spec_digest"], digest);
}

/// What an adoption is refused for, each as the family's one error with
/// its kind and the request field it concerns: a change the host's rules do
/// not allow (no rule means deny), a name no agent holds, a request with no
/// request id, one with a member the method does not take, and an operation
/// named twice or not at all.
#[test]
fn adopt_and_operation_refusals_carry_the_code_the_kind_and_the_field() {
    let scratch = tempfile::tempdir().unwrap();
    let mut gateway = Gateway::launched(scratch.path(), Some(worker_policy()));
    let digest = gateway.get(REVIEWER)["spec_digest"].clone();
    let denied = gateway.refused(
        "meta/identity/adopt",
        json!({"request_id": "r-1", "name": REVIEWER, "expected_spec_digest": digest}),
    );
    assert_eq!(denied["kind"], json!("change_denied"), "{denied:#}");
    assert_eq!(denied["change"], json!("adopt"), "{denied:#}");
    assert_eq!(denied["retryable"], json!(false), "{denied:#}");
    let record = gateway.get(REVIEWER);
    assert!(
        record.get("provenance").is_none(),
        "nothing was adopted: {record:#}"
    );

    let unknown = gateway.refused(
        "meta/identity/adopt",
        json!({"request_id": "r-2", "name": "nobody", "expected_spec_digest": digest}),
    );
    assert_eq!(unknown["kind"], json!("identity_unknown"), "{unknown:#}");
    assert_eq!(unknown["field"], json!("name"), "{unknown:#}");

    let unnamed = gateway.refused(
        "meta/identity/adopt",
        json!({"name": HOST_A, "expected_spec_digest": digest}),
    );
    assert_eq!(unnamed["kind"], json!("invalid_request"), "{unnamed:#}");
    assert_eq!(unnamed["field"], json!("request_id"), "{unnamed:#}");

    let malformed = gateway.refused(
        "meta/identity/adopt",
        json!({"request_id": "r-3", "identity": HOST_A, "expected_spec_digest": digest}),
    );
    assert_eq!(malformed["kind"], json!("invalid_request"), "{malformed:#}");
    assert_eq!(malformed["problem"], json!("malformed"), "{malformed:#}");

    for params in [
        json!({}),
        json!({"request_id": "r-1", "operation_id": "host.0.r-1"}),
    ] {
        let refused = gateway.refused("meta/identity/operation", params.clone());
        assert_eq!(
            refused["kind"],
            json!("invalid_request"),
            "{params}: {refused:#}"
        );
    }
    let not_an_id = gateway.refused("meta/identity/operation", json!({"operation_id": "r-1"}));
    assert_eq!(not_an_id["kind"], json!("invalid_request"), "{not_an_id:#}");
    assert_eq!(not_an_id["field"], json!("operation_id"), "{not_an_id:#}");
}

/// A policy with no rule denies every change: the default outcome is deny.
#[test]
fn a_policy_without_a_rule_denies_every_change() {
    let scratch = tempfile::tempdir().unwrap();
    let mut gateway = Gateway::launched(scratch.path(), Some(json!({"enabled": true})));
    let digest = gateway.get(HOST_A)["spec_digest"].clone();
    let denied = gateway.refused(
        "meta/identity/adopt",
        json!({"request_id": "r-1", "name": HOST_A, "expected_spec_digest": digest}),
    );
    assert_eq!(denied["kind"], json!("change_denied"), "{denied:#}");
    assert_eq!(denied["change"], json!("adopt"), "{denied:#}");
    let unknown = gateway.ok("meta/identity/operation", json!({"request_id": "r-1"}));
    assert_eq!(
        unknown["outcome"],
        json!("unknown"),
        "a refused change wrote nothing"
    );
}

/// A policy that names an outcome this release does not carry out is
/// refused at init: `notify`, for which no notice reaches an approver, and
/// `review`, for which no reviewer decides. The refusal names the rule and
/// what works today.
#[test]
fn a_policy_naming_notify_or_review_is_refused_at_init() {
    for (outcome, named) in [
        (
            json!({"kind": "notify", "approver": {"id": "owner"}}),
            "notify",
        ),
        (
            json!({"kind": "review", "approver": {"id": "owner"}, "reviewer": HOST_A}),
            "review",
        ),
    ] {
        let scratch = tempfile::tempdir().unwrap();
        let mut gateway = Gateway::start(scratch.path());
        let init = gateway.init(
            &scratch.path().join("state"),
            Some(json!({
                "enabled": true,
                "approval": {"rules": [{
                    "change": ["create"],
                    "target": {"profile": ["worker"]},
                    "outcome": outcome,
                }]},
            })),
        );
        assert_eq!(init["error"]["code"], json!(-32602), "{named}: {init:#}");
        let message = init["error"]["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("identity_creation.approval.rules[0].outcome.kind")
                && message.contains(named)
                && message.contains("require_approval"),
            "{named}: {message}"
        );
    }
}

// ---------------------------------------------------------------------------
// Create, update, archive, unarchive, remove
// ---------------------------------------------------------------------------

/// A policy for the rows about per-class judgement: the stdio host may
/// create agents of the profile `worker` and change what describes them,
/// and nothing says anything of their peers, so a peers change is denied.
fn descriptive_only_policy() -> Value {
    json!({
        "enabled": true,
        "approval": {"rules": [{
            "change": ["create", "descriptive"],
            "requester": {"kind": "host", "surface": "rpc"},
            "target": {"profile": ["worker"]},
            "outcome": {"kind": "allow"},
        }]},
        "peers": {"default": "allow"},
    })
}

impl Gateway {
    /// Create `agents` as the request `request_id`, which must commit.
    fn create(&mut self, request_id: &str, agents: Value) -> Value {
        let created = self.ok(
            "meta/identity/create",
            json!({"request_id": request_id, "agents": agents}),
        );
        assert_eq!(created["outcome"], json!("done"), "{created:#}");
        created
    }

    /// The spec digest `name` reads now.
    fn digest(&mut self, name: &str) -> Value {
        self.get(name)["spec_digest"].clone()
    }
}

fn outcome_agent<'a>(outcome: &'a Value, name: &str) -> &'a Value {
    outcome["agents"]
        .as_array()
        .and_then(|agents| agents.iter().find(|agent| agent["name"] == json!(name)))
        .unwrap_or_else(|| panic!("the outcome names {name}: {outcome:#}"))
}

/// A create commits its agents together, seats each dormant and wakes
/// nothing: every one reads `seated` and `dormant`, with the spec the
/// request gave it and who asked for it. The agents are there after a
/// restart whose roster never lists them. The same request answers the same
/// outcome again, and another request under its id is `operation_reused`.
#[test]
fn a_create_seats_dormant_agents_wakes_nothing_and_survives_a_restart() {
    let scratch = tempfile::tempdir().unwrap();
    let mut gateway = Gateway::launched(scratch.path(), Some(worker_policy()));
    let agents = json!([
        {"name": "scout", "profile": "worker", "display_name": "Scout",
         "instructions": ["Watch the queue."], "labels": {"shift": "night"},
         "context": {"queue": "inbound"}, "peers": ["ledger", HOST_A]},
        {"name": "ledger", "profile": "worker"},
    ]);
    let created = gateway.create("create-1", agents.clone());
    assert_eq!(
        created["agents"].as_array().map(Vec::len),
        Some(2),
        "{created:#}"
    );
    for name in ["scout", "ledger"] {
        let agent = outcome_agent(&created, name);
        assert_eq!(agent["state"], json!("committed"), "{created:#}");
        assert_eq!(agent["seat"], json!("seated"), "{created:#}");
        assert_eq!(agent["spec_revision"], json!(1), "{created:#}");
        assert!(agent["session_id"].is_string(), "{created:#}");
    }

    let listed = gateway.listed(json!({}));
    for name in ["scout", "ledger", HOST_A] {
        assert_eq!(
            named(&listed, name)["readiness"],
            json!("dormant"),
            "the create woke nothing: {listed:?}"
        );
    }
    let scout = gateway.get("scout");
    assert_eq!(
        scout["spec_digest"],
        outcome_agent(&created, "scout")["spec_digest"]
    );
    assert_eq!(scout["display_name"], json!("Scout"), "{scout:#}");
    assert_eq!(scout["instructions"], json!(["Watch the queue."]));
    assert_eq!(scout["labels"], json!({"shift": "night"}));
    assert_eq!(scout["context"], json!({"queue": "inbound"}));
    assert_eq!(scout["peers"], json!([HOST_A, "ledger"]), "{scout:#}");
    assert_eq!(
        scout["provenance"]["entered"],
        json!("created"),
        "{scout:#}"
    );
    assert_eq!(scout["provenance"]["requester_kind"], json!("host"));
    assert_eq!(scout["provenance"]["requester"], json!("rpc"));
    assert_eq!(scout["provenance"]["consent"], json!({"form": "allowed"}));
    for absent in ["origin", "custody", "created_by"] {
        assert!(scout.get(absent).is_none(), "{absent}: {scout:#}");
    }

    // The same request again, and another request under its id.
    let again = gateway.ok(
        "meta/identity/create",
        json!({"request_id": "create-1", "agents": agents}),
    );
    assert_eq!(again, created, "a retry replays");
    let reused = gateway.refused(
        "meta/identity/create",
        json!({"request_id": "create-1",
               "agents": [{"name": "scout", "profile": "worker", "display_name": "Other"},
                          {"name": "ledger", "profile": "worker"}]}),
    );
    assert_eq!(reused["kind"], json!("operation_reused"), "{reused:#}");

    // A restart on the same state: the roster lists neither agent.
    drop(gateway);
    let mut gateway = Gateway::launched(scratch.path(), Some(worker_policy()));
    let listed = gateway.listed(json!({}));
    let after = named(&listed, "scout");
    assert_eq!(after["spec_digest"], scout["spec_digest"], "{after:#}");
    assert_eq!(after["readiness"], json!("dormant"), "{after:#}");
    assert_eq!(gateway.get("scout")["peers"], json!([HOST_A, "ledger"]));
    // It takes its first delivery like any agent.
    gateway.turn("scout", "anything in the queue?");
    assert_eq!(gateway.get("scout")["readiness"], json!("live"));
}

/// What a create is refused for, each with its kind and the field of the
/// request it concerns, and nothing of a refused create exists.
#[test]
fn create_refusals_carry_the_kind_and_the_field_and_write_nothing() {
    let scratch = tempfile::tempdir().unwrap();
    let mut gateway = Gateway::launched(scratch.path(), Some(worker_policy()));
    let refusals = [
        (
            json!([{"name": "a", "profile": "worker"}, {"name": "b", "profile": "nobody"}]),
            "profile_unknown",
            Some("agents[1].profile"),
        ),
        (
            json!([{"name": "a", "profile": "reviewer"}]),
            "change_denied",
            None,
        ),
        (
            json!([{"name": "mk--x", "profile": "worker"}]),
            "invalid_name",
            Some("agents[0].name"),
        ),
        (
            json!([{"name": "a", "profile": "worker", "peers": ["nobody"]}]),
            "peer_unknown",
            Some("agents[0].peers[0]"),
        ),
        (
            json!([{"name": "a", "profile": "worker", "peers": ["a"]}]),
            "peer_not_allowed",
            Some("agents[0].peers[0]"),
        ),
        (
            json!([{"name": "a", "profile": "worker"}, {"name": "a", "profile": "worker"}]),
            "name_taken",
            Some("agents[1].name"),
        ),
        (json!([]), "invalid_request", Some("agents")),
        (
            json!([{"name": "a", "profile": "worker", "labels": {"role": "x"}}]),
            "reserved_key",
            Some("agents[0].labels"),
        ),
    ];
    for (index, (agents, kind, field)) in refusals.into_iter().enumerate() {
        let refused = gateway.refused(
            "meta/identity/create",
            json!({"request_id": format!("refused-{index}"), "agents": agents}),
        );
        assert_eq!(refused["kind"], json!(kind), "{agents}: {refused:#}");
        match field {
            Some(field) => assert_eq!(refused["field"], json!(field), "{agents}: {refused:#}"),
            None => assert!(refused.get("field").is_none(), "{agents}: {refused:#}"),
        }
    }
    // A name a configured agent holds is taken, and so is a member an
    // unknown member of the request: the method takes no such member.
    let taken = gateway.refused(
        "meta/identity/create",
        json!({"request_id": "taken", "agents": [{"name": HOST_A, "profile": "worker"}]}),
    );
    assert_eq!(taken["kind"], json!("name_taken"), "{taken:#}");
    assert_eq!(taken["by"], json!("identity"), "{taken:#}");
    let unknown_member = gateway.refused(
        "meta/identity/create",
        json!({"request_id": "member",
               "agents": [{"name": "a", "profile": "worker", "tools": ["shell"]}]}),
    );
    assert_eq!(
        unknown_member["kind"],
        json!("invalid_request"),
        "{unknown_member:#}"
    );
    let names: Vec<String> = gateway
        .listed(json!({"include_archived": true}))
        .iter()
        .filter_map(|agent| agent["name"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(
        names,
        [DESK, HOST_A, REVIEWER],
        "no refused create wrote an agent"
    );
}

/// An update replaces what it names and keeps the rest: one descriptive
/// label set and another removed keeps every other label; new instructions
/// and a new display name are in the record at once; the digest moves and
/// the revision counts. A stale digest is `stale_spec` with the current
/// one, and the committed update's retry replays.
#[test]
fn an_update_changes_what_it_names_and_keeps_the_rest() {
    let scratch = tempfile::tempdir().unwrap();
    let mut gateway = Gateway::launched(scratch.path(), Some(worker_policy()));
    gateway.create(
        "create-1",
        json!([{"name": "scout", "profile": "worker", "instructions": ["Watch the queue."],
                "labels": {"shift": "night", "desk": "east", "floor": "2"}},
               {"name": "ledger", "profile": "worker"}]),
    );
    let first = gateway.digest("scout");
    let request = json!({
        "request_id": "update-1", "name": "scout", "expected_spec_digest": first,
        "set": {"display_name": "Scout", "instructions": ["Watch the queue.", "Report hourly."],
                "labels": {"shift": "day"}, "labels_remove": ["floor"]},
        "peers_add": ["ledger"],
    });
    let updated = gateway.ok("meta/identity/update", request.clone());
    assert_eq!(updated["outcome"], json!("done"), "{updated:#}");
    let agent = outcome_agent(&updated, "scout");
    assert_eq!(agent["state"], json!("committed"), "{updated:#}");
    assert_eq!(agent["spec_revision"], json!(2), "{updated:#}");
    assert_ne!(agent["spec_digest"], first, "the spec moved: {updated:#}");

    let scout = gateway.get("scout");
    assert_eq!(scout["spec_digest"], agent["spec_digest"], "{scout:#}");
    assert_eq!(scout["display_name"], json!("Scout"));
    assert_eq!(
        scout["instructions"],
        json!(["Watch the queue.", "Report hourly."])
    );
    assert_eq!(
        scout["labels"],
        json!({"shift": "day", "desk": "east"}),
        "{scout:#}"
    );
    assert_eq!(scout["peers"], json!(["ledger"]), "{scout:#}");
    assert_eq!(
        scout["readiness"],
        json!("dormant"),
        "an update wakes nothing: {scout:#}"
    );

    // The retry of the committed update replays; it is never stale.
    let again = gateway.ok("meta/identity/update", request);
    assert_eq!(
        outcome_agent(&again, "scout")["spec_digest"],
        agent["spec_digest"]
    );
    assert_eq!(outcome_agent(&again, "scout")["spec_revision"], json!(2));
    assert_eq!(
        gateway.get("scout")["spec_revision"],
        json!(2),
        "nothing was written twice"
    );

    let stale = gateway.refused(
        "meta/identity/update",
        json!({"request_id": "update-2", "name": "scout", "expected_spec_digest": first,
               "set": {"display_name": "Other"}}),
    );
    assert_eq!(stale["kind"], json!("stale_spec"), "{stale:#}");
    assert_eq!(stale["field"], json!("expected_spec_digest"), "{stale:#}");
    assert_eq!(stale["current_digest"], agent["spec_digest"], "{stale:#}");
    let empty = gateway.refused(
        "meta/identity/update",
        json!({"request_id": "update-3", "name": "scout",
               "expected_spec_digest": agent["spec_digest"]}),
    );
    assert_eq!(empty["kind"], json!("invalid_request"), "{empty:#}");
    assert_eq!(empty["problem"], json!("empty"), "{empty:#}");
    let both = gateway.refused(
        "meta/identity/update",
        json!({"request_id": "update-4", "name": "scout",
               "expected_spec_digest": agent["spec_digest"],
               "set": {"labels": {"desk": "west"}, "labels_remove": ["desk"]}}),
    );
    assert_eq!(both["kind"], json!("invalid_request"), "{both:#}");
    assert_eq!(both["field"], json!("set.labels_remove"), "{both:#}");
    let profile = gateway.refused(
        "meta/identity/update",
        json!({"request_id": "update-5", "name": "scout",
               "expected_spec_digest": agent["spec_digest"], "set": {"profile": "nobody"}}),
    );
    assert_eq!(profile["kind"], json!("profile_unknown"), "{profile:#}");
    assert_eq!(profile["field"], json!("set.profile"), "{profile:#}");
}

/// An update that touches several change classes is judged per class and
/// the strictest outcome applies to the whole request: new instructions
/// (allowed) together with a new peer (no rule: denied) are denied as one,
/// and nothing of the update is applied. The instructions alone commit.
#[test]
fn an_update_is_judged_per_change_class_and_the_strictest_outcome_applies() {
    let scratch = tempfile::tempdir().unwrap();
    let mut gateway = Gateway::launched(scratch.path(), Some(descriptive_only_policy()));
    gateway.create(
        "create-1",
        json!([{"name": "scout", "profile": "worker"}, {"name": "ledger", "profile": "worker"}]),
    );
    let digest = gateway.digest("scout");
    let denied = gateway.refused(
        "meta/identity/update",
        json!({"request_id": "update-1", "name": "scout", "expected_spec_digest": digest,
               "set": {"instructions": ["Report hourly."]}, "peers_add": ["ledger"]}),
    );
    assert_eq!(denied["kind"], json!("change_denied"), "{denied:#}");
    assert_eq!(denied["change"], json!("peers"), "{denied:#}");
    let scout = gateway.get("scout");
    assert_eq!(
        scout["spec_digest"], digest,
        "nothing was applied: {scout:#}"
    );
    assert_eq!(scout["instructions"], json!([]), "{scout:#}");
    assert_eq!(scout["peers"], json!([]), "{scout:#}");

    let updated = gateway.ok(
        "meta/identity/update",
        json!({"request_id": "update-2", "name": "scout", "expected_spec_digest": digest,
               "set": {"instructions": ["Report hourly."]}}),
    );
    assert_eq!(updated["outcome"], json!("done"), "{updated:#}");
    assert_eq!(
        gateway.get("scout")["instructions"],
        json!(["Report hourly."])
    );
    // The other classes no rule names are denied the same way.
    let digest = gateway.digest("scout");
    for (request_id, method, change) in [
        ("archive-1", "meta/identity/archive", "archive"),
        ("remove-1", "meta/identity/remove", "remove"),
    ] {
        let denied = gateway.refused(
            method,
            json!({"request_id": request_id, "name": "scout", "expected_spec_digest": digest}),
        );
        assert_eq!(
            denied["kind"],
            json!("change_denied"),
            "{method}: {denied:#}"
        );
        assert_eq!(denied["change"], json!(change), "{method}: {denied:#}");
    }
}

/// An archive stops an agent and keeps it: it is listed only with archived
/// agents, a message to it is refused `identity_archived`, and its
/// unarchive brings it back on the session it was archived on. A removal
/// is for good: the agent is gone and its name is never available again.
#[test]
fn an_archive_keeps_an_agent_an_unarchive_restores_it_and_a_removal_retires_its_name() {
    let scratch = tempfile::tempdir().unwrap();
    let mut gateway = Gateway::launched(scratch.path(), Some(worker_policy()));
    gateway.create("create-1", json!([{"name": "scout", "profile": "worker"}]));
    gateway.turn("scout", "remember the number 41");
    let session = gateway.status("scout")["session_id"].clone();
    let digest = gateway.digest("scout");
    let named_scout = |request_id: &str| json!({"request_id": request_id, "name": "scout", "expected_spec_digest": digest});

    let not_archived = gateway.refused("meta/identity/unarchive", named_scout("unarchive-0"));
    assert_eq!(
        not_archived["kind"],
        json!("not_archived"),
        "{not_archived:#}"
    );
    assert_eq!(not_archived["field"], json!("name"), "{not_archived:#}");

    let archived = gateway.ok("meta/identity/archive", named_scout("archive-1"));
    let state = outcome_agent(&archived, "scout")["state"].clone();
    assert!(
        state == json!("archived") || state == json!("archiving"),
        "{archived:#}"
    );
    let scout = gateway.get("scout");
    assert_eq!(scout["archived"], json!(true), "{scout:#}");
    assert_eq!(scout["spec_digest"], digest, "an archive moves no digest");
    assert!(
        gateway
            .listed(json!({}))
            .iter()
            .all(|agent| agent["name"] != json!("scout")),
        "a list leaves archived agents out"
    );
    let with_archived = gateway.listed(json!({"include_archived": true}));
    assert_eq!(named(&with_archived, "scout")["archived"], json!(true));
    let refused_send = gateway.rpc(
        "meta/send",
        json!({"identity": "scout", "content": "are you there?"}),
    );
    assert_eq!(
        refused_send["error"]["data"]["kind"],
        json!("identity_archived"),
        "a message to an archived agent is refused: {refused_send:#}"
    );
    assert_eq!(
        gateway.ok("meta/identity/archive", named_scout("archive-1")),
        archived,
        "a retry replays"
    );

    let restored = gateway.ok("meta/identity/unarchive", named_scout("unarchive-1"));
    assert_eq!(
        outcome_agent(&restored, "scout")["state"],
        json!("restored"),
        "{restored:#}"
    );
    assert_eq!(gateway.get("scout")["archived"], json!(false));
    assert_eq!(
        gateway.status("scout")["session_id"],
        session,
        "it is back on the session it was archived on"
    );
    gateway.turn("scout", "which number did I give you?");

    let removed = gateway.ok("meta/identity/remove", named_scout("remove-1"));
    let state = outcome_agent(&removed, "scout")["state"].clone();
    assert!(
        state == json!("removed") || state == json!("retiring"),
        "{removed:#}"
    );
    let gone = gateway.refused("meta/identity/get", json!({"name": "scout"}));
    assert_eq!(gone["kind"], json!("identity_unknown"), "{gone:#}");
    let retaken = gateway.refused(
        "meta/identity/create",
        json!({"request_id": "create-2", "agents": [{"name": "scout", "profile": "worker"}]}),
    );
    assert_eq!(retaken["kind"], json!("name_taken"), "{retaken:#}");
    assert_eq!(retaken["by"], json!("removed"), "{retaken:#}");
    let again = gateway.ok("meta/identity/remove", named_scout("remove-1"));
    assert_eq!(
        outcome_agent(&again, "scout")["name"],
        json!("scout"),
        "a retry of the removal replays: {again:#}"
    );
}

/// An agent the host's configuration declares takes no run-time change but
/// its adoption: an update, an archive and a removal answer
/// `owner_read_only` at `name`, under a rule that allows each, and the
/// agent and its name are untouched. After its adoption the same changes
/// are admitted.
#[test]
fn a_configured_agent_takes_no_run_time_change_but_its_adoption() {
    let scratch = tempfile::tempdir().unwrap();
    let mut gateway = Gateway::launched(scratch.path(), Some(worker_policy()));
    let digest = gateway.digest(HOST_A);
    let named_host = |request_id: &str| json!({"request_id": request_id, "name": HOST_A, "expected_spec_digest": digest});
    let mut update = named_host("update-1");
    update["set"] = json!({"display_name": "Other"});
    for (method, params) in [
        ("meta/identity/update", update),
        ("meta/identity/archive", named_host("archive-1")),
        ("meta/identity/remove", named_host("remove-1")),
    ] {
        let refused = gateway.refused(method, params);
        assert_eq!(
            refused["kind"],
            json!("owner_read_only"),
            "{method}: {refused:#}"
        );
        assert_eq!(refused["field"], json!("name"), "{method}: {refused:#}");
        assert!(
            refused["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains("adopt")),
            "the refusal names the two ways: {refused:#}"
        );
    }
    let record = gateway.get(HOST_A);
    assert_eq!(record["spec_digest"], digest, "{record:#}");
    assert_eq!(record["display_name"], json!("Host A"), "{record:#}");

    gateway.ok("meta/identity/adopt", named_host("adopt-1"));
    let mut update = named_host("update-2");
    update["set"] = json!({"display_name": "Other"});
    let updated = gateway.ok("meta/identity/update", update);
    assert_eq!(updated["outcome"], json!("done"), "{updated:#}");
    assert_eq!(gateway.get(HOST_A)["display_name"], json!("Other"));
}

/// The host holds the mob's changes of agents and releases them: while
/// they are held every change is refused the retryable `changes_held` and
/// nothing is written. A withdrawal of an operation no decision is held for
/// is `decision_not_found`.
#[test]
fn a_changes_hold_refuses_every_change_until_it_is_released() {
    let scratch = tempfile::tempdir().unwrap();
    let mut gateway = Gateway::launched(scratch.path(), Some(worker_policy()));
    let held = gateway.ok(
        "meta/identity/changes_hold",
        json!({"held": true, "reason": "a deploy window"}),
    );
    assert_eq!(held, json!({"held": true}));
    let request = json!({"request_id": "create-1",
                         "agents": [{"name": "scout", "profile": "worker"}]});
    let refused = gateway.refused("meta/identity/create", request.clone());
    assert_eq!(refused["kind"], json!("changes_held"), "{refused:#}");
    assert_eq!(refused["retryable"], json!(true), "{refused:#}");
    let unknown = gateway.ok("meta/identity/operation", json!({"request_id": "create-1"}));
    assert_eq!(
        unknown["outcome"],
        json!("unknown"),
        "nothing was written: {unknown:#}"
    );

    let released = gateway.ok("meta/identity/changes_hold", json!({"held": false}));
    assert_eq!(released, json!({"held": false}));
    let created = gateway.ok("meta/identity/create", request);
    assert_eq!(
        created["outcome"],
        json!("done"),
        "the same request commits: {created:#}"
    );

    let nothing_held = gateway.refused("meta/identity/withdraw", json!({"request_id": "create-1"}));
    assert_eq!(
        nothing_held["kind"],
        json!("decision_not_found"),
        "{nothing_held:#}"
    );
}

/// A plan answers what the host's rules would give a change and the whole
/// rendering a decider would be shown, and writes nothing: the planned
/// agent does not exist afterwards. A change the rules deny is refused as
/// the change itself would be.
#[test]
fn a_plan_answers_the_outcome_and_the_rendering_and_writes_nothing() {
    let scratch = tempfile::tempdir().unwrap();
    let mut gateway = Gateway::launched(scratch.path(), Some(worker_policy()));
    let plan = gateway.ok(
        "meta/identity/plan",
        json!({"change": "create", "request_id": "plan-1",
               "agents": [{"name": "scout", "profile": "worker",
                           "instructions": ["Watch the queue."]}]}),
    );
    assert_eq!(plan["outcome"], json!("allow"), "{plan:#}");
    assert_eq!(plan["change"], json!("create"), "{plan:#}");
    assert!(plan.get("approver_id").is_none(), "{plan:#}");
    let rendering = plan["rendering"].as_str().expect("a rendering");
    assert!(
        rendering.contains("scout") && rendering.contains("Watch the queue."),
        "{rendering}"
    );
    for digest in ["request_digest", "subject_digest"] {
        assert!(
            plan[digest]
                .as_str()
                .is_some_and(|value| value.starts_with("sha256:")),
            "{digest}: {plan:#}"
        );
    }
    let missing = gateway.refused("meta/identity/get", json!({"name": "scout"}));
    assert_eq!(
        missing["kind"],
        json!("identity_unknown"),
        "a plan wrote nothing"
    );
    let unknown = gateway.ok("meta/identity/operation", json!({"request_id": "plan-1"}));
    assert_eq!(unknown["outcome"], json!("unknown"), "{unknown:#}");

    let denied = gateway.refused(
        "meta/identity/plan",
        json!({"change": "create", "request_id": "plan-2",
               "agents": [{"name": "judge", "profile": "reviewer"}]}),
    );
    assert_eq!(denied["kind"], json!("change_denied"), "{denied:#}");
    let unnamed = gateway.refused(
        "meta/identity/plan",
        json!({"request_id": "plan-3", "agents": [{"name": "scout", "profile": "worker"}]}),
    );
    assert_eq!(unnamed["kind"], json!("invalid_request"), "{unnamed:#}");
    assert_eq!(unnamed["field"], json!("change"), "{unnamed:#}");
}

// ---------------------------------------------------------------------------
// A candidate launch, and an older copy of the mob's store
// ---------------------------------------------------------------------------

impl Gateway {
    /// A candidate gateway launched on `scratch`, the state an authoritative
    /// launch left there, with `policy`.
    fn launched_candidate(scratch: &Path, policy: Value) -> Self {
        let mut gateway = Self::start(scratch);
        let init = gateway.rpc(
            "meta/init",
            json!({
                "mob_config": MOB_CONFIG,
                "persistent_state": scratch.join("state"),
                "has_roster_provider": true,
                "runtime_options": {
                    "demo_llm": true,
                    "identity_creation": policy,
                    "mob_composition": {"authority": "candidate"},
                },
            }),
        );
        assert!(
            init["error"].is_null(),
            "the candidate launch is accepted: {init:#}"
        );
        gateway
    }
}

/// Copy the directory `from` to `to`, whole.
fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("create the copy");
    for entry in std::fs::read_dir(from).expect("read the directory") {
        let entry = entry.expect("a directory entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("a file type").is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("copy a file");
        }
    }
}

/// The names a list names, archived agents included.
fn names(gateway: &mut Gateway) -> Vec<String> {
    gateway
        .listed(json!({"include_archived": true}))
        .iter()
        .filter_map(|agent| agent["name"].as_str().map(str::to_owned))
        .collect()
}

/// A candidate launch changes nothing: it refuses the family's eight
/// writers with the candidate refusal, before anything is read or written,
/// and serves its reads.
#[test]
fn a_candidate_launch_refuses_the_writers_and_serves_the_reads() {
    let scratch = tempfile::tempdir().unwrap();
    let mut gateway = Gateway::launched(scratch.path(), Some(worker_policy()));
    gateway.create("create-1", json!([{"name": "scout", "profile": "worker"}]));
    let digest = gateway.digest("scout");
    drop(gateway);

    let mut candidate = Gateway::launched_candidate(scratch.path(), worker_policy());
    let named_scout =
        json!({"request_id": "candidate-1", "name": "scout", "expected_spec_digest": digest});
    let mut update = named_scout.clone();
    update["set"] = json!({"display_name": "Other"});
    for (method, params) in [
        (
            "meta/identity/create",
            json!({"request_id": "candidate-1",
                   "agents": [{"name": "late", "profile": "worker"}]}),
        ),
        ("meta/identity/update", update),
        ("meta/identity/archive", named_scout.clone()),
        ("meta/identity/unarchive", named_scout.clone()),
        ("meta/identity/remove", named_scout.clone()),
        ("meta/identity/adopt", named_scout.clone()),
        (
            "meta/identity/withdraw",
            json!({"request_id": "candidate-1"}),
        ),
        ("meta/identity/changes_hold", json!({"held": true})),
    ] {
        let answer = candidate.rpc(method, params);
        assert_eq!(
            answer["error"]["code"],
            json!(-32047),
            "{method}: {answer:#}"
        );
        assert_eq!(
            answer["error"]["data"]["kind"],
            json!("candidate_launch"),
            "{method}: {answer:#}"
        );
    }
    assert_eq!(
        candidate.get("scout")["spec_digest"],
        digest,
        "the reads answer"
    );
    assert_eq!(names(&mut candidate), [DESK, HOST_A, REVIEWER, "scout"]);
    let unknown = candidate.ok(
        "meta/identity/operation",
        json!({"request_id": "candidate-1"}),
    );
    assert_eq!(unknown["outcome"], json!("unknown"), "{unknown:#}");
}

/// A host that rolls back to an older copy of the mob's store gets agents
/// whose records, members and sessions agree with that copy: an agent
/// created after the copy is absent and its name is free, a removal made
/// after the copy is undone with its tombstone, an update made after the
/// copy is gone, and every agent of the copy takes its next delivery on the
/// session it had.
#[test]
fn an_older_copy_of_the_store_yields_agents_records_and_sessions_that_agree() {
    let scratch = tempfile::tempdir().unwrap();
    let live = scratch.path().join("live");
    let copy = scratch.path().join("copy");
    std::fs::create_dir_all(&live).unwrap();

    // Before the copy: two agents, one of them with a conversation.
    let mut gateway = Gateway::launched(&live, Some(worker_policy()));
    gateway.create(
        "create-1",
        json!([{"name": "keeper", "profile": "worker", "instructions": ["Keep the ledger."],
                "peers": ["drifter"]},
               {"name": "drifter", "profile": "worker"}]),
    );
    gateway.turn("keeper", "remember the number 41");
    let keeper = gateway.get("keeper");
    let drifter = gateway.get("drifter");
    let keeper_session = gateway.status("keeper")["session_id"].clone();
    let drifter_session = gateway.status("drifter")["session_id"].clone();
    let at_copy = names(&mut gateway);
    drop(gateway);
    copy_dir(&live, &copy);

    // After the copy: a create, a removal and an update.
    let mut gateway = Gateway::launched(&live, Some(worker_policy()));
    gateway.create(
        "create-late",
        json!([{"name": "late", "profile": "worker"}]),
    );
    let removed = gateway.ok(
        "meta/identity/remove",
        json!({"request_id": "remove-drifter", "name": "drifter",
               "expected_spec_digest": drifter["spec_digest"]}),
    );
    assert_eq!(removed["outcome"], json!("done"), "{removed:#}");
    let updated = gateway.ok(
        "meta/identity/update",
        json!({"request_id": "update-keeper", "name": "keeper",
               "expected_spec_digest": keeper["spec_digest"],
               "set": {"instructions": ["Keep two ledgers."]}}),
    );
    assert_eq!(updated["outcome"], json!("done"), "{updated:#}");
    drop(gateway);

    // The host puts the older copy back where its state lives, and starts.
    std::fs::rename(&live, scratch.path().join("newer")).expect("set the newer state aside");
    std::fs::rename(&copy, &live).expect("put the older copy back");
    let mut gateway = Gateway::launched(&live, Some(worker_policy()));
    assert_eq!(
        names(&mut gateway),
        at_copy,
        "the agents of the copy, no more and no less"
    );
    let keeper_now = gateway.get("keeper");
    for member in [
        "spec_digest",
        "spec_revision",
        "instructions",
        "peers",
        "provenance",
    ] {
        assert_eq!(
            keeper_now[member], keeper[member],
            "keeper's {member} is the copy's"
        );
    }
    let drifter_now = gateway.get("drifter");
    assert_eq!(
        drifter_now["spec_digest"], drifter["spec_digest"],
        "the removal is undone"
    );
    assert_eq!(drifter_now["archived"], json!(false), "{drifter_now:#}");
    // Records and members agree: each agent of the copy is seated, on the
    // session it had, and takes its next delivery there.
    for (name, session) in [("keeper", &keeper_session), ("drifter", &drifter_session)] {
        assert_eq!(
            &gateway.status(name)["session_id"],
            session,
            "{name}'s session"
        );
        gateway.turn(name, "are you there?");
        assert_eq!(
            &gateway.status(name)["session_id"],
            session,
            "{name} stays on it"
        );
    }
    // What happened after the copy never happened here: its operations are
    // unknown, the late agent is absent, and its name is free.
    for request_id in ["create-late", "remove-drifter", "update-keeper"] {
        let unknown = gateway.ok("meta/identity/operation", json!({"request_id": request_id}));
        assert_eq!(
            unknown["outcome"],
            json!("unknown"),
            "{request_id}: {unknown:#}"
        );
    }
    let absent = gateway.refused("meta/identity/get", json!({"name": "late"}));
    assert_eq!(absent["kind"], json!("identity_unknown"), "{absent:#}");
    let again = gateway.create(
        "create-late",
        json!([{"name": "late", "profile": "worker"}]),
    );
    assert_eq!(
        outcome_agent(&again, "late")["seat"],
        json!("seated"),
        "{again:#}"
    );
    // The undone removal left no tombstone: the agent is removed again, and
    // only then is its name gone for good.
    let removed = gateway.ok(
        "meta/identity/remove",
        json!({"request_id": "remove-drifter", "name": "drifter",
               "expected_spec_digest": drifter["spec_digest"]}),
    );
    assert_eq!(removed["outcome"], json!("done"), "{removed:#}");
    let retaken = gateway.refused(
        "meta/identity/create",
        json!({"request_id": "create-drifter",
               "agents": [{"name": "drifter", "profile": "worker"}]}),
    );
    assert_eq!(retaken["by"], json!("removed"), "{retaken:#}");
}

// ---------------------------------------------------------------------------
// A move to another profile
// ---------------------------------------------------------------------------

/// A policy that lets the stdio host create agents of either profile and
/// move an agent between them.
fn profile_policy() -> Value {
    json!({
        "enabled": true,
        "approval": {"rules": [{
            "change": ["create", "profile"],
            "requester": {"kind": "host", "surface": "rpc"},
            "target": {"profile": ["worker", "reviewer"]},
            "outcome": {"kind": "allow"},
        }]},
    })
}

/// An update of an agent's profile moves the agent to that profile on the
/// same conversation: its record and its member name the new profile, its
/// session is the one it had, and its next turn runs there. A rule that
/// names only the profile the agent has does not admit the move: an update
/// is judged on the agent as it is and as the update leaves it.
#[test]
fn a_profile_update_moves_an_agent_to_the_profile_on_the_same_conversation() {
    let scratch = tempfile::tempdir().unwrap();
    let mut gateway = Gateway::launched(scratch.path(), Some(profile_policy()));
    gateway.create("create-1", json!([{"name": "scout", "profile": "worker"}]));
    gateway.turn("scout", "remember the number 41");
    let before = gateway.status("scout");
    assert_eq!(before["profile"], json!("worker"), "{before:#}");
    let digest = gateway.digest("scout");

    let moved = gateway.ok(
        "meta/identity/update",
        json!({"request_id": "move-1", "name": "scout", "expected_spec_digest": digest,
               "set": {"profile": "reviewer"}}),
    );
    assert_eq!(moved["outcome"], json!("done"), "{moved:#}");
    assert_eq!(
        outcome_agent(&moved, "scout")["spec_revision"],
        json!(2),
        "{moved:#}"
    );
    assert_eq!(gateway.get("scout")["profile"], json!("reviewer"));
    gateway.turn("scout", "which number did I give you?");
    let after = gateway.status("scout");
    assert_eq!(
        after["profile"],
        json!("reviewer"),
        "its member runs the profile: {after:#}"
    );
    assert_eq!(
        after["session_id"], before["session_id"],
        "on the same conversation"
    );
    drop(gateway);

    // Under a rule that names the profile `worker` alone the same move is
    // denied: the agent as the update leaves it is of a profile no rule
    // names.
    let other = tempfile::tempdir().unwrap();
    let mut policy = worker_policy();
    policy["approval"]["rules"][0]["change"] = json!(["create", "profile"]);
    let mut gateway = Gateway::launched(other.path(), Some(policy));
    gateway.create("create-1", json!([{"name": "scout", "profile": "worker"}]));
    let digest = gateway.digest("scout");
    let denied = gateway.refused(
        "meta/identity/update",
        json!({"request_id": "move-1", "name": "scout", "expected_spec_digest": digest,
               "set": {"profile": "reviewer"}}),
    );
    assert_eq!(denied["kind"], json!("change_denied"), "{denied:#}");
    assert_eq!(denied["change"], json!("profile"), "{denied:#}");
    assert_eq!(
        gateway.get("scout")["profile"],
        json!("worker"),
        "nothing moved"
    );
}

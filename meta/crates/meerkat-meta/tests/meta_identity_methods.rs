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
                            {"identity": DESK, "profile": "worker",
                             "labels": {"team": "core", "shift": "day", "floor": "2",
                                        "zone": "b"}},
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
        self.init_mob(state, MOB_CONFIG, policy)
    }

    /// [`Self::init`] with the mob configuration `mob_config`.
    fn init_mob(&mut self, state: &Path, mob_config: &str, policy: Option<Value>) -> Value {
        let mut runtime_options = json!({"demo_llm": true});
        if let Some(policy) = policy {
            runtime_options["identity_creation"] = policy;
        }
        self.rpc(
            "meta/init",
            json!({
                "mob_config": mob_config,
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

/// The verdict of one directory entry.
fn verdict_of(entry: &Value) -> &str {
    entry["verdict"]
        .as_str()
        .unwrap_or_else(|| panic!("a directory entry carries its verdict flat: {entry:#}"))
}

/// A launch without an identity policy, and one whose policy is not
/// enabled, list the family's methods and serve its reads. Every change and
/// every plan is refused `change_denied`, with a sentence that says the
/// launch gives no identity creation policy; the directory answers every
/// entry not allowed; nothing is written and no method answers method not
/// found.
#[test]
fn a_launch_without_an_enabled_identity_policy_serves_the_reads_and_refuses_every_change() {
    for policy in [None, Some(json!({"enabled": false}))] {
        let scratch = tempfile::tempdir().unwrap();
        let mut gateway = Gateway::launched(scratch.path(), policy.clone());
        let capabilities = gateway.ok("meta/capabilities", json!({}));
        let advertised = capabilities["methods"].as_array().expect("methods");
        for method in METHODS {
            assert!(
                advertised.contains(&json!(method)),
                "{method} is advertised under {policy:?}"
            );
        }

        // The reads answer.
        let agents = gateway.listed(json!({"include_archived": true}));
        let digest = named(&agents, HOST_A)["spec_digest"].clone();
        assert_eq!(gateway.get(HOST_A)["spec_digest"], digest);
        let directory = gateway.ok("meta/identity/directory", json!({}));
        let profiles = directory["profiles"].as_array().expect("profiles");
        let peers = directory["peers"].as_array().expect("peers");
        assert_eq!(profiles.len(), 2, "{directory:#}");
        assert_eq!(peers.len(), 3, "{directory:#}");
        for entry in profiles.iter().chain(peers) {
            assert_eq!(verdict_of(entry), "not_allowed", "{directory:#}");
        }
        let unknown = gateway.ok(
            "meta/identity/operation",
            json!({"request_id": "never-asked"}),
        );
        assert_eq!(unknown["outcome"], json!("unknown"), "{unknown:#}");

        // Every change is refused, and so is its plan.
        let one = json!({"name": HOST_A, "expected_spec_digest": digest});
        let changes = [
            (
                "create",
                json!({"agents": [{"name": "scout", "profile": "worker"}]}),
                "create",
            ),
            (
                "update",
                json!({"name": HOST_A, "expected_spec_digest": digest,
                       "set": {"display_name": "Other"}}),
                "descriptive",
            ),
            (
                "update",
                json!({"name": HOST_A, "expected_spec_digest": digest,
                       "set": {"profile": "reviewer"}}),
                "profile",
            ),
            (
                "update",
                json!({"name": HOST_A, "expected_spec_digest": digest, "peers_add": [DESK]}),
                "peers",
            ),
            ("archive", one.clone(), "archive"),
            ("unarchive", one.clone(), "archive"),
            ("remove", one.clone(), "remove"),
            ("adopt", one.clone(), "adopt"),
        ];
        let no_policy = |data: &Value, class: &str, asked: &str| {
            assert_eq!(data["kind"], json!("change_denied"), "{asked}: {data:#}");
            assert_eq!(data["change"], json!(class), "{asked}: {data:#}");
            assert_eq!(data["retryable"], json!(false), "{asked}: {data:#}");
            assert!(
                data["detail"]
                    .as_str()
                    .is_some_and(|detail| detail.contains("no identity creation policy")),
                "{asked} says why: {data:#}"
            );
        };
        for (index, (change, body, class)) in changes.iter().enumerate() {
            let mut params = body.clone();
            params["request_id"] = json!(format!("no-policy-{index}"));
            let method = format!("meta/identity/{change}");
            let refused = gateway.refused(&method, params.clone());
            no_policy(&refused, class, &method);
            params["change"] = json!(change);
            let planned = gateway.refused("meta/identity/plan", params);
            no_policy(&planned, class, &format!("the plan of {change}"));
        }
        let withdrawn = gateway.refused(
            "meta/identity/withdraw",
            json!({"request_id": "no-policy-0"}),
        );
        no_policy(&withdrawn, "create", "meta/identity/withdraw");
        let held = gateway.refused("meta/identity/changes_hold", json!({"held": true}));
        no_policy(&held, "create", "meta/identity/changes_hold");

        // Nothing was written: no operation, and the agents are as they were.
        let unknown = gateway.ok(
            "meta/identity/operation",
            json!({"request_id": "no-policy-0"}),
        );
        assert_eq!(unknown["outcome"], json!("unknown"), "{unknown:#}");
        assert_eq!(
            gateway.listed(json!({"include_archived": true})),
            agents,
            "the agents are as they were under {policy:?}"
        );
        let missing = gateway.refused("meta/identity/get", json!({"name": "scout"}));
        assert_eq!(missing["kind"], json!("identity_unknown"), "{missing:#}");
        let status = gateway.status(HOST_A);
        assert_eq!(status["identity"], json!(HOST_A), "{status:#}");
    }
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
    let mut names: Vec<&str> = agents
        .iter()
        .filter_map(|agent| agent["name"].as_str())
        .collect();
    names.sort_unstable();
    assert_eq!(names, [DESK, HOST_A, REVIEWER], "{agents:?}");
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
    // A list is in one stable order, and `after` continues it: the agents
    // after a name are exactly the ones the whole list names after it.
    let order = |agents: &[Value]| -> Vec<String> {
        agents
            .iter()
            .filter_map(|agent| agent["name"].as_str().map(str::to_owned))
            .collect()
    };
    let whole = order(&before);
    assert_eq!(whole.len(), 3, "{before:?}");
    assert_eq!(order(&gateway.listed(json!({}))), whole, "a stable order");
    for (index, name) in whole.iter().enumerate() {
        let after = gateway.listed(json!({"after": name}));
        assert_eq!(order(&after), whole[index + 1..], "after {name}");
    }
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

/// A policy that names a preparation other than `none` is refused at
/// init: no method of this release settles an agent's preparation, so an
/// agent of such a profile would wait for it without end. The refusal names
/// the policy path and says so.
#[test]
fn a_policy_naming_a_preparation_is_refused_at_init() {
    for (mode, rule) in [
        ("required", json!({"mode": "required"})),
        (
            "constructed",
            json!({"mode": "constructed", "construction_deadline_ms": 60_000}),
        ),
    ] {
        let scratch = tempfile::tempdir().unwrap();
        let mut gateway = Gateway::start(scratch.path());
        let init = gateway.init(
            &scratch.path().join("state"),
            Some(json!({
                "enabled": true,
                "preparation": {"worker": rule},
            })),
        );
        assert_eq!(init["error"]["code"], json!(-32602), "{mode}: {init:#}");
        let message = init["error"]["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("identity_creation.preparation.worker"),
            "{mode}: {message}"
        );
        // A required preparation is refused for what it is. A constructed
        // one needs a native host first, which the stdio gateway is not.
        if mode == "required" {
            assert!(
                message.contains("identity_creation.preparation.worker.mode")
                    && message.contains(mode)
                    && message.contains("no method of this release settles"),
                "{mode}: {message}"
            );
        } else {
            assert!(
                message.contains("needs a native host")
                    && !message.contains("no method of this release settles"),
                "{mode}: {message}"
            );
        }
    }
    // The mode `none` is what every profile has without a word: accepted.
    let scratch = tempfile::tempdir().unwrap();
    let mut gateway = Gateway::start(scratch.path());
    let init = gateway.init(
        &scratch.path().join("state"),
        Some(json!({
            "enabled": true,
            "preparation": {"worker": {"mode": "none"}},
        })),
    );
    assert!(init["error"].is_null(), "{init:#}");
}

/// A change whose rule says `require_approval` is held: it answers
/// `pending_decision` with its decision, nothing is created, the operation
/// reads back as held, the same request asked again answers the same
/// decision, and another request under its id is `operation_reused`. Its
/// requester withdraws it: the decision ends, and nothing was written.
#[test]
fn a_change_that_needs_approval_is_held_until_it_is_decided_or_withdrawn() {
    let scratch = tempfile::tempdir().unwrap();
    let policy = json!({
        "enabled": true,
        "approval": {"rules": [{
            "change": ["create"],
            "requester": {"kind": "host", "surface": "rpc"},
            "target": {"profile": ["worker"]},
            "outcome": {"kind": "require_approval", "approver": {"id": "owner"}},
        }]},
    });
    let mut gateway = Gateway::launched(scratch.path(), Some(policy));
    let request = json!({
        "request_id": "held-1",
        "agents": [{"name": "scout", "profile": "worker"}],
    });
    let held = gateway.ok("meta/identity/create", request.clone());
    assert_eq!(held["outcome"], json!("pending_decision"), "{held:#}");
    let decision = held["decision_id"]
        .as_str()
        .expect("a decision id")
        .to_owned();
    assert!(held["deadline_ms"].as_u64().is_some(), "{held:#}");
    assert!(
        held.get("agents").is_none(),
        "nothing was created: {held:#}"
    );
    let missing = gateway.refused("meta/identity/get", json!({"name": "scout"}));
    assert_eq!(missing["kind"], json!("identity_unknown"), "{missing:#}");

    let read = gateway.ok("meta/identity/operation", json!({"request_id": "held-1"}));
    assert_eq!(read["outcome"], json!("pending_decision"), "{read:#}");
    assert_eq!(read["decision_id"], json!(decision), "{read:#}");
    let again = gateway.ok("meta/identity/create", request);
    assert_eq!(
        again["decision_id"],
        json!(decision),
        "one decision: {again:#}"
    );
    let reused = gateway.refused(
        "meta/identity/create",
        json!({"request_id": "held-1", "agents": [{"name": "other", "profile": "worker"}]}),
    );
    assert_eq!(reused["kind"], json!("operation_reused"), "{reused:#}");

    let withdrawn = gateway.ok("meta/identity/withdraw", json!({"request_id": "held-1"}));
    assert_eq!(withdrawn["decision_id"], json!(decision), "{withdrawn:#}");
    assert_eq!(
        withdrawn["operation_id"], held["operation_id"],
        "{withdrawn:#}"
    );
    let closed = gateway.refused("meta/identity/withdraw", json!({"request_id": "held-1"}));
    assert_eq!(closed["kind"], json!("decision_closed"), "{closed:#}");
    let nothing = gateway.refused("meta/identity/withdraw", json!({"request_id": "never"}));
    assert_eq!(nothing["kind"], json!("decision_not_found"), "{nothing:#}");
    let missing = gateway.refused("meta/identity/get", json!({"name": "scout"}));
    assert_eq!(missing["kind"], json!("identity_unknown"), "{missing:#}");
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
    assert_eq!(taken["field"], json!("agents[0].name"), "{taken:#}");
    let second = gateway.refused(
        "meta/identity/create",
        json!({"request_id": "taken-2",
               "agents": [{"name": "fresh", "profile": "worker"},
                          {"name": HOST_A, "profile": "worker"}]}),
    );
    assert_eq!(second["kind"], json!("name_taken"), "{second:#}");
    assert_eq!(second["field"], json!("agents[1].name"), "{second:#}");
    let nothing = gateway.refused("meta/identity/get", json!({"name": "fresh"}));
    assert_eq!(
        nothing["kind"],
        json!("identity_unknown"),
        "all or nothing: {nothing:#}"
    );
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
    assert_eq!(
        names(&mut gateway),
        [DESK, HOST_A, REVIEWER],
        "no refused create wrote an agent"
    );
}

/// An agent without a display name has no `display_name` member on any
/// read: a list, a get and the directory leave the member out, for an agent
/// created at run time and for one its host configures, and write it for an
/// agent that has one.
#[test]
fn an_agent_without_a_display_name_has_no_display_name_member() {
    let scratch = tempfile::tempdir().unwrap();
    let mut gateway = Gateway::launched(scratch.path(), Some(worker_policy()));
    let created = gateway.create(
        "create-1",
        json!([{"name": "plain", "profile": "worker"},
               {"name": "named", "profile": "worker", "display_name": "Named One"}]),
    );
    assert!(
        outcome_agent(&created, "plain")
            .get("display_name")
            .is_none(),
        "{created:#}"
    );
    let listed = gateway.listed(json!({}));
    let directory = gateway.ok("meta/identity/directory", json!({}));
    let peers = directory["peers"].as_array().expect("peers").clone();
    // Created without one, and configured without one.
    for name in ["plain", REVIEWER] {
        assert!(
            named(&listed, name).get("display_name").is_none(),
            "list, {name}: {listed:?}"
        );
        let record = gateway.get(name);
        assert!(
            record.get("display_name").is_none(),
            "get, {name}: {record:#}"
        );
        assert!(
            named(&peers, name).get("display_name").is_none(),
            "directory, {name}: {directory:#}"
        );
    }
    // Created with one, and configured with one.
    for (name, display_name) in [("named", "Named One"), (HOST_A, "Host A")] {
        assert_eq!(
            named(&listed, name)["display_name"],
            json!(display_name),
            "list, {name}"
        );
        assert_eq!(
            gateway.get(name)["display_name"],
            json!(display_name),
            "get, {name}"
        );
        assert_eq!(
            named(&peers, name)["display_name"],
            json!(display_name),
            "directory, {name}"
        );
    }
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

/// An update of descriptive labels writes only what it names: one label
/// set and another removed keep the agent's authority label (a label the
/// policy declares an authority field) and every descriptive label the
/// update does not name. The authority label is no descriptive change: an
/// update that names it under the descriptive rule alone is not admitted.
#[test]
fn an_update_of_descriptive_labels_keeps_every_authority_label_and_every_other_label() {
    let scratch = tempfile::tempdir().unwrap();
    let policy = json!({
        "enabled": true,
        "reserved_keys": {"labels": ["team"]},
        "authority_fields": {"team": {"values": ["core", "field"]}},
        "approval": {"rules": [{
            "change": ["adopt", "descriptive"],
            "requester": {"kind": "host", "surface": "rpc"},
            "target": {"profile": ["worker"]},
            "outcome": {"kind": "allow"},
        }]},
    });
    let mut gateway = Gateway::launched(scratch.path(), Some(policy));
    let configured = gateway.digest(DESK);
    let adopted = gateway.ok(
        "meta/identity/adopt",
        json!({"request_id": "adopt-desk", "name": DESK, "expected_spec_digest": configured}),
    );
    assert_eq!(adopted["outcome"], json!("done"), "{adopted:#}");
    let before = gateway.get(DESK);
    assert_eq!(
        before["labels"],
        json!({"team": "core", "shift": "day", "floor": "2", "zone": "b"}),
        "the adoption keeps the labels: {before:#}"
    );

    let updated = gateway.ok(
        "meta/identity/update",
        json!({"request_id": "labels-1", "name": DESK,
               "expected_spec_digest": before["spec_digest"],
               "set": {"labels": {"shift": "night"}, "labels_remove": ["floor"]}}),
    );
    assert_eq!(updated["outcome"], json!("done"), "{updated:#}");
    let after = gateway.get(DESK);
    assert_eq!(
        after["labels"],
        json!({"team": "core", "shift": "night", "zone": "b"}),
        "the authority label and the unnamed label stay: {after:#}"
    );

    // The authority label is not a descriptive label: the descriptive rule
    // alone admits no change of it, and it stays.
    let authority = gateway.refused(
        "meta/identity/update",
        json!({"request_id": "labels-2", "name": DESK,
               "expected_spec_digest": after["spec_digest"],
               "set": {"labels": {"team": "field"}}}),
    );
    assert_eq!(authority["kind"], json!("change_denied"), "{authority:#}");
    assert_eq!(authority["change"], json!("authority"), "{authority:#}");
    assert_eq!(
        gateway.get(DESK)["labels"],
        json!({"team": "core", "shift": "night", "zone": "b"}),
        "a refused update writes nothing"
    );
}

/// An update removes a peer and keeps the rest: the peer it names is no
/// longer listed, and the other stays.
#[test]
fn an_update_removes_a_peer_and_keeps_the_rest() {
    let scratch = tempfile::tempdir().unwrap();
    let mut gateway = Gateway::launched(scratch.path(), Some(worker_policy()));
    gateway.create(
        "create-1",
        json!([{"name": "scout", "profile": "worker", "peers": ["ledger", "courier"]},
               {"name": "ledger", "profile": "worker"},
               {"name": "courier", "profile": "worker"}]),
    );
    let mut before: Vec<Value> = gateway.get("scout")["peers"]
        .as_array()
        .expect("peers")
        .clone();
    before.sort_by_key(|peer| peer.as_str().map(str::to_owned));
    assert_eq!(before, [json!("courier"), json!("ledger")]);
    let digest = gateway.digest("scout");
    let unwired = gateway.ok(
        "meta/identity/update",
        json!({"request_id": "update-peers", "name": "scout",
               "expected_spec_digest": digest, "peers_remove": ["ledger"]}),
    );
    assert_eq!(unwired["outcome"], json!("done"), "{unwired:#}");
    assert_eq!(gateway.get("scout")["peers"], json!(["courier"]));
    assert_eq!(gateway.get("ledger")["readiness"], json!("dormant"));
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
    // A retry replays the outcome. What became of the agent's schedules is
    // said on the first answer only: this launch has no schedule that
    // targets the agent, and says so there.
    assert_eq!(
        archived["agents"][0]["schedules"],
        json!({"effect": "paused", "count": 0, "schedules": []}),
        "{archived:#}"
    );
    let mut first = archived;
    first["agents"][0]
        .as_object_mut()
        .expect("an agent")
        .remove("schedules");
    assert_eq!(
        gateway.ok("meta/identity/archive", named_scout("archive-1")),
        first,
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
    assert_eq!(
        retaken["field"],
        json!("agents[0].name"),
        "a removed agent's reserved name names its agent of the request: {retaken:#}"
    );
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

/// A name is taken whoever holds it, and the refusal names the agent of
/// the request that asked for it: a live agent's name and an archived
/// agent's name alike.
#[test]
fn a_taken_name_names_its_agent_of_the_request_whoever_holds_it() {
    let scratch = tempfile::tempdir().unwrap();
    let mut gateway = Gateway::launched(scratch.path(), Some(worker_policy()));
    gateway.create(
        "create-1",
        json!([{"name": "scout", "profile": "worker"}, {"name": "ledger", "profile": "worker"}]),
    );
    let ledger = gateway.digest("ledger");
    let archived = gateway.ok(
        "meta/identity/archive",
        json!({"request_id": "archive-1", "name": "ledger", "expected_spec_digest": ledger}),
    );
    assert_eq!(archived["outcome"], json!("done"), "{archived:#}");
    for (request_id, holder) in [("taken-live", "scout"), ("taken-archived", "ledger")] {
        let taken = gateway.refused(
            "meta/identity/create",
            json!({"request_id": request_id,
                   "agents": [{"name": "fresh", "profile": "worker"},
                              {"name": holder, "profile": "worker"}]}),
        );
        assert_eq!(taken["kind"], json!("name_taken"), "{holder}: {taken:#}");
        assert_eq!(
            taken["field"],
            json!("agents[1].name"),
            "{holder}: {taken:#}"
        );
    }
    let nothing = gateway.refused("meta/identity/get", json!({"name": "fresh"}));
    assert_eq!(nothing["kind"], json!("identity_unknown"), "{nothing:#}");
}

/// A change of an agent whose profile is bound to a realm profile is held
/// like any other: the holder reads the realm profile from the realm's own
/// store, so the rendering a person decides on shows what the profile
/// declares. Without that read the change could not be shown, and would be
/// refused.
#[test]
fn a_change_on_a_profile_bound_to_a_realm_profile_is_held() {
    let scratch = tempfile::tempdir().unwrap();
    let state = scratch.path().join("state");
    let mob_config =
        format!("{MOB_CONFIG}\n[profiles.shared]\nrealm_profile = \"shared-worker\"\n");
    // A first launch makes the mob's storage, where the realm's profiles
    // are kept. It builds no agent of the profile, so the realm profile the
    // definition names need not exist yet.
    {
        let mut first = Gateway::start(scratch.path());
        let init = first.init_mob(&state, &mob_config, None);
        assert!(init["error"].is_null(), "{init:#}");
    }
    let shared = meerkat_mob::MobDefinition::from_toml(
        "[mob]\nid = \"realm-profile\"\n\n[profiles.shared]\nmodel = \"gpt-5.5\"\n\
         runtime_mode = \"turn_driven\"\n\n[profiles.shared.tools]\ncomms = true\n",
    )
    .expect("the realm profile's definition")
    .profiles["shared"]
        .as_inline()
        .cloned()
        .expect("an inline profile");
    let realm_profiles =
        meerkat_mob::store::SqliteRealmProfileStore::open(&state.join("event_log.sqlite3"))
            .expect("the mob storage's realm profiles");
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime")
        .block_on(meerkat_mob::RealmProfileStore::create(
            &realm_profiles,
            "shared-worker",
            &shared,
        ))
        .expect("store the realm profile");
    drop(realm_profiles);

    let policy = json!({
        "enabled": true,
        "approval": {"rules": [{
            "change": ["create"],
            "requester": {"kind": "host", "surface": "rpc"},
            "target": {"profile": ["shared"]},
            "outcome": {"kind": "require_approval", "approver": {"id": "owner"}},
        }]},
    });
    let mut gateway = Gateway::start(scratch.path());
    let init = gateway.init_mob(&state, &mob_config, Some(policy));
    assert!(init["error"].is_null(), "{init:#}");
    let held = gateway.ok(
        "meta/identity/create",
        json!({"request_id": "held-shared",
               "agents": [{"name": "courier", "profile": "shared"}]}),
    );
    assert_eq!(held["outcome"], json!("pending_decision"), "{held:#}");
    let plan = gateway.ok(
        "meta/identity/plan",
        json!({"change": "create", "request_id": "plan-shared",
               "agents": [{"name": "runner", "profile": "shared"}]}),
    );
    let rendering = plan["rendering"].as_str().expect("a rendering");
    assert!(
        rendering.contains("shared-worker"),
        "the rendering shows the realm profile: {rendering}"
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

/// The plan of every change shows, through the launch's own family, the
/// tool configuration a decision of it would hold: the agents' tools and
/// the declared configuration of each profile they run on. It guards the
/// rendering of a create, an update, an archive, an unarchive, a removal
/// and an adoption alike, so none of them can lose the block unseen.
#[test]
fn the_plan_of_every_change_shows_the_tool_configuration_a_decision_would_hold() {
    let scratch = tempfile::tempdir().unwrap();
    let mut gateway = Gateway::launched(scratch.path(), Some(worker_policy()));
    gateway.create(
        "create-1",
        json!([{"name": "scout", "profile": "worker"}, {"name": "ledger", "profile": "worker"}]),
    );
    let ledger = gateway.digest("ledger");
    let archived = gateway.ok(
        "meta/identity/archive",
        json!({"request_id": "archive-1", "name": "ledger", "expected_spec_digest": ledger}),
    );
    assert_eq!(archived["outcome"], json!("done"), "{archived:#}");
    let scout = gateway.digest("scout");
    let ledger = gateway.digest("ledger");
    let host_a = gateway.digest(HOST_A);
    let plans = [
        (
            "create",
            json!({"agents": [{"name": "other", "profile": "worker"}]}),
        ),
        (
            "update",
            json!({"name": "scout", "expected_spec_digest": scout,
                   "set": {"display_name": "Scout"}}),
        ),
        (
            "archive",
            json!({"name": "scout", "expected_spec_digest": scout}),
        ),
        (
            "unarchive",
            json!({"name": "ledger", "expected_spec_digest": ledger}),
        ),
        (
            "remove",
            json!({"name": "scout", "expected_spec_digest": scout}),
        ),
        (
            "adopt",
            json!({"name": HOST_A, "expected_spec_digest": host_a}),
        ),
    ];
    for (change, body) in plans {
        let mut params = body;
        params["change"] = json!(change);
        params["request_id"] = json!(format!("plan-{change}"));
        let plan = gateway.ok("meta/identity/plan", params);
        let rendering = plan["rendering"].as_str().expect("a rendering");
        assert!(
            rendering.contains("Agent profiles")
                && rendering.contains("Declaration of profile")
                && rendering.contains("Profile digest"),
            "the plan of {change} shows the tool configuration: {rendering}"
        );
        assert!(
            !rendering.contains("Expected spec digest"),
            "no rendering shows a spec digest ({change}): {rendering}"
        );
    }
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
/// The names of the mob's agents, archived ones included, in name order
/// (a list's own order is the mob's, which is another for a name the mob
/// stores in another form).
fn names(gateway: &mut Gateway) -> Vec<String> {
    let mut names: Vec<String> = gateway
        .listed(json!({"include_archived": true}))
        .iter()
        .filter_map(|agent| agent["name"].as_str().map(str::to_owned))
        .collect();
    names.sort_unstable();
    names
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
        ("meta/identity/adopt", named_scout),
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
// Rows on the gateway's runtime composition, in process: what a stdio
// request cannot show (two requesters, an agent requester, the host's own
// hooks, a first message that does not start, a governed runtime).
// ---------------------------------------------------------------------------

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use meerkat_authorization::clock::HostAuthorizationClock;
use meerkat_authorization::grant_policy::{
    AdmittedWorkPolicyOwner, OperationPolicyOwner, WorkOwnerAllowance,
};
use meerkat_authorization::grants::{LocalGrantAuthority, LocalGrantConfiguration};
use meerkat_authorization::policy::{LocalPolicyAllowance, LocalPolicyPurpose};
use meerkat_authorization::publication::LocalAuthorizationPublication;
use meerkat_authorization_contracts::evidence::EvidenceId;
use meerkat_authorization_contracts::work_association::InputAuthorityAssociation;
use meerkat_client::{LlmError, LlmEvent, LlmRequest};
use meerkat_core::auth::{PrincipalKind, PrincipalRef};
use meerkat_core::service::{CreateSessionRequest, SessionError};
use meerkat_core::types::StopReason;
use meerkat_core::{Message, OperationAuthorizationError, PreparedAuthorizationBinding};
use meerkat_meta::identity_creation::{
    AgentReadiness, AgentRecord, AgentRef, AgentSeat, ChangeRequester, ChangesHoldRequest,
    ConsequenceLine, CreateAgents, GetAgent, HookRefusal, IdentityChangeCall, IdentityChangeSpec,
    IdentityCreationPolicy, IdentityFamily, IdentityFamilyOptions, IdentityHostHooks,
    IdentityOutcome, IdentityRead, IdentityReadAnswer, IdentityRequest, InstructionOutcome,
    InstructionStep, InstructionsChanged, ListAgents, OperationId, OperationState, PeerVisibility,
    ReadDirectory, RequesterKind, Sha256Digest, ValidationRefusal,
};
use meerkat_meta::identity_first::contracts::RosterProvider;
use meerkat_meta::identity_first::{
    AgentAddressability, AgentIdentity, DurableAgentSpec, RosterContext, RosterError,
};
use meerkat_meta::{SessionHook, UnifiedRuntime, UnifiedRuntimeBuilder};
use meerkat_mob::{MobDefinition, ProfileName};
use meerkat_runtime::input_authority::NativeAdmissionError;
use meerkat_runtime::meerkat_machine::NativeGrantWorkConfiguration;

#[path = "support/llm_usage.rs"]
mod llm_usage;

/// A configured agent of the profile whose members have the mob tools.
const PLANNER: &str = "planner-a";
/// Bounds a failure only; a passing row never waits for it.
const TURN_BOUND: Duration = Duration::from_secs(90);

static NEXT_MOB: AtomicU64 = AtomicU64::new(0);

fn in_process_definition(mob_id: &str) -> MobDefinition {
    MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "{mob_id}"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.worker.tools]
comms = true

[profiles.planner]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.planner.tools]
comms = true
mob = true

[profiles.runner]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "autonomous_host"

[profiles.runner.tools]
comms = true
"#
    ))
    .expect("parse the definition")
}

fn fresh_mob_id() -> String {
    format!(
        "meta-identity-methods-{}-{}",
        std::process::id(),
        NEXT_MOB.fetch_add(1, Ordering::Relaxed)
    )
}

fn roster_spec(name: &str, profile: &str) -> DurableAgentSpec {
    DurableAgentSpec {
        identity: AgentIdentity::parse(name).unwrap(),
        profile: ProfileName::from(profile),
        addressability: AgentAddressability::Addressable,
        display_name: None,
        labels: BTreeMap::new(),
        context: None,
        additional_instructions: Vec::new(),
        initial_message: None,
        runtime_mode_override: None,
        backend: None,
        binding: None,
        placement: None,
    }
}

/// The host's roster: one agent of each profile. It never lists an agent a
/// row creates.
struct InProcessRoster;

#[async_trait]
impl RosterProvider for InProcessRoster {
    async fn roster(&self, _context: &RosterContext) -> Result<Vec<DurableAgentSpec>, RosterError> {
        Ok(vec![
            roster_spec(HOST_A, "worker"),
            roster_spec(PLANNER, "planner"),
        ])
    }
}

fn controller() -> meerkat_core::ControllerModelSelection {
    meerkat_core::ControllerModelSelection::new(
        meerkat_core::SessionLlmIdentity {
            model: "gpt-5.5".into(),
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        },
        serde_json::from_value(
            serde_json::json!({"realm": "meta-identity-methods", "account": "recording"}),
        )
        .expect("nonsecret identity of the fixture transport"),
        "recording-profile".into(),
        "recording-transport".into(),
    )
}

/// The members' model: each request's last message is sent on the row's
/// channel, and every request answers.
#[derive(Clone)]
struct RecordingClient {
    asked: tokio::sync::mpsc::UnboundedSender<String>,
}

/// How long a session of the rows takes to be built, in milliseconds:
/// none, but in the row about an agent that starts slowly.
static SESSION_DELAY_MS: AtomicU64 = AtomicU64::new(0);

/// How long the rows' model waits before it answers a turn, in
/// milliseconds: none, but in the row about an agent that starts slowly.
static MODEL_DELAY_MS: AtomicU64 = AtomicU64::new(0);

impl meerkat_client::LlmClient for RecordingClient {
    fn controller_model_selection(&self) -> Option<meerkat_core::ControllerModelSelection> {
        Some(controller())
    }

    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        if let Some(last) = request
            .messages
            .iter()
            .rev()
            .find(|message| !matches!(message, Message::SystemNotice(_)))
        {
            let _ = self
                .asked
                .send(serde_json::to_string(last).unwrap_or_default());
        }
        let [usage, done] =
            llm_usage::usage_then_done(request, meerkat::Provider::OpenAI, StopReason::EndTurn);
        let delay = MODEL_DELAY_MS.load(Ordering::Relaxed);
        Box::pin(async_stream::stream! {
            if delay > 0 {
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
            yield Ok(LlmEvent::TextDelta { delta: "noted".to_string(), meta: None });
            yield Ok(usage);
            yield Ok(done);
        })
    }

    fn provider(&self) -> meerkat::Provider {
        meerkat::Provider::OpenAI
    }

    fn health_check<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), LlmError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }
}

/// The instruction step a row's family is given: it records each update it
/// is asked about and each agent it is told was removed, and answers every
/// update `applies_at_next_run`.
#[derive(Default)]
struct RecordingStep {
    updated: Mutex<Vec<AskedUpdate>>,
    removed: Mutex<Vec<String>>,
}

/// One update the recording step was asked about: the agent, its
/// instructions before and its instructions now.
type AskedUpdate = (String, Vec<String>, Vec<String>);

#[async_trait]
impl InstructionStep for RecordingStep {
    async fn after_update(
        &self,
        _mob: &meerkat_mob::MobHandle,
        change: &InstructionsChanged,
    ) -> InstructionOutcome {
        self.updated.lock().unwrap().push((
            change.identity.clone(),
            change.previous.clone(),
            change.current.clone(),
        ));
        InstructionOutcome::AppliesAtNextRun
    }

    async fn after_removal(&self, _mob: &meerkat_mob::MobHandle, identity: &str) {
        self.removed.lock().unwrap().push(identity.to_owned());
    }
}

/// A host build hook that fails the next session it is asked to create,
/// once armed: the spawn of a new agent's first message then fails.
#[derive(Default)]
struct FailNextSession {
    armed: AtomicBool,
}

#[async_trait]
impl SessionHook for FailNextSession {
    async fn before_create(&self, _req: &mut CreateSessionRequest) -> Result<(), SessionError> {
        let delay = SESSION_DELAY_MS.load(Ordering::Relaxed);
        if delay > 0 {
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }
        if self.armed.swap(false, Ordering::SeqCst) {
            return Err(SessionError::Unsupported(
                "the row fails this session once".to_owned(),
            ));
        }
        Ok(())
    }
}

/// The host's own check: it refuses a request that names the agent
/// `forbidden`, and states one consequence line for every request.
struct NamingHooks;

impl IdentityHostHooks for NamingHooks {
    fn validate(&self, request: &IdentityRequest) -> Result<(), ValidationRefusal> {
        if request.action.identities().contains(&"forbidden") {
            return Err(ValidationRefusal::new(
                "the name forbidden is reserved here",
            ));
        }
        Ok(())
    }

    fn consequences(
        &self,
        _request: &IdentityRequest,
    ) -> Result<Vec<ConsequenceLine>, HookRefusal> {
        Ok(vec![ConsequenceLine::new(
            "This agent can read the ledger.",
        )])
    }
}

/// The in-process rows' policy: every change of an agent of the profile
/// `worker` is allowed to the host, to a person and to an agent of the
/// profile `planner`.
fn in_process_policy() -> Arc<IdentityCreationPolicy> {
    let changes = json!([
        "create",
        "descriptive",
        "capability",
        "peers",
        "profile",
        "archive",
        "remove",
        "adopt"
    ]);
    let rule = |requester: Value| {
        json!({"change": changes, "requester": requester,
               "target": {"profile": ["worker"]}, "outcome": {"kind": "allow"}})
    };
    Arc::new(
        IdentityCreationPolicy::parse(&json!({
            "enabled": true,
            "peers": {"default": "allow"},
            "approval": {"rules": [
                rule(json!({"kind": "host"})),
                rule(json!({"kind": "human"})),
                rule(json!({"kind": "agent", "profile": ["planner", "worker"]})),
            ]},
        }))
        .expect("the policy parses")
        .expect("the policy is enabled"),
    )
}

/// A host-built native grant configuration whose owners refuse: the runtime
/// it is installed on is governed.
struct RefusingOwner;

impl AdmittedWorkPolicyOwner for RefusingOwner {
    fn authorize_admitted_work(
        &self,
        _association: &InputAuthorityAssociation,
        _binding: &PreparedAuthorizationBinding,
        _purpose: LocalPolicyPurpose,
        _now_ms: u64,
    ) -> Result<WorkOwnerAllowance, OperationAuthorizationError> {
        Err(OperationAuthorizationError::Unavailable)
    }
}

impl OperationPolicyOwner for RefusingOwner {
    fn authorize_operation(
        &self,
        _association: &InputAuthorityAssociation,
        _binding: &PreparedAuthorizationBinding,
        _purpose: LocalPolicyPurpose,
        _now_ms: u64,
    ) -> Result<LocalPolicyAllowance, OperationAuthorizationError> {
        Err(OperationAuthorizationError::Unavailable)
    }
}

fn governed_configuration() -> NativeGrantWorkConfiguration {
    let grants = Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: PrincipalRef::in_domain(
                    PrincipalKind::ServiceAccount,
                    "grant-owner",
                    meerkat_core::TrustDomainId::new("meta-identity-methods").expect("domain"),
                )
                .expect("service principal"),
                namespace: EvidenceId::new("meta-identity-methods-grants").expect("namespace"),
                generation: 1,
            },
            LocalAuthorizationPublication::new(),
            Arc::new(HostAuthorizationClock),
        )
        .expect("host grant owner"),
    );
    let ingress: Arc<meerkat_runtime::meerkat_machine::NativeIngressCheck> =
        Arc::new(|_, _, _, _| -> Result<(), NativeAdmissionError> {
            Err(OperationAuthorizationError::Unavailable.into())
        });
    NativeGrantWorkConfiguration {
        grants,
        ingress,
        invocation_owner: Arc::new(RefusingOwner),
        operation_owner: Arc::new(RefusingOwner),
        retained_resume: None,
        kept_original: None,
        require_scoped_destination: true,
        configured_mcp_attachment: None,
    }
}

/// One booted gateway runtime with the family attached.
struct Rig {
    runtime: UnifiedRuntime,
    asked: tokio::sync::mpsc::UnboundedReceiver<String>,
    step: Arc<RecordingStep>,
    fail_next_session: Arc<FailNextSession>,
}

/// Boot the gateway's runtime composition on `state` and attach the rows'
/// policy to it, with the recording instruction step; `governed` installs a
/// host's native authorization owner, and `hooks` the host's own check.
async fn boot(state: &Path, governed: bool, hooks: Option<Arc<dyn IdentityHostHooks>>) -> Rig {
    let (runtime, asked, fail_next_session) = launch(state, governed).await;
    let step = Arc::new(RecordingStep::default());
    let mut options = IdentityFamilyOptions::new()
        .with_instruction_step(Arc::clone(&step) as Arc<dyn InstructionStep>);
    if let Some(hooks) = hooks {
        options = options.with_host_hooks(hooks);
    }
    assert!(
        runtime.attach_identity_policy(in_process_policy(), options),
        "the runtime serves identities and takes the policy"
    );
    Rig {
        runtime,
        asked,
        step,
        fail_next_session,
    }
}

/// The gateway's runtime composition on `state`, before any identity policy
/// is attached to it.
async fn launch(
    state: &Path,
    governed: bool,
) -> (
    UnifiedRuntime,
    tokio::sync::mpsc::UnboundedReceiver<String>,
    Arc<FailNextSession>,
) {
    launch_with(state, &fresh_mob_id(), governed, false).await
}

/// The approver id the rows' approval rule names.
const APPROVERS: &str = "team:owners";
/// The chat person the approver map lets decide for [`APPROVERS`].
const OWNER: &str = "U123";

/// [`launch`], and with `deciders` a launch whose approver map lets the
/// chat person [`OWNER`] decide for [`APPROVERS`] through a declared
/// ingress.
async fn launch_with(
    state: &Path,
    mob_id: &str,
    governed: bool,
    deciders: bool,
) -> (
    UnifiedRuntime,
    tokio::sync::mpsc::UnboundedReceiver<String>,
    Arc<FailNextSession>,
) {
    launch_roster(state, mob_id, governed, deciders, Arc::new(InProcessRoster)).await
}

/// [`launch_with`], over the host's roster `roster`.
async fn launch_roster(
    state: &Path,
    mob_id: &str,
    governed: bool,
    deciders: bool,
    roster: Arc<dyn RosterProvider>,
) -> (
    UnifiedRuntime,
    tokio::sync::mpsc::UnboundedReceiver<String>,
    Arc<FailNextSession>,
) {
    let (asked_tx, asked) = tokio::sync::mpsc::unbounded_channel();
    let fail_next_session = Arc::new(FailNextSession::default());
    let mut builder = UnifiedRuntimeBuilder::default()
        .definition(in_process_definition(mob_id))
        .persistent_state(state)
        .continuity_from_state_dir(state)
        .await
        .expect("open the state-dir identity substrate")
        .roster_provider(roster)
        .identity_runtime_instance_id("meta-identity-methods")
        .session_hook(Arc::clone(&fail_next_session) as Arc<dyn SessionHook>)
        .default_llm_client(Arc::new(RecordingClient { asked: asked_tx }));
    if governed {
        builder = builder.native_grant_work_configuration(governed_configuration());
    }
    if deciders {
        let access: meerkat_meta::AccessControlConfig = toml::from_str(
            r#"
enabled = false

[approvers."team:owners"]
subjects = ["owner@example.org"]
ingress_subjects = [{ ingress = "chat", subject = "U123" }]
"#,
        )
        .expect("the approver map");
        let ingresses: meerkat_meta::DecisionIngressDeclarations =
            serde_json::from_value(json!({"ingresses": [
                {"ingress": "chat", "attests": "person", "max_clock_skew_ms": 5000},
            ]}))
            .expect("ingress declarations");
        builder = builder
            .decision_ingresses(ingresses)
            .access_controller(meerkat_meta::AccessController::new(access).expect("controller"));
    }
    let runtime = Box::pin(builder.build())
        .await
        .expect("build the gateway-shaped UnifiedRuntime");
    (runtime, asked, fail_next_session)
}

fn now_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_millis(),
    )
    .expect("ms")
}

/// The decisions of a Console that asks for no sign-in, read-only when
/// `read_only`.
fn console_decisions(read_only: bool) -> meerkat_meta::RuntimeDecisionState {
    meerkat_meta::build_runtime_decision_state(meerkat_meta::RuntimeDecisionInputs {
        bigquery: meerkat_meta::BigQueryNaming {
            dataset: "meta_identity_methods".to_string(),
            table: "events".to_string(),
        },
        trusted_meta_toml: "modules = []\n".to_string(),
        auth: meerkat_meta::AuthPolicy {
            default_provider: meerkat_meta::AuthProvider::GoogleOAuth,
            email_allowlist: vec![],
        },
        trusted_oidc: meerkat_meta::TrustedOidcRuntimeConfig {
            discovery_json:
                r#"{"issuer":"https://trusted.meta.local","jwks_uri":"https://trusted.meta.local/.well-known/jwks.json"}"#
                    .to_string(),
            jwks_json: r#"{"keys":[{"kid":"kid-current","kty":"oct","alg":"HS256","k":"cGhhc2U3LXRydXN0ZWQtY3VycmVudC1zZWNyZXQ"}]}"#
                .to_string(),
            audience: "meerkat-console".to_string(),
            require_verified_email: false,
            shared_account_tokens: false,
        },
        console: meerkat_meta::ConsolePolicy {
            require_app_auth: false,
            read_only,
            ..meerkat_meta::ConsolePolicy::default()
        },
        ops: meerkat_meta::RuntimeOpsPolicy::default(),
        release_metadata_json: include_str!("../assets/release-targets.json").to_string(),
    })
    .expect("decision state builds")
}

/// The decisions of a Console that asks for sign-in and admits the two
/// persons the rows sign in as.
fn signed_console_decisions() -> meerkat_meta::RuntimeDecisionState {
    meerkat_meta::build_runtime_decision_state(meerkat_meta::RuntimeDecisionInputs {
        bigquery: meerkat_meta::BigQueryNaming {
            dataset: "meta_identity_methods".to_string(),
            table: "events".to_string(),
        },
        trusted_meta_toml: "modules = []\n".to_string(),
        auth: meerkat_meta::AuthPolicy {
            default_provider: meerkat_meta::AuthProvider::GoogleOAuth,
            email_allowlist: vec!["root@example.org".to_string(), "ann@example.org".to_string()],
        },
        trusted_oidc: meerkat_meta::TrustedOidcRuntimeConfig {
            discovery_json:
                r#"{"issuer":"https://trusted.meta.localhost","jwks_uri":"https://trusted.meta.localhost/.well-known/jwks.json"}"#
                    .to_string(),
            jwks_json: r#"{"keys":[{"kid":"kid-current","kty":"oct","alg":"HS256","k":"cGhhc2U3LXRydXN0ZWQtY3VycmVudC1zZWNyZXQ"}]}"#
                .to_string(),
            audience: "meerkat-console".to_string(),
            require_verified_email: false,
            shared_account_tokens: false,
        },
        console: meerkat_meta::ConsolePolicy {
            require_app_auth: true,
            read_only: false,
            ..meerkat_meta::ConsolePolicy::default()
        },
        ops: meerkat_meta::RuntimeOpsPolicy::default(),
        release_metadata_json: include_str!("../assets/release-targets.json").to_string(),
    })
    .expect("decision state builds")
}

/// What `/console/experience` says of changes of agents on `app`.
async fn experience_identity_changes(app: &axum::Router) -> Value {
    experience(app).await["identity_changes"].clone()
}

/// The two identity affordances of the agent `name` on `experience`'s
/// sidebar: whether a client offers its change, and its removal.
fn identity_affordances(experience: &Value, name: &str) -> (Value, Value) {
    let agents = experience["agent_sidebar"]["live_snapshot"]["agents"]
        .as_array()
        .unwrap_or_else(|| panic!("the experience lists agents: {experience:#}"));
    let agent = agents
        .iter()
        .find(|agent| agent["identity"] == json!(name))
        .unwrap_or_else(|| panic!("the experience lists {name}: {agents:?}"));
    (
        agent["affordances"]["can_change_identity"].clone(),
        agent["affordances"]["can_remove_identity"].clone(),
    )
}

/// `/console/experience` on `app`.
async fn experience(app: &axum::Router) -> Value {
    use tower::ServiceExt;
    let response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri("/console/experience")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024 * 1024)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    assert!(
        status.is_success(),
        "GET /console/experience: {status} {value}"
    );
    value
}

fn create_of(agents: Value) -> IdentityChangeSpec {
    IdentityChangeSpec::Create(
        serde_json::from_value(json!({"agents": agents})).expect("a create as a surface writes it"),
    )
}

fn agent_ref(name: &str, digest: &Sha256Digest) -> AgentRef {
    serde_json::from_value(json!({"name": name, "expected_spec_digest": digest.as_str()}))
        .expect("a change of one agent as a surface writes it")
}

fn refusal_of(outcome: &IdentityOutcome) -> (String, Option<String>) {
    let refusal = outcome
        .refusal()
        .unwrap_or_else(|| panic!("the change is refused: {outcome:?}"));
    (refusal.kind_token().to_owned(), refusal.field.clone())
}

impl Rig {
    fn family(&self) -> Arc<dyn IdentityFamily> {
        self.runtime
            .identity_family()
            .expect("the family is attached")
    }

    /// One change of the family, as `requester`'s request `request_id`.
    async fn change(
        &self,
        requester: &ChangeRequester,
        request_id: &str,
        change: IdentityChangeSpec,
    ) -> IdentityOutcome {
        let operation_id =
            OperationId::for_request(&requester.recorded(), request_id).expect("a request id");
        self.family()
            .change(
                &self.runtime.mob_handle(),
                requester,
                IdentityChangeCall::new(operation_id, change),
            )
            .await
    }

    /// The agent `name` in full, as the host reads it; `None` when no agent
    /// holds the name.
    async fn agent(&self, name: &str) -> Option<AgentRecord> {
        match self
            .family()
            .read(
                &self.runtime.mob_handle(),
                &ChangeRequester::host("ops"),
                IdentityRead::Get(GetAgent::new(name)),
            )
            .await
        {
            Ok(IdentityReadAnswer::Get(record)) => Some(*record),
            Ok(other) => panic!("a get answers one agent: {other:?}"),
            Err(refusal) => {
                assert_eq!(refusal.kind_token(), "identity_unknown", "{refusal}");
                None
            }
        }
    }

    async fn digest(&self, name: &str) -> Sha256Digest {
        self.agent(name)
            .await
            .unwrap_or_else(|| panic!("the agent {name} exists"))
            .agent
            .spec_digest
    }

    /// Wait until a member's turn is asked `marker`; `false` when none is
    /// within the bound.
    async fn asked(&mut self, marker: &str) -> bool {
        tokio::time::timeout(TURN_BOUND, async {
            while let Some(asked) = self.asked.recv().await {
                if asked.contains(marker) {
                    return true;
                }
            }
            false
        })
        .await
        .unwrap_or(false)
    }

    /// Whether a member's turn was asked `marker` so far.
    fn was_asked(&mut self, marker: &str) -> bool {
        let mut asked = false;
        while let Ok(turn) = self.asked.try_recv() {
            asked |= turn.contains(marker);
        }
        asked
    }

    async fn shutdown(self) {
        let _ = self.runtime.shutdown().await;
    }
}

/// A launch serves the family's reads from the moment its identity runtime
/// is installed, and its changes exactly when its identity policy is
/// attached. Before that the family is attached, the runtime says changes
/// are not enabled, a change is refused `change_denied`, a read answers and
/// the Console's experience says changes are not available. With the policy
/// the runtime says changes are enabled and the same Console, built before,
/// says they are available; a read-only Console never does.
#[tokio::test(flavor = "multi_thread")]
async fn the_family_serves_changes_exactly_when_the_launch_attached_its_policy() {
    let temp = tempfile::tempdir().unwrap();
    let (runtime, _asked, _fail_next_session) = launch(temp.path(), false).await;
    let family = runtime
        .identity_family()
        .expect("the family is attached with the identity runtime");
    assert!(!runtime.identity_changes_enabled(), "no policy yet");
    let console = runtime.build_reference_app_router(console_decisions(false));
    let read_only_console = runtime.build_reference_app_router(console_decisions(true));
    let closed = experience_identity_changes(&console).await;
    assert_eq!(closed["available"], json!(false), "{closed:#}");
    assert_eq!(closed["can_create"], json!(false), "{closed:#}");

    let host = ChangeRequester::host("ops");
    let alpha = || create_of(json!([{"name": "alpha", "profile": "worker"}]));
    let operation =
        |request: &str| OperationId::for_request(&host.recorded(), request).expect("a request id");
    let refused = family
        .change(
            &runtime.mob_handle(),
            &host,
            IdentityChangeCall::new(operation("r-1"), alpha()),
        )
        .await;
    assert_eq!(refusal_of(&refused).0, "change_denied", "{refused:?}");
    let read = family
        .read(
            &runtime.mob_handle(),
            &host,
            IdentityRead::Get(GetAgent::new(PLANNER)),
        )
        .await;
    assert!(
        matches!(read, Ok(IdentityReadAnswer::Get(_))),
        "a read answers without a policy: {read:?}"
    );

    assert!(runtime.attach_identity_policy(in_process_policy(), IdentityFamilyOptions::new()));
    assert!(runtime.identity_changes_enabled(), "the policy is attached");
    let open = experience_identity_changes(&console).await;
    assert_eq!(
        open,
        json!({"available": true, "can_create": true, "can_change": true, "can_remove": true}),
        "the Console built before the policy reads it at each call"
    );
    let read_only = experience_identity_changes(&read_only_console).await;
    assert_eq!(read_only["available"], json!(false), "{read_only:#}");

    // A client is offered a control only for an agent a run-time change
    // can change: an agent the host configures answers both affordances
    // false (its change would be refused `owner_read_only`), an agent
    // created at run time answers them true, and a read-only Console
    // answers false for both.
    let created = runtime
        .identity_family()
        .expect("the family, with the policy")
        .change(
            &runtime.mob_handle(),
            &host,
            IdentityChangeCall::new(operation("r-2"), alpha()),
        )
        .await;
    assert_eq!(created.token(), "done", "{created:?}");
    let offered = experience(&console).await;
    assert_eq!(
        identity_affordances(&offered, PLANNER),
        (json!(false), json!(false)),
        "an agent the host configures"
    );
    assert_eq!(
        identity_affordances(&offered, "alpha"),
        (json!(true), json!(true)),
        "an agent created at run time"
    );
    let closed = experience(&read_only_console).await;
    assert_eq!(
        identity_affordances(&closed, "alpha"),
        (json!(false), json!(false)),
        "a read-only Console"
    );
    let _ = runtime.shutdown().await;
}

/// A caller that sees the agents it names, and no other.
struct SeesOnly(&'static [&'static str]);

impl PeerVisibility for SeesOnly {
    fn sees(&self, name: &str) -> bool {
        self.0.contains(&name)
    }
}

/// A change names only the peers its caller may see. A peer the caller may
/// not see is answered exactly as a name no agent holds (`peer_unknown`,
/// the same field, the same sentence), whatever else the request gets
/// wrong, and never as `peer_not_allowed`: the peer rule that denies it
/// speaks only to a caller that sees it.
#[tokio::test(flavor = "multi_thread")]
async fn a_peer_the_caller_may_not_see_reads_as_a_name_no_agent_holds() {
    let temp = tempfile::tempdir().unwrap();
    let (runtime, _asked, _fail_next_session) = launch(temp.path(), false).await;
    let policy = IdentityCreationPolicy::parse(&json!({
        "enabled": true,
        "peers": {
            "rules": [{"id": "no-planner", "target": {"profile": ["planner"]},
                       "outcome": "deny"}],
            "default": "allow",
        },
        "approval": {"rules": [{
            "change": ["create", "peers"],
            "requester": {"kind": "host"},
            "target": {"profile": ["worker"]},
            "outcome": {"kind": "allow"},
        }]},
    }))
    .expect("the policy parses")
    .expect("the policy is enabled");
    assert!(runtime.attach_identity_policy(Arc::new(policy), IdentityFamilyOptions::new()));
    let whole = runtime.identity_family().expect("the family");
    let narrow = runtime
        .identity_family_seen_by(Arc::new(SeesOnly(&[HOST_A])))
        .expect("the family as a caller that sees one agent");
    let wide = runtime
        .identity_family_seen_by(Arc::new(SeesOnly(&[HOST_A, PLANNER])))
        .expect("the family as a caller that sees both agents");

    let host = ChangeRequester::host("ops");
    let mob = runtime.mob_handle();
    let asked = std::sync::atomic::AtomicU64::new(0);
    let refusal = |family: Arc<dyn IdentityFamily>, profile: &'static str, peer: &'static str| {
        let request = format!("peer-{}", asked.fetch_add(1, Ordering::Relaxed));
        let operation = OperationId::for_request(&host.recorded(), &request).expect("a request id");
        let change = create_of(json!([{"name": "gamma", "profile": profile, "peers": [peer]}]));
        let (mob, host) = (mob.clone(), host.clone());
        async move {
            let outcome = family
                .change(&mob, &host, IdentityChangeCall::new(operation, change))
                .await;
            let refusal = outcome
                .refusal()
                .unwrap_or_else(|| panic!("the create is refused: {outcome:?}"));
            (
                refusal.kind_token().to_owned(),
                refusal.field.clone(),
                refusal.detail(),
            )
        }
    };

    // A caller that sees the planner is told its peer rule denies it.
    let denied = refusal(Arc::clone(&whole), "worker", PLANNER).await;
    assert_eq!(denied.0, "peer_not_allowed", "{denied:?}");
    assert_eq!(
        denied.1.as_deref(),
        Some("agents[0].peers[0]"),
        "{denied:?}"
    );
    assert_eq!(refusal(Arc::clone(&wide), "worker", PLANNER).await, denied);

    // A caller that does not is told what it is told of a name no agent
    // holds.
    let hidden = refusal(Arc::clone(&narrow), "worker", PLANNER).await;
    let absent = refusal(Arc::clone(&narrow), "worker", "nobody").await;
    assert_eq!(hidden.0, "peer_unknown", "{hidden:?}");
    assert_eq!(
        hidden.1.as_deref(),
        Some("agents[0].peers[0]"),
        "{hidden:?}"
    );
    assert_eq!(hidden, absent, "nothing tells the two apart");
    assert_eq!(
        refusal(Arc::clone(&whole), "worker", "nobody").await,
        absent,
        "and it is what every caller is told of a name no agent holds"
    );

    // Whatever else the request gets wrong, the two still answer alike.
    let hidden = refusal(Arc::clone(&narrow), "no-such-profile", PLANNER).await;
    let absent = refusal(Arc::clone(&narrow), "no-such-profile", "nobody").await;
    assert_eq!(hidden, absent, "nothing tells the two apart");
    assert_ne!(hidden.0, "peer_not_allowed", "{hidden:?}");

    // An update's new peer is read the same way.
    let digest = match narrow
        .read(&mob, &host, IdentityRead::Get(GetAgent::new(HOST_A)))
        .await
    {
        Ok(IdentityReadAnswer::Get(record)) => record.agent.spec_digest.clone(),
        other => panic!("a get answers one agent: {other:?}"),
    };
    let update = |family: Arc<dyn IdentityFamily>, request: &'static str, peer: &'static str| {
        let operation = OperationId::for_request(&host.recorded(), request).expect("a request id");
        let change = IdentityChangeSpec::Update(
            serde_json::from_value(json!({
                "name": HOST_A, "expected_spec_digest": digest.as_str(), "peers_add": [peer],
            }))
            .expect("an update as a surface writes it"),
        );
        let (mob, host) = (mob.clone(), host.clone());
        async move {
            let outcome = family
                .change(&mob, &host, IdentityChangeCall::new(operation, change))
                .await;
            let refusal = outcome
                .refusal()
                .unwrap_or_else(|| panic!("the update is refused: {outcome:?}"));
            (
                refusal.kind_token().to_owned(),
                refusal.field.clone(),
                refusal.detail(),
            )
        }
    };
    let hidden = update(Arc::clone(&narrow), "update-hidden", PLANNER).await;
    let absent = update(Arc::clone(&narrow), "update-absent", "nobody").await;
    assert_eq!(hidden.0, "peer_unknown", "{hidden:?}");
    assert_eq!(hidden, absent, "nothing tells the two apart");
    let _ = runtime.shutdown().await;
}

/// A change that needs approval is held by the family, decided by a person
/// and carried out by the family's own driver, with nobody asking again:
/// the create answers `pending_decision` and creates nothing; the person
/// the approver map names approves it through `meta/decisions/decide`; the
/// agent then exists, dormant, its record names the consent it committed
/// under, the identity runtime serves it, and the operation reads `done`.
/// The requester never committed anything itself.
#[tokio::test(flavor = "multi_thread")]
async fn a_held_change_is_carried_out_once_a_person_approved_it() {
    use meerkat_core::approval::decision::{
        ApproverId, DecisionId, DecisionReference, PresentationAudience, PresentationRecord,
    };

    let temp = tempfile::tempdir().unwrap();
    let (runtime, _asked, _fail_next_session) =
        launch_with(temp.path(), &fresh_mob_id(), false, true).await;
    let policy = IdentityCreationPolicy::parse(&json!({
        "enabled": true,
        "approval": {"rules": [{
            "change": ["create"],
            "requester": {"kind": "host"},
            "target": {"profile": ["worker"]},
            "outcome": {"kind": "require_approval", "approver": {"id": APPROVERS}},
        }]},
    }))
    .expect("the policy parses")
    .expect("the policy is enabled");
    assert!(runtime.attach_identity_policy(Arc::new(policy), IdentityFamilyOptions::new()));
    let family = runtime.identity_family().expect("the family");
    let mob = runtime.mob_handle();
    let host = ChangeRequester::host("ops");
    let operation = OperationId::for_request(&host.recorded(), "held-1").expect("a request id");
    let scout = || create_of(json!([{"name": "scout", "profile": "worker"}]));

    let held = family
        .change(
            &mob,
            &host,
            IdentityChangeCall::new(operation.clone(), scout()),
        )
        .await;
    assert_eq!(held.token(), "pending_decision", "{held:?}");
    let decision_id = held.to_json()["decision_id"]
        .as_str()
        .expect("a decision id")
        .to_owned();
    let get = |name: &'static str| {
        let (family, mob, host) = (Arc::clone(&family), mob.clone(), host.clone());
        async move {
            match family
                .read(&mob, &host, IdentityRead::Get(GetAgent::new(name)))
                .await
            {
                Ok(IdentityReadAnswer::Get(record)) => Some(*record),
                _ => None,
            }
        }
    };
    assert!(
        get("scout").await.is_none(),
        "nothing is created while it is held"
    );

    // A host channel shows the decision to its approvers, and the person
    // the approver map names approves it.
    let owner = runtime.durable_decisions().owner().clone();
    let row = owner
        .get(&DecisionId::new(decision_id.as_str()).expect("a decision id"))
        .expect("the decision is read");
    owner
        .present(
            &row.decision_id,
            &PresentationRecord::complete(
                PresentationAudience::Approver {
                    approver_id: ApproverId::new(APPROVERS).expect("approver"),
                },
                &row.descriptor.rendering,
                None,
                DecisionReference::new("chat:owners").expect("channel"),
                DecisionReference::new(format!("message-{}", now_ms())).expect("message"),
                now_ms(),
            ),
        )
        .expect("present");
    struct NoNotices;
    #[async_trait]
    impl meerkat_meta::DecisionNoticeSink for NoNotices {
        async fn deliver(&self, _line: String) -> bool {
            true
        }
    }
    let connection = meerkat_meta::StdioDecisionConnection::new(
        runtime.durable_decisions().realm(),
        Arc::new(NoNotices) as Arc<dyn meerkat_meta::DecisionNoticeSink>,
    );
    let line =
        json!({"jsonrpc": "2.0", "id": "decide", "method": "meta/decisions/decide", "params": {
            "decision_id": decision_id,
            "approve": true,
            "presented_digest": row.descriptor.subject_digest.to_string(),
            "attestation": {
                "ingress": "chat",
                "subject": OWNER,
                "scope": "chat-session-1",
                "event_ref": format!("chat:event:{decision_id}"),
                "observed_at_ms": now_ms(),
            },
        }})
        .to_string();
    let answer: Value = serde_json::from_str(
        &connection
            .handle_line(&runtime, &line)
            .await
            .expect("a decision method answers"),
    )
    .expect("answer JSON");
    assert!(answer["error"].is_null(), "{answer:#}");

    // The family's driver carries it out: nobody asks again.
    let deadline = std::time::Instant::now() + TURN_BOUND;
    let created = loop {
        if let Some(record) = get("scout").await {
            break record;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the approved create was not carried out"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(
        created.agent.readiness,
        AgentReadiness::Dormant,
        "{created:?}"
    );
    let asked = created.agent.provenance.expect("its record says who asked");
    assert_eq!(asked.requester_kind, RequesterKind::Host);
    assert_eq!(asked.requester, "ops");
    let served = runtime
        .identity_runtime()
        .expect("the identity runtime")
        .registered_spec(&AgentIdentity::parse("scout").unwrap())
        .await
        .is_ok();
    assert!(
        served,
        "the identity runtime serves the agent from its record"
    );
    match family
        .read(&mob, &host, IdentityRead::Operation(operation.clone()))
        .await
    {
        Ok(IdentityReadAnswer::Operation(OperationState::Outcome(outcome))) => {
            assert_eq!(outcome.token(), "done", "{outcome:?}");
        }
        other => panic!("the operation reads as carried out: {other:?}"),
    }
    // The same request asked again answers what was carried out.
    let again = family
        .change(&mob, &host, IdentityChangeCall::new(operation, scout()))
        .await;
    assert_eq!(again.token(), "done", "{again:?}");
    let _ = runtime.shutdown().await;
}

/// The rules of the rows about held changes: the host creates agents of
/// the profile `worker` at once, but `scout`, whose create a person
/// decides, and a person decides every archive and every removal.
fn held_policy() -> Arc<IdentityCreationPolicy> {
    let approval = json!({"kind": "require_approval", "approver": {"id": APPROVERS}});
    Arc::new(
        IdentityCreationPolicy::parse(&json!({
            "enabled": true,
            "approval": {"rules": [
                {"change": ["create"], "requester": {"kind": "host"},
                 "target": {"profile": ["worker"]}, "outcome": {"kind": "allow"}},
                {"change": ["create"], "requester": {"kind": "host"},
                 "target": {"identity": ["scout"]}, "outcome": approval},
                {"change": ["archive", "remove", "descriptive"], "requester": {"kind": "host"},
                 "target": {"profile": ["worker"]}, "outcome": approval},
            ]},
        }))
        .expect("the policy parses")
        .expect("the policy is enabled"),
    )
}

/// The held decision of an outcome: its id.
fn decision_of(outcome: &IdentityOutcome) -> String {
    assert_eq!(outcome.token(), "pending_decision", "{outcome:?}");
    outcome.to_json()["decision_id"]
        .as_str()
        .expect("a decision id")
        .to_owned()
}

/// One request id holds one decision. While a create is held under an id,
/// another change under the same id never holds a second decision: a
/// change of an agent the host configures is refused `owner_read_only`
/// before the holder is asked, a change the rules would hold too is
/// `operation_reused`, and the operation still reads the one decision.
#[tokio::test(flavor = "multi_thread")]
async fn one_request_id_holds_one_decision_whatever_is_asked_under_it() {
    let temp = tempfile::tempdir().unwrap();
    let (runtime, _asked, _fail_next_session) =
        launch_with(temp.path(), &fresh_mob_id(), false, true).await;
    assert!(runtime.attach_identity_policy(
        held_policy(),
        IdentityFamilyOptions::new().with_host_hooks(Arc::new(NamingHooks))
    ));
    let family = runtime.identity_family().expect("the family");
    let mob = runtime.mob_handle();
    let host = ChangeRequester::host("ops");
    let ask = |request: &'static str, change: IdentityChangeSpec| {
        let (family, mob, host) = (Arc::clone(&family), mob.clone(), host.clone());
        async move {
            let operation =
                OperationId::for_request(&host.recorded(), request).expect("a request id");
            family
                .change(&mob, &host, IdentityChangeCall::new(operation, change))
                .await
        }
    };
    let digest = |name: &'static str| {
        let (family, mob, host) = (Arc::clone(&family), mob.clone(), host.clone());
        async move {
            match family
                .read(&mob, &host, IdentityRead::Get(GetAgent::new(name)))
                .await
            {
                Ok(IdentityReadAnswer::Get(record)) => record.agent.spec_digest.clone(),
                other => panic!("a get answers {name}: {other:?}"),
            }
        }
    };

    let made = ask(
        "r-0",
        create_of(json!([{"name": "alpha", "profile": "worker"}])),
    )
    .await;
    assert_eq!(made.token(), "done", "{made:?}");
    let scout = || create_of(json!([{"name": "scout", "profile": "worker"}]));
    let decision = decision_of(&ask("r-1", scout()).await);

    // An archive of an agent the host configures, under the same id: the
    // owner's rule answers before anything is held.
    let configured = ask(
        "r-1",
        IdentityChangeSpec::Archive(agent_ref(HOST_A, &digest(HOST_A).await)),
    )
    .await;
    assert_eq!(
        refusal_of(&configured).0,
        "owner_read_only",
        "{configured:?}"
    );
    // A removal the rules would hold too, under the same id: it is not the
    // request the decision holds.
    let other = ask(
        "r-1",
        IdentityChangeSpec::Remove(agent_ref("alpha", &digest("alpha").await)),
    )
    .await;
    assert_eq!(refusal_of(&other).0, "operation_reused", "{other:?}");
    // An update the rules would hold too, under the same id.
    let update: IdentityChangeSpec = IdentityChangeSpec::Update(
        serde_json::from_value(json!({
            "name": "alpha", "expected_spec_digest": digest("alpha").await.as_str(),
            "set": {"display_name": "Alpha"},
        }))
        .expect("an update as a surface writes it"),
    );
    let updated = ask("r-1", update).await;
    assert_eq!(refusal_of(&updated).0, "operation_reused", "{updated:?}");
    // A create the host's own check refuses, under the same id: the check
    // is not passed by, and nothing is held for it.
    let forbidden = ask(
        "r-1",
        create_of(json!([{"name": "forbidden", "profile": "worker"}])),
    )
    .await;
    assert_eq!(
        refusal_of(&forbidden).0,
        "validation_refused",
        "{forbidden:?}"
    );
    // A change of an agent the host configures under a fresh id, which the
    // rules would hold: refused, and nothing is held.
    let fresh = ask(
        "r-2",
        IdentityChangeSpec::Archive(agent_ref(HOST_A, &digest(HOST_A).await)),
    )
    .await;
    assert_eq!(refusal_of(&fresh).0, "owner_read_only", "{fresh:?}");
    let unheld = family
        .read(
            &mob,
            &host,
            IdentityRead::Operation(
                OperationId::for_request(&host.recorded(), "r-2").expect("a request id"),
            ),
        )
        .await;
    assert!(
        matches!(
            unheld,
            Ok(IdentityReadAnswer::Operation(OperationState::Unknown))
        ),
        "nothing is held for a refused change: {unheld:?}"
    );
    // Another create under the same id.
    let reused = ask(
        "r-1",
        create_of(json!([{"name": "other", "profile": "worker"}])),
    )
    .await;
    assert_eq!(refusal_of(&reused).0, "operation_reused", "{reused:?}");

    assert_eq!(
        decision_of(&ask("r-1", scout()).await),
        decision,
        "the one decision still holds the operation"
    );
    let _ = runtime.shutdown().await;
}

/// A person's approval is evidence, never authority over the launch's
/// policy. A create is held and approved; the launch that carries it out
/// has a policy that denies creates: nothing is created, the decision ends
/// refused, and the operation's read says the change was denied.
#[tokio::test(flavor = "multi_thread")]
async fn an_approved_change_the_launchs_policy_now_denies_is_not_carried_out() {
    use meerkat_core::approval::decision::{
        ApproverId, DecisionId, DecisionReference, PresentationAudience, PresentationRecord,
    };

    let temp = tempfile::tempdir().unwrap();
    let mob_id = fresh_mob_id();
    let host = ChangeRequester::host("ops");
    let operation = OperationId::for_request(&host.recorded(), "held-1").expect("a request id");
    let scout = || create_of(json!([{"name": "scout", "profile": "worker"}]));

    // The first launch holds the create.
    let (runtime, _asked, _fail_next_session) =
        launch_with(temp.path(), &mob_id, false, true).await;
    assert!(runtime.attach_identity_policy(held_policy(), IdentityFamilyOptions::new()));
    let held = runtime
        .identity_family()
        .expect("the family")
        .change(
            &runtime.mob_handle(),
            &host,
            IdentityChangeCall::new(operation.clone(), scout()),
        )
        .await;
    let decision_id = decision_of(&held);
    let _ = runtime.shutdown().await;
    drop(runtime);

    // A launch without an identity policy carries nothing out: the person
    // the approver map names approves the decision there.
    let (runtime, _asked, _fail_next_session) =
        launch_with(temp.path(), &mob_id, false, true).await;
    let owner = runtime.durable_decisions().owner().clone();
    let row = owner
        .get(&DecisionId::new(decision_id.as_str()).expect("a decision id"))
        .expect("the decision is read");
    owner
        .present(
            &row.decision_id,
            &PresentationRecord::complete(
                PresentationAudience::Approver {
                    approver_id: ApproverId::new(APPROVERS).expect("approver"),
                },
                &row.descriptor.rendering,
                None,
                DecisionReference::new("chat:owners").expect("channel"),
                DecisionReference::new(format!("message-{}", now_ms())).expect("message"),
                now_ms(),
            ),
        )
        .expect("present");
    struct NoNotices;
    #[async_trait]
    impl meerkat_meta::DecisionNoticeSink for NoNotices {
        async fn deliver(&self, _line: String) -> bool {
            true
        }
    }
    let connection = meerkat_meta::StdioDecisionConnection::new(
        runtime.durable_decisions().realm(),
        Arc::new(NoNotices) as Arc<dyn meerkat_meta::DecisionNoticeSink>,
    );
    let line =
        json!({"jsonrpc": "2.0", "id": "decide", "method": "meta/decisions/decide", "params": {
            "decision_id": decision_id,
            "approve": true,
            "presented_digest": row.descriptor.subject_digest.to_string(),
            "attestation": {
                "ingress": "chat",
                "subject": OWNER,
                "scope": "chat-session-1",
                "event_ref": format!("chat:event:{decision_id}"),
                "observed_at_ms": now_ms(),
            },
        }})
        .to_string();
    let answer: Value = serde_json::from_str(
        &connection
            .handle_line(&runtime, &line)
            .await
            .expect("a decision method answers"),
    )
    .expect("answer JSON");
    assert!(answer["error"].is_null(), "{answer:#}");
    let _ = runtime.shutdown().await;
    drop((runtime, owner, connection));

    // The launch that carries it out has a policy with no rule: it denies
    // every create.
    let (runtime, _asked, _fail_next_session) =
        launch_with(temp.path(), &mob_id, false, true).await;
    let denies = IdentityCreationPolicy::parse(&json!({"enabled": true}))
        .expect("the policy parses")
        .expect("the policy is enabled");
    assert!(runtime.attach_identity_policy(Arc::new(denies), IdentityFamilyOptions::new()));
    let family = runtime.identity_family().expect("the family");
    let mob = runtime.mob_handle();
    let deadline = std::time::Instant::now() + TURN_BOUND;
    let ended = loop {
        match family
            .read(&mob, &host, IdentityRead::Operation(operation.clone()))
            .await
        {
            Ok(IdentityReadAnswer::Operation(OperationState::Outcome(outcome)))
                if outcome.token() != "pending_decision" =>
            {
                break outcome;
            }
            Ok(_) => {}
            Err(refusal) => panic!("the operation is read: {refusal}"),
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the approved create was neither carried out nor refused"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(ended.token(), "refused", "{ended:?}");
    let said = ended.to_json().to_string();
    assert!(said.contains("change_denied"), "the read says why: {said}");
    let created = family
        .read(&mob, &host, IdentityRead::Get(GetAgent::new("scout")))
        .await;
    assert!(created.is_err(), "nothing was created: {created:?}");
    let _ = runtime.shutdown().await;
}

/// The issuer the Console's sign-in fixture trusts.
const ISSUER: &str = "https://trusted.meta.localhost";

/// The gateway's runtime composition on `state` under the access rules
/// `access` (TOML), with its Console, which asks for sign-in.
async fn console_under(state: &Path, access: &str) -> (UnifiedRuntime, axum::Router) {
    let (asked_tx, _asked) = tokio::sync::mpsc::unbounded_channel();
    let config: meerkat_meta::AccessControlConfig = toml::from_str(access).expect("access TOML");
    let builder = UnifiedRuntimeBuilder::default()
        .definition(in_process_definition(&fresh_mob_id()))
        .persistent_state(state)
        .continuity_from_state_dir(state)
        .await
        .expect("open the state-dir identity substrate")
        .roster_provider(Arc::new(InProcessRoster))
        .identity_runtime_instance_id("meta-identity-methods-console")
        .access_controller(meerkat_meta::AccessController::new(config).expect("controller"))
        .default_llm_client(Arc::new(RecordingClient { asked: asked_tx }));
    let runtime = Box::pin(builder.build())
        .await
        .expect("build the gateway-shaped UnifiedRuntime");
    let app = runtime.build_console_json_router(signed_console_decisions());
    (runtime, app)
}

/// One Console JSON-RPC call on `app`, signed in as `subject` when given.
async fn console_rpc(
    app: &axum::Router,
    subject: Option<&str>,
    method: &str,
    params: Value,
) -> Value {
    use tower::ServiceExt;
    let mut request = axum::http::Request::builder()
        .method("POST")
        .uri("/console/rpc")
        .header(axum::http::header::CONTENT_TYPE, "application/json");
    if let Some(subject) = subject {
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256);
        header.kid = Some("kid-current".to_string());
        let token = jsonwebtoken::encode(
            &header,
            &json!({
                "iss": ISSUER,
                "aud": "meerkat-console",
                "sub": subject,
                "email": subject,
                "provider": "google_oauth",
                "exp": chrono::Utc::now().timestamp() + 300,
            }),
            &jsonwebtoken::EncodingKey::from_secret(b"phase7-trusted-current-secret"),
        )
        .expect("signed fixture token");
        request = request.header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let request = request
        .body(axum::body::Body::from(
            json!({"jsonrpc": "2.0", "id": "identity", "method": method, "params": params})
                .to_string(),
        ))
        .expect("rpc request");
    let response = tokio::time::timeout(TURN_BOUND, app.clone().oneshot(request))
        .await
        .expect("bounded rpc")
        .expect("rpc response");
    let body = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .expect("rpc body");
    serde_json::from_slice(&body).unwrap_or_else(|_| panic!("rpc json for {method}"))
}

/// The result of a Console call that must answer one.
fn console_ok(answer: &Value) -> &Value {
    assert!(
        answer["error"].is_null(),
        "the call answers a result: {answer:#}"
    );
    &answer["result"]
}

/// The Console scopes the family to what its caller may see and do. A
/// caller that may view and change one agent: reads no peer it may not
/// view; plans a change only as it could make it, an agent it may not view
/// answering as a name no agent holds; names no peer it may not view;
/// reads only its own operations by id, while an administrator reads any;
/// states neither `proposer` nor `source_evidence`; and holds the mob's
/// changes only with `runtime.admin`.
#[tokio::test(flavor = "multi_thread")]
async fn the_console_scopes_the_family_to_what_its_caller_may_see_and_do() {
    const ROOT: &str = "root@example.org";
    const ANN: &str = "ann@example.org";
    let temp = tempfile::tempdir().unwrap();
    let (runtime, app) = console_under(
        temp.path(),
        r#"
enabled = true
admins = ["root@example.org"]

[[rules]]
id = "works"
actions = ["agent.identity.create", "agent.view", "agent.identity.manage", "agent.identity.remove"]

[[rules]]
id = "not-beta"
effect = "deny"
actions = ["agent.view", "agent.identity.manage", "agent.identity.remove"]
agents = ["beta"]
"#,
    )
    .await;
    let policy = IdentityCreationPolicy::parse(&json!({
        "enabled": true,
        "peers": {"default": "allow"},
        "approval": {"rules": [{
            "change": ["create", "descriptive", "peers", "archive", "remove"],
            "requester": {"kind": "human"},
            "target": {"profile": ["worker"]},
            "outcome": {"kind": "allow"},
        }]},
    }))
    .expect("the policy parses")
    .expect("the policy is enabled");
    assert!(runtime.attach_identity_policy(Arc::new(policy), IdentityFamilyOptions::new()));

    let made = console_rpc(
        &app,
        Some(ROOT),
        "meta/identity/create",
        json!({"request_id": "root-1", "agents": [
            {"name": "alpha", "profile": "worker", "peers": ["beta"]},
            {"name": "beta", "profile": "worker"},
        ]}),
    )
    .await;
    let made = console_ok(&made).clone();
    assert_eq!(made["outcome"], json!("done"), "{made:#}");
    let roots_operation = made["operation_id"]
        .as_str()
        .expect("an operation id")
        .to_owned();
    let get = |subject: &'static str, name: &'static str| {
        let app = app.clone();
        async move {
            console_rpc(
                &app,
                Some(subject),
                "meta/identity/get",
                json!({"name": name}),
            )
            .await
        }
    };

    // A read names no peer the caller may not view.
    let whole = get(ROOT, "alpha").await;
    assert_eq!(console_ok(&whole)["peers"], json!(["beta"]), "{whole:#}");
    let seen = get(ANN, "alpha").await;
    assert_eq!(console_ok(&seen)["peers"], json!([]), "{seen:#}");
    let alpha = console_ok(&seen)["spec_digest"].clone();
    let beta = console_ok(&get(ROOT, "beta").await)["spec_digest"].clone();

    // A plan is judged as the change it plans, on the agent it names: an
    // agent the caller may not view answers as a name no agent holds.
    let plan = |name: &'static str, digest: Value| {
        let app = app.clone();
        async move {
            console_rpc(
                &app,
                Some(ANN),
                "meta/identity/plan",
                json!({"change": "update", "request_id": "plan-1", "name": name,
                       "expected_spec_digest": digest, "set": {"display_name": "Other"}}),
            )
            .await
        }
    };
    let own = plan("alpha", alpha.clone()).await;
    assert_eq!(console_ok(&own)["outcome"], json!("allow"), "{own:#}");
    let hidden = plan("beta", beta.clone()).await;
    let unviewed = get(ANN, "beta").await;
    assert!(
        hidden["result"].is_null(),
        "no plan of an agent it may not view: {hidden:#}"
    );
    assert_eq!(
        (&hidden["error"]["code"], &hidden["error"]["data"]["kind"]),
        (
            &unviewed["error"]["code"],
            &unviewed["error"]["data"]["kind"]
        ),
        "a plan of an agent the caller may not view answers as a get of it: {hidden:#}"
    );
    assert_eq!(
        hidden["error"]["data"]["kind"],
        json!("identity_unknown"),
        "{hidden:#}"
    );

    // A peer the caller may not view is a name no agent holds.
    let peered = console_rpc(
        &app,
        Some(ANN),
        "meta/identity/create",
        json!({"request_id": "ann-0", "agents": [
            {"name": "gamma", "profile": "worker", "peers": ["beta"]},
        ]}),
    )
    .await;
    assert_eq!(
        peered["error"]["data"]["kind"],
        json!("peer_unknown"),
        "{peered:#}"
    );
    assert_eq!(
        peered["error"]["data"]["field"],
        json!("agents[0].peers[0]"),
        "{peered:#}"
    );

    // An operation is read by its own requester, and by an administrator.
    let created = console_rpc(
        &app,
        Some(ANN),
        "meta/identity/create",
        json!({"request_id": "ann-1", "agents": [{"name": "delta", "profile": "worker"}]}),
    )
    .await;
    let anns_operation = console_ok(&created)["operation_id"]
        .as_str()
        .expect("an operation id")
        .to_owned();
    let operation = |subject: &'static str, params: Value| {
        let app = app.clone();
        async move { console_rpc(&app, Some(subject), "meta/identity/operation", params).await }
    };
    let own = operation(ANN, json!({"request_id": "ann-1"})).await;
    assert_eq!(console_ok(&own)["outcome"], json!("done"), "{own:#}");
    let foreign = operation(ANN, json!({"operation_id": roots_operation})).await;
    assert_eq!(
        console_ok(&foreign)["outcome"],
        json!("unknown"),
        "another requester's operation reads as one nobody asked: {foreign:#}"
    );
    let administered = operation(ROOT, json!({"operation_id": anns_operation})).await;
    assert_eq!(
        console_ok(&administered)["outcome"],
        json!("done"),
        "{administered:#}"
    );

    // `proposer` and `source_evidence` are the host plane's: a person
    // states neither, and nothing is recorded.
    for (member, value) in [
        ("proposer", json!("ticket-7")),
        (
            "source_evidence",
            json!({"attestor": "crm", "subject": "case-7", "source_ref": "crm:case:7",
                   "observed_at_ms": 1_762_700_000_000_u64}),
        ),
    ] {
        let mut params = json!({"request_id": format!("ann-{member}"),
                   "agents": [{"name": "epsilon", "profile": "worker"}]});
        params[member] = value;
        let stated = console_rpc(&app, Some(ANN), "meta/identity/create", params).await;
        assert_eq!(
            stated["error"]["data"]["kind"],
            json!("invalid_request"),
            "{stated:#}"
        );
        assert_eq!(
            stated["error"]["data"]["field"],
            json!(member),
            "{stated:#}"
        );
    }
    let listed = console_rpc(&app, Some(ROOT), "meta/identity/list", json!({})).await;
    assert!(
        !console_ok(&listed).to_string().contains("epsilon"),
        "nothing was recorded: {listed:#}"
    );

    // The hold of the mob's changes needs `runtime.admin`: a person who has
    // it holds and releases them, and one who has not does not reach it.
    let denied = console_rpc(
        &app,
        Some(ANN),
        "meta/identity/changes_hold",
        json!({"held": true}),
    )
    .await;
    assert_eq!(
        denied["error"]["data"]["kind"],
        json!("access_denied"),
        "{denied:#}"
    );
    let held = console_rpc(
        &app,
        Some(ROOT),
        "meta/identity/changes_hold",
        json!({"held": true}),
    )
    .await;
    assert_eq!(console_ok(&held), &json!({"held": true}), "{held:#}");
    let stopped = console_rpc(
        &app,
        Some(ANN),
        "meta/identity/create",
        json!({"request_id": "ann-2", "agents": [{"name": "zeta", "profile": "worker"}]}),
    )
    .await;
    assert_eq!(
        stopped["error"]["data"]["kind"],
        json!("changes_held"),
        "{stopped:#}"
    );
    let released = console_rpc(
        &app,
        Some(ROOT),
        "meta/identity/changes_hold",
        json!({"held": false}),
    )
    .await;
    assert_eq!(
        console_ok(&released),
        &json!({"held": false}),
        "{released:#}"
    );
    let _ = runtime.shutdown().await;
}

/// On the Console a read and a plan of one agent answer an agent the caller
/// may not view exactly as a name no agent holds: the same error, whole.
/// An agent the caller may view and may not change is still refused as
/// access denied, which tells the caller nothing it may not know.
#[tokio::test(flavor = "multi_thread")]
async fn the_console_answers_an_agent_its_caller_may_not_view_as_a_name_no_agent_holds() {
    const ROOT: &str = "root@example.org";
    const ANN: &str = "ann@example.org";
    let temp = tempfile::tempdir().unwrap();
    let (runtime, app) = console_under(
        temp.path(),
        r#"
enabled = true
admins = ["root@example.org"]

[[rules]]
id = "works"
actions = ["agent.identity.create", "agent.view", "agent.identity.manage", "agent.identity.remove"]

[[rules]]
id = "not-beta"
effect = "deny"
actions = ["agent.view", "agent.identity.manage", "agent.identity.remove"]
agents = ["beta"]

[[rules]]
id = "reads-gamma-only"
effect = "deny"
actions = ["agent.identity.manage", "agent.identity.remove"]
agents = ["gamma"]
"#,
    )
    .await;
    let policy = IdentityCreationPolicy::parse(&json!({
        "enabled": true,
        "approval": {"rules": [{
            "change": ["create", "descriptive", "archive", "remove"],
            "requester": {"kind": "human"},
            "target": {"profile": ["worker"]},
            "outcome": {"kind": "allow"},
        }]},
    }))
    .expect("the policy parses")
    .expect("the policy is enabled");
    assert!(runtime.attach_identity_policy(Arc::new(policy), IdentityFamilyOptions::new()));
    let made = console_rpc(
        &app,
        Some(ROOT),
        "meta/identity/create",
        json!({"request_id": "root-1", "agents": [
            {"name": "beta", "profile": "worker"},
            {"name": "gamma", "profile": "worker"},
        ]}),
    )
    .await;
    assert_eq!(console_ok(&made)["outcome"], json!("done"), "{made:#}");
    let digest_of = |name: &'static str| {
        let app = app.clone();
        async move {
            let read =
                console_rpc(&app, Some(ROOT), "meta/identity/get", json!({"name": name})).await;
            console_ok(&read)["spec_digest"].clone()
        }
    };
    let beta = digest_of("beta").await;
    let gamma = digest_of("gamma").await;

    // A read of one agent.
    let get = |name: &'static str| {
        let app = app.clone();
        async move { console_rpc(&app, Some(ANN), "meta/identity/get", json!({"name": name})).await }
    };
    let nobody = get("nobody").await;
    assert_eq!(
        nobody["error"]["data"]["kind"],
        json!("identity_unknown"),
        "a name no agent holds: {nobody:#}"
    );
    assert_eq!(
        nobody["error"]["data"]["field"],
        json!("name"),
        "{nobody:#}"
    );
    let unviewed = get("beta").await;
    assert!(unviewed["result"].is_null(), "{unviewed:#}");
    assert_eq!(
        unviewed["error"], nobody["error"],
        "a read of an agent the caller may not view answers as a name no agent holds"
    );

    // A plan of each change of one agent, with the same members for both
    // names.
    for (change, more) in [
        (
            "update",
            json!({"expected_spec_digest": beta.clone(), "set": {"display_name": "Other"}}),
        ),
        ("archive", json!({"expected_spec_digest": beta.clone()})),
        ("remove", json!({"expected_spec_digest": beta.clone()})),
    ] {
        let plan = |name: &'static str| {
            let app = app.clone();
            let mut params = json!({"change": change, "request_id": "plan-1", "name": name});
            for (member, value) in more.as_object().expect("an object") {
                params[member.as_str()] = value.clone();
            }
            async move { console_rpc(&app, Some(ANN), "meta/identity/plan", params).await }
        };
        let nobody = plan("nobody").await;
        assert_eq!(
            nobody["error"]["data"]["kind"],
            json!("identity_unknown"),
            "a plan of {change} for a name no agent holds: {nobody:#}"
        );
        let unviewed = plan("beta").await;
        assert!(unviewed["result"].is_null(), "{unviewed:#}");
        assert_eq!(
            unviewed["error"], nobody["error"],
            "a plan of {change} for an agent the caller may not view answers as a name no \
             agent holds"
        );
    }

    // An agent the caller may view and may not change: the read answers
    // it, and a plan of its change is refused as access denied.
    let viewed = get("gamma").await;
    assert_eq!(console_ok(&viewed)["name"], json!("gamma"), "{viewed:#}");
    let denied = console_rpc(
        &app,
        Some(ANN),
        "meta/identity/plan",
        json!({"change": "update", "request_id": "plan-2", "name": "gamma",
               "expected_spec_digest": gamma, "set": {"display_name": "Other"}}),
    )
    .await;
    assert_eq!(
        denied["error"]["data"]["kind"],
        json!("access_denied"),
        "{denied:#}"
    );
    let _ = runtime.shutdown().await;
}

/// On the Console a change of one agent answers an agent the caller may
/// not view exactly as a name no agent holds: the same error, whole, for
/// an update, an archive, an unarchive, a removal and an adoption. Nothing
/// is changed. An agent the caller may view and may not change is refused
/// as access denied.
#[tokio::test(flavor = "multi_thread")]
async fn the_console_answers_a_change_of_an_agent_its_caller_may_not_view_as_a_name_no_agent_holds()
{
    const ROOT: &str = "root@example.org";
    const ANN: &str = "ann@example.org";
    let temp = tempfile::tempdir().unwrap();
    let (runtime, app) = console_under(
        temp.path(),
        r#"
enabled = true
admins = ["root@example.org"]

[[rules]]
id = "works"
actions = ["agent.identity.create", "agent.view", "agent.identity.manage", "agent.identity.remove"]

[[rules]]
id = "not-beta"
effect = "deny"
actions = ["agent.view", "agent.identity.manage", "agent.identity.remove"]
agents = ["beta"]

[[rules]]
id = "reads-gamma-only"
effect = "deny"
actions = ["agent.identity.manage", "agent.identity.remove"]
agents = ["gamma"]
"#,
    )
    .await;
    let policy = IdentityCreationPolicy::parse(&json!({
        "enabled": true,
        "approval": {"rules": [{
            "change": ["create", "descriptive", "archive", "remove", "adopt"],
            "requester": {"kind": "human"},
            "target": {"profile": ["worker"]},
            "outcome": {"kind": "allow"},
        }]},
    }))
    .expect("the policy parses")
    .expect("the policy is enabled");
    assert!(runtime.attach_identity_policy(Arc::new(policy), IdentityFamilyOptions::new()));
    let made = console_rpc(
        &app,
        Some(ROOT),
        "meta/identity/create",
        json!({"request_id": "root-1", "agents": [
            {"name": "beta", "profile": "worker"},
            {"name": "gamma", "profile": "worker"},
        ]}),
    )
    .await;
    assert_eq!(console_ok(&made)["outcome"], json!("done"), "{made:#}");
    let read = |name: &'static str| {
        let app = app.clone();
        async move {
            let read =
                console_rpc(&app, Some(ROOT), "meta/identity/get", json!({"name": name})).await;
            console_ok(&read).clone()
        }
    };
    let beta = read("beta").await;
    let digest = beta["spec_digest"].clone();

    let changes = [
        (
            "meta/identity/update",
            json!({"set": {"display_name": "Other"}}),
        ),
        ("meta/identity/archive", json!({})),
        ("meta/identity/unarchive", json!({})),
        ("meta/identity/remove", json!({})),
        ("meta/identity/adopt", json!({})),
    ];
    for (index, (method, more)) in changes.iter().enumerate() {
        let ask = |name: &'static str| {
            let app = app.clone();
            let mut params = json!({"request_id": format!("ann-{index}-{name}"), "name": name,
                                    "expected_spec_digest": digest.clone()});
            for (member, value) in more.as_object().expect("an object") {
                params[member.as_str()] = value.clone();
            }
            async move { console_rpc(&app, Some(ANN), method, params).await }
        };
        let nobody = ask("nobody").await;
        assert_eq!(
            nobody["error"]["data"]["kind"],
            json!("identity_unknown"),
            "{method} of a name no agent holds: {nobody:#}"
        );
        let unviewed = ask("beta").await;
        assert!(unviewed["result"].is_null(), "{method}: {unviewed:#}");
        assert_eq!(
            unviewed["error"], nobody["error"],
            "{method} of an agent the caller may not view answers as a name no agent holds"
        );
        // An agent the caller may view and may not change.
        let denied = ask("gamma").await;
        assert_eq!(
            denied["error"]["data"]["kind"],
            json!("access_denied"),
            "{method}: {denied:#}"
        );
    }
    assert_eq!(
        read("beta").await,
        beta,
        "nothing of the unviewed agent changed"
    );
    let _ = runtime.shutdown().await;
}

/// A Console caller that is no verified person withdraws nothing and holds
/// nothing: on a Console that signs nobody in, a held change stays held
/// when such a caller asks to withdraw it, and the mob's changes are not
/// held for it.
#[tokio::test(flavor = "multi_thread")]
async fn a_console_caller_that_is_no_verified_person_withdraws_and_holds_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let (runtime, _asked, _fail_next_session) = launch(temp.path(), false).await;
    let policy = IdentityCreationPolicy::parse(&json!({
        "enabled": true,
        "approval": {"rules": [{
            "change": ["create"],
            "requester": {"kind": "host", "surface": "console"},
            "target": {"profile": ["worker"]},
            "outcome": {"kind": "require_approval", "approver": {"id": "owner"}},
        }]},
    }))
    .expect("the policy parses")
    .expect("the policy is enabled");
    assert!(runtime.attach_identity_policy(Arc::new(policy), IdentityFamilyOptions::new()));
    let app = runtime.build_reference_app_router(console_decisions(false));
    let held = console_rpc(
        &app,
        None,
        "meta/identity/create",
        json!({"request_id": "anon-1", "agents": [{"name": "scout", "profile": "worker"}]}),
    )
    .await;
    assert_eq!(
        console_ok(&held)["outcome"],
        json!("pending_decision"),
        "{held:#}"
    );

    let withdrawn = console_rpc(
        &app,
        None,
        "meta/identity/withdraw",
        json!({"request_id": "anon-1"}),
    )
    .await;
    assert_eq!(
        withdrawn["error"]["data"]["kind"],
        json!("decision_not_found"),
        "a caller that is no verified person withdraws nothing: {withdrawn:#}"
    );
    let still = console_rpc(
        &app,
        None,
        "meta/identity/operation",
        json!({"request_id": "anon-1"}),
    )
    .await;
    assert_eq!(
        console_ok(&still)["outcome"],
        json!("pending_decision"),
        "{still:#}"
    );
    let hold = console_rpc(
        &app,
        None,
        "meta/identity/changes_hold",
        json!({"held": true}),
    )
    .await;
    assert_eq!(
        hold["error"]["data"]["kind"],
        json!("change_denied"),
        "{hold:#}"
    );
    let _ = runtime.shutdown().await;
}

/// After an adoption the agent's record is its one owner, also while the
/// host's roster still lists the name: what the identity runtime serves
/// the agent from follows the record, at once and after a restart.
#[tokio::test(flavor = "multi_thread")]
async fn after_an_adoption_the_identity_runtime_serves_the_agent_from_its_record() {
    let temp = tempfile::tempdir().unwrap();
    let mob_id = fresh_mob_id();
    let host = ChangeRequester::host("ops");
    let served = |runtime: &UnifiedRuntime| {
        let identities = runtime.identity_runtime().expect("the identity runtime");
        async move {
            identities
                .registered_spec(&AgentIdentity::parse(HOST_A).unwrap())
                .await
                .ok()
                .and_then(|registered| {
                    registered
                        .spec
                        .display_name
                        .as_ref()
                        .map(|name| name.as_str().to_owned())
                })
        }
    };
    {
        let (runtime, _asked, _fail_next_session) =
            launch_with(temp.path(), &mob_id, false, false).await;
        assert!(runtime.attach_identity_policy(in_process_policy(), IdentityFamilyOptions::new()));
        let family = runtime.identity_family().expect("the family");
        let mob = runtime.mob_handle();
        let digest = |family: Arc<dyn IdentityFamily>, mob: meerkat_mob::MobHandle| {
            let host = host.clone();
            async move {
                match family
                    .read(&mob, &host, IdentityRead::Get(GetAgent::new(HOST_A)))
                    .await
                {
                    Ok(IdentityReadAnswer::Get(record)) => record.agent.spec_digest.clone(),
                    other => panic!("a get answers the agent: {other:?}"),
                }
            }
        };
        let operation =
            |request: &str| OperationId::for_request(&host.recorded(), request).expect("an id");
        let adopted = family
            .change(
                &mob,
                &host,
                IdentityChangeCall::new(
                    operation("adopt-1"),
                    IdentityChangeSpec::Adopt(agent_ref(
                        HOST_A,
                        &digest(Arc::clone(&family), mob.clone()).await,
                    )),
                ),
            )
            .await;
        assert_eq!(adopted.token(), "done", "{adopted:?}");
        let renamed = family
            .change(
                &mob,
                &host,
                IdentityChangeCall::new(
                    operation("update-1"),
                    IdentityChangeSpec::Update(
                        serde_json::from_value(json!({
                            "name": HOST_A,
                            "expected_spec_digest":
                                digest(Arc::clone(&family), mob.clone()).await.as_str(),
                            "set": {"display_name": "Renamed"},
                        }))
                        .expect("an update as a surface writes it"),
                    ),
                ),
            )
            .await;
        assert_eq!(renamed.token(), "done", "{renamed:?}");
        assert_eq!(
            served(&runtime).await.as_deref(),
            Some("Renamed"),
            "the registration follows the record at once"
        );
        let _ = runtime.shutdown().await;
    }
    let (runtime, _asked, _fail_next_session) =
        launch_with(temp.path(), &mob_id, false, false).await;
    assert_eq!(
        served(&runtime).await.as_deref(),
        Some("Renamed"),
        "and after a restart, although the roster still lists the name without it"
    );
    let _ = runtime.shutdown().await;
}

/// A create and an update set who reaches an agent and how it runs, with
/// the members and the values of a roster row (`addressability`,
/// `runtime_mode_override`): the agent's registered row carries them as a
/// configured agent's does, an agent that names neither has a roster row's
/// defaults, and a value no roster row takes is no value of the member. An
/// update that sets either is of the change class `capability`: a policy
/// with no rule for that class denies it and nothing is applied.
#[tokio::test(flavor = "multi_thread")]
async fn a_create_and_an_update_set_addressability_and_runtime_mode_as_a_roster_row_does() {
    use meerkat_meta::identity_creation::UpdateAgent;
    use meerkat_mob::MobRuntimeMode;

    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), false, None).await;
    let host = ChangeRequester::host("ops");
    let created = rig
        .change(
            &host,
            "r-1",
            create_of(json!([
                {"name": "clerk", "profile": "worker", "addressability": "internal_only",
                 "runtime_mode_override": "turn_driven"},
                {"name": "scout", "profile": "worker"},
            ])),
        )
        .await;
    assert_eq!(created.token(), "done", "{created:?}");
    let registered = |name: &'static str| {
        let identities = rig
            .runtime
            .identity_runtime()
            .expect("the identity runtime");
        async move {
            identities
                .registered_spec(&AgentIdentity::parse(name).unwrap())
                .await
                .unwrap_or_else(|error| panic!("{name} is registered: {error}"))
                .spec
        }
    };
    let clerk = registered("clerk").await;
    assert_eq!(clerk.addressability, AgentAddressability::InternalOnly);
    assert_eq!(
        clerk.runtime_mode_override,
        Some(MobRuntimeMode::TurnDriven)
    );
    // An agent that names neither has what a roster row that names neither
    // has, as the configured agent beside it.
    let scout = registered("scout").await;
    let configured = registered(HOST_A).await;
    assert_eq!(scout.addressability, configured.addressability);
    assert_eq!(scout.addressability, AgentAddressability::Addressable);
    assert_eq!(scout.runtime_mode_override, None);

    // What a caller sets, it reads back: a get and a list entry say both
    // members, each only when it is not the default.
    let shown = |name: &'static str| {
        let family = rig.family();
        let mob = rig.runtime.mob_handle();
        let host = host.clone();
        async move {
            let got = match family
                .read(&mob, &host, IdentityRead::Get(GetAgent::new(name)))
                .await
            {
                Ok(IdentityReadAnswer::Get(record)) => {
                    serde_json::to_value(*record).expect("a get as a surface writes it")
                }
                other => panic!("a get answers one agent: {other:?}"),
            };
            let list: ListAgents =
                serde_json::from_value(json!({})).expect("a list as a surface writes it");
            let listed = match family.read(&mob, &host, IdentityRead::List(list)).await {
                Ok(IdentityReadAnswer::List(page)) => page
                    .agents
                    .into_iter()
                    .find(|agent| agent.name == name)
                    .map(|agent| serde_json::to_value(agent).expect("a list entry"))
                    .unwrap_or_else(|| panic!("{name} is listed")),
                other => panic!("a list answers a page: {other:?}"),
            };
            let members = |entry: &Value| {
                (
                    entry.get("addressability").cloned(),
                    entry.get("runtime_mode_override").cloned(),
                )
            };
            assert_eq!(members(&got), members(&listed), "{name}: a get and a list");
            members(&got)
        }
    };
    assert_eq!(
        shown("clerk").await,
        (Some(json!("internal_only")), Some(json!("turn_driven")))
    );
    assert_eq!(
        shown("scout").await,
        (None, None),
        "the defaults are unsaid"
    );
    assert_eq!(
        shown(HOST_A).await,
        (None, None),
        "a configured agent reads the same way"
    );

    let before = rig.digest("scout").await;
    let update = |set: Value| -> UpdateAgent {
        serde_json::from_value(
            json!({"name": "scout", "expected_spec_digest": before.as_str(),
                                      "set": set}),
        )
        .expect("an update as a surface writes it")
    };
    let updated = rig
        .change(
            &host,
            "r-2",
            IdentityChangeSpec::Update(update(json!({
                "addressability": "internal_only",
                "runtime_mode_override": "turn_driven",
            }))),
        )
        .await;
    assert_eq!(updated.token(), "done", "{updated:?}");
    let scout = registered("scout").await;
    assert_eq!(scout.addressability, AgentAddressability::InternalOnly);
    assert_eq!(
        scout.runtime_mode_override,
        Some(MobRuntimeMode::TurnDriven)
    );
    assert_ne!(rig.digest("scout").await, before, "the spec changed");
    assert_eq!(
        shown("scout").await,
        (Some(json!("internal_only")), Some(json!("turn_driven"))),
        "the update reads back"
    );

    // A value no roster row takes is no value of the member.
    for agent in [
        json!({"name": "other", "profile": "worker", "addressability": "public"}),
        json!({"name": "other", "profile": "worker", "runtime_mode_override": "batch"}),
    ] {
        assert!(
            serde_json::from_value::<CreateAgents>(json!({"agents": [agent]})).is_err(),
            "{agent}"
        );
    }
    for set in [
        json!({"addressability": "public"}),
        json!({"runtime_mode_override": "batch"}),
    ] {
        assert!(
            serde_json::from_value::<UpdateAgent>(
                json!({"name": "scout", "expected_spec_digest": before.as_str(), "set": set})
            )
            .is_err(),
            "{set}"
        );
    }
    let _ = rig.runtime.shutdown().await;

    // Under a policy with no rule for the class `capability` the update is
    // denied as that class, and nothing is applied.
    let temp = tempfile::tempdir().unwrap();
    let (runtime, _asked, _fail_next_session) = launch(temp.path(), false).await;
    let policy = IdentityCreationPolicy::parse(&json!({
        "enabled": true,
        "approval": {"rules": [{
            "change": ["create", "descriptive"],
            "requester": {"kind": "host"},
            "target": {"profile": ["worker"]},
            "outcome": {"kind": "allow"},
        }]},
    }))
    .expect("the policy parses")
    .expect("the policy is enabled");
    assert!(runtime.attach_identity_policy(Arc::new(policy), IdentityFamilyOptions::new()));
    let family = runtime.identity_family().expect("the family");
    let mob = runtime.mob_handle();
    let ask = |request: &'static str, change: IdentityChangeSpec| {
        let (family, mob, host) = (Arc::clone(&family), mob.clone(), host.clone());
        async move {
            let operation =
                OperationId::for_request(&host.recorded(), request).expect("a request id");
            family
                .change(&mob, &host, IdentityChangeCall::new(operation, change))
                .await
        }
    };
    let made = ask(
        "r-1",
        create_of(json!([{"name": "scout", "profile": "worker"}])),
    )
    .await;
    assert_eq!(made.token(), "done", "{made:?}");
    let digest = made.agents()[0].spec_digest.clone().expect("a spec digest");
    let denied = ask(
        "r-2",
        IdentityChangeSpec::Update(
            serde_json::from_value(json!({"name": "scout",
                "expected_spec_digest": digest.as_str(),
                "set": {"addressability": "internal_only"}}))
            .expect("an update as a surface writes it"),
        ),
    )
    .await;
    let data = denied
        .refusal()
        .unwrap_or_else(|| panic!("the update is refused: {denied:?}"))
        .to_data();
    assert_eq!(data["kind"], json!("change_denied"), "{data:#}");
    assert_eq!(data["change"], json!("capability"), "{data:#}");
    let unchanged = runtime
        .identity_runtime()
        .expect("the identity runtime")
        .registered_spec(&AgentIdentity::parse("scout").unwrap())
        .await
        .expect("scout is registered")
        .spec;
    assert_eq!(unchanged.addressability, AgentAddressability::Addressable);
    let _ = runtime.shutdown().await;
}

/// The schedules that target an agent follow its archive, its unarchive
/// and its removal, and the change says which. An archive pauses the Active
/// schedules that target the agent and leaves one somebody paused by name
/// as theirs; the same archive asked again changes nothing; a resume by
/// name takes a schedule over; the unarchive resumes exactly the schedules
/// still paused for their target, and says the agent's schedules that are
/// active after it; a removal ends every one. A plan of each change lists
/// what the change would answer. Another agent's schedule is never touched.
#[tokio::test(flavor = "multi_thread")]
async fn the_schedules_that_target_an_agent_follow_its_archive_unarchive_and_removal() {
    use meerkat::SchedulePhase::{Active, Deleted, Paused};
    use meerkat_meta::schedule_wiring::{MobAgentSchedules, ScheduleMobTargetRegistry};

    let temp = tempfile::tempdir().unwrap();
    let (runtime, _asked, _fail_next_session) = launch(temp.path(), false).await;
    let schedules = meerkat::ScheduleService::new(Arc::new(meerkat::MemoryScheduleStore::new()));
    let options = IdentityFamilyOptions::new().with_schedules(Arc::new(MobAgentSchedules::new(
        schedules.clone(),
        ScheduleMobTargetRegistry::default(),
    )));
    assert!(runtime.attach_identity_policy(in_process_policy(), options));
    let family = runtime.identity_family().expect("the family");
    let mob = runtime.mob_handle();
    let host = ChangeRequester::host("ops");
    let ask = |request: &'static str, change: IdentityChangeSpec| {
        let (family, mob, host) = (Arc::clone(&family), mob.clone(), host.clone());
        async move {
            let operation =
                OperationId::for_request(&host.recorded(), request).expect("a request id");
            family
                .change(&mob, &host, IdentityChangeCall::new(operation, change))
                .await
        }
    };
    let plan = |change: IdentityChangeSpec| {
        let (family, mob, host) = (Arc::clone(&family), mob.clone(), host.clone());
        async move {
            let operation =
                OperationId::for_request(&host.recorded(), "plan").expect("a request id");
            match family
                .read(
                    &mob,
                    &host,
                    IdentityRead::Plan(Box::new(IdentityChangeCall::new(operation, change))),
                )
                .await
            {
                Ok(IdentityReadAnswer::Plan(plan)) => {
                    serde_json::to_value(*plan).expect("a plan as a surface writes it")
                }
                other => panic!("a plan answers one: {other:?}"),
            }
        }
    };
    let made = ask(
        "r-1",
        create_of(json!([
            {"name": "scout", "profile": "worker"},
            {"name": "ledger", "profile": "worker"},
        ])),
    )
    .await;
    assert_eq!(made.token(), "done", "{made:?}");
    let digest = |name: &'static str| {
        let (family, mob, host) = (Arc::clone(&family), mob.clone(), host.clone());
        async move {
            match family
                .read(&mob, &host, IdentityRead::Get(GetAgent::new(name)))
                .await
            {
                Ok(IdentityReadAnswer::Get(record)) => record.agent.spec_digest,
                other => panic!("a get answers one agent: {other:?}"),
            }
        }
    };

    // Four schedules: three that target scout (one of them paused by name)
    // and one that targets another agent.
    let schedule = |name: &'static str, agent: &'static str| {
        let schedules = schedules.clone();
        async move {
            schedules
                .create(meerkat::CreateScheduleRequest {
                    name: Some(name.to_owned()),
                    description: None,
                    trigger: meerkat::TriggerSpec::Once {
                        due_at_utc: chrono::Utc::now() + chrono::Duration::days(1),
                    },
                    target: meerkat::TargetBinding::identity(
                        meerkat::IdentityTargetBinding::resumable(
                            agent,
                            meerkat::ScheduledSessionAction::Prompt {
                                prompt: meerkat_core::ContentInput::Text("Check in.".into()),
                                system_prompt: None,
                                render_metadata: None,
                                skill_refs: Vec::new(),
                                additional_instructions: Vec::new(),
                            },
                        ),
                    ),
                    misfire_policy: meerkat::MisfirePolicy::default(),
                    overlap_policy: meerkat::OverlapPolicy::default(),
                    missing_target_policy: meerkat::MissingTargetPolicy::default(),
                    labels: std::collections::BTreeMap::new(),
                    planning_horizon_days: Some(1),
                    planning_horizon_occurrences: Some(1),
                })
                .await
                .unwrap_or_else(|error| panic!("the schedule {name} is created: {error}"))
                .schedule_id
        }
    };
    let daily = schedule("daily", "scout").await;
    let weekly = schedule("weekly", "scout").await;
    let held = schedule("held", "scout").await;
    let other = schedule("other", "ledger").await;
    schedules.pause(&held).await.expect("a pause by name");
    let state = |id: &meerkat::ScheduleId| {
        let (schedules, id) = (schedules.clone(), id.clone());
        async move {
            let schedule = schedules.get(&id).await.expect("the schedule is read");
            (schedule.phase, schedule.paused_for_target())
        }
    };
    // The names a `schedules` member lists, with its effect and count.
    let said = |schedules: &Value| -> (String, u64, Vec<String>) {
        let mut names: Vec<String> = schedules["schedules"]
            .as_array()
            .unwrap_or_else(|| panic!("a list of schedules: {schedules:#}"))
            .iter()
            .map(|schedule| {
                assert!(schedule["schedule_id"].is_string(), "{schedule:#}");
                schedule["name"].as_str().expect("a name").to_owned()
            })
            .collect();
        names.sort_unstable();
        assert!(schedules.get("incomplete").is_none(), "{schedules:#}");
        (
            schedules["effect"].as_str().expect("an effect").to_owned(),
            schedules["count"].as_u64().expect("a count"),
            names,
        )
    };
    let names = |names: &[&str]| names.iter().map(ToString::to_string).collect::<Vec<_>>();

    // An archive: the plan lists what it would pause, and the answer what
    // it paused.
    let scout = digest("scout").await;
    let at_archive = scout.clone();
    let planned = plan(IdentityChangeSpec::Archive(agent_ref("scout", &scout))).await;
    assert_eq!(
        said(&planned["schedules"]),
        ("paused".to_owned(), 2, names(&["daily", "weekly"])),
        "{planned:#}"
    );
    assert_eq!(
        state(&daily).await,
        (Active, false),
        "a plan writes nothing"
    );
    let archived = ask(
        "r-2",
        IdentityChangeSpec::Archive(agent_ref("scout", &scout)),
    )
    .await;
    assert_eq!(archived.token(), "done", "{archived:?}");
    assert_eq!(
        said(&archived.to_json()["agents"][0]["schedules"]),
        ("paused".to_owned(), 2, names(&["daily", "weekly"])),
        "{archived:?}"
    );
    assert_eq!(state(&daily).await, (Paused, true));
    assert_eq!(state(&weekly).await, (Paused, true));
    assert_eq!(state(&held).await, (Paused, false), "a pause by name stays");
    assert_eq!(state(&other).await, (Active, false), "another agent's");

    // The same archive asked again: its outcome again, no schedules said,
    // nothing changed.
    let again = ask(
        "r-2",
        IdentityChangeSpec::Archive(agent_ref("scout", &scout)),
    )
    .await;
    assert_eq!(again.token(), "done", "{again:?}");
    assert!(
        again.to_json()["agents"][0].get("schedules").is_none(),
        "{again:?}"
    );
    assert_eq!(state(&daily).await, (Paused, true));
    assert_eq!(state(&held).await, (Paused, false));

    // A resume by name while the agent is archived takes the schedule over.
    schedules.resume(&weekly).await.expect("a resume by name");
    assert_eq!(state(&weekly).await, (Active, false));

    // The unarchive resumes exactly the schedule still paused for its
    // target. Its plan and its answer list the agent's schedules that are
    // active after it: the one a person resumed by name too, never the one
    // somebody paused by name.
    let scout = digest("scout").await;
    let planned = plan(IdentityChangeSpec::Unarchive(agent_ref("scout", &scout))).await;
    assert_eq!(
        said(&planned["schedules"]),
        ("resumed".to_owned(), 2, names(&["daily", "weekly"])),
        "{planned:#}"
    );
    let restored = ask(
        "r-3",
        IdentityChangeSpec::Unarchive(agent_ref("scout", &scout)),
    )
    .await;
    assert_eq!(restored.token(), "done", "{restored:?}");
    assert_eq!(
        said(&restored.to_json()["agents"][0]["schedules"]),
        ("resumed".to_owned(), 2, names(&["daily", "weekly"])),
        "{restored:?}"
    );
    assert_eq!(state(&daily).await, (Active, false));
    assert_eq!(state(&weekly).await, (Active, false));
    assert_eq!(state(&held).await, (Paused, false), "a pause by name stays");

    // The archive asked again after the unarchive undid it moves nothing.
    let stale = ask(
        "r-2",
        IdentityChangeSpec::Archive(agent_ref("scout", &at_archive)),
    )
    .await;
    assert_eq!(stale.token(), "done", "{stale:?}");
    assert_eq!(state(&daily).await, (Active, false));

    // A removal ends every schedule that targets the agent.
    let scout = digest("scout").await;
    let planned = plan(IdentityChangeSpec::Remove(agent_ref("scout", &scout))).await;
    assert_eq!(
        said(&planned["schedules"]),
        ("ended".to_owned(), 3, names(&["daily", "held", "weekly"])),
        "{planned:#}"
    );
    let removed = ask(
        "r-4",
        IdentityChangeSpec::Remove(agent_ref("scout", &scout)),
    )
    .await;
    assert_eq!(removed.token(), "done", "{removed:?}");
    assert_eq!(
        said(&removed.to_json()["agents"][0]["schedules"]),
        ("ended".to_owned(), 3, names(&["daily", "held", "weekly"])),
        "{removed:?}"
    );
    for ended in [&daily, &weekly, &held] {
        assert_eq!(state(ended).await.0, Deleted);
    }
    assert_eq!(state(&other).await, (Active, false), "another agent's");

    // A change that stops nobody says nothing of schedules.
    let ledger = digest("ledger").await;
    let updated = ask(
        "r-5",
        IdentityChangeSpec::Update(
            serde_json::from_value(json!({"name": "ledger",
                "expected_spec_digest": ledger.as_str(),
                "set": {"display_name": "The ledger"}}))
            .expect("an update as a surface writes it"),
        ),
    )
    .await;
    assert_eq!(updated.token(), "done", "{updated:?}");
    assert!(
        updated.to_json()["agents"][0].get("schedules").is_none(),
        "{updated:?}"
    );
    let _ = runtime.shutdown().await;
}

/// A one-shot schedule named `name` that targets the agent `agent`, due
/// tomorrow.
async fn check_in_schedule(
    schedules: &meerkat::ScheduleService,
    name: &str,
    agent: &str,
) -> meerkat::ScheduleId {
    schedules
        .create(meerkat::CreateScheduleRequest {
            name: Some(name.to_owned()),
            description: None,
            trigger: meerkat::TriggerSpec::Once {
                due_at_utc: chrono::Utc::now() + chrono::Duration::days(1),
            },
            target: meerkat::TargetBinding::identity(meerkat::IdentityTargetBinding::resumable(
                agent,
                meerkat::ScheduledSessionAction::Prompt {
                    prompt: meerkat_core::ContentInput::Text("Check in.".into()),
                    system_prompt: None,
                    render_metadata: None,
                    skill_refs: Vec::new(),
                    additional_instructions: Vec::new(),
                },
            )),
            misfire_policy: meerkat::MisfirePolicy::default(),
            overlap_policy: meerkat::OverlapPolicy::default(),
            missing_target_policy: meerkat::MissingTargetPolicy::default(),
            labels: std::collections::BTreeMap::new(),
            planning_horizon_days: Some(1),
            planning_horizon_occurrences: Some(1),
        })
        .await
        .unwrap_or_else(|error| panic!("the schedule {name} is created: {error}"))
        .schedule_id
}

/// An agent that asks for an archive is told what became of the archived
/// agent's schedules and how many they are, and is given no list of them:
/// the schedules may be anybody's. The host, asking for the unarchive, is
/// given the list. The same holds for a plan.
#[tokio::test(flavor = "multi_thread")]
async fn an_agent_requester_is_told_what_became_of_the_schedules_and_given_no_list() {
    use meerkat_meta::schedule_wiring::{MobAgentSchedules, ScheduleMobTargetRegistry};

    let temp = tempfile::tempdir().unwrap();
    let (runtime, _asked, _fail_next_session) = launch(temp.path(), false).await;
    let schedules = meerkat::ScheduleService::new(Arc::new(meerkat::MemoryScheduleStore::new()));
    let options = IdentityFamilyOptions::new().with_schedules(Arc::new(MobAgentSchedules::new(
        schedules.clone(),
        ScheduleMobTargetRegistry::default(),
    )));
    assert!(runtime.attach_identity_policy(in_process_policy(), options));
    let family = runtime.identity_family().expect("the family");
    let mob = runtime.mob_handle();
    let change = |requester: ChangeRequester, request: &'static str, change: IdentityChangeSpec| {
        let (family, mob) = (Arc::clone(&family), mob.clone());
        async move {
            let operation =
                OperationId::for_request(&requester.recorded(), request).expect("a request id");
            family
                .change(&mob, &requester, IdentityChangeCall::new(operation, change))
                .await
        }
    };
    let plan = |requester: ChangeRequester, change: IdentityChangeSpec| {
        let (family, mob) = (Arc::clone(&family), mob.clone());
        async move {
            let operation =
                OperationId::for_request(&requester.recorded(), "plan").expect("a request id");
            match family
                .read(
                    &mob,
                    &requester,
                    IdentityRead::Plan(Box::new(IdentityChangeCall::new(operation, change))),
                )
                .await
            {
                Ok(IdentityReadAnswer::Plan(plan)) => {
                    serde_json::to_value(*plan).expect("a plan as a surface writes it")
                }
                other => panic!("a plan answers one: {other:?}"),
            }
        }
    };
    let planner = ChangeRequester::agent_recorded(PLANNER);
    let host = ChangeRequester::host("ops");
    let made = change(
        planner.clone(),
        "r-1",
        create_of(json!([{"name": "scout", "profile": "worker"}])),
    )
    .await;
    assert_eq!(made.token(), "done", "{made:?}");
    let digest = match family
        .read(&mob, &host, IdentityRead::Get(GetAgent::new("scout")))
        .await
    {
        Ok(IdentityReadAnswer::Get(record)) => record.agent.spec_digest,
        other => panic!("a get answers one agent: {other:?}"),
    };
    let daily = check_in_schedule(&schedules, "daily", "scout").await;

    // The agent's plan and its archive: what, how many, and no list.
    let planned = plan(
        planner.clone(),
        IdentityChangeSpec::Archive(agent_ref("scout", &digest)),
    )
    .await;
    assert_eq!(
        planned["schedules"],
        json!({"effect": "paused", "count": 1}),
        "{planned:#}"
    );
    let archived = change(
        planner.clone(),
        "r-2",
        IdentityChangeSpec::Archive(agent_ref("scout", &digest)),
    )
    .await;
    assert_eq!(archived.token(), "done", "{archived:?}");
    assert_eq!(
        archived.to_json()["agents"][0]["schedules"],
        json!({"effect": "paused", "count": 1}),
        "{archived:?}"
    );
    let paused = schedules.get(&daily).await.expect("the schedule is read");
    assert!(
        paused.paused_for_target(),
        "the schedule followed the agent"
    );

    // The host's plan and its unarchive: the list.
    let listed = json!({
        "effect": "resumed",
        "count": 1,
        "schedules": [{"schedule_id": daily.to_string(), "name": "daily"}],
    });
    let planned = plan(
        host.clone(),
        IdentityChangeSpec::Unarchive(agent_ref("scout", &digest)),
    )
    .await;
    assert_eq!(planned["schedules"], listed, "{planned:#}");
    let unarchived = change(
        host.clone(),
        "r-3",
        IdentityChangeSpec::Unarchive(agent_ref("scout", &digest)),
    )
    .await;
    assert_eq!(unarchived.token(), "done", "{unarchived:?}");
    assert_eq!(
        unarchived.to_json()["agents"][0]["schedules"],
        listed,
        "{unarchived:?}"
    );
    let _ = runtime.shutdown().await;
}

/// On the Console the list of an agent's schedules, on the answer of its
/// archive, unarchive and removal and on their plan, is an administrator's:
/// a signed-in person without `runtime.admin` who may change the agent is
/// told what became of its schedules and how many they are, and no more.
#[tokio::test(flavor = "multi_thread")]
async fn the_console_lists_an_agents_schedules_only_to_an_administrator() {
    use meerkat_meta::schedule_wiring::{MobAgentSchedules, ScheduleMobTargetRegistry};

    const ROOT: &str = "root@example.org";
    const ANN: &str = "ann@example.org";
    let temp = tempfile::tempdir().unwrap();
    let (runtime, app) = console_under(
        temp.path(),
        r#"
enabled = true
admins = ["root@example.org"]

[[rules]]
id = "works"
actions = ["agent.identity.create", "agent.view", "agent.identity.manage", "agent.identity.remove"]
"#,
    )
    .await;
    let policy = IdentityCreationPolicy::parse(&json!({
        "enabled": true,
        "peers": {"default": "allow"},
        "approval": {"rules": [{
            "change": ["create", "archive", "remove"],
            "requester": {"kind": "human"},
            "target": {"profile": ["worker"]},
            "outcome": {"kind": "allow"},
        }]},
    }))
    .expect("the policy parses")
    .expect("the policy is enabled");
    let schedules = meerkat::ScheduleService::new(Arc::new(meerkat::MemoryScheduleStore::new()));
    let options = IdentityFamilyOptions::new().with_schedules(Arc::new(MobAgentSchedules::new(
        schedules.clone(),
        ScheduleMobTargetRegistry::default(),
    )));
    assert!(runtime.attach_identity_policy(Arc::new(policy), options));

    let made = console_rpc(
        &app,
        Some(ROOT),
        "meta/identity/create",
        json!({"request_id": "root-1", "agents": [
            {"name": "alpha", "profile": "worker"},
            {"name": "beta", "profile": "worker"},
        ]}),
    )
    .await;
    assert_eq!(console_ok(&made)["outcome"], json!("done"), "{made:#}");
    let daily = check_in_schedule(&schedules, "daily", "alpha").await;
    let nightly = check_in_schedule(&schedules, "nightly", "beta").await;
    let digest_of = |name: &'static str| {
        let app = app.clone();
        async move {
            let read =
                console_rpc(&app, Some(ROOT), "meta/identity/get", json!({"name": name})).await;
            console_ok(&read)["spec_digest"].clone()
        }
    };
    let call = |subject: &'static str, method: &'static str, params: Value| {
        let app = app.clone();
        async move {
            let answer = console_rpc(&app, Some(subject), method, params).await;
            console_ok(&answer).clone()
        }
    };

    // A person who may change the agent and does not administer the
    // runtime: what, how many, and no list, on the plan and on the answer.
    let alpha = digest_of("alpha").await;
    let counted = json!({"effect": "paused", "count": 1});
    let planned = call(
        ANN,
        "meta/identity/plan",
        json!({"change": "archive", "request_id": "ann-plan", "name": "alpha",
               "expected_spec_digest": alpha.clone()}),
    )
    .await;
    assert_eq!(planned["schedules"], counted, "{planned:#}");
    let archived = call(
        ANN,
        "meta/identity/archive",
        json!({"request_id": "ann-1", "name": "alpha", "expected_spec_digest": alpha}),
    )
    .await;
    assert_eq!(archived["outcome"], json!("done"), "{archived:#}");
    assert_eq!(archived["agents"][0]["schedules"], counted, "{archived:#}");
    let paused = schedules.get(&daily).await.expect("the schedule is read");
    assert!(
        paused.paused_for_target(),
        "the schedule followed the agent"
    );

    // An administrator: the list, on the plan and on the answer.
    let beta = digest_of("beta").await;
    let listed = json!({
        "effect": "paused",
        "count": 1,
        "schedules": [{"schedule_id": nightly.to_string(), "name": "nightly"}],
    });
    let planned = call(
        ROOT,
        "meta/identity/plan",
        json!({"change": "archive", "request_id": "root-plan", "name": "beta",
               "expected_spec_digest": beta.clone()}),
    )
    .await;
    assert_eq!(planned["schedules"], listed, "{planned:#}");
    let archived = call(
        ROOT,
        "meta/identity/archive",
        json!({"request_id": "root-2", "name": "beta", "expected_spec_digest": beta}),
    )
    .await;
    assert_eq!(archived["outcome"], json!("done"), "{archived:#}");
    assert_eq!(archived["agents"][0]["schedules"], listed, "{archived:#}");
    let _ = runtime.shutdown().await;
}

/// A change's answer never waits on another agent's start. An agent of a
/// profile that runs on its own is created, and its first turn is slow
/// (its model takes 20 s to answer): a second create, and an update of the
/// agent just created, are answered within a few seconds all the same.
#[tokio::test(flavor = "multi_thread")]
async fn a_change_after_a_create_does_not_wait_for_the_created_agents_start() {
    const BOUND: Duration = Duration::from_secs(8);
    MODEL_DELAY_MS.store(20_000, Ordering::Relaxed);
    let temp = tempfile::tempdir().unwrap();
    let (runtime, _asked, _fail_next_session) = launch(temp.path(), false).await;
    let policy = IdentityCreationPolicy::parse(&json!({
        "enabled": true,
        "approval": {"rules": [{
            "change": ["create", "descriptive"],
            "requester": {"kind": "host"},
            "target": {"profile": ["runner"]},
            "outcome": {"kind": "allow"},
        }]},
    }))
    .expect("the policy parses")
    .expect("the policy is enabled");
    assert!(runtime.attach_identity_policy(Arc::new(policy), IdentityFamilyOptions::new()));
    let family = runtime.identity_family().expect("the family");
    let mob = runtime.mob_handle();
    let host = ChangeRequester::host("ops");
    let ask = |request: &'static str, change: IdentityChangeSpec| {
        let (family, mob, host) = (Arc::clone(&family), mob.clone(), host.clone());
        async move {
            let operation =
                OperationId::for_request(&host.recorded(), request).expect("a request id");
            let asked = std::time::Instant::now();
            let outcome = family
                .change(&mob, &host, IdentityChangeCall::new(operation, change))
                .await;
            (outcome, asked.elapsed())
        }
    };
    let (first, took) = ask(
        "r-1",
        create_of(json!([{"name": "runner-1", "profile": "runner"}])),
    )
    .await;
    assert_eq!(first.token(), "done", "{first:?}");
    assert!(took < BOUND, "the create itself took {took:?}");
    let (second, took) = ask(
        "r-2",
        create_of(json!([{"name": "runner-2", "profile": "runner"}])),
    )
    .await;
    assert_eq!(second.token(), "done", "{second:?}");
    assert!(
        took < BOUND,
        "a create that follows a create waited {took:?} for the first agent's start"
    );
    let digest = first.agents()[0]
        .spec_digest
        .clone()
        .expect("a spec digest");
    let (renamed, took) = ask(
        "r-3",
        IdentityChangeSpec::Update(
            serde_json::from_value(json!({
                "name": "runner-1", "expected_spec_digest": digest.as_str(),
                "set": {"display_name": "Runner One"},
            }))
            .expect("an update as a surface writes it"),
        ),
    )
    .await;
    assert_eq!(renamed.token(), "done", "{renamed:?}");
    assert!(
        took < BOUND,
        "an update of the agent just created waited {took:?} for its start"
    );
    let _ = runtime.shutdown().await;
}

/// An error of Core's call that comes after Core committed the change is
/// no refusal of it: the seam reads Core's receipt first. After an adoption
/// and after an update committed, the same error answers each as committed;
/// under an operation nothing committed it is the retryable refusal, and
/// under another request's operation it is `operation_reused`.
#[tokio::test(flavor = "multi_thread")]
async fn a_late_error_after_a_committed_update_or_adoption_answers_it_as_committed() {
    use meerkat_meta::identity_creation::{
        AdoptIdentitySpec, ChangeCommit, CoreChangeExecutor, IdentityEntryFacts, RequestContext,
        RequestEnvelope, RequesterFacts, UpdateIdentitySpec, consent_of_plan, normalize_adopt,
        normalize_update,
    };

    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), false, None).await;
    let host = ChangeRequester::host("ops");
    let mob = rig.runtime.mob_handle();
    let policy = in_process_policy();
    let facts = RequesterFacts::host("ops");
    let late = || meerkat_mob::MobError::LifecycleOperationPending {
        intent: "a step of the member is still running".to_owned(),
    };
    let operation =
        |request: &str| OperationId::for_request(&host.recorded(), request).expect("an id");
    let seam = CoreChangeExecutor::new();

    // The adoption, through the family, and the same request as the seam
    // was handed it.
    let configured = rig.digest(HOST_A).await;
    let adopted = rig
        .change(
            &host,
            "adopt-1",
            IdentityChangeSpec::Adopt(agent_ref(HOST_A, &configured)),
        )
        .await;
    assert_eq!(adopted.token(), "done", "{adopted:?}");
    let adopt = normalize_adopt(
        &policy,
        &facts,
        RequestEnvelope::default(),
        &IdentityEntryFacts::new(HOST_A, configured.clone(), None, "worker", BTreeMap::new()),
        &AdoptIdentitySpec::new(HOST_A, configured.clone()),
    )
    .expect("the adoption is allowed");
    let consent = consent_of_plan(&adopt.plan).expect("the rules allow it");
    let answered = seam
        .after_core_error(
            &ChangeCommit::new(&mob, &operation("adopt-1"), &host, consent, &adopt.request),
            late(),
        )
        .await
        .expect("a typed outcome");
    assert_eq!(
        answered.outcome, adopted,
        "an adoption Core committed is answered as committed"
    );
    // Under an operation nothing committed, the error is the refusal.
    let never = seam
        .after_core_error(
            &ChangeCommit::new(
                &mob,
                &operation("adopt-never"),
                &host,
                consent,
                &adopt.request,
            ),
            late(),
        )
        .await
        .expect("a typed outcome");
    assert_eq!(
        refusal_of(&never.outcome).0,
        "admission_unsettled",
        "{never:?}"
    );

    // The update, the same way.
    let before = rig.digest(HOST_A).await;
    let renamed = rig
        .change(
            &host,
            "update-1",
            IdentityChangeSpec::Update(
                serde_json::from_value(json!({
                    "name": HOST_A, "expected_spec_digest": before.as_str(),
                    "set": {"display_name": "Renamed"},
                }))
                .expect("an update as a surface writes it"),
            ),
        )
        .await;
    assert_eq!(renamed.token(), "done", "{renamed:?}");
    let profiles: std::collections::BTreeSet<String> = [
        "worker".to_owned(),
        "planner".to_owned(),
        "runner".to_owned(),
    ]
    .into();
    let peers = BTreeMap::new();
    let spec: UpdateIdentitySpec = serde_json::from_value(json!({
        "identity": HOST_A,
        "expected_spec_digest": before.as_str(),
        "set": {"display_name": "Renamed"},
    }))
    .expect("an update");
    let update = normalize_update(
        &policy,
        &RequestContext {
            requester: &facts,
            profiles: &profiles,
            peers: &peers,
        },
        RequestEnvelope::default(),
        &IdentityEntryFacts::new(HOST_A, before.clone(), Some(1), "worker", BTreeMap::new()),
        &spec,
    )
    .expect("the update is allowed");
    let consent = consent_of_plan(&update.plan).expect("the rules allow it");
    let answered = seam
        .after_core_error(
            &ChangeCommit::new(
                &mob,
                &operation("update-1"),
                &host,
                consent,
                &update.request,
            ),
            late(),
        )
        .await
        .expect("a typed outcome");
    assert_eq!(
        answered.outcome.token(),
        "done",
        "an update Core committed is answered as committed: {answered:?}"
    );
    assert_eq!(
        answered.outcome.agents()[0].spec_digest,
        renamed.agents()[0].spec_digest
    );
    // Another request under the update's operation is not that update.
    let reused = seam
        .after_core_error(
            &ChangeCommit::new(&mob, &operation("update-1"), &host, consent, &adopt.request),
            late(),
        )
        .await
        .expect("a typed outcome");
    assert_eq!(
        refusal_of(&reused.outcome).0,
        "operation_reused",
        "{reused:?}"
    );
    rig.shutdown().await;
}

/// One request id names two operations for two requesters: the host and a
/// person that both choose `r-1` commit two changes, each requester's retry
/// answers its own, and each agent's record names who asked for it. A
/// requester's own id under another request is `operation_reused`.
#[tokio::test(flavor = "multi_thread")]
async fn one_request_id_names_two_operations_for_two_requesters() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), false, None).await;
    let host = ChangeRequester::host("ops");
    let person = ChangeRequester::human("person-1");
    let alpha = || create_of(json!([{"name": "alpha", "profile": "worker"}]));
    let beta = || create_of(json!([{"name": "beta", "profile": "worker"}]));

    let first = rig.change(&host, "r-1", alpha()).await;
    let second = rig.change(&person, "r-1", beta()).await;
    assert_eq!(first.token(), "done", "{first:?}");
    assert_eq!(second.token(), "done", "{second:?}");
    assert_ne!(
        first.operation_id(),
        second.operation_id(),
        "two operations"
    );
    assert_eq!(first.agents()[0].name, "alpha");
    assert_eq!(second.agents()[0].name, "beta");

    assert_eq!(
        rig.change(&host, "r-1", alpha()).await,
        first,
        "the host's retry"
    );
    assert_eq!(
        rig.change(&person, "r-1", beta()).await,
        second,
        "the person's retry"
    );
    let reused = rig.change(&person, "r-1", alpha()).await;
    assert_eq!(refusal_of(&reused).0, "operation_reused", "{reused:?}");

    let alpha = rig.agent("alpha").await.expect("alpha exists");
    let asked = alpha.agent.provenance.expect("its record says who asked");
    assert_eq!(asked.requester_kind, RequesterKind::Host);
    assert_eq!(asked.requester, "ops");
    let beta = rig.agent("beta").await.expect("beta exists");
    let asked = beta.agent.provenance.expect("its record says who asked");
    assert_eq!(asked.requester_kind, RequesterKind::Human);
    assert_eq!(asked.requester, "person-1");

    // The id of a committed operation names its request only: another
    // kind of change of the same agent under it, and the same kind of
    // change of another agent, are `operation_reused`, also where the new
    // request can no longer be judged against its agent (the agent was
    // removed, or the digest it names is not the agent's).
    let digest = alpha.agent.spec_digest.clone();
    let removed = rig
        .change(
            &host,
            "remove-1",
            IdentityChangeSpec::Remove(agent_ref("alpha", &digest)),
        )
        .await;
    assert_eq!(removed.token(), "done", "{removed:?}");
    let other_kind = rig
        .change(
            &host,
            "remove-1",
            IdentityChangeSpec::Archive(agent_ref("alpha", &digest)),
        )
        .await;
    assert_eq!(
        refusal_of(&other_kind).0,
        "operation_reused",
        "{other_kind:?}"
    );
    let other_agent = rig
        .change(
            &host,
            "remove-1",
            IdentityChangeSpec::Remove(agent_ref("beta", &digest)),
        )
        .await;
    assert_eq!(
        refusal_of(&other_agent).0,
        "operation_reused",
        "{other_agent:?}"
    );
    assert!(rig.agent("beta").await.is_some(), "beta is not removed");
    rig.shutdown().await;
}

/// An agent asks for a change as itself, read from its own record: an
/// agent of a profile with the mob tools is admitted by its ordinary grant
/// and by the rule that selects its profile, one without the mob tools is
/// refused by its grant, and no agent changes itself.
#[tokio::test(flavor = "multi_thread")]
async fn an_agent_requester_is_judged_by_its_own_profile_and_never_changes_itself() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), false, None).await;
    let planner = ChangeRequester::agent_recorded(PLANNER);
    let created = rig
        .change(
            &planner,
            "r-1",
            create_of(json!([{"name": "scout", "profile": "worker"}])),
        )
        .await;
    assert_eq!(created.token(), "done", "{created:?}");
    let scout = rig.agent("scout").await.expect("scout exists");
    let asked = scout.agent.provenance.expect("its record says who asked");
    assert_eq!(asked.requester_kind, RequesterKind::Agent);
    assert_eq!(asked.requester, PLANNER);

    // An agent without the mob tools holds no grant for any profile: asked
    // for by its recorded identity, Core answers that it holds none.
    let worker = ChangeRequester::agent_recorded(HOST_A);
    let refused = rig
        .change(
            &worker,
            "r-1",
            create_of(json!([{"name": "other", "profile": "worker"}])),
        )
        .await;
    assert_eq!(refusal_of(&refused).0, "grant_revoked", "{refused:?}");
    assert!(rig.agent("other").await.is_none(), "nothing was created");

    // No agent changes itself, whatever the change.
    let scout_requester = ChangeRequester::agent_recorded("scout");
    let digest = scout.agent.spec_digest.clone();
    for (request_id, change) in [
        (
            "self-archive",
            IdentityChangeSpec::Archive(agent_ref("scout", &digest)),
        ),
        (
            "self-unarchive",
            IdentityChangeSpec::Unarchive(agent_ref("scout", &digest)),
        ),
        (
            "self-remove",
            IdentityChangeSpec::Remove(agent_ref("scout", &digest)),
        ),
    ] {
        let refused = rig.change(&scout_requester, request_id, change).await;
        assert_eq!(
            refusal_of(&refused).0,
            "self_change_refused",
            "{request_id}: {refused:?}"
        );
    }
    // An agent's call carries no proposer: that is the host plane's.
    let mut proposed: CreateAgents =
        serde_json::from_value(json!({"agents": [{"name": "third", "profile": "worker"}]}))
            .unwrap();
    proposed.proposer = Some("somebody".to_owned());
    let refused = rig
        .change(&planner, "r-2", IdentityChangeSpec::Create(proposed))
        .await;
    assert_eq!(
        refusal_of(&refused),
        ("invalid_request".to_owned(), Some("proposer".to_owned())),
        "{refused:?}"
    );
    rig.shutdown().await;
}

/// The policy judges an agent's change as the identity its surface
/// attributed to the call, and Core reads the grant of the member of the
/// session the call ran in and records that member as the requester: the
/// two are one agent. A call attributed to one agent in the session of
/// another is refused `invalid_request`, naming no field, before Core is
/// asked for a grant, and nothing is created. The session's own member is
/// admitted by its grant.
#[tokio::test(flavor = "multi_thread")]
async fn an_agent_call_attributed_to_another_agent_than_its_sessions_member_is_refused() {
    use meerkat_meta::identity_creation::{
        ChangeCommit, CoreChangeExecutor, IdentityChangeExecutor, RequestContext, RequestEnvelope,
        RequesterFacts, consent_of_plan, core_member_name, normalize_create,
    };

    let temp = tempfile::tempdir().unwrap();
    let (runtime, asked, fail_next_session) = launch(temp.path(), false).await;
    let policy = IdentityCreationPolicy::parse(&json!({
        "enabled": true,
        "approval": {"rules": [
            {"change": ["create"], "requester": {"kind": "host"},
             "target": {"profile": ["worker", "planner"]}, "outcome": {"kind": "allow"}},
            {"change": ["create"],
             "requester": {"kind": "agent", "profile": ["worker", "planner"]},
             "target": {"profile": ["worker"]}, "outcome": {"kind": "allow"}},
        ]},
    }))
    .expect("the policy parses")
    .expect("the policy is enabled");
    let policy = Arc::new(policy);
    assert!(runtime.attach_identity_policy(Arc::clone(&policy), IdentityFamilyOptions::new()));
    let mut rig = Rig {
        runtime,
        asked,
        step: Arc::new(RecordingStep::default()),
        fail_next_session,
    };
    let host = ChangeRequester::host("ops");

    // An agent with the mob tools whose first message runs: its session is
    // built, and holds its grant. And an agent without them.
    let created = rig
        .change(
            &host,
            "create-1",
            create_of(json!([
                {"name": "chief", "profile": "planner", "first_message": "Begin the day."},
                {"name": "scout", "profile": "worker"},
            ])),
        )
        .await;
    assert_eq!(created.token(), "done", "{created:?}");
    assert!(
        rig.asked("Begin the day.").await,
        "the chief's first message runs"
    );
    let mob = rig.runtime.mob_handle();
    let chief = core_member_name("chief").expect("a member name");
    let session = match mob.identity_entry(&chief) {
        Ok(meerkat_mob::IdentityEntryRead::Entry(view)) => match view.standing {
            meerkat_mob::EntryStanding::Seated(seated) => {
                seated.session_id.expect("the chief has a session")
            }
            _ => panic!("the chief is seated"),
        },
        _ => panic!("the chief is an entry of the mob"),
    };
    assert_eq!(
        mob.member_of_session(&session)
            .await
            .map(|(member, _)| member),
        Some(chief),
        "the session is the chief's"
    );

    // The chief in its own session is admitted by its grant.
    let own = ChangeRequester::agent_session("chief", session.clone());
    let answered = rig
        .change(
            &own,
            "r-1",
            create_of(json!([{"name": "fourth", "profile": "worker"}])),
        )
        .await;
    assert_eq!(answered.token(), "done", "{answered:?}");

    // Attributed to scout, in the chief's session.
    let crossed = ChangeRequester::agent_session("scout", session.clone());
    let refused = rig
        .change(
            &crossed,
            "r-1",
            create_of(json!([{"name": "third", "profile": "worker"}])),
        )
        .await;
    assert_eq!(
        refusal_of(&refused),
        ("invalid_request".to_owned(), None),
        "{refused:?}"
    );
    assert!(rig.agent("third").await.is_none(), "nothing was created");

    // The seam asks again, for a caller that reaches it another way than
    // through the family: the same create committed there is refused the
    // same, and nothing is created; the session's own member is admitted.
    let seam = CoreChangeExecutor::new();
    let profiles: std::collections::BTreeSet<String> = [
        "worker".to_owned(),
        "planner".to_owned(),
        "runner".to_owned(),
    ]
    .into();
    let peers = BTreeMap::new();
    let create_as = |identity: &str, profile: &str, name: &str| {
        let create: CreateAgents =
            serde_json::from_value(json!({"agents": [{"name": name, "profile": "worker"}]}))
                .expect("a create as a surface writes it");
        normalize_create(
            &policy,
            &RequestContext {
                requester: &RequesterFacts::agent(identity, profile, BTreeMap::new()),
                profiles: &profiles,
                peers: &peers,
            },
            RequestEnvelope::default(),
            &create.to_create_specs().expect("the agents of a create"),
        )
        .expect("the create is allowed")
    };
    let request = create_as("scout", "worker", "fifth");
    let operation = OperationId::for_request(&crossed.recorded(), "seam-1").expect("an id");
    let refused = seam
        .commit(ChangeCommit::new(
            &mob,
            &operation,
            &crossed,
            consent_of_plan(&request.plan).expect("the rules allow it"),
            &request.request,
        ))
        .await
        .expect("a typed outcome");
    assert_eq!(
        refusal_of(&refused.outcome),
        ("invalid_request".to_owned(), None),
        "{refused:?}"
    );
    assert!(rig.agent("fifth").await.is_none(), "nothing was created");
    let request = create_as("chief", "planner", "fifth");
    let own = ChangeRequester::agent_session("chief", session);
    let operation = OperationId::for_request(&own.recorded(), "seam-1").expect("an id");
    let admitted = seam
        .commit(ChangeCommit::new(
            &mob,
            &operation,
            &own,
            consent_of_plan(&request.plan).expect("the rules allow it"),
            &request.request,
        ))
        .await
        .expect("a typed outcome");
    assert_eq!(admitted.outcome.token(), "done", "{admitted:?}");
    rig.shutdown().await;
}

/// A person is never asked to decide a call whose attribution is not its
/// session's member: under a rule that needs approval, a create attributed
/// to one agent in the session of another is refused `invalid_request` at
/// once, before anything is held. No decision holds its operation, and
/// nothing is created. The same create by the session's own member is
/// held for a person, as the rule says.
#[tokio::test(flavor = "multi_thread")]
async fn a_call_attributed_to_another_agent_than_its_sessions_member_is_never_held() {
    use meerkat_meta::identity_creation::core_member_name;

    let temp = tempfile::tempdir().unwrap();
    let (runtime, asked, fail_next_session) =
        launch_with(temp.path(), &fresh_mob_id(), false, true).await;
    let approval = json!({"kind": "require_approval", "approver": {"id": APPROVERS}});
    let policy = IdentityCreationPolicy::parse(&json!({
        "enabled": true,
        "approval": {"rules": [
            {"change": ["create"], "requester": {"kind": "host"},
             "target": {"profile": ["worker", "planner"]}, "outcome": {"kind": "allow"}},
            {"change": ["create"],
             "requester": {"kind": "agent", "profile": ["worker", "planner"]},
             "target": {"profile": ["worker"]}, "outcome": approval},
        ]},
    }))
    .expect("the policy parses")
    .expect("the policy is enabled");
    assert!(runtime.attach_identity_policy(Arc::new(policy), IdentityFamilyOptions::new()));
    let mut rig = Rig {
        runtime,
        asked,
        step: Arc::new(RecordingStep::default()),
        fail_next_session,
    };
    let host = ChangeRequester::host("ops");
    let created = rig
        .change(
            &host,
            "create-1",
            create_of(json!([
                {"name": "chief", "profile": "planner", "first_message": "Begin the day."},
                {"name": "scout", "profile": "worker"},
            ])),
        )
        .await;
    assert_eq!(created.token(), "done", "{created:?}");
    assert!(
        rig.asked("Begin the day.").await,
        "the chief's first message runs"
    );
    let mob = rig.runtime.mob_handle();
    let chief = core_member_name("chief").expect("a member name");
    let session = match mob.identity_entry(&chief) {
        Ok(meerkat_mob::IdentityEntryRead::Entry(view)) => match view.standing {
            meerkat_mob::EntryStanding::Seated(seated) => {
                seated.session_id.expect("the chief has a session")
            }
            _ => panic!("the chief is seated"),
        },
        _ => panic!("the chief is an entry of the mob"),
    };

    // Attributed to scout, in the chief's session: refused at once.
    let crossed = ChangeRequester::agent_session("scout", session.clone());
    let refused = rig
        .change(
            &crossed,
            "r-1",
            create_of(json!([{"name": "third", "profile": "worker"}])),
        )
        .await;
    assert_eq!(
        refusal_of(&refused),
        ("invalid_request".to_owned(), None),
        "{refused:?}"
    );
    let unheld = rig
        .family()
        .read(
            &mob,
            &host,
            IdentityRead::Operation(
                OperationId::for_request(&crossed.recorded(), "r-1").expect("a request id"),
            ),
        )
        .await;
    assert!(
        matches!(
            unheld,
            Ok(IdentityReadAnswer::Operation(OperationState::Unknown))
        ),
        "no decision holds a refused call: {unheld:?}"
    );
    assert!(rig.agent("third").await.is_none(), "nothing was created");

    // The session's own member, under the same rule, is held for a person.
    let own = ChangeRequester::agent_session("chief", session);
    let held = rig
        .change(
            &own,
            "r-1",
            create_of(json!([{"name": "fourth", "profile": "worker"}])),
        )
        .await;
    assert_eq!(held.token(), "pending_decision", "{held:?}");
    rig.shutdown().await;
}

/// The label by which a host says an agent is private to a person.
const PRIVATE_TO: &str = "console.private_to";

/// A host's roster with an agent that is private to a person: the two
/// agents every row has, and `confidant`, whose labels name its person.
/// With `first_message` its first message runs at the launch, so this
/// process builds it; without, it is seated dormant and never built here.
struct RosterWithPrivate {
    first_message: bool,
}

#[async_trait]
impl RosterProvider for RosterWithPrivate {
    async fn roster(&self, _context: &RosterContext) -> Result<Vec<DurableAgentSpec>, RosterError> {
        let mut confidant = roster_spec("confidant", "worker");
        confidant
            .labels
            .insert(PRIVATE_TO.to_owned(), "person-1".to_owned());
        if self.first_message {
            confidant.initial_message = Some("Keep this between us.".to_owned().into());
        }
        Ok(vec![
            roster_spec(HOST_A, "worker"),
            roster_spec(PLANNER, "planner"),
            confidant,
        ])
    }
}

/// The names a list of the family answers `requester`, archived agents
/// included, in name order.
async fn listed_to(
    family: &Arc<dyn IdentityFamily>,
    mob: &meerkat_mob::MobHandle,
    requester: &ChangeRequester,
) -> Vec<String> {
    let list: ListAgents = serde_json::from_value(json!({"include_archived": true}))
        .expect("a list as a surface writes it");
    match family.read(mob, requester, IdentityRead::List(list)).await {
        Ok(IdentityReadAnswer::List(page)) => {
            let mut names: Vec<String> = page.agents.into_iter().map(|agent| agent.name).collect();
            names.sort_unstable();
            names
        }
        other => panic!("a list answers a page: {other:?}"),
    }
}

/// The names a directory of the family offers `requester` as peers, in
/// name order.
async fn offered_to(
    family: &Arc<dyn IdentityFamily>,
    mob: &meerkat_mob::MobHandle,
    requester: &ChangeRequester,
) -> Vec<String> {
    match family
        .read(
            mob,
            requester,
            IdentityRead::Directory(ReadDirectory::default()),
        )
        .await
    {
        Ok(IdentityReadAnswer::Directory(page)) => {
            let mut names: Vec<String> = page.peers.into_iter().map(|peer| peer.name).collect();
            names.sort_unstable();
            names
        }
        other => panic!("a directory answers a page: {other:?}"),
    }
}

/// An agent is shown no agent that is private to a person, on any read of
/// the family: it is not listed, the directory does not offer it, a get
/// and a plan of it answer as for a name no agent holds, and it is no peer
/// (`peer_unknown`). The rule is the family's own: the host, and a person
/// whose view of the mob shows the agent, read it as before, and a person
/// whose view does not show it does not.
#[tokio::test(flavor = "multi_thread")]
async fn an_agent_is_shown_no_agent_that_is_private_to_a_person() {
    let temp = tempfile::tempdir().unwrap();
    let (runtime, asked, fail_next_session) = launch_roster(
        temp.path(),
        &fresh_mob_id(),
        false,
        false,
        Arc::new(RosterWithPrivate {
            first_message: true,
        }),
    )
    .await;
    assert!(runtime.attach_identity_policy(in_process_policy(), IdentityFamilyOptions::new()));
    let mut rig = Rig {
        runtime,
        asked,
        step: Arc::new(RecordingStep::default()),
        fail_next_session,
    };
    assert!(
        rig.asked("Keep this between us.").await,
        "the private agent's first message runs"
    );
    let family = rig.family();
    let mob = rig.runtime.mob_handle();
    let host = ChangeRequester::host("ops");
    let planner = ChangeRequester::agent_recorded(PLANNER);
    let person = ChangeRequester::human("person-1");
    let everyone = vec![
        "confidant".to_owned(),
        HOST_A.to_owned(),
        PLANNER.to_owned(),
    ];
    let others = vec![HOST_A.to_owned(), PLANNER.to_owned()];
    // The host reads every agent.
    assert_eq!(listed_to(&family, &mob, &host).await, everyone);
    assert_eq!(offered_to(&family, &mob, &host).await, everyone);

    // An agent is shown the others only.
    assert_eq!(listed_to(&family, &mob, &planner).await, others);
    assert_eq!(offered_to(&family, &mob, &planner).await, others);
    let got = family
        .read(
            &mob,
            &planner,
            IdentityRead::Get(GetAgent::new("confidant")),
        )
        .await
        .expect_err("an agent gets no private agent");
    let absent = family
        .read(&mob, &planner, IdentityRead::Get(GetAgent::new("nobody")))
        .await
        .expect_err("a name no agent holds");
    assert_eq!(got.kind_token(), "identity_unknown", "{got}");
    assert_eq!(
        (got.kind_token(), &got.field),
        (absent.kind_token(), &absent.field),
        "nothing tells the two apart"
    );
    let named = rig
        .change(
            &planner,
            "peer-1",
            create_of(json!([{"name": "gamma", "profile": "worker", "peers": ["confidant"]}])),
        )
        .await;
    assert_eq!(
        refusal_of(&named),
        (
            "peer_unknown".to_owned(),
            Some("agents[0].peers[0]".to_owned())
        ),
        "{named:?}"
    );
    assert!(rig.agent("gamma").await.is_none(), "nothing was created");

    // A person whose view of the mob shows the agent reads it, and one
    // whose view does not show it does not: what a view shows is its own.
    let shown = rig
        .runtime
        .identity_family_seen_by(Arc::new(SeesOnly(&["confidant", HOST_A, PLANNER])))
        .expect("the family as a caller that sees every agent");
    assert_eq!(listed_to(&shown, &mob, &person).await, everyone);
    assert_eq!(offered_to(&shown, &mob, &person).await, everyone);
    let hidden = rig
        .runtime
        .identity_family_seen_by(Arc::new(SeesOnly(&[HOST_A, PLANNER])))
        .expect("the family as a caller that sees the two others");
    assert_eq!(listed_to(&hidden, &mob, &person).await, others);
    assert_eq!(offered_to(&hidden, &mob, &person).await, others);
    rig.shutdown().await;
}

/// Who is private to a person is said by the agent's own record, not by
/// what this process happened to build: a private agent that is seated
/// dormant and never ran here is shown to no agent either. It is not
/// listed, the directory does not offer it, it is no peer, and a get, a
/// plan and every kind of change of it answer an agent exactly as a name
/// no agent holds. The host still reads it.
#[tokio::test(flavor = "multi_thread")]
async fn an_agent_is_shown_no_private_agent_that_never_ran() {
    let temp = tempfile::tempdir().unwrap();
    let (runtime, asked, fail_next_session) = launch_roster(
        temp.path(),
        &fresh_mob_id(),
        false,
        false,
        Arc::new(RosterWithPrivate {
            first_message: false,
        }),
    )
    .await;
    assert!(runtime.attach_identity_policy(in_process_policy(), IdentityFamilyOptions::new()));
    let mut rig = Rig {
        runtime,
        asked,
        step: Arc::new(RecordingStep::default()),
        fail_next_session,
    };
    let family = rig.family();
    let mob = rig.runtime.mob_handle();
    let host = ChangeRequester::host("ops");
    let planner = ChangeRequester::agent_recorded(PLANNER);
    let everyone = vec![
        "confidant".to_owned(),
        HOST_A.to_owned(),
        PLANNER.to_owned(),
    ];
    let others = vec![HOST_A.to_owned(), PLANNER.to_owned()];

    assert_eq!(listed_to(&family, &mob, &host).await, everyone);
    let digest = rig.digest("confidant").await;
    assert!(
        !rig.was_asked("Keep this between us."),
        "the private agent never ran here"
    );

    // The reads.
    assert_eq!(listed_to(&family, &mob, &planner).await, others);
    assert_eq!(offered_to(&family, &mob, &planner).await, others);
    let got = family
        .read(
            &mob,
            &planner,
            IdentityRead::Get(GetAgent::new("confidant")),
        )
        .await
        .expect_err("an agent gets no private agent");
    let absent = family
        .read(&mob, &planner, IdentityRead::Get(GetAgent::new("nobody")))
        .await
        .expect_err("a name no agent holds");
    assert_eq!(got.kind_token(), "identity_unknown", "{got}");
    assert_eq!(
        (got.kind_token(), &got.field),
        (absent.kind_token(), &absent.field),
        "nothing tells the two apart"
    );

    // A peer of a change.
    let named = rig
        .change(
            &planner,
            "peer-1",
            create_of(json!([{"name": "gamma", "profile": "worker", "peers": ["confidant"]}])),
        )
        .await;
    assert_eq!(
        refusal_of(&named),
        (
            "peer_unknown".to_owned(),
            Some("agents[0].peers[0]".to_owned())
        ),
        "{named:?}"
    );

    // A plan and every kind of change of it, as of a name no agent holds.
    let changes = |name: &str| {
        vec![
            (
                "update",
                IdentityChangeSpec::Update(
                    serde_json::from_value(json!({
                        "name": name, "expected_spec_digest": digest.as_str(),
                        "set": {"display_name": "Friend"},
                    }))
                    .expect("an update as a surface writes it"),
                ),
            ),
            (
                "archive",
                IdentityChangeSpec::Archive(agent_ref(name, &digest)),
            ),
            (
                "unarchive",
                IdentityChangeSpec::Unarchive(agent_ref(name, &digest)),
            ),
            (
                "remove",
                IdentityChangeSpec::Remove(agent_ref(name, &digest)),
            ),
            ("adopt", IdentityChangeSpec::Adopt(agent_ref(name, &digest))),
        ]
    };
    for ((kind, private), (_, unheld)) in changes("confidant").into_iter().zip(changes("nobody")) {
        let planned = family
            .read(
                &mob,
                &planner,
                IdentityRead::Plan(Box::new(IdentityChangeCall::new(
                    OperationId::for_request(&planner.recorded(), &format!("plan-{kind}"))
                        .expect("a request id"),
                    private.clone(),
                ))),
            )
            .await
            .expect_err("an agent plans no change of a private agent");
        assert_eq!(
            planned.kind_token(),
            "identity_unknown",
            "{kind}: {planned}"
        );
        let refused = rig.change(&planner, &format!("c-{kind}"), private).await;
        let absent = rig.change(&planner, &format!("n-{kind}"), unheld).await;
        assert_eq!(
            refusal_of(&refused).0,
            "identity_unknown",
            "{kind}: {refused:?}"
        );
        assert_eq!(
            refusal_of(&refused),
            refusal_of(&absent),
            "{kind}: nothing tells the two apart"
        );
    }
    assert_eq!(
        rig.digest("confidant").await,
        digest,
        "nothing changed the private agent"
    );
    rig.shutdown().await;
}

/// The requester's side of the same rule: a private agent that is seated
/// dormant and never ran here asks for no change and plans none. Its own
/// record says whom it is private to; nothing this process built has to.
/// An agent private to no one is admitted on the same launch.
#[tokio::test(flavor = "multi_thread")]
async fn a_private_agent_that_never_ran_asks_for_no_change() {
    let temp = tempfile::tempdir().unwrap();
    let (runtime, asked, fail_next_session) = launch_roster(
        temp.path(),
        &fresh_mob_id(),
        false,
        false,
        Arc::new(RosterWithPrivate {
            first_message: false,
        }),
    )
    .await;
    assert!(runtime.attach_identity_policy(in_process_policy(), IdentityFamilyOptions::new()));
    let mut rig = Rig {
        runtime,
        asked,
        step: Arc::new(RecordingStep::default()),
        fail_next_session,
    };
    let confidant = ChangeRequester::agent_recorded("confidant");
    let gamma = || create_of(json!([{"name": "gamma", "profile": "worker"}]));
    let refused = rig.change(&confidant, "r-1", gamma()).await;
    assert_eq!(
        refusal_of(&refused),
        ("private_requester".to_owned(), None),
        "{refused:?}"
    );
    assert!(rig.agent("gamma").await.is_none(), "nothing was created");
    let planned = rig
        .family()
        .read(
            &rig.runtime.mob_handle(),
            &confidant,
            IdentityRead::Plan(Box::new(IdentityChangeCall::new(
                OperationId::for_request(&confidant.recorded(), "r-2").expect("a request id"),
                gamma(),
            ))),
        )
        .await
        .expect_err("a private agent plans no change");
    assert_eq!(planned.kind_token(), "private_requester", "{planned}");
    assert!(
        !rig.was_asked("Keep this between us."),
        "the private agent never ran here"
    );

    // An agent that is private to no one is admitted: the refusal was
    // about whom the requester is private to.
    let created = rig
        .change(&ChangeRequester::agent_recorded(PLANNER), "r-1", gamma())
        .await;
    assert_eq!(created.token(), "done", "{created:?}");
    rig.shutdown().await;
}

/// Who asked for an agent is shown to an agent only while the asker's own
/// record can be read and shows it: once the asker is removed, its record
/// says nothing any more, so an agent is not told its name. The host
/// reads who asked as before.
#[tokio::test(flavor = "multi_thread")]
async fn who_asked_for_an_agent_is_not_shown_to_an_agent_once_the_asker_cannot_be_read() {
    let temp = tempfile::tempdir().unwrap();
    let (runtime, asked, fail_next_session) = launch(temp.path(), false).await;
    let changes = json!(["create", "archive", "remove"]);
    let rule = |requester: Value| {
        json!({"change": changes, "requester": requester,
               "target": {"profile": ["worker", "planner"]}, "outcome": {"kind": "allow"}})
    };
    let policy = Arc::new(
        IdentityCreationPolicy::parse(&json!({
            "enabled": true,
            "peers": {"default": "allow"},
            "approval": {"rules": [
                rule(json!({"kind": "host"})),
                rule(json!({"kind": "agent", "profile": ["planner"]})),
            ]},
        }))
        .expect("the policy parses")
        .expect("the policy is enabled"),
    );
    assert!(runtime.attach_identity_policy(policy, IdentityFamilyOptions::new()));
    let rig = Rig {
        runtime,
        asked,
        step: Arc::new(RecordingStep::default()),
        fail_next_session,
    };
    let family = rig.family();
    let mob = rig.runtime.mob_handle();
    let host = ChangeRequester::host("ops");
    let planner = ChangeRequester::agent_recorded(PLANNER);
    let made = rig
        .change(
            &host,
            "maker-1",
            create_of(json!([{"name": "maker", "profile": "planner"}])),
        )
        .await;
    assert_eq!(made.token(), "done", "{made:?}");
    let created = rig
        .change(
            &ChangeRequester::agent_recorded("maker"),
            "scout-1",
            create_of(json!([{"name": "scout", "profile": "worker"}])),
        )
        .await;
    assert_eq!(created.token(), "done", "{created:?}");
    let asked_by = |requester: ChangeRequester| {
        let family = Arc::clone(&family);
        let mob = mob.clone();
        async move {
            match family
                .read(&mob, &requester, IdentityRead::Get(GetAgent::new("scout")))
                .await
            {
                Ok(IdentityReadAnswer::Get(record)) => {
                    record.agent.provenance.map(|asked| asked.requester)
                }
                other => panic!("a get answers one agent: {other:?}"),
            }
        }
    };
    assert_eq!(
        asked_by(planner.clone()).await.as_deref(),
        Some("maker"),
        "an agent that is shown the asker reads who asked"
    );

    let archived = rig
        .change(
            &host,
            "maker-2",
            IdentityChangeSpec::Archive(agent_ref("maker", &rig.digest("maker").await)),
        )
        .await;
    assert_eq!(archived.token(), "done", "{archived:?}");
    let removed = rig
        .change(
            &host,
            "maker-3",
            IdentityChangeSpec::Remove(agent_ref("maker", &rig.digest("maker").await)),
        )
        .await;
    assert_eq!(removed.token(), "done", "{removed:?}");
    assert!(rig.agent("maker").await.is_none(), "the asker is removed");

    assert_eq!(
        asked_by(planner).await,
        None,
        "an agent is not told an asker whose record cannot be read"
    );
    assert_eq!(
        asked_by(host).await.as_deref(),
        Some("maker"),
        "the host reads who asked"
    );
    rig.shutdown().await;
}

/// A member private to a person asks for no change of agents: the family
/// refuses it `private_requester` before anything is read of its request,
/// for a change and for a plan, and nothing is created.
#[tokio::test(flavor = "multi_thread")]
async fn a_member_private_to_a_person_asks_for_no_change() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), false, None).await;
    let private = ChangeRequester::agent_recorded(PLANNER).private_to_a_person(true);
    let refused = rig
        .change(
            &private,
            "r-1",
            create_of(json!([{"name": "confidant", "profile": "worker"}])),
        )
        .await;
    assert_eq!(
        refusal_of(&refused),
        ("private_requester".to_owned(), None),
        "{refused:?}"
    );
    assert!(
        rig.agent("confidant").await.is_none(),
        "nothing was created"
    );
    // The refusal comes before anything is read, the requester's own
    // record included: a private member whose name no agent holds is
    // refused for being private, not for its name.
    let stranger = ChangeRequester::agent_recorded("nobody").private_to_a_person(true);
    let refused = rig
        .change(
            &stranger,
            "r-1",
            create_of(json!([{"name": "confidant", "profile": "worker"}])),
        )
        .await;
    assert_eq!(
        refusal_of(&refused),
        ("private_requester".to_owned(), None),
        "{refused:?}"
    );

    let operation_id = OperationId::for_request(&private.recorded(), "r-2").unwrap();
    let planned = rig
        .family()
        .read(
            &rig.runtime.mob_handle(),
            &private,
            IdentityRead::Plan(Box::new(IdentityChangeCall::new(
                operation_id,
                create_of(json!([{"name": "confidant", "profile": "worker"}])),
            ))),
        )
        .await
        .expect_err("a private member plans no change");
    assert_eq!(planned.kind_token(), "private_requester", "{planned}");

    // The same member, not private, is admitted: the refusal was about
    // whom it is private to and nothing else.
    let open = ChangeRequester::agent_recorded(PLANNER);
    let created = rig
        .change(
            &open,
            "r-1",
            create_of(json!([{"name": "confidant", "profile": "worker"}])),
        )
        .await;
    assert_eq!(created.token(), "done", "{created:?}");
    rig.shutdown().await;
}

/// After an update that changed an agent's instructions the family asks
/// the instruction step with the text before and the text now, and puts its
/// answer on the agent's outcome; an update that names no instructions
/// says nothing of them and asks nothing. A removal tells the step the
/// agent was removed; an archive tells it nothing.
#[tokio::test(flavor = "multi_thread")]
async fn an_instructions_update_asks_the_instruction_step_and_a_removal_tells_it() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), false, None).await;
    let host = ChangeRequester::host("ops");
    let created = rig
        .change(
            &host,
            "create-1",
            create_of(json!([{"name": "scout", "profile": "worker",
                              "instructions": ["Watch the queue."]}])),
        )
        .await;
    assert_eq!(created.token(), "done", "{created:?}");
    let update = |digest: &Sha256Digest, set: Value| {
        IdentityChangeSpec::Update(
            serde_json::from_value(json!({
                "name": "scout", "expected_spec_digest": digest.as_str(), "set": set,
            }))
            .expect("an update as a surface writes it"),
        )
    };

    let digest = rig.digest("scout").await;
    let updated = rig
        .change(
            &host,
            "update-1",
            update(
                &digest,
                json!({"instructions": ["Watch the queue.", "Report hourly."]}),
            ),
        )
        .await;
    assert_eq!(updated.token(), "done", "{updated:?}");
    assert_eq!(
        updated.agents()[0].instructions,
        Some(InstructionOutcome::AppliesAtNextRun),
        "the step's answer is the agent's instruction outcome: {updated:?}"
    );
    assert_eq!(
        *rig.step.updated.lock().unwrap(),
        [(
            "scout".to_owned(),
            vec!["Watch the queue.".to_owned()],
            vec!["Watch the queue.".to_owned(), "Report hourly.".to_owned()],
        )],
        "asked once, with the text before and the text now"
    );

    let digest = rig.digest("scout").await;
    let renamed = rig
        .change(
            &host,
            "update-2",
            update(&digest, json!({"display_name": "Scout"})),
        )
        .await;
    assert_eq!(
        renamed.agents()[0].instructions,
        None,
        "an update that names no instructions says nothing of them: {renamed:?}"
    );
    assert_eq!(
        rig.step.updated.lock().unwrap().len(),
        1,
        "the step is not asked again"
    );

    let digest = rig.digest("scout").await;
    let archived = rig
        .change(
            &host,
            "archive-1",
            IdentityChangeSpec::Archive(agent_ref("scout", &digest)),
        )
        .await;
    assert_eq!(archived.token(), "done", "{archived:?}");
    assert!(
        rig.step.removed.lock().unwrap().is_empty(),
        "an archive tells the step nothing"
    );
    let restored = rig
        .change(
            &host,
            "unarchive-1",
            IdentityChangeSpec::Unarchive(agent_ref("scout", &digest)),
        )
        .await;
    assert_eq!(restored.token(), "done", "{restored:?}");
    let removed = rig
        .change(
            &host,
            "remove-1",
            IdentityChangeSpec::Remove(agent_ref("scout", &digest)),
        )
        .await;
    assert_eq!(removed.token(), "done", "{removed:?}");
    assert_eq!(*rig.step.removed.lock().unwrap(), ["scout".to_owned()]);
    rig.shutdown().await;
}

/// What became of an update's instructions is part of the answer of the
/// commit that made the change. The same operation committed again at the
/// seam is Core's replay: it is answered as committed, states nothing of
/// the instructions, and the step is not asked again. `unchanged` is the
/// answer of an update that names the text the agent's record held before
/// it, and the step is asked nothing for it.
#[tokio::test(flavor = "multi_thread")]
async fn an_update_core_replays_states_nothing_of_its_instructions() {
    use meerkat_meta::identity_creation::{
        ChangeCommit, CoreChangeExecutor, IdentityChangeExecutor, IdentityEntryFacts,
        RequestContext, RequestEnvelope, RequesterFacts, UpdateIdentitySpec, consent_of_plan,
        normalize_update,
    };

    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), false, None).await;
    let host = ChangeRequester::host("ops");
    let created = rig
        .change(
            &host,
            "create-1",
            create_of(json!([{"name": "scout", "profile": "worker",
                              "instructions": ["Watch the queue."]}])),
        )
        .await;
    assert_eq!(created.token(), "done", "{created:?}");

    let mob = rig.runtime.mob_handle();
    let policy = in_process_policy();
    let facts = RequesterFacts::host("ops");
    let profiles: std::collections::BTreeSet<String> = [
        "worker".to_owned(),
        "planner".to_owned(),
        "runner".to_owned(),
    ]
    .into();
    let peers = BTreeMap::new();
    let step = Arc::new(RecordingStep::default());
    let seam = CoreChangeExecutor::new()
        .with_instruction_step(Arc::clone(&step) as Arc<dyn InstructionStep>);
    let request_of = |digest: &Sha256Digest, set: Value| {
        let spec: UpdateIdentitySpec = serde_json::from_value(json!({
            "identity": "scout", "expected_spec_digest": digest.as_str(), "set": set,
        }))
        .expect("an update");
        normalize_update(
            &policy,
            &RequestContext {
                requester: &facts,
                profiles: &profiles,
                peers: &peers,
            },
            RequestEnvelope::default(),
            &IdentityEntryFacts::new("scout", digest.clone(), Some(1), "worker", BTreeMap::new()),
            &spec,
        )
        .expect("the update is allowed")
    };
    let reported = vec!["Watch the queue.".to_owned(), "Report hourly.".to_owned()];

    // The commit that makes the change answers what became of it.
    let before = rig.digest("scout").await;
    let update = request_of(
        &before,
        json!({"additional_instructions": reported.clone()}),
    );
    let consent = consent_of_plan(&update.plan).expect("the rules allow it");
    let operation = OperationId::for_request(&host.recorded(), "update-1").expect("an id");
    let first = seam
        .commit(ChangeCommit::new(
            &mob,
            &operation,
            &host,
            consent,
            &update.request,
        ))
        .await
        .expect("a typed outcome");
    assert_eq!(first.outcome.token(), "done", "{first:?}");
    assert!(!first.replayed, "{first:?}");
    assert_eq!(
        first.outcome.agents()[0].instructions,
        Some(InstructionOutcome::AppliesAtNextRun),
        "the step's answer: {first:?}"
    );

    // The same operation again is Core's replay: nothing of the
    // instructions, and the step is not asked again.
    let second = seam
        .commit(ChangeCommit::new(
            &mob,
            &operation,
            &host,
            consent,
            &update.request,
        ))
        .await
        .expect("a typed outcome");
    assert_eq!(second.outcome.token(), "done", "{second:?}");
    assert!(
        second.replayed,
        "Core answers the operation's receipt: {second:?}"
    );
    assert_eq!(
        second.outcome.agents()[0].instructions,
        None,
        "a replay states nothing of the instructions: {second:?}"
    );
    assert_eq!(
        step.updated.lock().unwrap().len(),
        1,
        "the step is asked once"
    );

    // Another update that names the text the record holds: `unchanged`.
    let now = rig.digest("scout").await;
    let same = request_of(
        &now,
        json!({"additional_instructions": reported, "display_name": "Scout"}),
    );
    let consent = consent_of_plan(&same.plan).expect("the rules allow it");
    let operation = OperationId::for_request(&host.recorded(), "update-2").expect("an id");
    let third = seam
        .commit(ChangeCommit::new(
            &mob,
            &operation,
            &host,
            consent,
            &same.request,
        ))
        .await
        .expect("a typed outcome");
    assert_eq!(third.outcome.token(), "done", "{third:?}");
    assert_eq!(
        third.outcome.agents()[0].instructions,
        Some(InstructionOutcome::Unchanged),
        "{third:?}"
    );
    assert_eq!(
        step.updated.lock().unwrap().len(),
        1,
        "the step is asked nothing for text the record held"
    );
    rig.shutdown().await;
}

/// The seam itself refuses an adoption when the spec the agent's host
/// configures now is not the one whose digest the request names: an
/// adoption that is carried out after the configuration moved (one a
/// person approved later) is `stale_spec` with the current digest, at
/// `expected_spec_digest`, and nothing is written. With the spec the
/// request read, the same request is adopted.
#[tokio::test(flavor = "multi_thread")]
async fn the_seam_refuses_an_adoption_of_a_spec_that_moved_since_it_was_read() {
    use meerkat_meta::identity_creation::{
        AdoptIdentitySpec, AgentSpecs, ChangeCommit, CoreChangeExecutor, IdentityChangeExecutor,
        IdentityEntryFacts, IdentityRefusalKind, RequestEnvelope, RequesterFacts, consent_of_plan,
        normalize_adopt,
    };

    /// What a host configures for its one agent, as the seam is told.
    struct Configured(DurableAgentSpec);

    #[async_trait]
    impl AgentSpecs for Configured {
        async fn configured_spec(
            &self,
            _mob: &meerkat_mob::MobHandle,
            name: &str,
        ) -> Option<DurableAgentSpec> {
            (self.0.identity.as_str() == name).then(|| self.0.clone())
        }

        fn is_authority_field(&self, _key: &str) -> bool {
            false
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), false, None).await;
    let host = ChangeRequester::host("ops");
    let mob = rig.runtime.mob_handle();
    let policy = in_process_policy();
    let facts = RequesterFacts::host("ops");
    let read = rig.digest(HOST_A).await;
    let adopt = normalize_adopt(
        &policy,
        &facts,
        RequestEnvelope::default(),
        &IdentityEntryFacts::new(HOST_A, read.clone(), None, "worker", BTreeMap::new()),
        &AdoptIdentitySpec::new(HOST_A, read.clone()),
    )
    .expect("the adoption is allowed");
    let consent = consent_of_plan(&adopt.plan).expect("the rules allow it");

    // The configuration moved since the request read it.
    let mut moved = roster_spec(HOST_A, "worker");
    moved.additional_instructions = vec!["Answer briefly.".to_owned()];
    let seam = CoreChangeExecutor::new().with_agent_specs(Arc::new(Configured(moved)));
    let operation = OperationId::for_request(&host.recorded(), "adopt-moved").expect("an id");
    let refused = seam
        .commit(
            ChangeCommit::new(&mob, &operation, &host, consent, &adopt.request)
                .of_configured_target(true),
        )
        .await
        .expect("a typed outcome");
    assert_eq!(
        refusal_of(&refused.outcome),
        (
            "stale_spec".to_owned(),
            Some("expected_spec_digest".to_owned())
        ),
        "{refused:?}"
    );
    let refusal = refused.outcome.refusal().expect("a refusal");
    assert!(
        matches!(
            &refusal.kind,
            IdentityRefusalKind::StaleSpec { current_digest, .. } if current_digest != &read
        ),
        "the refusal names the digest of the spec configured now: {refusal:?}"
    );
    assert_eq!(
        rig.digest(HOST_A).await,
        read,
        "nothing was written: the agent reads as it did"
    );

    // With the spec the request read, the same request is adopted.
    let seam = CoreChangeExecutor::new()
        .with_agent_specs(Arc::new(Configured(roster_spec(HOST_A, "worker"))));
    let operation = OperationId::for_request(&host.recorded(), "adopt-read").expect("an id");
    let adopted = seam
        .commit(
            ChangeCommit::new(&mob, &operation, &host, consent, &adopt.request)
                .of_configured_target(true),
        )
        .await
        .expect("a typed outcome");
    assert_eq!(adopted.outcome.token(), "done", "{adopted:?}");
    rig.shutdown().await;
}

/// A create whose first message does not start still commits: the agent
/// exists, its outcome says its seat is owed, and it has no member. The
/// next change of the family drives the owed seat again, with no restart:
/// the agent is seated and its first message runs once.
#[tokio::test(flavor = "multi_thread")]
async fn a_create_whose_first_message_does_not_start_is_owed_its_seat_and_seated_without_a_restart()
{
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot(temp.path(), false, None).await;
    let host = ChangeRequester::host("ops");
    rig.fail_next_session.armed.store(true, Ordering::SeqCst);
    let created = rig
        .change(
            &host,
            "create-waiter",
            create_of(json!([{"name": "waiter", "profile": "worker",
                              "first_message": "Begin the watch."}])),
        )
        .await;
    assert_eq!(created.token(), "done", "the create committed: {created:?}");
    assert_eq!(
        created.agents()[0].seat,
        Some(AgentSeat::Owed),
        "{created:?}"
    );
    assert!(created.agents()[0].session_id.is_none(), "{created:?}");
    let waiter = rig.agent("waiter").await.expect("the agent exists");
    assert_eq!(
        waiter.agent.readiness,
        AgentReadiness::Unseated,
        "{waiter:?}"
    );
    assert!(
        !rig.was_asked("Begin the watch."),
        "its first message did not start"
    );
    // Its name is taken, and the same request answers the same state.
    let taken = rig
        .change(
            &host,
            "create-waiter-2",
            create_of(json!([{"name": "waiter", "profile": "worker"}])),
        )
        .await;
    assert_eq!(refusal_of(&taken).0, "name_taken", "{taken:?}");

    // That change drove the owed seat again: the agent is seated now, and
    // its first message runs once.
    assert!(
        rig.asked("Begin the watch.").await,
        "its first message runs"
    );
    let waiter = rig.agent("waiter").await.expect("the agent exists");
    assert_ne!(
        waiter.agent.readiness,
        AgentReadiness::Unseated,
        "{waiter:?}"
    );
    let again = rig
        .change(
            &host,
            "create-other",
            create_of(json!([{"name": "other", "profile": "worker"}])),
        )
        .await;
    assert_eq!(again.token(), "done", "{again:?}");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        !rig.was_asked("Begin the watch."),
        "it runs once, not at every change"
    );
    rig.shutdown().await;
}

/// A mob whose runtime is governed runs no first message, so a create that
/// gives one is refused before anything is written: nothing is committed
/// and the name stays free. The same agent without a first message is
/// created, dormant.
#[tokio::test(flavor = "multi_thread")]
async fn a_governed_runtime_refuses_a_first_message_and_creates_without_one() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot(temp.path(), true, None).await;
    let host = ChangeRequester::host("ops");
    let refused = rig
        .change(
            &host,
            "create-1",
            create_of(json!([{"name": "plain", "profile": "worker"},
                             {"name": "waiter", "profile": "worker",
                              "first_message": "Begin the watch."}])),
        )
        .await;
    let refusal = refused.refusal().expect("the create is refused").clone();
    assert_eq!(refusal.kind_token(), "unsupported", "{refusal}");
    assert_eq!(
        refusal.field.as_deref(),
        Some("agents[1].first_message"),
        "{refusal}"
    );
    assert_eq!(
        refusal.to_refused()["feature"],
        json!("first_message_in_governed_realm"),
        "{refusal}"
    );
    assert!(!refusal.is_retryable());
    for name in ["plain", "waiter"] {
        assert!(
            rig.agent(name).await.is_none(),
            "{name}: nothing was committed"
        );
    }

    let created = rig
        .change(
            &host,
            "create-2",
            create_of(json!([{"name": "waiter", "profile": "worker"}])),
        )
        .await;
    assert_eq!(created.token(), "done", "the name was free: {created:?}");
    assert_eq!(
        created.agents()[0].seat,
        Some(AgentSeat::Seated),
        "{created:?}"
    );
    let waiter = rig.agent("waiter").await.expect("the agent exists");
    assert_eq!(
        waiter.agent.readiness,
        AgentReadiness::Dormant,
        "{waiter:?}"
    );
    assert!(!rig.was_asked("Begin the watch."), "nothing ran");
    rig.shutdown().await;
}

/// The host's own check runs on the normalized request before anything is
/// committed: a request it refuses answers `validation_refused` with the
/// host's reason, and nothing of it exists. Its consequence lines are in
/// the rendering a plan shows. Only the host holds the mob's changes.
#[tokio::test(flavor = "multi_thread")]
async fn the_hosts_own_check_refuses_a_request_before_it_commits() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), false, Some(Arc::new(NamingHooks))).await;
    let host = ChangeRequester::host("ops");
    let refused = rig
        .change(
            &host,
            "create-1",
            create_of(json!([{"name": "forbidden", "profile": "worker"}])),
        )
        .await;
    let refusal = refused.refusal().expect("the create is refused").clone();
    assert_eq!(refusal.kind_token(), "validation_refused", "{refusal}");
    assert_eq!(
        refusal.to_refused()["reason"],
        json!("the name forbidden is reserved here"),
        "{refusal}"
    );
    assert!(
        rig.agent("forbidden").await.is_none(),
        "nothing was committed"
    );
    let created = rig
        .change(
            &host,
            "create-2",
            create_of(json!([{"name": "fine", "profile": "worker"}])),
        )
        .await;
    assert_eq!(created.token(), "done", "{created:?}");

    let operation_id = OperationId::for_request(&host.recorded(), "plan-1").unwrap();
    let plan = match rig
        .family()
        .read(
            &rig.runtime.mob_handle(),
            &host,
            IdentityRead::Plan(Box::new(IdentityChangeCall::new(
                operation_id,
                create_of(json!([{"name": "planned", "profile": "worker"}])),
            ))),
        )
        .await
        .expect("the change is planned")
    {
        IdentityReadAnswer::Plan(plan) => plan,
        other => panic!("a plan answers a plan: {other:?}"),
    };
    assert!(
        plan.rendering.contains("This agent can read the ledger."),
        "the host's line is in the rendering: {}",
        plan.rendering
    );

    // The hold of the mob's changes is the host's: its operator's, or a
    // verified person's whom the host's surface admitted to it. An agent
    // holds nothing, and neither does a caller a surface could not verify.
    let hold: ChangesHoldRequest = serde_json::from_value(json!({"held": true})).unwrap();
    for (who, requester) in [
        ("an agent", ChangeRequester::agent_recorded(PLANNER)),
        ("an unverified caller", ChangeRequester::host("console")),
    ] {
        let denied = rig
            .family()
            .changes_hold(&rig.runtime.mob_handle(), &requester, hold.clone())
            .await
            .expect_err("it holds nothing");
        assert_eq!(denied.kind_token(), "change_denied", "{who}: {denied}");
    }
    for (who, requester) in [
        ("a verified person", ChangeRequester::human("person-1")),
        ("the host's operator", ChangeRequester::host_operator("ops")),
    ] {
        let held = rig
            .family()
            .changes_hold(&rig.runtime.mob_handle(), &requester, hold.clone())
            .await
            .unwrap_or_else(|refusal| panic!("{who} holds the changes: {refusal}"));
        assert_eq!(serde_json::to_value(held).unwrap(), json!({"held": true}));
    }
    rig.shutdown().await;
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

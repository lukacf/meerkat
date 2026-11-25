//! The mobs of a host over the REAL stdio gateway: `meta/mobs/*`.
//!
//! A persistent gateway is launched with its mob. Its requests resolve the
//! mobs its host creates beside the launch mob, so the gateway serves the
//! six methods and says so. The row is an operator's whole way with a
//! second mob: it asks a real gateway, over its own stdio as an SDK host
//! does, to create the mob, stop it, redefine it, resume it and destroy
//! it, reads it in between by the methods and by another method that names
//! it, and prints every request with its answer.
//!
//! A second row pins what a launch does with a `[mob] id` that is empty or
//! only white space: the launch judges no id, `meta/mobs/create` does.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

#[path = "support/gateway_shutdown.rs"]
mod gateway_shutdown;
use tempfile::TempDir;

/// Hang detector, not a latency bound.
const BACKSTOP: Duration = Duration::from_secs(90);

const LAUNCH_MOB: &str = "hosted-mobs-protocol-launch";
const SECOND_MOB: &str = "hosted-mobs-protocol-second";
const THIRD_MOB: &str = "hosted-mobs-protocol-third";

/// The one long-term agent of the memory row: the launch mob has it by the
/// launch's roster, and each hosted mob by a create of the stdio host.
const DESK: &str = "desk";

/// The roster the harness answers the gateway's roster callback with: the
/// agent `desk` on the profile every definition of this file has. A gateway
/// asks its launch's roster for its launch mob alone.
fn roster_of_one() -> Value {
    json!([{
        "identity": DESK,
        "profile": "default",
        "addressability": "addressable",
        "display_name": null,
        "labels": {},
        "context": null,
        "additional_instructions": []
    }])
}

fn mob_config(mob_id: &str) -> String {
    format!(
        "[mob]\nid = \"{mob_id}\"\n\n[profiles.default]\nmodel = \"gpt-5.5\"\nexternal_addressable = true\n"
    )
}

/// The TOML of [`mob_config`] with a profile a long-term agent can be
/// seated on: an agent of a mob is a member of it, and a member's profile
/// has comms on.
fn mob_config_for_agents(mob_id: &str) -> String {
    format!(
        "{}\n[profiles.default.tools]\ncomms = true\n",
        mob_config(mob_id)
    )
}

/// The identity policy of a launch whose stdio host may create agents: the
/// one rule allows the host's creates, and every other change is denied.
fn the_host_creates_agents() -> Value {
    json!({
        "enabled": true,
        "approval": { "rules": [{
            "change": ["create"],
            "requester": { "kind": "host", "surface": "rpc" },
            "outcome": { "kind": "allow" },
        }] },
    })
}

struct Gateway {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout_lines: mpsc::Receiver<String>,
    stderr_lines: Arc<Mutex<Vec<String>>>,
    _workspace: TempDir,
    state: TempDir,
    next_id: u64,
    /// What the harness answers the gateway's roster callback with; a
    /// gateway launched without a roster provider asks for none.
    roster: Option<Value>,
}

impl Gateway {
    fn spawn() -> Self {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let state = tempfile::tempdir().expect("state tempdir");
        let mut child = Command::new(env!("CARGO_BIN_EXE_rpc_gateway"))
            .arg("--persistent")
            .current_dir(workspace.path())
            .env("ANTHROPIC_API_KEY", "sk-ant-regression-test")
            .env("OPENAI_API_KEY", "sk-regression-test")
            .env("XDG_STATE_HOME", workspace.path().join("xdg-state"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn rpc_gateway");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        let stderr = child.stderr.take().expect("stderr");
        let stderr_lines = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&stderr_lines);
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                sink.lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(line);
            }
        });
        let (tx, stdout_lines) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            stdin: Some(stdin),
            stdout_lines,
            stderr_lines,
            _workspace: workspace,
            state,
            next_id: 0,
            roster: None,
        }
    }

    /// This gateway's host provides a roster: `roster` answers each roster
    /// callback, for whichever mob the gateway asks it.
    fn with_roster(mut self, roster: Value) -> Self {
        self.roster = Some(roster);
        self
    }

    fn send(&mut self, value: &Value) {
        let stdin = self.stdin.as_mut().expect("stdin open");
        writeln!(stdin, "{value}").expect("write");
        stdin.flush().expect("flush");
    }

    /// Next stdout message within the backstop, or a panic naming the wait.
    fn next(&mut self, waiting_for: &str) -> Value {
        let start = Instant::now();
        loop {
            let remaining = BACKSTOP.saturating_sub(start.elapsed());
            let line = self
                .stdout_lines
                .recv_timeout(remaining)
                .unwrap_or_else(|_| {
                    panic!(
                        "no stdout line while waiting for {waiting_for}\nstderr:\n{}",
                        self.stderr_lines.lock().unwrap().join("\n")
                    )
                });
            if let Ok(message) = serde_json::from_str::<Value>(line.trim()) {
                return message;
            }
        }
    }

    /// One request and its answer. A request of the gateway to its host that
    /// arrives meanwhile is answered "no handler", as a host without
    /// providers answers; the roster callback of a gateway launched with a
    /// roster is answered that roster. The pair is printed.
    fn call(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = format!("r{}", self.next_id);
        let request = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        self.send(&request);
        loop {
            let message = self.next(method);
            if message["id"] == json!(id) && message.get("method").is_none() {
                // The launch's own mob definition and the state directory are
                // long: the transcript names them by their id only.
                let shown = if method == "meta/init" {
                    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": "(the launch)" })
                } else {
                    request
                };
                println!("--> {shown}\n<-- {message}\n");
                return message;
            }
            if let (Some(asked), Some(asked_id)) = (
                message.get("method").and_then(Value::as_str),
                message.get("id").cloned(),
            ) {
                let answer = match (&self.roster, asked) {
                    (Some(roster), "callback/roster_provider/roster") => {
                        json!({ "jsonrpc": "2.0", "id": asked_id, "result": roster })
                    }
                    _ => json!({
                        "jsonrpc": "2.0", "id": asked_id,
                        "error": { "code": -32601, "message": format!("no handler for {asked}") },
                    }),
                };
                self.send(&answer);
            }
        }
    }

    /// The result of a request that must succeed.
    fn result(&mut self, method: &str, params: Value) -> Value {
        let answer = self.call(method, params);
        assert!(
            answer.get("error").is_none(),
            "{method} succeeds: {answer}\nstderr:\n{}",
            self.stderr_lines.lock().unwrap().join("\n")
        );
        answer["result"].clone()
    }

    /// The error of a request that must be refused.
    fn refusal(&mut self, method: &str, params: Value) -> Value {
        let answer = self.call(method, params);
        assert!(
            answer.get("error").is_some(),
            "{method} is refused: {answer}"
        );
        answer["error"].clone()
    }

    /// The gateway's process has ended: its stdin is closed and its exit is
    /// awaited within the backstop. A second gateway starts over the same
    /// state directory only once the first holds none of its locks, and an
    /// answered `meta/shutdown` is not yet the end of the process.
    fn ended(&mut self) {
        self.stdin = None;
        let start = Instant::now();
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) if start.elapsed() < BACKSTOP => thread::sleep(Duration::from_millis(20)),
                other => panic!(
                    "the gateway's process did not end after its shutdown: {other:?}\nstderr:\n{}",
                    self.stderr_lines.lock().unwrap().join("\n")
                ),
            }
        }
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The TOML of [`mob_config`] with a second profile, `reviewer`.
fn mob_config_with_a_reviewer(mob_id: &str) -> String {
    format!(
        "{}\n[profiles.reviewer]\nmodel = \"gpt-5.5\"\nexternal_addressable = true\n",
        mob_config(mob_id)
    )
}

/// The ids of the mobs `meta/mobs/list` answers, in its order.
fn listed_ids(gateway: &mut Gateway) -> Vec<String> {
    gateway.result("meta/mobs/list", json!({}))["mobs"]
        .as_array()
        .expect("a list of mobs")
        .iter()
        .map(|mob| mob["mob_id"].as_str().expect("a mob id").to_string())
        .collect()
}

/// An operator's whole way with a second mob, through a real gateway. The
/// gateway says that it serves the methods over the mobs of its host:
/// `meta/capabilities` names `hosted_mobs` and lists the six. It lists its
/// launch mob alone, creates a second mob and lists both, the launch mob
/// first; another method that names the second mob resolves it. The second
/// mob is stopped, redefined at the revision the gateway read, resumed with
/// the new definition and destroyed, and the launch mob runs on through all
/// of it: it is refused as the target of a destroy, and it is the one mob
/// left. The gateway then shuts down as a gateway of one mob does.
#[test]
fn an_operator_creates_stops_redefines_resumes_and_destroys_a_second_mob_through_a_real_gateway() {
    let mut gateway = Gateway::spawn();
    let state = gateway.state.path().to_path_buf();
    let launched = gateway.result(
        "meta/init",
        json!({ "mob_config": mob_config(LAUNCH_MOB), "persistent_state": state }),
    );
    assert!(launched["http_base_url"].is_string(), "{launched}");

    let capabilities = gateway.result("meta/capabilities", json!({}));
    let names = |member: &str| -> Vec<String> {
        capabilities[member]
            .as_array()
            .expect("a list of names")
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect()
    };
    let features = names("feature_capabilities");
    assert!(
        features.iter().any(|name| name == "mob_addressing"),
        "the answer read is the gateway's list of features: {features:?}"
    );
    assert_eq!(
        features
            .iter()
            .filter(|name| name.as_str() == "hosted_mobs")
            .count(),
        1,
        "a gateway that serves the methods names `hosted_mobs`, once: {features:?}"
    );
    let mut of_the_family: Vec<String> = names("methods")
        .into_iter()
        .filter(|method| method.starts_with("meta/mobs/"))
        .collect();
    of_the_family.sort();
    assert_eq!(
        of_the_family,
        vec![
            "meta/mobs/create",
            "meta/mobs/destroy",
            "meta/mobs/get",
            "meta/mobs/lifecycle",
            "meta/mobs/list",
            "meta/mobs/redefine",
        ],
        "and lists the six methods"
    );

    // A gateway of one mob lists its launch mob.
    let alone = gateway.result("meta/mobs/list", json!({}));
    let alone = alone["mobs"].as_array().expect("a list of mobs");
    assert_eq!(alone.len(), 1, "{alone:?}");
    assert_eq!(alone[0]["mob_id"], json!(LAUNCH_MOB), "{alone:?}");
    assert_eq!(alone[0]["default"], json!(true), "{alone:?}");
    assert_eq!(alone[0]["state"], json!("running"), "{alone:?}");

    // The second mob: created, listed after the launch mob, and resolved by
    // another method that names it.
    let created = gateway.result(
        "meta/mobs/create",
        json!({ "mob_config": mob_config(SECOND_MOB) }),
    );
    assert_eq!(
        created,
        json!({ "mob": {
            "mob_id": SECOND_MOB,
            "default": false,
            "state": "running",
            "provenance": "host",
            "revision": 1,
        }}),
        "the created mob is a mob of the host, running, at its first revision"
    );
    assert_eq!(listed_ids(&mut gateway), vec![LAUNCH_MOB, SECOND_MOB]);
    let second = gateway.result("meta/status", json!({ "mob_id": SECOND_MOB }));
    assert!(
        second["mob_state"]
            .as_str()
            .is_some_and(|phase| phase.contains("Running")),
        "another method that names the second mob resolves it: {second}"
    );
    let taken = gateway.refusal(
        "meta/mobs/create",
        json!({ "mob_config": mob_config(SECOND_MOB) }),
    );
    assert_eq!(taken["code"], json!(-32059), "{taken}");
    assert_eq!(taken["data"]["kind"], json!("mob_exists"), "{taken}");

    // Stopped, redefined at the revision the gateway read, resumed.
    let stopped = gateway.result(
        "meta/mobs/lifecycle",
        json!({ "mob_id": SECOND_MOB, "action": "stop" }),
    );
    assert_eq!(stopped["mob"]["state"], json!("stopped"), "{stopped}");
    assert!(
        stopped["stop_report"].is_object(),
        "a stop answers the mob's stop report: {stopped}"
    );
    let read = gateway.result("meta/mobs/get", json!({ "mob_id": SECOND_MOB }));
    assert_eq!(read["mob"]["state"], json!("stopped"), "{read}");
    assert_eq!(read["mob"]["revision"], json!(1), "{read}");
    let redefined = gateway.result(
        "meta/mobs/redefine",
        json!({
            "mob_id": SECOND_MOB,
            "mob_config": mob_config_with_a_reviewer(SECOND_MOB),
            "expected_revision": 1,
        }),
    );
    assert_eq!(
        redefined["mob"],
        json!({
            "mob_id": SECOND_MOB,
            "default": false,
            "state": "stopped",
            "provenance": "host",
            "revision": 2,
        }),
        "the redefined mob is loaded again, stopped, at the next revision"
    );
    assert_eq!(
        redefined["receipt"]["previous_revision"],
        json!(1),
        "{redefined}"
    );
    assert_eq!(
        redefined["receipt"]["committed_revision"],
        json!(2),
        "{redefined}"
    );
    let resumed = gateway.result(
        "meta/mobs/lifecycle",
        json!({ "mob_id": SECOND_MOB, "action": "resume" }),
    );
    assert_eq!(resumed["mob"]["state"], json!("running"), "{resumed}");
    assert_eq!(resumed["mob"]["revision"], json!(2), "{resumed}");

    // The launch mob is not destroyed through the methods; the second mob
    // is, and is then a mob the gateway does not have.
    let launch = gateway.refusal("meta/mobs/destroy", json!({ "mob_id": LAUNCH_MOB }));
    assert_eq!(launch["code"], json!(-32059), "{launch}");
    assert_eq!(launch["data"]["kind"], json!("default_mob"), "{launch}");
    let destroyed = gateway.result("meta/mobs/destroy", json!({ "mob_id": SECOND_MOB }));
    assert_eq!(destroyed["mob_id"], json!(SECOND_MOB), "{destroyed}");
    assert!(
        destroyed["destroy_report"].is_object(),
        "a destroy answers the destroy's report: {destroyed}"
    );
    assert_eq!(listed_ids(&mut gateway), vec![LAUNCH_MOB]);
    let second = gateway.refusal("meta/status", json!({ "mob_id": SECOND_MOB }));
    assert_eq!(second["code"], json!(-32602), "{second}");
    assert_eq!(
        second["data"]["kind"],
        json!("foreign_mob_target"),
        "{second}"
    );
    let status = gateway.result("meta/status", json!({}));
    assert!(
        status["mob_state"]
            .as_str()
            .is_some_and(|phase| phase.contains("Running")),
        "the launch mob runs on: {status}"
    );

    let shutdown = gateway.call("meta/shutdown", json!({}));
    gateway_shutdown::assert_persistent_shutdown_completed(
        &shutdown,
        "a gateway that created, changed and destroyed a second mob",
    );
}

/// Every file below `dir`, by its path from `dir`, with its bytes; nothing for a
/// directory that does not exist.
fn files_below(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, into: &mut BTreeMap<PathBuf, Vec<u8>>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, into);
            } else if let Ok(bytes) = std::fs::read(&path) {
                let name = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
                into.insert(name, bytes);
            }
        }
    }
    let mut files = BTreeMap::new();
    walk(dir, dir, &mut files);
    files
}

/// The names of the entries directly in `dir`, sorted; none for a directory that does
/// not exist.
fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// The params of an agent memory request for the agent `desk` of the mob `mob` (the
/// launch mob when none is named).
fn of_desk(mob: Option<&str>, mut params: Value) -> Value {
    params["identity"] = json!(DESK);
    if let Some(mob) = mob {
        params["mob_id"] = json!(mob);
    }
    params
}

/// The stdio host creates the long-term agent `desk` in the hosted mob
/// `mob`, as an operator's host does: by the family of the mob's agents,
/// under the launch's identity policy. `request_id` is the host's id of the
/// operation.
fn desk_created_in(gateway: &mut Gateway, mob: &str, request_id: &str) {
    let created = gateway.result(
        "meta/identity/create",
        json!({
            "mob_id": mob,
            "request_id": request_id,
            "agents": [{ "name": DESK, "profile": "default" }],
        }),
    );
    assert_eq!(
        created["outcome"],
        json!("done"),
        "the create of the agent `desk` in the mob '{mob}' commits: {created}"
    );
}

/// The agent `desk` of `mob` remembers one note titled `title`.
fn remembers(gateway: &mut Gateway, mob: Option<&str>, title: &str) {
    gateway.result(
        "meta/agent_memory/remember",
        of_desk(
            mob,
            json!({ "title": title, "body": format!("the note {title}") }),
        ),
    );
}

/// The titles the agent `desk` of `mob` recalls, sorted.
fn recalls(gateway: &mut Gateway, mob: Option<&str>) -> Vec<String> {
    let recalled = gateway.result(
        "meta/agent_memory/recall",
        of_desk(mob, json!({ "selection": "always", "max_entries": 64 })),
    );
    let mut titles: Vec<String> = recalled["records"]
        .as_array()
        .expect("a list of records")
        .iter()
        .map(|record| record["title"].as_str().expect("a title").to_string())
        .collect();
    titles.sort();
    titles
}

/// A destroy through a real gateway takes the destroyed mob's memory with it, and a mob
/// created under its id starts with none. The gateway is launched as an SDK host
/// launches it, with a roster provider, agent memory and an identity policy, and
/// nothing is bound by hand: its own launch declares and binds the release of a hosted
/// mob's memory.
///
/// The agent `desk` of the launch mob (of the launch's roster), of a second and of a
/// third mob (each created by the stdio host through `meta/identity/create`: a hosted
/// mob has no agent of the launch's roster) each remember a note of their own. The
/// second mob is destroyed through `meta/mobs/destroy`, asked
/// once. Three observations, each with its own check:
/// 1. after the destroy answered and BEFORE any create under the id: nothing of the
///    second mob's memory is left on disk: its directory is gone, under its own name
///    and under every other;
/// 2. then a mob created under the same id, with an agent `desk` of its own, recalls
///    nothing;
/// 3. the third mob's and the launch mob's memory files are byte for byte what they
///    were before the destroy.
#[test]
fn a_destroy_through_a_real_gateway_removes_the_mobs_memory_and_a_mob_created_under_its_id_recalls_nothing()
 {
    let mut gateway = Gateway::spawn().with_roster(roster_of_one());
    let state = gateway.state.path().to_path_buf();
    let launched = gateway.result(
        "meta/init",
        json!({
            "mob_config": mob_config_for_agents(LAUNCH_MOB),
            "persistent_state": state,
            "has_roster_provider": true,
            "runtime_options": {
                "agent_memory": true,
                "identity_creation": the_host_creates_agents(),
            },
        }),
    );
    assert!(launched["http_base_url"].is_string(), "{launched}");
    let memory_root = state.join("agent-memory");
    let hosted = memory_root.join("hosted-mobs.d");

    remembers(&mut gateway, None, "of the launch mob");
    gateway.result(
        "meta/mobs/create",
        json!({ "mob_config": mob_config_for_agents(SECOND_MOB) }),
    );
    desk_created_in(&mut gateway, SECOND_MOB, "desk-of-the-second-mob");
    remembers(&mut gateway, Some(SECOND_MOB), "of the second mob");
    let with_the_second = names_in(&hosted);
    assert_eq!(
        with_the_second.len(),
        1,
        "the second mob keeps its memory in one directory of its own: {with_the_second:?}"
    );
    let of_the_second = with_the_second[0].clone();
    gateway.result(
        "meta/mobs/create",
        json!({ "mob_config": mob_config_for_agents(THIRD_MOB) }),
    );
    desk_created_in(&mut gateway, THIRD_MOB, "desk-of-the-third-mob");
    remembers(&mut gateway, Some(THIRD_MOB), "of the third mob");
    let with_both = names_in(&hosted);
    assert_eq!(with_both.len(), 2, "{with_both:?}");
    let of_the_third = with_both
        .iter()
        .find(|name| **name != of_the_second)
        .expect("the third mob's directory")
        .clone();
    assert_eq!(recalls(&mut gateway, None), ["of the launch mob"]);
    assert_eq!(
        recalls(&mut gateway, Some(SECOND_MOB)),
        ["of the second mob"]
    );
    assert_eq!(recalls(&mut gateway, Some(THIRD_MOB)), ["of the third mob"]);
    assert!(
        !files_below(&hosted.join(&of_the_second)).is_empty(),
        "the second mob's memory is on disk before its destroy"
    );

    // What the other two keep, read after their last memory request: nothing of
    // theirs is asked from here to the end of the row's third observation.
    let of_the_launch_mob = |root: &Path| -> BTreeMap<PathBuf, Vec<u8>> {
        files_below(root)
            .into_iter()
            .filter(|(path, _)| !path.starts_with("hosted-mobs.d"))
            .collect()
    };
    let third_before = files_below(&hosted.join(&of_the_third));
    let launch_before = of_the_launch_mob(&memory_root);

    // The destroy, asked once: its first answer is the report.
    let destroyed = gateway.result("meta/mobs/destroy", json!({ "mob_id": SECOND_MOB }));
    assert!(
        destroyed["destroy_report"].is_object(),
        "the destroy answers its report: {destroyed}"
    );

    // OBSERVATION 1: after the destroy and before any create under the id.
    let after_the_destroy = names_in(&hosted);
    println!(
        "OBSERVATION 1 (after the destroy, before any create): {} entr(y/ies) under \
         hosted-mobs.d: {after_the_destroy:?}; the destroyed mob's was {of_the_second:?}",
        after_the_destroy.len()
    );
    assert_eq!(
        after_the_destroy,
        vec![of_the_third.clone()],
        "nothing of the destroyed mob's memory is left under any name"
    );
    assert!(!hosted.join(&of_the_second).exists());

    // OBSERVATION 3, first reading: the others' files, before any create.
    assert_eq!(
        files_below(&hosted.join(&of_the_third)),
        third_before,
        "the third mob's memory files are byte for byte what they were"
    );
    assert_eq!(
        of_the_launch_mob(&memory_root),
        launch_before,
        "and so are the launch mob's"
    );
    println!(
        "OBSERVATION 3 (before any create): the third mob's {} file(s) and the launch \
         mob's {} file(s) are byte-equal",
        third_before.len(),
        launch_before.len()
    );

    // OBSERVATION 2: a mob created under the same id recalls nothing.
    gateway.result(
        "meta/mobs/create",
        json!({ "mob_config": mob_config_for_agents(SECOND_MOB) }),
    );
    desk_created_in(
        &mut gateway,
        SECOND_MOB,
        "desk-of-the-mob-under-the-same-id",
    );
    let recalled = recalls(&mut gateway, Some(SECOND_MOB));
    println!("OBSERVATION 2 (a mob created under the same id): recalls {recalled:?}");
    assert_eq!(
        recalled,
        Vec::<String>::new(),
        "a mob created under the destroyed mob's id recalls nothing"
    );

    // OBSERVATION 3, second reading: the others are untouched by that create too.
    assert_eq!(
        files_below(&hosted.join(&of_the_third)),
        third_before,
        "the third mob's memory files are still byte for byte what they were"
    );
    assert_eq!(
        of_the_launch_mob(&memory_root),
        launch_before,
        "and so are the launch mob's"
    );
    assert_eq!(recalls(&mut gateway, Some(THIRD_MOB)), ["of the third mob"]);
    assert_eq!(recalls(&mut gateway, None), ["of the launch mob"]);

    let shutdown = gateway.call("meta/shutdown", json!({}));
    gateway_shutdown::assert_persistent_shutdown_completed(
        &shutdown,
        "a gateway that destroyed a mob with memory and created one under its id",
    );
}

/// A gateway started again over its state destroys a stored mob that is not loaded as
/// its FIRST request: the answer is the destroy's report. The destroy loads the mob to
/// judge it; the host ends its own entry of the mob before the destroy's storage
/// cleanup looks for the process's holders of the mob's database.
#[test]
fn a_gateway_started_again_destroys_a_stored_mob_as_its_first_request() {
    let mut first = Gateway::spawn();
    let state = first.state.path().to_path_buf();
    first.result(
        "meta/init",
        json!({ "mob_config": mob_config(LAUNCH_MOB), "persistent_state": state }),
    );
    first.result(
        "meta/mobs/create",
        json!({ "mob_config": mob_config(SECOND_MOB) }),
    );
    let shutdown = first.call("meta/shutdown", json!({}));
    gateway_shutdown::assert_persistent_shutdown_completed(
        &shutdown,
        "the first gateway over the state directory",
    );
    first.ended();

    // The second process over the same state directory (`first` still owns it).
    let mut again = Gateway::spawn();
    again.result(
        "meta/init",
        json!({ "mob_config": mob_config(LAUNCH_MOB), "persistent_state": state }),
    );
    let answer = again.call("meta/mobs/destroy", json!({ "mob_id": SECOND_MOB }));
    println!("THE FIRST ANSWER of the destroy after the restart: {answer}");
    assert!(
        answer.get("error").is_none() && answer["result"]["destroy_report"].is_object(),
        "the first request after a restart, a destroy of a stored mob that is not \
         loaded, is answered the report: {answer}"
    );
    assert_eq!(listed_ids(&mut again), vec![LAUNCH_MOB]);
    let shutdown = again.call("meta/shutdown", json!({}));
    gateway_shutdown::assert_persistent_shutdown_completed(
        &shutdown,
        "a gateway started again that destroyed a stored mob",
    );
}

/// The same first request on a gateway whose mobs have agents and memory: launched
/// with a roster provider, agent memory and an identity policy, so each mob it loads
/// gets an identity plane of its own. Before the restart the second mob's agent `desk`
/// remembered a note. The gateway started again is asked nothing before the destroy of
/// that mob, which it has not loaded. Two observations, each with its own check:
/// 1. the first answer is the destroy's report (no error: neither a destroy that could
///    not finish nor a memory release that failed);
/// 2. nothing of the destroyed mob's memory is left on disk under any name, and the
///    launch mob's agent recalls what it recalled.
#[test]
fn a_gateway_started_again_destroys_a_stored_mob_with_memory_as_its_first_request() {
    let launch = |gateway: &mut Gateway, state: &Path| {
        gateway.result(
            "meta/init",
            json!({
                "mob_config": mob_config_for_agents(LAUNCH_MOB),
                "persistent_state": state,
                "has_roster_provider": true,
                "runtime_options": {
                    "agent_memory": true,
                    "identity_creation": the_host_creates_agents(),
                },
            }),
        );
    };
    let mut first = Gateway::spawn().with_roster(roster_of_one());
    let state = first.state.path().to_path_buf();
    let hosted = state.join("agent-memory").join("hosted-mobs.d");
    launch(&mut first, &state);
    remembers(&mut first, None, "of the launch mob");
    first.result(
        "meta/mobs/create",
        json!({ "mob_config": mob_config_for_agents(SECOND_MOB) }),
    );
    desk_created_in(&mut first, SECOND_MOB, "desk-of-the-second-mob");
    remembers(&mut first, Some(SECOND_MOB), "of the second mob");
    let before_the_restart = names_in(&hosted);
    assert_eq!(
        before_the_restart.len(),
        1,
        "the second mob keeps its memory in one directory of its own: {before_the_restart:?}"
    );
    let shutdown = first.call("meta/shutdown", json!({}));
    gateway_shutdown::assert_persistent_shutdown_completed(
        &shutdown,
        "the first gateway over the state directory",
    );
    first.ended();
    assert_eq!(
        names_in(&hosted),
        before_the_restart,
        "the second mob's memory outlives the gateway that wrote it"
    );

    // The second process over the same state directory (`first` still owns it).
    let mut again = Gateway::spawn().with_roster(roster_of_one());
    launch(&mut again, &state);
    let answer = again.call("meta/mobs/destroy", json!({ "mob_id": SECOND_MOB }));
    println!("OBSERVATION 1 (the first answer of the destroy after the restart): {answer}");
    assert!(
        answer.get("error").is_none() && answer["result"]["destroy_report"].is_object(),
        "the first request after a restart, a destroy of a stored mob with memory that \
         is not loaded, is answered the report: {answer}"
    );
    let after_the_destroy = names_in(&hosted);
    println!(
        "OBSERVATION 2 (after that destroy): {} entr(y/ies) under hosted-mobs.d: \
         {after_the_destroy:?}; the destroyed mob's was {before_the_restart:?}",
        after_the_destroy.len()
    );
    assert_eq!(
        after_the_destroy,
        Vec::<String>::new(),
        "nothing of the destroyed mob's memory is left under any name"
    );
    assert_eq!(recalls(&mut again, None), ["of the launch mob"]);
    assert_eq!(listed_ids(&mut again), vec![LAUNCH_MOB]);
    let shutdown = again.call("meta/shutdown", json!({}));
    gateway_shutdown::assert_persistent_shutdown_completed(
        &shutdown,
        "a gateway started again that destroyed a stored mob with memory",
    );
}

/// A launch takes the id its mob's definition gives: nothing at the launch
/// judges it, and only `meta/mobs/create` refuses a blank one. A launch
/// whose `[mob] id` is empty starts, serves and shuts down as any other. A
/// launch whose id is only white space is refused at its start, where the
/// mob gets its WorkGraph realm, and starts nothing. The row pins both as
/// they are.
#[test]
fn a_launch_takes_an_empty_mob_id_and_refuses_one_of_only_white_space_at_its_start() {
    let mut gateway = Gateway::spawn();
    let state = gateway.state.path().to_path_buf();
    let launched = gateway.result(
        "meta/init",
        json!({ "mob_config": mob_config(""), "persistent_state": state }),
    );
    assert!(launched["http_base_url"].is_string(), "{launched}");
    let status = gateway.result("meta/status", json!({}));
    assert!(
        status["mob_state"]
            .as_str()
            .is_some_and(|phase| phase.contains("Running")),
        "a launch mob with an empty id runs: {status}"
    );
    let shutdown = gateway.call("meta/shutdown", json!({}));
    gateway_shutdown::assert_persistent_shutdown_completed(
        &shutdown,
        "a gateway whose launch mob has an empty id",
    );

    let mut blank = Gateway::spawn();
    let state = blank.state.path().to_path_buf();
    let refused = blank.refusal(
        "meta/init",
        json!({ "mob_config": mob_config("   "), "persistent_state": state }),
    );
    assert_eq!(refused["code"], json!(-32603), "{refused}");
    assert!(
        refused["message"]
            .as_str()
            .is_some_and(|message| message.contains("has no valid WorkGraph realm")),
        "the start is refused where the mob gets its WorkGraph realm: {refused}"
    );
}

//! Member socket endpoints (`runtime_options.member_comms_address`) on one
//! stock `rpc_gateway --persistent` process.
//!
//! The option gives the gateway's members a signed TCP endpoint, and network
//! comms give every member of the gateway one signing key. So a gateway has
//! one member with a socket endpoint: the first member built holds the place
//! until it is retired. These rows pin what every other member is answered
//! and what it leaves behind:
//!
//! * a second member (an ensured member, a helper, a fork) is refused in
//!   plain text that names it and nothing of the member that holds the
//!   place, before anything is built or recorded:
//!   it is not listed, it binds no socket, the first member keeps its
//!   endpoint and its in-process route;
//! * the gateway starts again on the same state afterwards;
//! * retiring the holder frees the place for another member;
//! * a roster that declares two members starts: the first member built
//!   works, the other is listed broken with the refusal, and retiring it
//!   leaves the first as it was;
//! * at a restart the member whose endpoint is recorded holds the place
//!   ahead of a declared member that was never built;
//! * a single member on a fixed address keeps its peer id and address across
//!   a restart (the twin the refusal must not disturb).
//!
//! Every gateway is the shipped binary driven over its stdin protocol, with
//! `demo_llm` (no network, no credential).

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args
)]

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

#[path = "support/gateway_shutdown.rs"]
mod gateway_shutdown;

const INIT_TIMEOUT: Duration = Duration::from_mins(3);
const RPC_TIMEOUT: Duration = Duration::from_mins(1);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_mins(3);

const MOB: &str = r#"
[mob]
id = "socket-members"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.worker.tools]
comms = true
"#;

/// What a refusal must not show a caller: the mob machine's own rejection.
const MACHINE_WORDS: [&str; 3] = ["DSL authority", "guard rejected", "MobMachine"];
/// The comms registry's notice that a registration took another runtime's route.
const EVICTION_NOTICE: &str = "replaced an existing route";
/// The gateway's one start warning for a roster of several members.
const SEVERAL_MEMBERS_WARNING: &str =
    "serves one member per gateway; the first member built holds it";

/// The refusal `member` is answered while another member has the gateway's
/// place. It names the refused member only.
fn refusal_of(member: &str) -> String {
    format!(
        "member '{member}' cannot be created: this gateway runs with the member socket option \
         (member_comms_address), which serves one member per gateway, and another member of \
         this gateway already holds its comms key. Retire that member first, or run the \
         members without the option."
    )
}

/// Every string in `value`, for the facts of a member a refusal must not
/// carry.
fn strings_of(value: &Value, into: &mut Vec<String>) {
    match value {
        Value::String(text) => into.push(text.clone()),
        Value::Array(items) => items.iter().for_each(|item| strings_of(item, into)),
        Value::Object(fields) => fields.values().for_each(|field| strings_of(field, into)),
        _ => {}
    }
}

fn probe_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("probe port")
        .local_addr()
        .expect("probe addr")
        .port()
}

struct Gateway {
    label: String,
    child: Child,
    stdin: Option<ChildStdin>,
    lines: mpsc::Receiver<String>,
    stderr: Arc<Mutex<Vec<String>>>,
    next_id: u64,
    /// The identities this boot's roster provider declares, each on the
    /// `worker` profile; `None` for a gateway without a roster provider.
    roster: Option<Vec<&'static str>>,
}

impl Gateway {
    fn start(label: &str, roster: Option<&[&'static str]>) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_rpc_gateway"))
            .arg("--persistent")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn rpc_gateway --persistent");
        let stdin = child.stdin.take().expect("gateway stdin");
        let stdout = child.stdout.take().expect("gateway stdout");
        let child_stderr = child.stderr.take().expect("gateway stderr");

        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let stderr_sink = stderr.clone();
        thread::spawn(move || {
            for line in BufReader::new(child_stderr).lines() {
                let Ok(line) = line else { break };
                let Ok(mut sink) = stderr_sink.lock() else {
                    break;
                };
                if sink.len() < 2000 {
                    sink.push(line);
                }
            }
        });
        Self {
            label: label.to_string(),
            child,
            stdin: Some(stdin),
            lines: rx,
            stderr,
            next_id: 0,
            roster: roster.map(<[&'static str]>::to_vec),
        }
    }

    fn send(&mut self, message: &Value) {
        let stdin = self.stdin.as_mut().expect("gateway stdin remains open");
        writeln!(
            stdin,
            "{}",
            serde_json::to_string(message).expect("message")
        )
        .expect("write to gateway stdin");
        stdin.flush().expect("flush gateway stdin");
    }

    /// The SDK side of the gateway's callback bridge: the roster.
    fn answer_callback(&mut self, id: &Value, method: &str) {
        let answer = match (method, self.roster.as_ref()) {
            ("callback/roster_provider/roster", Some(roster)) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": roster
                    .iter()
                    .map(|identity| json!({ "identity": identity, "profile": "worker" }))
                    .collect::<Vec<_>>(),
            }),
            _ => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("no handler for {method}") },
            }),
        };
        self.send(&answer);
    }

    fn stderr_lines(&self) -> Vec<String> {
        self.stderr
            .lock()
            .map(|sink| sink.clone())
            .unwrap_or_default()
    }

    fn stderr_tail(&self) -> String {
        let lines = self.stderr_lines();
        let start = lines.len().saturating_sub(40);
        format!(
            "--- [{}] gateway stderr (last {} of {} lines) ---\n{}",
            self.label,
            lines.len() - start,
            lines.len(),
            lines[start..].join("\n")
        )
    }

    /// One request and its response message, whether a result or an error.
    fn exchange(&mut self, method: &str, params: Value, deadline: Duration) -> Value {
        self.next_id += 1;
        let id = format!("{}-{}", self.label, self.next_id);
        let request = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        self.send(&request);
        let start = Instant::now();
        loop {
            let Some(remaining) = deadline.checked_sub(start.elapsed()) else {
                panic!(
                    "[{}] no response to {method} within {deadline:?}\n{}",
                    self.label,
                    self.stderr_tail()
                );
            };
            let Ok(line) = self.lines.recv_timeout(remaining) else {
                panic!(
                    "[{}] gateway stdout closed while waiting for {method}\n{}",
                    self.label,
                    self.stderr_tail()
                );
            };
            let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            if message.get("id").and_then(Value::as_str) == Some(id.as_str())
                && message.get("method").is_none()
            {
                return message;
            }
            // A request of the gateway's own: a callback to answer.
            if let (Some(callback), Some(callback_id)) = (
                message.get("method").and_then(Value::as_str),
                message.get("id"),
            ) {
                let (callback, callback_id) = (callback.to_string(), callback_id.clone());
                self.answer_callback(&callback_id, &callback);
            }
        }
    }

    /// The result of a request that must succeed.
    fn result(&mut self, method: &str, params: Value) -> Value {
        let message = self.exchange(method, params, RPC_TIMEOUT);
        assert!(
            message.get("error").is_none(),
            "[{}] {method} failed: {message}\n{}",
            self.label,
            self.stderr_tail()
        );
        message["result"].clone()
    }

    /// The error text of a request that must be refused.
    fn refusal(&mut self, method: &str, params: Value) -> String {
        let message = self.exchange(method, params, RPC_TIMEOUT);
        let Some(error) = message.get("error") else {
            panic!(
                "[{}] {method} must be refused, got {message}\n{}",
                self.label,
                self.stderr_tail()
            );
        };
        error.to_string()
    }

    fn init(&mut self, state: &Path, member_comms_address: &str) -> Value {
        let mut params = json!({
            "persistent_state": state,
            "mob_config": MOB,
            "runtime_options": {
                "demo_llm": true,
                "member_comms_address": member_comms_address,
            }
        });
        if self.roster.is_some() {
            params["has_roster_provider"] = json!(true);
        }
        self.exchange("meta/init", params, INIT_TIMEOUT)
    }

    fn ensure_member(&mut self, member: &str) -> Value {
        self.exchange(
            "meta/ensure_member",
            json!({ "role": "worker", "agent_identity": member }),
            RPC_TIMEOUT,
        )
    }

    /// The listed members, as the gateway reports them.
    fn member_entries(&mut self) -> Vec<Value> {
        let listed = self.result("meta/list_members", json!({}));
        listed
            .as_array()
            .unwrap_or_else(|| panic!("list_members must return an array: {listed}"))
            .clone()
    }

    /// Each listed member's identity and state.
    fn members(&mut self) -> Vec<(String, String)> {
        let mut members: Vec<_> = self
            .member_entries()
            .iter()
            .map(|entry| {
                (
                    entry["agent_identity"].as_str().unwrap_or("?").to_string(),
                    entry["state"].as_str().unwrap_or("?").to_string(),
                )
            })
            .collect();
        members.sort();
        members
    }

    /// The listed members once `settled` holds for their identities and
    /// states; the row fails with what is listed when it never does.
    fn members_once(
        &mut self,
        what: &str,
        settled: impl Fn(&[(String, String)]) -> bool,
    ) -> Vec<(String, String)> {
        let deadline = Instant::now() + RPC_TIMEOUT;
        loop {
            let members = self.members();
            if settled(&members) {
                return members;
            }
            if Instant::now() >= deadline {
                let listed = self.member_entries();
                panic!(
                    "[{}] {what}: listed {listed:?}\n{}",
                    self.label,
                    self.stderr_tail()
                );
            }
            thread::sleep(Duration::from_millis(200));
        }
    }

    /// `text`, a refusal or a broken member's reason, carries nothing of
    /// `holder`, the member that has the place: not its name, its mob, its
    /// session or anything of its endpoint (peer id, key, address).
    fn assert_nothing_of(&mut self, holder: &str, text: &str) {
        let mut facts = vec![holder.to_string(), "socket-members".to_string()];
        strings_of(&self.peer_info(holder), &mut facts);
        let entry = self
            .member_entries()
            .into_iter()
            .find(|entry| entry["agent_identity"].as_str() == Some(holder))
            .unwrap_or_else(|| panic!("[{}] {holder} is not listed", self.label));
        for key in [
            "session_id",
            "bridge_session_id",
            "agent_runtime_id",
            "peer_id",
        ] {
            if let Some(value) = entry.get(key).and_then(Value::as_str) {
                facts.push(value.to_string());
            }
        }
        // Short generic words of an endpoint (a scheme, a role) say nothing
        // of the member.
        for fact in facts
            .iter()
            .filter(|fact| fact.as_str() == holder || fact.len() >= 8)
        {
            assert!(
                !text.contains(fact.as_str()),
                "[{}] a refusal names nothing of the member that holds the place, but it \
                 carries {fact:?}: {text}",
                self.label
            );
        }
    }

    /// Why the gateway lists `member` broken.
    fn broken_reason(&mut self, member: &str) -> String {
        let entry = self
            .member_entries()
            .into_iter()
            .find(|entry| entry["agent_identity"].as_str() == Some(member))
            .unwrap_or_else(|| panic!("[{}] {member} is not listed", self.label));
        assert_eq!(
            entry["state"], "broken",
            "[{}] {member} must be listed broken: {entry}",
            self.label
        );
        entry["error"].as_str().unwrap_or_default().to_string()
    }

    /// Ask the gateway to build the declared member `identity` now; the
    /// answer, whether a result or an error.
    fn warm(&mut self, identity: &str) -> Value {
        self.exchange(
            "meta/identity/warm",
            json!({ "identity": identity }),
            RPC_TIMEOUT,
        )
    }

    fn peer_info(&mut self, member: &str) -> Value {
        self.result("meta/cross_mob/peer_info", json!({ "member_id": member }))
    }

    /// The number of TCP sockets this gateway process listens on.
    #[cfg(target_os = "linux")]
    fn listen_sockets(&self) -> usize {
        let mut listening = std::collections::HashSet::new();
        for table in ["/proc/net/tcp", "/proc/net/tcp6"] {
            let Ok(text) = std::fs::read_to_string(table) else {
                continue;
            };
            for line in text.lines().skip(1) {
                let fields: Vec<&str> = line.split_whitespace().collect();
                // st is field 3 (0A = LISTEN), inode is field 9.
                if fields.get(3) == Some(&"0A")
                    && let Some(inode) = fields.get(9)
                {
                    listening.insert((*inode).to_string());
                }
            }
        }
        let fds = std::fs::read_dir(format!("/proc/{}/fd", self.child.id())).expect("gateway fds");
        fds.filter_map(Result::ok)
            .filter_map(|fd| std::fs::read_link(fd.path()).ok())
            .filter_map(|target| {
                let target = target.to_string_lossy().into_owned();
                target
                    .strip_prefix("socket:[")
                    .and_then(|rest| rest.strip_suffix(']'))
                    .map(str::to_string)
            })
            .filter(|inode| listening.contains(inode))
            .count()
    }

    fn shutdown_and_reap(mut self) {
        let response = self.exchange("meta/shutdown", json!({}), SHUTDOWN_TIMEOUT);
        gateway_shutdown::assert_persistent_shutdown_completed(
            &response,
            &format!("[{}] shutdown\n{}", self.label, self.stderr_tail()),
        );
        drop(self.stdin.take());
        let status = self.child.wait().expect("wait for gateway exit");
        assert!(
            status.success(),
            "[{}] rpc_gateway exited with {status}\n{}",
            self.label,
            self.stderr_tail()
        );
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn boot_with(
    label: &str,
    state: &Path,
    member_comms_address: &str,
    roster: Option<&[&'static str]>,
) -> Gateway {
    let mut gateway = Gateway::start(label, roster);
    let response = gateway.init(state, member_comms_address);
    assert!(
        response.get("error").is_none(),
        "[{label}] the gateway must start on this state: {response}\n{}",
        gateway.stderr_tail()
    );
    gateway
}

fn boot(label: &str, state: &Path, member_comms_address: &str) -> Gateway {
    boot_with(label, state, member_comms_address, None)
}

fn assert_ensured(gateway: &mut Gateway, member: &str) {
    let response = gateway.ensure_member(member);
    assert!(
        response.get("error").is_none(),
        "[{}] ensure_member {member} failed: {response}\n{}",
        gateway.label,
        gateway.stderr_tail()
    );
}

/// One way of asking the gateway for another member.
struct Request<'a> {
    /// The member it would create.
    member: &'a str,
    method: &'a str,
    params: Value,
}

fn ensure(member: &str) -> Request<'_> {
    Request {
        member,
        method: "meta/ensure_member",
        params: json!({ "role": "worker", "agent_identity": member }),
    }
}

fn helper(member: &str) -> Request<'_> {
    Request {
        member,
        method: "meta/spawn_helper",
        params: json!({
            "agent_identity": member,
            "task": "say one word",
            "result_label": "answer",
            "max_text_bytes": 4096,
        }),
    }
}

fn fork<'a>(source: &str, member: &'a str) -> Request<'a> {
    Request {
        member,
        method: "meta/fork_helper",
        params: json!({
            "source_member_id": source,
            "agent_identity": member,
            "task": "say one word",
            "result_label": "answer",
            "max_text_bytes": 4096,
        }),
    }
}

/// The refusal of another member while `first` holds the gateway's place,
/// and everything it must leave as it was.
fn assert_refused_cleanly(gateway: &mut Gateway, first: &str, request: Request<'_>) {
    let first_info = gateway.peer_info(first);
    #[cfg(target_os = "linux")]
    let sockets_before = gateway.listen_sockets();

    let second = request.member;
    let response = gateway.exchange(request.method, request.params, RPC_TIMEOUT);
    let Some(error) = response.get("error") else {
        panic!(
            "[{}] {} of a second member must be refused on a gateway whose members share one \
             comms key, got {response}",
            gateway.label, request.method
        );
    };
    let text = error.to_string();
    for word in MACHINE_WORDS {
        assert!(
            !text.contains(word),
            "[{}] the refusal must be plain text, not the machine's rejection ({word}): {text}",
            gateway.label
        );
    }
    assert!(
        text.contains(&refusal_of(second)),
        "[{}] the refusal must name the refused member and say what to do: {text}",
        gateway.label
    );
    assert!(
        error.get("data").is_none_or(Value::is_null),
        "[{}] the refusal carries no data: {error}",
        gateway.label
    );
    gateway.assert_nothing_of(first, &text);

    assert_eq!(
        gateway.members(),
        [(first.to_string(), "active".to_string())],
        "[{}] the refused member must not be listed, and the first member stays active",
        gateway.label
    );
    let lookup = gateway.refusal("meta/get_member", json!({ "member_id": second }));
    assert!(
        lookup.contains("not found"),
        "[{}] the refused member must not exist: {lookup}",
        gateway.label
    );
    #[cfg(target_os = "linux")]
    assert_eq!(
        gateway.listen_sockets(),
        sockets_before,
        "[{}] the refused member must not leave a listening socket",
        gateway.label
    );
    assert_eq!(
        gateway.peer_info(first),
        first_info,
        "[{}] the first member's endpoint must be unchanged",
        gateway.label
    );
    assert_no_route_was_taken(gateway);
}

fn assert_no_route_was_taken(gateway: &Gateway) {
    let evictions: Vec<_> = gateway
        .stderr_lines()
        .into_iter()
        .filter(|line| line.contains(EVICTION_NOTICE))
        .collect();
    assert!(
        evictions.is_empty(),
        "[{}] no member may take another member's in-process route: {evictions:?}",
        gateway.label
    );
}

/// The one member listed `state` among exactly the two members a roster of
/// two declares.
fn the_one(members: &[(String, String)], state: &str) -> String {
    let mut matching = members
        .iter()
        .filter(|(_, listed)| listed == state)
        .map(|(member, _)| member.clone());
    let one = matching
        .next()
        .unwrap_or_else(|| panic!("no member is {state}: {members:?}"));
    assert!(
        matching.next().is_none(),
        "exactly one member is {state}: {members:?}"
    );
    one
}

#[test]
fn a_second_socket_member_on_kernel_picked_ports_is_refused_cleanly_and_the_gateway_starts_again() {
    let state = tempfile::tempdir().expect("state dir");
    let mut gateway = boot("port-zero", state.path(), "127.0.0.1:0");
    assert_ensured(&mut gateway, "alice");

    assert_refused_cleanly(&mut gateway, "alice", ensure("carol"));
    gateway.shutdown_and_reap();

    let mut restarted = boot("port-zero-restart", state.path(), "127.0.0.1:0");
    let members = restarted.members();
    assert_eq!(
        members
            .iter()
            .map(|(member, _)| member.as_str())
            .collect::<Vec<_>>(),
        ["alice"],
        "only the first member exists after the restart: {members:?}"
    );
    restarted.shutdown_and_reap();
}

#[test]
fn a_second_socket_member_on_a_fixed_port_is_refused_cleanly_and_the_first_survives_a_restart() {
    let state = tempfile::tempdir().expect("state dir");
    let address = format!("127.0.0.1:{}", probe_port());
    let mut gateway = boot("fixed-port", state.path(), &address);
    assert_ensured(&mut gateway, "alice");
    let alice = gateway.peer_info("alice");

    assert_refused_cleanly(&mut gateway, "alice", ensure("carol"));
    gateway.shutdown_and_reap();

    let mut restarted = boot("fixed-port-restart", state.path(), &address);
    assert_eq!(
        restarted.members(),
        [("alice".to_string(), "active".to_string())]
    );
    assert_eq!(
        restarted.peer_info("alice"),
        alice,
        "the first member keeps its key, peer id and address across the restart"
    );
    restarted.shutdown_and_reap();
}

/// A helper and a fork are members too: each is refused as a second member
/// is, and the fork before its source's transcript is copied for it.
#[test]
fn a_helper_and_a_fork_are_refused_as_second_socket_members() {
    let state = tempfile::tempdir().expect("state dir");
    let mut gateway = boot("helper-fork", state.path(), "127.0.0.1:0");
    assert_ensured(&mut gateway, "alice");

    assert_refused_cleanly(&mut gateway, "alice", helper("helper-one"));
    assert_refused_cleanly(&mut gateway, "alice", fork("alice", "fork-one"));
    gateway.shutdown_and_reap();
}

/// The place is the holder's until it is retired, then the next member's.
#[test]
fn retiring_the_socket_member_frees_the_place_for_another_member() {
    let state = tempfile::tempdir().expect("state dir");
    let mut gateway = boot("retire-frees", state.path(), "127.0.0.1:0");
    assert_ensured(&mut gateway, "alice");
    assert_refused_cleanly(&mut gateway, "alice", ensure("carol"));

    gateway.result("meta/retire_member", json!({ "member_id": "alice" }));
    gateway.members_once("the retired member leaves the list", <[_]>::is_empty);
    assert_ensured(&mut gateway, "carol");
    assert_eq!(
        gateway.members(),
        [("carol".to_string(), "active".to_string())]
    );
    assert_refused_cleanly(&mut gateway, "carol", ensure("alice"));
    assert_no_route_was_taken(&gateway);
    gateway.shutdown_and_reap();
}

/// A roster of two members under the option: the start succeeds and warns
/// once, the first member built works, the other is listed broken with the
/// refusal, and retiring it leaves the first as it was.
#[test]
fn a_roster_of_two_socket_members_starts_with_one_member_and_the_other_broken() {
    let state = tempfile::tempdir().expect("state dir");
    let roster = ["alice", "zed"];
    let mut gateway = boot_with("roster-two", state.path(), "127.0.0.1:0", Some(&roster));
    let asked: Vec<Value> = roster
        .iter()
        .map(|identity| gateway.warm(identity))
        .collect();
    let count = |members: &[(String, String)], state: &str| {
        members.iter().filter(|(_, listed)| listed == state).count()
    };
    let members = gateway.members_once(
        &format!("one member is built and the other refused (asked: {asked:?})"),
        |members| {
            members.len() == 2 && count(members, "active") == 1 && count(members, "broken") == 1
        },
    );
    let holder = the_one(&members, "active");
    let refused = the_one(&members, "broken");
    let reason = gateway.broken_reason(&refused);
    assert!(
        reason.contains(&refusal_of(&refused)),
        "the refused member is listed with the refusal as its restore failure: {reason}"
    );
    let warnings = gateway
        .stderr_lines()
        .into_iter()
        .filter(|line| line.contains(SEVERAL_MEMBERS_WARNING))
        .count();
    assert_eq!(
        warnings,
        1,
        "the start warns once that the roster declares more members than the option serves\n{}",
        gateway.stderr_tail()
    );
    gateway.assert_nothing_of(&holder, &reason);
    let holder_info = gateway.peer_info(&holder);
    #[cfg(target_os = "linux")]
    let sockets = gateway.listen_sockets();

    // A member the roster still declares is seated again by the next roster
    // pass, so the declaration is withdrawn with the retire.
    gateway.result("meta/retire_member", json!({ "member_id": refused }));
    gateway.roster = Some(
        roster
            .iter()
            .copied()
            .filter(|identity| *identity == holder)
            .collect(),
    );
    gateway.result("meta/reconcile_identity", json!({}));
    gateway.members_once("the refused member is retired", |members| {
        members == [(holder.clone(), "active".to_string())]
    });
    assert_eq!(
        gateway.peer_info(&holder),
        holder_info,
        "the first member keeps its endpoint"
    );
    #[cfg(target_os = "linux")]
    assert_eq!(
        gateway.listen_sockets(),
        sockets,
        "the refused member never had a socket"
    );
    assert_no_route_was_taken(&gateway);
    gateway.shutdown_and_reap();
}

/// At a restart the member whose endpoint is recorded is restored and holds
/// the place again, whichever of the declared members it is, and the other
/// stays broken with nothing of the holder in its reason.
#[test]
fn at_a_restart_the_member_with_the_recorded_endpoint_holds_the_place() {
    let state = tempfile::tempdir().expect("state dir");
    let address = format!("127.0.0.1:{}", probe_port());
    let roster = ["alice", "zed"];
    let count = |members: &[(String, String)], state: &str| {
        members.iter().filter(|(_, listed)| listed == state).count()
    };

    let mut first = boot_with("recorded-1", state.path(), &address, Some(&roster));
    let asked: Vec<Value> = roster.iter().map(|identity| first.warm(identity)).collect();
    let members = first.members_once(
        &format!("one member is built and the other refused (asked: {asked:?})"),
        |members| {
            members.len() == 2 && count(members, "active") == 1 && count(members, "broken") == 1
        },
    );
    let holder = the_one(&members, "active");
    let refused = the_one(&members, "broken");
    let holder_info = first.peer_info(&holder);
    first.shutdown_and_reap();

    let mut gateway = boot_with("recorded-2", state.path(), &address, Some(&roster));
    // The member that was not built is asked for first.
    let asked = [gateway.warm(&refused), gateway.warm(&holder)];
    gateway.members_once(
        &format!("the recorded member is restored and the other is not (asked: {asked:?})"),
        |members| {
            let listed = |member: &str, state: &str| {
                members
                    .iter()
                    .any(|(name, listed)| name == member && listed == state)
            };
            members.len() == 2 && listed(&holder, "active") && listed(&refused, "broken")
        },
    );
    assert_eq!(
        gateway.peer_info(&holder),
        holder_info,
        "the restored member keeps its key, peer id and address"
    );
    // Core fails the restore of a member that was never built before its
    // session is created, with its own reason; whatever the reason says, it
    // carries nothing of the member that holds the place.
    let reason = gateway.broken_reason(&refused);
    gateway.assert_nothing_of(&holder, &reason);
    assert_no_route_was_taken(&gateway);
    gateway.shutdown_and_reap();
}

/// Twin: a single member on a fixed address, with no second member asked for.
#[test]
fn one_socket_member_on_a_fixed_port_keeps_its_endpoint_across_a_restart() {
    let state = tempfile::tempdir().expect("state dir");
    let address = format!("127.0.0.1:{}", probe_port());
    let mut gateway = boot("one-member", state.path(), &address);
    assert_ensured(&mut gateway, "alice");
    let alice = gateway.peer_info("alice");
    let key = std::fs::read(state.path().join(".rkat/identity/identity.key"))
        .expect("the gateway's member comms key");
    gateway.shutdown_and_reap();

    let mut restarted = boot("one-member-restart", state.path(), &address);
    assert_eq!(
        restarted.members(),
        [("alice".to_string(), "active".to_string())]
    );
    assert_eq!(restarted.peer_info("alice"), alice);
    assert_eq!(
        std::fs::read(state.path().join(".rkat/identity/identity.key")).expect("the key"),
        key,
        "the member comms key file is unchanged"
    );
    restarted.shutdown_and_reap();
}

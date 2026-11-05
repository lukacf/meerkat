//! The operator of an `rkat-rpc` stdio connection decides durable decisions.
//!
//! The observable is the real binary on a SQLite realm. The test holds the
//! decisions through the realm's own persistence: before the server starts,
//! or, opened beside it (realm leases are shared), while its connection is
//! open. When the client lists (`decision/list`), the connection presents to
//! its operator the open decisions of the returned page that are addressed to
//! an approver id its realm configuration lists for the operator (`operator`
//! unless `[governance.decisions] operator_approver_ids` says otherwise),
//! each once, as a
//! `decision/notice` carrying the whole rendering, and records what it showed
//! as a presentation to the approver on its own channel. The operator decides
//! through `decision/decide` only a decision the connection presented, and
//! only while its approver id is still listed; a client's own
//! `decision/present` records a host presenter's showing and never makes a
//! decision decidable.

// The operator is this process's operating-system user, which only a Unix
// build with local authorization has.
#![cfg(all(unix, feature = "local-authorization"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::disallowed_methods
)]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use meerkat::governed_realm::stock::StockPrincipals;
use meerkat_core::approval::decision::{
    ApproverId, DecisionHoldRequest, DecisionKind, DecisionOperationId, DecisionPrincipalKey,
    DecisionRenderer, DecisionRendering, DecisionRequestBody, DecisionSubjectKey, Sha256Digest,
};
use meerkat_core::connection::RealmId;
use meerkat_core::{PrincipalKind, PrincipalRef, TrustDomainId};
use serde_json::{Value, json};
use tempfile::TempDir;

const REALM_ID: &str = "rpc-stdio-operator-decisions";
const RENDERING: &str = "Release build 7 to production, with the change log attached.";
const DECISION_NOTICE: &str = "decision/notice";

/// Bound on one answer. Startup under a fully parallel crate run takes
/// about 70 s; the bound stays below the CI unit profile's 240 s ceiling.
const ANSWER_DEADLINE: Duration = Duration::from_secs(200);
/// Bound on a notice the server owes once it answered the request that
/// should have produced it.
const NOTICE_DEADLINE: Duration = Duration::from_secs(30);

/// Cargo and nextest advertise the crate's own binary to its integration
/// tests through the runtime environment (never at compile time, so the
/// test stays valid inside a nextest archive). A missing binary fails the
/// test instead of skipping it.
fn rkat_rpc_binary() -> PathBuf {
    let advertised = std::env::var_os("CARGO_BIN_EXE_rkat-rpc")
        .map(PathBuf::from)
        .expect("cargo advertises CARGO_BIN_EXE_rkat-rpc to the crate's integration tests");
    assert!(
        advertised.exists(),
        "CARGO_BIN_EXE_rkat-rpc names a missing binary: {}",
        advertised.display()
    );
    advertised
}

fn now_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_millis(),
    )
    .expect("milliseconds fit u64")
}

fn requester_key() -> DecisionPrincipalKey {
    let builder = PrincipalRef::in_domain(
        PrincipalKind::PersonalAgent,
        "builder",
        TrustDomainId::new("example.org").expect("trust domain"),
    )
    .expect("principal");
    DecisionPrincipalKey::of(&builder).expect("principal key")
}

/// The key of the realm's local operator: this process's operating-system
/// user, as a human of the realm's own trust domain.
fn operator_key() -> String {
    let realm = RealmId::parse(REALM_ID).expect("realm id");
    let principals = StockPrincipals::for_local_operator(&realm).expect("the local operator");
    DecisionPrincipalKey::of(&principals.operator)
        .expect("principal key")
        .to_string()
}

fn subject_digest() -> String {
    Sha256Digest::of(RENDERING.as_bytes()).to_string()
}

/// The hold of `subject` for `approver`, until far in the future.
fn hold_request(subject: &str, approver: &str) -> DecisionHoldRequest {
    DecisionHoldRequest::new(
        DecisionKind::new("release").expect("kind"),
        DecisionSubjectKey::new(subject).expect("subject key"),
        DecisionOperationId::new(format!("operation:{subject}")).expect("operation id"),
        Sha256Digest::of(br#"{"build":7}"#),
        DecisionRequestBody::from_value(&json!({"build": 7})).expect("request body"),
        requester_key(),
        ApproverId::new(approver).expect("approver id"),
        u64::MAX / 2,
        DecisionRendering::new(RENDERING).expect("rendering"),
        DecisionRenderer::new("render.v1").expect("renderer"),
    )
}

/// One SQLite realm in a temporary state root.
struct Realm {
    _temp: TempDir,
    project: PathBuf,
    state_root: PathBuf,
    user_root: PathBuf,
}

impl Realm {
    fn new() -> Self {
        let temp = TempDir::new().expect("temp dir");
        let project = temp.path().join("project");
        let state_root = temp.path().join("state");
        let user_root = temp.path().join("user");
        std::fs::create_dir_all(project.join(".rkat")).expect("project root");
        std::fs::create_dir_all(&user_root).expect("user root");
        std::fs::create_dir_all(&state_root).expect("state root");
        Self {
            _temp: temp,
            project,
            state_root,
            user_root,
        }
    }

    /// Replace the realm's head config document.
    fn write_config(&self, document: &str) {
        let paths = meerkat_store::realm_paths_in(&self.state_root, REALM_ID);
        std::fs::create_dir_all(&paths.root).expect("realm dir");
        std::fs::write(&paths.config_path, document).expect("head config");
    }

    /// Hold the decision of `subject` for `approver` through the realm's own
    /// persistence, opened in this process, and close it again. Returns the
    /// decision id.
    fn hold(&self, subject: &str, approver: &str) -> String {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime to open the realm");
        runtime.block_on(async {
            let (_, bundle) = meerkat::open_realm_persistence_in(
                &self.state_root,
                REALM_ID,
                Some(meerkat_store::RealmBackend::Sqlite),
                Some(meerkat_store::RealmOrigin::Explicit),
            )
            .await
            .expect("open the realm");
            let held = bundle
                .decision_owner()
                .hold(hold_request(subject, approver), false)
                .expect("hold the decision");
            held.decision_id.to_string()
        })
    }
}

/// One `rkat-rpc` stdio connection: the child's stdin and its stdout lines.
struct Server {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    next_id: u64,
    /// Every `decision/notice` the connection sent, in order.
    notices: Vec<Value>,
}

impl Server {
    /// Start `rkat-rpc` on `realm` over stdio and initialize the connection.
    fn start(realm: &Realm) -> Self {
        let mut child = Command::new(rkat_rpc_binary())
            .current_dir(&realm.project)
            .args([
                "--realm",
                REALM_ID,
                "--realm-backend",
                "sqlite",
                "--state-root",
                path_arg(&realm.state_root),
                "--context-root",
                path_arg(&realm.project),
                "--user-config-root",
                path_arg(&realm.user_root),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("rkat-rpc spawns");
        let stdin = child.stdin.take().expect("stdin pipe");
        let stdout = child.stdout.take().expect("stdout pipe");
        let (send, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        let mut server = Self {
            child,
            stdin,
            lines,
            next_id: 0,
            notices: Vec::new(),
        };
        let initialized = server.call("initialize", json!({}));
        assert!(
            initialized.get("result").is_some(),
            "initialize answers: {initialized}"
        );
        server
    }

    /// Send one request and wait for its answer, keeping every notice that
    /// arrives before it.
    fn call(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        writeln!(self.stdin, "{request}").expect("write the request");
        self.stdin.flush().expect("flush the request");
        let deadline = Instant::now() + ANSWER_DEADLINE;
        loop {
            let message = self
                .next_message(deadline)
                .unwrap_or_else(|| panic!("no answer to {method} within {ANSWER_DEADLINE:?}"));
            if message.get("id").and_then(Value::as_u64) == Some(id) {
                return message;
            }
        }
    }

    /// The next message the server wrote before `deadline`, `None` once it
    /// passes. A `decision/notice` is kept.
    fn next_message(&mut self, deadline: Instant) -> Option<Value> {
        loop {
            let left = deadline.checked_duration_since(Instant::now())?;
            let line = match self.lines.recv_timeout(left) {
                Ok(line) => line,
                Err(RecvTimeoutError::Timeout) => return None,
                Err(RecvTimeoutError::Disconnected) => panic!("rkat-rpc closed its output"),
            };
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if message.get("method").and_then(Value::as_str) == Some(DECISION_NOTICE) {
                self.notices.push(message["params"].clone());
            }
            return Some(message);
        }
    }

    /// The notice presenting `decision_id`, waiting for it if it has not
    /// arrived yet.
    fn presented(&mut self, decision_id: &str) -> Value {
        let deadline = Instant::now() + NOTICE_DEADLINE;
        loop {
            if let Some(notice) = self.notices.iter().find(|notice| {
                notice["decision_id"] == decision_id && notice["event"] == "presented"
            }) {
                return notice.clone();
            }
            if self.next_message(deadline).is_none() {
                panic!(
                    "the connection presented no {decision_id}; its notices: {:?}",
                    self.notices
                );
            }
        }
    }

    fn get(&mut self, decision_id: &str) -> Value {
        result(&self.call("decision/get", json!({"decision_id": decision_id})))["decision"].clone()
    }

    /// List as the client does; returns the page.
    fn list(&mut self, params: Value) -> Value {
        result(&self.call("decision/list", params)).clone()
    }

    fn decide(&mut self, decision_id: &str, approve: bool) -> Value {
        self.call(
            "decision/decide",
            json!({
                "decision_id": decision_id,
                "approve": approve,
                "presented_digest": subject_digest(),
            }),
        )
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn path_arg(path: &Path) -> &str {
    path.to_str().expect("utf-8 path")
}

fn result(response: &Value) -> &Value {
    assert!(
        response.get("error").is_none(),
        "expected success, got {response}"
    );
    &response["result"]
}

fn assert_refused(response: &Value, refusal: &str) {
    let error = response
        .get("error")
        .unwrap_or_else(|| panic!("expected the {refusal} refusal, got {response}"));
    assert_eq!(error["code"], -32602, "{response}");
    assert_eq!(error["data"]["kind"], "decision_refused", "{response}");
    assert_eq!(
        error["data"]["decision_refusal"]["kind"], refusal,
        "{response}"
    );
}

fn never_presented(decision: &Value) -> bool {
    decision
        .get("first_presented_to_approver_at_ms")
        .is_none_or(Value::is_null)
}

/// The decision ids of a listed page, in its order.
fn listed_ids(page: &Value) -> Vec<String> {
    page["decisions"]
        .as_array()
        .expect("a page of decisions")
        .iter()
        .map(|decision| {
            decision["decision_id"]
                .as_str()
                .expect("a decision id")
                .to_owned()
        })
        .collect()
}

/// The decisions the `presented` notices named, in order.
fn presented_ids(notices: &[Value]) -> Vec<String> {
    notices
        .iter()
        .filter(|notice| notice["event"] == "presented")
        .map(|notice| {
            notice["decision_id"]
                .as_str()
                .expect("a decision id")
                .to_owned()
        })
        .collect()
}

#[test]
fn a_decision_held_before_the_start_is_presented_when_the_client_first_lists() {
    let realm = Realm::new();
    let operators = realm.hold("build-7", "operator");
    let owners = realm.hold("build-8", "team:owners");

    let mut server = Server::start(&realm);
    // Starting presents nothing: the client's list is its ask.
    assert!(server.notices.is_empty(), "{:?}", server.notices);
    let page = server.list(json!({}));
    let notice = server.presented(&operators);

    assert_eq!(notice["kind"], "release");
    assert_eq!(notice["phase"], "open");
    assert_eq!(notice["approver_id"], "operator");
    assert_eq!(notice["requester"], requester_key().to_string());
    assert_eq!(
        notice["presentation"],
        json!({
            "rendering": RENDERING,
            "renderer": "render.v1",
            "subject_digest": subject_digest(),
            "parts": [{"index": 0, "count": 1, "part_digest": subject_digest()}],
        })
    );
    // The page the client got shows the presentation its list caused.
    let listed = page["decisions"]
        .as_array()
        .expect("a page of decisions")
        .iter()
        .find(|decision| decision["decision_id"] == operators)
        .expect("the operator's decision is listed");
    assert!(!never_presented(listed), "{listed}");
    // The connection recorded what it showed: to the approver, the whole
    // rendering, on its own channel, under the notice's id.
    let decision = server.get(&operators);
    let shown = &decision["presentations"][0];
    assert_eq!(
        shown["audience"],
        json!({"kind": "approver", "approver_id": "operator"})
    );
    assert_eq!(shown["subject_digest"], subject_digest());
    assert_eq!(shown["message_ref"], notice["notice_id"], "{shown}");
    assert!(
        shown["channel_ref"]
            .as_str()
            .is_some_and(|channel| channel.starts_with("rkat-rpc:stdio:")),
        "{shown}"
    );
    // Nothing presents a decision for an approver id the operator does not
    // act for.
    assert!(never_presented(&server.get(&owners)));
    assert_eq!(presented_ids(&server.notices), vec![operators]);
}

#[test]
fn a_connection_that_never_lists_presents_nothing_and_cannot_decide() {
    let realm = Realm::new();
    let decision_id = realm.hold("build-7", "operator");

    let mut server = Server::start(&realm);
    let refused = server.decide(&decision_id, true);

    assert_refused(&refused, "not_eligible");
    assert!(server.notices.is_empty(), "{:?}", server.notices);
    let decision = server.get(&decision_id);
    assert_eq!(decision["phase"], "open");
    assert!(never_presented(&decision), "{decision}");
}

#[test]
fn each_page_the_client_lists_presents_its_own_decisions() {
    let realm = Realm::new();
    let held: Vec<String> = ["build-1", "build-2", "build-3"]
        .into_iter()
        .map(|subject| realm.hold(subject, "operator"))
        .collect();

    let mut server = Server::start(&realm);
    let first = server.list(json!({"limit": 2}));
    let first_ids = listed_ids(&first);
    assert_eq!(first_ids.len(), 2, "{first}");
    for decision_id in &first_ids {
        server.presented(decision_id);
    }
    let rest: Vec<String> = held
        .iter()
        .filter(|decision_id| !first_ids.contains(decision_id))
        .cloned()
        .collect();
    let [last] = rest.as_slice() else {
        panic!("one decision is left for the next page: {rest:?}");
    };
    // Not on a page the client listed yet: neither presented nor decidable.
    assert_refused(&server.decide(last, true), "not_eligible");
    assert!(never_presented(&server.get(last)));

    let after = first["next_after"].as_str().expect("a next page");
    let second = server.list(json!({"limit": 2, "after": after}));
    assert_eq!(listed_ids(&second), vec![last.clone()], "{second}");
    server.presented(last);
    let decided = server.decide(last, true);
    assert_eq!(
        result(&decided)["decision"]["phase"],
        "approved",
        "{decided}"
    );
    // Each decision was presented once, on the page that listed it.
    let mut shown = presented_ids(&server.notices);
    shown.sort();
    let mut every = held.clone();
    every.sort();
    assert_eq!(shown, every);
}

#[test]
fn the_operator_decides_a_decision_presented_on_its_connection() {
    let realm = Realm::new();
    let decision_id = realm.hold("build-7", "operator");

    let mut server = Server::start(&realm);
    server.list(json!({}));
    server.presented(&decision_id);
    let decided = server.decide(&decision_id, true);

    let decision = &result(&decided)["decision"];
    assert_eq!(decision["phase"], "approved", "{decision}");
    let evidence = &decision["outcome"]["evidence"];
    assert_eq!(evidence["kind"], "human", "{evidence}");
    assert_eq!(evidence["assurance"], "device_trusted", "{evidence}");
    assert_eq!(evidence["ingress"], "rkat-rpc-stdio", "{evidence}");
    assert_eq!(evidence["approver_id"], "operator", "{evidence}");
    assert_eq!(evidence["presented_digest"], subject_digest(), "{evidence}");
    assert_eq!(evidence["decider"], operator_key(), "{evidence}");
}

#[test]
fn an_approver_id_not_listed_for_the_operator_is_neither_presented_nor_decided() {
    let realm = Realm::new();
    let decision_id = realm.hold("build-8", "team:owners");

    let mut server = Server::start(&realm);
    // The client's list shows it, and presents nothing the operator does not
    // act for.
    let page = server.list(json!({}));
    assert_eq!(listed_ids(&page), vec![decision_id.clone()], "{page}");
    let refused = server.decide(&decision_id, true);

    assert_refused(&refused, "not_eligible");
    assert!(server.notices.is_empty(), "{:?}", server.notices);
    let decision = server.get(&decision_id);
    assert_eq!(decision["phase"], "open");
    assert!(never_presented(&decision), "{decision}");
}

#[test]
fn a_decision_held_while_the_connection_is_open_is_presented_when_the_client_lists() {
    let realm = Realm::new();
    let mut server = Server::start(&realm);
    let decision_id = realm.hold("build-9", "operator");

    // The client's own presentation records a host presenter's showing; it
    // does not make the connection eligible to decide.
    let rendering =
        result(&server.call("decision/rendering", json!({"decision_id": decision_id}))).clone();
    let presented = server.call(
        "decision/present",
        json!({
            "decision_id": decision_id,
            "presentation": {
                "audience": {"kind": "approver", "approver_id": "operator"},
                "subject_digest": subject_digest(),
                "parts_shown": rendering["parts"],
                "channel_ref": "client:window",
                "message_ref": "client-message-1",
                "at_ms": now_ms(),
            },
        }),
    );
    result(&presented);
    assert_refused(&server.decide(&decision_id, true), "not_eligible");

    // The client lists: the connection presents it, and then its operator
    // decides it.
    server.list(json!({"approver_id": "operator"}));
    server.presented(&decision_id);
    let decided = server.decide(&decision_id, true);
    assert_eq!(
        result(&decided)["decision"]["phase"],
        "approved",
        "{decided}"
    );
}

#[test]
fn an_approver_id_the_config_lists_for_the_operator_is_presented_and_decided() {
    let realm = Realm::new();
    realm.write_config(
        "[governance.decisions]\noperator_approver_ids = [\"operator\", \"team:owners\"]\n",
    );
    let decision_id = realm.hold("build-8", "team:owners");

    let mut server = Server::start(&realm);
    server.list(json!({}));
    let notice = server.presented(&decision_id);
    assert_eq!(notice["approver_id"], "team:owners");
    let decided = server.decide(&decision_id, false);

    let decision = &result(&decided)["decision"];
    assert_eq!(decision["phase"], "rejected", "{decision}");
    assert_eq!(
        decision["outcome"]["evidence"]["approver_id"],
        "team:owners"
    );
}

#[test]
fn an_approver_id_no_longer_listed_is_refused_at_decide() {
    let realm = Realm::new();
    realm.write_config(
        "[governance.decisions]\noperator_approver_ids = [\"operator\", \"team:owners\"]\n",
    );
    let decision_id = realm.hold("build-8", "team:owners");

    let mut server = Server::start(&realm);
    server.list(json!({}));
    server.presented(&decision_id);
    // The operator's currentness is that its configuration still lists the
    // approver id when the decide arrives.
    realm.write_config("[governance.decisions]\noperator_approver_ids = [\"operator\"]\n");
    let refused = server.decide(&decision_id, true);

    assert_refused(&refused, "not_eligible");
    assert_eq!(server.get(&decision_id)["phase"], "open");
}

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::uninlined_format_args
)]

//! Durable decisions on the stdio gateway (`meta/decisions/*`): the host
//! reads, presents and withdraws the decisions of the runtime's state
//! directory, a list notifies the host of each open decision once, the
//! operator decides where the approver map lists it, and a declared ingress
//! attests the person who decided. Decisions are held in the test through
//! Core's own decision owner on the same state directory, as a host
//! composition or another process on the store would hold them.

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use meerkat_core::approval::decision::{
    ApproverId, DecisionHoldRequest, DecisionId, DecisionKind, DecisionOperationId, DecisionPhase,
    DecisionPrincipalKey, DecisionReference, DecisionRenderer, DecisionRendering,
    DecisionRequestBody, DecisionSubjectKey, PresentationAudience, PresentationRecord,
    Sha256Digest, StoredDecision,
};
use meerkat_core::{PrincipalKind, PrincipalRef, TrustDomainId};
use meerkat_runtime::decision::DurableDecisionService;
use serde_json::{Value, json};

const MOB_CONFIG: &str = r#"
[mob]
id = "gateway-decisions-test"

[profiles.default]
model = "gpt-5.5"
"#;

const RENDERING: &str = "Release build 7 to production, with the change log attached.";
const NOTICE_METHOD: &str = "meta/decisions/notice";
const LINE_WAIT: Duration = Duration::from_mins(2);

// ---------------------------------------------------------------------------
// The gateway process
// ---------------------------------------------------------------------------

/// One `rpc_gateway` process on a pipe: requests go to its stdin, and a
/// reader thread hands every stdout line to the test.
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

    /// Send `method` and read stdout until its answer: the answer, and the
    /// decision notices the gateway wrote before it.
    fn call(&mut self, method: &str, params: Value) -> (Value, Vec<Value>) {
        self.next_id += 1;
        let id = format!("decisions-{}", self.next_id);
        let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        writeln!(self.stdin, "{request}").expect("write request");
        self.stdin.flush().expect("flush request");
        let mut notices = Vec::new();
        loop {
            let line = self
                .lines
                .recv_timeout(LINE_WAIT)
                .unwrap_or_else(|_| panic!("no answer to {method}"));
            let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            if message.get("id") == Some(&json!(id)) {
                return (message, notices);
            }
            if message.get("method") == Some(&json!(NOTICE_METHOD)) {
                notices.push(message["params"].clone());
            }
        }
    }

    fn rpc(&mut self, method: &str, params: Value) -> Value {
        self.call(method, params).0
    }

    /// `meta/init` with the test mob, `persistent_state` when given, and
    /// `runtime_options`.
    fn init(&mut self, state: Option<&Path>, runtime_options: Value) -> Value {
        let mut params = json!({ "mob_config": MOB_CONFIG, "runtime_options": runtime_options });
        if let Some(state) = state {
            params["persistent_state"] = json!(state);
        }
        self.rpc("meta/init", params)
    }

    /// A gateway that answered its init, with `runtime_options`.
    fn ready(scratch: &Path, state: Option<&Path>, runtime_options: Value) -> Self {
        let mut gateway = Self::start(scratch);
        let init = gateway.init(state, runtime_options);
        assert!(
            init["result"]["contract_version"].is_string(),
            "init failed: {init}"
        );
        gateway
    }

    fn kill(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ---------------------------------------------------------------------------
// Decisions held through Core's owner on the gateway's state directory
// ---------------------------------------------------------------------------

/// The decision owner of the state directory `state`, as the runtime's
/// persistence opens it (`<state>/approval_decisions.sqlite3`).
fn decisions_of(state: &Path) -> DurableDecisionService {
    meerkat::ApprovalIssuerSelection::Persistent {
        store_path: state.to_path_buf(),
    }
    .open_decisions()
}

fn principal_in(kind: PrincipalKind, id: &str, domain: &str) -> PrincipalRef {
    PrincipalRef::in_domain(kind, id, TrustDomainId::new(domain).expect("trust domain"))
        .expect("principal")
}

fn requester_key() -> DecisionPrincipalKey {
    DecisionPrincipalKey::of(&principal_in(
        PrincipalKind::PersonalAgent,
        "builder",
        "example.org",
    ))
    .expect("principal key")
}

/// The principal key of the person `subject` as the declared ingress
/// `ingress` attests them.
fn ingress_person_key(ingress: &str, subject: &str) -> DecisionPrincipalKey {
    DecisionPrincipalKey::of(&principal_in(
        PrincipalKind::Human,
        subject,
        &format!("meta-ingress-{ingress}"),
    ))
    .expect("principal key")
}

fn hold_request(
    kind: &str,
    subject: &str,
    approver: &str,
    requester: DecisionPrincipalKey,
) -> DecisionHoldRequest {
    DecisionHoldRequest::new(
        DecisionKind::new(kind).expect("kind"),
        DecisionSubjectKey::new(subject).expect("subject key"),
        DecisionOperationId::new(format!("operation:{subject}")).expect("operation id"),
        Sha256Digest::of(br#"{"build":7}"#),
        DecisionRequestBody::from_value(&json!({"build": 7})).expect("request body"),
        requester,
        ApproverId::new(approver).expect("approver id"),
        u64::MAX / 2,
        DecisionRendering::new(RENDERING).expect("rendering"),
        DecisionRenderer::new("render.v1").expect("renderer"),
    )
}

fn hold(owner: &DurableDecisionService, subject: &str, approver: &str) -> StoredDecision {
    hold_for(owner, subject, approver, requester_key())
}

fn hold_for(
    owner: &DurableDecisionService,
    subject: &str,
    approver: &str,
    requester: DecisionPrincipalKey,
) -> StoredDecision {
    owner
        .hold(hold_request("release", subject, approver, requester), false)
        .expect("hold the decision")
}

fn stored(owner: &DurableDecisionService, decision_id: &DecisionId) -> StoredDecision {
    owner.get(decision_id).expect("read the stored decision")
}

fn now_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_millis(),
    )
    .expect("milliseconds")
}

fn subject_digest() -> String {
    Sha256Digest::of(RENDERING.as_bytes()).to_string()
}

/// A complete one-part presentation to the approver id `approver`, as the
/// host presenter records it on its channel.
fn host_presentation(approver: &str, channel_ref: &str, message_ref: &str, at_ms: u64) -> Value {
    json!({
        "audience": {"kind": "approver", "approver_id": approver},
        "subject_digest": subject_digest(),
        "parts_shown": [{"index": 0, "count": 1, "part_digest": subject_digest()}],
        "channel_ref": channel_ref,
        "message_ref": message_ref,
        "at_ms": at_ms,
    })
}

fn decision_ids(rows: &Value) -> Vec<String> {
    rows.as_array()
        .expect("decision rows")
        .iter()
        .map(|row| {
            row["decision_id"]
                .as_str()
                .expect("decision id")
                .to_string()
        })
        .collect()
}

fn write_access(dir: &Path, toml: &str) -> PathBuf {
    let path = dir.join("access.toml");
    std::fs::write(&path, toml).expect("write the access configuration");
    path
}

/// The ingress declarations of the attested rows: `chat` and `mail` attest
/// people, with a five second clock skew.
fn declared_ingresses() -> Value {
    json!({"ingresses": [
        {"ingress": "chat", "attests": "person", "max_clock_skew_ms": 5000},
        {"ingress": "mail", "attests": "person", "max_clock_skew_ms": 5000},
    ]})
}

fn attestation(subject: &str, event_ref: &str, observed_at_ms: u64) -> Value {
    json!({
        "ingress": "chat",
        "subject": subject,
        "scope": "chat-session-1",
        "event_ref": event_ref,
        "observed_at_ms": observed_at_ms,
    })
}

fn assert_refused(answer: &Value, refusal: &str) {
    assert_eq!(answer["error"]["code"], json!(-32602), "{answer}");
    assert_eq!(
        answer["error"]["data"]["kind"],
        json!("decision_refused"),
        "{answer}"
    );
    assert_eq!(
        answer["error"]["data"]["decision_refusal"]["kind"],
        json!(refusal),
        "{answer}"
    );
}

/// Invalid params with no typed `data`: the request names something this
/// launch does not accept, as a body that names an approver.
fn assert_invalid_params(answer: &Value) {
    assert_eq!(answer["error"]["code"], json!(-32602), "{answer}");
    assert!(
        answer["error"].get("data").is_none_or(Value::is_null),
        "an attestation refusal carries no typed data: {answer}"
    );
}

// ---------------------------------------------------------------------------
// Part 1: the stdio host's verbs
// ---------------------------------------------------------------------------

#[test]
fn decisions_list_pages_held_decisions_for_the_stdio_host() {
    let state = tempfile::tempdir().expect("state dir");
    let scratch = tempfile::tempdir().expect("scratch dir");
    let owner = decisions_of(state.path());
    let mut gateway = Gateway::ready(scratch.path(), Some(state.path()), json!({}));
    let mut expected: Vec<String> = (0..3)
        .map(|n| {
            hold(&owner, &format!("build-{n}"), "team:owners")
                .decision_id
                .to_string()
        })
        .collect();
    expected.sort();

    let first = gateway.rpc("meta/decisions/list", json!({"limit": 2}));
    assert!(first["error"].is_null(), "{first}");
    let page = &first["result"];
    assert_eq!(decision_ids(&page["decisions"]), expected[..2].to_vec());
    assert_eq!(
        page["statuses"],
        json!([]),
        "the host reads every row whole"
    );
    assert_eq!(page["unreadable"], json!([]));
    let row = &page["decisions"][0];
    assert_eq!(row["kind"], json!("release"));
    assert_eq!(row["phase"], json!("open"));
    assert_eq!(row["approver_id"], json!("team:owners"));
    assert_eq!(row["requester"], json!(requester_key().to_string()));
    assert_eq!(row["subject_digest"], json!(subject_digest()));
    let next_after = page["next_after"].as_str().expect("a next page");

    let second = gateway.rpc(
        "meta/decisions/list",
        json!({"limit": 2, "after": next_after}),
    );
    assert_eq!(
        decision_ids(&second["result"]["decisions"]),
        expected[2..].to_vec()
    );

    let filtered = gateway.rpc(
        "meta/decisions/list",
        json!({"approver_id": "team:nobody", "kind": "release", "phase": "open"}),
    );
    assert_eq!(filtered["result"]["decisions"], json!([]), "{filtered}");
    let refused = gateway.rpc("meta/decisions/list", json!({"limit": 0}));
    assert_eq!(refused["error"]["code"], json!(-32602), "{refused}");
    let unknown_field = gateway.rpc("meta/decisions/list", json!({"approver": "x"}));
    assert_eq!(
        unknown_field["error"]["code"],
        json!(-32602),
        "{unknown_field}"
    );
}

#[test]
fn decision_verbs_answer_no_decision_store_without_persistent_state() {
    let scratch = tempfile::tempdir().expect("scratch dir");
    let mut gateway = Gateway::ready(scratch.path(), None, json!({}));
    let absent = format!("decision:{}", "0".repeat(64));
    for (method, params) in [
        ("meta/decisions/list", json!({})),
        ("meta/decisions/get", json!({"decision_id": absent})),
        ("meta/decisions/rendering", json!({"decision_id": absent})),
        (
            "meta/decisions/present",
            json!({
                "decision_id": absent,
                "presentation": host_presentation("team:owners", "chat:owners", "m-1", 1),
            }),
        ),
        ("meta/decisions/withdraw", json!({"decision_id": absent})),
        (
            "meta/decisions/decide",
            json!({"decision_id": absent, "approve": true, "presented_digest": subject_digest()}),
        ),
    ] {
        let answer = gateway.rpc(method, params);
        assert_eq!(answer["error"]["code"], json!(-32020), "{method}: {answer}");
        assert_eq!(
            answer["error"]["data"]["kind"],
            json!("no_decision_store"),
            "{method}: {answer}"
        );
    }
}

#[test]
fn the_stdio_host_reads_a_decision_and_its_rendering() {
    let state = tempfile::tempdir().expect("state dir");
    let scratch = tempfile::tempdir().expect("scratch dir");
    let owner = decisions_of(state.path());
    let mut gateway = Gateway::ready(scratch.path(), Some(state.path()), json!({}));
    let held = hold(&owner, "build-7", "team:owners");
    let decision_id = held.decision_id.to_string();

    let got = gateway.rpc("meta/decisions/get", json!({"decision_id": decision_id}));
    assert!(got["error"].is_null(), "{got}");
    let decision = &got["result"]["decision"];
    assert_eq!(decision["decision_id"], json!(decision_id));
    assert_eq!(decision["phase"], json!("open"));
    assert_eq!(decision["renderer"], json!("render.v1"));
    assert_eq!(decision["revision"], json!(1));
    assert!(decision.get("request").is_none(), "{decision}");
    let with_request = gateway.rpc(
        "meta/decisions/get",
        json!({"decision_id": decision_id, "include_request": true}),
    );
    assert_eq!(
        with_request["result"]["decision"]["request"],
        json!({"build": 7})
    );

    let rendering = gateway.rpc(
        "meta/decisions/rendering",
        json!({"decision_id": decision_id, "max_part_bytes": 16}),
    );
    assert!(rendering["error"].is_null(), "{rendering}");
    assert_eq!(rendering["result"]["rendering"], json!(RENDERING));
    assert_eq!(
        rendering["result"]["subject_digest"],
        json!(subject_digest())
    );
    let parts = rendering["result"]["parts"].as_array().expect("parts");
    assert_eq!(parts.len(), RENDERING.len().div_ceil(16));
    assert_eq!(parts[0]["index"], json!(0));
    assert_eq!(parts[0]["count"], json!(parts.len()));

    let absent = gateway.rpc(
        "meta/decisions/get",
        json!({"decision_id": format!("decision:{}", "0".repeat(64))}),
    );
    assert_eq!(absent["error"]["code"], json!(-32602), "{absent}");
    assert_eq!(
        absent["error"]["data"]["kind"],
        json!("decision_not_found"),
        "{absent}"
    );
}

#[test]
fn a_held_decision_sends_one_notice_to_the_stdio_host() {
    let state = tempfile::tempdir().expect("state dir");
    let scratch = tempfile::tempdir().expect("scratch dir");
    let owner = decisions_of(state.path());
    let mut gateway = Gateway::ready(scratch.path(), Some(state.path()), json!({}));
    let first = hold(&owner, "build-7", "team:owners");

    let (answer, notices) = gateway.call("meta/decisions/list", json!({}));
    assert!(answer["error"].is_null(), "{answer}");
    assert_eq!(
        notices.len(),
        1,
        "one notice per open decision: {notices:?}"
    );
    let notice = &notices[0];
    assert_eq!(notice["decision_id"], json!(first.decision_id.to_string()));
    assert_eq!(notice["kind"], json!("release"));
    assert_eq!(notice["event"], json!("held"));
    assert_eq!(notice["phase"], json!("open"));
    assert_eq!(notice["approver_id"], json!("team:owners"));
    assert_eq!(notice["requester"], json!(requester_key().to_string()));
    assert_eq!(notice["rendering_ref"], json!(subject_digest()));
    assert!(
        notice.get("presentation").is_none_or(Value::is_null),
        "a held notice shows nothing: {notice}"
    );

    let (_, again) = gateway.call("meta/decisions/list", json!({}));
    assert!(again.is_empty(), "a decision is noticed once: {again:?}");

    let second = hold(&owner, "build-8", "team:owners");
    let (_, next) = gateway.call("meta/decisions/list", json!({}));
    assert_eq!(next.len(), 1, "{next:?}");
    assert_eq!(
        next[0]["decision_id"],
        json!(second.decision_id.to_string())
    );

    gateway
        .rpc(
            "meta/decisions/withdraw",
            json!({"decision_id": second.decision_id.to_string()}),
        )
        .get("result")
        .expect("withdrawn");
    let (_, after_end) = gateway.call("meta/decisions/list", json!({}));
    assert!(after_end.is_empty(), "{after_end:?}");
}

#[test]
fn the_presenter_records_the_channel_message_references() {
    let state = tempfile::tempdir().expect("state dir");
    let scratch = tempfile::tempdir().expect("scratch dir");
    let owner = decisions_of(state.path());
    let mut gateway = Gateway::ready(scratch.path(), Some(state.path()), json!({}));
    let held = hold(&owner, "build-7", "team:owners");
    let decision_id = held.decision_id.to_string();
    let at_ms = now_ms();

    let presented = gateway.rpc(
        "meta/decisions/present",
        json!({
            "decision_id": decision_id,
            "presentation": host_presentation("team:owners", "chat:owners", "message-1", at_ms),
        }),
    );
    assert!(presented["error"].is_null(), "{presented}");
    let decision = &presented["result"]["decision"];
    assert_eq!(decision["first_presented_to_approver_at_ms"], json!(at_ms));
    assert_eq!(
        decision["presentations"][0]["channel_ref"],
        json!("chat:owners")
    );
    assert_eq!(
        decision["presentations"][0]["message_ref"],
        json!("message-1")
    );

    let row = stored(&owner, &held.decision_id);
    assert_eq!(row.presentations.len(), 1);
    assert_eq!(row.presentations[0].channel_ref.as_str(), "chat:owners");
    assert_eq!(row.presentations[0].message_ref.as_str(), "message-1");
    assert_eq!(row.state.first_presented_to_approver_at_ms, Some(at_ms));

    let elsewhere = gateway.rpc(
        "meta/decisions/present",
        json!({
            "decision_id": decision_id,
            "presentation": host_presentation("team:other", "chat:other", "message-2", at_ms),
        }),
    );
    assert_refused(&elsewhere, "wrong_audience");
}

#[test]
fn the_stdio_host_withdraws_a_decision() {
    let state = tempfile::tempdir().expect("state dir");
    let scratch = tempfile::tempdir().expect("scratch dir");
    let owner = decisions_of(state.path());
    let mut gateway = Gateway::ready(scratch.path(), Some(state.path()), json!({}));
    let held = hold(&owner, "build-7", "team:owners");

    let withdrawn = gateway.rpc(
        "meta/decisions/withdraw",
        json!({"decision_id": held.decision_id.to_string()}),
    );
    assert!(withdrawn["error"].is_null(), "{withdrawn}");
    assert_eq!(withdrawn["result"]["decision"]["phase"], json!("withdrawn"));
    assert_eq!(
        stored(&owner, &held.decision_id).state.phase,
        DecisionPhase::Withdrawn
    );
    let again = gateway.rpc(
        "meta/decisions/withdraw",
        json!({"decision_id": held.decision_id.to_string()}),
    );
    assert_refused(&again, "not_open");
}

// ---------------------------------------------------------------------------
// Part 2: the operator, the approver map and the attested ingresses
// ---------------------------------------------------------------------------

const OPERATOR_MAP: &str = r#"
enabled = false

[approvers."team:owners"]
subjects = ["owner@example.org"]
ingress_subjects = [{ ingress = "operator" }]

[approvers."team:other"]
subjects = ["other@example.org"]
"#;

#[test]
fn the_operator_decides_only_where_the_map_lists_it() {
    let state = tempfile::tempdir().expect("state dir");
    let scratch = tempfile::tempdir().expect("scratch dir");
    let access = write_access(scratch.path(), OPERATOR_MAP);
    let owner = decisions_of(state.path());
    let mut gateway = Gateway::ready(
        scratch.path(),
        Some(state.path()),
        json!({"access_config_path": access}),
    );
    let listed = hold(&owner, "build-7", "team:owners");
    let unlisted = hold(&owner, "build-8", "team:other");

    let (answer, notices) = gateway.call("meta/decisions/list", json!({}));
    assert!(answer["error"].is_null(), "{answer}");
    assert_eq!(notices.len(), 2, "{notices:?}");
    let notice_of = |decision: &StoredDecision| {
        notices
            .iter()
            .find(|notice| notice["decision_id"] == json!(decision.decision_id.to_string()))
            .unwrap_or_else(|| panic!("no notice for {}", decision.decision_id))
            .clone()
    };
    let presented = notice_of(&listed);
    assert_eq!(presented["event"], json!("presented"), "{presented}");
    assert_eq!(presented["presentation"]["rendering"], json!(RENDERING));
    let held_only = notice_of(&unlisted);
    assert_eq!(held_only["event"], json!("held"), "{held_only}");
    assert!(held_only.get("presentation").is_none_or(Value::is_null));
    let listed_row = stored(&owner, &listed.decision_id);
    assert_eq!(listed_row.presentations.len(), 1);
    assert_eq!(
        listed_row.presentations[0].message_ref.as_str(),
        presented["notice_id"].as_str().expect("notice id")
    );

    let decided = gateway.rpc(
        "meta/decisions/decide",
        json!({
            "decision_id": listed.decision_id.to_string(),
            "approve": true,
            "presented_digest": subject_digest(),
        }),
    );
    assert!(decided["error"].is_null(), "{decided}");
    let outcome = &decided["result"]["decision"]["outcome"];
    assert_eq!(decided["result"]["decision"]["phase"], json!("approved"));
    assert_eq!(outcome["tier"], json!("human"));
    assert_eq!(outcome["evidence"]["ingress"], json!("operator"));
    assert_eq!(outcome["evidence"]["assurance"], json!("device_trusted"));
    assert!(
        outcome["evidence"]["decider"]
            .as_str()
            .is_some_and(|decider| decider.starts_with(r#"principal:["human","#)),
        "{outcome}"
    );

    let refused = gateway.rpc(
        "meta/decisions/decide",
        json!({
            "decision_id": unlisted.decision_id.to_string(),
            "approve": true,
            "presented_digest": subject_digest(),
        }),
    );
    assert_refused(&refused, "not_eligible");
    assert_eq!(
        stored(&owner, &unlisted.decision_id).state.phase,
        DecisionPhase::Open
    );
}

#[test]
fn a_presentation_recorded_elsewhere_never_makes_the_operator_eligible() {
    let state = tempfile::tempdir().expect("state dir");
    let scratch = tempfile::tempdir().expect("scratch dir");
    let access = write_access(scratch.path(), OPERATOR_MAP);
    let owner = decisions_of(state.path());
    let mut gateway = Gateway::ready(
        scratch.path(),
        Some(state.path()),
        json!({"access_config_path": access}),
    );
    let held = hold(&owner, "build-7", "team:owners");
    let decision_id = held.decision_id.to_string();
    // Another surface on the shared store records a complete presentation
    // to the approver, and the host presenter records one on its channel.
    owner
        .present(
            &held.decision_id,
            &PresentationRecord::complete(
                PresentationAudience::Approver {
                    approver_id: ApproverId::new("team:owners").expect("approver id"),
                },
                &DecisionRendering::new(RENDERING).expect("rendering"),
                None,
                DecisionReference::new("rkat-rpc:stdio:another-connection").expect("channel"),
                DecisionReference::new("notice-elsewhere").expect("message"),
                now_ms(),
            ),
        )
        .expect("a presentation recorded elsewhere");
    let host_presented = gateway.rpc(
        "meta/decisions/present",
        json!({
            "decision_id": decision_id,
            "presentation": host_presentation("team:owners", "chat:owners", "message-1", now_ms()),
        }),
    );
    assert!(host_presented["error"].is_null(), "{host_presented}");

    let decide = json!({
        "decision_id": decision_id,
        "approve": true,
        "presented_digest": subject_digest(),
    });
    let refused = gateway.rpc("meta/decisions/decide", decide.clone());
    assert_refused(&refused, "not_eligible");
    assert_eq!(
        stored(&owner, &held.decision_id).state.phase,
        DecisionPhase::Open
    );

    // Meta's own presentation on this connection makes it eligible.
    let (_, notices) = gateway.call("meta/decisions/list", json!({}));
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert_eq!(notices[0]["event"], json!("presented"));
    let decided = gateway.rpc("meta/decisions/decide", decide);
    assert!(decided["error"].is_null(), "{decided}");
    assert_eq!(decided["result"]["decision"]["phase"], json!("approved"));
}

const CHAT_MAP: &str = r#"
enabled = false

[approvers."team:owners"]
subjects = ["owner@example.org"]
ingress_subjects = [{ ingress = "chat", subject = "U123" }]
"#;

/// A gateway on `state` with the chat and mail ingresses declared and the
/// approver map `map`.
fn attested_gateway(scratch: &Path, state: &Path, map: &str) -> Gateway {
    let access = write_access(scratch, map);
    Gateway::ready(
        scratch,
        Some(state),
        json!({"access_config_path": access, "decisions": declared_ingresses()}),
    )
}

/// Hold a decision for `team:owners` and present it on the chat channel.
fn held_and_presented(
    gateway: &mut Gateway,
    owner: &DurableDecisionService,
    subject: &str,
) -> (StoredDecision, u64) {
    let held = hold(owner, subject, "team:owners");
    let at_ms = now_ms();
    let presented = gateway.rpc(
        "meta/decisions/present",
        json!({
            "decision_id": held.decision_id.to_string(),
            "presentation": host_presentation("team:owners", "chat:owners", subject, at_ms),
        }),
    );
    assert!(presented["error"].is_null(), "{presented}");
    (held, at_ms)
}

fn attested_decide(decision: &StoredDecision, attestation: Value) -> Value {
    json!({
        "decision_id": decision.decision_id.to_string(),
        "approve": true,
        "presented_digest": subject_digest(),
        "attestation": attestation,
    })
}

#[test]
fn decide_with_an_attestation_from_an_undeclared_ingress_is_refused() {
    let state = tempfile::tempdir().expect("state dir");
    let scratch = tempfile::tempdir().expect("scratch dir");
    let owner = decisions_of(state.path());
    let mut gateway = Gateway::ready(scratch.path(), Some(state.path()), json!({}));
    let (held, at_ms) = held_and_presented(&mut gateway, &owner, "build-7");

    let undeclared = gateway.rpc(
        "meta/decisions/decide",
        attested_decide(&held, attestation("U123", "chat:event-1", at_ms)),
    );
    assert_invalid_params(&undeclared);
    let mut operator = attestation("U123", "chat:event-2", at_ms);
    operator["ingress"] = json!("operator");
    let built_in = gateway.rpc("meta/decisions/decide", attested_decide(&held, operator));
    assert_invalid_params(&built_in);
    let row = stored(&owner, &held.decision_id);
    assert_eq!(row.state.phase, DecisionPhase::Open);
    assert!(row.outcome.is_none());
}

#[test]
fn decide_with_an_attestation_records_the_person_and_the_observed_time() {
    let state = tempfile::tempdir().expect("state dir");
    let scratch = tempfile::tempdir().expect("scratch dir");
    let owner = decisions_of(state.path());
    let mut gateway = attested_gateway(scratch.path(), state.path(), CHAT_MAP);
    let (held, at_ms) = held_and_presented(&mut gateway, &owner, "build-7");

    let stranger = gateway.rpc(
        "meta/decisions/decide",
        attested_decide(&held, attestation("U999", "chat:event-0", at_ms)),
    );
    assert_refused(&stranger, "not_eligible");

    let observed_at_ms = at_ms + 1;
    let decided = gateway.rpc(
        "meta/decisions/decide",
        attested_decide(&held, attestation("U123", "chat:event-1", observed_at_ms)),
    );
    assert!(decided["error"].is_null(), "{decided}");
    let decision = &decided["result"]["decision"];
    assert_eq!(decision["phase"], json!("approved"));
    let evidence = &decision["outcome"]["evidence"];
    assert_eq!(evidence["kind"], json!("human"));
    assert_eq!(
        evidence["decider"],
        json!(ingress_person_key("chat", "U123").to_string())
    );
    assert_eq!(evidence["assurance"], json!("person_authenticated"));
    assert_eq!(evidence["ingress"], json!("chat"));
    assert_eq!(evidence["event_ref"], json!("chat:event-1"));
    assert_eq!(evidence["observed_at_ms"], json!(observed_at_ms));
    assert_eq!(evidence["approver_id"], json!("team:owners"));
    assert_eq!(
        stored(&owner, &held.decision_id).state.phase,
        DecisionPhase::Approved
    );
}

#[test]
fn a_reused_attestation_event_ref_is_refused() {
    let state = tempfile::tempdir().expect("state dir");
    let scratch = tempfile::tempdir().expect("scratch dir");
    let owner = decisions_of(state.path());
    let mut gateway = attested_gateway(scratch.path(), state.path(), CHAT_MAP);
    let (first, at_ms) = held_and_presented(&mut gateway, &owner, "build-7");
    let (second, _) = held_and_presented(&mut gateway, &owner, "build-8");

    let decided = gateway.rpc(
        "meta/decisions/decide",
        attested_decide(&first, attestation("U123", "chat:event-1", at_ms + 1)),
    );
    assert!(decided["error"].is_null(), "{decided}");
    let reused = gateway.rpc(
        "meta/decisions/decide",
        attested_decide(&second, attestation("U123", "chat:event-1", at_ms + 2)),
    );
    assert_invalid_params(&reused);
    assert_eq!(
        stored(&owner, &second.decision_id).state.phase,
        DecisionPhase::Open
    );
    // The same decision may repeat its own event: the decision answers it.
    let repeated = gateway.rpc(
        "meta/decisions/decide",
        attested_decide(&first, attestation("U123", "chat:event-1", at_ms + 1)),
    );
    assert_refused(&repeated, "not_open");

    // The record of used events survives a restart.
    gateway.kill();
    let mut gateway = attested_gateway(scratch.path(), state.path(), CHAT_MAP);
    let after_restart = gateway.rpc(
        "meta/decisions/decide",
        attested_decide(&second, attestation("U123", "chat:event-1", now_ms())),
    );
    assert_invalid_params(&after_restart);
    let fresh = gateway.rpc(
        "meta/decisions/decide",
        attested_decide(&second, attestation("U123", "chat:event-2", now_ms())),
    );
    assert!(fresh["error"].is_null(), "{fresh}");
    assert_eq!(fresh["result"]["decision"]["phase"], json!("approved"));
}

#[test]
fn an_attestation_beyond_the_clock_skew_is_refused() {
    let state = tempfile::tempdir().expect("state dir");
    let scratch = tempfile::tempdir().expect("scratch dir");
    let owner = decisions_of(state.path());
    let mut gateway = attested_gateway(scratch.path(), state.path(), CHAT_MAP);
    let (held, _) = held_and_presented(&mut gateway, &owner, "build-7");

    let ahead = gateway.rpc(
        "meta/decisions/decide",
        attested_decide(
            &held,
            attestation("U123", "chat:event-1", now_ms() + 60_000),
        ),
    );
    assert_invalid_params(&ahead);
    assert_eq!(
        stored(&owner, &held.decision_id).state.phase,
        DecisionPhase::Open
    );
    let within = gateway.rpc(
        "meta/decisions/decide",
        attested_decide(&held, attestation("U123", "chat:event-2", now_ms() + 1_000)),
    );
    assert!(within["error"].is_null(), "{within}");
    assert_eq!(within["result"]["decision"]["phase"], json!("approved"));
}

#[test]
fn cross_ingress_self_approval_is_refused() {
    let state = tempfile::tempdir().expect("state dir");
    let scratch = tempfile::tempdir().expect("scratch dir");
    let owner = decisions_of(state.path());
    let map = r#"
enabled = false

[approvers."team:owners"]
subjects = ["owner@example.org"]
ingress_subjects = [{ ingress = "chat", subject = "U555" }]
people = [
  { ingress_subjects = [{ ingress = "chat", subject = "U123" }, { ingress = "mail", subject = "owner" }] },
]
"#;
    let mut gateway = attested_gateway(scratch.path(), state.path(), map);
    // The person asked by mail; their chat identity is linked in one entry.
    let held = hold_for(
        &owner,
        "build-7",
        "team:owners",
        ingress_person_key("mail", "owner"),
    );
    let at_ms = now_ms();
    let presented = gateway.rpc(
        "meta/decisions/present",
        json!({
            "decision_id": held.decision_id.to_string(),
            "presentation": host_presentation("team:owners", "chat:owners", "message-1", at_ms),
        }),
    );
    assert!(presented["error"].is_null(), "{presented}");

    let own = gateway.rpc(
        "meta/decisions/decide",
        attested_decide(&held, attestation("U123", "chat:event-1", at_ms + 1)),
    );
    assert_refused(&own, "self_approval");
    assert_eq!(
        stored(&owner, &held.decision_id).state.phase,
        DecisionPhase::Open
    );

    let other = gateway.rpc(
        "meta/decisions/decide",
        attested_decide(&held, attestation("U555", "chat:event-2", at_ms + 2)),
    );
    assert!(other["error"].is_null(), "{other}");
    assert_eq!(other["result"]["decision"]["phase"], json!("approved"));
}

// ---------------------------------------------------------------------------
// Part 2: launch-time validation of the declarations and the map
// ---------------------------------------------------------------------------

/// The `meta/init` answer of a fresh gateway with `runtime_options`.
fn init_answer(scratch: &Path, runtime_options: Value) -> Value {
    let state = tempfile::tempdir_in(scratch).expect("state dir");
    let mut gateway = Gateway::start(scratch);
    gateway.init(Some(state.path()), runtime_options)
}

fn assert_init_refused(answer: &Value, needle: &str) {
    assert!(
        answer["error"].is_object(),
        "the launch is refused: {answer}"
    );
    assert!(
        answer["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains(needle)),
        "the refusal names {needle:?}: {answer}"
    );
}

#[test]
fn runtime_options_decisions_declares_ingresses() {
    let scratch = tempfile::tempdir().expect("scratch dir");
    let accepted = init_answer(
        scratch.path(),
        json!({"decisions": {"ingresses": [
            {"ingress": "chat", "attests": "person", "max_clock_skew_ms": 5000},
        ]}}),
    );
    assert!(
        accepted["result"]["contract_version"].is_string(),
        "a declared ingress is accepted: {accepted}"
    );
    for (declaration, needle) in [
        (
            json!({"ingress": "operator", "attests": "person", "max_clock_skew_ms": 5000}),
            "operator",
        ),
        (
            json!({"ingress": "Chat!", "attests": "person", "max_clock_skew_ms": 5000}),
            "Chat!",
        ),
        (
            json!({"ingress": "chat", "attests": "service", "max_clock_skew_ms": 5000}),
            "attests",
        ),
        (
            json!({"ingress": "chat", "attests": "person"}),
            "max_clock_skew_ms",
        ),
        (
            json!({"ingress": "chat", "attests": "person", "max_clock_skew_ms": 5000, "trust": "all"}),
            "trust",
        ),
    ] {
        let refused = init_answer(
            scratch.path(),
            json!({"decisions": {"ingresses": [declaration]}}),
        );
        assert_init_refused(&refused, needle);
    }
    let duplicate = init_answer(
        scratch.path(),
        json!({"decisions": {"ingresses": [
            {"ingress": "chat", "attests": "person", "max_clock_skew_ms": 5000},
            {"ingress": "chat", "attests": "person", "max_clock_skew_ms": 1000},
        ]}}),
    );
    assert_init_refused(&duplicate, "chat");
}

#[test]
fn an_unknown_ingress_in_the_approvers_map_is_refused() {
    let scratch = tempfile::tempdir().expect("scratch dir");
    let unknown = scratch.path().join("unknown");
    std::fs::create_dir(&unknown).expect("dir");
    let access = write_access(&unknown, CHAT_MAP);
    let refused = init_answer(scratch.path(), json!({"access_config_path": access}));
    assert_init_refused(&refused, "unknown ingress");

    let operator_subject = scratch.path().join("operator-subject");
    std::fs::create_dir(&operator_subject).expect("dir");
    let access = write_access(
        &operator_subject,
        r#"
enabled = false

[approvers."team:owners"]
subjects = ["owner@example.org"]
ingress_subjects = [{ ingress = "operator", subject = "someone" }]
"#,
    );
    let refused = init_answer(scratch.path(), json!({"access_config_path": access}));
    assert_init_refused(&refused, "operator");

    let declared = scratch.path().join("declared");
    std::fs::create_dir(&declared).expect("dir");
    let access = write_access(&declared, CHAT_MAP);
    let accepted = init_answer(
        scratch.path(),
        json!({"access_config_path": access, "decisions": declared_ingresses()}),
    );
    assert!(
        accepted["result"]["contract_version"].is_string(),
        "{accepted}"
    );
}

#[test]
fn a_padded_ingress_subject_is_refused() {
    let scratch = tempfile::tempdir().expect("scratch dir");
    for (name, subject) in [("padded", " U123"), ("empty", "")] {
        let dir = scratch.path().join(name);
        std::fs::create_dir(&dir).expect("dir");
        let access = write_access(
            &dir,
            &format!(
                r#"
enabled = false

[approvers."team:owners"]
subjects = ["owner@example.org"]
ingress_subjects = [{{ ingress = "chat", subject = "{subject}" }}]
"#
            ),
        );
        let refused = init_answer(
            scratch.path(),
            json!({"access_config_path": access, "decisions": declared_ingresses()}),
        );
        assert_init_refused(&refused, "team:owners");
    }
}

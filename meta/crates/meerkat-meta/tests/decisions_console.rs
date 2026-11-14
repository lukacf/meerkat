#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! Durable decisions on the Console (`POST /console/rpc`): the reads
//! (`meta/decisions/list`, `get`, `rendering`) and the access admin's
//! withdraw. Who sees what is the kind's rule, applied whatever the access
//! switch: holders of a decision's approver id and access admins read its
//! content, its requester reads the status of their own requests (the
//! content too only for `tool_grant.issue`), and an Open Console's
//! anonymous caller reads nothing held. `decisions.view` gates the reads
//! while access control is enforced; the withdraw needs `decisions.admin`.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use meerkat_client::TestClient;
use meerkat_core::approval::decision::{
    ApproverId, DecisionHoldRequest, DecisionKind, DecisionOperationId, DecisionPhase,
    DecisionPrincipalKey, DecisionRenderer, DecisionRendering, DecisionRequestBody,
    DecisionSubjectKey, Sha256Digest, StoredDecision,
};
use meerkat_core::{PrincipalKind, PrincipalRef, TrustDomainId};
use meerkat_meta::{
    ACCESS_ACTIONS, AccessApprover, AccessControlConfig, AccessController, AuthPolicy,
    BigQueryNaming, ConsolePolicy, GatingOrigin, RuntimeDecisionInputs, RuntimeOpsPolicy,
    TrustedOidcRuntimeConfig, UnifiedRuntime, build_runtime_decision_state,
};
use meerkat_mob::MobDefinition;
use meerkat_runtime::decision::DurableDecisionService;
use serde_json::{Value, json};
use tower::ServiceExt;

const ISSUER: &str = "https://trusted.meta.localhost";
const RENDERING: &str = "Release build 7 to production, with the change log attached.";
const ADMIN: &str = "admin@example.test";
const ALICE: &str = "alice@example.test";
const BOB: &str = "bob@example.test";
const CAROL: &str = "carol@example.test";
const DAVE: &str = "dave@example.test";
const ERIN: &str = "erin@example.test";

fn decision_state(require_app_auth: bool) -> meerkat_meta::RuntimeDecisionState {
    build_runtime_decision_state(RuntimeDecisionInputs {
        bigquery: BigQueryNaming {
            dataset: "decisions_dataset".to_string(),
            table: "decisions_table".to_string(),
        },
        trusted_meta_toml: r#"
[[modules]]
id = "router"
command = "router-bin"
args = []
restart_policy = "always"
"#
        .to_string(),
        auth: AuthPolicy {
            default_provider: meerkat_meta::AuthProvider::GoogleOAuth,
            email_allowlist: [ADMIN, ALICE, BOB, CAROL, DAVE, ERIN]
                .iter()
                .map(|email| (*email).to_string())
                .collect(),
        },
        trusted_oidc: TrustedOidcRuntimeConfig {
            discovery_json: json!({
                "issuer": ISSUER,
                "jwks_uri": format!("{ISSUER}/.well-known/jwks.json"),
            })
            .to_string(),
            jwks_json: r#"{"keys":[{"kid":"kid-current","kty":"oct","alg":"HS256","k":"cGhhc2U3LXRydXN0ZWQtY3VycmVudC1zZWNyZXQ"}]}"#
                .to_string(),
            audience: "meerkat-console".to_string(),
            require_verified_email: false,
            shared_account_tokens: false,
        },
        console: ConsolePolicy {
            require_app_auth,
            ..ConsolePolicy::default()
        },
        ops: RuntimeOpsPolicy::default(),
        release_metadata_json: include_str!("../assets/release-targets.json").to_string(),
    })
    .expect("decision state builds")
}

/// A persistent runtime on `state` with the access configuration `access`
/// (TOML), and its Console router.
async fn console(
    state: &Path,
    access: Option<&str>,
    require_app_auth: bool,
) -> (UnifiedRuntime, axum::Router) {
    console_with(state, access, require_app_auth, None).await
}

/// [`console`], declaring the decision ingresses `ingresses`
/// (`runtime_options.decisions`).
async fn console_with(
    state: &Path,
    access: Option<&str>,
    require_app_auth: bool,
    ingresses: Option<Value>,
) -> (UnifiedRuntime, axum::Router) {
    static NEXT_MOB: AtomicU64 = AtomicU64::new(0);
    let definition = MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "decisions-console-{}"

[profiles.lead]
model = "gpt-5.5"
"#,
        NEXT_MOB.fetch_add(1, Ordering::Relaxed)
    ))
    .expect("definition");
    let mut builder = UnifiedRuntime::builder()
        .definition(definition)
        .persistent_state(state)
        .default_llm_client(Arc::new(TestClient::default()));
    if let Some(access) = access {
        let config: AccessControlConfig = toml::from_str(access).expect("access TOML");
        builder = builder.access_controller(AccessController::new(config).expect("controller"));
    }
    if let Some(ingresses) = ingresses {
        builder =
            builder.decision_ingresses(serde_json::from_value(ingresses).expect("declarations"));
    }
    let runtime = Box::pin(builder.build()).await.expect("persistent runtime");
    let app = runtime.build_console_json_router(decision_state(require_app_auth));
    (runtime, app)
}

/// One Console JSON-RPC call, signed in as `subject` when given.
async fn rpc(app: &axum::Router, subject: Option<&str>, method: &str, params: Value) -> Value {
    let mut request = Request::builder()
        .method("POST")
        .uri("/console/rpc")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(subject) = subject {
        let mut jwt_header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256);
        jwt_header.kid = Some("kid-current".to_string());
        let token = jsonwebtoken::encode(
            &jwt_header,
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
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let request = request
        .body(Body::from(
            json!({"jsonrpc": "2.0", "id": "decisions", "method": method, "params": params})
                .to_string(),
        ))
        .expect("rpc request");
    let response = tokio::time::timeout(Duration::from_secs(30), app.clone().oneshot(request))
        .await
        .expect("bounded rpc")
        .expect("rpc response");
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .expect("rpc body");
    serde_json::from_slice(&body).expect("rpc json")
}

fn decisions_of(state: &Path) -> DurableDecisionService {
    meerkat::ApprovalIssuerSelection::Persistent {
        store_path: state.to_path_buf(),
    }
    .open_decisions()
}

fn agent_requester() -> DecisionPrincipalKey {
    DecisionPrincipalKey::of(
        &PrincipalRef::in_domain(
            PrincipalKind::PersonalAgent,
            "builder",
            TrustDomainId::new("example.org").expect("domain"),
        )
        .expect("principal"),
    )
    .expect("key")
}

/// The principal key of the Console human `subject`, as the Console
/// qualifies a verified OIDC subject.
fn console_requester(subject: &str) -> DecisionPrincipalKey {
    DecisionPrincipalKey::of(
        &PrincipalRef::in_domain(
            PrincipalKind::Human,
            subject,
            TrustDomainId::new(format!("oidc-issuer:{ISSUER}")).expect("domain"),
        )
        .expect("principal"),
    )
    .expect("key")
}

fn hold(
    owner: &DurableDecisionService,
    kind: &str,
    subject: &str,
    approver: &str,
    requester: DecisionPrincipalKey,
) -> StoredDecision {
    owner
        .hold(
            DecisionHoldRequest::new(
                DecisionKind::new(kind).expect("kind"),
                DecisionSubjectKey::new(subject).expect("subject"),
                DecisionOperationId::new(format!("operation:{subject}")).expect("operation"),
                Sha256Digest::of(br#"{"build":7}"#),
                DecisionRequestBody::from_value(&json!({"build": 7})).expect("body"),
                requester,
                ApproverId::new(approver).expect("approver id"),
                u64::MAX / 2,
                DecisionRendering::new(RENDERING).expect("rendering"),
                DecisionRenderer::new("render.v1").expect("renderer"),
            ),
            false,
        )
        .expect("hold")
}

fn ids(rows: &Value) -> Vec<String> {
    let mut ids: Vec<String> = rows
        .as_array()
        .unwrap_or_else(|| panic!("rows: {rows}"))
        .iter()
        .map(|row| {
            row["decision_id"]
                .as_str()
                .expect("decision id")
                .to_string()
        })
        .collect();
    ids.sort();
    ids
}

fn sorted(decisions: &[&StoredDecision]) -> Vec<String> {
    let mut ids: Vec<String> = decisions
        .iter()
        .map(|decision| decision.decision_id.to_string())
        .collect();
    ids.sort();
    ids
}

fn assert_no_held_text(answer: &Value) {
    assert!(
        !answer.to_string().contains(RENDERING),
        "no held content reaches this caller: {answer}"
    );
}

/// A status row: what the requester may see of their own request.
fn assert_status_only(status: &Value, decision: &StoredDecision) {
    assert_eq!(
        status["decision_id"],
        json!(decision.decision_id.to_string())
    );
    assert_eq!(status["kind"], json!(decision.descriptor.kind.to_string()));
    assert_eq!(status["phase"], json!("open"));
    assert_eq!(
        status["deadline_at_ms"],
        json!(decision.descriptor.deadline_at_ms)
    );
    for content in [
        "rendering",
        "subject_digest",
        "presentations",
        "request",
        "renderer",
    ] {
        assert!(status.get(content).is_none(), "{content} in {status}");
    }
}

const ENFORCED: &str = r#"
enabled = true
admins = ["admin@example.test"]

[[rules]]
id = "decision-readers"
subjects = ["alice@example.test", "bob@example.test", "carol@example.test"]
actions = ["decisions.view"]

[[rules]]
id = "decision-admin"
subjects = ["erin@example.test"]
actions = ["decisions.admin"]

[approvers."team:owners"]
subjects = ["alice@example.test"]

[approvers."team:ops"]
subjects = ["bob@example.test"]
"#;

#[tokio::test]
async fn decisions_list_shows_a_caller_only_its_approver_ids_and_requests() {
    let dir = tempfile::tempdir().expect("dir");
    let state = dir.path().join("state");
    let (runtime, app) = console(&state, Some(ENFORCED), true).await;
    let owner = decisions_of(&state);
    let owners = hold(&owner, "release", "a", "team:owners", agent_requester());
    let ops = hold(&owner, "release", "b", "team:ops", agent_requester());
    let carol_release = hold(&owner, "release", "c", "team:ops", console_requester(CAROL));
    let carol_grant = hold(
        &owner,
        "tool_grant.issue",
        "d",
        "team:ops",
        console_requester(CAROL),
    );

    let alice = rpc(&app, Some(ALICE), "meta/decisions/list", json!({})).await;
    assert!(alice["error"].is_null(), "{alice}");
    assert_eq!(ids(&alice["result"]["decisions"]), sorted(&[&owners]));
    assert_eq!(alice["result"]["statuses"], json!([]));

    let bob = rpc(&app, Some(BOB), "meta/decisions/list", json!({})).await;
    assert_eq!(
        ids(&bob["result"]["decisions"]),
        sorted(&[&ops, &carol_release, &carol_grant])
    );

    let carol = rpc(&app, Some(CAROL), "meta/decisions/list", json!({})).await;
    assert_eq!(ids(&carol["result"]["decisions"]), sorted(&[&carol_grant]));
    assert_eq!(ids(&carol["result"]["statuses"]), sorted(&[&carol_release]));
    assert_status_only(&carol["result"]["statuses"][0], &carol_release);

    let admin = rpc(&app, Some(ADMIN), "meta/decisions/list", json!({})).await;
    assert_eq!(
        ids(&admin["result"]["decisions"]),
        sorted(&[&owners, &ops, &carol_release, &carol_grant])
    );

    let dave = rpc(&app, Some(DAVE), "meta/decisions/list", json!({})).await;
    assert_eq!(dave["error"]["code"], json!(-32030), "{dave}");
    assert_eq!(
        dave["error"]["data"]["kind"],
        json!("access_denied"),
        "{dave}"
    );

    let carol_reads_status = rpc(
        &app,
        Some(CAROL),
        "meta/decisions/get",
        json!({"decision_id": carol_release.decision_id.to_string()}),
    )
    .await;
    assert!(carol_reads_status["result"].get("decision").is_none());
    assert_status_only(&carol_reads_status["result"]["status"], &carol_release);
    let carol_reads_grant = rpc(
        &app,
        Some(CAROL),
        "meta/decisions/rendering",
        json!({"decision_id": carol_grant.decision_id.to_string()}),
    )
    .await;
    assert_eq!(carol_reads_grant["result"]["rendering"], json!(RENDERING));
    let carol_reads_other = rpc(
        &app,
        Some(CAROL),
        "meta/decisions/get",
        json!({"decision_id": owners.decision_id.to_string()}),
    )
    .await;
    assert_eq!(
        carol_reads_other["error"]["data"]["kind"],
        json!("decision_not_found"),
        "{carol_reads_other}"
    );
    drop(app);
    runtime.shutdown().await;
}

/// The N11 rows: the requester of a held release reads its status, never
/// its text.
async fn requester_reads_status_only(access: &str) {
    let dir = tempfile::tempdir().expect("dir");
    let state = dir.path().join("state");
    let (runtime, app) = console(&state, Some(access), true).await;
    let owner = decisions_of(&state);
    let release = hold(
        &owner,
        "route.release",
        "answer-1",
        "team:owners",
        console_requester(CAROL),
    );
    let id = json!({"decision_id": release.decision_id.to_string()});

    let got = rpc(&app, Some(CAROL), "meta/decisions/get", id.clone()).await;
    assert!(got["error"].is_null(), "{got}");
    assert_status_only(&got["result"]["status"], &release);
    assert_no_held_text(&got);
    let with_request = rpc(
        &app,
        Some(CAROL),
        "meta/decisions/get",
        json!({"decision_id": release.decision_id.to_string(), "include_request": true}),
    )
    .await;
    assert_no_held_text(&with_request);
    assert!(with_request["result"]["status"].get("request").is_none());
    let rendering = rpc(&app, Some(CAROL), "meta/decisions/rendering", id.clone()).await;
    assert_eq!(rendering["error"]["code"], json!(-32030), "{rendering}");
    assert_eq!(
        rendering["error"]["data"]["kind"],
        json!("content_not_visible"),
        "{rendering}"
    );
    assert_no_held_text(&rendering);
    let listed = rpc(&app, Some(CAROL), "meta/decisions/list", json!({})).await;
    assert_eq!(listed["result"]["decisions"], json!([]), "{listed}");
    assert_eq!(ids(&listed["result"]["statuses"]), sorted(&[&release]));
    assert_no_held_text(&listed);

    // The holder of the approver id reads it whole.
    let holder = rpc(&app, Some(ALICE), "meta/decisions/rendering", id).await;
    assert_eq!(holder["result"]["rendering"], json!(RENDERING), "{holder}");
    drop(app);
    runtime.shutdown().await;
}

/// The requester of a change of an agent reads its own request whole, for
/// each `identity.*` kind: the rendering is what it asked for, shown to it
/// already when it asks what the change would get. Reading it gives no
/// standing to decide it: the Console serves no decide.
#[tokio::test]
async fn the_requester_of_a_change_of_an_agent_reads_its_own_request_whole() {
    let dir = tempfile::tempdir().expect("dir");
    let state = dir.path().join("state");
    let (runtime, app) = console(&state, Some(ENFORCED), true).await;
    let owner = decisions_of(&state);
    for kind in [
        "identity.create",
        "identity.update",
        "identity.archive",
        "identity.unarchive",
        "identity.remove",
        "identity.adopt",
    ] {
        let held = hold(&owner, kind, kind, "team:ops", console_requester(CAROL));
        let reads = rpc(
            &app,
            Some(CAROL),
            "meta/decisions/rendering",
            json!({"decision_id": held.decision_id.to_string()}),
        )
        .await;
        assert_eq!(
            reads["result"]["rendering"],
            json!(RENDERING),
            "{kind}: {reads}"
        );
        let whole = rpc(
            &app,
            Some(CAROL),
            "meta/decisions/get",
            json!({"decision_id": held.decision_id.to_string()}),
        )
        .await;
        assert!(whole["result"].get("decision").is_some(), "{kind}: {whole}");
        let decide = rpc(
            &app,
            Some(CAROL),
            "meta/decisions/decide",
            json!({
                "decision_id": held.decision_id.to_string(),
                "approve": true,
                "presented_digest": held.descriptor.subject_digest.to_string(),
            }),
        )
        .await;
        assert_eq!(decide["error"]["code"], json!(-32601), "{kind}: {decide}");
    }
    drop(app);
    runtime.shutdown().await;
}

#[tokio::test]
async fn requester_cannot_read_a_held_release() {
    requester_reads_status_only(ENFORCED).await;
}

#[tokio::test]
async fn requester_cannot_read_a_held_release_with_access_control_off() {
    requester_reads_status_only(
        r#"
enabled = false

[approvers."team:owners"]
subjects = ["alice@example.test"]
"#,
    )
    .await;
}

#[tokio::test]
async fn open_console_shows_no_held_text() {
    let dir = tempfile::tempdir().expect("dir");
    let state = dir.path().join("state");
    let (runtime, app) = console(
        &state,
        Some(
            r#"
enabled = false

[approvers."team:owners"]
subjects = ["alice@example.test"]
"#,
        ),
        false,
    )
    .await;
    let owner = decisions_of(&state);
    let release = hold(
        &owner,
        "route.release",
        "answer-1",
        "team:owners",
        agent_requester(),
    );
    let id = json!({"decision_id": release.decision_id.to_string()});

    let listed = rpc(&app, None, "meta/decisions/list", json!({})).await;
    assert!(listed["error"].is_null(), "{listed}");
    assert_eq!(listed["result"]["decisions"], json!([]));
    assert_eq!(listed["result"]["statuses"], json!([]));
    for method in ["meta/decisions/get", "meta/decisions/rendering"] {
        let answer = rpc(&app, None, method, id.clone()).await;
        assert_eq!(
            answer["error"]["data"]["kind"],
            json!("decision_not_found"),
            "{method}: {answer}"
        );
        assert_no_held_text(&answer);
    }
    drop(app);
    runtime.shutdown().await;
}

#[tokio::test]
async fn console_withdraw_needs_decisions_admin() {
    let dir = tempfile::tempdir().expect("dir");
    let state = dir.path().join("state");
    let (runtime, app) = console(&state, Some(ENFORCED), true).await;
    let owner = decisions_of(&state);
    let first = hold(&owner, "release", "a", "team:owners", agent_requester());
    let second = hold(&owner, "release", "b", "team:owners", agent_requester());

    let holder = rpc(
        &app,
        Some(ALICE),
        "meta/decisions/withdraw",
        json!({"decision_id": first.decision_id.to_string()}),
    )
    .await;
    assert_eq!(holder["error"]["code"], json!(-32030), "{holder}");
    assert_eq!(holder["error"]["data"]["kind"], json!("access_denied"));
    assert_eq!(
        owner.get(&first.decision_id).expect("row").state.phase,
        DecisionPhase::Open
    );

    let admin = rpc(
        &app,
        Some(ADMIN),
        "meta/decisions/withdraw",
        json!({"decision_id": first.decision_id.to_string()}),
    )
    .await;
    assert!(admin["error"].is_null(), "{admin}");
    assert_eq!(admin["result"]["decision"]["phase"], json!("withdrawn"));
    let rule_admin = rpc(
        &app,
        Some(ERIN),
        "meta/decisions/withdraw",
        json!({"decision_id": second.decision_id.to_string()}),
    )
    .await;
    assert!(rule_admin["error"].is_null(), "{rule_admin}");
    assert_eq!(
        owner.get(&second.decision_id).expect("row").state.phase,
        DecisionPhase::Withdrawn
    );
    drop(app);
    runtime.shutdown().await;
}

#[tokio::test]
async fn capabilities_list_the_decision_methods_the_caller_may_call() {
    let dir = tempfile::tempdir().expect("dir");
    let state = dir.path().join("state");
    let (runtime, app) = console(&state, Some(ENFORCED), true).await;
    let decision_methods = |answer: Value| -> Vec<String> {
        let mut methods: Vec<String> = answer["result"]["methods"]
            .as_array()
            .unwrap_or_else(|| panic!("methods: {answer}"))
            .iter()
            .filter_map(Value::as_str)
            .filter(|method| method.starts_with("meta/decisions/"))
            .map(str::to_string)
            .collect();
        methods.sort();
        methods
    };

    let reader = decision_methods(rpc(&app, Some(ALICE), "meta/capabilities", json!({})).await);
    assert_eq!(
        reader,
        [
            "meta/decisions/get",
            "meta/decisions/list",
            "meta/decisions/rendering"
        ]
    );
    let rule_admin = decision_methods(rpc(&app, Some(ERIN), "meta/capabilities", json!({})).await);
    assert_eq!(rule_admin, ["meta/decisions/withdraw"]);
    let admin = decision_methods(rpc(&app, Some(ADMIN), "meta/capabilities", json!({})).await);
    assert_eq!(
        admin,
        [
            "meta/decisions/get",
            "meta/decisions/list",
            "meta/decisions/rendering",
            "meta/decisions/withdraw"
        ]
    );
    let nobody = decision_methods(rpc(&app, Some(DAVE), "meta/capabilities", json!({})).await);
    assert!(nobody.is_empty(), "{nobody:?}");
    drop(app);
    runtime.shutdown().await;
}

#[tokio::test]
async fn the_console_http_surface_refuses_an_attestation_field() {
    let dir = tempfile::tempdir().expect("dir");
    let state = dir.path().join("state");
    let (runtime, app) = console(&state, Some(ENFORCED), true).await;
    let owner = decisions_of(&state);
    let held = hold(&owner, "release", "a", "team:owners", agent_requester());
    let digest = Sha256Digest::of(RENDERING.as_bytes()).to_string();
    let at_ms = now_ms();

    let attested = rpc(
        &app,
        Some(ALICE),
        "meta/decisions/decide",
        json!({
            "decision_id": held.decision_id.to_string(),
            "approve": true,
            "presented_digest": digest,
            "attestation": {
                "ingress": "chat",
                "subject": "U123",
                "scope": "chat-session-1",
                "event_ref": "chat:event-1",
                "observed_at_ms": at_ms,
                // What the person was shown, whole: a surface that took the
                // field would decide with it.
                "presentation": {
                    "audience": {"kind": "approver", "approver_id": "team:owners"},
                    "subject_digest": digest,
                    "parts_shown": [{"index": 0, "count": 1, "part_digest": digest}],
                    "channel_ref": "chat:owners",
                    "message_ref": "message-1",
                    "at_ms": at_ms,
                },
            },
        }),
    )
    .await;
    assert_eq!(attested["error"]["code"], json!(-32601), "{attested}");
    // Nothing was applied: the Console still reads the decision open and
    // undecided.
    let read = rpc(
        &app,
        Some(ALICE),
        "meta/decisions/get",
        json!({"decision_id": held.decision_id.to_string()}),
    )
    .await;
    assert_eq!(read["result"]["decision"]["phase"], json!("open"), "{read}");
    assert!(read["result"]["decision"].get("outcome").is_none());
    drop(app);
    runtime.shutdown().await;
}

#[test]
fn a_star_rule_grants_the_new_decision_actions() {
    for action in ["decisions.view", "decisions.decide", "decisions.admin"] {
        assert!(ACCESS_ACTIONS.contains(&action), "{action} is declared");
    }
    for pattern in ["*", "decisions.*"] {
        let config: AccessControlConfig = toml::from_str(&format!(
            r#"
enabled = true
admins = ["admin@example.test"]

[[rules]]
id = "everything"
subjects = ["alice@example.test"]
actions = ["{pattern}"]
"#
        ))
        .expect("access TOML");
        let controller = AccessController::new(config).expect("the pattern is accepted");
        let view = controller.view_for_subject(Some(ALICE));
        for action in ["decisions.view", "decisions.decide", "decisions.admin"] {
            assert!(view.allows(action), "{pattern} grants {action}");
        }
        let outsider = controller.view_for_subject(Some(DAVE));
        assert!(!outsider.allows("decisions.view"));
    }
}

const WITHDRAW: &str = "meta/decisions/withdraw";

const ACCESS_OFF: &str = r#"
enabled = false

[approvers."team:owners"]
subjects = ["alice@example.test"]
"#;

/// Carol reads decisions and holds two approver ids.
const SEVERAL_STREAMS: &str = r#"
enabled = true
admins = ["admin@example.test"]

[[rules]]
id = "decision-readers"
subjects = ["carol@example.test"]
actions = ["decisions.view"]

[approvers."team:owners"]
subjects = ["carol@example.test"]

[approvers."team:ops"]
subjects = ["carol@example.test"]

[approvers."team:other"]
subjects = ["alice@example.test"]
"#;

fn now_ms() -> u64 {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_millis(),
    )
    .expect("milliseconds")
}

/// Make the stored row of `decision` one the store cannot decode.
fn make_unreadable(state: &Path, decision: &StoredDecision) {
    let store = rusqlite::Connection::open(state.join("approval_decisions.sqlite3"))
        .expect("open the decision store");
    store
        .busy_timeout(Duration::from_secs(10))
        .expect("busy timeout");
    let changed = store
        .execute(
            "UPDATE approval_decisions SET row_json = ?1 WHERE decision_id = ?2",
            rusqlite::params![b"not a decision".to_vec(), decision.decision_id.to_string()],
        )
        .expect("overwrite the row");
    assert_eq!(changed, 1, "one row is made unreadable");
}

/// What one caller's walk over every list page read.
#[derive(Debug, Default)]
struct Listed {
    whole: Vec<String>,
    statuses: Vec<String>,
    unreadable: Vec<String>,
}

/// Every page of `meta/decisions/list` with `limit`, signed in as
/// `subject`. Each cursor must name the last row of its page, a row the
/// caller was given.
async fn walk(app: &axum::Router, subject: Option<&str>, limit: u64) -> Listed {
    walk_filtered(app, subject, limit, json!({})).await
}

/// [`walk`] with the list filters `filters`.
async fn walk_filtered(
    app: &axum::Router,
    subject: Option<&str>,
    limit: u64,
    filters: Value,
) -> Listed {
    let mut listed = Listed::default();
    let mut after: Option<String> = None;
    for _ in 0..64 {
        let mut params = filters.clone();
        params["limit"] = json!(limit);
        if let Some(after) = &after {
            params["after"] = json!(after);
        }
        let page = rpc(app, subject, "meta/decisions/list", params).await;
        assert!(page["error"].is_null(), "{page}");
        let result = &page["result"];
        let whole = ids(&result["decisions"]);
        let statuses = ids(&result["statuses"]);
        let unreadable = ids(&result["unreadable"]);
        let mut given: Vec<String> = whole
            .iter()
            .chain(&statuses)
            .chain(&unreadable)
            .cloned()
            .collect();
        given.sort();
        listed.whole.extend(whole);
        listed.statuses.extend(statuses);
        listed.unreadable.extend(unreadable);
        match result.get("next_after").and_then(Value::as_str) {
            None => {
                listed.whole.sort();
                listed.statuses.sort();
                listed.unreadable.sort();
                return listed;
            }
            Some(cursor) => {
                assert_eq!(
                    given.last().map(String::as_str),
                    Some(cursor),
                    "the cursor names the last row the caller was given: {page}"
                );
                after = Some(cursor.to_string());
            }
        }
    }
    panic!("the listing did not end");
}

/// A withdraw the Console refused with `code`: the answer holds no part of
/// the decision, and the decision is as it was.
fn assert_withdraw_refused(
    answer: &Value,
    code: i64,
    owner: &DurableDecisionService,
    decision: &StoredDecision,
) {
    assert_eq!(answer["error"]["code"], json!(code), "{answer}");
    assert!(answer["result"].is_null(), "{answer}");
    let text = answer.to_string();
    for field in [
        "\"decision\"",
        "operation_id",
        "subject_digest",
        decision.descriptor.operation_id.as_str(),
        decision.descriptor.approver_id.as_str(),
    ] {
        assert!(!text.contains(field), "{field} in {answer}");
    }
    let row = owner.get(&decision.decision_id).expect("row");
    assert_eq!(row.state.phase, DecisionPhase::Open, "{answer}");
    assert!(row.outcome.is_none());
    assert!(row.state.ended_at_ms.is_none());
}

#[tokio::test]
async fn the_console_list_cursor_never_names_a_decision_the_caller_cannot_see() {
    let dir = tempfile::tempdir().expect("dir");
    let state = dir.path().join("state");
    let (runtime, app) = console(&state, Some(ENFORCED), true).await;
    let owner = decisions_of(&state);
    let owners: Vec<StoredDecision> = (0..3)
        .map(|n| {
            hold(
                &owner,
                "release",
                &format!("owners-{n}"),
                "team:owners",
                agent_requester(),
            )
        })
        .collect();
    for n in 0..3 {
        hold(
            &owner,
            "release",
            &format!("ops-{n}"),
            "team:ops",
            agent_requester(),
        );
    }
    let carol_release = hold(
        &owner,
        "route.release",
        "carol-1",
        "team:ops",
        console_requester(CAROL),
    );
    let broken = hold(
        &owner,
        "release",
        "owners-broken",
        "team:owners",
        agent_requester(),
    );
    make_unreadable(&state, &broken);

    // The administrator reads the store's own pages, the unreadable row too.
    let admin = walk(&app, Some(ADMIN), 1).await;
    assert_eq!(
        admin.unreadable,
        [broken.decision_id.to_string()],
        "the store holds one unreadable row"
    );
    assert_eq!(admin.whole.len(), 7, "{admin:?}");

    // A reader with `decisions.view` pages over what it may see only.
    let alice_rows: Vec<&StoredDecision> = owners.iter().collect();
    for limit in [1, 2, 100] {
        let alice = walk(&app, Some(ALICE), limit).await;
        assert_eq!(alice.whole, sorted(&alice_rows), "limit {limit}");
        assert!(alice.statuses.is_empty(), "limit {limit}: {alice:?}");
        assert!(alice.unreadable.is_empty(), "limit {limit}: {alice:?}");
        let carol = walk(&app, Some(CAROL), limit).await;
        assert!(carol.whole.is_empty(), "limit {limit}: {carol:?}");
        assert_eq!(carol.statuses, sorted(&[&carol_release]), "limit {limit}");
        assert!(carol.unreadable.is_empty(), "limit {limit}: {carol:?}");
    }
    drop(app);
    runtime.shutdown().await;

    // A reader who holds two approver ids and asked for decisions of its
    // own: its holder and request streams merge in decision id order, a row
    // in two streams is given once, its own filters apply inside each
    // stream, and every cursor names a row it was given.
    let several = dir.path().join("several");
    let (runtime, app) = console(&several, Some(SEVERAL_STREAMS), true).await;
    let owner = decisions_of(&several);
    let mut owners_rows = Vec::new();
    let mut ops_rows = Vec::new();
    for n in 0..3 {
        owners_rows.push(hold(
            &owner,
            "release",
            &format!("owners-{n}"),
            "team:owners",
            agent_requester(),
        ));
        ops_rows.push(hold(
            &owner,
            "release",
            &format!("ops-{n}"),
            "team:ops",
            agent_requester(),
        ));
    }
    // Held for an approver id Carol holds, and asked by Carol: one whole row.
    let own_held = hold(
        &owner,
        "release",
        "own-owners",
        "team:owners",
        console_requester(CAROL),
    );
    let asked: Vec<StoredDecision> = (0..2)
        .map(|n| {
            hold(
                &owner,
                "route.release",
                &format!("asked-{n}"),
                "team:other",
                console_requester(CAROL),
            )
        })
        .collect();
    hold(
        &owner,
        "release",
        "other-0",
        "team:other",
        agent_requester(),
    );
    let broken = hold(
        &owner,
        "release",
        "ops-broken",
        "team:ops",
        agent_requester(),
    );
    make_unreadable(&several, &broken);
    let whole_rows: Vec<&StoredDecision> = owners_rows
        .iter()
        .chain(&ops_rows)
        .chain([&own_held])
        .collect();
    let asked_rows: Vec<&StoredDecision> = asked.iter().collect();
    for limit in [1, 2, 3, 100] {
        let carol = walk(&app, Some(CAROL), limit).await;
        assert_eq!(carol.whole, sorted(&whole_rows), "limit {limit}");
        assert_eq!(carol.statuses, sorted(&asked_rows), "limit {limit}");
        assert!(carol.unreadable.is_empty(), "limit {limit}: {carol:?}");
    }
    let ops_only = walk_filtered(&app, Some(CAROL), 2, json!({"approver_id": "team:ops"})).await;
    let ops_refs: Vec<&StoredDecision> = ops_rows.iter().collect();
    assert_eq!(ops_only.whole, sorted(&ops_refs));
    assert!(ops_only.statuses.is_empty(), "{ops_only:?}");
    let not_held = walk_filtered(&app, Some(CAROL), 2, json!({"approver_id": "team:nobody"})).await;
    assert!(
        not_held.whole.is_empty() && not_held.statuses.is_empty(),
        "an approver id the caller does not hold opens no stream: {not_held:?}"
    );
    let own_requests = walk_filtered(
        &app,
        Some(CAROL),
        1,
        json!({"requester": console_requester(CAROL).to_string()}),
    )
    .await;
    assert_eq!(own_requests.whole, sorted(&[&own_held]));
    assert_eq!(own_requests.statuses, sorted(&asked_rows));
    let releases = walk_filtered(&app, Some(CAROL), 1, json!({"kind": "route.release"})).await;
    assert!(releases.whole.is_empty(), "{releases:?}");
    assert_eq!(releases.statuses, sorted(&asked_rows));
    drop(app);
    runtime.shutdown().await;

    // An Open Console with access control off: the anonymous caller holds
    // no approver id and asked for nothing, so it reads nothing, and no
    // cursor names a decision.
    let open = dir.path().join("open");
    let (runtime, app) = console(&open, Some(ACCESS_OFF), false).await;
    let owner = decisions_of(&open);
    for n in 0..3 {
        hold(
            &owner,
            "release",
            &format!("owners-{n}"),
            "team:owners",
            agent_requester(),
        );
    }
    let broken = hold(
        &owner,
        "release",
        "owners-broken",
        "team:owners",
        agent_requester(),
    );
    make_unreadable(&open, &broken);
    for limit in [1, 100] {
        let page = rpc(&app, None, "meta/decisions/list", json!({ "limit": limit })).await;
        assert!(page["error"].is_null(), "{page}");
        assert_eq!(page["result"]["decisions"], json!([]), "{page}");
        assert_eq!(page["result"]["statuses"], json!([]), "{page}");
        assert_eq!(page["result"]["unreadable"], json!([]), "{page}");
        assert!(
            page["result"].get("next_after").is_none(),
            "no cursor names a decision: {page}"
        );
    }
    drop(app);
    runtime.shutdown().await;
}

#[tokio::test]
async fn console_withdraw_with_access_control_off_is_refused() {
    let dir = tempfile::tempdir().expect("dir");
    let state = dir.path().join("state");
    let (runtime, app) = console(&state, Some(ACCESS_OFF), true).await;
    let owner = decisions_of(&state);
    let held = hold(&owner, "release", "a", "team:owners", agent_requester());

    // The holder, a would-be administrator and anyone else: with access
    // control off the Console withdraws nothing.
    for subject in [ALICE, ADMIN, CAROL] {
        let answer = rpc(
            &app,
            Some(subject),
            WITHDRAW,
            json!({"decision_id": held.decision_id.to_string()}),
        )
        .await;
        assert_withdraw_refused(&answer, -32601, &owner, &held);
    }
    let capabilities = rpc(&app, Some(ALICE), "meta/capabilities", json!({})).await;
    let methods = capabilities["result"]["methods"]
        .as_array()
        .unwrap_or_else(|| panic!("methods: {capabilities}"));
    assert!(
        !methods.contains(&json!(WITHDRAW)),
        "the Console offers no withdraw: {capabilities}"
    );
    drop(app);
    runtime.shutdown().await;
}

#[tokio::test]
async fn an_open_console_anonymous_caller_cannot_withdraw() {
    let dir = tempfile::tempdir().expect("dir");
    let state = dir.path().join("state");
    let (runtime, app) = console(&state, Some(ACCESS_OFF), false).await;
    let owner = decisions_of(&state);
    let held = hold(&owner, "release", "a", "team:owners", agent_requester());

    let answer = rpc(
        &app,
        None,
        WITHDRAW,
        json!({"decision_id": held.decision_id.to_string()}),
    )
    .await;
    assert_withdraw_refused(&answer, -32601, &owner, &held);
    // The same answer for an id that names no decision.
    let absent = rpc(
        &app,
        None,
        WITHDRAW,
        json!({"decision_id": format!("decision:{}", "0".repeat(64))}),
    )
    .await;
    assert_eq!(absent["error"], answer["error"], "{absent}");
    drop(app);
    runtime.shutdown().await;
}

#[tokio::test]
async fn console_withdraw_without_an_access_controller_is_refused() {
    let dir = tempfile::tempdir().expect("dir");
    let state = dir.path().join("state");
    let (runtime, app) = console(&state, None, false).await;
    let owner = decisions_of(&state);
    let held = hold(&owner, "release", "a", "team:owners", agent_requester());

    let answer = rpc(
        &app,
        None,
        WITHDRAW,
        json!({"decision_id": held.decision_id.to_string()}),
    )
    .await;
    assert_withdraw_refused(&answer, -32601, &owner, &held);
    let absent = rpc(
        &app,
        None,
        WITHDRAW,
        json!({"decision_id": format!("decision:{}", "0".repeat(64))}),
    )
    .await;
    assert_eq!(absent["error"], answer["error"], "{absent}");
    drop(app);
    runtime.shutdown().await;
}

#[tokio::test]
async fn a_requester_cannot_withdraw_its_own_route_release_or_grant_child_from_the_console() {
    for (access, code) in [(ACCESS_OFF, -32601), (ENFORCED, -32030)] {
        let dir = tempfile::tempdir().expect("dir");
        let state = dir.path().join("state");
        let (runtime, app) = console(&state, Some(access), true).await;
        let owner = decisions_of(&state);
        let release = hold(
            &owner,
            "route.release",
            "answer-1",
            "team:owners",
            console_requester(CAROL),
        );
        let child = hold(
            &owner,
            "grant.child",
            "child-1",
            "team:owners",
            console_requester(CAROL),
        );
        for decision in [&release, &child] {
            let answer = rpc(
                &app,
                Some(CAROL),
                WITHDRAW,
                json!({"decision_id": decision.decision_id.to_string()}),
            )
            .await;
            assert_withdraw_refused(&answer, code, &owner, decision);
        }
        drop(app);
        runtime.shutdown().await;
    }
}

#[tokio::test]
async fn a_console_operator_form_decide_is_method_not_found_and_applies_nothing() {
    let dir = tempfile::tempdir().expect("dir");
    let state = dir.path().join("state");
    let (runtime, app) = console(&state, Some(ENFORCED), true).await;
    let owner = decisions_of(&state);
    let held = hold(&owner, "release", "a", "team:owners", agent_requester());

    // The holder and the administrator alike: the Console decides nothing.
    for subject in [ALICE, ADMIN] {
        let answer = rpc(
            &app,
            Some(subject),
            "meta/decisions/decide",
            json!({
                "decision_id": held.decision_id.to_string(),
                "approve": true,
                "presented_digest": Sha256Digest::of(RENDERING.as_bytes()).to_string(),
            }),
        )
        .await;
        assert_eq!(answer["error"]["code"], json!(-32601), "{answer}");
    }
    let row = owner.get(&held.decision_id).expect("row");
    assert_eq!(row.state.phase, DecisionPhase::Open);
    assert!(row.outcome.is_none());
    drop(app);
    runtime.shutdown().await;
}

/// Dave holds `team:owners` only as the Console subject of a `people`
/// entry.
const PEOPLE_SUBJECT: &str = r#"
enabled = true
admins = ["admin@example.test"]

[[rules]]
id = "people-holder"
subjects = ["dave@example.test"]
actions = ["decisions.view", "agent.view", "gating.decide"]

[approvers."team:owners"]
people = [{ subjects = ["dave@example.test"], ingress_subjects = [{ ingress = "operator" }] }]
"#;

#[tokio::test]
async fn a_people_subject_holds_the_approver_id_on_the_console() {
    let dir = tempfile::tempdir().expect("dir");
    let state = dir.path().join("state");
    let (runtime, app) = console(&state, Some(PEOPLE_SUBJECT), true).await;
    let owner = decisions_of(&state);
    let held = hold(&owner, "release", "a", "team:owners", agent_requester());

    // Console reads: the person reads the decision whole.
    let listed = rpc(&app, Some(DAVE), "meta/decisions/list", json!({})).await;
    assert_eq!(
        ids(&listed["result"]["decisions"]),
        sorted(&[&held]),
        "{listed}"
    );
    assert_eq!(listed["result"]["statuses"], json!([]), "{listed}");
    let rendering = rpc(
        &app,
        Some(DAVE),
        "meta/decisions/rendering",
        json!({"decision_id": held.decision_id.to_string()}),
    )
    .await;
    assert_eq!(
        rendering["result"]["rendering"],
        json!(RENDERING),
        "{rendering}"
    );

    // Gating decide: the gate names the approver id, and the Console decides
    // as the signed-in person, who holds that id only as the people subject.
    let evaluated = runtime
        .evaluate_gating_action_with_origin(
            serde_json::from_value(json!({
                "action": "release the reviewed artifact",
                "actor_id": "requester",
                "risk_tier": "r3",
                "requested_approver": "team:owners",
            }))
            .expect("gating request"),
            Some(GatingOrigin {
                identity: "router".to_string(),
                conversation_id: None,
                interaction_id: None,
            }),
        )
        .await;
    let pending_id = evaluated.pending_id.clone().expect("a pending gate");
    let decided = rpc(
        &app,
        Some(DAVE),
        "meta/gating/decide",
        json!({
            "pending_id": pending_id,
            "decision": "approve",
        }),
    )
    .await;
    assert!(decided["error"].is_null(), "{decided}");
    assert_eq!(decided["result"]["approver_id"], json!("team:owners"));
    drop(app);
    runtime.shutdown().await;
}

/// The decision ingresses `chat` and `mail`.
fn chat_and_mail() -> Value {
    json!({"ingresses": [
        {"ingress": "chat", "attests": "person", "max_clock_skew_ms": 5000},
        {"ingress": "mail", "attests": "person", "max_clock_skew_ms": 5000},
    ]})
}

/// The ENFORCED configuration with `team:owners` held as `approver` (TOML).
fn enforced_with_owners(approver: &str) -> AccessControlConfig {
    let mut config: AccessControlConfig = toml::from_str(ENFORCED).expect("access TOML");
    let owners: AccessApprover = toml::from_str(approver).expect("approver TOML");
    config.approvers.insert("team:owners".to_string(), owners);
    config
}

/// A Console access save in the checked form every Console access write
/// takes: `config` with the owner instance and the revision `subject` reads
/// from `meta/access/get`.
async fn checked_access_set(
    app: &axum::Router,
    subject: &str,
    config: &AccessControlConfig,
) -> Value {
    let read = rpc(app, Some(subject), "meta/access/get", json!({})).await;
    assert!(read["error"].is_null(), "{read}");
    let owner_instance = read["result"]["owner_instance"].clone();
    let expected_revision = read["result"]["revision"].clone();
    assert!(
        owner_instance.is_string() && expected_revision.is_u64(),
        "{read}"
    );
    rpc(
        app,
        Some(subject),
        "meta/access/set",
        json!({"checked_v1": {
            "config": config,
            "owner_instance": owner_instance,
            "expected_revision": expected_revision,
        }}),
    )
    .await
}

#[tokio::test]
async fn an_identity_in_two_people_entries_of_one_binding_is_refused() {
    // At load: the operator, an ingress subject and a Console subject, each
    // listed in two people entries of one binding.
    for (case, identity) in [
        (
            r#"
[approvers."team:owners"]
people = [
  { ingress_subjects = [{ ingress = "operator" }] },
  { ingress_subjects = [{ ingress = "operator" }, { ingress = "chat", subject = "U1" }] },
]
"#,
            "operator",
        ),
        (
            r#"
[approvers."team:owners"]
people = [
  { ingress_subjects = [{ ingress = "chat", subject = "U123" }] },
  { ingress_subjects = [{ ingress = "chat", subject = "U123" }, { ingress = "mail", subject = "owner" }] },
]
"#,
            "U123",
        ),
        (
            r#"
[approvers."team:owners"]
people = [
  { subjects = ["dave@example.test"] },
  { subjects = ["dave@example.test"], ingress_subjects = [{ ingress = "chat", subject = "U9" }] },
]
"#,
            "dave@example.test",
        ),
    ] {
        let config: AccessControlConfig = toml::from_str(case).expect("access TOML");
        let refused = AccessController::new(config)
            .expect_err("an identity in two people entries of one binding is refused at load");
        let text = refused.to_string();
        assert!(
            text.contains("team:owners") && text.contains(identity),
            "the refusal names the binding and the identity: {text}"
        );
    }
    // Linking is per binding: one person is linked in each binding it holds.
    let per_binding: AccessControlConfig = toml::from_str(
        r#"
[approvers."team:owners"]
people = [{ ingress_subjects = [{ ingress = "chat", subject = "U123" }, { ingress = "operator" }] }]

[approvers."team:ops"]
people = [{ ingress_subjects = [{ ingress = "chat", subject = "U123" }, { ingress = "operator" }] }]
"#,
    )
    .expect("access TOML");
    AccessController::new(per_binding).expect("one entry per binding is accepted");

    // A Console save is refused the same way, and saves nothing.
    let dir = tempfile::tempdir().expect("dir");
    let state = dir.path().join("state");
    let (runtime, app) = console_with(&state, Some(ENFORCED), true, Some(chat_and_mail())).await;
    let before = runtime.access_controller().expect("controller").snapshot();
    let config = enforced_with_owners(
        r#"
subjects = ["alice@example.test"]
people = [
  { ingress_subjects = [{ ingress = "chat", subject = "U123" }] },
  { ingress_subjects = [{ ingress = "chat", subject = "U123" }, { ingress = "mail", subject = "owner" }] },
]
"#,
    );
    let saved = checked_access_set(&app, ADMIN, &config).await;
    assert_eq!(saved["error"]["code"], json!(-32602), "{saved}");
    assert_eq!(
        saved["error"]["data"]["kind"],
        json!("invalid_access_config"),
        "{saved}"
    );
    assert_eq!(
        saved["error"]["data"]["reason"],
        json!("duplicate_person_identity"),
        "{saved}"
    );
    let after = runtime.access_controller().expect("controller").snapshot();
    assert_eq!(after.1, before.1, "nothing was saved");
    assert_eq!(after.0, before.0);
    drop(app);
    runtime.shutdown().await;
}

#[tokio::test]
async fn a_console_access_save_naming_an_undeclared_ingress_is_refused_unknown_ingress() {
    let dir = tempfile::tempdir().expect("dir");
    let state = dir.path().join("state");
    let (runtime, app) = console_with(&state, Some(ENFORCED), true, Some(chat_and_mail())).await;
    let before = runtime.access_controller().expect("controller").snapshot();

    let undeclared = enforced_with_owners(
        r#"
subjects = ["alice@example.test"]
ingress_subjects = [{ ingress = "sms", subject = "+15550100" }]
"#,
    );
    let refused = checked_access_set(&app, ADMIN, &undeclared).await;
    assert_eq!(refused["error"]["code"], json!(-32602), "{refused}");
    assert_eq!(
        refused["error"]["data"]["kind"],
        json!("invalid_access_config"),
        "{refused}"
    );
    assert_eq!(
        refused["error"]["data"]["reason"],
        json!("unknown_ingress"),
        "{refused}"
    );
    assert!(
        refused["error"]["data"]["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("\"sms\"")),
        "the detail names the ingress: {refused}"
    );
    let after = runtime.access_controller().expect("controller").snapshot();
    assert_eq!(after.1, before.1, "nothing was saved");

    // A declared ingress saves.
    let declared = enforced_with_owners(
        r#"
subjects = ["alice@example.test"]
ingress_subjects = [{ ingress = "chat", subject = "U123" }]
"#,
    );
    let saved = checked_access_set(&app, ADMIN, &declared).await;
    assert!(saved["error"].is_null(), "{saved}");
    let saved_revision = runtime
        .access_controller()
        .expect("controller")
        .snapshot()
        .1;
    assert!(saved_revision > before.1, "the declared ingress was saved");
    drop(app);
    runtime.shutdown().await;
}

#[tokio::test]
async fn an_unreadable_row_reads_as_not_found_to_a_non_administrator() {
    let absent = format!("decision:{}", "0".repeat(64));
    // An enforced reader who holds nothing and asked for nothing, and an
    // Open Console's anonymous caller with access control off.
    for (case, access, require_app_auth, subject) in [
        ("enforced", ENFORCED, true, Some(CAROL)),
        ("open", ACCESS_OFF, false, None),
    ] {
        let dir = tempfile::tempdir().expect("dir");
        let state = dir.path().join("state");
        let (runtime, app) = console(&state, Some(access), require_app_auth).await;
        let owner = decisions_of(&state);
        let broken = hold(
            &owner,
            "release",
            "probe-broken",
            "team:ops",
            agent_requester(),
        );
        make_unreadable(&state, &broken);
        let readable = hold(
            &owner,
            "release",
            "probe-readable",
            "team:ops",
            agent_requester(),
        );
        for method in ["meta/decisions/get", "meta/decisions/rendering"] {
            let unreadable = rpc(
                &app,
                subject,
                method,
                json!({"decision_id": broken.decision_id.to_string()}),
            )
            .await;
            let invisible = rpc(
                &app,
                subject,
                method,
                json!({"decision_id": readable.decision_id.to_string()}),
            )
            .await;
            let missing = rpc(&app, subject, method, json!({"decision_id": absent})).await;
            assert_eq!(
                missing["error"]["data"]["kind"],
                json!("decision_not_found"),
                "{missing}"
            );
            assert_eq!(
                invisible, missing,
                "{case} {method}: a row the caller cannot see"
            );
            assert_eq!(
                unreadable, missing,
                "{case} {method}: an unreadable row answers as an absent id"
            );
            if case == "enforced" {
                let administrator = rpc(
                    &app,
                    Some(ADMIN),
                    method,
                    json!({"decision_id": broken.decision_id.to_string()}),
                )
                .await;
                assert_eq!(
                    administrator["error"]["data"]["kind"],
                    json!("decision_unreadable"),
                    "{administrator}"
                );
                assert_eq!(
                    administrator["error"]["code"],
                    json!(-32603),
                    "{administrator}"
                );
            }
        }
        drop(app);
        runtime.shutdown().await;
    }
}

/// Only an access administrator is told of the rows the store cannot
/// decode. A person who administers decisions by a rule is kept apart from
/// other people, and is told of none: the list names no such row, and by
/// its id the row answers as an id that names no decision.
#[tokio::test]
async fn only_an_access_administrator_is_told_of_rows_the_store_cannot_decode() {
    let absent = format!("decision:{}", "0".repeat(64));
    // ERIN reads and administers decisions by a rule; ADMIN is an access
    // administrator.
    let access = r#"
enabled = true
admins = ["admin@example.test"]

[[rules]]
id = "decision-admin-by-rule"
subjects = ["erin@example.test"]
actions = ["decisions.view", "decisions.admin"]
"#;
    let dir = tempfile::tempdir().expect("dir");
    let state = dir.path().join("state");
    let (runtime, app) = console(&state, Some(access), true).await;
    let owner = decisions_of(&state);
    let readable = hold(
        &owner,
        "release",
        "decode-readable",
        "team:ops",
        agent_requester(),
    );
    let broken = hold(
        &owner,
        "release",
        "decode-broken",
        "team:ops",
        agent_requester(),
    );
    make_unreadable(&state, &broken);
    let by_rule = walk(&app, Some(ERIN), 1).await;
    let access_admin = walk(&app, Some(ADMIN), 1).await;
    let by_rule_get = rpc(
        &app,
        Some(ERIN),
        "meta/decisions/get",
        json!({"decision_id": broken.decision_id.to_string()}),
    )
    .await;
    let by_rule_absent = rpc(
        &app,
        Some(ERIN),
        "meta/decisions/get",
        json!({"decision_id": absent}),
    )
    .await;
    drop(app);
    runtime.shutdown().await;

    assert_eq!(
        access_admin.unreadable,
        [broken.decision_id.to_string()],
        "an access administrator is told of the row"
    );
    assert_eq!(by_rule.whole, sorted(&[&readable]), "{by_rule:?}");
    assert!(
        by_rule.unreadable.is_empty(),
        "a person who administers decisions by a rule is told of none: {by_rule:?}"
    );
    assert_eq!(
        by_rule_get, by_rule_absent,
        "by its id the row answers as an id that names no decision"
    );
}

/// The console audit log's rows of `meta/decisions/withdraw`, read by the
/// access admin, read again for a short while until there are `expected` of
/// them (a row lands with its change's answer, or just after it).
async fn withdraw_audit_rows(app: &axum::Router, expected: usize) -> Vec<Value> {
    let mut rows = Vec::new();
    for _ in 0..50 {
        let read = rpc(
            app,
            Some(ADMIN),
            "meta/console/audit",
            json!({"log": "changes"}),
        )
        .await;
        assert!(
            read["error"].is_null(),
            "the console keeps an audit log: {read}"
        );
        rows = read["result"]["records"]
            .as_array()
            .unwrap_or_else(|| panic!("records: {read}"))
            .iter()
            .filter(|row| row["method"] == json!(WITHDRAW))
            .cloned()
            .collect();
        if rows.len() >= expected {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    rows
}

/// A Console withdraw is a console change: the console audit log records
/// it, the administrator's withdraw as committed and a refused one as
/// refused with its code and kind, each naming the decision.
#[tokio::test]
async fn a_console_withdraw_writes_a_console_audit_row() {
    let dir = tempfile::tempdir().expect("dir");
    let state = dir.path().join("state");
    let (runtime, app) = console(&state, Some(ENFORCED), true).await;
    let owner = decisions_of(&state);
    let held = hold(&owner, "release", "a", "team:owners", agent_requester());
    let decision_id = held.decision_id.to_string();

    let refused = rpc(
        &app,
        Some(ALICE),
        WITHDRAW,
        json!({"decision_id": decision_id}),
    )
    .await;
    assert_eq!(
        refused["error"]["data"]["kind"],
        json!("access_denied"),
        "{refused}"
    );
    let withdrawn = rpc(
        &app,
        Some(ADMIN),
        WITHDRAW,
        json!({"decision_id": decision_id}),
    )
    .await;
    assert!(withdrawn["error"].is_null(), "{withdrawn}");

    let rows = withdraw_audit_rows(&app, 2).await;
    assert_eq!(rows.len(), 2, "one row for each withdraw: {rows:#?}");
    let refused_row = rows
        .iter()
        .find(|row| row["outcome"] == json!("refused"))
        .unwrap_or_else(|| panic!("a refused row: {rows:#?}"));
    assert_eq!(refused_row["code"], json!(-32030), "{refused_row:#}");
    assert_eq!(
        refused_row["kind"],
        json!("access_denied"),
        "{refused_row:#}"
    );
    let committed_row = rows
        .iter()
        .find(|row| row["outcome"] == json!("committed"))
        .unwrap_or_else(|| panic!("a committed row: {rows:#?}"));
    for row in [refused_row, committed_row] {
        assert_eq!(row["target"]["decision_id"], json!(decision_id), "{row:#}");
    }
    drop(app);
    runtime.shutdown().await;
}

/// A Console withdraw is a console change: one made for another caller
/// than its token (`expected_caller_scope`) is refused `caller_changed` and
/// withdraws nothing, and the caller's own scope withdraws. With access
/// control off the Console serves no withdraw, whatever scope it names.
#[tokio::test]
async fn a_console_withdraw_made_for_another_caller_is_refused() {
    let dir = tempfile::tempdir().expect("dir");
    let state = dir.path().join("state");
    let (runtime, app) = console(&state, Some(ENFORCED), true).await;
    let owner = decisions_of(&state);
    let held = hold(&owner, "release", "a", "team:owners", agent_requester());
    let params = |scope: &str| {
        json!({
            "decision_id": held.decision_id.to_string(),
            "expected_caller_scope": scope,
        })
    };

    let refused = rpc(
        &app,
        Some(ADMIN),
        WITHDRAW,
        params("console-caller:another"),
    )
    .await;
    assert_eq!(
        refused["error"]["data"]["kind"],
        json!("caller_changed"),
        "{refused}"
    );
    assert_withdraw_refused(&refused, -32056, &owner, &held);

    // The caller's own scope, as `meta/console/whoami` reports it, withdraws.
    let whoami = rpc(&app, Some(ADMIN), "meta/console/whoami", json!({})).await;
    let scope = whoami["result"]["caller_scope"]
        .as_str()
        .unwrap_or_else(|| panic!("caller scope: {whoami}"))
        .to_string();
    let withdrawn = rpc(&app, Some(ADMIN), WITHDRAW, params(&scope)).await;
    assert!(withdrawn["error"].is_null(), "{withdrawn}");
    assert_eq!(
        owner.get(&held.decision_id).expect("row").state.phase,
        DecisionPhase::Withdrawn
    );
    drop(app);
    runtime.shutdown().await;

    let off_dir = tempfile::tempdir().expect("dir");
    let off_state = off_dir.path().join("state");
    let (off_runtime, off_app) = console(&off_state, Some(ACCESS_OFF), true).await;
    let off_owner = decisions_of(&off_state);
    let off_held = hold(&off_owner, "release", "b", "team:owners", agent_requester());
    let not_served = rpc(
        &off_app,
        Some(ADMIN),
        WITHDRAW,
        json!({
            "decision_id": off_held.decision_id.to_string(),
            "expected_caller_scope": "console-caller:another",
        }),
    )
    .await;
    assert_withdraw_refused(&not_served, -32601, &off_owner, &off_held);
    drop(off_app);
    off_runtime.shutdown().await;
}

/// A read-only Console refuses an administrator's withdraw (`-32010`)
/// before any store read, and the decision stays open; its capabilities
/// list the reads and no withdraw.
#[tokio::test]
async fn a_read_only_console_refuses_an_administrators_withdraw() {
    let dir = tempfile::tempdir().expect("dir");
    let state = dir.path().join("state");
    let (runtime, writable) = console(&state, Some(ENFORCED), true).await;
    drop(writable);
    let mut read_only = decision_state(true);
    read_only.console.read_only = true;
    let app = runtime.build_console_json_router(read_only);
    let owner = decisions_of(&state);
    let held = hold(&owner, "release", "a", "team:owners", agent_requester());

    let refused = rpc(
        &app,
        Some(ADMIN),
        WITHDRAW,
        json!({"decision_id": held.decision_id.to_string()}),
    )
    .await;
    assert_withdraw_refused(&refused, -32010, &owner, &held);

    let capabilities = rpc(&app, Some(ADMIN), "meta/capabilities", json!({})).await;
    let methods: Vec<&str> = capabilities["result"]["methods"]
        .as_array()
        .unwrap_or_else(|| panic!("methods: {capabilities}"))
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        methods.contains(&"meta/decisions/list"),
        "the reads stay listed: {capabilities}"
    );
    assert!(
        !methods.contains(&WITHDRAW),
        "a read-only Console lists no withdraw: {capabilities}"
    );
    drop(app);
    runtime.shutdown().await;
}

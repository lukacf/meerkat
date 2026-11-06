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
    ACCESS_ACTIONS, AccessControlConfig, AccessController, AuthPolicy, BigQueryNaming,
    ConsolePolicy, RuntimeDecisionInputs, RuntimeOpsPolicy, TrustedOidcRuntimeConfig,
    UnifiedRuntime, build_runtime_decision_state,
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

    let attested = rpc(
        &app,
        Some(ALICE),
        "meta/decisions/decide",
        json!({
            "decision_id": held.decision_id.to_string(),
            "approve": true,
            "presented_digest": Sha256Digest::of(RENDERING.as_bytes()).to_string(),
            "attestation": {
                "ingress": "chat",
                "subject": "U123",
                "scope": "chat-session-1",
                "event_ref": "chat:event-1",
                "observed_at_ms": 1,
            },
        }),
    )
    .await;
    assert!(attested["error"].is_object(), "{attested}");
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

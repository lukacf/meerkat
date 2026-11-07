//! Who sent a console message. Each `user_input` frame names its sender,
//! which the Console sets from the caller it authenticated (never from the
//! request body). Only a person's send on a console that requires sign-in
//! carries the person's qualified principal to the agent's run as its
//! requester. The HTTP send routes refuse the origin kinds of Meerkat's
//! internal emitters.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use meerkat_client::TestClient;
use meerkat_core::{Message, PrincipalKind, PrincipalRef, TrustDomainId, TurnRequester};
use meerkat_meta::{
    AccessControlConfig, AccessController, AccessRule, AuthPolicy, BigQueryNaming, ConsolePolicy,
    RuntimeDecisionInputs, RuntimeDecisionState, RuntimeOpsPolicy, TrustedOidcRuntimeConfig,
    UnifiedRuntime, UnifiedRuntimeBuilder, build_runtime_decision_state,
};
use meerkat_mob::{MobDefinition, SpawnMemberSpec};
use serde_json::{Value, json};
use tower::ServiceExt;

/// A development issuer (a `.localhost` host), whose HS256 fixture tokens
/// the console verifies (signature, issuer, audience, allowlist).
const ISSUER: &str = "https://trusted.meta.localhost";
const ALICE: &str = "alice@example.test";
const BOB: &str = "bob@example.test";
const ROBOT: &str = "svc:robot";
/// The access admin an enabled access controller needs; no row sends as it.
const ADMIN: &str = "root@example.test";
const MEMBER: &str = "router";
const WAIT: Duration = Duration::from_secs(30);

fn decisions(require_app_auth: bool) -> RuntimeDecisionState {
    build_runtime_decision_state(RuntimeDecisionInputs {
        bigquery: BigQueryNaming {
            dataset: "senders_dataset".to_string(),
            table: "senders_table".to_string(),
        },
        trusted_meta_toml: "[[modules]]\nid = \"router\"\ncommand = \"router-bin\"\nargs = []\nrestart_policy = \"always\"\n".to_string(),
        auth: AuthPolicy {
            default_provider: meerkat_meta::AuthProvider::GoogleOAuth,
            email_allowlist: vec![ALICE.to_string(), BOB.to_string(), ROBOT.to_string()],
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

/// The OIDC `sub` of the person whose email is `email`.
fn subject_of(email: &str) -> String {
    format!("{email}-sub")
}

/// The person's qualified principal: the verified `sub` in the trust domain
/// of its verified issuer.
fn person(email: &str) -> PrincipalRef {
    PrincipalRef::in_domain(
        PrincipalKind::Human,
        subject_of(email),
        TrustDomainId::new(format!("oidc-issuer:{ISSUER}")).expect("trust domain"),
    )
    .expect("qualified person")
}

/// An `Authorization` value for a token with `claims`, signed by the
/// trusted issuer's development key.
fn bearer_with(mut claims: Value) -> String {
    claims["iss"] = json!(ISSUER);
    claims["aud"] = json!("meerkat-console");
    claims["exp"] = json!(chrono::Utc::now().timestamp() + 300);
    let mut jwt_header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256);
    jwt_header.kid = Some("kid-current".to_string());
    let token = jsonwebtoken::encode(
        &jwt_header,
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(b"phase7-trusted-current-secret"),
    )
    .expect("signed fixture token");
    format!("Bearer {token}")
}

/// A verified person's token.
fn person_bearer(email: &str) -> String {
    bearer_with(json!({
        "sub": subject_of(email),
        "email": email,
        "provider": "google_oauth",
    }))
}

/// A service's token: a `svc:` subject with the service actor type.
fn service_bearer() -> String {
    bearer_with(json!({ "sub": ROBOT, "actor_type": "service", "provider": "generic_oidc" }))
}

/// A persistent composition over `state`, as a gateway boots one.
async fn boot(state: &Path, mob_id: &str) -> UnifiedRuntime {
    UnifiedRuntimeBuilder::default()
        .definition(
            MobDefinition::from_toml(&format!(
                r#"
[mob]
id = "{mob_id}"
[profiles.agent]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "autonomous_host"
[profiles.agent.tools]
comms = true
"#
            ))
            .expect("definition"),
        )
        .persistent_state(state)
        .comms(true)
        .default_llm_client(Arc::new(TestClient::default()))
        .build()
        .await
        .expect("runtime boots")
}

/// A booted composition with the one member every row sends to.
async fn boot_with_member(state: &Path, mob_id: &str) -> UnifiedRuntime {
    let runtime = boot(state, mob_id).await;
    runtime
        .spawn(SpawnMemberSpec::from_wire(
            "agent".to_string(),
            MEMBER.to_string(),
            Some("You are router.".into()),
            None,
            None,
        ))
        .await
        .expect("spawn the member");
    runtime
}

fn mob_id(row: &str) -> String {
    format!("console-senders-{row}-{}", uuid::Uuid::new_v4())
}

fn state_dir() -> tempfile::TempDir {
    tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap()
}

async fn json_of(response: axum::response::Response) -> (StatusCode, Value) {
    let status = response.status();
    let body = to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .expect("body");
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

/// `POST /console/send` with the `Authorization` value `bearer`.
async fn send(
    app: &axum::Router,
    bearer: &str,
    content: &str,
    key: &str,
    origin_kind: Option<&str>,
) -> (StatusCode, Value) {
    let mut body = json!({
        "identity": MEMBER,
        "content": content,
        "origin": "console",
        "idempotency_key": key,
    });
    if let Some(kind) = origin_kind {
        body["origin_kind"] = json!(kind);
    }
    json_of(
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/console/send")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, bearer)
                    .body(Body::from(body.to_string()))
                    .expect("send request"),
            )
            .await
            .expect("send response"),
    )
    .await
}

/// One `/console/rpc` call with the `Authorization` value `bearer`.
async fn rpc(app: &axum::Router, bearer: &str, method: &str, params: Value) -> Value {
    let (status, answer) = json_of(
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/console/rpc")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, bearer)
                    .body(Body::from(
                        json!({
                            "jsonrpc": "2.0",
                            "id": "senders",
                            "method": method,
                            "params": params,
                        })
                        .to_string(),
                    ))
                    .expect("rpc request"),
            )
            .await
            .expect("rpc response"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{method}: {answer}");
    answer
}

/// The caller scope `meta/console/whoami` reports for `bearer`.
async fn caller_scope(app: &axum::Router, bearer: &str) -> Value {
    let whoami = rpc(app, bearer, "meta/console/whoami", json!({})).await;
    let scope = whoami["result"]["caller_scope"].clone();
    assert!(scope.is_string(), "{whoami}");
    scope
}

/// The member's `user_input` frames that console sends made, as `bearer`'s
/// view reads them.
async fn user_inputs(app: &axum::Router, bearer: &str) -> Vec<Value> {
    let page = rpc(
        app,
        bearer,
        "meta/console/query_timeline",
        json!({ "identity": MEMBER }),
    )
    .await;
    page["result"]["frames"]
        .as_array()
        .unwrap_or_else(|| panic!("timeline frames: {page}"))
        .iter()
        .filter(|frame| frame["kind"] == "user_input" && frame["source"]["kind"] == "send")
        .cloned()
        .collect()
}

fn frame_with<'a>(frames: &'a [Value], content: &str) -> &'a Value {
    frames
        .iter()
        .find(|frame| frame["payload"]["content"] == json!(content))
        .unwrap_or_else(|| panic!("a frame of {content:?}: {frames:#?}"))
}

/// The sender a frame of a person's send names, as a view reads it: the
/// person, its assurance and its caller scope, never a requester key.
fn person_sender(email: &str, caller_scope: &Value) -> Value {
    json!({
        "kind": "person",
        "assurance": "person_authenticated",
        "display_name": email,
        "caller_scope": caller_scope,
    })
}

/// The requesters of the member's committed user rows whose text is
/// `content`, once the member committed one.
async fn requester_rows(runtime: &UnifiedRuntime, content: &str) -> Vec<Option<TurnRequester>> {
    let member = runtime
        .mob_handle()
        .get_member(&meerkat_mob::ids::AgentIdentity::from(MEMBER))
        .await
        .expect("roster read")
        .expect("the member is listed");
    let session = member
        .bridge_session_id()
        .expect("the member has a session")
        .to_string();
    tokio::time::timeout(WAIT, async {
        loop {
            let rows: Vec<_> = runtime
                .mob_runtime()
                .read_session_history(&session, 0, None)
                .await
                .expect("history read")
                .messages
                .into_iter()
                .filter_map(|message| match message {
                    Message::User(user) if user.text_content() == content => {
                        Some(user.identity.requester)
                    }
                    _ => None,
                })
                .collect();
            if !rows.is_empty() {
                return rows;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("the member committed {content:?}"))
}

/// Two people sending to one member of an Enforced console are two senders:
/// each `user_input` frame names its own (kind, assurance, display name and
/// caller scope, never the requester key), each turn's transcript rows name
/// the person as their direct requester, and a gateway restart keeps the
/// senders.
#[tokio::test(flavor = "multi_thread")]
async fn two_people_one_member_distinct_senders() {
    let state = state_dir();
    let mob = mob_id("two-people");
    let runtime = boot_with_member(state.path(), &mob).await;
    let app = runtime.build_reference_app_router(decisions(true));

    let alice_scope = caller_scope(&app, &person_bearer(ALICE)).await;
    let bob_scope = caller_scope(&app, &person_bearer(BOB)).await;
    assert_ne!(alice_scope, bob_scope, "two people are two callers");

    let (status, sent) = send(&app, &person_bearer(ALICE), "from alice", "alice-1", None).await;
    assert_eq!(status, StatusCode::OK, "{sent}");
    let (status, sent) = send(&app, &person_bearer(BOB), "from bob", "bob-1", None).await;
    assert_eq!(status, StatusCode::OK, "{sent}");

    let frames = user_inputs(&app, &person_bearer(ALICE)).await;
    assert_eq!(
        frame_with(&frames, "from alice")["payload"]["sender"],
        person_sender(ALICE, &alice_scope),
        "{frames:#?}"
    );
    assert_eq!(
        frame_with(&frames, "from bob")["payload"]["sender"],
        person_sender(BOB, &bob_scope),
        "{frames:#?}"
    );

    assert_eq!(
        requester_rows(&runtime, "from alice").await,
        vec![Some(TurnRequester::direct(person(ALICE)))],
        "alice's turn names alice as its direct requester"
    );
    assert_eq!(
        requester_rows(&runtime, "from bob").await,
        vec![Some(TurnRequester::direct(person(BOB)))],
        "bob's turn names bob as its direct requester"
    );

    let _ = runtime.shutdown().await;
    drop(app);
    drop(runtime);
    let runtime = boot(state.path(), &mob).await;
    let app = runtime.build_reference_app_router(decisions(true));
    let frames = user_inputs(&app, &person_bearer(BOB)).await;
    assert_eq!(
        frame_with(&frames, "from alice")["payload"]["sender"],
        person_sender(ALICE, &alice_scope),
        "a restart keeps alice's sender: {frames:#?}"
    );
    assert_eq!(
        frame_with(&frames, "from bob")["payload"]["sender"],
        person_sender(BOB, &bob_scope),
        "a restart keeps bob's sender: {frames:#?}"
    );
    let _ = runtime.shutdown().await;
}

/// The origin kinds of Meerkat's internal emitters (scheduler, connector,
/// flow, policy, system) are refused on the HTTP send routes, REST and RPC,
/// before anything is recorded; an operator's send keeps its kind.
#[tokio::test(flavor = "multi_thread")]
async fn http_send_refuses_reserved_origin_kind() {
    let state = state_dir();
    let runtime = boot_with_member(state.path(), &mob_id("reserved-kinds")).await;
    let app = runtime.build_reference_app_router(decisions(true));
    let alice = person_bearer(ALICE);

    for kind in ["scheduler", "connector", "flow", "policy", "system"] {
        let (status, refused) = send(
            &app,
            &alice,
            &format!("as {kind}"),
            &format!("rest-{kind}"),
            Some(kind),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{kind}: {refused}");
        assert_eq!(
            refused["error"], "reserved_origin_kind",
            "{kind}: {refused}"
        );

        let answer = rpc(
            &app,
            &alice,
            "meta/console/send",
            json!({
                "identity": MEMBER,
                "content": format!("rpc as {kind}"),
                "origin": "console",
                "idempotency_key": format!("rpc-{kind}"),
                "origin_kind": kind,
            }),
        )
        .await;
        assert_eq!(answer["error"]["code"], json!(-32602), "{kind}: {answer}");
        assert!(answer["result"].is_null(), "{kind}: {answer}");
    }
    assert!(
        user_inputs(&app, &alice).await.is_empty(),
        "a refused send records nothing"
    );

    let (status, sent) = send(
        &app,
        &alice,
        "as an operator",
        "rest-operator",
        Some("operator"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{sent}");
    let frames = user_inputs(&app, &alice).await;
    assert_eq!(
        frame_with(&frames, "as an operator")["payload"]["origin_kind"],
        "operator"
    );
    let _ = runtime.shutdown().await;
}

/// An operator probe is the caller's own label for an automated check: the
/// HTTP send keeps it, the frame still names who sent it, and the person's
/// probe carries the person as its requester, as their operator send would.
#[tokio::test(flavor = "multi_thread")]
async fn http_send_keeps_operator_probe_with_its_sender() {
    let state = state_dir();
    let runtime = boot_with_member(state.path(), &mob_id("probe")).await;
    let app = runtime.build_reference_app_router(decisions(true));
    let alice = person_bearer(ALICE);
    let alice_scope = caller_scope(&app, &alice).await;

    let (status, sent) = send(
        &app,
        &alice,
        "probe reply",
        "probe-1",
        Some("operator_probe"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{sent}");
    let frames = user_inputs(&app, &alice).await;
    let frame = frame_with(&frames, "probe reply");
    assert_eq!(frame["payload"]["origin_kind"], "operator_probe", "{frame}");
    assert_eq!(
        frame["payload"]["sender"],
        person_sender(ALICE, &alice_scope),
        "{frame}"
    );
    assert_eq!(
        requester_rows(&runtime, "probe reply").await,
        vec![Some(TurnRequester::direct(person(ALICE)))],
        "the probe's turn names its person as its direct requester"
    );
    let _ = runtime.shutdown().await;
}

/// A service is no person: its send's frame names the service, and the run
/// it starts has no requester.
#[tokio::test(flavor = "multi_thread")]
async fn service_send_names_no_requester() {
    let state = state_dir();
    let runtime = boot_with_member(state.path(), &mob_id("service")).await;
    let app = runtime.build_reference_app_router(decisions(true));
    let robot = service_bearer();
    let robot_scope = caller_scope(&app, &robot).await;

    let (status, sent) = send(&app, &robot, "from the robot", "robot-1", None).await;
    assert_eq!(status, StatusCode::OK, "{sent}");
    let frames = user_inputs(&app, &robot).await;
    assert_eq!(
        frame_with(&frames, "from the robot")["payload"]["sender"],
        json!({
            "kind": "service",
            "assurance": "service_credential",
            "display_name": ROBOT,
            "caller_scope": robot_scope,
        }),
        "{frames:#?}"
    );
    assert_eq!(
        requester_rows(&runtime, "from the robot").await,
        vec![None],
        "a service's run has no requester"
    );
    let _ = runtime.shutdown().await;
}

/// On an Open console no send names a requester, a person's who volunteered
/// a verified token included: the frame's sender still names the person,
/// and the run the send starts has no requester.
#[tokio::test(flavor = "multi_thread")]
async fn open_console_person_send_names_no_requester() {
    let state = state_dir();
    let mut runtime = boot_with_member(state.path(), &mob_id("open")).await;
    // An open console verifies a volunteered token only with an access
    // controller; this one admits everyone to view and send.
    runtime.set_access_controller(
        AccessController::new(AccessControlConfig {
            enabled: true,
            admins: vec![ADMIN.to_string()],
            rules: vec![AccessRule {
                id: "everyone-views-and-sends".to_string(),
                actions: vec!["agent.view".to_string(), "agent.send".to_string()],
                ..AccessRule::default()
            }],
            ..AccessControlConfig::default()
        })
        .expect("access controller"),
    );
    let app = runtime.build_reference_app_router(decisions(false));
    let alice = person_bearer(ALICE);
    let alice_scope = caller_scope(&app, &alice).await;

    let (status, sent) = send(
        &app,
        &alice,
        "from alice on an open console",
        "open-1",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{sent}");
    let frames = user_inputs(&app, &alice).await;
    assert_eq!(
        frame_with(&frames, "from alice on an open console")["payload"]["sender"],
        person_sender(ALICE, &alice_scope),
        "the open console still names the verified person: {frames:#?}"
    );
    assert_eq!(
        requester_rows(&runtime, "from alice on an open console").await,
        vec![None],
        "a run started from an open console has no requester"
    );
    let _ = runtime.shutdown().await;
}

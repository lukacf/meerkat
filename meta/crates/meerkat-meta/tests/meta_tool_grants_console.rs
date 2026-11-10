#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! Scoped tool grants on the Console (`POST /console/rpc`): the access
//! action `agent.tool_grants.manage` gates grant and revoke on the identity
//! (a revoke on the identity its grant names), `agent.view` gates list and
//! audit, an `agent.*` rule covers the new action, a grant records the
//! signed-in principal as its issuer's caller, and every refusal is the
//! typed `tool_grant_request_refused` umbrella, as on the stdio gateway.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use meerkat_client::TestClient;
use meerkat_meta::identity_first::contracts::RosterProvider;
use meerkat_meta::identity_first::{
    AgentAddressability, AgentIdentity, DurableAgentSpec, RosterContext, RosterError,
};
use meerkat_meta::{
    ACCESS_ACTIONS, AccessControlConfig, AccessController, AuthPolicy, BigQueryNaming,
    ConsolePolicy, RuntimeDecisionInputs, RuntimeOpsPolicy, TrustedOidcRuntimeConfig,
    UnifiedRuntime, UnifiedRuntimeBuilder, build_runtime_decision_state,
};
use meerkat_mob::{MobDefinition, ProfileName};
use serde_json::{Value, json};
use tower::ServiceExt;

const ISSUER: &str = "https://trusted.meta.localhost";
const ADMIN: &str = "admin@example.test";
const VIEWER: &str = "viewer@example.test";
const MANAGER: &str = "manager@example.test";
const STAR: &str = "star@example.test";
const STRANGER: &str = "stranger@example.test";
/// Views both identities, manages the grants of `agent-a` only.
const SCOPED: &str = "scoped@example.test";
/// Views and manages `agent-a` only: `agent-b` is hidden from it.
const NARROW: &str = "narrow@example.test";

const IDENTITY: &str = "agent-a";
/// The roster's second identity.
const OTHER: &str = "agent-b";
const GATED_TOOL: &str = "datetime";
const TEN_MINUTES_MS: u64 = 600_000;
const MANAGE: &str = "agent.tool_grants.manage";
/// A grant id no grant holds.
const UNKNOWN_GRANT: &str = "00000000-0000-4000-8000-000000000000";

const ACCESS: &str = r#"
enabled = true
admins = ["admin@example.test"]

[[rules]]
id = "viewers"
subjects = ["viewer@example.test", "manager@example.test"]
actions = ["agent.view"]

[[rules]]
id = "grant-managers"
subjects = ["manager@example.test"]
actions = ["agent.tool_grants.manage"]

[[rules]]
id = "every-agent-verb"
subjects = ["star@example.test"]
actions = ["agent.*"]

[[rules]]
id = "scoped-viewer"
subjects = ["scoped@example.test"]
actions = ["agent.view"]
agents = ["agent-a", "agent-b"]

[[rules]]
id = "narrow-viewer"
subjects = ["narrow@example.test"]
actions = ["agent.view"]
agents = ["agent-a"]

[[rules]]
id = "one-identity-grant-managers"
subjects = ["scoped@example.test", "narrow@example.test"]
actions = ["agent.tool_grants.manage"]
agents = ["agent-a"]
"#;

static NEXT_MOB: AtomicU64 = AtomicU64::new(0);

fn definition() -> MobDefinition {
    MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "meta-tool-grants-console-{}-{}"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.worker.tools]
comms = true
builtins = true
grant_gated = {{ tools = ["{GATED_TOOL}"] }}
"#,
        std::process::id(),
        NEXT_MOB.fetch_add(1, Ordering::Relaxed)
    ))
    .expect("definition")
}

fn spec() -> DurableAgentSpec {
    spec_of(IDENTITY)
}

fn spec_of(identity: &str) -> DurableAgentSpec {
    DurableAgentSpec {
        identity: AgentIdentity::parse(identity).unwrap(),
        profile: ProfileName::from("worker"),
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

struct TwoIdentityRoster;

#[async_trait]
impl RosterProvider for TwoIdentityRoster {
    async fn roster(&self, _context: &RosterContext) -> Result<Vec<DurableAgentSpec>, RosterError> {
        Ok(vec![spec(), spec_of(OTHER)])
    }
}

fn decision_state() -> meerkat_meta::RuntimeDecisionState {
    build_runtime_decision_state(RuntimeDecisionInputs {
        bigquery: BigQueryNaming {
            dataset: "grants_dataset".to_string(),
            table: "grants_table".to_string(),
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
            email_allowlist: [ADMIN, VIEWER, MANAGER, STAR, STRANGER, SCOPED, NARROW]
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
            require_app_auth: true,
            ..ConsolePolicy::default()
        },
        ops: RuntimeOpsPolicy::default(),
        release_metadata_json: include_str!("../assets/release-targets.json").to_string(),
    })
    .expect("decision state builds")
}

/// The gateway's runtime on `state` with the access configuration `access`
/// (TOML), and its Console router.
async fn console(state: &Path, access: Option<&str>) -> (UnifiedRuntime, axum::Router) {
    let mut builder = UnifiedRuntimeBuilder::default()
        .definition(definition())
        .persistent_state(state)
        .continuity_from_state_dir(state)
        .await
        .expect("open the state-dir identity substrate")
        .roster_provider(Arc::new(TwoIdentityRoster))
        .identity_runtime_instance_id("meta-tool-grants-console")
        .default_llm_client(Arc::new(TestClient::default()));
    if let Some(access) = access {
        let config: AccessControlConfig = toml::from_str(access).expect("access TOML");
        builder = builder.access_controller(AccessController::new(config).expect("controller"));
    }
    let runtime = Box::pin(builder.build()).await.expect("runtime builds");
    let app = runtime.build_console_json_router(decision_state());
    (runtime, app)
}

/// One Console JSON-RPC call, signed in as `subject`.
async fn rpc(app: &axum::Router, subject: &str, method: &str, params: Value) -> Value {
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
    let request = Request::builder()
        .method("POST")
        .uri("/console/rpc")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(
            json!({"jsonrpc": "2.0", "id": "grants", "method": method, "params": params})
                .to_string(),
        ))
        .expect("rpc request");
    let response = tokio::time::timeout(Duration::from_mins(1), app.clone().oneshot(request))
        .await
        .expect("bounded rpc")
        .expect("rpc response");
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .expect("rpc body");
    serde_json::from_slice(&body).expect("rpc json")
}

fn grant_params() -> Value {
    grant_params_for(IDENTITY)
}

fn grant_params_for(identity: &str) -> Value {
    json!({
        "identity": identity,
        "target": {"kind": "tool", "name": GATED_TOOL},
        "duration_ms": TEN_MINUTES_MS,
    })
}

/// The id of a grant `subject` issued to `identity`, which must be issued.
async fn issued(app: &axum::Router, subject: &str, identity: &str) -> String {
    let granted = rpc(
        app,
        subject,
        "meta/tool_grants/grant",
        grant_params_for(identity),
    )
    .await;
    assert!(granted["error"].is_null(), "{granted:#?}");
    granted["result"]["grant"]["grant_id"]
        .as_str()
        .unwrap_or_else(|| panic!("a grant id: {granted:#?}"))
        .to_string()
}

/// The status of `identity`'s grant `grant_id`, as an admin lists it.
async fn status_of(app: &axum::Router, identity: &str, grant_id: &str) -> Value {
    let listed = rpc(
        app,
        ADMIN,
        "meta/tool_grants/list",
        json!({"identity": identity, "include_ended": true}),
    )
    .await;
    listed["result"]["grants"]
        .as_array()
        .and_then(|grants| {
            grants
                .iter()
                .find(|grant| grant["grant_id"] == json!(grant_id))
        })
        .map(|grant| grant["status"].clone())
        .unwrap_or_else(|| panic!("{identity} lists {grant_id}: {listed:#?}"))
}

fn assert_access_denied(response: &Value, action: &str) {
    assert_eq!(
        response["error"]["code"],
        json!(-32030),
        "refused access_denied: {response:#?}"
    );
    assert_eq!(
        response["error"]["data"]["action"],
        json!(action),
        "{response:#?}"
    );
}

fn refusal_kind(response: &Value) -> String {
    assert_eq!(response["error"]["code"], json!(-32602), "{response:#?}");
    assert_eq!(
        response["error"]["data"]["kind"],
        json!("tool_grant_request_refused"),
        "{response:#?}"
    );
    response["error"]["data"]["tool_grant_request_refusal"]["kind"]
        .as_str()
        .unwrap_or_else(|| panic!("the umbrella names its kind: {response:#?}"))
        .to_string()
}

/// `agent.tool_grants.manage` gates grant and revoke on the identity, and
/// `agent.view` list and audit; an `agent.*` allow covers the new action; a
/// grant records the signed-in principal as its caller.
#[tokio::test(flavor = "multi_thread")]
async fn grant_and_revoke_need_the_manage_action_and_reads_need_agent_view() {
    assert!(
        ACCESS_ACTIONS.contains(&MANAGE),
        "agent.tool_grants.manage is declared"
    );
    let temp = tempfile::tempdir().unwrap();
    let (runtime, app) = console(temp.path(), Some(ACCESS)).await;

    let denied = rpc(&app, VIEWER, "meta/tool_grants/grant", grant_params()).await;
    assert_access_denied(&denied, MANAGE);

    let granted = rpc(&app, MANAGER, "meta/tool_grants/grant", grant_params()).await;
    assert!(granted["error"].is_null(), "{granted:#?}");
    assert_eq!(
        granted["result"]["grant"]["issuer"],
        json!({"kind": "operator", "caller": MANAGER}),
        "the console records the signed-in principal: {granted:#?}"
    );
    let grant_id = granted["result"]["grant"]["grant_id"].clone();

    let by_star = rpc(&app, STAR, "meta/tool_grants/grant", grant_params()).await;
    assert!(
        by_star["error"].is_null(),
        "an agent.* rule covers the new action: {by_star:#?}"
    );

    for method in ["meta/tool_grants/list", "meta/tool_grants/audit"] {
        let read = rpc(&app, VIEWER, method, json!({"identity": IDENTITY})).await;
        assert!(read["error"].is_null(), "{method}: {read:#?}");
        let stranger = rpc(&app, STRANGER, method, json!({"identity": IDENTITY})).await;
        assert_access_denied(&stranger, "agent.view");
    }

    let revoke_denied = rpc(
        &app,
        VIEWER,
        "meta/tool_grants/revoke",
        json!({"grant_id": grant_id}),
    )
    .await;
    assert_access_denied(&revoke_denied, MANAGE);
    let revoked = rpc(
        &app,
        MANAGER,
        "meta/tool_grants/revoke",
        json!({"grant_id": grant_id}),
    )
    .await;
    assert!(revoked["error"].is_null(), "{revoked:#?}");
    assert_eq!(revoked["result"]["changed"], json!(true), "{revoked:#?}");

    // The audit records who revoked it.
    let audit = rpc(
        &app,
        MANAGER,
        "meta/tool_grants/audit",
        json!({"identity": IDENTITY}),
    )
    .await;
    let revoked_row = audit["result"]["rows"]
        .as_array()
        .and_then(|rows| {
            rows.iter().find(|row| {
                row["event"]["kind"] == json!("revoked") && row["event"]["grant_id"] == grant_id
            })
        })
        .unwrap_or_else(|| panic!("the audit has the revoked row: {audit:#?}"));
    assert_eq!(
        revoked_row["event"]["actor"],
        json!(MANAGER),
        "the revoked row names the signed-in principal: {audit:#?}"
    );
    let _ = runtime.shutdown().await;
}

/// The Console answers a grant's `applies` and an identity's `gate` as the
/// stdio gateway does. Nothing has sent this Console's member a turn, so no
/// build of it is live here: a grant applies `at_next_build`, and the list
/// answers the gate the member's next build resolves, with no running build.
#[tokio::test(flavor = "multi_thread")]
async fn the_console_answers_whether_a_grant_applies_and_the_members_gate() {
    let temp = tempfile::tempdir().unwrap();
    let (runtime, app) = console(temp.path(), Some(ACCESS)).await;

    let granted = rpc(&app, MANAGER, "meta/tool_grants/grant", grant_params()).await;
    assert!(granted["error"].is_null(), "{granted:#?}");
    assert_eq!(
        granted["result"]["applies"],
        json!("at_next_build"),
        "{granted:#?}"
    );

    let listed = rpc(
        &app,
        VIEWER,
        "meta/tool_grants/list",
        json!({"identity": IDENTITY}),
    )
    .await;
    assert!(listed["error"].is_null(), "{listed:#?}");
    let gate = &listed["result"]["gate"];
    assert!(
        gate.is_object(),
        "the list answers the member's gate: {listed:#?}"
    );
    assert_eq!(
        gate["next_build"]["gated"]["tools"],
        json!([GATED_TOOL]),
        "{listed:#?}"
    );
    assert_eq!(gate["running"], Value::Null, "{listed:#?}");
    let _ = runtime.shutdown().await;
}

/// A revoke is judged on its grant's identity under resource-scoped rules:
/// a principal that may manage one identity's grants revokes them, and its
/// revoke of another identity's grant, which it may view but not manage, is
/// `access_denied` naming that identity, with the grant left active.
#[tokio::test(flavor = "multi_thread")]
async fn a_console_revoke_is_judged_on_the_grants_identity_under_resource_scoped_rules() {
    let temp = tempfile::tempdir().unwrap();
    let (runtime, app) = console(temp.path(), Some(ACCESS)).await;
    let own = issued(&app, ADMIN, IDENTITY).await;
    let other = issued(&app, ADMIN, OTHER).await;

    let denied = rpc(
        &app,
        SCOPED,
        "meta/tool_grants/revoke",
        json!({"grant_id": other}),
    )
    .await;
    assert_access_denied(&denied, MANAGE);
    assert_eq!(
        denied["error"]["data"]["resource"],
        json!(OTHER),
        "the refusal names the identity the caller may view: {denied:#?}"
    );
    assert_eq!(
        status_of(&app, OTHER, &other).await,
        json!("active"),
        "the refused revoke leaves the grant active"
    );

    let revoked = rpc(
        &app,
        SCOPED,
        "meta/tool_grants/revoke",
        json!({"grant_id": own}),
    )
    .await;
    assert!(
        revoked["error"].is_null(),
        "a principal revokes the grants of the identity it manages: {revoked:#?}"
    );
    assert_eq!(revoked["result"]["changed"], json!(true), "{revoked:#?}");
    let _ = runtime.shutdown().await;
}

/// A revoke tells a caller that may not view the grant's identity nothing:
/// it is answered exactly as a grant id no grant holds is (`not_found`, the
/// same message), and the grant stays active.
#[tokio::test(flavor = "multi_thread")]
async fn a_console_revoke_answers_a_caller_that_cannot_view_the_identity_as_for_an_unknown_grant() {
    let temp = tempfile::tempdir().unwrap();
    let (runtime, app) = console(temp.path(), Some(ACCESS)).await;
    let other = issued(&app, ADMIN, OTHER).await;

    for caller in [NARROW, STRANGER] {
        let hidden = rpc(
            &app,
            caller,
            "meta/tool_grants/revoke",
            json!({"grant_id": other}),
        )
        .await;
        let unknown = rpc(
            &app,
            caller,
            "meta/tool_grants/revoke",
            json!({"grant_id": UNKNOWN_GRANT}),
        )
        .await;
        assert_eq!(refusal_kind(&hidden), "not_found", "{caller}");
        assert_eq!(
            hidden["error"].to_string().replace(&other, "GRANT"),
            unknown["error"].to_string().replace(UNKNOWN_GRANT, "GRANT"),
            "{caller}: the answer does not tell a hidden grant from an unknown one"
        );
    }
    assert_eq!(
        status_of(&app, OTHER, &other).await,
        json!("active"),
        "the hidden grant stays active"
    );
    let _ = runtime.shutdown().await;
}

/// The Console answers the grant methods' refusals in the umbrella shape the
/// stdio gateway answers.
#[tokio::test(flavor = "multi_thread")]
async fn console_tool_grant_refusals_are_typed() {
    let temp = tempfile::tempdir().unwrap();
    let (runtime, app) = console(temp.path(), None).await;

    let unknown = rpc(
        &app,
        ADMIN,
        "meta/tool_grants/grant",
        json!({"identity": "nobody", "target": {"kind": "tool", "name": GATED_TOOL}, "duration_ms": TEN_MINUTES_MS}),
    )
    .await;
    assert_eq!(refusal_kind(&unknown), "subject_unknown");

    let not_gated = rpc(
        &app,
        ADMIN,
        "meta/tool_grants/grant",
        json!({"identity": IDENTITY, "target": {"kind": "tool", "name": "view_image"}, "duration_ms": TEN_MINUTES_MS}),
    )
    .await;
    assert_eq!(refusal_kind(&not_gated), "target_not_gated");

    let empty = rpc(
        &app,
        ADMIN,
        "meta/tool_grants/grant",
        json!({"identity": IDENTITY, "target": {"kind": "tool", "name": ""}, "duration_ms": TEN_MINUTES_MS}),
    )
    .await;
    assert_eq!(refusal_kind(&empty), "empty_target");

    let window = rpc(
        &app,
        ADMIN,
        "meta/tool_grants/grant",
        json!({"identity": IDENTITY, "target": {"kind": "tool", "name": GATED_TOOL}}),
    )
    .await;
    assert_eq!(refusal_kind(&window), "invalid_window");

    let decision_ref = rpc(
        &app,
        ADMIN,
        "meta/tool_grants/grant",
        json!({
            "identity": IDENTITY,
            "target": {"kind": "tool", "name": GATED_TOOL},
            "duration_ms": TEN_MINUTES_MS,
            "decision_ref": "",
        }),
    )
    .await;
    assert_eq!(refusal_kind(&decision_ref), "invalid_decision_ref");

    let not_found = rpc(
        &app,
        ADMIN,
        "meta/tool_grants/revoke",
        json!({"grant_id": "00000000-0000-4000-8000-000000000000"}),
    )
    .await;
    assert_eq!(refusal_kind(&not_found), "not_found");
    let _ = runtime.shutdown().await;
}

/// The Console serves the grant methods whole and advertises them, and
/// serves neither the methods later parts add (`meta/tool_grants/attach`
/// and `request`) nor any realm MCP method (host administration, served on
/// the stdio gateway only): each of those is method not found here and
/// absent from the capabilities.
#[tokio::test(flavor = "multi_thread")]
async fn the_console_serves_the_grant_methods_whole_and_no_realm_mcp_method() {
    let temp = tempfile::tempdir().unwrap();
    let (runtime, app) = console(temp.path(), None).await;
    let capabilities = rpc(&app, ADMIN, "meta/capabilities", json!({})).await;
    let methods = capabilities["result"]["methods"]
        .as_array()
        .unwrap_or_else(|| panic!("methods: {capabilities:#?}"))
        .clone();
    for method in [
        "meta/tool_grants/grant",
        "meta/tool_grants/revoke",
        "meta/tool_grants/list",
        "meta/tool_grants/audit",
    ] {
        assert!(
            methods.contains(&json!(method)),
            "{method} is advertised: {methods:?}"
        );
    }
    for method in [
        "meta/tool_grants/attach",
        "meta/tool_grants/request",
        "meta/realm_mcp/servers",
        "meta/realm_mcp/install",
        "meta/realm_mcp/set_secret",
        "meta/realm_mcp/remove_secret",
        "meta/realm_mcp/remove",
    ] {
        assert!(
            !methods.contains(&json!(method)),
            "{method} is not advertised: {methods:?}"
        );
        let answer = rpc(&app, ADMIN, method, json!({})).await;
        assert_eq!(
            answer["error"]["code"],
            json!(-32601),
            "{method} is not served: {answer:#?}"
        );
    }
    let _ = runtime.shutdown().await;
}

//! `/console/experience` after a real identity reset, on the composition a
//! gateway serves: the unified runtime with its identity runtime, and one
//! reference router carrying the shared console aggregator projection.
//!
//! A reset moves the identity to a successor session on the same roster row.
//! The FIRST experience read after it must already describe the successor,
//! in the sidebar row and in the identity_status row, and agree with
//! `/console/identities`. Hosts read the experience once after a reset and do
//! not retry, so a read that answers from a projection collected before the
//! reset is wrong, not merely late.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::Request;
use meerkat_client::{LlmClient, LlmError, LlmEvent, LlmRequest};
use meerkat_core::{Message, Provider, StopReason};
use meerkat_meta::identity_first::{
    AgentAddressability, AgentIdentity, DurableAgentSpec, LocalContinuityStore, LocalLeaseProvider,
    MutableRosterProvider,
};
use meerkat_meta::{
    AuthPolicy, AuthProvider, BigQueryNaming, ConsolePolicy, RuntimeDecisionInputs,
    RuntimeOpsPolicy, TrustedOidcRuntimeConfig, UnifiedRuntime, build_runtime_decision_state,
};
use meerkat_mob::MobDefinition;
use serde_json::{Value, json};
use tower::ServiceExt;

#[path = "support/llm_usage.rs"]
mod llm_usage;

const IDENTITY: &str = "voice-keeper";
/// A second identity, so the roster is never empty when the keeper leaves it.
const OTHER: &str = "other-keeper";

fn open_console_decisions() -> meerkat_meta::RuntimeDecisionState {
    build_runtime_decision_state(RuntimeDecisionInputs {
        bigquery: BigQueryNaming {
            dataset: "console_experience_identity_reset".to_string(),
            table: "events".to_string(),
        },
        trusted_meta_toml: "modules = []\n".to_string(),
        auth: AuthPolicy {
            default_provider: AuthProvider::GoogleOAuth,
            email_allowlist: vec![],
        },
        trusted_oidc: TrustedOidcRuntimeConfig {
            discovery_json:
                r#"{"issuer":"https://trusted.meta.local","jwks_uri":"https://trusted.meta.local/.well-known/jwks.json"}"#
                    .to_string(),
            jwks_json: r#"{"keys":[{"kid":"kid-current","kty":"oct","alg":"HS256","k":"cGhhc2U3LXRydXN0ZWQtY3VycmVudC1zZWNyZXQ"}]}"#
                .to_string(),
            audience: "meerkat-console".to_string(),
            require_verified_email: false,
        },
        console: ConsolePolicy {
            require_app_auth: false,
            ..ConsolePolicy::default()
        },
        ops: RuntimeOpsPolicy::default(),
        release_metadata_json: include_str!("../assets/release-targets.json").to_string(),
    })
    .expect("decision state builds")
}

/// The members never take a turn here; the client only has to exist.
struct QuietClient;

#[async_trait::async_trait]
impl LlmClient for QuietClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(&'a self, request: &'a LlmRequest) -> meerkat_client::types::LlmStream<'a> {
        let [usage, done] =
            llm_usage::usage_then_done(request, Provider::OpenAI, StopReason::EndTurn);
        Box::pin(futures::stream::iter(vec![
            Ok(LlmEvent::TextDelta {
                delta: "ok".to_string(),
                meta: None,
            }),
            Ok(usage),
            Ok(done),
        ]))
    }

    fn provider(&self) -> Provider {
        Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

fn keeper_spec(labels: BTreeMap<String, String>) -> DurableAgentSpec {
    DurableAgentSpec {
        identity: AgentIdentity::parse(IDENTITY).unwrap(),
        profile: "worker".into(),
        addressability: AgentAddressability::Addressable,
        display_name: None,
        labels,
        context: None,
        additional_instructions: Vec::new(),
        initial_message: None,
        runtime_mode_override: Some(meerkat_mob::MobRuntimeMode::AutonomousHost),
        backend: None,
        binding: None,
        placement: None,
    }
}

/// The unified runtime with its identity runtime and the reference router a
/// gateway serves. The router is built once and shared by every request, so
/// its console read model persists across requests as it does in a gateway.
struct ResetFixture {
    runtime: UnifiedRuntime,
    roster: Arc<MutableRosterProvider>,
    app: axum::Router,
    _scratch: tempfile::TempDir,
}

impl ResetFixture {
    async fn new() -> Self {
        let scratch = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let definition = MobDefinition::from_toml(&format!(
            r#"
[mob]
id = "console-experience-identity-reset-{}"
[profiles.worker]
model = "gpt-5.5"
runtime_mode = "autonomous_host"
external_addressable = true
[profiles.worker.tools]
comms = true
"#,
            uuid::Uuid::new_v4()
        ))
        .unwrap();
        let roster = Arc::new(MutableRosterProvider::new(vec![
            keeper_spec(BTreeMap::new()),
            DurableAgentSpec {
                identity: AgentIdentity::parse(OTHER).unwrap(),
                ..keeper_spec(BTreeMap::new())
            },
        ]));
        let runtime = UnifiedRuntime::builder()
            .definition(definition)
            .continuity_store(Arc::new(LocalContinuityStore::in_memory().unwrap()))
            .lease_provider(Arc::new(LocalLeaseProvider::new()))
            .roster_provider(roster.clone())
            .scratch_dir(scratch.path())
            .ephemeral_runtime_store(true)
            .identity_runtime_instance_id("console-experience-identity-reset")
            .default_llm_client(Arc::new(QuietClient))
            .build()
            .await
            .unwrap();
        let app = runtime.build_reference_app_router(open_console_decisions());
        Self {
            runtime,
            roster,
            app,
            _scratch: scratch,
        }
    }

    async fn get(&self, path: &str) -> Value {
        let response = self
            .app
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024 * 1024)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        assert!(status.is_success(), "GET {path}: {status} {value}");
        value
    }

    async fn rpc(&self, method: &str, params: Value) -> Value {
        let response = self
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/console/rpc")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024 * 1024)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    async fn bound_session(&self) -> String {
        self.runtime
            .identity_runtime()
            .unwrap()
            .status(&AgentIdentity::parse(IDENTITY).unwrap())
            .await
            .unwrap()
            .session_id
            .expect("the identity is bound to a session")
            .to_string()
    }

    async fn stop(self) {
        self.runtime.mob_handle().stop().await.unwrap();
    }
}

/// The keeper's sidebar row and identity_status row of one experience read.
fn keeper_rows(experience: &Value) -> (Value, Value) {
    let sidebar = experience["agent_sidebar"]["live_snapshot"]["agents"]
        .as_array()
        .expect("sidebar agents")
        .iter()
        .filter(|agent| agent["identity"] == IDENTITY)
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        sidebar.len(),
        1,
        "one sidebar row for the keeper: {sidebar:?}"
    );
    let status = experience["identity_status"]["rows"]
        .as_array()
        .expect("identity_status rows")
        .iter()
        .filter(|row| row["identity"] == IDENTITY)
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        status.len(),
        1,
        "one identity_status row for the keeper: {status:?}"
    );
    (sidebar[0].clone(), status[0].clone())
}

fn identities_session(identities: &Value) -> Value {
    let rows = identities["identities"]
        .as_array()
        .expect("identities")
        .iter()
        .filter(|record| record["identity"] == IDENTITY)
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        rows.len(),
        1,
        "one identity record for the keeper: {rows:?}"
    );
    rows[0]["session_id"].clone()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_first_experience_read_after_a_console_reset_names_the_successor_session() {
    let fixture = ResetFixture::new().await;
    let predecessor = fixture.bound_session().await;

    // A console that has been open before the reset: its read model already
    // describes the predecessor.
    let (sidebar, status) = keeper_rows(&fixture.get("/console/experience").await);
    assert_eq!(sidebar["session_id"], predecessor);
    assert_eq!(status["session_id"], predecessor);

    // A real identity reset through the console. The identity stays in the
    // roster provider and keeps its one roster row.
    let reset = fixture
        .rpc("meta/reset", json!({ "identity": IDENTITY }))
        .await;
    let successor = reset["result"]["session_id"]
        .as_str()
        .unwrap_or_else(|| panic!("reset succeeds: {reset}"))
        .to_string();
    assert_ne!(successor, predecessor);
    assert_eq!(reset["result"]["agent_runtime_id"], "rt:voice-keeper:1");
    assert_eq!(fixture.bound_session().await, successor);
    assert_eq!(
        identities_session(&fixture.get("/console/identities").await),
        successor,
        "/console/identities names the successor"
    );

    // The predecessor's runtime id is refused as a control target with the
    // typed stale-binding refusal, naming the successor binding, and refusing
    // it changes nothing. (A control call never reads the console read model,
    // so this does not refresh it.)
    let stale = fixture
        .rpc("meta/reset", json!({ "identity": "rt:voice-keeper:0" }))
        .await;
    assert!(
        stale.get("result").is_none_or(Value::is_null),
        "a reset naming the predecessor's runtime id is refused: {stale}"
    );
    assert_eq!(
        stale["error"],
        json!({
            "code": -32000,
            "message": "reset failed: identity runtime binding for voice-keeper points at \
                        rt:voice-keeper:1, but requested live member is rt:voice-keeper:0",
            "data": {
                "kind": "stale_identity_runtime_binding",
                "identity": "voice-keeper",
                "registered_runtime_member_id": "rt:voice-keeper:1",
                "live_runtime_member_id": "rt:voice-keeper:0",
                "registered_session_id": successor,
                "live_session_id": null,
            },
        }),
        "{stale}"
    );
    assert_eq!(fixture.bound_session().await, successor);

    // The FIRST experience read after the reset: no retry and no wait.
    let (sidebar, status) = keeper_rows(&fixture.get("/console/experience").await);
    assert_eq!(
        sidebar["session_id"], successor,
        "the sidebar row names the predecessor session after the reset: {sidebar}"
    );
    assert_eq!(
        status["session_id"], successor,
        "the identity_status row names the predecessor session after the reset: {status}"
    );
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_first_experience_read_after_a_reset_shows_the_reprofiled_successor_row() {
    let fixture = ResetFixture::new().await;

    let (sidebar, _) = keeper_rows(&fixture.get("/console/experience").await);
    assert_eq!(sidebar["group"], "worker", "no group label yet: {sidebar}");

    // The host reprofiles the keeper; a reset rebuilds it under the current
    // roster spec, so the successor row carries the new label.
    fixture.roster.upsert(keeper_spec(BTreeMap::from([(
        "group".to_string(),
        "keepers".to_string(),
    )])));
    let reset = fixture
        .rpc("meta/reset", json!({ "identity": IDENTITY }))
        .await;
    assert!(reset["result"].is_object(), "reset succeeds: {reset}");
    let status = fixture
        .runtime
        .identity_runtime()
        .unwrap()
        .status(&AgentIdentity::parse(IDENTITY).unwrap())
        .await
        .unwrap();
    assert_eq!(
        status.labels.get("group").map(String::as_str),
        Some("keepers"),
        "the reset adopted the current roster spec"
    );

    // The FIRST experience read after the reset shows the successor's row,
    // not the row collected before the reset.
    let (sidebar, status_row) = keeper_rows(&fixture.get("/console/experience").await);
    assert_eq!(
        sidebar["group"], "keepers",
        "the sidebar row keeps the predecessor's labels after the reset: {sidebar}"
    );
    assert_eq!(
        status_row["labels"]["group"], "keepers",
        "the identity_status row keeps the predecessor's labels after the reset: {status_row}"
    );
    assert_eq!(sidebar["session_id"], fixture.bound_session().await);
    fixture.stop().await;
}

/// The keeper's rows of one experience read, if any.
fn keeper_rows_if_any(experience: &Value) -> (Vec<Value>, Vec<Value>) {
    let sidebar = experience["agent_sidebar"]["live_snapshot"]["agents"]
        .as_array()
        .expect("sidebar agents")
        .iter()
        .filter(|agent| agent["identity"] == IDENTITY)
        .cloned()
        .collect();
    let status = experience["identity_status"]["rows"]
        .as_array()
        .expect("identity_status rows")
        .iter()
        .filter(|row| row["identity"] == IDENTITY)
        .cloned()
        .collect();
    (sidebar, status)
}

/// A delete starts the identity over: a live identity moves to a fresh
/// session at once and stays live and registered, and Core keeps the old
/// session archived (CHANGELOG, Unreleased: "`delete_identity` starts the
/// identity over and keeps its old session in Core [...] A live identity
/// moves to a fresh session at once and stays live"). So the FIRST
/// experience read after the delete shows no row for the deleted session:
/// the keeper's rows name the fresh one, as `/console/identities` does.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_first_experience_read_after_a_delete_shows_no_row_for_the_deleted_session() {
    let fixture = ResetFixture::new().await;
    let deleted_session = fixture.bound_session().await;
    let (sidebar, _) = keeper_rows(&fixture.get("/console/experience").await);
    assert_eq!(sidebar["state"], "active");
    assert_eq!(sidebar["session_id"], deleted_session);

    let deleted = fixture
        .rpc("meta/delete_identity", json!({ "identity": IDENTITY }))
        .await;
    assert!(deleted["result"].is_object(), "delete succeeds: {deleted}");
    let fresh = fixture.bound_session().await;
    assert_ne!(
        fresh, deleted_session,
        "the identity runtime keeps the identity, on a fresh session"
    );
    assert_eq!(
        identities_session(&fixture.get("/console/identities").await),
        fresh,
        "/console/identities names the fresh session"
    );

    // The FIRST experience read after the delete: no retry and no wait.
    let (sidebar, status) = keeper_rows(&fixture.get("/console/experience").await);
    assert_eq!(
        sidebar["session_id"], fresh,
        "the sidebar row names the fresh session, not the deleted one: {sidebar}"
    );
    assert_eq!(
        status["session_id"], fresh,
        "the identity_status row names the fresh session, not the deleted one: {status}"
    );
    assert_eq!(
        sidebar["state"], "active",
        "the identity stays live: {sidebar}"
    );
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_first_experience_read_after_a_retire_shows_no_active_row_for_the_retired_identity() {
    let fixture = ResetFixture::new().await;
    let bound = fixture.bound_session().await;
    let (sidebar, _) = keeper_rows(&fixture.get("/console/experience").await);
    assert_eq!(sidebar["state"], "active");

    let retired = fixture
        .rpc("meta/retire", json!({ "identity": IDENTITY }))
        .await;
    assert!(retired["result"].is_object(), "retire succeeds: {retired}");
    // The owner keeps the retired identity with its binding. A retired
    // identity is dormant: the identity runtime reads Core's member history
    // and has no retiring state (CHANGELOG, Unreleased: "A registered
    // identity's `state` is `active`, `dormant` or `broken`; `retiring`,
    // `suspended` and `uninitialized` are gone"), so /console/identities
    // lists it as dormant.
    let identities = fixture.get("/console/identities").await;
    let record = identities["identities"]
        .as_array()
        .expect("identities")
        .iter()
        .find(|record| record["identity"] == IDENTITY)
        .cloned()
        .unwrap_or_else(|| panic!("the retired identity stays listed: {identities}"));
    assert_eq!(record["health"], "dormant", "{record}");
    assert_eq!(record["session_id"], bound, "{record}");

    // The FIRST experience read after the retire: no retry and no wait.
    let (sidebar, status) = keeper_rows_if_any(&fixture.get("/console/experience").await);
    assert!(
        sidebar
            .iter()
            .chain(status.iter())
            .all(|row| row["state"] != "active"),
        "the retired identity is still listed as active: sidebar {sidebar:?}, identity_status {status:?}"
    );
    for row in sidebar.iter().chain(status.iter()) {
        assert_eq!(row["session_id"], bound, "the owner's binding: {row}");
    }
    fixture.stop().await;
}

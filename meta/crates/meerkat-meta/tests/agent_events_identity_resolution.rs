//! `/agents/{id}/events` supports both legacy alias-shaped roster rows and
//! real identity-first members with durable core roster identities.
//! Identity-first subscriptions resolve the authorized core identity while
//! retaining the exact runtime alias as their generation fence. These tests
//! drive native tool/agent events through the actual HTTP endpoint, including
//! reset invalidation. Legacy direct/label resolution and unknown-id 404s stay
//! unchanged.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures::StreamExt;
use meerkat_client::TestClient;
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
use meerkat_mob::{MobDefinition, SpawnMemberSpec};
use tower::ServiceExt;

#[path = "support/llm_usage.rs"]
mod llm_usage;

fn open_console_decisions() -> meerkat_meta::RuntimeDecisionState {
    build_runtime_decision_state(RuntimeDecisionInputs {
        bigquery: BigQueryNaming {
            dataset: "agent_events_identity_test".to_string(),
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_events_route_resolves_durable_identities_and_aliases() {
    let definition = MobDefinition::from_toml(
        r#"
[mob]
id = "agent-events-identity-test"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true

[profiles.worker.tools]
comms = true
"#,
    )
    .expect("definition parses");
    let runtime = UnifiedRuntime::builder()
        .definition(definition)
        .default_llm_client(Arc::new(TestClient::default()))
        .build()
        .await
        .expect("runtime builds");

    // Identity-first-shaped member: runtime-alias id + durable identity label.
    let mut labels = BTreeMap::new();
    labels.insert("agent_identity".to_string(), "lead".to_string());
    runtime
        .mob_handle()
        .spawn_spec(
            SpawnMemberSpec::from_wire(
                "worker".to_string(),
                meerkat_meta::member_comms_id::mob_member_id_str("rt:lead:0").into_owned(),
                Some("You are the lead.".into()),
                None,
                None,
            )
            .with_labels(labels),
        )
        .await
        .expect("member spawns");

    let app = runtime.build_reference_app_router(open_console_decisions());

    for (path, expected, label) in [
        (
            "/agents/lead/events",
            StatusCode::OK,
            "durable identity must resolve via the roster label",
        ),
        (
            "/agents/rt:lead:0/events",
            StatusCode::OK,
            "runtime alias must encode directly",
        ),
        (
            "/agents/nobody-here/events",
            StatusCode::NOT_FOUND,
            "unknown ids must 404, never stream an error body",
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(path)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), expected, "{label}: GET {path}");
    }

    let _ = runtime.mob_handle().stop().await;
}

struct NativeToolClient;

#[async_trait::async_trait]
impl LlmClient for NativeToolClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(&'a self, request: &'a LlmRequest) -> meerkat_client::types::LlmStream<'a> {
        let after_tool = matches!(request.messages.last(), Some(Message::ToolResults { .. }));
        let [usage, done] = llm_usage::usage_then_done(
            request,
            Provider::OpenAI,
            if after_tool {
                StopReason::EndTurn
            } else {
                StopReason::ToolUse
            },
        );
        let event = if after_tool {
            LlmEvent::TextDelta {
                delta: "Native tool observation completed.".to_string(),
                meta: None,
            }
        } else {
            LlmEvent::ToolCallComplete {
                id: "native-peers".to_string(),
                name: "peers".to_string(),
                args: serde_json::json!({}),
                meta: None,
            }
        };
        Box::pin(futures::stream::iter(vec![Ok(event), Ok(usage), Ok(done)]))
    }

    fn provider(&self) -> Provider {
        Provider::OpenAI
    }
    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

struct IdentityFixture {
    runtime: UnifiedRuntime,
    identity: AgentIdentity,
    alias: String,
    _scratch: tempfile::TempDir,
}

impl IdentityFixture {
    async fn new() -> Self {
        let scratch = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let identity = AgentIdentity::parse("voice-keeper").unwrap();
        let definition = MobDefinition::from_toml(&format!(
            r#"
[mob]
id = "native-identity-events-{}"
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
        let roster = Arc::new(MutableRosterProvider::new(vec![DurableAgentSpec {
            identity: identity.clone(),
            profile: "worker".into(),
            addressability: AgentAddressability::Addressable,
            display_name: None,
            labels: BTreeMap::new(),
            context: None,
            additional_instructions: Vec::new(),
            initial_message: None,
            runtime_mode_override: Some(meerkat_mob::MobRuntimeMode::AutonomousHost),
            backend: None,
            binding: None,
            placement: None,
        }]));
        let runtime = UnifiedRuntime::builder()
            .definition(definition)
            .continuity_store(Arc::new(LocalContinuityStore::in_memory().unwrap()))
            .lease_provider(Arc::new(LocalLeaseProvider::new()))
            .roster_provider(roster)
            .scratch_dir(scratch.path())
            .ephemeral_runtime_store(true)
            .identity_runtime_instance_id("native-events-test")
            .default_llm_client(Arc::new(NativeToolClient))
            .build()
            .await
            .unwrap();
        let alias = runtime
            .identity_runtime()
            .unwrap()
            .status(&identity)
            .await
            .unwrap()
            .agent_runtime_id
            .unwrap()
            .to_string();
        assert_eq!(alias, "rt:voice-keeper:0");
        assert!(
            runtime
                .mob_handle()
                .get_member(&"voice-keeper".into())
                .await
                .unwrap()
                .is_some(),
            "the real identity plane must own a durable core roster row, not an alias-shaped fixture"
        );
        Self {
            runtime,
            identity,
            alias,
            _scratch: scratch,
        }
    }

    fn app(&self) -> axum::Router {
        self.runtime
            .build_reference_app_router(open_console_decisions())
    }

    async fn response(&self, alias: &str) -> axum::response::Response {
        self.app()
            .oneshot(
                Request::builder()
                    .uri(format!("/agents/{alias}/events"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn run_native_tool_turn(&self) {
        let handle = self.runtime.mob_handle();
        let member = handle
            .get_member(&self.identity.as_str().into())
            .await
            .unwrap()
            .unwrap();
        let interaction = uuid::Uuid::new_v4();
        let turn = handle
            .start_host_human_input_bounded(
                member.agent_runtime_id,
                member.fence_token,
                meerkat_mob::WorkSpec::new(
                    "Inspect peers then answer.",
                    meerkat_mob::WorkOrigin::External,
                )
                .with_interaction_id(meerkat_core::interaction::InteractionId(interaction)),
                meerkat_core::types::HandlingMode::Queue,
                meerkat_mob::MobDeliveryIdentity::new(
                    interaction.to_string(),
                    interaction.to_string(),
                )
                .unwrap(),
                std::time::Instant::now() + Duration::from_secs(15),
            )
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(15), turn.wait())
            .await
            .unwrap()
            .unwrap();
    }

    async fn stop(self) {
        self.runtime.mob_handle().stop().await.unwrap();
    }
}

async fn assert_native_tool_and_text(response: axum::response::Response) {
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "native SSE must open successfully"
    );
    let mut stream = response.into_body().into_data_stream();
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut text = String::new();
        let mut tool_seen = false;
        let mut text_seen = false;
        while !(tool_seen && text_seen) {
            let bytes = stream
                .next()
                .await
                .expect("current generation stream must remain open")
                .unwrap();
            text.push_str(std::str::from_utf8(&bytes).unwrap());
            while let Some(end) = text.find("\n\n") {
                let frame = text[..end].to_string();
                text.drain(..end + 2);
                for line in frame.lines().filter_map(|line| line.strip_prefix("data:")) {
                    let event: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
                    if event["type"] == "tool_execution_completed" {
                        assert_eq!(event["name"], "peers");
                        assert_eq!(event["tool_call_id"], "native-peers");
                        assert_eq!(event["is_error"], false);
                        tool_seen = true;
                    }
                    text_seen |= event["type"] == "text_delta";
                }
            }
        }
    })
    .await
    .expect("native tool and agent payloads must traverse the HTTP endpoint");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn identity_first_logical_agent_event_subscription_delivers_native_tool_and_agent_events() {
    let fixture = IdentityFixture::new().await;
    let response = fixture.response(fixture.identity.as_str()).await;
    fixture.run_native_tool_turn().await;
    assert_native_tool_and_text(response).await;
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn identity_first_runtime_alias_event_subscription_delivers_native_tool_and_agent_events() {
    let fixture = IdentityFixture::new().await;
    let response = fixture.response(&fixture.alias).await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "current runtime alias must resolve the authorized core identity"
    );
    fixture.run_native_tool_turn().await;
    assert_native_tool_and_text(response).await;
    fixture.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn identity_first_reset_closes_old_http_streams_and_rejects_stale_alias() {
    let fixture = IdentityFixture::new().await;
    let logical = fixture.response(fixture.identity.as_str()).await;
    let alias = fixture.response(&fixture.alias).await;
    assert_eq!(logical.status(), StatusCode::OK);
    assert_eq!(alias.status(), StatusCode::OK);
    let successor = fixture
        .runtime
        .identity_runtime()
        .unwrap()
        .reset(&fixture.identity)
        .await
        .unwrap();
    let successor_runtime_id = meerkat_meta::identity_first::live_binding_runtime_id(&successor)
        .expect("Core holds the reset successor live");
    assert_ne!(successor_runtime_id.as_str(), fixture.alias);
    for response in [logical, alias] {
        tokio::time::timeout(
            Duration::from_secs(5),
            axum::body::to_bytes(response.into_body(), 1024 * 1024),
        )
        .await
        .expect("reset must terminate an idle old-generation HTTP stream")
        .unwrap();
    }
    assert!(
        !fixture.response(&fixture.alias).await.status().is_success(),
        "the old runtime alias must be rejected, not retargeted onto the successor"
    );
    let current = fixture.response(successor_runtime_id.as_str()).await;
    assert_eq!(current.status(), StatusCode::OK);
    fixture.run_native_tool_turn().await;
    assert_native_tool_and_text(current).await;
    fixture.stop().await;
}

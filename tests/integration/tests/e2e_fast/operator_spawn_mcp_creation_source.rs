//! A member whose tool surface wraps a real MCP source spawns through the mob
//! operator tools, and every child records that member's session as its
//! creation source.
//!
//! The composition is the production one: a runtime-backed persistent session
//! service builds the member from a borrowed `CreateSessionRequest`, the
//! factory wraps the member's external tools (the mob operator tools) with the
//! MCP router adapter for the profile's MCP server, and the member's own model
//! turn calls `spawn_member` (with parent auto-wiring) and then
//! `spawn_many_members`. The MCP server is an in-process rmcp
//! streamable-HTTP server serving the shared test handler.
#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use meerkat_client::LlmRequest;
use meerkat_core::{ContentInput, HandlingMode, Message};
use meerkat_mob::{
    AgentIdentity, MemberCreationProvenance, MobBackendKind, MobDefinition, MobId, ProfileBinding,
    ProfileName,
};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};

use super::council_support::{CouncilFixture, ScriptedTurn, last_user_text, participant_profile};

const PROBE: &str = "MCP-OPERATOR-SPAWN-PROBE";
const PARENT: &str = "mcp-operator-parent";
const SINGLE: &str = "mcp-operator-single";
const BATCH: &str = "mcp-operator-batch";
const MCP_TOOL: &str = "mcp_form";

/// An in-process rmcp streamable-HTTP server; aborted on drop.
struct McpServer {
    url: String,
    task: tokio::task::JoinHandle<()>,
}

impl McpServer {
    async fn start() -> Self {
        let mut config = StreamableHttpServerConfig::default();
        config.stateful_mode = false;
        config.json_response = true;
        let service = StreamableHttpService::new(
            || Ok(mcp_test_server::FormTestServer::default()),
            Arc::new(LocalSessionManager::default()),
            config,
        );
        let app = axum::Router::new().nest_service("/mcp", service);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self { url, task }
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn definition(mob_id: &MobId, mcp_url: &str) -> MobDefinition {
    let mut profile = participant_profile("member with mob operator tools and an MCP source");
    profile.tools.mob = true;
    profile.external_addressable = true;
    profile.runtime_mode = meerkat_mob::MobRuntimeMode::TurnDriven;
    profile.tools.mcp_servers = vec![meerkat_core::mcp_config::McpServerConfig::streamable_http(
        "records",
        mcp_url.to_string(),
        Default::default(),
    )];
    let mut profiles = BTreeMap::new();
    profiles.insert(
        ProfileName::from("participant"),
        ProfileBinding::Inline(Box::new(profile)),
    );
    let mut definition = MobDefinition::explicit(mob_id.clone());
    definition.profiles = profiles;
    definition
}

fn tool_results(messages: &[Message]) -> BTreeMap<String, String> {
    messages
        .iter()
        .filter_map(|message| match message {
            Message::ToolResults { results, .. } => Some(results),
            _ => None,
        })
        .flatten()
        .map(|result| (result.tool_use_id.clone(), result.text_content()))
        .collect()
}

#[derive(Default)]
struct Observed {
    probe_tools: Option<BTreeSet<String>>,
    results: Option<BTreeMap<String, String>>,
}

/// The parent's model: on the probe turn, spawn one child with parent
/// auto-wiring, then a batch of one, then record what it saw.
fn script(observed: Arc<Mutex<Observed>>) -> impl Fn(&LlmRequest) -> ScriptedTurn + Send + Sync {
    move |request| {
        if !last_user_text(request).contains(PROBE) {
            return ScriptedTurn::Text("ok".to_string());
        }
        let mut observed = observed.lock().unwrap();
        observed.probe_tools.get_or_insert_with(|| {
            request
                .tools
                .iter()
                .map(|tool| tool.name.to_string())
                .collect()
        });
        let results = tool_results(&request.messages);
        if !results.contains_key("call-spawn") {
            return ScriptedTurn::ToolCall {
                id: "call-spawn".to_string(),
                name: "spawn_member".to_string(),
                args: serde_json::json!({
                    "profile": "participant",
                    "member_id": SINGLE,
                    "auto_wire_parent": true,
                }),
            };
        }
        if !results.contains_key("call-spawn-many") {
            return ScriptedTurn::ToolCall {
                id: "call-spawn-many".to_string(),
                name: "spawn_many_members".to_string(),
                args: serde_json::json!({
                    "specs": [{ "profile": "participant", "member_id": BATCH }],
                }),
            };
        }
        observed.results = Some(results);
        ScriptedTurn::Text("spawned".to_string())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_fast_operator_spawns_from_an_mcp_wrapped_member_record_the_parent_as_source() {
    let server = McpServer::start().await;
    let observed = Arc::new(Mutex::new(Observed::default()));
    let fixture = CouncilFixture::new_runtime_backed(script(Arc::clone(&observed)));
    let state = &fixture.state;

    let mob_id = MobId::from(format!("mcp-operator-{}", uuid::Uuid::new_v4().simple()));
    state
        .mob_create_definition(definition(&mob_id, &server.url))
        .await
        .expect("create the mob");
    state
        .mob_spawn(
            &mob_id,
            ProfileName::from("participant"),
            AgentIdentity::from(PARENT),
            Some(meerkat_mob::MobRuntimeMode::TurnDriven),
            Some(MobBackendKind::Session),
            None,
        )
        .await
        .expect("spawn the parent member");
    let handle = state.handle_for(&mob_id).await.expect("mob handle");
    let parent_session = handle
        .resolve_bridge_session_id(&AgentIdentity::from(PARENT))
        .await
        .expect("parent session");
    let turn = handle
        .member(&AgentIdentity::from(PARENT))
        .await
        .expect("parent member handle")
        .start_turn(
            ContentInput::Text(PROBE.to_string()),
            HandlingMode::Queue,
            meerkat_mob::MemberTurnOptions::default(),
            None,
        )
        .await
        .expect("probe turn admitted");
    tokio::time::timeout(Duration::from_secs(60), turn.wait())
        .await
        .expect("the probe turn completes")
        .expect("the probe turn succeeds");

    {
        let observed = observed.lock().unwrap();
        let tools = observed
            .probe_tools
            .as_ref()
            .expect("the probe turn reached the model");
        assert!(
            tools.contains(MCP_TOOL),
            "the member's surface carries the MCP source's tool: {tools:?}"
        );
        assert!(tools.contains("spawn_member") && tools.contains("spawn_many_members"));
        let results = observed
            .results
            .as_ref()
            .expect("the probe turn saw both spawn results");
        for call in ["call-spawn", "call-spawn-many"] {
            let text = &results[call];
            assert!(!text.contains("\"error\""), "{call} succeeds: {text}");
        }
    }

    for child in [SINGLE, BATCH] {
        let child_session = handle
            .resolve_bridge_session_id(&AgentIdentity::from(child))
            .await
            .unwrap_or_else(|| panic!("{child} session"));
        let creation = handle
            .member_creation_for_session(&child_session)
            .await
            .expect("read the child's creation facts")
            .expect("the child has creation facts");
        match &creation.creation.provenance {
            MemberCreationProvenance::Spawn { source } => assert_eq!(
                source.session_id, parent_session,
                "{child}'s creation source is the calling member's session"
            ),
            other => panic!("{child} must record Spawn {{ source }}, got {other:?}"),
        }
    }

    // Parent auto-wiring used the same per-call owner.
    let roster = handle.roster().await;
    let single = roster
        .list()
        .find(|entry| entry.agent_identity.as_str() == SINGLE)
        .expect("the single child is in the roster");
    assert!(
        single.wired_to.contains(&AgentIdentity::from(PARENT)),
        "the auto-wired child is wired to its parent: {:?}",
        single.wired_to
    );

    drop(roster);
    let _ = state.mob_destroy(&mob_id).await;
    fixture.teardown().await;
    drop(server);
}

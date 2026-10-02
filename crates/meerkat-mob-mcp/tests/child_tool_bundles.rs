//! Host tool bundles for mobs created through the public mob tools: a child
//! profile may name only bundles the host registered as child-available, and
//! names them by id. Everything else is refused with one indistinguishable
//! message and nothing is created.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use meerkat_core::types::{ContentInput, HandlingMode, ToolCallView, ToolDef, ToolResult};
use meerkat_core::{AgentToolDispatcher, ToolDispatchOutcome, ToolError};
use meerkat_mob::{AgentIdentity, BoundedResultSpec, MobId, WorkOrigin, WorkSpec};
use meerkat_mob_mcp::{ChildToolBundleAvailability, ChildToolBundles, handle_public_tools_call};
use serde_json::json;
use support::{CouncilFixture, ScriptedTurn};

const CHILD_TOOL: &str = "child_probe";
const HOST_TOOL: &str = "host_secret";

struct OneTool(&'static str);

#[async_trait::async_trait]
impl AgentToolDispatcher for OneTool {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        vec![Arc::new(ToolDef::new(
            self.0,
            "Host-registered bundle tool.",
            json!({"type": "object", "properties": {}}),
        ))]
        .into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        Ok(ToolDispatchOutcome::sync_result(ToolResult::new(
            call.id.to_string(),
            "{}".to_string(),
            false,
        )))
    }

    fn capabilities(&self) -> meerkat_core::agent::DispatcherCapabilities {
        meerkat_core::agent::DispatcherCapabilities::default()
    }
}

fn bundles() -> ChildToolBundles {
    ChildToolBundles::new()
        .register(
            "child-probe",
            Arc::new(OneTool(CHILD_TOOL)),
            ChildToolBundleAvailability::ChildAvailable,
        )
        .register(
            "host-secret",
            Arc::new(OneTool(HOST_TOOL)),
            ChildToolBundleAvailability::HostOnly,
        )
}

/// A fixture whose model records the tool names of every request.
fn fixture() -> (CouncilFixture, Arc<Mutex<Vec<Vec<String>>>>) {
    let recorded = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&recorded);
    let fixture = CouncilFixture::new_with(
        move |request| {
            sink.lock().unwrap().push(
                request
                    .tools
                    .iter()
                    .map(|tool| tool.name.to_string())
                    .collect(),
            );
            ScriptedTurn::Text("ok".to_string())
        },
        |state, _root| state.with_child_tool_bundles(bundles()),
    );
    (fixture, recorded)
}

fn definition(mob_id: &str, bundle: &str) -> serde_json::Value {
    json!({
        "id": mob_id,
        "profiles": {
            "worker": {
                "model": "claude-sonnet-4-6",
                "runtime_mode": "turn_driven",
                "tools": { "comms": true, "rust_bundles": [bundle] }
            }
        }
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn a_child_available_bundle_reaches_the_child_member() {
    let (fixture, recorded) = fixture();
    let mob_id = format!("child-{}", fixture.scope);
    handle_public_tools_call(
        &fixture.state,
        "meerkat_mob_create",
        &json!({ "definition": definition(&mob_id, "child-probe") }),
    )
    .await
    .expect("a child-available bundle is accepted");
    handle_public_tools_call(
        &fixture.state,
        "meerkat_mob_spawn",
        &json!({ "mob_id": mob_id, "profile": "worker", "agent_identity": "child-worker" }),
    )
    .await
    .expect("spawn the child member");

    let handle = fixture
        .state
        .handle_for(&MobId::from(mob_id.as_str()))
        .await
        .expect("child mob handle");
    let spec = BoundedResultSpec::new("probe", 4096).expect("bounded result spec");
    let work = handle
        .start_work_for_identity_bounded(
            AgentIdentity::from("child-worker"),
            WorkSpec::new(
                ContentInput::Text("hello".to_string()),
                WorkOrigin::Internal,
            ),
            HandlingMode::Queue,
            spec.clone(),
        )
        .await
        .expect("start a turn");
    tokio::time::timeout(Duration::from_secs(60), work.wait_bounded(spec))
        .await
        .expect("turn completes within the failure bound")
        .expect("turn succeeds");

    let requests = recorded.lock().unwrap().clone();
    let tools = requests.last().expect("the member reached the model");
    assert!(tools.iter().any(|name| name == CHILD_TOOL), "{tools:?}");
    assert!(!tools.iter().any(|name| name == HOST_TOOL), "{tools:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn host_only_and_unregistered_bundles_are_refused_alike() {
    let (fixture, _recorded) = fixture();
    let mut messages = Vec::new();
    for bundle in ["host-secret", "never-registered"] {
        let mob_id = format!("child-{}-{bundle}", fixture.scope);
        let error = handle_public_tools_call(
            &fixture.state,
            "meerkat_mob_create",
            &json!({ "definition": definition(&mob_id, bundle) }),
        )
        .await
        .expect_err("a bundle that is not child-available is refused");
        assert_eq!(error.code, -32602, "{error:?}");
        messages.push(error.message.replace(bundle, "<bundle>"));
        assert!(
            fixture
                .state
                .handle_for(&MobId::from(mob_id.as_str()))
                .await
                .is_err(),
            "no mob is created for a refused definition"
        );
    }
    assert_eq!(
        messages[0], messages[1],
        "host-only and unregistered bundles must be indistinguishable"
    );
    assert!(
        messages[0].contains("not available to child mobs"),
        "{}",
        messages[0]
    );
}

//! Host tool bundles for child mobs: the host supplies every bundle it
//! registered as child-available to the members of a mob created through the
//! agent `mob_create`, and never a host-only one. Callers cannot name bundles
//! at all, and mobs the host creates get none of the child bundles.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use meerkat_core::types::{ContentInput, HandlingMode, ToolCallView, ToolDef, ToolResult};
use meerkat_core::{AgentToolDispatcher, SessionId, ToolDispatchOutcome, ToolError};
use meerkat_mob::{AgentIdentity, BoundedResultSpec, MobId, MobRuntimeMode, WorkOrigin, WorkSpec};
use meerkat_mob_mcp::{
    AgentMobToolSurface, ChildToolBundleAvailability, ChildToolBundles, handle_public_tools_call,
};
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

fn definition(mob_id: &str, tools: serde_json::Value) -> serde_json::Value {
    json!({
        "id": mob_id,
        "profiles": {
            "worker": {
                "model": "claude-sonnet-4-6",
                "tools": tools
            }
        }
    })
}

fn agent_surface(fixture: &CouncilFixture) -> Arc<dyn AgentToolDispatcher> {
    Arc::new(AgentMobToolSurface::new(
        Arc::clone(&fixture.state),
        None,
        meerkat_runtime::mob_operator_authority::create_only_mob_operator_authority()
            .expect("generated authority"),
        "claude-sonnet-4-5".to_string(),
        SessionId::new(),
        None,
        None,
        None,
    ))
}

async fn agent_mob_create(
    fixture: &CouncilFixture,
    definition: serde_json::Value,
) -> Result<ToolDispatchOutcome, ToolError> {
    let raw =
        serde_json::value::RawValue::from_string(json!({ "definition": definition }).to_string())
            .unwrap();
    agent_surface(fixture)
        .dispatch(ToolCallView {
            id: "surface-call",
            name: "mob_create",
            args: &raw,
        })
        .await
}

/// Spawn one member of `mob_id`, run a turn, and return the tool names of
/// its last model request.
async fn member_tools(
    fixture: &CouncilFixture,
    recorded: &Mutex<Vec<Vec<String>>>,
    mob_id: &str,
) -> Vec<String> {
    let mob_id = MobId::from(mob_id);
    fixture
        .state
        .mob_spawn(
            &mob_id,
            "worker".into(),
            AgentIdentity::from("worker-1"),
            Some(MobRuntimeMode::TurnDriven),
            None,
            None,
        )
        .await
        .expect("spawn the member");
    let handle = fixture.state.handle_for(&mob_id).await.expect("mob handle");
    let spec = BoundedResultSpec::new("probe", 4096).expect("bounded result spec");
    let work = handle
        .start_work_for_identity_bounded(
            AgentIdentity::from("worker-1"),
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
    recorded
        .lock()
        .unwrap()
        .last()
        .cloned()
        .expect("the member reached the model")
}

#[tokio::test(flavor = "multi_thread")]
async fn child_available_bundles_reach_child_members_without_naming_them() {
    let (fixture, recorded) = fixture();
    let mob_id = format!("child-{}", fixture.scope);
    agent_mob_create(&fixture, definition(&mob_id, json!({ "comms": true })))
        .await
        .expect("the agent creates a child mob");
    let tools = member_tools(&fixture, &recorded, &mob_id).await;
    assert!(tools.iter().any(|name| name == CHILD_TOOL), "{tools:?}");
    assert!(!tools.iter().any(|name| name == HOST_TOOL), "{tools:?}");
    fixture.teardown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_host_created_mob_gets_no_child_bundles() {
    let (fixture, recorded) = fixture();
    let mob_id = format!("host-{}", fixture.scope);
    handle_public_tools_call(
        &fixture.state,
        "meerkat_mob_create",
        &json!({ "definition": definition(&mob_id, json!({ "comms": true })) }),
    )
    .await
    .expect("the host creates a mob");
    let tools = member_tools(&fixture, &recorded, &mob_id).await;
    assert!(!tools.iter().any(|name| name == CHILD_TOOL), "{tools:?}");
    assert!(!tools.iter().any(|name| name == HOST_TOOL), "{tools:?}");
    fixture.teardown().await;
}

/// Bundle ids are not public input: naming one is an argument error on both
/// the agent and the public create, and nothing is created.
#[tokio::test(flavor = "multi_thread")]
async fn callers_cannot_name_tool_bundles() {
    let (fixture, _recorded) = fixture();
    let agent_mob = format!("child-{}", fixture.scope);
    let error = agent_mob_create(
        &fixture,
        definition(
            &agent_mob,
            json!({ "comms": true, "rust_bundles": ["child-probe"] }),
        ),
    )
    .await
    .expect_err("an agent cannot name a tool bundle");
    assert!(
        matches!(error, ToolError::InvalidArguments { .. }),
        "{error:?}"
    );
    let public_mob = format!("public-{}", fixture.scope);
    let error = handle_public_tools_call(
        &fixture.state,
        "meerkat_mob_create",
        &json!({ "definition": definition(&public_mob, json!({ "rust_bundles": ["child-probe"] })) }),
    )
    .await
    .expect_err("a public caller cannot name a tool bundle");
    assert_eq!(error.code, -32602, "{error:?}");
    for mob_id in [agent_mob, public_mob] {
        assert!(
            fixture
                .state
                .handle_for(&MobId::from(mob_id.as_str()))
                .await
                .is_err(),
            "no mob is created"
        );
    }
    fixture.teardown().await;
}

/// The supplied bundle ids persist with the child definition, like the child
/// application tool policy persists with its members. A host that restarts
/// with a bundle withdrawn does not silently drop it or grant a substitute:
/// resuming the child member refuses with a typed error naming the bundle.
#[tokio::test(flavor = "multi_thread")]
async fn resuming_a_child_after_the_host_withdraws_its_bundle_refuses_typed() {
    let (fixture, recorded) = fixture();
    let mob_id = format!("child-{}", fixture.scope);
    agent_mob_create(&fixture, definition(&mob_id, json!({ "comms": true })))
        .await
        .expect("the agent creates a child mob");
    let tools = member_tools(&fixture, &recorded, &mob_id).await;
    assert!(tools.iter().any(|name| name == CHILD_TOOL), "{tools:?}");
    let handle = fixture
        .state
        .handle_for(&MobId::from(mob_id.as_str()))
        .await
        .expect("child mob handle");
    handle.shutdown().await.expect("shut the child mob down");

    // The restarted host registers no child bundles at all.
    let restarted = fixture.restart_state();
    let outcome = async {
        let handle = restarted.handle_for(&MobId::from(mob_id.as_str())).await?;
        let spec = BoundedResultSpec::new("resume", 4096)
            .map_err(|error| meerkat_mob::MobError::Internal(error.to_string()))?;
        let work = handle
            .start_work_for_identity_bounded(
                AgentIdentity::from("worker-1"),
                WorkSpec::new(
                    ContentInput::Text("again".to_string()),
                    WorkOrigin::Internal,
                ),
                HandlingMode::Queue,
                spec.clone(),
            )
            .await?;
        tokio::time::timeout(Duration::from_secs(60), work.wait_bounded(spec))
            .await
            .map_err(|error| meerkat_mob::MobError::Internal(error.to_string()))?
            .map(|_| ())
            .map_err(|error| meerkat_mob::MobError::Internal(format!("{error:?}")))
    }
    .await;
    let error = outcome.expect_err("the withdrawn bundle is never silently dropped");
    let message = error.to_string();
    assert!(
        matches!(&error, meerkat_mob::MobError::ToolBundleUnavailable { bundle } if bundle == "child-probe")
            || message.contains("tool bundle 'child-probe' is not registered"),
        "a typed refusal naming the bundle, got: {error:?}"
    );
    let calls_after_restart = recorded.lock().unwrap().len();
    assert_eq!(
        calls_after_restart, 1,
        "the refused member never reached the model"
    );
    fixture.teardown().await;
}

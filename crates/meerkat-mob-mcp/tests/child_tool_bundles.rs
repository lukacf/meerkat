//! Host tool bundles for child mobs: the host supplies every bundle it
//! registered as child-available to the members of a mob created through the
//! agent `mob_create`, and never a host-only one. A bundle the host registered
//! as offered reaches only the child profiles that opt into it with
//! `tools.bundles`; callers cannot name any other bundle, and mobs the host
//! creates get none of the child bundles.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use meerkat_client::LlmRequest;
use meerkat_core::types::{ContentInput, HandlingMode, ToolCallView, ToolDef, ToolResult};
use meerkat_core::{
    AgentToolDispatcher, ApplicationToolPolicyBinding, PolicyDigest, PolicyEvaluationProvenance,
    PolicyEvaluationSupervisorConfig, PolicyId, PolicyProviderGeneration, PolicyProviderId,
    PolicyRevision, SessionId, ToolConsequenceDenial, ToolConsequenceFailure,
    ToolConsequenceNarrowingPolicy, ToolConsequencePolicyRegistry, ToolConsequencePolicySnapshot,
    ToolConsequenceRequest, ToolConsequenceVerdict, ToolDispatchOutcome, ToolError,
};
use meerkat_mob::{AgentIdentity, BoundedResultSpec, MobId, MobRuntimeMode, WorkOrigin, WorkSpec};
use meerkat_mob_mcp::{
    AgentMobToolSurface, ChildToolBundleAvailability, ChildToolBundles, MobMcpState,
    handle_public_tools_call,
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
    fixture_with(bundles())
}

/// [`fixture`] over the host registrations `bundles`.
fn fixture_with(bundles: ChildToolBundles) -> (CouncilFixture, Arc<Mutex<Vec<Vec<String>>>>) {
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
        move |state, _root| state.with_child_tool_bundles(bundles),
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
    agent_surface_owned_by(fixture, SessionId::new())
}

/// The agent surface of `owner`'s session: the mobs it creates are that
/// session's child mobs.
fn agent_surface_owned_by(
    fixture: &CouncilFixture,
    owner: SessionId,
) -> Arc<dyn AgentToolDispatcher> {
    Arc::new(AgentMobToolSurface::new(
        Arc::clone(&fixture.state),
        None,
        meerkat_runtime::mob_operator_authority::create_only_mob_operator_authority()
            .expect("generated authority"),
        "claude-sonnet-4-5".to_string(),
        owner,
        None,
        None,
        None,
    ))
}

async fn agent_mob_create(
    fixture: &CouncilFixture,
    definition: serde_json::Value,
) -> Result<ToolDispatchOutcome, ToolError> {
    agent_mob_create_on(agent_surface(fixture), definition).await
}

async fn agent_mob_create_on(
    surface: Arc<dyn AgentToolDispatcher>,
    definition: serde_json::Value,
) -> Result<ToolDispatchOutcome, ToolError> {
    let raw =
        serde_json::value::RawValue::from_string(json!({ "definition": definition }).to_string())
            .unwrap();
    surface
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
    member_tools_of(fixture, recorded, mob_id, "worker", "worker-1").await
}

/// Spawn `member` of `profile` in `mob_id`, run a turn, and return the tool
/// names of its last model request.
async fn member_tools_of(
    fixture: &CouncilFixture,
    recorded: &Mutex<Vec<Vec<String>>>,
    mob_id: &str,
    profile: &str,
    member: &str,
) -> Vec<String> {
    let mob_id = MobId::from(mob_id);
    fixture
        .state
        .mob_spawn(
            &mob_id,
            profile.into(),
            AgentIdentity::from(member),
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
            AgentIdentity::from(member),
            WorkSpec::new(
                ContentInput::Text("hello".to_string()),
                WorkOrigin::Internal,
            ),
            HandlingMode::Queue,
            spec.clone(),
        )
        .await
        .expect("start a turn");
    tokio::time::timeout(Duration::from_mins(1), work.wait_bounded(spec))
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
    // A child mob is restored only while its owner session resolves; an
    // owner with no durable record holds the child before activation. So
    // the creating session here is a real, durable member session.
    fixture.seed_source_mob(&["owner"]).await;
    let owner = fixture
        .state
        .mob_resolve_bridge_session_id(&fixture.source_mob_id(), &AgentIdentity::from("owner"))
        .await
        .expect("resolve the owner session")
        .expect("the owner member has a session");
    agent_mob_create_on(
        agent_surface_owned_by(&fixture, owner),
        definition(&mob_id, json!({ "comms": true })),
    )
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
    // The owner's mob is restored by the restarted host too; this lifetime's
    // copy stops first, keeping the owner session durable.
    fixture
        .state
        .handle_for(&fixture.source_mob_id())
        .await
        .expect("owner mob handle")
        .shutdown()
        .await
        .expect("shut the owner mob down");

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
        tokio::time::timeout(Duration::from_mins(1), work.wait_bounded(spec))
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

// ---------------------------------------------------------------------------
// Opt-in: bundles the host offers to child mobs (ChildOffered) reach only the
// child profiles that name them in `tools.bundles`.
// ---------------------------------------------------------------------------

const OFFERED_TOOL: &str = "offered_probe";
const OFFERED_DENIED_TOOL: &str = "offered_denied";
const OTHER_OFFERED_TOOL: &str = "other_offered_probe";
const OFFERED: &str = "offered-pair";
const OTHER_OFFERED: &str = "other-offered";

/// A bundle of two tools that records each call that reaches it.
struct PairTools(Arc<Mutex<Vec<String>>>);

#[async_trait::async_trait]
impl AgentToolDispatcher for PairTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        [OFFERED_TOOL, OFFERED_DENIED_TOOL]
            .into_iter()
            .map(|name| {
                Arc::new(ToolDef::new(
                    name,
                    "Host-offered bundle tool.",
                    json!({"type": "object", "properties": {}}),
                ))
            })
            .collect::<Vec<_>>()
            .into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        self.0.lock().unwrap().push(call.name.to_string());
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

/// [`bundles`] plus the offered pair, registered `availability`.
fn offering(
    dispatched: &Arc<Mutex<Vec<String>>>,
    availability: ChildToolBundleAvailability,
) -> ChildToolBundles {
    bundles().register(
        OFFERED,
        Arc::new(PairTools(Arc::clone(dispatched))),
        availability,
    )
}

fn offered_fixture() -> (
    CouncilFixture,
    Arc<Mutex<Vec<Vec<String>>>>,
    Arc<Mutex<Vec<String>>>,
) {
    let dispatched = Arc::new(Mutex::new(Vec::new()));
    let (fixture, recorded) = fixture_with(offering(
        &dispatched,
        ChildToolBundleAvailability::ChildOffered,
    ));
    (fixture, recorded, dispatched)
}

/// A child definition with a profile that does not opt in (`worker`) and one
/// whose tools are `opted_tools` (`opted`).
fn opt_in_definition(mob_id: &str, opted_tools: serde_json::Value) -> serde_json::Value {
    json!({
        "id": mob_id,
        "profiles": {
            "worker": { "model": "claude-sonnet-4-6", "tools": { "comms": true } },
            "opted": { "model": "claude-sonnet-4-6", "tools": opted_tools }
        }
    })
}

fn opted(bundles: serde_json::Value) -> serde_json::Value {
    json!({ "comms": true, "bundles": bundles })
}

async fn withheld(state: &MobMcpState, mob_id: &str) -> Vec<(String, String)> {
    state
        .handle_for(&MobId::from(mob_id))
        .await
        .expect("mob handle")
        .withheld_tool_bundles()
        .await
        .expect("read the withheld bundles")
        .into_iter()
        .map(|withheld| (withheld.profile.to_string(), withheld.bundle))
        .collect()
}

/// R1: an offered bundle reaches only the child profiles that opt into it;
/// the child-available bundle still reaches every profile, the host-only one
/// none.
#[tokio::test(flavor = "multi_thread")]
async fn an_offered_bundle_reaches_only_the_profiles_that_opt_into_it() {
    let (fixture, recorded, _dispatched) = offered_fixture();
    let mob_id = format!("child-{}", fixture.scope);
    agent_mob_create(
        &fixture,
        opt_in_definition(&mob_id, opted(json!([OFFERED]))),
    )
    .await
    .expect("the agent creates a child mob that opts in");
    let plain = member_tools_of(&fixture, &recorded, &mob_id, "worker", "worker-1").await;
    assert!(!plain.iter().any(|name| name == OFFERED_TOOL), "{plain:?}");
    assert!(plain.iter().any(|name| name == CHILD_TOOL), "{plain:?}");
    let opted = member_tools_of(&fixture, &recorded, &mob_id, "opted", "opted-1").await;
    assert!(opted.iter().any(|name| name == OFFERED_TOOL), "{opted:?}");
    assert!(opted.iter().any(|name| name == CHILD_TOOL), "{opted:?}");
    assert!(!opted.iter().any(|name| name == HOST_TOOL), "{opted:?}");
    assert!(withheld(&fixture.state, &mob_id).await.is_empty());
    fixture.teardown().await;
}

/// R2: only an offered bundle may be named. A host-only, a child-available or
/// an unknown name is an argument error that lists what the host offers, and
/// nothing is created.
#[tokio::test(flavor = "multi_thread")]
async fn naming_a_bundle_the_host_does_not_offer_is_an_argument_error() {
    let (fixture, _recorded, _dispatched) = offered_fixture();
    for (index, bundle) in ["host-secret", "child-probe", "never-registered"]
        .into_iter()
        .enumerate()
    {
        let mob_id = format!("child-{index}-{}", fixture.scope);
        let error = agent_mob_create(&fixture, opt_in_definition(&mob_id, opted(json!([bundle]))))
            .await
            .expect_err("a bundle the host does not offer is refused");
        let ToolError::InvalidArguments { reason, .. } = &error else {
            panic!("an argument error, got {error:?}");
        };
        assert!(
            reason.contains(&format!("'{bundle}'")) && reason.contains(OFFERED),
            "the refusal names the bundle and lists the offered ones: {reason}"
        );
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

/// R2: a create that is not a child mob's offers no bundle: the public create
/// answers invalid params, whatever the host offers to children.
#[tokio::test(flavor = "multi_thread")]
async fn a_host_create_offers_no_bundle_to_opt_into() {
    let (fixture, _recorded, _dispatched) = offered_fixture();
    let mob_id = format!("public-{}", fixture.scope);
    let error = handle_public_tools_call(
        &fixture.state,
        "meerkat_mob_create",
        &json!({ "definition": opt_in_definition(&mob_id, opted(json!([OFFERED]))) }),
    )
    .await
    .expect_err("a host create offers no bundle");
    assert_eq!(error.code, -32602, "{error:?}");
    assert!(error.message.contains(&format!("'{OFFERED}'")), "{error:?}");
    assert!(
        fixture
            .state
            .handle_for(&MobId::from(mob_id.as_str()))
            .await
            .is_err(),
        "no mob is created"
    );
    fixture.teardown().await;
}

/// N2: an empty or a repeated bundle name is an argument error at decode.
#[tokio::test(flavor = "multi_thread")]
async fn an_empty_or_repeated_bundle_name_is_an_argument_error() {
    let (fixture, _recorded, _dispatched) = offered_fixture();
    for (index, (bundles, expected)) in
        [(json!([""]), "empty"), (json!([OFFERED, OFFERED]), "twice")]
            .into_iter()
            .enumerate()
    {
        let mob_id = format!("child-{index}-{}", fixture.scope);
        let error = agent_mob_create(&fixture, opt_in_definition(&mob_id, opted(bundles)))
            .await
            .expect_err("a malformed opt-in is refused");
        let ToolError::InvalidArguments { reason, .. } = &error else {
            panic!("an argument error, got {error:?}");
        };
        assert!(reason.contains(expected), "{reason}");
    }
    fixture.teardown().await;
}

/// Create, from a durable owner member's session, a child mob whose `opted`
/// profile opts into the offered pair and denies one of its tools, and seat
/// its `opted-1` member. Returns the child mob id.
async fn create_durable_opted_child(
    fixture: &CouncilFixture,
    recorded: &Mutex<Vec<Vec<String>>>,
) -> String {
    let mob_id = format!("child-{}", fixture.scope);
    // A child mob is restored only while its owner session resolves, so the
    // creating session is a real, durable member session.
    fixture.seed_source_mob(&["owner"]).await;
    let owner = fixture
        .state
        .mob_resolve_bridge_session_id(&fixture.source_mob_id(), &AgentIdentity::from("owner"))
        .await
        .expect("resolve the owner session")
        .expect("the owner member has a session");
    agent_mob_create_on(
        agent_surface_owned_by(fixture, owner),
        opt_in_definition(
            &mob_id,
            json!({ "comms": true, "bundles": [OFFERED], "deny": [OFFERED_DENIED_TOOL] }),
        ),
    )
    .await
    .expect("the agent creates a child mob that opts in");
    let tools = member_tools_of(fixture, recorded, &mob_id, "opted", "opted-1").await;
    assert!(tools.iter().any(|name| name == OFFERED_TOOL), "{tools:?}");
    assert!(
        !tools.iter().any(|name| name == OFFERED_DENIED_TOOL),
        "a denied tool is hidden from the catalog: {tools:?}"
    );
    mob_id
}

/// Run one more turn of `opted-1` in `mob_id` on `state`, the member
/// rebuilt by this lifetime.
async fn resumed_turn(state: &MobMcpState, mob_id: &str) {
    let handle = state
        .handle_for(&MobId::from(mob_id))
        .await
        .expect("the restored child mob");
    let spec = BoundedResultSpec::new("resume", 4096).expect("bounded result spec");
    let work = handle
        .start_work_for_identity_bounded(
            AgentIdentity::from("opted-1"),
            WorkSpec::new(
                ContentInput::Text("again".to_string()),
                WorkOrigin::Internal,
            ),
            HandlingMode::Queue,
            spec.clone(),
        )
        .await
        .expect("start a turn on the restored member");
    tokio::time::timeout(Duration::from_mins(1), work.wait_bounded(spec))
        .await
        .expect("turn completes within the failure bound")
        .expect("the restored member builds and runs");
}

/// Restart `fixture` cold over the same stores with `bundles` as the host's
/// child registrations; returns the tool names of each model request the
/// restarted lifetime makes.
async fn restart_with_bundles(
    fixture: &mut CouncilFixture,
    bundles: ChildToolBundles,
) -> Arc<Mutex<Vec<Vec<String>>>> {
    let restarted_tools = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&restarted_tools);
    fixture
        .restart_cold_with(
            move |request| {
                sink.lock().unwrap().push(
                    request
                        .tools
                        .iter()
                        .map(|tool| tool.name.to_string())
                        .collect::<Vec<_>>(),
                );
                ScriptedTurn::Text("ok".to_string())
            },
            move |state, _root| state.with_child_tool_bundles(bundles),
        )
        .await;
    restarted_tools
}

/// R3: the host's offer is the ceiling. A host that restarts registering the
/// bundle host-only withholds it: the member rebuilds and runs without its
/// tools (the child-available bundle, strict, is still supplied), its deny
/// entry naming one of them does not fail the build, and the handle reports
/// the bundle withheld for the profile.
#[tokio::test(flavor = "multi_thread")]
async fn a_restart_under_a_lowered_ceiling_withholds_the_opted_bundle() {
    let (mut fixture, recorded, dispatched) = offered_fixture();
    let mob_id = create_durable_opted_child(&fixture, &recorded).await;

    let restarted_tools = restart_with_bundles(
        &mut fixture,
        offering(&dispatched, ChildToolBundleAvailability::HostOnly),
    )
    .await;
    resumed_turn(&fixture.state, &mob_id).await;
    let tools = restarted_tools
        .lock()
        .unwrap()
        .last()
        .cloned()
        .expect("the restored member reached the model");
    assert!(!tools.iter().any(|name| name == OFFERED_TOOL), "{tools:?}");
    assert!(tools.iter().any(|name| name == CHILD_TOOL), "{tools:?}");
    assert_eq!(
        withheld(&fixture.state, &mob_id).await,
        [("opted".to_string(), OFFERED.to_string())]
    );
    fixture.teardown().await;
}

/// R3 control and R4: a restart that still offers the bundle mounts it again
/// with nothing withheld, and a bundle the host newly offers is not mounted:
/// the profile gets what it opted into, not everything offered now.
#[tokio::test(flavor = "multi_thread")]
async fn a_restart_resupplies_exactly_what_the_profile_opted_into() {
    let (mut fixture, recorded, dispatched) = offered_fixture();
    let mob_id = create_durable_opted_child(&fixture, &recorded).await;

    let offered = offering(&dispatched, ChildToolBundleAvailability::ChildOffered).register(
        OTHER_OFFERED,
        Arc::new(OneTool(OTHER_OFFERED_TOOL)),
        ChildToolBundleAvailability::ChildOffered,
    );
    let restarted_tools = restart_with_bundles(&mut fixture, offered).await;
    resumed_turn(&fixture.state, &mob_id).await;
    let tools = restarted_tools
        .lock()
        .unwrap()
        .last()
        .cloned()
        .expect("the restored member reached the model");
    assert!(tools.iter().any(|name| name == OFFERED_TOOL), "{tools:?}");
    assert!(
        !tools.iter().any(|name| name == OTHER_OFFERED_TOOL),
        "a newly offered bundle is not opted into: {tools:?}"
    );
    assert!(withheld(&fixture.state, &mob_id).await.is_empty());
    fixture.teardown().await;
}

/// The member calls the pair's denied tool, then its other tool, then
/// answers.
fn calls_denied_then_offered() -> impl Fn(&LlmRequest) -> ScriptedTurn + Send + Sync + 'static {
    let step = AtomicUsize::new(0);
    move |_request| match step.fetch_add(1, Ordering::SeqCst) {
        0 => ScriptedTurn::ToolCall {
            id: "call-denied".to_string(),
            name: OFFERED_DENIED_TOOL.to_string(),
            args: json!({}),
        },
        1 => ScriptedTurn::ToolCall {
            id: "call-offered".to_string(),
            name: OFFERED_TOOL.to_string(),
            args: json!({}),
        },
        _ => ScriptedTurn::Text("done".to_string()),
    }
}

/// Create a child mob whose `opted` profile has `opted_tools`, seat
/// `opted-1` and run one turn; return the pair tools that ran.
async fn opted_member_calls(
    fixture: &CouncilFixture,
    dispatched: &Mutex<Vec<String>>,
    opted_tools: serde_json::Value,
) -> Vec<String> {
    let mob_id = format!("child-{}", fixture.scope);
    agent_mob_create(fixture, opt_in_definition(&mob_id, opted_tools))
        .await
        .expect("the agent creates the child mob");
    let mob_id = MobId::from(mob_id.as_str());
    fixture
        .state
        .mob_spawn(
            &mob_id,
            "opted".into(),
            AgentIdentity::from("opted-1"),
            Some(MobRuntimeMode::TurnDriven),
            None,
            None,
        )
        .await
        .expect("spawn the member");
    let handle = fixture.state.handle_for(&mob_id).await.expect("mob handle");
    let spec = BoundedResultSpec::new("calls", 4096).expect("bounded result spec");
    let work = handle
        .start_work_for_identity_bounded(
            AgentIdentity::from("opted-1"),
            WorkSpec::new(ContentInput::Text("go".to_string()), WorkOrigin::Internal),
            HandlingMode::Queue,
            spec.clone(),
        )
        .await
        .expect("start a turn");
    tokio::time::timeout(Duration::from_mins(1), work.wait_bounded(spec))
        .await
        .expect("turn completes within the failure bound")
        .expect("turn succeeds");
    dispatched.lock().unwrap().clone()
}

/// R7 (narrowing): a profile's deny list governs an opted bundle's tools as
/// it governs every other tool: the execution gate refuses the denied one.
#[tokio::test(flavor = "multi_thread")]
async fn a_deny_entry_refuses_an_opted_bundle_tool_at_the_gate() {
    let dispatched = Arc::new(Mutex::new(Vec::new()));
    let offered = offering(&dispatched, ChildToolBundleAvailability::ChildOffered);
    let fixture = CouncilFixture::new_with(calls_denied_then_offered(), move |state, _root| {
        state.with_child_tool_bundles(offered)
    });
    let ran = opted_member_calls(
        &fixture,
        &dispatched,
        json!({ "comms": true, "bundles": [OFFERED], "deny": [OFFERED_DENIED_TOOL] }),
    )
    .await;
    assert_eq!(ran, [OFFERED_TOOL]);
    fixture.teardown().await;
}

/// A host policy that denies the pair's denied tool for every member it
/// governs.
struct DenyPairTool;

impl ToolConsequencePolicySnapshot for DenyPairTool {
    fn provenance(&self) -> PolicyEvaluationProvenance {
        PolicyEvaluationProvenance {
            revision: PolicyRevision(1),
            digest: PolicyDigest::from_canonical_bytes(b"deny-pair-tool"),
        }
    }

    fn evaluate(&self, request: &ToolConsequenceRequest) -> ToolConsequenceVerdict {
        if request.tool_name.as_str() == OFFERED_DENIED_TOOL {
            ToolConsequenceVerdict::Deny(ToolConsequenceDenial::new(
                "host_policy_denied",
                "the host policy denies this tool",
            ))
        } else {
            ToolConsequenceVerdict::Allow
        }
    }
}

struct HostProvider(PolicyProviderId);

impl ToolConsequenceNarrowingPolicy for HostProvider {
    fn provider_id(&self) -> &PolicyProviderId {
        &self.0
    }

    fn generation(&self) -> PolicyProviderGeneration {
        PolicyProviderGeneration(1)
    }

    fn snapshot(
        &self,
        _policy_id: &PolicyId,
    ) -> Result<Arc<dyn ToolConsequencePolicySnapshot>, ToolConsequenceFailure> {
        Ok(Arc::new(DenyPairTool))
    }
}

fn policy_registry() -> Arc<ToolConsequencePolicyRegistry> {
    Arc::new(
        ToolConsequencePolicyRegistry::new(
            vec![Arc::new(HostProvider(
                PolicyProviderId::new("host").unwrap(),
            ))],
            PolicyEvaluationSupervisorConfig::default(),
            None,
        )
        .unwrap(),
    )
}

/// R7 (quotas and policy): the host's child application tool policy governs
/// an opted bundle's tools exactly as it governs a child-available bundle's.
#[tokio::test(flavor = "multi_thread")]
async fn the_host_child_policy_governs_an_opted_bundle_as_a_supplied_one() {
    for (availability, opted_tools) in [
        (
            ChildToolBundleAvailability::ChildOffered,
            json!({ "comms": true, "bundles": [OFFERED] }),
        ),
        (
            ChildToolBundleAvailability::ChildAvailable,
            json!({ "comms": true }),
        ),
    ] {
        let dispatched = Arc::new(Mutex::new(Vec::new()));
        let offered = offering(&dispatched, availability);
        let fixture = CouncilFixture::new_with(calls_denied_then_offered(), move |state, _root| {
            state
                .with_child_tool_bundles(offered)
                .with_tool_consequence_policy_registry(policy_registry())
                .with_child_application_tool_policy(ApplicationToolPolicyBinding::Provider {
                    provider_id: PolicyProviderId::new("host").unwrap(),
                    policy_id: PolicyId::new("child-tools").unwrap(),
                })
        });
        let ran = opted_member_calls(&fixture, &dispatched, opted_tools).await;
        assert_eq!(ran, [OFFERED_TOOL], "{availability:?}");
        fixture.teardown().await;
    }
}

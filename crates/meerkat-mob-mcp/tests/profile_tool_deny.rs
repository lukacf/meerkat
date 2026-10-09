//! A downstream app's per-profile deny set, end to end over the production session
//! service with the real agent mob tools: a `mob` profile denies the tools a
//! child identity agent could use to spawn or rewire broader same-mob members,
//! while keeping fork_off, council, mob_check_member and mob_retire_member.
//!
//! The deny list covers both mob tool sources: the agent mob tools composed
//! as the `mob` family (`mob_spawn_member`, `mob_wire`, ...) and the mob
//! operator tools mounted as external tools (`spawn_member`, `wire_members`,
//! ...). Every call goes through the member's own model turn, so it reaches
//! the member's outermost execution gate.
#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use meerkat_client::LlmRequest;
use meerkat_core::{ContentInput, HandlingMode, Message};
use meerkat_mob::{
    AgentIdentity, MobBackendKind, MobControlPrincipal, MobDefinition, MobId, ProfileBinding,
    ProfileName,
};
use meerkat_mob_mcp::MobMcpState;
use support::{
    CouncilFixture, ScriptedCouncilClient, ScriptedTurn, last_user_text, participant_profile,
};

const PROBE: &str = "DOWNSTREAM-DENY-PROBE";

const DENIED: &[&str] = &[
    "spawn_member",
    "spawn_many_members",
    "mob_spawn_member",
    "wire_members",
    "unwire_members",
    "mob_wire",
    "mob_unwire",
    "mob_create",
    "mob_destroy",
];

const KEPT: &[&str] = &[
    "fork_off",
    "council",
    "mob_check_member",
    "mob_retire_member",
];

fn pilot_host_definition(mob_id: &MobId) -> MobDefinition {
    let mut profile = participant_profile("team identity agent");
    profile.tools.mob = true;
    // The test drives the member's own turn from outside the mob.
    profile.external_addressable = true;
    profile.tools.deny = DENIED.iter().map(|name| (*name).to_string()).collect();
    let mut profiles = BTreeMap::new();
    profiles.insert(
        ProfileName::from("participant"),
        ProfileBinding::Inline(Box::new(profile)),
    );
    let mut definition = MobDefinition::explicit(mob_id.clone());
    definition.profiles = profiles;
    definition
}

/// The production composition: a runtime-backed persistent service whose
/// builder carries the agent mob tool factory (`wire_mob_tools`), so a `mob`
/// profile's members mount the real agent mob tools.
fn wired_state(root: &std::path::Path, client: ScriptedCouncilClient) -> Arc<MobMcpState> {
    let project_root = root.join("project-root");
    std::fs::create_dir_all(&project_root).expect("project root");
    std::fs::write(project_root.join("AGENTS.md"), "# deny fixture\n").expect("AGENTS.md");
    let factory = meerkat::AgentFactory::new(root.join("factory-store"))
        .user_config_root(root.join("user-config"))
        .runtime_root(root.join("runtime-root"))
        .project_root(project_root.clone())
        .context_root(project_root)
        .builtins(false)
        .comms(true);
    let mut builder = meerkat::FactoryAgentBuilder::new(factory, meerkat::Config::default());
    builder.default_llm_client = Some(Arc::new(client));
    let store = Arc::new(meerkat_store::JsonlStore::new(root.join("sessions-jsonl")));
    builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(store.clone())));
    let mob_tools_slot = Arc::clone(&builder.default_mob_tools);
    let store_dyn: Arc<dyn meerkat::SessionStore> = store;
    let (service, runtime) = meerkat::surface::build_runtime_backed_service(
        builder,
        32,
        meerkat::PersistenceBundle::new(
            store_dyn,
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
            Arc::new(meerkat_store::MemoryBlobStore::default()),
        )
        .expect("construct runtime authority"),
    );
    meerkat_mob_mcp::wire_mob_tools(
        &mob_tools_slot,
        Arc::new(service),
        Some(runtime),
        None,
        MobControlPrincipal::Owner,
    )
    .expect("wire mob tools")
}

/// The tool-result text of each call in `messages`, keyed by call id.
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

type ProbeResults = Arc<Mutex<Option<BTreeMap<String, String>>>>;

/// The member's model: on the probe turn, call every probe tool once, in
/// order, then record the results it saw.
fn probe_script(
    final_results: ProbeResults,
    probes: Vec<&'static str>,
) -> impl Fn(&LlmRequest) -> ScriptedTurn + Send + Sync + 'static {
    move |request| {
        if !last_user_text(request).contains(PROBE) {
            return ScriptedTurn::Text("ok".to_string());
        }
        // One call per probe, in order; each later request carries the
        // results so far.
        let results = tool_results(&request.messages);
        match probes.get(results.len()) {
            Some(tool) => ScriptedTurn::ToolCall {
                id: format!("call-{tool}"),
                name: (*tool).to_string(),
                args: serde_json::json!({}),
            },
            None => {
                *final_results.lock().unwrap() = Some(results);
                ScriptedTurn::Text("probed".to_string())
            }
        }
    }
}

/// Create the mob, spawn the member (its build must accept every deny name),
/// drive the probe turn, and check every `denied` call is refused by the
/// gate while the `kept` ones pass it.
async fn probe_pilot_host_member(
    state: &Arc<MobMcpState>,
    final_results: &ProbeResults,
    denied: &[&str],
    kept: &[&str],
) {
    let mob_id = MobId::from(format!("profile-deny-{}", uuid::Uuid::new_v4().simple()));
    state
        .mob_create_definition(pilot_host_definition(&mob_id))
        .await
        .expect("create the mob");
    // A name in no tool vocabulary would fail the spawn as
    // `DeclaredToolUnknown`.
    state
        .mob_spawn(
            &mob_id,
            ProfileName::from("participant"),
            AgentIdentity::from("kitchen"),
            Some(meerkat_mob::MobRuntimeMode::TurnDriven),
            Some(MobBackendKind::Session),
            None,
        )
        .await
        .expect("spawn the member with the downstream deny set");
    let handle = state.handle_for(&mob_id).await.expect("mob handle");
    let turn = handle
        .member(&AgentIdentity::from("kitchen"))
        .await
        .expect("member handle")
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
        .expect("gate denials do not fail the turn");

    let results = final_results
        .lock()
        .unwrap()
        .clone()
        .expect("the probe turn reached its final request");
    for tool in denied {
        let text = results
            .get(&format!("call-{tool}"))
            .unwrap_or_else(|| panic!("{tool} was called"));
        assert!(
            text.contains("\"error\":\"access_denied\""),
            "{tool} is denied by the profile: {text}"
        );
    }
    for tool in kept {
        let text = results
            .get(&format!("call-{tool}"))
            .unwrap_or_else(|| panic!("{tool} was called"));
        // Empty arguments make the kept tools fail their own validation; what
        // matters is that the execution gate let them through.
        assert!(
            !text.contains("\"error\":\"access_denied\""),
            "{tool} is kept: {text}"
        );
    }
    let _ = state.mob_destroy(&mob_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pilot_host_deny_set_gates_denied_mob_tools_and_keeps_the_rest() {
    let final_results = ProbeResults::default();
    let temp = tempfile::tempdir().expect("temp dir");
    let probes = DENIED.iter().chain(KEPT).copied().collect();
    let state = wired_state(
        temp.path(),
        ScriptedCouncilClient::new(probe_script(Arc::clone(&final_results), probes)),
    );
    probe_pilot_host_member(&state, &final_results, DENIED, KEPT).await;
}

/// The same deny set on a composition without the agent mob tool factory
/// (no `wire_mob_tools`): the agent mob tools are not mounted, yet every name
/// is in a tool vocabulary, so the member builds; its denied names are inert
/// where unmounted. The mob operator tools it does mount are still gated.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pilot_host_deny_set_builds_without_the_agent_mob_tool_factory() {
    const MOUNTED_DENIED: &[&str] = &[
        "spawn_member",
        "spawn_many_members",
        "wire_members",
        "unwire_members",
    ];
    const MOUNTED_KEPT: &[&str] = &["retire_member", "member_status"];
    let final_results = ProbeResults::default();
    let probes = MOUNTED_DENIED.iter().chain(MOUNTED_KEPT).copied().collect();
    let fixture =
        CouncilFixture::new_runtime_backed(probe_script(Arc::clone(&final_results), probes));
    probe_pilot_host_member(&fixture.state, &final_results, MOUNTED_DENIED, MOUNTED_KEPT).await;
    fixture.teardown().await;
}

/// The member's model for the snapshot tests: on every probe turn, call
/// `spawn_member` and `wire_members` with arguments that would create and wire
/// a real child if the gate let them through, then record the results that
/// turn saw (results of earlier turns are not counted).
fn snapshot_probe_script(
    turn_results: Arc<Mutex<Vec<BTreeMap<String, String>>>>,
) -> impl Fn(&LlmRequest) -> ScriptedTurn + Send + Sync + 'static {
    move |request| {
        if !last_user_text(request).contains(PROBE) {
            return ScriptedTurn::Text("ok".to_string());
        }
        let turn_start = request
            .messages
            .iter()
            .rposition(|message| matches!(message, Message::User(_)))
            .unwrap_or(0);
        let results = tool_results(&request.messages[turn_start..]);
        match results.len() {
            0 => ScriptedTurn::ToolCall {
                id: "call-spawn_member".to_string(),
                name: "spawn_member".to_string(),
                args: serde_json::json!({
                    "profile": "participant",
                    "member_id": "probe-child",
                }),
            },
            1 => ScriptedTurn::ToolCall {
                id: "call-wire_members".to_string(),
                name: "wire_members".to_string(),
                args: serde_json::json!({
                    "member_id": "kitchen",
                    "peer_member_id": "probe-child",
                }),
            },
            _ => {
                turn_results.lock().unwrap().push(results);
                ScriptedTurn::Text("probed".to_string())
            }
        }
    }
}

/// Drive one probe turn on `kitchen` and check both calls were refused by
/// the gate and no child was created.
async fn probe_snapshot_member(
    handle: &meerkat_mob::MobHandle,
    turn_results: &Mutex<Vec<BTreeMap<String, String>>>,
    expected_turns: usize,
) {
    let turn = handle
        .member(&AgentIdentity::from("kitchen"))
        .await
        .expect("member handle")
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
        .expect("gate denials do not fail the turn");
    let turns = turn_results.lock().unwrap().clone();
    assert_eq!(turns.len(), expected_turns, "{turns:?}");
    let results = turns.last().expect("this turn's results");
    for tool in ["spawn_member", "wire_members"] {
        let text = results
            .get(&format!("call-{tool}"))
            .unwrap_or_else(|| panic!("{tool} was called"));
        assert!(
            text.contains("\"error\":\"access_denied\""),
            "{tool} is denied by the role's current profile: {text}"
        );
    }
    assert!(
        handle
            .get_member(&AgentIdentity::from("probe-child"))
            .await
            .expect("read roster")
            .is_none(),
        "no child is created"
    );
}

/// 0.8.52 deny-on-resume regression: a member spawned with a profile
/// snapshot (`override_profile`, as an identity-first host takes for provider
/// params) that predates the role's deny list is denied spawn_member and
/// wire_members by the role's CURRENT profile, on its fresh build and again
/// after an explicit resume rebuilds it from the persisted snapshot.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn snapshot_member_is_denied_by_the_roles_current_profile_fresh_and_resumed() {
    let turn_results = Arc::new(Mutex::new(Vec::new()));
    let temp = tempfile::tempdir().expect("temp dir");
    let state = wired_state(
        temp.path(),
        ScriptedCouncilClient::new(snapshot_probe_script(Arc::clone(&turn_results))),
    );
    let mob_id = MobId::from(format!("profile-deny-{}", uuid::Uuid::new_v4().simple()));
    let definition = pilot_host_definition(&mob_id);
    let mut snapshot = definition.profiles[&ProfileName::from("participant")]
        .as_inline()
        .expect("inline participant profile")
        .clone();
    snapshot.tools.deny.clear();
    state
        .mob_create_definition(definition)
        .await
        .expect("create the mob");
    let mut spec = meerkat_mob::SpawnMemberSpec::new(
        ProfileName::from("participant"),
        AgentIdentity::from("kitchen"),
    );
    spec.runtime_mode = Some(meerkat_mob::MobRuntimeMode::TurnDriven);
    spec.backend = Some(MobBackendKind::Session);
    spec.override_profile = Some(snapshot);
    state
        .mob_spawn_spec(&mob_id, spec)
        .await
        .expect("spawn the member on a snapshot without the deny");
    let handle = state.handle_for(&mob_id).await.expect("mob handle");
    probe_snapshot_member(&handle, &turn_results, 1).await;

    handle.stop().await.expect("stop");
    handle
        .resume()
        .await
        .expect("explicit resume rebuilds the member from its snapshot");
    probe_snapshot_member(&handle, &turn_results, 2).await;
    let _ = state.mob_destroy(&mob_id).await;
}

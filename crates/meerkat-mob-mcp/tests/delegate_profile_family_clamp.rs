//! A delegate helper's tool surface may not exceed its parent's: a family the
//! parent's profile leaves off stays off for every helper the parent
//! delegates to, whatever `tooling` the parent's model passes.
//!
//! The parent's profile restriction carries its `deny` names and `read_only`
//! to the helper, but a family the profile merely disables is not a deny
//! entry. A model-supplied `tooling: {mode: "profile", source: inline}`
//! replaces the helper's profile, so it must not switch such a family on.
//!
//! End to end over the production session service with the real agent mob
//! tools: the parent member's own model turn calls `delegate`, and the
//! helper's own model turn calls the family's tool, so each call reaches the
//! calling session's outermost execution gate.
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
use support::{ScriptedCouncilClient, ScriptedTurn, last_user_text, participant_profile};

const DIRECT_PROBE: &str = "FAMILY-CLAMP-DIRECT-PROBE";
const PROBE: &str = "FAMILY-CLAMP-PROBE";
const HELPER_TASK: &str = "FAMILY-CLAMP-HELPER-TASK";

/// A tool of a family the parent's profile leaves off, and the text its
/// result carries only when the tool actually executed.
struct FamilyProbe {
    /// The inline profile `tools` key that switches the family on.
    family: &'static str,
    tool: &'static str,
    args: serde_json::Value,
    executed_marker: &'static str,
}

fn shell_probe() -> FamilyProbe {
    FamilyProbe {
        family: "shell",
        tool: "shell",
        // The marker only appears once a shell expands the arithmetic.
        args: serde_json::json!({ "command": "echo FAMILY-CLAMP-RAN-$((6*7))" }),
        executed_marker: "FAMILY-CLAMP-RAN-42",
    }
}

fn builtins_probe() -> FamilyProbe {
    FamilyProbe {
        family: "builtins",
        tool: "datetime",
        args: serde_json::json!({}),
        executed_marker: "unix_timestamp",
    }
}

/// One member whose profile mounts the agent mob tools (so it may delegate)
/// and leaves every other family at its default: off, not denied.
fn parent_definition(mob_id: &MobId) -> MobDefinition {
    let mut profile = participant_profile("delegating parent");
    profile.tools.mob = true;
    // The test drives the member's own turn from outside the mob.
    profile.external_addressable = true;
    assert!(!profile.tools.shell && !profile.tools.builtins);
    assert!(profile.tools.deny.is_empty());
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
/// builder carries the agent mob tool factory (`wire_mob_tools`).
fn wired_state(root: &std::path::Path, client: ScriptedCouncilClient) -> Arc<MobMcpState> {
    let project_root = root.join("project-root");
    std::fs::create_dir_all(&project_root).expect("project root");
    std::fs::write(project_root.join("AGENTS.md"), "# clamp fixture\n").expect("AGENTS.md");
    let factory = meerkat::AgentFactory::new(root.join("factory-store"))
        .user_config_root(root.join("user-config"))
        .runtime_root(root.join("runtime-root"))
        .project_root(project_root.clone())
        .context_root(project_root)
        .builtins(false)
        .shell(false)
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
        ),
    );
    meerkat_mob_mcp::wire_mob_tools(
        &mob_tools_slot,
        Arc::new(service),
        Some(runtime),
        None,
        MobControlPrincipal::Owner,
    )
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

type Seen = Arc<Mutex<Option<BTreeMap<String, String>>>>;

#[derive(Default, Clone)]
struct Observed {
    /// The parent's tool results at the end of its probe turn.
    parent: Seen,
    /// The helper's tool results at the end of its delegated turn, if the
    /// helper ran at all.
    helper: Seen,
}

/// Both sessions share one scripted client. On its direct probe turn the
/// parent calls the family's tool itself (the family is off for it, so the
/// call is an unknown tool and ends that turn). On its probe turn it
/// delegates with an inline profile that switches the family on, and the
/// helper calls the tool.
fn script(
    observed: Observed,
    probe: &FamilyProbe,
) -> impl Fn(&LlmRequest) -> ScriptedTurn + Send + Sync + 'static {
    let family = probe.family;
    let tool = probe.tool;
    let args = probe.args.clone();
    move |request| {
        let user = last_user_text(request);
        let results = tool_results(&request.messages);
        if user.contains(HELPER_TASK) {
            if results.is_empty() {
                return ScriptedTurn::ToolCall {
                    id: "call-helper-probe".to_string(),
                    name: tool.to_string(),
                    args: args.clone(),
                };
            }
            *observed.helper.lock().unwrap() = Some(results);
            return ScriptedTurn::Text("helper done".to_string());
        }
        if user.contains(DIRECT_PROBE) {
            return ScriptedTurn::ToolCall {
                id: "call-parent-probe".to_string(),
                name: tool.to_string(),
                args: args.clone(),
            };
        }
        if !user.contains(PROBE) {
            return ScriptedTurn::Text("ok".to_string());
        }
        let mut helper_tools = serde_json::Map::new();
        helper_tools.insert("comms".to_string(), true.into());
        helper_tools.insert(family.to_string(), true.into());
        if !results.contains_key("call-delegate") {
            return ScriptedTurn::ToolCall {
                id: "call-delegate".to_string(),
                name: "delegate".to_string(),
                args: serde_json::json!({
                    "task": HELPER_TASK,
                    "member_id": "helper",
                    "result_label": "helper_result",
                    "max_text_bytes": 4096,
                    "tooling": {
                        "mode": "profile",
                        "source": {
                            "type": "inline",
                            "model": "claude-haiku-4-5-20251001",
                            "tools": helper_tools
                        }
                    }
                }),
            };
        }
        *observed.parent.lock().unwrap() = Some(results);
        ScriptedTurn::Text("probed".to_string())
    }
}

async fn assert_helper_cannot_enable_parent_disabled_family(probe: FamilyProbe) {
    let observed = Observed::default();
    let temp = tempfile::tempdir().expect("temp dir");
    let state = wired_state(
        temp.path(),
        ScriptedCouncilClient::new(script(observed.clone(), &probe)),
    );
    let mob_id = MobId::from(format!("clamp-{}", uuid::Uuid::new_v4().simple()));
    state
        .mob_create_definition(parent_definition(&mob_id))
        .await
        .expect("create the mob");
    state
        .mob_spawn(
            &mob_id,
            ProfileName::from("participant"),
            AgentIdentity::from("parent"),
            Some(meerkat_mob::MobRuntimeMode::TurnDriven),
            Some(MobBackendKind::Session),
            None,
        )
        .await
        .expect("spawn the parent");
    let parent_member = state
        .handle_for(&mob_id)
        .await
        .expect("mob handle")
        .member(&AgentIdentity::from("parent"))
        .await
        .expect("member handle");
    let run_turn = |text: &'static str| {
        let parent_member = parent_member.clone();
        async move {
            let turn = parent_member
                .start_turn(
                    ContentInput::Text(text.to_string()),
                    HandlingMode::Queue,
                    meerkat_mob::MemberTurnOptions::default(),
                    None,
                )
                .await
                .expect("turn admitted");
            tokio::time::timeout(Duration::from_secs(120), turn.wait())
                .await
                .expect("the turn completes")
        }
    };

    // Precondition: the family is off for the parent itself, so its own
    // call is an unknown tool.
    let Err(direct) = run_turn(DIRECT_PROBE).await else {
        panic!("the parent's own call to a tool of a family it leaves off fails its turn");
    };
    assert!(
        format!("{direct:?}").contains(&format!("Tool not found: {}", probe.tool)),
        "{} is not mounted for the parent: {direct:?}",
        probe.tool
    );

    run_turn(PROBE)
        .await
        .expect("a refused delegate or helper call does not fail the parent's turn");

    let parent = observed
        .parent
        .lock()
        .unwrap()
        .clone()
        .expect("the parent's probe turn reached its final request");
    // A refused delegate is a correct outcome too: then the helper never ran.
    let helper = observed.helper.lock().unwrap().clone();
    if let Some(helper) = helper {
        let helper_probe = &helper["call-helper-probe"];
        assert!(
            !helper_probe.contains(probe.executed_marker),
            "a delegate helper executed {} from the `{}` family its parent's profile leaves \
             off, by naming the family in an inline tooling profile: {helper_probe}\n\
             delegate result: {}",
            probe.tool,
            probe.family,
            parent["call-delegate"]
        );
    }
    let _ = state.mob_destroy(&mob_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_inline_delegate_profile_cannot_enable_shell_the_parent_leaves_off() {
    assert_helper_cannot_enable_parent_disabled_family(shell_probe()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_inline_delegate_profile_cannot_enable_builtins_the_parent_leaves_off() {
    assert_helper_cannot_enable_parent_disabled_family(builtins_probe()).await;
}

//! A profile-sourced child's tools may not exceed its parent's: a family the
//! parent's profile leaves off stays out of reach for every helper or member
//! the parent spawns, whatever `tooling` the parent's model passes.
//!
//! The parent's profile restriction carries its `deny` names and `read_only`
//! to the child, but a family the profile merely disables is not a deny
//! entry. A model-supplied `tooling: {mode: "profile", source: inline}`
//! replaces the child's profile and so decides what the child mounts; the
//! parent's visible tools cap what the child may dispatch.
//!
//! End to end over the production session service with the real agent mob
//! tools: the parent member's own model turn calls `delegate` or
//! `mob_spawn_member`, and the child's own model turn calls the family's
//! tool, so each call reaches the calling session's outermost execution gate.
#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod support;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use meerkat_client::LlmRequest;
use meerkat_core::{ContentInput, HandlingMode, Message};
use meerkat_mob::{
    AgentIdentity, BoundedResultSpec, MobBackendKind, MobDefinition, MobId, ProfileBinding,
    ProfileName, WorkOrigin, WorkSpec,
};
use meerkat_mob_mcp::MobMcpState;
use support::{ScriptedCouncilClient, ScriptedTurn, last_user_text, participant_profile};

const DIRECT_PROBE: &str = "FAMILY-CLAMP-DIRECT-PROBE";
const PROBE: &str = "FAMILY-CLAMP-PROBE";
const HELPER_TASK: &str = "FAMILY-CLAMP-HELPER-TASK";
const GRANDCHILD_TASK: &str = "FAMILY-CLAMP-GRANDCHILD-TASK";
const WORKER_TASK: &str = "FAMILY-CLAMP-WORKER-TASK";

/// A tool of a family the parent's profile leaves off, and the text its
/// result carries only when the tool actually executed.
#[derive(Clone)]
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

const HOST_NOTE: &str = "host_note";

/// A permitted host tool every member mounts (a mob-wide provider), so the
/// parent sees it and a capped child may call it. Records each call.
struct HostNote(Arc<Mutex<Vec<String>>>);

#[async_trait::async_trait]
impl meerkat_core::AgentToolDispatcher for HostNote {
    fn tools(&self) -> Arc<[Arc<meerkat_core::types::ToolDef>]> {
        Arc::from([Arc::new(
            meerkat_core::types::ToolDef::new(
                HOST_NOTE,
                "Record a note on the host.",
                serde_json::json!({"type": "object", "properties": {}}),
            )
            .with_provenance(meerkat_core::ToolProvenance {
                kind: meerkat_core::ToolSourceKind::Callback,
                source_id: HOST_NOTE.into(),
            }),
        )])
    }

    async fn dispatch(
        &self,
        call: meerkat_core::types::ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        self.0.lock().unwrap().push(call.id.to_string());
        Ok(meerkat_core::ToolDispatchOutcome::sync_result(
            meerkat_core::types::ToolResult::new(call.id.to_string(), "noted".to_string(), false),
        ))
    }
}

/// One member whose profile mounts the agent mob tools (so it may delegate
/// and spawn) and leaves every other family at its default: off, not denied.
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

/// The `tooling` argument of a profile-sourced spawn whose inline profile
/// switches on `families` (plus comms, which every mob member needs).
fn inline_tooling(families: &[&str]) -> serde_json::Value {
    let mut tools = serde_json::Map::new();
    tools.insert("comms".to_string(), true.into());
    for family in families {
        tools.insert((*family).to_string(), true.into());
    }
    serde_json::json!({
        "mode": "profile",
        "source": {
            "type": "inline",
            "model": "claude-haiku-4-5-20251001",
            "tools": tools
        }
    })
}

fn delegate_call(id: &str, task: &str, member_id: &str, families: &[&str]) -> ScriptedTurn {
    ScriptedTurn::ToolCall {
        id: id.to_string(),
        name: "delegate".to_string(),
        args: serde_json::json!({
            "task": task,
            "member_id": member_id,
            "result_label": "helper_result",
            "max_text_bytes": 4096,
            "tooling": inline_tooling(families),
        }),
    }
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

/// The tool results each session saw at the end of its turn, if it got there.
#[derive(Default, Clone)]
struct Observed {
    parent: Seen,
    helper: Seen,
    grandchild: Seen,
    /// One entry per worker turn.
    worker: Arc<Mutex<Vec<BTreeMap<String, String>>>>,
    /// The tools the helper was offered on its first request.
    helper_tools: Arc<Mutex<Option<Vec<String>>>>,
    /// The call ids of the results the helper's model got back, in order.
    helper_result_order: Arc<Mutex<Option<Vec<String>>>>,
}

/// Call `tool` once, then record what came back and answer.
fn probe_once(
    results: BTreeMap<String, String>,
    id: &str,
    probe: &FamilyProbe,
    record: impl FnOnce(BTreeMap<String, String>),
) -> ScriptedTurn {
    if results.contains_key(id) {
        record(results);
        ScriptedTurn::Text("probed".to_string())
    } else {
        ScriptedTurn::ToolCall {
            id: id.to_string(),
            name: probe.tool.to_string(),
            args: probe.args.clone(),
        }
    }
}

/// Every session shares one scripted client; each turn is told apart by the
/// task text it was given. `parent_spawn` makes the parent's probe-turn
/// calls, given the results so far, ending with the one whose id is
/// `call-spawn`.
fn script(
    observed: Observed,
    probe: FamilyProbe,
    parent_spawn: impl Fn(&BTreeMap<String, String>) -> ScriptedTurn + Send + Sync + 'static,
    helper_turn: HelperTurn,
) -> impl Fn(&LlmRequest) -> ScriptedTurn + Send + Sync + 'static {
    move |request| {
        let user = last_user_text(request);
        let results = tool_results(&request.messages);
        if user.contains(GRANDCHILD_TASK) {
            let seen = Arc::clone(&observed.grandchild);
            return probe_once(results, "call-grandchild-probe", &probe, |r| {
                *seen.lock().unwrap() = Some(r);
            });
        }
        if user.contains(HELPER_TASK) {
            observed
                .helper_tools
                .lock()
                .unwrap()
                .get_or_insert_with(|| {
                    request
                        .tools
                        .iter()
                        .map(|tool| tool.name.to_string())
                        .collect()
                });
            let seen = Arc::clone(&observed.helper);
            return match helper_turn {
                HelperTurn::Probe => probe_once(results, "call-helper-probe", &probe, |r| {
                    *seen.lock().unwrap() = Some(r);
                }),
                HelperTurn::ProbeWithSibling { hidden_first } => {
                    if results.contains_key("call-helper-probe") {
                        if let Some(Message::ToolResults { results: last, .. }) =
                            request.messages.last()
                        {
                            *observed.helper_result_order.lock().unwrap() = Some(
                                last.iter()
                                    .map(|result| result.tool_use_id.clone())
                                    .collect(),
                            );
                        }
                        *seen.lock().unwrap() = Some(results);
                        return ScriptedTurn::Text("helper done".to_string());
                    }
                    let hidden = (
                        "call-helper-probe".to_string(),
                        probe.tool.to_string(),
                        probe.args.clone(),
                    );
                    let sibling = (
                        "call-helper-sibling".to_string(),
                        HOST_NOTE.to_string(),
                        serde_json::json!({}),
                    );
                    ScriptedTurn::ToolCalls(if hidden_first {
                        vec![hidden, sibling]
                    } else {
                        vec![sibling, hidden]
                    })
                }
                HelperTurn::DelegateFamily => {
                    if results.contains_key("call-helper-delegate") {
                        *seen.lock().unwrap() = Some(results);
                        ScriptedTurn::Text("helper done".to_string())
                    } else {
                        delegate_call(
                            "call-helper-delegate",
                            GRANDCHILD_TASK,
                            "grandchild",
                            &[probe.family],
                        )
                    }
                }
            };
        }
        if user.contains(WORKER_TASK) {
            // A durable worker sees its earlier turns too: each turn probes
            // under its own call id, and records the result it just got.
            let probes = results
                .keys()
                .filter(|id| id.starts_with("call-worker-probe-"))
                .count();
            let answered_this_turn =
                matches!(request.messages.last(), Some(Message::ToolResults { .. }));
            if answered_this_turn {
                let latest = format!("call-worker-probe-{}", probes - 1);
                let mut turn = BTreeMap::new();
                turn.insert("call-worker-probe".to_string(), results[&latest].clone());
                observed.worker.lock().unwrap().push(turn);
                return ScriptedTurn::Text("probed".to_string());
            }
            return ScriptedTurn::ToolCall {
                id: format!("call-worker-probe-{probes}"),
                name: probe.tool.to_string(),
                args: probe.args.clone(),
            };
        }
        if user.contains(DIRECT_PROBE) {
            return ScriptedTurn::ToolCall {
                id: "call-parent-probe".to_string(),
                name: probe.tool.to_string(),
                args: probe.args.clone(),
            };
        }
        if !user.contains(PROBE) {
            return ScriptedTurn::Text("ok".to_string());
        }
        if results.contains_key("call-spawn") {
            *observed.parent.lock().unwrap() = Some(results);
            return ScriptedTurn::Text("probed".to_string());
        }
        parent_spawn(&results)
    }
}

#[derive(Clone, Copy)]
enum HelperTurn {
    /// The helper calls the family's tool itself.
    Probe,
    /// The helper calls the family's tool and the permitted host tool in
    /// one batch, the family's tool first or second.
    ProbeWithSibling { hidden_first: bool },
    /// The helper delegates on, to a grandchild whose inline profile switches
    /// the family on; the grandchild calls the tool.
    DelegateFamily,
}

/// A mob with one parent member, over the production composition.
struct ParentMob {
    state: Arc<MobMcpState>,
    mob_id: MobId,
    /// Every call that reached the permitted host tool.
    host_calls: Arc<Mutex<Vec<String>>>,
    _temp: tempfile::TempDir,
}

impl ParentMob {
    async fn new(
        mob_id: MobId,
        script: impl Fn(&LlmRequest) -> ScriptedTurn + Send + Sync + 'static,
    ) -> Self {
        let temp = tempfile::tempdir().expect("temp dir");
        let host_calls = Arc::new(Mutex::new(Vec::new()));
        let host: Arc<dyn meerkat_core::AgentToolDispatcher> =
            Arc::new(HostNote(Arc::clone(&host_calls)));
        let state = support::agent_mob_tools_state(
            temp.path(),
            Arc::new(ScriptedCouncilClient::new(script)),
            |state| {
                let provider: meerkat_mob::ExternalToolsProvider =
                    Arc::new(move || Some(Arc::clone(&host)));
                state.with_external_tools_provider(Some(provider))
            },
        );
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
        Self {
            state,
            mob_id,
            host_calls,
            _temp: temp,
        }
    }

    async fn parent_turn(&self, text: &str) -> Result<(), meerkat_mob::MobError> {
        let turn = self
            .state
            .handle_for(&self.mob_id)
            .await
            .expect("mob handle")
            .member(&AgentIdentity::from("parent"))
            .await
            .expect("member handle")
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
            .map(|_| ())
    }

    /// Precondition: the family is off for the parent itself, so its own
    /// call is an unknown tool (which ends that turn).
    async fn assert_family_off_for_parent(&self, probe: &FamilyProbe) {
        let Err(direct) = self.parent_turn(DIRECT_PROBE).await else {
            panic!("the parent's own call to a tool of a family it leaves off fails its turn");
        };
        assert!(
            format!("{direct:?}").contains(&format!("Tool not found: {}", probe.tool)),
            "{} is not mounted for the parent: {direct:?}",
            probe.tool
        );
    }

    /// Run one worker turn to completion.
    async fn worker_turn(&self, mob_id: &MobId) {
        let handle = self.state.handle_for(mob_id).await.expect("mob handle");
        let spec = BoundedResultSpec::new("turn", 4096).expect("bounded result spec");
        let work = handle
            .start_work_for_identity_bounded(
                AgentIdentity::from("worker"),
                WorkSpec::new(
                    ContentInput::Text(WORKER_TASK.to_string()),
                    WorkOrigin::Internal,
                ),
                HandlingMode::Queue,
                spec.clone(),
            )
            .await
            .expect("start a worker turn");
        tokio::time::timeout(Duration::from_secs(120), work.wait_bounded(spec))
            .await
            .expect("the worker turn completes")
            .expect("a refused call does not fail the worker's turn");
    }

    async fn teardown(self) {
        let _ = self.state.mob_destroy(&self.mob_id).await;
    }
}

fn fresh_mob_id() -> MobId {
    MobId::from(format!("clamp-{}", uuid::Uuid::new_v4().simple()))
}

fn seen(slot: &Seen, who: &str) -> BTreeMap<String, String> {
    slot.lock()
        .unwrap()
        .clone()
        .unwrap_or_else(|| panic!("the {who} turn reached its final request"))
}

/// The child's call was refused as that call only: the tool is mounted (its
/// profile switched the family on) but outside its ceiling, so it is hidden
/// from the child, and the child's model is told it was denied. `result` is
/// the tool result the child's model saw.
fn assert_refused_at_dispatch(result: &str, probe: &FamilyProbe, who: &str) {
    assert!(
        !result.contains(probe.executed_marker),
        "a {who} executed {} from the `{}` family its parent's profile leaves off, by naming \
         the family in an inline tooling profile: {result}",
        probe.tool,
        probe.family,
    );
    assert!(
        result.contains("\"error\":\"access_denied\""),
        "the {who}'s {} call is refused as outside its ceiling: {result}",
        probe.tool
    );
}

async fn assert_delegate_helper_is_capped(probe: FamilyProbe, hidden_first: bool) {
    let observed = Observed::default();
    let family = probe.family;
    let mob = ParentMob::new(
        fresh_mob_id(),
        script(
            observed.clone(),
            probe.clone(),
            move |_| delegate_call("call-spawn", HELPER_TASK, "helper", &[family]),
            HelperTurn::ProbeWithSibling { hidden_first },
        ),
    )
    .await;
    mob.assert_family_off_for_parent(&probe).await;
    mob.parent_turn(PROBE)
        .await
        .expect("a refused helper call does not fail the parent's turn");

    let parent = seen(&observed.parent, "parent's probe");
    // The helper keeps what its parent can see (its comms tools) and is
    // offered nothing of the family: capped, not closed.
    let offered = observed
        .helper_tools
        .lock()
        .unwrap()
        .clone()
        .expect("the helper made a request");
    assert!(
        offered.iter().any(|name| name == "send_message"),
        "the helper keeps tools its parent can see: {offered:?}"
    );
    assert!(
        !offered.iter().any(|name| name == probe.tool),
        "the helper is not offered {}: {offered:?}",
        probe.tool
    );
    // The refusal is the helper's own model feedback: its turn carries on
    // to an answer, and the delegate completes.
    let helper = seen(&observed.helper, "helper");
    assert_refused_at_dispatch(&helper["call-helper-probe"], &probe, "delegate helper");
    // The permitted sibling in the same batch ran, exactly once, and its
    // model got its exact result; the results came back in call order.
    assert_eq!(helper["call-helper-sibling"], "noted");
    assert_eq!(
        *mob.host_calls.lock().unwrap(),
        ["call-helper-sibling"],
        "the permitted sibling's effect happened once"
    );
    let expected_order = if hidden_first {
        ["call-helper-probe", "call-helper-sibling"]
    } else {
        ["call-helper-sibling", "call-helper-probe"]
    };
    assert_eq!(
        observed
            .helper_result_order
            .lock()
            .unwrap()
            .clone()
            .unwrap(),
        expected_order,
        "the helper's next request carries both results in call order"
    );
    assert!(
        parent["call-spawn"].contains("\"status\":\"completed\""),
        "the helper ran to completion: {}",
        parent["call-spawn"]
    );
    mob.teardown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_inline_delegate_profile_cannot_enable_shell_the_parent_leaves_off() {
    assert_delegate_helper_is_capped(shell_probe(), true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_capped_helper_runs_a_permitted_sibling_called_before_the_refused_one() {
    assert_delegate_helper_is_capped(shell_probe(), false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_inline_delegate_profile_cannot_enable_builtins_the_parent_leaves_off() {
    assert_delegate_helper_is_capped(builtins_probe(), true).await;
}

/// The cap is transitive: a helper (itself profile-sourced) that delegates
/// on with an inline profile switching the family on is capped by its own
/// visible tools, which its parent's already capped.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_recursive_inline_delegate_profile_stays_within_the_root_parent() {
    let probe = shell_probe();
    let observed = Observed::default();
    let mob = ParentMob::new(
        fresh_mob_id(),
        script(
            observed.clone(),
            probe.clone(),
            // The helper may delegate (`mob`), but not run the family.
            |_| delegate_call("call-spawn", HELPER_TASK, "helper", &["mob"]),
            HelperTurn::DelegateFamily,
        ),
    )
    .await;
    mob.assert_family_off_for_parent(&probe).await;
    mob.parent_turn(PROBE)
        .await
        .expect("a refused grandchild call does not fail the parent's turn");

    // The grandchild's model is told its call was denied and answers; the
    // helper's delegate completes.
    let helper = seen(&observed.helper, "helper");
    let grandchild = seen(&observed.grandchild, "grandchild");
    assert_refused_at_dispatch(&grandchild["call-grandchild-probe"], &probe, "grandchild");
    assert!(
        helper["call-helper-delegate"].contains("\"status\":\"completed\""),
        "the grandchild ran to completion: {}",
        helper["call-helper-delegate"]
    );
    mob.teardown().await;
}

/// `mob_spawn_member` with profile tooling is capped the same way, for a
/// durable member of a mob the parent created.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_inline_spawn_member_profile_cannot_enable_shell_the_parent_leaves_off() {
    let probe = shell_probe();
    let observed = Observed::default();
    let child_mob_id = fresh_mob_id();
    let child = child_mob_id.to_string();
    let mob = ParentMob::new(
        fresh_mob_id(),
        script(
            observed.clone(),
            probe.clone(),
            move |results| {
                if results.contains_key("call-create") {
                    // The created mob is the parent's to manage, so it may
                    // spawn there with explicit tooling.
                    ScriptedTurn::ToolCall {
                        id: "call-spawn".to_string(),
                        name: "mob_spawn_member".to_string(),
                        args: serde_json::json!({
                            "mob_id": child,
                            "profile": "worker",
                            "member_id": "worker",
                            "runtime_mode": "turn_driven",
                            "tooling": inline_tooling(&["shell"]),
                        }),
                    }
                } else {
                    ScriptedTurn::ToolCall {
                        id: "call-create".to_string(),
                        name: "mob_create".to_string(),
                        args: serde_json::json!({ "definition": {
                            "id": child,
                            "profiles": { "worker": {
                                "model": "claude-haiku-4-5-20251001",
                                "tools": { "comms": true }
                            } }
                        } }),
                    }
                }
            },
            HelperTurn::Probe,
        ),
    )
    .await;
    mob.assert_family_off_for_parent(&probe).await;
    mob.parent_turn(PROBE)
        .await
        .expect("the parent's spawn turn");
    let parent = seen(&observed.parent, "parent's probe");
    for call in ["call-create", "call-spawn"] {
        assert!(
            !parent[call].contains("\"error\":\""),
            "the parent's {call} succeeded: {}",
            parent[call]
        );
    }

    mob.worker_turn(&child_mob_id).await;
    // The worker is durable: fresh work in the same session meets the same
    // ceiling and the same local refusal.
    mob.worker_turn(&child_mob_id).await;
    let worker = observed.worker.lock().unwrap().clone();
    assert_eq!(worker.len(), 2, "both worker turns reached their answers");
    for turn in &worker {
        assert_refused_at_dispatch(&turn["call-worker-probe"], &probe, "spawned member");
    }
    let _ = mob.state.mob_destroy(&child_mob_id).await;
    mob.teardown().await;
}

/// The ordinary worker path is untouched: `mob_spawn_member` with a mob
/// definition profile, `auto_wire_parent` and no `tooling` spawns the
/// profile's member as before. It is a role-based spawn, not parent-owned
/// inheritance, so no inherited ceiling is required or handed over.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tooling_absent_spawn_member_spawns_the_definition_profile_as_before() {
    let probe = shell_probe();
    let observed = Observed::default();
    let mob_id = fresh_mob_id();
    let own_mob = mob_id.to_string();
    let mob = ParentMob::new(
        mob_id,
        script(
            observed.clone(),
            probe,
            move |_| ScriptedTurn::ToolCall {
                id: "call-spawn".to_string(),
                name: "mob_spawn_member".to_string(),
                args: serde_json::json!({
                    "mob_id": own_mob,
                    "profile": "participant",
                    "member_id": "worker",
                    "auto_wire_parent": true,
                }),
            },
            HelperTurn::Probe,
        ),
    )
    .await;
    mob.parent_turn(PROBE)
        .await
        .expect("the parent's spawn turn");
    let parent = seen(&observed.parent, "parent's probe");
    assert!(
        !parent["call-spawn"].contains("\"error\":\""),
        "the tooling-absent spawn succeeds: {}",
        parent["call-spawn"]
    );
    mob.state
        .handle_for(&mob.mob_id)
        .await
        .expect("mob handle")
        .member(&AgentIdentity::from("worker"))
        .await
        .expect("the worker is a member of the mob");
    mob.teardown().await;
}

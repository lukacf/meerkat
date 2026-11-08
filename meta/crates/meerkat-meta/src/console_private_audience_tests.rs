//! The private-owner rule at every seam of the console: one resolution of
//! every member a request names, with a hidden member answered by the
//! method's own path for a member that does not exist; role-wide dispatch that
//! never reaches a private member; the members a private member creates
//! private too; and lists, events, panels and prechecks that name no member a
//! caller may not know of, with or without an access configuration.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::sync::Arc;

use meerkat_mob::SpawnMemberSpec;
use serde_json::{Value, json};

use super::console_human_admission_tests::BOB;
use super::console_private_side_chat_tests::{
    PRIVATE_TO, ROOT, TestResult, alice_claims, bearer, bob_claims, enforced_console,
    fork_on_console, fork_with_labels, root_claims, row, runtime, sidebar,
};
use super::console_side_chat_tests::{
    persistent_console_spec, persistent_gateway_runtime_in, seeded_member,
};
use super::console_token_lapse_tests::console_rpc_as;
use super::*;

/// `answer` with `name` replaced by a placeholder, so the answers a method
/// gives two names compare as answers.
fn masked(answer: &Value, name: &str) -> String {
    answer.to_string().replace(name, "<member>")
}

/// The caller scope the console reports for the holder of `bearer`.
async fn caller_scope(app: &Router, bearer: &str) -> String {
    console_rpc_as(app, Some(bearer), "meta/console/whoami", json!({})).await["result"]
        ["caller_scope"]
        .as_str()
        .expect("the caller scope")
        .to_string()
}

/// The labels of `member`'s roster row.
async fn roster_labels(
    handle: &meerkat_mob::MobHandle,
    member: &str,
) -> Result<BTreeMap<String, String>, Box<dyn std::error::Error + Send + Sync>> {
    Ok(handle
        .get_member(&AgentIdentity::from(member))
        .await?
        .ok_or_else(|| format!("{member} is seated"))?
        .labels)
}

/// The requests of every kind of member-scoped console method, each naming
/// `name` where the method takes the member it reads or acts on.
fn member_requests(name: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("meta/get_member", json!({ "member_id": name })),
        ("meta/member_status", json!({ "member_id": name })),
        ("meta/member_health", json!({ "member_id": name })),
        ("meta/status_identity", json!({ "identity": name })),
        ("meta/inspect_identity", json!({ "identity": name })),
        ("meta/console/inspect_identity", json!({ "identity": name })),
        ("meta/identity/resolved_tools", json!({ "identity": name })),
        ("meta/identity/routing_status", json!({ "identity": name })),
        (
            "meta/send_status",
            json!({ "identity": name, "input_id": "input-1" }),
        ),
        ("meta/schedule/list", json!({ "identity": name })),
        (
            "meta/agent_memory/recall",
            json!({ "identity": name, "query": "heron" }),
        ),
        ("meta/retire", json!({ "identity": name })),
        ("meta/retire_member", json!({ "member_id": name })),
        ("meta/respawn", json!({ "identity": name })),
        ("meta/respawn_member", json!({ "member_id": name })),
        ("meta/reload_member", json!({ "member_id": name })),
        ("meta/reset", json!({ "identity": name })),
        ("meta/force_cancel_member", json!({ "member_id": name })),
        (
            "meta/stop_member_run",
            json!({ "member_id": name, "run_id": "run-1" }),
        ),
        (
            "meta/fork_member",
            json!({ "source_member_id": name, "agent_identity": "bob-side" }),
        ),
        (
            "meta/cross_mob/wire_local",
            json!({
                "local_member_id": name,
                "remote_comms_name": "elsewhere/worker/peer",
                "remote_peer_id": "peer-1",
                "remote_address": "inproc://elsewhere",
            }),
        ),
    ]
}

/// A request naming a member hidden from its caller takes the method's own
/// path for a member that does not exist: the answer is the one a name no
/// member holds gets, never a refusal of its own, so nothing tells the caller
/// the member exists. Nothing happens to the hidden member, and its owner
/// still reads it.
#[tokio::test]
async fn a_hidden_member_answers_as_an_absent_one() -> TestResult {
    let (_temp_dir, runtime) = runtime("hidden-answers-as-absent").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let app = console_json_router_with_state(enforced_console(&runtime, None));
    let alice = bearer(alice_claims());
    let bob = bearer(bob_claims());
    let forked = fork_on_console(&app, Some(&alice), "alpha", "alpha-side").await;
    assert!(forked["error"].is_null(), "{forked:#}");

    for ((method, hidden), (_, absent)) in member_requests("alpha-side")
        .into_iter()
        .zip(member_requests("alpha-gone"))
    {
        let hidden_answer = console_rpc_as(&app, Some(&bob), method, hidden).await;
        let absent_answer = console_rpc_as(&app, Some(&bob), method, absent).await;
        assert_eq!(
            masked(&hidden_answer, "alpha-side"),
            masked(&absent_answer, "alpha-gone"),
            "{method} answers a hidden member as an absent one: \
             hidden {hidden_answer:#}, absent {absent_answer:#}"
        );
    }
    assert!(
        runtime
            .handle()
            .get_member(&AgentIdentity::from("alpha-side"))
            .await?
            .is_some(),
        "the side chat is still seated"
    );
    let read = console_rpc_as(
        &app,
        Some(&alice),
        "meta/get_member",
        json!({ "member_id": "alpha-side" }),
    )
    .await;
    assert!(read["error"].is_null(), "its owner reads it: {read:#}");
    Ok(())
}

/// Every method literal of the console's dispatch, and every method of the
/// families it serves, is in the one table of the member fields a console
/// method names, so no method that names a member escapes the resolution
/// every request goes through. A method that names no member is in it with
/// no field.
#[test]
fn every_console_method_is_in_the_member_field_table() {
    fn literals(source: &str, out: &mut BTreeSet<String>) {
        for prefix in ["\"meta/", "\"live/"] {
            let mut rest = source;
            while let Some(at) = rest.find(prefix) {
                let tail = &rest[at + 1..];
                let end = tail.find('"').unwrap_or(0);
                let method = &tail[..end];
                if !method.is_empty()
                    && !method.ends_with('/')
                    && method
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "_/".contains(c))
                {
                    out.insert(method.to_string());
                }
                rest = &tail[end.max(1)..];
            }
        }
    }
    let mut methods = BTreeSet::new();
    for source in [
        include_str!("http_console.rs"),
        include_str!("console_voice.rs"),
        include_str!("http_console/shared_panels.rs"),
        include_str!("http_console/schedules.rs"),
        include_str!("http_console/audit.rs"),
        include_str!("http_console/mcp_apps.rs"),
    ] {
        literals(source, &mut methods);
    }
    // The console's own methods its dispatch names by constant.
    methods.extend(
        [
            crate::rpc::CONSOLE_WHOAMI_METHOD,
            crate::rpc::CONSOLE_REVOKE_SESSIONS_METHOD,
            crate::rpc::CONSOLE_AUDIT_METHOD,
        ]
        .map(str::to_string),
    );
    methods.extend(
        crate::rpc::workgraph_methods::WORKGRAPH_READ_METHODS
            .iter()
            .chain(crate::rpc::workgraph_methods::WORKGRAPH_MUTATE_METHODS)
            .chain(crate::rpc::workgraph_methods::WORKGRAPH_CONSOLE_MUTATE_METHODS)
            .chain(crate::mob_announcements::rpc::ANNOUNCEMENT_METHODS)
            .chain(crate::rpc::mob_methods::MEMBER_DECLARATION_METHODS)
            .map(|method| (*method).to_string()),
    );
    methods.extend(
        [
            crate::rpc::topology_methods::TOPOLOGY_QUERY_METHOD,
            crate::rpc::topology_methods::TOPOLOGY_PLAN_METHOD,
            crate::rpc::topology_methods::TOPOLOGY_APPLY_METHOD,
            crate::rpc::topology_methods::TOPOLOGY_OPERATION_METHOD,
            crate::rpc::topology_methods::TOPOLOGY_AUDIT_METHOD,
        ]
        .map(str::to_string),
    );
    let missing = methods
        .iter()
        .filter(|method| console_method_member_fields(method).is_none())
        .cloned()
        .collect::<Vec<_>>();
    assert!(
        missing.is_empty(),
        "console methods missing from the member field table: {missing:?}"
    );
}

/// A side chat whose alias needs the roster's comms-safe encoding (its
/// source's identity holds `:`) is decided by the member it is, not by the
/// spelling a handler holds: another person's readiness and completion
/// listings name it in no spelling, while its owner's do.
#[tokio::test]
async fn an_encoded_private_side_chat_is_absent_from_readiness_and_completions() -> TestResult {
    let (_temp_dir, runtime) = runtime("encoded-private-side-chat").await?;
    let source = crate::member_comms_id::mob_member_id("router:main");
    Box::pin(
        runtime
            .handle()
            .spawn_spec(SpawnMemberSpec::new("worker", source.clone())),
    )
    .await?;
    let member = runtime.handle().member(&source).await?;
    member
        .start_turn(
            meerkat_core::types::ContentInput::Text("The code word is heron.".to_string()),
            meerkat_core::types::HandlingMode::Queue,
            meerkat_mob::MemberTurnOptions::new(),
            None,
        )
        .await?
        .wait()
        .await?;
    let app = console_json_router_with_state(enforced_console(&runtime, None));
    let alice = bearer(alice_claims());
    let bob = bearer(bob_claims());
    let forked = fork_on_console(&app, Some(&alice), "router:main", "router:main-side").await;
    assert!(forked["error"].is_null(), "{forked:#}");
    let side = crate::member_comms_id::mob_member_id("router:main-side");
    runtime.handle().ensure_member_live(&side).await?;

    let names_side_chat = |answer: &Value| {
        let text = answer.to_string();
        text.contains("router:main-side") || text.contains(side.as_str())
    };
    for method in ["meta/wait_ready", "meta/collect_completed"] {
        let params = json!({ "timeout_ms": 5000 });
        let bobs = console_rpc_as(&app, Some(&bob), method, params.clone()).await;
        assert!(
            !names_side_chat(&bobs),
            "{method} names Alice's side chat to Bob: {bobs:#}"
        );
    }
    let alices = console_rpc_as(
        &app,
        Some(&alice),
        "meta/wait_ready",
        json!({ "timeout_ms": 5000 }),
    )
    .await;
    assert!(
        names_side_chat(&alices),
        "its owner's readiness listing names it: {alices:#}"
    );
    Ok(())
}

/// A mob definition tail with one flow whose only step targets the `worker`
/// role, the role a side chat of a worker is seated in.
const WORKER_FLOW_TAIL: &str = r#"
[flows.main]
description = "One step on the worker role."

[flows.main.steps.work]
role = "worker"
message = "Repeat the code word you were told, if any."

[flows.main.root.nodes.node_work]
kind = "step"
step_id = "work"
depends_on = []
depends_on_mode = "all"
"#;

/// Poll `meta/flow_status` of `run_id` as the holder of `bearer` until the
/// run ends; the last answer.
async fn settled_flow_status(app: &Router, bearer: &str, run_id: &str) -> Value {
    let mut status = Value::Null;
    for _ in 0..200 {
        status = console_rpc_as(
            app,
            Some(bearer),
            "meta/flow_status",
            json!({ "run_id": run_id }),
        )
        .await;
        let state = status["result"]["status"].as_str().unwrap_or_default();
        if matches!(state, "completed" | "failed" | "canceled" | "cancelled") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    status
}

/// Another person's role-wide flow of the side chat's role never reaches the
/// private side chat: its step runs on the role's other members, and the run
/// record names the side chat nowhere.
#[tokio::test]
async fn another_persons_role_wide_flow_never_reaches_a_private_side_chat() -> TestResult {
    let (_temp_dir, spec) =
        persistent_console_spec("private-role-wide-flow", "", WORKER_FLOW_TAIL)?;
    let runtime = Box::pin(MobRuntime::bootstrap(spec)).await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let app = console_json_router_with_state(enforced_console(&runtime, None));
    let alice = bearer(alice_claims());
    let bob = bearer(bob_claims());
    let forked = fork_on_console(&app, Some(&alice), "alpha", "alpha-side").await;
    assert!(forked["error"].is_null(), "{forked:#}");

    let started = console_rpc_as(
        &app,
        Some(&bob),
        "meta/run_flow",
        json!({ "flow_id": "main" }),
    )
    .await;
    let run_id = started["result"]["run_id"]
        .as_str()
        .ok_or_else(|| format!("the flow started: {started:#}"))?
        .to_string();
    let status = settled_flow_status(&app, &bob, &run_id).await;
    assert_eq!(status["result"]["status"], json!("completed"), "{status:#}");
    assert!(
        !status.to_string().contains("alpha-side"),
        "the role-wide run never reached the private side chat: {status:#}"
    );
    let owners = settled_flow_status(&app, &alice, &run_id).await;
    assert!(
        !owners.to_string().contains("alpha-side"),
        "the side chat's own owner reads no step of it either: {owners:#}"
    );
    Ok(())
}

/// A run its owner bound to her private side chat (an explicit role target)
/// is hers: another person's `meta/flow_status` answers it as a run that does
/// not exist, and `meta/list_runs` lists it to her only.
#[tokio::test]
async fn a_run_that_reached_a_hidden_member_is_shown_to_no_one_else() -> TestResult {
    let (_temp_dir, spec) = persistent_console_spec("private-bound-flow", "", WORKER_FLOW_TAIL)?;
    let runtime = Box::pin(MobRuntime::bootstrap(spec)).await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let app = console_json_router_with_state(enforced_console(&runtime, None));
    let alice = bearer(alice_claims());
    let bob = bearer(bob_claims());
    let forked = fork_on_console(&app, Some(&alice), "alpha", "alpha-side").await;
    assert!(forked["error"].is_null(), "{forked:#}");

    let started = console_rpc_as(
        &app,
        Some(&alice),
        "meta/run_flow",
        json!({ "flow_id": "main", "role_targets": { "worker": ["alpha", "alpha-side"] } }),
    )
    .await;
    let run_id = started["result"]["run_id"]
        .as_str()
        .ok_or_else(|| format!("Alice's bound run started: {started:#}"))?
        .to_string();
    let alices = settled_flow_status(&app, &alice, &run_id).await;
    assert!(
        alices.to_string().contains("alpha-side"),
        "its owner reads her run: {alices:#}"
    );

    let bobs = console_rpc_as(
        &app,
        Some(&bob),
        "meta/flow_status",
        json!({ "run_id": run_id }),
    )
    .await;
    let absent = console_rpc_as(
        &app,
        Some(&bob),
        "meta/flow_status",
        json!({ "run_id": "00000000-0000-4000-8000-000000000000" }),
    )
    .await;
    assert_eq!(
        masked(&bobs, &run_id),
        masked(&absent, "00000000-0000-4000-8000-000000000000"),
        "another person reads her run as a run that does not exist"
    );
    let listed = console_rpc_as(&app, Some(&bob), "meta/list_runs", json!({})).await;
    assert!(
        !listed.to_string().contains("alpha-side") && !listed.to_string().contains(&run_id),
        "another person's run listing names neither her run nor her side chat: {listed:#}"
    );
    let listed = console_rpc_as(&app, Some(&alice), "meta/list_runs", json!({})).await;
    assert!(
        listed.to_string().contains(&run_id),
        "her own listing names her run: {listed:#}"
    );
    Ok(())
}

/// The members a private side chat's own turns create in a mob of their own
/// (a `mob_create` mob, and the implicit mob `delegate` runs its helpers in)
/// are private to the side chat's owner, and the shared topology never wires
/// them: each carries the owner on its roster row and is seated exempt from
/// the definition's wiring. Another person sees neither, before and after the
/// gateway restarts; the owner sees both.
#[tokio::test]
async fn members_a_private_side_chat_creates_in_child_mobs_are_private() -> TestResult {
    const MOB: &str = "private-child-mobs";
    let state = tempfile::tempdir()?;
    let runtime = persistent_gateway_runtime_in(state.path(), MOB, "", "").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let app = console_json_router_with_state(enforced_console(&runtime, None));
    let alice = bearer(alice_claims());
    let bob = bearer(bob_claims());
    let alice_scope = caller_scope(&app, &alice).await;
    let forked = fork_on_console(&app, Some(&alice), "alpha", "alpha-side").await;
    assert!(forked["error"].is_null(), "{forked:#}");
    let side_chat = AgentIdentity::from("alpha-side");
    runtime.handle().ensure_member_live(&side_chat).await?;
    let side_session = runtime
        .handle()
        .get_member(&side_chat)
        .await?
        .and_then(|entry| entry.bridge_session_id().cloned())
        .ok_or("the side chat has a session")?;
    let tools = runtime.agent_mob_mcp_state().ok_or("agent mob tools")?;

    // Its `mob_create`: a mob of its own, owned by its session, with a
    // member seated for that session as the agent mob tools seat one.
    let team_id = meerkat_mob::MobId::from("alpha-side-team");
    let team: meerkat_mob::MobDefinition = serde_json::from_value(json!({
        "id": "alpha-side-team",
        "profiles": { "worker": {
            "model": "gpt-5.5",
            "runtime_mode": "turn_driven",
            "tools": { "comms": true }
        } }
    }))?;
    tools
        .mob_create_definition_with_owner_bridge_session(team, side_session.clone(), true, false)
        .await?;
    let team_handle = tools.handle_for(&team_id).await?;
    Box::pin(
        team_handle.spawn_spec_with_generated_owner_context(
            SpawnMemberSpec::new("worker", AgentIdentity::from("sub-helper"))
                .with_runtime_mode(meerkat_mob::MobRuntimeMode::TurnDriven),
            side_session.clone(),
        ),
    )
    .await?;
    // Its `delegate`: the implicit mob of its session, with a helper.
    let implicit = tools
        .get_or_create_implicit_mob_for_bridge_session(&side_session.to_string(), "gpt-5.5")
        .await?;
    let implicit_handle = tools.handle_for(&implicit).await?;
    Box::pin(
        implicit_handle.spawn_spec_with_generated_owner_context(
            SpawnMemberSpec::new("delegate", AgentIdentity::from("delegate-helper"))
                .with_runtime_mode(meerkat_mob::MobRuntimeMode::TurnDriven),
            side_session.clone(),
        ),
    )
    .await?;

    for (handle, member) in [
        (&team_handle, "sub-helper"),
        (&implicit_handle, "delegate-helper"),
    ] {
        let entry = handle
            .get_member(&AgentIdentity::from(member))
            .await?
            .ok_or_else(|| format!("{member} is seated"))?;
        assert_eq!(
            entry.labels.get(PRIVATE_TO),
            Some(&alice_scope),
            "{member} is private to the side chat's owner: {:?}",
            entry.labels
        );
        assert_eq!(
            entry.definition_wiring,
            meerkat_mob::DefinitionWiring::Exempt,
            "{member} is seated outside the definition's wiring"
        );
    }
    for member in ["sub-helper", "delegate-helper"] {
        assert!(
            row(&sidebar(&app, Some(&bob)).await, member).is_none(),
            "another person does not see {member}"
        );
        assert!(
            row(&sidebar(&app, Some(&alice)).await, member).is_some(),
            "its owner sees {member}"
        );
    }

    // The gateway restarts over the same state.
    runtime.handle().shutdown().await?;
    let _ = runtime
        .stand_down_child_mobs(std::time::Instant::now() + std::time::Duration::from_secs(90))
        .await;
    drop(runtime);
    let runtime = persistent_gateway_runtime_in(state.path(), MOB, "", "").await?;
    let app = console_json_router_with_state(enforced_console(&runtime, None));
    for member in ["sub-helper", "delegate-helper"] {
        assert!(
            row(&sidebar(&app, Some(&bob)).await, member).is_none(),
            "after a restart another person still does not see {member}"
        );
        let read = console_rpc_as(
            &app,
            Some(&bob),
            "meta/get_member",
            json!({ "member_id": member }),
        )
        .await;
        let absent = console_rpc_as(
            &app,
            Some(&bob),
            "meta/get_member",
            json!({ "member_id": "nobody-here" }),
        )
        .await;
        assert_eq!(masked(&read, member), masked(&absent, "nobody-here"));
    }
    Ok(())
}

/// Another person's `meta/reset_all` neither closes a private side chat nor
/// names it.
#[tokio::test]
async fn reset_all_leaves_other_peoples_private_side_chats_alone() -> TestResult {
    let (_temp_dir, runtime) = runtime("private-reset-all").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let app = console_json_router_with_state(enforced_console(&runtime, None));
    let alice = bearer(alice_claims());
    let bob = bearer(bob_claims());
    let forked = fork_on_console(&app, Some(&alice), "alpha", "alpha-side").await;
    assert!(forked["error"].is_null(), "{forked:#}");

    let reset = console_rpc_as(&app, Some(&bob), "meta/reset_all", json!({})).await;
    assert!(
        !reset.to_string().contains("alpha-side"),
        "the answer names Alice's side chat nowhere: {reset:#}"
    );
    assert!(
        runtime
            .handle()
            .get_member(&AgentIdentity::from("alpha-side"))
            .await?
            .is_some(),
        "Alice's side chat is still seated: {reset:#}"
    );
    assert!(
        row(&sidebar(&app, Some(&alice)).await, "alpha-side").is_some(),
        "and its owner still sees it"
    );
    Ok(())
}

/// A shared member wired to a private side chat names it in no peer list
/// another person reads: its console row's `wired_to` and the identity
/// inspection's peers leave it out, while the owner's name it.
#[tokio::test]
async fn peer_lists_name_no_hidden_member() -> TestResult {
    let (_temp_dir, runtime) = runtime("private-peer-lists").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let app = console_json_router_with_state(enforced_console(&runtime, None));
    let alice = bearer(alice_claims());
    let bob = bearer(bob_claims());
    let forked = fork_on_console(&app, Some(&alice), "alpha", "alpha-side").await;
    assert!(forked["error"].is_null(), "{forked:#}");
    runtime
        .handle()
        .wire(
            AgentIdentity::from("alpha"),
            AgentIdentity::from("alpha-side"),
        )
        .await?;

    let bobs_row = row(&sidebar(&app, Some(&bob)).await, "alpha")
        .cloned()
        .ok_or("Bob sees alpha")?;
    assert!(
        !bobs_row.to_string().contains("alpha-side"),
        "Bob's row of alpha names the side chat nowhere: {bobs_row:#}"
    );
    let bobs_member = console_rpc_as(
        &app,
        Some(&bob),
        "meta/get_member",
        json!({ "member_id": "alpha" }),
    )
    .await;
    assert!(
        !bobs_member.to_string().contains("alpha-side"),
        "{bobs_member:#}"
    );
    let bobs_inspection = console_rpc_as(
        &app,
        Some(&bob),
        "meta/inspect_identity",
        json!({ "identity": "alpha" }),
    )
    .await;
    assert!(
        !bobs_inspection.to_string().contains("alpha-side"),
        "{bobs_inspection:#}"
    );
    let alices_row = row(&sidebar(&app, Some(&alice)).await, "alpha")
        .cloned()
        .ok_or("Alice sees alpha")?;
    assert!(
        alices_row["wired_to"].to_string().contains("alpha-side"),
        "the owner's row names her side chat among alpha's peers: {alices_row:#}"
    );
    Ok(())
}

/// On a console with sign-in, a non-administrator is shown no identity scope
/// whose member the console does not know (it ended before this process
/// read who it belonged to): the member may have been private to someone.
/// A member the console knows as shared shows.
#[tokio::test]
async fn an_unknown_ended_members_memory_shows_to_no_non_administrator() -> TestResult {
    use crate::AgentMemoryProvider as _;

    let memory_dir = tempfile::tempdir()?;
    let store = crate::memory::SqliteAgentMemoryStore::open(memory_dir.path())?;
    let realm = crate::AgentMemoryConfig::default().realm;
    for identity in ["ghost", "alpha"] {
        store
            .remember(
                &realm,
                &crate::identity_first::AgentIdentity::parse(identity)?,
                crate::NewAgentMemory {
                    title: format!("{identity} note"),
                    body: format!("what {identity} wrote"),
                    tags: Vec::new(),
                },
            )
            .await?;
    }
    let controller = AccessController::audience_only();
    controller.record_agent_attributes(crate::access::AgentResourceAttributes {
        identity: "alpha".to_string(),
        agent_id: Some("alpha".to_string()),
        role: Some("worker".to_string()),
        labels: BTreeMap::new(),
    });
    let bob = controller.view_for_caller(Some(BOB), Some("bob-key"));
    let panel_store: &dyn crate::memory::capabilities::MemoryPanelStore = &store;
    let answer = handle_memory_panel_records(
        Some(panel_store),
        MemoryPanelCaller::own(Some(&bob)),
        &json!({}),
        json!(1),
    )
    .await;
    let text = answer.to_string();
    assert!(
        text.contains("alpha note"),
        "a known shared member shows: {answer:#}"
    );
    assert!(
        !text.contains("ghost note"),
        "a member the console does not know shows to no non-administrator: {answer:#}"
    );
    Ok(())
}

/// On a console with sign-in, a non-administrator attaches no session whose
/// member the console may not show them: a private side chat's session, a
/// session no member owns and a session that does not exist are all answered
/// alike, and nothing is seated.
#[tokio::test]
async fn a_session_a_caller_may_not_know_is_attached_by_no_non_administrator() -> TestResult {
    let (_temp_dir, runtime) = runtime("private-attach-session").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let app = console_json_router_with_state(enforced_console(&runtime, None));
    let alice = bearer(alice_claims());
    let bob = bearer(bob_claims());
    let forked = fork_on_console(&app, Some(&alice), "alpha", "alpha-side").await;
    assert!(forked["error"].is_null(), "{forked:#}");
    let side_chat = AgentIdentity::from("alpha-side");
    runtime.handle().ensure_member_live(&side_chat).await?;
    let side_session = runtime
        .handle()
        .get_member(&side_chat)
        .await?
        .and_then(|entry| entry.bridge_session_id().cloned())
        .ok_or("the side chat has a session")?;
    let missing = meerkat_core::types::SessionId::new();

    let attach = |alias: &'static str, session: String| {
        let app = app.clone();
        let bob = bob.clone();
        async move {
            console_rpc_as(
                &app,
                Some(&bob),
                "meta/attach_existing_session",
                json!({ "role": "worker", "agent_identity": alias, "session_id": session }),
            )
            .await
        }
    };
    let private = attach("bob-reader-1", side_session.to_string()).await;
    let nonexistent = attach("bob-reader-2", missing.to_string()).await;
    assert_eq!(
        masked(&private, &side_session.to_string()).replace("bob-reader-1", "<alias>"),
        masked(&nonexistent, &missing.to_string()).replace("bob-reader-2", "<alias>"),
        "a private side chat's session answers as a session that does not exist"
    );
    for alias in ["bob-reader-1", "bob-reader-2"] {
        assert!(
            runtime
                .handle()
                .get_member(&AgentIdentity::from(alias))
                .await?
                .is_none(),
            "nothing is seated as {alias}"
        );
    }
    Ok(())
}

/// The memory panel lists no proposal a member hidden from the caller wrote.
#[tokio::test]
async fn the_memory_panel_lists_no_proposal_a_hidden_member_wrote() -> TestResult {
    use crate::AgentMemoryProvider as _;
    use crate::memory::records::MemoryAuthor;

    let memory_dir = tempfile::tempdir()?;
    let store = crate::memory::SqliteAgentMemoryStore::open(memory_dir.path())?;
    store
        .propose(
            &crate::MemoryScope::Mob {
                realm: "default".to_string(),
                mob: "team".to_string(),
            },
            crate::NewMemoryRecord {
                kind: crate::MemoryKind::Fact,
                title: "PRIVATE-TITLE: a private member's proposal".to_string(),
                description: "private description".to_string(),
                body: "PRIVATE-BODY".to_string(),
                tags: Vec::new(),
                evidence: Vec::new(),
                verification: None,
            },
            MemoryAuthor::Agent {
                identity: "alpha-side".to_string(),
            },
        )
        .await?;
    let controller = AccessController::audience_only();
    controller.record_agent_attributes(crate::access::AgentResourceAttributes {
        identity: "alpha-side".to_string(),
        agent_id: Some("alpha-side".to_string()),
        role: Some("worker".to_string()),
        labels: BTreeMap::from([(PRIVATE_TO.to_string(), "alice-key".to_string())]),
    });
    let bob = controller.view_for_caller(Some(BOB), Some("bob-key"));
    let panel_store: &dyn crate::memory::capabilities::MemoryPanelStore = &store;
    let answer = handle_memory_panel_proposals(
        Some(panel_store),
        MemoryPanelCaller::own(Some(&bob)),
        &json!({}),
        json!(1),
    )
    .await;
    assert!(
        !answer.to_string().contains("PRIVATE-TITLE") && !answer.to_string().contains("alpha-side"),
        "Bob is shown nothing of the private member's proposal: {answer:#}"
    );
    let alice = controller.view_for_caller(Some("alice@example.test"), Some("alice-key"));
    let answer = handle_memory_panel_proposals(
        Some(panel_store),
        MemoryPanelCaller::own(Some(&alice)),
        &json!({}),
        json!(1),
    )
    .await;
    assert!(
        answer.to_string().contains("PRIVATE-TITLE"),
        "its owner is shown it: {answer:#}"
    );
    Ok(())
}

/// The host's send precheck refuses a target hidden from the viewer exactly
/// as `/console/send` does, on a console without access configuration too.
#[tokio::test]
async fn the_host_send_precheck_refuses_a_hidden_target_as_the_send_does() -> TestResult {
    let (_temp_dir, runtime) = runtime("private-send-precheck").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let state = enforced_console(&runtime, None);
    let app = console_json_router_with_state(state.clone());
    let alice = bearer(alice_claims());
    let forked = fork_on_console(&app, Some(&alice), "alpha", "alpha-side").await;
    assert!(forked["error"].is_null(), "{forked:#}");

    let precheck = ConsoleSendPrecheck::new(state.decisions.clone(), Some(runtime.clone()), None);
    let bob = super::console_human_admission_tests::authenticate(&state, bob_claims())
        .ok_or("Bob authenticates")?;
    let viewer = ConsoleViewerContext {
        principal: bob.principal.clone(),
        service_identity: bob.service_identity,
        access_view: bob.access_view.clone(),
        human: bob.human.clone(),
    };
    let hidden = precheck.check(&viewer, "alpha-side").await;
    let absent = precheck.check(&viewer, "alpha-gone").await;
    assert_eq!(
        format!("{hidden:?}").replace("alpha-side", "<member>"),
        format!("{absent:?}").replace("alpha-gone", "<member>"),
        "the precheck answers a hidden target as an unknown one"
    );
    assert!(hidden.is_err(), "the precheck refuses the hidden target");
    Ok(())
}

/// The dreams panels stay readable beside a private member's memory: the
/// realm's private identity scopes, which dreams no longer cover, withhold
/// no run from another person and tell them nothing about private memory.
#[tokio::test]
async fn dreams_stay_readable_beside_private_memory() -> TestResult {
    use crate::AgentMemoryProvider as _;

    let memory_dir = tempfile::tempdir()?;
    let store = crate::memory::SqliteAgentMemoryStore::open(memory_dir.path())?;
    let realm = crate::AgentMemoryConfig::default().realm;
    for identity in ["alpha", "alpha-side"] {
        store
            .remember(
                &realm,
                &crate::identity_first::AgentIdentity::parse(identity)?,
                crate::NewAgentMemory {
                    title: format!("{identity} note"),
                    body: format!("what {identity} wrote"),
                    tags: Vec::new(),
                },
            )
            .await?;
    }
    let controller = AccessController::audience_only();
    for (identity, labels) in [
        ("alpha", BTreeMap::new()),
        (
            "alpha-side",
            BTreeMap::from([(PRIVATE_TO.to_string(), "alice-key".to_string())]),
        ),
    ] {
        controller.record_agent_attributes(crate::access::AgentResourceAttributes {
            identity: identity.to_string(),
            agent_id: Some(identity.to_string()),
            role: Some("worker".to_string()),
            labels,
        });
    }
    let bob = controller.view_for_caller(Some(BOB), Some("bob-key"));
    let panel_store: &dyn crate::memory::capabilities::MemoryPanelStore = &store;
    assert!(
        memory_panel_realm_fully_readable(panel_store, MemoryPanelCaller::own(Some(&bob)), &realm)
            .await?,
        "a private member's memory withholds no dream from another person"
    );
    for method in ["meta/memory/panel/dreams", "meta/memory/panel/dream_runs"] {
        let answer = match method {
            "meta/memory/panel/dreams" => {
                handle_memory_panel_dreams(
                    Some(panel_store),
                    MemoryPanelCaller::own(Some(&bob)),
                    &json!({}),
                    json!(1),
                )
                .await
            }
            _ => {
                handle_memory_panel_dream_runs(
                    Some(panel_store),
                    MemoryPanelCaller::own(Some(&bob)),
                    &json!({}),
                    json!(1),
                )
                .await
            }
        };
        assert!(
            !answer.to_string().contains("withheld_by_access\":true"),
            "{method} tells another person nothing about private memory: {answer:#}"
        );
    }
    Ok(())
}

/// The member rows of a console without access configuration, and every
/// read that decides a member by them, hide a member private to another
/// person: the agent and structural streams, timeline frames with and
/// without captured provenance, the topology query, the identity records and
/// the memory panel. (Rows with an access configuration pin the same checks
/// under enforcement.)
#[tokio::test]
async fn a_private_member_is_hidden_on_every_read_without_access_configuration() -> TestResult {
    use crate::console_aggregator::{
        ConsoleFrameSource, ConsoleFrameSourceKind, ConsoleFrameStatus, MetaConsoleAggregator,
        NewConsoleFrame,
    };

    let (_temp_dir, runtime) = runtime("private-reads-no-access").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let state = enforced_console(&runtime, None);
    let app = console_json_router_with_state(state.clone());
    let alice_scope = caller_scope(&app, &bearer(alice_claims())).await;
    fork_with_labels(
        &runtime,
        "alpha",
        "alpha-side",
        &[
            (crate::side_chat::SIDE_CHAT_OF_LABEL, "alpha"),
            (PRIVATE_TO, alice_scope.as_str()),
        ],
    )
    .await?;
    let view = |claims: Value| {
        super::console_human_admission_tests::authenticate(&state, claims)
            .map(|auth| auth.audience)
            .expect("the console resolves the caller's audience")
    };
    let (alice, bob) = (view(alice_claims()), view(bob_claims()));
    assert!(!bob.enforced(), "no access configuration is installed");
    prime_access_cache_from_runtime(&runtime, &bob.controller()).await;
    prime_access_cache_from_runtime(&runtime, &alice.controller()).await;

    // The agent stream of the member.
    assert!(
        crate::http_sse::agent_stream_hidden(&bob, "alpha-side", "alpha-side"),
        "the agent stream is refused to another person"
    );
    assert!(!crate::http_sse::agent_stream_hidden(
        &alice,
        "alpha-side",
        "alpha-side"
    ));

    // Timeline frames, with captured provenance and without.
    let entry = runtime
        .handle()
        .get_member(&AgentIdentity::from("alpha-side"))
        .await?
        .ok_or("the side chat is seated")?;
    let side_session = entry
        .bridge_session_id()
        .map(ToString::to_string)
        .ok_or("the side chat's session")?;
    let aggregator = MetaConsoleAggregator::in_memory();
    aggregator.register_runtime_handles_with_policy(
        "private-reads",
        "",
        runtime.clone(),
        None,
        ConsoleEventStore::new(),
        Arc::new(crate::console_aggregator::AllowAllConsoleVisibilityPolicy),
    );
    let provenance = aggregator
        .capture_member_provenance_for_test("private-reads", "alpha-side", &side_session)
        .await
        .ok_or("the owner captures the side chat")?;
    let frame = |key: &str, provenance: Option<_>| NewConsoleFrame {
        id: None,
        dedupe_key: key.to_string(),
        timestamp_ms: 2_000,
        runtime_key: "private-reads".to_string(),
        identity: "alpha-side".to_string(),
        conversation_id: Some("alpha-side".to_string()),
        session_id: Some(side_session.clone()),
        kind: "text_delta".to_string(),
        status: ConsoleFrameStatus::Completed,
        payload: json!({ "delta": "heron" }),
        source: ConsoleFrameSource {
            member_provenance: provenance,
            kind: ConsoleFrameSourceKind::ConsoleEvent,
            source_cursor: None,
        },
        source_event_id: Some(key.to_string()),
        interaction_id: None,
        turn_id: None,
        run_id: None,
        parent_frame_id: None,
        caused_by_frame_id: None,
    };
    let captured = aggregator
        .store()
        .append_if_absent(frame("captured", Some(provenance)))
        .await?
        .frame;
    let uncaptured = aggregator
        .store()
        .append_if_absent(frame("uncaptured", None))
        .await?
        .frame;
    for frame in [&captured, &uncaptured] {
        assert!(!crate::console_aggregator::timeline_frame_visible_to(
            &bob, frame
        ));
        assert!(crate::console_aggregator::timeline_frame_visible_to(
            &alice, frame
        ));
        let decider = crate::console_aggregator::TimelineFrameDecider::new(bob.clone());
        assert!(!decider.visible(frame));
    }
    assert!(!bob.can_view_uncaptured_member("alpha-side"));

    // The topology query and the identity records.
    let topology = crate::topology_control::TopologyRuntimeHandle::new(
        runtime.handle(),
        None,
        Arc::new(tokio::sync::RwLock::new(BTreeSet::new())),
        crate::topology_control::TopologyController::default(),
        None,
        false,
    );
    let queried =
        crate::rpc::topology_methods::handle_query(&topology, json!(1), Some(&bob), false).await;
    let queried = serde_json::to_value(&queried)?;
    assert!(
        queried["error"].is_null() && !queried.to_string().contains("alpha-side"),
        "the topology query names the side chat nowhere: {queried:#}"
    );
    let queried =
        crate::rpc::topology_methods::handle_query(&topology, json!(1), Some(&alice), false).await;
    assert!(
        serde_json::to_value(&queried)?
            .to_string()
            .contains("alpha-side"),
        "its owner's query names it"
    );
    let records = Box::pin(visible_identity_records(&aggregator, Some(&bob))).await?;
    assert!(
        !serde_json::to_string(&records)?.contains("alpha-side"),
        "the identity records name it nowhere"
    );

    // The memory panel's identity scope of it.
    assert!(!memory_panel_identity_visible(
        MemoryPanelCaller::own(Some(&bob)),
        "alpha-side"
    ));
    assert!(memory_panel_identity_visible(
        MemoryPanelCaller::own(Some(&alice)),
        "alpha-side"
    ));
    Ok(())
}

/// Priming records a private member that ended, and that this process knows
/// was private, with its owner: another person's view still hides it.
#[test]
fn priming_records_an_ended_private_member_with_its_owner() {
    let controller = AccessController::audience_only();
    record_live_and_registered_attributes(
        &controller,
        Vec::new(),
        &BTreeMap::new(),
        &BTreeSet::new(),
        &BTreeMap::from([("alpha-side".to_string(), Some("alice-key".to_string()))]),
        "",
    );
    let bob = controller.view_for_caller(Some(BOB), Some("bob-key"));
    let alice = controller.view_for_caller(Some("alice@example.test"), Some("alice-key"));
    assert!(bob.hides_agent("alpha-side"));
    assert!(!alice.hides_agent("alpha-side"));
}

/// A stale runtime alias of a private member is the member it names: hidden
/// from another person, as its current alias is.
#[test]
fn a_stale_runtime_alias_of_a_private_member_is_hidden() {
    let controller = AccessController::audience_only();
    controller.record_agent_attributes(crate::access::AgentResourceAttributes {
        identity: "alpha-side".to_string(),
        agent_id: Some("rt:alpha-side:1".to_string()),
        role: Some("worker".to_string()),
        labels: BTreeMap::from([(PRIVATE_TO.to_string(), "alice-key".to_string())]),
    });
    controller.record_current_runtime_aliases([(
        "alpha-side".to_string(),
        Some("rt:alpha-side:1".to_string()),
    )]);
    let bob = controller.view_for_caller(Some(BOB), Some("bob-key"));
    assert!(bob.hides_agent("rt:alpha-side:1"));
    assert!(bob.hides_agent("rt:alpha-side:0"), "a stale alias too");
    assert!(
        bob.hides_agent(crate::member_comms_id::mob_member_id("alpha-side").as_str()),
        "and the roster's own spelling"
    );
}

/// A Meta fork of a member that carries a side chat's lineage blanks each
/// of those labels on the child the child's own spec does not set, so the
/// child is no side chat of its source's source.
#[tokio::test]
async fn a_meta_fork_blanks_the_lineage_labels_its_source_carries() -> TestResult {
    let (_temp_dir, runtime) = runtime("meta-fork-blanks-lineage").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    fork_with_labels(
        &runtime,
        "alpha",
        "alpha-side",
        &[
            (crate::side_chat::SIDE_CHAT_OF_LABEL, "alpha"),
            (
                crate::side_chat::SIDE_CHAT_CREATOR_LABEL,
                "alice@example.test",
            ),
        ],
    )
    .await?;
    fork_with_labels(&runtime, "alpha-side", "alpha-side-copy", &[]).await?;
    let labels = roster_labels(&runtime.handle(), "alpha-side-copy").await?;
    for key in [
        crate::side_chat::SIDE_CHAT_OF_LABEL,
        crate::side_chat::SIDE_CHAT_CREATOR_LABEL,
    ] {
        assert_eq!(
            labels.get(key).map(String::as_str),
            Some(""),
            "the fork's own row blanks {key}: {labels:?}"
        );
    }
    Ok(())
}

/// The member creation surfaces outside the console refuse a label only
/// Meta writes, naming it, and seat nothing: `UnifiedRuntime::spawn`, the
/// host fork API and the SDK gateway's `meta/ensure_member`. The identity
/// creation policy counts the four among the label keys Meta itself sets.
#[tokio::test]
async fn reserved_labels_are_refused_by_host_and_sdk_member_creation() -> TestResult {
    // `alpha` has had a turn, so it is a fork source the host fork API can
    // copy: a fork that names a reserved label fails on the label alone.
    let (_state, runtime, _app) = signed_in_unified_runtime("reserved-host-labels").await?;
    for key in crate::member_comms_id::RESERVED_AUTHORITY_LABELS {
        let labels = BTreeMap::from([(key.to_string(), "alpha".to_string())]);
        let refused = runtime
            .spawn(
                SpawnMemberSpec::new("worker", AgentIdentity::from("eve"))
                    .with_labels(labels.clone()),
            )
            .await
            .expect_err("UnifiedRuntime::spawn refuses the label");
        assert!(refused.to_string().contains(key), "{key}: {refused}");
        let refused = runtime
            .fork_member(
                "alpha",
                SpawnMemberSpec::new("worker", AgentIdentity::from("eve")).with_labels(labels),
                None,
            )
            .await
            .expect_err("the host fork API refuses the label");
        assert!(
            matches!(refused, crate::fork::ForkMemberError::InvalidRequest(ref message) if message.contains(key)),
            "{key}: {refused}"
        );
        assert!(
            crate::identity_creation::META_RESERVED_LABEL_KEYS.contains(&key),
            "the identity creation policy counts {key} among Meta's own label keys"
        );
        let request = json!({
            "jsonrpc": "2.0",
            "id": "reserved",
            "method": "meta/ensure_member",
            "params": { "role": "worker", "agent_identity": "eve", "labels": { key: "alpha" } },
        })
        .to_string();
        let raw = crate::rpc::handle_unified_rpc_json(
            &runtime,
            &request,
            std::time::Duration::from_secs(5),
            None,
            None,
        )
        .await;
        let answer: Value = serde_json::from_str(&raw)?;
        assert_eq!(answer["error"]["code"], json!(-32602), "{key}: {answer:#}");
        assert!(
            answer["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains(key)),
            "the refusal names {key}: {answer:#}"
        );
    }
    assert!(
        runtime
            .mob_handle()
            .get_member(&AgentIdentity::from("eve"))
            .await?
            .is_none(),
        "nothing is seated"
    );
    Ok(())
}

/// A unified runtime with persistent sessions, one worker `alpha` that has
/// had a turn, and its reference app with sign-in for Alice, Bob and root.
async fn signed_in_unified_runtime(
    mob_id: &str,
) -> Result<
    (tempfile::TempDir, Arc<crate::UnifiedRuntime>, Router),
    Box<dyn std::error::Error + Send + Sync>,
> {
    let state_dir = tempfile::tempdir()?;
    let runtime = Arc::new(
        crate::UnifiedRuntime::builder()
            .definition(meerkat_mob::MobDefinition::from_toml(&format!(
                "[mob]\nid = '{mob_id}'\n[profiles.worker]\nmodel = 'gpt-5.5'\n\
                 runtime_mode = 'turn_driven'\nexternal_addressable = true\n\
                 [profiles.worker.tools]\ncomms = true\n"
            ))?)
            .persistent_state(state_dir.path())
            .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
            .build()
            .await?,
    );
    runtime
        .spawn(SpawnMemberSpec::new("worker", AgentIdentity::from("alpha")))
        .await?;
    runtime
        .mob_handle()
        .member(&AgentIdentity::from("alpha"))
        .await?
        .start_turn(
            meerkat_core::types::ContentInput::Text("The code word is heron.".to_string()),
            meerkat_core::types::HandlingMode::Queue,
            meerkat_mob::MemberTurnOptions::new(),
            None,
        )
        .await?
        .wait()
        .await?;
    let mut decisions = super::console_human_admission_tests::state().decisions;
    decisions.auth.email_allowlist.extend([
        "alice@example.test".to_string(),
        "root@example.test".to_string(),
    ]);
    let app = runtime.build_reference_app_router(decisions);
    Ok((state_dir, runtime, app))
}

/// A wiring event that names a member hidden from the caller reaches no one
/// else: the mob event query drops an event one of whose endpoints the
/// caller's audience hides, and the owner still reads it.
#[tokio::test]
async fn wiring_events_naming_a_hidden_member_reach_no_one_else() -> TestResult {
    let (_state, runtime, app) = signed_in_unified_runtime("private-wiring-events").await?;
    let alice = bearer(alice_claims());
    let bob = bearer(bob_claims());
    let forked = fork_on_console(&app, Some(&alice), "alpha", "alpha-side").await;
    assert!(forked["error"].is_null(), "{forked:#}");
    runtime
        .mob_handle()
        .wire(
            AgentIdentity::from("alpha"),
            AgentIdentity::from("alpha-side"),
        )
        .await?;

    let bobs = console_rpc_as(&app, Some(&bob), "meta/mob_events/query", json!({})).await;
    assert!(bobs["error"].is_null(), "{bobs:#}");
    assert!(
        !bobs.to_string().contains("alpha-side"),
        "no event Bob reads names Alice's side chat: {bobs:#}"
    );
    let alices = console_rpc_as(&app, Some(&alice), "meta/mob_events/query", json!({})).await;
    assert!(
        alices.to_string().contains("alpha-side"),
        "its owner reads the events of her side chat: {alices:#}"
    );
    Ok(())
}

/// An announcement subscription names the member it delivers into: a
/// request naming a member hidden from its caller is refused as a request
/// naming an unknown member, before the announcement owner is asked, so no
/// one subscribes another person's private member to anything.
#[tokio::test]
async fn announcement_requests_naming_a_hidden_member_are_refused_as_unknown() -> TestResult {
    let (_temp_dir, runtime) = runtime("private-announcements").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let app = console_json_router_with_state(enforced_console(&runtime, None));
    let alice = bearer(alice_claims());
    let bob = bearer(bob_claims());
    let forked = fork_on_console(&app, Some(&alice), "alpha", "alpha-side").await;
    assert!(forked["error"].is_null(), "{forked:#}");

    let refused = console_rpc_as(
        &app,
        Some(&bob),
        "meta/mob_announcements/subscriptions/put",
        json!({
            "operation_id": "op-bob",
            "subscription_id": "sub-bob",
            "member": { "mob_id": "private-announcements", "identity": "alpha-side" },
            "topics": ["news"],
        }),
    )
    .await;
    assert_eq!(refused["error"]["code"], json!(-32001), "{refused:#}");
    assert_eq!(
        refused["error"]["message"],
        json!("unknown identity: alpha-side"),
        "{refused:#}"
    );
    Ok(())
}

/// The announcement listings name no member hidden from their caller: the
/// subscription, delivery and audit rows of such a member are left out, and
/// a shared member's stay.
#[test]
fn announcement_listings_name_no_hidden_member() {
    let controller = AccessController::audience_only();
    controller.record_agent_attributes(crate::access::AgentResourceAttributes {
        identity: "alpha-side".to_string(),
        agent_id: Some("alpha-side".to_string()),
        role: Some("worker".to_string()),
        labels: BTreeMap::from([(PRIVATE_TO.to_string(), "alice-key".to_string())]),
    });
    controller.record_agent_attributes(crate::access::AgentResourceAttributes {
        identity: "alpha".to_string(),
        agent_id: Some("alpha".to_string()),
        role: Some("worker".to_string()),
        labels: BTreeMap::new(),
    });
    let bob = controller.view_for_caller(Some(BOB), Some("bob-key"));
    let member = |identity: &str| json!({ "mob_id": "team", "identity": identity });
    for (method, mut result, key) in [
        (
            "meta/mob_announcements/subscriptions/list",
            json!({ "subscriptions": [
                { "id": "sub-private", "member": member("alpha-side") },
                { "id": "sub-shared", "member": member("alpha") },
            ] }),
            "subscriptions",
        ),
        (
            "meta/mob_announcements/deliveries/list",
            json!({ "deliveries": [
                { "batch_id": "b-private", "member": member("alpha-side") },
                { "batch_id": "b-shared", "member": member("alpha") },
            ] }),
            "deliveries",
        ),
        (
            "meta/mob_announcements/audit/list",
            json!({ "entries": [
                { "action": "subscribed", "member": member("alpha-side") },
                { "action": "subscribed", "member": member("alpha") },
            ] }),
            "entries",
        ),
    ] {
        retain_announcement_rows_visible_to(&bob, method, &mut result);
        let rows = result[key].as_array().cloned().unwrap_or_default();
        assert_eq!(rows.len(), 1, "{method}: {result:#}");
        assert!(
            !result.to_string().contains("alpha-side"),
            "{method} names no hidden member: {result:#}"
        );
    }
}

/// A WorkGraph goal records the member it aims at, so another person may not
/// aim one at a member hidden from them: the request is refused as one
/// naming an unknown member, and no goal targets the hidden member.
#[tokio::test]
async fn a_workgraph_goal_aimed_at_a_hidden_member_targets_no_one() -> TestResult {
    let (_state, runtime, app) = signed_in_unified_runtime("private-workgraph-goal").await?;
    let alice = bearer(alice_claims());
    let bob = bearer(bob_claims());
    let forked = fork_on_console(&app, Some(&alice), "alpha", "alpha-side").await;
    assert!(forked["error"].is_null(), "{forked:#}");

    let refused = console_rpc_as(
        &app,
        Some(&bob),
        "meta/workgraph/goal/create",
        json!({
            "title": "Bob's goal for alpha-side",
            "target": { "kind": "identity", "identity": "alpha-side" },
        }),
    )
    .await;
    assert_eq!(refused["error"]["code"], json!(-32001), "{refused:#}");
    assert_eq!(
        refused["error"]["message"],
        json!("unknown identity: alpha-side"),
        "{refused:#}"
    );
    let items = runtime
        .workgraph_service()
        .ok_or("the runtime keeps a work graph")?
        .list(meerkat::WorkItemFilter::default())
        .await?;
    assert!(
        !serde_json::to_string(&items)?.contains("Bob's goal for alpha-side"),
        "no goal targets the hidden member"
    );
    Ok(())
}

/// A console with no local runtime primes the caller's audience from the
/// runtimes it aggregates before it decides a timeline frame: the owner of a
/// private member reads its frames, another person does not.
#[tokio::test]
async fn an_aggregator_console_primes_before_it_decides_timeline_frames() -> TestResult {
    use crate::console_aggregator::{
        ConsoleFrameSource, ConsoleFrameSourceKind, ConsoleFrameStatus, MetaConsoleAggregator,
        NewConsoleFrame,
    };
    use tower::ServiceExt;

    let (_temp_dir, runtime) = runtime("aggregator-priming").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let alice_scope = caller_scope(
        &console_json_router_with_state(enforced_console(&runtime, None)),
        &bearer(alice_claims()),
    )
    .await;
    fork_with_labels(
        &runtime,
        "alpha",
        "alpha-side",
        &[
            (crate::side_chat::SIDE_CHAT_OF_LABEL, "alpha"),
            (PRIVATE_TO, alice_scope.as_str()),
        ],
    )
    .await?;
    let aggregator = MetaConsoleAggregator::in_memory();
    aggregator.register_runtime_handles_with_policy(
        "agg",
        "",
        runtime.clone(),
        None,
        ConsoleEventStore::new(),
        Arc::new(crate::console_aggregator::AllowAllConsoleVisibilityPolicy),
    );
    aggregator
        .store()
        .append_if_absent(NewConsoleFrame {
            id: None,
            dedupe_key: "uncaptured-private".to_string(),
            timestamp_ms: 2_000,
            runtime_key: "agg".to_string(),
            identity: "alpha-side".to_string(),
            conversation_id: Some("alpha-side".to_string()),
            session_id: None,
            kind: "text_delta".to_string(),
            status: ConsoleFrameStatus::Completed,
            payload: json!({ "delta": "PRIVATE-DELTA" }),
            source: ConsoleFrameSource {
                member_provenance: None,
                kind: ConsoleFrameSourceKind::ConsoleEvent,
                source_cursor: None,
            },
            source_event_id: Some("uncaptured-private".to_string()),
            interaction_id: None,
            turn_id: None,
            run_id: None,
            parent_frame_id: None,
            caused_by_frame_id: None,
        })
        .await?;
    let mut state = enforced_console(&runtime, None);
    state.runtime = None;
    state.console_aggregator = Some(aggregator);
    let app = console_json_router_with_state(state);
    let timeline = |bearer: String| {
        let app = app.clone();
        async move {
            let response = app
                .oneshot(
                    axum::http::Request::builder()
                        // The member's own timeline: a frame of a member the
                        // console's identity records do not list is read only
                        // by a query for its identity.
                        .uri("/console/timeline?identity=alpha-side")
                        .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
                        .body(axum::body::Body::empty())
                        .expect("timeline request"),
                )
                .await
                .expect("timeline response");
            let body = axum::body::to_bytes(response.into_body(), 1 << 22)
                .await
                .expect("timeline body");
            String::from_utf8_lossy(&body).into_owned()
        }
    };
    assert!(
        !timeline(bearer(bob_claims()))
            .await
            .contains("PRIVATE-DELTA"),
        "another person reads no frame of the private member"
    );
    assert!(
        timeline(bearer(alice_claims()))
            .await
            .contains("PRIVATE-DELTA"),
        "its owner reads it: the console primed her audience first"
    );
    Ok(())
}

/// The voice preface of a member names no peer private to a person: the
/// voice model would tell whoever speaks the names of private side chats
/// that are not theirs.
#[cfg(feature = "openai-live")]
#[tokio::test]
async fn the_voice_preface_names_no_private_peer() -> TestResult {
    let (_temp_dir, runtime) = runtime("private-voice-preface").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    fork_with_labels(
        &runtime,
        "alpha",
        "alpha-side",
        &[
            (crate::side_chat::SIDE_CHAT_OF_LABEL, "alpha"),
            (PRIVATE_TO, "console-caller:v1:aa"),
        ],
    )
    .await?;
    runtime
        .handle()
        .ensure_member_live(&AgentIdentity::from("alpha"))
        .await?;
    let session = runtime
        .handle()
        .get_member(&AgentIdentity::from("alpha"))
        .await?
        .and_then(|entry| entry.bridge_session_id().cloned())
        .ok_or("alpha has a session")?;
    let preface = crate::console_voice::capabilities::MemberPreface::new(runtime.handle())
        .preface_for_session(&session)
        .await
        .ok_or("alpha has a preface")?;
    assert!(
        !preface.contains("alpha-side"),
        "the preface names no private peer: {preface}"
    );
    Ok(())
}

/// One record set of the console audit log (`changes` or `tool_calls`), as
/// the holder of `bearer` reads it.
async fn audit_records(app: &Router, bearer: &str, log: &str) -> Vec<Value> {
    let read = console_rpc_as(
        app,
        Some(bearer),
        crate::rpc::CONSOLE_AUDIT_METHOD,
        json!({ "log": log, "limit": 500 }),
    )
    .await;
    read["result"]["records"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("the audit log answers: {read:#}"))
}

/// An MCP app tool call on `identity` on the console route, as the holder of
/// `bearer`: the console refuses it (no app serves the member), and the call
/// writes its row all the same.
async fn call_tool_on(app: &Router, bearer: &str, identity: &str) {
    use tower::ServiceExt;
    let body = json!({
        "identity": identity,
        "sessionId": meerkat_core::types::SessionId::new().to_string(),
        "toolCallId": format!("call-{identity}"),
        "name": "file_ticket",
        "arguments": {},
    });
    let response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/console/mcp-apps/call-tool")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
                .body(axum::body::Body::from(body.to_string()))
                .expect("tool call request"),
        )
        .await
        .expect("tool call response");
    let _ = axum::body::to_bytes(response.into_body(), 1 << 20).await;
}

/// The console audit log names no member a reader's audience hides: the
/// change rows (a private side chat's fork, a change on it) and the MCP app
/// tool call row whose member is another person's private side chat are left
/// out of the read, with or without an access configuration, and the rows
/// that name a member the reader may know stay. The side chat's owner, and an
/// access admin, read every row.
#[tokio::test]
async fn the_audit_read_leaves_out_rows_naming_a_hidden_member() -> TestResult {
    let (_temp_dir, runtime) = runtime("private-audit-rows").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let store: Arc<dyn crate::runtime::PersistentMetadataStore> =
        Arc::new(crate::runtime::InMemoryMetadataStore::new());
    let log = Arc::new(
        crate::console_audit::ConsoleAuditLog::over(
            store,
            crate::console_audit::ConsoleAuditLimits::default(),
        )
        .ok_or("the store keeps realm documents")?,
    );
    let audited = |access: Option<AccessController>| {
        console_json_router_with_state(ConsoleJsonState {
            console_audit: Some(Arc::clone(&log)),
            ..enforced_console(&runtime, access)
        })
    };
    let unconfigured = audited(None);
    let enforced = audited(Some(
        AccessController::new(crate::access::AccessControlConfig {
            enabled: true,
            admins: vec![ROOT.to_string()],
            rules: vec![
                AccessRule {
                    id: "everyone-drives-every-agent".to_string(),
                    subjects: vec!["*".to_string()],
                    actions: vec!["agent.*".to_string()],
                    agents: vec!["*".to_string()],
                    ..AccessRule::default()
                },
                AccessRule {
                    id: "everyone-reads-the-audit".to_string(),
                    subjects: vec!["*".to_string()],
                    actions: vec!["audit.view".to_string()],
                    ..AccessRule::default()
                },
            ],
            ..crate::access::AccessControlConfig::default()
        })
        .map_err(|error| format!("{error:?}"))?,
    ));
    let alice = bearer(alice_claims());
    let bob = bearer(bob_claims());
    let root = bearer(root_claims());

    let forked = fork_on_console(&unconfigured, Some(&alice), "alpha", "alpha-side").await;
    assert!(forked["error"].is_null(), "{forked:#}");
    for identity in ["alpha-side", "alpha"] {
        let _ = console_rpc_as(
            &unconfigured,
            Some(&alice),
            "meta/reset",
            json!({ "identity": identity }),
        )
        .await;
        call_tool_on(&unconfigured, &alice, identity).await;
    }

    let naming_side_chat = |rows: &[Value]| -> Vec<Value> {
        rows.iter()
            .filter(|row| row.to_string().contains("alpha-side"))
            .cloned()
            .collect()
    };
    let naming_alpha_only = |rows: &[Value]| -> usize {
        rows.iter()
            .filter(|row| row["target"]["identity"] == json!("alpha"))
            .count()
    };
    for (posture, app) in [("unconfigured", &unconfigured), ("enforced", &enforced)] {
        let changes = audit_records(app, &bob, "changes").await;
        let calls = audit_records(app, &bob, "tool_calls").await;
        assert_eq!(
            naming_side_chat(&changes),
            Vec::<Value>::new(),
            "{posture}: Bob reads no change row naming Alice's side chat"
        );
        assert_eq!(
            naming_side_chat(&calls),
            Vec::<Value>::new(),
            "{posture}: Bob reads no tool call row naming Alice's side chat"
        );
        assert_eq!(
            naming_alpha_only(&changes),
            1,
            "{posture}: the change on alpha stays: {changes:#?}"
        );
        assert_eq!(
            naming_alpha_only(&calls),
            1,
            "{posture}: the tool call on alpha stays: {calls:#?}"
        );
    }
    // The fork may be asked again while alpha settles its last turn: each
    // ask writes a row, so the reads are compared by method.
    let methods = |rows: &[Value]| -> BTreeSet<String> {
        naming_side_chat(rows)
            .iter()
            .filter_map(|row| row["method"].as_str().map(str::to_string))
            .collect()
    };
    for (reader, app, bearer) in [
        ("the owner", &unconfigured, &alice),
        ("an access admin", &enforced, &root),
    ] {
        let changes = audit_records(app, bearer, "changes").await;
        let calls = audit_records(app, bearer, "tool_calls").await;
        assert_eq!(
            methods(&changes),
            BTreeSet::from(["meta/fork_member".to_string(), "meta/reset".to_string()]),
            "{reader} reads the fork and the change on the side chat: {changes:#?}"
        );
        assert_eq!(
            naming_side_chat(&calls).len(),
            1,
            "{reader} reads the tool call on the side chat: {calls:#?}"
        );
    }
    Ok(())
}

/// A private side chat's stored session names its private owner, which is
/// what keeps the memory distiller out of its transcript: the distiller reads
/// the session's stored member labels (`TranscriptSource::private_to_a_person`)
/// and distills no session private to a person, whatever asks for it (a run's
/// end, a retire or reset boundary, a compaction harvest;
/// `distiller_skips_private_side_chats`), so nothing said in it reaches any
/// identity's memory, the agent's it was forked from included. The agent's own
/// session is distilled as before. The row reads the store the runtime
/// persists its sessions through, the one a runtime with agent memory hands
/// its distiller, which holds the side chat's transcript.
#[tokio::test]
async fn a_private_side_chats_stored_session_keeps_the_distiller_out() -> TestResult {
    use super::console_side_chat_tests::persistent_gateway_runtime_and_store_in;
    use crate::memory::distiller::TranscriptSource as _;

    let state = tempfile::tempdir()?;
    let (runtime, store) = persistent_gateway_runtime_and_store_in(
        state.path(),
        "private-side-chat-distiller",
        "",
        "",
    )
    .await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let app = console_json_router_with_state(enforced_console(&runtime, None));
    let forked = fork_on_console(&app, Some(&bearer(alice_claims())), "alpha", "alpha-side").await;
    assert!(forked["error"].is_null(), "{forked:#}");
    let side_chat = AgentIdentity::from("alpha-side");
    runtime.handle().ensure_member_live(&side_chat).await?;
    runtime
        .handle()
        .member(&side_chat)
        .await?
        .start_turn(
            meerkat_core::types::ContentInput::Text("My salary is private.".to_string()),
            meerkat_core::types::HandlingMode::Queue,
            meerkat_mob::MemberTurnOptions::new(),
            None,
        )
        .await?
        .wait()
        .await?;
    let mut sessions = Vec::new();
    for member in ["alpha", "alpha-side"] {
        let session = runtime
            .handle()
            .get_member(&AgentIdentity::from(member))
            .await?
            .and_then(|entry| entry.bridge_session_id().cloned())
            .ok_or_else(|| format!("{member} has a session"))?;
        sessions.push(session.to_string());
    }
    let transcripts = crate::memory::distiller::SessionStoreTranscriptSource::new(store);
    let said = transcripts
        .read(&sessions[1], 0)
        .await?
        .ok_or("the store holds the side chat's session")?;
    assert!(
        said.messages
            .iter()
            .any(|message| message.text.contains("My salary is private.")),
        "the store holds what was said in the side chat: {said:?}"
    );
    assert!(
        !transcripts.private_to_a_person(&sessions[0]).await?,
        "the agent's own session is distilled as before"
    );
    assert!(
        transcripts.private_to_a_person(&sessions[1]).await?,
        "the side chat's stored session names its private owner"
    );
    Ok(())
}

/// The members every dispatch of flow run `run_id` targeted, from the mob's
/// own per-target dispatch records.
async fn dispatched_members(
    handle: &meerkat_mob::MobHandle,
    run_id: &str,
) -> Result<BTreeSet<String>, Box<dyn std::error::Error + Send + Sync>> {
    let mut members = BTreeSet::new();
    let mut cursor = 0;
    loop {
        let events = handle.poll_events(cursor, 256).await?;
        let Some(last) = events.last().map(|event| event.cursor) else {
            break;
        };
        for event in events {
            if let meerkat_mob::MobEventKind::StepDispatched {
                run_id: dispatched,
                target,
                ..
            } = event.kind
                && dispatched.to_string() == run_id
            {
                members.insert(target.identity.to_string());
            }
        }
        cursor = last;
    }
    Ok(members)
}

/// `meta/run_flow` of the definition's `main` flow as the holder of
/// `bearer`; the run's id.
async fn start_main_flow(
    app: &Router,
    bearer: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let started = console_rpc_as(
        app,
        Some(bearer),
        "meta/run_flow",
        json!({ "flow_id": "main" }),
    )
    .await;
    Ok(started["result"]["run_id"]
        .as_str()
        .ok_or_else(|| format!("the flow started: {started:#}"))?
        .to_string())
}

/// A fork Meerkat makes of a private side chat (as the side chat's own
/// `fork_off` seats its child: a default seating through the durable fork,
/// which asks no spawn customizer) takes the side chat's owner and its
/// exemption from the definition's wiring. It is seated exempt, the
/// definition wires it to no shared member, and another person's role-wide
/// flow of its role never dispatches to it.
#[tokio::test]
async fn a_fork_of_a_private_side_chat_stays_out_of_the_shared_topology() -> TestResult {
    let tail = format!("[wiring]\nauto_wire_orchestrator = true\n{WORKER_FLOW_TAIL}");
    let (_temp_dir, spec) =
        persistent_console_spec("private-side-chat-fork", "orchestrator = \"lead\"", &tail)?;
    let runtime = Box::pin(MobRuntime::bootstrap(spec)).await?;
    seeded_member(&runtime, "lead", "lead", &[]).await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let app = console_json_router_with_state(enforced_console(&runtime, None));
    let alice = bearer(alice_claims());
    let bob = bearer(bob_claims());
    let alice_scope = caller_scope(&app, &alice).await;
    let forked = fork_on_console(&app, Some(&alice), "alpha", "alpha-side").await;
    assert!(forked["error"].is_null(), "{forked:#}");
    let side = AgentIdentity::from("alpha-side");
    runtime.handle().ensure_member_live(&side).await?;
    runtime
        .handle()
        .member(&side)
        .await?
        .start_turn(
            meerkat_core::types::ContentInput::Text("My salary is private.".to_string()),
            meerkat_core::types::HandlingMode::Queue,
            meerkat_mob::MemberTurnOptions::new(),
            None,
        )
        .await?
        .wait()
        .await?;

    let child = AgentIdentity::from("alpha-side-child");
    let mut seated = false;
    for _ in 0..50 {
        match runtime
            .handle()
            .fork_member(&side, SpawnMemberSpec::new("worker", child.clone()), None)
            .await
        {
            Ok(_) => {
                seated = true;
                break;
            }
            Err(meerkat_mob::MobError::ForkSourceUnavailable { .. }) => {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            Err(error) => return Err(error.into()),
        }
    }
    assert!(seated, "the side chat settles for its fork");
    let entry = runtime
        .handle()
        .get_member(&child)
        .await?
        .ok_or("the fork is seated")?;
    assert_eq!(
        entry.labels.get(PRIVATE_TO),
        Some(&alice_scope),
        "the fork is private to the side chat's owner"
    );
    assert_eq!(
        entry.definition_wiring,
        meerkat_mob::DefinitionWiring::Exempt,
        "the fork is seated outside the shared topology"
    );
    for shared in ["lead", "alpha"] {
        assert!(
            !entry.wired_to.iter().any(|peer| peer.as_str() == shared),
            "the fork is wired to no shared member: {:?}",
            entry.wired_to
        );
        let shared_entry = runtime
            .handle()
            .get_member(&AgentIdentity::from(shared))
            .await?
            .ok_or("the shared member is seated")?;
        assert!(
            !shared_entry
                .wired_to
                .iter()
                .any(|peer| peer.as_str() == "alpha-side-child"),
            "{shared} is not wired to the fork: {:?}",
            shared_entry.wired_to
        );
    }

    let run_id = start_main_flow(&app, &bob).await?;
    let status = settled_flow_status(&app, &bob, &run_id).await;
    assert_eq!(status["result"]["status"], json!("completed"), "{status:#}");
    assert_eq!(
        dispatched_members(&runtime.handle(), &run_id).await?,
        BTreeSet::from(["alpha".to_string()]),
        "another person's role-wide step dispatched to the shared worker only"
    );
    Ok(())
}

/// A run's members are every member it dispatched to, as the mob's
/// per-target dispatch records name them. A member a host seated private to
/// Alice and left under the definition's wiring is a target of Bob's
/// role-wide flow, so that run reached a member hidden from Bob: Bob reads it
/// as a run that does not exist (no status, a cancel answered as for an
/// absent run, no listing), and Alice reads it whole.
#[tokio::test]
async fn a_role_wide_run_that_reached_a_private_member_is_absent_to_another_person() -> TestResult {
    let (_temp_dir, spec) = persistent_console_spec("private-run-targets", "", WORKER_FLOW_TAIL)?;
    let runtime = Box::pin(MobRuntime::bootstrap(spec)).await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let app = console_json_router_with_state(enforced_console(&runtime, None));
    let alice = bearer(alice_claims());
    let bob = bearer(bob_claims());
    let alice_scope = caller_scope(&app, &alice).await;
    seeded_member(
        &runtime,
        "worker",
        "alpha-secret",
        &[(PRIVATE_TO, alice_scope.as_str())],
    )
    .await?;
    let secret = runtime
        .handle()
        .get_member(&AgentIdentity::from("alpha-secret"))
        .await?
        .ok_or("the private member is seated")?;
    assert_eq!(
        secret.definition_wiring,
        meerkat_mob::DefinitionWiring::Applies,
        "the host left the private member under the definition's wiring"
    );

    let run_id = start_main_flow(&app, &bob).await?;
    let alices = settled_flow_status(&app, &alice, &run_id).await;
    assert_eq!(alices["result"]["status"], json!("completed"), "{alices:#}");
    assert!(
        dispatched_members(&runtime.handle(), &run_id)
            .await?
            .contains("alpha-secret"),
        "the role-wide run reached the private member"
    );
    assert!(
        alices.to_string().contains("alpha-secret"),
        "its owner reads the run whole: {alices:#}"
    );

    const ABSENT_RUN: &str = "00000000-0000-4000-8000-000000000000";
    for method in ["meta/flow_status", "meta/cancel_flow"] {
        let bobs = console_rpc_as(&app, Some(&bob), method, json!({ "run_id": run_id })).await;
        let absent =
            console_rpc_as(&app, Some(&bob), method, json!({ "run_id": ABSENT_RUN })).await;
        assert_eq!(
            masked(&bobs, &run_id),
            masked(&absent, ABSENT_RUN),
            "{method}: another person reads the run as a run that does not exist: \
             {bobs:#}, absent {absent:#}"
        );
    }
    let listed = console_rpc_as(&app, Some(&bob), "meta/list_runs", json!({})).await;
    assert!(
        !listed.to_string().contains(&run_id) && !listed.to_string().contains("alpha-secret"),
        "another person's run listing names neither the run nor the private member: {listed:#}"
    );
    Ok(())
}

/// The owner key a WorkGraph target names a member of mob `mob` by.
fn agent_owner_key(mob: &str, member: &str) -> Value {
    json!({ "kind": "agent", "id": format!("mob/{mob}/agent/{member}") })
}

/// A WorkGraph target names a member in any of its forms: its identity, its
/// owner key (as `owner` or `lowered_owner`), an owner key of kind
/// `session`, and its session. In every form a member hidden from the caller
/// is refused as the identity form is, as an unknown member, in a goal, a
/// reassignment and a supervisor's key; an attention list filtered by it
/// lists what a name no member holds lists; nothing is bound to it. A shared
/// member is a target in the owner-key form.
#[tokio::test]
async fn workgraph_targets_naming_a_hidden_member_are_refused_in_every_form() -> TestResult {
    const MOB: &str = "private-workgraph-forms";
    let (_state, runtime, app) = signed_in_unified_runtime(MOB).await?;
    let alice = bearer(alice_claims());
    let bob = bearer(bob_claims());
    let forked = fork_on_console(&app, Some(&alice), "alpha", "alpha-side").await;
    assert!(forked["error"].is_null(), "{forked:#}");
    let side = AgentIdentity::from("alpha-side");
    runtime.mob_handle().ensure_member_live(&side).await?;
    let session = runtime
        .mob_handle()
        .get_member(&side)
        .await?
        .and_then(|entry| entry.bridge_session_id().cloned())
        .ok_or("the side chat has a session")?
        .to_string();
    let hidden_forms = [
        json!({ "kind": "owner", "owner_key": agent_owner_key(MOB, "alpha-side") }),
        json!({ "kind": "lowered_owner", "owner_key": agent_owner_key(MOB, "alpha-side") }),
        json!({ "kind": "owner", "owner_key": { "kind": "session", "id": session } }),
        json!({ "kind": "session", "session_id": session }),
    ];

    let shared = console_rpc_as(
        &app,
        Some(&bob),
        "meta/workgraph/goal/create",
        json!({
            "title": "Bob's goal",
            "target": { "kind": "owner", "owner_key": agent_owner_key(MOB, "alpha") },
        }),
    )
    .await;
    assert!(
        shared["error"].is_null(),
        "a shared member is a target in the owner-key form: {shared:#}"
    );
    let binding_id = shared["result"]["attention"]["binding_id"].clone();
    let revision = shared["result"]["item"]["revision"].clone();

    for target in &hidden_forms {
        let created = console_rpc_as(
            &app,
            Some(&bob),
            "meta/workgraph/goal/create",
            json!({ "title": "Bob's other goal", "target": target }),
        )
        .await;
        assert_eq!(
            created["error"]["code"],
            json!(-32001),
            "a goal aimed at the hidden member as {target} is refused as an unknown member's: \
             {created:#}"
        );
        let reassigned = console_rpc_as(
            &app,
            Some(&bob),
            "meta/workgraph/attention/reassign",
            json!({ "binding_id": binding_id, "expected_revision": revision, "target": target }),
        )
        .await;
        assert_eq!(
            reassigned["error"]["code"],
            json!(-32001),
            "a reassignment onto the hidden member as {target} is refused as an unknown \
             member's: {reassigned:#}"
        );
    }
    let supervised = console_rpc_as(
        &app,
        Some(&bob),
        "meta/workgraph/goal/create",
        json!({
            "title": "Bob's supervised goal",
            "target": { "kind": "identity", "identity": "nobody-here" },
            "completion_policy": {
                "kind": "supervisor",
                "owner_key": agent_owner_key(MOB, "alpha-side"),
            },
        }),
    )
    .await;
    assert_eq!(
        supervised["error"]["code"],
        json!(-32001),
        "a supervisor key naming the hidden member is refused as an unknown member's: \
         {supervised:#}"
    );
    let list = |member: &str| json!({ "target": { "kind": "lowered_owner", "owner_key": agent_owner_key(MOB, member) } });
    let hidden = console_rpc_as(
        &app,
        Some(&bob),
        "meta/workgraph/attention/list",
        list("alpha-side"),
    )
    .await;
    let absent = console_rpc_as(
        &app,
        Some(&bob),
        "meta/workgraph/attention/list",
        list("alpha-gone"),
    )
    .await;
    assert_eq!(
        masked(&hidden, "alpha-side"),
        masked(&absent, "alpha-gone"),
        "a list filtered by the hidden member lists what an absent one lists"
    );
    let alices = console_rpc_as(
        &app,
        Some(&alice),
        "meta/workgraph/attention/list",
        json!({}),
    )
    .await;
    assert!(
        alices["error"].is_null()
            && !alices.to_string().contains("agent/alpha-side")
            && !alices.to_string().contains(&session),
        "nothing is bound to the hidden member: {alices:#}"
    );
    Ok(())
}

/// A goal another person aims at a private side chat by its owner key is
/// refused, and the side chat keeps taking turns: no binding of another
/// person's ever reaches it (a binding's tools would be ones a private
/// member is built without, which ends its turns).
#[tokio::test]
async fn another_persons_goal_by_owner_key_leaves_a_private_side_chat_taking_turns() -> TestResult {
    const MOB: &str = "private-workgraph-turns";
    let (_state, runtime, app) = signed_in_unified_runtime(MOB).await?;
    let alice = bearer(alice_claims());
    let bob = bearer(bob_claims());
    let forked = fork_on_console(&app, Some(&alice), "alpha", "alpha-side").await;
    assert!(forked["error"].is_null(), "{forked:#}");
    let side = AgentIdentity::from("alpha-side");
    runtime.mob_handle().ensure_member_live(&side).await?;
    let turn = |text: &'static str| {
        let runtime = Arc::clone(&runtime);
        let side = side.clone();
        async move {
            runtime
                .mob_handle()
                .member(&side)
                .await
                .map_err(|error| error.to_string())?
                .start_turn(
                    meerkat_core::types::ContentInput::Text(text.to_string()),
                    meerkat_core::types::HandlingMode::Queue,
                    meerkat_mob::MemberTurnOptions::new(),
                    None,
                )
                .await
                .map_err(|error| error.to_string())?
                .wait()
                .await
                .map(|_| ())
                .map_err(|error| error.to_string())
        }
    };
    assert_eq!(
        turn("A first turn.").await,
        Ok(()),
        "the side chat takes a turn"
    );
    let created = console_rpc_as(
        &app,
        Some(&bob),
        "meta/workgraph/goal/create",
        json!({
            "title": "Bob's goal",
            "target": { "kind": "owner", "owner_key": agent_owner_key(MOB, "alpha-side") },
        }),
    )
    .await;
    assert_eq!(
        turn("A second turn.").await,
        Ok(()),
        "the side chat still takes turns after another person's goal: {created:#}"
    );
    assert_eq!(
        created["error"]["code"],
        json!(-32001),
        "the goal is refused as aimed at an unknown member: {created:#}"
    );
    Ok(())
}

/// The runtime administration reads (the routing table, the delivery
/// history with its payloads, the edge reconcile and the storage doctor)
/// name no member hidden from their caller on a console with sign-in and no
/// access configuration: every row that names one is left out. The member's
/// owner reads them whole.
#[tokio::test]
async fn runtime_administration_reads_name_no_member_hidden_from_their_caller() -> TestResult {
    let (_temp_dir, spec) = persistent_console_spec(
        "private-admin-reads",
        "orchestrator = \"lead\"",
        "[wiring]\nauto_wire_orchestrator = true\n",
    )?;
    let runtime = Box::pin(MobRuntime::bootstrap(spec)).await?;
    seeded_member(&runtime, "lead", "lead", &[]).await?;
    let owner = crate::start_meta_runtime(
        crate::MetaConfig {
            modules: vec![],
            discovery: crate::DiscoverySpec {
                namespace: "private-admin-reads".to_string(),
                modules: vec![],
            },
            pre_spawn: vec![],
        },
        vec![],
        std::time::Duration::from_secs(1),
    )?;
    let owner = Arc::new(tokio::sync::Mutex::new(owner));
    let mut state = enforced_console(&runtime, None);
    state.module_runtime = Some(Arc::clone(&owner));
    let app = console_json_router_with_state(state);
    let alice = bearer(alice_claims());
    let bob = bearer(bob_claims());
    let alice_scope = caller_scope(&app, &alice).await;
    seeded_member(
        &runtime,
        "worker",
        "alpha-secret",
        &[(PRIVATE_TO, alice_scope.as_str())],
    )
    .await?;
    {
        let mut owner = owner.lock().await;
        owner.add_runtime_route(crate::RuntimeRoute {
            route_key: "mob.member.alpha-secret".to_string(),
            recipient: "alpha-secret".to_string(),
            channel: Some("notification".to_string()),
            sink: "mob_member".to_string(),
            target_module: "delivery".to_string(),
            retry_max: None,
            backoff_ms: None,
            rate_limit_per_minute: None,
        })?;
        owner.record_delivery_for_test(crate::runtime::DeliveryRecord {
            delivery_id: "delivery-1".to_string(),
            route_id: "route-1".to_string(),
            recipient: "alpha-secret".to_string(),
            sink: "mob_member".to_string(),
            target_module: "delivery".to_string(),
            payload: json!({ "text": "The salary review is due." }),
            status: "delivered".to_string(),
            attempts: Vec::new(),
            first_attempt_ms: 1,
            final_attempt_ms: 1,
            idempotency_key: None,
            sink_adapter: None,
        });
    }
    let doctored = tempfile::tempdir()?;
    crate::identity_first::FirstBootManifest::write_new(
        doctored.path(),
        vec![crate::identity_first::FirstBootRow::Resume {
            identity: crate::identity_first::AgentIdentity::parse("alpha-secret")?,
            session: meerkat_core::types::SessionId::new(),
        }],
    )?;

    for (method, params) in [
        ("meta/routing/routes/list", json!({})),
        ("meta/delivery/history", json!({ "limit": 50 })),
        ("meta/reconcile_edges", json!({})),
        (
            "meta/storage/doctor",
            json!({ "state_dir": doctored.path() }),
        ),
    ] {
        let alices = console_rpc_as(&app, Some(&alice), method, params.clone()).await;
        assert!(
            alices["error"].is_null() && alices.to_string().contains("alpha-secret"),
            "{method} names the private member to its owner: {alices:#}"
        );
        let bobs = console_rpc_as(&app, Some(&bob), method, params).await;
        assert!(
            bobs["error"].is_null(),
            "{method} answers another person: {bobs:#}"
        );
        assert!(
            !bobs.to_string().contains("alpha-secret"),
            "{method} names the private member to another person nowhere: {bobs:#}"
        );
    }
    Ok(())
}

/// Stands in for meerkat's agent mob tools: records each call's tool, and
/// answers `mob_check_member` through Core's admission of an owned member
/// target for `caller` without manage scope, as meerkat's tool does (a name
/// no member holds is absent; a member the caller does not own is refused).
struct OwnedMemberTools {
    handle: meerkat_mob::MobHandle,
    caller: AgentIdentity,
    calls: std::sync::Mutex<Vec<String>>,
}

impl OwnedMemberTools {
    fn new(handle: meerkat_mob::MobHandle, caller: &str) -> Arc<Self> {
        Arc::new(Self {
            handle,
            caller: AgentIdentity::from(caller),
            calls: std::sync::Mutex::new(Vec::new()),
        })
    }

    fn calls(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

#[async_trait::async_trait]
impl meerkat_core::AgentToolDispatcher for OwnedMemberTools {
    fn tools(&self) -> Arc<[Arc<meerkat_core::types::ToolDef>]> {
        Arc::from(Vec::new())
    }

    async fn dispatch(
        &self,
        call: meerkat_core::types::ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(call.name.to_string());
        let done = || {
            Ok(meerkat_core::ToolDispatchOutcome::sync_result(
                meerkat_core::types::ToolResult::new(call.id.to_string(), "{}".to_string(), false),
            ))
        };
        if call.name != "mob_check_member" {
            return done();
        }
        let args: Value = serde_json::from_str(call.args.get()).map_err(|error| {
            meerkat_core::ToolError::invalid_arguments(call.name, error.to_string())
        })?;
        let mob_id = meerkat_mob::MobId::from(args["mob_id"].as_str().unwrap_or_default());
        let member = AgentIdentity::from(args["member_id"].as_str().unwrap_or_default());
        match self
            .handle
            .resolve_owned_member_target_admission(false, Some(&self.caller), &member)
            .await
            .map_err(|error| meerkat_core::ToolError::execution_failed(error.to_string()))?
        {
            meerkat_mob::OwnedMemberTargetAdmission::Allowed => done(),
            meerkat_mob::OwnedMemberTargetAdmission::Denied => {
                Err(meerkat_core::ToolError::access_denied(call.name))
            }
            meerkat_mob::OwnedMemberTargetAdmission::Absent(absence) => {
                Err(absence.to_tool_error(call.name, &mob_id, &member))
            }
        }
    }
}

/// One agent mob tool call `name` with `args`, through `tools`.
async fn agent_tool_call(
    tools: &Arc<dyn meerkat_core::AgentToolDispatcher>,
    name: &str,
    args: Value,
) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
    let args = serde_json::value::RawValue::from_string(args.to_string())
        .map_err(|error| meerkat_core::ToolError::invalid_arguments(name, error.to_string()))?;
    tools
        .dispatch_with_context(
            meerkat_core::types::ToolCallView {
                id: "agent-tool-call",
                name,
                args: &args,
            },
            &meerkat_core::ToolDispatchContext::default(),
        )
        .await
}

/// The session of `member` of `runtime`.
async fn member_session(
    runtime: &MobRuntime,
    member: &str,
) -> Result<meerkat_core::types::SessionId, Box<dyn std::error::Error + Send + Sync>> {
    let identity = AgentIdentity::from(member);
    runtime.handle().ensure_member_live(&identity).await?;
    Ok(runtime
        .handle()
        .get_member(&identity)
        .await?
        .and_then(|entry| entry.bridge_session_id().cloned())
        .ok_or_else(|| format!("{member} has a session"))?)
}

/// A member private to a person does not convene a council: its agent's
/// `council` call is refused typed before meerkat's tool runs (the council's
/// participants would be forks of shared members carrying that person's
/// question). A shared member's call reaches the tool.
#[tokio::test]
async fn a_private_member_cannot_convene_a_council() -> TestResult {
    const MOB: &str = "private-council";
    let state = tempfile::tempdir()?;
    let runtime = persistent_gateway_runtime_in(state.path(), MOB, "", "").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let app = console_json_router_with_state(enforced_console(&runtime, None));
    let forked = fork_on_console(&app, Some(&bearer(alice_claims())), "alpha", "alpha-side").await;
    assert!(forked["error"].is_null(), "{forked:#}");
    let overrides = runtime
        .implicit_delegate_retirement_overrides()
        .ok_or("the runtime installs the agent mob tools")?;
    let council = json!({
        "question": "Which of us remembers the code word?",
        "participants": ["alpha"],
    });

    let tools = OwnedMemberTools::new(runtime.handle(), "alpha-side");
    let side_chat = crate::mob_handle_runtime::agent_mob_tool_wrapper_as_member_for_test(
        tools.clone(),
        overrides.clone(),
        &runtime,
        member_session(&runtime, "alpha-side").await?,
    );
    match agent_tool_call(&side_chat, "council", council.clone()).await {
        Err(meerkat_core::ToolError::PolicyDenied { denial }) => {
            assert_eq!(denial.code, "private_member_council", "{}", denial.message);
        }
        other => panic!("a private member's council is refused as a policy denial: {other:?}"),
    }
    assert!(
        tools.calls().is_empty(),
        "meerkat's council tool never ran for the private member"
    );

    let tools = OwnedMemberTools::new(runtime.handle(), "alpha");
    let shared = crate::mob_handle_runtime::agent_mob_tool_wrapper_as_member_for_test(
        tools.clone(),
        overrides,
        &runtime,
        member_session(&runtime, "alpha").await?,
    );
    assert!(
        agent_tool_call(&shared, "council", council).await.is_ok(),
        "a shared member's council reaches the tool"
    );
    assert_eq!(tools.calls(), vec!["council".to_string()]);
    Ok(())
}

/// An agent's `mob_check_member` naming a member private to a person the
/// agent does not act for answers exactly as a name no member holds does:
/// nothing tells the agent the member exists. Its owner's private member
/// checks it as before.
#[tokio::test]
async fn mob_check_member_answers_a_hidden_member_as_an_absent_one() -> TestResult {
    const MOB: &str = "private-check-member";
    let state = tempfile::tempdir()?;
    let runtime = persistent_gateway_runtime_in(state.path(), MOB, "", "").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let app = console_json_router_with_state(enforced_console(&runtime, None));
    let forked = fork_on_console(&app, Some(&bearer(alice_claims())), "alpha", "alpha-side").await;
    assert!(forked["error"].is_null(), "{forked:#}");
    let overrides = runtime
        .implicit_delegate_retirement_overrides()
        .ok_or("the runtime installs the agent mob tools")?;
    let check = |member: &str| json!({ "mob_id": MOB, "member_id": member });
    let answer = |outcome: Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError>,
                  name: &str| {
        let text = match outcome {
            Ok(outcome) => format!("ok: {outcome:?}"),
            Err(error) => format!("error: {}", error.to_transcript_content()),
        };
        text.replace(name, "<member>")
    };

    let shared = crate::mob_handle_runtime::agent_mob_tool_wrapper_as_member_for_test(
        OwnedMemberTools::new(runtime.handle(), "alpha"),
        overrides.clone(),
        &runtime,
        member_session(&runtime, "alpha").await?,
    );
    let hidden = answer(
        agent_tool_call(&shared, "mob_check_member", check("alpha-side")).await,
        "alpha-side",
    );
    let absent = answer(
        agent_tool_call(&shared, "mob_check_member", check("alpha-gone")).await,
        "alpha-gone",
    );
    assert_eq!(
        hidden, absent,
        "a shared agent's check of a private member answers as for an absent member"
    );

    let private = crate::mob_handle_runtime::agent_mob_tool_wrapper_as_member_for_test(
        OwnedMemberTools::new(runtime.handle(), "alpha-side"),
        overrides,
        &runtime,
        member_session(&runtime, "alpha-side").await?,
    );
    let own = answer(
        agent_tool_call(&private, "mob_check_member", check("alpha-side")).await,
        "alpha-side",
    );
    assert_ne!(
        own, absent,
        "the owner's own private member checks it as an existing member"
    );
    Ok(())
}

/// An agent's schedule tools refuse a target that names a member private to
/// a person the agent does not act for, as an act on a member outside the
/// agent's reach (`access_denied`), in every target form that names one: the
/// member, a fork helper of it, and its session. A member private to the
/// same person schedules into it, and a shared target stays open.
#[tokio::test]
async fn schedule_tools_refuse_a_private_target_unless_the_caller_shares_its_owner() -> TestResult {
    const MOB: &str = "private-schedule-targets";
    let state = tempfile::tempdir()?;
    let runtime = persistent_gateway_runtime_in(state.path(), MOB, "", "").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let app = console_json_router_with_state(enforced_console(&runtime, None));
    let alice = bearer(alice_claims());
    for side_chat in ["alpha-side", "alpha-note"] {
        let forked = fork_on_console(&app, Some(&alice), "alpha", side_chat).await;
        assert!(forked["error"].is_null(), "{forked:#}");
    }
    let service = meerkat::ScheduleService::new(Arc::new(meerkat::MemoryScheduleStore::new()));
    let registry = crate::schedule_wiring::ScheduleMobTargetRegistry::default();
    registry.set_mob_state(runtime.agent_mob_mcp_state());
    let tools = crate::schedule_wiring::agent_schedule_tools_for_test(service, registry);
    let side_session = member_session(&runtime, "alpha-side").await?.to_string();
    let create = |target: Value| {
        json!({
            "name": "a reminder",
            "trigger": {
                "type": "interval",
                "start_at_utc": "2026-07-01T05:00:00Z",
                "every_seconds": 86400,
            },
            "target": target,
        })
    };
    let member_target = |member: &str| {
        json!({
            "target_kind": "mob",
            "type": "member",
            "mob_id": MOB,
            "member_id": member,
            "action": { "type": "send", "content": "Look at the salary review." },
        })
    };
    let schedule_as = |caller: meerkat_core::types::SessionId, target: Value| {
        let tools = Arc::clone(&tools);
        let args = create(target);
        async move {
            let args = serde_json::value::RawValue::from_string(args.to_string())
                .map_err(|error| error.to_string())?;
            tools
                .dispatch_with_context(
                    meerkat_core::types::ToolCallView {
                        id: "schedule-create",
                        name: "meerkat_schedule_create",
                        args: &args,
                    },
                    &meerkat_core::ToolDispatchContext::default()
                        .with_runtime_identity(caller, None),
                )
                .await
                .map(|_| ())
                .map_err(|error| format!("{error:?}"))
        }
    };

    let shared_agent = member_session(&runtime, "alpha").await?;
    for target in [
        member_target("alpha-side"),
        json!({
            "target_kind": "mob",
            "type": "fork_helper",
            "mob_id": MOB,
            "source_member_id": "alpha-side",
            "action": { "type": "send", "content": "Look at the salary review." },
        }),
        json!({
            "target_kind": "session",
            "type": "resumable_session",
            "session_id": side_session,
            "action": { "type": "prompt", "prompt": "Look at the salary review." },
        }),
    ] {
        let refused = schedule_as(shared_agent.clone(), target.clone()).await;
        assert!(
            matches!(&refused, Err(error) if error.contains("AccessDenied")),
            "a shared agent's schedule into a private member as {target} is refused as an \
             act outside its reach: {refused:?}"
        );
    }
    assert_eq!(
        schedule_as(shared_agent, member_target("alpha")).await,
        Ok(()),
        "a shared target stays open"
    );
    assert_eq!(
        schedule_as(
            member_session(&runtime, "alpha-note").await?,
            member_target("alpha-side"),
        )
        .await,
        Ok(()),
        "a member private to the same person schedules into it"
    );
    Ok(())
}

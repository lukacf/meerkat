//! The memory panel over notes that were set aside at the first start
//! ([`crate::memory::set_aside`]): an access administrator reads them,
//! forgotten, with their text, and nobody else is shown a scope, a record or
//! a row of them, with or without an access configuration, on a console that
//! identifies people and on one that identifies nobody.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{Value, json};

use super::console_human_admission_tests::BOB;
use super::console_private_side_chat_tests::{
    ALICE, ROOT, TestResult, alice_claims, bearer, enforced_console, fork_on_console,
};
use super::console_side_chat_tests::{persistent_gateway_runtime_in, seeded_member};
use super::*;
use crate::AgentMemoryProvider as _;
use crate::memory::records::{MemoryAuthor, MemoryKind, MemoryScope, NewMemoryRecord};

const REALM: &str = "team";

fn note(title: &str, body: &str) -> NewMemoryRecord {
    NewMemoryRecord {
        kind: MemoryKind::Fact,
        title: title.to_string(),
        description: String::new(),
        body: body.to_string(),
        tags: Vec::new(),
        evidence: Vec::new(),
        verification: None,
    }
}

async fn remember(
    store: &crate::memory::SqliteAgentMemoryStore,
    name: &str,
    title: &str,
    body: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    Ok(store
        .remember_authored(
            &MemoryScope::Identity {
                realm: REALM.to_string(),
                identity: name.to_string(),
            },
            note(title, body),
            MemoryAuthor::Agent {
                identity: name.to_string(),
            },
        )
        .await?
        .memory_id)
}

/// A shared member the console knows.
fn shared_member(identity: &str) -> crate::access::AgentResourceAttributes {
    crate::access::AgentResourceAttributes {
        identity: identity.to_string(),
        agent_id: Some(identity.to_string()),
        role: Some("worker".to_string()),
        labels: BTreeMap::new(),
    }
}

/// Prime `controller` as a console priming does after a start: the live
/// shared members `helper` and `ledger` (a later member of the name whose
/// notes were set aside), and what the private-owner record read from the
/// store's private rows.
async fn primed(
    controller: &AccessController,
    store: &crate::memory::SqliteAgentMemoryStore,
) -> TestResult {
    let owners = crate::private_owners::PrivateOwners::default();
    owners.bind_private_retirements(Arc::new(store.clone()), REALM.to_string());
    owners
        .first_attempts_settled(std::time::Duration::from_secs(5))
        .await;
    assert!(
        !owners.first_read_pending(),
        "the store's private rows were read"
    );
    record_live_and_registered_attributes(
        controller,
        vec![shared_member("helper"), shared_member("ledger")],
        &BTreeMap::new(),
        &BTreeSet::new(),
        &owners.memory_members(),
        "",
    );
    Ok(())
}

/// Every panel read that can show a record or a scope, as text.
async fn panel_reads(
    store: &crate::memory::SqliteAgentMemoryStore,
    view: &AccessView,
    set_aside_id: &str,
) -> String {
    let panel_store: &dyn crate::memory::capabilities::MemoryPanelStore = store;
    let caller = MemoryPanelCaller::own(Some(view));
    let key = crate::memory::sqlite_store::set_aside_scope_key("ledger");
    let mut answers: Vec<Value> = Vec::new();
    for params in [
        json!({ "realm": REALM }),
        json!({ "realm": REALM, "identity": "ledger" }),
        json!({ "realm": REALM, "scope": "identity", "scope_key": key }),
        json!({ "realm": REALM, "status": "tombstoned" }),
    ] {
        answers
            .push(handle_memory_panel_records(Some(panel_store), caller, &params, json!(1)).await);
    }
    answers.push(
        handle_memory_panel_record(
            Some(panel_store),
            caller,
            &json!({ "realm": REALM, "memory_id": set_aside_id }),
            json!(1),
        )
        .await,
    );
    answers.push(
        handle_memory_panel_overview(
            Some(panel_store),
            caller,
            &json!({ "realm": REALM }),
            json!(1),
        )
        .await,
    );
    Value::Array(answers).to_string()
}

/// An access administrator reads the set-aside notes in the panel: the
/// scope in the overview, the records in the listings, each record by id
/// with its text. The person the notes belonged to, another person, a
/// caller who sent no token and the one operator of a console that
/// identifies nobody are shown none of it, while a later shared member of
/// the name is live; a shared member's notes show to them as before.
#[tokio::test]
async fn set_aside_notes_show_to_an_access_administrator_and_to_nobody_else() -> TestResult {
    let memory_dir = tempfile::tempdir()?;
    let store = crate::memory::SqliteAgentMemoryStore::open(memory_dir.path())?;
    let ledger_id = remember(&store, "ledger", "Ledger fact", "LEDGER-KEPT").await?;
    remember(&store, "helper", "Helper fact", "HELPER-KEPT").await?;
    store
        .set_plain_names_aside(
            REALM,
            "mob:main",
            BTreeSet::from(["ledger".to_string()]),
            crate::memory::set_aside::now_ms() + 1,
        )
        .await?;
    let key = crate::memory::sqlite_store::set_aside_scope_key("ledger");

    // With an access configuration: everyone may read every agent's memory,
    // and `root` is the one administrator.
    let enforced = AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![ROOT.to_string()],
        rules: vec![crate::access::AccessRule {
            id: "everyone-reads-every-agent".to_string(),
            subjects: vec!["*".to_string()],
            actions: vec!["agent.*".to_string()],
            ..crate::access::AccessRule::default()
        }],
        ..crate::access::AccessControlConfig::default()
    })?;
    primed(&enforced, &store).await?;
    let root = enforced.view_for_caller(Some(ROOT), Some("root-key"));
    let shown = panel_reads(&store, &root, &ledger_id).await;
    assert!(
        shown.contains("Ledger fact") && shown.contains("LEDGER-KEPT") && shown.contains(&key),
        "an access administrator reads the set-aside notes: {shown}"
    );
    assert!(
        shown.contains("tombstoned"),
        "they read as forgotten: {shown}"
    );

    // Without one: nobody is an administrator.
    let open = AccessController::audience_only();
    primed(&open, &store).await?;
    let callers = [
        (
            "the person the notes belonged to",
            enforced.view_for_caller(Some(ALICE), Some("console-caller:v1:aa")),
        ),
        (
            "another person",
            enforced.view_for_caller(Some(BOB), Some("bob-key")),
        ),
        (
            "another person, no access configuration",
            open.view_for_caller(Some(BOB), Some("bob-key")),
        ),
        (
            "a caller who sent no token",
            open.view_for_caller(None, None).on_console(true),
        ),
        (
            "the one operator of a console that identifies nobody",
            open.view_for_caller(None, None).on_console(false),
        ),
    ];
    for (who, view) in &callers {
        let shown = panel_reads(&store, view, &ledger_id).await;
        assert!(
            !shown.contains("Ledger fact")
                && !shown.contains("LEDGER-KEPT")
                && !shown.contains(&key),
            "{who} is shown nothing of the set-aside notes: {shown}"
        );
        assert!(
            shown.contains("Helper fact"),
            "{who} reads a shared member's notes as before: {shown}"
        );
    }
    Ok(())
}

/// The first start's step reads its names from the mob's own event log as a
/// start finds it: a private console side chat that a person opened and
/// that ended before the restart is in it, so the notes under its name are
/// set aside, and the notes under the name of the shared member it was
/// forked from stay.
#[tokio::test]
async fn the_step_reads_an_ended_private_side_chats_name_from_the_mobs_log_after_a_restart()
-> TestResult {
    const MOB: &str = "set-aside-log-after-restart";
    let state = tempfile::tempdir()?;
    let runtime = persistent_gateway_runtime_in(state.path(), MOB, "", "").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let app = console_json_router_with_state(enforced_console(&runtime, None));
    let forked = fork_on_console(&app, Some(&bearer(alice_claims())), "alpha", "alpha-side").await;
    assert!(forked["error"].is_null(), "{forked:#}");
    drop(app);
    let side_chat = AgentIdentity::from("alpha-side");
    runtime.handle().retire(side_chat.clone()).await?;
    let mut ended = false;
    for _ in 0..100 {
        if runtime.handle().get_member(&side_chat).await?.is_none() {
            ended = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(ended, "the side chat ended");
    runtime.handle().shutdown().await?;
    let _ = runtime
        .stand_down_child_mobs(std::time::Instant::now() + std::time::Duration::from_secs(90))
        .await;
    drop(runtime);
    let runtime = persistent_gateway_runtime_in(state.path(), MOB, "", "").await?;

    let memory_dir = tempfile::tempdir()?;
    let store = crate::memory::SqliteAgentMemoryStore::open(memory_dir.path())?;
    remember(&store, "alpha-side", "Side chat fact", "SIDE-CHAT-KEPT").await?;
    remember(&store, "alpha", "Alpha fact", "ALPHA-KEPT").await?;
    let journal = runtime.handle().observer();
    assert_eq!(
        crate::memory::set_aside::test_entries::memory_start(
            &store,
            REALM,
            &journal,
            &crate::memory::set_aside::HostedMobs::default(),
            true,
            crate::memory::set_aside::now_ms() + 1,
        )
        .await,
        crate::memory::set_aside::MemoryStart::Attach
    );

    let panel_store: &dyn crate::memory::capabilities::MemoryPanelStore = &store;
    let page = panel_store
        .records_page(REALM, None, None, None, 16, None)
        .await?;
    let mut notes: Vec<(String, &str, String)> = page
        .records
        .into_iter()
        .map(|record| {
            (
                record.scope.key().to_string(),
                record.status.kind_str(),
                record.body,
            )
        })
        .collect();
    notes.sort();
    assert_eq!(
        notes,
        [
            ("alpha".to_string(), "active", "ALPHA-KEPT".to_string()),
            (
                crate::memory::sqlite_store::set_aside_scope_key("alpha-side"),
                "tombstoned",
                "SIDE-CHAT-KEPT".to_string()
            ),
        ]
    );
    assert!(
        store.plain_names_were_set_aside(REALM, MOB).await?,
        "the step is recorded for the mob the log belongs to"
    );
    runtime.handle().shutdown().await?;
    Ok(())
}

/// A persisted gateway runtime after a restart over the same state, in
/// which, before the restart, a private side chat created a mob of its own
/// (its `mob_create`, with the member `sub-helper` seated for its session)
/// and ran the helper `delegate-helper` in the implicit mob of its
/// `delegate`. Answers the restarted runtime and the ids of those two mobs.
async fn restarted_with_a_private_side_chats_mobs(
    state: &std::path::Path,
    mob: &str,
) -> Result<
    (crate::mob_handle_runtime::MobRuntime, [String; 2]),
    Box<dyn std::error::Error + Send + Sync>,
> {
    let runtime = persistent_gateway_runtime_in(state, mob, "", "").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let app = console_json_router_with_state(enforced_console(&runtime, None));
    let forked = fork_on_console(&app, Some(&bearer(alice_claims())), "alpha", "alpha-side").await;
    assert!(forked["error"].is_null(), "{forked:#}");
    drop(app);
    let side_chat = AgentIdentity::from("alpha-side");
    runtime.handle().ensure_member_live(&side_chat).await?;
    let side_session = runtime
        .handle()
        .get_member(&side_chat)
        .await?
        .and_then(|entry| entry.bridge_session_id().cloned())
        .ok_or("the side chat has a session")?;
    let tools = runtime.agent_mob_mcp_state().ok_or("agent mob tools")?;

    // Its `mob_create`: a mob of its own, with a member seated for its
    // session as the agent mob tools seat one.
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
            meerkat_mob::SpawnMemberSpec::new("worker", AgentIdentity::from("sub-helper"))
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
            meerkat_mob::SpawnMemberSpec::new("delegate", AgentIdentity::from("delegate-helper"))
                .with_runtime_mode(meerkat_mob::MobRuntimeMode::TurnDriven),
            side_session.clone(),
        ),
    )
    .await?;
    let created = [team_id.to_string(), implicit.to_string()];
    drop((team_handle, implicit_handle, tools));

    // The gateway restarts over the same state.
    runtime.handle().shutdown().await?;
    let _ = runtime
        .stand_down_child_mobs(std::time::Instant::now() + std::time::Duration::from_secs(90))
        .await;
    drop(runtime);
    let runtime = persistent_gateway_runtime_in(state, mob, "", "").await?;
    Ok((runtime, created))
}

/// An agent memory store as an earlier release leaves it: notes under the
/// names of the two members seated in a private side chat's mobs
/// ([`restarted_with_a_private_side_chats_mobs`]) and under the name of
/// the shared member the side chat was forked from, and no marker.
async fn store_with_the_created_mobs_members_notes(
    dir: &std::path::Path,
) -> Result<crate::memory::SqliteAgentMemoryStore, Box<dyn std::error::Error + Send + Sync>> {
    let store = crate::memory::SqliteAgentMemoryStore::open(dir)?;
    remember(&store, "sub-helper", "Team fact", "TEAM-MEMBER-KEPT").await?;
    remember(
        &store,
        "delegate-helper",
        "Helper fact",
        "DELEGATE-HELPER-KEPT",
    )
    .await?;
    remember(&store, "alpha", "Alpha fact", "ALPHA-KEPT").await?;
    Ok(store)
}

/// That store once the step has set the two members' notes aside: every
/// record's scope key, status and text, the shared member's as they were.
async fn assert_the_created_mobs_members_notes_are_set_aside(
    store: &crate::memory::SqliteAgentMemoryStore,
) -> TestResult {
    let panel_store: &dyn crate::memory::capabilities::MemoryPanelStore = store;
    let page = panel_store
        .records_page(REALM, None, None, None, 16, None)
        .await?;
    let mut notes: Vec<(String, &str, String)> = page
        .records
        .into_iter()
        .map(|record| {
            (
                record.scope.key().to_string(),
                record.status.kind_str(),
                record.body,
            )
        })
        .collect();
    notes.sort();
    assert_eq!(
        notes,
        [
            ("alpha".to_string(), "active", "ALPHA-KEPT".to_string()),
            (
                crate::memory::sqlite_store::set_aside_scope_key("delegate-helper"),
                "tombstoned",
                "DELEGATE-HELPER-KEPT".to_string()
            ),
            (
                crate::memory::sqlite_store::set_aside_scope_key("sub-helper"),
                "tombstoned",
                "TEAM-MEMBER-KEPT".to_string()
            ),
        ]
    );
    Ok(())
}

/// The first start's step reads the event logs of the mobs a private side
/// chat's own turns created (a `mob_create` mob, and the implicit mob its
/// `delegate` runs helpers in) as a start finds them after a restart: the
/// notes under the names of the members seated there are set aside with the
/// composition's own step, and a shared member's notes stay. The step's
/// answer names the logs it read: the composition's own first and once,
/// then the two created mobs'.
#[tokio::test]
async fn the_step_reads_the_names_held_in_the_mobs_a_private_side_chat_created_after_a_restart()
-> TestResult {
    const MOB: &str = "set-aside-hosted-mobs-after-restart";
    let state = tempfile::tempdir()?;
    let (runtime, created) = restarted_with_a_private_side_chats_mobs(state.path(), MOB).await?;
    let memory_dir = tempfile::tempdir()?;
    let store = store_with_the_created_mobs_members_notes(memory_dir.path()).await?;

    let journal = runtime.handle().observer();
    let tools = runtime.agent_mob_mcp_state().ok_or("agent mob tools")?;
    let hosted = crate::memory::set_aside::HostedMobLogs::of(Some(tools.as_ref()), MOB)
        .await
        .map_err(|error| error.to_string())?;
    let answer = crate::memory::set_aside::test_entries::set_aside_at_first_start(
        &store,
        REALM,
        &journal,
        &hosted.mobs(),
        true,
        crate::memory::set_aside::now_ms() + 1,
    )
    .await
    .map_err(|error| error.to_string())?
    .ok_or("a launch that speaks for its composition runs the step")?;

    assert_eq!(
        answer.outcome,
        crate::memory::sqlite_store::SetAsideOutcome::Done {
            names: 2,
            records: 2
        }
    );
    assert_eq!(
        answer.mobs_read.first().map(String::as_str),
        Some(MOB),
        "the composition's own log is read first: {:?}",
        answer.mobs_read
    );
    let mut hosted_read: Vec<String> = answer.mobs_read.iter().skip(1).cloned().collect();
    hosted_read.sort();
    let mut created = created.to_vec();
    created.sort();
    assert_eq!(
        hosted_read, created,
        "then the created mob's and the implicit mob's, each once, and the own not again"
    );
    assert!(answer.not_read.is_empty(), "{:?}", answer.not_read);
    assert_the_created_mobs_members_notes_are_set_aside(&store).await?;
    assert!(
        store.plain_names_were_set_aside(REALM, MOB).await?,
        "the step is recorded for the composition's own mob"
    );
    assert!(
        !store
            .plain_names_were_set_aside(REALM, "alpha-side-team")
            .await?,
        "a hosted mob has no marker of its own"
    );
    runtime.handle().shutdown().await?;
    Ok(())
}

/// A composition's own start of agent memory
/// (`crate::memory::set_aside::memory_start_for`, which the gateway and a
/// library build call with their bootstrapped mob runtime) hands the step
/// the mobs the agent mob tools host. Called after a restart over the
/// persisted state, its own read of the hosted mobs returns the restored
/// mobs: it sets aside the notes under the names held in the mobs a private
/// side chat created.
#[tokio::test]
async fn a_compositions_memory_start_reads_the_hosted_mobs_after_a_restart() -> TestResult {
    const MOB: &str = "set-aside-composition-start-after-restart";
    let state = tempfile::tempdir()?;
    let memory_dir = tempfile::tempdir()?;
    let store = store_with_the_created_mobs_members_notes(memory_dir.path()).await?;
    let (runtime, _created) = restarted_with_a_private_side_chats_mobs(state.path(), MOB).await?;

    assert_eq!(
        crate::memory::set_aside::memory_start_for(
            &store,
            REALM,
            &runtime,
            crate::memory::set_aside::now_ms() + 1,
        )
        .await,
        crate::memory::set_aside::MemoryStart::Attach
    );

    assert_the_created_mobs_members_notes_are_set_aside(&store).await?;
    assert!(
        store.plain_names_were_set_aside(REALM, MOB).await?,
        "the step is recorded for the composition's own mob"
    );
    runtime.handle().shutdown().await?;
    Ok(())
}

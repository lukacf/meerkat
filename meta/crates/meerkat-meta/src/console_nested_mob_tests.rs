//! The console lists the mobs nested under its gateway's own: each mob a
//! member created, nested under that member's mob at any depth. A mob the
//! host created is a mob of its own and is not listed. A mob's place comes
//! from its creation
//! record alone; a mob created for a session no roster names (a temporary
//! council's) is not listed. A member of a nested mob is named by its mob and
//! name. A mob is listed to a caller only through members that caller is
//! listed, and building the list wakes no member.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use meerkat_client::TestClient;
use meerkat_core::types::SessionId;
use meerkat_mob::{
    AgentIdentity, MobDefinition, MobHandle, MobRuntimeMode, MobStorage, ProfileName,
    SpawnMemberSpec,
};
use serde_json::{Value, json};

use super::*;
use crate::access::{AccessControlConfig, AccessController, AccessView};
use crate::auth::session_fence::ConsoleSessionFences;
use crate::console_aggregator::{
    AllowAllConsoleVisibilityPolicy, ConsoleVisibilityPolicy,
    HideImplicitDelegateMembersConsoleVisibilityPolicy,
};
use crate::mob_handle_runtime::MobRuntime;
use crate::runtime::{ConsoleLiveSnapshot, ConsoleRestJsonRequest};
use crate::{MobBootstrapOptions, MobBootstrapSpec};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// The profiles every mob of these rows offers: a worker, and `delegate`,
/// the profile the helpers of the `delegate` tool run under.
fn profiles() -> Value {
    json!({
        "worker": { "model": "gpt-5.5", "runtime_mode": "turn_driven", "tools": { "comms": true } },
        "delegate": { "model": "gpt-5.5", "runtime_mode": "turn_driven", "tools": { "comms": true } }
    })
}

/// A gateway runtime whose primary mob is `mob_id`, with agent mob tools.
/// Its host may create a mob through the tools' mob set, as on every launch
/// with no declared store: the rows here make a mob no member created that
/// way.
async fn runtime(mob_id: &str, root: &std::path::Path) -> TestResult<MobRuntime> {
    let definition: MobDefinition =
        serde_json::from_value(json!({ "id": mob_id, "profiles": profiles() }))?;
    let runtime = MobRuntime::bootstrap(
        MobBootstrapSpec::ephemeral(
            definition,
            MobStorage::in_memory(),
            root.to_path_buf(),
            16,
            None,
        )
        .expect("build the nested mob fixture spec")
        .with_options(MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(TestClient::default())),
        }),
    )
    .await?;
    Ok(runtime)
}

/// Seat `member` on `handle` under `profile`, turn-driven, so no model runs.
async fn seat(
    handle: &MobHandle,
    profile: &str,
    member: &str,
    labels: BTreeMap<String, String>,
) -> TestResult {
    let mut spec = SpawnMemberSpec::new(ProfileName::from(profile), AgentIdentity::from(member));
    spec.runtime_mode = Some(MobRuntimeMode::TurnDriven);
    if !labels.is_empty() {
        spec.labels = Some(labels);
    }
    handle.ensure_member(spec).await?;
    Ok(())
}

/// The bridge session `member` of `handle` runs on.
async fn session_of(handle: &MobHandle, member: &str) -> TestResult<SessionId> {
    Ok(handle
        .resolve_bridge_session_id(&AgentIdentity::from(member))
        .await
        .ok_or_else(|| format!("{member} has a bridge session"))?)
}

/// A creation record that names the member `creator` of `mob` as it is
/// seated there: at its generation, on its session.
async fn created_by_the_seated(
    mob: &MobHandle,
    creator: &str,
) -> TestResult<meerkat_mob::MobProvenance> {
    let seated = mob
        .get_member(&AgentIdentity::from(creator))
        .await?
        .ok_or("the creating member is seated")?;
    Ok(meerkat_mob::MobProvenance::OperatorMember {
        parent_mob_id: mob.mob_id().clone(),
        parent_incarnation: mob.incarnation(),
        creator_identity: AgentIdentity::from(creator),
        creator_generation: seated.generation,
        owner_session: seated
            .bridge_session_id()
            .cloned()
            .ok_or("the creating member has a session")?,
    })
}

/// A mob made through the runtime's agent mob state: owned by the session
/// `owner`, as the agent mob tools make one, or by no member.
async fn mob(
    runtime: &MobRuntime,
    mob_id: &str,
    owner: Option<SessionId>,
) -> TestResult<MobHandle> {
    let state = runtime
        .agent_mob_mcp_state()
        .ok_or("the runtime carries agent mob tools")?;
    let definition: MobDefinition =
        serde_json::from_value(json!({ "id": mob_id, "profiles": profiles() }))?;
    let created =
        match owner {
            Some(owner) => {
                Box::pin(state.mob_create_definition_with_owner_bridge_session(
                    definition, owner, true, false,
                ))
                .await
            }
            None => Box::pin(state.mob_create_definition(definition)).await,
        }
        .map_err(|error| error.to_string())?;
    Ok(Box::pin(state.handle_for(&created))
        .await
        .map_err(|error| error.to_string())?)
}

/// The live snapshot a console with `policy` serves, built and narrowed as
/// its experience route builds and narrows it.
async fn snapshot(
    runtime: &MobRuntime,
    policy: &dyn ConsoleVisibilityPolicy,
) -> ConsoleLiveSnapshot {
    let (mut snapshot, _) = Box::pin(build_live_snapshot(
        runtime,
        &[],
        None,
        policy,
        &ConsoleSnapshotReadModel::default(),
        None,
    ))
    .await;
    apply_console_visibility_policy(&mut snapshot, policy);
    snapshot
}

/// The experience a caller with `audience` gets over `snapshot` (the
/// anonymous caller of a console without sign-in when `None`).
fn experience(snapshot: &ConsoleLiveSnapshot, audience: Option<&AccessView>) -> Value {
    experience_of(snapshot, crate::runtime::SnapshotRows::OfRosters, audience)
}

/// [`experience`] over a snapshot whose member rows are `rows`.
fn experience_of(
    snapshot: &ConsoleLiveSnapshot,
    rows: crate::runtime::SnapshotRows,
    audience: Option<&AccessView>,
) -> Value {
    let decisions = RuntimeDecisionState::local_console(
        crate::ConsolePolicy {
            require_app_auth: false,
            ..Default::default()
        },
        None,
    );
    let response = render_console_rest_json_route(
        &decisions,
        &ConsoleSessionFences::detached(),
        &ConsoleRestJsonRequest {
            method: "GET".to_string(),
            path: "/console/experience".to_string(),
            auth: None,
        },
        None,
        Some(snapshot),
        rows,
        None,
        audience,
        false,
        false,
        &BTreeSet::new(),
        None,
    );
    assert_eq!(response.status, 200, "experience: {:?}", response.body);
    response.body
}

/// The mob `mob_id` as the experience lists it.
fn listed_mob<'a>(experience: &'a Value, mob_id: &str) -> Option<&'a Value> {
    experience
        .pointer("/topology/live_snapshot/mobs")?
        .as_array()?
        .iter()
        .find(|mob| mob["mob_id"] == mob_id)
}

/// The ids of the mobs the experience lists.
fn listed_mob_ids(experience: &Value) -> BTreeSet<String> {
    experience
        .pointer("/topology/live_snapshot/mobs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|mob| mob["mob_id"].as_str().map(str::to_string))
        .collect()
}

/// The sidebar row of the member shown as `member` in the experience (the
/// rows of a fixture that seats each name once).
fn row<'a>(experience: &'a Value, member: &str) -> Option<&'a Value> {
    experience
        .pointer("/agent_sidebar/live_snapshot/agents")?
        .as_array()?
        .iter()
        .find(|agent| agent["label"] == member)
}

/// A mob entry's fields as the rows compare them: its parent, its host, how
/// many members it lists and its state.
fn placement(mob: &Value) -> (Option<&str>, Option<&str>, Option<u64>, Option<&str>) {
    (
        mob["parent_mob_id"].as_str(),
        mob["host_identity"].as_str(),
        mob["member_count"].as_u64(),
        mob["state"].as_str(),
    )
}

/// A mob no member owns is a mob the host created, and a mob of its own:
/// the launch mob's console lists neither it nor its members. It has a
/// console of its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mob_no_member_owns_is_a_mob_of_its_own_and_is_not_listed() -> TestResult {
    let root = tempfile::tempdir()?;
    let runtime = runtime("nested-top", root.path()).await?;
    seat(&runtime.handle(), "worker", "planner", BTreeMap::new()).await?;
    let library = mob(&runtime, "nested-top-library", None).await?;
    seat(&library, "worker", "archivist", BTreeMap::new()).await?;

    let experience = experience(
        &snapshot(&runtime, &AllowAllConsoleVisibilityPolicy).await,
        None,
    );

    assert!(
        row(&experience, "archivist").is_none(),
        "the launch mob's console lists no member of a mob the host created"
    );
    assert!(
        listed_mob(&experience, "nested-top-library").is_none(),
        "the launch mob's console lists no mob the host created"
    );
    assert_eq!(
        listed_mob_ids(&experience),
        BTreeSet::from(["nested-top".to_string()])
    );
    Ok(())
}

/// Mobs nest at any depth, each under the member whose session created it:
/// a mob a nested member creates sits in its creator's mob (the release
/// before this change listed neither as a mob). A mob the host created is a
/// mob of its own and no group of this one: it is not listed, and neither
/// is a mob one of its members created.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mobs_nest_at_any_depth_under_the_member_that_created_them() -> TestResult {
    let root = tempfile::tempdir()?;
    let runtime = runtime("nested-depth", root.path()).await?;
    let primary = runtime.handle();
    seat(&primary, "worker", "planner", BTreeMap::new()).await?;
    let research = mob(
        &runtime,
        "nested-depth-research",
        Some(session_of(&primary, "planner").await?),
    )
    .await?;
    seat(&research, "worker", "researcher", BTreeMap::new()).await?;
    let reading = mob(
        &runtime,
        "nested-depth-reading",
        Some(session_of(&research, "researcher").await?),
    )
    .await?;
    seat(&reading, "worker", "reader", BTreeMap::new()).await?;
    let shelf = mob(&runtime, "nested-depth-shelf", None).await?;
    seat(&shelf, "worker", "curator", BTreeMap::new()).await?;
    let clerks = mob(
        &runtime,
        "nested-depth-clerks",
        Some(session_of(&shelf, "curator").await?),
    )
    .await?;
    seat(&clerks, "worker", "clerk", BTreeMap::new()).await?;

    let experience = experience(
        &snapshot(&runtime, &AllowAllConsoleVisibilityPolicy).await,
        None,
    );

    for (mob_id, expected) in [
        ("nested-depth", (None, None, Some(1), Some("running"))),
        (
            "nested-depth-research",
            (
                Some("nested-depth"),
                Some("planner"),
                Some(1),
                Some("running"),
            ),
        ),
        (
            "nested-depth-reading",
            (
                Some("nested-depth-research"),
                Some("nested-depth-research/researcher"),
                Some(1),
                Some("running"),
            ),
        ),
    ] {
        let listed = listed_mob(&experience, mob_id).ok_or(format!("{mob_id} is listed"))?;
        assert_eq!(placement(listed), expected, "{mob_id}: {listed}");
    }
    for (member, mob_id, host) in [
        ("researcher", "nested-depth-research", Some("planner")),
        (
            "reader",
            "nested-depth-reading",
            Some("nested-depth-research/researcher"),
        ),
    ] {
        let listed = row(&experience, member).ok_or(format!("{member} is listed"))?;
        assert_eq!(listed["member_mob_id"], mob_id, "{member}: {listed}");
        assert_eq!(
            listed["labels"]["delegate_host_identity"].as_str(),
            host,
            "{member}: {listed}"
        );
    }
    // The mob the host created and the mob its member created.
    for (mob_id, member) in [
        ("nested-depth-shelf", "curator"),
        ("nested-depth-clerks", "clerk"),
    ] {
        assert!(
            listed_mob(&experience, mob_id).is_none(),
            "{mob_id} is no group of the launch mob"
        );
        assert!(
            row(&experience, member).is_none(),
            "{member} of {mob_id} is not listed"
        );
    }
    Ok(())
}

/// The stock console lists a nested mob's members and the mob, and keeps
/// hiding the helpers of the `delegate` tool (role `delegate`): a mob whose
/// only members are such helpers is not listed. The stock console of the
/// release before this change hid every nested mob.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_stock_console_lists_nested_mobs_and_keeps_delegate_helpers_hidden() -> TestResult {
    let root = tempfile::tempdir()?;
    let runtime = runtime("nested-stock", root.path()).await?;
    let primary = runtime.handle();
    seat(&primary, "worker", "planner", BTreeMap::new()).await?;
    let planner_session = session_of(&primary, "planner").await?;
    let team = mob(&runtime, "nested-stock-team", Some(planner_session.clone())).await?;
    seat(&team, "worker", "analyst", BTreeMap::new()).await?;
    seat(&team, "delegate", "helper-one", BTreeMap::new()).await?;
    let helpers = mob(&runtime, "nested-stock-helpers", Some(planner_session)).await?;
    seat(&helpers, "delegate", "helper-two", BTreeMap::new()).await?;

    let experience = experience(
        &snapshot(
            &runtime,
            &HideImplicitDelegateMembersConsoleVisibilityPolicy,
        )
        .await,
        None,
    );

    let analyst = row(&experience, "analyst").ok_or("the stock console lists the member")?;
    assert_eq!(analyst["member_mob_id"], "nested-stock-team");
    assert!(
        row(&experience, "helper-one").is_none(),
        "a delegate helper stays hidden"
    );
    assert!(
        row(&experience, "helper-two").is_none(),
        "a delegate helper stays hidden"
    );
    assert_eq!(
        listed_mob_ids(&experience),
        BTreeSet::from(["nested-stock".to_string(), "nested-stock-team".to_string()]),
        "the helpers' mob has no member the console shows"
    );
    let team = listed_mob(&experience, "nested-stock-team").ok_or("the mob is listed")?;
    assert_eq!(
        placement(team),
        (
            Some("nested-stock"),
            Some("planner"),
            Some(1),
            Some("running")
        )
    );
    Ok(())
}

/// A mob is listed to a caller only through members that caller may see,
/// with its count of them: a mob whose only member is private to one person
/// is listed to that person alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mob_is_listed_only_to_callers_who_may_see_one_of_its_members() -> TestResult {
    let root = tempfile::tempdir()?;
    let runtime = runtime("nested-private", root.path()).await?;
    let primary = runtime.handle();
    seat(&primary, "worker", "planner", BTreeMap::new()).await?;
    let planner_session = session_of(&primary, "planner").await?;
    let diary = mob(
        &runtime,
        "nested-private-diary",
        Some(planner_session.clone()),
    )
    .await?;
    seat(
        &diary,
        "worker",
        "diarist",
        BTreeMap::from([(
            crate::access::PRIVATE_TO_LABEL.to_string(),
            "person:alice".to_string(),
        )]),
    )
    .await?;
    let shared = mob(&runtime, "nested-private-shared", Some(planner_session)).await?;
    seat(&shared, "worker", "notetaker", BTreeMap::new()).await?;
    seat(
        &shared,
        "worker",
        "confidant",
        BTreeMap::from([(
            crate::access::PRIVATE_TO_LABEL.to_string(),
            "person:alice".to_string(),
        )]),
    )
    .await?;
    let listed = snapshot(&runtime, &AllowAllConsoleVisibilityPolicy).await;
    let audience = AccessController::audience_only();

    let alice = experience(
        &listed,
        Some(&audience.view_for_caller(Some("alice@example.test"), Some("person:alice"))),
    );
    assert!(row(&alice, "diarist").is_some());
    assert_eq!(
        listed_mob(&alice, "nested-private-diary").map(placement),
        Some((
            Some("nested-private"),
            Some("planner"),
            Some(1),
            Some("running")
        ))
    );
    assert_eq!(
        listed_mob(&alice, "nested-private-shared").map(placement),
        Some((
            Some("nested-private"),
            Some("planner"),
            Some(2),
            Some("running")
        ))
    );

    let bob = experience(
        &listed,
        Some(&audience.view_for_caller(Some("bob@example.test"), Some("person:bob"))),
    );
    assert!(row(&bob, "diarist").is_none());
    assert!(row(&bob, "confidant").is_none());
    assert!(listed_mob(&bob, "nested-private-diary").is_none(), "{bob}");
    assert_eq!(
        listed_mob(&bob, "nested-private-shared").map(placement),
        Some((
            Some("nested-private"),
            Some("planner"),
            Some(1),
            Some("running")
        ))
    );
    Ok(())
}

/// The experience the holder of `bearer` gets from the console route.
async fn experience_on(app: &Router, bearer: &str) -> Value {
    use tower::ServiceExt;
    let response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/console/experience")
                .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
                .body(axum::body::Body::empty())
                .expect("experience request"),
        )
        .await
        .expect("experience response");
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 1 << 22)
        .await
        .expect("experience body");
    serde_json::from_slice(&body).expect("experience JSON")
}

/// On the stock console, while the gateway runs, a mob a private side chat
/// creates, and the member its session seats in it, show to the side chat's
/// owner and to no one else: the private-owner rule decides a nested mob's
/// member as it decides a primary one, and the mob follows its members.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_private_side_chats_mob_shows_to_its_owner_alone_on_the_stock_console() -> TestResult {
    use super::console_private_side_chat_tests::{
        PRIVATE_TO, alice_claims, bearer, bob_claims, enforced_console, fork_on_console,
    };
    use super::console_side_chat_tests::{persistent_gateway_runtime_in, seeded_member};

    // A runtime as a gateway composes one: it carries the agent mob tools.
    let state = tempfile::tempdir()?;
    let runtime =
        persistent_gateway_runtime_in(state.path(), "nested-private-side-chat", "", "").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let mut console = enforced_console(&runtime, None);
    console.visibility_policy = Arc::new(HideImplicitDelegateMembersConsoleVisibilityPolicy);
    let app = console_json_router_with_state(console);
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
    let team = mob(&runtime, "alpha-side-team", Some(side_session.clone())).await?;
    Box::pin(
        team.spawn_spec_with_generated_owner_context(
            SpawnMemberSpec::new("worker", AgentIdentity::from("sub-helper"))
                .with_runtime_mode(MobRuntimeMode::TurnDriven),
            side_session,
        ),
    )
    .await?;
    let helper = team
        .get_member(&AgentIdentity::from("sub-helper"))
        .await?
        .ok_or("the helper is seated")?;
    assert!(
        helper.labels.contains_key(PRIVATE_TO),
        "the helper is private to the side chat's owner: {:?}",
        helper.labels
    );

    let owner = experience_on(&app, &alice).await;
    let listed = row(&owner, "sub-helper").ok_or("the owner sees the helper")?;
    assert_eq!(listed["member_mob_id"], "alpha-side-team", "{listed}");
    assert_eq!(
        listed_mob(&owner, "alpha-side-team").map(placement),
        Some((
            Some("nested-private-side-chat"),
            Some("alpha-side"),
            Some(1),
            Some("running")
        )),
        "{owner}"
    );

    let other = experience_on(&app, &bob).await;
    assert!(
        row(&other, "sub-helper").is_none(),
        "another person does not see the helper"
    );
    assert!(
        listed_mob(&other, "alpha-side-team").is_none(),
        "another person is not told the mob exists: {other}"
    );
    Ok(())
}

/// The roster priming of the stream, RPC, send and timeline routes walks
/// the mobs the console's snapshot walks: a member of a mob a listed member
/// created is listed, a member of a mob created for a session no roster
/// names is not, and alternating the console experience with the priming
/// records one value per identity.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_snapshot_and_the_priming_walk_the_same_mobs() -> TestResult {
    let root = tempfile::tempdir()?;
    let runtime = runtime("nested-priming", root.path()).await?;
    let primary = runtime.handle();
    seat(&primary, "worker", "planner", BTreeMap::new()).await?;
    let library = mob(
        &runtime,
        "nested-priming-library",
        Some(session_of(&primary, "planner").await?),
    )
    .await?;
    seat(&library, "worker", "archivist", BTreeMap::new()).await?;
    let archive = mob(&runtime, "nested-priming-archive", Some(SessionId::new())).await?;
    seat(&archive, "worker", "scout", BTreeMap::new()).await?;
    let controller = AccessController::new(AccessControlConfig {
        enabled: true,
        admins: vec!["root@example.test".to_string()],
        ..AccessControlConfig::default()
    })?;
    let publication = meerkat_authorization::publication::LocalAuthorizationPublication::new();
    controller
        .join_authorization_publication(publication.clone())
        .map_err(|_| "a fresh controller joins its first publication")?;
    let decisions = RuntimeDecisionState::local_console(
        crate::ConsolePolicy {
            require_app_auth: false,
            ..Default::default()
        },
        None,
    );
    let console_experience = |snapshot: &ConsoleLiveSnapshot| {
        let response = crate::runtime::handle_console_rest_json_route_with_snapshot_and_access(
            &decisions,
            &ConsoleRestJsonRequest {
                method: "GET".to_string(),
                path: "/console/experience".to_string(),
                auth: Some(crate::ConsoleAccessRequest {
                    provider: crate::AuthProvider::GoogleOAuth,
                    email: "root@example.test".to_string(),
                }),
            },
            Some(snapshot),
            Some(&controller),
        );
        assert_eq!(response.status, 200, "experience: {:?}", response.body);
        response.body
    };

    let listed = snapshot(&runtime, &AllowAllConsoleVisibilityPolicy).await;
    let first = console_experience(&listed);
    assert!(
        row(&first, "archivist").is_some(),
        "the member of the mob a listed member created is listed"
    );
    assert!(
        row(&first, "scout").is_none(),
        "the member of the mob no record places is not listed"
    );
    prime_access_cache_from_runtime(&runtime, &controller).await;
    let quiet = publication
        .observe(|| ())
        .map_err(|_| "a quiet publication")?
        .1;
    for _ in 0..2 {
        console_experience(&snapshot(&runtime, &AllowAllConsoleVisibilityPolicy).await);
        prime_access_cache_from_runtime(&runtime, &controller).await;
    }
    assert!(
        quiet.check_current().is_ok(),
        "alternating the console experience and the roster priming publishes no access change"
    );
    Ok(())
}

/// `meta/reset_all` acts on the members it acted on in the release before
/// this change, whatever the console now lists: on a console that shows
/// implicit delegates it retires
/// the members of the mobs whose owner session a listed member holds, and
/// never a member of a mob no member owns or whose owner no member holds;
/// the stock console retires no nested member at all. Either console
/// retires the primary mob's member that no identity registers, as before.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reset_all_retires_exactly_the_nested_members_it_retired_before() -> TestResult {
    // Each console runs over mobs of its own: two runtimes of one process
    // cannot hold one mob id.
    for (stock, mob_id) in [(false, "nested-reset"), (true, "nested-reset-stock")] {
        let team_id = format!("{mob_id}-team");
        let expected = if stock {
            vec![json!({ "identity": "planner" })]
        } else {
            vec![
                json!({ "identity": "analyst", "mob_id": team_id }),
                json!({ "identity": "planner" }),
            ]
        };
        let root = tempfile::tempdir()?;
        let runtime = runtime(mob_id, root.path()).await?;
        let primary = runtime.handle();
        seat(&primary, "worker", "planner", BTreeMap::new()).await?;
        let team = mob(
            &runtime,
            &team_id,
            Some(session_of(&primary, "planner").await?),
        )
        .await?;
        seat(&team, "worker", "analyst", BTreeMap::new()).await?;
        let library = mob(&runtime, &format!("{mob_id}-library"), None).await?;
        seat(&library, "worker", "archivist", BTreeMap::new()).await?;
        let archive = mob(
            &runtime,
            &format!("{mob_id}-archive"),
            Some(SessionId::new()),
        )
        .await?;
        seat(&archive, "worker", "scout", BTreeMap::new()).await?;
        let policy: &dyn ConsoleVisibilityPolicy = if stock {
            &HideImplicitDelegateMembersConsoleVisibilityPolicy
        } else {
            &AllowAllConsoleVisibilityPolicy
        };

        let body = reset_all_live_console_agents(&runtime, None, None, None, policy)
            .await
            .map_err(|error| error.to_string())?;

        assert_eq!(
            body["retired_delegates"],
            json!(expected),
            "stock {stock}: {body}"
        );
        for (handle, member) in [(&library, "archivist"), (&archive, "scout")] {
            assert!(
                handle
                    .get_member(&AgentIdentity::from(member))
                    .await?
                    .is_some(),
                "stock {stock}: reset_all leaves {member} seated"
            );
        }
        if stock {
            assert!(
                team.get_member(&AgentIdentity::from("analyst"))
                    .await?
                    .is_some(),
                "the stock console's reset_all leaves the nested member seated"
            );
        }
    }
    Ok(())
}

/// The sidebar row of the member `name` of the mob `mob_id`, found by the
/// row's mob and its label (the member's name, which no mob qualifies).
fn member_row<'a>(experience: &'a Value, mob_id: &str, name: &str) -> Option<&'a Value> {
    experience
        .pointer("/agent_sidebar/live_snapshot/agents")?
        .as_array()?
        .iter()
        .find(|agent| agent["member_mob_id"] == mob_id && agent["label"] == name)
}

/// The `agent_id` of every sidebar row.
fn agent_ids(experience: &Value) -> BTreeSet<String> {
    experience
        .pointer("/agent_sidebar/live_snapshot/agents")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|agent| agent["agent_id"].as_str().map(str::to_string))
        .collect()
}

/// The `identity` of every topology node.
fn node_ids(experience: &Value) -> BTreeSet<String> {
    experience
        .pointer("/topology/live_snapshot/nodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|node| node["identity"].as_str().map(str::to_string))
        .collect()
}

fn private_to(owner: &str) -> BTreeMap<String, String> {
    BTreeMap::from([(
        crate::access::PRIVATE_TO_LABEL.to_string(),
        owner.to_string(),
    )])
}

/// The view of a signed-in person on a console that enforces no rule.
fn person(audience: &AccessController, name: &str) -> AccessView {
    audience.view_for_caller(
        Some(&format!("{name}@example.test")),
        Some(&format!("person:{name}")),
    )
}

/// The view of an access admin on a console that enforces access.
fn admin() -> TestResult<AccessView> {
    let controller = AccessController::new(AccessControlConfig {
        enabled: true,
        admins: vec!["root@example.test".to_string()],
        ..AccessControlConfig::default()
    })?;
    Ok(controller.view_for_caller(Some("root@example.test"), None))
}

/// A host policy as one was written for the release before this change: it
/// answers the switch for implicit delegate members and no method added
/// since.
struct PolicyWrittenBeforeThisChange;

impl ConsoleVisibilityPolicy for PolicyWrittenBeforeThisChange {
    fn include_implicit_delegate_members(&self) -> bool {
        false
    }
}

/// A policy written for the release before this change that hides implicit
/// delegate members keeps hiding every member of every nested mob, and so
/// every nested mob: an
/// upgrade shows such a console nothing it hid.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_policy_written_before_this_change_keeps_hiding_every_nested_member() -> TestResult {
    let root = tempfile::tempdir()?;
    let runtime = runtime("nested-old-policy", root.path()).await?;
    let primary = runtime.handle();
    seat(&primary, "worker", "planner", BTreeMap::new()).await?;
    let team = mob(
        &runtime,
        "nested-old-policy-team",
        Some(session_of(&primary, "planner").await?),
    )
    .await?;
    seat(&team, "worker", "analyst", BTreeMap::new()).await?;
    seat(&team, "delegate", "helper", BTreeMap::new()).await?;
    let library = mob(&runtime, "nested-old-policy-library", None).await?;
    seat(&library, "worker", "archivist", BTreeMap::new()).await?;

    let experience = experience(
        &snapshot(&runtime, &PolicyWrittenBeforeThisChange).await,
        None,
    );

    assert_eq!(
        agent_ids(&experience),
        BTreeSet::from(["planner".to_string()]),
        "only the console's own mob's member is listed"
    );
    assert_eq!(
        node_ids(&experience),
        BTreeSet::from(["planner".to_string()])
    );
    assert_eq!(
        listed_mob_ids(&experience),
        BTreeSet::from(["nested-old-policy".to_string()])
    );
    Ok(())
}

/// A member's mob is the mob whose roster holds it. The labels a member or
/// its spawner can write (`source_mob_id`, `delegate_host_identity`) place it
/// nowhere and name no creator: a primary member that carries them stays a
/// primary member, and a nested member that names another mob stays in its
/// own.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_label_a_member_wrote_places_it_in_no_other_mob() -> TestResult {
    let root = tempfile::tempdir()?;
    let runtime = runtime("nested-label", root.path()).await?;
    let primary = runtime.handle();
    seat(&primary, "worker", "planner", BTreeMap::new()).await?;
    seat(
        &primary,
        "worker",
        "impostor",
        BTreeMap::from([
            (
                "source_mob_id".to_string(),
                "nested-label-vault".to_string(),
            ),
            ("delegate_host_identity".to_string(), "planner".to_string()),
        ]),
    )
    .await?;
    let vault = mob(
        &runtime,
        "nested-label-vault",
        Some(session_of(&primary, "planner").await?),
    )
    .await?;
    seat(
        &vault,
        "worker",
        "keeper",
        BTreeMap::from([("source_mob_id".to_string(), "nested-label".to_string())]),
    )
    .await?;

    let experience = experience(
        &snapshot(&runtime, &AllowAllConsoleVisibilityPolicy).await,
        None,
    );

    let impostor = member_row(&experience, "nested-label", "impostor")
        .ok_or("the member that wrote the label is listed in its own mob")?;
    assert!(
        impostor["labels"].get("source_mob_id").is_none(),
        "{impostor}"
    );
    assert!(
        impostor["labels"].get("delegate_host_identity").is_none(),
        "{impostor}"
    );
    let keeper = member_row(&experience, "nested-label-vault", "keeper")
        .ok_or("the nested member stays in its own mob")?;
    assert_eq!(keeper["labels"]["source_mob_id"], "nested-label-vault");
    assert_eq!(
        listed_mob(&experience, "nested-label").map(|mob| mob["member_count"].clone()),
        Some(json!(2))
    );
    assert_eq!(
        listed_mob(&experience, "nested-label-vault").map(|mob| mob["member_count"].clone()),
        Some(json!(1)),
        "the mob the label named gains no member"
    );
    Ok(())
}

/// A mob created for a session no roster names is not listed, and neither
/// are its members, on any console: that is the creation record of a
/// temporary council's mob (the council seats forks of members in a mob it
/// creates for a fresh session), and the release before this change listed
/// none.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mob_created_for_a_session_no_roster_names_is_not_listed() -> TestResult {
    let root = tempfile::tempdir()?;
    let runtime = runtime("nested-council", root.path()).await?;
    seat(&runtime.handle(), "worker", "planner", BTreeMap::new()).await?;
    let seats = mob(&runtime, "nested-council-seats", Some(SessionId::new())).await?;
    seat(&seats, "worker", "panelist", BTreeMap::new()).await?;

    let policies: [&dyn ConsoleVisibilityPolicy; 2] = [
        &AllowAllConsoleVisibilityPolicy,
        &HideImplicitDelegateMembersConsoleVisibilityPolicy,
    ];
    for policy in policies {
        let experience = experience(&snapshot(&runtime, policy).await, None);
        assert_eq!(
            listed_mob_ids(&experience),
            BTreeSet::from(["nested-council".to_string()]),
            "{experience}"
        );
        assert_eq!(
            agent_ids(&experience),
            BTreeSet::from(["planner".to_string()])
        );
        assert_eq!(
            node_ids(&experience),
            BTreeSet::from(["planner".to_string()])
        );
    }
    Ok(())
}

/// Every mob the host created is a mob of its own: the launch mob's console
/// lists none of them and none of their members.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mobs_the_host_created_are_mobs_of_their_own_and_are_not_listed() -> TestResult {
    let root = tempfile::tempdir()?;
    let runtime = runtime("nested-hosted", root.path()).await?;
    seat(&runtime.handle(), "worker", "planner", BTreeMap::new()).await?;
    let support = mob(&runtime, "nested-hosted-support", None).await?;
    seat(&support, "worker", "agent-one", BTreeMap::new()).await?;
    let billing = mob(&runtime, "nested-hosted-billing", None).await?;
    seat(&billing, "worker", "agent-two", BTreeMap::new()).await?;

    let experience = experience(
        &snapshot(
            &runtime,
            &HideImplicitDelegateMembersConsoleVisibilityPolicy,
        )
        .await,
        None,
    );

    for (mob_id, member) in [
        ("nested-hosted-support", "agent-one"),
        ("nested-hosted-billing", "agent-two"),
    ] {
        assert!(
            listed_mob(&experience, mob_id).is_none(),
            "the launch mob's console lists no mob the host created: {mob_id}"
        );
        assert!(
            member_row(&experience, mob_id, member).is_none(),
            "the launch mob's console lists no member of a mob the host created: {member}"
        );
    }
    Ok(())
}

/// A mob stays listed under the mob of the member that created it after
/// that member left: the mob's creation record names it, whatever session
/// the roster binds now. The creator, no longer listed, is not named.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mob_stays_listed_after_its_creator_left() -> TestResult {
    let root = tempfile::tempdir()?;
    let runtime = runtime("nested-left", root.path()).await?;
    let primary = runtime.handle();
    seat(&primary, "worker", "planner", BTreeMap::new()).await?;
    seat(&primary, "worker", "founder", BTreeMap::new()).await?;
    let team = mob(
        &runtime,
        "nested-left-team",
        Some(session_of(&primary, "founder").await?),
    )
    .await?;
    seat(&team, "worker", "analyst", BTreeMap::new()).await?;
    primary.retire(AgentIdentity::from("founder")).await?;

    let experience = experience(
        &snapshot(
            &runtime,
            &HideImplicitDelegateMembersConsoleVisibilityPolicy,
        )
        .await,
        None,
    );

    assert!(member_row(&experience, "nested-left-team", "analyst").is_some());
    assert_eq!(
        listed_mob(&experience, "nested-left-team").map(placement),
        Some((Some("nested-left"), None, Some(1), Some("running"))),
        "{experience}"
    );
    Ok(())
}

/// Hides the member `secretary` and its identity.
struct HidesSecretary;

impl ConsoleVisibilityPolicy for HidesSecretary {
    fn member_visible(&self, member: &crate::runtime::ConsoleMember) -> bool {
        member.agent_identity != "secretary"
    }

    fn identity_visible(&self, record: &crate::console_aggregator::ConsoleIdentityRecord) -> bool {
        record.identity != "secretary"
    }
}

/// A mob with no member a caller sees is listed to no one, an access admin
/// included: a mob with no member at all, and an entry that names a state
/// this console does not list (a mob held or quarantined at a restore has no
/// member to list).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mob_with_no_listed_member_is_listed_to_no_one_an_access_admin_included() -> TestResult {
    let root = tempfile::tempdir()?;
    let runtime = runtime("nested-empty", root.path()).await?;
    seat(&runtime.handle(), "worker", "planner", BTreeMap::new()).await?;
    mob(&runtime, "nested-empty-shelf", None).await?;
    let mut listed = snapshot(&runtime, &AllowAllConsoleVisibilityPolicy).await;
    for state in ["held", "quarantined"] {
        listed.mobs.push(serde_json::from_value(json!({
            "mob_id": format!("nested-empty-{state}"),
            "state": state,
            "member_count": 0,
        }))?);
    }

    let seen = experience(&listed, Some(&admin()?));

    assert_eq!(
        listed_mob_ids(&seen),
        BTreeSet::from(["nested-empty".to_string()]),
        "{seen}"
    );
    Ok(())
}

/// Members with one name in several mobs are each listed, in their own mob,
/// under an identity that holds the mob: no member and no mob disappears, a
/// primary member keeps its bare name, and each is decided for a caller by
/// its own attributes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn members_with_one_name_in_several_mobs_are_each_listed_in_their_own_mob() -> TestResult {
    let root = tempfile::tempdir()?;
    let runtime = runtime("nested-names", root.path()).await?;
    let primary = runtime.handle();
    seat(&primary, "worker", "planner", BTreeMap::new()).await?;
    seat(&primary, "worker", "reviewer", BTreeMap::new()).await?;
    let alpha = mob(
        &runtime,
        "nested-names-alpha",
        Some(session_of(&primary, "planner").await?),
    )
    .await?;
    seat(&alpha, "worker", "reviewer", private_to("person:alice")).await?;
    seat(&alpha, "worker", "writer", BTreeMap::new()).await?;
    let beta = mob(
        &runtime,
        "nested-names-beta",
        Some(session_of(&primary, "reviewer").await?),
    )
    .await?;
    seat(&beta, "worker", "reviewer", BTreeMap::new()).await?;
    let listed = snapshot(
        &runtime,
        &HideImplicitDelegateMembersConsoleVisibilityPolicy,
    )
    .await;

    // The owner of the private namesake is listed every member.
    let audience = AccessController::audience_only();
    let all = experience(&listed, Some(&person(&audience, "alice")));
    for (mob_id, name) in [
        ("nested-names", "reviewer"),
        ("nested-names-alpha", "reviewer"),
        ("nested-names-alpha", "writer"),
        ("nested-names-beta", "reviewer"),
    ] {
        assert!(
            member_row(&all, mob_id, name).is_some(),
            "{name} of {mob_id} is listed: {all}"
        );
    }
    let identities = BTreeSet::from([
        "planner".to_string(),
        "reviewer".to_string(),
        "nested-names-alpha/reviewer".to_string(),
        "nested-names-alpha/writer".to_string(),
        "nested-names-beta/reviewer".to_string(),
    ]);
    assert_eq!(agent_ids(&all), identities);
    assert_eq!(node_ids(&all), identities);
    for (mob_id, count) in [
        ("nested-names", 2),
        ("nested-names-alpha", 2),
        ("nested-names-beta", 1),
    ] {
        assert_eq!(
            listed_mob(&all, mob_id).map(|mob| mob["member_count"].clone()),
            Some(json!(count)),
            "{mob_id}"
        );
    }

    // Another person: the private namesake is hidden, the two others stay.
    let other = experience(&listed, Some(&person(&audience, "bob")));
    assert_eq!(
        agent_ids(&other),
        BTreeSet::from([
            "planner".to_string(),
            "reviewer".to_string(),
            "nested-names-alpha/writer".to_string(),
            "nested-names-beta/reviewer".to_string(),
        ])
    );
    Ok(())
}

/// The ids of the mobs the gateway's agent mob state holds.
async fn mobs_held(runtime: &MobRuntime) -> BTreeSet<String> {
    match runtime.agent_mob_mcp_state() {
        Some(state) => Box::pin(state.mob_handles_snapshot())
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|(mob_id, _)| mob_id.to_string())
            .collect(),
        None => BTreeSet::new(),
    }
}

/// The console's reads of nested mobs wake no member and load no mob: a
/// dormant member of a nested mob is listed like a live one and stays
/// dormant across the snapshot, the experience and the access priming of the
/// routes, under either stock policy, and the gateway holds the same mobs
/// after them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_reads_of_nested_mobs_wake_no_member_and_load_no_mob() -> TestResult {
    use super::console_side_chat_tests::{persistent_gateway_runtime_in, seeded_member};

    let state = tempfile::tempdir()?;
    let runtime = persistent_gateway_runtime_in(state.path(), "nested-dormant", "", "").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let primary = runtime.handle();
    primary
        .ensure_member_live(&AgentIdentity::from("alpha"))
        .await?;
    let team = mob(
        &runtime,
        "nested-dormant-team",
        Some(session_of(&primary, "alpha").await?),
    )
    .await?;
    seat(&team, "worker", "resting", BTreeMap::new()).await?;
    let resting = AgentIdentity::from("resting");
    // Refused retryably until the member's runtime binding has landed.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        match team.release_member(&resting).await {
            Ok(()) => break,
            Err(meerkat_mob::MobError::LifecycleOperationPending { .. })
                if tokio::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            Err(error) => return Err(error.into()),
        }
    }
    let mut changes = team.machine_state_changes();
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while !team.member_unmaterialized(&resting) {
            changes
                .changed()
                .await
                .expect("the mob's machine state stays published");
        }
    })
    .await
    .map_err(|_| "the release left the nested member dormant")?;
    let before = mobs_held(&runtime).await;

    let policies: [&dyn ConsoleVisibilityPolicy; 2] = [
        &AllowAllConsoleVisibilityPolicy,
        &HideImplicitDelegateMembersConsoleVisibilityPolicy,
    ];
    for policy in policies {
        let seen = experience(&snapshot(&runtime, policy).await, None);
        assert!(
            member_row(&seen, "nested-dormant-team", "resting").is_some(),
            "the dormant member is listed: {seen}"
        );
    }
    let controller = AccessController::audience_only();
    prime_access_cache_from_runtime(&runtime, &controller).await;

    assert!(
        team.member_unmaterialized(&resting),
        "no read woke the dormant member"
    );
    assert_eq!(mobs_held(&runtime).await, before, "no read loaded a mob");
    Ok(())
}

/// The answer of the console RPC `method` with `params` for the holder of
/// `bearer`.
async fn rpc_answer(app: &Router, bearer: &str, method: &str, params: Value) -> Value {
    use tower::ServiceExt;
    let response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/console/rpc")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
                .body(axum::body::Body::from(
                    json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params })
                        .to_string(),
                ))
                .expect("rpc request"),
        )
        .await
        .expect("rpc response");
    let body = axum::body::to_bytes(response.into_body(), 1 << 22)
        .await
        .expect("rpc body");
    serde_json::from_slice(&body).unwrap_or(Value::Null)
}

/// Every route that reaches a nested mob's member applies the rule of the
/// list read to the caller: the member's owner reaches it under its mob and
/// name, and another person is answered, by each route, exactly as for a
/// member that does not exist in a mob that does not exist. Nothing tells
/// that person the member or its mob is there.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_routes_that_reach_a_nested_member_answer_another_person_as_for_no_member() -> TestResult
{
    use super::console_private_side_chat_tests::{
        alice_claims, bearer, bob_claims, enforced_console, fork_on_console, send_on_console,
    };
    use super::console_side_chat_tests::{persistent_gateway_runtime_in, seeded_member};

    let state = tempfile::tempdir()?;
    let runtime = persistent_gateway_runtime_in(state.path(), "nested-routes", "", "").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let mut console = enforced_console(&runtime, None);
    console.visibility_policy = Arc::new(HideImplicitDelegateMembersConsoleVisibilityPolicy);
    let aggregator = crate::console_aggregator::MetaConsoleAggregator::in_memory();
    aggregator.register_runtime_handles_with_policy(
        "default",
        "",
        runtime.clone(),
        None,
        ConsoleEventStore::new(),
        Arc::new(HideImplicitDelegateMembersConsoleVisibilityPolicy),
    );
    console.console_aggregator = Some(aggregator);
    let app = console_json_router_with_state(console);
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
    let team = mob(&runtime, "nested-routes-team", Some(side_session.clone())).await?;
    Box::pin(
        team.spawn_spec_with_generated_owner_context(
            SpawnMemberSpec::new("worker", AgentIdentity::from("sub-helper"))
                .with_runtime_mode(MobRuntimeMode::TurnDriven),
            side_session,
        ),
    )
    .await?;
    let member = "nested-routes-team/sub-helper";
    let no_member = "nested-routes-nowhere/nobody";

    // The owner reaches the member under its mob and name.
    let owner = experience_on(&app, &alice).await;
    assert_eq!(
        member_row(&owner, "nested-routes-team", "sub-helper").map(|row| row["agent_id"].clone()),
        Some(json!(member)),
        "{owner}"
    );
    let inspected = rpc_answer(
        &app,
        &alice,
        "meta/console/inspect_identity",
        json!({ "identity": member }),
    )
    .await;
    assert!(
        inspected["error"].is_null() && inspected.to_string().contains(member),
        "the owner inspects the member: {inspected:#}"
    );

    // Another person: every route that names the member answers as for a
    // member that is not there.
    let (status, sent) = send_on_console(&app, &bob, member).await;
    let (absent_status, absent_sent) = send_on_console(&app, &bob, no_member).await;
    assert_eq!(
        (status, sent.to_string().replace(member, "<member>")),
        (
            absent_status,
            absent_sent.to_string().replace(no_member, "<member>")
        ),
        "a send"
    );
    for (method, field) in [
        ("meta/console/inspect_identity", "identity"),
        ("meta/console/query_timeline", "identity"),
        ("meta/inspect_identity", "identity"),
        ("meta/status_identity", "identity"),
        ("meta/retire", "identity"),
        ("meta/retire_member", "member_id"),
        ("meta/schedule/list", "identity"),
        ("meta/get_member", "member_id"),
        ("meta/member_status", "member_id"),
    ] {
        let hidden = rpc_answer(&app, &bob, method, json!({ field: member })).await;
        let absent = rpc_answer(&app, &bob, method, json!({ field: no_member })).await;
        assert_eq!(
            hidden.to_string().replace(member, "<member>"),
            absent.to_string().replace(no_member, "<member>"),
            "{method} answers the hidden member as no member: hidden {hidden:#}, absent \
             {absent:#}"
        );
    }
    let listed = rpc_answer(&app, &bob, "meta/console/list_identities", json!({})).await;
    assert!(
        !listed.to_string().contains("sub-helper")
            && !listed.to_string().contains("nested-routes-team"),
        "the identities read names neither the member nor its mob: {listed:#}"
    );
    assert!(
        team.get_member(&AgentIdentity::from("sub-helper"))
            .await?
            .is_some(),
        "the member is still seated"
    );
    Ok(())
}

/// The form of a nested member's identity at its two edges. A member whose
/// own name holds `/` is named by its mob and that name, and listed. A
/// nested member whose mob and name spell a primary member's name and
/// that primary member are a pair the roster shows neither of (contract
/// v0.6.0, `shared_spelling_rows`), and the nested mob counts what is
/// listed of it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_nested_member_form_is_unambiguous_at_its_edges() -> TestResult {
    let root = tempfile::tempdir()?;
    let runtime = runtime("nested-form", root.path()).await?;
    let primary = runtime.handle();
    seat(&primary, "worker", "planner", BTreeMap::new()).await?;
    // A primary member whose name spells a nested member's identity.
    seat(
        &primary,
        "worker",
        crate::member_comms_id::mob_member_id_str("nested-form-team/reviewer").as_ref(),
        BTreeMap::new(),
    )
    .await?;
    let team = mob(
        &runtime,
        "nested-form-team",
        Some(session_of(&primary, "planner").await?),
    )
    .await?;
    seat(&team, "worker", "reviewer", BTreeMap::new()).await?;
    seat(
        &team,
        "worker",
        crate::member_comms_id::mob_member_id_str("drafts/editor").as_ref(),
        BTreeMap::new(),
    )
    .await?;

    let seen = experience(
        &snapshot(
            &runtime,
            &HideImplicitDelegateMembersConsoleVisibilityPolicy,
        )
        .await,
        None,
    );

    let editor = member_row(&seen, "nested-form-team", "drafts/editor")
        .ok_or("the member whose name holds a slash is listed")?;
    assert_eq!(editor["agent_id"], "nested-form-team/drafts/editor");
    assert_eq!(
        crate::console_member_ref::parse_nested_member_identity("nested-form-team/drafts/editor"),
        Some(("nested-form-team", "drafts/editor"))
    );
    let named: Vec<&Value> = seen
        .pointer("/agent_sidebar/live_snapshot/agents")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|agent| agent["agent_id"] == "nested-form-team/reviewer")
        .collect();
    assert_eq!(
        named.len(),
        0,
        "the roster shows neither member of a pair (contract v0.6.0 \
         shared_spelling_rows): {seen}"
    );
    assert_eq!(
        listed_mob(&seen, "nested-form-team").map(|mob| mob["member_count"].clone()),
        Some(json!(1)),
        "the nested mob counts what is listed of it: its member whose name holds a slash"
    );
    Ok(())
}

/// A mob's creation record is read from its store once per mob and kept:
/// the snapshot, the experience and the access priming of the routes read
/// it again from memory. That holds for a mob the console does not list
/// too: a mob the host created, and a mob created for a session no roster
/// names.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mobs_creation_record_is_read_from_its_store_once() -> TestResult {
    let root = tempfile::tempdir()?;
    let runtime = runtime("nested-record", root.path()).await?;
    let primary = runtime.handle();
    seat(&primary, "worker", "planner", BTreeMap::new()).await?;
    let team = mob(
        &runtime,
        "nested-record-team",
        Some(session_of(&primary, "planner").await?),
    )
    .await?;
    seat(&team, "worker", "analyst", BTreeMap::new()).await?;
    let library = mob(&runtime, "nested-record-library", None).await?;
    seat(&library, "worker", "archivist", BTreeMap::new()).await?;
    let seats = mob(&runtime, "nested-record-seats", Some(SessionId::new())).await?;
    seat(&seats, "worker", "panelist", BTreeMap::new()).await?;
    let controller = AccessController::audience_only();

    for _ in 0..3 {
        let seen = experience(
            &snapshot(&runtime, &AllowAllConsoleVisibilityPolicy).await,
            None,
        );
        assert_eq!(listed_mob_ids(&seen).len(), 2, "{seen}");
        prime_access_cache_from_runtime(&runtime, &controller).await;
    }

    assert_eq!(
        runtime.console_mob_records().store_reads(),
        3,
        "one read per mob, the two the console does not list included"
    );
    Ok(())
}

/// The label of the members the access rules of [`bob_denied_the_vault`]
/// deny `bob`.
fn vault_tier() -> BTreeMap<String, String> {
    BTreeMap::from([("tier".to_string(), "vault".to_string())])
}

/// An access configuration where everyone may do everything with every
/// agent, except that `bob` is denied every agent labelled `tier=vault`;
/// `root` is an access admin.
fn bob_denied_the_vault() -> TestResult<AccessController> {
    Ok(AccessController::new(AccessControlConfig {
        enabled: true,
        admins: vec!["root@example.test".to_string()],
        rules: vec![
            crate::access::AccessRule {
                id: "everyone-every-agent".to_string(),
                subjects: vec!["*".to_string()],
                actions: vec!["agent.*".to_string()],
                agents: vec!["*".to_string()],
                ..Default::default()
            },
            crate::access::AccessRule {
                id: "bob-not-the-vault".to_string(),
                effect: crate::access::AccessEffect::Deny,
                subjects: vec!["bob@example.test".to_string()],
                actions: vec!["agent.*".to_string()],
                match_labels: vault_tier(),
                ..Default::default()
            },
        ],
        ..AccessControlConfig::default()
    })?)
}

/// The view of the signed-in person `name` under `access`.
fn person_under(access: &AccessController, name: &str) -> AccessView {
    access.view_for_caller(
        Some(&format!("{name}@example.test")),
        Some(&format!("person:{name}")),
    )
}

/// A mob in which a caller can see no member is not listed to that caller
/// at all, also when a mob nested under it is: the listed mob sits under its
/// nearest listed ancestor, and nothing names the mob above it or the member
/// in it that created the listed one. One row per kind of caller: a person
/// the rules deny the upper mob's member, a person they allow, an access
/// admin, and an anonymous caller.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mob_with_no_member_a_caller_sees_is_not_listed_above_a_mob_the_caller_sees() -> TestResult
{
    let root = tempfile::tempdir()?;
    let runtime = runtime("nested-ancestor", root.path()).await?;
    let primary = runtime.handle();
    seat(&primary, "worker", "planner", BTreeMap::new()).await?;
    let vault = mob(
        &runtime,
        "nested-ancestor-vault",
        Some(session_of(&primary, "planner").await?),
    )
    .await?;
    seat(&vault, "worker", "keeper", vault_tier()).await?;
    let desk = mob(
        &runtime,
        "nested-ancestor-desk",
        Some(session_of(&vault, "keeper").await?),
    )
    .await?;
    seat(&desk, "worker", "clerk", BTreeMap::new()).await?;
    let listed = snapshot(&runtime, &AllowAllConsoleVisibilityPolicy).await;
    let access = bob_denied_the_vault()?;

    // A person the rules deny the vault's member.
    let seen = experience(&listed, Some(&person_under(&access, "bob")));
    assert!(
        member_row(&seen, "nested-ancestor-desk", "clerk").is_some(),
        "the desk's member is listed: {seen}"
    );
    assert!(
        listed_mob(&seen, "nested-ancestor-vault").is_none(),
        "the vault is not listed: {seen}"
    );
    assert_eq!(
        listed_mob(&seen, "nested-ancestor-desk").map(placement),
        Some((Some("nested-ancestor"), None, Some(1), Some("running"))),
        "the desk sits under the nearest listed mob, its creator not named"
    );
    assert!(
        !seen.to_string().contains("nested-ancestor-vault"),
        "nothing names the vault or its member: {seen}"
    );

    // A person the rules allow, and an access admin, see the whole chain.
    for (caller, view) in [
        ("a person the rules allow", person_under(&access, "alice")),
        (
            "an access admin",
            access.view_for_caller(Some("root@example.test"), None),
        ),
    ] {
        let seen = experience(&listed, Some(&view));
        assert_eq!(
            listed_mob(&seen, "nested-ancestor-vault").map(placement),
            Some((
                Some("nested-ancestor"),
                Some("planner"),
                Some(1),
                Some("running")
            )),
            "{caller}"
        );
        assert_eq!(
            listed_mob(&seen, "nested-ancestor-desk").map(placement),
            Some((
                Some("nested-ancestor-vault"),
                Some("nested-ancestor-vault/keeper"),
                Some(1),
                Some("running")
            )),
            "{caller}"
        );
    }

    // An anonymous caller: whatever the rules list it, a mob is named only
    // with a member of it.
    let seen = experience(&listed, Some(&access.view_for_caller(None, None)));
    for mob_id in ["nested-ancestor-vault", "nested-ancestor-desk"] {
        match listed_mob(&seen, mob_id) {
            Some(mob) => assert!(mob["member_count"].as_u64() >= Some(1), "{mob}"),
            None => assert!(
                !seen.to_string().contains(mob_id),
                "an anonymous caller is not told of {mob_id}: {seen}"
            ),
        }
    }
    Ok(())
}

/// The member that created a mob is named nowhere to a caller who cannot
/// see it: not on the mob, not in a label of the mob's members
/// (`delegate_host_identity`), not among their peers. The experience such a
/// caller gets does not hold the creator's name at all; a caller who sees
/// the creator is told.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_creator_is_named_nowhere_to_a_caller_who_cannot_see_it() -> TestResult {
    let root = tempfile::tempdir()?;
    let runtime = runtime("nested-creator", root.path()).await?;
    let primary = runtime.handle();
    seat(&primary, "worker", "planner", BTreeMap::new()).await?;
    seat(&primary, "worker", "secretary", vault_tier()).await?;
    let team = mob(
        &runtime,
        "nested-creator-team",
        Some(session_of(&primary, "secretary").await?),
    )
    .await?;
    seat(&team, "worker", "aide", BTreeMap::new()).await?;
    let listed = snapshot(&runtime, &AllowAllConsoleVisibilityPolicy).await;
    let access = bob_denied_the_vault()?;

    let other = experience(&listed, Some(&person_under(&access, "bob")));
    let aide = member_row(&other, "nested-creator-team", "aide")
        .ok_or("the person the rules deny the creator sees the mob's member")?;
    assert!(
        aide["labels"].get("delegate_host_identity").is_none(),
        "{aide}"
    );
    assert_eq!(
        listed_mob(&other, "nested-creator-team").map(placement),
        Some((Some("nested-creator"), None, Some(1), Some("running")))
    );
    assert!(
        !other.to_string().contains("secretary"),
        "nothing names the creator: {other}"
    );

    let allowed = experience(&listed, Some(&person_under(&access, "alice")));
    let aide = member_row(&allowed, "nested-creator-team", "aide").ok_or("the member is listed")?;
    assert_eq!(aide["labels"]["delegate_host_identity"], "secretary");
    assert_eq!(
        listed_mob(&allowed, "nested-creator-team").map(placement),
        Some((
            Some("nested-creator"),
            Some("secretary"),
            Some(1),
            Some("running")
        ))
    );

    // A policy that hides the creator hides its name the same way.
    let hidden = experience(&snapshot(&runtime, &HidesSecretary).await, None);
    assert!(member_row(&hidden, "nested-creator-team", "aide").is_some());
    assert!(
        !hidden.to_string().contains("secretary"),
        "nothing names the creator the policy hides: {hidden}"
    );
    Ok(())
}

/// The mob a member sits in and the member that created that mob stay the
/// console's own facts on the spawn registry's path too. A spawn's labels
/// are kept by the registry and fill the gaps of a member's row; a
/// `source_mob_id` or a `delegate_host_identity` among them places the
/// member of the console's own mob nowhere else, and a rule that selects a
/// nested mob's members by the mob label does not match it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_label_a_spawn_registered_places_a_member_in_no_other_mob() -> TestResult {
    let root = tempfile::tempdir()?;
    let runtime = runtime("nested-registry", root.path()).await?;
    let primary = runtime.handle();
    seat(&primary, "worker", "planner", BTreeMap::new()).await?;
    seat(&primary, "worker", "impostor", BTreeMap::new()).await?;
    let vault = mob(
        &runtime,
        "nested-registry-vault",
        Some(session_of(&primary, "planner").await?),
    )
    .await?;
    seat(&vault, "worker", "keeper", BTreeMap::new()).await?;
    // The registry holds what the spawn of `impostor` carried, as an agent's
    // spawn tool call records it.
    let sink = crate::console_spawn::ConsoleSpawnSink::for_mob(
        ConsoleEventStore::new(),
        primary.mob_id().as_str(),
    );
    runtime.install_console_spawn_sink(sink.clone());
    sink.project_spawned_member(&crate::console_spawn::ConsoleSpawnSeed {
        mob_id: Some(primary.mob_id().to_string()),
        member_id: "impostor".to_string(),
        identity: "impostor".to_string(),
        initial_message: None,
        labels: BTreeMap::from([
            (
                "source_mob_id".to_string(),
                "nested-registry-vault".to_string(),
            ),
            ("delegate_host_identity".to_string(), "planner".to_string()),
        ]),
        spawned_by: Some("planner".to_string()),
        spawned_by_session: None,
        session_id: None,
        via_tool: "mob_spawn_member".to_string(),
    })
    .await;

    let listed = snapshot(&runtime, &AllowAllConsoleVisibilityPolicy).await;
    let everyone = experience(&listed, None);
    let impostor = row(&everyone, "impostor").ok_or("the member is listed")?;
    assert_eq!(
        impostor["member_mob_id"], "nested-registry",
        "the member whose spawn carried the labels is listed in its own mob: {impostor}"
    );
    assert!(
        impostor["labels"].get("source_mob_id").is_none(),
        "{impostor}"
    );
    assert!(
        impostor["labels"].get("delegate_host_identity").is_none(),
        "{impostor}"
    );

    // A caller whose rule selects the vault's members by the mob label is
    // listed the vault's member alone.
    let access = AccessController::new(AccessControlConfig {
        enabled: true,
        admins: vec!["root@example.test".to_string()],
        rules: vec![crate::access::AccessRule {
            id: "the-vault".to_string(),
            subjects: vec!["carol@example.test".to_string()],
            actions: vec!["agent.view".to_string()],
            match_labels: BTreeMap::from([(
                "source_mob_id".to_string(),
                "nested-registry-vault".to_string(),
            )]),
            ..Default::default()
        }],
        ..AccessControlConfig::default()
    })?;
    let carol = experience(&listed, Some(&person_under(&access, "carol")));
    assert_eq!(
        agent_ids(&carol),
        BTreeSet::from(["nested-registry-vault/keeper".to_string()]),
        "{carol}"
    );
    Ok(())
}

/// A mob whose creation record does not say which member created it
/// (several rosters named the session that created it, and the record picks
/// none) has an owner no one can read: after a restart a spawn into it is
/// refused, and the members already seated in it keep the owner their own
/// spawn gave them. The console lists such a mob at the top level, with no
/// creator named, and decides its members by who the caller is:
///
/// - with access control on, the admins the configuration names are listed
///   the mob and its members, and no other caller is listed a member that
///   has no owner of its own (an enabled configuration always names an
///   admin, so the mob can be managed);
/// - with access control off and no sign-in, the anonymous caller is the
///   console's operator and is listed them exactly as an admin is; a
///   signed-in caller of a console with access control off is listed
///   neither (such a console names no admin: side chats are private per
///   person there, and so is what no one can tell the owner of);
/// - a member with an owner of its own stays that person's and the admins',
///   with access control on or off.
///
/// On the stock policy and on one that lists every member.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mob_whose_creation_record_picks_no_creator_is_its_admins_or_its_operators() -> TestResult
{
    use super::console_side_chat_tests::{persistent_gateway_runtime_in, seeded_member};

    const TEAM: &str = "nested-undecided-team";
    const MEMBER: &str = "undecided-aide";
    const ALICES: &str = "undecided-alices-aide";
    let state = tempfile::tempdir()?;
    let runtime = persistent_gateway_runtime_in(state.path(), "nested-undecided", "", "").await?;
    seeded_member(&runtime, "worker", "planner", &[]).await?;
    let tools = runtime.agent_mob_mcp_state().ok_or("agent mob tools")?;
    let sessions = runtime
        .session_service()
        .ok_or("the runtime's session service")?
        .clone();
    let definition: MobDefinition =
        serde_json::from_value(json!({ "id": TEAM, "profiles": profiles() }))?;
    let team = meerkat_mob::MobBuilder::new(definition, MobStorage::in_memory())
        .with_session_service(Arc::clone(&sessions))
        .with_default_llm_client(Arc::new(TestClient::default()))
        .with_provenance(meerkat_mob::MobProvenance::Ambiguous {
            matching_rosters: 2,
        })
        .create()
        .await?;
    tools
        .try_mob_insert_handle(meerkat_mob::MobId::from(TEAM), team.clone())
        .await?;
    seat(&team, "worker", MEMBER, BTreeMap::new()).await?;
    seat(&team, "worker", ALICES, private_to("person:alice")).await?;

    // Access control off: a console with no access configuration.
    let off = AccessController::audience_only();
    // Access control on: every signed-in person may do everything with every
    // agent, and `root` is the admin.
    let on = AccessController::new(AccessControlConfig {
        enabled: true,
        admins: vec!["root@example.test".to_string()],
        rules: vec![crate::access::AccessRule {
            id: "everyone-every-agent".to_string(),
            subjects: vec!["*".to_string()],
            actions: vec!["agent.*".to_string()],
            agents: vec!["*".to_string()],
            ..Default::default()
        }],
        ..AccessControlConfig::default()
    })?;
    assert!(
        AccessController::new(AccessControlConfig {
            enabled: true,
            ..AccessControlConfig::default()
        })
        .is_err(),
        "access control cannot be on with no admin named"
    );
    let members = |seen: &Value| -> BTreeSet<String> {
        agent_ids(seen)
            .into_iter()
            .filter(|agent| agent.starts_with(TEAM))
            .collect()
    };
    let ownerless = format!("{TEAM}/{MEMBER}");
    let alices = format!("{TEAM}/{ALICES}");
    let both = BTreeSet::from([ownerless.clone(), alices.clone()]);

    let policies: [&dyn ConsoleVisibilityPolicy; 2] = [
        &AllowAllConsoleVisibilityPolicy,
        &HideImplicitDelegateMembersConsoleVisibilityPolicy,
    ];
    for policy in policies {
        let listed = snapshot(&runtime, policy).await;
        for (caller, view, expected) in [
            (
                "the admin of a console with access control on",
                on.view_for_caller(Some("root@example.test"), None),
                both.clone(),
            ),
            (
                "the anonymous operator of a console with access control off",
                off.view_for_caller(None, None).on_console(false),
                BTreeSet::from([ownerless.clone()]),
            ),
            (
                "a signed-in caller of a console with access control off",
                person(&off, "bob"),
                BTreeSet::new(),
            ),
            (
                "the owner of a member, signed in, access control off",
                person(&off, "alice"),
                BTreeSet::from([alices.clone()]),
            ),
            (
                "the owner of a member, access control on",
                person_under(&on, "alice"),
                BTreeSet::from([alices.clone()]),
            ),
            (
                "another signed-in person, access control on",
                person_under(&on, "bob"),
                BTreeSet::new(),
            ),
            (
                "an anonymous caller, access control on",
                on.view_for_caller(None, None),
                BTreeSet::new(),
            ),
        ] {
            let seen = experience(&listed, Some(&view));
            assert_eq!(
                members(&seen),
                expected,
                "the members of the mob {caller} is listed: {seen}"
            );
            assert_eq!(
                listed_mob(&seen, TEAM).map(placement),
                (!expected.is_empty()).then_some((
                    None,
                    None,
                    u64::try_from(expected.len()).ok(),
                    Some("running")
                )),
                "the mob as {caller} is listed it, at the top level and with no creator: {seen}"
            );
        }
    }
    Ok(())
}

/// Wait until the record of who each mob is private to has read the mob
/// `mob_id`: its own reading of a mob that reaches the mob set.
async fn the_record_has_read(runtime: &MobRuntime, mob_id: &str) -> TestResult {
    let owners = runtime.private_owners();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
    while !owners.has_read_the_mob(mob_id) {
        if tokio::time::Instant::now() >= deadline {
            return Err(format!("the record did not read the mob {mob_id}").into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    Ok(())
}

/// Leave the record of who each mob is private to as it stands for a mob it
/// has not read yet: it holds nothing for the mob `mob_id`, and the read of
/// that mob is owed until the value handed back is dropped. The record's own
/// reading of the mob, which follows the mob reaching the mob set, is waited
/// for first, so that it does not land in the middle of the row.
async fn record_that_has_not_read(
    runtime: &MobRuntime,
    mob_id: &str,
) -> TestResult<crate::private_owners::OwedMobReadInTests> {
    the_record_has_read(runtime, mob_id).await?;
    let owners = runtime.private_owners();
    owners.forget_mob_in_tests(mob_id);
    Ok(owners.owe_the_read_of_in_tests(mob_id))
}

/// A mob the gateway holds that the record of who each mob is private to
/// has not read (the record holds nothing for it and its read is owed) may
/// be private to a person. Until the record reads it, a member of it with
/// no owner of its own is an access admin's alone, or the anonymous
/// operator's on a console with neither access control nor sign-in. The
/// console's listing starts no read. Once the record has read the mob as
/// private to no one, the member is listed as any shared member.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mob_whose_owner_is_not_read_yet_is_its_admins_or_its_operators() -> TestResult {
    use super::console_side_chat_tests::{persistent_gateway_runtime_in, seeded_member};

    const TEAM: &str = "nested-unread-team";
    let state = tempfile::tempdir()?;
    let runtime = persistent_gateway_runtime_in(state.path(), "nested-unread", "", "").await?;
    seeded_member(&runtime, "worker", "planner", &[]).await?;
    assert!(
        runtime
            .private_owners()
            .seeded(&runtime.handle())
            .await
            .landed(),
        "the record's first read landed before the mob reaches the gateway"
    );
    let tools = runtime.agent_mob_mcp_state().ok_or("agent mob tools")?;
    let sessions = runtime
        .session_service()
        .ok_or("the runtime's session service")?
        .clone();
    let definition: MobDefinition =
        serde_json::from_value(json!({ "id": TEAM, "profiles": profiles() }))?;
    let team = meerkat_mob::MobBuilder::new(definition, MobStorage::in_memory())
        .with_session_service(sessions)
        .with_default_llm_client(Arc::new(TestClient::default()))
        .with_provenance(created_by_the_seated(&runtime.handle(), "planner").await?)
        .create()
        .await?;
    tools
        .try_mob_insert_handle(meerkat_mob::MobId::from(TEAM), team.clone())
        .await?;
    seat(&team, "worker", "aide", BTreeMap::new()).await?;
    let aide = format!("{TEAM}/aide");
    let owed = record_that_has_not_read(&runtime, TEAM).await?;
    let off = AccessController::audience_only();
    let policy = HideImplicitDelegateMembersConsoleVisibilityPolicy;

    let listed = snapshot(&runtime, &policy).await;
    assert!(
        !agent_ids(&experience(&listed, Some(&person(&off, "bob")))).contains(&aide),
        "a signed-in person is not listed the member of a mob whose owner is not read yet"
    );
    assert!(
        agent_ids(&experience(&listed, Some(&admin()?))).contains(&aide),
        "an access admin is"
    );
    assert!(
        agent_ids(&experience(
            &listed,
            Some(&off.view_for_caller(None, None).on_console(false))
        ))
        .contains(&aide),
        "and so is the anonymous operator of a console with access control off"
    );

    // The listing started no read: the record still holds nothing for the
    // mob. The record then reads it, as its reading of the mob set does: the
    // mob is private to no one, and a later listing shows its member to
    // everyone.
    assert!(
        runtime.private_owners().recorded_mob_owner(TEAM).is_none(),
        "the console's listing has the record read no mob"
    );
    runtime.private_owners().read_loaded_mobs().await?;
    drop(owed);
    let listed = snapshot(&runtime, &policy).await;
    assert!(
        agent_ids(&experience(&listed, Some(&person(&off, "bob")))).contains(&aide),
        "once the record read the mob as private to no one, every person is listed its member"
    );
    Ok(())
}

/// The roster priming of a runtime served under an identity namespace keeps
/// each member of the runtime's own mob decided by its own role and labels,
/// also when a nested mob's id is that namespace and seats a member of the
/// same name: the two would spell one identity, and nothing of the nested
/// member lands on the runtime's own. With the stock empty namespace the two
/// are recorded under their own names, as before.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_priming_in_a_namespace_keeps_a_primary_members_role_from_a_nested_namesake()
-> TestResult {
    const NAMESPACE: &str = "ops-priming-namespace";
    let root = tempfile::tempdir()?;
    let runtime = runtime("nested-namespace-priming", root.path()).await?;
    let primary = runtime.handle();
    seat(&primary, "worker", "planner", BTreeMap::new()).await?;
    seat(&primary, "worker", "clerk", BTreeMap::new()).await?;
    // A mob whose id is the namespace, with a member the rules deny `bob`.
    let nested = mob(
        &runtime,
        NAMESPACE,
        Some(session_of(&primary, "planner").await?),
    )
    .await?;
    seat(&nested, "worker", "clerk", vault_tier()).await?;
    let namespaced_clerk = format!("{NAMESPACE}/clerk");

    let access = bob_denied_the_vault()?;
    prime_access_cache_from_runtime_in_namespace(
        &runtime,
        &access,
        NAMESPACE,
        PrivateOwnerRecordReader::OwnRuntime,
        None,
    )
    .await;
    assert!(
        person_under(&access, "bob").can_view_agent(&namespaced_clerk),
        "the runtime's own member is decided by its own labels"
    );

    // The stock empty namespace: each under its own name.
    let stock = bob_denied_the_vault()?;
    prime_access_cache_from_runtime_in_namespace(
        &runtime,
        &stock,
        "",
        PrivateOwnerRecordReader::OwnRuntime,
        None,
    )
    .await;
    let bob = person_under(&stock, "bob");
    assert!(bob.can_view_agent("clerk"), "the runtime's own member");
    assert!(
        !bob.can_view_agent(&namespaced_clerk),
        "the nested member is decided by its own labels"
    );
    Ok(())
}

/// The access attributes the roster priming records for an agent its host
/// configures that has no member place it in no mob: a `source_mob_id` or a
/// `delegate_host_identity` its configuration carries is dropped, so a rule
/// that selects a nested mob's members by the mob label does not match the
/// agent. Its role and a label of its own stay.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_access_attributes_of_an_agent_with_no_member_carry_no_console_mob_label() -> TestResult
{
    let root = tempfile::tempdir()?;
    let runtime = runtime("nested-unseated-labels", root.path()).await?;
    let core_bridge = Arc::new(crate::identity_first::core_test_bridge::CoreReadsBridge::default());
    let identities =
        crate::identity_first::IdentityRuntime::new(crate::identity_first::IdentityRuntimeConfig {
            continuity_store: Arc::new(
                crate::identity_first::LocalContinuityStore::in_memory().expect("continuity store"),
            ),
            lease_provider: Arc::new(crate::identity_first::LocalLeaseProvider::new()),
            runtime_instance_id: "nested-unseated-labels".to_string(),
            has_runtime_store: true,
            durability_policy: crate::identity_first::DurabilityPolicy::SyncWriteThrough,
            bridge: Some(core_bridge),
            default_timeout: None,
        });
    identities
        .register(crate::identity_first::DurableAgentSpec {
            identity: crate::identity_first::AgentIdentity::parse("dormant").expect("identity"),
            profile: ProfileName::from("worker"),
            addressability: crate::identity_first::AgentAddressability::Addressable,
            display_name: None,
            labels: BTreeMap::from([
                ("source_mob_id".to_string(), "vault".to_string()),
                ("delegate_host_identity".to_string(), "planner".to_string()),
                ("tier".to_string(), "public".to_string()),
            ]),
            context: None,
            additional_instructions: Vec::new(),
            initial_message: None,
            runtime_mode_override: None,
            backend: None,
            binding: None,
            placement: None,
        })
        .await
        .expect("the agent registers");

    let attributes = unseated_agent_access_attributes(
        &runtime.handle(),
        Some(&identities),
        &BTreeSet::new(),
        &BTreeMap::new(),
    )
    .await;
    let dormant = attributes
        .iter()
        .find(|attributes| attributes.identity == "dormant")
        .ok_or("the agent with no member is recorded")?;
    assert_eq!(dormant.role.as_deref(), Some("worker"), "{dormant:?}");
    assert_eq!(
        dormant.labels.get("tier").map(String::as_str),
        Some("public"),
        "a label of its own stays: {dormant:?}"
    );
    for key in crate::console_spawn::CONSOLE_MOB_LABELS {
        assert!(
            !dormant.labels.contains_key(key),
            "the attributes carry no `{key}`: {dormant:?}"
        );
    }
    Ok(())
}

/// What the roster priming records in a namespace, given members as a walk
/// could hand them over: a member of the runtime's own mob keeps its value
/// under its namespaced identity whatever a nested member spells there, and
/// an ended member the spawn registry holds for a mob named as the namespace
/// is recorded under no identity. A nested member under an identity of its
/// own is recorded as before.
#[test]
fn the_roster_priming_records_no_nested_value_under_a_primary_members_namespaced_identity()
-> TestResult {
    let attributes =
        |identity: &str, alias: &str, labels: BTreeMap<String, String>| AgentResourceAttributes {
            identity: identity.to_string(),
            agent_id: Some(alias.to_string()),
            role: Some("worker".to_string()),
            labels,
        };
    let nested_in = |mob_id: &str| {
        let mut labels = vault_tier();
        labels.insert("source_mob_id".to_string(), mob_id.to_string());
        labels
    };
    let access = bob_denied_the_vault()?;
    let mut registered = crate::console_spawn::SpawnRegistry::default();
    registered.register(
        "ops",
        "ops/usher",
        "usher",
        BTreeMap::from([("via_tool".to_string(), "mob_spawn_member".to_string())]),
    );
    record_live_and_registered_attributes(
        &access,
        vec![
            attributes("clerk", "rt:clerk:1", BTreeMap::new()),
            attributes("ops/clerk", "ops/clerk", nested_in("ops")),
            attributes("desk/aide", "desk/aide", nested_in("desk")),
        ],
        &registered,
        &BTreeSet::new(),
        &BTreeSet::from(["clerk".to_string()]),
        &BTreeMap::new(),
        "ops",
    );
    let bob = person_under(&access, "bob");
    assert!(
        bob.can_view_agent("ops/clerk"),
        "the runtime's own member keeps its value under its namespaced identity"
    );
    assert!(
        bob.knows_agent("ops/desk/aide") && !bob.can_view_agent("ops/desk/aide"),
        "a nested member under an identity of its own is recorded by its own labels"
    );
    assert!(
        !bob.knows_agent("ops/usher"),
        "nothing is recorded for an ended member of a mob named as the namespace"
    );
    Ok(())
}

/// An ended member of the console's own mob whose name holds a `/` takes no
/// mob from its name. The spawn registry keeps its entry under the console's
/// own mob: it is recorded as an ended member of that mob, with no label of
/// a mob it never sat in, so a grant on the mob its name could spell shows
/// nothing of it.
#[test]
fn an_ended_member_of_the_consoles_own_mob_takes_no_mob_from_a_name_that_holds_a_slash()
-> TestResult {
    const NAME: &str = "ghost-team/helper";
    const ALICE: &str = "alice@example.test";
    const DAVE: &str = "dave@example.test";
    let access = AccessController::new(alice_drives_every_agent_and(vec![
        crate::access::AccessRule {
            match_labels: BTreeMap::from([("source_mob_id".to_string(), "ghost-team".to_string())]),
            ..agent_rule("dave-is-granted-the-ghost-team", DAVE)
        },
    ]))?;
    let mut registered = crate::console_spawn::SpawnRegistry::default();
    registered.register(
        "own-mob",
        NAME,
        NAME,
        BTreeMap::from([("via_tool".to_string(), "mob_spawn_member".to_string())]),
    );
    record_live_and_registered_attributes(
        &access,
        Vec::new(),
        &registered,
        &BTreeSet::new(),
        &BTreeSet::new(),
        &BTreeMap::from([(NAME.to_string(), None)]),
        "",
    );
    let sees = |subject: &str| {
        access
            .view_for_caller(Some(subject), Some(&format!("person:{subject}")))
            .can_view_agent(NAME)
    };
    assert!(
        sees(ALICE),
        "the ended member is known: a person allowed every agent sees it"
    );
    assert!(
        !sees(DAVE),
        "a grant on the mob its name could spell shows nothing of a member that never sat in it"
    );
    Ok(())
}

/// An ended member of the console's own mob whose name holds a `/` keeps the
/// owner the record of who each member is private to holds for it. The
/// spawn registry keeps its entry under the console's own mob, and no mob
/// of the id its name starts with exists: the roster priming does not take
/// it for a member of such a mob, so it stays its owner's and is hidden
/// from another person.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_ended_member_of_the_consoles_own_mob_whose_name_holds_a_slash_keeps_its_recorded_owner()
-> TestResult {
    const NAME: &str = "ghost-team/helper";
    let root = tempfile::tempdir()?;
    let runtime = runtime("slash-name-own-mob", root.path()).await?;
    let primary = runtime.handle();
    seat(&primary, "worker", "planner", BTreeMap::new()).await?;
    assert!(
        runtime.private_owners().seeded(&primary).await.landed(),
        "the record's first read landed"
    );
    // The record knows the member as its build made it: private to alice.
    runtime.private_owners().record(
        primary.mob_id().as_str(),
        NAME,
        Some(&BTreeMap::from([(
            crate::access::PRIVATE_TO_LABEL.to_string(),
            "person:alice".to_string(),
        )])),
    );
    // The entry a spawn registered for it, in the console's own mob.
    let sink = crate::console_spawn::ConsoleSpawnSink::for_mob(
        ConsoleEventStore::new(),
        primary.mob_id().as_str(),
    );
    runtime.install_console_spawn_sink(sink.clone());
    sink.project_spawned_member(&crate::console_spawn::ConsoleSpawnSeed {
        mob_id: Some(primary.mob_id().to_string()),
        member_id: NAME.to_string(),
        identity: NAME.to_string(),
        initial_message: None,
        labels: BTreeMap::new(),
        spawned_by: None,
        spawned_by_session: None,
        session_id: None,
        via_tool: "mob_spawn_member".to_string(),
    })
    .await;
    assert!(
        runtime.console_identity_labels().await.holds_identity(NAME),
        "the spawn registry holds the member's entry"
    );

    let off = AccessController::audience_only();
    prime_access_cache_from_runtime(&runtime, &off).await;
    assert!(
        !person(&off, "alice").hides_agent(NAME),
        "the ended member is still its owner's"
    );
    assert!(
        person(&off, "bob").hides_agent(NAME),
        "another person is kept from it"
    );
    Ok(())
}

/// Where the two rules meet. A runtime served under a namespace holds a
/// nested mob whose id is that namespace, and an ended member of its own
/// mob whose name starts with that id and a `/`. The spawn registry's
/// entries of the nested mob are of a mob the namespace claims: they are
/// recorded by neither reader. The own mob's ended member is no member of
/// that mob: its entry stays, and it keeps the owner its record holds,
/// whichever reader primes last.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_ended_own_member_named_into_a_mob_the_namespace_claims_keeps_its_recorded_owner()
-> TestResult {
    const NAMESPACE: &str = "ops-claimed-namespace";
    let own_name = format!("{NAMESPACE}/helper");
    let usher = format!("{NAMESPACE}/usher");
    let root = tempfile::tempdir()?;
    let runtime = runtime("nested-claimed-and-named", root.path()).await?;
    let primary = runtime.handle();
    seat(&primary, "worker", "planner", BTreeMap::new()).await?;
    assert!(
        runtime.private_owners().seeded(&primary).await.landed(),
        "the record's first read landed"
    );
    let nested = mob(
        &runtime,
        NAMESPACE,
        Some(session_of(&primary, "planner").await?),
    )
    .await?;
    seat(&nested, "worker", "clerk", BTreeMap::new()).await?;
    // The record knows the own mob's member as its build made it: private
    // to alice.
    runtime.private_owners().record(
        primary.mob_id().as_str(),
        &own_name,
        Some(&BTreeMap::from([(
            crate::access::PRIVATE_TO_LABEL.to_string(),
            "person:alice".to_string(),
        )])),
    );
    // The entries a spawn registered: one for a member of the nested mob,
    // one for the own mob's member.
    let sink = crate::console_spawn::ConsoleSpawnSink::for_mob(
        ConsoleEventStore::new(),
        primary.mob_id().as_str(),
    );
    runtime.install_console_spawn_sink(sink.clone());
    for (mob_id, name) in [
        (NAMESPACE.to_string(), "usher"),
        (primary.mob_id().to_string(), own_name.as_str()),
    ] {
        sink.project_spawned_member(&crate::console_spawn::ConsoleSpawnSeed {
            mob_id: Some(mob_id),
            member_id: name.to_string(),
            identity: name.to_string(),
            initial_message: None,
            labels: BTreeMap::new(),
            spawned_by: None,
            spawned_by_session: None,
            session_id: None,
            via_tool: "mob_spawn_member".to_string(),
        })
        .await;
    }
    let aggregator = crate::console_aggregator::MetaConsoleAggregator::in_memory();
    aggregator.register_runtime_handles_with_policy(
        "served",
        NAMESPACE,
        runtime.clone(),
        None,
        ConsoleEventStore::new(),
        Arc::new(HideImplicitDelegateMembersConsoleVisibilityPolicy),
    );

    for own_console_first in [true, false] {
        let off = AccessController::audience_only();
        if own_console_first {
            prime_access_cache_from_runtime(&runtime, &off).await;
            prime_access_cache_from_aggregator(&aggregator, &off).await;
        } else {
            prime_access_cache_from_aggregator(&aggregator, &off).await;
            prime_access_cache_from_runtime(&runtime, &off).await;
        }
        assert!(
            !person(&off, "alice").hides_agent(&own_name),
            "the own mob's ended member is still its owner's (own console first: \
             {own_console_first})"
        );
        assert!(
            person(&off, "bob").hides_agent(&own_name),
            "another person is kept from it (own console first: {own_console_first})"
        );
        assert!(
            !person(&off, "bob").knows_agent(&usher),
            "the ended member of the mob the namespace claims is recorded by neither reader \
             (own console first: {own_console_first})"
        );
    }
    Ok(())
}

/// In the access cache every route primes, a member of a mob private to a
/// person is that person's whatever its own row says, while it is seated
/// and after it ended: a member seated with no private owner of its own in
/// a mob a private member created is hidden from another person and not
/// from the mob's owner, and stays so once it left its roster and only the
/// spawn registry holds it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unlabelled_member_of_a_private_mob_is_its_owners_in_the_access_cache_seated_and_ended()
-> TestResult {
    use super::console_side_chat_tests::{persistent_gateway_runtime_in, seeded_member};

    const TEAM: &str = "nested-private-cache-team";
    let state = tempfile::tempdir()?;
    let runtime =
        persistent_gateway_runtime_in(state.path(), "nested-private-cache", "", "").await?;
    let primary = runtime.handle();
    // The creator: a member of the runtime's own mob, private to alice.
    seeded_member(
        &runtime,
        "worker",
        "keeper",
        &[(crate::access::PRIVATE_TO_LABEL, "person:alice")],
    )
    .await?;
    assert!(
        runtime.private_owners().seeded(&primary).await.landed(),
        "the record's first read landed before the mob reaches the gateway"
    );
    let tools = runtime.agent_mob_mcp_state().ok_or("agent mob tools")?;
    let sessions = runtime
        .session_service()
        .ok_or("the runtime's session service")?
        .clone();
    let definition: MobDefinition =
        serde_json::from_value(json!({ "id": TEAM, "profiles": profiles() }))?;
    let team = meerkat_mob::MobBuilder::new(definition, MobStorage::in_memory())
        .with_session_service(sessions)
        .with_default_llm_client(Arc::new(TestClient::default()))
        .with_provenance(meerkat_mob::MobProvenance::OperatorMember {
            parent_mob_id: primary.mob_id().clone(),
            parent_incarnation: primary.incarnation(),
            creator_identity: AgentIdentity::from("keeper"),
            creator_generation: meerkat_mob::Generation::INITIAL,
            owner_session: session_of(&primary, "keeper").await?,
        })
        .create()
        .await?;
    tools
        .try_mob_insert_handle(meerkat_mob::MobId::from(TEAM), team.clone())
        .await?;
    // A member whose own spawn gave it no private owner, and the registry's
    // record of that spawn.
    seat(&team, "worker", "aide", BTreeMap::new()).await?;
    runtime.private_owners().read_mob(&team).await?;
    let sink = crate::console_spawn::ConsoleSpawnSink::for_mob(
        ConsoleEventStore::new(),
        primary.mob_id().as_str(),
    );
    runtime.install_console_spawn_sink(sink.clone());
    sink.project_spawned_member(&crate::console_spawn::ConsoleSpawnSeed {
        mob_id: Some(TEAM.to_string()),
        member_id: "aide".to_string(),
        identity: "aide".to_string(),
        initial_message: None,
        labels: BTreeMap::new(),
        spawned_by: None,
        spawned_by_session: None,
        session_id: None,
        via_tool: "mob_spawn_member".to_string(),
    })
    .await;
    let aide = format!("{TEAM}/aide");
    let access = AccessController::audience_only();

    prime_access_cache_from_runtime(&runtime, &access).await;
    assert!(
        person(&access, "bob").hides_agent(&aide),
        "while it is seated another person is kept from it"
    );
    assert!(!person(&access, "alice").hides_agent(&aide));

    let identity = AgentIdentity::from("aide");
    team.retire(identity.clone()).await?;
    for _ in 0..100 {
        if team.get_member(&identity).await?.is_none() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        team.get_member(&identity).await?.is_none(),
        "the member left its roster"
    );
    prime_access_cache_from_runtime(&runtime, &access).await;
    assert!(
        person(&access, "bob").knows_agent(&aide) && person(&access, "bob").hides_agent(&aide),
        "after it ended another person is still kept from it"
    );
    assert!(
        !person(&access, "alice").hides_agent(&aide),
        "its owner is not"
    );
    Ok(())
}

/// A member of a nested mob that is private to a person stays that person's
/// in the access cache after it ended: the spawn registry keeps it under its
/// mob and name, and the record of who each member is private to is read
/// under that same key. Another person is kept from it while it is seated
/// and after it left; its owner is not.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_ended_private_member_of_a_nested_mob_keeps_its_owner_in_the_access_cache() -> TestResult
{
    const DIARY: &str = "nested-ended-diary";
    let root = tempfile::tempdir()?;
    let runtime = runtime("nested-ended", root.path()).await?;
    let primary = runtime.handle();
    seat(&primary, "worker", "planner", BTreeMap::new()).await?;
    let diary = mob(
        &runtime,
        DIARY,
        Some(session_of(&primary, "planner").await?),
    )
    .await?;
    seat(&diary, "worker", "diarist", private_to("person:alice")).await?;
    // The registry holds the member's spawn, as an agent's spawn tool call
    // records it: under its mob and name.
    let sink = crate::console_spawn::ConsoleSpawnSink::for_mob(
        ConsoleEventStore::new(),
        primary.mob_id().as_str(),
    );
    runtime.install_console_spawn_sink(sink.clone());
    sink.project_spawned_member(&crate::console_spawn::ConsoleSpawnSeed {
        mob_id: Some(DIARY.to_string()),
        member_id: "diarist".to_string(),
        identity: "diarist".to_string(),
        initial_message: None,
        labels: BTreeMap::new(),
        spawned_by: Some("planner".to_string()),
        spawned_by_session: None,
        session_id: None,
        via_tool: "mob_spawn_member".to_string(),
    })
    .await;
    let diarist = format!("{DIARY}/diarist");
    assert!(
        runtime
            .console_identity_labels()
            .await
            .holds_identity(&diarist),
        "the registry keeps the member under its mob and name"
    );
    let access = AccessController::audience_only();

    prime_access_cache_from_runtime(&runtime, &access).await;
    assert!(
        person(&access, "bob").hides_agent(&diarist),
        "while it is seated another person is kept from it"
    );
    assert!(!person(&access, "alice").hides_agent(&diarist));

    // The member ends.
    let identity = AgentIdentity::from("diarist");
    diary.retire(identity.clone()).await?;
    for _ in 0..100 {
        if diary.get_member(&identity).await?.is_none() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        diary.get_member(&identity).await?.is_none(),
        "the member left its roster"
    );
    prime_access_cache_from_runtime(&runtime, &access).await;
    assert!(
        person(&access, "bob").hides_agent(&diarist),
        "after it ended another person is still kept from it"
    );
    assert!(
        !person(&access, "alice").hides_agent(&diarist),
        "its owner is not"
    );
    Ok(())
}

/// The roster priming in a namespace of more than one component records
/// nothing for an ended member the spawn registry holds for a mob whose id
/// is the namespace's first component: such a mob is no console mob there.
#[test]
fn the_roster_priming_records_no_ended_member_of_a_mob_named_as_the_namespaces_first_component()
-> TestResult {
    let access = bob_denied_the_vault()?;
    let mut registered = crate::console_spawn::SpawnRegistry::default();
    registered.register(
        "ops",
        "ops/usher",
        "usher",
        BTreeMap::from([("via_tool".to_string(), "mob_spawn_member".to_string())]),
    );
    record_live_and_registered_attributes(
        &access,
        Vec::new(),
        &registered,
        &BTreeSet::new(),
        &BTreeSet::new(),
        &BTreeMap::new(),
        "ops/east",
    );
    let bob = person_under(&access, "bob");
    for spelling in ["ops/east/ops/usher", "ops/usher"] {
        assert!(
            !bob.knows_agent(spelling),
            "nothing is recorded under {spelling}"
        );
    }
    Ok(())
}

/// Who is served a member of a mob whose owner cannot be told is decided by
/// the console, never by whether the caller carries a subject. On a console
/// that can identify a person, with access control off, the caller who sent
/// no token is refused such a member exactly as a signed-in person who is
/// not its owner is: neither is listed it, and it is hidden from both. Only
/// the one operator of a console that identifies nobody is served it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_caller_with_no_subject_is_refused_a_mob_of_unknown_owner_as_a_signed_in_person_is()
-> TestResult {
    use super::console_side_chat_tests::{persistent_gateway_runtime_in, seeded_member};

    const TEAM: &str = "nested-no-subject-team";
    let state = tempfile::tempdir()?;
    let runtime = persistent_gateway_runtime_in(state.path(), "nested-no-subject", "", "").await?;
    seeded_member(&runtime, "worker", "planner", &[]).await?;
    assert!(
        runtime
            .private_owners()
            .seeded(&runtime.handle())
            .await
            .landed(),
        "the record's first read landed before the mob reaches the gateway"
    );
    let tools = runtime.agent_mob_mcp_state().ok_or("agent mob tools")?;
    let sessions = runtime
        .session_service()
        .ok_or("the runtime's session service")?
        .clone();
    let definition: MobDefinition =
        serde_json::from_value(json!({ "id": TEAM, "profiles": profiles() }))?;
    let team = meerkat_mob::MobBuilder::new(definition, MobStorage::in_memory())
        .with_session_service(sessions)
        .with_default_llm_client(Arc::new(TestClient::default()))
        .with_provenance(created_by_the_seated(&runtime.handle(), "planner").await?)
        .create()
        .await?;
    tools
        .try_mob_insert_handle(meerkat_mob::MobId::from(TEAM), team.clone())
        .await?;
    seat(&team, "worker", "aide", BTreeMap::new()).await?;
    let aide = format!("{TEAM}/aide");
    let _owed = record_that_has_not_read(&runtime, TEAM).await?;
    let off = AccessController::audience_only();
    let listed = snapshot(
        &runtime,
        &HideImplicitDelegateMembersConsoleVisibilityPolicy,
    )
    .await;
    prime_access_cache_from_runtime(&runtime, &off).await;

    // A console that can identify a person: the signed-in person and the
    // caller with no subject.
    let signed_in = person(&off, "bob");
    let no_subject = off.view_for_caller(None, None).on_console(true);
    for (caller, view) in [
        ("a signed-in person", &signed_in),
        ("the caller with no subject", &no_subject),
    ] {
        assert!(
            !agent_ids(&experience(&listed, Some(view))).contains(&aide),
            "{caller} is not listed the member"
        );
        assert!(
            view.hides_agent(&aide),
            "the member is hidden from {caller}"
        );
    }

    // A console that identifies nobody: its one operator.
    let operator = off.view_for_caller(None, None).on_console(false);
    assert!(
        agent_ids(&experience(&listed, Some(&operator))).contains(&aide),
        "the one operator of a console that identifies nobody is listed the member"
    );
    assert!(!operator.hides_agent(&aide));
    Ok(())
}

/// A mob whose creation record names a creating member that nothing says
/// the owner of (that member's mob is not hosted) may be private to a
/// person: the record does not take it to be private to no one. The console
/// does not list such a mob (its recorded parent is not listed), so a
/// member seated in it is recorded as one the console does not list, with
/// what is not known of it: it is hidden from a signed-in person and is not
/// recorded as a shared member. The one operator of a console that
/// identifies nobody is not kept from it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_of_a_mob_whose_creating_members_owner_nothing_says_is_no_shared_member()
-> TestResult {
    use super::console_side_chat_tests::{persistent_gateway_runtime_in, seeded_member};

    const TEAM: &str = "nested-creator-unknown-team";
    let state = tempfile::tempdir()?;
    let runtime =
        persistent_gateway_runtime_in(state.path(), "nested-creator-unknown", "", "").await?;
    seeded_member(&runtime, "worker", "planner", &[]).await?;
    assert!(
        runtime
            .private_owners()
            .seeded(&runtime.handle())
            .await
            .landed(),
        "the record's first read landed before the mob reaches the gateway"
    );
    let tools = runtime.agent_mob_mcp_state().ok_or("agent mob tools")?;
    let sessions = runtime
        .session_service()
        .ok_or("the runtime's session service")?
        .clone();
    let definition: MobDefinition =
        serde_json::from_value(json!({ "id": TEAM, "profiles": profiles() }))?;
    let team = meerkat_mob::MobBuilder::new(definition, MobStorage::in_memory())
        .with_session_service(sessions)
        .with_default_llm_client(Arc::new(TestClient::default()))
        .with_provenance(meerkat_mob::MobProvenance::OperatorMember {
            parent_mob_id: meerkat_mob::MobId::from("a-mob-nothing-hosts"),
            parent_incarnation: meerkat_core::service::MobIncarnationId::mint(),
            creator_identity: AgentIdentity::from("creator"),
            creator_generation: meerkat_mob::Generation::INITIAL,
            owner_session: SessionId::new(),
        })
        .create()
        .await?;
    tools
        .try_mob_insert_handle(meerkat_mob::MobId::from(TEAM), team.clone())
        .await?;
    seat(&team, "worker", "aide", BTreeMap::new()).await?;
    let aide = format!("{TEAM}/aide");
    the_record_has_read(&runtime, TEAM).await?;

    let off = AccessController::audience_only();
    let listed = snapshot(
        &runtime,
        &HideImplicitDelegateMembersConsoleVisibilityPolicy,
    )
    .await;
    prime_access_cache_from_runtime(&runtime, &off).await;
    let signed_in = person(&off, "bob");
    assert!(
        !agent_ids(&experience(&listed, Some(&signed_in))).contains(&aide),
        "no caller is listed a member of a mob the console does not list"
    );
    assert!(
        signed_in.hides_agent(&aide),
        "the member is hidden from a signed-in person"
    );
    let operator = off.view_for_caller(None, None).on_console(false);
    assert!(
        !operator.hides_agent(&aide),
        "the one operator of a console that identifies nobody is not kept from it"
    );
    Ok(())
}

/// One access controller primed by a runtime's own console and by a console
/// that serves the same runtime under a namespace holds one value under
/// each identity, whichever primes last. A nested mob whose id is that
/// namespace is listed by neither reader while the runtime is served under
/// it, so `<namespace>/<name>` is the runtime's own member in both, and an
/// ended member the spawn registry holds for that mob is recorded by
/// neither. Once the runtime is served under the namespace no longer, its
/// own console lists the mob again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mob_named_as_a_namespace_its_runtime_is_served_under_is_listed_by_no_reader()
-> TestResult {
    const NAMESPACE: &str = "ops-served-namespace";
    let root = tempfile::tempdir()?;
    let runtime = runtime("nested-served-namespace", root.path()).await?;
    let primary = runtime.handle();
    seat(&primary, "worker", "planner", BTreeMap::new()).await?;
    seat(&primary, "worker", "clerk", BTreeMap::new()).await?;
    // A mob whose id is the namespace, with a member the rules deny `bob`,
    // and the registry's record of a member of it that has ended.
    let nested = mob(
        &runtime,
        NAMESPACE,
        Some(session_of(&primary, "planner").await?),
    )
    .await?;
    seat(&nested, "worker", "clerk", vault_tier()).await?;
    let sink = crate::console_spawn::ConsoleSpawnSink::for_mob(
        ConsoleEventStore::new(),
        primary.mob_id().as_str(),
    );
    runtime.install_console_spawn_sink(sink.clone());
    sink.project_spawned_member(&crate::console_spawn::ConsoleSpawnSeed {
        mob_id: Some(NAMESPACE.to_string()),
        member_id: "usher".to_string(),
        identity: "usher".to_string(),
        initial_message: None,
        labels: BTreeMap::new(),
        spawned_by: None,
        spawned_by_session: None,
        session_id: None,
        via_tool: "mob_spawn_member".to_string(),
    })
    .await;
    let aggregator = crate::console_aggregator::MetaConsoleAggregator::in_memory();
    aggregator.register_runtime_handles_with_policy(
        "served",
        NAMESPACE,
        runtime.clone(),
        None,
        ConsoleEventStore::new(),
        Arc::new(HideImplicitDelegateMembersConsoleVisibilityPolicy),
    );
    let clerk = format!("{NAMESPACE}/clerk");
    let usher = format!("{NAMESPACE}/usher");

    for own_console_first in [true, false] {
        let access = bob_denied_the_vault()?;
        if own_console_first {
            prime_access_cache_from_runtime(&runtime, &access).await;
            prime_access_cache_from_aggregator(&aggregator, &access).await;
        } else {
            prime_access_cache_from_aggregator(&aggregator, &access).await;
            prime_access_cache_from_runtime(&runtime, &access).await;
        }
        let bob = person_under(&access, "bob");
        assert!(
            bob.can_view_agent(&clerk),
            "the runtime's own member holds the identity (own console first: {own_console_first})"
        );
        assert!(
            !bob.knows_agent(&usher),
            "the ended member of the mob is recorded by neither reader (own console first: \
             {own_console_first})"
        );
    }

    // Served under the namespace no longer: the runtime's own console lists
    // the mob again, and its member is decided by its own labels.
    aggregator.unregister_runtime("served");
    let access = bob_denied_the_vault()?;
    prime_access_cache_from_runtime(&runtime, &access).await;
    let bob = person_under(&access, "bob");
    assert!(bob.can_view_agent("clerk"), "the runtime's own member");
    assert!(
        bob.knows_agent(&clerk) && !bob.can_view_agent(&clerk),
        "the nested member is listed again and decided by its own labels"
    );
    Ok(())
}

/// One runtime served under a namespace and under a longer one that starts
/// with it (`<outer>` and `<outer>/<inner>`), beside a nested mob whose id
/// is the longer one's last component. The reader of the shorter namespace
/// would name that mob's member `<outer>/<inner>/<name>`, and that is the
/// runtime's own member `<name>` for the reader of the longer one. The mob
/// is listed by no reader while the runtime is served under the longer
/// namespace, so one access controller primed by both readers holds the
/// runtime's own member under that identity, whichever primes last. Once
/// the runtime is served under the longer namespace no longer, the reader
/// of the shorter one lists the mob again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mob_named_as_a_later_component_of_a_namespace_its_runtime_is_served_under_is_listed_by_no_reader()
-> TestResult {
    const OUTER: &str = "ops-two-readers";
    const INNER: &str = "east-two-readers";
    let longer = format!("{OUTER}/{INNER}");
    let root = tempfile::tempdir()?;
    let runtime = runtime("nested-two-namespaces", root.path()).await?;
    let primary = runtime.handle();
    seat(&primary, "worker", "planner", BTreeMap::new()).await?;
    seat(&primary, "worker", "clerk", BTreeMap::new()).await?;
    // A mob whose id is the longer namespace's last component, with a
    // member the rules deny `bob`.
    let nested = mob(
        &runtime,
        INNER,
        Some(session_of(&primary, "planner").await?),
    )
    .await?;
    seat(&nested, "worker", "clerk", vault_tier()).await?;
    let outer = crate::console_aggregator::MetaConsoleAggregator::in_memory();
    outer.register_runtime_handles_with_policy(
        "outer",
        OUTER,
        runtime.clone(),
        None,
        ConsoleEventStore::new(),
        Arc::new(HideImplicitDelegateMembersConsoleVisibilityPolicy),
    );
    let inner = crate::console_aggregator::MetaConsoleAggregator::in_memory();
    inner.register_runtime_handles_with_policy(
        "inner",
        &longer,
        runtime.clone(),
        None,
        ConsoleEventStore::new(),
        Arc::new(HideImplicitDelegateMembersConsoleVisibilityPolicy),
    );
    // The identity both readers would compose.
    let shared = format!("{longer}/clerk");

    for outer_first in [true, false] {
        let access = bob_denied_the_vault()?;
        if outer_first {
            prime_access_cache_from_aggregator(&outer, &access).await;
            prime_access_cache_from_aggregator(&inner, &access).await;
        } else {
            prime_access_cache_from_aggregator(&inner, &access).await;
            prime_access_cache_from_aggregator(&outer, &access).await;
        }
        let bob = person_under(&access, "bob");
        assert!(
            bob.can_view_agent(&shared),
            "the runtime's own member holds the identity (the shorter namespace first: \
             {outer_first})"
        );
    }

    // Served under the longer namespace no longer: the reader of the
    // shorter one lists the mob again, and its member is decided by its own
    // labels.
    inner.unregister_runtime("inner");
    let access = bob_denied_the_vault()?;
    prime_access_cache_from_aggregator(&outer, &access).await;
    let bob = person_under(&access, "bob");
    assert!(
        bob.can_view_agent(&format!("{OUTER}/clerk")),
        "the runtime's own member"
    );
    assert!(
        bob.knows_agent(&shared) && !bob.can_view_agent(&shared),
        "the nested member is listed again and decided by its own labels"
    );
    Ok(())
}

/// A registration that replaces another under the same runtime key ends the
/// replaced one: the runtime is served under the new namespace and under
/// the replaced one no longer, so a mob the replaced namespace claimed is
/// claimed no longer. Once the runtime is unregistered neither namespace
/// claims a mob.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_replaced_registration_ends_what_its_namespace_claimed() -> TestResult {
    let root = tempfile::tempdir()?;
    let runtime = runtime("nested-replaced-registration", root.path()).await?;
    let records = runtime.console_mob_records();
    let aggregator = crate::console_aggregator::MetaConsoleAggregator::in_memory();
    let register_under = |namespace: &str| {
        aggregator.register_runtime_handles_with_policy(
            "served",
            namespace,
            runtime.clone(),
            None,
            ConsoleEventStore::new(),
            Arc::new(HideImplicitDelegateMembersConsoleVisibilityPolicy),
        );
    };
    register_under("ops");
    assert!(records.claims_mob("", "ops"));
    assert!(!records.claims_mob("", "east"));

    register_under("east");
    assert!(
        !records.claims_mob("", "ops"),
        "the replaced registration's namespace claims its mob no longer"
    );
    assert!(records.claims_mob("", "east"));

    aggregator.unregister_runtime("served");
    assert!(!records.claims_mob("", "ops"));
    assert!(
        !records.claims_mob("", "east"),
        "no namespace claims a mob once the runtime is served no longer"
    );
    Ok(())
}

/// A mob created again under the id of a destroyed mob is the mob of the
/// member that created it this time: who the destroyed mob was private to
/// says nothing about it. One person's agent creates a mob and it is
/// destroyed; another person's agent creates a mob under the same id. A
/// member seated in it is the second person's: that person is listed it,
/// and the first person is not.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mob_created_again_under_a_destroyed_mobs_id_is_its_new_creators() -> TestResult {
    const TEAM: &str = "nested-recreated-team";
    let root = tempfile::tempdir()?;
    let runtime = runtime("nested-recreated", root.path()).await?;
    let primary = runtime.handle();
    seat(
        &primary,
        "worker",
        "alices-agent",
        private_to("person:alice"),
    )
    .await?;
    seat(&primary, "worker", "bobs-agent", private_to("person:bob")).await?;
    let state = runtime
        .agent_mob_mcp_state()
        .ok_or("the runtime carries agent mob tools")?;

    // The first mob under the id, created by alice's agent, then destroyed.
    let first = mob(
        &runtime,
        TEAM,
        Some(session_of(&primary, "alices-agent").await?),
    )
    .await?;
    seat(&first, "worker", "aide", BTreeMap::new()).await?;
    drop(first);
    Box::pin(state.mob_destroy(&meerkat_mob::MobId::from(TEAM)))
        .await
        .map_err(|error| format!("the first mob is destroyed: {error:?}"))?;

    // The second mob under the same id, created by bob's agent.
    let second = mob(
        &runtime,
        TEAM,
        Some(session_of(&primary, "bobs-agent").await?),
    )
    .await?;
    seat(&second, "worker", "aide", BTreeMap::new()).await?;
    let seated = second
        .get_member(&AgentIdentity::from("aide"))
        .await?
        .ok_or("the aide is seated")?;
    assert_eq!(
        seated
            .labels
            .get(crate::access::PRIVATE_TO_LABEL)
            .map(String::as_str),
        Some("person:bob"),
        "a member seated in the mob is its new creator's person's: {:?}",
        seated.labels
    );

    let aide = format!("{TEAM}/aide");
    let audience = AccessController::audience_only();
    let listed = snapshot(
        &runtime,
        &HideImplicitDelegateMembersConsoleVisibilityPolicy,
    )
    .await;
    assert!(
        agent_ids(&experience(&listed, Some(&person(&audience, "bob")))).contains(&aide),
        "the person whose agent created the mob is listed its member"
    );
    assert!(
        !agent_ids(&experience(&listed, Some(&person(&audience, "alice")))).contains(&aide),
        "the person the destroyed mob was private to is not"
    );
    Ok(())
}

/// The console schedule methods that still answer for a schedule of a member
/// that ended: the listing, and the two that act on a schedule by its id.
const ENDED_MEMBER_SCHEDULE_METHODS: [&str; 3] = [
    "meta/schedule/list",
    "meta/schedule/pause",
    "meta/schedule/delete",
];

/// A console schedule for `member`, stored in `service` as the console
/// stores one a person defined: its text is `scheduled-for-<member>`. The
/// schedule's id.
async fn schedule_for(service: &meerkat::ScheduleService, member: &str) -> TestResult<String> {
    // The schedule is the console's own mob's: its send names no mob.
    let default_mob = crate::member_key::DefaultMob::new(meerkat_mob::MobId::from("launch"));
    let send = crate::console_schedules::ConsoleScheduledSend::new(
        &default_mob,
        default_mob.mob_id(),
        member.to_string(),
        format!("scheduled-for-{member}"),
        crate::console_schedules::ConsoleScheduleSender::Human {
            principal: "alice@example.test".to_string(),
            provider: "google_oauth".to_string(),
            issuer: "https://issuer.example.test".to_string(),
            subject: "oidc-sub-alice".to_string(),
        },
        1,
    );
    let schedule = service
        .create(meerkat::CreateScheduleRequest {
            name: None,
            description: None,
            trigger: serde_json::from_value(json!({
                "type": "interval",
                "start_at_utc": (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
                "every_seconds": 3600,
            }))?,
            target: send.target()?,
            misfire_policy: meerkat::MisfirePolicy::default(),
            overlap_policy: meerkat::OverlapPolicy::default(),
            missing_target_policy: meerkat::MissingTargetPolicy::default(),
            labels: BTreeMap::new(),
            planning_horizon_days: None,
            planning_horizon_occurrences: None,
        })
        .await?;
    Ok(schedule.schedule_id.to_string())
}

/// What `method` answers the holder of `bearer` for the schedule
/// `schedule_id` of `member`, with the member's name written as `MEMBER`.
async fn schedule_answer(
    app: &Router,
    bearer: &str,
    method: &str,
    member: &str,
    schedule_id: &str,
) -> String {
    let mut params = json!({ "identity": member });
    if method != "meta/schedule/list" {
        params["schedule_id"] = json!(schedule_id);
    }
    let answer = rpc_answer(app, bearer, method, params).await;
    json!({ "result": answer["result"], "error": answer["error"] })
        .to_string()
        .replace(member, "MEMBER")
}

/// Whether `method`, asked by the caller of `view`, reaches `member`: the
/// name the method goes on with is the member's own, not the name the
/// console puts in its place for a member the caller is answered about as
/// for one that does not exist.
fn schedule_method_reaches(view: &AccessView, method: &str, member: &str) -> bool {
    let mut params = json!({ "identity": member, "schedule_id": "any-schedule" });
    console_rpc_resolve_members(Some(view), method, &mut params)
        .expect("a member a schedule method names is never refused");
    params["identity"] == json!(member)
}

/// A member of a nested mob that was private to a person stays that
/// person's under its mob and name after it ended and the gateway started
/// again. The console knows it then from the durable record of who each
/// member is private to, with nothing in memory naming it: the person it
/// was private to reads, pauses and deletes the console schedule defined for
/// it, and another person is answered by each of those methods as for a
/// member that does not exist.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_ended_private_member_of_a_nested_mob_stays_its_owners_after_a_restart() -> TestResult {
    use super::console_private_audience_tests::{
        member_session, own_mob_create, restarted, runtime_with_a_private_side_chat,
        seated_private_owner,
    };
    use super::console_private_side_chat_tests::{
        ALICE, alice_claims, bearer, bob_claims, enforced_console, everyone_drives_every_agent,
    };

    const MOB: &str = "nested-restart";
    const TEAM: &str = "nested-restart-team";
    let state = tempfile::tempdir()?;
    let (runtime, app, alice_scope) = runtime_with_a_private_side_chat(state.path(), MOB).await?;
    // The side chat's own mob, and a member seated in it: private to the
    // side chat's person, by the label its spawn gave it.
    let team = own_mob_create(&runtime, "alpha-side", TEAM).await?;
    assert_eq!(
        seated_private_owner(
            &team,
            "sub-helper",
            member_session(&runtime, "alpha-side").await?
        )
        .await?,
        Some(alice_scope.clone()),
        "the member is seated private to the side chat's person"
    );
    // The member ends, then the gateway starts again.
    let identity = AgentIdentity::from("sub-helper");
    team.retire(identity.clone()).await?;
    for _ in 0..100 {
        if team.get_member(&identity).await?.is_none() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        team.get_member(&identity).await?.is_none(),
        "the member left its roster"
    );
    drop(team);
    drop(app);
    let runtime = restarted(runtime, state.path(), MOB).await?;
    let member = format!("{TEAM}/sub-helper");
    let no_member = "nested-restart-nowhere/nobody";
    assert!(
        !runtime
            .console_identity_labels()
            .await
            .holds_identity(&member),
        "after the restart nothing in memory names the member"
    );

    // The console schedule a person defined for the member while it ran.
    let service = meerkat::ScheduleService::new(Arc::new(meerkat::MemoryScheduleStore::default()));
    runtime.console_schedules().attach(
        service.clone(),
        crate::schedule_wiring::ScheduleFiringHostBinding::external_driver(),
    );
    let schedule_id = schedule_for(&service, &member).await?;

    let mut wrong = Vec::new();
    // What every route primes: the member is known under its mob and name,
    // with its owner.
    let access = AccessController::audience_only();
    prime_access_cache_from_runtime(&runtime, &access).await;
    let owner = access.view_for_caller(Some(ALICE), Some(&alice_scope));
    let other = person(&access, "bob");
    if !owner.knows_agent(&member) {
        wrong.push("the access cache does not know the ended member".to_string());
    }
    if !other.hides_agent(&member) {
        wrong.push("another person is not kept from the ended member".to_string());
    }
    if owner.hides_agent(&member) {
        wrong.push("the member's own person is kept from it".to_string());
    }

    // The console's schedule methods, asked by each person.
    let aggregator = crate::console_aggregator::MetaConsoleAggregator::in_memory();
    aggregator.register_runtime_handles_with_policy(
        "default",
        "",
        runtime.clone(),
        None,
        ConsoleEventStore::new(),
        Arc::new(AllowAllConsoleVisibilityPolicy),
    );
    let mut console = enforced_console(&runtime, Some(everyone_drives_every_agent()));
    console.console_aggregator = Some(aggregator);
    let app = console_json_router_with_state(console);
    let alice = bearer(alice_claims());
    let bob = bearer(bob_claims());
    for method in ENDED_MEMBER_SCHEDULE_METHODS {
        let hidden = schedule_answer(&app, &bob, method, &member, &schedule_id).await;
        let absent = schedule_answer(&app, &bob, method, no_member, &schedule_id).await;
        if hidden != absent {
            wrong.push(format!(
                "{method} answers another person {hidden}, not as for no member ({absent})"
            ));
        }
    }
    let listed = schedule_answer(&app, &alice, "meta/schedule/list", &member, &schedule_id).await;
    if !listed.contains("scheduled-for-MEMBER") {
        wrong.push(format!(
            "the member's own person does not read its schedule: {listed}"
        ));
    }
    let own = json!({ "identity": member, "schedule_id": schedule_id });
    let paused = rpc_answer(&app, &alice, "meta/schedule/pause", own.clone()).await;
    if paused["result"]["phase"] != json!("paused") {
        wrong.push(format!(
            "the member's own person does not pause its schedule: {paused}"
        ));
    }
    let deleted = rpc_answer(&app, &alice, "meta/schedule/delete", own).await;
    if deleted["result"]["deleted"] != json!(true) {
        wrong.push(format!(
            "the member's own person does not delete its schedule: {deleted}"
        ));
    }
    assert!(
        wrong.is_empty(),
        "an ended private member of a nested mob after a restart: {wrong:#?}"
    );
    Ok(())
}

/// The rules of an access configuration decide a member of a nested mob
/// the same way after it ended and the gateway started again as while it
/// was seated, as far as the console still knows the member, and closed
/// where it does not. After the restart the console knows the member's
/// name and who it was private to, not its role or its labels: its mob is
/// the entry its record is kept under, not a label the record holds. A
/// caller a rule denies by the member's role, and one a rule denies by one
/// of its labels, stay denied with another rule allowing them every agent:
/// they do not see the member, and its console schedule is not listed to,
/// paused or deleted by the one of them the console signs in. A grant by
/// the label of the member's mob, which the console itself writes on a
/// seated member, shows nothing of the ended member, as a grant on another
/// mob and a grant by role show nothing of it. (A deny by that label still
/// refuses it: `a_deny_on_the_label_of_the_mob_a_member_sat_in_still_refuses_it_after_its_end`.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rules_on_role_and_label_decide_an_ended_member_of_a_nested_mob_closed_after_a_restart()
-> TestResult {
    use super::console_private_audience_tests::{
        member_session, own_mob_create, restarted, runtime_with_a_private_side_chat,
    };
    use super::console_private_side_chat_tests::{
        ALICE, ROOT, alice_claims, bearer, bob_claims, enforced_console,
    };
    use crate::access::{AccessEffect, AccessRule};

    const MOB: &str = "nested-rules-restart";
    const TEAM: &str = "nested-rules-restart-team";
    const BOB: &str = "bob@example.test";
    const CAROL: &str = "carol@example.test";
    const DAVE: &str = "dave@example.test";
    const ERIN: &str = "erin@example.test";
    const FRANK: &str = "frank@example.test";

    // Alice, Bob and Carol may do everything to every agent; Bob is denied
    // the workers, Carol the vault tier. Dave is granted the team's mob by
    // the label the console writes, Erin another mob, Frank the workers.
    let rule = |id: &str, subject: &str| AccessRule {
        id: id.to_string(),
        subjects: vec![subject.to_string()],
        actions: vec!["agent.*".to_string()],
        ..AccessRule::default()
    };
    let label = |key: &str, value: &str| BTreeMap::from([(key.to_string(), value.to_string())]);
    let config = AccessControlConfig {
        enabled: true,
        admins: vec![ROOT.to_string()],
        rules: vec![
            AccessRule {
                subjects: vec![ALICE.to_string(), BOB.to_string(), CAROL.to_string()],
                agents: vec!["*".to_string()],
                ..rule("three-people-drive-every-agent", ALICE)
            },
            AccessRule {
                effect: AccessEffect::Deny,
                roles: vec!["worker".to_string()],
                ..rule("bob-is-denied-the-workers", BOB)
            },
            AccessRule {
                effect: AccessEffect::Deny,
                match_labels: label("tier", "vault"),
                ..rule("carol-is-denied-the-vault", CAROL)
            },
            AccessRule {
                match_labels: label("source_mob_id", TEAM),
                ..rule("dave-is-granted-the-team", DAVE)
            },
            AccessRule {
                match_labels: label("source_mob_id", "another-team"),
                ..rule("erin-is-granted-another-team", ERIN)
            },
            AccessRule {
                roles: vec!["worker".to_string()],
                ..rule("frank-is-granted-the-workers", FRANK)
            },
        ],
        ..AccessControlConfig::default()
    };
    // Who sees the member, by subject, on a controller primed as every
    // route primes one.
    let member = format!("{TEAM}/vault-helper");
    let sees = |access: &AccessController, subject: &str| {
        access
            .view_for_caller(Some(subject), Some(&format!("person:{subject}")))
            .can_view_agent(&member)
    };

    let state = tempfile::tempdir()?;
    let (runtime, app, _alice_scope) = runtime_with_a_private_side_chat(state.path(), MOB).await?;
    // A shared member's own mob, and a worker of the vault tier seated in
    // it: private to no one.
    let team = own_mob_create(&runtime, "alpha", TEAM).await?;
    let mut spec = SpawnMemberSpec::new("worker", AgentIdentity::from("vault-helper"))
        .with_runtime_mode(MobRuntimeMode::TurnDriven);
    spec.labels = Some(label("tier", "vault"));
    Box::pin(
        team.spawn_spec_with_generated_owner_context(
            spec,
            member_session(&runtime, "alpha").await?,
        ),
    )
    .await?;

    // While the member is seated the rules decide it by its row.
    let access = AccessController::new(config.clone())?;
    prime_access_cache_from_runtime(&runtime, &access).await;
    assert_eq!(
        [ALICE, BOB, CAROL, DAVE, ERIN, FRANK].map(|subject| sees(&access, subject)),
        [true, false, false, true, false, true],
        "while it is seated: alice, bob, carol, dave, erin, frank"
    );

    // The member ends, then the gateway starts again.
    let identity = AgentIdentity::from("vault-helper");
    team.retire(identity.clone()).await?;
    for _ in 0..100 {
        if team.get_member(&identity).await?.is_none() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        team.get_member(&identity).await?.is_none(),
        "the member left its roster"
    );
    drop(team);
    drop(app);
    let runtime = restarted(runtime, state.path(), MOB).await?;
    assert!(
        !runtime
            .console_identity_labels()
            .await
            .holds_identity(&member),
        "after the restart nothing in memory names the member"
    );
    let service = meerkat::ScheduleService::new(Arc::new(meerkat::MemoryScheduleStore::default()));
    runtime.console_schedules().attach(
        service.clone(),
        crate::schedule_wiring::ScheduleFiringHostBinding::external_driver(),
    );
    let schedule_id = schedule_for(&service, &member).await?;

    let mut wrong = Vec::new();
    let access = AccessController::new(config.clone())?;
    prime_access_cache_from_runtime(&runtime, &access).await;
    if !access
        .view_for_caller(Some(ALICE), None)
        .knows_agent(&member)
    {
        wrong.push("the access cache does not know the ended member".to_string());
    }
    // As while it was seated, but for the grants by the mob's label and by
    // role: neither the label nor the role is known of the member any more,
    // and an allow on what is not known grants nothing.
    let seen = [ALICE, BOB, CAROL, DAVE, ERIN, FRANK].map(|subject| sees(&access, subject));
    if seen != [true, false, false, false, false, false] {
        wrong.push(format!(
            "after the restart alice, bob, carol, dave, erin and frank see the member as {seen:?}"
        ));
    }

    // The console's schedule methods, asked by the person a rule denies
    // the member's role, and by a person no rule denies.
    let aggregator = crate::console_aggregator::MetaConsoleAggregator::in_memory();
    aggregator.register_runtime_handles_with_policy(
        "default",
        "",
        runtime.clone(),
        None,
        ConsoleEventStore::new(),
        Arc::new(AllowAllConsoleVisibilityPolicy),
    );
    let mut console = enforced_console(&runtime, Some(AccessController::new(config)?));
    console.console_aggregator = Some(aggregator);
    let app = console_json_router_with_state(console);
    let alice = bearer(alice_claims());
    let bob = bearer(bob_claims());
    let own = json!({ "identity": member, "schedule_id": schedule_id });
    let listed = schedule_answer(&app, &bob, "meta/schedule/list", &member, &schedule_id).await;
    if listed.contains("scheduled-for-MEMBER") {
        wrong.push(format!(
            "the person denied the member's role lists its schedule: {listed}"
        ));
    }
    let paused = rpc_answer(&app, &bob, "meta/schedule/pause", own.clone()).await;
    if paused["result"]["phase"] == json!("paused") {
        wrong.push(format!(
            "the person denied the member's role pauses its schedule: {paused}"
        ));
    }
    let deleted = rpc_answer(&app, &bob, "meta/schedule/delete", own).await;
    if deleted["result"]["deleted"] == json!(true) {
        wrong.push(format!(
            "the person denied the member's role deletes its schedule: {deleted}"
        ));
    }
    let listed = schedule_answer(&app, &alice, "meta/schedule/list", &member, &schedule_id).await;
    if !listed.contains("scheduled-for-MEMBER") {
        wrong.push(format!(
            "a person no rule denies does not read the member's schedule: {listed}"
        ));
    }
    assert!(
        wrong.is_empty(),
        "rules on role and label for an ended member of a nested mob after a restart: {wrong:#?}"
    );
    Ok(())
}

/// A member of a mob private to a person stays that person's after its mob
/// was destroyed, whatever its own row said: a member seated with no private
/// owner of its own in a mob a private member created is hidden from another
/// person and not from the mob's owner while it is seated, and stays so once
/// the mob is destroyed and only the spawn registry holds the member. Each
/// console schedule method that answers for an ended member reaches it for
/// the mob's owner, and for another person goes on as for a member that does
/// not exist.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unlabelled_member_of_a_destroyed_private_mob_stays_its_owners() -> TestResult {
    use super::console_side_chat_tests::{persistent_gateway_runtime_in, seeded_member};

    const TEAM: &str = "nested-destroyed-private-team";
    let state = tempfile::tempdir()?;
    let runtime =
        persistent_gateway_runtime_in(state.path(), "nested-destroyed-private", "", "").await?;
    let primary = runtime.handle();
    // The creator: a member of the runtime's own mob, private to alice.
    seeded_member(
        &runtime,
        "worker",
        "keeper",
        &[(crate::access::PRIVATE_TO_LABEL, "person:alice")],
    )
    .await?;
    assert!(
        runtime.private_owners().seeded(&primary).await.landed(),
        "the record's first read landed before the mob reaches the gateway"
    );
    let tools = runtime.agent_mob_mcp_state().ok_or("agent mob tools")?;
    let sessions = runtime
        .session_service()
        .ok_or("the runtime's session service")?
        .clone();
    let definition: MobDefinition =
        serde_json::from_value(json!({ "id": TEAM, "profiles": profiles() }))?;
    let team = meerkat_mob::MobBuilder::new(definition, MobStorage::in_memory())
        .with_session_service(sessions)
        .with_default_llm_client(Arc::new(TestClient::default()))
        .with_provenance(meerkat_mob::MobProvenance::OperatorMember {
            parent_mob_id: primary.mob_id().clone(),
            parent_incarnation: primary.incarnation(),
            creator_identity: AgentIdentity::from("keeper"),
            creator_generation: meerkat_mob::Generation::INITIAL,
            owner_session: session_of(&primary, "keeper").await?,
        })
        .create()
        .await?;
    tools
        .try_mob_insert_handle(meerkat_mob::MobId::from(TEAM), team.clone())
        .await?;
    // A member whose own spawn gave it no private owner, and the registry's
    // record of that spawn.
    seat(&team, "worker", "aide", BTreeMap::new()).await?;
    runtime.private_owners().read_mob(&team).await?;
    let sink = crate::console_spawn::ConsoleSpawnSink::for_mob(
        ConsoleEventStore::new(),
        primary.mob_id().as_str(),
    );
    runtime.install_console_spawn_sink(sink.clone());
    sink.project_spawned_member(&crate::console_spawn::ConsoleSpawnSeed {
        mob_id: Some(TEAM.to_string()),
        member_id: "aide".to_string(),
        identity: "aide".to_string(),
        initial_message: None,
        labels: BTreeMap::new(),
        spawned_by: None,
        spawned_by_session: None,
        session_id: None,
        via_tool: "mob_spawn_member".to_string(),
    })
    .await;
    let aide = format!("{TEAM}/aide");
    let access = AccessController::audience_only();

    prime_access_cache_from_runtime(&runtime, &access).await;
    assert!(
        person(&access, "bob").hides_agent(&aide),
        "while it is seated another person is kept from it"
    );
    assert!(!person(&access, "alice").hides_agent(&aide));

    // The mob is destroyed with the member seated in it.
    drop(team);
    Box::pin(tools.mob_destroy(&meerkat_mob::MobId::from(TEAM)))
        .await
        .map_err(|error| format!("the mob is destroyed: {error:?}"))?;
    assert!(
        runtime
            .console_identity_labels()
            .await
            .holds_identity(&aide),
        "the spawn registry still holds the member under its mob and name"
    );
    prime_access_cache_from_runtime(&runtime, &access).await;
    let mut wrong = Vec::new();
    let other = person(&access, "bob");
    let owner = person(&access, "alice");
    if !(other.knows_agent(&aide) && other.hides_agent(&aide)) {
        wrong.push("another person is not kept from the member of the destroyed mob".to_string());
    }
    if owner.hides_agent(&aide) {
        wrong.push("the mob's owner is kept from the member".to_string());
    }
    for method in ENDED_MEMBER_SCHEDULE_METHODS {
        if schedule_method_reaches(&other, method, &aide) {
            wrong.push(format!("{method} reaches the member for another person"));
        }
        if !schedule_method_reaches(&owner, method, &aide) {
            wrong.push(format!(
                "{method} does not reach the member for the mob's owner"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "an unlabelled member of a destroyed private mob: {wrong:#?}"
    );
    Ok(())
}

/// The enforced configuration of the rows on ended members: Alice may do
/// everything to every agent, then `more`.
fn alice_drives_every_agent_and(more: Vec<crate::access::AccessRule>) -> AccessControlConfig {
    let mut rules = vec![crate::access::AccessRule {
        agents: vec!["*".to_string()],
        ..agent_rule("alice-drives-every-agent", "alice@example.test")
    }];
    rules.extend(more);
    AccessControlConfig {
        enabled: true,
        admins: vec!["root@example.test".to_string()],
        rules,
        ..AccessControlConfig::default()
    }
}

/// An allow rule over every agent action for `subject`, with no selector.
fn agent_rule(id: &str, subject: &str) -> crate::access::AccessRule {
    crate::access::AccessRule {
        id: id.to_string(),
        subjects: vec![subject.to_string()],
        actions: vec!["agent.*".to_string()],
        ..crate::access::AccessRule::default()
    }
}

/// `agent_rule` as a deny.
fn agent_deny(id: &str, subject: &str) -> crate::access::AccessRule {
    crate::access::AccessRule {
        effect: crate::access::AccessEffect::Deny,
        ..agent_rule(id, subject)
    }
}

/// Retire `member` of `mob` and wait until it left the roster.
async fn retired(mob: &MobHandle, member: &str) -> TestResult {
    let identity = AgentIdentity::from(member);
    mob.retire(identity.clone()).await?;
    for _ in 0..100 {
        if mob.get_member(&identity).await?.is_none() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    if mob.get_member(&identity).await?.is_some() {
        return Err(format!("{member} did not leave its roster").into());
    }
    Ok(())
}

/// What is wrong with how the console decides the ended `member` for Bob,
/// whom a rule of `config` refuses it, and for Alice, whom none does: on a
/// controller primed as every route primes one, Bob has neither `agent.view`
/// nor `agent.send` on it, the decision fence shows him no request it
/// raised, an administrator's preview of him answers as his own view does,
/// and the memory panel's rule for an identity's scope (which every
/// row and record of the panel passes before it leaves the server) shows
/// him nothing of its memory while it shows Alice; through the console Bob
/// is not answered the member's status and neither lists, pauses nor
/// deletes a schedule defined for it, and Alice lists it. Each message
/// starts with `case`.
async fn refused_through_the_console(
    runtime: &MobRuntime,
    service: &meerkat::ScheduleService,
    config: &AccessControlConfig,
    member: &str,
    case: &str,
) -> TestResult<Vec<String>> {
    use super::console_private_side_chat_tests::{
        alice_claims, bearer, bob_claims, enforced_console,
    };
    const BOB: &str = "bob@example.test";
    let mut wrong = Vec::new();
    let access = AccessController::new(config.clone())?;
    prime_access_cache_from_runtime(runtime, &access).await;
    let view = access.view_for_caller(Some(BOB), Some("person:bob"));
    if !view.knows_agent(member) {
        wrong.push(format!("{case}: the access cache does not know the member"));
    }
    for action in [ACTION_AGENT_VIEW, crate::access::ACTION_AGENT_SEND] {
        if view.allows_agent(action, member) {
            wrong.push(format!("{case}: the refused person holds {action}"));
        }
    }
    if access
        .decision_fence(Some(BOB), Some("person:bob"))
        .can_see_origin(member, None)
    {
        wrong.push(format!(
            "{case}: the decision fence shows the refused person a request of the member"
        ));
    }
    // The decision `meta/access/preview` answers an administrator with.
    let previewed = access
        .admin_preview(Some("root@example.test"), Some(BOB))
        .map(|preview| {
            [ACTION_AGENT_VIEW, crate::access::ACTION_AGENT_SEND]
                .map(|action| preview.decide_agent(action, member).is_allow())
        });
    if !matches!(previewed, Ok([false, false])) {
        wrong.push(format!(
            "{case}: an administrator's preview of the refused person does not answer \
             as the person's own view does"
        ));
    }
    if memory_panel_identity_visible(MemoryPanelCaller::own(Some(&view)), member) {
        wrong.push(format!(
            "{case}: the refused person is shown the member's memory scope"
        ));
    }
    // A person no rule refuses is shown the memory scope of a member a plain
    // name names. The console's name of a nested member holds a `/`: it is
    // the key of no member's memory scope, and such a key is shown to access
    // administrators alone.
    let unrefused = access.view_for_caller(Some("alice@example.test"), Some("person:alice"));
    let plain_name = crate::memory::records::IdentityScopeKey::of(member).is_named();
    let shown = memory_panel_identity_visible(MemoryPanelCaller::own(Some(&unrefused)), member);
    if shown != plain_name {
        wrong.push(format!(
            "{case}: a person no rule refuses is shown the member's memory scope: {shown} \
             (the member is named by a plain name: {plain_name})"
        ));
    }

    let schedule_id = schedule_for(service, member).await?;
    let aggregator = crate::console_aggregator::MetaConsoleAggregator::in_memory();
    aggregator.register_runtime_handles_with_policy(
        "default",
        "",
        runtime.clone(),
        None,
        ConsoleEventStore::new(),
        Arc::new(AllowAllConsoleVisibilityPolicy),
    );
    let mut console = enforced_console(runtime, Some(AccessController::new(config.clone())?));
    console.console_aggregator = Some(aggregator);
    let app = console_json_router_with_state(console);
    let alice = bearer(alice_claims());
    let bob = bearer(bob_claims());
    let own = json!({ "identity": member, "schedule_id": schedule_id });
    let status = rpc_answer(
        &app,
        &bob,
        "meta/member_status",
        json!({ "member_id": member }),
    )
    .await;
    if !status["result"]["status"].is_null() {
        wrong.push(format!(
            "{case}: the refused person is answered the member's status: {status}"
        ));
    }
    let listed = schedule_answer(&app, &bob, "meta/schedule/list", member, &schedule_id).await;
    if listed.contains("scheduled-for-MEMBER") {
        wrong.push(format!(
            "{case}: the refused person lists the schedule: {listed}"
        ));
    }
    let paused = rpc_answer(&app, &bob, "meta/schedule/pause", own.clone()).await;
    if paused["result"]["phase"] == json!("paused") {
        wrong.push(format!(
            "{case}: the refused person pauses the schedule: {paused}"
        ));
    }
    let deleted = rpc_answer(&app, &bob, "meta/schedule/delete", own).await;
    if deleted["result"]["deleted"] == json!(true) {
        wrong.push(format!(
            "{case}: the refused person deletes the schedule: {deleted}"
        ));
    }
    let listed = schedule_answer(&app, &alice, "meta/schedule/list", member, &schedule_id).await;
    if !listed.contains("scheduled-for-MEMBER") {
        wrong.push(format!(
            "{case}: a person no rule refuses does not read the schedule: {listed}"
        ));
    }
    Ok(wrong)
}

/// A spawn an agent's tool made, as the console's spawn registry records
/// it: `member` of `mob_id`, spawned by `spawned_by` with `labels`.
async fn registered_spawn(
    sink: &crate::console_spawn::ConsoleSpawnSink,
    mob_id: &str,
    member: &str,
    spawned_by: &str,
    labels: BTreeMap<String, String>,
) {
    sink.project_spawned_member(&crate::console_spawn::ConsoleSpawnSeed {
        mob_id: Some(mob_id.to_string()),
        member_id: member.to_string(),
        identity: member.to_string(),
        initial_message: None,
        labels,
        spawned_by: Some(spawned_by.to_string()),
        spawned_by_session: None,
        session_id: None,
        via_tool: "mob_spawn_member".to_string(),
    })
    .await;
}

/// A member an agent's tool spawned keeps an entry in the console's spawn
/// registry after it ended, until the gateway starts again. What the
/// registry holds is of the last spawn the tool made under the name: the
/// labels the call carried and who made it, never the member's role in
/// its mob, and not necessarily of the member that ended. So nothing of
/// it decides the ended member: the console knows the member's name, its
/// mob and who it was private to, the same before a restart and after
/// one. A person a rule denies the member's role, or the agent that
/// spawned it, stays refused: no view, no send, no request of it at the
/// decision fence, and its console schedule is not listed to, paused or
/// deleted by that person. A grant on the spawner, a grant by a label the
/// spawn carried and a grant by its role show nothing of the ended
/// member. A label named `role` that the spawn carried is a label and
/// nothing more: a grant on that value as a role shows nothing, seated or
/// ended. A member the ended one spawned is decided through it: what is
/// not known of the ended one is not known of its lineage.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nothing_a_spawn_recorded_decides_an_ended_member_the_spawn_registry_holds() -> TestResult {
    const TEAM: &str = "registry-held-team";
    const ALICE: &str = "alice@example.test";
    const BOB: &str = "bob@example.test";
    const DAVE: &str = "dave@example.test";
    const ERIN: &str = "erin@example.test";
    const FRANK: &str = "frank@example.test";
    const GRACE: &str = "grace@example.test";
    const HEIDI: &str = "heidi@example.test";
    let label = |key: &str, value: &str| BTreeMap::from([(key.to_string(), value.to_string())]);

    let root = tempfile::tempdir()?;
    let runtime = runtime("registry-held", root.path()).await?;
    let primary = runtime.handle();
    // The planner is no worker: nothing but the member's own role decides
    // a rule on the workers.
    seat(&primary, "delegate", "planner", BTreeMap::new()).await?;
    let team = mob(&runtime, TEAM, Some(session_of(&primary, "planner").await?)).await?;
    // A worker of the vault tier whose spawn also carried a label named
    // `role`, and a member that worker spawned in turn.
    let spawn_labels = BTreeMap::from([
        ("tier".to_string(), "vault".to_string()),
        ("role".to_string(), "auditor".to_string()),
    ]);
    seat(&team, "worker", "helper", spawn_labels.clone()).await?;
    seat(&team, "delegate", "apprentice", BTreeMap::new()).await?;
    let sink = crate::console_spawn::ConsoleSpawnSink::for_mob(
        ConsoleEventStore::new(),
        primary.mob_id().as_str(),
    );
    runtime.install_console_spawn_sink(sink.clone());
    let helper = format!("{TEAM}/helper");
    let apprentice = format!("{TEAM}/apprentice");
    registered_spawn(&sink, TEAM, "helper", "planner", spawn_labels).await;
    registered_spawn(&sink, TEAM, "apprentice", &helper, BTreeMap::new()).await;

    // Bob is allowed what the planner spawned and denied the workers. Dave
    // is allowed what the planner spawned, Erin the vault tier, Frank the
    // auditors, Grace the workers. Heidi may do everything to every agent
    // and is denied what the planner spawned.
    let config = alice_drives_every_agent_and(vec![
        crate::access::AccessRule {
            agents: vec!["planner".to_string()],
            ..agent_rule("bob-is-granted-the-planners-members", BOB)
        },
        crate::access::AccessRule {
            roles: vec!["worker".to_string()],
            ..agent_deny("bob-is-denied-the-workers", BOB)
        },
        crate::access::AccessRule {
            agents: vec!["planner".to_string()],
            ..agent_rule("dave-is-granted-the-planners-members", DAVE)
        },
        crate::access::AccessRule {
            match_labels: label("tier", "vault"),
            ..agent_rule("erin-is-granted-the-vault", ERIN)
        },
        crate::access::AccessRule {
            roles: vec!["auditor".to_string()],
            ..agent_rule("frank-is-granted-the-auditors", FRANK)
        },
        crate::access::AccessRule {
            roles: vec!["worker".to_string()],
            ..agent_rule("grace-is-granted-the-workers", GRACE)
        },
        crate::access::AccessRule {
            agents: vec!["*".to_string()],
            ..agent_rule("heidi-drives-every-agent", HEIDI)
        },
        crate::access::AccessRule {
            agents: vec!["planner".to_string()],
            ..agent_deny("heidi-is-denied-the-planners-members", HEIDI)
        },
    ]);
    let sees = |access: &AccessController, subject: &str, member: &str| {
        access
            .view_for_caller(Some(subject), Some(&format!("person:{subject}")))
            .can_view_agent(member)
    };
    let people = [ALICE, BOB, DAVE, ERIN, FRANK, GRACE, HEIDI];

    let mut wrong = Vec::new();
    // While the member is seated its roster row decides it.
    let access = AccessController::new(config.clone())?;
    prime_access_cache_from_runtime(&runtime, &access).await;
    let seated = people.map(|subject| sees(&access, subject, &helper));
    if seated != [true, false, true, true, false, true, false] {
        wrong.push(format!(
            "seated, alice, bob, dave, erin, frank, grace and heidi see the member as {seated:?}"
        ));
    }
    if sees(&access, BOB, &apprentice) || !sees(&access, DAVE, &apprentice) {
        wrong.push("seated: the member it spawned is not decided through it".to_string());
    }

    // The member ends; the gateway keeps running and the registry keeps it.
    retired(&team, "helper").await?;
    assert!(
        runtime
            .console_identity_labels()
            .await
            .holds_identity(&helper),
        "the spawn registry still holds the ended member"
    );
    let access = AccessController::new(config.clone())?;
    prime_access_cache_from_runtime(&runtime, &access).await;
    // Only its name, its mob and its owner are known: an allow on every
    // agent still shows it, every allow that reads more grants nothing,
    // and every deny that reads more applies.
    let ended = people.map(|subject| sees(&access, subject, &helper));
    if ended != [true, false, false, false, false, false, false] {
        wrong.push(format!(
            "ended, alice, bob, dave, erin, frank, grace and heidi see the member as {ended:?}"
        ));
    }
    if sees(&access, BOB, &apprentice) {
        wrong.push(
            "ended: the person denied the member's role sees the member it spawned".to_string(),
        );
    }
    if sees(&access, DAVE, &apprentice) {
        wrong.push(
            "ended: a grant above the ended member reaches the member it spawned".to_string(),
        );
    }
    let service = meerkat::ScheduleService::new(Arc::new(meerkat::MemoryScheduleStore::default()));
    runtime.console_schedules().attach(
        service.clone(),
        crate::schedule_wiring::ScheduleFiringHostBinding::external_driver(),
    );
    wrong.extend(
        refused_through_the_console(&runtime, &service, &config, &helper, "a deny by role").await?,
    );
    assert!(
        wrong.is_empty(),
        "an ended member the spawn registry holds: {wrong:#?}"
    );
    Ok(())
}

/// An ended member's name is known, so a grant on that name still decides:
/// it shows the ended member, and it reaches a living member the ended one
/// spawned, whose own row names its spawner. A grant on an agent above the
/// ended member reached that living member while its spawner lived and no
/// longer does: who spawned the ended member is not known.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_grant_on_an_ended_members_own_name_still_reaches_the_member_it_spawned() -> TestResult {
    const TEAM: &str = "named-spawner-team";
    const DAVE: &str = "dave@example.test";
    const IVAN: &str = "ivan@example.test";

    let root = tempfile::tempdir()?;
    let runtime = runtime("named-spawner", root.path()).await?;
    let primary = runtime.handle();
    seat(&primary, "delegate", "planner", BTreeMap::new()).await?;
    let team = mob(&runtime, TEAM, Some(session_of(&primary, "planner").await?)).await?;
    seat(&team, "worker", "helper", BTreeMap::new()).await?;
    seat(&team, "delegate", "apprentice", BTreeMap::new()).await?;
    let sink = crate::console_spawn::ConsoleSpawnSink::for_mob(
        ConsoleEventStore::new(),
        primary.mob_id().as_str(),
    );
    runtime.install_console_spawn_sink(sink.clone());
    let helper = format!("{TEAM}/helper");
    let apprentice = format!("{TEAM}/apprentice");
    registered_spawn(&sink, TEAM, "helper", "planner", BTreeMap::new()).await;
    registered_spawn(&sink, TEAM, "apprentice", &helper, BTreeMap::new()).await;

    // Dave is allowed what the planner spawned, Ivan what the helper
    // spawned (and the helper itself).
    let config = alice_drives_every_agent_and(vec![
        crate::access::AccessRule {
            agents: vec!["planner".to_string()],
            ..agent_rule("dave-is-granted-the-planners-members", DAVE)
        },
        crate::access::AccessRule {
            agents: vec![helper.clone()],
            ..agent_rule("ivan-is-granted-the-helper-and-its-members", IVAN)
        },
    ]);
    let sees = |access: &AccessController, subject: &str, member: &str| {
        access
            .view_for_caller(Some(subject), Some(&format!("person:{subject}")))
            .can_view_agent(member)
    };

    let mut wrong = Vec::new();
    let access = AccessController::new(config.clone())?;
    prime_access_cache_from_runtime(&runtime, &access).await;
    let seated = [
        sees(&access, DAVE, &helper),
        sees(&access, DAVE, &apprentice),
        sees(&access, IVAN, &helper),
        sees(&access, IVAN, &apprentice),
    ];
    if seated != [true, true, true, true] {
        wrong.push(format!(
            "seated, dave and ivan see the helper and the member it spawned as {seated:?}"
        ));
    }

    retired(&team, "helper").await?;
    let access = AccessController::new(config)?;
    prime_access_cache_from_runtime(&runtime, &access).await;
    let ended = [
        sees(&access, DAVE, &helper),
        sees(&access, DAVE, &apprentice),
        sees(&access, IVAN, &helper),
        sees(&access, IVAN, &apprentice),
    ];
    if ended != [false, false, true, true] {
        wrong.push(format!(
            "the helper ended, dave and ivan see it and the member it spawned as {ended:?}"
        ));
    }
    assert!(
        wrong.is_empty(),
        "a grant on an ended member's own name: {wrong:#?}"
    );
    Ok(())
}

/// A member of the console's own mob that ended is decided closed by a
/// deny on its role, as a member of a nested mob is: while the gateway that
/// seated it still runs (the spawn registry holds it, without its role),
/// and after the gateway started again (only the record of who each member
/// is private to knows it). A person denied the workers, whose other rule
/// allows every agent, has no view and no send on the ended worker, sees no
/// request of it at the decision fence, and neither lists, pauses nor
/// deletes its console schedule; a person no rule refuses lists it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_deny_by_role_decides_an_ended_member_of_the_consoles_own_mob_closed() -> TestResult {
    use super::console_private_audience_tests::restarted;
    use super::console_side_chat_tests::persistent_gateway_runtime_in;
    const MOB: &str = "own-mob-ended-rules";
    const BOB: &str = "bob@example.test";
    const MEMBER: &str = "own-helper";

    let state = tempfile::tempdir()?;
    let runtime = persistent_gateway_runtime_in(state.path(), MOB, "", "").await?;
    let primary = runtime.handle();
    // The planner is no worker: nothing but the member's own role decides
    // a rule on the workers.
    seat(&primary, "lead", "planner", BTreeMap::new()).await?;
    seat(&primary, "worker", MEMBER, BTreeMap::new()).await?;
    let sink = crate::console_spawn::ConsoleSpawnSink::for_mob(
        ConsoleEventStore::new(),
        primary.mob_id().as_str(),
    );
    runtime.install_console_spawn_sink(sink.clone());
    registered_spawn(&sink, MOB, MEMBER, "planner", BTreeMap::new()).await;
    let config = alice_drives_every_agent_and(vec![
        crate::access::AccessRule {
            agents: vec!["*".to_string()],
            ..agent_rule("bob-drives-every-agent", BOB)
        },
        crate::access::AccessRule {
            roles: vec!["worker".to_string()],
            ..agent_deny("bob-is-denied-the-workers", BOB)
        },
    ]);

    let access = AccessController::new(config.clone())?;
    prime_access_cache_from_runtime(&runtime, &access).await;
    assert!(
        !access
            .view_for_caller(Some(BOB), Some("person:bob"))
            .can_view_agent(MEMBER),
        "while it is seated the person denied the workers does not see the member"
    );

    // The member ends; the gateway keeps running.
    retired(&primary, MEMBER).await?;
    assert!(
        runtime
            .console_identity_labels()
            .await
            .holds_identity(MEMBER),
        "the spawn registry still holds the ended member"
    );
    let service = meerkat::ScheduleService::new(Arc::new(meerkat::MemoryScheduleStore::default()));
    runtime.console_schedules().attach(
        service.clone(),
        crate::schedule_wiring::ScheduleFiringHostBinding::external_driver(),
    );
    let mut wrong = refused_through_the_console(
        &runtime,
        &service,
        &config,
        MEMBER,
        "ended, the gateway still running",
    )
    .await?;

    // The gateway starts again: nothing in memory names the member.
    drop(primary);
    let runtime = restarted(runtime, state.path(), MOB).await?;
    assert!(
        !runtime
            .console_identity_labels()
            .await
            .holds_identity(MEMBER),
        "after the restart nothing in memory names the member"
    );
    let service = meerkat::ScheduleService::new(Arc::new(meerkat::MemoryScheduleStore::default()));
    runtime.console_schedules().attach(
        service.clone(),
        crate::schedule_wiring::ScheduleFiringHostBinding::external_driver(),
    );
    wrong.extend(
        refused_through_the_console(&runtime, &service, &config, MEMBER, "after a restart").await?,
    );
    assert!(
        wrong.is_empty(),
        "an ended member of the console's own mob under a deny by role: {wrong:#?}"
    );
    Ok(())
}

/// After the gateway started again the console knows an ended member of a
/// nested mob by its name, who it was private to and its mob. A deny on one
/// of its labels and a deny that names another agent (which may have
/// spawned it) both read something the console no longer knows, and both
/// refuse the member through the console: the person such a rule applies
/// to, whose other rule allows every agent, has no view and no send on it,
/// sees no request of it at the decision fence, and neither lists, pauses
/// nor deletes its console schedule, while a person no rule refuses lists
/// it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn denies_by_label_and_by_another_agent_refuse_an_ended_member_of_a_nested_mob_through_the_console()
-> TestResult {
    use super::console_private_audience_tests::{
        member_session, own_mob_create, restarted, runtime_with_a_private_side_chat,
    };
    const MOB: &str = "nested-denies-restart";
    const TEAM: &str = "nested-denies-restart-team";
    const BOB: &str = "bob@example.test";

    let state = tempfile::tempdir()?;
    let (runtime, app, _alice_scope) = runtime_with_a_private_side_chat(state.path(), MOB).await?;
    let team = own_mob_create(&runtime, "alpha", TEAM).await?;
    let mut spec = SpawnMemberSpec::new("worker", AgentIdentity::from("vault-helper"))
        .with_runtime_mode(MobRuntimeMode::TurnDriven);
    spec.labels = Some(BTreeMap::from([("tier".to_string(), "vault".to_string())]));
    Box::pin(
        team.spawn_spec_with_generated_owner_context(
            spec,
            member_session(&runtime, "alpha").await?,
        ),
    )
    .await?;
    retired(&team, "vault-helper").await?;
    drop(team);
    drop(app);
    let runtime = restarted(runtime, state.path(), MOB).await?;
    let member = format!("{TEAM}/vault-helper");
    assert!(
        !runtime
            .console_identity_labels()
            .await
            .holds_identity(&member),
        "after the restart nothing in memory names the member"
    );
    let service = meerkat::ScheduleService::new(Arc::new(meerkat::MemoryScheduleStore::default()));
    runtime.console_schedules().attach(
        service.clone(),
        crate::schedule_wiring::ScheduleFiringHostBinding::external_driver(),
    );

    let bob_drives_every_agent = crate::access::AccessRule {
        agents: vec!["*".to_string()],
        ..agent_rule("bob-drives-every-agent", BOB)
    };
    let mut wrong = Vec::new();
    for (case, deny) in [
        (
            "a deny by label",
            crate::access::AccessRule {
                match_labels: BTreeMap::from([("tier".to_string(), "vault".to_string())]),
                ..agent_deny("bob-is-denied-the-vault", BOB)
            },
        ),
        (
            "a deny that names another agent",
            crate::access::AccessRule {
                agents: vec!["some-other-agent".to_string()],
                ..agent_deny("bob-is-denied-another-agents-members", BOB)
            },
        ),
    ] {
        let config = alice_drives_every_agent_and(vec![bob_drives_every_agent.clone(), deny]);
        wrong
            .extend(refused_through_the_console(&runtime, &service, &config, &member, case).await?);
    }
    assert!(
        wrong.is_empty(),
        "an ended member of a nested mob after a restart: {wrong:#?}"
    );
    Ok(())
}

/// A member's role in the console's identity records is its role in its
/// mob, whatever labels the member carries: a label named `role` that its
/// spawn wrote is not shown in the role's place and decides no rule on a
/// role. The identity list shows such a member to a person granted its role
/// in the mob and to no person granted the label's value as a role, and a
/// person denied its role in the mob is not listed it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_label_named_role_is_not_a_members_role_in_the_identity_list() -> TestResult {
    const BOB: &str = "bob@example.test";
    const FRANK: &str = "frank@example.test";
    const GRACE: &str = "grace@example.test";
    let root = tempfile::tempdir()?;
    let runtime = runtime("role-label", root.path()).await?;
    seat(
        &runtime.handle(),
        "worker",
        "claimant",
        BTreeMap::from([("role".to_string(), "auditor".to_string())]),
    )
    .await?;
    let aggregator = crate::console_aggregator::MetaConsoleAggregator::in_memory();
    aggregator.register_runtime_handles_with_policy(
        "default",
        "",
        runtime.clone(),
        None,
        ConsoleEventStore::new(),
        Arc::new(AllowAllConsoleVisibilityPolicy),
    );
    let mut wrong = Vec::new();
    let records = aggregator.list_identities().await?;
    let role = records
        .iter()
        .find(|record| record.identity == "claimant")
        .and_then(|record| record.labels.get("role").cloned());
    if role.as_deref() != Some("worker") {
        wrong.push(format!(
            "the record's role is {role:?}, not the member's role in its mob"
        ));
    }

    let config = alice_drives_every_agent_and(vec![
        crate::access::AccessRule {
            agents: vec!["*".to_string()],
            ..agent_rule("bob-drives-every-agent", BOB)
        },
        crate::access::AccessRule {
            roles: vec!["worker".to_string()],
            ..agent_deny("bob-is-denied-the-workers", BOB)
        },
        crate::access::AccessRule {
            roles: vec!["auditor".to_string()],
            ..agent_rule("frank-is-granted-the-auditors", FRANK)
        },
        crate::access::AccessRule {
            roles: vec!["worker".to_string()],
            ..agent_rule("grace-is-granted-the-workers", GRACE)
        },
    ]);
    let access = AccessController::new(config)?;
    prime_access_cache_from_runtime(&runtime, &access).await;
    let mut listed = Vec::new();
    for subject in [BOB, FRANK, GRACE] {
        let view = access.view_for_caller(Some(subject), Some(&format!("person:{subject}")));
        let records = Box::pin(visible_identity_records(&aggregator, Some(&view))).await?;
        listed.push(records.iter().any(|record| record.identity == "claimant"));
    }
    if listed != [false, false, true] {
        wrong.push(format!(
            "bob, frank and grace are listed the member as {listed:?}"
        ));
    }
    assert!(wrong.is_empty(), "a label named role: {wrong:#?}");
    Ok(())
}

/// A member the host seats through the mob handle, and a member an agent
/// spawns with the mob's own member tool `spawn_member`, leave nothing in
/// the console's spawn registry: only `mob_spawn_member` writes it. Once
/// such a member ended, with the gateway that seated it still running, the
/// console knows it from its record of who each member is private to alone,
/// which holds its name and its owner and neither its role nor its labels.
/// A deny on its role decides it closed all the same: the person the rule
/// applies to, whose other rule allows every agent, has no view and no send
/// on it, is not shown its memory scope or its status, sees no request of
/// it at the decision fence, and neither lists, pauses nor deletes its
/// console schedule, while a person no rule refuses reads all of it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_deny_by_role_refuses_an_ended_member_no_spawn_record_names() -> TestResult {
    const BOB: &str = "bob@example.test";
    const MEMBER: &str = "handle-helper";
    let root = tempfile::tempdir()?;
    let runtime = runtime("no-spawn-record", root.path()).await?;
    let primary = runtime.handle();
    seat(&primary, "worker", MEMBER, BTreeMap::new()).await?;
    let config = alice_drives_every_agent_and(vec![
        crate::access::AccessRule {
            agents: vec!["*".to_string()],
            ..agent_rule("bob-drives-every-agent", BOB)
        },
        crate::access::AccessRule {
            roles: vec!["worker".to_string()],
            ..agent_deny("bob-is-denied-the-workers", BOB)
        },
    ]);
    let access = AccessController::new(config.clone())?;
    prime_access_cache_from_runtime(&runtime, &access).await;
    assert!(
        !access
            .view_for_caller(Some(BOB), Some("person:bob"))
            .can_view_agent(MEMBER),
        "while it is seated the person denied the workers does not see the member"
    );

    retired(&primary, MEMBER).await?;
    assert!(
        !runtime
            .console_identity_labels()
            .await
            .holds_identity(MEMBER),
        "the spawn registry never held the member"
    );
    let service = meerkat::ScheduleService::new(Arc::new(meerkat::MemoryScheduleStore::default()));
    runtime.console_schedules().attach(
        service.clone(),
        crate::schedule_wiring::ScheduleFiringHostBinding::external_driver(),
    );
    let wrong = refused_through_the_console(
        &runtime,
        &service,
        &config,
        MEMBER,
        "no spawn record, ended, the gateway still running",
    )
    .await?;
    assert!(
        wrong.is_empty(),
        "an ended member no spawn record names: {wrong:#?}"
    );
    Ok(())
}

/// The console's spawn registry holds the labels a spawn call carried. A
/// label the member's roster row carried besides (a host's spawn customizer
/// gave it) is not among them, and once the member ended nothing holds it:
/// it is not known, which is not the same as "the member did not carry it".
/// So a deny on such a label refuses the ended member while the gateway
/// that seated it still runs, as it did while the member was seated and as
/// it does after a restart, and an allow on such a label grants nothing. A
/// label the spawn call did carry is the call's value, not the row's: it
/// decides nothing for the ended member either, so a deny on it applies
/// and an allow on it grants nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_deny_by_a_label_the_spawn_did_not_carry_refuses_an_ended_member_the_spawn_registry_holds()
-> TestResult {
    const BOB: &str = "bob@example.test";
    const DAVE: &str = "dave@example.test";
    const ERIN: &str = "erin@example.test";
    const FRANK: &str = "frank@example.test";
    const MEMBER: &str = "labelled-helper";
    let label = |key: &str, value: &str| BTreeMap::from([(key.to_string(), value.to_string())]);
    let root = tempfile::tempdir()?;
    let runtime = runtime("label-not-in-the-spawn", root.path()).await?;
    let primary = runtime.handle();
    seat(&primary, "delegate", "planner", BTreeMap::new()).await?;
    // The roster row carries two labels; the spawn call carried one.
    seat(
        &primary,
        "worker",
        MEMBER,
        BTreeMap::from([
            ("tier".to_string(), "vault".to_string()),
            ("team".to_string(), "blue".to_string()),
        ]),
    )
    .await?;
    let sink = crate::console_spawn::ConsoleSpawnSink::for_mob(
        ConsoleEventStore::new(),
        primary.mob_id().as_str(),
    );
    runtime.install_console_spawn_sink(sink.clone());
    registered_spawn(
        &sink,
        primary.mob_id().as_str(),
        MEMBER,
        "planner",
        label("team", "blue"),
    )
    .await;

    // Bob may do everything to every agent and is denied the vault tier.
    // Dave is granted the blue team, Erin the vault tier. Frank may do
    // everything to every agent and is denied the blue team, the label
    // the spawn call carried.
    let config = alice_drives_every_agent_and(vec![
        crate::access::AccessRule {
            agents: vec!["*".to_string()],
            ..agent_rule("bob-drives-every-agent", BOB)
        },
        crate::access::AccessRule {
            match_labels: label("tier", "vault"),
            ..agent_deny("bob-is-denied-the-vault", BOB)
        },
        crate::access::AccessRule {
            match_labels: label("team", "blue"),
            ..agent_rule("dave-is-granted-the-blue-team", DAVE)
        },
        crate::access::AccessRule {
            match_labels: label("tier", "vault"),
            ..agent_rule("erin-is-granted-the-vault", ERIN)
        },
        crate::access::AccessRule {
            agents: vec!["*".to_string()],
            ..agent_rule("frank-drives-every-agent", FRANK)
        },
        crate::access::AccessRule {
            match_labels: label("team", "blue"),
            ..agent_deny("frank-is-denied-the-blue-team", FRANK)
        },
    ]);
    let sees = |access: &AccessController, subject: &str| {
        access
            .view_for_caller(Some(subject), Some(&format!("person:{subject}")))
            .can_view_agent(MEMBER)
    };
    let mut wrong = Vec::new();
    let access = AccessController::new(config.clone())?;
    prime_access_cache_from_runtime(&runtime, &access).await;
    let seated = [BOB, DAVE, ERIN, FRANK].map(|subject| sees(&access, subject));
    if seated != [false, true, true, false] {
        wrong.push(format!(
            "seated, bob, dave, erin and frank see the member as {seated:?}"
        ));
    }

    retired(&primary, MEMBER).await?;
    assert!(
        runtime
            .console_identity_labels()
            .await
            .holds_identity(MEMBER),
        "the spawn registry still holds the ended member"
    );
    let access = AccessController::new(config.clone())?;
    prime_access_cache_from_runtime(&runtime, &access).await;
    let ended = [BOB, DAVE, ERIN, FRANK].map(|subject| sees(&access, subject));
    if ended != [false, false, false, false] {
        wrong.push(format!(
            "ended, bob, dave, erin and frank see the member as {ended:?}"
        ));
    }
    let service = meerkat::ScheduleService::new(Arc::new(meerkat::MemoryScheduleStore::default()));
    runtime.console_schedules().attach(
        service.clone(),
        crate::schedule_wiring::ScheduleFiringHostBinding::external_driver(),
    );
    wrong.extend(
        refused_through_the_console(
            &runtime,
            &service,
            &config,
            MEMBER,
            "a deny by a label the spawn did not carry",
        )
        .await?,
    );
    assert!(
        wrong.is_empty(),
        "an ended member whose roster row carried a label its spawn did not: {wrong:#?}"
    );
    Ok(())
}

/// A console of `runtime` with sign-in, under `config`, with an aggregator
/// over the runtime: what serves the identity lists.
fn console_with_identity_lists(
    runtime: &MobRuntime,
    config: &AccessControlConfig,
) -> TestResult<Router> {
    use super::console_private_side_chat_tests::enforced_console;
    let aggregator = crate::console_aggregator::MetaConsoleAggregator::in_memory();
    aggregator.register_runtime_handles_with_policy(
        "default",
        "",
        runtime.clone(),
        None,
        ConsoleEventStore::new(),
        Arc::new(AllowAllConsoleVisibilityPolicy),
    );
    let mut console = enforced_console(runtime, Some(AccessController::new(config.clone())?));
    console.console_aggregator = Some(aggregator);
    Ok(console_json_router_with_state(console))
}

/// The identities the console's two identity list doors name to the holder
/// of `bearer`: `GET /console/identities`, then `meta/console/list_identities`.
async fn identity_list_doors(app: &Router, bearer: &str) -> [BTreeSet<String>; 2] {
    use tower::ServiceExt;
    let names = |rows: Option<&Value>| -> BTreeSet<String> {
        rows.and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|row| row["identity"].as_str().map(str::to_string))
            .collect()
    };
    let response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/console/identities")
                .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
                .body(axum::body::Body::empty())
                .expect("identities request"),
        )
        .await
        .expect("identities response");
    let body = axum::body::to_bytes(response.into_body(), 1 << 22)
        .await
        .expect("identities body");
    let route: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let rpc = rpc_answer(app, bearer, "meta/console/list_identities", json!({})).await;
    [
        names(route.pointer("/identities")),
        names(rpc.pointer("/result/identities")),
    ]
}

/// The identity lists never show a person a member that person's view of
/// it refuses, whatever a rule selects by. A member whose spawn carried a
/// label named `role` is refused, by a deny on that label, at the view and
/// in both identity lists alike. An allow on that label alone admits the
/// person at the view and does not list the member: a list row is shown
/// when the member's record and the view both allow it, and the record's
/// `role` is the member's role in its mob, not the label.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rule_on_a_label_named_role_decides_the_identity_lists_no_wider_than_the_view()
-> TestResult {
    use super::console_private_side_chat_tests::{alice_claims, bearer, bob_claims};
    const BOB: &str = "bob@example.test";
    const MEMBER: &str = "claimant";
    const PLAIN: &str = "plain";
    let role_label = || BTreeMap::from([("role".to_string(), "auditor".to_string())]);
    let root = tempfile::tempdir()?;
    let runtime = runtime("role-label-lists", root.path()).await?;
    seat(&runtime.handle(), "worker", MEMBER, role_label()).await?;
    seat(&runtime.handle(), "worker", PLAIN, BTreeMap::new()).await?;
    let alice = bearer(alice_claims());
    let bob = bearer(bob_claims());
    let views = |config: &AccessControlConfig| -> TestResult<AccessController> {
        Ok(AccessController::new(config.clone())?)
    };
    let mut wrong = Vec::new();

    // A deny on the label, beside an allow on every agent.
    let denied = alice_drives_every_agent_and(vec![
        crate::access::AccessRule {
            agents: vec!["*".to_string()],
            ..agent_rule("bob-drives-every-agent", BOB)
        },
        crate::access::AccessRule {
            match_labels: role_label(),
            ..agent_deny("bob-is-denied-the-label", BOB)
        },
    ]);
    let access = views(&denied)?;
    prime_access_cache_from_runtime(&runtime, &access).await;
    if access
        .view_for_caller(Some(BOB), Some("person:bob"))
        .can_view_agent(MEMBER)
    {
        wrong.push("a deny on the label: the view admits the person".to_string());
    }
    let app = console_with_identity_lists(&runtime, &denied)?;
    for (door, listed) in ["GET /console/identities", "meta/console/list_identities"]
        .into_iter()
        .zip(identity_list_doors(&app, &bob).await)
    {
        if listed.contains(MEMBER) {
            wrong.push(format!(
                "a deny on the label: {door} names the member to the person"
            ));
        }
        if !listed.contains(PLAIN) {
            wrong.push(format!(
                "a deny on the label: {door} does not name another member to the person"
            ));
        }
    }
    for (door, listed) in ["GET /console/identities", "meta/console/list_identities"]
        .into_iter()
        .zip(identity_list_doors(&app, &alice).await)
    {
        if !listed.contains(MEMBER) {
            wrong.push(format!(
                "a deny on the label: {door} does not name the member to a person no rule refuses"
            ));
        }
    }

    // An allow on the label alone.
    let allowed = alice_drives_every_agent_and(vec![crate::access::AccessRule {
        match_labels: role_label(),
        ..agent_rule("bob-is-granted-the-label", BOB)
    }]);
    let access = views(&allowed)?;
    prime_access_cache_from_runtime(&runtime, &access).await;
    if !access
        .view_for_caller(Some(BOB), Some("person:bob"))
        .can_view_agent(MEMBER)
    {
        wrong.push("an allow on the label alone: the view refuses the person".to_string());
    }
    let app = console_with_identity_lists(&runtime, &allowed)?;
    for (door, listed) in ["GET /console/identities", "meta/console/list_identities"]
        .into_iter()
        .zip(identity_list_doors(&app, &bob).await)
    {
        if listed.contains(MEMBER) {
            wrong.push(format!(
                "an allow on the label alone: {door} names the member to the person"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "a rule on a label named role and the identity lists: {wrong:#?}"
    );
    Ok(())
}

/// What outlives a member in the console's spawn registry is of the last
/// spawn made under the name, not of the member that ended: an ended
/// member is decided by nothing an earlier member of its name recorded.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_ended_member_is_not_decided_by_an_earlier_member_of_its_name() -> TestResult {
    const BOB: &str = "bob@example.test";
    const DAVE: &str = "dave@example.test";
    const ERIN: &str = "erin@example.test";
    const FRANK: &str = "frank@example.test";
    const MEMBER: &str = "reseated";
    let label = |key: &str, value: &str| BTreeMap::from([(key.to_string(), value.to_string())]);
    let root = tempfile::tempdir()?;
    let runtime = runtime("name-reuse", root.path()).await?;
    let primary = runtime.handle();
    seat(&primary, "worker", "planner", BTreeMap::new()).await?;
    let sink = crate::console_spawn::ConsoleSpawnSink::for_mob(
        ConsoleEventStore::new(),
        primary.mob_id().as_str(),
    );
    runtime.install_console_spawn_sink(sink.clone());
    // The first member: a worker of the open tier, spawned by the planner's
    // tool. It ends.
    seat(&primary, "worker", MEMBER, label("tier", "open")).await?;
    registered_spawn(
        &sink,
        primary.mob_id().as_str(),
        MEMBER,
        "planner",
        label("tier", "open"),
    )
    .await;
    retired(&primary, MEMBER).await?;
    // The second member of the name: seated by the host, another role,
    // the sealed tier. Nothing registers it. It ends too.
    seat(&primary, "delegate", MEMBER, label("tier", "sealed")).await?;

    // Bob may do everything to every agent and is denied the sealed tier;
    // Frank the same, denied the delegates. Dave is granted what the
    // planner spawned, Erin the open tier.
    let config = alice_drives_every_agent_and(vec![
        crate::access::AccessRule {
            agents: vec!["*".to_string()],
            ..agent_rule("bob-drives-every-agent", BOB)
        },
        crate::access::AccessRule {
            match_labels: label("tier", "sealed"),
            ..agent_deny("bob-is-denied-the-sealed-tier", BOB)
        },
        crate::access::AccessRule {
            agents: vec!["planner".to_string()],
            ..agent_rule("dave-is-granted-the-planners-members", DAVE)
        },
        crate::access::AccessRule {
            match_labels: label("tier", "open"),
            ..agent_rule("erin-is-granted-the-open-tier", ERIN)
        },
        crate::access::AccessRule {
            agents: vec!["*".to_string()],
            ..agent_rule("frank-drives-every-agent", FRANK)
        },
        crate::access::AccessRule {
            roles: vec!["delegate".to_string()],
            ..agent_deny("frank-is-denied-the-delegates", FRANK)
        },
    ]);
    let sees = |access: &AccessController, subject: &str| {
        access
            .view_for_caller(Some(subject), Some(&format!("person:{subject}")))
            .can_view_agent(MEMBER)
    };
    let mut wrong = Vec::new();
    let access = AccessController::new(config.clone())?;
    prime_access_cache_from_runtime(&runtime, &access).await;
    // While the second member is seated its own row decides the denies.
    let seated = [BOB, FRANK].map(|subject| sees(&access, subject));
    if seated != [false, false] {
        wrong.push(format!(
            "the second member seated, bob and frank see it as {seated:?}"
        ));
    }

    retired(&primary, MEMBER).await?;
    let access = AccessController::new(config.clone())?;
    prime_access_cache_from_runtime(&runtime, &access).await;
    let ended = [BOB, DAVE, ERIN, FRANK].map(|subject| sees(&access, subject));
    if ended != [false, false, false, false] {
        wrong.push(format!(
            "the second member ended, bob, dave, erin and frank see it as {ended:?}"
        ));
    }
    let service = meerkat::ScheduleService::new(Arc::new(meerkat::MemoryScheduleStore::default()));
    runtime.console_schedules().attach(
        service.clone(),
        crate::schedule_wiring::ScheduleFiringHostBinding::external_driver(),
    );
    wrong.extend(
        refused_through_the_console(
            &runtime,
            &service,
            &config,
            MEMBER,
            "a deny on the second member's label",
        )
        .await?,
    );
    assert!(
        wrong.is_empty(),
        "an ended member under a name an earlier member held: {wrong:#?}"
    );
    Ok(())
}

/// An agent the console's access cache does not know is not judged as a
/// bare name where a rule that applies to the caller reads what a name does
/// not carry (a role, a label, another agent): a method that reads or acts
/// on an agent refuses such a caller, as the memory panel does, and the
/// refusal says nothing of the agent. An agent private to another person
/// and a name no agent holds are answered alike, each under the name as
/// the caller spelled it. A caller no such rule applies to is answered by
/// the method. A spawn is not refused so: it names an agent that does not
/// exist yet.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_method_on_an_agent_the_console_does_not_know_is_refused_alike_for_a_hidden_agent_and_for_no_agent()
-> TestResult {
    use super::console_private_side_chat_tests::{
        alice_claims, bearer, bob_claims, enforced_console, fork_on_console,
    };
    use super::console_side_chat_tests::{persistent_gateway_runtime_in, seeded_member};
    const BOB: &str = "bob@example.test";
    const HIDDEN: &str = "alpha-side";
    const NOBODY: &str = "never-recorded";
    let config = alice_drives_every_agent_and(vec![
        crate::access::AccessRule {
            agents: vec!["*".to_string()],
            ..agent_rule("bob-drives-every-agent", BOB)
        },
        crate::access::AccessRule {
            roles: vec!["worker".to_string()],
            ..agent_deny("bob-is-denied-the-workers", BOB)
        },
    ]);
    let state = tempfile::tempdir()?;
    let runtime = persistent_gateway_runtime_in(state.path(), "unknown-agent", "", "").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let app = console_json_router_with_state(enforced_console(
        &runtime,
        Some(AccessController::new(config.clone())?),
    ));
    let alice = bearer(alice_claims());
    let bob = bearer(bob_claims());
    // A side chat private to Alice: hidden from Bob.
    let forked = fork_on_console(&app, Some(&alice), "alpha", HIDDEN).await;
    assert!(forked["error"].is_null(), "{forked:#}");

    let mut wrong = Vec::new();
    let hidden = rpc_answer(
        &app,
        &bob,
        "meta/member_status",
        json!({ "member_id": HIDDEN }),
    )
    .await;
    let absent = rpc_answer(
        &app,
        &bob,
        "meta/member_status",
        json!({ "member_id": NOBODY }),
    )
    .await;
    if absent["error"]["code"] != json!(ACCESS_DENIED_RPC_CODE)
        || absent["error"]["data"]["resource"] != json!(NOBODY)
    {
        wrong.push(format!(
            "the status of a name no agent holds, for a person a deny by role applies to: {absent}"
        ));
    }
    if hidden.to_string().replace(HIDDEN, "<member>")
        != absent.to_string().replace(NOBODY, "<member>")
    {
        wrong.push(format!(
            "an agent hidden from the person is not answered as a name no agent holds: hidden \
             {hidden}, absent {absent}"
        ));
    }
    let unrefused = rpc_answer(
        &app,
        &alice,
        "meta/member_status",
        json!({ "member_id": NOBODY }),
    )
    .await;
    if unrefused["error"]["code"] == json!(ACCESS_DENIED_RPC_CODE) {
        wrong.push(format!(
            "the same status, for a person no rule refuses: {unrefused}"
        ));
    }
    // A spawn names an agent that does not exist yet: the check does not
    // refuse it for what it cannot know of the name.
    let access = AccessController::new(config)?;
    let spawn = json!({
        "role": "worker",
        "agent_identity": "a-new-name",
        "session_id": SessionId::new().to_string(),
    });
    if let Some(error) = console_rpc_access_violation(
        Some(&access.view_for_caller(Some(BOB), Some("person:bob"))),
        "meta/attach_existing_session",
        &spawn,
    ) {
        wrong.push(format!(
            "a spawn under a name no agent holds yet: {}",
            error.message
        ));
    }
    assert!(
        wrong.is_empty(),
        "an agent the console does not know: {wrong:#?}"
    );
    Ok(())
}

/// A snapshot whose member rows are identity records is shown to a caller
/// the route resolved, whose audience the route primed from the runtimes.
/// With no caller resolved there is no view of an agent to ask, and the
/// experience shows no member: no sidebar row and no topology node. Roster
/// rows are shown as before.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn identity_record_rows_are_shown_to_no_caller_the_route_did_not_resolve() -> TestResult {
    let root = tempfile::tempdir()?;
    let runtime = runtime("record-rows-no-caller", root.path()).await?;
    seat(&runtime.handle(), "worker", "helper", BTreeMap::new()).await?;
    let listed = snapshot(&runtime, &AllowAllConsoleVisibilityPolicy).await;
    let of_rosters = agent_ids(&experience_of(
        &listed,
        crate::runtime::SnapshotRows::OfRosters,
        None,
    ));
    assert!(
        of_rosters.contains("helper"),
        "roster rows are shown: {of_rosters:?}"
    );
    let of_records = experience_of(
        &listed,
        crate::runtime::SnapshotRows::OfIdentityRecords,
        None,
    );
    let sidebar = agent_ids(&of_records);
    assert!(
        sidebar.is_empty(),
        "record rows with no caller resolved: {sidebar:?}"
    );
    let nodes = of_records.pointer("/topology/live_snapshot/nodes");
    assert!(
        nodes.and_then(Value::as_array).is_none_or(Vec::is_empty),
        "record rows with no caller resolved, the topology: {nodes:?}"
    );
    Ok(())
}

/// A console with no runtime of its own (it serves its runtimes through the
/// aggregator) decides its sidebar no wider than the view of each agent, and
/// its experience read leaves the access cache as the runtimes' rosters
/// wrote it. A member whose spawn carried a label named `role` is absent
/// from the sidebar of a person a deny on that label applies to, and after
/// another person's experience read that person's view of the member is
/// still refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_console_with_no_runtime_of_its_own_decides_its_sidebar_no_wider_than_the_view()
-> TestResult {
    use super::console_private_side_chat_tests::{
        alice_claims, bearer, bob_claims, enforced_console,
    };
    const BOB: &str = "bob@example.test";
    const MEMBER: &str = "claimant";
    const PLAIN: &str = "plain";
    let role_label = || BTreeMap::from([("role".to_string(), "auditor".to_string())]);
    let root = tempfile::tempdir()?;
    let runtime = runtime("role-label-aggregate", root.path()).await?;
    seat(&runtime.handle(), "worker", MEMBER, role_label()).await?;
    seat(&runtime.handle(), "worker", PLAIN, BTreeMap::new()).await?;
    let config = alice_drives_every_agent_and(vec![
        crate::access::AccessRule {
            agents: vec!["*".to_string()],
            ..agent_rule("bob-drives-every-agent", BOB)
        },
        crate::access::AccessRule {
            match_labels: role_label(),
            ..agent_deny("bob-is-denied-the-label", BOB)
        },
    ]);
    let aggregator = crate::console_aggregator::MetaConsoleAggregator::in_memory();
    aggregator.register_runtime_handles_with_policy(
        "default",
        "",
        runtime.clone(),
        None,
        ConsoleEventStore::new(),
        Arc::new(AllowAllConsoleVisibilityPolicy),
    );
    let access = AccessController::new(config)?;
    let mut console = enforced_console(&runtime, Some(access.clone()));
    console.runtime = None;
    console.console_aggregator = Some(aggregator);
    let app = console_json_router_with_state(console);
    let alice = bearer(alice_claims());
    let bob = bearer(bob_claims());

    let mut wrong = Vec::new();
    let shown = agent_ids(&experience_on(&app, &bob).await);
    if shown.contains(MEMBER) {
        wrong.push("the sidebar names the member to the person the deny applies to".to_string());
    }
    if !shown.contains(PLAIN) {
        wrong.push("the sidebar does not name another member to that person".to_string());
    }
    if !agent_ids(&experience_on(&app, &alice).await).contains(MEMBER) {
        wrong.push("the sidebar does not name the member to a person no rule refuses".to_string());
    }
    // After the experience reads, the cache still holds the member as its
    // roster row says: the person's view of it is refused.
    if access
        .view_for_caller(Some(BOB), Some("person:bob"))
        .can_view_agent(MEMBER)
    {
        wrong.push("after an experience read the view admits the person".to_string());
    }
    for (door, listed) in ["GET /console/identities", "meta/console/list_identities"]
        .into_iter()
        .zip(identity_list_doors(&app, &bob).await)
    {
        if listed.contains(MEMBER) {
            wrong.push(format!("{door} names the member to the person"));
        }
    }
    assert!(
        wrong.is_empty(),
        "a console with no runtime of its own: {wrong:#?}"
    );
    Ok(())
}

/// The console writes a nested member's `delegate_host_identity` label
/// into the access cache and not into the member's identity record. A deny
/// on that label refuses the member at the view and keeps it out of both
/// identity lists all the same: a list row is shown only if the view of its
/// agent allows it too.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_deny_on_a_label_only_the_cache_holds_keeps_a_member_out_of_the_identity_lists()
-> TestResult {
    use super::console_private_side_chat_tests::{alice_claims, bearer, bob_claims};
    const BOB: &str = "bob@example.test";
    const TEAM: &str = "label-in-the-cache-team";
    let root = tempfile::tempdir()?;
    let runtime = runtime("label-in-the-cache", root.path()).await?;
    let primary = runtime.handle();
    seat(&primary, "worker", "planner", BTreeMap::new()).await?;
    let team = mob(&runtime, TEAM, Some(session_of(&primary, "planner").await?)).await?;
    seat(&team, "worker", "helper", BTreeMap::new()).await?;
    let member = format!("{TEAM}/helper");
    let config = alice_drives_every_agent_and(vec![
        crate::access::AccessRule {
            agents: vec!["*".to_string()],
            ..agent_rule("bob-drives-every-agent", BOB)
        },
        crate::access::AccessRule {
            match_labels: BTreeMap::from([(
                "delegate_host_identity".to_string(),
                "planner".to_string(),
            )]),
            ..agent_deny("bob-is-denied-the-planners-mobs", BOB)
        },
    ]);
    let mut wrong = Vec::new();
    let access = AccessController::new(config.clone())?;
    prime_access_cache_from_runtime(&runtime, &access).await;
    if access
        .view_for_caller(Some(BOB), Some("person:bob"))
        .can_view_agent(&member)
    {
        wrong.push("the view admits the person the deny applies to".to_string());
    }
    let app = console_with_identity_lists(&runtime, &config)?;
    let alice = bearer(alice_claims());
    let bob = bearer(bob_claims());
    for (door, listed) in ["GET /console/identities", "meta/console/list_identities"]
        .into_iter()
        .zip(identity_list_doors(&app, &bob).await)
    {
        if listed.contains(&member) {
            wrong.push(format!("{door} names the member to the person"));
        }
    }
    for (door, listed) in ["GET /console/identities", "meta/console/list_identities"]
        .into_iter()
        .zip(identity_list_doors(&app, &alice).await)
    {
        if !listed.contains(&member) {
            wrong.push(format!(
                "{door} does not name the member to a person no rule refuses"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "a deny on a label only the cache holds: {wrong:#?}"
    );
    Ok(())
}

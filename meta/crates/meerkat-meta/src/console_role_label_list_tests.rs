//! A member's role in the console's lists is its role in its mob, and a list
//! shows a person no agent that person's view of the agent refuses. A label
//! named `role` that a member carries is a label: it is never shown or
//! decided in the role's place. The identity list on both of its doors, and
//! the sidebar of a console with no runtime of its own, decide each row no
//! wider than the view of its agent.

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

use super::console_private_side_chat_tests::{alice_claims, bearer, bob_claims, enforced_console};
use super::*;
use crate::access::{AccessControlConfig, AccessController, AccessEffect, AccessRule};
use crate::console_aggregator::AllowAllConsoleVisibilityPolicy;
use crate::mob_handle_runtime::MobRuntime;
use crate::{MobBootstrapOptions, MobBootstrapSpec};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const BOB: &str = "bob@example.test";

/// The profiles every mob of these rows offers.
fn profiles() -> Value {
    json!({
        "worker": { "model": "gpt-5.5", "runtime_mode": "turn_driven", "tools": { "comms": true } },
        "delegate": { "model": "gpt-5.5", "runtime_mode": "turn_driven", "tools": { "comms": true } }
    })
}

/// A gateway runtime whose own mob is `mob_id`, with agent mob tools.
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
        .expect("build the fixture spec")
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

/// A mob made through the runtime's agent mob state, owned by the session
/// `owner`, as the agent mob tools make one.
async fn mob_owned_by(
    runtime: &MobRuntime,
    mob_id: &str,
    owner: SessionId,
) -> TestResult<MobHandle> {
    let state = runtime
        .agent_mob_mcp_state()
        .ok_or("the runtime carries agent mob tools")?;
    let definition: MobDefinition =
        serde_json::from_value(json!({ "id": mob_id, "profiles": profiles() }))?;
    let created = Box::pin(
        state.mob_create_definition_with_owner_bridge_session(definition, owner, true, false),
    )
    .await
    .map_err(|error| error.to_string())?;
    Ok(Box::pin(state.handle_for(&created))
        .await
        .map_err(|error| error.to_string())?)
}

/// The label these rows' member carries: one named `role`, whose value is
/// no role of its mob.
fn role_label() -> BTreeMap<String, String> {
    BTreeMap::from([("role".to_string(), "auditor".to_string())])
}

/// An allow rule over every agent action for `subject`, with no selector.
fn agent_rule(id: &str, subject: &str) -> AccessRule {
    AccessRule {
        id: id.to_string(),
        subjects: vec![subject.to_string()],
        actions: vec!["agent.*".to_string()],
        ..AccessRule::default()
    }
}

/// `agent_rule` as a deny.
fn agent_deny(id: &str, subject: &str) -> AccessRule {
    AccessRule {
        effect: AccessEffect::Deny,
        ..agent_rule(id, subject)
    }
}

/// An enforced configuration: Alice may do everything to every agent, then
/// `more`.
fn alice_drives_every_agent_and(more: Vec<AccessRule>) -> AccessControlConfig {
    let mut rules = vec![AccessRule {
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

/// Bob may do everything to every agent.
fn bob_drives_every_agent() -> AccessRule {
    AccessRule {
        agents: vec!["*".to_string()],
        ..agent_rule("bob-drives-every-agent", BOB)
    }
}

/// An aggregator over `runtime` alone, under no namespace.
fn aggregator_over(runtime: &MobRuntime) -> crate::console_aggregator::MetaConsoleAggregator {
    let aggregator = crate::console_aggregator::MetaConsoleAggregator::in_memory();
    aggregator.register_runtime_handles_with_policy(
        "default",
        "",
        runtime.clone(),
        None,
        ConsoleEventStore::new(),
        Arc::new(AllowAllConsoleVisibilityPolicy),
    );
    aggregator
}

/// A console of `runtime` with sign-in, under `config`, with an aggregator
/// over the runtime: what serves the identity lists.
fn console_with_identity_lists(
    runtime: &MobRuntime,
    config: &AccessControlConfig,
) -> TestResult<Router> {
    let mut console = enforced_console(runtime, Some(AccessController::new(config.clone())?));
    console.console_aggregator = Some(aggregator_over(runtime));
    Ok(console_json_router_with_state(console))
}

/// A console with sign-in, under `access`, with no runtime of its own: it
/// serves `runtime` through an aggregator alone.
fn console_with_no_runtime_of_its_own(runtime: &MobRuntime, access: &AccessController) -> Router {
    let mut console = enforced_console(runtime, Some(access.clone()));
    console.runtime = None;
    console.console_aggregator = Some(aggregator_over(runtime));
    console_json_router_with_state(console)
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

/// The names of the console's two identity list doors, in the order
/// [`identity_list_doors`] answers them.
const IDENTITY_LIST_DOORS: [&str; 2] = ["GET /console/identities", "meta/console/list_identities"];

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

/// A member's role in the console's identity records is its role in its
/// mob, whatever labels the member carries: a label named `role` that its
/// spawn wrote is not shown in the role's place and decides no rule on a
/// role. The identity list shows such a member to a person granted its role
/// in the mob and to no person granted the label's value as a role, and a
/// person denied its role in the mob is not listed it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_label_named_role_is_not_a_members_role_in_the_identity_list() -> TestResult {
    const FRANK: &str = "frank@example.test";
    const GRACE: &str = "grace@example.test";
    let root = tempfile::tempdir()?;
    let runtime = runtime("role-label", root.path()).await?;
    seat(&runtime.handle(), "worker", "claimant", role_label()).await?;
    let aggregator = aggregator_over(&runtime);
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
        bob_drives_every_agent(),
        AccessRule {
            roles: vec!["worker".to_string()],
            ..agent_deny("bob-is-denied-the-workers", BOB)
        },
        AccessRule {
            roles: vec!["auditor".to_string()],
            ..agent_rule("frank-is-granted-the-auditors", FRANK)
        },
        AccessRule {
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
    const MEMBER: &str = "claimant";
    const PLAIN: &str = "plain";
    let root = tempfile::tempdir()?;
    let runtime = runtime("role-label-lists", root.path()).await?;
    seat(&runtime.handle(), "worker", MEMBER, role_label()).await?;
    seat(&runtime.handle(), "worker", PLAIN, BTreeMap::new()).await?;
    let alice = bearer(alice_claims());
    let bob = bearer(bob_claims());
    let mut wrong = Vec::new();

    // A deny on the label, beside an allow on every agent.
    let denied = alice_drives_every_agent_and(vec![
        bob_drives_every_agent(),
        AccessRule {
            match_labels: role_label(),
            ..agent_deny("bob-is-denied-the-label", BOB)
        },
    ]);
    let access = AccessController::new(denied.clone())?;
    prime_access_cache_from_runtime(&runtime, &access).await;
    if access
        .view_for_caller(Some(BOB), Some("person:bob"))
        .can_view_agent(MEMBER)
    {
        wrong.push("a deny on the label: the view admits the person".to_string());
    }
    let app = console_with_identity_lists(&runtime, &denied)?;
    for (door, listed) in IDENTITY_LIST_DOORS
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
    for (door, listed) in IDENTITY_LIST_DOORS
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
    let allowed = alice_drives_every_agent_and(vec![AccessRule {
        match_labels: role_label(),
        ..agent_rule("bob-is-granted-the-label", BOB)
    }]);
    let access = AccessController::new(allowed.clone())?;
    prime_access_cache_from_runtime(&runtime, &access).await;
    if !access
        .view_for_caller(Some(BOB), Some("person:bob"))
        .can_view_agent(MEMBER)
    {
        wrong.push("an allow on the label alone: the view refuses the person".to_string());
    }
    let app = console_with_identity_lists(&runtime, &allowed)?;
    for (door, listed) in IDENTITY_LIST_DOORS
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

/// A console with no runtime of its own (it serves its runtimes through the
/// aggregator) decides its sidebar no wider than the view of each agent, and
/// its experience read leaves the access cache as the runtimes' rosters
/// wrote it. A member whose spawn carried a label named `role` is absent
/// from the sidebar of a person a deny on that label applies to, and after
/// another person's experience read that person's view of the member is
/// still refused. A person denied the member's role in its mob is shown no
/// row of it either, and stays refused after an experience read.
///
/// The case of the deny on the role fails on the release before this
/// change, where such a console showed and decided a member by a label
/// named `role` in the role's place. The cases of the deny on the label
/// pass there: they hold the sidebar and the cache in place once the
/// record's `role` is the member's role in its mob, and fail with that
/// change alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_console_with_no_runtime_of_its_own_decides_its_sidebar_no_wider_than_the_view()
-> TestResult {
    const MEMBER: &str = "claimant";
    const PLAIN: &str = "plain";
    let root = tempfile::tempdir()?;
    let runtime = runtime("role-label-aggregate", root.path()).await?;
    seat(&runtime.handle(), "worker", MEMBER, role_label()).await?;
    seat(&runtime.handle(), "delegate", PLAIN, BTreeMap::new()).await?;
    let alice = bearer(alice_claims());
    let bob = bearer(bob_claims());
    let mut wrong = Vec::new();

    // A deny on the label, beside an allow on every agent.
    let access = AccessController::new(alice_drives_every_agent_and(vec![
        bob_drives_every_agent(),
        AccessRule {
            match_labels: role_label(),
            ..agent_deny("bob-is-denied-the-label", BOB)
        },
    ]))?;
    let app = console_with_no_runtime_of_its_own(&runtime, &access);
    let shown = agent_ids(&experience_on(&app, &bob).await);
    if shown.contains(MEMBER) {
        wrong.push(
            "a deny on the label: the sidebar names the member to the person the deny applies to"
                .to_string(),
        );
    }
    if !shown.contains(PLAIN) {
        wrong.push(
            "a deny on the label: the sidebar does not name another member to that person"
                .to_string(),
        );
    }
    if !agent_ids(&experience_on(&app, &alice).await).contains(MEMBER) {
        wrong.push(
            "a deny on the label: the sidebar does not name the member to a person no rule refuses"
                .to_string(),
        );
    }
    // After the experience reads, the cache still holds the member as its
    // roster row says: the person's view of it is refused.
    if access
        .view_for_caller(Some(BOB), Some("person:bob"))
        .can_view_agent(MEMBER)
    {
        wrong.push(
            "a deny on the label: after an experience read the view admits the person".to_string(),
        );
    }
    for (door, listed) in IDENTITY_LIST_DOORS
        .into_iter()
        .zip(identity_list_doors(&app, &bob).await)
    {
        if listed.contains(MEMBER) {
            wrong.push(format!(
                "a deny on the label: {door} names the member to the person"
            ));
        }
    }

    // A deny on the member's role in its mob, beside an allow on every agent.
    let access = AccessController::new(alice_drives_every_agent_and(vec![
        bob_drives_every_agent(),
        AccessRule {
            roles: vec!["worker".to_string()],
            ..agent_deny("bob-is-denied-the-workers", BOB)
        },
    ]))?;
    let app = console_with_no_runtime_of_its_own(&runtime, &access);
    let shown = agent_ids(&experience_on(&app, &bob).await);
    if shown.contains(MEMBER) {
        wrong.push(
            "a deny on the role: the sidebar names the member to the person the deny applies to"
                .to_string(),
        );
    }
    if !shown.contains(PLAIN) {
        wrong.push(
            "a deny on the role: the sidebar does not name a member of another role to that person"
                .to_string(),
        );
    }
    if access
        .view_for_caller(Some(BOB), Some("person:bob"))
        .can_view_agent(MEMBER)
    {
        wrong.push(
            "a deny on the role: after an experience read the view admits the person".to_string(),
        );
    }
    assert!(
        wrong.is_empty(),
        "a console with no runtime of its own: {wrong:#?}"
    );
    Ok(())
}

/// The console writes the `delegate_host_identity` label of a member of a
/// mob one of its members created into the access cache and not into the
/// member's identity record. A deny on that label refuses the member at the
/// view and keeps it out of both identity lists all the same: a list row is
/// shown only if the view of its agent allows it too.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_deny_on_a_label_only_the_cache_holds_keeps_a_member_out_of_the_identity_lists()
-> TestResult {
    const TEAM: &str = "label-in-the-cache-team";
    const MEMBER: &str = "helper";
    let root = tempfile::tempdir()?;
    let runtime = runtime("label-in-the-cache", root.path()).await?;
    let primary = runtime.handle();
    seat(&primary, "worker", "planner", BTreeMap::new()).await?;
    let team = mob_owned_by(&runtime, TEAM, session_of(&primary, "planner").await?).await?;
    seat(&team, "worker", MEMBER, BTreeMap::new()).await?;
    let config = alice_drives_every_agent_and(vec![
        bob_drives_every_agent(),
        AccessRule {
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
        .can_view_agent(MEMBER)
    {
        wrong.push("the view admits the person the deny applies to".to_string());
    }
    let app = console_with_identity_lists(&runtime, &config)?;
    let alice = bearer(alice_claims());
    let bob = bearer(bob_claims());
    for (door, listed) in IDENTITY_LIST_DOORS
        .into_iter()
        .zip(identity_list_doors(&app, &bob).await)
    {
        if listed.contains(MEMBER) {
            wrong.push(format!("{door} names the member to the person"));
        }
    }
    for (door, listed) in IDENTITY_LIST_DOORS
        .into_iter()
        .zip(identity_list_doors(&app, &alice).await)
    {
        if !listed.contains(MEMBER) {
            wrong.push(format!(
                "{door} does not name the member to a person no rule refuses: {listed:?}"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "a deny on a label only the cache holds: {wrong:#?}"
    );
    Ok(())
}

/// The live snapshot a console of `runtime` serves, built and narrowed as
/// its experience route builds and narrows it.
async fn snapshot(runtime: &MobRuntime) -> ConsoleLiveSnapshot {
    let policy = AllowAllConsoleVisibilityPolicy;
    let (mut snapshot, _) = Box::pin(build_live_snapshot(
        runtime,
        &[],
        None,
        &policy,
        &ConsoleSnapshotReadModel::default(),
        None,
    ))
    .await;
    apply_console_visibility_policy(&mut snapshot, &policy);
    snapshot
}

/// The experience over `snapshot`, whose member rows are `rows`, on a
/// console without sign-in under `access`, for a caller with `audience`
/// (`None`: a request whose caller no route resolved).
fn experience_of(
    snapshot: &ConsoleLiveSnapshot,
    rows: SnapshotRows,
    access: Option<&AccessController>,
    audience: Option<&crate::access::AccessView>,
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
        &crate::auth::session_fence::ConsoleSessionFences::detached(),
        &ConsoleRestJsonRequest {
            method: "GET".to_string(),
            path: "/console/experience".to_string(),
            auth: None,
        },
        None,
        Some(snapshot),
        rows,
        access,
        audience,
        false,
        false,
        &BTreeSet::new(),
    );
    assert_eq!(response.status, 200, "experience: {:?}", response.body);
    response.body
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
    let listed = snapshot(&runtime).await;
    let of_rosters = agent_ids(&experience_of(&listed, SnapshotRows::OfRosters, None, None));
    assert!(
        of_rosters.contains("helper"),
        "roster rows are shown: {of_rosters:?}"
    );
    let of_records = experience_of(&listed, SnapshotRows::OfIdentityRecords, None, None);
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

/// What is wrong after an experience read over a snapshot that still lists
/// a member the access cache holds as ended (its rows, of kind `rows`, were
/// walked before the member's end), for a person granted the member's role
/// in its mob and nothing else. The ended member's entry says nothing of
/// its role, so that person is refused it before the read; the read must
/// leave the entry as it is, name the member to nobody the view refuses,
/// and the person must be refused after it.
async fn wrong_after_a_read_that_lists_an_ended_member(
    mob_id: &str,
    rows: SnapshotRows,
) -> TestResult<Vec<String>> {
    const GRACE: &str = "grace@example.test";
    const MEMBER: &str = "claimant";
    let root = tempfile::tempdir()?;
    let runtime = runtime(mob_id, root.path()).await?;
    seat(&runtime.handle(), "worker", MEMBER, BTreeMap::new()).await?;
    let listed = snapshot(&runtime).await;
    let access = AccessController::new(alice_drives_every_agent_and(vec![AccessRule {
        roles: vec!["worker".to_string()],
        ..agent_rule("grace-is-granted-the-workers", GRACE)
    }]))?;
    // The member has ended since, and a priming recorded it so.
    record_live_and_registered_attributes(
        &access,
        Vec::new(),
        &crate::console_spawn::SpawnRegistry::default(),
        &BTreeSet::new(),
        &BTreeSet::new(),
        &BTreeMap::from([(MEMBER.to_string(), None)]),
        "",
    );
    let grace = access.view_for_caller(Some(GRACE), Some("person:grace"));
    let mut wrong = Vec::new();
    if !grace.knows_agent(MEMBER) {
        wrong.push("the cache does not know the ended member".to_string());
    }
    if grace.can_view_agent(MEMBER) {
        wrong.push("before the read: the view admits the person".to_string());
    }
    let shown = agent_ids(&experience_of(&listed, rows, Some(&access), Some(&grace)));
    if shown.contains(MEMBER) {
        wrong.push("the read names the ended member to the person".to_string());
    }
    if grace.can_view_agent(MEMBER) {
        wrong.push("after the read: the view admits the person".to_string());
    }
    Ok(wrong)
}

/// An experience read over identity record rows writes none of them into
/// the access cache: the entry of a member that has ended stays the ended
/// one, whatever a record of the member still listed says, and the member
/// is shown to no person the view of it refuses.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_experience_read_over_identity_record_rows_leaves_an_ended_members_entry_as_it_is()
-> TestResult {
    let wrong = wrong_after_a_read_that_lists_an_ended_member(
        "record-rows-ended-entry",
        SnapshotRows::OfIdentityRecords,
    )
    .await?;
    assert!(
        wrong.is_empty(),
        "identity record rows and an ended member: {wrong:#?}"
    );
    Ok(())
}

/// An experience read over roster rows that were walked before a member's
/// end does not make the ended member live again in the access cache: its
/// entry stays the ended one until a read of the roster says otherwise, and
/// the member is shown to no person the view of it refuses.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_experience_read_over_roster_rows_leaves_an_ended_members_entry_as_it_is() -> TestResult
{
    let wrong = wrong_after_a_read_that_lists_an_ended_member(
        "roster-rows-ended-entry",
        SnapshotRows::OfRosters,
    )
    .await?;
    assert!(
        wrong.is_empty(),
        "roster rows and an ended member: {wrong:#?}"
    );
    Ok(())
}

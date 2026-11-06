//! Private side chats on a console with sign-in: a side chat a signed-in
//! person forks is private to them. Its audience is its source's audience
//! narrowed to its creator, plus the access admins, with or without an access
//! configuration. Every other caller gets the answer an unknown member gets.
//! A console without sign-in keeps side chats shared, and so does every side
//! chat forked before the private-owner label existed.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use meerkat_mob::SpawnMemberSpec;
use serde_json::{Value, json};

use super::console_human_admission_tests::{BOB, authenticate, state};
use super::console_side_chat_tests::{persistent_console_spec, seeded_member};
use super::console_token_lapse_tests::{console_rpc_as, now_seconds, token};
use super::*;
use crate::console_aggregator::AllowAllConsoleVisibilityPolicy;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

const ALICE: &str = "alice@example.test";
const ROOT: &str = "root@example.test";
/// The label a member's private owner is named in.
const PRIVATE_TO: &str = "console.private_to";

fn alice_claims() -> Value {
    json!({ "sub": "oidc-sub-alice", "email": ALICE, "provider": "google_oauth" })
}

fn bob_claims() -> Value {
    json!({ "sub": "oidc-sub-bob", "email": BOB, "provider": "google_oauth" })
}

fn root_claims() -> Value {
    json!({ "sub": "oidc-sub-root", "email": ROOT, "provider": "google_oauth" })
}

fn bearer(claims: Value) -> String {
    token(claims, Some(now_seconds() + 3600))
}

/// A console runtime whose sessions persist, as a durable fork needs.
async fn runtime(
    mob_id: &str,
) -> Result<(tempfile::TempDir, MobRuntime), Box<dyn std::error::Error + Send + Sync>> {
    let (temp_dir, spec) = persistent_console_spec(mob_id, "", "")?;
    let runtime = Box::pin(MobRuntime::bootstrap(spec)).await?;
    Ok((temp_dir, runtime))
}

/// An access configuration under which every signed-in caller may do
/// everything to every agent, with `root` its one admin.
fn everyone_drives_every_agent() -> AccessController {
    AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![ROOT.to_string()],
        rules: vec![AccessRule {
            id: "everyone-drives-every-agent".to_string(),
            subjects: vec!["*".to_string()],
            actions: vec!["agent.*".to_string()],
            agents: vec!["*".to_string()],
            ..AccessRule::default()
        }],
        ..crate::access::AccessControlConfig::default()
    })
    .expect("access controller")
}

/// The console of `runtime` with sign-in on (the Enforced posture) for Alice,
/// Bob and root, under `access`.
fn enforced_console(runtime: &MobRuntime, access: Option<AccessController>) -> ConsoleJsonState {
    let mut state = state();
    state
        .decisions
        .auth
        .email_allowlist
        .extend([ALICE.to_string(), ROOT.to_string()]);
    state.runtime = Some(runtime.clone());
    state.access = access;
    state
}

/// The console of `runtime` without sign-in (the Open posture).
fn open_console(runtime: &MobRuntime) -> ConsoleJsonState {
    let mut state = state();
    state.decisions.console.require_app_auth = false;
    state.runtime = Some(runtime.clone());
    state
}

/// `meta/fork_member` of `source` into `side_chat` on the console route as
/// the holder of `bearer` (none on an open console), retried while the
/// runtime still settles the source's last turn.
async fn fork_on_console(
    app: &Router,
    bearer: Option<&str>,
    source: &str,
    side_chat: &str,
) -> Value {
    let params = json!({ "source_member_id": source, "agent_identity": side_chat });
    let mut answer = Value::Null;
    for _ in 0..50 {
        answer = console_rpc_as(app, bearer, "meta/fork_member", params.clone()).await;
        let busy = answer["error"]["data"]["kind"] == json!("fork_source_unavailable")
            && answer["error"]["data"]["cause"] == json!("running");
        if !busy {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    answer
}

/// The experience's sidebar agent rows as the holder of `bearer` reads them.
async fn sidebar(app: &Router, bearer: Option<&str>) -> Vec<Value> {
    use tower::ServiceExt;
    let mut request = axum::http::Request::builder()
        .method("GET")
        .uri("/console/experience");
    if let Some(bearer) = bearer {
        request = request.header(header::AUTHORIZATION, format!("Bearer {bearer}"));
    }
    let response = app
        .clone()
        .oneshot(
            request
                .body(axum::body::Body::empty())
                .expect("experience request"),
        )
        .await
        .expect("experience response");
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 1 << 22)
        .await
        .expect("experience body");
    let experience: Value = serde_json::from_slice(&body).expect("experience json");
    experience["agent_sidebar"]["live_snapshot"]["agents"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn row<'a>(rows: &'a [Value], member: &str) -> Option<&'a Value> {
    rows.iter().find(|row| row["member_id"] == json!(member))
}

/// `POST /console/send` to `identity` as the holder of `bearer`.
async fn send_on_console(app: &Router, bearer: &str, identity: &str) -> (StatusCode, Value) {
    use tower::ServiceExt;
    let response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/console/send")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
                .body(axum::body::Body::from(
                    json!({
                        "identity": identity,
                        "content": "hello",
                        "origin": "console",
                        "idempotency_key": format!("send-{identity}"),
                    })
                    .to_string(),
                ))
                .expect("send request"),
        )
        .await
        .expect("send response");
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("send body");
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

/// The answer an unknown member gets on the console's JSON-RPC route.
fn assert_unknown_identity(answer: &Value, identity: &str) {
    assert_eq!(answer["error"]["code"], json!(-32001), "{answer:#}");
    assert_eq!(
        answer["error"]["message"],
        json!(format!("unknown identity: {identity}")),
        "{answer:#}"
    );
    assert!(answer["error"]["data"].is_null(), "{answer:#}");
}

/// What every member other than the side chat's creator meets: the side
/// chat is on no surface it reads, and every request naming it is answered
/// as for an unknown member. Nothing it asks for happens.
async fn assert_hidden_from(app: &Router, runtime: &MobRuntime, bearer: &str, side_chat: &str) {
    assert!(
        row(&sidebar(app, Some(bearer)).await, side_chat).is_none(),
        "the side chat is on no sidebar but its creator's and the admins'"
    );
    let read = console_rpc_as(
        app,
        Some(bearer),
        "meta/get_member",
        json!({ "member_id": side_chat }),
    )
    .await;
    assert_unknown_identity(&read, side_chat);
    let (status, sent) = send_on_console(app, bearer, side_chat).await;
    assert_eq!(
        (status, &sent["error"]),
        (StatusCode::NOT_FOUND, &json!("unknown_identity")),
        "a send to the side chat is answered as a send to an unknown member: {sent:#}"
    );
    let retired = console_rpc_as(
        app,
        Some(bearer),
        "meta/retire",
        json!({ "member_id": side_chat }),
    )
    .await;
    assert_unknown_identity(&retired, side_chat);
    let forked = console_rpc_as(
        app,
        Some(bearer),
        "meta/fork_member",
        json!({ "source_member_id": side_chat, "agent_identity": format!("{side_chat}-again") }),
    )
    .await;
    assert_unknown_identity(&forked, side_chat);
    assert!(
        runtime
            .handle()
            .get_member(&AgentIdentity::from(side_chat))
            .await
            .expect("roster read")
            .is_some(),
        "the side chat is still seated"
    );
}

/// On a console with sign-in, a side chat a person forks is private to them:
/// another person who may view, send to and close the source sees it on no
/// surface and is answered as for an unknown member when they read, send
/// to, close or fork it. Its creator sees it and is told it is theirs alone.
#[tokio::test]
async fn side_chat_private_to_creator_on_enforced() -> TestResult {
    let (_temp_dir, runtime) = runtime("private-side-chat-enforced").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let app = console_json_router_with_state(enforced_console(
        &runtime,
        Some(everyone_drives_every_agent()),
    ));
    let alice = bearer(alice_claims());
    let bob = bearer(bob_claims());

    let forked = fork_on_console(&app, Some(&alice), "alpha", "alpha-side").await;
    assert!(
        forked["error"].is_null(),
        "Alice forks the side chat: {forked:#}"
    );

    assert!(
        row(&sidebar(&app, Some(&bob)).await, "alpha").is_some(),
        "Bob sees the source"
    );
    assert_hidden_from(&app, &runtime, &bob, "alpha-side").await;

    let alice_rows = sidebar(&app, Some(&alice)).await;
    let side_chat = row(&alice_rows, "alpha-side").expect("Alice sees her side chat");
    assert_eq!(side_chat["side_chat_of"], json!("alpha"));
    assert_eq!(side_chat["side_chat_audience"], json!("private_to_you"));
    Ok(())
}

/// An access admin sees every private side chat, named as private to its
/// creator, while another person still does not.
#[tokio::test]
async fn access_admin_sees_side_chats() -> TestResult {
    let (_temp_dir, runtime) = runtime("private-side-chat-admin").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let app = console_json_router_with_state(enforced_console(
        &runtime,
        Some(everyone_drives_every_agent()),
    ));
    let alice = bearer(alice_claims());
    let forked = fork_on_console(&app, Some(&alice), "alpha", "alpha-side").await;
    assert!(
        forked["error"].is_null(),
        "Alice forks the side chat: {forked:#}"
    );

    assert!(
        row(
            &sidebar(&app, Some(&bearer(bob_claims()))).await,
            "alpha-side"
        )
        .is_none(),
        "Bob does not see Alice's side chat"
    );
    let root = bearer(root_claims());
    let root_rows = sidebar(&app, Some(&root)).await;
    let side_chat = row(&root_rows, "alpha-side").expect("the access admin sees it");
    assert_eq!(side_chat["side_chat_audience"], json!("private_to_creator"));
    let read = console_rpc_as(
        &app,
        Some(&root),
        "meta/get_member",
        json!({ "member_id": "alpha-side" }),
    )
    .await;
    assert!(
        read["error"].is_null(),
        "the access admin reads it: {read:#}"
    );
    Ok(())
}

/// Without an access configuration a console with sign-in shows everything
/// to everyone who signs in, except another person's side chat: only its
/// creator sees it.
#[tokio::test]
async fn side_chat_private_without_access_config() -> TestResult {
    let (_temp_dir, runtime) = runtime("private-side-chat-no-access").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let app = console_json_router_with_state(enforced_console(&runtime, None));
    let alice = bearer(alice_claims());
    let bob = bearer(bob_claims());
    let forked = fork_on_console(&app, Some(&alice), "alpha", "alpha-side").await;
    assert!(
        forked["error"].is_null(),
        "Alice forks the side chat: {forked:#}"
    );

    assert!(row(&sidebar(&app, Some(&bob)).await, "alpha").is_some());
    assert_hidden_from(&app, &runtime, &bob, "alpha-side").await;
    let alice_rows = sidebar(&app, Some(&alice)).await;
    assert_eq!(
        row(&alice_rows, "alpha-side").expect("Alice sees her side chat")["side_chat_audience"],
        json!("private_to_you")
    );
    Ok(())
}

/// A console without sign-in has one anonymous caller, so a side chat forked
/// there stays shared: every caller sees it, and it says so.
#[tokio::test]
async fn open_console_side_chat_stays_shared() -> TestResult {
    let (_temp_dir, runtime) = runtime("private-side-chat-open").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let app = console_json_router_with_state(open_console(&runtime));
    let forked = fork_on_console(&app, None, "alpha", "alpha-side").await;
    assert!(
        forked["error"].is_null(),
        "the side chat is forked: {forked:#}"
    );

    let rows = sidebar(&app, None).await;
    let side_chat = row(&rows, "alpha-side").expect("the open console lists the side chat");
    assert_eq!(side_chat["side_chat_audience"], json!("shared"));
    let read = console_rpc_as(
        &app,
        None,
        "meta/get_member",
        json!({ "member_id": "alpha-side" }),
    )
    .await;
    assert!(read["error"].is_null(), "{read:#}");
    Ok(())
}

/// Fork `source` into `side_chat` as the console's side chat fork seats one,
/// with `labels` as the side chat's own roster labels, retried while the
/// runtime still settles the source's last turn.
async fn fork_with_labels(
    runtime: &MobRuntime,
    source: &str,
    side_chat: &str,
    labels: &[(&str, &str)],
) -> TestResult {
    let labels = labels
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect::<BTreeMap<_, _>>();
    for _ in 0..50 {
        let spec = SpawnMemberSpec::new("worker", AgentIdentity::from(side_chat))
            .with_labels(labels.clone())
            .with_definition_wiring(meerkat_mob::DefinitionWiring::Exempt);
        match crate::fork::fork_member_on_runtime(
            runtime,
            None,
            source,
            spec,
            None,
            crate::fork::ForkChildRetention::UntilRetired,
            None,
        )
        .await
        {
            Ok(_) => return Ok(()),
            Err(crate::fork::ForkMemberError::Mob(
                meerkat_mob::MobError::ForkSourceUnavailable {
                    cause: meerkat_mob::error::ForkSourceUnavailableCause::Running,
                    ..
                },
            )) => tokio::time::sleep(std::time::Duration::from_millis(100)).await,
            Err(error) => return Err(error.to_string().into()),
        }
    }
    Err("the source stayed busy".into())
}

/// A side chat forked before the private-owner label existed carries none:
/// it keeps its source's audience (a reader of the source sees it, a caller
/// who may not view the source does not) and is marked shared.
#[tokio::test]
async fn legacy_side_chat_keeps_source_audience() -> TestResult {
    let (_temp_dir, runtime) = runtime("private-side-chat-legacy").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    fork_with_labels(
        &runtime,
        "alpha",
        "alpha-side",
        &[(crate::side_chat::SIDE_CHAT_OF_LABEL, "alpha")],
    )
    .await?;
    let access = AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![ROOT.to_string()],
        rules: vec![AccessRule {
            id: "bob-reads-the-source".to_string(),
            subjects: vec![BOB.to_string()],
            actions: vec![ACTION_AGENT_VIEW.to_string()],
            agents: vec!["alpha".to_string()],
            ..AccessRule::default()
        }],
        ..crate::access::AccessControlConfig::default()
    })?;
    let app = console_json_router_with_state(enforced_console(&runtime, Some(access)));

    let bob_rows = sidebar(&app, Some(&bearer(bob_claims()))).await;
    let side_chat = row(&bob_rows, "alpha-side").expect("a reader of the source sees it");
    assert_eq!(side_chat["side_chat_audience"], json!("shared"));
    assert!(
        row(
            &sidebar(&app, Some(&bearer(alice_claims()))).await,
            "alpha-side"
        )
        .is_none(),
        "a caller who may not view the source does not see it"
    );
    Ok(())
}

/// The private-owner label is Meta's to write at a side chat fork. Console
/// member creation refuses it as a label, and a member that carries it from
/// anywhere else has no private owner: the label is not projected and hides
/// the member from nobody.
#[tokio::test]
async fn private_to_label_cannot_be_set_by_label_edits() -> TestResult {
    let (_temp_dir, runtime) = runtime("private-side-chat-label-edits").await?;
    let app = console_json_router_with_state(enforced_console(&runtime, None));
    let alice = bearer(alice_claims());
    let alice_scope = console_rpc_as(&app, Some(&alice), "meta/console/whoami", json!({})).await
        ["result"]["caller_scope"]
        .as_str()
        .expect("Alice's caller scope")
        .to_string();

    let refused = console_rpc_as(
        &app,
        Some(&alice),
        "meta/ensure_member",
        json!({
            "role": "worker",
            "agent_identity": "eve",
            "labels": { PRIVATE_TO: alice_scope },
        }),
    )
    .await;
    assert_eq!(refused["error"]["code"], json!(-32602), "{refused:#}");
    assert!(
        runtime
            .handle()
            .get_member(&AgentIdentity::from("eve"))
            .await?
            .is_none(),
        "nothing is seated"
    );

    Box::pin(runtime.handle().spawn_spec(
        SpawnMemberSpec::new("worker", AgentIdentity::from("forger")).with_labels(BTreeMap::from(
            [(PRIVATE_TO.to_string(), alice_scope.clone())],
        )),
    ))
    .await?;
    let bob_rows = sidebar(&app, Some(&bearer(bob_claims()))).await;
    let forger = row(&bob_rows, "forger").expect("a forged label hides nothing");
    assert!(
        forger["labels"].get(PRIVATE_TO).is_none(),
        "a member no side chat fork made has no private owner: {forger:#}"
    );
    Ok(())
}

/// The private-owner label of a side chat binds every agent check of the
/// console, whichever surface asks: the member rows, every route's access
/// priming, stream events and timeline frames. A caller other than its owner
/// and the admins is refused; a member that carries the label without a side
/// chat fork is refused to nobody.
#[tokio::test]
async fn a_private_owner_label_hides_a_side_chat_on_every_surface_and_a_forged_label_hides_nothing()
-> TestResult {
    use crate::console_aggregator::{
        ConsoleFrameSource, ConsoleFrameSourceKind, ConsoleFrameStatus, MetaConsoleAggregator,
        NewConsoleFrame,
    };

    let (_temp_dir, runtime) = runtime("private-side-chat-surfaces").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let state = enforced_console(&runtime, Some(everyone_drives_every_agent()));
    let app = console_json_router_with_state(state.clone());
    let alice_scope = console_rpc_as(
        &app,
        Some(&bearer(alice_claims())),
        "meta/console/whoami",
        json!({}),
    )
    .await["result"]["caller_scope"]
        .as_str()
        .expect("Alice's caller scope")
        .to_string();
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
    Box::pin(runtime.handle().spawn_spec(
        SpawnMemberSpec::new("worker", AgentIdentity::from("forger")).with_labels(BTreeMap::from(
            [(PRIVATE_TO.to_string(), alice_scope.clone())],
        )),
    ))
    .await?;

    // The console's member rows carry the owner of the side chat only.
    let (snapshot, _) = Box::pin(build_live_snapshot(
        &runtime,
        &[],
        None,
        &AllowAllConsoleVisibilityPolicy,
        &ConsoleSnapshotReadModel::default(),
        None,
    ))
    .await;
    let row_labels = |member: &str| {
        snapshot
            .members
            .iter()
            .find(|row| row.agent_identity == member)
            .map(|row| row.labels.clone())
            .unwrap_or_else(|| panic!("{member} is a console member"))
    };
    assert_eq!(
        row_labels("alpha-side").get(PRIVATE_TO),
        Some(&alice_scope),
        "a side chat fork's owner is projected"
    );
    assert_eq!(
        row_labels("forger").get(PRIVATE_TO),
        None,
        "a label no side chat fork wrote is not"
    );

    // Every route's access priming.
    let controller = state.access.clone().expect("access controller");
    prime_access_cache_from_runtime(&runtime, &controller).await;
    let view = |claims: Value| {
        authenticate(&state, claims)
            .and_then(|auth| auth.access_view)
            .expect("the console resolves the caller's access")
    };
    let (alice, bob, root) = (
        view(alice_claims()),
        view(bob_claims()),
        view(root_claims()),
    );
    assert!(alice.can_view_agent("alpha-side"), "its owner sees it");
    assert!(
        root.can_view_agent("alpha-side"),
        "the access admin sees it"
    );
    assert!(!bob.can_view_agent("alpha-side"), "another person does not");
    for action in [ACTION_AGENT_SEND, ACTION_AGENT_RETIRE, ACTION_AGENT_SPAWN] {
        assert!(!bob.allows_agent(action, "alpha-side"), "{action}");
    }
    assert!(bob.can_view_agent("forger"), "a forged label hides nothing");

    // Stream events, by the exact authority of the incarnation that emits
    // them.
    let entry = runtime
        .handle()
        .get_member(&AgentIdentity::from("alpha-side"))
        .await?
        .ok_or("the side chat is seated")?;
    let events = sse_member_authorization_projection(
        &runtime.handle(),
        Some(&runtime),
        &AllowAllConsoleVisibilityPolicy,
        "alpha-side",
        &entry.agent_runtime_id,
        entry.fence_token,
    )
    .await
    .ok_or("the side chat's events are attributed")?
    .attributes;
    assert!(
        alice
            .decide_agent_with_attributes(ACTION_AGENT_VIEW, &events)
            .is_allow()
    );
    assert!(
        !bob.decide_agent_with_attributes(ACTION_AGENT_VIEW, &events)
            .is_allow()
    );

    // Timeline frames, by the provenance the owner captured.
    let side_session = entry
        .bridge_session_id()
        .map(ToString::to_string)
        .ok_or("the side chat's session")?;
    let aggregator = MetaConsoleAggregator::in_memory();
    aggregator.register_runtime_handles_with_policy(
        "private-side-chat-frames",
        "",
        runtime.clone(),
        None,
        ConsoleEventStore::new(),
        Arc::new(AllowAllConsoleVisibilityPolicy),
    );
    let provenance = aggregator
        .capture_member_provenance_for_test("private-side-chat-frames", "alpha-side", &side_session)
        .await
        .ok_or("the owner captures the side chat")?;
    let frame = aggregator
        .store()
        .append_if_absent(NewConsoleFrame {
            id: None,
            dedupe_key: "private-side-chat-frame".to_string(),
            timestamp_ms: 2_000,
            runtime_key: "private-side-chat-frames".to_string(),
            identity: "alpha-side".to_string(),
            conversation_id: Some("alpha-side".to_string()),
            session_id: Some(side_session),
            kind: "text_delta".to_string(),
            status: ConsoleFrameStatus::Completed,
            payload: json!({ "delta": "heron" }),
            source: ConsoleFrameSource {
                member_provenance: Some(provenance),
                kind: ConsoleFrameSourceKind::ConsoleEvent,
                source_cursor: None,
            },
            source_event_id: Some("private-side-chat-frame".to_string()),
            interaction_id: None,
            turn_id: None,
            run_id: None,
            parent_frame_id: None,
            caused_by_frame_id: None,
        })
        .await?
        .frame;
    assert!(crate::console_aggregator::timeline_frame_visible_to(
        &alice, &frame
    ));
    assert!(crate::console_aggregator::timeline_frame_visible_to(
        &root, &frame
    ));
    assert!(!crate::console_aggregator::timeline_frame_visible_to(
        &bob, &frame
    ));

    // The sidebar, over the route.
    assert!(
        row(
            &sidebar(&app, Some(&bearer(bob_claims()))).await,
            "alpha-side"
        )
        .is_none()
    );
    assert!(row(&sidebar(&app, Some(&bearer(bob_claims()))).await, "forger").is_some());
    Ok(())
}

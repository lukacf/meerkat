//! The mob a console request is served for: the console's door hands its
//! dispatch the values of the mob the host's resolver resolved, and the
//! values its state captured only for a method of the host and on a door
//! without a resolver.

#![allow(clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use serde_json::{Value, json};

use super::*;
use crate::rpc::mob_addressing::{HOST_SCOPED_CONSOLE_METHODS, NamedMob};

/// A runtime over one mob whose id starts with `tag`.
async fn runtime(tag: &str) -> crate::UnifiedRuntime {
    let definition = meerkat_mob::MobDefinition::from_toml(&format!(
        "[mob]\nid = \"console-door-{tag}-{}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n",
        uuid::Uuid::new_v4().simple()
    ))
    .expect("mob definition");
    crate::UnifiedRuntime::builder()
        .definition(definition)
        .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
        .build()
        .await
        .expect("runtime builds")
}

/// An open console's state built by hand over `runtime`'s mob: it captured
/// that mob's wrapper and nothing else of it.
fn state_over(runtime: &crate::UnifiedRuntime) -> ConsoleJsonState {
    ConsoleJsonState {
        session_fences: crate::auth::session_fence::ConsoleSessionFences::detached(),
        decisions: RuntimeDecisionState::local_console(
            crate::ConsolePolicy {
                require_app_auth: false,
                ..Default::default()
            },
            None,
        ),
        runtime: Some(runtime.mob_runtime().clone()),
        module_runtime: None,
        contact_directory: None,
        event_log: None,
        gateway_peer_keys: None,
        identity_runtime: None,
        console_events: None,
        console_aggregator: None,
        mob_events: None,
        metadata_table: None,
        visibility_policy: Arc::new(crate::console_aggregator::AllowAllConsoleVisibilityPolicy),
        snapshot_read_model: Default::default(),
        access: None,
        memory_panel: None,
        identity_roster: None,
        workgraph: None,
        topology: None,
        job_health_projection: None,
        governed_ingress: Default::default(),
        mob_announcements: Default::default(),
        durable_decisions: Default::default(),
        console_audit: None,
        console_attention: None,
    }
}

/// The answer of `meta/status` on the console RPC door of `state`, with
/// `resolver` as the door's resolver.
async fn console_status(
    state: &ConsoleJsonState,
    resolver: Option<crate::unified_runtime::MobResolver>,
) -> Value {
    let response = console_rpc_with_resolver(
        state.clone(),
        None,
        resolver,
        HeaderMap::new(),
        "/console/rpc".parse().expect("uri"),
        json!({"jsonrpc": "2.0", "id": 1, "method": "meta/status", "params": {}}),
    )
    .await
    .into_response();
    let body = axum::body::to_bytes(response.into_body(), 1 << 24)
        .await
        .expect("the answer's body");
    serde_json::from_slice(&body).expect("the answer is JSON")
}

/// The step both call sites of the console's dispatch share: with the
/// host's resolver a request is served for the mob the resolver resolves,
/// whatever mob the state captured; a method of the host is served without
/// resolving (the state's values, which it does not read); a door without
/// a resolver serves the mob its state was built over; and an id the
/// resolver's host does not have is refused.
#[tokio::test]
async fn a_console_request_is_served_for_the_mob_the_hosts_resolver_resolves() {
    let captured = runtime("captured").await;
    let resolved = runtime("resolved").await;
    let captured_id = captured.mob_runtime().mob_id().clone();
    let resolved_id = resolved.mob_runtime().mob_id().clone();
    assert_ne!(captured_id, resolved_id);
    let state = state_over(&captured);
    let state_runtime = state.runtime.as_ref().expect("the state holds a runtime");
    let resolver = resolved.mob_resolver();
    let unnamed = NamedMob::default();

    let served = console_request_mob(
        &state,
        state_runtime,
        Some(&resolver),
        &unnamed,
        "meta/list_members",
    )
    .await
    .expect("a method that serves one mob resolves");
    assert_eq!(served.runtime.mob_id(), &resolved_id);
    assert!(
        served.mob_events.is_some(),
        "the resolved mob's event store"
    );
    assert!(
        served.topology.is_some(),
        "the resolved mob's topology seam"
    );

    // A method of the host is served without resolving: the door hands the
    // dispatch the state's values, as a door that asked the resolver to
    // resolve would not.
    for method in HOST_SCOPED_CONSOLE_METHODS {
        let served = console_request_mob(&state, state_runtime, Some(&resolver), &unnamed, method)
            .await
            .expect("a method of the host is served");
        assert_eq!(served.runtime.mob_id(), &captured_id, "{method}");
        assert!(served.mob_events.is_none(), "{method}");
    }

    let served = console_request_mob(&state, state_runtime, None, &unnamed, "meta/list_members")
        .await
        .expect("a door without a resolver serves its state's mob");
    assert_eq!(served.runtime.mob_id(), &captured_id);

    let absent = NamedMob::from_query(Some("console-door-absent-mob".to_string()));
    let refused = console_request_mob(
        &state,
        state_runtime,
        Some(&resolver),
        &absent,
        "meta/list_members",
    )
    .await
    .err();
    assert_eq!(
        refused
            .as_ref()
            .and_then(|error| error.data.as_ref())
            .map(|data| data["kind"].clone()),
        Some(json!("foreign_mob_target")),
        "{refused:?}"
    );
    // The console door has callers: its refusal names the id the request
    // named and no other mob.
    assert!(
        refused
            .as_ref()
            .and_then(|error| error.data.as_ref())
            .is_some_and(|data| {
                data["requested_mob_id"] == json!("console-door-absent-mob")
                    && data.get("gateway_mob_id").is_none()
            }),
        "{refused:?}"
    );
    captured.shutdown().await;
    resolved.shutdown().await;
}

/// The console RPC answers for the resolver's mob: a console whose state
/// was captured from a running mob and whose resolver is another host's,
/// with a stopped mob, reports the stopped mob; the same state on a door
/// without a resolver reports its own, running mob.
#[tokio::test]
async fn the_console_rpc_answers_for_the_mob_the_resolver_resolves() {
    let captured = runtime("captured").await;
    let resolved = runtime("resolved").await;
    resolved
        .mob_handle()
        .stop()
        .await
        .expect("the resolver's mob stops");
    let state = state_over(&captured);

    let through_the_resolver = console_status(&state, Some(resolved.mob_resolver())).await;
    assert_eq!(
        through_the_resolver["result"]["running"],
        json!(false),
        "{through_the_resolver}"
    );
    let without_a_resolver = console_status(&state, None).await;
    assert_eq!(
        without_a_resolver["result"]["running"],
        json!(true),
        "{without_a_resolver}"
    );
    captured.shutdown().await;
    resolved.shutdown().await;
}

/// Both call sites of the console's dispatch (the RPC answer and the
/// multipart answer) hand it the request mob's values, and neither hands
/// it the state's captured per-mob values any more. Read from the source:
/// the multipart site serves a send, which needs a console aggregator this
/// row does not build.
#[test]
fn both_call_sites_of_the_console_dispatch_take_the_request_mobs_values() {
    let source: String = include_str!("http_console.rs")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    assert_eq!(
        source
            .matches("handle_console_runtime_rpc_with_visibility(&request_mob.runtime,")
            .count(),
        2
    );
    assert_eq!(
        source
            .matches(
                "request_mob.identity_roster.clone(),request_mob.workgraph.as_ref(),\
                 request_mob.topology.as_ref(),"
            )
            .count(),
        2
    );
    assert_eq!(
        source
            .matches(
                "state.identity_roster.clone(),state.workgraph.as_ref(),state.topology.as_ref(),"
            )
            .count(),
        0
    );
    assert_eq!(source.matches("request_mob.mob_events.clone(),").count(), 2);
    assert_eq!(
        source
            .matches("request_mob.console_events.clone(),")
            .count(),
        2
    );
    assert_eq!(
        source
            .matches("state.gateway_peer_keys.as_ref(),state.console_events.clone(),")
            .count(),
        0
    );
    assert_eq!(
        source
            .matches("request_mob.identity_runtime.clone(),")
            .count(),
        2
    );
}

/// The console events the door hands its dispatch for a request of
/// `meta/list_members` that names the mob `named`, on a door with
/// `resolver`.
async fn console_events_handed(
    state: &ConsoleJsonState,
    runtime: &MobRuntime,
    resolver: Option<&crate::unified_runtime::MobResolver>,
    named: Option<&meerkat_mob::MobId>,
) -> ConsoleEventStore {
    let named = NamedMob::from_query(named.map(|mob| mob.as_str().to_string()));
    console_request_mob(state, runtime, resolver, &named, "meta/list_members")
        .await
        .expect("the request's mob resolves")
        .console_events
        .expect("the console keeps console events")
}

/// The console's dispatch is handed the console events of the mob its
/// request is served for. For a request the door resolved to a second mob
/// they are that mob's own store and not the host's: an event recorded
/// through them under an identity name is the second mob's, and the launch
/// mob's agent of the same name gets none. For a request that names no
/// mob, or the launch mob by its id, and on a door without the host's
/// resolver, they are the host's own store.
#[tokio::test]
async fn a_console_request_is_handed_the_console_events_of_the_mob_it_is_served_for() {
    let launch = runtime("launch").await;
    let second = runtime("second").await;
    let launch_id = launch.mob_runtime().mob_id().clone();
    let second_id = second.mob_runtime().mob_id().clone();
    let host_events = launch.console_events();
    let state = ConsoleJsonState {
        console_events: Some(host_events.clone()),
        ..state_over(&launch)
    };
    let resolver = launch
        .mob_resolver()
        .serving_a_second_mob_in_tests(&second.mob_resolver());
    let wrapper = launch.mob_runtime();

    let for_the_second_mob =
        console_events_handed(&state, wrapper, Some(&resolver), Some(&second_id)).await;
    let for_no_mob = console_events_handed(&state, wrapper, Some(&resolver), None).await;
    let for_the_launch_mob =
        console_events_handed(&state, wrapper, Some(&resolver), Some(&launch_id)).await;
    let without_a_resolver = console_events_handed(&state, wrapper, None, None).await;

    assert!(
        for_the_second_mob.is_the_store(&host_events.of_mob(Some(&second_id))),
        "a request for the second mob is handed the second mob's console events"
    );
    assert!(
        !for_the_second_mob.is_the_store(&host_events),
        "a request for the second mob is not handed the host's own console events"
    );
    for (named, events) in [
        ("no mob", &for_no_mob),
        ("the launch mob by its id", &for_the_launch_mob),
        ("a door without a resolver", &without_a_resolver),
    ] {
        assert!(
            events.is_the_store(&host_events),
            "{named}: the host's own console events"
        );
    }

    let of_the_launch_mob_before = host_events.latest_event_id().await;
    let of_the_second_mob_before = for_the_second_mob.latest_event_id().await;
    for_the_second_mob
        .record_lifecycle("alice", "retired", json!({}))
        .await;
    assert_ne!(
        for_the_second_mob.latest_event_id().await,
        of_the_second_mob_before,
        "the event is recorded under the second mob's agent"
    );
    assert_eq!(
        host_events.latest_event_id().await,
        of_the_launch_mob_before,
        "the launch mob's agent of the same name gets no event"
    );

    launch.shutdown().await;
    second.shutdown().await;
}

/// One GET on `router`: the status and the body as JSON.
async fn get_json(router: &Router, uri: &str) -> (StatusCode, Value) {
    use tower::ServiceExt;
    let request = axum::http::Request::builder()
        .method("GET")
        .uri(uri)
        .body(axum::body::Body::empty())
        .expect("request");
    let response = router.clone().oneshot(request).await.expect("the answer");
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 1 << 24)
        .await
        .expect("the answer's body");
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

/// `GET /console/experience` answers for the resolver's mob: a console
/// whose state was captured from one mob and whose router carries the
/// resolver of another host shows the resolver's mob; the same state on a
/// router without a resolver shows its own.
#[tokio::test]
async fn the_console_experience_answers_for_the_mob_the_resolver_resolves() {
    let captured = runtime("captured").await;
    let resolved = runtime("resolved").await;
    let captured_id = captured.mob_runtime().mob_id().to_string();
    let resolved_id = resolved.mob_runtime().mob_id().to_string();
    let shown_mob = "/topology/live_snapshot/mob_id";

    let through_the_resolver = console_json_router_with_state(state_over(&captured))
        .layer(axum::Extension(resolved.mob_resolver()));
    let (status, body) = get_json(&through_the_resolver, "/console/experience").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.pointer(shown_mob), Some(&json!(resolved_id)), "{body}");

    let without_a_resolver = console_json_router_with_state(state_over(&captured));
    let (status, body) = get_json(&without_a_resolver, "/console/experience").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.pointer(shown_mob), Some(&json!(captured_id)), "{body}");
    captured.shutdown().await;
    resolved.shutdown().await;
}

/// A runtime over one mob that has an identity plane and a console roster:
/// its host gave it the stores and the roster of an identity-first launch.
async fn identity_first_runtime(tag: &str, scratch: &tempfile::TempDir) -> crate::UnifiedRuntime {
    let definition = meerkat_mob::MobDefinition::from_toml(&format!(
        "[mob]\nid = \"console-door-{tag}-{}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n",
        uuid::Uuid::new_v4().simple()
    ))
    .expect("mob definition");
    let roster = Arc::new(crate::identity_first::MutableRosterProvider::new(Vec::new()));
    let runtime = crate::UnifiedRuntime::builder()
        .definition(definition)
        .continuity_store(Arc::new(
            crate::identity_first::LocalContinuityStore::in_memory().expect("continuity store"),
        ))
        .lease_provider(Arc::new(crate::identity_first::LocalLeaseProvider::new()))
        .roster_provider(Arc::clone(&roster) as Arc<dyn crate::identity_first::RosterProvider>)
        .scratch_dir(scratch.path())
        .ephemeral_runtime_store(true)
        .identity_runtime_instance_id(format!("console-door-{tag}"))
        .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
        .build()
        .await
        .expect("runtime builds");
    runtime.set_console_identity_roster(roster);
    runtime
}

/// What a console route serves its request with: with the host's resolver
/// the six per-mob values of the state are the resolved mob's, read at the
/// request, whatever the state captured when its router was built; a
/// route without a resolver keeps the values of the mob its state was
/// built over; an id the host does not serve is refused over HTTP with the
/// id the request named and no other mob.
#[tokio::test]
async fn a_console_route_is_served_with_the_values_of_the_mob_the_resolver_resolves() {
    let scratch = tempfile::tempdir().expect("tempdir");
    let captured = runtime("captured").await;
    let resolved = identity_first_runtime("resolved", &scratch).await;
    let captured_id = captured.mob_runtime().mob_id().clone();
    let resolved_id = resolved.mob_runtime().mob_id().clone();
    let resolver = resolved.mob_resolver();
    let resolved_mob = resolver
        .resolve(None)
        .await
        .expect("the resolver's default mob");
    // The fixture tells the two apart on every value. The resolved mob has
    // an identity runtime, an event store, a console roster and a topology
    // seam, and the captured state has none of them; the state holds a
    // WorkGraph service exactly when the resolved mob has none.
    assert!(resolved_mob.identity_runtime().is_some());
    assert!(resolved_mob.console_identity_roster().is_some());
    let resolved_has_a_workgraph = resolved_mob.workgraph_service().is_some();
    let mut state = state_over(&captured);
    state.workgraph = (!resolved_has_a_workgraph).then(|| {
        meerkat::WorkGraphService::with_scope(
            Arc::new(meerkat::MemoryWorkGraphStore::new()),
            "console-door-captured-realm".to_string(),
            meerkat::WorkNamespace::default(),
        )
    });
    let unnamed = NamedMob::default();
    // The caller of these requests: one of a console without access rules.
    let caller = || AccessController::audience_only().view_for_subject(None);

    let (served, _) = console_route_state(state.clone(), Some(&resolver), &unnamed, caller())
        .await
        .expect("a request that names no mob is served");
    assert_eq!(
        served.runtime.as_ref().map(MobRuntime::mob_id),
        Some(&resolved_id)
    );
    assert!(served.identity_runtime.is_some(), "its identity runtime");
    assert!(served.mob_events.is_some(), "its event store");
    assert!(served.topology.is_some(), "its topology seam");
    assert!(served.identity_roster.is_some(), "its console roster");
    assert_eq!(
        served.workgraph.is_some(),
        resolved_has_a_workgraph,
        "its WorkGraph service"
    );

    let by_id = NamedMob::from_query(Some(resolved_id.to_string()));
    let (served, _) = console_route_state(state.clone(), Some(&resolver), &by_id, caller())
        .await
        .expect("the mob's own id is the same request");
    assert_eq!(
        served.runtime.as_ref().map(MobRuntime::mob_id),
        Some(&resolved_id)
    );

    let (served, _) = console_route_state(state.clone(), None, &unnamed, caller())
        .await
        .expect("a route without a resolver serves its state's mob");
    assert_eq!(
        served.runtime.as_ref().map(MobRuntime::mob_id),
        Some(&captured_id)
    );
    assert!(served.identity_runtime.is_none());
    assert!(served.mob_events.is_none());
    assert!(served.topology.is_none());
    assert!(served.identity_roster.is_none());
    assert_eq!(served.workgraph.is_some(), !resolved_has_a_workgraph);

    // An id the host does not serve, with and without a resolver; a route
    // without a resolver also refuses the id of the resolver's mob, which
    // its state was not built over.
    let absent = NamedMob::from_query(Some("console-door-absent-mob".to_string()));
    for (resolver, named, requested) in [
        (Some(&resolver), &absent, "console-door-absent-mob"),
        (None, &absent, "console-door-absent-mob"),
        (None, &by_id, resolved_id.as_str()),
    ] {
        let refused = console_route_state(state.clone(), resolver, named, caller())
            .await
            .err();
        let Some((status, Json(body))) = refused else {
            panic!("{requested} is refused");
        };
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(
            body,
            json!({"error": "foreign_mob_target", "requested_mob_id": requested})
        );
        let checked = refuse_console_route_of_another_mob(&state, resolver, named, caller())
            .await
            .err();
        assert_eq!(
            checked.map(|(status, Json(body))| (status, body)),
            Some((StatusCode::NOT_FOUND, body))
        );
    }
    assert!(
        refuse_console_route_of_another_mob(&state, Some(&resolver), &unnamed, caller())
            .await
            .is_ok()
    );
    assert!(
        refuse_console_route_of_another_mob(&state, None, &unnamed, caller())
            .await
            .is_ok()
    );
    captured.shutdown().await;
    resolved.shutdown().await;
}

/// A runtime over one mob whose launch is a candidate: it does not speak
/// for its composition, so it admits no member input.
async fn candidate_runtime(tag: &str, scratch: &tempfile::TempDir) -> crate::UnifiedRuntime {
    let definition = meerkat_mob::MobDefinition::from_toml(&format!(
        "[mob]\nid = \"console-door-{tag}-{}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n",
        uuid::Uuid::new_v4().simple()
    ))
    .expect("mob definition");
    let spec = crate::MobBootstrapSpec::ephemeral(
        definition,
        meerkat_mob::MobStorage::in_memory(),
        scratch.path().to_path_buf(),
        4,
        None,
    )
    .expect("ephemeral spec")
    .with_options(crate::MobBootstrapOptions {
        allow_ephemeral_sessions: true,
        notify_orchestrator_on_resume: true,
        default_llm_client: Some(Arc::new(meerkat_client::TestClient::default())),
    })
    .with_composition_authority(
        crate::mob_composition_manifest::CompositionAuthority::NonAuthoritative,
    );
    crate::UnifiedRuntime::bootstrap_with_options(
        spec,
        crate::MetaConfig {
            modules: Vec::new(),
            discovery: crate::DiscoverySpec {
                namespace: format!("console-door-{tag}"),
                modules: Vec::new(),
            },
            pre_spawn: Vec::new(),
        },
        Vec::new(),
        std::time::Duration::from_secs(2),
        crate::runtime::RuntimeOptions::default(),
        Arc::new(crate::InMemoryMetadataStore::new()),
    )
    .await
    .expect("bootstrap runtime")
}

/// One POST of a JSON body on `router`: the status and the body as JSON.
async fn post_json(router: &Router, uri: &str, body: &Value) -> (StatusCode, Value) {
    use tower::ServiceExt;
    let request = axum::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .expect("request");
    let response = router.clone().oneshot(request).await.expect("the answer");
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 1 << 24)
        .await
        .expect("the answer's body");
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

/// The console's send routes answer for the resolver's mob: a console
/// whose state was captured from a launch that speaks for its composition
/// and whose router carries the resolver of a candidate launch refuses a
/// send and its recovery as a candidate does; the same state on a router
/// without a resolver does not.
#[tokio::test]
async fn the_console_send_routes_answer_for_the_mob_the_resolver_resolves() {
    let scratch = tempfile::tempdir().expect("tempdir");
    let captured = runtime("captured").await;
    let resolved = candidate_runtime("resolved", &scratch).await;
    assert!(captured.mob_runtime().speaks_for_composition());
    assert!(!resolved.mob_runtime().speaks_for_composition());
    let requests = [
        (
            "/console/send",
            json!({
                "identity": "worker-1",
                "content": "hello",
                "origin": "console",
                "idempotency_key": "console-door-send"
            }),
        ),
        (
            "/console/send/recover",
            json!({
                "identity": "worker-1",
                "origin": "console",
                "idempotency_key": "console-door-send"
            }),
        ),
    ];
    let through_the_resolver = console_json_router_with_state(state_over(&captured))
        .layer(axum::Extension(resolved.mob_resolver()));
    let without_a_resolver = console_json_router_with_state(state_over(&captured));
    for (uri, body) in &requests {
        let (status, answer) = post_json(&through_the_resolver, uri, body).await;
        assert_eq!(
            (status, &answer["error"]),
            (StatusCode::CONFLICT, &json!("candidate_launch")),
            "{uri}: {answer}"
        );
        let (status, answer) = post_json(&without_a_resolver, uri, body).await;
        assert!(
            status != StatusCode::CONFLICT && answer["error"] != json!("candidate_launch"),
            "{uri}: {status} {answer}"
        );
    }
    captured.shutdown().await;
    resolved.shutdown().await;
}

/// The console's tool application routes answer for the resolver's mob: a
/// console whose state was captured from a launch that speaks for its
/// composition and whose router carries the resolver of a candidate launch
/// refuses a tool call as a candidate does; the same state on a router
/// without a resolver does not.
#[tokio::test]
async fn the_console_app_routes_answer_for_the_mob_the_resolver_resolves() {
    let scratch = tempfile::tempdir().expect("tempdir");
    let captured = runtime("captured").await;
    let resolved = candidate_runtime("resolved", &scratch).await;
    let call = json!({
        "identity": "worker-1",
        "sessionId": uuid::Uuid::new_v4().to_string(),
        "toolCallId": "console-door-call",
        "name": "console-door-tool",
        "arguments": {}
    });
    let uri = "/console/mcp-apps/call-tool";
    let through_the_resolver = console_json_router_with_state(state_over(&captured))
        .layer(axum::Extension(resolved.mob_resolver()));
    let (status, answer) = post_json(&through_the_resolver, uri, &call).await;
    assert_eq!(
        (status, &answer["error"]),
        (StatusCode::CONFLICT, &json!("candidate_launch")),
        "{answer}"
    );
    let without_a_resolver = console_json_router_with_state(state_over(&captured));
    let (status, answer) = post_json(&without_a_resolver, uri, &call).await;
    assert!(
        status != StatusCode::CONFLICT && answer["error"] != json!("candidate_launch"),
        "{status} {answer}"
    );
    captured.shutdown().await;
    resolved.shutdown().await;
}

/// The secret the fixture console's sign-in tokens are signed with.
const DOOR_ROWS_SIGNING_SECRET: &str = "console-door-rows-signing";
/// An administrator of the fixture's access configuration.
const DOOR_ROWS_ADMIN: &str = "root@example.test";
/// A person who signs in and whom no access rule names.
const DOOR_ROWS_WITHOUT_RULES: &str = "nobody@example.test";
/// A person one access rule names: an allow that selects the second mob.
const DOOR_ROWS_OF_THE_SECOND_MOB: &str = "second@example.test";
/// A person two access rules name: an allow that selects the second mob,
/// and a deny of every action in it.
const DOOR_ROWS_ALLOWED_AND_DENIED: &str = "both@example.test";
/// A person one access rule names: an allow of the runtime administration
/// methods, the audit read and the view of agents that lists no mob, so it
/// holds in the launch mob and selects no other.
const DOOR_ROWS_OPERATOR: &str = "operator@example.test";
/// A person two access rules name: the same allow that lists no mob, and
/// an allow of the same actions that selects the second mob. So the person
/// holds the actions in the launch mob and may address the second mob.
const DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB: &str = "operator-of-second@example.test";
/// A person no rule names by subject, who is in a group one rule names: an
/// allow of the view of agents that lists no mob.
const DOOR_ROWS_IN_A_GROUP: &str = "grouped@example.test";
/// The group of [`DOOR_ROWS_IN_A_GROUP`].
const DOOR_ROWS_GROUP: &str = "door-rows-group";

/// A sign-in token of `email` for the fixture console.
fn door_rows_token(email: &str) -> String {
    jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
        &json!({
            "iss": "http://127.0.0.1/meta-gateway",
            "aud": "persistent-gateway",
            "sub": email,
            "email": email,
            "exp": chrono::Utc::now().timestamp() + 300,
        }),
        &jsonwebtoken::EncodingKey::from_secret(DOOR_ROWS_SIGNING_SECRET.as_bytes()),
    )
    .expect("token")
}

/// The state of a console over `launch`'s mob that requires a sign-in and
/// has access rules about a second mob `second_mob`: one administrator, one
/// person whose allow selects the second mob, one person whose allow
/// selects it and whose deny takes every action in it away, a person no
/// rule names, two people who hold the runtime administration methods and
/// the audit read by a rule that lists no mob (a second rule selects the
/// second mob for one of them), and a person a rule names through a group.
fn door_rows_state(launch: &crate::UnifiedRuntime, second_mob: &str) -> ConsoleJsonState {
    let second_mob = second_mob.to_string();
    let operator_actions = vec![
        crate::access::ACTION_RUNTIME_ADMIN.to_string(),
        crate::access::ACTION_AUDIT_VIEW.to_string(),
        crate::access::ACTION_AGENT_VIEW.to_string(),
    ];
    let controller = AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![DOOR_ROWS_ADMIN.to_string()],
        groups: std::collections::BTreeMap::from([(
            DOOR_ROWS_GROUP.to_string(),
            crate::access::AccessGroup {
                description: None,
                members: vec![DOOR_ROWS_IN_A_GROUP.to_string()],
            },
        )]),
        rules: vec![
            crate::access::AccessRule {
                id: "operates-the-launch-mob".to_string(),
                subjects: vec![
                    DOOR_ROWS_OPERATOR.to_string(),
                    DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB.to_string(),
                ],
                actions: operator_actions.clone(),
                ..crate::access::AccessRule::default()
            },
            crate::access::AccessRule {
                id: "operates-the-second-mob".to_string(),
                subjects: vec![DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB.to_string()],
                actions: operator_actions,
                mobs: vec![second_mob.clone()],
                ..crate::access::AccessRule::default()
            },
            crate::access::AccessRule {
                id: "the-group-sees-the-launch-mob".to_string(),
                groups: vec![DOOR_ROWS_GROUP.to_string()],
                actions: vec![crate::access::ACTION_AGENT_VIEW.to_string()],
                ..crate::access::AccessRule::default()
            },
            crate::access::AccessRule {
                id: "sees-the-second-mob".to_string(),
                subjects: vec![
                    DOOR_ROWS_OF_THE_SECOND_MOB.to_string(),
                    DOOR_ROWS_ALLOWED_AND_DENIED.to_string(),
                ],
                actions: vec![crate::access::ACTION_AGENT_VIEW.to_string()],
                mobs: vec![second_mob.clone()],
                ..crate::access::AccessRule::default()
            },
            crate::access::AccessRule {
                id: "nothing-in-the-second-mob".to_string(),
                effect: crate::access::AccessEffect::Deny,
                subjects: vec![DOOR_ROWS_ALLOWED_AND_DENIED.to_string()],
                actions: vec!["*".to_string()],
                mobs: vec![second_mob],
                ..crate::access::AccessRule::default()
            },
        ],
        ..crate::access::AccessControlConfig::default()
    })
    .expect("access configuration");
    let decisions = crate::console_auth_config::parse_console_auth_config(&json!({
        "shared_secret": DOOR_ROWS_SIGNING_SECRET,
        "email_allowlist": [
            DOOR_ROWS_ADMIN,
            DOOR_ROWS_WITHOUT_RULES,
            DOOR_ROWS_OF_THE_SECOND_MOB,
            DOOR_ROWS_ALLOWED_AND_DENIED,
            DOOR_ROWS_OPERATOR,
            DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB,
            DOOR_ROWS_IN_A_GROUP,
        ],
    }))
    .expect("console sign-in configuration");
    ConsoleJsonState {
        decisions,
        access: Some(controller),
        ..state_over(launch)
    }
}

/// The console of `state` on a router whose resolver serves `launch`'s mob
/// and, as a second mob, `second`'s. The router and the resolver it
/// carries.
fn console_of_two_mobs_over(
    state: ConsoleJsonState,
    launch: &crate::UnifiedRuntime,
    second: &crate::UnifiedRuntime,
) -> (Router, crate::unified_runtime::MobResolver) {
    let resolver = launch
        .mob_resolver()
        .serving_a_second_mob_in_tests(&second.mob_resolver());
    let router = console_json_router_with_state(state).layer(axum::Extension(resolver.clone()));
    (router, resolver)
}

/// A console over `launch`'s mob that requires a sign-in and has access
/// rules ([`door_rows_state`]), on a router whose resolver also serves
/// `second`'s mob. The router and the resolver it carries.
fn console_of_two_mobs(
    launch: &crate::UnifiedRuntime,
    second: &crate::UnifiedRuntime,
) -> (Router, crate::unified_runtime::MobResolver) {
    let state = door_rows_state(launch, second.mob_id().as_str());
    console_of_two_mobs_over(state, launch, second)
}

/// The requests [`console_doors_naming`] sends to the console's two
/// request doors, each with the method it asks: a method that serves one
/// mob on the request door and on the multipart route, a method whose own
/// parameter the mob id is, and a method of the host.
const REQUEST_DOORS: [(&str, &str); 4] = [
    ("console RPC", "meta/list_members"),
    ("console multipart", "meta/console/send"),
    (
        "console RPC, a method that keeps its mob id",
        "meta/mob_events/query",
    ),
    ("console RPC, a method of the host", "meta/access/status"),
];

/// The console's doors that read a mob id (its request door with three
/// kinds of method, the multipart route and the twelve console routes),
/// each with a request that names the mob `mob_id`, or no mob, and nothing
/// else it could act on.
fn console_doors_naming(
    mob_id: Option<&str>,
) -> Vec<(&'static str, axum::http::Request<axum::body::Body>)> {
    let get = |uri: String| {
        axum::http::Request::builder()
            .method("GET")
            .uri(uri)
            .body(axum::body::Body::empty())
            .expect("GET request")
    };
    let post = |uri: &str, body: &Value| {
        axum::http::Request::builder()
            .method("POST")
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(body.to_string()))
            .expect("POST request")
    };
    let naming = |mut body: Value| {
        if let Some(mob_id) = mob_id {
            body["mob_id"] = json!(mob_id);
        }
        body
    };
    let query = mob_id.map_or_else(String::new, |mob_id| format!("?mob_id={mob_id}"));
    let rpc = |method: &str| json!({"jsonrpc": "2.0", "id": "row", "method": method, "params": naming(json!({}))});
    let multipart = format!(
        "--rows\r\nContent-Disposition: form-data; name=\"payload\"\r\n\r\n{}\r\n--rows--\r\n",
        rpc(REQUEST_DOORS[1].1)
    );
    let app = json!({
        "identity": "worker-1",
        "sessionId": "7f3c2a10-5b6d-4e8f-9a1b-2c3d4e5f6a7b",
        "toolCallId": "console-door-rows-call",
    });
    let mut read = app.clone();
    read["uri"] = json!("ui://console-door-rows/app");
    let mut call = app.clone();
    call["name"] = json!("console-door-rows-tool");
    call["arguments"] = json!({});
    let mut doors = vec![
        (
            REQUEST_DOORS[0].0,
            post("/console/rpc", &rpc(REQUEST_DOORS[0].1)),
        ),
        (
            REQUEST_DOORS[1].0,
            axum::http::Request::builder()
                .method("POST")
                .uri("/console/rpc/multipart")
                .header(header::CONTENT_TYPE, "multipart/form-data; boundary=rows")
                .body(axum::body::Body::from(multipart))
                .expect("multipart request"),
        ),
        (
            REQUEST_DOORS[2].0,
            post("/console/rpc", &rpc(REQUEST_DOORS[2].1)),
        ),
        (
            REQUEST_DOORS[3].0,
            post("/console/rpc", &rpc(REQUEST_DOORS[3].1)),
        ),
    ];
    for (door, path) in [
        ("GET /console/experience", "/console/experience"),
        ("GET /console/modules", "/console/modules"),
        ("GET /console/identities", "/console/identities"),
        ("GET /console/timeline", "/console/timeline"),
        ("GET /console/timeline/stream", "/console/timeline/stream"),
        (
            "GET /console/identity/worker-1/stream",
            "/console/identity/worker-1/stream",
        ),
        ("GET /console/gating/stream", "/console/gating/stream"),
    ] {
        doors.push((door, get(format!("{path}{query}"))));
    }
    for (door, uri, body) in [
        (
            "POST /console/send",
            "/console/send",
            json!({
                "identity": "worker-1",
                "content": "hello",
                "origin": "console",
                "idempotency_key": "console-door-rows-send"
            }),
        ),
        (
            "POST /console/send/recover",
            "/console/send/recover",
            json!({
                "identity": "worker-1",
                "origin": "console",
                "idempotency_key": "console-door-rows-send"
            }),
        ),
        (
            "POST /console/mcp-apps/resolve",
            "/console/mcp-apps/resolve",
            app,
        ),
        (
            "POST /console/mcp-apps/read-resource",
            "/console/mcp-apps/read-resource",
            read,
        ),
        (
            "POST /console/mcp-apps/call-tool",
            "/console/mcp-apps/call-tool",
            call,
        ),
    ] {
        doors.push((door, post(uri, &naming(body))));
    }
    doors
}

/// What a door answered: its status, its headers (sorted, without the
/// time of the answer) and, when the answer is JSON, its body. The body of
/// a stream is not read.
type DoorAnswer = (StatusCode, Vec<(String, String)>, String);

/// The answer of `request` on `app`, sent with a sign-in of `email`.
async fn ask_as(
    app: &Router,
    mut request: axum::http::Request<axum::body::Body>,
    email: &str,
) -> DoorAnswer {
    request.headers_mut().insert(
        header::AUTHORIZATION,
        format!("Bearer {}", door_rows_token(email))
            .parse()
            .expect("header value"),
    );
    ask(app, request).await
}

/// The answer of `request` on `app`, sent as it is.
async fn ask(app: &Router, request: axum::http::Request<axum::body::Body>) -> DoorAnswer {
    use tower::ServiceExt;
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        app.clone().oneshot(request),
    )
    .await
    .expect("the door answers in time")
    .expect("the door answers");
    let status = response.status();
    let mut headers: Vec<(String, String)> = response
        .headers()
        .iter()
        .filter(|(name, _)| *name != header::DATE)
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                String::from_utf8_lossy(value.as_bytes()).into_owned(),
            )
        })
        .collect();
    headers.sort();
    let json = headers
        .iter()
        .any(|(name, value)| name == "content-type" && value.starts_with("application/json"));
    if !json {
        return (status, headers, String::new());
    }
    let body = axum::body::to_bytes(response.into_body(), 1 << 22)
        .await
        .expect("the answer's body");
    (status, headers, String::from_utf8_lossy(&body).into_owned())
}

/// Whether `answer` is the refusal of the mob id `requested`, as a console
/// door gives it: a JSON-RPC error on the two request doors, HTTP 404 on a
/// console route.
fn is_the_mob_ids_refusal(door: &str, answer: &DoorAnswer, requested: &str) -> bool {
    let Ok(body) = serde_json::from_str::<Value>(&answer.2) else {
        return false;
    };
    if door.starts_with("console") {
        answer.0 == StatusCode::OK
            && body["error"]["code"] == json!(-32602)
            && body["error"]["data"]["kind"] == json!("foreign_mob_target")
            && body["error"]["data"]["requested_mob_id"] == json!(requested)
    } else {
        answer.0 == StatusCode::NOT_FOUND
            && body == json!({"error": "foreign_mob_target", "requested_mob_id": requested})
    }
}

/// An id no mob of the fixture has, of the length of `mob_id`: that id
/// with its last character changed. So two answers that differ only in the
/// id they echo have one length.
fn an_id_no_mob_has(mob_id: &str) -> String {
    let mut absent = mob_id.to_string();
    let last = absent.pop().expect("a mob id is not empty");
    absent.push(if last == '0' { '1' } else { '0' });
    absent
}

/// `answer` with the id `id` it echoes replaced by one text, so that two
/// answers that differ only in the id their requests named compare equal.
fn apart_from_the_id(answer: &DoorAnswer, id: &str) -> DoorAnswer {
    (
        answer.0,
        answer.1.clone(),
        answer.2.replace(id, "<the id the request named>"),
    )
}

/// A request of `method` with `params` on the console's request door.
fn rpc_request(method: &str, params: &Value) -> axum::http::Request<axum::body::Body> {
    axum::http::Request::builder()
        .method("POST")
        .uri("/console/rpc")
        .header(header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(
            json!({"jsonrpc": "2.0", "id": "row", "method": method, "params": params}).to_string(),
        ))
        .expect("console request")
}

/// The params of a request that names the mob `mob_id`, or no mob, and
/// nothing else.
fn naming_mob(mob_id: Option<&str>) -> Value {
    mob_id.map_or_else(|| json!({}), |mob_id| json!({"mob_id": mob_id}))
}

/// How often `resolver` was asked to resolve the mob `mob_id` or to say
/// whether it is served.
fn asked_about(resolver: &crate::unified_runtime::MobResolver, mob_id: &str) -> usize {
    resolver
        .resolved_for_in_tests()
        .into_iter()
        .chain(resolver.asked_whether_served_in_tests())
        .filter(|named| named.as_ref().is_some_and(|named| named.as_str() == mob_id))
        .count()
}

/// The console methods served per mob, as the console's file lists them:
/// the string literals of its constant `CONSOLE_METHODS_SERVED_PER_MOB`,
/// read from its source. Empty when the file names no such list.
fn console_methods_served_per_mob() -> Vec<&'static str> {
    let source = include_str!("http_console.rs");
    let Some(from) = source.find("const CONSOLE_METHODS_SERVED_PER_MOB") else {
        return Vec::new();
    };
    let list = &source[from..];
    let list = &list[..list.find("];").unwrap_or(list.len())];
    list.split('"').skip(1).step_by(2).collect()
}

/// The console methods of the host, as the console lists them
/// (`CONSOLE_METHODS_OF_THE_HOST`, beside the `mobs` selector, whose lists
/// are among them).
fn console_methods_of_the_host() -> Vec<&'static str> {
    mobs_selector::CONSOLE_METHODS_OF_THE_HOST.to_vec()
}

/// The methods of the host, by name: the methods that answer from values
/// of the host and take no mob as a selector of what they answer. A mob
/// named beside one of them is judged like any other id and selects
/// nothing. The list is closed: the access administration and its preview,
/// the blob upload, the caller's own attention entries and their
/// dismissal, the console audit read, who the caller is, the decision list
/// and its rendering, and the host's one gating state (its pending
/// entries, its audit and its decision). The console's own list
/// (`CONSOLE_METHODS_OF_THE_HOST`) must name exactly these: a name joins
/// both in one change, with what its row here asks of it. Sorted, one name
/// per line.
const METHODS_OF_THE_HOST: &[&str] = &[
    "meta/access/enable",
    "meta/access/get",
    "meta/access/groups/delete",
    "meta/access/groups/set",
    "meta/access/preview",
    "meta/access/rules/delete",
    "meta/access/rules/upsert",
    "meta/access/set",
    "meta/access/status",
    "meta/blob/upload",
    "meta/console/attention",
    "meta/console/attention/dismiss",
    "meta/console/audit",
    "meta/console/whoami",
    "meta/decisions/list",
    "meta/decisions/rendering",
    "meta/gating/audit",
    "meta/gating/decide",
    "meta/gating/pending",
    "meta/gating/questions",
];

/// Every console method of the addressing rows' lists: the methods that
/// serve one mob, the host-scoped ones, and the ones served from the launch
/// mob's own wiring. The addressing rows keep these lists equal to what the
/// console's dispatch serves, so a method added to the console is among
/// them.
fn every_console_method() -> Vec<&'static str> {
    let mut methods = console_methods_that_serve_one_mob();
    methods.extend(HOST_SCOPED_CONSOLE_METHODS);
    methods.extend(CONSOLE_METHODS_OF_THE_LAUNCH_MOBS_WIRING);
    methods
}

/// The console methods a request that names another mob is refused on:
/// every console method that is on neither of the console's two lists (the
/// methods served per mob and the methods of the host), and three names no
/// list of the console holds, as a method added later is. The announcement
/// methods that carry a mob of their own are not among them: the id they
/// name is their own parameter, and
/// [`an_announcement_method_that_names_its_own_mob_is_judged_with_its_caller`]
/// asks them.
fn console_methods_refused_for_another_mob() -> Vec<&'static str> {
    let served_per_mob = console_methods_served_per_mob();
    let mut methods = every_console_method();
    methods.extend([
        "meta/live/status",
        "live/webrtc/answer",
        "meta/a_method_no_list_names",
    ]);
    methods.retain(|method| {
        !served_per_mob.contains(method)
            && !METHODS_OF_THE_HOST.contains(method)
            && !crate::rpc::mob_addressing::ANNOUNCEMENT_METHODS_NAMING_A_MOB.contains(method)
            // The methods over the mobs of the host that act on one mob
            // name it with their own required `mob_id` and answer for every
            // mob the host has: their rows are the family's own
            // (`rpc::mob_hosting_tests`).
            && !crate::rpc::mob_hosting::MOB_HOSTING_METHODS_NAMING_A_MOB.contains(method)
    });
    methods
}

/// The caller a console door goes on with for a mob other than the launch
/// mob sees that mob: its audience and its access view read a name as that
/// mob's agent, in the form a route holds its caller (by value) and in the
/// form the request doors hold it (by reference: the caller the door was
/// handed is left as it was).
#[test]
fn a_console_doors_caller_is_handed_back_seeing_the_mob_of_its_request() {
    use crate::rpc::mob_addressing::DoorCaller;
    let second = meerkat_mob::MobId::from("second");
    let view = AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![DOOR_ROWS_ADMIN.to_string()],
        rules: vec![crate::access::AccessRule {
            id: "sees-second".to_string(),
            subjects: vec![DOOR_ROWS_OF_THE_SECOND_MOB.to_string()],
            actions: vec![crate::access::ACTION_AGENT_VIEW.to_string()],
            mobs: vec!["second".to_string()],
            ..crate::access::AccessRule::default()
        }],
        ..crate::access::AccessControlConfig::default()
    })
    .expect("access configuration")
    .view_for_subject(Some(DOOR_ROWS_OF_THE_SECOND_MOB));
    let caller = ConsoleHttpAuthContext {
        principal: Some(DOOR_ROWS_OF_THE_SECOND_MOB.to_string()),
        service_identity: false,
        send_scope: ConsoleSendScope::Anonymous,
        access_view: Some(view.clone()),
        audience: view,
        human: None,
        verification: None,
    };
    let sees = |caller: &ConsoleHttpAuthContext| {
        [
            caller.audience.can_view_agent("alice"),
            caller
                .access_view
                .as_ref()
                .is_some_and(|view| view.can_view_agent("alice")),
        ]
    };
    assert_eq!(
        sees(&caller),
        [false; 2],
        "the rule selects the second mob only"
    );
    assert_eq!(sees(&caller.clone().seeing_mob(&second)), [true; 2]);
    let by_reference = std::borrow::Cow::Borrowed(&caller).seeing_mob(&second);
    assert_eq!(sees(&by_reference), [true; 2]);
    assert_eq!(sees(&caller), [false; 2]);
}

/// A mob other than the launch mob that the caller may not address is
/// answered, on every console door, exactly as an id no mob has: the same
/// status, the same headers and the same body apart from the id the request
/// named, and the answer names no other mob. Nothing is asked of the
/// resolver about that mob for that caller. It holds for a signed-in person
/// no rule names, for a person a rule names through a group and for a
/// person whose rule lists no mob (each rule is the launch mob's), and for
/// a caller who sends no sign-in to an open console whose access rules are
/// enforced.
#[tokio::test]
async fn a_mob_the_caller_may_not_address_is_refused_like_a_mob_the_host_does_not_have() {
    let launch = runtime("launch").await;
    let second = runtime("second").await;
    let launch_mob = launch.mob_id();
    let second_mob = second.mob_id();
    let absent_mob = an_id_no_mob_has(&second_mob);
    let (app, resolver) = console_of_two_mobs(&launch, &second);
    let open_state = ConsoleJsonState {
        decisions: RuntimeDecisionState::local_console(
            crate::ConsolePolicy {
                require_app_auth: false,
                ..Default::default()
            },
            None,
        ),
        ..door_rows_state(&launch, &second_mob)
    };
    let (open_app, open_resolver) = console_of_two_mobs_over(open_state, &launch, &second);

    let mut wrong = Vec::new();
    for (who, email) in [
        ("a person no rule names", Some(DOOR_ROWS_WITHOUT_RULES)),
        (
            "a person a rule names through a group",
            Some(DOOR_ROWS_IN_A_GROUP),
        ),
        ("a person whose rule lists no mob", Some(DOOR_ROWS_OPERATOR)),
        ("a caller of an open console who sends no sign-in", None),
    ] {
        let mut for_the_absent_mob = console_doors_naming(Some(&absent_mob)).into_iter();
        for (door, for_the_second_mob) in console_doors_naming(Some(&second_mob)) {
            let (_, for_the_absent_mob) = for_the_absent_mob.next().expect("the same doors");
            let (second_answer, absent_answer) = match email {
                Some(email) => (
                    ask_as(&app, for_the_second_mob, email).await,
                    ask_as(&app, for_the_absent_mob, email).await,
                ),
                None => (
                    ask(&open_app, for_the_second_mob).await,
                    ask(&open_app, for_the_absent_mob).await,
                ),
            };
            // An open console's own rules may answer before a route reads
            // its mob: what holds there is that the two answers are one.
            if email.is_some() && !is_the_mob_ids_refusal(door, &absent_answer, &absent_mob) {
                wrong.push(format!(
                    "{who}, {door}: an id no mob has is not answered the mob id's refusal: \
                     {absent_answer:?}"
                ));
            }
            if apart_from_the_id(&second_answer, &second_mob)
                != apart_from_the_id(&absent_answer, &absent_mob)
            {
                wrong.push(format!(
                    "{who}, {door}: is answered differently for the second mob and for an id no \
                     mob has: {second_answer:?} / {absent_answer:?}"
                ));
            }
            if second_answer.2.contains(&launch_mob) || absent_answer.2.contains(&launch_mob) {
                wrong.push(format!(
                    "{who}, {door}: the answer names a mob the request did not name: \
                     {second_answer:?} / {absent_answer:?}"
                ));
            }
        }
    }
    let asked_about_the_second_mob =
        asked_about(&resolver, &second_mob) + asked_about(&open_resolver, &second_mob);
    if asked_about_the_second_mob != 0 {
        wrong.push(format!(
            "the resolver was asked about a mob the caller may not address: \
             {asked_about_the_second_mob} times"
        ));
    }

    drop(app);
    drop(open_app);
    launch.shutdown().await;
    second.shutdown().await;
    assert!(
        wrong.is_empty(),
        "a mob the caller may not address is not answered as an id no mob has:\n{}",
        wrong.join("\n")
    );
}

/// A caller who may address a second mob reaches the door's own rules on a
/// method the console serves per mob and on a method of the host (which the
/// named mob selects nothing on), and is answered the mob id's refusal
/// on every other method that hands the door an addressing id, as every
/// caller is (the announcement methods and the methods over the mobs of
/// the host whose own parameter names their mob hand it none)
/// ([`a_console_method_not_listed_as_served_per_mob_refuses_a_second_mob`]).
/// Asked on the console's two request doors with every kind of method
/// ([`REQUEST_DOORS`]) by the person whose rule selects that mob, by an
/// administrator, and by the person whose allow selects the mob while a
/// deny takes every action in it away (the door's own rules refuse that
/// person what the deny covers). The twelve console routes are asked by
/// [`no_console_route_serves_a_second_mob_before_its_reads_are_that_mobs`].
/// The launch mob named by its id is served, on every door, to a person no
/// rule names as a request that names no mob is.
#[tokio::test]
async fn a_caller_who_may_address_a_second_mob_reaches_the_own_rules_of_a_method_served_per_mob() {
    let launch = runtime("launch").await;
    let second = runtime("second").await;
    let launch_mob = launch.mob_id();
    let second_mob = second.mob_id();
    let (app, _resolver) = console_of_two_mobs(&launch, &second);
    let served_per_mob = console_methods_served_per_mob();
    // A send is served per mob on the request door and not on the multipart
    // route, whose uploads are written for the launch mob's members only.
    let is_served_per_mob = |door: &str| {
        REQUEST_DOORS.iter().any(|(asked, method)| {
            *asked == door
                && served_per_mob.contains(method)
                && !(door == "console multipart" && *method == "meta/console/send")
        })
    };
    let is_of_the_host = |door: &str| {
        REQUEST_DOORS
            .iter()
            .any(|(asked, method)| *asked == door && METHODS_OF_THE_HOST.contains(method))
    };

    let mut wrong = Vec::new();
    if !REQUEST_DOORS.iter().any(|(door, _)| is_of_the_host(door)) {
        wrong.push(format!(
            "the request doors are asked a method of the host: {REQUEST_DOORS:?}"
        ));
    }
    let kinds_asked: Vec<bool> = REQUEST_DOORS
        .iter()
        .map(|(door, _)| is_served_per_mob(door))
        .collect();
    if !kinds_asked.contains(&true) || !kinds_asked.contains(&false) {
        wrong.push(format!(
            "the request doors are asked a method served per mob and one that is not: \
             {REQUEST_DOORS:?} / {kinds_asked:?}"
        ));
    }
    for (who, email) in [
        (
            "the person whose rule selects the mob",
            DOOR_ROWS_OF_THE_SECOND_MOB,
        ),
        ("an administrator", DOOR_ROWS_ADMIN),
        (
            "the person whose allow selects the mob and whose deny takes all in it away",
            DOOR_ROWS_ALLOWED_AND_DENIED,
        ),
    ] {
        let mut request_doors = 0;
        for (door, request) in console_doors_naming(Some(&second_mob)) {
            if !door.starts_with("console") {
                continue;
            }
            request_doors += 1;
            let answer = ask_as(&app, request, email).await;
            let refused = is_the_mob_ids_refusal(door, &answer, &second_mob);
            if is_served_per_mob(door) || is_of_the_host(door) {
                if refused || answer.0 == StatusCode::UNAUTHORIZED {
                    wrong.push(format!(
                        "{who}, {door}: the second mob is not answered by its door's own rules: \
                         {answer:?}"
                    ));
                }
            } else if !refused {
                wrong.push(format!(
                    "{who}, {door}: a method that is neither served per mob nor the host's is \
                     answered for the second mob: {answer:?}"
                ));
            }
        }
        assert_eq!(
            request_doors,
            REQUEST_DOORS.len(),
            "every request of the two request doors is asked"
        );
    }
    let mut naming_no_mob = console_doors_naming(None).into_iter();
    for (door, by_id) in console_doors_naming(Some(&launch_mob)) {
        let (_, no_mob) = naming_no_mob.next().expect("the same doors");
        let by_id = ask_as(&app, by_id, DOOR_ROWS_WITHOUT_RULES).await;
        let no_mob = ask_as(&app, no_mob, DOOR_ROWS_WITHOUT_RULES).await;
        if is_the_mob_ids_refusal(door, &by_id, &launch_mob) || by_id.0 != no_mob.0 {
            wrong.push(format!(
                "{door}: the launch mob by its id is not served to a person no rule names as no \
                 mob is: {by_id:?} / {no_mob:?}"
            ));
        }
    }

    drop(app);
    launch.shutdown().await;
    second.shutdown().await;
    assert!(
        wrong.is_empty(),
        "a caller who may address a mob is not answered by the methods served per mob alone:\n{}",
        wrong.join("\n")
    );
}

/// The console routes whose reads after the take are the resolved mob's,
/// by their path in the console's router: the send and its recovery. A
/// route joins this list in the change that makes its reads per mob, with
/// the column of the route table (the route guard compares the two).
const ROUTES_THAT_READ_PER_MOB: &[&str] = &["/console/send", "/console/send/recover"];

/// The path of the console route a door of `console_doors_naming` asks, as
/// the console's router mounts it.
fn route_path_of(door: &str) -> &str {
    let path = door
        .strip_prefix("GET ")
        .or_else(|| door.strip_prefix("POST "))
        .expect("a console route's door names its method");
    if path == "/console/identity/worker-1/stream" {
        "/console/identity/{identity}/stream"
    } else {
        path
    }
}

/// A console route serves a mob other than the launch mob only once what
/// it reads is that mob's. Until then it answers a request for a second
/// served mob with the mob id's refusal, whoever asks: an administrator,
/// who may address every mob, is refused like every other caller, and the
/// resolver is asked nothing about that mob. A route whose reads are per
/// mob ([`ROUTES_THAT_READ_PER_MOB`]) does not answer an administrator so;
/// its own rows ask what it answers
/// (`console_door_send_tests`). The routes that refuse are asked first, so
/// that what the resolver was asked is theirs alone.
#[tokio::test]
async fn no_console_route_serves_a_second_mob_before_its_reads_are_that_mobs() {
    let launch = runtime("launch").await;
    let second = runtime("second").await;
    let second_mob = second.mob_id();
    let (app, resolver) = console_of_two_mobs(&launch, &second);

    let mut wrong = Vec::new();
    let mut routes = 0;
    let (reading_per_mob, of_the_launch_mob_only): (Vec<_>, Vec<_>) =
        console_doors_naming(Some(&second_mob))
            .into_iter()
            .filter(|(door, _)| !door.starts_with("console"))
            .partition(|(door, _)| ROUTES_THAT_READ_PER_MOB.contains(&route_path_of(door)));
    for (door, request) in of_the_launch_mob_only {
        routes += 1;
        let answer = ask_as(&app, request, DOOR_ROWS_ADMIN).await;
        if !is_the_mob_ids_refusal(door, &answer, &second_mob) {
            wrong.push(format!("{door}: serves a second mob: {answer:?}"));
        }
    }
    let asked_about_the_second_mob = asked_about(&resolver, &second_mob);
    if asked_about_the_second_mob != 0 {
        wrong.push(format!(
            "the resolver was asked about a mob these routes do not serve: \
             {asked_about_the_second_mob} times"
        ));
    }
    let routes_that_read_per_mob = reading_per_mob.len();
    for (door, request) in reading_per_mob {
        routes += 1;
        let answer = ask_as(&app, request, DOOR_ROWS_ADMIN).await;
        if is_the_mob_ids_refusal(door, &answer, &second_mob) {
            wrong.push(format!(
                "{door}: reads per mob and refuses a second mob: {answer:?}"
            ));
        }
    }
    assert_eq!(routes, 12, "the twelve console routes are asked");
    assert_eq!(
        routes_that_read_per_mob,
        ROUTES_THAT_READ_PER_MOB.len(),
        "every route listed as reading per mob is asked"
    );

    drop(app);
    launch.shutdown().await;
    second.shutdown().await;
    assert!(
        wrong.is_empty(),
        "a console route whose reads are not per mob yet does not refuse a second mob:\n{}",
        wrong.join("\n")
    );
}

/// The console methods served from the launch mob's own wiring (the voice
/// methods and the voice channel methods) answer for the launch mob alone:
/// a request of one that names a second served mob is answered the mob id's
/// refusal, whoever asks, and a request that names no mob or the launch mob
/// by its id is answered by the method.
#[tokio::test]
async fn a_console_method_served_from_the_launch_mobs_wiring_refuses_a_second_mob() {
    let launch = runtime("launch").await;
    let second = runtime("second").await;
    let launch_mob = launch.mob_id();
    let second_mob = second.mob_id();
    let (app, _resolver) = console_of_two_mobs(&launch, &second);
    let request = |method: &str, mob_id: Option<&str>| {
        let params = mob_id.map_or_else(|| json!({}), |mob_id| json!({"mob_id": mob_id}));
        axum::http::Request::builder()
            .method("POST")
            .uri("/console/rpc")
            .header(header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(
                json!({"jsonrpc": "2.0", "id": "row", "method": method, "params": params})
                    .to_string(),
            ))
            .expect("console request")
    };

    let mut wrong = Vec::new();
    for method in [
        crate::console_voice::VOICE_OPEN_METHOD,
        crate::console_voice::VOICE_READINESS_METHOD,
        crate::console_voice::VOICE_CLOSE_METHOD,
        crate::console_voice::VOICE_ANSWER_RECEIVED_METHOD,
        crate::console_voice::VOICE_REPLACEMENT_METHOD,
        crate::console_voice::VOICE_ACTIVITY_METHOD,
        crate::console_voice::VOICE_CONTEXT_STATUS_METHOD,
        crate::console_voice::VOICE_CAPTIONS_METHOD,
        "meta/live/playback_owner/register",
        "meta/live/playback_owner/revoke",
        "meta/live/status",
        "meta/live/close",
        "meta/live/refresh",
        "meta/live/interrupt",
        "live/webrtc/answer",
    ] {
        let for_the_second_mob =
            ask_as(&app, request(method, Some(&second_mob)), DOOR_ROWS_ADMIN).await;
        if !is_the_mob_ids_refusal("console RPC", &for_the_second_mob, &second_mob) {
            wrong.push(format!(
                "{method}: is answered for a second mob: {for_the_second_mob:?}"
            ));
        }
        for mob_id in [None, Some(launch_mob.as_str())] {
            let for_the_launch_mob = ask_as(&app, request(method, mob_id), DOOR_ROWS_ADMIN).await;
            if is_the_mob_ids_refusal("console RPC", &for_the_launch_mob, &launch_mob) {
                wrong.push(format!(
                    "{method}: the launch mob is refused for its id: {for_the_launch_mob:?}"
                ));
            }
        }
    }

    drop(app);
    launch.shutdown().await;
    second.shutdown().await;
    assert!(
        wrong.is_empty(),
        "a console method served from the launch mob's wiring answers for another mob:\n{}",
        wrong.join("\n")
    );
}

/// The console's list of the methods served from the launch mob's own
/// wiring names the eight voice methods and nothing else, and the door
/// takes them, and the voice channel methods the same handler answers, for
/// the launch mob only; no other method is taken that way.
#[test]
fn the_methods_of_the_launch_mobs_wiring_are_the_voice_methods() {
    let mut voice = [
        crate::console_voice::VOICE_OPEN_METHOD,
        crate::console_voice::VOICE_READINESS_METHOD,
        crate::console_voice::VOICE_CLOSE_METHOD,
        crate::console_voice::VOICE_ANSWER_RECEIVED_METHOD,
        crate::console_voice::VOICE_REPLACEMENT_METHOD,
        crate::console_voice::VOICE_ACTIVITY_METHOD,
        crate::console_voice::VOICE_CONTEXT_STATUS_METHOD,
        crate::console_voice::VOICE_CAPTIONS_METHOD,
    ];
    voice.sort_unstable();
    assert_eq!(CONSOLE_METHODS_OF_THE_LAUNCH_MOBS_WIRING, voice);
    for method in voice
        .into_iter()
        .chain(["meta/live/status", "live/webrtc/answer"])
    {
        assert!(
            console_method_is_of_the_launch_mobs_wiring(method),
            "{method}"
        );
    }
    for method in ["meta/list_members", "meta/console/send", "meta/status"] {
        assert!(
            !console_method_is_of_the_launch_mobs_wiring(method),
            "{method}"
        );
    }
}

/// A console mounted without the host's resolver knows the one mob its
/// state was built over. On every door, every caller is answered the mob
/// id's refusal for every other id, the id of a mob the caller may address
/// included, and the door's own mob named by its id is served as a request
/// that names no mob is.
#[tokio::test]
async fn a_console_without_the_resolver_serves_its_own_mob_alone_to_every_caller() {
    let launch = runtime("launch").await;
    let second = runtime("second").await;
    let launch_mob = launch.mob_id();
    let second_mob = second.mob_id();
    let app = console_json_router_with_state(door_rows_state(&launch, &second_mob));

    let mut wrong = Vec::new();
    for email in [
        DOOR_ROWS_ADMIN,
        DOOR_ROWS_OF_THE_SECOND_MOB,
        DOOR_ROWS_ALLOWED_AND_DENIED,
        DOOR_ROWS_WITHOUT_RULES,
    ] {
        for (door, request) in console_doors_naming(Some(&second_mob)) {
            let answer = ask_as(&app, request, email).await;
            if !is_the_mob_ids_refusal(door, &answer, &second_mob) {
                wrong.push(format!(
                    "{email}, {door}: another id is not answered the mob id's refusal: {answer:?}"
                ));
            }
        }
        let mut naming_no_mob = console_doors_naming(None).into_iter();
        for (door, by_id) in console_doors_naming(Some(&launch_mob)) {
            let (_, no_mob) = naming_no_mob.next().expect("the same doors");
            let by_id = ask_as(&app, by_id, email).await;
            let no_mob = ask_as(&app, no_mob, email).await;
            if is_the_mob_ids_refusal(door, &by_id, &launch_mob) || by_id.0 != no_mob.0 {
                wrong.push(format!(
                    "{email}, {door}: the door's own mob by its id is not served as no mob is: \
                     {by_id:?} / {no_mob:?}"
                ));
            }
        }
    }

    drop(app);
    launch.shutdown().await;
    second.shutdown().await;
    assert!(
        wrong.is_empty(),
        "a console without the resolver does not serve its own mob alone:\n{}",
        wrong.join("\n")
    );
}

/// One stored frame of the agent `alice`, of the mob `mob_id` (none for the
/// launch mob's), whose text is `text`.
fn timeline_rows_frame(
    text: &str,
    mob_id: Option<&str>,
) -> crate::console_aggregator::NewConsoleFrame {
    crate::console_aggregator::NewConsoleFrame {
        mob_id: mob_id.map(meerkat_mob::MobId::from),
        id: None,
        dedupe_key: text.to_string(),
        timestamp_ms: 1,
        runtime_key: "timeline-rows".to_string(),
        identity: "alice".to_string(),
        conversation_id: Some("alice".to_string()),
        session_id: Some("alice-session".to_string()),
        kind: "text_complete".to_string(),
        status: crate::console_aggregator::ConsoleFrameStatus::Completed,
        payload: json!({ "text": text }),
        source: crate::console_aggregator::ConsoleFrameSource {
            member_provenance: None,
            kind: ConsoleFrameSourceKind::SessionHistory,
            source_cursor: None,
        },
        source_event_id: None,
        interaction_id: None,
        turn_id: None,
        run_id: None,
        parent_frame_id: None,
        caused_by_frame_id: None,
    }
}

/// A timeline request reads the frames of the mob it is for. Two mobs each
/// have an agent `alice` with one stored frame. A request that names no
/// mob, or the launch mob by its id, reads the launch mob's frame and not
/// the other's, on the request door (`meta/console/query_timeline`) and on
/// `GET /console/timeline`. A request that names the second mob reads the
/// second mob's frame and not the launch mob's once its door serves another
/// mob: the request door when the console lists the method as served per
/// mob, the route when its reads are per mob
/// ([`ROUTES_THAT_READ_PER_MOB`]). Until then each refuses it.
#[tokio::test]
async fn a_timeline_request_reads_the_frames_of_the_mob_it_names() {
    let served_per_mob = console_methods_served_per_mob();
    let launch = runtime("launch").await;
    let second = runtime("second").await;
    let launch_mob = launch.mob_id();
    let second_mob = second.mob_id();
    let store = crate::console_aggregator::InMemoryConsoleLogStore::new();
    for frame in [
        timeline_rows_frame("frame-of-the-launch-mob", None),
        timeline_rows_frame("frame-of-the-second-mob", Some(&second_mob)),
    ] {
        store
            .append_if_absent(frame)
            .await
            .expect("the frame is stored");
    }
    let mut state = door_rows_state(&launch, &second_mob);
    state.console_aggregator = Some(MetaConsoleAggregator::new(Arc::new(store)));
    let (app, _resolver) = console_of_two_mobs_over(state, &launch, &second);
    let through_the_request_door = |mob_id: Option<&str>| {
        let mut params = json!({"identity": "alice"});
        if let Some(mob_id) = mob_id {
            params["mob_id"] = json!(mob_id);
        }
        axum::http::Request::builder()
            .method("POST")
            .uri("/console/rpc")
            .header(header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(
                json!({
                    "jsonrpc": "2.0",
                    "id": "row",
                    "method": "meta/console/query_timeline",
                    "params": params,
                })
                .to_string(),
            ))
            .expect("console request")
    };
    let through_the_route = |mob_id: Option<&str>| {
        let mob = mob_id.map_or_else(String::new, |mob_id| format!("&mob_id={mob_id}"));
        axum::http::Request::builder()
            .method("GET")
            .uri(format!("/console/timeline?identity=alice{mob}"))
            .body(axum::body::Body::empty())
            .expect("GET request")
    };

    let mut wrong = Vec::new();
    for (named, mob_id, read, not_read) in [
        (
            "no mob",
            None,
            "frame-of-the-launch-mob",
            "frame-of-the-second-mob",
        ),
        (
            "the launch mob",
            Some(launch_mob.as_str()),
            "frame-of-the-launch-mob",
            "frame-of-the-second-mob",
        ),
        (
            "the second mob",
            Some(second_mob.as_str()),
            "frame-of-the-second-mob",
            "frame-of-the-launch-mob",
        ),
    ] {
        for (door, request) in [
            (
                "meta/console/query_timeline",
                through_the_request_door(mob_id),
            ),
            ("GET /console/timeline", through_the_route(mob_id)),
        ] {
            let answer = ask_as(&app, request, DOOR_ROWS_ADMIN).await;
            let (reads_per_mob, refusal_of) = if door == "GET /console/timeline" {
                (
                    ROUTES_THAT_READ_PER_MOB.contains(&"/console/timeline"),
                    door,
                )
            } else {
                (served_per_mob.contains(&door), "console RPC")
            };
            if mob_id == Some(second_mob.as_str()) && !reads_per_mob {
                if !is_the_mob_ids_refusal(refusal_of, &answer, &second_mob) {
                    wrong.push(format!(
                        "{door}, a request that names {named}: is not refused: {answer:?}"
                    ));
                }
                continue;
            }
            if answer.0 != StatusCode::OK || !answer.2.contains(read) || answer.2.contains(not_read)
            {
                wrong.push(format!(
                    "{door}, a request that names {named}: does not read {read} alone: {answer:?}"
                ));
            }
        }
    }

    drop(app);
    launch.shutdown().await;
    second.shutdown().await;
    assert!(
        wrong.is_empty(),
        "a timeline request does not read the frames of the mob it names:\n{}",
        wrong.join("\n")
    );
}

/// A timeline request that names no agent reads the frames of the mob it
/// is for, and no frame of another mob, even for a caller who may read
/// both. Two mobs each have one stored frame. A request that names no mob,
/// or the launch mob by its id, reads the launch mob's frame alone, on the
/// request door (`meta/console/query_timeline`) and on
/// `GET /console/timeline`. A request that names the second mob reads the
/// second mob's frame alone where its door serves another mob, and is
/// refused where it does not.
#[tokio::test]
async fn a_timeline_request_that_names_no_agent_reads_the_frames_of_its_mob_alone() {
    let served_per_mob = console_methods_served_per_mob();
    let launch = runtime("launch").await;
    let second = runtime("second").await;
    let launch_mob = launch.mob_id();
    let second_mob = second.mob_id();
    let store = crate::console_aggregator::InMemoryConsoleLogStore::new();
    for frame in [
        timeline_rows_frame("frame-of-the-launch-mob", None),
        timeline_rows_frame("frame-of-the-second-mob", Some(&second_mob)),
    ] {
        store
            .append_if_absent(frame)
            .await
            .expect("the frame is stored");
    }
    let mut state = door_rows_state(&launch, &second_mob);
    state.console_aggregator = Some(MetaConsoleAggregator::new(Arc::new(store)));
    let (app, _resolver) = console_of_two_mobs_over(state, &launch, &second);
    let through_the_request_door =
        |mob_id: Option<&str>| rpc_request("meta/console/query_timeline", &naming_mob(mob_id));
    let through_the_route = |mob_id: Option<&str>| {
        let mob = mob_id.map_or_else(String::new, |mob_id| format!("?mob_id={mob_id}"));
        axum::http::Request::builder()
            .method("GET")
            .uri(format!("/console/timeline{mob}"))
            .body(axum::body::Body::empty())
            .expect("GET request")
    };

    let mut wrong = Vec::new();
    for (named, mob_id, read, not_read) in [
        (
            "no mob",
            None,
            "frame-of-the-launch-mob",
            "frame-of-the-second-mob",
        ),
        (
            "the launch mob",
            Some(launch_mob.as_str()),
            "frame-of-the-launch-mob",
            "frame-of-the-second-mob",
        ),
        (
            "the second mob",
            Some(second_mob.as_str()),
            "frame-of-the-second-mob",
            "frame-of-the-launch-mob",
        ),
    ] {
        for (door, request) in [
            (
                "meta/console/query_timeline",
                through_the_request_door(mob_id),
            ),
            ("GET /console/timeline", through_the_route(mob_id)),
        ] {
            let answer = ask_as(&app, request, DOOR_ROWS_ADMIN).await;
            let (reads_per_mob, refusal_of) = if door == "GET /console/timeline" {
                (
                    ROUTES_THAT_READ_PER_MOB.contains(&"/console/timeline"),
                    door,
                )
            } else {
                (served_per_mob.contains(&door), "console RPC")
            };
            if mob_id == Some(second_mob.as_str()) && !reads_per_mob {
                if !is_the_mob_ids_refusal(refusal_of, &answer, &second_mob) {
                    wrong.push(format!(
                        "{door}, a request that names {named}: is not refused: {answer:?}"
                    ));
                }
                continue;
            }
            if answer.0 != StatusCode::OK || !answer.2.contains(read) || answer.2.contains(not_read)
            {
                wrong.push(format!(
                    "{door}, a request that names {named}: does not read {read} alone: {answer:?}"
                ));
            }
        }
    }

    drop(app);
    launch.shutdown().await;
    second.shutdown().await;
    assert!(
        wrong.is_empty(),
        "a timeline request that names no agent reads another mob's frames:\n{}",
        wrong.join("\n")
    );
}

/// A timeline query names the mob its request is served for as a stored
/// frame does: none for the host's default mob, which is the query a
/// request built before a request could name a mob, and the mob's id for
/// another mob. A `mob_id` among the params of the request door's form is
/// not read: the door took the request's mob before the method saw them.
#[test]
fn a_timeline_query_names_the_mob_its_request_is_served_for() {
    let second = meerkat_mob::MobId::from("second");
    let of_the_route = || ConsoleTimelineHttpQuery {
        identity: Some(" alice ".to_string()),
        conversation_id: None,
        after: None,
        before: None,
        mode: None,
        limit: None,
    };
    let of_the_default_mob = timeline_query_from_http(of_the_route(), None, None);
    assert_eq!(
        of_the_default_mob,
        ConsoleTimelineWindowQuery {
            identity: Some("alice".to_string()),
            ..Default::default()
        },
        "the default mob's query is the query a request always built"
    );
    assert_eq!(
        timeline_query_from_http(of_the_route(), None, Some(&second)),
        ConsoleTimelineWindowQuery {
            mob_id: Some(second.clone()),
            ..of_the_default_mob
        }
    );

    let params = json!({"identity": "alice", "mob_id": "a-mob-the-params-carry"});
    let of_the_default_mob = timeline_query_from_rpc(&params, None).expect("a timeline query");
    assert_eq!(
        of_the_default_mob,
        ConsoleTimelineWindowQuery {
            identity: Some("alice".to_string()),
            ..Default::default()
        }
    );
    assert_eq!(
        timeline_query_from_rpc(&params, Some(&second)).expect("a timeline query"),
        ConsoleTimelineWindowQuery {
            mob_id: Some(second),
            ..of_the_default_mob
        }
    );
}

/// A mob other than the default mob is served only where the door's
/// resolver stated the host's default mob. Without that statement the
/// request is refused as one for a mob that did not resolve, by the id it
/// named, and the dispatch is never handed that mob as its host's default
/// mob. The default mob is served either way.
#[tokio::test]
async fn a_second_mob_is_not_served_where_the_hosts_default_mob_is_not_stated() {
    let launch = runtime("launch").await;
    let second = runtime("second").await;
    let launch_id = launch.mob_runtime().mob_id().clone();
    let second_id = second.mob_runtime().mob_id().clone();
    let resolver = launch
        .mob_resolver()
        .serving_a_second_mob_in_tests(&second.mob_resolver());
    let second_mob = resolver
        .resolve(Some(&second_id))
        .await
        .expect("the second mob resolves");
    let default_mob = resolver
        .resolve(None)
        .await
        .expect("the default mob resolves");

    let refused = ConsoleRequestMob::resolved(&second_mob, None)
        .err()
        .expect("a second mob is not served without the host's default mob");
    assert_eq!(refused.code, -32602);
    assert_eq!(
        refused.data,
        Some(json!({
            "kind": "foreign_mob_target",
            "requested_mob_id": second_id.as_str(),
        }))
    );

    let served = ConsoleRequestMob::resolved(&second_mob, Some(launch_id.clone()))
        .expect("a second mob is served where the host's default mob is stated");
    assert_eq!(served.served.stored.as_ref(), Some(&second_id));
    assert_eq!(
        served.served.host_default_mob(&served.runtime).mob_id(),
        &launch_id
    );
    for stated in [Some(launch_id.clone()), None] {
        let served =
            ConsoleRequestMob::resolved(&default_mob, stated).expect("the default mob is served");
        assert_eq!(served.served.stored, None);
        assert_eq!(
            served.served.host_default_mob(&served.runtime).mob_id(),
            &launch_id
        );
    }
    launch.shutdown().await;
    second.shutdown().await;
}

/// What the console's request door states to its dispatch about the mob a
/// request is served for: how a stored record names it (none for the
/// host's default mob), and the host's default mob, which is the one the
/// door's resolver serves a request that names no mob, whatever mob the
/// request is for. A door without the resolver knows one mob: that mob is
/// its host's default mob.
#[tokio::test]
async fn the_request_door_states_the_served_mob_and_its_hosts_default_mob() {
    let launch = runtime("launch").await;
    let second = runtime("second").await;
    let launch_id = launch.mob_runtime().mob_id().clone();
    let second_id = second.mob_runtime().mob_id().clone();
    let state = state_over(&launch);
    let state_runtime = state.runtime.as_ref().expect("the state holds a runtime");
    let resolver = launch
        .mob_resolver()
        .serving_a_second_mob_in_tests(&second.mob_resolver());

    let for_the_second_mob = console_request_mob(
        &state,
        state_runtime,
        Some(&resolver),
        &NamedMob::from_query(Some(second_id.to_string())),
        "meta/schedule/list",
    )
    .await
    .expect("the second mob resolves");
    assert_eq!(for_the_second_mob.runtime.mob_id(), &second_id);
    assert_eq!(for_the_second_mob.served.stored.as_ref(), Some(&second_id));
    assert_eq!(
        for_the_second_mob
            .served
            .host_default_mob(&for_the_second_mob.runtime)
            .mob_id(),
        &launch_id,
        "the host's default mob is the resolver's, not the request's"
    );

    for resolver in [Some(&resolver), None] {
        let for_no_mob = console_request_mob(
            &state,
            state_runtime,
            resolver,
            &NamedMob::default(),
            "meta/schedule/list",
        )
        .await
        .expect("a request that names no mob is served");
        assert_eq!(for_no_mob.runtime.mob_id(), &launch_id);
        assert_eq!(for_no_mob.served.stored, None);
        assert_eq!(
            for_no_mob
                .served
                .host_default_mob(&for_no_mob.runtime)
                .mob_id(),
            &launch_id
        );
    }
    launch.shutdown().await;
    second.shutdown().await;
}

/// The console methods that serve one mob, as the addressing rows list
/// them: the string literals of `CONSOLE_METHODS_THAT_SERVE_ONE_MOB`, read
/// from that file's source.
fn console_methods_that_serve_one_mob() -> Vec<&'static str> {
    let source = include_str!("rpc/mob_addressing_tests.rs");
    let from = source
        .find("const CONSOLE_METHODS_THAT_SERVE_ONE_MOB")
        .expect("the list of the console methods that serve one mob");
    let list = &source[from..];
    let list = &list[..list.find("];").expect("the list ends")];
    list.split('"').skip(1).step_by(2).collect()
}

/// The methods of that list whose answer to a request with empty params
/// names the mob it is answered for.
const METHODS_WHOSE_ANSWER_NAMES_THEIR_MOB: &[&str] = &[
    "meta/mob_events/query",
    "meta/mob_events/subscribe",
    "meta/topology/query",
    "meta/workgraph/snapshot",
];

/// Every console method that serves one mob answers for the mob its door's
/// resolver resolves. On a door whose state was captured from one runtime
/// and whose resolver is another runtime's, no method of the list names
/// the captured runtime's mob in its answer, and the methods whose answer
/// names a mob name the resolver's.
#[tokio::test]
async fn every_method_that_serves_one_mob_answers_for_the_resolvers_mob() {
    use tower::ServiceExt;
    let methods = console_methods_that_serve_one_mob();
    for method in METHODS_WHOSE_ANSWER_NAMES_THEIR_MOB {
        assert!(methods.contains(method), "{method} is on the list");
    }
    let captured = runtime("captured").await;
    let resolved = runtime("resolved").await;
    let captured_id = captured.mob_id();
    let resolved_id = resolved.mob_id();
    let app = console_json_router_with_state(state_over(&captured))
        .layer(axum::Extension(resolved.mob_resolver()));

    let mut wrong = Vec::new();
    for method in &methods {
        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/console/rpc")
            .header(header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(
                json!({"jsonrpc": "2.0", "id": "row", "method": method, "params": {}}).to_string(),
            ))
            .expect("console request");
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            app.clone().oneshot(request),
        )
        .await
        .expect("the door answers in time")
        .expect("the door answers");
        let body = axum::body::to_bytes(response.into_body(), 1 << 22)
            .await
            .expect("the answer's body");
        let answer = String::from_utf8_lossy(&body);
        if answer.contains(&captured_id) {
            wrong.push(format!(
                "{method}: the answer names the mob the door's state was captured from"
            ));
        }
        if METHODS_WHOSE_ANSWER_NAMES_THEIR_MOB.contains(method) && !answer.contains(&resolved_id) {
            wrong.push(format!(
                "{method}: the answer does not name the mob the resolver resolves"
            ));
        }
    }

    drop(app);
    captured.shutdown().await;
    resolved.shutdown().await;
    assert!(
        wrong.is_empty(),
        "a console method that serves one mob does not answer for the resolver's mob:\n{}",
        wrong.join("\n")
    );
}

/// The announcement methods that carry a mob of their own name it in their
/// own `mob_id`. The console's request door judges that id with the
/// request's caller, like the id of every other method. A mob other than
/// the launch mob that the caller may not address is answered the mob id's
/// refusal, exactly as an id no mob has, whatever the caller holds in the
/// launch mob. A caller who may address the mob is answered by the method.
/// A request that names no mob, or the launch mob by its id, is answered
/// as one answer. The resolver is asked nothing about the second mob.
#[tokio::test]
async fn an_announcement_method_that_names_its_own_mob_is_judged_with_its_caller() {
    let launch = runtime("launch").await;
    let second = runtime("second").await;
    let launch_mob = launch.mob_id();
    let second_mob = second.mob_id();
    let absent_mob = an_id_no_mob_has(&second_mob);
    let (app, resolver) = console_of_two_mobs(&launch, &second);
    let request = |method: &str, mob_id: Option<&str>| rpc_request(method, &naming_mob(mob_id));

    let mut wrong = Vec::new();
    let methods = crate::rpc::mob_addressing::ANNOUNCEMENT_METHODS_NAMING_A_MOB;
    assert_eq!(methods.len(), 11, "the family's methods that name a mob");
    for method in methods {
        for (who, email) in [
            ("a person no rule names", DOOR_ROWS_WITHOUT_RULES),
            (
                "a person who holds the methods by a rule that lists no mob",
                DOOR_ROWS_OPERATOR,
            ),
        ] {
            let second_answer = ask_as(&app, request(method, Some(&second_mob)), email).await;
            let absent_answer = ask_as(&app, request(method, Some(&absent_mob)), email).await;
            if !is_the_mob_ids_refusal("console RPC", &second_answer, &second_mob) {
                wrong.push(format!(
                    "{who}, {method}: a mob the caller may not address is not answered the mob \
                     id's refusal: {second_answer:?}"
                ));
            }
            if apart_from_the_id(&second_answer, &second_mob)
                != apart_from_the_id(&absent_answer, &absent_mob)
            {
                wrong.push(format!(
                    "{who}, {method}: is answered differently for the second mob and for an id \
                     no mob has: {second_answer:?} / {absent_answer:?}"
                ));
            }
        }
        for (who, email) in [
            ("an administrator", DOOR_ROWS_ADMIN),
            (
                "a person who holds the methods by a rule that selects the mob",
                DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB,
            ),
        ] {
            let answer = ask_as(&app, request(method, Some(&second_mob)), email).await;
            if is_the_mob_ids_refusal("console RPC", &answer, &second_mob) {
                wrong.push(format!(
                    "{who}, {method}: a mob the caller may address is not answered by the \
                     method: {answer:?}"
                ));
            }
        }
        let answer = ask_as(
            &app,
            request(method, Some(&absent_mob)),
            DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB,
        )
        .await;
        if !is_the_mob_ids_refusal("console RPC", &answer, &absent_mob) {
            wrong.push(format!(
                "{method}: a person whose rule selects the second mob alone is answered by the \
                 method for another id: {answer:?}"
            ));
        }
        let no_mob = ask_as(&app, request(method, None), DOOR_ROWS_OPERATOR).await;
        let by_id = ask_as(&app, request(method, Some(&launch_mob)), DOOR_ROWS_OPERATOR).await;
        if is_the_mob_ids_refusal("console RPC", &by_id, &launch_mob) || by_id != no_mob {
            wrong.push(format!(
                "{method}: the launch mob by its id is not answered as no mob is: {by_id:?} / \
                 {no_mob:?}"
            ));
        }
    }
    let asked_about_the_second_mob = asked_about(&resolver, &second_mob);
    if asked_about_the_second_mob != 0 {
        wrong.push(format!(
            "the resolver was asked about the second mob: {asked_about_the_second_mob} times"
        ));
    }

    drop(app);
    launch.shutdown().await;
    second.shutdown().await;
    assert!(
        wrong.is_empty(),
        "an announcement method's own mob is not judged with its caller:\n{}",
        wrong.join("\n")
    );
}

/// A console audit log in memory that holds one change row per entry of
/// `changes`: a `meta/retire_member` an administrator made, with the
/// params `changes` gives, answered with a result.
async fn audit_log_holding(changes: &[Value]) -> Arc<crate::console_audit::ConsoleAuditLog> {
    let store: Arc<dyn crate::runtime::PersistentMetadataStore> =
        Arc::new(crate::runtime::InMemoryMetadataStore::new());
    let log = crate::console_audit::ConsoleAuditLog::over(
        store,
        crate::console_audit::ConsoleAuditLimits::default(),
    )
    .expect("the store keeps realm documents");
    for params in changes {
        let actor = crate::console_audit::ConsoleAuditActor {
            kind: "person",
            principal: Some(DOOR_ROWS_ADMIN.to_string()),
            issuer: None,
            subject: None,
            assurance: None,
            login_scope_digest: None,
        };
        log.record(crate::console_audit::ConsoleAuditEntry::answered(
            actor,
            "meta/retire_member".to_string(),
            crate::console_audit::audit_target(params),
            &json!({"jsonrpc": "2.0", "id": "row", "result": {}}),
        ))
        .await
        .expect("the row is written");
    }
    Arc::new(log)
}

/// The params of a console audit read that selects every mob (`mobs`): a
/// read without the selector is answered the launch mob's rows alone,
/// whoever asks, so the rows below, which ask what a caller is shown of
/// another mob's rows, ask for every mob.
fn audit_read_of_every_mob() -> Value {
    json!({"limit": 500, "mobs": "*"})
}

/// The members the change rows of the console audit log name, as `email`
/// reads the log on `app` with every mob selected: the `member_id` of each
/// row's target. `Err` with the answer when the read is refused.
async fn audit_members_as(app: &Router, email: &str) -> Result<Vec<String>, String> {
    let answer = ask_as(
        app,
        rpc_request(crate::rpc::CONSOLE_AUDIT_METHOD, &audit_read_of_every_mob()),
        email,
    )
    .await;
    let body: Value = serde_json::from_str(&answer.2).map_err(|_| format!("{answer:?}"))?;
    let records = body["result"]["records"]
        .as_array()
        .ok_or_else(|| format!("{answer:?}"))?;
    Ok(records
        .iter()
        .filter_map(|row| row["target"]["member_id"].as_str())
        .map(str::to_string)
        .collect())
}

/// The console audit read that selects every mob shows a row that names a
/// mob other than the launch mob only to a caller who may address that mob,
/// by the judgement the doors make of a request's mob. A caller who holds
/// the audit read by
/// a rule that lists no mob reads the launch mob's rows (a row that names
/// no mob, and one that names the launch mob by its id) and no other. A
/// caller whose rule selects the second mob reads that mob's rows too, and
/// no row of an id no rule selects. A row whose mob is blank names no mob
/// and is an administrator's alone. An administrator reads every row.
#[tokio::test]
async fn the_audit_read_shows_a_row_of_another_mob_to_a_caller_who_may_address_it() {
    let launch = runtime("launch").await;
    let second = runtime("second").await;
    let launch_mob = launch.mob_id();
    let second_mob = second.mob_id();
    let absent_mob = an_id_no_mob_has(&second_mob);
    let log = audit_log_holding(&[
        json!({"member_id": "of-the-launch-mob"}),
        json!({"mob_id": second_mob, "member_id": "of-the-second-mob"}),
        json!({"mob_id": launch_mob, "member_id": "of-the-launch-mob-by-its-id"}),
        json!({"mob_id": absent_mob, "member_id": "of-an-id-no-mob-has"}),
        json!({"mob_id": " ", "member_id": "of-a-blank-mob"}),
    ])
    .await;
    let state = ConsoleJsonState {
        console_audit: Some(log),
        ..door_rows_state(&launch, &second_mob)
    };
    let (app, _resolver) = console_of_two_mobs_over(state, &launch, &second);

    let mut wrong = Vec::new();
    for (who, email, expected) in [
        (
            "a caller whose rule lists no mob",
            DOOR_ROWS_OPERATOR,
            vec!["of-the-launch-mob", "of-the-launch-mob-by-its-id"],
        ),
        (
            "a caller whose rule selects the second mob",
            DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB,
            vec![
                "of-the-launch-mob",
                "of-the-second-mob",
                "of-the-launch-mob-by-its-id",
            ],
        ),
        (
            "an administrator",
            DOOR_ROWS_ADMIN,
            vec![
                "of-the-launch-mob",
                "of-the-second-mob",
                "of-the-launch-mob-by-its-id",
                "of-an-id-no-mob-has",
                "of-a-blank-mob",
            ],
        ),
    ] {
        match audit_members_as(&app, email).await {
            Ok(read) if read == expected => {}
            Ok(read) => wrong.push(format!(
                "{who}: reads the rows of {read:?}, not of {expected:?}"
            )),
            Err(answer) => wrong.push(format!("{who}: the read is refused: {answer}")),
        }
    }
    if let Ok(read) = audit_members_as(&app, DOOR_ROWS_WITHOUT_RULES).await {
        wrong.push(format!(
            "a person no rule names reads the audit log: {read:?}"
        ));
    }

    drop(app);
    launch.shutdown().await;
    second.shutdown().await;
    assert!(
        wrong.is_empty(),
        "the audit read does not show a row of another mob by its caller's address:\n{}",
        wrong.join("\n")
    );
}

/// What [`audit_page_as`] answers of one read: the page's rows (each row's
/// `seq` and the member its target names), its `next_after_seq`, its
/// `has_more`, and the rows as the answer holds them; `Err` with the answer
/// when the read is refused.
type AuditPageRead = Result<(Vec<(u64, String)>, u64, bool, Value), String>;

/// One page of the console audit log as `email` reads it on `app` with the
/// params `params`: its rows (each row's `seq` and the member its target
/// names), its `next_after_seq`, its `has_more`, and the rows as the answer
/// holds them. `Err` with the answer when the read is refused.
async fn audit_page_as(app: &Router, email: &str, params: Value) -> AuditPageRead {
    let answer = ask_as(
        app,
        rpc_request(crate::rpc::CONSOLE_AUDIT_METHOD, &params),
        email,
    )
    .await;
    let body: Value = serde_json::from_str(&answer.2).map_err(|_| format!("{answer:?}"))?;
    let page = &body["result"];
    let records = page["records"]
        .as_array()
        .ok_or_else(|| format!("{answer:?}"))?;
    let rows = records
        .iter()
        .map(|row| {
            (
                row["seq"].as_u64().unwrap_or(u64::MAX),
                row["target"]["member_id"]
                    .as_str()
                    .unwrap_or("<no member>")
                    .to_string(),
            )
        })
        .collect();
    match (page["next_after_seq"].as_u64(), page["has_more"].as_bool()) {
        (Some(next_after_seq), Some(has_more)) => {
            Ok((rows, next_after_seq, has_more, page["records"].clone()))
        }
        _ => Err(format!("{answer:?}")),
    }
}

/// The console of the four rows below: a console of two mobs whose audit
/// log holds eight change rows. The first, fourth and eighth name a mob no
/// rule lists (an id no mob has); the second, fifth and sixth name the
/// second mob, which one rule lists for one person; the third and seventh
/// name no mob, so they are the launch mob's. A read that selects every mob
/// shows that person five rows and an administrator all eight; a read that
/// selects no mob shows every reader the two rows of the launch mob. The
/// router and the two launches.
async fn console_with_an_audit_log_of_two_mobs()
-> (Router, crate::UnifiedRuntime, crate::UnifiedRuntime) {
    let launch = runtime("launch").await;
    let second = runtime("second").await;
    let second_mob = second.mob_id();
    let other_mob = an_id_no_mob_has(&second_mob);
    let log = audit_log_holding(&[
        json!({"mob_id": other_mob, "member_id": "first-of-another-mob"}),
        json!({"mob_id": second_mob, "member_id": "second-of-the-second-mob"}),
        json!({"member_id": "third-of-the-launch-mob"}),
        json!({"mob_id": other_mob, "member_id": "fourth-of-another-mob"}),
        json!({"mob_id": second_mob, "member_id": "fifth-of-the-second-mob"}),
        json!({"mob_id": second_mob, "member_id": "sixth-of-the-second-mob"}),
        json!({"member_id": "seventh-of-the-launch-mob"}),
        json!({"mob_id": other_mob, "member_id": "eighth-of-another-mob"}),
    ])
    .await;
    let state = ConsoleJsonState {
        console_audit: Some(log),
        ..door_rows_state(&launch, &second_mob)
    };
    let (app, _resolver) = console_of_two_mobs_over(state, &launch, &second);
    (app, launch, second)
}

/// The members the rows of an audit page name, in the page's order.
fn members_of(rows: &[(u64, String)]) -> Vec<&str> {
    rows.iter().map(|(_, member)| member.as_str()).collect()
}

/// The five rows a read that selects every mob shows the person a rule
/// lists the second mob for, oldest first.
const AUDIT_ROWS_OF_THE_PERSON: [&str; 5] = [
    "second-of-the-second-mob",
    "third-of-the-launch-mob",
    "fifth-of-the-second-mob",
    "sixth-of-the-second-mob",
    "seventh-of-the-launch-mob",
];

/// A page of the console audit log is cut to its limit after the rows its
/// reader is not shown are left out. The read selects every mob: the page
/// of a person a rule lists one mob for holds the oldest rows of that mob
/// and of the launch mob after the cursor, up to the limit, whatever rows
/// of another mob lie between them.
#[tokio::test]
async fn a_console_audit_page_is_cut_after_the_rows_of_a_mob_its_reader_may_not_address_are_left_out()
 {
    let (app, launch, second) = console_with_an_audit_log_of_two_mobs().await;
    let whole = audit_page_as(&app, DOOR_ROWS_ADMIN, json!({"limit": 500, "mobs": "*"})).await;
    let shown = audit_page_as(
        &app,
        DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB,
        json!({"limit": 500, "mobs": "*"}),
    )
    .await;
    let mut pages = Vec::new();
    for limit in [1_u64, 2, 3, 5, 8] {
        pages.push((
            limit,
            audit_page_as(
                &app,
                DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB,
                json!({"limit": limit, "mobs": "*"}),
            )
            .await,
        ));
    }
    drop(app);
    launch.shutdown().await;
    second.shutdown().await;

    // The scene, read: the log holds eight rows and the person is shown
    // the five of the second mob and of the launch mob.
    let whole = whole.expect("an administrator reads the log");
    let shown = shown.expect("the person reads the log");
    println!(
        "the log holds {} rows; the person is shown {}",
        whole.0.len(),
        shown.0.len()
    );
    assert_eq!(
        members_of(&whole.0),
        [
            "first-of-another-mob",
            "second-of-the-second-mob",
            "third-of-the-launch-mob",
            "fourth-of-another-mob",
            "fifth-of-the-second-mob",
            "sixth-of-the-second-mob",
            "seventh-of-the-launch-mob",
            "eighth-of-another-mob",
        ],
        "the rows the log holds"
    );
    assert_eq!(
        members_of(&shown.0),
        AUDIT_ROWS_OF_THE_PERSON,
        "the rows the person is shown"
    );

    for (limit, page) in pages {
        let page = page.expect("the person reads a page");
        let held = usize::try_from(limit).map_or(AUDIT_ROWS_OF_THE_PERSON.len(), |limit| {
            limit.min(AUDIT_ROWS_OF_THE_PERSON.len())
        });
        let expected = &AUDIT_ROWS_OF_THE_PERSON[..held];
        assert_eq!(
            members_of(&page.0),
            expected,
            "the person's page of {limit}: {page:?}"
        );
    }
}

/// A reader who is shown every row of the console audit log is answered,
/// by a read that selects every mob, the rows after its cursor, oldest
/// first, up to the limit, each as the whole read holds it, with a cursor
/// that names the last of them and `has_more` while a row follows.
#[tokio::test]
async fn a_console_audit_page_answers_a_reader_shown_every_row_the_rows_after_its_cursor() {
    let (app, launch, second) = console_with_an_audit_log_of_two_mobs().await;
    let whole = audit_page_as(&app, DOOR_ROWS_ADMIN, json!({"limit": 500, "mobs": "*"})).await;
    let page_one = audit_page_as(&app, DOOR_ROWS_ADMIN, json!({"limit": 3, "mobs": "*"})).await;
    let after_one = page_one.as_ref().map_or(0, |page| page.1);
    let page_two = audit_page_as(
        &app,
        DOOR_ROWS_ADMIN,
        json!({"limit": 3, "after_seq": after_one, "mobs": "*"}),
    )
    .await;
    let after_two = page_two.as_ref().map_or(0, |page| page.1);
    let page_three = audit_page_as(
        &app,
        DOOR_ROWS_ADMIN,
        json!({"limit": 5, "after_seq": after_two, "mobs": "*"}),
    )
    .await;
    let pages = [page_one, page_two, page_three];
    drop(app);
    launch.shutdown().await;
    second.shutdown().await;

    let whole = whole.expect("an administrator reads the log");
    let rows = whole.3.as_array().expect("the rows of the whole read");
    assert_eq!(rows.len(), 8, "the log holds eight rows: {whole:?}");
    for (page, (from, to, has_more)) in
        pages
            .into_iter()
            .zip([(0_usize, 3_usize, true), (3, 6, true), (6, 8, false)])
    {
        let page = page.expect("an administrator reads a page");
        assert_eq!(
            page.3,
            json!(&rows[from..to]),
            "an administrator's page holds the rows {from} to {to} of the log, whole"
        );
        assert_eq!(
            (page.1, page.2),
            (whole.0[to - 1].0, has_more),
            "its cursor names its last row, and more follows while a row does: {page:?}"
        );
    }
}

/// The cursor of a page of the console audit log names a row shown to its
/// reader, and `has_more` counts the rows shown to that reader. A person a
/// rule lists one mob for reads the log one row at a time, selecting every
/// mob: every page holds one row that person is shown, every cursor names
/// such a row, and `has_more` ends with the person's last row though a row
/// of another mob follows it. A read after that row holds none, keeps its
/// cursor and has no more.
#[tokio::test]
async fn a_console_audit_cursor_names_a_row_shown_to_its_reader_and_has_more_counts_such_rows() {
    let (app, launch, second) = console_with_an_audit_log_of_two_mobs().await;
    let whole = audit_page_as(&app, DOOR_ROWS_ADMIN, json!({"limit": 500, "mobs": "*"})).await;
    let person = DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB;
    let mut read = Vec::new();
    let mut after: Option<u64> = None;
    // Six reads: the person's five rows, and one read after the last.
    for _ in 0..6 {
        let params = match after {
            Some(after) => json!({"limit": 1, "after_seq": after, "mobs": "*"}),
            None => json!({"limit": 1, "mobs": "*"}),
        };
        let page = audit_page_as(&app, person, params).await;
        let read_on = page.is_ok();
        if let Ok(page) = &page {
            after = Some(page.1);
        }
        read.push(page);
        if !read_on {
            break;
        }
    }
    drop(app);
    launch.shutdown().await;
    second.shutdown().await;

    let whole = whole.expect("an administrator reads the log");
    let read: Vec<_> = read
        .into_iter()
        .map(|page| page.expect("the person reads a page"))
        .collect();
    let seq_of = |member: &str| {
        whole
            .0
            .iter()
            .find(|(_, named)| named == member)
            .map(|(seq, _)| *seq)
            .unwrap_or_else(|| panic!("the log holds a row of {member}: {whole:?}"))
    };
    let pages: Vec<(Vec<&str>, u64, bool)> = read
        .iter()
        .map(|page| (members_of(&page.0), page.1, page.2))
        .collect();
    println!(
        "the log holds {} rows; the person's six reads: {pages:?}",
        whole.0.len()
    );
    let mut expected: Vec<(Vec<&str>, u64, bool)> = AUDIT_ROWS_OF_THE_PERSON
        .iter()
        .enumerate()
        .map(|(at, member)| {
            (
                vec![*member],
                seq_of(member),
                at + 1 < AUDIT_ROWS_OF_THE_PERSON.len(),
            )
        })
        .collect();
    expected.push((Vec::new(), seq_of("seventh-of-the-launch-mob"), false));
    assert_eq!(
        pages, expected,
        "one row the person is shown a page, each cursor a row the person was shown"
    );
}

/// A read of the console audit log that selects no mob answers the launch
/// mob's rows alone, whoever asks, and its pages are cut after the other
/// rows are left out. A person a rule lists the second mob for, and an
/// administrator, are each answered the two rows of the launch mob: one a
/// page with a cursor that names it, and both in a page of five.
#[tokio::test]
async fn a_console_audit_read_that_selects_no_mob_answers_pages_of_the_launch_mobs_rows() {
    let (app, launch, second) = console_with_an_audit_log_of_two_mobs().await;
    let whole = audit_page_as(&app, DOOR_ROWS_ADMIN, json!({"limit": 500, "mobs": "*"})).await;
    let mut read = Vec::new();
    for reader in [DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB, DOOR_ROWS_ADMIN] {
        let first = audit_page_as(&app, reader, json!({"limit": 1})).await;
        let after_first = first.as_ref().map_or(0, |page| page.1);
        let next = audit_page_as(&app, reader, json!({"limit": 1, "after_seq": after_first})).await;
        let both = audit_page_as(&app, reader, json!({"limit": 5})).await;
        read.push((reader, [first, next, both]));
    }
    drop(app);
    launch.shutdown().await;
    second.shutdown().await;

    let whole = whole.expect("an administrator reads the log");
    let seq_of = |member: &str| {
        whole
            .0
            .iter()
            .find(|(_, named)| named == member)
            .map(|(seq, _)| *seq)
            .unwrap_or_else(|| panic!("the log holds a row of {member}: {whole:?}"))
    };
    let (third, seventh) = (
        seq_of("third-of-the-launch-mob"),
        seq_of("seventh-of-the-launch-mob"),
    );
    for (reader, pages) in read {
        let pages: Vec<(Vec<String>, u64, bool)> = pages
            .into_iter()
            .map(|page| {
                let page = page.expect("the reader reads a page");
                (
                    page.0.into_iter().map(|(_, member)| member).collect(),
                    page.1,
                    page.2,
                )
            })
            .collect();
        println!("{reader}, selecting no mob: {pages:?}");
        assert_eq!(
            pages,
            [
                (vec!["third-of-the-launch-mob".to_string()], third, true),
                (
                    vec!["seventh-of-the-launch-mob".to_string()],
                    seventh,
                    false
                ),
                (
                    vec![
                        "third-of-the-launch-mob".to_string(),
                        "seventh-of-the-launch-mob".to_string(),
                    ],
                    seventh,
                    false
                ),
            ],
            "{reader}: a read that selects no mob"
        );
    }
}

/// On a console of one mob, a page of the audit log holds the rows shown to
/// its reader, whatever rows name a member hidden from that reader. Three
/// of six rows name a member private to another person: the first, the
/// third and the last. A reader who is not that person, selecting no mob,
/// is answered full pages of the three other rows; the cursor names the
/// last row of the page; `has_more` ends with the reader's last row though
/// a row follows it. A read after a cursor that names a row the reader is
/// not shown answers the reader's rows after that row: each once, none
/// before it. A read after the reader's last row holds none, keeps its
/// cursor and has no more.
#[tokio::test]
async fn a_console_audit_page_of_one_mob_holds_the_rows_shown_to_its_reader() {
    let launch = runtime("launch").await;
    let log = audit_log_holding(&[
        json!({"member_id": "kept-first"}),
        json!({"member_id": "desk-second"}),
        json!({"member_id": "kept-third"}),
        json!({"member_id": "desk-fourth"}),
        json!({"member_id": "desk-fifth"}),
        json!({"member_id": "kept-sixth"}),
    ])
    .await;
    let state = ConsoleJsonState {
        console_audit: Some(log),
        ..door_rows_state(&launch, "a-mob-no-rule-is-asked-of")
    };
    let controller = state.access.clone().expect("the fixture has access rules");
    for (identity, private) in [
        ("kept-first", true),
        ("desk-second", false),
        ("kept-third", true),
        ("desk-fourth", false),
        ("desk-fifth", false),
        ("kept-sixth", true),
    ] {
        controller.record_agent_attributes(crate::access::AgentResourceAttributes {
            identity: identity.to_string(),
            agent_id: None,
            role: None,
            labels: if private {
                std::collections::BTreeMap::from([(
                    crate::access::PRIVATE_TO_LABEL.to_string(),
                    "another-person".to_string(),
                )])
            } else {
                std::collections::BTreeMap::new()
            },
        });
    }
    let app = console_json_router_with_state(state).layer(axum::Extension(launch.mob_resolver()));
    let reader = DOOR_ROWS_OPERATOR;
    let whole = audit_page_as(&app, DOOR_ROWS_ADMIN, json!({"limit": 500})).await;
    let shown = audit_page_as(&app, reader, json!({"limit": 500})).await;
    let mut pages = Vec::new();
    for limit in [1_u64, 2, 3, 5] {
        pages.push(audit_page_as(&app, reader, json!({"limit": limit})).await);
    }
    // The cursors a reader may hold: the first, the third and the last row
    // of the log, which it is not shown, and the fifth, its own last row.
    let seqs: Vec<u64> = whole
        .as_ref()
        .map(|whole| whole.0.iter().map(|(seq, _)| *seq).collect())
        .unwrap_or_default();
    let mut after = Vec::new();
    for at in [0_usize, 2, 4, 5] {
        let cursor = seqs.get(at).copied().unwrap_or(u64::MAX);
        after.push(audit_page_as(&app, reader, json!({"limit": 5, "after_seq": cursor})).await);
    }
    drop(app);
    launch.shutdown().await;

    // The scene, read: the log holds six rows and the reader is shown the
    // three that name no member private to another person.
    let whole = whole.expect("an administrator reads the log");
    let shown = shown.expect("the reader reads the log");
    println!(
        "the log holds {} rows; the reader is shown {}",
        whole.0.len(),
        shown.0.len()
    );
    assert_eq!(
        members_of(&whole.0),
        [
            "kept-first",
            "desk-second",
            "kept-third",
            "desk-fourth",
            "desk-fifth",
            "kept-sixth",
        ],
        "the rows the log holds"
    );
    assert_eq!(
        members_of(&shown.0),
        ["desk-second", "desk-fourth", "desk-fifth"],
        "the rows the reader is shown"
    );
    let (second, fourth, fifth, sixth) = (seqs[1], seqs[3], seqs[4], seqs[5]);
    let read = |pages: Vec<AuditPageRead>| {
        pages
            .into_iter()
            .map(|page| {
                let page = page.expect("the reader reads a page");
                (
                    page.0
                        .into_iter()
                        .map(|(_, member)| member)
                        .collect::<Vec<_>>(),
                    page.1,
                    page.2,
                )
            })
            .collect::<Vec<_>>()
    };
    let row = |member: &str| member.to_string();
    let pages = read(pages);
    println!("the reader's pages of 1, 2, 3 and 5: {pages:?}");
    assert_eq!(
        pages,
        [
            (vec![row("desk-second")], second, true),
            (vec![row("desk-second"), row("desk-fourth")], fourth, true),
            (
                vec![row("desk-second"), row("desk-fourth"), row("desk-fifth")],
                fifth,
                false
            ),
            (
                vec![row("desk-second"), row("desk-fourth"), row("desk-fifth")],
                fifth,
                false
            ),
        ],
        "full pages of the rows shown to the reader, each cursor the page's last row"
    );
    let after = read(after);
    println!(
        "the reader's reads after the first, the third, the fifth and the last row: {after:?}"
    );
    assert_eq!(
        after,
        [
            (
                vec![row("desk-second"), row("desk-fourth"), row("desk-fifth")],
                fifth,
                false
            ),
            (vec![row("desk-fourth"), row("desk-fifth")], fifth, false),
            (Vec::new(), fifth, false),
            (Vec::new(), sixth, false),
        ],
        "a read after a row: the reader's rows after it, each once; none after its last row, \
         with the cursor kept"
    );
}

/// The console audit read judges the member a row names in the row's mob.
/// Two mobs each have agents `alice` and `bob`: the second mob's `alice`
/// and the launch mob's `bob` are private to another person. A caller who
/// may address both mobs and is not that person reads, with every mob
/// selected, the rows of the
/// launch mob's `alice` and of the second mob's `bob`, and neither row of a
/// private agent. An administrator reads all four.
#[tokio::test]
async fn the_audit_read_judges_a_rows_member_in_the_rows_mob() {
    let launch = runtime("launch").await;
    let second = runtime("second").await;
    let second_mob = second.mob_id();
    let second_id = second.mob_runtime().mob_id().clone();
    let log = audit_log_holding(&[
        json!({"member_id": "alice"}),
        json!({"mob_id": second_mob, "member_id": "alice"}),
        json!({"member_id": "bob"}),
        json!({"mob_id": second_mob, "member_id": "bob"}),
    ])
    .await;
    let state = ConsoleJsonState {
        console_audit: Some(log),
        ..door_rows_state(&launch, &second_mob)
    };
    let controller = state.access.clone().expect("the fixture has access rules");
    let agent = |identity: &str, private: bool| crate::access::AgentResourceAttributes {
        identity: identity.to_string(),
        agent_id: None,
        role: None,
        labels: if private {
            std::collections::BTreeMap::from([(
                crate::access::PRIVATE_TO_LABEL.to_string(),
                "another-person".to_string(),
            )])
        } else {
            std::collections::BTreeMap::new()
        },
    };
    controller.record_agent_attributes(agent("alice", false));
    controller.record_agent_attributes(agent("bob", true));
    controller.record_agent_attributes_in(&second_id, agent("alice", true));
    controller.record_agent_attributes_in(&second_id, agent("bob", false));
    let (app, _resolver) = console_of_two_mobs_over(state, &launch, &second);
    // The rows a read holds, each as the mob its target names (none for the
    // launch mob) and its member.
    let rows_as = |email: &'static str| {
        let app = app.clone();
        let second_mob = second_mob.clone();
        async move {
            let answer = ask_as(
                &app,
                rpc_request(crate::rpc::CONSOLE_AUDIT_METHOD, &audit_read_of_every_mob()),
                email,
            )
            .await;
            let body: Value = serde_json::from_str(&answer.2).unwrap_or(Value::Null);
            body["result"]["records"]
                .as_array()
                .map(|records| {
                    records
                        .iter()
                        .map(|row| {
                            let of_the_second_mob =
                                row["target"]["mob_id"] == json!(second_mob.as_str());
                            let member = row["target"]["member_id"].as_str().unwrap_or_default();
                            format!(
                                "{}'s {member}",
                                if of_the_second_mob {
                                    "the second mob"
                                } else {
                                    "the launch mob"
                                }
                            )
                        })
                        .collect::<Vec<_>>()
                })
                .ok_or(answer)
        }
    };

    let mut wrong = Vec::new();
    for (who, email, expected) in [
        (
            "a caller who may address both mobs",
            DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB,
            vec!["the launch mob's alice", "the second mob's bob"],
        ),
        (
            "an administrator",
            DOOR_ROWS_ADMIN,
            vec![
                "the launch mob's alice",
                "the second mob's alice",
                "the launch mob's bob",
                "the second mob's bob",
            ],
        ),
    ] {
        match rows_as(email).await {
            Ok(read) if read == expected => {}
            Ok(read) => wrong.push(format!("{who}: reads {read:?}, not {expected:?}")),
            Err(answer) => wrong.push(format!("{who}: the read is refused: {answer:?}")),
        }
    }

    drop(app);
    launch.shutdown().await;
    second.shutdown().await;
    assert!(
        wrong.is_empty(),
        "the audit read does not judge a row's member in the row's mob:\n{}",
        wrong.join("\n")
    );
}

/// The label every member of the rows below carries.
const TOLD_APART_LABEL: (&str, &str) = ("door-rows", "told-apart");
/// The one member of the launch mob in the rows below.
const ONLY_IN_THE_LAUNCH_MOB: &str = "only-in-launch";
/// The one member of the second mob in the rows below.
const ONLY_IN_THE_SECOND_MOB: &str = "only-in-second";

/// A runtime over one mob whose id starts with `tag`, with one member
/// `member` that carries [`TOLD_APART_LABEL`], and the state directory the
/// runtime keeps (a member needs a profile with comms, and comms a state
/// directory).
async fn runtime_with_member(
    tag: &str,
    member: &str,
) -> (tempfile::TempDir, crate::UnifiedRuntime) {
    runtime_with_labelled_member(
        tag,
        member,
        std::collections::BTreeMap::from([(
            TOLD_APART_LABEL.0.to_string(),
            TOLD_APART_LABEL.1.to_string(),
        )]),
    )
    .await
}

/// [`runtime_with_member`] with the member's labels given.
async fn runtime_with_labelled_member(
    tag: &str,
    member: &str,
    labels: std::collections::BTreeMap<String, String>,
) -> (tempfile::TempDir, crate::UnifiedRuntime) {
    let state_dir = tempfile::tempdir().expect("a state directory");
    let definition = meerkat_mob::MobDefinition::from_toml(&format!(
        "[mob]\nid = \"console-door-{tag}-{}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n\
         external_addressable = true\nruntime_mode = \"turn_driven\"\n\n\
         [profiles.worker.tools]\ncomms = true\n",
        uuid::Uuid::new_v4().simple()
    ))
    .expect("mob definition");
    let runtime = Box::pin(
        crate::UnifiedRuntime::builder()
            .definition(definition)
            .persistent_state(state_dir.path())
            .comms(true)
            .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
            .build(),
    )
    .await
    .expect("runtime builds");
    runtime
        .mob_handle()
        .spawn_spec(
            SpawnMemberSpec::from_wire("worker".to_string(), member.to_string(), None, None, None)
                .with_labels(labels),
        )
        .await
        .expect("the member is spawned");
    (state_dir, runtime)
}

/// The mob an answer is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnsweredFor {
    TheLaunchMob,
    TheSecondMob,
}

/// The methods served per mob that the row below tells the two mobs apart
/// on itself, each with the params it asks with (beside the mob).
const TOLD_APART_HERE: &[(&str, &str)] = &[
    (
        "meta/find_members",
        r#"{"label_key":"door-rows","label_value":"told-apart"}"#,
    ),
    ("meta/get_member", r#"{"member_id":"only-in-second"}"#),
    ("meta/list_members", "{}"),
    ("meta/mob_events/query", "{}"),
    ("meta/mob_events/subscribe", "{}"),
    ("meta/topology/query", "{}"),
    ("meta/workgraph/snapshot", "{}"),
];

/// The methods served per mob that another row tells the two mobs apart
/// on, each with that row: the inspection of an identity (its row needs
/// a console that holds both mobs' registrations), the timeline's own
/// rows (one for a request that names an identity, one for a request
/// that names none), the send and its recovery (their row needs two mobs
/// that each have an identity plane and a console that holds both mobs'
/// registrations), the delivery history's own row, the row of the
/// family of a mob's long-term
/// agents (it needs a launch with an identity policy and a second mob the
/// mob set created), the row of the schedule door (those rows need a frame
/// log, the host's module runtime and a schedule service), the status (its
/// row needs a second mob that does not run), the stop of a member's run
/// (its row asks for a member only one of the two mobs has), and the row
/// of the topology methods.
const TOLD_APART_BY_ANOTHER_ROW: &[(&str, &str)] = &[
    (
        "meta/console/inspect_identity",
        "the_inspection_of_an_identity_is_served_for_the_mob_a_request_names",
    ),
    (
        "meta/console/query_timeline",
        "a_timeline_request_reads_the_frames_of_the_mob_it_names and \
         without_a_mobs_selector_a_timeline_read_answers_for_its_requests_mob_alone",
    ),
    (
        "meta/console/recover_send",
        "console_door_send_tests::a_hosted_mobs_send_and_its_recovery_are_that_mobs_own_on_the_request_door",
    ),
    (
        "meta/console/send",
        "console_door_send_tests::a_hosted_mobs_send_and_its_recovery_are_that_mobs_own_on_the_request_door",
    ),
    (
        "meta/delivery/history",
        "the_consoles_delivery_history_lists_the_deliveries_of_the_mob_it_names",
    ),
    (
        "meta/identity/adopt",
        "the_consoles_methods_of_the_family_tell_two_mobs_apart",
    ),
    (
        "meta/identity/archive",
        "the_consoles_methods_of_the_family_tell_two_mobs_apart",
    ),
    (
        "meta/identity/changes_hold",
        "the_consoles_methods_of_the_family_tell_two_mobs_apart",
    ),
    (
        "meta/identity/create",
        "the_consoles_methods_of_the_family_tell_two_mobs_apart",
    ),
    (
        "meta/identity/directory",
        "the_consoles_methods_of_the_family_tell_two_mobs_apart",
    ),
    (
        "meta/identity/get",
        "the_consoles_methods_of_the_family_tell_two_mobs_apart",
    ),
    (
        "meta/identity/list",
        "the_consoles_methods_of_the_family_tell_two_mobs_apart",
    ),
    (
        "meta/identity/operation",
        "the_consoles_methods_of_the_family_tell_two_mobs_apart",
    ),
    (
        "meta/identity/plan",
        "the_consoles_methods_of_the_family_tell_two_mobs_apart",
    ),
    (
        "meta/identity/remove",
        "the_consoles_methods_of_the_family_tell_two_mobs_apart",
    ),
    (
        "meta/identity/unarchive",
        "the_consoles_methods_of_the_family_tell_two_mobs_apart",
    ),
    (
        "meta/identity/update",
        "the_consoles_methods_of_the_family_tell_two_mobs_apart",
    ),
    (
        "meta/identity/withdraw",
        "the_consoles_withdraw_of_a_held_change_tells_two_mobs_apart",
    ),
    (
        "meta/schedule/create",
        "member_keyed_schedule_door_through_the_console_a_second_mobs_schedule_is_its_own",
    ),
    (
        "meta/schedule/delete",
        "member_keyed_schedule_door_through_the_console_a_second_mobs_schedule_is_its_own",
    ),
    (
        "meta/schedule/list",
        "member_keyed_schedule_door_through_the_console_a_second_mobs_schedule_is_its_own",
    ),
    (
        "meta/schedule/pause",
        "member_keyed_schedule_door_through_the_console_a_second_mobs_schedule_is_its_own",
    ),
    (
        "meta/schedule/resume",
        "member_keyed_schedule_door_through_the_console_a_second_mobs_schedule_is_its_own",
    ),
    (
        "meta/schedule/update",
        "member_keyed_schedule_door_through_the_console_a_second_mobs_schedule_is_its_own",
    ),
    ("meta/status", "the_status_of_a_second_mob_is_that_mobs_own"),
    (
        "meta/stop_member_run",
        "the_stop_of_a_run_is_served_for_the_mob_a_request_names",
    ),
    (
        "meta/topology/apply",
        "the_console_topology_methods_tell_two_mobs_apart",
    ),
    (
        "meta/topology/audit/query",
        "the_console_topology_methods_tell_two_mobs_apart",
    ),
    (
        "meta/topology/operation/get",
        "the_console_topology_methods_tell_two_mobs_apart",
    ),
    (
        "meta/topology/plan",
        "the_console_topology_methods_tell_two_mobs_apart",
    ),
];

/// Which of the two mobs the answer `answer` of `method` is for, read from
/// what only one of them holds: its one member, or its id. `None` for an
/// answer that tells neither.
fn answered_for(
    method: &str,
    answer: &DoorAnswer,
    launch_mob: &str,
    second_mob: &str,
) -> Option<AnsweredFor> {
    let body: Value = serde_json::from_str(&answer.2).ok()?;
    if method == "meta/get_member" {
        // Asked for the second mob's member: found there, and nowhere else.
        return if body["error"].is_null() && body["result"].is_object() {
            Some(AnsweredFor::TheSecondMob)
        } else if !body["error"].is_null() {
            // The launch mob has no member of that name.
            Some(AnsweredFor::TheLaunchMob)
        } else {
            None
        };
    }
    if !body["error"].is_null() {
        return None;
    }
    let (of_the_launch_mob, of_the_second_mob) =
        if matches!(method, "meta/list_members" | "meta/find_members") {
            (ONLY_IN_THE_LAUNCH_MOB, ONLY_IN_THE_SECOND_MOB)
        } else {
            (launch_mob, second_mob)
        };
    match (
        answer.2.contains(of_the_launch_mob),
        answer.2.contains(of_the_second_mob),
    ) {
        (true, false) => Some(AnsweredFor::TheLaunchMob),
        (false, true) => Some(AnsweredFor::TheSecondMob),
        _ => None,
    }
}

/// The event subscription is served for the mob a request names, as the
/// stream it hands back is. An administrator's request that names a second
/// served mob is answered with the address of that mob's stream, which
/// names the mob, like a request that names the launch mob; a request that
/// names no mob is answered with the address of the launch mob's stream;
/// and the method is on the console's list of the methods served per mob.
/// A person who may not address the second mob is answered the mob id's
/// refusal, exactly as for an id no mob has, and the resolver is asked
/// nothing about that mob for that person.
#[tokio::test]
async fn the_event_subscription_is_served_for_the_mob_a_request_names() {
    let launch = runtime("launch").await;
    let second = runtime("second").await;
    let launch_mob = launch.mob_id();
    let second_mob = second.mob_id();
    let absent_mob = an_id_no_mob_has(&second_mob);
    let (app, resolver) = console_of_two_mobs(&launch, &second);
    let method = "meta/mob_events/subscribe";
    let ask = |mob_id: Option<&str>, email: &'static str| {
        ask_as(&app, rpc_request(method, &naming_mob(mob_id)), email)
    };

    let hidden = ask(Some(&second_mob), DOOR_ROWS_WITHOUT_RULES).await;
    let absent = ask(Some(&absent_mob), DOOR_ROWS_WITHOUT_RULES).await;
    let asked_about_the_hidden_mob = asked_about(&resolver, &second_mob);
    let for_the_second_mob = ask(Some(&second_mob), DOOR_ROWS_ADMIN).await;
    let for_the_launch_mob = ask(Some(&launch_mob), DOOR_ROWS_ADMIN).await;
    let for_no_mob = ask(None, DOOR_ROWS_ADMIN).await;
    drop(app);
    launch.shutdown().await;
    second.shutdown().await;

    assert!(
        is_the_mob_ids_refusal("console RPC", &hidden, &second_mob),
        "a person who may not address the second mob is refused: {hidden:?}"
    );
    assert_eq!(
        apart_from_the_id(&hidden, &second_mob),
        apart_from_the_id(&absent, &absent_mob),
        "a mob the caller may not address is answered as an id no mob has"
    );
    assert_eq!(
        asked_about_the_hidden_mob, 0,
        "the resolver is asked nothing about the second mob for that person"
    );
    for (named, answer, names) in [
        (
            "the second mob",
            &for_the_second_mob,
            Some(second_mob.as_str()),
        ),
        (
            "the launch mob",
            &for_the_launch_mob,
            Some(launch_mob.as_str()),
        ),
        ("no mob", &for_no_mob, None),
    ] {
        let body: Value = serde_json::from_str(&answer.2).expect("a JSON answer");
        let address = body["result"]["subscribe_url"].as_str();
        assert!(
            body["error"].is_null() && address.is_some(),
            "{named}: is answered with the address of the stream: {answer:?}"
        );
        if let Some(mob_id) = names {
            assert!(
                address.is_some_and(|address| address.contains(mob_id)),
                "{named}: the address names the mob: {address:?}"
            );
        }
    }
    assert!(
        console_methods_served_per_mob().contains(&method),
        "the subscription is listed as served per mob"
    );
}

/// The inspection of an identity is served for the mob a request names. On
/// a console of two mobs that each have a member of their own and a member
/// of one name, an administrator's inspection that names the second mob
/// answers the second mob's members and no member of the launch mob: its
/// own member is found there and nowhere else, the launch mob's own member
/// is not found there, and of the two members of one name each request is
/// answered the member of the mob it names, found in that mob's own
/// registration, with that mob as the mob whose door answered. An
/// inspection that names the launch mob by its id is one that names no
/// mob. A person who may not address the second mob is answered the mob
/// id's refusal, exactly as for an id no mob has, and the resolver is asked
/// nothing about that mob for that person. The method is on the console's
/// list of the methods served per mob.
#[tokio::test]
async fn the_inspection_of_an_identity_is_served_for_the_mob_a_request_names() {
    const SHARED_NAME: &str = "reviewer";
    const LAUNCH_REGISTRATION: &str = "runtime-a";
    let (_launch_state, launch) = runtime_with_member("launch", ONLY_IN_THE_LAUNCH_MOB).await;
    let (_second_state, second) = runtime_with_member("second", ONLY_IN_THE_SECOND_MOB).await;
    for runtime in [&launch, &second] {
        runtime
            .mob_handle()
            .spawn_spec(SpawnMemberSpec::from_wire(
                "worker".to_string(),
                SHARED_NAME.to_string(),
                None,
                None,
                None,
            ))
            .await
            .expect("the member of one name is spawned");
    }
    let launch_mob = launch.mob_id();
    let second_mob = second.mob_id();
    let absent_mob = an_id_no_mob_has(&second_mob);
    // The host's console: it was told its default mob and holds the
    // registration of the launch mob's runtime and of the second mob's
    // console events, as a host registers them.
    let aggregator = MetaConsoleAggregator::new(Arc::new(
        crate::console_aggregator::InMemoryConsoleLogStore::new(),
    ));
    aggregator.tell_default_mob(crate::member_key::DefaultMob::new(
        launch.mob_handle().mob_id().clone(),
    ));
    aggregator.register_runtime_handles_with_policy(
        LAUNCH_REGISTRATION,
        "",
        launch.mob_runtime().clone(),
        None,
        launch.console_events(),
        Arc::new(crate::console_aggregator::AllowAllConsoleVisibilityPolicy),
    );
    aggregator
        .register_mob_console_events(
            second.mob_handle().mob_id(),
            second.mob_runtime().clone(),
            second.console_events(),
            Arc::new(crate::console_aggregator::AllowAllConsoleVisibilityPolicy),
        )
        .expect("the second mob's console events register");
    let state = ConsoleJsonState {
        console_aggregator: Some(aggregator),
        ..door_rows_state(&launch, second_mob.as_str())
    };
    let (app, resolver) = console_of_two_mobs_over(state, &launch, &second);
    let method = "meta/console/inspect_identity";
    let inspect = |mob_id: Option<&str>, identity: &'static str, email: &'static str| {
        let mut params = naming_mob(mob_id);
        params["identity"] = json!(identity);
        ask_as(&app, rpc_request(method, &params), email)
    };
    // What an inspection answered: the member it found, the mob whose door
    // answered and the registration the member was found in; or its error.
    let answered = |answer: &DoorAnswer| -> String {
        let body: Value = serde_json::from_str(&answer.2).unwrap_or(Value::Null);
        let record = &body["result"]["identity"];
        match (record["identity"].as_str(), record["mob_id"].as_str()) {
            (Some(identity), Some(mob_id)) if body["error"].is_null() => {
                let registration = if record["runtime_key"] == json!(LAUNCH_REGISTRATION) {
                    "the launch mob's registration"
                } else {
                    "another registration"
                };
                format!("{identity} of {mob_id} in {registration}")
            }
            _ => format!("error {}", body["error"]["code"]),
        }
    };
    let found_in = |mob: &str, identity: &str, registration: &str| {
        format!("{identity} of {mob} in {registration}")
    };

    let asked: [(Option<&str>, &'static str); 7] = [
        (Some(second_mob.as_str()), ONLY_IN_THE_SECOND_MOB),
        (None, ONLY_IN_THE_SECOND_MOB),
        (Some(second_mob.as_str()), ONLY_IN_THE_LAUNCH_MOB),
        (None, ONLY_IN_THE_LAUNCH_MOB),
        (Some(second_mob.as_str()), SHARED_NAME),
        (None, SHARED_NAME),
        (Some(launch_mob.as_str()), SHARED_NAME),
    ];
    let expected = [
        found_in(&second_mob, ONLY_IN_THE_SECOND_MOB, "another registration"),
        "error -32001".to_string(),
        "error -32001".to_string(),
        found_in(
            &launch_mob,
            ONLY_IN_THE_LAUNCH_MOB,
            "the launch mob's registration",
        ),
        found_in(&second_mob, SHARED_NAME, "another registration"),
        found_in(&launch_mob, SHARED_NAME, "the launch mob's registration"),
        found_in(&launch_mob, SHARED_NAME, "the launch mob's registration"),
    ];
    let hidden = inspect(
        Some(&second_mob),
        ONLY_IN_THE_SECOND_MOB,
        DOOR_ROWS_WITHOUT_RULES,
    )
    .await;
    let absent = inspect(
        Some(&absent_mob),
        ONLY_IN_THE_SECOND_MOB,
        DOOR_ROWS_WITHOUT_RULES,
    )
    .await;
    let asked_about_the_hidden_mob = asked_about(&resolver, &second_mob);
    // A member is found once its mob lists it, a moment after it is seated:
    // the inspections are asked until they answer as expected, or ten
    // seconds passed.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut answers = Vec::new();
    loop {
        answers.clear();
        for (mob_id, identity) in asked {
            answers.push(answered(&inspect(mob_id, identity, DOOR_ROWS_ADMIN).await));
        }
        if answers == expected || tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    drop(app);
    launch.shutdown().await;
    second.shutdown().await;

    assert!(
        is_the_mob_ids_refusal("console RPC", &hidden, &second_mob),
        "a person who may not address the second mob is refused: {hidden:?}"
    );
    assert_eq!(
        apart_from_the_id(&hidden, &second_mob),
        apart_from_the_id(&absent, &absent_mob),
        "a mob the caller may not address is answered as an id no mob has"
    );
    assert_eq!(
        asked_about_the_hidden_mob, 0,
        "the resolver is asked nothing about the second mob for that person"
    );
    assert_eq!(
        answers, expected,
        "the inspections, in order: the second mob's own member in its mob and in the launch \
         mob; the launch mob's own member in the second mob and in its own; the member of one \
         name in the second mob, in the launch mob, and in the launch mob named by its id"
    );
    assert!(
        console_methods_served_per_mob().contains(&method),
        "the inspection is listed as served per mob"
    );
}

/// A member action is held for a mob whose identity plane is owed. The
/// door that resolves a request's mob reads where that mob's identity
/// plane stands, and the stop of a member's run asks the hold before it
/// reads its member: for a mob whose plane is owed it is answered the
/// console's refusal of a part that is not available, with no typed data,
/// and for every other mob the hold says nothing.
#[test]
fn a_member_action_is_held_for_a_mob_whose_identity_plane_is_owed() {
    let owed = ServedMob {
        identity_plane_owed: true,
        ..ServedMob::default()
    };
    let held = member_action_held_for_an_owed_identity_plane(&owed)
        .map(|refusal| (refusal.code, refusal.data.is_none()));
    assert_eq!(
        held,
        Some((-32004, true)),
        "a mob whose identity plane is owed is refused as not available"
    );
    assert!(
        member_action_held_for_an_owed_identity_plane(&ServedMob::default()).is_none(),
        "a mob whose identity plane is not owed is not held"
    );
    let source = include_str!("http_console.rs");
    let function = |head: &str| {
        let text = source.find(head).map_or("", |from| &source[from..]);
        &text[..text.find("\n}\n").unwrap_or(text.len())]
    };
    assert!(
        function("async fn console_request_mob(")
            .contains("resolver.identity_plane_standing(&mob)"),
        "the door that resolves a request's mob reads where its identity plane stands"
    );
    let dispatch = function("async fn handle_console_runtime_rpc_with_visibility(");
    let arm = dispatch
        .split("\"meta/stop_member_run\" =>")
        .nth(1)
        .unwrap_or_default();
    let before_its_member = arm.split("\"member_id\"").next().unwrap_or_default();
    assert!(
        before_its_member
            .contains("member_action_held_for_an_owed_identity_plane(&governance.served_mob)"),
        "the stop of a member's run asks the hold before it reads its member"
    );
}

/// The stop of a member's run is served for the mob a request names. On a
/// console of two mobs that each have one member of their own, an
/// administrator's stop that names the second mob is answered for the
/// second mob's own member as a stop that names no mob is answered for the
/// launch mob's own member, and for the launch mob's member as a stop that
/// names no mob is answered for a member the launch mob does not have: it
/// reaches the members of the mob it names and no other's. A stop that
/// names the launch mob by its id is a stop that names no mob. The run the
/// requests name is no member's run, so nothing is stopped. A person who
/// may not address the second mob is answered the mob id's refusal,
/// exactly as for an id no mob has, and the resolver is asked nothing
/// about that mob for that person. The method is on the console's list of
/// the methods served per mob.
#[tokio::test]
async fn the_stop_of_a_run_is_served_for_the_mob_a_request_names() {
    let (_launch_state, launch) = runtime_with_member("launch", ONLY_IN_THE_LAUNCH_MOB).await;
    let (_second_state, second) = runtime_with_member("second", ONLY_IN_THE_SECOND_MOB).await;
    let launch_mob = launch.mob_id();
    let second_mob = second.mob_id();
    let absent_mob = an_id_no_mob_has(&second_mob);
    let (app, resolver) = console_of_two_mobs(&launch, &second);
    let method = "meta/stop_member_run";
    let stop = |mob_id: Option<&str>, member: &'static str, email: &'static str| {
        let mut params = naming_mob(mob_id);
        params["member_id"] = json!(member);
        params["run_id"] = json!("00000000-0000-4000-8000-000000000000");
        params["reason"] = json!("a row asks");
        ask_as(&app, rpc_request(method, &params), email)
    };

    let hidden = stop(
        Some(&second_mob),
        ONLY_IN_THE_SECOND_MOB,
        DOOR_ROWS_WITHOUT_RULES,
    )
    .await;
    let absent = stop(
        Some(&absent_mob),
        ONLY_IN_THE_SECOND_MOB,
        DOOR_ROWS_WITHOUT_RULES,
    )
    .await;
    let asked_about_the_hidden_mob = asked_about(&resolver, &second_mob);
    let of_the_second_mobs_own =
        stop(Some(&second_mob), ONLY_IN_THE_SECOND_MOB, DOOR_ROWS_ADMIN).await;
    let of_the_second_mob_for_the_launch_mobs =
        stop(Some(&second_mob), ONLY_IN_THE_LAUNCH_MOB, DOOR_ROWS_ADMIN).await;
    let of_no_mobs_own = stop(None, ONLY_IN_THE_LAUNCH_MOB, DOOR_ROWS_ADMIN).await;
    let of_no_mob_for_the_second_mobs = stop(None, ONLY_IN_THE_SECOND_MOB, DOOR_ROWS_ADMIN).await;
    let of_the_launch_mobs_own =
        stop(Some(&launch_mob), ONLY_IN_THE_LAUNCH_MOB, DOOR_ROWS_ADMIN).await;
    drop(app);
    launch.shutdown().await;
    second.shutdown().await;

    assert!(
        is_the_mob_ids_refusal("console RPC", &hidden, &second_mob),
        "a person who may not address the second mob is refused: {hidden:?}"
    );
    assert_eq!(
        apart_from_the_id(&hidden, &second_mob),
        apart_from_the_id(&absent, &absent_mob),
        "a mob the caller may not address is answered as an id no mob has"
    );
    assert_eq!(
        asked_about_the_hidden_mob, 0,
        "the resolver is asked nothing about the second mob for that person"
    );
    // An answer with the two members' names and the two mobs' ids replaced
    // by one word each: two answers that differ only in the names their
    // requests gave compare equal.
    let apart_from_the_names = |answer: &DoorAnswer| {
        answer
            .2
            .replace(ONLY_IN_THE_SECOND_MOB, "<member>")
            .replace(ONLY_IN_THE_LAUNCH_MOB, "<member>")
            .replace(second_mob.as_str(), "<mob>")
            .replace(launch_mob.as_str(), "<mob>")
    };
    let mut wrong = Vec::new();
    if is_the_mob_ids_refusal("console RPC", &of_the_second_mobs_own, &second_mob) {
        wrong.push(format!(
            "an administrator's stop that names the second mob is refused like an id no mob has: \
             {of_the_second_mobs_own:?}"
        ));
    }
    if apart_from_the_names(&of_the_second_mobs_own) != apart_from_the_names(&of_no_mobs_own) {
        wrong.push(format!(
            "a stop of the second mob's own member is not answered as a stop of the launch \
             mob's own member is: {of_the_second_mobs_own:?} / {of_no_mobs_own:?}"
        ));
    }
    if apart_from_the_names(&of_the_second_mob_for_the_launch_mobs)
        != apart_from_the_names(&of_no_mob_for_the_second_mobs)
    {
        wrong.push(format!(
            "a stop that names the second mob and the launch mob's member is not answered as a \
             stop of a member its mob does not have: {of_the_second_mob_for_the_launch_mobs:?} \
             / {of_no_mob_for_the_second_mobs:?}"
        ));
    }
    if apart_from_the_names(&of_no_mobs_own) == apart_from_the_names(&of_no_mob_for_the_second_mobs)
    {
        wrong.push(format!(
            "the fixture: a stop of a member a mob has is answered as a stop of a member it does \
             not have: {of_no_mobs_own:?}"
        ));
    }
    if of_the_launch_mobs_own.2 != of_no_mobs_own.2 {
        wrong.push(format!(
            "a stop that names the launch mob by its id is not answered as one that names no \
             mob: {of_the_launch_mobs_own:?} / {of_no_mobs_own:?}"
        ));
    }
    if !console_methods_served_per_mob().contains(&method) {
        wrong.push("the stop is not listed as served per mob".to_string());
    }
    assert!(
        wrong.is_empty(),
        "the stop of a member's run on a console of two mobs:\n{}",
        wrong.join("\n")
    );
}

/// Every console method served per mob tells two mobs apart. On a console
/// of two mobs that each have one member of their own, an administrator's
/// request that names the second mob is answered with what only the second
/// mob holds, and a request that names no mob, or the launch mob by its
/// id, with what only the launch mob holds. The console's list of the
/// methods served per mob names exactly the methods a row tells the two
/// mobs apart on: this row, or the row named beside the method.
#[tokio::test]
async fn every_method_served_per_mob_tells_two_mobs_apart() {
    let (_launch_state, launch) = runtime_with_member("launch", ONLY_IN_THE_LAUNCH_MOB).await;
    let (_second_state, second) = runtime_with_member("second", ONLY_IN_THE_SECOND_MOB).await;
    let launch_mob = launch.mob_id();
    let second_mob = second.mob_id();
    let (app, _resolver) = console_of_two_mobs(&launch, &second);

    let mut wrong = Vec::new();
    for (method, params) in TOLD_APART_HERE {
        let params: Value = serde_json::from_str(params).expect("the row's params are JSON");
        for (named, mob_id, expected) in [
            ("no mob", None, AnsweredFor::TheLaunchMob),
            (
                "the launch mob",
                Some(launch_mob.as_str()),
                AnsweredFor::TheLaunchMob,
            ),
            (
                "the second mob",
                Some(second_mob.as_str()),
                AnsweredFor::TheSecondMob,
            ),
        ] {
            let mut params = params.clone();
            if let Some(mob_id) = mob_id {
                params["mob_id"] = json!(mob_id);
            }
            let answer = ask_as(&app, rpc_request(method, &params), DOOR_ROWS_ADMIN).await;
            let told = answered_for(method, &answer, &launch_mob, &second_mob);
            if told != Some(expected) {
                wrong.push(format!(
                    "{method}, a request that names {named}: is answered for {told:?}, not for \
                     {expected:?}: {answer:?}"
                ));
            }
        }
    }
    // A caller who is no administrator and may address the second mob is
    // served that mob's member too, through its view of that mob.
    for method in ["meta/list_members", "meta/find_members"] {
        let mut params: Value = TOLD_APART_HERE
            .iter()
            .find(|(told, _)| *told == method)
            .map(|(_, params)| serde_json::from_str(params).expect("the row's params are JSON"))
            .expect("the method is told apart here");
        params["mob_id"] = json!(second_mob);
        let answer = ask_as(
            &app,
            rpc_request(method, &params),
            DOOR_ROWS_OF_THE_SECOND_MOB,
        )
        .await;
        let told = answered_for(method, &answer, &launch_mob, &second_mob);
        if told != Some(AnsweredFor::TheSecondMob) {
            wrong.push(format!(
                "{method}, a caller whose rule selects the second mob: is answered for \
                 {told:?}, not for the second mob: {answer:?}"
            ));
        }
    }
    let mut told_apart: Vec<&str> = TOLD_APART_HERE
        .iter()
        .chain(TOLD_APART_BY_ANOTHER_ROW)
        .map(|(method, _)| *method)
        .chain(workgraph_methods_told_apart())
        .collect();
    told_apart.sort_unstable();
    let listed = console_methods_served_per_mob();
    if listed != told_apart {
        wrong.push(format!(
            "the console lists {listed:?} as served per mob; the rows tell two mobs apart on \
             {told_apart:?}"
        ));
    }
    let serve_one_mob = console_methods_that_serve_one_mob();
    for method in &listed {
        if !serve_one_mob.contains(method) {
            wrong.push(format!(
                "{method}: listed as served per mob, and not a console method that serves one mob"
            ));
        }
    }

    drop(app);
    launch.shutdown().await;
    second.shutdown().await;
    assert!(
        wrong.is_empty(),
        "a console method served per mob does not tell two mobs apart:\n{}",
        wrong.join("\n")
    );
}

/// A request of `method` with `params` on the console's multipart route,
/// with no file part.
fn multipart_request(method: &str, params: &Value) -> axum::http::Request<axum::body::Body> {
    let payload = json!({"jsonrpc": "2.0", "id": "row", "method": method, "params": params});
    axum::http::Request::builder()
        .method("POST")
        .uri("/console/rpc/multipart")
        .header(header::CONTENT_TYPE, "multipart/form-data; boundary=rows")
        .body(axum::body::Body::from(format!(
            "--rows\r\nContent-Disposition: form-data; name=\"payload\"\r\n\r\n{payload}\r\n--rows--\r\n"
        )))
        .expect("multipart request")
}

/// A console method is served with another mob named only when it is on one
/// of the console's two lists: the methods served per mob, which answer
/// for the named mob, and the methods of the host, which the named mob
/// selects nothing on. Every other method that hands the door an addressing
/// id (a host-scoped method that is not
/// on the host's list, a method that serves one mob and is not listed, a
/// method served from the launch mob's own wiring, and a name no list
/// holds, as a method added later is) answers a request that names a
/// second served mob with the mob id's refusal, whoever asks: an
/// administrator is refused like every other caller, on the request door
/// and on the multipart route, and the resolver is asked nothing about
/// that mob. The announcement methods that carry a mob of their own are
/// not asked here: the id they name is their own parameter, and
/// [`an_announcement_method_that_names_its_own_mob_is_judged_with_its_caller`]
/// asks them. Nor are the methods over the mobs of the host that act on
/// one mob (`meta/mobs/get`, `lifecycle`, `redefine`, `destroy`): the id
/// they name is the mob they act on, and the family's own rows ask them.
#[tokio::test]
async fn a_console_method_not_listed_as_served_per_mob_refuses_a_second_mob() {
    let launch = runtime("launch").await;
    let second = runtime("second").await;
    let second_mob = second.mob_id();
    let (app, resolver) = console_of_two_mobs(&launch, &second);
    let mut methods = console_methods_refused_for_another_mob();
    // The two methods that end their caller's own sign-in are asked last.
    methods.sort_by_key(|method| {
        matches!(
            *method,
            crate::rpc::CONSOLE_REVOKE_SESSIONS_METHOD | crate::rpc::CONSOLE_SIGN_OUT_METHOD
        )
    });
    assert!(
        methods.len() > 100,
        "the console's methods are asked: {}",
        methods.len()
    );
    let naming_the_second_mob = naming_mob(Some(&second_mob));

    let mut wrong = Vec::new();
    for method in &methods {
        let answer = ask_as(
            &app,
            rpc_request(method, &naming_the_second_mob),
            DOOR_ROWS_ADMIN,
        )
        .await;
        if !is_the_mob_ids_refusal("console RPC", &answer, &second_mob) {
            wrong.push(format!(
                "{method}: is answered for a second mob: {} {:.160}",
                answer.0, answer.2
            ));
        }
    }
    {
        let method = "meta/console/send";
        let answer = ask_as(
            &app,
            multipart_request(method, &naming_the_second_mob),
            DOOR_ROWS_ADMIN,
        )
        .await;
        if !is_the_mob_ids_refusal("console multipart", &answer, &second_mob) {
            wrong.push(format!(
                "{method} on the multipart route: is answered for a second mob: {} {:.160}",
                answer.0, answer.2
            ));
        }
    }
    let asked_about_the_second_mob = asked_about(&resolver, &second_mob);
    if asked_about_the_second_mob != 0 {
        wrong.push(format!(
            "the resolver was asked about a mob these methods do not serve: \
             {asked_about_the_second_mob} times"
        ));
    }

    drop(app);
    launch.shutdown().await;
    second.shutdown().await;
    assert!(
        wrong.is_empty(),
        "a console method that is on neither of the console's two lists answers for a second \
         mob:\n{}",
        wrong.join("\n")
    );
}

/// A request served for a mob records that mob's members as that mob's.
/// After an administrator's requests for the launch mob and for the second
/// mob, the console's access rules know each mob's member as an agent of
/// its own mob and of no other.
#[tokio::test]
async fn a_request_served_for_a_mob_records_its_members_as_that_mobs() {
    let (_launch_state, launch) = runtime_with_member("launch", ONLY_IN_THE_LAUNCH_MOB).await;
    let (_second_state, second) = runtime_with_member("second", ONLY_IN_THE_SECOND_MOB).await;
    let launch_mob = launch.mob_id();
    let second_mob = second.mob_id();
    let second_id = second.mob_runtime().mob_id().clone();
    let state = door_rows_state(&launch, &second_mob);
    let controller = state.access.clone().expect("the fixture has access rules");
    let (app, _resolver) = console_of_two_mobs_over(state, &launch, &second);

    for (mob_id, expected) in [
        (None, AnsweredFor::TheLaunchMob),
        (Some(second_mob.as_str()), AnsweredFor::TheSecondMob),
    ] {
        let method = "meta/list_members";
        let answer = ask_as(
            &app,
            rpc_request(method, &naming_mob(mob_id)),
            DOOR_ROWS_ADMIN,
        )
        .await;
        assert_eq!(
            answered_for(method, &answer, &launch_mob, &second_mob),
            Some(expected),
            "the request is served for the mob it names: {answer:?}"
        );
    }
    let of_the_launch_mob = controller.view_for_subject(Some(DOOR_ROWS_WITHOUT_RULES));
    let of_the_second_mob = of_the_launch_mob.in_mob(&second_id);
    let known = [
        of_the_launch_mob.knows_agent(ONLY_IN_THE_LAUNCH_MOB),
        of_the_launch_mob.knows_agent(ONLY_IN_THE_SECOND_MOB),
        of_the_second_mob.knows_agent(ONLY_IN_THE_SECOND_MOB),
        of_the_second_mob.knows_agent(ONLY_IN_THE_LAUNCH_MOB),
    ];
    drop(app);
    launch.shutdown().await;
    second.shutdown().await;
    assert_eq!(
        known,
        [true, false, true, false],
        "each mob's member is known as an agent of its own mob alone (the launch mob's view of \
         its own member, of the second mob's; the second mob's view of its own, of the launch \
         mob's)"
    );
}

/// A send's last check of its human reads the access rules for the agent
/// of the mob the send is for. A person whose rule lets them send to the
/// second mob's agents still sends to that mob's agent and not to the
/// launch mob's agent of the same name; for a person whose rule lists no
/// mob it is the reverse.
#[test]
fn a_sends_last_check_of_its_human_is_made_in_the_mob_the_send_is_for() {
    let human = super::console_human_input_tests::current_console_human();
    let subject = human.current().expect("a current human");
    let second = meerkat_mob::MobId::from("second");
    let identity = crate::identity_first::AgentIdentity::parse("alice").expect("identity");
    let with_a_rule_that_lists = |mobs: Vec<String>| {
        let controller = AccessController::new(crate::access::AccessControlConfig {
            enabled: true,
            admins: vec![DOOR_ROWS_ADMIN.to_string()],
            rules: vec![crate::access::AccessRule {
                id: "sends".to_string(),
                subjects: vec![subject.clone()],
                actions: vec![crate::access::ACTION_AGENT_SEND.to_string()],
                mobs,
                ..crate::access::AccessRule::default()
            }],
            ..crate::access::AccessControlConfig::default()
        })
        .expect("access configuration");
        let alice = || crate::access::AgentResourceAttributes {
            identity: "alice".to_string(),
            agent_id: None,
            role: None,
            labels: std::collections::BTreeMap::new(),
        };
        controller.record_agent_attributes(alice());
        controller.record_agent_attributes_in(&second, alice());
        controller
    };
    let of_the_second_mob = with_a_rule_that_lists(vec!["second".to_string()]);
    let of_the_launch_mob = with_a_rule_that_lists(Vec::new());
    let sends = |controller: &AccessController, mob: Option<&meerkat_mob::MobId>| {
        human_still_sends(&human, Some(controller), &identity, mob).is_ok()
    };
    assert_eq!(
        [
            sends(&of_the_second_mob, Some(&second)),
            sends(&of_the_second_mob, None),
            sends(&of_the_launch_mob, None),
            sends(&of_the_launch_mob, Some(&second)),
        ],
        [true, false, true, false],
        "a rule of the second mob: its agent, the launch mob's; a rule that lists no mob: the \
         launch mob's agent, the second mob's"
    );
}

/// Whether a send must be governed is read from the runtime of the mob the
/// request is served for, not from the runtime the console's state was
/// built over, and the governance states that mob to the dispatch.
#[tokio::test]
async fn a_requests_governance_reads_the_runtime_of_the_mob_it_is_served_for() {
    let launch = runtime("launch").await;
    let second = runtime("second").await;
    let second_id = second.mob_runtime().mob_id().clone();
    let state = state_over(&launch);
    let state_runtime = state.runtime.as_ref().expect("the state holds a runtime");
    let resolver = launch
        .mob_resolver()
        .serving_a_second_mob_in_tests(&second.mob_resolver());
    let request_mob = console_request_mob(
        &state,
        state_runtime,
        Some(&resolver),
        &NamedMob::from_query(Some(second_id.to_string())),
        "meta/list_members",
    )
    .await
    .expect("the second mob resolves");
    let caller = ConsoleHttpAuthContext {
        principal: None,
        service_identity: false,
        send_scope: ConsoleSendScope::Anonymous,
        access_view: None,
        audience: AccessController::disabled().view_for_subject(None),
        human: None,
        verification: None,
    };
    // What a console whose own runtime's sends are governed states first.
    let mut governance = ConsoleSendGovernance::for_request(&state, &caller);
    governance.runtime_governed = true;
    let governance = governance.serving(&request_mob);
    let governed = governance.runtime_governed;
    let served = governance.served_mob.stored.clone();
    launch.shutdown().await;
    second.shutdown().await;
    assert!(
        !governed,
        "the second mob's runtime governs no send, whatever the state's runtime does"
    );
    assert_eq!(served, Some(second_id));
}

/// The other direction of
/// [`a_requests_governance_reads_the_runtime_of_the_mob_it_is_served_for`]:
/// the console's state was built over a runtime that governs no send, and
/// the request is served for a mob whose runtime's machine has a host's
/// native work authorization owner. The request's send must be governed:
/// the governance reads the served mob's runtime, and what the state's
/// runtime answered does not stand in for it.
///
/// The fixture is two runtimes side by side, not a host. On a launch the
/// wrapper of a mob hosted beside the launch mob shares the launch's
/// process part (`MobRuntime::of_hosted_mob`), and whether sends are
/// governed is a fact of that part: the two wrappers of a real host always
/// answer the same. The row pins which wrapper the governance reads; it
/// shows no host on which the two differ.
#[tokio::test]
async fn a_request_for_a_mob_whose_runtime_governs_sends_is_governed_on_any_console() {
    let scratch = tempfile::tempdir().expect("tempdir");
    let launch = runtime("launch").await;
    let second = crate::unified_runtime::held_identity_test_support::governed_runtime(
        &scratch,
        "console-door-governed",
    )
    .await;
    let second_id = second.mob_runtime().mob_id().clone();
    let the_states_runtime_governs = launch.mob_runtime().native_work_governed();
    let the_second_mobs_runtime_governs = second.mob_runtime().native_work_governed();
    let state = state_over(&launch);
    let state_runtime = state.runtime.as_ref().expect("the state holds a runtime");
    let resolver = launch
        .mob_resolver()
        .serving_a_second_mob_in_tests(&second.mob_resolver());
    let request_mob = console_request_mob(
        &state,
        state_runtime,
        Some(&resolver),
        &NamedMob::from_query(Some(second_id.to_string())),
        "meta/list_members",
    )
    .await
    .expect("the second mob resolves");
    let caller = ConsoleHttpAuthContext {
        principal: None,
        service_identity: false,
        send_scope: ConsoleSendScope::Anonymous,
        access_view: None,
        audience: AccessController::disabled().view_for_subject(None),
        human: None,
        verification: None,
    };
    // What a console whose own runtime governs no send states first.
    let governance = ConsoleSendGovernance::for_request(&state, &caller);
    let stated_first = governance.runtime_governed;
    let governance = governance.serving(&request_mob);
    let governed = governance.runtime_governed;
    let served = governance.served_mob.stored.clone();
    launch.shutdown().await;
    second.shutdown().await;
    assert_eq!(
        (the_states_runtime_governs, the_second_mobs_runtime_governs),
        (false, true),
        "the fixture: the state's runtime governs no send and the second mob's runtime does"
    );
    assert!(
        !stated_first,
        "before its door serves it, the governance states the state's runtime"
    );
    assert!(
        governed,
        "the second mob's runtime governs its sends, whatever the state's runtime does"
    );
    assert_eq!(served, Some(second_id));
}

/// A multipart send's uploads are written for the launch mob's members
/// alone. An upload for a member of another mob is answered the mob id's
/// refusal; one that names no mob, or the launch mob by its id, is not.
#[test]
fn an_upload_for_a_member_of_another_mob_reaches_no_store() {
    let launch = meerkat_mob::MobId::from("launch");
    let naming = |mob_id: &str| NamedMob::from_query(Some(mob_id.to_string()));
    let refused = refuse_upload_for_another_mob(&naming("second"), Some(&launch))
        .expect_err("an upload for another mob is refused");
    assert_eq!(refused.code, -32602);
    assert_eq!(
        refused.data,
        Some(json!({"kind": "foreign_mob_target", "requested_mob_id": "second"}))
    );
    for named in [NamedMob::default(), naming("launch")] {
        assert!(
            refuse_upload_for_another_mob(&named, Some(&launch)).is_ok(),
            "{named:?}"
        );
    }
    // A door that knows no launch mob by its id serves none under an id.
    assert!(refuse_upload_for_another_mob(&naming("launch"), None).is_err());
}

/// A request served for a second mob leaves the launch mob's agents as the
/// access rules know them. Each mob has an agent `alice`: the launch mob's
/// is private to a person, the second mob's is shared. After a request for
/// the launch mob and then one for the second mob, a caller who is not that
/// person is still kept from the launch mob's `alice`, and is not kept from
/// the second mob's.
#[tokio::test]
async fn a_request_served_for_another_mob_leaves_a_private_agent_of_the_launch_mob_its_owner() {
    let (_launch_state, launch) = runtime_with_labelled_member(
        "launch",
        "alice",
        std::collections::BTreeMap::from([(
            crate::access::PRIVATE_TO_LABEL.to_string(),
            "another-person".to_string(),
        )]),
    )
    .await;
    let (_second_state, second) =
        runtime_with_labelled_member("second", "alice", std::collections::BTreeMap::new()).await;
    let second_mob = second.mob_id();
    let second_id = second.mob_runtime().mob_id().clone();
    let state = door_rows_state(&launch, &second_mob);
    let controller = state.access.clone().expect("the fixture has access rules");
    let (app, _resolver) = console_of_two_mobs_over(state, &launch, &second);

    for mob_id in [None, Some(second_mob.as_str())] {
        let answer = ask_as(
            &app,
            rpc_request("meta/list_members", &naming_mob(mob_id)),
            DOOR_ROWS_ADMIN,
        )
        .await;
        assert!(
            answer.0 == StatusCode::OK && answer.2.contains("alice"),
            "the request is served, and lists its mob's agent: {answer:?}"
        );
    }
    let of_the_launch_mob = controller.view_for_subject(Some(DOOR_ROWS_WITHOUT_RULES));
    let of_the_second_mob = of_the_launch_mob.in_mob(&second_id);
    let kept_from = [
        of_the_launch_mob.hides_agent("alice"),
        of_the_second_mob.hides_agent("alice"),
    ];
    drop(app);
    launch.shutdown().await;
    second.shutdown().await;
    assert_eq!(
        kept_from,
        [true, false],
        "a caller who is not its owner is kept from the launch mob's private agent, and not \
         from the second mob's shared agent of the same name"
    );
}

/// Whether `answer` is the announcement family's own answer on a console
/// whose host keeps no announcements: the method answered the request.
fn answered_by_the_announcement_family(answer: &DoorAnswer) -> bool {
    serde_json::from_str::<Value>(&answer.2)
        .is_ok_and(|body| body["error"]["data"]["kind"] == json!("mob_announcements_unavailable"))
}

/// On a gateway that serves one mob and enforces access rules, the
/// announcement methods that carry a mob of their own refuse a caller who
/// is no administrator a request that names any id other than the launch
/// mob's, unless a rule lists that mob for the caller: the answer is the
/// mob id's refusal. The door asks nothing about the id, so a mob a member
/// created and an id no mob has are one case to it; both are asked. An
/// administrator, every caller of a console whose access rules are not
/// enforced, a request that names no mob and a request that names the
/// launch mob by its id are answered by the method.
#[tokio::test]
async fn on_a_gateway_of_one_mob_an_announcement_method_refuses_another_id_to_a_caller_no_rule_lets_address_it()
 {
    let launch = runtime("launch").await;
    let launch_mob = launch.mob_id();
    let app = console_json_router_with_state(door_rows_state(&launch, "a-mob-no-rule-is-asked-of"))
        .layer(axum::Extension(launch.mob_resolver()));
    let without_enforced_access = console_json_router_with_state(state_over(&launch))
        .layer(axum::Extension(launch.mob_resolver()));
    let other_ids = [
        (
            "a mob a member created",
            "console-door-a-member-created-mob",
        ),
        ("an id no mob has", "console-door-an-id-no-mob-has"),
    ];
    let request = |method: &str, mob_id: Option<&str>| rpc_request(method, &naming_mob(mob_id));

    let mut wrong = Vec::new();
    for method in crate::rpc::mob_addressing::ANNOUNCEMENT_METHODS_NAMING_A_MOB {
        for (which, mob_id) in other_ids {
            let answer = ask_as(&app, request(method, Some(mob_id)), DOOR_ROWS_OPERATOR).await;
            if !is_the_mob_ids_refusal("console RPC", &answer, mob_id) {
                wrong.push(format!(
                    "{method}, {which}: a caller who is no administrator is not answered the mob \
                     id's refusal: {answer:?}"
                ));
            }
            let answer = ask_as(&app, request(method, Some(mob_id)), DOOR_ROWS_ADMIN).await;
            if !answered_by_the_announcement_family(&answer) {
                wrong.push(format!(
                    "{method}, {which}: an administrator is not answered by the method: \
                     {answer:?}"
                ));
            }
            let answer = ask(&without_enforced_access, request(method, Some(mob_id))).await;
            if !answered_by_the_announcement_family(&answer) {
                wrong.push(format!(
                    "{method}, {which}: a caller of a console that enforces no access rules is \
                     not answered by the method: {answer:?}"
                ));
            }
        }
        for (which, mob_id) in [
            ("no mob", None),
            ("the launch mob", Some(launch_mob.as_str())),
        ] {
            let answer = ask_as(&app, request(method, mob_id), DOOR_ROWS_OPERATOR).await;
            if !answered_by_the_announcement_family(&answer) {
                wrong.push(format!(
                    "{method}, a request that names {which}: a caller who holds the methods is \
                     not answered by the method: {answer:?}"
                ));
            }
        }
    }

    drop(app);
    drop(without_enforced_access);
    launch.shutdown().await;
    assert!(
        wrong.is_empty(),
        "on a gateway of one mob an announcement method is not answered as it should be:\n{}",
        wrong.join("\n")
    );
}

/// A method of the host that names no mob, or the launch mob by its id, is
/// served: it is never answered the mob id's refusal, and the two requests
/// end the same way (the same status, and a result for both or the same
/// error code). The methods that act for the launch mob when a request
/// names no mob are among them. The two methods that end their caller's
/// own sign-in are not asked: their second request is another caller's.
#[tokio::test]
async fn a_method_of_the_host_that_names_no_mob_or_the_launch_mob_is_served() {
    let launch = runtime("launch").await;
    let second = runtime("second").await;
    let launch_mob = launch.mob_id();
    let (app, _resolver) = console_of_two_mobs(&launch, &second);
    let how_it_ended = |answer: &DoorAnswer| {
        let body: Value = serde_json::from_str(&answer.2).unwrap_or(Value::Null);
        (answer.0, body["error"]["code"].as_i64())
    };

    let mut wrong = Vec::new();
    let mut asked = 0;
    for method in HOST_SCOPED_CONSOLE_METHODS {
        if matches!(
            *method,
            crate::rpc::CONSOLE_REVOKE_SESSIONS_METHOD | crate::rpc::CONSOLE_SIGN_OUT_METHOD
        ) {
            continue;
        }
        asked += 1;
        let no_mob = ask_as(
            &app,
            rpc_request(method, &naming_mob(None)),
            DOOR_ROWS_ADMIN,
        )
        .await;
        let by_id = ask_as(
            &app,
            rpc_request(method, &naming_mob(Some(&launch_mob))),
            DOOR_ROWS_ADMIN,
        )
        .await;
        if is_the_mob_ids_refusal("console RPC", &by_id, &launch_mob)
            || how_it_ended(&by_id) != how_it_ended(&no_mob)
        {
            wrong.push(format!(
                "{method}: the launch mob by its id is not served as no mob is: {by_id:?} / \
                 {no_mob:?}"
            ));
        }
    }
    assert_eq!(
        asked,
        HOST_SCOPED_CONSOLE_METHODS.len() - 2,
        "every method of the host but the two that end a sign-in is asked"
    );
    assert_eq!(asked, 48, "the host's methods");

    drop(app);
    launch.shutdown().await;
    second.shutdown().await;
    assert!(
        wrong.is_empty(),
        "a method of the host is not served for the launch mob:\n{}",
        wrong.join("\n")
    );
}

/// Requests of the announcement family that name the mob `mob_id` in a
/// member of their params other than the top-level `mob_id`: a member, a
/// role selector, a set of mobs, a cursor, an audience grant, a
/// conversation member, a declared subscription and a declared source.
fn announcement_requests_naming_a_mob_inside(mob_id: &str) -> Vec<(&'static str, Value)> {
    let member = json!({"mob_id": mob_id, "identity": "alice"});
    vec![
        (
            "meta/mob_announcements/subscriptions/put",
            json!({"member": member}),
        ),
        (
            "meta/mob_announcements/subscriptions/put",
            json!({"members": {"mob_id": mob_id, "role": "worker"}}),
        ),
        (
            "meta/mob_announcements/subscriptions/put",
            json!({"mobs": [mob_id]}),
        ),
        (
            "meta/mob_announcements/subscriptions/list",
            json!({"member": member}),
        ),
        (
            "meta/mob_announcements/deliveries/list",
            json!({"member": member}),
        ),
        (
            "meta/mob_announcements/members/get",
            json!({"member": member}),
        ),
        (
            "meta/mob_announcements/members/list",
            json!({"after": member}),
        ),
        (
            "meta/mob_announcements/audit/list",
            json!({"member": member}),
        ),
        (
            "meta/mob_announcements/sources/put",
            json!({"audience": [{"kind": "member", "member": member}]}),
        ),
        (
            "meta/mob_announcements/sources/put",
            json!({"audience": [{"kind": "role", "mob_id": mob_id, "role": "worker"}]}),
        ),
        (
            "meta/mob_announcements/sources/put",
            json!({"conversation_members": [member]}),
        ),
        (
            "meta/mob_announcements/declarations/apply",
            json!({"declarations": {"subscriptions": [{"mobs": [mob_id]}]}}),
        ),
        (
            "meta/mob_announcements/declarations/apply",
            json!({"declarations": {"sources": [{"mob_id": mob_id}]}}),
        ),
    ]
}

/// Whether `answer` is the request doors' refusal of malformed params:
/// invalid params, with no word about a mob.
fn is_the_malformed_params_refusal(answer: &DoorAnswer) -> bool {
    answer.0 == StatusCode::OK
        && serde_json::from_str::<Value>(&answer.2)
            .is_ok_and(|body| body["error"] == json!({"code": -32602, "message": "Invalid params"}))
}

/// A request of the announcement family is judged for every mob it names,
/// wherever in its params it names it. A mob other than the launch mob
/// that the caller may not address is answered the mob id's refusal,
/// exactly as an id no mob has, whatever the caller holds in the launch
/// mob; a caller who may address the mob, and a request that names the
/// launch mob, are not refused for the mob. A member given by position,
/// where the family would read the mob it names, is refused as malformed
/// params for a caller who may not address every mob. The resolver is
/// asked nothing about the second mob.
#[tokio::test]
async fn a_request_of_the_announcement_family_is_judged_for_every_mob_it_names() {
    let launch = runtime("launch").await;
    let second = runtime("second").await;
    let launch_mob = launch.mob_id();
    let second_mob = second.mob_id();
    let absent_mob = an_id_no_mob_has(&second_mob);
    let (app, resolver) = console_of_two_mobs(&launch, &second);

    let mut wrong = Vec::new();
    let for_the_absent_mob = announcement_requests_naming_a_mob_inside(&absent_mob);
    let for_the_launch_mob = announcement_requests_naming_a_mob_inside(&launch_mob);
    for (index, (method, params)) in announcement_requests_naming_a_mob_inside(&second_mob)
        .into_iter()
        .enumerate()
    {
        let asked = format!("{method} {params}");
        let second_answer = ask_as(&app, rpc_request(method, &params), DOOR_ROWS_OPERATOR).await;
        let absent_answer = ask_as(
            &app,
            rpc_request(method, &for_the_absent_mob[index].1),
            DOOR_ROWS_OPERATOR,
        )
        .await;
        if !is_the_mob_ids_refusal("console RPC", &second_answer, &second_mob) {
            wrong.push(format!(
                "{asked}: a mob the caller may not address is not answered the mob id's \
                 refusal: {second_answer:?}"
            ));
        }
        if apart_from_the_id(&second_answer, &second_mob)
            != apart_from_the_id(&absent_answer, &absent_mob)
        {
            wrong.push(format!(
                "{asked}: is answered differently for the second mob and for an id no mob has: \
                 {second_answer:?} / {absent_answer:?}"
            ));
        }
        for (who, email) in [
            ("an administrator", DOOR_ROWS_ADMIN),
            (
                "a person whose rule selects the mob",
                DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB,
            ),
        ] {
            let answer = ask_as(&app, rpc_request(method, &params), email).await;
            if is_the_mob_ids_refusal("console RPC", &answer, &second_mob) {
                wrong.push(format!(
                    "{asked}, {who}: a mob the caller may address is refused: {answer:?}"
                ));
            }
        }
        let answer = ask_as(
            &app,
            rpc_request(method, &for_the_launch_mob[index].1),
            DOOR_ROWS_OPERATOR,
        )
        .await;
        if is_the_mob_ids_refusal("console RPC", &answer, &launch_mob) {
            wrong.push(format!(
                "{method}: the launch mob named inside the params is refused: {answer:?}"
            ));
        }
    }
    let by_position = json!({"member": [second_mob, "alice"]});
    let method = "meta/mob_announcements/subscriptions/list";
    let answer = ask_as(&app, rpc_request(method, &by_position), DOOR_ROWS_OPERATOR).await;
    if !is_the_malformed_params_refusal(&answer) {
        wrong.push(format!(
            "{method}: a member given by position is not refused as malformed params: {answer:?}"
        ));
    }
    let asked_about_the_second_mob = asked_about(&resolver, &second_mob);
    if asked_about_the_second_mob != 0 {
        wrong.push(format!(
            "the resolver was asked about the second mob: {asked_about_the_second_mob} times"
        ));
    }

    drop(app);
    launch.shutdown().await;
    second.shutdown().await;
    assert!(
        wrong.is_empty(),
        "a request of the announcement family is not judged for every mob it names:\n{}",
        wrong.join("\n")
    );
}

/// The announcement family reads a member given by position as the member
/// given by name: the reason its door refuses the positional form.
#[test]
fn a_member_of_the_announcement_family_given_by_position_is_read_as_one() {
    let by_position: crate::mob_announcements::Member =
        serde_json::from_value(json!(["mob-b", "alice"])).expect("a member by position");
    let by_name: crate::mob_announcements::Member =
        serde_json::from_value(json!({"mob_id": "mob-b", "identity": "alice"}))
            .expect("a member by name");
    assert_eq!(by_position, by_name);
    assert_eq!(
        serde_json::to_value(&by_position).expect("a member serializes")["mob_id"],
        json!("mob-b")
    );
}

/// The console judges the mobs a request of the announcement family names
/// once, before the family reads the request, for a caller who may not
/// address every mob: an administrator, and every caller where access is
/// not enforced, may address every mob and is not judged; a caller a rule
/// lists one mob for is judged for every other. The family's answer goes to
/// the caller as the family made it: what a caller may not see the family
/// leaves out itself, on its own rows, and the console filters nothing
/// after the method, so no count, cursor or page length is changed behind
/// it. Read from the callers' views and from the console's source.
#[test]
fn the_console_judges_an_announcement_requests_mobs_and_filters_no_answer() {
    let runtime_admin = vec![crate::access::ACTION_RUNTIME_ADMIN.to_string()];
    let controller = AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![DOOR_ROWS_ADMIN.to_string()],
        rules: vec![
            crate::access::AccessRule {
                id: "operates-the-launch-mob".to_string(),
                subjects: vec![
                    DOOR_ROWS_OPERATOR.to_string(),
                    DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB.to_string(),
                ],
                actions: runtime_admin.clone(),
                ..crate::access::AccessRule::default()
            },
            crate::access::AccessRule {
                id: "operates-the-second-mob".to_string(),
                subjects: vec![DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB.to_string()],
                actions: runtime_admin,
                mobs: vec!["second".to_string()],
                ..crate::access::AccessRule::default()
            },
        ],
        ..crate::access::AccessControlConfig::default()
    })
    .expect("access configuration");
    let second = meerkat_mob::MobId::from("second");
    let third = meerkat_mob::MobId::from("third");

    let administrator = controller.view_for_subject(Some(DOOR_ROWS_ADMIN));
    let operator = controller.view_for_subject(Some(DOOR_ROWS_OPERATOR));
    let of_the_second_mob = controller.view_for_subject(Some(DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB));
    let not_enforced = AccessController::disabled().view_for_subject(None);
    assert!(administrator.may_address_every_mob());
    assert!(not_enforced.may_address_every_mob());
    assert!(!operator.may_address_every_mob());
    assert!(!of_the_second_mob.may_address_every_mob());
    assert_eq!(
        [
            operator.may_address_other_mob(&second),
            of_the_second_mob.may_address_other_mob(&second),
            of_the_second_mob.may_address_other_mob(&third),
            administrator.may_address_other_mob(&third),
        ],
        [false, true, false, true],
        "the mobs a caller may address"
    );

    let source = include_str!("http_console.rs");
    assert_eq!(
        source
            .matches("judge_the_mobs_an_announcement_request_names(")
            .count(),
        2,
        "the judgement of a request's mobs is written once and called once"
    );
    assert_eq!(
        source.matches("retain_announcement_rows").count(),
        0,
        "the console filters no answer of the announcement family after the method"
    );
}

/// A runtime over one mob that keeps announcements: an identity-first
/// launch over a state directory, composed as a gateway that enables them
/// composes it. Its roster registers `agent:a` and `agent:t`, the two
/// members the rows' sources name: the announcement methods admit a source
/// a caller sends only when every member it names is seated. The state
/// directory and the runtime.
async fn runtime_with_announcements(tag: &str) -> (tempfile::TempDir, crate::UnifiedRuntime) {
    let state_dir = tempfile::tempdir().expect("a state directory");
    let mob_id = format!("console-door-{tag}-{}", uuid::Uuid::new_v4().simple());
    let definition = meerkat_mob::MobDefinition::from_toml(&format!(
        "[mob]\nid = \"{mob_id}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n\
         external_addressable = true\nruntime_mode = \"turn_driven\"\n\n\
         [profiles.worker.tools]\ncomms = true\n"
    ))
    .expect("mob definition");
    let seated = |identity: &str| crate::identity_first::DurableAgentSpec {
        identity: crate::identity_first::AgentIdentity::parse(identity).expect("identity"),
        profile: "worker".into(),
        addressability: crate::identity_first::AgentAddressability::Addressable,
        display_name: None,
        labels: std::collections::BTreeMap::new(),
        context: None,
        additional_instructions: Vec::new(),
        initial_message: None,
        runtime_mode_override: None,
        backend: None,
        binding: None,
        placement: None,
    };
    let roster = Arc::new(crate::identity_first::MutableRosterProvider::new(vec![
        seated("agent:a"),
        seated("agent:t"),
    ]));
    let builder = crate::UnifiedRuntime::builder()
        .definition(definition)
        .persistent_state(state_dir.path())
        .continuity_from_state_dir(state_dir.path())
        .await
        .expect("the identity stores of the state directory")
        .roster_provider(roster as Arc<dyn crate::identity_first::RosterProvider>)
        .identity_bootstrap_mode(crate::identity_first::IdentityBootstrapMode::LazyMaterialize)
        .identity_runtime_instance_id(mob_id)
        .comms(true)
        .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
        .mob_announcements(crate::mob_announcements::AnnouncementConfig::default());
    let runtime = Box::pin(builder.build()).await.expect("runtime builds");
    assert!(
        runtime.mob_announcements().is_some(),
        "the runtime keeps announcements"
    );
    let (seating, timed_out) = runtime
        .identity_runtime()
        .expect("an identity-first runtime")
        .wait_identity_bootstrap_terminal(std::time::Duration::from_mins(2))
        .await;
    assert!(
        !timed_out && seating.ready,
        "the roster's members are seated: {seating:?}"
    );
    (state_dir, runtime)
}

/// A whole declared source `source_id` of the mob `mob_id`, as
/// `declarations/apply` takes it: a mailbox source with topics of its own,
/// two members of the mob in its audience (the two the fixture's roster
/// registers), a direct route, and a triage route with its member, the
/// topics it forwards on and the audience it forwards to.
fn whole_declared_source(source_id: &str, mob_id: &str) -> Value {
    let member = |identity: &str| json!({"mob_id": mob_id, "identity": identity});
    let grant = |identity: &str| json!({"kind": "member", "member": member(identity)});
    json!({
        "source_id": source_id,
        "identity": {
            "provider": "google.gmail",
            "principal": {
                "kind": "connector_verified_account",
                "issuer": "https://accounts.example",
                "strategy_id": "oauth-code",
                "subject": format!("subject-of-{source_id}"),
            },
            "scope": {"kind": "mailbox_label", "id": "INBOX"},
        },
        "connection": {"kind": "connect_setup", "id": format!("setup-of-{source_id}")},
        "observation": {"baseline": "delta", "deletions": "explicit"},
        "topics": [format!("mail.{source_id}.received")],
        "audience": [grant("agent:a"), grant("agent:t")],
        "routes": [
            {"kind": "direct"},
            {
                "kind": "triage",
                "member": member("agent:t"),
                "forward_topics": [format!("mail.{source_id}.triaged")],
                "forward_audience": [grant("agent:a")],
            },
        ],
    })
}

/// A whole, valid request of the announcement family that names the launch
/// mob alone is answered by the family for a caller who may not address
/// every mob, as it is for an administrator. On a gateway of one mob that
/// keeps announcements and enforces access rules, `sources/put` and
/// `declarations/apply` are sent whole, each with a source that holds a
/// triage route, by an administrator and by a caller who holds the runtime
/// administration and whom no rule lists another mob for: each is answered
/// with the family's result.
#[tokio::test(flavor = "multi_thread")]
async fn a_whole_request_of_the_announcement_family_for_the_launch_mob_is_answered_by_the_family() {
    let (_state_dir, launch) = runtime_with_announcements("whole").await;
    let launch_mob = launch.mob_id();
    for source_id in ["deska", "deskb", "feed", "wire"] {
        let declared: Result<crate::mob_announcements::declarations::DeclaredSource, _> =
            serde_json::from_value(whole_declared_source(source_id, &launch_mob));
        assert!(declared.is_ok(), "a whole declared source: {declared:?}");
    }
    let state = ConsoleJsonState {
        mob_announcements: launch.mob_announcements_slot(),
        ..door_rows_state(&launch, "console-door-no-second-mob")
    };
    let app = console_json_router_with_state(state).layer(axum::Extension(launch.mob_resolver()));
    let put = |source_id: &str| {
        let mut source = whole_declared_source(source_id, &launch_mob);
        source["operation_id"] = json!(format!("put-{source_id}"));
        source
    };
    let apply = |revision: u64, source_ids: &[&str]| {
        let sources: Vec<Value> = source_ids
            .iter()
            .map(|source_id| whole_declared_source(source_id, &launch_mob))
            .collect();
        json!({
            "revision": revision,
            "declarations": {"revision": revision, "sources": sources, "subscriptions": []},
        })
    };

    let mut wrong = Vec::new();
    for (who, email, method, params) in [
        (
            "an administrator",
            DOOR_ROWS_ADMIN,
            "meta/mob_announcements/sources/put",
            put("deska"),
        ),
        (
            "an administrator",
            DOOR_ROWS_ADMIN,
            "meta/mob_announcements/declarations/apply",
            apply(1, &["feed"]),
        ),
        (
            "a caller no rule lists another mob for",
            DOOR_ROWS_OPERATOR,
            "meta/mob_announcements/sources/put",
            put("deskb"),
        ),
        (
            "a caller no rule lists another mob for",
            DOOR_ROWS_OPERATOR,
            "meta/mob_announcements/declarations/apply",
            apply(2, &["feed", "wire"]),
        ),
    ] {
        let answer = ask_as(&app, rpc_request(method, &params), email).await;
        let answered = serde_json::from_str::<Value>(&answer.2)
            .is_ok_and(|body| body["error"].is_null() && body["result"].is_object());
        if !answered {
            wrong.push(format!(
                "{method}, {who}: is not answered with the family's result: {answer:?}"
            ));
        }
    }

    drop(app);
    launch.shutdown().await;
    assert!(
        wrong.is_empty(),
        "a whole request of the announcement family for the launch mob is not answered by the \
         family:\n{}",
        wrong.join("\n")
    );
}

/// A launch that keeps announcements releases, at the create of a mob,
/// what an earlier mob of the same id left with them, through the
/// announcements' own owner. The launch said before its runtime was built
/// that it composes announcements, and the composition bound the owner's
/// release when it attached the owner: the runtime reads the release as
/// bound. An announcement made in a mob a member created stays with the
/// announcements when that mob is destroyed, and is gone once the member
/// creates a mob under its id: no read by its key answers it.
#[tokio::test(flavor = "multi_thread")]
async fn a_mob_created_under_an_earlier_mobs_id_finds_none_of_its_announcements() {
    use crate::mob_announcements::{AnnounceRequest, Announcer, SourceKey, Topic};
    use crate::mob_announcements_release::MobAnnouncementsStanding;

    let (_state_dir, launch) = runtime_with_announcements("release").await;
    let announcements = launch
        .mob_announcements()
        .expect("the launch keeps announcements");
    let mut wrong = Vec::new();
    match launch.mob_runtime().mob_announcements_release().standing() {
        MobAnnouncementsStanding::Bound(_) => {}
        MobAnnouncementsStanding::NotDeclared => wrong.push(
            "the launch did not say it composes announcements: their release at a create is \
             not declared"
                .to_string(),
        ),
        MobAnnouncementsStanding::Owed => wrong.push(
            "the announcements' owner is attached and its release at a create is not bound"
                .to_string(),
        ),
    }

    let set = launch
        .mob_runtime()
        .agent_mob_mcp_state()
        .expect("the launch's mob set");
    let definition = meerkat_mob::MobDefinition::from_toml(&format!(
        "[mob]\nid = \"release-rows-second-{}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n",
        uuid::Uuid::new_v4().simple()
    ))
    .expect("mob definition");
    let again = definition.clone();
    // The mob is a member's: the member is seated in the launch mob and
    // creates with its session, as its mob tools do.
    launch
        .mob_handle()
        .spawn_spec(SpawnMemberSpec::from_wire(
            "worker".to_string(),
            "maker".to_string(),
            None,
            None,
            None,
        ))
        .await
        .expect("the member is spawned");
    let maker = launch
        .mob_handle()
        .resolve_bridge_session_id(&meerkat_mob::AgentIdentity::from("maker"))
        .await
        .expect("the member's session");
    let second = Box::pin(set.mob_create_definition_with_owner_bridge_session(
        definition,
        maker.clone(),
        true,
        false,
    ))
    .await
    .expect("a member creates a mob");

    let of_the_second =
        crate::mob_announcements::MobId::new(second.to_string()).expect("the mob's id");
    let key = SourceKey::new("left-by-the-earlier-mob").expect("a source key");
    announcements
        .announce(AnnounceRequest {
            mob_id: of_the_second.clone(),
            announced_by: Announcer::Host,
            source_key: key.clone(),
            topics: [Topic::new("release-rows").expect("a topic")].into(),
            body: "of the earlier mob".to_string(),
            correlation_id: None,
            causes: Vec::new(),
        })
        .await
        .expect("the host announces in a mob it hosts");

    let destroyed = Box::pin(set.mob_destroy(&second)).await;
    assert!(destroyed.is_ok(), "the second mob is destroyed");
    let kept = announcements
        .announcement(&of_the_second, &key)
        .await
        .expect("the announcements read");
    assert!(
        kept.is_some(),
        "the destroyed mob's announcement is still with the announcements"
    );

    let created =
        Box::pin(set.mob_create_definition_with_owner_bridge_session(again, maker, true, false))
            .await
            .expect("the member creates a mob under the earlier mob's id");
    assert_eq!(created, second, "the new mob stands under the same id");
    let left = announcements
        .announcement(&of_the_second, &key)
        .await
        .expect("the announcements read");
    launch.shutdown().await;
    if let Some(left) = left {
        wrong.push(format!(
            "the mob created under the id finds an announcement of the earlier mob: {:?}",
            left.body
        ));
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The announcements keep nothing under a mob id they cannot hold: their
/// ids have a length and hold no control character, and a mob's id has no
/// such rule. So there is nothing to release for such an id: the launch's
/// release at a create answers that it is done, and the create goes on as
/// it does on a launch that keeps no announcements.
#[tokio::test(flavor = "multi_thread")]
async fn the_release_for_a_mob_id_the_announcements_cannot_hold_has_nothing_to_release() {
    use crate::mob_announcements_release::MobAnnouncementsStanding;

    let (_state_dir, launch) = runtime_with_announcements("unheld-id").await;
    let MobAnnouncementsStanding::Bound(release) =
        launch.mob_runtime().mob_announcements_release().standing()
    else {
        panic!("the launch's release of what the announcements keep is bound");
    };
    let mut answers = Vec::new();
    for id in ["with-a\u{7}-bell".to_string(), "x".repeat(257)] {
        answers.push(
            release
                .release_mob(&meerkat_mob::MobId::from(id.as_str()))
                .await,
        );
    }
    launch.shutdown().await;
    assert_eq!(
        answers,
        vec![Ok(()), Ok(())],
        "an id the announcements cannot hold has nothing to release"
    );
}

/// A host-scoped method asked on the console's request door with the id of
/// a mob of the host that is known and not loaded loads nothing, whoever
/// asks: the mob the request names stays not loaded.
///
/// On a method the door lists as a method of the host
/// ([`METHODS_OF_THE_HOST_AMONG_THE_READS`]) the id is judged with the
/// caller and then selects nothing: a signed-in administrator is answered
/// what the same request that names no mob is answered, byte for byte,
/// and a caller no rule names is answered the mob id's refusal. Every
/// other host-scoped method is the launch mob's alone: a request that
/// names another mob is answered the mob id's refusal, the
/// administrator's as that of a caller no rule names. The administrator's
/// same request that names no mob is answered in both cases.
///
/// The mob is one the runtime held an entry of before it was unloaded:
/// the row waits for that entry before it puts the mob to sleep.
///
/// The console requires a sign-in and enforces access rules
/// ([`door_rows_state`]); its resolver is the launch's own, over a state
/// directory, with a second mob the mob set unloaded. The methods are the
/// console's host-scoped reads: a host method that changes the console's
/// own access state would change who the row's next caller is.
#[tokio::test(flavor = "multi_thread")]
async fn a_host_scoped_request_of_a_signed_in_caller_for_a_mob_that_is_not_loaded_loads_nothing() {
    /// The console's host-scoped methods that read and change nothing.
    const HOST_SCOPED_CONSOLE_READS: &[&str] = &[
        "meta/access/get",
        "meta/access/status",
        "meta/console/attention",
        "meta/console/audit",
        "meta/console/whoami",
        "meta/cross_mob/directory",
        "meta/decisions/list",
        "meta/mob_announcements/subscriptions/list",
        "meta/peer_pubkey",
        "meta/schedule/tools",
        "meta/storage/doctor",
    ];
    /// Those of them the door lists as methods of the host: a mob named
    /// beside one selects nothing.
    const METHODS_OF_THE_HOST_AMONG_THE_READS: &[&str] = &[
        "meta/access/get",
        "meta/access/status",
        "meta/console/attention",
        "meta/console/audit",
        "meta/console/whoami",
        "meta/decisions/list",
    ];

    let stored = crate::unified_runtime::StoredHost::new();
    let launch = stored.launch().await;
    let definition = meerkat_mob::MobDefinition::from_toml(&format!(
        "[mob]\nid = \"console-door-asleep-{}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n",
        uuid::Uuid::new_v4().simple()
    ))
    .expect("mob definition");
    let second = Box::pin(
        launch
            .mob_runtime()
            .agent_mob_mcp_state()
            .expect("the launch's mob set")
            .mob_create_definition(definition),
    )
    .await
    .expect("the mob set creates a second mob of the host");
    // The runtime holds the second mob's entry before the mob is put to
    // sleep: the row asks about a mob the host has served and unloaded.
    let entry_by = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while !launch.mob_entries_in_tests().contains(&second) {
        assert!(
            tokio::time::Instant::now() < entry_by,
            "the runtime holds the entry of the mob its set created"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    crate::unified_runtime::asleep(&launch, &[&second]).await;
    let not_loaded = || async {
        let known = crate::unified_runtime::known(&launch).await;
        known.not_loaded.contains(&second) && !known.loaded.contains(&second)
    };
    let app = console_json_router_with_state(door_rows_state(&launch, second.as_str()))
        .layer(axum::Extension(launch.mob_resolver()));
    let mut wrong = Vec::new();
    for method in HOST_SCOPED_CONSOLE_READS {
        if !HOST_SCOPED_CONSOLE_METHODS.contains(method) {
            wrong.push(format!("{method}: not a method of the host"));
            continue;
        }
        let named = naming_mob(Some(second.as_str()));
        let for_no_mob = ask_as(
            &app,
            rpc_request(method, &naming_mob(None)),
            DOOR_ROWS_ADMIN,
        )
        .await;
        if is_the_mob_ids_refusal("console RPC", &for_no_mob, second.as_str()) {
            wrong.push(format!(
                "{method}: an administrator's request that names no mob is refused: \
                 {for_no_mob:?}"
            ));
        }
        let of_the_host = METHODS_OF_THE_HOST_AMONG_THE_READS.contains(method);
        if super::mobs_selector::console_method_is_of_the_host(method) != of_the_host {
            wrong.push(format!(
                "{method}: this row holds it as a method of the host: {of_the_host}; the \
                 door's list says otherwise"
            ));
        }
        for (who, email) in [
            ("an administrator", DOOR_ROWS_ADMIN),
            ("a caller no rule names", DOOR_ROWS_WITHOUT_RULES),
        ] {
            let answer = ask_as(&app, rpc_request(method, &named), email).await;
            if of_the_host && email == DOOR_ROWS_ADMIN {
                // The named mob selects nothing: the answer is the one of
                // the request that names no mob.
                if answer != for_no_mob {
                    wrong.push(format!(
                        "{method}: {who} who names a mob beside a method of the host is \
                         not answered what the request that names no mob is answered:\n  \
                         with the mob named: {answer:?}\n  with no mob named: {for_no_mob:?}"
                    ));
                }
            } else if !is_the_mob_ids_refusal("console RPC", &answer, second.as_str()) {
                wrong.push(format!(
                    "{method}: {who} is not answered the mob id's refusal for a mob other \
                     than the launch mob: {answer:?}"
                ));
            }
            if !not_loaded().await {
                wrong.push(format!("{method}: the request of {who} loaded the mob"));
                crate::unified_runtime::asleep(&launch, &[&second]).await;
            }
        }
    }

    drop(app);
    launch.shutdown().await;
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// A request whose params are given by position, or are no object at all,
/// is refused as malformed params on the console's two request doors,
/// whoever asks and whatever its method: a request names its mob by a
/// member of its params, and the doors read it nowhere else. The named
/// form is answered as before.
#[tokio::test]
async fn a_request_whose_params_are_no_object_is_refused_on_the_request_doors() {
    let launch = runtime("launch").await;
    let second = runtime("second").await;
    let second_mob = second.mob_id();
    let (app, resolver) = console_of_two_mobs(&launch, &second);

    let mut wrong = Vec::new();
    for method in [
        "meta/mob_announcements/sources/list",
        "meta/mob_announcements/get",
        "meta/list_members",
        "meta/mob_events/query",
        "meta/access/status",
    ] {
        for (shape, params) in [
            ("a list that begins with a mob id", json!([second_mob])),
            ("an empty list", json!([])),
            ("a text", json!(second_mob)),
            ("a number", json!(7)),
        ] {
            for email in [DOOR_ROWS_OPERATOR, DOOR_ROWS_ADMIN] {
                let answer = ask_as(&app, rpc_request(method, &params), email).await;
                if !is_the_malformed_params_refusal(&answer) {
                    wrong.push(format!(
                        "{method}, {shape}, {email}: is not refused as malformed params: \
                         {answer:?}"
                    ));
                }
            }
        }
        let named = ask_as(&app, rpc_request(method, &json!({})), DOOR_ROWS_ADMIN).await;
        let absent = ask_as(&app, rpc_request(method, &Value::Null), DOOR_ROWS_ADMIN).await;
        if is_the_malformed_params_refusal(&named) || is_the_malformed_params_refusal(&absent) {
            wrong.push(format!(
                "{method}: params that are an object, or absent, are refused as malformed: \
                 {named:?} / {absent:?}"
            ));
        }
    }
    let answer = ask_as(
        &app,
        multipart_request("meta/console/send", &json!([second_mob])),
        DOOR_ROWS_ADMIN,
    )
    .await;
    if !is_the_malformed_params_refusal(&answer) {
        wrong.push(format!(
            "the multipart route: params given by position are not refused: {answer:?}"
        ));
    }
    let asked_about_the_second_mob = asked_about(&resolver, &second_mob);
    if asked_about_the_second_mob != 0 {
        wrong.push(format!(
            "the resolver was asked about the second mob: {asked_about_the_second_mob} times"
        ));
    }

    drop(app);
    launch.shutdown().await;
    second.shutdown().await;
    assert!(
        wrong.is_empty(),
        "params that are no object are not refused on the request doors:\n{}",
        wrong.join("\n")
    );
}

/// A caller who is no administrator addresses a mob other than the launch
/// mob by a separate allow rule that lists the mob: the rule that gives the
/// caller the runtime administration methods and the audit read lists no
/// mob, and a second rule that applies to the caller lists the other mob in
/// `mobs`. On a gateway of one mob such a caller is answered by an
/// announcement method that names that mob and reads, with every mob
/// selected, the audit row that names it; a caller with the first rule
/// alone is refused the one and not shown the other. The mob is one a
/// member of the launch mob created and that runs: the host does not
/// serve it, and the console knows its members as its own.
#[tokio::test]
async fn a_separate_rule_that_lists_a_mob_lets_its_caller_address_that_mob() {
    let (_launch_state, launch) =
        runtime_with_labelled_member("launch", "maker", std::collections::BTreeMap::new()).await;
    let listed_mob = "console-door-a-member-created-mob";
    // A member of the launch mob creates the listed mob: it is made for
    // the member's session, as the agent mob tools make one.
    let maker = meerkat_mob::ids::AgentIdentity::from("maker");
    launch
        .mob_handle()
        .ensure_member_live(&maker)
        .await
        .expect("the member is live");
    let makers_session = launch
        .mob_handle()
        .get_member(&maker)
        .await
        .expect("the roster reads")
        .and_then(|entry| entry.bridge_session_id().cloned())
        .expect("the member has a session");
    let definition: meerkat_mob::MobDefinition = serde_json::from_value(json!({
        "id": listed_mob,
        "profiles": { "worker": {
            "model": "gpt-5.5",
            "runtime_mode": "turn_driven",
            "tools": { "comms": true },
        } },
    }))
    .expect("mob definition");
    Box::pin(
        launch
            .mob_runtime()
            .agent_mob_mcp_state()
            .expect("the launch's mob set")
            .mob_create_definition_with_owner_bridge_session(
                definition,
                makers_session,
                true,
                false,
            ),
    )
    .await
    .expect("the member's mob is created");
    let log = audit_log_holding(&[
        json!({"member_id": "of-the-launch-mob"}),
        json!({"mob_id": listed_mob, "member_id": "of-the-listed-mob"}),
    ])
    .await;
    let state = ConsoleJsonState {
        console_audit: Some(log),
        ..door_rows_state(&launch, listed_mob)
    };
    let app = console_json_router_with_state(state).layer(axum::Extension(launch.mob_resolver()));
    let method = "meta/mob_announcements/sources/list";
    let request = || rpc_request(method, &naming_mob(Some(listed_mob)));

    let with_the_first_rule_alone = ask_as(&app, request(), DOOR_ROWS_OPERATOR).await;
    let with_both_rules = ask_as(&app, request(), DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB).await;
    let read_with_the_first_rule_alone = audit_members_as(&app, DOOR_ROWS_OPERATOR).await;
    let read_with_both_rules = audit_members_as(&app, DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB).await;
    drop(app);
    launch.shutdown().await;

    assert!(
        is_the_mob_ids_refusal("console RPC", &with_the_first_rule_alone, listed_mob),
        "a caller no rule of whom lists the mob is refused: {with_the_first_rule_alone:?}"
    );
    assert!(
        answered_by_the_announcement_family(&with_both_rules),
        "a caller with a separate rule that lists the mob is answered by the method: \
         {with_both_rules:?}"
    );
    assert_eq!(
        read_with_the_first_rule_alone,
        Ok(vec!["of-the-launch-mob".to_string()])
    );
    assert_eq!(
        read_with_both_rules,
        Ok(vec![
            "of-the-launch-mob".to_string(),
            "of-the-listed-mob".to_string()
        ])
    );
}

/// The audit read records the members of a mob the host serves before it
/// judges a row of that mob. Each mob has an agent `alice`: the second
/// mob's is private to a person, the launch mob's is shared, and no request
/// was served for the second mob before the read. With every mob selected,
/// a caller who may address
/// both mobs and is not that person reads the launch mob's row and not the
/// second mob's; an administrator reads both.
#[tokio::test]
async fn the_audit_read_records_a_served_mobs_members_before_it_judges_its_rows() {
    let (_launch_state, launch) =
        runtime_with_labelled_member("launch", "alice", std::collections::BTreeMap::new()).await;
    let (_second_state, second) = runtime_with_labelled_member(
        "second",
        "alice",
        std::collections::BTreeMap::from([(
            crate::access::PRIVATE_TO_LABEL.to_string(),
            "another-person".to_string(),
        )]),
    )
    .await;
    let second_mob = second.mob_id();
    let log = audit_log_holding(&[
        json!({"member_id": "alice"}),
        json!({"mob_id": second_mob, "member_id": "alice"}),
    ])
    .await;
    let state = ConsoleJsonState {
        console_audit: Some(log),
        ..door_rows_state(&launch, &second_mob)
    };
    let (app, _resolver) = console_of_two_mobs_over(state, &launch, &second);
    let rows_as = |email: &'static str| {
        let app = app.clone();
        let second_mob = second_mob.clone();
        async move {
            let answer = ask_as(
                &app,
                rpc_request(crate::rpc::CONSOLE_AUDIT_METHOD, &audit_read_of_every_mob()),
                email,
            )
            .await;
            let body: Value = serde_json::from_str(&answer.2).unwrap_or(Value::Null);
            body["result"]["records"]
                .as_array()
                .map(|records| {
                    records
                        .iter()
                        .map(|row| row["target"]["mob_id"] == json!(second_mob.as_str()))
                        .collect::<Vec<bool>>()
                })
                .ok_or(answer)
        }
    };

    let as_a_caller_of_both_mobs = rows_as(DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB).await;
    let as_an_administrator = rows_as(DOOR_ROWS_ADMIN).await;
    drop(app);
    launch.shutdown().await;
    second.shutdown().await;
    assert_eq!(
        as_a_caller_of_both_mobs,
        Ok(vec![false]),
        "a caller who is not its owner reads the launch mob's row alone (each row: whether it \
         is the second mob's)"
    );
    assert_eq!(as_an_administrator, Ok(vec![false, true]));
}

/// The answers of `first` and of `second` on `app`, both sent with one
/// sign-in of `email`: an answer that states when its caller's sign-in ends
/// states the same time in both.
async fn ask_both_as(
    app: &Router,
    mut first: axum::http::Request<axum::body::Body>,
    mut second: axum::http::Request<axum::body::Body>,
    email: &str,
) -> (DoorAnswer, DoorAnswer) {
    let sign_in: axum::http::HeaderValue = format!("Bearer {}", door_rows_token(email))
        .parse()
        .expect("header value");
    first
        .headers_mut()
        .insert(header::AUTHORIZATION, sign_in.clone());
    second.headers_mut().insert(header::AUTHORIZATION, sign_in);
    (ask(app, first).await, ask(app, second).await)
}

/// Whether `resolver` was asked to resolve the mob `mob_id`: the one
/// question that loads a mob that is known and not loaded.
fn resolved(resolver: &crate::unified_runtime::MobResolver, mob_id: &str) -> bool {
    resolver
        .resolved_for_in_tests()
        .iter()
        .any(|named| named.as_ref().is_some_and(|named| named.as_str() == mob_id))
}

/// A mob named beside a method of the host selects nothing. Every method
/// of the host ([`METHODS_OF_THE_HOST`]) answers an administrator's request
/// that names a second served mob with the bytes it answers the same
/// request that names no mob, on the request door and, for the upload, on
/// the multipart route; and the named mob is never resolved, so the
/// request loads nothing. The id is still judged like any other: a person
/// no rule lets address the second mob is answered the mob id's refusal,
/// exactly as for an id no mob has, and an administrator who names an id
/// no mob has is answered that refusal too.
#[tokio::test]
async fn a_mob_named_beside_a_method_of_the_host_selects_nothing() {
    let launch = runtime("launch").await;
    let second = runtime("second").await;
    let second_mob = second.mob_id();
    let absent_mob = an_id_no_mob_has(&second_mob);
    let (app, resolver) = console_of_two_mobs(&launch, &second);
    let naming_no_mob = naming_mob(None);
    let naming_the_second_mob = naming_mob(Some(&second_mob));
    let naming_the_absent_mob = naming_mob(Some(&absent_mob));

    let mut wrong = Vec::new();
    for method in METHODS_OF_THE_HOST {
        let (unnamed, named) = ask_both_as(
            &app,
            rpc_request(method, &naming_no_mob),
            rpc_request(method, &naming_the_second_mob),
            DOOR_ROWS_ADMIN,
        )
        .await;
        if named != unnamed {
            wrong.push(format!(
                "{method}: an administrator's request that names a second mob is not answered \
                 as one that names no mob: {named:?} / {unnamed:?}"
            ));
        }
        let (for_the_second_mob, for_the_absent_mob) = ask_both_as(
            &app,
            rpc_request(method, &naming_the_second_mob),
            rpc_request(method, &naming_the_absent_mob),
            DOOR_ROWS_WITHOUT_RULES,
        )
        .await;
        if !is_the_mob_ids_refusal("console RPC", &for_the_second_mob, &second_mob)
            || apart_from_the_id(&for_the_second_mob, &second_mob)
                != apart_from_the_id(&for_the_absent_mob, &absent_mob)
        {
            wrong.push(format!(
                "{method}: a person who may not address the second mob is not refused it as an \
                 id no mob has: {for_the_second_mob:?} / {for_the_absent_mob:?}"
            ));
        }
        let of_an_administrator = ask_as(
            &app,
            rpc_request(method, &naming_the_absent_mob),
            DOOR_ROWS_ADMIN,
        )
        .await;
        if !is_the_mob_ids_refusal("console RPC", &of_an_administrator, &absent_mob) {
            wrong.push(format!(
                "{method}: an administrator who names an id no mob has is not answered the mob \
                 id's refusal: {of_an_administrator:?}"
            ));
        }
    }
    let upload = "meta/blob/upload";
    let (unnamed, named) = ask_both_as(
        &app,
        multipart_request(upload, &naming_no_mob),
        multipart_request(upload, &naming_the_second_mob),
        DOOR_ROWS_ADMIN,
    )
    .await;
    if named != unnamed {
        wrong.push(format!(
            "{upload} on the multipart route: an administrator's request that names a second \
             mob is not answered as one that names no mob: {named:?} / {unnamed:?}"
        ));
    }
    if resolved(&resolver, &second_mob) {
        wrong.push("a method of the host resolved the mob named beside it".to_string());
    }

    drop(app);
    launch.shutdown().await;
    second.shutdown().await;
    assert!(
        wrong.is_empty(),
        "a mob named beside a method of the host does not select nothing:\n{}",
        wrong.join("\n")
    );
}

/// The status is served per mob: a request that names a second served mob
/// is answered that mob's own status, and no part of the launch mob's. The
/// second mob of the row was stopped and the launch mob runs; the
/// console's router holds the launch mob's detached jobs. A request that
/// names no mob, or the launch mob by its id, reads `running` true and
/// those jobs; a request that names the second mob reads `running` false
/// and no job of the launch mob, for an administrator and for a person
/// whose rule selects that mob alike (the method has no gate of its own).
#[tokio::test]
async fn the_status_of_a_second_mob_is_that_mobs_own() {
    let launch = runtime("launch").await;
    let second = runtime("second").await;
    let launch_mob = launch.mob_id();
    let second_mob = second.mob_id();
    second
        .mob_handle()
        .stop()
        .await
        .expect("the second mob stops");
    let jobs_of_the_launch_mob = json!({"awaiting": ["a-job-of-the-launch-mob"]});
    let state = ConsoleJsonState {
        job_health_projection: Some(Arc::new(std::sync::RwLock::new(Some(
            json!({"detached_jobs": jobs_of_the_launch_mob}),
        )))),
        ..door_rows_state(&launch, &second_mob)
    };
    let (app, _resolver) = console_of_two_mobs_over(state, &launch, &second);
    let status_as = |mob_id: Option<&str>, email: &'static str| {
        let app = app.clone();
        let params = naming_mob(mob_id);
        async move {
            let answer = ask_as(&app, rpc_request("meta/status", &params), email).await;
            serde_json::from_str::<Value>(&answer.2).unwrap_or(Value::Null)
        }
    };

    let mut wrong = Vec::new();
    let of_no_mob = status_as(None, DOOR_ROWS_ADMIN).await;
    if of_no_mob["result"]["running"] != json!(true)
        || of_no_mob["result"]["detached_jobs"] != jobs_of_the_launch_mob
    {
        wrong.push(format!(
            "a request that names no mob does not read the launch mob's status: {of_no_mob}"
        ));
    }
    let of_the_launch_mob = status_as(Some(&launch_mob), DOOR_ROWS_ADMIN).await;
    if of_the_launch_mob != of_no_mob {
        wrong.push(format!(
            "a request that names the launch mob by its id is not answered as one that names \
             no mob: {of_the_launch_mob} / {of_no_mob}"
        ));
    }
    let of_the_second_mob = status_as(Some(&second_mob), DOOR_ROWS_ADMIN).await;
    for (who, answer) in [
        ("an administrator", of_the_second_mob.clone()),
        (
            "a person whose rule selects the mob",
            status_as(Some(&second_mob), DOOR_ROWS_OF_THE_SECOND_MOB).await,
        ),
    ] {
        if !answer["error"].is_null() || answer["result"]["running"] != json!(false) {
            wrong.push(format!(
                "{who}: a request that names the second mob does not read that mob's status \
                 (it does not run): {answer}"
            ));
        }
        if answer.to_string().contains("a-job-of-the-launch-mob") {
            wrong.push(format!(
                "{who}: the second mob's status carries the launch mob's jobs: {answer}"
            ));
        }
        if answer["result"] != of_the_second_mob["result"] {
            wrong.push(format!(
                "{who}: is answered another status of the second mob than an administrator: \
                 {answer} / {of_the_second_mob}"
            ));
        }
    }

    drop(app);
    launch.shutdown().await;
    second.shutdown().await;
    assert!(
        wrong.is_empty(),
        "the status of a second mob is not that mob's own:\n{}",
        wrong.join("\n")
    );
}

/// A status request for a mob the caller may not address is answered
/// exactly as one for an id no mob has: the mob id's refusal, equal in
/// status, headers and body apart from the id the request named, naming no
/// other mob, and with nothing asked of the host about that mob. A caller
/// who may address the mob (a person whose rule selects it, an
/// administrator) is answered its status.
#[tokio::test]
async fn the_status_of_a_mob_the_caller_may_not_address_is_answered_as_that_of_an_id_no_mob_has() {
    let launch = runtime("launch").await;
    let second = runtime("second").await;
    let launch_mob = launch.mob_id();
    let second_mob = second.mob_id();
    let absent_mob = an_id_no_mob_has(&second_mob);
    let (app, resolver) = console_of_two_mobs(&launch, &second);
    let status = "meta/status";

    let mut wrong = Vec::new();
    for (who, email) in [
        ("a person no rule names", DOOR_ROWS_WITHOUT_RULES),
        (
            "a person a rule names through a group",
            DOOR_ROWS_IN_A_GROUP,
        ),
        ("a person whose rule lists no mob", DOOR_ROWS_OPERATOR),
    ] {
        let (for_the_second_mob, for_the_absent_mob) = ask_both_as(
            &app,
            rpc_request(status, &naming_mob(Some(&second_mob))),
            rpc_request(status, &naming_mob(Some(&absent_mob))),
            email,
        )
        .await;
        if !is_the_mob_ids_refusal("console RPC", &for_the_second_mob, &second_mob)
            || !is_the_mob_ids_refusal("console RPC", &for_the_absent_mob, &absent_mob)
        {
            wrong.push(format!(
                "{who}: is not answered the mob id's refusal: {for_the_second_mob:?} / \
                 {for_the_absent_mob:?}"
            ));
        }
        if apart_from_the_id(&for_the_second_mob, &second_mob)
            != apart_from_the_id(&for_the_absent_mob, &absent_mob)
        {
            wrong.push(format!(
                "{who}: is answered differently for the second mob and for an id no mob has: \
                 {for_the_second_mob:?} / {for_the_absent_mob:?}"
            ));
        }
        if for_the_second_mob.2.contains(&launch_mob) {
            wrong.push(format!(
                "{who}: the refusal names a mob the request did not name: {for_the_second_mob:?}"
            ));
        }
    }
    let asked_about_the_second_mob = asked_about(&resolver, &second_mob);
    if asked_about_the_second_mob != 0 {
        wrong.push(format!(
            "the resolver was asked about a mob the caller may not address: \
             {asked_about_the_second_mob} times"
        ));
    }
    for (who, email) in [
        (
            "a person whose rule selects the mob",
            DOOR_ROWS_OF_THE_SECOND_MOB,
        ),
        ("an administrator", DOOR_ROWS_ADMIN),
    ] {
        let answer = ask_as(
            &app,
            rpc_request(status, &naming_mob(Some(&second_mob))),
            email,
        )
        .await;
        let body: Value = serde_json::from_str(&answer.2).unwrap_or(Value::Null);
        if !body["error"].is_null() || !body["result"].is_object() {
            wrong.push(format!(
                "{who}: may address the second mob and is not answered its status: {answer:?}"
            ));
        }
    }

    drop(app);
    launch.shutdown().await;
    second.shutdown().await;
    assert!(
        wrong.is_empty(),
        "the status of a mob the caller may not address is not answered as that of an id no \
         mob has:\n{}",
        wrong.join("\n")
    );
}

/// The text of the function `name` in the console's file: from its head to
/// the end of its body. Empty when the file holds no such function.
fn console_function(name: &str) -> &'static str {
    let source = include_str!("http_console.rs");
    let Some(from) = source.find(&format!("fn {name}<")) else {
        return "";
    };
    let text = &source[from..];
    &text[..text.find("\n}\n").unwrap_or(text.len())]
}

/// On which console methods a request that names another mob is served is
/// two closed lists in the console's file, and nothing else: the methods
/// served per mob and the methods of the host. The lists cannot drift:
///
/// - the console's list of the host's methods names exactly the closed
///   list of this file ([`METHODS_OF_THE_HOST`]), whose every name
///   [`a_mob_named_beside_a_method_of_the_host_selects_nothing`] asks;
/// - the console's list of the methods served per mob names exactly the
///   methods a row tells two mobs apart on
///   ([`every_method_served_per_mob_tells_two_mobs_apart`]);
/// - each list is sorted and names a method once, no method is on both,
///   and every name is a console method that is neither served from the
///   launch mob's own wiring nor a method that carries a mob of its own
///   (an announcement method that names one, or a method over the mobs of
///   a host that names its mob itself);
/// - every other console method, and a name no list holds, as a method
///   added later is, is one
///   [`a_console_method_not_listed_as_served_per_mob_refuses_a_second_mob`]
///   asks: so a new method is on exactly one of the two lists, with what
///   that list's row asks of it, or is refused for another mob;
/// - the door that takes a request's mob reads both lists and takes the
///   caller of every other method that hands it an addressing id as one who
///   addresses the launch mob alone.
#[test]
fn every_console_method_is_on_one_of_the_two_lists_or_is_refused_for_another_mob() {
    let of_the_host = console_methods_of_the_host();
    let served_per_mob = console_methods_served_per_mob();
    let every_method = every_console_method();
    let refused = console_methods_refused_for_another_mob();
    let names_its_own_mob: Vec<&str> =
        crate::rpc::mob_addressing::ANNOUNCEMENT_METHODS_NAMING_A_MOB
            .iter()
            .chain(crate::rpc::mob_hosting::MOB_HOSTING_METHODS_NAMING_A_MOB)
            .copied()
            .collect();

    let mut found = Vec::new();
    if of_the_host != METHODS_OF_THE_HOST {
        found.push(format!(
            "the console lists {of_the_host:?} as the host's methods; the closed list is \
             {METHODS_OF_THE_HOST:?}"
        ));
    }
    let mut told_apart: Vec<&str> = TOLD_APART_HERE
        .iter()
        .chain(TOLD_APART_BY_ANOTHER_ROW)
        .map(|(method, _)| *method)
        .chain(workgraph_methods_told_apart())
        .collect();
    told_apart.sort_unstable();
    if served_per_mob != told_apart {
        found.push(format!(
            "the console lists {served_per_mob:?} as served per mob; the rows tell two mobs \
             apart on {told_apart:?}"
        ));
    }
    for (list, listed) in [
        ("the host's methods", &of_the_host),
        ("the methods served per mob", &served_per_mob),
    ] {
        if !listed.windows(2).all(|pair| pair[0] < pair[1]) {
            found.push(format!("{list}: not sorted, or a name twice"));
        }
        for method in listed {
            if !every_method.contains(method) {
                found.push(format!("{method}, of {list}: not a console method"));
            }
            if CONSOLE_METHODS_OF_THE_LAUNCH_MOBS_WIRING.contains(method) {
                found.push(format!(
                    "{method}, of {list}: served from the launch mob's own wiring"
                ));
            }
            if names_its_own_mob.contains(method) {
                found.push(format!("{method}, of {list}: carries a mob of its own"));
            }
        }
    }
    for method in &every_method {
        let places = [
            METHODS_OF_THE_HOST.contains(method),
            served_per_mob.contains(method),
            refused.contains(method),
            names_its_own_mob.contains(method),
        ];
        if places.iter().filter(|place| **place).count() != 1 {
            found.push(format!(
                "{method}: not on exactly one of the two lists, the refused methods and the \
                 methods that carry a mob of their own: {places:?}"
            ));
        }
    }
    if !refused.contains(&"meta/a_method_no_list_names") {
        found.push("a name no list holds is not among the refused methods".to_string());
    }
    let door = console_function("take_console_request_mob");
    for reads in [
        "console_method_is_served_per_mob(&request.method)",
        "console_method_is_of_the_host(&request.method)",
        "LaunchMobOnly(caller)",
    ] {
        if !door.contains(reads) {
            found.push(format!(
                "the door that takes a request's mob does not read `{reads}`"
            ));
        }
    }
    assert!(
        found.is_empty(),
        "the console's two lists of the methods served with another mob named:\n{}",
        found.join("\n")
    );
}

/// The audit read loads no mob. A row of a mob the host serves that is not
/// loaded names a member nothing recorded for this read: with every mob
/// selected it is shown to an administrator alone, whoever may address the
/// mob, and the read asks the
/// host's resolver to resolve no such mob (a resolve is what loads a mob).
/// Each mob has a shared agent `alice`.
#[tokio::test]
async fn the_audit_read_loads_no_mob_to_judge_its_rows() {
    let (_launch_state, launch) =
        runtime_with_labelled_member("launch", "alice", std::collections::BTreeMap::new()).await;
    let (_second_state, second) =
        runtime_with_labelled_member("second", "alice", std::collections::BTreeMap::new()).await;
    let second_mob = second.mob_id();
    let log = audit_log_holding(&[
        json!({"member_id": "alice"}),
        json!({"mob_id": second_mob, "member_id": "alice"}),
    ])
    .await;
    let state = ConsoleJsonState {
        console_audit: Some(log),
        ..door_rows_state(&launch, &second_mob)
    };
    let resolver = launch
        .mob_resolver()
        .serving_a_second_mob_in_tests(&second.mob_resolver())
        .not_loaded_in_tests();
    let app = console_json_router_with_state(state).layer(axum::Extension(resolver.clone()));
    let rows_as = |email: &'static str| {
        let app = app.clone();
        let second_mob = second_mob.clone();
        async move {
            let answer = ask_as(
                &app,
                rpc_request(crate::rpc::CONSOLE_AUDIT_METHOD, &audit_read_of_every_mob()),
                email,
            )
            .await;
            let body: Value = serde_json::from_str(&answer.2).unwrap_or(Value::Null);
            body["result"]["records"]
                .as_array()
                .map(|records| {
                    records
                        .iter()
                        .map(|row| row["target"]["mob_id"] == json!(second_mob.as_str()))
                        .collect::<Vec<bool>>()
                })
                .ok_or(answer)
        }
    };

    let as_a_caller_of_both_mobs = rows_as(DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB).await;
    let as_an_administrator = rows_as(DOOR_ROWS_ADMIN).await;
    let resolved_for_the_second_mob = resolver
        .resolved_for_in_tests()
        .into_iter()
        .flatten()
        .filter(|named| named.as_str() == second_mob.as_str())
        .count();
    drop(app);
    launch.shutdown().await;
    second.shutdown().await;
    assert_eq!(
        as_a_caller_of_both_mobs,
        Ok(vec![false]),
        "a caller who may address the mob and is no administrator reads the launch mob's row \
         alone while the mob is not loaded (each row: whether it is the second mob's)"
    );
    assert_eq!(as_an_administrator, Ok(vec![false, true]));
    assert_eq!(
        resolved_for_the_second_mob, 0,
        "the audit read asked for no resolve of the second mob"
    );
}

/// A runtime over one mob whose id starts with `tag`, whose host lets an
/// operator edit the topology, with the two members `alice` and `bob`, and
/// the state directory the runtime keeps.
async fn editable_runtime_with_alice_and_bob(
    tag: &str,
) -> (tempfile::TempDir, crate::UnifiedRuntime) {
    let state_dir = tempfile::tempdir().expect("a state directory");
    let definition = meerkat_mob::MobDefinition::from_toml(&format!(
        "[mob]\nid = \"console-topology-{tag}-{}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n\
         external_addressable = true\nruntime_mode = \"turn_driven\"\n\n\
         [profiles.worker.tools]\ncomms = true\n",
        uuid::Uuid::new_v4().simple()
    ))
    .expect("mob definition");
    let runtime = Box::pin(
        crate::UnifiedRuntime::builder()
            .definition(definition)
            .persistent_state(state_dir.path())
            .comms(true)
            .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
            .topology_control(crate::topology_control::TopologyControlPolicy {
                mode: crate::topology_control::TopologyControlMode::Editable,
                ..crate::topology_control::TopologyControlPolicy::default()
            })
            .expect("an editable topology policy")
            .build(),
    )
    .await
    .expect("runtime builds");
    for member in ["alice", "bob"] {
        runtime
            .mob_handle()
            .spawn_spec(SpawnMemberSpec::from_wire(
                "worker".to_string(),
                member.to_string(),
                None,
                None,
                None,
            ))
            .await
            .expect("the member is spawned");
    }
    (state_dir, runtime)
}

/// The params of a topology request that names the mob `mob_id`, or no
/// mob, with the members of `request` beside it.
fn topology_params(mob_id: Option<&str>, request: Value) -> Value {
    let mut params = request;
    if let Some(mob_id) = mob_id {
        params["mob_id"] = json!(mob_id);
    }
    params
}

/// The operations of a request that does `action` to the edge between
/// `alice` and `bob` of the mob the request is for.
fn alice_bob_operations(action: &str) -> Value {
    json!([{
        "action": action,
        "edge": {"a": {"identity": "alice"}, "b": {"identity": "bob"}},
    }])
}

/// The body of `answer`, or null.
fn body_of(answer: &DoorAnswer) -> Value {
    serde_json::from_str(&answer.2).unwrap_or(Value::Null)
}

/// Whether the topology `snapshot` holds an edge between `alice` and `bob`
/// that an operator added.
fn an_operator_connected_alice_and_bob(snapshot: &Value) -> bool {
    snapshot["edges"].as_array().is_some_and(|edges| {
        edges.iter().any(|edge| {
            let ends = [
                edge["edge"]["a"]["identity"].as_str(),
                edge["edge"]["b"]["identity"].as_str(),
            ];
            ends.contains(&Some("alice"))
                && ends.contains(&Some("bob"))
                && edge["operator_added"] == json!(true)
        })
    })
}

/// The console's topology methods tell two mobs apart. On a console of two
/// mobs whose members have the same names, an administrator's apply that
/// names the second mob connects that mob's two members and moves that
/// mob's revision; the launch mob's revision and wiring stay. After it,
/// each of the five methods answers for the mob its request names: the
/// query its own revision and edges, the plan its own base revision, the
/// operation read and the audit read the second mob's operation for that
/// mob alone, and a request that names no mob, or the launch mob by its
/// id, the launch mob's.
#[tokio::test]
async fn the_console_topology_methods_tell_two_mobs_apart() {
    let (_launch_state, launch) = editable_runtime_with_alice_and_bob("launch").await;
    let (_second_state, second) = editable_runtime_with_alice_and_bob("second").await;
    let launch_mob = launch.mob_id();
    let second_mob = second.mob_id();
    let (app, _resolver) = console_of_two_mobs(&launch, &second);
    let ask_topology = |method: &'static str, mob_id: Option<&str>, request: Value| {
        let app = app.clone();
        let params = topology_params(mob_id, request);
        async move { body_of(&ask_as(&app, rpc_request(method, &params), DOOR_ROWS_ADMIN).await) }
    };
    let launch_mob_or_none = [None, Some(launch_mob.as_str())];

    let mut wrong = Vec::new();
    let applied = ask_topology(
        "meta/topology/apply",
        Some(second_mob.as_str()),
        json!({
            "expected_revision": 0,
            "idempotency_key": "told-apart",
            "operations": alice_bob_operations("connect"),
        }),
    )
    .await;
    if applied["result"]["status"] != json!("applied") || applied["result"]["revision"] != json!(1)
    {
        wrong.push(format!(
            "an apply that names the second mob is not applied there: {applied}"
        ));
    }
    let operation_id = applied["result"]["operation_id"]
        .as_str()
        .unwrap_or("no-operation")
        .to_string();

    let of_the_second =
        ask_topology("meta/topology/query", Some(second_mob.as_str()), json!({})).await;
    if of_the_second["result"]["authority"] != json!(second_mob)
        || of_the_second["result"]["revision"] != json!(1)
        || !an_operator_connected_alice_and_bob(&of_the_second["result"])
    {
        wrong.push(format!(
            "a query that names the second mob does not read its revision and its operator's \
             edge: {of_the_second}"
        ));
    }
    for mob_id in launch_mob_or_none {
        let of_the_launch = ask_topology("meta/topology/query", mob_id, json!({})).await;
        if of_the_launch["result"]["authority"] != json!(launch_mob)
            || of_the_launch["result"]["revision"] != json!(0)
            || an_operator_connected_alice_and_bob(&of_the_launch["result"])
        {
            wrong.push(format!(
                "a query that names {mob_id:?} does not read the launch mob as it was: \
                 {of_the_launch}"
            ));
        }
    }

    let planned = ask_topology(
        "meta/topology/plan",
        Some(second_mob.as_str()),
        json!({"expected_revision": 1, "operations": alice_bob_operations("disconnect")}),
    )
    .await;
    if planned["result"]["authority"] != json!(second_mob)
        || planned["result"]["base_revision"] != json!(1)
    {
        wrong.push(format!(
            "a plan that names the second mob is not made at that mob's revision: {planned}"
        ));
    }
    for mob_id in launch_mob_or_none {
        let at_the_seconds_revision = ask_topology(
            "meta/topology/plan",
            mob_id,
            json!({"expected_revision": 1, "operations": alice_bob_operations("connect")}),
        )
        .await;
        if at_the_seconds_revision["error"].is_null() {
            wrong.push(format!(
                "a plan that names {mob_id:?} is made at the second mob's revision: \
                 {at_the_seconds_revision}"
            ));
        }
        let at_its_own = ask_topology(
            "meta/topology/plan",
            mob_id,
            json!({"expected_revision": 0, "operations": alice_bob_operations("connect")}),
        )
        .await;
        if at_its_own["result"]["authority"] != json!(launch_mob)
            || at_its_own["result"]["base_revision"] != json!(0)
        {
            wrong.push(format!(
                "a plan that names {mob_id:?} is not made at the launch mob's revision: \
                 {at_its_own}"
            ));
        }
    }

    let read = ask_topology(
        "meta/topology/operation/get",
        Some(second_mob.as_str()),
        json!({"operation_id": operation_id}),
    )
    .await;
    if read["result"]["operation_id"] != json!(operation_id) {
        wrong.push(format!(
            "the operation read that names the second mob does not find its operation: {read}"
        ));
    }
    let audited = ask_topology(
        "meta/topology/audit/query",
        Some(second_mob.as_str()),
        json!({}),
    )
    .await;
    let names_the_operation = |page: &Value| {
        page["result"]["records"].as_array().is_some_and(|records| {
            records
                .iter()
                .any(|record| record["operation_id"] == json!(operation_id))
        })
    };
    if !names_the_operation(&audited) {
        wrong.push(format!(
            "the audit read that names the second mob does not hold its operation: {audited}"
        ));
    }
    for mob_id in launch_mob_or_none {
        let read = ask_topology(
            "meta/topology/operation/get",
            mob_id,
            json!({"operation_id": operation_id}),
        )
        .await;
        if read["error"].is_null() {
            wrong.push(format!(
                "the operation read that names {mob_id:?} finds the second mob's operation: \
                 {read}"
            ));
        }
        let audited = ask_topology("meta/topology/audit/query", mob_id, json!({})).await;
        if audited["result"]["records"].as_array().is_none() || names_the_operation(&audited) {
            wrong.push(format!(
                "the audit read that names {mob_id:?} does not answer the launch mob's records \
                 alone: {audited}"
            ));
        }
    }

    let listed = console_methods_served_per_mob();
    for method in [
        "meta/topology/apply",
        "meta/topology/audit/query",
        "meta/topology/operation/get",
        "meta/topology/plan",
        "meta/topology/query",
    ] {
        if !listed.contains(&method) {
            wrong.push(format!("{method} is not listed as served per mob"));
        }
    }
    drop(app);
    launch.shutdown().await;
    second.shutdown().await;
    assert!(
        wrong.is_empty(),
        "a console topology method does not tell two mobs apart:\n{}",
        wrong.join("\n")
    );
}

/// The state of a console over `launch`'s mob that requires a sign-in and
/// has access rules about the topology of two mobs: one administrator; one
/// person whose allow of the topology actions lists no mob, so it holds in
/// the launch mob alone, and who may address the second mob by a rule that
/// lets it see that mob's agents; and one person whose allow of the same
/// actions selects the second mob.
fn topology_rows_state(launch: &crate::UnifiedRuntime, second_mob: &str) -> ConsoleJsonState {
    let topology_actions = vec![
        crate::access::ACTION_AGENT_VIEW.to_string(),
        crate::access::ACTION_TOPOLOGY_VIEW.to_string(),
        crate::access::ACTION_TOPOLOGY_AUDIT.to_string(),
        crate::access::ACTION_TOPOLOGY_CONNECT.to_string(),
        crate::access::ACTION_TOPOLOGY_DISCONNECT.to_string(),
        crate::access::ACTION_TOPOLOGY_RECONNECT.to_string(),
    ];
    let controller = AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![DOOR_ROWS_ADMIN.to_string()],
        rules: vec![
            crate::access::AccessRule {
                id: "edits-the-launch-mobs-topology".to_string(),
                subjects: vec![DOOR_ROWS_OPERATOR.to_string()],
                actions: topology_actions.clone(),
                ..crate::access::AccessRule::default()
            },
            crate::access::AccessRule {
                id: "sees-the-second-mobs-agents".to_string(),
                subjects: vec![DOOR_ROWS_OPERATOR.to_string()],
                actions: vec![crate::access::ACTION_AGENT_VIEW.to_string()],
                mobs: vec![second_mob.to_string()],
                ..crate::access::AccessRule::default()
            },
            crate::access::AccessRule {
                id: "edits-the-second-mobs-topology".to_string(),
                subjects: vec![DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB.to_string()],
                actions: topology_actions,
                mobs: vec![second_mob.to_string()],
                ..crate::access::AccessRule::default()
            },
        ],
        ..crate::access::AccessControlConfig::default()
    })
    .expect("access configuration");
    let decisions = crate::console_auth_config::parse_console_auth_config(&json!({
        "shared_secret": DOOR_ROWS_SIGNING_SECRET,
        "email_allowlist": [
            DOOR_ROWS_ADMIN,
            DOOR_ROWS_OPERATOR,
            DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB,
        ],
    }))
    .expect("console sign-in configuration");
    ConsoleJsonState {
        decisions,
        access: Some(controller),
        ..state_over(launch)
    }
}

/// A rule that lists no mob grants no topology action in a second mob. A
/// person whose allow of the topology actions lists no mob, and who may
/// address the second mob, reads and plans the launch mob's topology; for
/// the second mob that person's read shows no agent and no edge, the audit
/// read shows no record, and the plan and the apply are refused by the
/// door's own access rule, not as a mob the host does not have, and change
/// nothing there. A person whose allow selects the second mob reads its
/// topology, connects its members and reads the record and the operation
/// of it; the person who holds no topology action there is refused the
/// read of that operation by the access rule, and is told that an
/// operation that is not there was not found.
#[tokio::test]
async fn rule_without_mobs_grants_no_topology_action_in_a_second_mob() {
    let (_launch_state, launch) = editable_runtime_with_alice_and_bob("launch").await;
    let (_second_state, second) = editable_runtime_with_alice_and_bob("second").await;
    let second_mob = second.mob_id();
    let state = topology_rows_state(&launch, second_mob.as_str());
    let (app, _resolver) = console_of_two_mobs_over(state, &launch, &second);
    let connect = |key: &str| {
        json!({
            "expected_revision": 0,
            "idempotency_key": key,
            "operations": alice_bob_operations("connect"),
        })
    };
    // In this order: the apply is asked before the audit read, so the audit
    // read of a mob is asked when that mob holds a record.
    let requests = |key: &str| {
        [
            ("meta/topology/query", json!({})),
            (
                "meta/topology/plan",
                json!({"expected_revision": 0, "operations": alice_bob_operations("connect")}),
            ),
            ("meta/topology/apply", connect(key)),
            ("meta/topology/audit/query", json!({})),
        ]
    };
    let records_shown = |body: &Value| body["result"]["records"].as_array().map(Vec::len);

    let mut wrong = Vec::new();
    for (method, request) in requests("in-the-second-mob-without-a-rule") {
        let answer = ask_as(
            &app,
            rpc_request(method, &topology_params(Some(second_mob.as_str()), request)),
            DOOR_ROWS_OPERATOR,
        )
        .await;
        let body = body_of(&answer);
        if method == "meta/topology/query" {
            // A read shows a caller the agents it may inspect: none here.
            let shown = |list: &str| body["result"][list].as_array().map(Vec::len);
            if shown("nodes") != Some(0) || shown("edges") != Some(0) {
                wrong.push(format!(
                    "{method} for the second mob, asked by a person whose rule lists no mob: \
                     shows an agent or an edge of that mob: {answer:?}"
                ));
            }
            continue;
        }
        if method == "meta/topology/audit/query" {
            // The audit read shows a caller the records of the agents it
            // may inspect and audit in the request's mob: none here.
            if records_shown(&body) != Some(0) {
                wrong.push(format!(
                    "{method} for the second mob, asked by a person whose rule lists no mob: \
                     shows a record of that mob: {answer:?}"
                ));
            }
            continue;
        }
        if body["error"].is_null() || is_the_mob_ids_refusal("console RPC", &answer, &second_mob) {
            wrong.push(format!(
                "{method} for the second mob, asked by a person whose rule lists no mob: is not \
                 refused by the door's own access rule: {answer:?}"
            ));
        }
    }
    let untouched = body_of(
        &ask_as(
            &app,
            rpc_request(
                "meta/topology/query",
                &topology_params(Some(second_mob.as_str()), json!({})),
            ),
            DOOR_ROWS_ADMIN,
        )
        .await,
    );
    if untouched["result"]["revision"] != json!(0)
        || an_operator_connected_alice_and_bob(&untouched["result"])
    {
        wrong.push(format!(
            "the refused apply changed the second mob's topology: {untouched}"
        ));
    }
    for (method, request) in requests("in-the-launch-mob") {
        let answer = ask_as(
            &app,
            rpc_request(method, &topology_params(None, request)),
            DOOR_ROWS_OPERATOR,
        )
        .await;
        let body = body_of(&answer);
        if !body["error"].is_null() || body["result"].is_null() {
            wrong.push(format!(
                "{method} for the launch mob, asked by the person whose rule lists no mob: is \
                 not served: {answer:?}"
            ));
        }
    }
    let mut applied_in_the_second_mob = None;
    for (method, request) in requests("in-the-second-mob-by-its-rule") {
        let answer = ask_as(
            &app,
            rpc_request(method, &topology_params(Some(second_mob.as_str()), request)),
            DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB,
        )
        .await;
        let body = body_of(&answer);
        if !body["error"].is_null() || body["result"].is_null() {
            wrong.push(format!(
                "{method} for the second mob, asked by a person whose rule selects that mob: is \
                 not served: {answer:?}"
            ));
        }
        if method == "meta/topology/apply" {
            applied_in_the_second_mob = body["result"]["operation_id"].as_str().map(str::to_string);
        }
        if method == "meta/topology/audit/query" && records_shown(&body).is_none_or(|n| n == 0) {
            wrong.push(format!(
                "{method} for the second mob, asked by a person whose rule selects that mob: \
                 shows no record of that mob after its apply: {answer:?}"
            ));
        }
    }
    // The operation read of the second mob: the person whose rule selects
    // that mob reads its operation; the person who holds no topology action
    // there is refused by the access rule for an operation that is there,
    // and told that an operation that is not there was not found.
    let operation_id = applied_in_the_second_mob.unwrap_or_else(|| "no-operation".to_string());
    for (subject, operation_id, refusal) in [
        (
            DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB,
            operation_id.as_str(),
            None,
        ),
        (
            DOOR_ROWS_OPERATOR,
            operation_id.as_str(),
            Some("access_denied"),
        ),
        (
            DOOR_ROWS_OPERATOR,
            "an-operation-that-is-not-there",
            Some("topology_operation_not_found"),
        ),
    ] {
        let answer = ask_as(
            &app,
            rpc_request(
                "meta/topology/operation/get",
                &topology_params(
                    Some(second_mob.as_str()),
                    json!({"operation_id": operation_id}),
                ),
            ),
            subject,
        )
        .await;
        let body = body_of(&answer);
        let as_it_should = match refusal {
            None => body["result"]["operation_id"] == json!(operation_id),
            Some(kind) => body["result"].is_null() && body["error"]["data"]["kind"] == json!(kind),
        };
        if !as_it_should {
            wrong.push(format!(
                "meta/topology/operation/get for the second mob's operation {operation_id}, \
                 asked by {subject}: not answered with {refusal:?} (none: the operation): \
                 {answer:?}"
            ));
        }
    }

    drop(app);
    launch.shutdown().await;
    second.shutdown().await;
    assert!(
        wrong.is_empty(),
        "the topology actions of a second mob are not judged by that mob's own rules:\n{}",
        wrong.join("\n")
    );
}

/// A delivery record of the host in its wire form: the delivery
/// `delivery_id`, made for the mob `mob_id` names (for the host's default
/// mob when it names none).
fn delivery_rows_record(delivery_id: &str, mob_id: Option<&str>) -> Value {
    let mut record = json!({
        "delivery_id": delivery_id,
        "route_id": "route-1",
        "recipient": "desk",
        "sink": "webhook",
        "target_module": "delivery",
        "payload": { "text": "hello" },
        "status": "delivered",
        "attempts": [],
        "first_attempt_ms": 1,
        "final_attempt_ms": 1,
        "idempotency_key": null,
        "sink_adapter": null,
    });
    if let Some(mob_id) = mob_id {
        record["mob_id"] = json!(mob_id);
    }
    record
}

/// The console's delivery history is served per mob. On a console of two
/// mobs whose host made one delivery for each, an administrator's request
/// that names the second mob lists the second mob's delivery alone, and a
/// request that names no mob, or the launch mob by its id, lists the
/// launch mob's alone.
#[tokio::test]
async fn the_consoles_delivery_history_lists_the_deliveries_of_the_mob_it_names() {
    const OF_THE_LAUNCH_MOB: &str = "delivery-of-the-launch-mob";
    const OF_THE_SECOND_MOB: &str = "delivery-of-the-second-mob";
    let launch = runtime("launch").await;
    let second = runtime("second").await;
    let launch_mob = launch.mob_id();
    let second_mob = second.mob_id();
    let owner = crate::start_meta_runtime(
        crate::MetaConfig {
            modules: vec![],
            discovery: crate::DiscoverySpec {
                namespace: "delivery-history-two-mobs".to_string(),
                modules: vec![],
            },
            pre_spawn: vec![],
        },
        vec![],
        std::time::Duration::from_secs(1),
    )
    .expect("the host's module runtime");
    let owner = Arc::new(tokio::sync::Mutex::new(owner));
    {
        let mut owner = owner.lock().await;
        for (delivery_id, mob_id) in [
            (OF_THE_LAUNCH_MOB, None),
            (OF_THE_SECOND_MOB, Some(second_mob.as_str())),
        ] {
            owner.record_delivery_for_test(
                serde_json::from_value(delivery_rows_record(delivery_id, mob_id))
                    .expect("a delivery record"),
            );
        }
    }
    let mut state = door_rows_state(&launch, &second_mob);
    state.module_runtime = Some(Arc::clone(&owner));
    let (app, _resolver) = console_of_two_mobs_over(state, &launch, &second);

    let mut wrong = Vec::new();
    for (named, mob_id, expected) in [
        ("no mob", None, OF_THE_LAUNCH_MOB),
        (
            "the launch mob",
            Some(launch_mob.as_str()),
            OF_THE_LAUNCH_MOB,
        ),
        (
            "the second mob",
            Some(second_mob.as_str()),
            OF_THE_SECOND_MOB,
        ),
    ] {
        let mut params = json!({ "limit": 50 });
        if let Some(mob_id) = mob_id {
            params["mob_id"] = json!(mob_id);
        }
        let answer = ask_as(
            &app,
            rpc_request("meta/delivery/history", &params),
            DOOR_ROWS_ADMIN,
        )
        .await;
        let body: Value = serde_json::from_str(&answer.2).unwrap_or(Value::Null);
        let listed: Vec<&str> = body["result"]["deliveries"]
            .as_array()
            .map(|deliveries| {
                deliveries
                    .iter()
                    .filter_map(|delivery| delivery["delivery_id"].as_str())
                    .collect()
            })
            .unwrap_or_default();
        if listed != [expected] {
            wrong.push(format!(
                "a request that names {named}: lists {listed:?}, not {expected:?} alone: \
                 {answer:?}"
            ));
        }
    }

    drop(app);
    launch.shutdown().await;
    second.shutdown().await;
    assert!(
        wrong.is_empty(),
        "the delivery history does not list the deliveries of the mob it names:\n{}",
        wrong.join("\n")
    );
}

/// The ids of the deliveries the answer `answer` of `meta/delivery/history`
/// lists, in its order; `None` for an answer that is no listing.
fn delivery_rows_listed(answer: &DoorAnswer) -> Option<Vec<String>> {
    let body: Value = serde_json::from_str(&answer.2).ok()?;
    Some(
        body["result"]["deliveries"]
            .as_array()?
            .iter()
            .filter_map(|delivery| delivery["delivery_id"].as_str().map(str::to_string))
            .collect(),
    )
}

/// The console's delivery history of a second mob hides what that mob
/// hides from its caller. The second mob has an agent that is private to
/// another person, and the host made two deliveries for that mob: one to
/// that agent and one to a recipient that is no agent. A caller who may
/// address the second mob and holds its runtime administration, and who is
/// neither an administrator nor that person, is listed the second delivery
/// alone; an administrator is listed both.
#[tokio::test]
async fn the_consoles_delivery_history_of_a_second_mob_hides_a_delivery_to_an_agent_private_to_another_person()
 {
    const TO_THE_PRIVATE_AGENT: &str = "delivery-to-the-private-agent";
    const TO_NO_AGENT: &str = "delivery-to-no-agent";
    let launch = runtime("launch").await;
    let (_second_state, second) = runtime_with_labelled_member(
        "second",
        "alice",
        std::collections::BTreeMap::from([(
            crate::access::PRIVATE_TO_LABEL.to_string(),
            "another-person".to_string(),
        )]),
    )
    .await;
    let second_mob = second.mob_id();
    let owner = crate::start_meta_runtime(
        crate::MetaConfig {
            modules: vec![],
            discovery: crate::DiscoverySpec {
                namespace: "delivery-history-private-agent".to_string(),
                modules: vec![],
            },
            pre_spawn: vec![],
        },
        vec![],
        std::time::Duration::from_secs(1),
    )
    .expect("the host's module runtime");
    let owner = Arc::new(tokio::sync::Mutex::new(owner));
    {
        let mut owner = owner.lock().await;
        for (delivery_id, recipient) in [(TO_THE_PRIVATE_AGENT, "alice"), (TO_NO_AGENT, "desk")] {
            let mut record = delivery_rows_record(delivery_id, Some(second_mob.as_str()));
            record["recipient"] = json!(recipient);
            owner.record_delivery_for_test(
                serde_json::from_value(record).expect("a delivery record"),
            );
        }
    }
    let mut state = door_rows_state(&launch, &second_mob);
    state.module_runtime = Some(Arc::clone(&owner));
    let (app, _resolver) = console_of_two_mobs_over(state, &launch, &second);
    let history_as = |email: &'static str| {
        ask_as(
            &app,
            rpc_request(
                "meta/delivery/history",
                &json!({ "limit": 50, "mob_id": second_mob.as_str() }),
            ),
            email,
        )
    };

    let as_a_caller_kept_apart = history_as(DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB).await;
    let as_an_administrator = history_as(DOOR_ROWS_ADMIN).await;
    drop(app);
    launch.shutdown().await;
    second.shutdown().await;

    assert_eq!(
        delivery_rows_listed(&as_a_caller_kept_apart),
        Some(vec![TO_NO_AGENT.to_string()]),
        "a caller who is not the agent's person is not shown the delivery to it: \
         {as_a_caller_kept_apart:?}"
    );
    assert_eq!(
        delivery_rows_listed(&as_an_administrator),
        Some(vec![
            TO_THE_PRIVATE_AGENT.to_string(),
            TO_NO_AGENT.to_string()
        ]),
        "an administrator is shown both deliveries: {as_an_administrator:?}"
    );
}

/// The notice of an approval asked for an agent of a second mob is that
/// mob's delivery, and the console's delivery history judges it there. The
/// second mob has an agent that is private to another person and an agent
/// that is private to nobody; the launch mob has an agent of the private
/// agent's name that is private to nobody. The host keeps two notices for
/// the second mob (deliveries to the approvals address). Each is for a
/// request the host's gating owner holds, made with one of the second
/// mob's two agents as its origin, and no word of a notice names an agent:
/// the two differ only in their request's origin. The history shows a
/// notice as the pending list shows its request, by the origin's agent in
/// that mob. The launch mob's history lists neither notice; the second
/// mob's history lists both to an administrator; a caller who may address
/// that mob and is not the private agent's person is listed the shared
/// agent's notice alone. So the other is left out for its request's agent
/// in the second mob: not because it is a notice, not for a word of its
/// own, and not by the launch mob's agent of that name.
#[tokio::test]
async fn the_notice_of_an_approval_for_a_second_mobs_private_agent_is_judged_in_that_mob() {
    const NOTICE: &str = "delivery-of-the-approval-notice";
    const NOTICE_OF_A_SHARED_AGENT: &str = "delivery-of-a-shared-agents-approval-notice";
    const AN_ACTOR_THAT_IS_NO_AGENT: &str = "host-job";
    let (_launch_state, launch) =
        runtime_with_labelled_member("launch", "alice", std::collections::BTreeMap::new()).await;
    let (_second_state, second) = runtime_with_labelled_member(
        "second",
        "alice",
        std::collections::BTreeMap::from([(
            crate::access::PRIVATE_TO_LABEL.to_string(),
            "another-person".to_string(),
        )]),
    )
    .await;
    second
        .mob_handle()
        .spawn_spec(SpawnMemberSpec::from_wire(
            "worker".to_string(),
            "carol".to_string(),
            None,
            None,
            None,
        ))
        .await
        .expect("the shared agent is spawned");
    let second_mob = second.mob_id();
    let owner = crate::start_meta_runtime(
        crate::MetaConfig {
            modules: vec![],
            discovery: crate::DiscoverySpec {
                namespace: "delivery-history-approval-notice".to_string(),
                modules: vec![],
            },
            pre_spawn: vec![],
        },
        vec![],
        std::time::Duration::from_secs(1),
    )
    .expect("the host's module runtime");
    let owner = Arc::new(tokio::sync::Mutex::new(owner));
    {
        let mut owner = owner.lock().await;
        for (delivery_id, member) in [(NOTICE, "alice"), (NOTICE_OF_A_SHARED_AGENT, "carol")] {
            // The request the notice is for: an approval asked with the
            // member of the second mob as its origin. Its actor names no
            // agent, and neither does any word of the notice.
            let pending_id = owner
                .evaluate_gating_action_with_origin(
                    serde_json::from_value(json!({
                        "action": "publish_report",
                        "actor_id": AN_ACTOR_THAT_IS_NO_AGENT,
                        "risk_tier": "r3",
                        "approval_timeout_ms": 600_000,
                    }))
                    .expect("a gating request"),
                    Some(
                        serde_json::from_value(json!({
                            "identity": member,
                            "mob_id": second_mob.as_str(),
                        }))
                        .expect("an origin"),
                    ),
                )
                .pending_id
                .expect("the action waits for an approval");
            let mut record = delivery_rows_record(delivery_id, Some(second_mob.as_str()));
            record["recipient"] = json!("approvals@example.com");
            record["payload"] = json!({
                "kind": "gating_approval_request",
                "pending_id": pending_id,
                "action_id": format!("action-of-{delivery_id}"),
                "action": "publish_report",
                "actor_id": AN_ACTOR_THAT_IS_NO_AGENT,
                "risk_tier": "r3",
                "requested_approver": "bob",
                "deadline_at_ms": 1,
            });
            record["idempotency_key"] = json!(format!("gating-approval-{pending_id}"));
            owner.record_delivery_for_test(
                serde_json::from_value(record).expect("a delivery record"),
            );
        }
    }
    let mut state = door_rows_state(&launch, &second_mob);
    state.module_runtime = Some(Arc::clone(&owner));
    let (app, _resolver) = console_of_two_mobs_over(state, &launch, &second);
    let history_as = |email: &'static str, mob_id: Option<&str>| {
        let mut params = json!({ "limit": 50 });
        if let Some(mob_id) = mob_id {
            params["mob_id"] = json!(mob_id);
        }
        ask_as(&app, rpc_request("meta/delivery/history", &params), email)
    };

    let of_the_launch_mob = history_as(DOOR_ROWS_ADMIN, None).await;
    let of_the_second_mob_as_an_administrator =
        history_as(DOOR_ROWS_ADMIN, Some(second_mob.as_str())).await;
    let of_the_second_mob_as_a_caller_kept_apart = history_as(
        DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB,
        Some(second_mob.as_str()),
    )
    .await;
    drop(app);
    launch.shutdown().await;
    second.shutdown().await;

    assert_eq!(
        delivery_rows_listed(&of_the_launch_mob),
        Some(Vec::new()),
        "the launch mob's history lists nothing of the second mob's notice: {of_the_launch_mob:?}"
    );
    assert_eq!(
        delivery_rows_listed(&of_the_second_mob_as_an_administrator),
        Some(vec![
            NOTICE.to_string(),
            NOTICE_OF_A_SHARED_AGENT.to_string()
        ]),
        "an administrator is listed both notices in the second mob's history: \
         {of_the_second_mob_as_an_administrator:?}"
    );
    assert_eq!(
        delivery_rows_listed(&of_the_second_mob_as_a_caller_kept_apart),
        Some(vec![NOTICE_OF_A_SHARED_AGENT.to_string()]),
        "a caller who is not the private agent's person is listed the shared agent's notice \
         alone: {of_the_second_mob_as_a_caller_kept_apart:?}"
    );
}

/// A gating request made with no origin is the request's mob's. The host's
/// gating owner holds three such requests that wait: one whose audit rows
/// name the second mob (`request_mob_id` in a row's detail, as the stdio
/// door's evaluation for that mob writes it), one whose rows name no mob
/// (the launch mob's), and one none of whose rows the audit holds any more,
/// whose mob is not known. Three people hold the gating view and the gating
/// decision by a rule that lists no mob; a second rule selects the second
/// mob for one of them; an administrator.
///
/// On a console that serves the second mob: the pending list and the audit
/// read show the second mob's request to those who may address that mob,
/// the launch mob's to everyone, and the one of no known mob to the
/// administrator alone. A decide of the second mob's request by the person
/// who may not address that mob is answered as an id that is not pending,
/// and the request still waits.
///
/// On a console whose host serves the launch mob alone, as its resolver
/// states it, the request of no known mob is the launch mob's, as it has
/// always been. On a router that carries no resolver, which nothing told
/// which mobs its host has, it is the administrator's alone.
#[tokio::test]
async fn a_gating_request_with_no_origin_is_its_request_mobs() {
    let (_launch_state, launch) =
        runtime_with_labelled_member("launch", "alice", std::collections::BTreeMap::new()).await;
    let (_second_state, second) =
        runtime_with_labelled_member("second", "alice", std::collections::BTreeMap::new()).await;
    let second_mob = second.mob_id();
    let an_owner = || {
        crate::start_meta_runtime(
            crate::MetaConfig {
                modules: vec![],
                discovery: crate::DiscoverySpec {
                    namespace: "gating-request-mob".to_string(),
                    modules: vec![],
                },
                pre_spawn: vec![],
            },
            vec![],
            std::time::Duration::from_secs(1),
        )
        .expect("the host's module runtime")
    };
    // Three requests with no origin, then the stored state as an owner
    // keeps it: the rows of the first name the second mob, the rows of the
    // third are gone.
    let mut first = an_owner();
    let mut pend = || {
        let evaluated = first.evaluate_gating_action(
            serde_json::from_value(json!({
                "action": "publish_report",
                "actor_id": "host-job",
                "risk_tier": "r3",
                "approval_timeout_ms": 600_000,
            }))
            .expect("a gating request"),
        );
        (
            evaluated
                .pending_id
                .expect("the action waits for an approval"),
            evaluated.action_id,
        )
    };
    let (of_the_second_mob, action_of_the_second_mob) = pend();
    let (of_the_launch_mob, action_of_the_launch_mob) = pend();
    let (of_no_known_mob, action_of_no_known_mob) = pend();
    let mut stored = first.gating_state_snapshot();
    stored
        .audit
        .retain(|entry| entry.action_id != action_of_no_known_mob);
    for entry in &mut stored.audit {
        if entry.action_id == action_of_the_second_mob {
            entry.detail["request_mob_id"] = json!(second_mob.as_str());
        }
    }
    let mut owner = an_owner();
    owner
        .restore_gating_state(stored)
        .expect("the stored state is restored");
    let owner = Arc::new(tokio::sync::Mutex::new(owner));

    let gating = vec![
        crate::access::ACTION_GATING_VIEW.to_string(),
        crate::access::ACTION_GATING_DECIDE.to_string(),
    ];
    let access = || {
        AccessController::new(crate::access::AccessControlConfig {
            enabled: true,
            admins: vec![DOOR_ROWS_ADMIN.to_string()],
            rules: vec![
                crate::access::AccessRule {
                    id: "gating-of-the-launch-mob".to_string(),
                    subjects: vec![
                        DOOR_ROWS_OPERATOR.to_string(),
                        DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB.to_string(),
                    ],
                    actions: gating.clone(),
                    ..crate::access::AccessRule::default()
                },
                crate::access::AccessRule {
                    id: "gating-of-the-second-mob".to_string(),
                    subjects: vec![DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB.to_string()],
                    actions: gating.clone(),
                    mobs: vec![second_mob.clone()],
                    ..crate::access::AccessRule::default()
                },
            ],
            ..crate::access::AccessControlConfig::default()
        })
        .expect("access configuration")
    };
    let state = || {
        let mut state = door_rows_state(&launch, &second_mob);
        state.access = Some(access());
        state.module_runtime = Some(Arc::clone(&owner));
        state
    };
    let (of_two_mobs, _resolver) = console_of_two_mobs_over(state(), &launch, &second);
    // A router whose resolver is the launch's own: that host has no mob
    // beside its launch mob.
    let of_one_mob =
        console_json_router_with_state(state()).layer(axum::Extension(launch.mob_resolver()));
    // A router that carries no resolver: nothing told it which mobs its
    // host has.
    let of_no_resolver = console_json_router_with_state(state());

    // The pending requests a door lists, and the actions whose rows the
    // audit read serves; `None` for an answer that is no listing.
    let pending_listed = |answer: &DoorAnswer| -> Option<Vec<String>> {
        let body: Value = serde_json::from_str(&answer.2).ok()?;
        Some(
            body["result"]["pending"]
                .as_array()?
                .iter()
                .filter_map(|entry| entry["pending_id"].as_str().map(str::to_string))
                .collect(),
        )
    };
    let actions_served = |answer: &DoorAnswer| -> Option<std::collections::BTreeSet<String>> {
        let body: Value = serde_json::from_str(&answer.2).ok()?;
        Some(
            body["result"]["entries"]
                .as_array()?
                .iter()
                .filter_map(|entry| entry["action_id"].as_str().map(str::to_string))
                .collect(),
        )
    };
    let pending_of = |ids: &[&String]| Some(ids.iter().map(|id| (*id).clone()).collect::<Vec<_>>());
    let actions_of = |ids: &[&String]| {
        Some(
            ids.iter()
                .map(|id| (*id).clone())
                .collect::<std::collections::BTreeSet<_>>(),
        )
    };

    let mut wrong = Vec::new();
    for (console, app, email, shown, rows_shown) in [
        (
            "two mobs",
            &of_two_mobs,
            DOOR_ROWS_ADMIN,
            pending_of(&[&of_the_second_mob, &of_the_launch_mob, &of_no_known_mob]),
            actions_of(&[&action_of_the_second_mob, &action_of_the_launch_mob]),
        ),
        (
            "two mobs",
            &of_two_mobs,
            DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB,
            pending_of(&[&of_the_second_mob, &of_the_launch_mob]),
            actions_of(&[&action_of_the_second_mob, &action_of_the_launch_mob]),
        ),
        (
            "two mobs",
            &of_two_mobs,
            DOOR_ROWS_OPERATOR,
            pending_of(&[&of_the_launch_mob]),
            actions_of(&[&action_of_the_launch_mob]),
        ),
        // The second mob's request is withheld from a person who may not
        // address that mob, whatever the console serves.
        (
            "one mob",
            &of_one_mob,
            DOOR_ROWS_OPERATOR,
            pending_of(&[&of_the_launch_mob, &of_no_known_mob]),
            actions_of(&[&action_of_the_launch_mob]),
        ),
        // A router with no resolver was not told that its host serves no
        // other mob: the request of no known mob is the administrator's
        // alone there too.
        (
            "no resolver",
            &of_no_resolver,
            DOOR_ROWS_OPERATOR,
            pending_of(&[&of_the_launch_mob]),
            actions_of(&[&action_of_the_launch_mob]),
        ),
        (
            "no resolver",
            &of_no_resolver,
            DOOR_ROWS_ADMIN,
            pending_of(&[&of_the_second_mob, &of_the_launch_mob, &of_no_known_mob]),
            actions_of(&[&action_of_the_second_mob, &action_of_the_launch_mob]),
        ),
    ] {
        let pending = ask_as(app, rpc_request("meta/gating/pending", &json!({})), email).await;
        if pending_listed(&pending) != shown {
            wrong.push(format!(
                "a console of {console}, the pending list of {email}: {pending:?}"
            ));
        }
        let audit = ask_as(
            app,
            rpc_request("meta/gating/audit", &json!({ "limit": 200 })),
            email,
        )
        .await;
        if actions_served(&audit) != rows_shown {
            wrong.push(format!(
                "a console of {console}, the audit read of {email}: {audit:?}"
            ));
        }
    }

    // A decide of the second mob's request by the person who may not
    // address that mob: the answer of an id that is not pending.
    let decide = |pending_id: &str| {
        rpc_request(
            "meta/gating/decide",
            &json!({ "pending_id": pending_id, "decision": "approve" }),
        )
    };
    let of_an_id_that_is_not_pending =
        ask_as(&of_two_mobs, decide("no-such-request"), DOOR_ROWS_OPERATOR).await;
    let of_the_second_mobs_request =
        ask_as(&of_two_mobs, decide(&of_the_second_mob), DOOR_ROWS_OPERATOR).await;
    if of_the_second_mobs_request != of_an_id_that_is_not_pending {
        wrong.push(format!(
            "a decide of the second mob's request by a person who may not address that mob: \
             {of_the_second_mobs_request:?}, an id that is not pending: \
             {of_an_id_that_is_not_pending:?}"
        ));
    }
    let still_waits = owner
        .lock()
        .await
        .list_gating_pending()
        .iter()
        .any(|entry| entry.pending_id == of_the_second_mob);
    if !still_waits {
        wrong.push("the second mob's request no longer waits after that decide".to_string());
    }
    drop(of_two_mobs);
    drop(of_one_mob);
    launch.shutdown().await;
    second.shutdown().await;

    assert!(
        wrong.is_empty(),
        "a gating request with no origin is not served as its request's mob's:\n{}",
        wrong.join("\n")
    );
}

/// A gating request made for a mob the host created is a request of the
/// mob that stands under its id. A host keeps its mobs in files and created
/// two mobs beside its launch mob. Its gating owner takes requests with no
/// origin that wait: one for the launch mob, and one for each of the two
/// mobs. An access rule lists both mobs for one person, who holds the
/// gating view and the gating decision; an administrator.
///
/// While both mobs stand, that person is listed the three requests and
/// reads the rows of the three. The host destroys one of the two mobs: no
/// mob stands under its id, and that person is listed the other two
/// requests and reads their rows. The host creates a mob under the id, and
/// the owner takes a request for it: that person is listed it and reads
/// its rows, with the other two; a decide by that person of the request
/// made before the mob was created is answered as an id that is not
/// pending. An administrator reads the rows of every request throughout.
///
/// The host starts again over its files, so the two mobs are known and not
/// loaded, and the owner takes a request for each: that person is listed
/// both and reads the rows of both, and neither mob is loaded for it.
#[tokio::test]
async fn a_gating_request_for_a_mob_the_host_created_is_of_the_mob_that_stands_under_its_id() {
    use crate::rpc::mob_hosting_tests::{config, end, result, stored_host};

    let temp = tempfile::tempdir().expect("tempdir");
    let launch_mob = format!("door-rows-standing-{}", uuid::Uuid::new_v4().simple());
    let kept = format!("{launch_mob}-kept");
    let again = format!("{launch_mob}-again");
    let gating = vec![
        crate::access::ACTION_GATING_VIEW.to_string(),
        crate::access::ACTION_GATING_DECIDE.to_string(),
    ];
    let access = || {
        AccessController::new(crate::access::AccessControlConfig {
            enabled: true,
            admins: vec![DOOR_ROWS_ADMIN.to_string()],
            rules: vec![
                crate::access::AccessRule {
                    id: "gating-of-the-launch-mob".to_string(),
                    subjects: vec![
                        DOOR_ROWS_OPERATOR.to_string(),
                        DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB.to_string(),
                    ],
                    actions: gating.clone(),
                    ..crate::access::AccessRule::default()
                },
                crate::access::AccessRule {
                    id: "gating-of-the-two-mobs".to_string(),
                    subjects: vec![DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB.to_string()],
                    actions: gating.clone(),
                    mobs: vec![kept.clone(), again.clone()],
                    ..crate::access::AccessRule::default()
                },
            ],
            ..crate::access::AccessControlConfig::default()
        })
        .expect("access configuration")
    };
    // The console of `host`, whose gating owner is the host's own.
    let console_of = |host: &crate::UnifiedRuntime| {
        let mut state = door_rows_state(host, &again);
        state.access = Some(access());
        state.module_runtime = Some(host.module_runtime_handle());
        console_json_router_with_state(state).layer(axum::Extension(host.mob_resolver()))
    };
    // A request with no origin the host's gating owner takes now for the
    // mob `mob_id` (none: the launch mob): its pending id and action id.
    async fn request_for(host: &crate::UnifiedRuntime, mob_id: Option<&str>) -> (String, String) {
        let mob = mob_id.map(meerkat_mob::MobId::from);
        let owner = host.module_runtime_handle();
        let evaluated = owner.lock().await.evaluate_gating_action_of_mob(
            serde_json::from_value(json!({
                "action": "publish_report",
                "actor_id": "host-job",
                "risk_tier": "r3",
                "approval_timeout_ms": 600_000,
            }))
            .expect("a gating request"),
            mob.as_ref(),
        );
        (
            evaluated
                .pending_id
                .expect("the action waits for an approval"),
            evaluated.action_id,
        )
    }
    // The pending ids a door lists to `email` and the actions whose rows
    // its audit read serves; the answer itself for one that is no listing.
    async fn listed_to(
        app: &Router,
        email: &str,
    ) -> Result<
        (
            std::collections::BTreeSet<String>,
            std::collections::BTreeSet<String>,
        ),
        DoorAnswer,
    > {
        let of = |answer: &DoorAnswer, list: &str, member: &str| {
            let body: Value = serde_json::from_str(&answer.2).ok()?;
            Some(
                body["result"][list]
                    .as_array()?
                    .iter()
                    .filter_map(|entry| entry[member].as_str().map(str::to_string))
                    .collect::<std::collections::BTreeSet<_>>(),
            )
        };
        let pending = ask_as(app, rpc_request("meta/gating/pending", &json!({})), email).await;
        let audit = ask_as(
            app,
            rpc_request("meta/gating/audit", &json!({ "limit": 200 })),
            email,
        )
        .await;
        match (
            of(&pending, "pending", "pending_id"),
            of(&audit, "entries", "action_id"),
        ) {
            (Some(pending), Some(actions)) => Ok((pending, actions)),
            (None, _) => Err(pending),
            (_, None) => Err(audit),
        }
    }
    let both = |requests: &[&(String, String)]| {
        Ok::<_, DoorAnswer>((
            requests
                .iter()
                .map(|(pending_id, _)| pending_id.clone())
                .collect::<std::collections::BTreeSet<_>>(),
            requests
                .iter()
                .map(|(_, action_id)| action_id.clone())
                .collect::<std::collections::BTreeSet<_>>(),
        ))
    };
    let a_while = || tokio::time::sleep(std::time::Duration::from_millis(40));
    let mut wrong = Vec::new();

    let launch = stored_host(&temp, &launch_mob).await;
    for mob_id in [&kept, &again] {
        result(
            &launch,
            "meta/mobs/create",
            json!({"mob_config": config(mob_id)}),
        )
        .await;
    }
    a_while().await;
    let of_the_launch_mob = request_for(&launch, None).await;
    let of_the_kept_mob = request_for(&launch, Some(&kept)).await;
    let before = request_for(&launch, Some(&again)).await;
    let app = console_of(&launch);

    // Both mobs stand.
    let listed = listed_to(&app, DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB).await;
    if listed != both(&[&of_the_launch_mob, &of_the_kept_mob, &before]) {
        wrong.push(format!(
            "while both mobs stand, the person a rule lists them for: {listed:?}"
        ));
    }

    // No mob stands under the id of the mob the host destroyed.
    result(&launch, "meta/mobs/destroy", json!({"mob_id": again})).await;
    let listed = listed_to(&app, DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB).await;
    if listed != both(&[&of_the_launch_mob, &of_the_kept_mob]) {
        wrong.push(format!(
            "while no mob stands under the id, the person a rule lists it for: {listed:?}"
        ));
    }
    let to_an_administrator = listed_to(&app, DOOR_ROWS_ADMIN).await;
    if !to_an_administrator
        .as_ref()
        .is_ok_and(|(_, actions)| actions.contains(&before.1))
    {
        wrong.push(format!(
            "while no mob stands under the id, an administrator: {to_an_administrator:?}"
        ));
    }

    // A mob the host creates under the id, and a request for it.
    a_while().await;
    result(
        &launch,
        "meta/mobs/create",
        json!({"mob_config": config(&again)}),
    )
    .await;
    a_while().await;
    let since = request_for(&launch, Some(&again)).await;
    let listed = listed_to(&app, DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB).await;
    if listed != both(&[&of_the_launch_mob, &of_the_kept_mob, &since]) {
        wrong.push(format!(
            "once a mob stands under the id again, the person a rule lists it for: {listed:?}"
        ));
    }
    let to_an_administrator = listed_to(&app, DOOR_ROWS_ADMIN).await;
    if !to_an_administrator.as_ref().is_ok_and(|(_, actions)| {
        [&of_the_launch_mob, &of_the_kept_mob, &before, &since]
            .iter()
            .all(|(_, action_id)| actions.contains(action_id))
    }) {
        wrong.push(format!(
            "once a mob stands under the id again, an administrator: {to_an_administrator:?}"
        ));
    }
    let to_a_person_no_rule_lists_a_mob_for = listed_to(&app, DOOR_ROWS_OPERATOR).await;
    if to_a_person_no_rule_lists_a_mob_for != both(&[&of_the_launch_mob]) {
        wrong.push(format!(
            "a person no rule lists a mob for: {to_a_person_no_rule_lists_a_mob_for:?}"
        ));
    }
    // A decide of the request made before the mob was created: the answer
    // of an id that is not pending.
    {
        let decide = |pending_id: &str| {
            ask_as(
                &app,
                rpc_request(
                    "meta/gating/decide",
                    &json!({ "pending_id": pending_id, "decision": "approve" }),
                ),
                DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB,
            )
        };
        let of_an_id_that_is_not_pending = decide("no-such-request").await;
        let of_the_request_made_before = decide(&before.0).await;
        if of_the_request_made_before != of_an_id_that_is_not_pending {
            wrong.push(format!(
                "a decide of the request made before the mob was created: \
                 {of_the_request_made_before:?}, an id that is not pending: \
                 {of_an_id_that_is_not_pending:?}"
            ));
        }
    }

    // The host starts again: its two mobs are known and not loaded.
    drop(app);
    end(launch).await;
    let launch = stored_host(&temp, &launch_mob).await;
    let resolver = launch.mob_resolver();
    let loaded = |when: &str| {
        [&kept, &again]
            .iter()
            .filter(|mob_id| {
                resolver
                    .resolve_if_loaded(&meerkat_mob::MobId::from(mob_id.as_str()))
                    .is_some()
            })
            .map(|mob_id| format!("{mob_id} is loaded {when}"))
            .collect::<Vec<_>>()
    };
    wrong.extend(loaded("after the start"));
    let of_the_kept_mob = request_for(&launch, Some(&kept)).await;
    let of_the_mob_created_again = request_for(&launch, Some(&again)).await;
    let app = console_of(&launch);
    let listed = listed_to(&app, DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB).await;
    if listed != both(&[&of_the_kept_mob, &of_the_mob_created_again]) {
        wrong.push(format!(
            "of two mobs that are not loaded, the person a rule lists them for: {listed:?}"
        ));
    }
    wrong.extend(loaded("after the gating reads"));

    drop(app);
    end(launch).await;
    assert!(
        wrong.is_empty(),
        "the gating requests of a mob the host created, by the mob that stands under its id:\n{}",
        wrong.join("\n")
    );
}

/// A row of the console audit log that names a mob the host created is a
/// row of the mob that stands under its id. A host keeps its mobs in files
/// and created two mobs beside its launch mob. The console audit log holds
/// a change that names no mob and a change that names each of the two
/// mobs; none names a member. An access rule lists both mobs for one
/// person, who holds the audit view; an administrator.
///
/// While both mobs stand, that person reads the three rows. The host
/// destroys one of the two mobs: no mob stands under its id, and that
/// person reads the other two rows. The host creates a mob under the id,
/// and a change names it: that person reads its row, with the other two.
/// An administrator reads every row throughout, and a person no rule lists
/// a mob for the row that names no mob.
///
/// The host starts again over its files, so the two mobs are known and not
/// loaded: that person reads the same three rows, and neither mob is
/// loaded for it.
#[tokio::test]
async fn a_console_audit_row_of_a_mob_the_host_created_is_of_the_mob_that_stands_under_its_id() {
    use crate::rpc::mob_hosting_tests::{config, end, result, stored_host};

    let temp = tempfile::tempdir().expect("tempdir");
    let launch_mob = format!("door-rows-audit-standing-{}", uuid::Uuid::new_v4().simple());
    let kept = format!("{launch_mob}-kept");
    let again = format!("{launch_mob}-again");
    let audit_view = vec![crate::access::ACTION_AUDIT_VIEW.to_string()];
    let access = || {
        AccessController::new(crate::access::AccessControlConfig {
            enabled: true,
            admins: vec![DOOR_ROWS_ADMIN.to_string()],
            rules: vec![
                crate::access::AccessRule {
                    id: "audit-of-the-launch-mob".to_string(),
                    subjects: vec![
                        DOOR_ROWS_OPERATOR.to_string(),
                        DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB.to_string(),
                    ],
                    actions: audit_view.clone(),
                    ..crate::access::AccessRule::default()
                },
                crate::access::AccessRule {
                    id: "audit-of-the-two-mobs".to_string(),
                    subjects: vec![DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB.to_string()],
                    actions: audit_view.clone(),
                    mobs: vec![kept.clone(), again.clone()],
                    ..crate::access::AccessRule::default()
                },
            ],
            ..crate::access::AccessControlConfig::default()
        })
        .expect("access configuration")
    };
    // The host's console audit log, kept over the host's start again.
    let log = audit_log_holding(&[]).await;
    // The console of `host` over that log.
    let console_of = |host: &crate::UnifiedRuntime| {
        let mut state = door_rows_state(host, &again);
        state.access = Some(access());
        state.console_audit = Some(Arc::clone(&log));
        console_json_router_with_state(state).layer(axum::Extension(host.mob_resolver()))
    };
    // A change row the log takes now that names the mob `mob_id` (none: no
    // mob) and no member: its sequence number.
    async fn change_naming(
        log: &crate::console_audit::ConsoleAuditLog,
        mob_id: Option<&str>,
    ) -> u64 {
        let actor = crate::console_audit::ConsoleAuditActor {
            kind: "person",
            principal: Some(DOOR_ROWS_ADMIN.to_string()),
            issuer: None,
            subject: None,
            assurance: None,
            login_scope_digest: None,
        };
        log.record(crate::console_audit::ConsoleAuditEntry::answered(
            actor,
            "meta/mob_labels/set".to_string(),
            crate::console_audit::audit_target(&naming_mob(mob_id)),
            &json!({"jsonrpc": "2.0", "id": "row", "result": {}}),
        ))
        .await
        .expect("the row is written")
        .seq
    }
    // The sequence numbers of the change rows `email` reads with every mob
    // selected; the answer itself for a read that is no page.
    async fn rows_read_by(
        app: &Router,
        email: &str,
    ) -> Result<std::collections::BTreeSet<u64>, DoorAnswer> {
        let answer = ask_as(
            app,
            rpc_request(crate::rpc::CONSOLE_AUDIT_METHOD, &audit_read_of_every_mob()),
            email,
        )
        .await;
        let rows = serde_json::from_str::<Value>(&answer.2)
            .ok()
            .and_then(|body| {
                Some(
                    body["result"]["records"]
                        .as_array()?
                        .iter()
                        .filter_map(|row| row["seq"].as_u64())
                        .collect::<std::collections::BTreeSet<_>>(),
                )
            });
        rows.ok_or(answer)
    }
    let rows = |seqs: &[u64]| {
        Ok::<_, DoorAnswer>(
            seqs.iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>(),
        )
    };
    let a_while = || tokio::time::sleep(std::time::Duration::from_millis(40));
    let mut wrong = Vec::new();

    let launch = stored_host(&temp, &launch_mob).await;
    for mob_id in [&kept, &again] {
        result(
            &launch,
            "meta/mobs/create",
            json!({"mob_config": config(mob_id)}),
        )
        .await;
    }
    a_while().await;
    let of_no_mob = change_naming(&log, None).await;
    let of_the_kept_mob = change_naming(&log, Some(&kept)).await;
    let before = change_naming(&log, Some(&again)).await;
    let app = console_of(&launch);

    // Both mobs stand.
    let read = rows_read_by(&app, DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB).await;
    if read != rows(&[of_no_mob, of_the_kept_mob, before]) {
        wrong.push(format!(
            "while both mobs stand, the person a rule lists them for reads {read:?}"
        ));
    }

    // No mob stands under the id of the mob the host destroyed.
    result(&launch, "meta/mobs/destroy", json!({"mob_id": again})).await;
    let read = rows_read_by(&app, DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB).await;
    if read != rows(&[of_no_mob, of_the_kept_mob]) {
        wrong.push(format!(
            "while no mob stands under the id, the person a rule lists it for reads {read:?}"
        ));
    }
    let read = rows_read_by(&app, DOOR_ROWS_ADMIN).await;
    if read != rows(&[of_no_mob, of_the_kept_mob, before]) {
        wrong.push(format!(
            "while no mob stands under the id, an administrator reads {read:?}"
        ));
    }

    // A mob the host creates under the id, and a change that names it.
    a_while().await;
    result(
        &launch,
        "meta/mobs/create",
        json!({"mob_config": config(&again)}),
    )
    .await;
    a_while().await;
    let since = change_naming(&log, Some(&again)).await;
    for (who, email, shown) in [
        (
            "the person a rule lists it for",
            DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB,
            vec![of_no_mob, of_the_kept_mob, since],
        ),
        (
            "an administrator",
            DOOR_ROWS_ADMIN,
            vec![of_no_mob, of_the_kept_mob, before, since],
        ),
        (
            "a person no rule lists a mob for",
            DOOR_ROWS_OPERATOR,
            vec![of_no_mob],
        ),
    ] {
        let read = rows_read_by(&app, email).await;
        if read != rows(&shown) {
            wrong.push(format!(
                "once a mob stands under the id again, {who} reads {read:?}, not {shown:?}"
            ));
        }
    }

    // The host starts again: its two mobs are known and not loaded.
    drop(app);
    end(launch).await;
    let launch = stored_host(&temp, &launch_mob).await;
    let resolver = launch.mob_resolver();
    let loaded = |when: &str| {
        [&kept, &again]
            .iter()
            .filter(|mob_id| {
                resolver
                    .resolve_if_loaded(&meerkat_mob::MobId::from(mob_id.as_str()))
                    .is_some()
            })
            .map(|mob_id| format!("{mob_id} is loaded {when}"))
            .collect::<Vec<_>>()
    };
    wrong.extend(loaded("after the start"));
    let app = console_of(&launch);
    let read = rows_read_by(&app, DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB).await;
    if read != rows(&[of_no_mob, of_the_kept_mob, since]) {
        wrong.push(format!(
            "of two mobs that are not loaded, the person a rule lists them for reads {read:?}"
        ));
    }
    wrong.extend(loaded("after the audit read"));

    drop(app);
    end(launch).await;
    assert!(
        wrong.is_empty(),
        "the console audit rows of a mob the host created, by the mob that stands under its \
         id:\n{}",
        wrong.join("\n")
    );
}

/// The rule of the row above holds for a row that names a member and for
/// the log's other record set, the tool calls. A host created two mobs
/// beside its launch mob, and an access rule lists both for one person who
/// holds the audit view. The log holds a change that names a member of
/// each of the two mobs and a tool call that names each mob.
///
/// While both mobs stand, that person reads the two changes and the two
/// tool calls. The host destroys one mob and creates a mob under its id; a
/// change names a member of it and a tool call names it. That person reads
/// the rows written since that mob was created, with the other mob's; an
/// administrator reads every row of both sets.
#[tokio::test]
async fn a_member_row_and_a_tool_call_row_of_a_mob_are_of_the_mob_that_stands_under_its_id() {
    use crate::console_audit::ConsoleAuditSet;
    use crate::rpc::mob_hosting_tests::{config, end, result, stored_host};

    let temp = tempfile::tempdir().expect("tempdir");
    let launch_mob = format!("door-rows-audit-sets-{}", uuid::Uuid::new_v4().simple());
    let kept = format!("{launch_mob}-kept");
    let again = format!("{launch_mob}-again");
    let audit_view = vec![crate::access::ACTION_AUDIT_VIEW.to_string()];
    let access = AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![DOOR_ROWS_ADMIN.to_string()],
        rules: vec![
            crate::access::AccessRule {
                id: "audit-of-the-launch-mob".to_string(),
                subjects: vec![DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB.to_string()],
                actions: audit_view.clone(),
                ..crate::access::AccessRule::default()
            },
            crate::access::AccessRule {
                id: "audit-of-the-two-mobs".to_string(),
                subjects: vec![DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB.to_string()],
                actions: audit_view,
                mobs: vec![kept.clone(), again.clone()],
                ..crate::access::AccessRule::default()
            },
        ],
        ..crate::access::AccessControlConfig::default()
    })
    .expect("access configuration");
    let log = audit_log_holding(&[]).await;
    // A row the record set `set` takes now that names the mob `mob_id`
    // and, in the changes, a member of it: its sequence number in its set.
    async fn row_naming(
        log: &crate::console_audit::ConsoleAuditLog,
        set: ConsoleAuditSet,
        mob_id: &str,
    ) -> u64 {
        let actor = crate::console_audit::ConsoleAuditActor {
            kind: "person",
            principal: Some(DOOR_ROWS_ADMIN.to_string()),
            issuer: None,
            subject: None,
            assurance: None,
            login_scope_digest: None,
        };
        let params = match set {
            ConsoleAuditSet::Changes => json!({"mob_id": mob_id, "member_id": "worker-1"}),
            ConsoleAuditSet::ToolCalls => json!({"mob_id": mob_id}),
        };
        log.record_in(
            set,
            crate::console_audit::ConsoleAuditEntry::answered(
                actor,
                "meta/retire_member".to_string(),
                crate::console_audit::audit_target(&params),
                &json!({"jsonrpc": "2.0", "id": "row", "result": {}}),
            ),
        )
        .await
        .expect("the row is written")
        .seq
    }
    // The sequence numbers of the rows of `set` that `email` reads with
    // every mob selected; the answer itself for a read that is no page.
    async fn rows_read_by(
        app: &Router,
        set: &str,
        email: &str,
    ) -> Result<std::collections::BTreeSet<u64>, DoorAnswer> {
        let answer = ask_as(
            app,
            rpc_request(
                crate::rpc::CONSOLE_AUDIT_METHOD,
                &json!({"limit": 500, "mobs": "*", "log": set}),
            ),
            email,
        )
        .await;
        let rows = serde_json::from_str::<Value>(&answer.2)
            .ok()
            .and_then(|body| {
                Some(
                    body["result"]["records"]
                        .as_array()?
                        .iter()
                        .filter_map(|row| row["seq"].as_u64())
                        .collect::<std::collections::BTreeSet<_>>(),
                )
            });
        rows.ok_or(answer)
    }
    let rows = |seqs: &[u64]| {
        Ok::<_, DoorAnswer>(
            seqs.iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>(),
        )
    };
    let a_while = || tokio::time::sleep(std::time::Duration::from_millis(40));
    let mut wrong = Vec::new();

    let launch = stored_host(&temp, &launch_mob).await;
    for mob_id in [&kept, &again] {
        result(
            &launch,
            "meta/mobs/create",
            json!({"mob_config": config(mob_id)}),
        )
        .await;
    }
    a_while().await;
    let mut before = Vec::new();
    for set in [ConsoleAuditSet::Changes, ConsoleAuditSet::ToolCalls] {
        before.push((
            row_naming(&log, set, &kept).await,
            row_naming(&log, set, &again).await,
        ));
    }
    let mut state = door_rows_state(&launch, &again);
    state.access = Some(access);
    state.console_audit = Some(Arc::clone(&log));
    let app = console_json_router_with_state(state).layer(axum::Extension(launch.mob_resolver()));

    // Both mobs stand.
    for (set, (of_the_kept_mob, of_the_other)) in ["changes", "tool_calls"].into_iter().zip(&before)
    {
        let read = rows_read_by(&app, set, DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB).await;
        if read != rows(&[*of_the_kept_mob, *of_the_other]) {
            wrong.push(format!(
                "while both mobs stand, the person a rule lists them for reads of {set}: {read:?}"
            ));
        }
    }

    // The host destroys one mob and creates a mob under its id; a row of
    // each set names it.
    result(&launch, "meta/mobs/destroy", json!({"mob_id": again})).await;
    a_while().await;
    result(
        &launch,
        "meta/mobs/create",
        json!({"mob_config": config(&again)}),
    )
    .await;
    a_while().await;
    let since = [
        row_naming(&log, ConsoleAuditSet::Changes, &again).await,
        row_naming(&log, ConsoleAuditSet::ToolCalls, &again).await,
    ];
    for ((set, (of_the_kept_mob, earlier)), since) in ["changes", "tool_calls"]
        .into_iter()
        .zip(&before)
        .zip(since)
    {
        let read = rows_read_by(&app, set, DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB).await;
        if read != rows(&[*of_the_kept_mob, since]) {
            wrong.push(format!(
                "once a mob stands under the id again, the person a rule lists it for reads of \
                 {set}: {read:?}, not {:?}",
                [*of_the_kept_mob, since]
            ));
        }
        let read = rows_read_by(&app, set, DOOR_ROWS_ADMIN).await;
        if read != rows(&[*of_the_kept_mob, *earlier, since]) {
            wrong.push(format!(
                "once a mob stands under the id again, an administrator reads of {set}: {read:?}"
            ));
        }
    }

    drop(app);
    end(launch).await;
    assert!(
        wrong.is_empty(),
        "the member rows and the tool call rows of a mob the host created, by the mob that \
         stands under its id:\n{}",
        wrong.join("\n")
    );
}

/// A row of a mob that is not loaded is an administrator's alone also where
/// access is not enforced. The console requires a sign-in, so it tells its
/// people apart, and its access rules are switched off. Nothing says who a
/// member of a mob that is not loaded is private to, so a row that names
/// one is shown to no caller the console keeps apart from other people: a
/// person who is no administrator reads the launch mob's row alone, an
/// administrator reads both, and the read still asks for no resolve of the
/// mob. Each mob has a shared agent `alice`.
#[tokio::test]
async fn a_row_of_a_mob_that_is_not_loaded_is_an_administrators_alone_where_access_is_not_enforced()
{
    let (_launch_state, launch) =
        runtime_with_labelled_member("launch", "alice", std::collections::BTreeMap::new()).await;
    let (_second_state, second) =
        runtime_with_labelled_member("second", "alice", std::collections::BTreeMap::new()).await;
    let second_mob = second.mob_id();
    let log = audit_log_holding(&[
        json!({"member_id": "alice"}),
        json!({"mob_id": second_mob, "member_id": "alice"}),
    ])
    .await;
    let not_enforced = AccessController::new(crate::access::AccessControlConfig {
        enabled: false,
        admins: vec![DOOR_ROWS_ADMIN.to_string()],
        ..crate::access::AccessControlConfig::default()
    })
    .expect("access configuration");
    let state = ConsoleJsonState {
        console_audit: Some(log),
        access: Some(not_enforced),
        ..door_rows_state(&launch, &second_mob)
    };
    let resolver = launch
        .mob_resolver()
        .serving_a_second_mob_in_tests(&second.mob_resolver())
        .not_loaded_in_tests();
    let app = console_json_router_with_state(state).layer(axum::Extension(resolver.clone()));
    let rows_as = |email: &'static str| {
        let app = app.clone();
        let second_mob = second_mob.clone();
        async move {
            let answer = ask_as(
                &app,
                rpc_request(crate::rpc::CONSOLE_AUDIT_METHOD, &audit_read_of_every_mob()),
                email,
            )
            .await;
            let body: Value = serde_json::from_str(&answer.2).unwrap_or(Value::Null);
            body["result"]["records"]
                .as_array()
                .map(|records| {
                    records
                        .iter()
                        .map(|row| row["target"]["mob_id"] == json!(second_mob.as_str()))
                        .collect::<Vec<bool>>()
                })
                .ok_or(answer)
        }
    };

    let as_a_person_who_is_no_administrator = rows_as(DOOR_ROWS_WITHOUT_RULES).await;
    let as_an_administrator = rows_as(DOOR_ROWS_ADMIN).await;
    let resolved_for_the_second_mob = resolver
        .resolved_for_in_tests()
        .into_iter()
        .flatten()
        .filter(|named| named.as_str() == second_mob.as_str())
        .count();
    drop(app);
    launch.shutdown().await;
    second.shutdown().await;
    assert_eq!(
        as_a_person_who_is_no_administrator,
        Ok(vec![false]),
        "a person who is no administrator reads the launch mob's row alone while the mob is not \
         loaded, also where access is not enforced (each row: whether it is the second mob's)"
    );
    assert_eq!(as_an_administrator, Ok(vec![false, true]));
    assert_eq!(
        resolved_for_the_second_mob, 0,
        "the audit read asked for no resolve of the second mob"
    );
}

/// A row of a mob the host does not serve, and that no member of the
/// console's mob created, is an administrator's alone for a caller the
/// console keeps apart from other people. The log holds a row
/// of a mob the host served when the row was written and serves no longer.
/// Nothing says who the member that row names was private to, so the row
/// is shown to no caller the console keeps apart, as a row of a mob that
/// is not loaded is, and it is never judged by the launch mob's agent of
/// the same name. The launch mob has a shared agent `alice`; the console
/// requires a sign-in and its access rules are switched off, so a person
/// who is no administrator reads the launch mob's row alone and an
/// administrator reads both.
#[tokio::test]
async fn a_row_of_a_mob_the_host_does_not_serve_is_an_administrators_alone_for_a_caller_kept_apart()
{
    const ENDED_MOB: &str = "a-mob-the-host-serves-no-longer";
    let (_launch_state, launch) =
        runtime_with_labelled_member("launch", "alice", std::collections::BTreeMap::new()).await;
    let log = audit_log_holding(&[
        json!({"member_id": "alice"}),
        json!({"mob_id": ENDED_MOB, "member_id": "alice"}),
    ])
    .await;
    let not_enforced = AccessController::new(crate::access::AccessControlConfig {
        enabled: false,
        admins: vec![DOOR_ROWS_ADMIN.to_string()],
        ..crate::access::AccessControlConfig::default()
    })
    .expect("access configuration");
    let state = ConsoleJsonState {
        console_audit: Some(log),
        access: Some(not_enforced),
        ..door_rows_state(&launch, ENDED_MOB)
    };
    let app = console_json_router_with_state(state).layer(axum::Extension(launch.mob_resolver()));
    // The rows a read holds, each as whether it is the row of the mob the
    // host does not serve.
    let rows_as = |email: &'static str| {
        let app = app.clone();
        async move {
            let answer = ask_as(
                &app,
                rpc_request(crate::rpc::CONSOLE_AUDIT_METHOD, &audit_read_of_every_mob()),
                email,
            )
            .await;
            let body: Value = serde_json::from_str(&answer.2).unwrap_or(Value::Null);
            body["result"]["records"]
                .as_array()
                .map(|records| {
                    records
                        .iter()
                        .map(|row| row["target"]["mob_id"] == json!(ENDED_MOB))
                        .collect::<Vec<bool>>()
                })
                .ok_or(answer)
        }
    };

    let as_a_person_who_is_no_administrator = rows_as(DOOR_ROWS_WITHOUT_RULES).await;
    let as_an_administrator = rows_as(DOOR_ROWS_ADMIN).await;
    drop(app);
    launch.shutdown().await;
    assert_eq!(
        as_a_person_who_is_no_administrator,
        Ok(vec![false]),
        "a person who is no administrator reads the launch mob's row alone (each row: whether it \
         is the row of the mob the host does not serve)"
    );
    assert_eq!(as_an_administrator, Ok(vec![false, true]));
}

/// The WorkGraph methods the console serves beside the snapshot: every
/// method of the family. Each is served per mob, and
/// [`the_workgraph_methods_read_and_change_the_graph_of_the_mob_a_request_names`]
/// tells the two mobs apart on each.
fn workgraph_methods_told_apart() -> Vec<&'static str> {
    let mut methods: Vec<&'static str> = crate::rpc::workgraph_methods::WORKGRAPH_READ_METHODS
        .iter()
        .chain(crate::rpc::workgraph_methods::WORKGRAPH_MUTATE_METHODS)
        .chain(crate::rpc::workgraph_methods::WORKGRAPH_CONSOLE_MUTATE_METHODS)
        .copied()
        .filter(|method| *method != "meta/workgraph/snapshot")
        .collect();
    methods.sort_unstable();
    methods
}

/// The params the row below asks a WorkGraph method with, beside the mob.
/// `ITEM` stands for the id of an item, `GOAL` for the id of a goal's item
/// and `BINDING` for the id of that goal's attention binding: all three are
/// of the second mob's graph alone. A change is asked with a revision no
/// item has, so it changes nothing: what tells the mobs apart is whether
/// the graph that was asked holds what the request names. The two methods
/// that create (an item, a goal) are not here: the row creates through
/// them. The goal confirmation is not here either: only a signed-in person
/// confirms, and
/// [`a_goal_confirmation_is_made_in_the_graph_of_the_mob_its_request_names`]
/// asks it as one.
const WORKGRAPH_REQUESTS: &[(&str, &str)] = &[
    (
        "meta/workgraph/attention/break_glass_reassign",
        r#"{"binding_id":"BINDING","expected_revision":987654,"reason":"door rows",
            "target":{"kind":"owner","owner_key":{"kind":"principal","id":"door-rows-other"}}}"#,
    ),
    ("meta/workgraph/attention/list", "{}"),
    (
        "meta/workgraph/attention/pause",
        r#"{"binding_id":"BINDING","expected_revision":987654}"#,
    ),
    ("meta/workgraph/attention/prune", "{}"),
    (
        "meta/workgraph/attention/reassign",
        r#"{"binding_id":"BINDING","expected_revision":987654,
            "target":{"kind":"owner","owner_key":{"kind":"principal","id":"door-rows-other"}}}"#,
    ),
    (
        "meta/workgraph/attention/resume",
        r#"{"binding_id":"BINDING","expected_revision":987654}"#,
    ),
    (
        "meta/workgraph/block",
        r#"{"id":"ITEM","expected_revision":987654}"#,
    ),
    (
        "meta/workgraph/claim",
        r#"{"id":"ITEM","expected_revision":987654}"#,
    ),
    (
        "meta/workgraph/close",
        r#"{"id":"ITEM","expected_revision":987654,"status":"completed"}"#,
    ),
    ("meta/workgraph/events", "{}"),
    (
        "meta/workgraph/evidence/add",
        r#"{"id":"ITEM","expected_revision":987654,"evidence":{"kind":"note","id":"door-rows"}}"#,
    ),
    ("meta/workgraph/facts", "{}"),
    ("meta/workgraph/get", r#"{"id":"ITEM"}"#),
    (
        "meta/workgraph/goal/request_close",
        r#"{"binding_id":"BINDING","expected_revision":987654}"#,
    ),
    ("meta/workgraph/goal/status", r#"{"binding_id":"BINDING"}"#),
    (
        "meta/workgraph/link",
        r#"{"kind":"related","from_id":"ITEM","to_id":"GOAL"}"#,
    ),
    ("meta/workgraph/list", "{}"),
    (
        "meta/workgraph/policy/escalate",
        r#"{"id":"ITEM","expected_revision":987654,"binding_id":"BINDING",
            "completion_policy":{"kind":"host_confirmed"}}"#,
    ),
    ("meta/workgraph/ready", "{}"),
    (
        "meta/workgraph/release",
        r#"{"id":"ITEM","expected_revision":987654}"#,
    ),
    (
        "meta/workgraph/update",
        r#"{"id":"ITEM","expected_revision":987654,"title":"renamed"}"#,
    ),
];

/// Every WorkGraph method is served per mob: it reads and changes the graph
/// of the mob its request names, and nothing of another mob's. The second
/// mob's graph is given an item, a goal with its attention binding, and a
/// second goal that was closed, all through the console's door by requests
/// that name that mob; the launch mob's graph is left empty. Then every
/// other method of the family is asked for what only the second mob's
/// graph holds: a request that names the second mob is answered from that
/// graph, and the same request that names no mob, or the launch mob by its
/// id, is answered as a graph without it answers, the two alike. The
/// methods asked are the whole family.
#[tokio::test]
async fn the_workgraph_methods_read_and_change_the_graph_of_the_mob_a_request_names() {
    let (_launch_state, launch) = runtime_with_member("launch", ONLY_IN_THE_LAUNCH_MOB).await;
    let (_second_state, second) = runtime_with_member("second", ONLY_IN_THE_SECOND_MOB).await;
    let (app, _resolver) = console_of_two_mobs(&launch, &second);
    let wrong =
        workgraph_family_served_from_another_graph(&app, &launch.mob_id(), &second.mob_id()).await;
    drop(app);
    launch.shutdown().await;
    second.shutdown().await;
    assert!(
        wrong.is_empty(),
        "a WorkGraph method is not served from the graph of the mob its request names:\n{}",
        wrong.join("\n")
    );
}

/// What [`the_workgraph_methods_read_and_change_the_graph_of_the_mob_a_request_names`]
/// finds on the console `app` of a launch mob and a second mob: one line
/// for each method that is not served from the graph of the mob its
/// request names, none when every method is.
async fn workgraph_family_served_from_another_graph(
    app: &Router,
    launch_mob: &str,
    second_mob: &str,
) -> Vec<String> {
    let (launch_mob, second_mob) = (launch_mob.to_string(), second_mob.to_string());
    let ask = |method: &'static str, mut params: Value, mob_id: Option<&str>| {
        let app = app.clone();
        if let Some(mob_id) = mob_id {
            params["mob_id"] = json!(mob_id);
        }
        async move {
            let answer = ask_as(&app, rpc_request(method, &params), DOOR_ROWS_ADMIN).await;
            serde_json::from_str::<Value>(&answer.2).unwrap_or(Value::Null)
        }
    };
    let owner = |id: &str| json!({"kind": "owner", "owner_key": {"kind": "principal", "id": id}});

    let mut wrong = Vec::new();
    // What only the second mob's graph holds, made through the door.
    let item = ask(
        "meta/workgraph/create",
        json!({"title": "an item of the second mob"}),
        Some(&second_mob),
    )
    .await;
    let goal = ask(
        "meta/workgraph/goal/create",
        json!({"title": "a goal of the second mob", "target": owner("door-rows-owner")}),
        Some(&second_mob),
    )
    .await;
    let closed = ask(
        "meta/workgraph/goal/create",
        json!({"title": "a closed goal of the second mob", "target": owner("door-rows-closed")}),
        Some(&second_mob),
    )
    .await;
    let ids = (
        item["result"]["item"]["id"].as_str(),
        goal["result"]["item"]["id"].as_str(),
        goal["result"]["attention"]["binding_id"].as_str(),
        closed["result"]["attention"]["binding_id"].as_str(),
        closed["result"]["item"]["revision"].as_u64(),
    );
    let (Some(item_id), Some(goal_id), Some(binding_id), Some(closed_binding), Some(revision)) =
        ids
    else {
        return vec![format!(
            "the second mob's graph is not given its item and its goals through the door: \
             {item} / {goal} / {closed}"
        )];
    };
    let closing = ask(
        "meta/workgraph/goal/request_close",
        json!({"binding_id": closed_binding, "expected_revision": revision}),
        Some(&second_mob),
    )
    .await;
    if !closing["error"].is_null() {
        wrong.push(format!(
            "the second mob's second goal is not closed: {closing}"
        ));
    }
    // The two methods that create are told apart by what they made: it is
    // in the second mob's graph and not in the launch mob's.
    for (method, made) in [
        ("meta/workgraph/list", item_id),
        ("meta/workgraph/attention/list", binding_id),
    ] {
        let of_the_second_mob = ask(method, json!({}), Some(&second_mob)).await;
        let of_the_launch_mob = ask(method, json!({}), None).await;
        if !of_the_second_mob.to_string().contains(made)
            || of_the_launch_mob.to_string().contains(made)
            || !of_the_launch_mob["error"].is_null()
        {
            wrong.push(format!(
                "{method}: what a request that named the second mob created is not in that \
                 mob's graph alone: {of_the_second_mob} / {of_the_launch_mob}"
            ));
        }
    }
    for (method, params) in WORKGRAPH_REQUESTS {
        let params = params
            .replace("ITEM", item_id)
            .replace("GOAL", goal_id)
            .replace("BINDING", binding_id);
        let params: Value = serde_json::from_str(&params).expect("the row's params are JSON");
        let of_no_mob = ask(method, params.clone(), None).await;
        let of_the_launch_mob = ask(method, params.clone(), Some(&launch_mob)).await;
        let of_the_second_mob = ask(method, params, Some(&second_mob)).await;
        if of_the_launch_mob != of_no_mob {
            wrong.push(format!(
                "{method}: a request that names the launch mob by its id is not answered as \
                 one that names no mob: {of_the_launch_mob} / {of_no_mob}"
            ));
        }
        if of_the_second_mob == of_no_mob {
            wrong.push(format!(
                "{method}: a request that names the second mob is answered as one for the \
                 launch mob's graph: {of_the_second_mob}"
            ));
        }
        if of_the_second_mob["error"]["data"]["kind"] == json!("foreign_mob_target") {
            wrong.push(format!(
                "{method}: a request that names the second mob is refused: {of_the_second_mob}"
            ));
        }
    }
    let mut asked: Vec<&str> = WORKGRAPH_REQUESTS
        .iter()
        .map(|(method, _)| *method)
        .chain([
            "meta/workgraph/create",
            "meta/workgraph/goal/create",
            // Asked by a signed-in person, in the row after this one.
            "meta/workgraph/goal/confirm",
        ])
        .collect();
    asked.sort_unstable();
    if asked != workgraph_methods_told_apart() {
        wrong.push(format!(
            "the row asks {asked:?}; the family's methods beside the snapshot are {:?}",
            workgraph_methods_told_apart()
        ));
    }

    wrong
}

/// A goal confirmation is made in the graph of the mob its request names.
/// Only a signed-in person confirms a goal, so this row's caller is one. A
/// goal is created in the second mob's graph by a request that names that
/// mob. A confirmation of it with a revision no item has, which changes
/// nothing, is answered from the second mob's graph when it names that
/// mob, and as a graph without that goal answers when it names no mob or
/// the launch mob by its id, the two alike.
#[tokio::test]
async fn a_goal_confirmation_is_made_in_the_graph_of_the_mob_its_request_names() {
    let (_launch_state, launch) = runtime_with_member("launch", ONLY_IN_THE_LAUNCH_MOB).await;
    let (_second_state, second) = runtime_with_member("second", ONLY_IN_THE_SECOND_MOB).await;
    let state = ConsoleJsonState {
        runtime: Some(launch.mob_runtime().clone()),
        ..super::console_human_admission_tests::state()
    };
    let (app, _resolver) = console_of_two_mobs_over(state, &launch, &second);
    let wrong =
        goal_confirmation_made_in_another_graph(&app, &launch.mob_id(), &second.mob_id()).await;
    drop(app);
    launch.shutdown().await;
    second.shutdown().await;
    assert!(
        wrong.is_empty(),
        "a goal confirmation is not made in the graph of the mob its request names:\n{}",
        wrong.join("\n")
    );
}

/// What [`a_goal_confirmation_is_made_in_the_graph_of_the_mob_its_request_names`]
/// finds on the console `app` of a launch mob and a second mob, asked as a
/// signed-in person: one line for each way the confirmation is not made in
/// the graph of the mob its request names, none when it is.
async fn goal_confirmation_made_in_another_graph(
    app: &Router,
    launch_mob: &str,
    second_mob: &str,
) -> Vec<String> {
    use super::console_token_lapse_tests::{console_rpc_as, human_claims, now_seconds, token};

    let mut claims = human_claims();
    claims["sid"] = json!("door-rows-login");
    let person = token(claims, Some(now_seconds() + 3600));
    let ask = |method: &'static str, mut params: Value, mob_id: Option<&str>| {
        let app = app.clone();
        let person = person.clone();
        if let Some(mob_id) = mob_id {
            params["mob_id"] = json!(mob_id);
        }
        async move { console_rpc_as(&app, Some(&person), method, params).await }
    };

    let goal = ask(
        "meta/workgraph/goal/create",
        json!({
            "title": "a goal of the second mob",
            "target": {"kind": "owner", "owner_key": {"kind": "principal", "id": "door-rows-owner"}},
        }),
        Some(second_mob),
    )
    .await;
    let Some(binding_id) = goal["result"]["attention"]["binding_id"].as_str() else {
        return vec![format!(
            "the second mob's graph is not given a goal through the door: {goal}"
        )];
    };
    let confirm = json!({"binding_id": binding_id, "expected_revision": 987_654});
    let method = "meta/workgraph/goal/confirm";
    let of_no_mob = ask(method, confirm.clone(), None).await;
    let of_the_launch_mob = ask(method, confirm.clone(), Some(launch_mob)).await;
    let of_the_second_mob = ask(method, confirm, Some(second_mob)).await;

    let mut wrong = Vec::new();
    if of_the_launch_mob != of_no_mob {
        wrong.push(format!(
            "a confirmation that names the launch mob by its id is not answered as one that \
             names no mob: {of_the_launch_mob} / {of_no_mob}"
        ));
    }
    if of_the_second_mob == of_no_mob {
        wrong.push(format!(
            "a confirmation that names the second mob is answered as one for the launch mob's \
             graph: {of_the_second_mob}"
        ));
    }
    for answer in [&of_no_mob, &of_the_second_mob] {
        let kind = &answer["error"]["data"]["kind"];
        if *kind == json!("approver_unauthenticated") || *kind == json!("foreign_mob_target") {
            wrong.push(format!(
                "the confirmation does not reach the graph: {answer}"
            ));
        }
    }
    wrong
}

/// A definition with one profile under an id of its own that starts with
/// `tag`, for a mob of [`launch_with_a_graph`]'s host.
fn definition_of_a_hosts_mob(tag: &str) -> meerkat_mob::MobDefinition {
    meerkat_mob::MobDefinition::from_toml(&format!(
        "[mob]\nid = \"console-door-{tag}-{}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n\n\
         [profiles.worker.tools]\ncomms = true\n",
        uuid::Uuid::new_v4().simple()
    ))
    .expect("mob definition")
}

/// A launch that declares its mob to its mob set, as a gateway does, with a
/// WorkGraph service of its own, and whose requests may resolve the mobs
/// that set builds for the host.
async fn launch_with_a_graph(temp: &tempfile::TempDir) -> crate::UnifiedRuntime {
    let store: Arc<dyn meerkat_runtime::RuntimeStore> =
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
    let blobs: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(crate::blob_store::Base64BlobStoreAdapter::new(Arc::new(
            crate::blob_store::ObjectStoreBlobStore::memory(),
        )));
    let machine = Arc::new(
        meerkat_runtime::MeerkatMachine::persistent(store.clone(), blobs.clone())
            .expect("the runtime machine"),
    );
    let jobs: Arc<dyn meerkat::DetachedJobStore> = Arc::new(meerkat::MemoryDetachedJobStore::new());
    let mut builder = meerkat::FactoryAgentBuilder::new(
        meerkat::AgentFactory::new(temp.path())
            .builtins(false)
            .comms(true),
        meerkat::Config::default(),
    );
    builder.default_detached_job_store = Some(Arc::clone(&jobs));
    let sessions = Arc::new(meerkat_session::PersistentSessionService::new(
        builder,
        8,
        Arc::new(meerkat_store::MemoryStore::new()),
        store.clone(),
        blobs,
    ));
    let launch_definition = definition_of_a_hosts_mob("host");
    let namespace = launch_definition.id.to_string();
    let graph = meerkat::WorkGraphService::with_scope(
        Arc::new(meerkat::MemoryWorkGraphStore::new()),
        "console-door-host-realm".to_string(),
        meerkat::WorkNamespace::default(),
    );
    let spec = crate::MobBootstrapSpec::declared(
        launch_definition,
        crate::mob_composition_manifest::DeclaredMobStore::in_memory(),
        sessions,
    )
    .with_session_runtime_adapter(Arc::clone(&machine))
    .expect("the launch adopts the runtime machine")
    .with_runtime_delivery(crate::MobRuntimeDelivery::new(store, jobs))
    .with_workgraph_service(Some(graph))
    .with_agent_mob_tools(Arc::new(std::sync::RwLock::new(None)))
    .expect("the agent mob tools")
    .with_child_mob_storage(crate::ChildMobStorage::ProcessBound)
    .with_options(crate::MobBootstrapOptions {
        allow_ephemeral_sessions: false,
        notify_orchestrator_on_resume: true,
        default_llm_client: Some(Arc::new(meerkat_client::TestClient::default())),
    });
    Box::pin(crate::UnifiedRuntime::bootstrap_with_options(
        spec,
        crate::MetaConfig {
            modules: Vec::new(),
            discovery: crate::DiscoverySpec {
                namespace,
                modules: Vec::new(),
            },
            pre_spawn: Vec::new(),
        },
        Vec::new(),
        std::time::Duration::from_secs(2),
        crate::runtime::RuntimeOptions::default(),
        Arc::new(crate::InMemoryMetadataStore::new()),
    ))
    .await
    .expect("a launch over a declared store")
}

/// Have `launch`'s mob set create a second mob of the host from
/// `definition`: a hosted mob as a host has one today. Its id.
async fn hosted_mob_of(
    launch: &crate::UnifiedRuntime,
    definition: meerkat_mob::MobDefinition,
) -> meerkat_mob::MobId {
    Box::pin(
        launch
            .mob_runtime()
            .agent_mob_mcp_state()
            .expect("the launch's mob set")
            .mob_create_definition(definition),
    )
    .await
    .expect("the mob set creates a second mob of the host")
}

/// [`launch_with_a_graph`] and a second mob its set built for the host.
/// The launch and the id of its hosted mob.
async fn launch_with_a_graph_and_a_hosted_mob(
    temp: &tempfile::TempDir,
) -> (crate::UnifiedRuntime, meerkat_mob::MobId) {
    let launch = launch_with_a_graph(temp).await;
    let hosted = hosted_mob_of(&launch, definition_of_a_hosts_mob("hosted")).await;
    (launch, hosted)
}

/// Shut `launch` down after the mob `hosted` its set built for the host.
async fn shut_down_with_its_hosted_mob(
    launch: &crate::UnifiedRuntime,
    hosted: &meerkat_mob::MobId,
) {
    if let Some(set) = launch.mob_runtime().agent_mob_mcp_state()
        && let Ok(handle) = set.handle_for(hosted).await
    {
        let _ = handle.shutdown().await;
    }
    tokio::time::timeout(std::time::Duration::from_secs(30), launch.shutdown())
        .await
        .expect("the launch shuts down");
}

/// The WorkGraph family is served per mob on a host as a gateway composes
/// it: what
/// [`the_workgraph_methods_read_and_change_the_graph_of_the_mob_a_request_names`]
/// holds for a second launch's mob holds for a mob the launch's mob set
/// built for the host, whose graph is its own view of the host's store.
#[tokio::test]
async fn the_workgraph_methods_read_and_change_the_graph_of_a_hosted_mob_a_request_names() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (launch, hosted) = launch_with_a_graph_and_a_hosted_mob(&temp).await;
    let state = door_rows_state(&launch, hosted.as_str());
    let app = console_json_router_with_state(state).layer(axum::Extension(launch.mob_resolver()));
    let wrong =
        workgraph_family_served_from_another_graph(&app, &launch.mob_id(), hosted.as_str()).await;
    drop(app);
    shut_down_with_its_hosted_mob(&launch, &hosted).await;
    assert!(
        wrong.is_empty(),
        "a WorkGraph method is not served from the graph of the hosted mob its request names:\n{}",
        wrong.join("\n")
    );
}

/// A goal confirmation is made in the graph of a hosted mob its request
/// names: what
/// [`a_goal_confirmation_is_made_in_the_graph_of_the_mob_its_request_names`]
/// holds for a second launch's mob holds for a mob the launch's mob set
/// built for the host.
#[tokio::test]
async fn a_goal_confirmation_is_made_in_the_graph_of_a_hosted_mob_its_request_names() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (launch, hosted) = launch_with_a_graph_and_a_hosted_mob(&temp).await;
    let state = ConsoleJsonState {
        runtime: Some(launch.mob_runtime().clone()),
        ..super::console_human_admission_tests::state()
    };
    let app = console_json_router_with_state(state).layer(axum::Extension(launch.mob_resolver()));
    let wrong =
        goal_confirmation_made_in_another_graph(&app, &launch.mob_id(), hosted.as_str()).await;
    drop(app);
    shut_down_with_its_hosted_mob(&launch, &hosted).await;
    assert!(
        wrong.is_empty(),
        "a goal confirmation is not made in the graph of the hosted mob its request names:\n{}",
        wrong.join("\n")
    );
}

/// On the door of a mob the host serves beside its launch mob, the member a
/// request names is read among that mob's members alone: what the launch
/// mob lists decides nothing there. A member of the launch mob made a mob
/// with a member `helper`; the hosted mob has a member of its own whose
/// name is the one string the launch mob's door knows that member by. An
/// administrator inspects on the hosted mob's door. By the two values (the
/// made mob and `helper`) the answer is the one for a mob id no mob has,
/// apart from the id: a member of a mob made under the launch mob is no
/// member there. By the one string the answer is the hosted mob's own
/// member. The launch mob's door answers the made mob's member by the same
/// two values, so the member is one the console lists.
#[tokio::test]
async fn a_hosted_mobs_door_reads_the_member_a_request_names_among_its_own_members() {
    const MAKER: &str = "maker";
    const NESTED_NAME: &str = "helper";
    const LAUNCH_REGISTRATION: &str = "runtime-a";
    let method = "meta/console/inspect_identity";
    let temp = tempfile::tempdir().expect("tempdir");
    let (launch, hosted) = launch_with_a_graph_and_a_hosted_mob(&temp).await;
    let launch_mob = launch.mob_id();
    let member = |name: &str| {
        SpawnMemberSpec::from_wire("worker".to_string(), name.to_string(), None, None, None)
    };
    let mut fixture = vec![
        Box::pin(launch.spawn(member(MAKER)))
            .await
            .map(|_| ())
            .map_err(|error| format!("{error:?}")),
    ];
    // The mob the launch mob's member makes with its session, as its mob
    // tools do, and that mob's member.
    let set = launch
        .mob_runtime()
        .agent_mob_mcp_state()
        .expect("the launch's mob set");
    let maker = launch
        .mob_handle()
        .resolve_bridge_session_id(&meerkat_mob::AgentIdentity::from(MAKER))
        .await
        .expect("the maker's session");
    let made = Box::pin(set.mob_create_definition_with_owner_bridge_session(
        definition_of_a_hosts_mob("made"),
        maker,
        true,
        false,
    ))
    .await
    .expect("a member makes a mob");
    let made_handle = Box::pin(set.handle_for(&made))
        .await
        .expect("the made mob's handle");
    fixture.push(
        Box::pin(made_handle.spawn_spec(member(NESTED_NAME)))
            .await
            .map(|_| ())
            .map_err(|error| format!("{error:?}")),
    );
    let one_string = format!("{made}/{NESTED_NAME}");
    // The host's console: it was told its default mob and holds the
    // registration of the launch mob's runtime and of the hosted mob's
    // console events, as a host registers them. The hosted mob gets its own
    // member under the one string.
    let aggregator = MetaConsoleAggregator::new(Arc::new(
        crate::console_aggregator::InMemoryConsoleLogStore::new(),
    ));
    aggregator.tell_default_mob(crate::member_key::DefaultMob::new(
        launch.mob_handle().mob_id().clone(),
    ));
    aggregator.register_runtime_handles_with_policy(
        LAUNCH_REGISTRATION,
        "",
        launch.mob_runtime().clone(),
        None,
        launch.console_events(),
        Arc::new(crate::console_aggregator::AllowAllConsoleVisibilityPolicy),
    );
    match launch.mob_resolver().resolve(Some(&hosted)).await {
        Ok(mob) => {
            let handle = mob.mob_handle();
            let own_name = crate::member_comms_id::mob_member_id_str(&one_string);
            fixture.push(
                Box::pin(handle.spawn_spec(member(&own_name)))
                    .await
                    .map(|_| ())
                    .map_err(|error| format!("{error:?}")),
            );
            fixture.push(match mob.console_events_of_its_entry() {
                Some(console_events) => aggregator
                    .register_mob_console_events(
                        &hosted,
                        mob.mob_runtime().clone(),
                        console_events,
                        Arc::new(crate::console_aggregator::AllowAllConsoleVisibilityPolicy),
                    )
                    .map(|_| ())
                    .map_err(|error| format!("{error:?}")),
                None => Err("the hosted mob has no console events of its own".to_string()),
            });
        }
        Err(error) => fixture.push(Err(format!("{error:?}"))),
    }
    let state = ConsoleJsonState {
        console_aggregator: Some(aggregator),
        ..door_rows_state(&launch, hosted.as_str())
    };
    let app = console_json_router_with_state(state).layer(axum::Extension(launch.mob_resolver()));
    let absent_mob = an_id_no_mob_has(made.as_str());
    let inspection = |mob_id: Option<&str>, identity: &str, member_mob: Option<&str>| {
        let mut params = naming_mob(mob_id);
        params["identity"] = json!(identity);
        if let Some(member_mob) = member_mob {
            params["member_mob_id"] = json!(member_mob);
        }
        rpc_request(method, &params)
    };
    // The member an inspection answered, by the three values of its row;
    // or its error.
    let named = |answer: &DoorAnswer| -> String {
        let body: Value = serde_json::from_str(&answer.2).unwrap_or(Value::Null);
        let record = &body["result"]["identity"];
        match (
            record["member_name"].as_str(),
            record["member_mob_id"].as_str(),
            record["mob_id"].as_str(),
        ) {
            (Some(name), Some(mob), Some(door)) if body["error"].is_null() => {
                format!("{name} of {mob} on the door of {door}")
            }
            _ => format!(
                "error {}: {}",
                body["error"]["code"], body["error"]["message"]
            ),
        }
    };
    let the_made_mobs_member = format!("{NESTED_NAME} of {made} on the door of {launch_mob}");
    let the_hosted_mobs_own = format!("{one_string} of {hosted} on the door of {hosted}");
    // A member is found once its mob lists it, a moment after it is seated:
    // the inspections are asked until the launch mob's door and the hosted
    // mob's answer their members, or ten seconds passed.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    let (witness, by_one_string, by_two_values, of_no_mob) = loop {
        let witness = named(
            &ask_as(
                &app,
                inspection(None, NESTED_NAME, Some(made.as_str())),
                DOOR_ROWS_ADMIN,
            )
            .await,
        );
        let by_one_string = named(
            &ask_as(
                &app,
                inspection(Some(hosted.as_str()), &one_string, None),
                DOOR_ROWS_ADMIN,
            )
            .await,
        );
        let by_two_values = ask_as(
            &app,
            inspection(Some(hosted.as_str()), NESTED_NAME, Some(made.as_str())),
            DOOR_ROWS_ADMIN,
        )
        .await;
        let of_no_mob = ask_as(
            &app,
            inspection(Some(hosted.as_str()), NESTED_NAME, Some(&absent_mob)),
            DOOR_ROWS_ADMIN,
        )
        .await;
        if (witness == the_made_mobs_member && by_one_string == the_hosted_mobs_own)
            || tokio::time::Instant::now() >= deadline
        {
            break (witness, by_one_string, by_two_values, of_no_mob);
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };
    drop(app);
    let _ = made_handle.shutdown().await;
    shut_down_with_its_hosted_mob(&launch, &hosted).await;

    let mut wrong = Vec::new();
    if fixture.iter().any(Result::is_err) {
        wrong.push(format!(
            "the fixture (the maker, the made mob's member, the hosted mob's member, its \
             registration): {fixture:?}"
        ));
    }
    if witness != the_made_mobs_member {
        wrong.push(format!(
            "the launch mob's door does not answer the made mob's member by its two values: \
             {witness}"
        ));
    }
    if by_one_string != the_hosted_mobs_own {
        wrong.push(format!(
            "the hosted mob's door does not answer its own member by the one string: \
             {by_one_string}"
        ));
    }
    if apart_from_the_id(&by_two_values, made.as_str())
        != apart_from_the_id(&of_no_mob, &absent_mob)
    {
        wrong.push(format!(
            "the hosted mob's door answers the two values of the made mob's member and those \
             of a mob id no mob has differently: {} and {}",
            by_two_values.2, of_no_mob.2
        ));
    }
    assert!(
        wrong.is_empty(),
        "the hosted mob's door does not read the member among its own mob's members:\n{}",
        wrong.join("\n")
    );
}

/// A host as a gateway composes it, for the two rows below: a launch mob
/// and a mob the host serves beside it, each with one member, and a mob
/// each of those members made with its session, as its mob tools do, with
/// one member of its own.
struct HostOfTwoMobsAndTheirMadeMobs {
    launch: crate::UnifiedRuntime,
    hosted: meerkat_mob::MobId,
    hosted_mob: crate::unified_runtime::ResolvedMob,
    /// The mob the launch mob's member made, and the mob the hosted mob's
    /// member made.
    under_the_launch: meerkat_mob::MobId,
    under_the_hosted: meerkat_mob::MobId,
    made_handles: Vec<meerkat_mob::MobHandle>,
    /// What each step of the fixture answered.
    fixture: Vec<Result<(), String>>,
}

impl HostOfTwoMobsAndTheirMadeMobs {
    const LAUNCH_MEMBER: &'static str = "maker";
    const HOSTED_MEMBER: &'static str = "hosted-maker";
    const UNDER_THE_LAUNCH: &'static str = "helper";
    const UNDER_THE_HOSTED: &'static str = "aide";

    async fn start(temp: &tempfile::TempDir) -> Self {
        let (launch, hosted) = launch_with_a_graph_and_a_hosted_mob(temp).await;
        let member = |name: &str| {
            SpawnMemberSpec::from_wire("worker".to_string(), name.to_string(), None, None, None)
        };
        let hosted_mob = launch
            .mob_resolver()
            .resolve(Some(&hosted))
            .await
            .expect("the hosted mob resolves");
        let hosted_handle = hosted_mob.mob_handle();
        let mut fixture = vec![
            Box::pin(launch.spawn(member(Self::LAUNCH_MEMBER)))
                .await
                .map(|_| ())
                .map_err(|error| format!("{error:?}")),
            Box::pin(hosted_handle.spawn_spec(member(Self::HOSTED_MEMBER)))
                .await
                .map(|_| ())
                .map_err(|error| format!("{error:?}")),
        ];
        let set = launch
            .mob_runtime()
            .agent_mob_mcp_state()
            .expect("the launch's mob set");
        let mut made = Vec::new();
        let mut made_handles = Vec::new();
        for (makers_mob, maker, tag, name) in [
            (
                launch.mob_handle(),
                Self::LAUNCH_MEMBER,
                "under-launch",
                Self::UNDER_THE_LAUNCH,
            ),
            (
                hosted_handle,
                Self::HOSTED_MEMBER,
                "under-hosted",
                Self::UNDER_THE_HOSTED,
            ),
        ] {
            let session = makers_mob
                .resolve_bridge_session_id(&meerkat_mob::AgentIdentity::from(maker))
                .await
                .expect("the maker's session");
            let mob_id = Box::pin(set.mob_create_definition_with_owner_bridge_session(
                definition_of_a_hosts_mob(tag),
                session,
                true,
                false,
            ))
            .await
            .expect("a member makes a mob");
            let handle = Box::pin(set.handle_for(&mob_id))
                .await
                .expect("the made mob's handle");
            fixture.push(
                Box::pin(handle.spawn_spec(member(name)))
                    .await
                    .map(|_| ())
                    .map_err(|error| format!("{error:?}")),
            );
            made.push(mob_id);
            made_handles.push(handle);
        }
        let under_the_hosted = made.pop().expect("the mob made under the hosted mob");
        let under_the_launch = made.pop().expect("the mob made under the launch mob");
        Self {
            launch,
            hosted,
            hosted_mob,
            under_the_launch,
            under_the_hosted,
            made_handles,
            fixture,
        }
    }

    async fn shut_down(self) {
        for handle in &self.made_handles {
            let _ = handle.shutdown().await;
        }
        drop(self.hosted_mob);
        shut_down_with_its_hosted_mob(&self.launch, &self.hosted).await;
    }
}

/// The access records of a mob hold that mob's own agents and the members
/// of the mobs made under it, and nothing of another mob of the host. A
/// launch mob's member made a mob with a member; a mob the host serves
/// beside the launch mob has a member, which made a mob with a member too.
/// The records of each of the two mobs are read as a request for that mob
/// reads them, and then counted entry by entry: the hosted mob's hold its
/// own member and the member of the mob made under it, and no entry of the
/// launch mob's member or of the mob made under the launch mob; the launch
/// mob's hold its own member and the member of the mob made under it, and
/// no entry of the mob made under the hosted mob. On the hosted mob's door
/// a timeline read by two values that name the launch mob's member, or the
/// member of the mob made under the launch mob, is answered as for a mob id
/// no mob has, apart from the id.
#[tokio::test]
async fn the_access_records_of_a_mob_hold_nothing_of_another_mob_of_the_host() {
    type Host = HostOfTwoMobsAndTheirMadeMobs;
    const LAUNCH_REGISTRATION: &str = "runtime-a";
    let temp = tempfile::tempdir().expect("tempdir");
    let host = Host::start(&temp).await;
    let launch_mob = host.launch.mob_id();
    let under_the_launch = host.under_the_launch.to_string();
    let under_the_hosted = host.under_the_hosted.to_string();

    // What each mob's records are to hold: its own agents, and under each
    // mob made under it that mob's members, by the name its door gives
    // them.
    type Records = (Vec<String>, std::collections::BTreeMap<String, Vec<String>>);
    let records_of = |own: &str, made: &str, member: &str| -> Records {
        (
            vec![own.to_string()],
            std::collections::BTreeMap::from([(
                made.to_string(),
                vec![format!("{made}/{member}")],
            )]),
        )
    };
    let of_the_launch_expected = records_of(
        Host::LAUNCH_MEMBER,
        &under_the_launch,
        Host::UNDER_THE_LAUNCH,
    );
    let of_the_hosted_expected = records_of(
        Host::HOSTED_MEMBER,
        &under_the_hosted,
        Host::UNDER_THE_HOSTED,
    );
    // A member is recorded once its mob lists it, a moment after it is
    // seated: the records are read until they hold what is expected, or
    // ten seconds passed. Each reading is of new records.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    let (of_the_launch, of_the_hosted) = loop {
        let records = AccessController::audience_only();
        prime_access_cache_from_runtime(host.launch.mob_runtime(), &records).await;
        prime_access_cache_from_runtime_of(
            host.hosted_mob.mob_runtime(),
            &records,
            Some(&host.hosted),
        )
        .await;
        let read = (
            records.recorded_identities_in_tests(None),
            records.recorded_identities_in_tests(Some(&host.hosted)),
        );
        if (read.0 == of_the_launch_expected && read.1 == of_the_hosted_expected)
            || tokio::time::Instant::now() >= deadline
        {
            break read;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };

    // The hosted mob's door, for the timeline by two values.
    let aggregator = MetaConsoleAggregator::new(Arc::new(
        crate::console_aggregator::InMemoryConsoleLogStore::new(),
    ));
    aggregator.tell_default_mob(crate::member_key::DefaultMob::new(
        host.launch.mob_handle().mob_id().clone(),
    ));
    aggregator.register_runtime_handles_with_policy(
        LAUNCH_REGISTRATION,
        "",
        host.launch.mob_runtime().clone(),
        None,
        host.launch.console_events(),
        Arc::new(crate::console_aggregator::AllowAllConsoleVisibilityPolicy),
    );
    let mut fixture = host.fixture.clone();
    fixture.push(match host.hosted_mob.console_events_of_its_entry() {
        Some(console_events) => aggregator
            .register_mob_console_events(
                &host.hosted,
                host.hosted_mob.mob_runtime().clone(),
                console_events,
                Arc::new(crate::console_aggregator::AllowAllConsoleVisibilityPolicy),
            )
            .map(|_| ())
            .map_err(|error| format!("{error:?}")),
        None => Err("the hosted mob has no console events of its own".to_string()),
    });
    let state = ConsoleJsonState {
        console_aggregator: Some(aggregator),
        ..door_rows_state(&host.launch, host.hosted.as_str())
    };
    let app =
        console_json_router_with_state(state).layer(axum::Extension(host.launch.mob_resolver()));
    let timeline = |identity: &str, member_mob: &str| {
        let mut params = naming_mob(Some(host.hosted.as_str()));
        params["identity"] = json!(identity);
        params["member_mob_id"] = json!(member_mob);
        rpc_request("meta/console/query_timeline", &params)
    };
    let mut by_two_values = Vec::new();
    for (what, identity, member_mob) in [
        (
            "the launch mob's member",
            Host::LAUNCH_MEMBER,
            launch_mob.as_str(),
        ),
        (
            "the member of the mob made under the launch mob",
            Host::UNDER_THE_LAUNCH,
            under_the_launch.as_str(),
        ),
    ] {
        let absent_mob = an_id_no_mob_has(member_mob);
        let named = ask_as(&app, timeline(identity, member_mob), DOOR_ROWS_ADMIN).await;
        let of_no_mob = ask_as(&app, timeline(identity, &absent_mob), DOOR_ROWS_ADMIN).await;
        by_two_values.push((
            what,
            apart_from_the_id(&named, member_mob),
            apart_from_the_id(&of_no_mob, &absent_mob),
        ));
    }
    drop(app);
    host.shut_down().await;

    let mut wrong = Vec::new();
    if fixture.iter().any(Result::is_err) {
        wrong.push(format!(
            "the fixture (the two members, the two made mobs' members, the hosted mob's \
             registration): {fixture:?}"
        ));
    }
    if of_the_launch != of_the_launch_expected {
        wrong.push(format!(
            "the launch mob's records hold {of_the_launch:?}, not {of_the_launch_expected:?}"
        ));
    }
    if of_the_hosted != of_the_hosted_expected {
        wrong.push(format!(
            "the hosted mob's records hold {of_the_hosted:?}, not {of_the_hosted_expected:?}"
        ));
    }
    for (what, named, of_no_mob) in by_two_values {
        if named != of_no_mob {
            wrong.push(format!(
                "the hosted mob's door answers a timeline by two values that name {what} and \
                 one for a mob id no mob has differently: {} and {}",
                named.2, of_no_mob.2
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "a mob's access records hold something of another mob of the host:\n{}",
        wrong.join("\n")
    );
}

/// A mob whose creation record does not say which member created it is
/// listed once, at the top level of the launch mob's console, and under no
/// mob the host serves beside the launch mob: what the record leaves
/// undecided attributes nothing to such a mob. The launch mob's console
/// lists that mob, marked as of an undecided owner, beside the mob its own
/// member made; the hosted mob's console lists the mob its member made and
/// no other.
#[tokio::test]
async fn a_mob_of_undecided_creation_is_listed_for_the_launch_mob_alone() {
    type Host = HostOfTwoMobsAndTheirMadeMobs;
    let temp = tempfile::tempdir().expect("tempdir");
    let host = Host::start(&temp).await;
    let definition = definition_of_a_hosts_mob("undecided");
    let undecided = definition.id.clone();
    let sessions = host
        .launch
        .mob_runtime()
        .session_service()
        .expect("the launch's session service")
        .clone();
    let mut fixture = host.fixture.clone();
    let undecided_handle =
        match meerkat_mob::MobBuilder::new(definition, meerkat_mob::MobStorage::in_memory())
            .with_session_service(sessions)
            .with_default_llm_client(Arc::new(meerkat_client::TestClient::default()))
            .with_provenance(meerkat_mob::MobProvenance::Ambiguous {
                matching_rosters: 2,
            })
            .create()
            .await
        {
            Ok(handle) => {
                let set = host
                    .launch
                    .mob_runtime()
                    .agent_mob_mcp_state()
                    .expect("the launch's mob set");
                fixture.push(
                    Box::pin(set.try_mob_insert_handle(undecided.clone(), handle.clone()))
                        .await
                        .map_err(|error| format!("{error:?}")),
                );
                Some(handle)
            }
            Err(error) => {
                fixture.push(Err(format!("{error:?}")));
                None
            }
        };
    // The mobs a console lists, each with whether its owner is undecided.
    let listed = |listing: crate::console_mob_listing::ConsoleMobListing| {
        let mut listed: Vec<(String, bool)> = listing
            .listed
            .into_iter()
            .map(|mob| (mob.mob_id, mob.owner_undecidable))
            .collect();
        listed.sort();
        listed
    };
    let for_the_launch = listed(
        Box::pin(crate::console_mob_listing::console_mob_listing(
            host.launch.mob_runtime(),
            "",
        ))
        .await,
    );
    let for_the_hosted = listed(
        Box::pin(crate::console_mob_listing::console_mob_listing(
            host.hosted_mob.mob_runtime(),
            "",
        ))
        .await,
    );
    let mut for_the_launch_expected = vec![
        (host.under_the_launch.to_string(), false),
        (undecided.to_string(), true),
    ];
    for_the_launch_expected.sort();
    let for_the_hosted_expected = vec![(host.under_the_hosted.to_string(), false)];
    if let Some(handle) = &undecided_handle {
        let _ = handle.shutdown().await;
    }
    host.shut_down().await;

    let mut wrong = Vec::new();
    if fixture.iter().any(Result::is_err) {
        wrong.push(format!(
            "the fixture (the two members, the two made mobs' members, the mob of undecided \
             creation): {fixture:?}"
        ));
    }
    if for_the_launch != for_the_launch_expected {
        wrong.push(format!(
            "the launch mob's console lists {for_the_launch:?}, not {for_the_launch_expected:?}"
        ));
    }
    if for_the_hosted != for_the_hosted_expected {
        wrong.push(format!(
            "the hosted mob's console lists {for_the_hosted:?}, not {for_the_hosted_expected:?}"
        ));
    }
    assert!(
        wrong.is_empty(),
        "a mob of undecided creation is not listed for the launch mob alone:\n{}",
        wrong.join("\n")
    );
}

/// The changing methods the row below asks with the revisions the named
/// mob's graph has, in the order it asks them: every changing method of
/// the family beside the two that create, which the row gives each graph
/// its items with, and the confirmation, which only a signed-in person
/// makes.
const WORKGRAPH_CHANGES: [&str; 14] = [
    "meta/workgraph/update",
    "meta/workgraph/evidence/add",
    "meta/workgraph/claim",
    "meta/workgraph/release",
    "meta/workgraph/block",
    "meta/workgraph/link",
    "meta/workgraph/attention/pause",
    "meta/workgraph/attention/resume",
    "meta/workgraph/goal/request_close",
    "meta/workgraph/attention/reassign",
    // Before the break-glass reassign, which moves the binding this names.
    "meta/workgraph/policy/escalate",
    "meta/workgraph/attention/break_glass_reassign",
    "meta/workgraph/close",
    "meta/workgraph/attention/prune",
];

/// The changes of [`WORKGRAPH_CHANGES`] that are answered a result in the
/// row below, in the order it asks them: the same thirteen for each of its
/// two mobs. The one other change is refused there, for each mob alike
/// ([`WORKGRAPH_CHANGE_REFUSED`]).
const WORKGRAPH_CHANGES_THAT_LAND: [&str; 13] = [
    "meta/workgraph/update",
    "meta/workgraph/evidence/add",
    "meta/workgraph/claim",
    "meta/workgraph/release",
    "meta/workgraph/block",
    "meta/workgraph/link",
    "meta/workgraph/attention/pause",
    "meta/workgraph/attention/resume",
    "meta/workgraph/goal/request_close",
    "meta/workgraph/policy/escalate",
    "meta/workgraph/attention/break_glass_reassign",
    "meta/workgraph/close",
    "meta/workgraph/attention/prune",
];

/// The one change of [`WORKGRAPH_CHANGES`] the row below holds as refused,
/// with the words its refusal carries: a reassign of a binding of a goal
/// that names no mode, which is in the mode a reassign is not derived
/// for. The refusal changes neither graph.
const WORKGRAPH_CHANGE_REFUSED: (&str, &str) =
    ("meta/workgraph/attention/reassign", "is in 'pursue' mode");

/// A WorkGraph change lands in the graph of the mob its request names and
/// in no other. On a host with a hosted mob, each of the two mobs' graphs
/// is given an item and three goals through the console's door. Then each
/// change of [`WORKGRAPH_CHANGES`] is asked for one mob, with the ids and
/// the revisions that mob's own graph has, and after every request both
/// graphs are read (their items, events, facts and attention bindings): a
/// request that was answered a result changed the graph of the mob it
/// named, a request that was refused changed neither graph, and no request
/// changed the other mob's graph. The changes that are answered a result
/// are exactly those of [`WORKGRAPH_CHANGES_THAT_LAND`], for each mob, and
/// the one that is refused is refused with the words of
/// [`WORKGRAPH_CHANGE_REFUSED`].
/// At the end each mob's snapshot, read through the door, shows that mob's
/// goals and none of the other's, and a read by id for one mob of the
/// other mob's item is not answered that item.
#[tokio::test]
async fn a_workgraph_change_lands_in_the_graph_of_the_mob_its_request_names_and_in_no_other() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (launch, hosted) = launch_with_a_graph_and_a_hosted_mob(&temp).await;
    let hosted_mob = hosted.as_str().to_string();
    let state = door_rows_state(&launch, &hosted_mob);
    let app = console_json_router_with_state(state).layer(axum::Extension(launch.mob_resolver()));
    let ask = |method: &'static str, mut params: Value, mob_id: Option<&str>| {
        let app = app.clone();
        if let Some(mob_id) = mob_id {
            params["mob_id"] = json!(mob_id);
        }
        async move {
            let answer = ask_as(&app, rpc_request(method, &params), DOOR_ROWS_ADMIN).await;
            serde_json::from_str::<Value>(&answer.2).unwrap_or(Value::Null)
        }
    };
    let owner = |id: &str| json!({"kind": "owner", "owner_key": {"kind": "principal", "id": id}});
    // A mob's graph as the door reads it: its items, events, facts and
    // attention bindings.
    let graph = |mob_id: Option<&str>| {
        let reads = [
            ask("meta/workgraph/list", json!({}), mob_id),
            ask("meta/workgraph/events", json!({}), mob_id),
            ask("meta/workgraph/facts", json!({}), mob_id),
            ask("meta/workgraph/attention/list", json!({}), mob_id),
        ];
        async move {
            let mut read = Vec::new();
            for answer in reads {
                read.push(answer.await);
            }
            read
        }
    };

    let mut wrong = Vec::new();
    let mut landed = Vec::new();
    // Each mob's item, by its id in that mob's graph.
    let mut items = Vec::new();
    let mobs = [
        ("the hosted mob", Some(hosted_mob.as_str()), None),
        ("the launch mob", None, Some(hosted_mob.as_str())),
    ];
    for (named, mob_id, other) in mobs {
        // Each goal has a target of its own: a target holds one active
        // attention binding in a graph.
        let goal = |title: &str, held_by: &str| {
            ask(
                "meta/workgraph/goal/create",
                json!({"title": format!("{title} of {named}"), "target": owner(held_by)}),
                mob_id,
            )
        };
        let item = ask(
            "meta/workgraph/create",
            json!({"title": format!("an item of {named}")}),
            mob_id,
        )
        .await;
        let held = goal("a goal that is paused", "door-rows-holds").await;
        let moved = goal("a goal that is reassigned", "door-rows-hands-over").await;
        let closing = goal("a goal that is closed", "door-rows-closes").await;
        let ids = (
            item["result"]["item"]["id"].as_str(),
            held["result"]["attention"]["binding_id"].as_str(),
            moved["result"]["item"]["id"].as_str(),
            moved["result"]["attention"]["binding_id"].as_str(),
            closing["result"]["item"]["id"].as_str(),
            closing["result"]["attention"]["binding_id"].as_str(),
        );
        let (
            Some(item_id),
            Some(held_binding),
            Some(moved_goal),
            Some(moved_binding),
            Some(closing_goal),
            Some(closing_binding),
        ) = ids
        else {
            wrong.push(format!(
                "{named}: its graph is not given an item and three goals through the door: \
                 {item} / {held} / {moved} / {closing}"
            ));
            continue;
        };
        items.push((named, mob_id, item_id.to_string()));
        // The revision the named mob's graph has now, of an item and of an
        // attention binding.
        let item_revision = |id: &str| {
            let read = ask("meta/workgraph/get", json!({"id": id}), mob_id);
            async move { read.await["result"]["item"]["revision"].as_u64() }
        };
        let binding_revision = |binding_id: &str| {
            let read = ask("meta/workgraph/attention/list", json!({}), mob_id);
            let binding_id = json!(binding_id);
            async move {
                read.await["result"]["attention"]
                    .as_array()
                    .and_then(|bindings| {
                        bindings
                            .iter()
                            .find(|binding| binding["binding_id"] == binding_id)
                    })
                    .and_then(|binding| binding["machine_state"]["revision"].as_u64())
            }
        };

        let mut before = [graph(mob_id).await, graph(other).await];
        for method in WORKGRAPH_CHANGES {
            let item_now = item_revision(item_id).await;
            let params = match method {
                "meta/workgraph/update" => {
                    json!({"id": item_id, "expected_revision": item_now, "title": "renamed"})
                }
                "meta/workgraph/evidence/add" => json!({
                    "id": item_id,
                    "expected_revision": item_now,
                    "evidence": {"kind": "note", "id": "door-rows"},
                }),
                "meta/workgraph/claim" | "meta/workgraph/release" | "meta/workgraph/block" => {
                    json!({"id": item_id, "expected_revision": item_now})
                }
                "meta/workgraph/link" => {
                    json!({"kind": "related", "from_id": item_id, "to_id": moved_goal})
                }
                "meta/workgraph/attention/pause" | "meta/workgraph/attention/resume" => json!({
                    "binding_id": held_binding,
                    "expected_revision": binding_revision(held_binding).await,
                }),
                "meta/workgraph/goal/request_close" => json!({
                    "binding_id": closing_binding,
                    "expected_revision": item_revision(closing_goal).await,
                }),
                "meta/workgraph/attention/reassign" => json!({
                    "binding_id": moved_binding,
                    "expected_revision": binding_revision(moved_binding).await,
                    "target": owner("door-rows-other"),
                }),
                "meta/workgraph/attention/break_glass_reassign" => json!({
                    "binding_id": moved_binding,
                    "expected_revision": binding_revision(moved_binding).await,
                    "reason": "door rows",
                    "target": owner("door-rows-third"),
                }),
                "meta/workgraph/policy/escalate" => json!({
                    "id": moved_goal,
                    "expected_revision": item_revision(moved_goal).await,
                    "binding_id": moved_binding,
                    "completion_policy": {"kind": "host_confirmed"},
                }),
                "meta/workgraph/close" => json!({
                    "id": item_id,
                    "expected_revision": item_now,
                    "status": "completed",
                }),
                _ => json!({}),
            };
            let answer = ask(method, params, mob_id).await;
            let after = [graph(mob_id).await, graph(other).await];
            let answered_a_result = answer["error"].is_null() && !answer["result"].is_null();
            if after[1] != before[1] {
                wrong.push(format!(
                    "{method} for {named}: the other mob's graph is not as it was: {answer}"
                ));
            }
            if answered_a_result {
                landed.push((named, method));
                // A prune with nothing to prune is answered a result and
                // changes nothing.
                if after[0] == before[0] && method != "meta/workgraph/attention/prune" {
                    wrong.push(format!(
                        "{method} for {named}: it was answered a result and the graph of the \
                         mob it named is as it was: {answer}"
                    ));
                }
            } else {
                if after[0] != before[0] {
                    wrong.push(format!(
                        "{method} for {named}: it was refused and the graph of the mob it \
                         named changed: {answer}"
                    ));
                }
                // The refusal the row holds, by its words.
                let (refused, words) = WORKGRAPH_CHANGE_REFUSED;
                if method != refused
                    || !answer["error"]["message"]
                        .as_str()
                        .is_some_and(|message| message.contains(words))
                {
                    wrong.push(format!(
                        "{method} for {named}: it is refused, and not as the one refusal the \
                         row holds ({refused}: {words}): {answer}"
                    ));
                }
            }
            before = after;
        }
    }
    for (named, _, _) in mobs {
        let of_the_mob: Vec<&str> = landed
            .iter()
            .filter(|(of, _)| *of == named)
            .map(|(_, method)| *method)
            .collect();
        if of_the_mob != WORKGRAPH_CHANGES_THAT_LAND {
            wrong.push(format!(
                "{named}: the changes answered a result are {of_the_mob:?}; the row holds \
                 {WORKGRAPH_CHANGES_THAT_LAND:?}"
            ));
        }
    }
    // A read by id for one mob reaches that mob's graph alone: it answers
    // the mob's own item and not the item of the other mob.
    for (named, mob_id, own_item) in &items {
        let own = ask("meta/workgraph/get", json!({"id": own_item}), *mob_id).await;
        if own["result"]["item"]["id"] != json!(own_item) {
            wrong.push(format!(
                "meta/workgraph/get for {named}: its own item is not answered: {own}"
            ));
        }
        for (other_named, _, other_item) in items.iter().filter(|(other, _, _)| other != named) {
            let answer = ask("meta/workgraph/get", json!({"id": other_item}), *mob_id).await;
            if !answer["result"]["item"].is_null() {
                wrong.push(format!(
                    "meta/workgraph/get for {named} with the id of the item of {other_named}: \
                     it is answered an item: {answer}"
                ));
            }
        }
    }
    for (named, mob_id, _) in mobs {
        let snapshot = ask("meta/workgraph/snapshot", json!({}), mob_id).await;
        let shown = snapshot.to_string();
        let of_the_other = if mob_id.is_some() {
            "of the launch mob"
        } else {
            "of the hosted mob"
        };
        if !snapshot["error"].is_null()
            || !shown.contains(&format!("a goal that is paused of {named}"))
            || shown.contains(of_the_other)
        {
            wrong.push(format!(
                "the snapshot for {named} does not show that mob's graph alone: {shown}"
            ));
        }
    }
    let mut asked: Vec<&str> = WORKGRAPH_CHANGES
        .into_iter()
        .chain([
            "meta/workgraph/create",
            "meta/workgraph/goal/create",
            // A signed-in person's, asked in the confirmation rows.
            "meta/workgraph/goal/confirm",
        ])
        .collect();
    asked.sort_unstable();
    let mut changing: Vec<&str> = crate::rpc::workgraph_methods::WORKGRAPH_MUTATE_METHODS
        .iter()
        .chain(crate::rpc::workgraph_methods::WORKGRAPH_CONSOLE_MUTATE_METHODS)
        .copied()
        .collect();
    changing.sort_unstable();
    if asked != changing {
        wrong.push(format!(
            "the row asks {asked:?}; the family's changing methods are {changing:?}"
        ));
    }

    drop(app);
    shut_down_with_its_hosted_mob(&launch, &hosted).await;
    assert!(
        wrong.is_empty(),
        "a WorkGraph change does not land in the graph of the mob its request names alone:\n{}",
        wrong.join("\n")
    );
}

/// The text of the launch mob's stored frame in the row below.
const OF_THE_LAUNCH_MOBS_MEMBER: &str = "frame-of-the-launch-mobs-member";
/// The text of the stored frame of the second mob's shared member.
const OF_THE_SECOND_MOBS_SHARED_MEMBER: &str = "frame-of-the-second-mobs-shared-member";
/// The text of the stored frame of the second mob's member that is private
/// to another person.
const OF_THE_SECOND_MOBS_PRIVATE_MEMBER: &str = "frame-of-the-second-mobs-private-member";

/// Which of the three stored frames of the row below `body` holds: the
/// launch mob's member's, the second mob's shared member's, and the second
/// mob's private member's.
fn frames_held(body: &str) -> [bool; 3] {
    [
        body.contains(OF_THE_LAUNCH_MOBS_MEMBER),
        body.contains(OF_THE_SECOND_MOBS_SHARED_MEMBER),
        body.contains(OF_THE_SECOND_MOBS_PRIVATE_MEMBER),
    ]
}

/// The source of a stored frame of the member `member` of the mob `mob_id`,
/// with the member as the frame recorded it: a worker with no label.
fn source_recording_the_member(
    member: &str,
    mob_id: &str,
) -> crate::console_aggregator::ConsoleFrameSource {
    serde_json::from_value(json!({
        "kind": "session_history",
        "member_provenance": {
            "identity": {
                "identity": member,
                "display_name": member,
                "runtime_key": "timeline-rows",
                "runtime_member_id": member,
                "visibility": "addressable",
                "addressable": true,
                "health": "active",
            },
            "member": {
                "agent_identity": member,
                "role": "worker",
                "state": "active",
                "wired_to": [],
            },
            "primary_mob_id": mob_id,
            "source_mob_id": mob_id,
            "lineage": [
                {"identity": member, "agent_id": member, "role": "worker", "labels": {}},
            ],
        },
    }))
    .expect("a frame source that records its member")
}

/// A console of two mobs for the row below, as a gateway that just
/// started: nothing was asked of it yet. Each mob has a shared member
/// `open`; the second mob also has a member `kept` that is private to
/// another person. The console's frame log holds one frame of the launch
/// mob's `open` and one of each member of the second mob, each with its
/// member as the frame recorded it.
async fn console_with_frames_of_two_mobs() -> (
    Router,
    crate::unified_runtime::MobResolver,
    [(tempfile::TempDir, crate::UnifiedRuntime); 2],
) {
    let (launch_state, launch) =
        runtime_with_labelled_member("launch", "open", std::collections::BTreeMap::new()).await;
    let (second_state, second) =
        runtime_with_labelled_member("second", "open", std::collections::BTreeMap::new()).await;
    second
        .mob_handle()
        .spawn_spec(
            SpawnMemberSpec::from_wire("worker".to_string(), "kept".to_string(), None, None, None)
                .with_labels(std::collections::BTreeMap::from([(
                    crate::access::PRIVATE_TO_LABEL.to_string(),
                    "another-person".to_string(),
                )])),
        )
        .await
        .expect("the private member is spawned");
    let launch_mob = launch.mob_id();
    let second_mob = second.mob_id();
    let store = crate::console_aggregator::InMemoryConsoleLogStore::new();
    for (text, mob_id, member) in [
        (OF_THE_LAUNCH_MOBS_MEMBER, None, "open"),
        (
            OF_THE_SECOND_MOBS_SHARED_MEMBER,
            Some(second_mob.as_str()),
            "open",
        ),
        (
            OF_THE_SECOND_MOBS_PRIVATE_MEMBER,
            Some(second_mob.as_str()),
            "kept",
        ),
    ] {
        store
            .append_if_absent(crate::console_aggregator::NewConsoleFrame {
                identity: member.to_string(),
                conversation_id: Some(member.to_string()),
                session_id: Some(format!("{member}-session")),
                source: source_recording_the_member(member, mob_id.unwrap_or(launch_mob.as_str())),
                ..timeline_rows_frame(text, mob_id)
            })
            .await
            .expect("the frame is stored");
    }
    let state = ConsoleJsonState {
        console_aggregator: Some(MetaConsoleAggregator::new(Arc::new(store))),
        ..door_rows_state(&launch, &second_mob)
    };
    let (app, resolver) = console_of_two_mobs_over(state, &launch, &second);
    (
        app,
        resolver,
        [(launch_state, launch), (second_state, second)],
    )
}

/// The body of the answer of `request` on `app`, sent with a sign-in of
/// `email`: the whole body of a JSON answer, and of a stream what it sends
/// up to the end of its snapshot.
async fn timeline_answer_as(
    app: &Router,
    mut request: axum::http::Request<axum::body::Body>,
    email: &str,
) -> String {
    use futures::StreamExt;
    use tower::ServiceExt;

    request.headers_mut().insert(
        header::AUTHORIZATION,
        format!("Bearer {}", door_rows_token(email))
            .parse()
            .expect("header value"),
    );
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        app.clone().oneshot(request),
    )
    .await
    .expect("the door answers in time")
    .expect("the door answers");
    let mut body = response.into_body().into_data_stream();
    let mut text = String::new();
    while !text.contains("snapshot_complete") {
        match tokio::time::timeout(std::time::Duration::from_secs(5), body.next()).await {
            Ok(Some(Ok(chunk))) => text.push_str(&String::from_utf8_lossy(&chunk)),
            // The answer ended (a JSON answer), or sends nothing more.
            _ => break,
        }
    }
    text
}

/// A timeline read over every mob judges a frame of a loaded second mob's
/// member on the two routes as the request door judges it. On a gateway
/// that just started, a caller who may address both mobs, who is no
/// administrator, reads `GET /console/timeline` and
/// `GET /console/timeline/stream` with every mob selected before any
/// request of the second mob was served: each route shows the frame of
/// the second mob's shared member and not the frame of its member that is
/// private to another person, exactly as `meta/console/query_timeline`
/// with every mob selected does; an administrator is shown all three
/// frames. The routes resolve no mob.
#[tokio::test]
async fn a_timeline_route_over_every_mob_judges_a_second_mobs_frames_as_the_request_door_does() {
    let mut wrong = Vec::new();
    for route in [
        "/console/timeline?mobs=*",
        "/console/timeline/stream?mobs=*",
    ] {
        let (app, resolver, runtimes) = console_with_frames_of_two_mobs().await;
        let second_mob = runtimes[1].1.mob_id();
        let get = || {
            axum::http::Request::builder()
                .method("GET")
                .uri(route)
                .body(axum::body::Body::empty())
                .expect("GET request")
        };
        // The route first: nothing was asked of this console before.
        let on_the_route = frames_held(
            &timeline_answer_as(&app, get(), DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB).await,
        );
        let resolved_by_the_route = resolved(&resolver, &second_mob);
        let on_the_request_door = frames_held(
            &timeline_answer_as(
                &app,
                rpc_request("meta/console/query_timeline", &json!({"mobs": "*"})),
                DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB,
            )
            .await,
        );
        let of_an_administrator =
            frames_held(&timeline_answer_as(&app, get(), DOOR_ROWS_ADMIN).await);
        drop(app);
        for (_state, runtime) in runtimes {
            runtime.shutdown().await;
        }
        if on_the_route != [true, true, false] {
            wrong.push(format!(
                "{route}: a caller who may address both mobs is not shown the launch mob's \
                 frame, the second mob's shared member's frame and no frame of its private \
                 member: {on_the_route:?}"
            ));
        }
        if on_the_route != on_the_request_door {
            wrong.push(format!(
                "{route}: the route shows {on_the_route:?}, the request door \
                 {on_the_request_door:?}"
            ));
        }
        if of_an_administrator != [true, true, true] {
            wrong.push(format!(
                "{route}: an administrator is not shown all three frames: \
                 {of_an_administrator:?}"
            ));
        }
        if resolved_by_the_route {
            wrong.push(format!("{route}: the route resolved the second mob"));
        }
    }
    assert!(
        wrong.is_empty(),
        "a timeline route over every mob does not judge a second mob's frames as the request \
         door does:\n{}",
        wrong.join("\n")
    );
}

/// The text of the stored frame of the member of a mob of the host that no
/// rule lists for the caller of the row below.
const OF_THE_UNLISTED_MOBS_MEMBER: &str = "frame-of-the-unlisted-mobs-member";

/// A timeline read over every mob reads the mobs its caller may address
/// and no other. A host has its launch mob and two more mobs, each with a
/// shared member `open` that has one stored frame. A caller whose rules
/// list one of the two, who is no administrator, reads the timeline with
/// every mob selected on the route, on the stream and on the request door:
/// each answers the launch mob's frame and the frame of the mob the rules
/// list, and no frame of the other mob. An administrator is answered all
/// three frames, and without the selector the same caller is answered the
/// launch mob's frame alone.
#[tokio::test]
async fn a_timeline_read_over_every_mob_answers_the_one_of_two_hosted_mobs_its_caller_may_address()
{
    const MEMBER: &str = "open";
    let temp = tempfile::tempdir().expect("tempdir");
    let launch = launch_with_a_graph(&temp).await;
    let launch_mob = launch.mob_id();
    let listed = hosted_mob_of(&launch, definition_of_a_hosts_mob("listed")).await;
    let unlisted = hosted_mob_of(&launch, definition_of_a_hosts_mob("unlisted")).await;
    let member =
        || SpawnMemberSpec::from_wire("worker".to_string(), MEMBER.to_string(), None, None, None);
    let mut fixture = vec![
        Box::pin(launch.spawn(member()))
            .await
            .map(|_| ())
            .map_err(|error| format!("{error:?}")),
    ];
    for hosted in [&listed, &unlisted] {
        fixture.push(match launch.resolve_mob(Some(hosted)).await {
            Ok(mob) => {
                let handle = mob.mob_handle();
                Box::pin(handle.spawn_spec(member()))
                    .await
                    .map(|_| ())
                    .map_err(|error| format!("{error:?}"))
            }
            Err(error) => Err(format!("{error:?}")),
        });
    }
    let store = crate::console_aggregator::InMemoryConsoleLogStore::new();
    for (text, mob_id) in [
        (OF_THE_LAUNCH_MOBS_MEMBER, None),
        (OF_THE_SECOND_MOBS_SHARED_MEMBER, Some(listed.as_str())),
        (OF_THE_UNLISTED_MOBS_MEMBER, Some(unlisted.as_str())),
    ] {
        fixture.push(
            store
                .append_if_absent(crate::console_aggregator::NewConsoleFrame {
                    identity: MEMBER.to_string(),
                    conversation_id: Some(MEMBER.to_string()),
                    session_id: Some(format!("{MEMBER}-session")),
                    source: source_recording_the_member(
                        MEMBER,
                        mob_id.unwrap_or(launch_mob.as_str()),
                    ),
                    ..timeline_rows_frame(text, mob_id)
                })
                .await
                .map(|_| ())
                .map_err(|error| error.to_string()),
        );
    }
    let state = ConsoleJsonState {
        console_aggregator: Some(MetaConsoleAggregator::new(Arc::new(store))),
        ..door_rows_state(&launch, listed.as_str())
    };
    let app = console_json_router_with_state(state).layer(axum::Extension(launch.mob_resolver()));
    // Which of the three frames a body holds: the launch mob's, the listed
    // mob's, the unlisted mob's.
    let held = |body: &str| {
        [
            body.contains(OF_THE_LAUNCH_MOBS_MEMBER),
            body.contains(OF_THE_SECOND_MOBS_SHARED_MEMBER),
            body.contains(OF_THE_UNLISTED_MOBS_MEMBER),
        ]
    };
    let get = |uri: &str| {
        axum::http::Request::builder()
            .method("GET")
            .uri(uri)
            .body(axum::body::Body::empty())
            .expect("GET request")
    };

    let mut wrong = Vec::new();
    for door in [
        "/console/timeline?mobs=*",
        "/console/timeline/stream?mobs=*",
        "meta/console/query_timeline",
    ] {
        let request = || {
            if door.starts_with('/') {
                get(door)
            } else {
                rpc_request(door, &json!({"mobs": "*"}))
            }
        };
        let of_the_caller =
            held(&timeline_answer_as(&app, request(), DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB).await);
        let of_an_administrator = held(&timeline_answer_as(&app, request(), DOOR_ROWS_ADMIN).await);
        if of_the_caller != [true, true, false] {
            wrong.push(format!(
                "{door}: a caller who may address one of the two mobs is not answered the \
                 launch mob's frame, that mob's frame and no frame of the other mob: \
                 {of_the_caller:?}"
            ));
        }
        if of_an_administrator != [true, true, true] {
            wrong.push(format!(
                "{door}: an administrator is not answered all three frames: \
                 {of_an_administrator:?}"
            ));
        }
    }
    let without_the_selector = held(
        &timeline_answer_as(
            &app,
            get("/console/timeline"),
            DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB,
        )
        .await,
    );
    if without_the_selector != [true, false, false] {
        wrong.push(format!(
            "/console/timeline without the selector: the caller is not answered the launch \
             mob's frame alone: {without_the_selector:?}"
        ));
    }
    drop(app);
    if let Some(set) = launch.mob_runtime().agent_mob_mcp_state()
        && let Ok(handle) = set.handle_for(&unlisted).await
    {
        let _ = handle.shutdown().await;
    }
    shut_down_with_its_hosted_mob(&launch, &listed).await;
    if fixture.iter().any(Result::is_err) {
        wrong.push(format!(
            "the fixture (the three members, then the three frames): {fixture:?}"
        ));
    }
    assert!(
        wrong.is_empty(),
        "a timeline read over every mob does not answer the mobs its caller may address and \
         no other:\n{}",
        wrong.join("\n")
    );
}

/// The text of the live frame that ends the reading of the row below.
const THE_LAST_LIVE_FRAME: &str = "frame-that-ends-the-rows-reading";

/// What the stream `body` sends from here up to the text `until`, added to
/// `sent`. It stops where the stream ends or sends nothing more.
async fn read_stream_until(body: &mut axum::body::BodyDataStream, sent: &mut String, until: &str) {
    use futures::StreamExt;
    while !sent.contains(until) {
        match tokio::time::timeout(std::time::Duration::from_secs(5), body.next()).await {
            Ok(Some(Ok(chunk))) => sent.push_str(&String::from_utf8_lossy(&chunk)),
            _ => break,
        }
    }
}

/// A timeline stream over every mob judges a live frame of a member in the
/// frame's own mob, also when the launch mob has a member of that name and
/// the frame's mob was loaded after the stream opened. The launch mob has a
/// shared member `reviewer`. A caller who may address both mobs, who is no
/// administrator, opens `GET /console/timeline/stream` with every mob
/// selected. Then the host's mob set creates a second mob, which gets a
/// member `reviewer` that is private to another person; a frame of that
/// member is written, and after it a frame of the launch mob's member. The
/// stream sends the launch mob's frames and not the frame of the second
/// mob's member, as `meta/console/query_timeline` with every mob selected
/// answers the same caller afterwards; an administrator is answered all
/// three frames.
#[tokio::test]
async fn a_timeline_stream_over_every_mob_judges_a_live_frame_of_a_member_in_the_frames_own_mob() {
    use tower::ServiceExt;
    const SHARED_NAME: &str = "reviewer";

    let temp = tempfile::tempdir().expect("tempdir");
    let launch = launch_with_a_graph(&temp).await;
    let launch_mob = launch.mob_id();
    let hosted_definition = definition_of_a_hosts_mob("hosted");
    let hosted = hosted_definition.id.clone();
    let member = |labels: std::collections::BTreeMap<String, String>| {
        SpawnMemberSpec::from_wire(
            "worker".to_string(),
            SHARED_NAME.to_string(),
            None,
            None,
            None,
        )
        .with_labels(labels)
    };
    let of_the_launch_mob = Box::pin(launch.spawn(member(std::collections::BTreeMap::new())))
        .await
        .map(|_| ())
        .map_err(|error| format!("{error:?}"));
    let aggregator = MetaConsoleAggregator::new(Arc::new(
        crate::console_aggregator::InMemoryConsoleLogStore::new(),
    ));
    let state = ConsoleJsonState {
        console_aggregator: Some(aggregator.clone()),
        ..door_rows_state(&launch, hosted.as_str())
    };
    let resolver = launch.mob_resolver();
    let app = console_json_router_with_state(state).layer(axum::Extension(resolver.clone()));
    // A frame of the member `reviewer` of a mob, which recorded its member
    // as a worker with no label.
    let frame = |text: &str, mob_id: Option<&str>| crate::console_aggregator::NewConsoleFrame {
        identity: SHARED_NAME.to_string(),
        conversation_id: Some(SHARED_NAME.to_string()),
        session_id: Some(format!("{SHARED_NAME}-session")),
        source: source_recording_the_member(SHARED_NAME, mob_id.unwrap_or(launch_mob.as_str())),
        ..timeline_rows_frame(text, mob_id)
    };
    let mut written = vec![
        aggregator
            .store_and_emit_for_test(frame(OF_THE_LAUNCH_MOBS_MEMBER, None))
            .await
            .map_err(|error| error.to_string()),
    ];

    // The stream opens while the host has no second mob.
    let mut request = axum::http::Request::builder()
        .method("GET")
        .uri("/console/timeline/stream?mobs=*")
        .body(axum::body::Body::empty())
        .expect("GET request");
    request.headers_mut().insert(
        header::AUTHORIZATION,
        format!(
            "Bearer {}",
            door_rows_token(DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB)
        )
        .parse()
        .expect("header value"),
    );
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        app.clone().oneshot(request),
    )
    .await
    .expect("the stream opens in time")
    .expect("the stream opens");
    let status = response.status();
    let mut body = response.into_body().into_data_stream();
    let mut sent = String::new();
    read_stream_until(&mut body, &mut sent, "snapshot_complete").await;
    let loaded_when_the_stream_opened = resolver.resolve_if_loaded(&hosted).is_some();

    // The second mob, with its member that is private to another person
    // (through the mob's own handle: the runtime's spawn takes that label
    // from no caller).
    let created = hosted_mob_of(&launch, hosted_definition).await;
    let of_the_second_mob = match launch.resolve_mob(Some(&hosted)).await {
        Ok(mob) => {
            let handle = mob.mob_handle();
            Box::pin(handle.spawn_spec(member(std::collections::BTreeMap::from([(
                crate::access::PRIVATE_TO_LABEL.to_string(),
                "another-person".to_string(),
            )]))))
            .await
            .map(|_| ())
            .map_err(|error| format!("{error:?}"))
        }
        Err(error) => Err(format!("{error:?}")),
    };
    let loaded_after_it_was_created = resolver.resolve_if_loaded(&hosted).is_some();
    for (text, mob_id) in [
        (OF_THE_SECOND_MOBS_PRIVATE_MEMBER, Some(hosted.as_str())),
        (THE_LAST_LIVE_FRAME, None),
    ] {
        written.push(
            aggregator
                .store_and_emit_for_test(frame(text, mob_id))
                .await
                .map_err(|error| error.to_string()),
        );
    }
    read_stream_until(&mut body, &mut sent, THE_LAST_LIVE_FRAME).await;
    drop(body);
    let every_mob = || rpc_request("meta/console/query_timeline", &json!({"mobs": "*"}));
    let on_the_request_door =
        timeline_answer_as(&app, every_mob(), DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB).await;
    let of_an_administrator = timeline_answer_as(&app, every_mob(), DOOR_ROWS_ADMIN).await;
    drop(app);
    shut_down_with_its_hosted_mob(&launch, &hosted).await;

    // Which of the row's three frames a text holds: the launch mob's first
    // frame, the frame of the second mob's member, the last live frame.
    let frames = |text: &str| {
        [
            text.contains(OF_THE_LAUNCH_MOBS_MEMBER),
            text.contains(OF_THE_SECOND_MOBS_PRIVATE_MEMBER),
            text.contains(THE_LAST_LIVE_FRAME),
        ]
    };
    let mut wrong = Vec::new();
    if status != StatusCode::OK
        || of_the_launch_mob.is_err()
        || of_the_second_mob.is_err()
        || created != hosted
        || written.iter().any(Result::is_err)
    {
        wrong.push(format!(
            "the fixture: the stream opened {status}; the launch mob's member \
             {of_the_launch_mob:?}; the second mob {created} with its member \
             {of_the_second_mob:?}; the frames {written:?}"
        ));
    }
    if loaded_when_the_stream_opened || !loaded_after_it_was_created {
        wrong.push(format!(
            "the second mob is not loaded after the stream opened and only then: loaded at \
             the opening {loaded_when_the_stream_opened}, after its creation \
             {loaded_after_it_was_created}"
        ));
    }
    if frames(&of_an_administrator) != [true, true, true] {
        wrong.push(format!(
            "an administrator is not answered all three frames: {:?}",
            frames(&of_an_administrator)
        ));
    }
    if frames(&sent) != [true, false, true] {
        wrong.push(format!(
            "the stream does not send the launch mob's two frames and no frame of the second \
             mob's member that is private to another person: {:?}",
            frames(&sent)
        ));
    }
    if frames(&sent) != frames(&on_the_request_door) {
        wrong.push(format!(
            "the stream sends {:?}, the request door answers {:?}",
            frames(&sent),
            frames(&on_the_request_door)
        ));
    }
    assert!(
        wrong.is_empty(),
        "a timeline stream over every mob does not judge a live frame of a member in the \
         frame's own mob:\n{}",
        wrong.join("\n")
    );
}

/// Open `GET /console/timeline/stream` with every mob selected on `app` as
/// the holder of `email`, and read it up to the end of its snapshot: its
/// status, its body and what it sent so far.
async fn every_mob_stream_as(
    app: &axum::Router,
    email: &str,
) -> (StatusCode, axum::body::BodyDataStream, String) {
    use tower::ServiceExt;
    let mut request = axum::http::Request::builder()
        .method("GET")
        .uri("/console/timeline/stream?mobs=*")
        .body(axum::body::Body::empty())
        .expect("GET request");
    request.headers_mut().insert(
        header::AUTHORIZATION,
        format!("Bearer {}", door_rows_token(email))
            .parse()
            .expect("header value"),
    );
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        app.clone().oneshot(request),
    )
    .await
    .expect("the stream opens in time")
    .expect("the stream opens");
    let status = response.status();
    let mut body = response.into_body().into_data_stream();
    let mut sent = String::new();
    read_stream_until(&mut body, &mut sent, "snapshot_complete").await;
    (status, body, sent)
}

/// A timeline stream over every mob sends the live frames of a member of a
/// mob the host serves beside the launch mob, whether the stream opened
/// before that mob was created or after. The launch mob has a member
/// `reviewer`. A caller who may address both mobs, who is no
/// administrator, opens `GET /console/timeline/stream` with every mob
/// selected while the host has no second mob; the host's mob set creates
/// a second mob with a member `reviewer` that is private to no one, and
/// the caller opens a second such stream. A frame of the second mob's
/// member is written, and after it a frame of the launch mob's member.
/// Each stream sends the launch mob's two frames and the frame of the
/// second mob's member, as `meta/console/query_timeline` with every mob
/// selected answers the same caller.
#[tokio::test]
async fn a_timeline_stream_over_every_mob_sends_the_live_frames_of_a_member_of_another_mob() {
    const SHARED_NAME: &str = "reviewer";
    const OF_THE_SECOND_MOBS_MEMBER: &str = "frame-of-the-second-mobs-shared-member";

    let temp = tempfile::tempdir().expect("tempdir");
    let launch = launch_with_a_graph(&temp).await;
    let launch_mob = launch.mob_id();
    let hosted_definition = definition_of_a_hosts_mob("hosted");
    let hosted = hosted_definition.id.clone();
    let member = || {
        SpawnMemberSpec::from_wire(
            "worker".to_string(),
            SHARED_NAME.to_string(),
            None,
            None,
            None,
        )
    };
    let of_the_launch_mob = Box::pin(launch.spawn(member()))
        .await
        .map(|_| ())
        .map_err(|error| format!("{error:?}"));
    let aggregator = MetaConsoleAggregator::new(Arc::new(
        crate::console_aggregator::InMemoryConsoleLogStore::new(),
    ));
    let state = ConsoleJsonState {
        console_aggregator: Some(aggregator.clone()),
        ..door_rows_state(&launch, hosted.as_str())
    };
    let app = console_json_router_with_state(state).layer(axum::Extension(launch.mob_resolver()));
    // A frame of the member `reviewer` of a mob, which recorded its member
    // as a worker with no label.
    let frame = |text: &str, mob_id: Option<&str>| crate::console_aggregator::NewConsoleFrame {
        identity: SHARED_NAME.to_string(),
        conversation_id: Some(SHARED_NAME.to_string()),
        session_id: Some(format!("{SHARED_NAME}-session")),
        source: source_recording_the_member(SHARED_NAME, mob_id.unwrap_or(launch_mob.as_str())),
        ..timeline_rows_frame(text, mob_id)
    };
    let mut written = vec![
        aggregator
            .store_and_emit_for_test(frame(OF_THE_LAUNCH_MOBS_MEMBER, None))
            .await
            .map_err(|error| error.to_string()),
    ];

    // One stream opens while the host has no second mob, one after the
    // host's mob set created it with its member.
    let (opened_before, mut before, mut sent_before) =
        every_mob_stream_as(&app, DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB).await;
    let created = hosted_mob_of(&launch, hosted_definition).await;
    let of_the_second_mob = match launch.resolve_mob(Some(&hosted)).await {
        Ok(mob) => Box::pin(mob.mob_handle().spawn_spec(member()))
            .await
            .map(|_| ())
            .map_err(|error| format!("{error:?}")),
        Err(error) => Err(format!("{error:?}")),
    };
    let (opened_after, mut after, mut sent_after) =
        every_mob_stream_as(&app, DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB).await;
    for (text, mob_id) in [
        (OF_THE_SECOND_MOBS_MEMBER, Some(hosted.as_str())),
        (THE_LAST_LIVE_FRAME, None),
    ] {
        written.push(
            aggregator
                .store_and_emit_for_test(frame(text, mob_id))
                .await
                .map_err(|error| error.to_string()),
        );
    }
    read_stream_until(&mut before, &mut sent_before, THE_LAST_LIVE_FRAME).await;
    read_stream_until(&mut after, &mut sent_after, THE_LAST_LIVE_FRAME).await;
    drop(before);
    drop(after);
    let on_the_request_door = timeline_answer_as(
        &app,
        rpc_request("meta/console/query_timeline", &json!({"mobs": "*"})),
        DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB,
    )
    .await;
    drop(app);
    shut_down_with_its_hosted_mob(&launch, &hosted).await;

    // Which of the row's three frames a text holds: the launch mob's first
    // frame, the frame of the second mob's member, the last live frame.
    let frames = |text: &str| {
        [
            text.contains(OF_THE_LAUNCH_MOBS_MEMBER),
            text.contains(OF_THE_SECOND_MOBS_MEMBER),
            text.contains(THE_LAST_LIVE_FRAME),
        ]
    };
    let mut wrong = Vec::new();
    if opened_before != StatusCode::OK
        || opened_after != StatusCode::OK
        || of_the_launch_mob.is_err()
        || of_the_second_mob.is_err()
        || created != hosted
        || written.iter().any(Result::is_err)
    {
        wrong.push(format!(
            "the fixture: the streams opened {opened_before} and {opened_after}; the launch \
             mob's member {of_the_launch_mob:?}; the second mob {created} with its member \
             {of_the_second_mob:?}; the frames {written:?}"
        ));
    }
    if frames(&on_the_request_door) != [true, true, true] {
        wrong.push(format!(
            "the request door does not answer the caller all three frames: {:?}",
            frames(&on_the_request_door)
        ));
    }
    for (when, sent) in [("before", &sent_before), ("after", &sent_after)] {
        if frames(sent) != [true, true, true] {
            wrong.push(format!(
                "the stream opened {when} the second mob was created does not send all three \
                 frames: {:?}",
                frames(sent)
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "a timeline stream over every mob does not send the live frames of a member of \
         another mob:\n{}",
        wrong.join("\n")
    );
}

/// The host roster of the row below: one configured agent, so its runtime
/// serves identities.
struct OneConfiguredAgent;

#[async_trait::async_trait]
impl crate::identity_first::contracts::RosterProvider for OneConfiguredAgent {
    async fn roster(
        &self,
        _context: &crate::identity_first::RosterContext,
    ) -> Result<Vec<crate::identity_first::DurableAgentSpec>, crate::identity_first::RosterError>
    {
        Ok(vec![crate::identity_first::DurableAgentSpec {
            identity: crate::identity_first::AgentIdentity::parse("host-a")
                .expect("a configured agent's name"),
            profile: "worker".into(),
            addressability: crate::identity_first::AgentAddressability::Addressable,
            display_name: None,
            labels: std::collections::BTreeMap::new(),
            context: None,
            additional_instructions: Vec::new(),
            initial_message: None,
            runtime_mode_override: None,
            backend: None,
            binding: None,
            placement: None,
        }])
    }
}

/// What only the host plane states of a request for a long-term agent, who
/// proposed it (`proposer`) and the evidence of its source
/// (`source_evidence`), is not taken from a Console caller who is no
/// verified person. Such a caller's request is served as the host's on the
/// console surface, and what the caller states is no host's attestation.
/// On a console that asks no sign-in, over a runtime whose policy allows a
/// host's create: a create that states a proposer is refused at that
/// member, and so is one that states source evidence, each as the family
/// refuses a person's (`invalid_request`, naming the member); the same
/// create that states neither is carried out; and the mob's agents then
/// include the agent of the plain request and neither agent of a refused
/// one.
#[tokio::test]
async fn an_identity_request_of_a_caller_who_is_no_verified_person_states_nothing_of_the_host_plane()
 {
    use crate::identity_creation::{IdentityCreationPolicy, IdentityFamilyOptions};

    let temp = tempfile::tempdir().expect("tempdir");
    let owner = Box::pin(
        crate::unified_runtime::UnifiedRuntimeBuilder::default()
            .definition(
                meerkat_mob::MobDefinition::from_toml(&format!(
                    "[mob]\nid = \"console-door-host-plane-{}\"\n\n[profiles.worker]\nmodel = \
                     \"gpt-5.5\"\nexternal_addressable = true\nruntime_mode = \
                     \"turn_driven\"\n\n[profiles.worker.tools]\ncomms = true\n",
                    uuid::Uuid::new_v4().simple()
                ))
                .expect("mob definition"),
            )
            .persistent_state(temp.path())
            .continuity_from_state_dir(temp.path())
            .await
            .map_err(|error| error.to_string())
            .expect("the state directory keeps the identities")
            .roster_provider(Arc::new(OneConfiguredAgent))
            .identity_runtime_instance_id("console-door-host-plane-rows")
            .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
            .build(),
    )
    .await
    .map_err(|error| error.to_string())
    .expect("a runtime that serves identities");
    let policy = IdentityCreationPolicy::parse(&json!({
        "enabled": true,
        "approval": {"rules": [{
            "change": ["create"],
            "requester": {"kind": "host"},
            "target": {"profile": ["worker"]},
            "outcome": {"kind": "allow"},
        }]},
    }))
    .expect("the policy parses")
    .expect("the policy is enabled");
    let attached = owner.attach_identity_policy(Arc::new(policy), IdentityFamilyOptions::new());
    let runtime = owner.mob_runtime().clone();
    let identity_runtime = owner
        .identity_runtime_ref()
        .cloned()
        .expect("the runtime serves identities");
    let app = console_json_router_with_state(ConsoleJsonState {
        identity_runtime: Some(crate::identity_first::PublicIdentityRuntime::new(
            Arc::clone(&identity_runtime),
            runtime,
        )),
        ..state_over(&owner)
    });
    let create = |request_id: &str, name: &str, stated: Option<(&str, Value)>| {
        let mut params = json!({
            "request_id": request_id,
            "agents": [{"name": name, "profile": "worker"}],
        });
        if let Some((member, value)) = stated {
            params[member] = value;
        }
        let app = app.clone();
        async move {
            let answer = ask(&app, rpc_request("meta/identity/create", &params)).await;
            serde_json::from_str::<Value>(&answer.2).unwrap_or(Value::Null)
        }
    };

    let mut wrong = Vec::new();
    if !attached {
        wrong.push("the fixture: the runtime does not take the identity policy".to_string());
    }
    for (member, value, name) in [
        ("proposer", json!("someone@example.test"), "stated-proposer"),
        (
            "source_evidence",
            json!({
                "attestor": "an-adapter",
                "subject": "a-subject",
                "source_ref": "an-event",
                "observed_at_ms": 1,
            }),
            "stated-evidence",
        ),
    ] {
        let answer = create(&format!("states-{member}"), name, Some((member, value))).await;
        if !answer["result"].is_null()
            || answer["error"]["data"]["kind"] != json!("invalid_request")
            || answer["error"]["data"]["field"] != json!(member)
        {
            wrong.push(format!(
                "a create that states `{member}` is not refused at that member: {answer}"
            ));
        }
    }
    let plain = create("states-nothing", "kept-plain", None).await;
    if !plain["error"].is_null() || plain["result"].is_null() {
        wrong.push(format!(
            "the create that states neither is not carried out: {plain}"
        ));
    }
    let listed = ask(&app, rpc_request("meta/identity/list", &json!({})))
        .await
        .2;
    if !listed.contains("kept-plain")
        || listed.contains("stated-proposer")
        || listed.contains("stated-evidence")
    {
        wrong.push(format!(
            "the mob's agents are not the plain request's agent and neither agent of a \
             refused request: {listed}"
        ));
    }
    drop(app);
    owner.shutdown().await;
    assert!(
        wrong.is_empty(),
        "an identity request of a caller who is no verified person states something of the \
         host plane:\n{}",
        wrong.join("\n")
    );
}

/// The methods over the mobs of the host (`meta/mobs/*`) need the runtime
/// admin grant on a console that requires a sign-in and enforces access
/// rules. A signed-in person no rule names is refused on each of the six
/// with the door's access refusal, the reads too, and nothing is read or
/// changed. A person a rule gives the grant, and an administrator, are
/// answered by the family.
#[tokio::test]
async fn the_methods_over_the_mobs_of_the_host_are_refused_without_the_admin_grant() {
    let temp = tempfile::tempdir().expect("tempdir");
    let launch_mob = format!("door-rows-hosting-{}", uuid::Uuid::new_v4().simple());
    let second = format!("{launch_mob}-second");
    let launch = crate::rpc::mob_hosting_tests::host(&temp, &launch_mob).await;
    let app = console_json_router_with_state(door_rows_state(&launch, "a-mob-no-rule-is-asked-of"))
        .layer(axum::Extension(launch.mob_resolver()));
    let config = crate::rpc::mob_hosting_tests::config;
    let body_of =
        |answer: &DoorAnswer| -> Value { serde_json::from_str(&answer.2).unwrap_or(Value::Null) };

    let mut wrong = Vec::new();
    for (method, params) in [
        ("meta/mobs/list", json!({})),
        ("meta/mobs/get", json!({"mob_id": launch_mob})),
        ("meta/mobs/create", json!({"mob_config": config(&second)})),
        (
            "meta/mobs/lifecycle",
            json!({"mob_id": launch_mob, "action": "stop"}),
        ),
        (
            "meta/mobs/redefine",
            json!({"mob_id": launch_mob, "mob_config": config(&launch_mob), "expected_revision": 1}),
        ),
        ("meta/mobs/destroy", json!({"mob_id": launch_mob})),
    ] {
        let answer = ask_as(&app, rpc_request(method, &params), DOOR_ROWS_WITHOUT_RULES).await;
        let body = body_of(&answer);
        if body["error"]["code"] != json!(ACCESS_DENIED_RPC_CODE)
            || body["error"]["data"]["kind"] != json!("access_denied")
            || body["error"]["data"]["action"] != json!(crate::access::ACTION_RUNTIME_ADMIN)
        {
            wrong.push(format!(
                "{method}: a person without the admin grant is not refused by the access check: \
                 {} {:.200}",
                answer.0, answer.2
            ));
        }
    }
    let set = launch
        .mob_runtime()
        .agent_mob_mcp_state()
        .expect("the launch's mob set");
    let managed: Vec<String> = set
        .mob_inventory()
        .await
        .expect("the inventory reads")
        .managed
        .iter()
        .map(|(mob_id, observation)| format!("{mob_id}: {:?}", observation.phase))
        .collect();
    if managed != vec![format!("{launch_mob}: Running")] {
        wrong.push(format!(
            "something was created, stopped or destroyed for a person without the grant: the \
             host's mobs are {managed:?}"
        ));
    }
    drop(set);

    // A person a rule gives the grant reads the list.
    let listed = body_of(
        &ask_as(
            &app,
            rpc_request("meta/mobs/list", &json!({})),
            DOOR_ROWS_OPERATOR,
        )
        .await,
    );
    if listed["result"]["mobs"].as_array().map(Vec::len) != Some(1) {
        wrong.push(format!(
            "meta/mobs/list: a person with the grant is not served: {listed}"
        ));
    }
    // An administrator is answered by the family on each of the six. The
    // redefinition of a mob that lives in memory is the family's own
    // refusal, not the door's.
    for (method, params, served) in [
        ("meta/mobs/list", json!({}), "result"),
        (
            "meta/mobs/create",
            json!({"mob_config": config(&second)}),
            "result",
        ),
        ("meta/mobs/get", json!({"mob_id": second}), "result"),
        (
            "meta/mobs/lifecycle",
            json!({"mob_id": second, "action": "stop"}),
            "result",
        ),
        (
            "meta/mobs/redefine",
            json!({"mob_id": second, "mob_config": config(&second), "expected_revision": 1}),
            "error",
        ),
        ("meta/mobs/destroy", json!({"mob_id": second}), "result"),
    ] {
        let answer = ask_as(&app, rpc_request(method, &params), DOOR_ROWS_ADMIN).await;
        let body = body_of(&answer);
        if body.get(served).is_none()
            || body["error"]["code"] == json!(ACCESS_DENIED_RPC_CODE)
            || body["error"]["code"] == json!(-32601)
        {
            wrong.push(format!(
                "{method}: an administrator is not answered by the family: {} {:.200}",
                answer.0, answer.2
            ));
        }
    }

    drop(app);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(30), launch.shutdown()).await;
    assert!(
        wrong.is_empty(),
        "the methods over the mobs of the host on a console with access rules:\n{}",
        wrong.join("\n")
    );
}

/// A `meta/mobs/get` loads the mob it names when the host created it and it
/// is not loaded, and reads the mob's store when the host's records have
/// not: for a caller the access check refuses, neither happens. After a
/// start of the host, a signed-in person no rule names is refused by the
/// door's access check and the mob stays not loaded: the family was not
/// asked, so no store was opened for that caller. The same request by a
/// person who holds the admin grant and whom a rule lists the mob for is
/// answered with the mob's entry, its phase and its revision, and the mob
/// is loaded, stopped.
#[tokio::test]
async fn a_get_of_a_mob_that_is_not_loaded_opens_nothing_for_a_caller_without_the_admin_grant() {
    use crate::rpc::mob_hosting_tests::{config, end, result, stored_host};

    let temp = tempfile::tempdir().expect("tempdir");
    let launch_mob = format!("door-rows-get-{}", uuid::Uuid::new_v4().simple());
    let second = format!("{launch_mob}-second");
    {
        let earlier = stored_host(&temp, &launch_mob).await;
        result(
            &earlier,
            "meta/mobs/create",
            json!({"mob_config": config(&second)}),
        )
        .await;
        result(
            &earlier,
            "meta/mobs/lifecycle",
            json!({"mob_id": second, "action": "stop"}),
        )
        .await;
        end(earlier).await;
    }

    let launch = stored_host(&temp, &launch_mob).await;
    let app = console_json_router_with_state(door_rows_state(&launch, &second))
        .layer(axum::Extension(launch.mob_resolver()));
    let set = launch
        .mob_runtime()
        .agent_mob_mcp_state()
        .expect("the launch's mob set");
    let second_id = meerkat_mob::MobId::from(second.as_str());
    let phase_if_loaded = || async {
        set.mob_inventory()
            .await
            .expect("the inventory reads")
            .managed
            .iter()
            .find(|(mob_id, _)| mob_id == &second_id)
            .map(|(_, observation)| observation.phase)
    };
    let body_of =
        |answer: &DoorAnswer| -> Value { serde_json::from_str(&answer.2).unwrap_or(Value::Null) };
    assert_eq!(
        phase_if_loaded().await,
        None,
        "the fixture of this row is a host that started again and has not loaded the second mob"
    );

    let gets = || rpc_request("meta/mobs/get", &json!({"mob_id": second}));
    let refused = body_of(&ask_as(&app, gets(), DOOR_ROWS_WITHOUT_RULES).await);
    assert_eq!(
        refused["error"]["code"],
        json!(ACCESS_DENIED_RPC_CODE),
        "a person without the admin grant is refused by the access check: {refused}"
    );
    assert_eq!(
        refused["error"]["data"]["action"],
        json!(crate::access::ACTION_RUNTIME_ADMIN),
        "{refused}"
    );
    assert_eq!(
        phase_if_loaded().await,
        None,
        "nothing was loaded for the caller the access check refused"
    );

    let served = body_of(&ask_as(&app, gets(), DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB).await);
    assert_eq!(
        served["result"]["mob"],
        json!({
            "mob_id": second,
            "default": false,
            "state": "stopped",
            "provenance": "host",
            "revision": 1,
        }),
        "a person with the admin grant whom a rule lists the mob for is answered with the \
         mob's entry: {served}"
    );
    assert_eq!(
        phase_if_loaded().await,
        Some(meerkat_mob::MobState::Stopped),
        "the get of that person loaded the mob, stopped"
    );

    drop(app);
    drop(set);
    end(launch).await;
}

/// A change over the mobs of the host reads its caller's grants again right
/// before it is made. The caller here was given the admin grant when its
/// request was taken, and the grant is withdrawn before the change: the
/// read is answered on the view the request was taken with, the create is
/// refused with the door's access refusal and creates nothing. With the
/// grant still in place the same create is served: a separate rule lists
/// the mob the caller creates for it.
#[tokio::test]
async fn a_change_over_the_mobs_of_the_host_reads_its_callers_grants_again() {
    let temp = tempfile::tempdir().expect("tempdir");
    let launch_mob = format!("door-rows-regrant-{}", uuid::Uuid::new_v4().simple());
    let second = format!("{launch_mob}-second");
    let launch = crate::rpc::mob_hosting_tests::host(&temp, &launch_mob).await;
    let state = door_rows_state(&launch, "a-mob-no-rule-is-asked-of");
    let resolver = launch.mob_resolver();
    let config = crate::rpc::mob_hosting_tests::config;
    let granted = |with_the_grant: bool| crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![DOOR_ROWS_ADMIN.to_string()],
        rules: if with_the_grant {
            // The grant, by a rule that lists no mob, and a separate rule
            // that lists the mob the caller creates.
            let holds_the_grant = crate::access::AccessRule {
                id: "administers-the-runtime".to_string(),
                subjects: vec![DOOR_ROWS_OPERATOR.to_string()],
                actions: vec![crate::access::ACTION_RUNTIME_ADMIN.to_string()],
                ..crate::access::AccessRule::default()
            };
            vec![
                holds_the_grant.clone(),
                crate::access::AccessRule {
                    id: "administers-the-second-mob".to_string(),
                    mobs: vec![second.clone()],
                    ..holds_the_grant
                },
            ]
        } else {
            Vec::new()
        },
        ..crate::access::AccessControlConfig::default()
    };
    let controller = AccessController::new(granted(true)).expect("access configuration");
    // The caller as its request was taken: with the grant.
    let taken = controller.view_for_subject(Some(DOOR_ROWS_OPERATOR));
    let caller = ConsoleHttpAuthContext {
        principal: Some(DOOR_ROWS_OPERATOR.to_string()),
        service_identity: false,
        send_scope: ConsoleSendScope::Anonymous,
        access_view: Some(taken.clone()),
        audience: taken,
        human: None,
        verification: None,
    };
    let request = |method: &str, params: Value| JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!("row")),
        method: method.to_string(),
        params,
    };
    let creates = request("meta/mobs/create", json!({"mob_config": config(&second)}));
    let managed = || async {
        launch
            .mob_runtime()
            .agent_mob_mcp_state()
            .expect("the launch's mob set")
            .mob_inventory()
            .await
            .expect("the inventory reads")
            .managed
            .len()
    };

    controller
        .replace_config(granted(false))
        .expect("the grant is withdrawn");
    let listed = console_mob_hosting_rpc(
        &state,
        Some(&resolver),
        &caller,
        false,
        &request("meta/mobs/list", json!({})),
    )
    .await;
    assert!(
        listed["result"]["mobs"].is_array(),
        "a read is answered on the view its request was taken with: {listed}"
    );
    let refused = console_mob_hosting_rpc(&state, Some(&resolver), &caller, false, &creates).await;
    assert_eq!(
        refused["error"]["code"],
        json!(ACCESS_DENIED_RPC_CODE),
        "{refused}"
    );
    assert_eq!(
        refused["error"]["data"]["action"],
        json!(crate::access::ACTION_RUNTIME_ADMIN),
        "{refused}"
    );
    assert_eq!(managed().await, 1, "the refused create made no mob");

    controller
        .replace_config(granted(true))
        .expect("the grant is given again");
    let served = console_mob_hosting_rpc(&state, Some(&resolver), &caller, false, &creates).await;
    assert_eq!(
        served["result"]["mob"]["mob_id"],
        json!(second),
        "with the grant in place the create is served: {served}"
    );
    assert_eq!(managed().await, 2);

    let set = launch
        .mob_runtime()
        .agent_mob_mcp_state()
        .expect("the launch's mob set");
    if let Ok(handle) = set
        .handle_for(&meerkat_mob::MobId::from(second.as_str()))
        .await
    {
        let _ = handle.shutdown().await;
    }
    drop(set);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(30), launch.shutdown()).await;
}

/// What a mob set keeps of the mobs of its host and of the mob `mob_id`,
/// in one text, read without a load: the loaded mobs with their phases,
/// the held, the quarantined and the not loaded ones, and for `mob_id`,
/// when it is loaded, its incarnation and the revision of its definition.
async fn kept_of_the_hosts_mobs(
    set: &meerkat_mob_mcp::MobMcpState,
    mob_id: &meerkat_mob::MobId,
) -> String {
    let inventory = match set.mob_inventory().await {
        Ok(inventory) => inventory,
        Err(error) => return format!("the inventory does not read: {error}"),
    };
    let mut loaded: Vec<String> = inventory
        .managed
        .iter()
        .map(|(id, observation)| format!("{id}: {:?}", observation.phase))
        .collect();
    loaded.sort();
    let mut held: Vec<String> = inventory
        .held
        .iter()
        .map(|(id, _)| id.to_string())
        .collect();
    held.sort();
    let mut quarantined: Vec<String> = inventory
        .quarantined
        .iter()
        .map(|row| row.mob_id.to_string())
        .collect();
    quarantined.sort();
    let mut not_loaded: Vec<String> = inventory.dormant.iter().map(ToString::to_string).collect();
    not_loaded.sort();
    let handle = set
        .mob_handles_snapshot()
        .await
        .ok()
        .and_then(|handles| handles.into_iter().find(|(id, _)| id == mob_id));
    let of_the_mob = match handle {
        Some((_, handle)) => format!(
            "loaded, incarnation {:?}, revision {:?}",
            handle.incarnation(),
            handle.definition_revision().await.ok().flatten()
        ),
        None => "not loaded".to_string(),
    };
    format!(
        "loaded {loaded:?}; held {held:?}; quarantined {quarantined:?}; not loaded \
         {not_loaded:?}; {mob_id}: {of_the_mob}"
    )
}

/// The requests of the methods over the mobs of the host that name the mob
/// `mob_id`, each with a word for it: the read, the stop, the resume, the
/// redefinition, a create under the id, and the destroy last.
fn requests_over_the_mob(mob_id: &str) -> Vec<(&'static str, &'static str, Value)> {
    let config = crate::rpc::mob_hosting_tests::config;
    vec![
        ("get", "meta/mobs/get", json!({"mob_id": mob_id})),
        (
            "stop",
            "meta/mobs/lifecycle",
            json!({"mob_id": mob_id, "action": "stop"}),
        ),
        (
            "resume",
            "meta/mobs/lifecycle",
            json!({"mob_id": mob_id, "action": "resume"}),
        ),
        (
            "redefine",
            "meta/mobs/redefine",
            json!({"mob_id": mob_id, "mob_config": config(mob_id), "expected_revision": 1}),
        ),
        (
            "create",
            "meta/mobs/create",
            json!({"mob_config": config(mob_id)}),
        ),
        ("destroy", "meta/mobs/destroy", json!({"mob_id": mob_id})),
    ]
}

/// The ids `meta/mobs/list` answers `email` on `app`, in the answer's
/// order, or the whole answer when it is no list.
async fn mobs_listed_to(app: &Router, email: &str) -> Result<Vec<String>, String> {
    let answer = ask_as(app, rpc_request("meta/mobs/list", &json!({})), email).await;
    let body: Value = serde_json::from_str(&answer.2).unwrap_or(Value::Null);
    body["result"]["mobs"]
        .as_array()
        .map(|mobs| {
            mobs.iter()
                .map(|mob| mob["mob_id"].as_str().unwrap_or("<no id>").to_string())
                .collect()
        })
        .ok_or(answer.2)
}

/// The methods over the mobs of the host serve the callers who may address
/// the mob a request names: an administrator for every mob, and a person
/// who holds the admin grant for the mob an allow rule lists for them.
/// Such a person creates the mob under the id the rule lists, reads it,
/// stops it, replaces its definition, resumes it and destroys it, and its
/// list shows the launch mob and that mob and no other. An administrator
/// does the same with a mob no rule lists and is listed every mob. On a
/// console that enforces no access rules every caller is served the same
/// requests for a mob of its own and is listed that mob.
#[tokio::test]
async fn the_methods_over_the_mobs_of_the_host_serve_a_caller_who_may_address_the_mob() {
    use crate::rpc::mob_hosting_tests::{config, end, stored_host};

    let temp = tempfile::tempdir().expect("tempdir");
    let launch_mob = format!("door-rows-serves-{}", uuid::Uuid::new_v4().simple());
    let listed_by_a_rule = format!("{launch_mob}-second");
    let listed_by_no_rule = format!("{launch_mob}-third");
    let launch = stored_host(&temp, &launch_mob).await;
    let app = console_json_router_with_state(door_rows_state(&launch, &listed_by_a_rule))
        .layer(axum::Extension(launch.mob_resolver()));
    let with_a_reviewer = |mob_id: &str| {
        format!(
            "{}\n[profiles.reviewer]\nmodel = \"gpt-5.5\"\n\n[profiles.reviewer.tools]\ncomms = \
             true\n",
            config(mob_id)
        )
    };
    let manages = |mob_id: &str| {
        vec![
            (
                "create",
                "meta/mobs/create",
                json!({"mob_config": config(mob_id)}),
            ),
            ("get", "meta/mobs/get", json!({"mob_id": mob_id})),
            (
                "stop",
                "meta/mobs/lifecycle",
                json!({"mob_id": mob_id, "action": "stop"}),
            ),
            (
                "redefine",
                "meta/mobs/redefine",
                json!({
                    "mob_id": mob_id,
                    "mob_config": with_a_reviewer(mob_id),
                    "expected_revision": 1,
                }),
            ),
            (
                "resume",
                "meta/mobs/lifecycle",
                json!({"mob_id": mob_id, "action": "resume"}),
            ),
        ]
    };

    let mut wrong = Vec::new();
    for (who, email, mob_id) in [
        (
            "a person a rule lists the mob for",
            DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB,
            &listed_by_a_rule,
        ),
        ("an administrator", DOOR_ROWS_ADMIN, &listed_by_no_rule),
    ] {
        for (what, method, params) in manages(mob_id) {
            let answer = ask_as(&app, rpc_request(method, &params), email).await;
            let body: Value = serde_json::from_str(&answer.2).unwrap_or(Value::Null);
            if body["result"]["mob"]["mob_id"] != json!(mob_id) {
                wrong.push(format!(
                    "{who}, {what}: not served: {} {:.300}",
                    answer.0, answer.2
                ));
            }
        }
    }
    let launch_and = |others: &[&String]| -> Result<Vec<String>, String> {
        Ok(std::iter::once(&launch_mob)
            .chain(others.iter().copied())
            .cloned()
            .collect())
    };
    let listed = [
        (
            "a person a rule lists one mob for is listed the launch mob and that mob",
            mobs_listed_to(&app, DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB).await,
            launch_and(&[&listed_by_a_rule]),
        ),
        (
            "an administrator is listed every mob",
            mobs_listed_to(&app, DOOR_ROWS_ADMIN).await,
            launch_and(&[&listed_by_a_rule, &listed_by_no_rule]),
        ),
    ];
    for (what, found, expected) in listed {
        if found != expected {
            wrong.push(format!("list: {what}: {found:?}"));
        }
    }
    for (who, email, mob_id) in [
        (
            "a person a rule lists the mob for",
            DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB,
            &listed_by_a_rule,
        ),
        ("an administrator", DOOR_ROWS_ADMIN, &listed_by_no_rule),
    ] {
        let answer = ask_as(
            &app,
            rpc_request("meta/mobs/destroy", &json!({"mob_id": mob_id})),
            email,
        )
        .await;
        let body: Value = serde_json::from_str(&answer.2).unwrap_or(Value::Null);
        if body["result"]["mob_id"] != json!(mob_id) {
            wrong.push(format!(
                "{who}, destroy: not served: {} {:.300}",
                answer.0, answer.2
            ));
        }
    }

    // A console that enforces no access rules: every caller may address
    // every mob, and the family serves each request.
    let open = console_json_router_with_state(state_over(&launch))
        .layer(axum::Extension(launch.mob_resolver()));
    let on_an_open_console = format!("{launch_mob}-fourth");
    for (what, method, params) in manages(&on_an_open_console) {
        let answer = ask(&open, rpc_request(method, &params)).await;
        let body: Value = serde_json::from_str(&answer.2).unwrap_or(Value::Null);
        if body["result"]["mob"]["mob_id"] != json!(on_an_open_console) {
            wrong.push(format!(
                "a console without access rules, {what}: not served: {} {:.300}",
                answer.0, answer.2
            ));
        }
    }
    let answer = ask(&open, rpc_request("meta/mobs/list", &json!({}))).await;
    let body: Value = serde_json::from_str(&answer.2).unwrap_or(Value::Null);
    if body["result"]["mobs"].as_array().map(Vec::len) != Some(2) {
        wrong.push(format!(
            "a console without access rules, list: not the launch mob and the mob it created: \
             {:.300}",
            answer.2
        ));
    }
    let answer = ask(
        &open,
        rpc_request("meta/mobs/destroy", &json!({"mob_id": on_an_open_console})),
    )
    .await;
    let body: Value = serde_json::from_str(&answer.2).unwrap_or(Value::Null);
    if body["result"]["mob_id"] != json!(on_an_open_console) {
        wrong.push(format!(
            "a console without access rules, destroy: not served: {:.300}",
            answer.2
        ));
    }

    drop(open);
    drop(app);
    end(launch).await;
    assert!(
        wrong.is_empty(),
        "the methods over the mobs of the host for callers who may address the mob:\n{}",
        wrong.join("\n")
    );
}

/// A caller who holds the admin grant and may not address a mob is refused
/// every method over the mobs of the host that names it, with the door's
/// one answer for a mob id it does not serve to its caller: the answer an
/// id no mob has gets, byte for byte apart from the id. So it is for a
/// person no rule lists a mob for, and for a person a rule lists another
/// mob for: the read, the stop, the resume, the redefinition, a create
/// under the id and the destroy. After each refusal the host has the mobs
/// it had in the phases they were in, the mob is the incarnation it was at
/// the revision it was at, and the files of its store and of its memory
/// are the files that were there. The list of such a caller leaves the mob
/// out. A person a rule lists the mob for who does not hold the grant is
/// refused by the access check, with the door's access refusal.
#[tokio::test]
async fn a_caller_who_may_not_address_a_mob_is_refused_its_management_like_an_id_no_mob_has() {
    use crate::unified_runtime::mob_memory_release_tests::HostOfARememberingMob;

    let (fixture, launch) = HostOfARememberingMob::launched().await;
    let launch_mob = launch.mob_id().clone();
    let listed_by_a_rule = fixture.mob.to_string();
    // An id that sorts after the other mob's and is no part of its name.
    let listed_by_no_rule = format!("mob-memory-third-{}", uuid::Uuid::new_v4().simple());
    crate::rpc::mob_hosting_tests::result(
        &launch,
        "meta/mobs/create",
        json!({"mob_config": crate::rpc::mob_hosting_tests::config(&listed_by_no_rule)}),
    )
    .await;
    let app = console_json_router_with_state(door_rows_state(&launch, &listed_by_a_rule))
        .layer(axum::Extension(launch.mob_resolver()));
    let set = launch
        .mob_runtime()
        .agent_mob_mcp_state()
        .expect("the launch's mob set");
    // The files of the fixture's mob, by name: of its memory and of its
    // store, without the two files a database keeps beside itself while a
    // reader or a writer has it open.
    let file_names = || -> Vec<std::path::PathBuf> {
        fixture
            .memory_files_of_the_mob()
            .into_keys()
            .chain(fixture.state_files_of_the_mob().into_keys())
            .filter(|path| {
                let name = path.to_string_lossy();
                !name.ends_with("-shm") && !name.ends_with("-wal")
            })
            .collect()
    };

    let mut wrong = Vec::new();
    for (who, email, mob_id) in [
        (
            "a person no rule lists a mob for",
            DOOR_ROWS_OPERATOR,
            &listed_by_a_rule,
        ),
        (
            "a person a rule lists another mob for",
            DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB,
            &listed_by_no_rule,
        ),
    ] {
        let named = meerkat_mob::MobId::from(mob_id.as_str());
        let absent = an_id_no_mob_has(mob_id);
        for ((what, method, params), (_, _, of_the_absent)) in requests_over_the_mob(mob_id)
            .into_iter()
            .zip(requests_over_the_mob(&absent))
        {
            let before = (kept_of_the_hosts_mobs(&set, &named).await, file_names());
            let answer = ask_as(&app, rpc_request(method, &params), email).await;
            if !is_the_mob_ids_refusal("console RPC", &answer, mob_id) {
                wrong.push(format!(
                    "{who}, {what}: not refused as a mob id the door does not serve to its \
                     caller: {} {:.300}",
                    answer.0, answer.2
                ));
            }
            let after = (kept_of_the_hosts_mobs(&set, &named).await, file_names());
            if after != before {
                wrong.push(format!(
                    "{who}, {what}: the host does not keep what it kept: before {before:?}, \
                     after {after:?}"
                ));
            }
            let for_the_absent = ask_as(&app, rpc_request(method, &of_the_absent), email).await;
            if apart_from_the_id(&answer, mob_id) != apart_from_the_id(&for_the_absent, &absent) {
                wrong.push(format!(
                    "{who}, {what}: not the answer the same caller gets for an id no mob has: \
                     {:.300} and {:.300}",
                    answer.2, for_the_absent.2
                ));
            }
            if what == "create" {
                // An administrator's create under an id no mob has is
                // served: there is no refusal of it to compare with.
                continue;
            }
            let to_an_administrator =
                ask_as(&app, rpc_request(method, &of_the_absent), DOOR_ROWS_ADMIN).await;
            if apart_from_the_id(&answer, mob_id)
                != apart_from_the_id(&to_an_administrator, &absent)
            {
                wrong.push(format!(
                    "{who}, {what}: not the answer an administrator gets for an id no mob has: \
                     {:.300} and {:.300}",
                    answer.2, to_an_administrator.2
                ));
            }
        }
    }

    let launch_and = |others: &[&String]| -> Result<Vec<String>, String> {
        Ok(std::iter::once(&launch_mob)
            .chain(others.iter().copied())
            .cloned()
            .collect())
    };
    for (what, found, expected) in [
        (
            "a person no rule lists a mob for is listed the launch mob alone",
            mobs_listed_to(&app, DOOR_ROWS_OPERATOR).await,
            launch_and(&[]),
        ),
        (
            "a person a rule lists one mob for is listed the launch mob and that mob",
            mobs_listed_to(&app, DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB).await,
            launch_and(&[&listed_by_a_rule]),
        ),
        (
            "an administrator is listed every mob",
            mobs_listed_to(&app, DOOR_ROWS_ADMIN).await,
            launch_and(&[&listed_by_a_rule, &listed_by_no_rule]),
        ),
    ] {
        if found != expected {
            wrong.push(format!("list: {what}: {found:?}"));
        }
    }
    for (what, method, params) in [
        ("list", "meta/mobs/list", json!({})),
        ("get", "meta/mobs/get", json!({"mob_id": listed_by_a_rule})),
    ] {
        let answer = ask_as(
            &app,
            rpc_request(method, &params),
            DOOR_ROWS_OF_THE_SECOND_MOB,
        )
        .await;
        let body: Value = serde_json::from_str(&answer.2).unwrap_or(Value::Null);
        if body["error"]["code"] != json!(ACCESS_DENIED_RPC_CODE)
            || body["error"]["data"]["action"] != json!(crate::access::ACTION_RUNTIME_ADMIN)
        {
            wrong.push(format!(
                "a person a rule lists the mob for who does not hold the grant, {what}: not \
                 refused by the access check: {:.300}",
                answer.2
            ));
        }
    }

    drop(app);
    if let Ok(handle) = set
        .handle_for(&meerkat_mob::MobId::from(listed_by_no_rule.as_str()))
        .await
    {
        let _ = handle.shutdown().await;
    }
    drop(set);
    fixture.ends(launch).await;
    assert!(
        wrong.is_empty(),
        "the methods over the mobs of the host for callers who may not address the mob:\n{}",
        wrong.join("\n")
    );
}

/// A request over a mob of the host that is refused for its caller loads
/// nothing and opens nothing. After a start of the host the mob beside the
/// launch mob is known and not loaded, and its agent's memory and its
/// store are on disk. A person who holds the admin grant and whom no rule
/// lists the mob for asks the list, the read, the stop, the resume, the
/// redefinition, a create under the id and the destroy: after each the mob
/// is still not loaded, no file of its memory is open, and the files of
/// its store and of its memory are byte for byte what they were. The read
/// of an administrator then loads the mob: nothing else kept it from
/// loading.
#[tokio::test]
async fn a_refused_request_over_a_mob_that_is_not_loaded_loads_and_opens_nothing() {
    use crate::unified_runtime::mob_memory_release_tests::HostOfARememberingMob;

    let (fixture, first) = HostOfARememberingMob::launched().await;
    let launch = fixture.started_again(first).await;
    let launch_mob = launch.mob_id().clone();
    let mob_id = fixture.mob.to_string();
    let app = console_json_router_with_state(door_rows_state(&launch, "a-mob-no-rule-is-asked-of"))
        .layer(axum::Extension(launch.mob_resolver()));
    let set = launch
        .mob_runtime()
        .agent_mob_mcp_state()
        .expect("the launch's mob set");
    let at_rest = (
        kept_of_the_hosts_mobs(&set, &fixture.mob).await,
        fixture.memory_files_of_the_mob(),
        fixture.state_files_of_the_mob(),
        fixture.memory_files_open_of_the_mob(),
    );
    assert!(
        at_rest.0.ends_with(&format!("{mob_id}: not loaded")),
        "the fixture of this row is a host that started again and has not loaded the mob: {}",
        at_rest.0
    );
    assert!(
        !at_rest.1.is_empty() && !at_rest.2.is_empty(),
        "the mob's memory and its store are on disk: {:?} and {:?}",
        at_rest.1.keys(),
        at_rest.2.keys()
    );

    let mut wrong = Vec::new();
    let listed = mobs_listed_to(&app, DOOR_ROWS_OPERATOR).await;
    if listed != Ok(vec![launch_mob]) {
        wrong.push(format!(
            "list: a person no rule lists a mob for is not listed the launch mob alone: \
             {listed:?}"
        ));
    }
    for (what, method, params) in requests_over_the_mob(&mob_id) {
        let answer = ask_as(&app, rpc_request(method, &params), DOOR_ROWS_OPERATOR).await;
        if !is_the_mob_ids_refusal("console RPC", &answer, &mob_id) {
            wrong.push(format!(
                "{what}: not refused as a mob id the door does not serve to its caller: {} \
                 {:.300}",
                answer.0, answer.2
            ));
        }
        let kept = kept_of_the_hosts_mobs(&set, &fixture.mob).await;
        if kept != at_rest.0 {
            wrong.push(format!(
                "{what}: the host does not keep what it kept: before {}, after {kept}",
                at_rest.0
            ));
        }
        if fixture.memory_files_open_of_the_mob() != at_rest.3 {
            wrong.push(format!("{what}: a file of the mob's memory was opened"));
        }
        if fixture.memory_files_of_the_mob() != at_rest.1 {
            wrong.push(format!("{what}: the files of the mob's memory changed"));
        }
        if fixture.state_files_of_the_mob() != at_rest.2 {
            wrong.push(format!("{what}: the files of the mob's store changed"));
        }
    }

    let read = ask_as(
        &app,
        rpc_request("meta/mobs/get", &json!({"mob_id": mob_id})),
        DOOR_ROWS_ADMIN,
    )
    .await;
    let body: Value = serde_json::from_str(&read.2).unwrap_or(Value::Null);
    let kept = kept_of_the_hosts_mobs(&set, &fixture.mob).await;
    if body["result"]["mob"]["mob_id"] != json!(mob_id) || kept.ends_with(": not loaded") {
        wrong.push(format!(
            "the read of an administrator did not load the mob: {:.300}; {kept}",
            read.2
        ));
    }

    drop(app);
    drop(set);
    fixture.ends(launch).await;
    assert!(
        wrong.is_empty(),
        "refused requests over a mob of the host that is not loaded:\n{}",
        wrong.join("\n")
    );
}

/// A change over a mob of the host judges the mob with the caller's rules
/// as they stand right before the change. The caller here holds the admin
/// grant, and a rule listed the mob for it when its request was taken;
/// that rule is withdrawn before the change, and the grant stays. The read
/// is answered on the view the request was taken with. The stop, the
/// redefinition, a create under the id and the destroy are refused like an
/// id no mob has, and the mob is the incarnation it was, running. With the
/// rule in place again the stop is served.
#[tokio::test]
async fn a_change_over_a_mob_of_the_host_judges_the_mob_with_its_callers_rules_read_again() {
    let temp = tempfile::tempdir().expect("tempdir");
    let launch_mob = format!("door-rows-rejudge-{}", uuid::Uuid::new_v4().simple());
    let second = format!("{launch_mob}-second");
    let launch = crate::rpc::mob_hosting_tests::host(&temp, &launch_mob).await;
    crate::rpc::mob_hosting_tests::result(
        &launch,
        "meta/mobs/create",
        json!({"mob_config": crate::rpc::mob_hosting_tests::config(&second)}),
    )
    .await;
    let state = door_rows_state(&launch, "a-mob-no-rule-is-asked-of");
    let resolver = launch.mob_resolver();
    let ruled = |lists_the_mob: bool| {
        let holds_the_grant = crate::access::AccessRule {
            id: "administers-the-runtime".to_string(),
            subjects: vec![DOOR_ROWS_OPERATOR.to_string()],
            actions: vec![crate::access::ACTION_RUNTIME_ADMIN.to_string()],
            ..crate::access::AccessRule::default()
        };
        let mut rules = vec![holds_the_grant.clone()];
        if lists_the_mob {
            rules.push(crate::access::AccessRule {
                id: "administers-the-second-mob".to_string(),
                mobs: vec![second.clone()],
                ..holds_the_grant
            });
        }
        crate::access::AccessControlConfig {
            enabled: true,
            admins: vec![DOOR_ROWS_ADMIN.to_string()],
            rules,
            ..crate::access::AccessControlConfig::default()
        }
    };
    let controller = AccessController::new(ruled(true)).expect("access configuration");
    // The caller as its request was taken: a rule lists the mob for it.
    let taken = controller.view_for_subject(Some(DOOR_ROWS_OPERATOR));
    let caller = ConsoleHttpAuthContext {
        principal: Some(DOOR_ROWS_OPERATOR.to_string()),
        service_identity: false,
        send_scope: ConsoleSendScope::Anonymous,
        access_view: Some(taken.clone()),
        audience: taken,
        human: None,
        verification: None,
    };
    let asks = |method: &'static str, params: Value| {
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!("row")),
            method: method.to_string(),
            params,
        };
        let (state, resolver, caller) = (&state, &resolver, &caller);
        async move { console_mob_hosting_rpc(state, Some(resolver), caller, false, &request).await }
    };
    let set = launch
        .mob_runtime()
        .agent_mob_mcp_state()
        .expect("the launch's mob set");
    let second_id = meerkat_mob::MobId::from(second.as_str());
    let refused_like_an_id_no_mob_has = json!({
        "jsonrpc": "2.0",
        "id": "row",
        "error": crate::rpc::mob_addressing::mob_not_served(&second),
    });

    controller
        .replace_config(ruled(false))
        .expect("the rule that lists the mob is withdrawn");
    let mut wrong = Vec::new();
    let read = asks("meta/mobs/get", json!({"mob_id": second})).await;
    if read["result"]["mob"]["mob_id"] != json!(second) {
        wrong.push(format!(
            "get: a read is not answered on the view its request was taken with: {read}"
        ));
    }
    let before = kept_of_the_hosts_mobs(&set, &second_id).await;
    for (what, method, params) in requests_over_the_mob(&second) {
        if matches!(what, "get" | "resume") {
            continue;
        }
        let answer = asks(method, params).await;
        if answer != refused_like_an_id_no_mob_has {
            wrong.push(format!(
                "{what}: not refused like an id no mob has once the rule is withdrawn: {answer}"
            ));
        }
        let after = kept_of_the_hosts_mobs(&set, &second_id).await;
        if after != before {
            wrong.push(format!(
                "{what}: the host does not keep what it kept: before {before}, after {after}"
            ));
        }
    }

    controller
        .replace_config(ruled(true))
        .expect("the rule that lists the mob is given again");
    let stopped = asks(
        "meta/mobs/lifecycle",
        json!({"mob_id": second, "action": "stop"}),
    )
    .await;
    if stopped["result"]["mob"]["state"] != json!("stopped") {
        wrong.push(format!(
            "stop: not served with the rule in place again: {stopped}"
        ));
    }

    if let Ok(handle) = set.handle_for(&second_id).await {
        let _ = handle.shutdown().await;
    }
    drop(set);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(30), launch.shutdown()).await;
    assert!(
        wrong.is_empty(),
        "changes over a mob of the host after the rule that lists it was withdrawn:\n{}",
        wrong.join("\n")
    );
}

/// A deny rule that lists a mob does not keep the methods over the mobs of
/// the host from a caller an allow rule lists that mob for. A mob is a
/// caller's to address by an allow rule that lists it, and the family's one
/// check, the admin grant, names no agent, which a rule that lists mobs
/// never matches. So a person who holds the grant by a rule that lists no
/// mob, whom an allow rule lists the mob for and whom a deny rule of every
/// action lists the same mob for, reads the mob, stops it and resumes it.
/// A person whom the deny alone lists the mob for is refused like an id no
/// mob has: no allow rule lists the mob for that person. What keeps the
/// family from a person is a deny of the grant that lists no mob: that
/// person is answered the door's access refusal on the list and the read.
#[tokio::test]
async fn a_deny_that_lists_a_mob_does_not_keep_its_management_from_a_caller_an_allow_lists_it_for()
{
    const LISTED_AND_DENIED: &str = "listed-and-denied@example.test";
    const DENIED_ALONE: &str = "denied-alone@example.test";
    const DENIED_THE_GRANT: &str = "denied-the-grant@example.test";

    let temp = tempfile::tempdir().expect("tempdir");
    let launch_mob = format!("door-rows-deny-{}", uuid::Uuid::new_v4().simple());
    let second = format!("{launch_mob}-second");
    let launch = crate::rpc::mob_hosting_tests::host(&temp, &launch_mob).await;
    crate::rpc::mob_hosting_tests::result(
        &launch,
        "meta/mobs/create",
        json!({"mob_config": crate::rpc::mob_hosting_tests::config(&second)}),
    )
    .await;
    let state = door_rows_state(&launch, "a-mob-no-rule-is-asked-of");
    let resolver = launch.mob_resolver();
    let people = |names: &[&str]| -> Vec<String> {
        names.iter().map(|person| (*person).to_string()).collect()
    };
    let controller = AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![DOOR_ROWS_ADMIN.to_string()],
        rules: vec![
            crate::access::AccessRule {
                id: "administers-the-runtime".to_string(),
                subjects: people(&[LISTED_AND_DENIED, DENIED_ALONE, DENIED_THE_GRANT]),
                actions: vec![crate::access::ACTION_RUNTIME_ADMIN.to_string()],
                ..crate::access::AccessRule::default()
            },
            crate::access::AccessRule {
                id: "sees-the-second-mob".to_string(),
                subjects: people(&[LISTED_AND_DENIED, DENIED_THE_GRANT]),
                actions: vec![crate::access::ACTION_AGENT_VIEW.to_string()],
                mobs: vec![second.clone()],
                ..crate::access::AccessRule::default()
            },
            crate::access::AccessRule {
                id: "nothing-in-the-second-mob".to_string(),
                effect: crate::access::AccessEffect::Deny,
                subjects: people(&[LISTED_AND_DENIED, DENIED_ALONE]),
                actions: vec!["*".to_string()],
                mobs: vec![second.clone()],
                ..crate::access::AccessRule::default()
            },
            crate::access::AccessRule {
                id: "no-administration-of-the-runtime".to_string(),
                effect: crate::access::AccessEffect::Deny,
                subjects: people(&[DENIED_THE_GRANT]),
                actions: vec![crate::access::ACTION_RUNTIME_ADMIN.to_string()],
                ..crate::access::AccessRule::default()
            },
        ],
        ..crate::access::AccessControlConfig::default()
    })
    .expect("access configuration");
    let asks = |person: &'static str, method: &'static str, params: Value| {
        let view = controller.view_for_subject(Some(person));
        let caller = ConsoleHttpAuthContext {
            principal: Some(person.to_string()),
            service_identity: false,
            send_scope: ConsoleSendScope::Anonymous,
            access_view: Some(view.clone()),
            audience: view,
            human: None,
            verification: None,
        };
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(json!("row")),
            method: method.to_string(),
            params,
        };
        let (state, resolver) = (&state, &resolver);
        async move { console_mob_hosting_rpc(state, Some(resolver), &caller, false, &request).await }
    };
    let set = launch
        .mob_runtime()
        .agent_mob_mcp_state()
        .expect("the launch's mob set");
    let second_id = meerkat_mob::MobId::from(second.as_str());

    let mut wrong = Vec::new();
    let read = asks(
        LISTED_AND_DENIED,
        "meta/mobs/get",
        json!({"mob_id": second}),
    )
    .await;
    if read["result"]["mob"]["mob_id"] != json!(second) {
        wrong.push(format!(
            "get: not served to a person an allow lists the mob for and a deny lists it for: {read}"
        ));
    }
    for (action, state_after) in [("stop", "stopped"), ("resume", "running")] {
        let changed = asks(
            LISTED_AND_DENIED,
            "meta/mobs/lifecycle",
            json!({"mob_id": second, "action": action}),
        )
        .await;
        if changed["result"]["mob"]["state"] != json!(state_after) {
            wrong.push(format!(
                "{action}: not served to a person an allow lists the mob for and a deny lists it \
                 for: {changed}"
            ));
        }
    }

    let before = kept_of_the_hosts_mobs(&set, &second_id).await;
    let refused_like_an_id_no_mob_has = json!({
        "jsonrpc": "2.0",
        "id": "row",
        "error": crate::rpc::mob_addressing::mob_not_served(&second),
    });
    for (what, method, params) in [
        ("get", "meta/mobs/get", json!({"mob_id": second})),
        (
            "stop",
            "meta/mobs/lifecycle",
            json!({"mob_id": second, "action": "stop"}),
        ),
    ] {
        let answer = asks(DENIED_ALONE, method, params).await;
        if answer != refused_like_an_id_no_mob_has {
            wrong.push(format!(
                "{what}: a person whom the deny alone lists the mob for is not refused like an id \
                 no mob has: {answer}"
            ));
        }
    }
    for (what, method, params) in [
        ("list", "meta/mobs/list", json!({})),
        ("get", "meta/mobs/get", json!({"mob_id": second})),
    ] {
        let answer = asks(DENIED_THE_GRANT, method, params).await;
        if answer["error"]["code"] != json!(ACCESS_DENIED_RPC_CODE)
            || answer["error"]["data"]["action"] != json!(crate::access::ACTION_RUNTIME_ADMIN)
        {
            wrong.push(format!(
                "{what}: a person a deny that lists no mob takes the grant from is not refused by \
                 the access check: {answer}"
            ));
        }
    }
    let after = kept_of_the_hosts_mobs(&set, &second_id).await;
    if after != before {
        wrong.push(format!(
            "the refusals: the host does not keep what it kept: before {before}, after {after}"
        ));
    }

    if let Ok(handle) = set.handle_for(&second_id).await {
        let _ = handle.shutdown().await;
    }
    drop(set);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(30), launch.shutdown()).await;
    assert!(
        wrong.is_empty(),
        "the methods over the mobs of the host under a deny rule that lists the mob:\n{}",
        wrong.join("\n")
    );
}

/// A change over a mob of the host that is refused for its caller writes
/// the row every refused console change writes to the console audit log:
/// who asked, the method, the id the request named, refused, with the code
/// and the kind of the door's refusal of a mob id. The row of a mob that
/// exists and the row of an id no mob has are the same apart from the id.
/// Each is shown to a reader who may address the mob it names: an
/// administrator reads both, and the refused caller, who holds the audit
/// read, reads neither. The host keeps its mobs as they were.
#[tokio::test]
async fn a_refused_change_over_a_mob_of_the_host_writes_the_audit_row_of_a_refused_change() {
    const DESTROY: &str = "meta/mobs/destroy";

    let temp = tempfile::tempdir().expect("tempdir");
    let launch_mob = format!("door-rows-audit-{}", uuid::Uuid::new_v4().simple());
    let second = format!("{launch_mob}-second");
    let absent = an_id_no_mob_has(&second);
    let launch = crate::rpc::mob_hosting_tests::host(&temp, &launch_mob).await;
    crate::rpc::mob_hosting_tests::result(
        &launch,
        "meta/mobs/create",
        json!({"mob_config": crate::rpc::mob_hosting_tests::config(&second)}),
    )
    .await;
    let state = ConsoleJsonState {
        console_audit: Some(audit_log_holding(&[]).await),
        ..door_rows_state(&launch, "a-mob-no-rule-is-asked-of")
    };
    let app = console_json_router_with_state(state).layer(axum::Extension(launch.mob_resolver()));
    let set = launch
        .mob_runtime()
        .agent_mob_mcp_state()
        .expect("the launch's mob set");
    let second_id = meerkat_mob::MobId::from(second.as_str());

    let mut wrong = Vec::new();
    let before = kept_of_the_hosts_mobs(&set, &second_id).await;
    for mob_id in [&second, &absent] {
        let answer = ask_as(
            &app,
            rpc_request(DESTROY, &json!({"mob_id": mob_id})),
            DOOR_ROWS_OPERATOR,
        )
        .await;
        if !is_the_mob_ids_refusal("console RPC", &answer, mob_id) {
            wrong.push(format!(
                "destroy of '{mob_id}': not refused as a mob id the door does not serve to its \
                 caller: {} {:.300}",
                answer.0, answer.2
            ));
        }
    }
    let after = kept_of_the_hosts_mobs(&set, &second_id).await;
    if after != before {
        wrong.push(format!(
            "the host does not keep what it kept: before {before}, after {after}"
        ));
    }

    // The destroy rows of the log as `email` reads them with every mob
    // selected (the rows name other mobs than the launch mob), oldest
    // first, or the whole answer when the read is refused.
    let destroy_rows_as = |email: &'static str| {
        let app = &app;
        async move {
            let answer = ask_as(
                app,
                rpc_request(crate::rpc::CONSOLE_AUDIT_METHOD, &audit_read_of_every_mob()),
                email,
            )
            .await;
            let body: Value = serde_json::from_str(&answer.2).unwrap_or(Value::Null);
            body["result"]["records"]
                .as_array()
                .map(|rows| {
                    rows.iter()
                        .filter(|row| row["method"] == json!(DESTROY))
                        .cloned()
                        .collect::<Vec<Value>>()
                })
                .ok_or(answer.2)
        }
    };
    match destroy_rows_as(DOOR_ROWS_ADMIN).await {
        Ok(rows) => {
            let named: Vec<&Value> = rows.iter().map(|row| &row["target"]["mob_id"]).collect();
            if named != [&json!(second), &json!(absent)] {
                wrong.push(format!(
                    "an administrator does not read one row for each refused destroy, in order: \
                     {rows:?}"
                ));
            }
            // A row without its number, its time and the id its request named.
            let apart_from_the_id = |row: &Value| {
                let mut row = row.clone();
                if let Some(fields) = row.as_object_mut() {
                    fields.remove("seq");
                    fields.remove("at_ms");
                }
                if let Some(target) = row["target"].as_object_mut() {
                    target.remove("mob_id");
                }
                row
            };
            for row in &rows {
                if row["outcome"] != json!("refused")
                    || row["code"] != json!(-32602)
                    || row["kind"] != json!("foreign_mob_target")
                    || row["actor"]["principal"] != json!(DOOR_ROWS_OPERATOR)
                    || row.get("receipt").is_some()
                {
                    wrong.push(format!(
                        "a row of a refused destroy is not who asked, refused, with the code and \
                         the kind of the mob id's refusal and no receipt: {row}"
                    ));
                }
            }
            if let [of_the_mob, of_the_absent] = rows.as_slice()
                && apart_from_the_id(of_the_mob) != apart_from_the_id(of_the_absent)
            {
                wrong.push(format!(
                    "the row of a mob that exists and the row of an id no mob has differ in more \
                     than the id: {of_the_mob} and {of_the_absent}"
                ));
            }
        }
        Err(answer) => wrong.push(format!(
            "an administrator's audit read is refused: {answer:.300}"
        )),
    }
    match destroy_rows_as(DOOR_ROWS_OPERATOR).await {
        Ok(rows) if rows.is_empty() => {}
        Ok(rows) => wrong.push(format!(
            "the refused caller reads the rows of mobs it may not address: {rows:?}"
        )),
        Err(answer) => wrong.push(format!(
            "the audit read of a person who holds it is refused: {answer:.300}"
        )),
    }

    drop(app);
    if let Ok(handle) = set.handle_for(&second_id).await {
        let _ = handle.shutdown().await;
    }
    drop(set);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(30), launch.shutdown()).await;
    assert!(
        wrong.is_empty(),
        "the audit rows of refused changes over a mob of the host:\n{}",
        wrong.join("\n")
    );
}

/// What `meta/storage/doctor` over the state directory `state_dir` answers
/// `email` on `app`: its result, or the whole answer when it is no result.
async fn doctor_answer_to(
    app: &Router,
    email: &str,
    state_dir: &std::path::Path,
) -> Result<Value, String> {
    let answer = ask_as(
        app,
        rpc_request(
            crate::rpc::storage_methods::STORAGE_DOCTOR_METHOD,
            &json!({"state_dir": state_dir.display().to_string()}),
        ),
        email,
    )
    .await;
    let body: Value = serde_json::from_str(&answer.2).unwrap_or(Value::Null);
    match body.get("result") {
        Some(result) if result.get("diagnosis").is_some() => Ok(result.clone()),
        _ => Err(answer.2),
    }
}

/// Every path a doctor's answer holds: the path of each finding, and the
/// root and each database of each inventory entry.
fn doctor_paths(answer: &Value) -> Vec<String> {
    let diagnosis = &answer["diagnosis"];
    let texts = |rows: &Value, member: &str| -> Vec<String> {
        rows.as_array()
            .into_iter()
            .flatten()
            .filter_map(|row| row[member].as_str().map(str::to_string))
            .collect()
    };
    let mut paths = texts(&diagnosis["findings"], "path");
    paths.extend(texts(&diagnosis["inventory"], "root"));
    for entry in diagnosis["inventory"].as_array().into_iter().flatten() {
        paths.extend(texts(&entry["databases"], "path"));
    }
    paths
}

/// The ids of `mobs` a doctor's answer names: in a path, or anywhere else
/// in its text.
fn mobs_a_doctor_names(answer: &Value, mobs: &[&str]) -> Vec<String> {
    let text = answer.to_string();
    mobs.iter()
        .filter(|mob| text.contains(**mob))
        .map(|mob| (*mob).to_string())
        .collect()
}

/// The number a census total of a doctor's answer states right before
/// `what`: the total is the finding of `code` whose path is the state
/// directory itself. `None` when the answer holds no such number.
fn census_number(
    answer: &Value,
    state_dir: &std::path::Path,
    code: &str,
    what: &str,
) -> Option<u64> {
    let state_dir = state_dir.display().to_string();
    answer["diagnosis"]["findings"]
        .as_array()?
        .iter()
        .filter(|finding| finding["code"] == json!(code) && finding["path"] == json!(state_dir))
        .filter_map(|finding| finding["message"].as_str())
        .find_map(|message| {
            message
                .split(what)
                .next()
                .filter(|before| before.len() < message.len())?
                .rsplit(' ')
                .next()?
                .parse()
                .ok()
        })
}

/// The stores of the census of the mobs a host created that a doctor's
/// answer has a finding of their own for: the findings of that census whose
/// path is not the state directory.
fn host_created_stores_found(answer: &Value, state_dir: &std::path::Path) -> usize {
    let state_dir = state_dir.display().to_string();
    answer["diagnosis"]["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|finding| {
            finding["code"] == json!(crate::storage_doctor::FINDING_HOST_CREATED_MOBS)
                && finding["path"] != json!(state_dir)
        })
        .count()
}

/// The three counts of mob stores a doctor's answer states: the stores of
/// the census of the mobs the host created, the stores of the identity row
/// census and the journals of the mob event census.
fn mob_stores_counted(
    answer: &Value,
    state_dir: &std::path::Path,
) -> (Option<u64>, Option<u64>, Option<u64>) {
    (
        census_number(
            answer,
            state_dir,
            crate::storage_doctor::FINDING_HOST_CREATED_MOBS,
            " mob store(s)",
        ),
        census_number(
            answer,
            state_dir,
            crate::storage_doctor::FINDING_IDENTITY_STORE_ROWS,
            " mob store(s)",
        ),
        census_number(
            answer,
            state_dir,
            crate::storage_doctor::FINDING_OPERATOR_INSTRUCTION_MOB_EVENTS,
            " mob journal(s)",
        ),
    )
}

/// The storage doctor on the console answers a caller for the launch mob's
/// stores and for the stores of the mobs the caller may address. A host
/// keeps its mobs in files and created two mobs beside its launch mob; an
/// access rule lists one of them for one person. A person who holds the
/// admin grant and whom no rule lists a mob for is answered no path of
/// either mob's store, and the census of the mobs the host created counts
/// none in no store. The person the rule lists one mob for is answered that
/// mob's store alone, counted once. An administrator is answered both. The
/// two older counts of mob stores are lower by the stores left out: each of
/// the three counts of mob stores is of the stores the caller is shown.
#[tokio::test]
async fn the_storage_doctor_on_the_console_answers_the_mob_stores_its_caller_may_address() {
    use crate::rpc::mob_hosting_tests::{config, end, result, stored_host};

    let temp = tempfile::tempdir().expect("tempdir");
    let launch_mob = format!("door-rows-doctor-{}", uuid::Uuid::new_v4().simple());
    let listed_by_a_rule = format!("{launch_mob}-second");
    let listed_by_no_rule = format!("{launch_mob}-third");
    let launch = stored_host(&temp, &launch_mob).await;
    for mob_id in [&listed_by_a_rule, &listed_by_no_rule] {
        result(
            &launch,
            "meta/mobs/create",
            json!({"mob_config": config(mob_id)}),
        )
        .await;
    }
    let app = console_json_router_with_state(door_rows_state(&launch, &listed_by_a_rule))
        .layer(axum::Extension(launch.mob_resolver()));
    let beside = [listed_by_a_rule.as_str(), listed_by_no_rule.as_str()];

    let mut wrong = Vec::new();
    let to_an_administrator = doctor_answer_to(&app, DOOR_ROWS_ADMIN, temp.path()).await;
    let all = match &to_an_administrator {
        Ok(answer) => mob_stores_counted(answer, temp.path()),
        Err(answer) => {
            wrong.push(format!(
                "an administrator's doctor is refused: {answer:.300}"
            ));
            (None, None, None)
        }
    };
    let fewer_by = |count: Option<u64>, left_out: u64| count.and_then(|n| n.checked_sub(left_out));
    for (who, email, shown, left_out) in [
        (
            "a person no rule lists a mob for",
            DOOR_ROWS_OPERATOR,
            Vec::<&str>::new(),
            2_u64,
        ),
        (
            "a person a rule lists one mob for",
            DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB,
            vec![listed_by_a_rule.as_str()],
            1,
        ),
        ("an administrator", DOOR_ROWS_ADMIN, beside.to_vec(), 0),
    ] {
        let answer = match doctor_answer_to(&app, email, temp.path()).await {
            Ok(answer) => answer,
            Err(answer) => {
                wrong.push(format!("{who}: the doctor is refused: {answer:.300}"));
                continue;
            }
        };
        let named = mobs_a_doctor_names(&answer, &beside);
        if named != shown {
            wrong.push(format!(
                "{who}: the answer names the mobs {named:?}, not {shown:?}; its paths: {:?}",
                doctor_paths(&answer)
                    .into_iter()
                    .filter(|path| beside.iter().any(|mob| path.contains(mob)))
                    .collect::<Vec<_>>()
            ));
        }
        let host_created = (
            census_number(
                &answer,
                temp.path(),
                crate::storage_doctor::FINDING_HOST_CREATED_MOBS,
                " mob(s) the host created",
            ),
            host_created_stores_found(&answer, temp.path()),
        );
        if host_created != (u64::try_from(shown.len()).ok(), shown.len()) {
            wrong.push(format!(
                "{who}: the census of the mobs the host created counts {host_created:?}, not \
                 the {} shown",
                shown.len()
            ));
        }
        let counted = mob_stores_counted(&answer, temp.path());
        let expected = (
            fewer_by(all.0, left_out),
            fewer_by(all.1, left_out),
            fewer_by(all.2, left_out),
        );
        if counted != expected || counted.0.is_none() {
            wrong.push(format!(
                "{who}: the answer counts the mob stores {counted:?}, not {expected:?}: an \
                 administrator's counts {all:?} less the {left_out} left out"
            ));
        }
    }

    drop(app);
    end(launch).await;
    assert!(
        wrong.is_empty(),
        "the storage doctor on the console, by its caller:\n{}",
        wrong.join("\n")
    );
}

/// The storage doctor on the console shows a caller who may not address
/// every mob a file that lies beside a hosted mob's store (a lock, a
/// backup, a quarantined copy) only for the mob that stands under the
/// store's id. A host keeps its mobs in files and created two mobs beside
/// its launch mob; an access rule lists one of them for one person. Beside
/// each store lie the lock file the store keeps, and a backup and a
/// quarantined copy named as the offline tools name them, with the time
/// they are written at.
///
/// The host destroys both mobs: the stores are gone, the files beside them
/// are not, and no mob stands under either id. The person the rule lists
/// one mob for, and a person no rule lists a mob for, are answered nothing
/// that names either mob. An administrator is answered the files of both.
///
/// The host creates a mob under the listed id, and a backup and a
/// quarantined copy of its store are written. The person the rule lists
/// that id for is answered the standing mob's store, its lock file and the
/// backup and the quarantined copy written since, and neither the backup
/// nor the quarantined copy that is older than the standing mob. The
/// person no rule lists a mob for is answered nothing of either id still,
/// and an administrator every file.
#[tokio::test]
async fn the_storage_doctor_on_the_console_shows_a_file_beside_a_mob_store_only_for_the_mob_that_stands_under_its_id()
 {
    use crate::rpc::mob_hosting_tests::{config, end, result, stored_host};

    let temp = tempfile::tempdir().expect("tempdir");
    let launch_mob = format!("door-rows-doctor-files-{}", uuid::Uuid::new_v4().simple());
    let listed_by_a_rule = format!("{launch_mob}-second");
    let listed_by_no_rule = format!("{launch_mob}-third");
    let launch = stored_host(&temp, &launch_mob).await;
    for mob_id in [&listed_by_a_rule, &listed_by_no_rule] {
        result(
            &launch,
            "meta/mobs/create",
            json!({"mob_config": config(mob_id)}),
        )
        .await;
    }
    let app = console_json_router_with_state(door_rows_state(&launch, &listed_by_a_rule))
        .layer(axum::Extension(launch.mob_resolver()));
    let beside = [listed_by_a_rule.as_str(), listed_by_no_rule.as_str()];

    // The directory of the mob stores, and the names in it: a mob's store,
    // its lock file, and a backup and a quarantined copy of the store named
    // with the time `at` (seconds of the clock), as the offline tools name
    // them.
    let mobs_dir = crate::child_mob_storage::child_mob_root(temp.path());
    let store_of = |mob_id: &str| format!("mob--{mob_id}.db");
    let lock_of = |mob_id: &str| format!("mob--{mob_id}.db.mfence");
    let backup_of = |mob_id: &str, at: u64| format!("mob--{mob_id}.db.pre-0.9.17-{at}");
    let quarantine_of = |mob_id: &str, at: u64| format!("mob--{mob_id}.db.corrupt-{at}");
    let on_disk = |name: &str| mobs_dir.join(name).is_file();
    let seconds_now = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_secs())
    };
    // The names of the files of the two mobs an answer holds a path of.
    let names_shown = |answer: &Value| -> std::collections::BTreeSet<String> {
        doctor_paths(answer)
            .into_iter()
            .filter(|path| beside.iter().any(|mob| path.contains(mob)))
            .filter_map(|path| {
                std::path::Path::new(&path)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .collect()
    };
    let mut wrong = Vec::new();

    // While both mobs stand: a backup and a quarantined copy are written
    // beside each store.
    let earlier = seconds_now();
    let mut left_by_the_first_mobs = Vec::new();
    for mob_id in beside {
        for name in [store_of(mob_id), lock_of(mob_id)] {
            assert!(
                on_disk(&name),
                "the scene: the store of a mob that stands keeps {name}"
            );
        }
        for name in [backup_of(mob_id, earlier), quarantine_of(mob_id, earlier)] {
            std::fs::write(mobs_dir.join(&name), b"a copy").expect("a file beside the store");
            left_by_the_first_mobs.push(name);
        }
        left_by_the_first_mobs.push(lock_of(mob_id));
    }

    // The host destroys both mobs: no mob stands under either id, and the
    // files beside the stores are still there.
    for mob_id in beside {
        result(&launch, "meta/mobs/destroy", json!({"mob_id": mob_id})).await;
        assert!(
            !on_disk(&store_of(mob_id)),
            "the scene: a destroy removes the mob's store"
        );
    }
    for name in &left_by_the_first_mobs {
        assert!(on_disk(name), "the scene: a destroy leaves {name}");
    }
    for (who, email, shown) in [
        (
            "the person a rule lists one id for",
            DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB,
            false,
        ),
        (
            "a person no rule lists a mob for",
            DOOR_ROWS_OPERATOR,
            false,
        ),
        ("an administrator", DOOR_ROWS_ADMIN, true),
    ] {
        match doctor_answer_to(&app, email, temp.path()).await {
            Ok(answer) => {
                let names = names_shown(&answer);
                let missing: Vec<&String> = left_by_the_first_mobs
                    .iter()
                    .filter(|name| !names.contains(*name))
                    .collect();
                if shown && !missing.is_empty() {
                    wrong.push(format!(
                        "while no mob stands under either id, {who} is not answered {missing:?}"
                    ));
                }
                let named = mobs_a_doctor_names(&answer, &beside);
                let nothing_of_either_id = names.is_empty() && named.is_empty();
                if !(shown || nothing_of_either_id) {
                    wrong.push(format!(
                        "while no mob stands under either id, {who} is answered {names:?} and \
                         the answer names {named:?}"
                    ));
                }
            }
            Err(answer) => wrong.push(format!(
                "while no mob stands under either id, the doctor refuses {who}: {answer:.300}"
            )),
        }
    }

    // The host creates a mob under the listed id. The clock passes into a
    // later second, so the name of a file written now holds a time after
    // the mob's creation.
    result(
        &launch,
        "meta/mobs/create",
        json!({"mob_config": config(&listed_by_a_rule)}),
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let later = seconds_now();
    let since = [
        backup_of(&listed_by_a_rule, later),
        quarantine_of(&listed_by_a_rule, later),
    ];
    for name in &since {
        std::fs::write(mobs_dir.join(name), b"a copy").expect("a file beside the store");
    }
    let of_the_standing_mob = [
        store_of(&listed_by_a_rule),
        lock_of(&listed_by_a_rule),
        since[0].clone(),
        since[1].clone(),
    ];
    for name in &of_the_standing_mob {
        assert!(
            on_disk(name),
            "the scene: the mob that stands under the id keeps {name}"
        );
    }
    let older_than_the_standing_mob = [
        backup_of(&listed_by_a_rule, earlier),
        quarantine_of(&listed_by_a_rule, earlier),
    ];
    assert!(
        !since
            .iter()
            .any(|name| older_than_the_standing_mob.contains(name)),
        "the scene: the files written since have names of their own"
    );

    match doctor_answer_to(&app, DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB, temp.path()).await {
        Ok(answer) => {
            let names = names_shown(&answer);
            let missing: Vec<&String> = of_the_standing_mob
                .iter()
                .filter(|name| !names.contains(*name))
                .collect();
            if !missing.is_empty() {
                wrong.push(format!(
                    "the person a rule lists the id for is not answered {missing:?} of the mob \
                     that stands under it; answered: {names:?}"
                ));
            }
            let older: Vec<&String> = older_than_the_standing_mob
                .iter()
                .filter(|name| names.contains(*name))
                .collect();
            if !older.is_empty() {
                wrong.push(format!(
                    "the person a rule lists the id for is answered {older:?}, older than the \
                     mob that stands under it"
                ));
            }
            let named = mobs_a_doctor_names(&answer, &beside);
            if named != [listed_by_a_rule.clone()] {
                wrong.push(format!(
                    "the answer to the person a rule lists one id for names {named:?}; of the \
                     two ids it holds {names:?}"
                ));
            }
        }
        Err(answer) => wrong.push(format!(
            "the doctor refuses the person a rule lists the id for: {answer:.300}"
        )),
    }
    match doctor_answer_to(&app, DOOR_ROWS_OPERATOR, temp.path()).await {
        Ok(answer) => {
            let (names, named) = (names_shown(&answer), mobs_a_doctor_names(&answer, &beside));
            if !(names.is_empty() && named.is_empty()) {
                wrong.push(format!(
                    "a person no rule lists a mob for is answered {names:?} and the answer \
                     names {named:?}"
                ));
            }
        }
        Err(answer) => wrong.push(format!(
            "the doctor refuses a person no rule lists a mob for: {answer:.300}"
        )),
    }
    match doctor_answer_to(&app, DOOR_ROWS_ADMIN, temp.path()).await {
        Ok(answer) => {
            let names = names_shown(&answer);
            let missing: Vec<&String> = left_by_the_first_mobs
                .iter()
                .chain(&of_the_standing_mob)
                .filter(|name| !names.contains(*name))
                .collect();
            if !missing.is_empty() {
                wrong.push(format!("an administrator is not answered {missing:?}"));
            }
        }
        Err(answer) => wrong.push(format!(
            "the doctor refuses an administrator: {answer:.300}"
        )),
    }

    drop(app);
    end(launch).await;
    assert!(
        wrong.is_empty(),
        "the storage doctor on the console over the files beside a mob store:\n{}",
        wrong.join("\n")
    );
}

/// The storage doctor on the console shows a caller who may not address
/// every mob no store under whose id no mob stands. A host keeps its mobs
/// in files, created a mob beside its launch mob and destroyed it; an
/// access rule lists that mob's id for one person. A store file then lies
/// under `mobs/` at the destroyed mob's own name, as a destroy that could
/// not remove its store leaves one, and the host's mob set lists no mob
/// under the id. The person the rule lists the id for, and a person no rule
/// lists a mob for, are answered no path of that file and nothing that
/// names the id, and the three counts of mob stores they are answered are
/// those they were answered before the file lay there. An administrator is
/// answered the file.
#[tokio::test]
async fn the_storage_doctor_on_the_console_shows_no_store_of_an_id_under_which_no_mob_stands() {
    use crate::rpc::mob_hosting_tests::{config, end, result, stored_host};

    let temp = tempfile::tempdir().expect("tempdir");
    let launch_mob = format!("door-rows-doctor-gone-{}", uuid::Uuid::new_v4().simple());
    let gone = format!("{launch_mob}-second");
    let launch = stored_host(&temp, &launch_mob).await;
    result(
        &launch,
        "meta/mobs/create",
        json!({"mob_config": config(&gone)}),
    )
    .await;
    let app = console_json_router_with_state(door_rows_state(&launch, &gone))
        .layer(axum::Extension(launch.mob_resolver()));
    let store_file = format!("mob--{gone}.db");
    let store = crate::child_mob_storage::child_mob_root(temp.path()).join(&store_file);
    assert!(
        store.is_file(),
        "the scene: the mob that stands keeps its store at {store:?}"
    );
    result(&launch, "meta/mobs/destroy", json!({"mob_id": gone})).await;
    assert!(
        !store.exists(),
        "the scene: a destroy removes the mob's store"
    );

    let people = [
        (
            "the person a rule lists the id for",
            DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB,
        ),
        ("a person no rule lists a mob for", DOOR_ROWS_OPERATOR),
    ];
    let mut wrong = Vec::new();
    // The three counts each of the two is answered while nothing lies
    // under the id.
    let mut counted_before = Vec::new();
    for (who, email) in people {
        match doctor_answer_to(&app, email, temp.path()).await {
            Ok(answer) => counted_before.push(mob_stores_counted(&answer, temp.path())),
            Err(answer) => {
                wrong.push(format!("{who}: the doctor is refused: {answer:.300}"));
                counted_before.push((None, None, None));
            }
        }
    }

    // A store file at the destroyed mob's name: a database, as a store is.
    let planted = rusqlite::Connection::open(&store).expect("a database at the store's name");
    planted
        .execute_batch("CREATE TABLE left_by_a_destroy (x INTEGER);")
        .expect("the database is written");
    drop(planted);
    assert!(
        store.is_file(),
        "the scene: a store file lies at the destroyed mob's name"
    );
    // The scene, read: the host's mob set lists no mob under the id.
    let listed = launch.mob_resolver().hosting_of_every_mob().await;
    if !matches!(&listed, Ok(mobs) if mobs.iter().all(|(mob, _)| mob.as_str() != gone)) {
        wrong.push(format!(
            "the fixture of this row is an id the host's mob set lists no mob under: {listed:?}"
        ));
    }

    for ((who, email), before) in people.into_iter().zip(counted_before) {
        match doctor_answer_to(&app, email, temp.path()).await {
            Ok(answer) => {
                let of_the_id: Vec<String> = doctor_paths(&answer)
                    .into_iter()
                    .filter(|path| path.contains(&gone))
                    .collect();
                if !of_the_id.is_empty() || answer.to_string().contains(&gone) {
                    wrong.push(format!(
                        "{who} is answered what names the id; its paths: {of_the_id:?}"
                    ));
                }
                let counted = mob_stores_counted(&answer, temp.path());
                if counted != before || counted.0.is_none() {
                    wrong.push(format!(
                        "{who}: the three counts of mob stores are {counted:?}; they were \
                         {before:?} before the file lay there"
                    ));
                }
            }
            Err(answer) => wrong.push(format!("{who}: the doctor is refused: {answer:.300}")),
        }
    }
    match doctor_answer_to(&app, DOOR_ROWS_ADMIN, temp.path()).await {
        Ok(answer) => {
            if !doctor_paths(&answer)
                .iter()
                .any(|path| path.ends_with(&store_file))
            {
                wrong.push("an administrator is not answered the store file".to_string());
            }
        }
        Err(answer) => wrong.push(format!(
            "the doctor refuses an administrator: {answer:.300}"
        )),
    }

    drop(app);
    end(launch).await;
    assert!(
        wrong.is_empty(),
        "the storage doctor on the console over a store under whose id no mob stands:\n{}",
        wrong.join("\n")
    );
}

/// The storage doctor on the console shows a caller who may not address
/// every mob no agent memory under whose id no mob stands. A host keeps a
/// mob's memory in a directory of the mob's own under its memory root. A
/// second such directory lies there under the name of an id the host's mob
/// set lists no mob under, with a realm database and a backup beside it,
/// as a release of a mob's memory that could not take the directory out of
/// its name leaves one; an access
/// rule lists that id for one person. That person, and a person no rule
/// lists a mob for, are answered no path in that directory and nothing
/// that names the id. An administrator is answered the database and the
/// backup.
#[tokio::test]
async fn the_storage_doctor_on_the_console_shows_no_memory_of_an_id_under_which_no_mob_stands() {
    use crate::unified_runtime::mob_memory_release_tests::HostOfARememberingMob;

    let (fixture, launch) = HostOfARememberingMob::launched().await;
    let state_dir = fixture.state_dir();
    let gone = format!("door-rows-memory-gone-{}", uuid::Uuid::new_v4().simple());
    let segment = crate::memory::sqlite_store::hosted_mob_segment(&gone)
        .expect("the id has a directory name");
    let hosted = fixture
        .memory_dir_of_the_mob()
        .parent()
        .expect("the mob's memory directory lies in the directory of the hosted mobs")
        .to_path_buf();
    let dir = hosted.join(&segment);
    crate::memory::sqlite_store::SqliteAgentMemoryStore::open(&dir)
        .expect("a memory store in the directory")
        .open_realm_ledgered("alpha")
        .expect("a realm database");
    let database = dir.join("alpha.sqlite3");
    let backup = dir.join("alpha.sqlite3.pre-0.9.17-1");
    std::fs::write(&backup, b"a copy").expect("a backup beside the database");
    assert!(
        database.is_file() && backup.is_file(),
        "the scene: a realm database and a backup lie in {dir:?}"
    );
    // The directory as a path names it.
    let in_the_directory = format!(
        "{}/{segment}/",
        crate::memory::sqlite_store::HOSTED_MOBS_DIR
    );
    let app = console_json_router_with_state(door_rows_state(&launch, &gone))
        .layer(axum::Extension(launch.mob_resolver()));

    let mut wrong = Vec::new();
    // The scene, read: the host's mob set lists no mob under the id.
    let listed = launch.mob_resolver().hosting_of_every_mob().await;
    if !matches!(&listed, Ok(mobs) if mobs.iter().all(|(mob, _)| mob.as_str() != gone)) {
        wrong.push(format!(
            "the fixture of this row is an id the host's mob set lists no mob under: {listed:?}"
        ));
    }
    for (who, email, shown) in [
        ("an administrator", DOOR_ROWS_ADMIN, true),
        (
            "the person a rule lists the id for",
            DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB,
            false,
        ),
        (
            "a person no rule lists a mob for",
            DOOR_ROWS_OPERATOR,
            false,
        ),
    ] {
        match doctor_answer_to(&app, email, &state_dir).await {
            Ok(answer) => {
                let of_the_directory: Vec<String> = doctor_paths(&answer)
                    .into_iter()
                    .filter(|path| path.contains(&in_the_directory))
                    .collect();
                let both = ["alpha.sqlite3", "alpha.sqlite3.pre-0.9.17-1"]
                    .iter()
                    .all(|name| of_the_directory.iter().any(|path| path.ends_with(name)));
                if shown && !both {
                    wrong.push(format!(
                        "{who} is not answered the database and the backup: {of_the_directory:?}"
                    ));
                }
                let names_the_id =
                    !of_the_directory.is_empty() || answer.to_string().contains(&gone);
                if names_the_id && !shown {
                    wrong.push(format!(
                        "{who} is answered what names the id; its paths: {of_the_directory:?}"
                    ));
                }
            }
            Err(answer) => wrong.push(format!("{who}: the doctor is refused: {answer:.300}")),
        }
    }

    drop(app);
    fixture.ends(launch).await;
    assert!(
        wrong.is_empty(),
        "the storage doctor on the console over memory under whose id no mob stands:\n{}",
        wrong.join("\n")
    );
}

/// On a host whose launch declared its mob to no mob set the storage doctor
/// on the console answers every caller who holds its grant for the whole
/// directory: such a host has no mob of its own beside its launch mob. A
/// directory holds, under `mobs/`, a mob store with its lock file and a
/// backup named with a time long past. A person who holds the grant and is
/// no administrator is answered the store, the lock file and the backup, as
/// an administrator is.
#[tokio::test]
async fn the_storage_doctor_on_the_console_answers_the_whole_directory_on_a_host_with_no_mob_set_of_its_own()
 {
    use crate::rpc::mob_hosting_tests::config;

    let launch_mob = format!("door-rows-doctor-no-set-{}", uuid::Uuid::new_v4().simple());
    let launch = crate::UnifiedRuntime::builder()
        .definition(
            meerkat_mob::MobDefinition::from_toml(&config(&launch_mob))
                .expect("a definition with one profile"),
        )
        .default_llm_client(std::sync::Arc::new(meerkat_client::TestClient::default()))
        .build()
        .await
        .expect("runtime builds");
    // The scene, read: the launch's mob set holds no launch mob, and the
    // host's records list no mob.
    assert!(
        launch
            .mob_runtime()
            .agent_mob_mcp_state()
            .is_some_and(|set| set.default_mob_id().is_none()),
        "the fixture of this row is a launch whose mob set holds no launch mob"
    );
    let listed = launch.mob_resolver().hosting_of_every_mob().await;
    assert!(
        matches!(&listed, Ok(mobs) if mobs.is_empty()),
        "the fixture of this row is a host whose records list no mob: {listed:?}"
    );

    let temp = tempfile::tempdir().expect("tempdir");
    let mobs_dir = crate::child_mob_storage::child_mob_root(temp.path());
    std::fs::create_dir_all(&mobs_dir).expect("the directory of the mob stores");
    let store = "mob--a-child.db";
    let planted =
        rusqlite::Connection::open(mobs_dir.join(store)).expect("a database at a store's name");
    planted
        .execute_batch("CREATE TABLE of_a_child (x INTEGER);")
        .expect("the database is written");
    drop(planted);
    let beside = [format!("{store}.mfence"), format!("{store}.pre-0.9.17-1")];
    for name in &beside {
        std::fs::write(mobs_dir.join(name), b"").expect("a file beside the store");
    }
    let app = console_json_router_with_state(door_rows_state(&launch, "a-mob-no-rule-is-asked-of"))
        .layer(axum::Extension(launch.mob_resolver()));

    let mut wrong = Vec::new();
    for (who, email) in [
        ("an administrator", DOOR_ROWS_ADMIN),
        (
            "a person who holds the grant and is no administrator",
            DOOR_ROWS_OPERATOR,
        ),
    ] {
        match doctor_answer_to(&app, email, temp.path()).await {
            Ok(answer) => {
                let paths = doctor_paths(&answer);
                let missing: Vec<&str> = std::iter::once(store)
                    .chain(beside.iter().map(String::as_str))
                    .filter(|name| !paths.iter().any(|path| path.ends_with(name)))
                    .collect();
                if !missing.is_empty() {
                    wrong.push(format!("{who} is not answered {missing:?} under mobs/"));
                }
            }
            Err(answer) => wrong.push(format!("{who}: the doctor is refused: {answer:.300}")),
        }
    }

    drop(app);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(30), launch.shutdown()).await;
    assert!(
        wrong.is_empty(),
        "the storage doctor on the console of a host with no mob set of its own:\n{}",
        wrong.join("\n")
    );
}

/// The storage doctor on the console shows every caller who holds its grant
/// the files beside the store of a mob a member of the launch mob made, by
/// the mob that stands. A host keeps its mobs in files and a member of its
/// launch mob made a mob. Beside that mob's store lie its lock file, a
/// backup named with a time long past and a backup named with the time it
/// is written at, after the mob was made. A person who holds the grant and
/// whom no rule lists a mob for is answered the store, the lock file and
/// the backup written since, and not the backup that is older than the
/// mob; an administrator is answered all four.
#[tokio::test]
async fn the_storage_doctor_on_the_console_shows_the_files_beside_the_store_of_a_mob_a_member_of_the_launch_mob_made()
 {
    use crate::rpc::mob_hosting_tests::{member_created_mob, stored_host};

    let temp = tempfile::tempdir().expect("tempdir");
    let launch_mob = format!(
        "door-rows-doctor-child-files-{}",
        uuid::Uuid::new_v4().simple()
    );
    let launch = stored_host(&temp, &launch_mob).await;
    let of_a_member = member_created_mob(&launch, &format!("{launch_mob}-second"))
        .await
        .to_string();
    let mobs_dir = crate::child_mob_storage::child_mob_root(temp.path());
    let store = format!("mob--{of_a_member}.db");
    let lock = format!("{store}.mfence");
    for name in [&store, &lock] {
        assert!(
            mobs_dir.join(name).is_file(),
            "the scene: the store of the mob a member made keeps {name}"
        );
    }
    // The clock passes into a later second than the one the mob was made
    // in, so the name of a backup written now holds a time after it.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let older = format!("{store}.pre-0.9.17-1");
    let since = format!("{store}.pre-0.9.17-{now}");
    for name in [&older, &since] {
        std::fs::write(mobs_dir.join(name), b"a copy").expect("a backup beside the store");
    }
    let app = console_json_router_with_state(door_rows_state(&launch, "a-mob-no-rule-is-asked-of"))
        .layer(axum::Extension(launch.mob_resolver()));

    let mut wrong = Vec::new();
    for (who, email, shown_the_older) in [
        ("an administrator", DOOR_ROWS_ADMIN, true),
        (
            "a person no rule lists a mob for",
            DOOR_ROWS_OPERATOR,
            false,
        ),
    ] {
        match doctor_answer_to(&app, email, temp.path()).await {
            Ok(answer) => {
                let paths = doctor_paths(&answer);
                let holds = |name: &String| paths.iter().any(|path| path.ends_with(name.as_str()));
                let missing: Vec<&String> = [&store, &lock, &since]
                    .into_iter()
                    .filter(|name| !holds(name))
                    .collect();
                if !missing.is_empty() {
                    wrong.push(format!("{who} is not answered {missing:?}"));
                }
                if holds(&older) != shown_the_older {
                    wrong.push(format!(
                        "{who}: the backup older than the mob is {}",
                        if shown_the_older {
                            "not answered"
                        } else {
                            "answered"
                        }
                    ));
                }
            }
            Err(answer) => wrong.push(format!("{who}: the doctor is refused: {answer:.300}")),
        }
    }

    drop(app);
    let set = launch
        .mob_runtime()
        .agent_mob_mcp_state()
        .expect("the launch's mob set");
    if let Ok(handle) = set
        .handle_for(&meerkat_mob::MobId::from(of_a_member.as_str()))
        .await
    {
        let _ = handle.shutdown().await;
    }
    drop(set);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(30), launch.shutdown()).await;
    assert!(
        wrong.is_empty(),
        "the storage doctor on the console over the files beside a store of a mob a member of \
         the launch mob made:\n{}",
        wrong.join("\n")
    );
}

/// The storage doctor answers the stdio door and an administrator on the
/// console for every mob store of the state directory, with the same counts:
/// the stores of two mobs the host created and of a mob one of its agents
/// created. The stdio door has one caller, the host.
#[tokio::test]
async fn the_storage_doctor_answers_the_stdio_door_and_an_administrator_for_every_mob_store() {
    use crate::rpc::mob_hosting_tests::{config, member_created_mob, result, stored_host};

    let temp = tempfile::tempdir().expect("tempdir");
    let launch_mob = format!("door-rows-doctor-all-{}", uuid::Uuid::new_v4().simple());
    let second = format!("{launch_mob}-second");
    let third = format!("{launch_mob}-third");
    let launch = stored_host(&temp, &launch_mob).await;
    for mob_id in [&second, &third] {
        result(
            &launch,
            "meta/mobs/create",
            json!({"mob_config": config(mob_id)}),
        )
        .await;
    }
    let of_a_member = member_created_mob(&launch, &format!("{launch_mob}-fourth"))
        .await
        .to_string();
    let app = console_json_router_with_state(door_rows_state(&launch, &second))
        .layer(axum::Extension(launch.mob_resolver()));
    let beside = [second.as_str(), third.as_str(), of_a_member.as_str()];

    let mut wrong = Vec::new();
    let on_stdio = result(
        &launch,
        crate::rpc::storage_methods::STORAGE_DOCTOR_METHOD,
        json!({"state_dir": temp.path().display().to_string()}),
    )
    .await;
    let read = |answer: &Value| {
        (
            mobs_a_doctor_names(answer, &beside),
            census_number(
                answer,
                temp.path(),
                crate::storage_doctor::FINDING_HOST_CREATED_MOBS,
                " mob(s) the host created",
            ),
            host_created_stores_found(answer, temp.path()),
            mob_stores_counted(answer, temp.path()),
        )
    };
    let of_the_stdio_door = read(&on_stdio);
    if of_the_stdio_door.0 != beside
        || of_the_stdio_door.1 != Some(2)
        || of_the_stdio_door.2 != 2
        || of_the_stdio_door.3.0 != Some(3)
    {
        wrong.push(format!(
            "the stdio door is not answered every mob store, two mobs the host created in three \
             stores: {of_the_stdio_door:?}"
        ));
    }
    match doctor_answer_to(&app, DOOR_ROWS_ADMIN, temp.path()).await {
        Ok(answer) => {
            let of_an_administrator = read(&answer);
            if of_an_administrator != of_the_stdio_door {
                wrong.push(format!(
                    "an administrator on the console is not answered what the stdio door is: \
                     {of_an_administrator:?} and {of_the_stdio_door:?}"
                ));
            }
            // The paths that name a mob beside the launch mob, sorted.
            let of_the_mobs_beside = |answer: &Value| {
                let mut paths: Vec<String> = doctor_paths(answer)
                    .into_iter()
                    .filter(|path| beside.iter().any(|mob| path.contains(mob)))
                    .collect();
                paths.sort();
                paths
            };
            let (console_paths, stdio_paths) =
                (of_the_mobs_beside(&answer), of_the_mobs_beside(&on_stdio));
            if console_paths != stdio_paths || console_paths.is_empty() {
                wrong.push(format!(
                    "an administrator on the console and the stdio door are answered different \
                     paths of the mobs beside the launch mob: {console_paths:?} and \
                     {stdio_paths:?}"
                ));
            }
        }
        Err(answer) => wrong.push(format!(
            "an administrator's doctor is refused: {answer:.300}"
        )),
    }

    drop(app);
    let set = launch
        .mob_runtime()
        .agent_mob_mcp_state()
        .expect("the launch's mob set");
    for mob_id in beside {
        if let Ok(handle) = set.handle_for(&meerkat_mob::MobId::from(mob_id)).await {
            let _ = handle.shutdown().await;
        }
    }
    drop(set);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(30), launch.shutdown()).await;
    assert!(
        wrong.is_empty(),
        "the storage doctor on the stdio door and for an administrator:\n{}",
        wrong.join("\n")
    );
}

/// The storage doctor on the console answers every holder of the admin
/// grant the store of a mob that a member of the launch mob made: such a
/// mob is in the launch mob's own reach, which every caller the door lets
/// in is answered. The host has one mob beside its launch mob, made by a
/// member of the launch mob. A person who holds the grant and whom no rule
/// lists a mob for is answered the path of its store and the counts of mob
/// stores an administrator is answered.
#[tokio::test]
async fn the_storage_doctor_on_the_console_answers_the_store_of_a_mob_a_member_of_the_launch_mob_made()
 {
    use crate::rpc::mob_hosting_tests::{member_created_mob, stored_host};

    let temp = tempfile::tempdir().expect("tempdir");
    let launch_mob = format!("door-rows-doctor-child-{}", uuid::Uuid::new_v4().simple());
    let launch = stored_host(&temp, &launch_mob).await;
    let of_a_member = member_created_mob(&launch, &format!("{launch_mob}-second"))
        .await
        .to_string();
    let app = console_json_router_with_state(door_rows_state(&launch, "a-mob-no-rule-is-asked-of"))
        .layer(axum::Extension(launch.mob_resolver()));
    let beside = [of_a_member.as_str()];

    let mut wrong = Vec::new();
    let mut counts = Vec::new();
    for (who, email) in [
        ("an administrator", DOOR_ROWS_ADMIN),
        ("a person no rule lists a mob for", DOOR_ROWS_OPERATOR),
    ] {
        match doctor_answer_to(&app, email, temp.path()).await {
            Ok(answer) => {
                let named = mobs_a_doctor_names(&answer, &beside);
                if named != beside {
                    wrong.push(format!(
                        "{who}: the answer names the mobs {named:?}, not the mob a member of the \
                         launch mob made"
                    ));
                }
                counts.push(mob_stores_counted(&answer, temp.path()));
            }
            Err(answer) => wrong.push(format!("{who}: the doctor is refused: {answer:.300}")),
        }
    }
    if let [of_an_administrator, of_the_person] = counts.as_slice()
        && (of_the_person != of_an_administrator || of_the_person.0 != Some(1))
    {
        wrong.push(format!(
            "a person no rule lists a mob for is answered the counts {of_the_person:?}, an \
             administrator {of_an_administrator:?}: not the same counts of one mob store"
        ));
    }

    drop(app);
    let set = launch
        .mob_runtime()
        .agent_mob_mcp_state()
        .expect("the launch's mob set");
    if let Ok(handle) = set
        .handle_for(&meerkat_mob::MobId::from(of_a_member.as_str()))
        .await
    {
        let _ = handle.shutdown().await;
    }
    drop(set);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(30), launch.shutdown()).await;
    assert!(
        wrong.is_empty(),
        "the storage doctor on the console over a mob a member of the launch mob made:\n{}",
        wrong.join("\n")
    );
}

/// The storage doctor on the console leaves out, with the store of a mob
/// its caller may not address, the store of a mob that a member of that mob
/// made: such a mob is in the reach of the mob the host created. The host
/// created one mob beside its launch mob, an access rule lists it for one
/// person, and a member of it made a mob of its own. A person who holds the
/// admin grant and whom no rule lists a mob for is answered no path of
/// either store, and the counts of mob stores are lower by two than an
/// administrator's. The person the rule lists the mob for is answered both
/// stores, as an administrator is.
#[tokio::test]
async fn the_storage_doctor_on_the_console_leaves_out_a_mob_made_under_a_mob_its_caller_may_not_address()
 {
    use crate::rpc::mob_hosting_tests::{config, member_created_mob_in, result, stored_host};

    let temp = tempfile::tempdir().expect("tempdir");
    let launch_mob = format!("door-rows-doctor-below-{}", uuid::Uuid::new_v4().simple());
    let hosted = format!("{launch_mob}-second");
    let launch = stored_host(&temp, &launch_mob).await;
    result(
        &launch,
        "meta/mobs/create",
        json!({"mob_config": config(&hosted)}),
    )
    .await;
    let below = member_created_mob_in(
        &launch,
        &meerkat_mob::MobId::from(hosted.as_str()),
        &format!("{launch_mob}-third"),
    )
    .await
    .to_string();
    let app = console_json_router_with_state(door_rows_state(&launch, &hosted))
        .layer(axum::Extension(launch.mob_resolver()));
    let beside = [hosted.as_str(), below.as_str()];

    let mut wrong = Vec::new();
    let mut of_an_administrator = None;
    for (who, email, shown) in [
        ("an administrator", DOOR_ROWS_ADMIN, beside.to_vec()),
        (
            "a person a rule lists the mob the host created for",
            DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB,
            beside.to_vec(),
        ),
        (
            "a person no rule lists a mob for",
            DOOR_ROWS_OPERATOR,
            Vec::new(),
        ),
    ] {
        match doctor_answer_to(&app, email, temp.path()).await {
            Ok(answer) => {
                let named = mobs_a_doctor_names(&answer, &beside);
                if named != shown {
                    wrong.push(format!(
                        "{who}: the answer names the mobs {named:?}, not {shown:?}; its paths: \
                         {:?}",
                        doctor_paths(&answer)
                            .into_iter()
                            .filter(|path| beside.iter().any(|mob| path.contains(mob)))
                            .collect::<Vec<_>>()
                    ));
                }
                let counted = mob_stores_counted(&answer, temp.path());
                let all = *of_an_administrator.get_or_insert(counted);
                let left_out = u64::try_from(beside.len() - shown.len()).unwrap_or(u64::MAX);
                let fewer = |count: Option<u64>| count.and_then(|n| n.checked_sub(left_out));
                let expected = (fewer(all.0), fewer(all.1), fewer(all.2));
                if counted != expected || counted.0.is_none() {
                    wrong.push(format!(
                        "{who}: the answer counts the mob stores {counted:?}, not {expected:?}: \
                         an administrator's counts {all:?} less the {left_out} left out"
                    ));
                }
            }
            Err(answer) => wrong.push(format!("{who}: the doctor is refused: {answer:.300}")),
        }
    }

    drop(app);
    let set = launch
        .mob_runtime()
        .agent_mob_mcp_state()
        .expect("the launch's mob set");
    for mob_id in [&below, &hosted] {
        if let Ok(handle) = set
            .handle_for(&meerkat_mob::MobId::from(mob_id.as_str()))
            .await
        {
            let _ = handle.shutdown().await;
        }
    }
    drop(set);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(30), launch.shutdown()).await;
    assert!(
        wrong.is_empty(),
        "the storage doctor on the console over a mob made under a mob the host created:\n{}",
        wrong.join("\n")
    );
}

/// A mob that a member of a mob the host created made is judged by the mob
/// the host created, not by its own id. The scene of the row above, with
/// the access rule listing the lower mob and not the mob the host created.
/// The host's records decide the lower mob's reach: it is the mob the host
/// created. The person the rule lists the lower mob for may not address the
/// mob the host created, and is answered no path of either store, with the
/// counts of mob stores lower by two than an administrator's.
#[tokio::test]
async fn the_storage_doctor_on_the_console_judges_a_mob_made_under_a_mob_the_host_created_by_that_mob()
 {
    use crate::rpc::mob_hosting_tests::{config, member_created_mob_in, result, stored_host};

    let temp = tempfile::tempdir().expect("tempdir");
    let launch_mob = format!("door-rows-doctor-lower-{}", uuid::Uuid::new_v4().simple());
    let hosted = format!("{launch_mob}-second");
    let launch = stored_host(&temp, &launch_mob).await;
    result(
        &launch,
        "meta/mobs/create",
        json!({"mob_config": config(&hosted)}),
    )
    .await;
    let below = member_created_mob_in(
        &launch,
        &meerkat_mob::MobId::from(hosted.as_str()),
        &format!("{launch_mob}-third"),
    )
    .await
    .to_string();
    // The rule lists the lower mob alone.
    let app = console_json_router_with_state(door_rows_state(&launch, &below))
        .layer(axum::Extension(launch.mob_resolver()));
    let beside = [hosted.as_str(), below.as_str()];

    let mut wrong = Vec::new();
    // The scene, read: the records put the lower mob in the reach of the
    // mob the host created.
    let reaches = launch.mob_resolver().hosting_of_every_mob().await;
    let of_the_lower_mob = reaches.as_ref().ok().and_then(|reaches| {
        reaches
            .iter()
            .find(|(mob, _)| mob.as_str() == below)
            .map(|(_, reach)| reach.clone())
    });
    println!("the reach of the lower mob, by the set's records: {of_the_lower_mob:?}");
    if of_the_lower_mob != Some(Ok(Some(meerkat_mob::MobId::from(hosted.as_str())))) {
        wrong.push(format!(
            "the fixture of this row is a lower mob whose reach the records decide: \
             {of_the_lower_mob:?} of {reaches:?}"
        ));
    }
    let mut of_an_administrator = None;
    for (who, email, shown) in [
        ("an administrator", DOOR_ROWS_ADMIN, beside.to_vec()),
        (
            "a person a rule lists the lower mob alone for",
            DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB,
            Vec::new(),
        ),
    ] {
        match doctor_answer_to(&app, email, temp.path()).await {
            Ok(answer) => {
                let named = mobs_a_doctor_names(&answer, &beside);
                if named != shown {
                    wrong.push(format!(
                        "{who}: the answer names the mobs {named:?}, not {shown:?}; its paths: \
                         {:?}",
                        doctor_paths(&answer)
                            .into_iter()
                            .filter(|path| beside.iter().any(|mob| path.contains(mob)))
                            .collect::<Vec<_>>()
                    ));
                }
                let counted = mob_stores_counted(&answer, temp.path());
                let all = *of_an_administrator.get_or_insert(counted);
                let left_out = u64::try_from(beside.len() - shown.len()).unwrap_or(u64::MAX);
                let fewer = |count: Option<u64>| count.and_then(|n| n.checked_sub(left_out));
                let expected = (fewer(all.0), fewer(all.1), fewer(all.2));
                if counted != expected || counted.0.is_none() {
                    wrong.push(format!(
                        "{who}: the answer counts the mob stores {counted:?}, not {expected:?}: \
                         an administrator's counts {all:?} less the {left_out} left out"
                    ));
                }
            }
            Err(answer) => wrong.push(format!("{who}: the doctor is refused: {answer:.300}")),
        }
    }

    drop(app);
    let set = launch
        .mob_runtime()
        .agent_mob_mcp_state()
        .expect("the launch's mob set");
    for mob_id in [&below, &hosted] {
        if let Ok(handle) = set
            .handle_for(&meerkat_mob::MobId::from(mob_id.as_str()))
            .await
        {
            let _ = handle.shutdown().await;
        }
    }
    drop(set);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(30), launch.shutdown()).await;
    assert!(
        wrong.is_empty(),
        "the storage doctor on the console over a mob judged by the mob the host created:\n{}",
        wrong.join("\n")
    );
}

/// The storage doctor on the console leaves the memory store of a mob out
/// of the answer of a caller who may not address the mob. The mob beside
/// the launch mob keeps its agent's memory in a directory of its own under
/// the launch's memory root. A person who holds the admin grant and whom no
/// rule lists the mob for is answered no path in that directory and no path
/// of the mob's store; an administrator is answered both.
#[tokio::test]
async fn the_storage_doctor_on_the_console_leaves_out_the_memory_of_a_mob_its_caller_may_not_address()
 {
    use crate::unified_runtime::mob_memory_release_tests::HostOfARememberingMob;

    let (fixture, launch) = HostOfARememberingMob::launched().await;
    let state_dir = fixture.state_dir();
    let mob_id = fixture.mob.to_string();
    // The mob's memory directory as a path names it: the directory of the
    // hosted mobs and the mob's own directory in it.
    let memory_dir = format!(
        "{}/{}",
        crate::memory::sqlite_store::HOSTED_MOBS_DIR,
        fixture
            .memory_dir_of_the_mob()
            .file_name()
            .expect("the mob's memory directory has a name")
            .to_string_lossy()
    );
    let app = console_json_router_with_state(door_rows_state(&launch, "a-mob-no-rule-is-asked-of"))
        .layer(axum::Extension(launch.mob_resolver()));

    let mut wrong = Vec::new();
    for (who, email, shown) in [
        ("an administrator", DOOR_ROWS_ADMIN, true),
        (
            "a person no rule lists a mob for",
            DOOR_ROWS_OPERATOR,
            false,
        ),
    ] {
        match doctor_answer_to(&app, email, &state_dir).await {
            Ok(answer) => {
                let paths = doctor_paths(&answer);
                let of_its_memory: Vec<&String> = paths
                    .iter()
                    .filter(|path| path.contains(&memory_dir))
                    .collect();
                let of_its_store: Vec<&String> = paths
                    .iter()
                    .filter(|path| path.contains(&mob_id) && !path.contains(&memory_dir))
                    .collect();
                if of_its_memory.is_empty() == shown || of_its_store.is_empty() == shown {
                    wrong.push(format!(
                        "{who}: the mob's memory and its store are {}: memory {of_its_memory:?}, \
                         store {of_its_store:?}",
                        if shown {
                            "not both answered"
                        } else {
                            "answered"
                        }
                    ));
                }
                if !shown && answer.to_string().contains(&mob_id) {
                    wrong.push(format!("{who}: the answer names the mob somewhere"));
                }
            }
            Err(answer) => wrong.push(format!("{who}: the doctor is refused: {answer:.300}")),
        }
    }

    drop(app);
    fixture.ends(launch).await;
    assert!(
        wrong.is_empty(),
        "the storage doctor on the console over a mob's memory:\n{}",
        wrong.join("\n")
    );
}

/// A mob store whose creation record does not read is judged by the id of
/// its mob. A host created a mob beside its launch mob and stopped; the
/// file of the mob's store is then no database any more; the host starts
/// again over the same state. Its mob set lists the mob, and its records do
/// not decide whose reach the mob is in. A person who holds the admin grant
/// and whom no rule lists that id for is answered nothing of the mob; a
/// person who holds it and whom a rule lists that id for is answered the
/// store's file, and so is an administrator.
///
/// When that mob was created does not read either. A backup lies beside its
/// store, named with a time a day from now: the person a rule lists the id
/// for is not answered it, for its age against the mob that stands cannot
/// be told; an administrator is.
#[tokio::test]
async fn the_storage_doctor_on_the_console_judges_a_mob_store_whose_creation_does_not_read_by_the_id_of_its_mob()
 {
    use crate::unified_runtime::mob_memory_release_tests::HostOfARememberingMob;

    let (fixture, first) = HostOfARememberingMob::launched().await;
    let state_dir = fixture.state_dir();
    let mob_id = fixture.mob.to_string();
    // The file of the mob's store as the first launch keeps it, under the
    // state directory's `mobs`.
    let of_the_mob_on_disk: Vec<std::path::PathBuf> = fixture
        .state_files_of_the_mob()
        .into_keys()
        .map(|below| state_dir.join(below))
        .collect();
    let databases: Vec<std::path::PathBuf> = of_the_mob_on_disk
        .iter()
        .filter(|path| {
            path.extension().is_some_and(|extension| extension == "db")
                && path
                    .parent()
                    .and_then(std::path::Path::file_name)
                    .is_some_and(|directory| directory == "mobs")
        })
        .cloned()
        .collect();
    assert_eq!(
        databases.len(),
        1,
        "the mob has one store file under mobs: {of_the_mob_on_disk:?}"
    );
    let store = databases[0].clone();
    // A backup of the store, named as the offline tools name one, with a
    // time a day from now.
    let backup = {
        let a_day_from_now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_secs())
            + 86_400;
        let mut name = store.clone().into_os_string();
        name.push(format!(".pre-0.9.17-{a_day_from_now}"));
        std::path::PathBuf::from(name)
    };
    std::fs::write(&backup, b"a copy").expect("a backup beside the store");
    let backup_file = backup
        .file_name()
        .expect("the backup has a name")
        .to_string_lossy()
        .into_owned();
    let launch = fixture
        .started_again_after(first, || {
            // The store's file is no database any more, and nothing of its
            // write-ahead log is left beside it.
            std::fs::write(&store, [b'x'; 4096]).expect("the store file is overwritten");
            for beside in ["-wal", "-shm"] {
                let mut name = store.clone().into_os_string();
                name.push(beside);
                let _ = std::fs::remove_file(name);
            }
        })
        .await;
    let app = console_json_router_with_state(door_rows_state(&launch, &mob_id))
        .layer(axum::Extension(launch.mob_resolver()));

    // The scene, read: the set's records list the mob and do not decide its
    // reach.
    let reaches = launch.mob_resolver().hosting_of_every_mob().await;
    let of_the_mob = reaches.as_ref().ok().and_then(|reaches| {
        reaches
            .iter()
            .find(|(mob, _)| mob == &fixture.mob)
            .map(|(_, reach)| reach.clone())
    });
    println!("the reach of the mob, by the set's records: {of_the_mob:?}");
    let mut wrong = Vec::new();
    if !matches!(of_the_mob, Some(Err(_))) {
        wrong.push(format!(
            "the fixture of this row is a mob the set lists whose reach its records do not \
             decide: {of_the_mob:?} of {reaches:?}"
        ));
    }
    // And when the mob was created does not read, while the backup is
    // still beside the store.
    let created = launch
        .mob_resolver()
        .standing_mob_created_at_ms(&fixture.mob)
        .await;
    if created.is_some() || !backup.is_file() {
        wrong.push(format!(
            "the fixture of this row is a mob whose creation does not read, with a backup              beside its store: created {created:?}, the backup is there: {}",
            backup.is_file()
        ));
    }
    let store_file = store
        .file_name()
        .expect("the store file has a name")
        .to_string_lossy()
        .into_owned();
    for (who, email, shown, shown_the_backup) in [
        ("an administrator", DOOR_ROWS_ADMIN, true, true),
        (
            "a person a rule lists the mob's id for",
            DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB,
            true,
            false,
        ),
        (
            "a person no rule lists a mob for",
            DOOR_ROWS_OPERATOR,
            false,
            false,
        ),
    ] {
        match doctor_answer_to(&app, email, &state_dir).await {
            Ok(answer) => {
                let of_its_store: Vec<String> = doctor_paths(&answer)
                    .into_iter()
                    .filter(|path| path.ends_with(&store_file))
                    .collect();
                if of_its_store.is_empty() == shown {
                    wrong.push(format!(
                        "{who}: the file of the mob's store is {}: {of_its_store:?}",
                        if shown { "not answered" } else { "answered" }
                    ));
                }
                let the_backup = doctor_paths(&answer)
                    .iter()
                    .any(|path| path.ends_with(&backup_file));
                if the_backup != shown_the_backup {
                    wrong.push(format!(
                        "{who}: the backup beside the mob's store is {}",
                        if shown_the_backup {
                            "not answered"
                        } else {
                            "answered"
                        }
                    ));
                }
                if !shown && answer.to_string().contains(&mob_id) {
                    wrong.push(format!("{who}: the answer names the mob somewhere"));
                }
            }
            Err(answer) => wrong.push(format!("{who}: the doctor is refused: {answer:.300}")),
        }
    }

    drop(app);
    fixture.ends(launch).await;
    assert!(
        wrong.is_empty(),
        "the storage doctor on the console over a mob store whose creation does not read:\n{}",
        wrong.join("\n")
    );
}

/// The route list on the console answers a caller the launch mob's routes
/// and the routes of the mobs the caller may address. The host added one
/// route for its launch mob, one for each of two mobs it created beside it,
/// and one for a mob a member of the launch mob made; an access rule lists
/// one of the mobs the host created for one person. A person who holds the
/// admin grant and whom no rule lists a mob for is answered the launch
/// mob's route and the route of the mob its member made, which is in the
/// launch mob's own reach; the person the rule lists a mob for, that mob's
/// route too; an administrator all four.
#[tokio::test]
async fn the_route_list_on_the_console_answers_the_routes_of_the_mobs_its_caller_may_address() {
    use crate::rpc::mob_hosting_tests::{config, host, member_created_mob, result};

    let temp = tempfile::tempdir().expect("tempdir");
    let launch_mob = format!("door-rows-routes-{}", uuid::Uuid::new_v4().simple());
    let listed_by_a_rule = format!("{launch_mob}-second");
    let listed_by_no_rule = format!("{launch_mob}-third");
    let launch = host(&temp, &launch_mob).await;
    for mob_id in [&listed_by_a_rule, &listed_by_no_rule] {
        result(
            &launch,
            "meta/mobs/create",
            json!({"mob_config": config(mob_id)}),
        )
        .await;
    }
    let of_a_member = member_created_mob(&launch, &format!("{launch_mob}-fourth")).await;
    let route = |route_key: &str, mob_id: Option<&String>| {
        let mut route = json!({
            "route_key": route_key,
            "recipient": "desk",
            "sink": "a-sink",
            "target_module": "a-module",
        });
        if let Some(mob_id) = mob_id {
            route["mob_id"] = json!(mob_id);
        }
        route
    };
    for (route_key, mob_id) in [
        ("of-the-launch-mob", None),
        ("of-the-mob-a-rule-lists", Some(&listed_by_a_rule)),
        ("of-the-mob-no-rule-lists", Some(&listed_by_no_rule)),
    ] {
        result(&launch, "meta/routing/routes/add", route(route_key, mob_id)).await;
    }
    // The route of the mob a member made is added as a Rust host adds one.
    launch
        .module_runtime_handle()
        .lock()
        .await
        .add_runtime_route(crate::runtime::RuntimeRoute {
            route_key: "of-a-mob-a-member-made".to_string(),
            mob_id: Some(of_a_member.clone()),
            recipient: "desk".to_string(),
            channel: None,
            sink: "a-sink".to_string(),
            target_module: "a-module".to_string(),
            retry_max: None,
            backoff_ms: None,
            rate_limit_per_minute: None,
        })
        .expect("the route of the mob a member made");
    let state = ConsoleJsonState {
        module_runtime: Some(launch.module_runtime_handle()),
        ..door_rows_state(&launch, &listed_by_a_rule)
    };
    let app = console_json_router_with_state(state).layer(axum::Extension(launch.mob_resolver()));

    let mut wrong = Vec::new();
    for (who, email, expected) in [
        (
            "a person no rule lists a mob for",
            DOOR_ROWS_OPERATOR,
            vec!["of-a-mob-a-member-made", "of-the-launch-mob"],
        ),
        (
            "a person a rule lists one mob for",
            DOOR_ROWS_OPERATOR_OF_THE_SECOND_MOB,
            vec![
                "of-a-mob-a-member-made",
                "of-the-launch-mob",
                "of-the-mob-a-rule-lists",
            ],
        ),
        (
            "an administrator",
            DOOR_ROWS_ADMIN,
            vec![
                "of-a-mob-a-member-made",
                "of-the-launch-mob",
                "of-the-mob-a-rule-lists",
                "of-the-mob-no-rule-lists",
            ],
        ),
    ] {
        let answer = ask_as(
            &app,
            rpc_request("meta/routing/routes/list", &json!({})),
            email,
        )
        .await;
        let body: Value = serde_json::from_str(&answer.2).unwrap_or(Value::Null);
        let Some(routes) = body["result"]["routes"].as_array() else {
            wrong.push(format!("{who}: no list of routes: {:.300}", answer.2));
            continue;
        };
        let mut keys: Vec<&str> = routes
            .iter()
            .filter_map(|route| route["route_key"].as_str())
            .collect();
        keys.sort_unstable();
        if keys != expected {
            wrong.push(format!(
                "{who}: answered the routes {keys:?}, not {expected:?}: {:.400}",
                answer.2
            ));
        }
        if expected.len() < 4 && answer.2.contains(&listed_by_no_rule) {
            wrong.push(format!(
                "{who}: the answer names a mob the caller may not address"
            ));
        }
    }

    drop(app);
    let set = launch
        .mob_runtime()
        .agent_mob_mcp_state()
        .expect("the launch's mob set");
    for mob_id in [
        listed_by_a_rule.as_str(),
        listed_by_no_rule.as_str(),
        of_a_member.as_str(),
    ] {
        if let Ok(handle) = set.handle_for(&meerkat_mob::MobId::from(mob_id)).await {
            let _ = handle.shutdown().await;
        }
    }
    drop(set);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(30), launch.shutdown()).await;
    assert!(
        wrong.is_empty(),
        "the route list on the console, by its caller:\n{}",
        wrong.join("\n")
    );
}

/// The audit row of a create over the mobs of the host names the mob the
/// create is of: the id of the definition the request carries. So the row
/// is shown as the rows of the other changes of a mob are, to a reader who
/// may address that mob. An administrator's create and a create refused for
/// its caller each leave a row with the id; an administrator reads both,
/// and a person who holds the audit read and may not address the mobs reads
/// neither. A create whose definition does not read names no mob: its row
/// is an administrator's alone, like the row of a mob id that is blank.
#[tokio::test]
async fn the_audit_row_of_a_create_over_the_mobs_of_the_host_names_the_mob_it_is_of() {
    const CREATE: &str = "meta/mobs/create";

    let temp = tempfile::tempdir().expect("tempdir");
    let launch_mob = format!("door-rows-audit-create-{}", uuid::Uuid::new_v4().simple());
    let made = format!("{launch_mob}-second");
    let refused = format!("{launch_mob}-third");
    let launch = crate::rpc::mob_hosting_tests::host(&temp, &launch_mob).await;
    let state = ConsoleJsonState {
        console_audit: Some(audit_log_holding(&[]).await),
        ..door_rows_state(&launch, "a-mob-no-rule-is-asked-of")
    };
    let app = console_json_router_with_state(state).layer(axum::Extension(launch.mob_resolver()));
    let config = crate::rpc::mob_hosting_tests::config;

    let mut wrong = Vec::new();
    let created = ask_as(
        &app,
        rpc_request(CREATE, &json!({"mob_config": config(&made)})),
        DOOR_ROWS_ADMIN,
    )
    .await;
    let body: Value = serde_json::from_str(&created.2).unwrap_or(Value::Null);
    if body["result"]["mob"]["mob_id"] != json!(made) {
        wrong.push(format!(
            "an administrator's create is not served: {:.300}",
            created.2
        ));
    }
    let answer = ask_as(
        &app,
        rpc_request(CREATE, &json!({"mob_config": config(&refused)})),
        DOOR_ROWS_OPERATOR,
    )
    .await;
    if !is_the_mob_ids_refusal("console RPC", &answer, &refused) {
        wrong.push(format!(
            "a create by a person no rule lists the id for is not refused: {:.300}",
            answer.2
        ));
    }
    let unread = ask_as(
        &app,
        rpc_request(CREATE, &json!({"mob_config": "[mob\nid ="})),
        DOOR_ROWS_OPERATOR,
    )
    .await;
    let body: Value = serde_json::from_str(&unread.2).unwrap_or(Value::Null);
    if body["error"]["code"] != json!(-32602) {
        wrong.push(format!(
            "a create whose definition does not read is not refused for its params: {:.300}",
            unread.2
        ));
    }

    // The mob each create row of the log names, as `email` reads the log
    // with every mob selected, oldest first: `null` for a row that names
    // none.
    let create_rows_as = |email: &'static str| {
        let app = &app;
        async move {
            let answer = ask_as(
                app,
                rpc_request(crate::rpc::CONSOLE_AUDIT_METHOD, &audit_read_of_every_mob()),
                email,
            )
            .await;
            let body: Value = serde_json::from_str(&answer.2).unwrap_or(Value::Null);
            body["result"]["records"]
                .as_array()
                .map(|rows| {
                    rows.iter()
                        .filter(|row| row["method"] == json!(CREATE))
                        .map(|row| {
                            (
                                row["target"]["mob_id"].clone(),
                                row["outcome"].as_str().unwrap_or("<none>").to_string(),
                            )
                        })
                        .collect::<Vec<(Value, String)>>()
                })
                .ok_or(answer.2)
        }
    };
    let committed = |mob_id: &str| (json!(mob_id), "committed".to_string());
    let was_refused = |mob_id: &str| (json!(mob_id), "refused".to_string());
    match create_rows_as(DOOR_ROWS_ADMIN).await {
        Ok(rows) if rows == [committed(&made), was_refused(&refused), was_refused("")] => {}
        Ok(rows) => wrong.push(format!(
            "an administrator does not read the three create rows, each with the mob it is of \
             (none for a definition that does not read): {rows:?}"
        )),
        Err(answer) => wrong.push(format!(
            "an administrator's audit read is refused: {answer:.300}"
        )),
    }
    match create_rows_as(DOOR_ROWS_OPERATOR).await {
        Ok(rows) if rows.is_empty() => {}
        Ok(rows) => wrong.push(format!(
            "a person who may address neither mob reads create rows: {rows:?}"
        )),
        Err(answer) => wrong.push(format!(
            "the audit read of a person who holds it is refused: {answer:.300}"
        )),
    }

    drop(app);
    let set = launch
        .mob_runtime()
        .agent_mob_mcp_state()
        .expect("the launch's mob set");
    if let Ok(handle) = set
        .handle_for(&meerkat_mob::MobId::from(made.as_str()))
        .await
    {
        let _ = handle.shutdown().await;
    }
    drop(set);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(30), launch.shutdown()).await;
    assert!(
        wrong.is_empty(),
        "the audit rows of creates over the mobs of the host:\n{}",
        wrong.join("\n")
    );
}

/// What a read of the host's own records answers, by the mobs its door
/// stated for the caller. A caller who is answered every mob is answered
/// the records of every mob, and the storage doctor leaves no store out.
/// A caller who is answered some mobs is answered the launch mob's records
/// (those that name no mob, and those that name it by its id), the records
/// of those mobs and of no other, a mob the host's mob set does not list
/// included; the doctor shows the stores and the memory directories of
/// the mobs the door named as answered, and of no other.
/// When the host's records could not be read the caller is answered the
/// launch mob's records alone, and the doctor leaves every mob's store out.
/// On a host whose launch declared its mob to no mob set the caller is
/// answered the launch mob's records alone too, and the doctor leaves no
/// store out.
#[tokio::test]
async fn a_read_of_the_hosts_records_answers_the_mobs_its_door_stated() {
    use crate::storage_doctor::MobStoresLeftOut;
    use std::collections::BTreeSet;

    let temp = tempfile::tempdir().expect("tempdir");
    let launch_id = format!("door-rows-records-{}", uuid::Uuid::new_v4().simple());
    let launch = crate::rpc::mob_hosting_tests::host(&temp, &launch_id).await;
    let mob = |id: &str| meerkat_mob::MobId::from(id);
    let (launch_mob, second, third) = (mob(&launch_id), mob("second"), mob("third"));
    let some = MobsOfHostRecords::Only {
        answered: BTreeSet::from([second.clone()]),
        created_at_ms: std::collections::BTreeMap::new(),
    };

    for (stated, of_the_second, of_the_third) in [
        (&MobsOfHostRecords::Every, true, true),
        (&some, true, false),
        (&MobsOfHostRecords::NoMobSet, false, false),
        (&MobsOfHostRecords::LaunchMobAlone, false, false),
    ] {
        assert!(
            stated.answers(&launch_mob, None) && stated.answers(&launch_mob, Some(&launch_mob)),
            "the launch mob's records are every caller's: {stated:?}"
        );
        assert_eq!(
            (
                stated.answers(&launch_mob, Some(&second)),
                stated.answers(&launch_mob, Some(&third)),
                stated.answers(&launch_mob, Some(&mob("no-mob-of-the-set"))),
            ),
            (
                of_the_second,
                of_the_third,
                *stated == MobsOfHostRecords::Every
            ),
            "{stated:?}"
        );
    }

    let runtime = launch.mob_runtime();
    assert_eq!(
        mob_stores_left_out_for(runtime, &MobsOfHostRecords::Every),
        MobStoresLeftOut::none()
    );
    assert_eq!(
        mob_stores_left_out_for(runtime, &MobsOfHostRecords::NoMobSet),
        MobStoresLeftOut::none()
    );
    assert_eq!(
        mob_stores_left_out_for(runtime, &MobsOfHostRecords::LaunchMobAlone),
        MobStoresLeftOut::every()
    );
    // This host keeps its mobs in memory: a mob the caller is answered has
    // no store file to name, and its memory directory is named. The time
    // the mob was created at did not read.
    assert_eq!(
        mob_stores_left_out_for(runtime, &some),
        MobStoresLeftOut::showing_only(
            std::iter::empty(),
            [(std::ffi::OsString::from("second"), None)]
        )
    );

    let _ = tokio::time::timeout(std::time::Duration::from_secs(30), launch.shutdown()).await;
}

/// A door that was not told which mobs of the host its caller is answered
/// answers the launch mob alone. That is what a served mob holds before a
/// door states the mobs, and what a door with no resolver to ask the host's
/// records with states for a caller who may not address every mob. A caller
/// who may address every mob (an administrator, and every caller where
/// access is not enforced) is answered every mob, with a resolver or
/// without one.
#[tokio::test]
async fn a_door_that_was_not_told_the_mobs_of_the_hosts_records_answers_the_launch_mob_alone() {
    let temp = tempfile::tempdir().expect("tempdir");
    let launch_id = format!("door-rows-not-told-{}", uuid::Uuid::new_v4().simple());
    let launch = crate::rpc::mob_hosting_tests::host(&temp, &launch_id).await;
    let state = door_rows_state(&launch, "a-mob-no-rule-is-asked-of");
    let controller = state.access.clone().expect("the fixture has access rules");
    let operator = controller.view_for_subject(Some(DOOR_ROWS_OPERATOR));
    let administrator = controller.view_for_subject(Some(DOOR_ROWS_ADMIN));
    let where_nothing_is_enforced =
        AccessController::disabled().view_for_subject(Some(DOOR_ROWS_OPERATOR));
    // The scene, read: who may address every mob.
    let may_address_every_mob = (
        operator.may_address_every_mob(),
        administrator.may_address_every_mob(),
        where_nothing_is_enforced.may_address_every_mob(),
    );
    let stated = (
        mobs_of_the_hosts_records_for(
            None,
            &operator,
            crate::rpc::storage_methods::STORAGE_DOCTOR_METHOD,
        )
        .await,
        mobs_of_the_hosts_records_for(
            None,
            &administrator,
            crate::rpc::storage_methods::STORAGE_DOCTOR_METHOD,
        )
        .await,
        mobs_of_the_hosts_records_for(
            None,
            &where_nothing_is_enforced,
            crate::rpc::storage_methods::STORAGE_DOCTOR_METHOD,
        )
        .await,
    );
    let _ = tokio::time::timeout(std::time::Duration::from_secs(30), launch.shutdown()).await;

    assert_eq!(
        may_address_every_mob,
        (false, true, true),
        "an operator, an administrator, a caller where access is not enforced"
    );
    assert_eq!(
        MobsOfHostRecords::default(),
        MobsOfHostRecords::LaunchMobAlone,
        "what a served mob holds before a door states the mobs"
    );
    assert_eq!(
        stated,
        (
            MobsOfHostRecords::LaunchMobAlone,
            MobsOfHostRecords::Every,
            MobsOfHostRecords::Every,
        ),
        "what a door with no resolver states for each of the three callers"
    );
}

/// The console's send and its recovery for a mob the host serves beside
/// its launch mob: the rows share this file's fixtures.
#[path = "console_door_send_tests.rs"]
mod console_door_send_tests;

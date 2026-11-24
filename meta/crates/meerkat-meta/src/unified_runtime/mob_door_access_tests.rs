//! The request doors that have callers, on a console that knows who asks
//! and has access rules: what such a door answers for the mob a request
//! names.
//!
//! A door knows its caller before it reads the mob id. Its answer for an id
//! it does not serve to the caller is one refusal: the same whoever asks
//! and whatever the reason, naming the id the request named and no other
//! mob. The host's own mob named by its id is the same request as no mob:
//! the door's own rules answer it.
//!
//! The rows ask every door of the console that reads a mob id: the two
//! request doors, the four event streams and the twelve console routes.

#![allow(clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};
use tower::ServiceExt;

use super::UnifiedRuntime;
use super::identity_install_phase_tests::runtime;
use crate::access::{AccessControlConfig, AccessController};

const SIGNING_SECRET: &str = "console-test-signing";
/// An administrator of the fixture's access configuration.
const ADMIN: &str = "root@example.test";
/// A person who signs in and whom no access rule names.
const WITHOUT_RULES: &str = "nobody@example.test";
/// An id no mob of the fixture host has.
const ABSENT_MOB: &str = "door-access-rows-absent-mob";
/// The session a tool application request of these rows names: one value,
/// so that two requests of a row differ in the mob they name and in nothing
/// else.
const APP_SESSION: &str = "7f3c2a10-5b6d-4e8f-9a1b-2c3d4e5f6a7b";

/// The seven console routes that read: each takes the mob a request names
/// as a query parameter. `/console/modules` is the second path of the
/// handler of `/console/experience`.
const CONSOLE_READ_ROUTES: &[&str] = &[
    "GET /console/experience",
    "GET /console/modules",
    "GET /console/identities",
    "GET /console/timeline",
    "GET /console/timeline/stream",
    "GET /console/identity/worker-1/stream",
    "GET /console/gating/stream",
];

/// The reference application of `runtime` as a console that requires a
/// sign-in and has access rules: one administrator, no rule. It mounts
/// every door the rows ask, the WorkGraph fact stream among them: the
/// fixture's mob has a WorkGraph.
fn console_with_access(runtime: &mut UnifiedRuntime) -> axum::Router {
    let controller = AccessController::new(AccessControlConfig {
        enabled: true,
        admins: vec![ADMIN.to_string()],
        ..AccessControlConfig::default()
    })
    .expect("access configuration");
    runtime.set_access_controller(controller);
    let decisions = crate::console_auth_config::parse_console_auth_config(&json!({
        "shared_secret": SIGNING_SECRET,
        "email_allowlist": [ADMIN, WITHOUT_RULES],
    }))
    .expect("console sign-in configuration");
    runtime.build_reference_app_router(decisions)
}

/// A sign-in token of `email`, signed with `secret`.
fn token(email: &str, secret: &str) -> String {
    jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
        &json!({
            "iss": "http://127.0.0.1/meta-gateway",
            "aud": "persistent-gateway",
            "sub": email,
            "email": email,
            "exp": chrono::Utc::now().timestamp() + 300,
        }),
        &jsonwebtoken::EncodingKey::from_secret(secret.as_bytes()),
    )
    .expect("token")
}

/// The doors of the fixture that read a mob id, each with a request that
/// names `mob_id` and nothing else it could act on.
fn doors(mob_id: &str) -> Vec<(&'static str, Request<axum::body::Body>)> {
    doors_naming(Some(mob_id))
}

/// A GET of `uri` with no body.
fn get(uri: String) -> Request<axum::body::Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .body(axum::body::Body::empty())
        .expect("GET request")
}

/// A POST of the JSON `body` on `uri`.
fn post(uri: &str, body: &Value) -> Request<axum::body::Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .expect("POST request")
}

/// The five console routes that take a JSON body, each with a body that
/// names no mob: a send to a member the fixture does not have and the
/// recovery of it, and the three tool application routes for such a member.
/// Each takes the mob a request names as the member `mob_id` of its body.
fn console_post_routes() -> Vec<(&'static str, &'static str, Value)> {
    let app = json!({
        "identity": "worker-1",
        "sessionId": APP_SESSION,
        "toolCallId": "door-access-rows-call",
    });
    let mut read = app.clone();
    read["uri"] = json!("ui://door-access-rows/app");
    let mut call = app.clone();
    call["name"] = json!("door-access-rows-tool");
    call["arguments"] = json!({});
    vec![
        (
            "POST /console/send",
            "/console/send",
            json!({
                "identity": "worker-1",
                "content": "hello",
                "origin": "console",
                "idempotency_key": "door-access-rows-send"
            }),
        ),
        (
            "POST /console/send/recover",
            "/console/send/recover",
            json!({
                "identity": "worker-1",
                "origin": "console",
                "idempotency_key": "door-access-rows-send"
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
    ]
}

/// The doors of the fixture that read a mob id, each with a request that
/// names the mob `mob_id`, or no mob, and nothing else it could act on: the
/// console's request door and its multipart route, the four event streams
/// (an agent's, the mob's, the structural events, the WorkGraph facts) and
/// the twelve console routes (seven that read, with the id as a query
/// parameter; five that take a body, with the id as a member of it).
fn doors_naming(mob_id: Option<&str>) -> Vec<(&'static str, Request<axum::body::Body>)> {
    let params = mob_id.map_or_else(|| json!({}), |mob_id| json!({"mob_id": mob_id}));
    let query = mob_id.map_or_else(String::new, |mob_id| format!("?mob_id={mob_id}"));
    let rpc =
        |method: &str| json!({"jsonrpc": "2.0", "id": "row", "method": method, "params": params});
    let multipart = format!(
        "--rows\r\nContent-Disposition: form-data; name=\"payload\"\r\n\r\n{}\r\n--rows--\r\n",
        rpc("meta/console/send")
    );
    let structural = super::mob_events::MOB_EVENTS_STREAM_PATH;
    let workgraph_facts = crate::http_sse::WORKGRAPH_FACTS_STREAM_PATH;
    let mut doors = vec![
        (
            "console RPC",
            post("/console/rpc", &rpc("meta/list_members")),
        ),
        (
            "console multipart",
            Request::builder()
                .method("POST")
                .uri("/console/rpc/multipart")
                .header(header::CONTENT_TYPE, "multipart/form-data; boundary=rows")
                .body(axum::body::Body::from(multipart))
                .expect("multipart request"),
        ),
        (
            "agent stream",
            get(format!("/agents/worker-1/events{query}")),
        ),
        ("mob stream", get(format!("/mob/events{query}"))),
        ("structural stream", get(format!("{structural}{query}"))),
        (
            "WorkGraph fact stream",
            get(format!("{workgraph_facts}{query}")),
        ),
    ];
    for route in CONSOLE_READ_ROUTES {
        let path = route.trim_start_matches("GET ");
        doors.push((*route, get(format!("{path}{query}"))));
    }
    for (route, uri, mut body) in console_post_routes() {
        if let Some(mob_id) = mob_id {
            body["mob_id"] = json!(mob_id);
        }
        doors.push((route, post(uri, &body)));
    }
    doors
}

/// The status and, when the answer is JSON, the body of `request`, sent
/// with `bearer` when one is given. The body of a stream is not read.
async fn answer(
    app: &axum::Router,
    mut request: Request<axum::body::Body>,
    bearer: Option<&str>,
) -> (StatusCode, String) {
    if let Some(bearer) = bearer {
        request.headers_mut().insert(
            header::AUTHORIZATION,
            format!("Bearer {bearer}").parse().expect("header value"),
        );
    }
    let response = tokio::time::timeout(Duration::from_secs(30), app.clone().oneshot(request))
        .await
        .expect("the door answers in time")
        .expect("the door answers");
    let status = response.status();
    let json = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/json"));
    if !json {
        return (status, String::new());
    }
    let body = axum::body::to_bytes(response.into_body(), 1 << 22)
        .await
        .expect("the answer's body");
    (status, String::from_utf8_lossy(&body).into_owned())
}

/// Whether `body` is the refusal of the mob id `requested`, as a door
/// gives it: a JSON-RPC error on the console's request doors, a plain
/// answer on a stream and on a console route.
fn refuses_the_mob_id(door: &str, status: StatusCode, body: &str, requested: &str) -> bool {
    let Ok(body) = serde_json::from_str::<Value>(body) else {
        return false;
    };
    if door.starts_with("console") {
        status == StatusCode::OK
            && body["error"]["code"] == json!(-32602)
            && body["error"]["data"]["kind"] == json!("foreign_mob_target")
            && body["error"]["data"]["requested_mob_id"] == json!(requested)
    } else {
        status == StatusCode::NOT_FOUND
            && body["error"] == json!("foreign_mob_target")
            && body["requested_mob_id"] == json!(requested)
    }
}

/// Whether two answers of `door` to one caller are one answer.
///
/// A console route that reads and serves its request answers a body that
/// carries the time of the read, and a send route answers with the send's
/// own record: of those the status is compared, and of a send route also
/// the kind of its answer (the member `error`). Every other answer is
/// compared whole.
fn one_answer(door: &str, first: &(StatusCode, String), second: &(StatusCode, String)) -> bool {
    if door.starts_with("GET /console/") && first.0 == StatusCode::OK {
        return first.0 == second.0;
    }
    if door.starts_with("POST /console/send") {
        let kind = |body: &str| {
            serde_json::from_str::<Value>(body)
                .ok()
                .map(|body| body["error"].clone())
        };
        return first.0 == second.0 && kind(&first.1) == kind(&second.1);
    }
    first == second
}

/// Whether `status` and `body` are what `door` answers, by its own rules,
/// a signed-in person no access rule names who asks for the host's own mob:
/// the event streams, the gating stream and the two send routes refuse the
/// access, the tool application routes answer that there is no such
/// application for the caller, and the two request doors and the routes
/// that read serve the request with what the caller may see. A door this
/// function does not name is answered `false`: a new door states its own
/// answer here.
fn the_doors_own_answer_for_a_person_no_rule_names(
    door: &str,
    status: StatusCode,
    body: &str,
) -> bool {
    match door {
        "agent stream"
        | "mob stream"
        | "structural stream"
        | "WorkGraph fact stream"
        | "GET /console/gating/stream"
        | "POST /console/send"
        | "POST /console/send/recover" => {
            status == StatusCode::FORBIDDEN && body.contains("access_denied")
        }
        "POST /console/mcp-apps/resolve"
        | "POST /console/mcp-apps/read-resource"
        | "POST /console/mcp-apps/call-tool" => {
            status == StatusCode::FORBIDDEN && body.contains("mcp_app_unavailable")
        }
        "console RPC"
        | "console multipart"
        | "GET /console/experience"
        | "GET /console/modules"
        | "GET /console/identities"
        | "GET /console/timeline"
        | "GET /console/timeline/stream"
        | "GET /console/identity/worker-1/stream" => status == StatusCode::OK,
        _ => false,
    }
}

/// The rows ask every door that reads a mob id: the two request doors, the
/// four event streams and the twelve console routes, each once.
#[test]
fn the_rows_ask_every_door_that_reads_a_mob_id() {
    let doors: Vec<&str> = doors_naming(None)
        .into_iter()
        .map(|(door, _)| door)
        .collect();
    let routes = doors
        .iter()
        .filter(|door| door.starts_with("GET /console/") || door.starts_with("POST /console/"))
        .count();
    let mut once = doors.clone();
    once.sort_unstable();
    once.dedup();
    assert_eq!(routes, 12, "the twelve console routes: {doors:?}");
    assert_eq!(
        doors.len(),
        18,
        "two request doors, four streams: {doors:?}"
    );
    assert_eq!(once.len(), doors.len(), "each door once: {doors:?}");
}

/// No door reads the mob a request names before it knows the caller: a
/// request without a sign-in, or with one this console did not issue, is
/// answered as not signed in, whatever mob id it names, and the answer says
/// nothing of the id.
#[tokio::test]
async fn no_door_reads_a_mob_id_before_it_knows_the_caller() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut runtime = runtime(&temp).await;
    let default_mob = runtime.mob_id();
    let app = console_with_access(&mut runtime);
    let forged = token(ADMIN, "another-signing-secret");

    let mut wrong = Vec::new();
    for mob_id in [ABSENT_MOB, default_mob.as_str()] {
        for bearer in [None, Some(forged.as_str())] {
            for (door, request) in doors(mob_id) {
                let (status, body) = answer(&app, request, bearer).await;
                if status != StatusCode::UNAUTHORIZED
                    || body.contains("foreign_mob_target")
                    || body.contains(mob_id)
                {
                    wrong.push(format!(
                        "{door}, {} sign-in, mob id {mob_id}: {status} {body}",
                        if bearer.is_some() { "a forged" } else { "no" }
                    ));
                }
            }
        }
    }
    drop(app);
    runtime.shutdown().await;
    assert!(
        wrong.is_empty(),
        "a door answered a caller it does not know otherwise than as not signed in:\n{}",
        wrong.join("\n")
    );
}

/// A mob id a door does not serve to its caller is refused with one
/// answer. It names the id the request named and no other mob (the id of
/// the host's default mob is nowhere in it), and it is the same, byte for
/// byte, for an administrator and for a person no access rule names: it
/// does not tell why the id is not served.
#[tokio::test]
async fn a_refused_mob_id_names_no_mob_the_caller_did_not_name() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut runtime = runtime(&temp).await;
    let default_mob = runtime.mob_id();
    let app = console_with_access(&mut runtime);
    let admin = token(ADMIN, SIGNING_SECRET);
    let without_rules = token(WITHOUT_RULES, SIGNING_SECRET);

    let mut wrong = Vec::new();
    let mut for_the_admin = doors(ABSENT_MOB).into_iter();
    for (door, request) in doors(ABSENT_MOB) {
        let (_, as_admin) = for_the_admin.next().expect("the same doors");
        let (status, body) = answer(&app, request, Some(&without_rules)).await;
        let (admin_status, admin_body) = answer(&app, as_admin, Some(&admin)).await;
        if !refuses_the_mob_id(door, status, &body, ABSENT_MOB) {
            wrong.push(format!(
                "{door}: not the refusal of the id: {status} {body}"
            ));
        }
        if body.contains(default_mob.as_str()) || admin_body.contains(default_mob.as_str()) {
            wrong.push(format!(
                "{door}: the refusal names the host's default mob: {body} / {admin_body}"
            ));
        }
        if (status, &body) != (admin_status, &admin_body) {
            wrong.push(format!(
                "{door}: the refusal differs between callers: {status} {body} / {admin_status} \
                 {admin_body}"
            ));
        }
    }

    // A door mounted without the host's resolver knows only its own mob,
    // and refuses another id the same way, to an administrator too.
    let own_mob = meerkat_mob::MobId::from(default_mob.as_str());
    let administrator = AccessController::new(AccessControlConfig {
        enabled: true,
        admins: vec![ADMIN.to_string()],
        ..AccessControlConfig::default()
    })
    .expect("access configuration")
    .view_for_subject(Some(ADMIN));
    let refusal = crate::rpc::mob_addressing::refuse_unless_default(
        &crate::rpc::mob_addressing::NamedMob::from_query(Some(ABSENT_MOB.to_string())),
        Some(&own_mob),
        administrator,
        "row",
    )
    .expect_err("a door without a resolver refuses another mob's id");
    let refusal = format!("{} {:?}", refusal.message, refusal.data);
    if refusal.contains(default_mob.as_str()) || !refusal.contains(ABSENT_MOB) {
        wrong.push(format!(
            "a door without a resolver: the refusal names the door's own mob: {refusal}"
        ));
    }

    drop(app);
    runtime.shutdown().await;
    assert!(
        wrong.is_empty(),
        "a refused mob id is not one answer that names the requested id only:\n{}",
        wrong.join("\n")
    );
}

/// With one mob, the host's own mob named by its id is the same request as
/// no mob on each door that reads a mob id, for an administrator and for a
/// signed-in person no access rule names alike: the door's own rules answer
/// it, and the mob id's refusal is for another id only. What a door's own
/// rules answer the person no rule names is stated per door in
/// [`the_doors_own_answer_for_a_person_no_rule_names`].
#[tokio::test]
async fn the_hosts_own_mob_named_by_its_id_is_answered_by_the_doors_own_rules() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut runtime = runtime(&temp).await;
    let default_mob = runtime.mob_id();
    let app = console_with_access(&mut runtime);

    let mut wrong = Vec::new();
    for (who, email) in [
        ("an administrator", ADMIN),
        ("a person no rule names", WITHOUT_RULES),
    ] {
        let bearer = token(email, SIGNING_SECRET);
        let mut naming_no_mob = doors_naming(None).into_iter();
        for (door, by_id) in doors(default_mob.as_str()) {
            let (_, no_mob) = naming_no_mob.next().expect("the same doors");
            let by_id = answer(&app, by_id, Some(&bearer)).await;
            let no_mob = answer(&app, no_mob, Some(&bearer)).await;
            let (status, body) = &by_id;
            if !one_answer(door, &by_id, &no_mob) {
                wrong.push(format!(
                    "{who}, {door}: the mob's own id is answered {status} {body}, no mob {} {}",
                    no_mob.0, no_mob.1
                ));
            }
            if body.contains("foreign_mob_target") {
                wrong.push(format!(
                    "{who}, {door}: the host's own mob is refused by its id: {status} {body}"
                ));
            }
            if email == WITHOUT_RULES
                && !the_doors_own_answer_for_a_person_no_rule_names(door, *status, body)
            {
                wrong.push(format!(
                    "{who}, {door}: not the door's own answer: {status} {body}"
                ));
            }
        }
    }
    drop(app);
    runtime.shutdown().await;
    assert!(
        wrong.is_empty(),
        "the host's own mob by its id is not answered as no mob is:\n{}",
        wrong.join("\n")
    );
}

/// A signed-in person no access rule names who names no mob asks for the
/// host's own mob: each door answers by its own rules, as stated per door
/// in [`the_doors_own_answer_for_a_person_no_rule_names`], and none answers
/// the mob id's refusal. The console's experience names the host's own mob
/// by its id to that person as to every signed-in caller.
#[tokio::test]
async fn a_person_no_rule_names_who_names_no_mob_is_answered_as_ruled() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut runtime = runtime(&temp).await;
    let default_mob = runtime.mob_id();
    let app = console_with_access(&mut runtime);
    let bearer = token(WITHOUT_RULES, SIGNING_SECRET);

    let mut wrong = Vec::new();
    for (door, request) in doors_naming(None) {
        let (status, body) = answer(&app, request, Some(&bearer)).await;
        if body.contains("foreign_mob_target") {
            wrong.push(format!(
                "{door}: a request that names no mob is refused for its mob: {status} {body}"
            ));
        }
        if !the_doors_own_answer_for_a_person_no_rule_names(door, status, &body) {
            wrong.push(format!(
                "{door}: not the door's own answer: {status} {body}"
            ));
        }
        if door == "GET /console/experience" {
            let named = serde_json::from_str::<Value>(&body)
                .ok()
                .map(|body| body["topology"]["live_snapshot"]["mob_id"].clone());
            if named != Some(json!(default_mob.as_str())) {
                wrong.push(format!(
                    "{door}: the experience does not name the host's own mob: {named:?}"
                ));
            }
        }
    }
    drop(app);
    runtime.shutdown().await;
    assert!(
        wrong.is_empty(),
        "a person no rule names who names no mob is not answered by the doors' own rules:\n{}",
        wrong.join("\n")
    );
}

/// A person one access rule names: an allow that selects the second mob.
const OF_THE_SECOND_MOB: &str = "second@example.test";
/// A person two access rules name: an allow that selects the second mob,
/// and a deny of every action in it.
const ALLOWED_AND_DENIED: &str = "both@example.test";

/// The four event streams over `launch`'s mob, each from its own router
/// constructor, on a console that requires a sign-in and has access rules,
/// with a resolver that also serves `second`'s mob: one administrator, one
/// person whose allow selects the second mob, one person whose allow
/// selects it and whose deny takes every action in it away, and a person
/// no rule names. The router and the resolver it carries.
fn streams_of_two_mobs(
    launch: &UnifiedRuntime,
    second: &UnifiedRuntime,
) -> (axum::Router, super::MobResolver) {
    let second_mob = second.mob_id();
    let controller = AccessController::new(AccessControlConfig {
        enabled: true,
        admins: vec![ADMIN.to_string()],
        rules: vec![
            crate::access::AccessRule {
                id: "sees-the-second-mob".to_string(),
                subjects: vec![
                    OF_THE_SECOND_MOB.to_string(),
                    ALLOWED_AND_DENIED.to_string(),
                ],
                actions: vec![crate::access::ACTION_AGENT_VIEW.to_string()],
                mobs: vec![second_mob.clone()],
                ..crate::access::AccessRule::default()
            },
            crate::access::AccessRule {
                id: "nothing-in-the-second-mob".to_string(),
                effect: crate::access::AccessEffect::Deny,
                subjects: vec![ALLOWED_AND_DENIED.to_string()],
                actions: vec!["*".to_string()],
                mobs: vec![second_mob],
                ..crate::access::AccessRule::default()
            },
        ],
        ..AccessControlConfig::default()
    })
    .expect("access configuration");
    let decisions = crate::console_auth_config::parse_console_auth_config(&json!({
        "shared_secret": SIGNING_SECRET,
        "email_allowlist": [ADMIN, WITHOUT_RULES, OF_THE_SECOND_MOB, ALLOWED_AND_DENIED],
    }))
    .expect("console sign-in configuration");
    let resolver = launch
        .mob_resolver()
        .serving_a_second_mob_in_tests(&second.mob_resolver());
    let mob_runtime = launch.mob_runtime().clone();
    let router = crate::http_sse::agent_events_sse_router_with_access(
        Arc::new(
            |_agent_id: String| -> crate::http_sse::AgentEventSubscribeFuture {
                Box::pin(async {
                    Err(crate::MobRuntimeError::InvalidConfig(
                        "the fixture opens no agent stream".to_string(),
                    ))
                })
            },
        ),
        Some(decisions.clone()),
        Some(controller.clone()),
    )
    .merge(crate::http_sse::mob_events_sse_router_with_access(
        Arc::new(move || {
            let mob_runtime = mob_runtime.clone();
            Box::pin(async move { mob_runtime.handle().subscribe_mob_events().await })
        }),
        Some(decisions.clone()),
        Some(controller.clone()),
    ))
    .merge(
        crate::http_sse::mob_structural_events_sse_router_with_access(
            launch.mob_handle(),
            launch.mob_events_store(),
            Some(decisions.clone()),
            Some(controller.clone()),
        ),
    )
    .merge(crate::http_sse::workgraph_facts_sse_router(
        crate::workgraph_events::WorkGraphFactHub::new(),
        Some(decisions),
        Some(controller),
    ))
    .layer(axum::Extension(resolver.clone()));
    (router, resolver)
}

/// The four event streams, each with a request that names the mob
/// `mob_id` and nothing else.
fn streams_naming(mob_id: &str) -> Vec<(&'static str, Request<axum::body::Body>)> {
    let structural = super::mob_events::MOB_EVENTS_STREAM_PATH;
    let workgraph_facts = crate::http_sse::WORKGRAPH_FACTS_STREAM_PATH;
    vec![
        (
            "agent stream",
            get(format!("/agents/worker-1/events?mob_id={mob_id}")),
        ),
        ("mob stream", get(format!("/mob/events?mob_id={mob_id}"))),
        (
            "structural stream",
            get(format!("{structural}?mob_id={mob_id}")),
        ),
        (
            "WorkGraph fact stream",
            get(format!("{workgraph_facts}?mob_id={mob_id}")),
        ),
    ]
}

/// What a stream route answered: its status, its headers (sorted, without
/// the time of the answer) and, when the answer is JSON, its body. The
/// body of a stream is not read.
type StreamAnswer = (StatusCode, Vec<(String, String)>, String);

/// The answer of `request` on `app`, sent with a sign-in of `email`.
async fn stream_answer_as(
    app: &axum::Router,
    mut request: Request<axum::body::Body>,
    email: &str,
) -> StreamAnswer {
    request.headers_mut().insert(
        header::AUTHORIZATION,
        format!("Bearer {}", token(email, SIGNING_SECRET))
            .parse()
            .expect("header value"),
    );
    let response = tokio::time::timeout(Duration::from_secs(30), app.clone().oneshot(request))
        .await
        .expect("the stream route answers in time")
        .expect("the stream route answers");
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
    let body = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("the answer's body");
    (status, headers, String::from_utf8_lossy(&body).into_owned())
}

/// The caller a stream route goes on with for a mob other than the launch
/// mob sees that mob: its audience and its access view read a name as that
/// mob's agent.
#[test]
fn a_streams_caller_is_handed_back_seeing_the_mob_of_its_request() {
    use crate::rpc::mob_addressing::DoorCaller;
    let second = meerkat_mob::MobId::from("second");
    let view = AccessController::new(AccessControlConfig {
        enabled: true,
        admins: vec![ADMIN.to_string()],
        rules: vec![crate::access::AccessRule {
            id: "sees-second".to_string(),
            subjects: vec![OF_THE_SECOND_MOB.to_string()],
            actions: vec![crate::access::ACTION_AGENT_VIEW.to_string()],
            mobs: vec!["second".to_string()],
            ..crate::access::AccessRule::default()
        }],
        ..AccessControlConfig::default()
    })
    .expect("access configuration")
    .view_for_subject(Some(OF_THE_SECOND_MOB));
    let caller = crate::http_sse::SseCaller {
        access_view: Some(view.clone()),
        audience: view,
        authentication: None,
    };
    let sees = |caller: &crate::http_sse::SseCaller| {
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
    assert_eq!(sees(&caller.seeing_mob(&second)), [true; 2]);
}

/// A stream of a mob other than the launch mob that the caller may not
/// address is answered, on each of the four event streams, exactly as a
/// stream of an id no mob has: the same status, the same headers and the
/// same body apart from the id the request named, and the answer names no
/// other mob. Nothing is asked of the resolver about that mob for that
/// caller.
#[tokio::test]
async fn a_stream_of_a_mob_the_caller_may_not_address_is_refused_like_one_of_a_mob_the_host_does_not_have()
 {
    let temp = tempfile::tempdir().expect("tempdir");
    let second_temp = tempfile::tempdir().expect("tempdir");
    let launch = runtime(&temp).await;
    let second = runtime(&second_temp).await;
    let launch_mob = launch.mob_id();
    let second_mob = second.mob_id();
    // An id no mob has, of the second mob's length: that id with its last
    // character changed.
    let mut absent_mob = second_mob.clone();
    let last = absent_mob.pop().expect("a mob id is not empty");
    absent_mob.push(if last == '0' { '1' } else { '0' });
    let (app, resolver) = streams_of_two_mobs(&launch, &second);

    let mut wrong = Vec::new();
    let mut for_the_absent_mob = streams_naming(&absent_mob).into_iter();
    for (door, for_the_second_mob) in streams_naming(&second_mob) {
        let (_, for_the_absent_mob) = for_the_absent_mob.next().expect("the same streams");
        let second_answer = stream_answer_as(&app, for_the_second_mob, WITHOUT_RULES).await;
        let absent_answer = stream_answer_as(&app, for_the_absent_mob, WITHOUT_RULES).await;
        if !refuses_the_mob_id(door, absent_answer.0, &absent_answer.2, &absent_mob) {
            wrong.push(format!(
                "{door}: an id no mob has is not answered the mob id's refusal: {absent_answer:?}"
            ));
        }
        let apart_from_the_id = |answer: &StreamAnswer, id: &str| {
            (
                answer.0,
                answer.1.clone(),
                answer.2.replace(id, "<the id the request named>"),
            )
        };
        if apart_from_the_id(&second_answer, &second_mob)
            != apart_from_the_id(&absent_answer, &absent_mob)
        {
            wrong.push(format!(
                "{door}: is answered differently for the second mob and for an id no mob has: \
                 {second_answer:?} / {absent_answer:?}"
            ));
        }
        if second_answer.2.contains(&launch_mob) || absent_answer.2.contains(&launch_mob) {
            wrong.push(format!(
                "{door}: the answer names a mob the request did not name: {second_answer:?} / \
                 {absent_answer:?}"
            ));
        }
    }
    let asked_about_the_second_mob = resolver
        .resolved_for_in_tests()
        .into_iter()
        .chain(resolver.asked_whether_served_in_tests())
        .filter(|named| {
            named
                .as_ref()
                .is_some_and(|named| named.as_str() == second_mob)
        })
        .count();
    if asked_about_the_second_mob != 0 {
        wrong.push(format!(
            "the resolver was asked about a mob the caller may not address: \
             {asked_about_the_second_mob} times"
        ));
    }

    drop(app);
    launch.shutdown().await;
    second.shutdown().await;
    assert!(
        wrong.is_empty(),
        "a stream of a mob the caller may not address is not answered as one of an id no mob \
         has:\n{}",
        wrong.join("\n")
    );
}

/// An event stream serves the launch mob only until it is built over the
/// mob a request names: a stream is read from the launch mob's handles, so
/// a request for a second served mob is answered the mob id's refusal,
/// whoever asks. The person whose rule selects that mob, the person whose
/// deny takes everything in it away and an administrator are refused like
/// every other caller, and the resolver is asked nothing about that mob.
#[tokio::test]
async fn no_stream_serves_a_second_mob_before_it_is_built_over_that_mob() {
    let temp = tempfile::tempdir().expect("tempdir");
    let second_temp = tempfile::tempdir().expect("tempdir");
    let launch = runtime(&temp).await;
    let second = runtime(&second_temp).await;
    let second_mob = second.mob_id();
    let (app, resolver) = streams_of_two_mobs(&launch, &second);

    let mut wrong = Vec::new();
    for (who, email) in [
        ("the person whose rule selects the mob", OF_THE_SECOND_MOB),
        ("an administrator", ADMIN),
        (
            "the person whose allow selects the mob and whose deny takes all in it away",
            ALLOWED_AND_DENIED,
        ),
    ] {
        for (door, request) in streams_naming(&second_mob) {
            let answer = stream_answer_as(&app, request, email).await;
            if !refuses_the_mob_id(door, answer.0, &answer.2, &second_mob) {
                wrong.push(format!(
                    "{who}, {door}: is answered for a second mob: {answer:?}"
                ));
            }
        }
    }
    let asked_about_the_second_mob = resolver
        .resolved_for_in_tests()
        .into_iter()
        .chain(resolver.asked_whether_served_in_tests())
        .filter(|named| {
            named
                .as_ref()
                .is_some_and(|named| named.as_str() == second_mob)
        })
        .count();
    if asked_about_the_second_mob != 0 {
        wrong.push(format!(
            "the resolver was asked about a mob no stream serves: \
             {asked_about_the_second_mob} times"
        ));
    }

    drop(app);
    launch.shutdown().await;
    second.shutdown().await;
    assert!(
        wrong.is_empty(),
        "a stream that is not built over the mob a request names does not refuse that mob:\n{}",
        wrong.join("\n")
    );
}

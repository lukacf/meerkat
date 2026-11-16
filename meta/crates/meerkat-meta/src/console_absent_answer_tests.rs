//! A request that names a member hidden from its caller gets the answer a
//! name no member holds gets, on the doors that record something for the
//! member they name (a remembered fact, a console schedule) and on the REST
//! send routes: for a caller the access rules refuse, for a caller they
//! allow, and on a console with no access rules. The answer for a name no
//! member holds is pinned as it stands, and the member's owner is answered
//! for the member itself.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use serde_json::{Value, json};

use super::console_human_admission_tests::{BOB, authenticate};
use super::console_private_audience_tests::fully_composed_console;
use super::console_private_side_chat_tests::{
    ALICE, ROOT, TestResult, alice_claims, bearer, bob_claims, enforced_console, fork_on_console,
    runtime, send_on_console,
};
use super::console_side_chat_tests::seeded_member;
use super::console_token_lapse_tests::console_rpc_as;
use super::*;

/// A member every caller may see.
const SHARED: &str = "alpha";
/// A side chat private to Alice: hidden from Bob.
const HIDDEN: &str = "alpha-side";
/// A name no member holds.
const ABSENT: &str = "alpha-gone";

const NO_RULES: &str = "no access rules";
const ALLOWED: &str = "rules that allow the caller";
const REFUSED: &str = "rules that refuse the caller";

/// `answer` with `name` replaced by a placeholder, so the answers given two
/// names compare as answers: the code, the words and every field.
fn masked(answer: &str, name: &str) -> String {
    answer.replace(name, "<member>")
}

/// Access rules under which `subject` may do everything these doors ask
/// for to every agent, and nobody else anything, with root their one admin.
fn rules_for(subject: &str) -> AccessController {
    AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![ROOT.to_string()],
        rules: vec![AccessRule {
            id: "drives-every-agent".to_string(),
            subjects: vec![subject.to_string()],
            actions: vec!["agent.*".to_string(), "schedule.manage".to_string()],
            agents: vec!["*".to_string()],
            ..AccessRule::default()
        }],
        ..crate::access::AccessControlConfig::default()
    })
    .expect("access controller")
}

/// The rules' refusal of `action` on the name no member holds.
fn refused_by_the_rules(action: &str) -> Value {
    json!({
        "code": -32030,
        "message": format!("access denied: {action}"),
        "data": { "action": action, "kind": "access_denied", "resource": ABSENT },
    })
}

/// A console whose runtime holds `SHARED` and `HIDDEN`, with every part the
/// doors read (agent memory, console schedules, an aggregator), signed in
/// for Alice and Bob.
struct PrivateSideChat {
    _runtime_dir: tempfile::TempDir,
    _memory_dir: tempfile::TempDir,
    runtime: MobRuntime,
    state: ConsoleJsonState,
}

impl PrivateSideChat {
    async fn new(name: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let (runtime_dir, runtime) = runtime(name).await?;
        seeded_member(&runtime, "worker", SHARED, &[]).await?;
        runtime.console_schedules().attach(
            meerkat::ScheduleService::new(Arc::new(meerkat::MemoryScheduleStore::default())),
            crate::schedule_wiring::ScheduleFiringHostBinding::external_driver(),
        );
        let forking = console_json_router_with_state(enforced_console(&runtime, None));
        let forked = fork_on_console(&forking, Some(&bearer(alice_claims())), SHARED, HIDDEN).await;
        assert!(forked["error"].is_null(), "{forked:#}");
        let memory_dir = tempfile::tempdir()?;
        let state = fully_composed_console(&runtime, HIDDEN, memory_dir.path()).await?;
        Ok(Self {
            _runtime_dir: runtime_dir,
            _memory_dir: memory_dir,
            runtime,
            state,
        })
    }

    /// The console under each access posture its callers can meet: no
    /// access rules, rules that allow everyone, and rules that allow only
    /// the member's owner.
    fn postures(&self) -> Vec<(&'static str, ConsoleJsonState)> {
        let under = |access: Option<AccessController>| {
            let mut state = self.state.clone();
            state.access = access;
            state
        };
        vec![
            (NO_RULES, under(None)),
            (ALLOWED, under(Some(rules_for("*")))),
            (REFUSED, under(Some(rules_for(ALICE)))),
        ]
    }
}

/// Where `method` departs, under each posture, from: Bob is answered for a
/// name no member holds with `pinned(posture)`, the error as it stands,
/// whatever value another spelling of a member carries (the method reads
/// `identity` alone); for the hidden member with the same answer; and the
/// member's owner without an error.
async fn rpc_differences(
    console: &PrivateSideChat,
    method: &str,
    params: impl Fn(&str, &str) -> Value,
    pinned: impl Fn(&str) -> Value,
) -> Vec<String> {
    let (alice, bob) = (bearer(alice_claims()), bearer(bob_claims()));
    let mut differences = Vec::new();
    for (posture, state) in console.postures() {
        let app = console_json_router_with_state(state);
        let hidden = console_rpc_as(&app, Some(&bob), method, params(HIDDEN, posture)).await;
        let absent = console_rpc_as(&app, Some(&bob), method, params(ABSENT, posture)).await;
        let (hidden, absent) = (&hidden["error"], &absent["error"]);
        if *absent != pinned(posture) {
            differences.push(format!(
                "{method}, {posture}: a name no member holds is answered {absent}, pinned {}",
                pinned(posture)
            ));
        }
        for spelling in ["member_id", "agent_id", "agent_identity"] {
            let mut request = params(ABSENT, posture);
            request[spelling] = json!(7);
            let answer = console_rpc_as(&app, Some(&bob), method, request).await;
            if answer["error"] != pinned(posture) {
                differences.push(format!(
                    "{method}, {posture}: a name no member holds, with a number under \
                     {spelling}, is answered {}, pinned {}",
                    answer["error"],
                    pinned(posture)
                ));
            }
        }
        if masked(&hidden.to_string(), HIDDEN) != masked(&absent.to_string(), ABSENT) {
            differences.push(format!(
                "{method}, {posture}: the hidden member is answered {hidden}, a name no \
                 member holds {absent}"
            ));
        }
        let owners = console_rpc_as(&app, Some(&alice), method, params(HIDDEN, posture)).await;
        if !owners["error"].is_null() {
            differences.push(format!(
                "{method}, {posture}: the member's owner is answered {owners}"
            ));
        }
    }
    differences
}

/// A fact remembered for a member hidden from the caller is answered as one
/// for a name no member holds; the member's owner remembers it.
#[tokio::test]
async fn a_fact_for_a_hidden_member_is_answered_as_one_for_an_absent_member() -> TestResult {
    let console = PrivateSideChat::new("absent-answer-remembered-fact").await?;
    let differences = rpc_differences(
        &console,
        "meta/agent_memory/remember",
        |name, posture| {
            json!({
                "identity": name,
                "title": format!("The heron, {posture}"),
                "body": "The heron is at the north pond.",
                "tags": ["heron"],
            })
        },
        |posture| match posture {
            REFUSED => refused_by_the_rules("agent.memory.write"),
            _ => json!({
                "code": -32602,
                "message": format!(
                    "agent memory write failed: invalid agent memory config: unknown \
                     identity: {ABSENT}"
                ),
            }),
        },
    )
    .await;
    assert!(differences.is_empty(), "{differences:#?}");
    Ok(())
}

/// A console schedule defined for a member hidden from the caller is
/// answered as one for a name no member holds, the error's data included;
/// the member's owner defines it.
#[tokio::test]
async fn a_schedule_for_a_hidden_member_is_answered_as_one_for_an_absent_member() -> TestResult {
    let console = PrivateSideChat::new("absent-answer-console-schedule").await?;
    let start = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
    let differences = rpc_differences(
        &console,
        "meta/schedule/create",
        |name, posture| {
            json!({
                "identity": name,
                "content": "Post the morning summary.",
                "trigger": {
                    "type": "interval",
                    "start_at_utc": start,
                    "every_seconds": 3600,
                },
                "name": format!("Morning summary, {posture}"),
            })
        },
        |posture| match posture {
            REFUSED => refused_by_the_rules("agent.send"),
            _ => json!({
                "code": -32001,
                "message": format!("unknown identity: {ABSENT}"),
                "data": { "kind": "unknown_identity" },
            }),
        },
    )
    .await;
    assert!(differences.is_empty(), "{differences:#?}");
    Ok(())
}

/// `POST /console/send/recover` for `identity` as the holder of `bearer`,
/// with a key no send used.
async fn recover_on_console(app: &Router, bearer: &str, identity: &str) -> (StatusCode, Value) {
    use tower::ServiceExt;
    let response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/console/send/recover")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
                .body(axum::body::Body::from(
                    json!({
                        "identity": identity,
                        "origin": "console",
                        "idempotency_key": "a-key-no-send-used",
                    })
                    .to_string(),
                ))
                .expect("recover request"),
        )
        .await
        .expect("recover response");
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("recover body");
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

/// The viewer the host's send precheck is asked about: the holder of
/// `claims` on the console of `state`.
fn viewer(
    state: &ConsoleJsonState,
    claims: Value,
) -> Result<ConsoleViewerContext, Box<dyn std::error::Error + Send + Sync>> {
    let caller = authenticate(state, claims).ok_or("the caller authenticates")?;
    Ok(ConsoleViewerContext {
        principal: caller.principal,
        service_identity: caller.service_identity,
        access_view: caller.access_view,
        human: caller.human,
    })
}

/// The REST send, its recovery and the host's send precheck answer a member
/// hidden from the caller as a name no member holds; the member's owner
/// passes the precheck and is answered by the recovery itself.
#[tokio::test]
async fn the_rest_send_routes_answer_a_hidden_member_as_an_absent_one() -> TestResult {
    let console = PrivateSideChat::new("absent-answer-rest-send").await?;
    let (alice, bob) = (bearer(alice_claims()), bearer(bob_claims()));
    let refused = format!(
        "{} {}",
        StatusCode::FORBIDDEN,
        json!({
            "error": "access_denied",
            "message": "you are not allowed to send to this agent",
        })
    );
    let mut differences = Vec::new();
    for (posture, state) in console.postures() {
        let app = console_json_router_with_state(state.clone());
        // Each door's answers for the hidden member and for a name no member
        // holds, and that door's answer for the latter as it stands.
        let mut compare = |door: &str, hidden: String, absent: String, pinned: String| {
            if absent != pinned {
                differences.push(format!(
                    "{door}, {posture}: a name no member holds is answered {absent}, pinned \
                     {pinned}"
                ));
            }
            if masked(&hidden, HIDDEN) != masked(&absent, ABSENT) {
                differences.push(format!(
                    "{door}, {posture}: the hidden member is answered {hidden}, a name no \
                     member holds {absent}"
                ));
            }
        };

        let hidden = send_on_console(&app, &bob, HIDDEN).await;
        let absent = send_on_console(&app, &bob, ABSENT).await;
        compare(
            "POST /console/send",
            format!("{} {}", hidden.0, hidden.1),
            format!("{} {}", absent.0, absent.1),
            match posture {
                REFUSED => refused.clone(),
                _ => format!(
                    "{} {}",
                    StatusCode::NOT_FOUND,
                    json!({
                        "error": "unknown_identity",
                        "message": format!("unknown identity: {ABSENT}"),
                    })
                ),
            },
        );

        let hidden = recover_on_console(&app, &bob, HIDDEN).await;
        let absent = recover_on_console(&app, &bob, ABSENT).await;
        compare(
            "POST /console/send/recover",
            format!("{} {}", hidden.0, hidden.1),
            format!("{} {}", absent.0, absent.1),
            match posture {
                REFUSED => refused.clone(),
                _ => format!(
                    "{} {}",
                    StatusCode::NOT_FOUND,
                    json!({
                        "error": "send_not_found",
                        "message": "no console send of yours has this idempotency key",
                    })
                ),
            },
        );

        let precheck = ConsoleSendPrecheck::new(
            state.decisions.clone(),
            Some(console.runtime.clone()),
            state.access.clone(),
        )
        .with_identity_runtime(
            state
                .identity_runtime
                .as_ref()
                .map(|identity_runtime| Arc::clone(identity_runtime.runtime())),
        );
        let bobs = viewer(&state, bob_claims())?;
        let hidden = precheck.check(&bobs, HIDDEN).await;
        let absent = precheck.check(&bobs, ABSENT).await;
        compare(
            "the host's send precheck",
            format!("{hidden:?}"),
            format!("{absent:?}"),
            match posture {
                REFUSED => "Err(AccessDenied)".to_string(),
                _ => format!("Err(UnknownIdentity(\"unknown identity: {ABSENT}\"))"),
            },
        );

        let owners = precheck
            .check(&viewer(&state, alice_claims())?, HIDDEN)
            .await;
        if owners.is_err() {
            differences.push(format!(
                "the host's send precheck, {posture}: the member's owner is answered {owners:?}"
            ));
        }
        let (status, owners) = recover_on_console(&app, &alice, HIDDEN).await;
        if (status, &owners["error"]) != (StatusCode::NOT_FOUND, &json!("send_not_found")) {
            differences.push(format!(
                "POST /console/send/recover, {posture}: the member's owner is answered \
                 {status} {owners}"
            ));
        }
    }
    assert!(differences.is_empty(), "{differences:#?}");
    Ok(())
}

/// A whole request to remember a fact for `name`.
fn fact_for(name: &str) -> Value {
    json!({
        "identity": name,
        "title": "The heron",
        "body": "The heron is at the north pond.",
        "tags": ["heron"],
    })
}

/// A whole request to define a console schedule for `name`.
fn schedule_for(name: &str) -> Value {
    json!({
        "identity": name,
        "content": "Post the morning summary.",
        "trigger": {
            "type": "interval",
            "start_at_utc": (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
            "every_seconds": 3600,
        },
        "name": "Morning summary",
    })
}

/// `request` without its parameter `key`.
fn without(mut request: Value, key: &str) -> Value {
    if let Some(fields) = request.as_object_mut() {
        fields.remove(key);
    }
    request
}

/// `request` with `value` under `key`.
fn with(mut request: Value, key: &str, value: Value) -> Value {
    request[key] = value;
    request
}

/// One request of a method as a row sends it: whole, or short of the
/// parameter `short` (the one its handler asks for after the member), with
/// the error a name no member holds is answered, as it stands.
struct RequestForm {
    method: &'static str,
    form: &'static str,
    whole: fn(&str) -> Value,
    short: Option<&'static str>,
    pinned: Value,
}

/// The forms of both methods' requests under `posture`.
fn fact_and_schedule_requests(posture: &str) -> Vec<RequestForm> {
    let refused = posture == REFUSED;
    vec![
        RequestForm {
            method: "meta/agent_memory/remember",
            form: "whole",
            whole: fact_for,
            short: None,
            pinned: if refused {
                refused_by_the_rules("agent.memory.write")
            } else {
                json!({
                    "code": -32602,
                    "message": format!(
                        "agent memory write failed: invalid agent memory config: unknown \
                         identity: {ABSENT}"
                    ),
                })
            },
        },
        RequestForm {
            method: "meta/agent_memory/remember",
            form: "without its body",
            whole: fact_for,
            short: Some("body"),
            pinned: if refused {
                refused_by_the_rules("agent.memory.write")
            } else {
                json!({
                    "code": -32602,
                    "message": "Invalid params: body must be a non-empty string",
                })
            },
        },
        RequestForm {
            method: "meta/schedule/create",
            form: "whole",
            whole: schedule_for,
            short: None,
            pinned: if refused {
                refused_by_the_rules("agent.send")
            } else {
                json!({
                    "code": -32001,
                    "message": format!("unknown identity: {ABSENT}"),
                    "data": { "kind": "unknown_identity" },
                })
            },
        },
        RequestForm {
            method: "meta/schedule/create",
            form: "without its message",
            whole: schedule_for,
            short: Some("content"),
            pinned: if refused {
                refused_by_the_rules("agent.send")
            } else {
                json!({
                    "code": -32602,
                    "message": "content must be a non-empty string",
                })
            },
        },
    ]
}

/// Where Bob's answers for the hidden member and for a name no member
/// holds differ on `app`: `request` builds the request for a name, and
/// `pinned` is the error the name no member holds is answered, as it
/// stands.
async fn request_differences(
    app: &Router,
    method: &str,
    what: &str,
    request: impl Fn(&str) -> Value,
    pinned: &Value,
    differences: &mut Vec<String>,
) {
    let bob = bearer(bob_claims());
    let hidden = console_rpc_as(app, Some(&bob), method, request(HIDDEN)).await;
    let absent = console_rpc_as(app, Some(&bob), method, request(ABSENT)).await;
    let (hidden, absent) = (&hidden["error"], &absent["error"]);
    if absent != pinned {
        differences.push(format!(
            "{method}, {what}: a name no member holds is answered {absent}, pinned {pinned}"
        ));
    }
    if masked(&hidden.to_string(), HIDDEN) != masked(&absent.to_string(), ABSENT) {
        differences.push(format!(
            "{method}, {what}: the hidden member is answered {hidden}, a name no member holds \
             {absent}"
        ));
    }
}

/// A fact or a console schedule for a member hidden from the caller, with
/// the member named a second time under another spelling of a member, is
/// answered as the same request for a name no member holds, whole and short
/// of a parameter alike.
#[tokio::test]
async fn a_fact_or_a_schedule_naming_a_hidden_member_twice_is_answered_as_for_an_absent_one()
-> TestResult {
    let console = PrivateSideChat::new("absent-answer-named-twice").await?;
    let mut differences = Vec::new();
    for (posture, state) in console.postures() {
        let app = console_json_router_with_state(state);
        for request in fact_and_schedule_requests(posture) {
            for spelling in ["member_id", "agent_id", "agent_identity"] {
                let what = format!(
                    "{}, the name also under {spelling}, {posture}",
                    request.form
                );
                let twice = |name: &str| {
                    let twice = with((request.whole)(name), spelling, json!(name));
                    match request.short {
                        Some(key) => without(twice, key),
                        None => twice,
                    }
                };
                request_differences(
                    &app,
                    request.method,
                    &what,
                    twice,
                    &request.pinned,
                    &mut differences,
                )
                .await;
            }
        }
    }
    assert!(differences.is_empty(), "{differences:#?}");
    Ok(())
}

/// A request short of a parameter, and a schedule with a condition its
/// method does not take, are answered for a member hidden from the caller as
/// for a name no member holds: the refusals come in one order for both.
#[tokio::test]
async fn a_short_request_for_a_hidden_member_is_answered_as_for_an_absent_one() -> TestResult {
    let console = PrivateSideChat::new("absent-answer-short-request").await?;
    let mut differences = Vec::new();
    for (posture, state) in console.postures() {
        let app = console_json_router_with_state(state);
        for request in fact_and_schedule_requests(posture) {
            let Some(key) = request.short else { continue };
            let what = format!("{}, {posture}", request.form);
            let short = |name: &str| without((request.whole)(name), key);
            request_differences(
                &app,
                request.method,
                &what,
                short,
                &request.pinned,
                &mut differences,
            )
            .await;
        }
        let pinned = if posture == REFUSED {
            refused_by_the_rules("agent.send")
        } else {
            json!({
                "code": -32602,
                "message": "meta/schedule/create takes no expected_revision",
                "data": { "kind": "unsupported_param", "param": "expected_revision" },
            })
        };
        request_differences(
            &app,
            "meta/schedule/create",
            &format!("with an expected revision, {posture}"),
            |name| with(schedule_for(name), "expected_revision", json!(1)),
            &pinned,
            &mut differences,
        )
        .await;
    }
    assert!(differences.is_empty(), "{differences:#?}");
    Ok(())
}

/// `POST` of `body` to the REST route `route` as the holder of `bearer`.
async fn post_rest(app: &Router, bearer: &str, route: &str, body: Value) -> String {
    use tower::ServiceExt;
    let response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri(route)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
                .body(axum::body::Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("body");
    format!(
        "{status} {}",
        serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null)
    )
}

/// A whole REST send to `name`.
fn send_to(name: &str) -> Value {
    json!({
        "identity": name,
        "content": "hello",
        "origin": "console",
        "idempotency_key": format!("send-{name}"),
    })
}

/// Bob's answers for `name` on the three REST doors of the console of
/// `state`: the send, its recovery under a key no send used, and the host's
/// send precheck.
async fn rest_answers(
    console: &PrivateSideChat,
    state: &ConsoleJsonState,
    name: &str,
) -> Result<[(&'static str, String); 3], Box<dyn std::error::Error + Send + Sync>> {
    let app = console_json_router_with_state(state.clone());
    let bob = bearer(bob_claims());
    let sent = post_rest(&app, &bob, "/console/send", send_to(name)).await;
    let recovered = recover_on_console(&app, &bob, name).await;
    let precheck = ConsoleSendPrecheck::new(
        state.decisions.clone(),
        Some(console.runtime.clone()),
        state.access.clone(),
    )
    .with_identity_runtime(
        state
            .identity_runtime
            .as_ref()
            .map(|identity_runtime| Arc::clone(identity_runtime.runtime())),
    );
    let checked = precheck.check(&viewer(state, bob_claims())?, name).await;
    Ok([
        ("POST /console/send", sent),
        (
            "POST /console/send/recover",
            format!("{} {}", recovered.0, recovered.1),
        ),
        ("the host's send precheck", format!("{checked:?}")),
    ])
}

/// What each REST door answers a caller the rules allow for a name no
/// member holds, as it stands.
fn rest_answers_for_an_absent_name() -> [String; 3] {
    [
        format!(
            "{} {}",
            StatusCode::NOT_FOUND,
            json!({
                "error": "unknown_identity",
                "message": format!("unknown identity: {ABSENT}"),
            })
        ),
        format!(
            "{} {}",
            StatusCode::NOT_FOUND,
            json!({
                "error": "send_not_found",
                "message": "no console send of yours has this idempotency key",
            })
        ),
        format!("Err(UnknownIdentity(\"unknown identity: {ABSENT}\"))"),
    ]
}

/// Access rules that allow every caller everything these doors ask for on
/// exactly `names`, and nothing on any other agent.
fn rules_that_name(names: &[&str]) -> AccessController {
    AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![ROOT.to_string()],
        rules: vec![AccessRule {
            id: "drives-the-named-agents".to_string(),
            subjects: vec!["*".to_string()],
            actions: vec!["agent.*".to_string(), "schedule.manage".to_string()],
            agents: names.iter().map(|name| (*name).to_string()).collect(),
            ..AccessRule::default()
        }],
        ..crate::access::AccessControlConfig::default()
    })
    .expect("access controller")
}

/// A stated behaviour of the send gate, pinned as it is: the gate asks the
/// access rules about the name a hidden member is answered under, not about
/// the member's own name. Under a rule that allows exactly the member's
/// name, the REST send, its recovery and the host's precheck therefore
/// refuse a send to a member hidden from the caller by the rules, where the
/// same send to another name the rule names is answered as a send to a name
/// no member holds.
#[tokio::test]
async fn a_rule_naming_a_hidden_member_is_asked_about_another_name_on_the_send_gate() -> TestResult
{
    let console = PrivateSideChat::new("absent-answer-rules-by-name").await?;
    let mut state = console.state.clone();
    state.access = Some(rules_that_name(&[HIDDEN, ABSENT]));
    let hidden = rest_answers(&console, &state, HIDDEN).await?;
    let absent = rest_answers(&console, &state, ABSENT).await?;
    let refused = format!(
        "{} {}",
        StatusCode::FORBIDDEN,
        json!({
            "error": "access_denied",
            "message": "you are not allowed to send to this agent",
        })
    );
    let refusals = [refused.clone(), refused, "Err(AccessDenied)".to_string()];
    for ((((door, hidden), (_, absent)), refusal), pinned) in hidden
        .into_iter()
        .zip(absent)
        .zip(refusals)
        .zip(rest_answers_for_an_absent_name())
    {
        assert_eq!(
            hidden, refusal,
            "{door}: the hidden member, which the rule names"
        );
        assert_eq!(
            absent, pinned,
            "{door}: a name no member holds, which the rule names"
        );
    }
    Ok(())
}

/// A stated behaviour of the two RPC methods, pinned as it is: on these
/// methods the access rules are asked about the name a hidden member is
/// answered under, not about the member's own name. Under a rule that
/// allows exactly the member's name, a fact or a console schedule for a
/// member hidden from the caller is therefore refused by the rules, where
/// the same request for another name the rule names goes on to its handler.
#[tokio::test]
async fn a_rule_naming_a_hidden_member_is_asked_about_another_name_on_the_two_rpc_methods()
-> TestResult {
    let console = PrivateSideChat::new("absent-answer-rpc-rules-by-name").await?;
    let mut state = console.state.clone();
    state.access = Some(rules_that_name(&[HIDDEN, ABSENT]));
    let app = console_json_router_with_state(state);
    let bob = bearer(bob_claims());
    for request in fact_and_schedule_requests(ALLOWED) {
        if request.short.is_some() {
            continue;
        }
        let action = match request.method {
            "meta/agent_memory/remember" => "agent.memory.write",
            _ => "agent.send",
        };
        let hidden =
            console_rpc_as(&app, Some(&bob), request.method, (request.whole)(HIDDEN)).await;
        let absent =
            console_rpc_as(&app, Some(&bob), request.method, (request.whole)(ABSENT)).await;
        assert_eq!(
            hidden["error"],
            json!({
                "code": -32030,
                "message": format!("access denied: {action}"),
                "data": { "action": action, "kind": "access_denied", "resource": HIDDEN },
            }),
            "{}: the hidden member, which the rule names",
            request.method
        );
        assert_eq!(
            absent["error"], request.pinned,
            "{}: a name no member holds, which the rule names",
            request.method
        );
    }
    Ok(())
}

/// Access rules under which every caller may do everything these doors ask
/// for to every agent, and a deny by the role `worker` applies to every
/// caller.
fn rules_with_a_deny_by_role() -> AccessController {
    let actions = vec!["agent.*".to_string(), "schedule.manage".to_string()];
    AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![ROOT.to_string()],
        rules: vec![
            AccessRule {
                id: "drives-every-agent".to_string(),
                subjects: vec!["*".to_string()],
                actions: actions.clone(),
                agents: vec!["*".to_string()],
                ..AccessRule::default()
            },
            AccessRule {
                id: "keeps-out-of-workers".to_string(),
                effect: crate::access::AccessEffect::Deny,
                subjects: vec!["*".to_string()],
                actions,
                roles: vec!["worker".to_string()],
                ..AccessRule::default()
            },
        ],
        ..crate::access::AccessControlConfig::default()
    })
    .expect("access controller")
}

/// A caller the rules allow every agent, and to whom a deny by role
/// applies, is answered alike for a member hidden from them and for a name
/// no member holds, on every door of this file: a fact and a console
/// schedule, with the member named once and twice, the REST send, its
/// recovery and the host's precheck. What the Console answers such a caller
/// for a name its records do not know is not pinned here; the two answers
/// are compared with each other.
#[tokio::test]
async fn a_caller_under_a_deny_by_role_is_answered_alike_for_a_hidden_member_and_an_absent_one()
-> TestResult {
    let console = PrivateSideChat::new("absent-answer-deny-by-role").await?;
    let mut state = console.state.clone();
    state.access = Some(rules_with_a_deny_by_role());
    let app = console_json_router_with_state(state.clone());
    let bob = bearer(bob_claims());
    let mut differences = Vec::new();
    for request in fact_and_schedule_requests(ALLOWED) {
        if request.short.is_some() {
            continue;
        }
        for second in [
            None,
            Some("member_id"),
            Some("agent_id"),
            Some("agent_identity"),
        ] {
            let named = |name: &str| match second {
                Some(spelling) => with((request.whole)(name), spelling, json!(name)),
                None => (request.whole)(name),
            };
            let hidden = console_rpc_as(&app, Some(&bob), request.method, named(HIDDEN)).await;
            let absent = console_rpc_as(&app, Some(&bob), request.method, named(ABSENT)).await;
            let (hidden, absent) = (&hidden["error"], &absent["error"]);
            if masked(&hidden.to_string(), HIDDEN) != masked(&absent.to_string(), ABSENT) {
                differences.push(format!(
                    "{}, the name {}: the hidden member is answered {hidden}, a name no member \
                     holds {absent}",
                    request.method,
                    second.map_or("once".to_string(), |spelling| format!(
                        "also under {spelling}"
                    )),
                ));
            }
        }
    }
    let hidden = rest_answers(&console, &state, HIDDEN).await?;
    let absent = rest_answers(&console, &state, ABSENT).await?;
    for ((door, hidden), (_, absent)) in hidden.into_iter().zip(absent) {
        if masked(&hidden, HIDDEN) != masked(&absent, ABSENT) {
            differences.push(format!(
                "{door}: the hidden member is answered {hidden}, a name no member holds {absent}"
            ));
        }
    }
    assert!(differences.is_empty(), "{differences:#?}");
    Ok(())
}

/// A host that governs no agent itself; installed, it closes the send path
/// that carries no binding for every target.
struct InstalledHost;

impl crate::console_governed_ingress::ConsoleGovernedIngress for InstalledHost {
    fn governs(&self, _identity: &crate::identity_first::AgentIdentity) -> bool {
        false
    }

    fn binder_for(
        &self,
        _identity: &crate::identity_first::AgentIdentity,
        _human: &crate::runtime::AuthenticatedConsoleHuman,
    ) -> Result<Arc<dyn crate::identity_first::GovernedInputBinder>, String> {
        Err("this host binds no test send".to_string())
    }
}

/// `POST /console/send` answers a member hidden from the caller as a name no
/// member holds also where the send is refused for another reason first: a
/// send short of its idempotency key, and a send on a Console whose sends
/// are governed.
#[tokio::test]
async fn the_rest_send_answers_a_hidden_member_as_an_absent_one_at_its_other_refusals() -> TestResult
{
    let console = PrivateSideChat::new("absent-answer-rest-send-order").await?;
    let bob = bearer(bob_claims());
    let refused = format!(
        "{} {}",
        StatusCode::FORBIDDEN,
        json!({
            "error": "access_denied",
            "message": "you are not allowed to send to this agent",
        })
    );
    let mut differences = Vec::new();
    let mut compare = |what: String, hidden: String, absent: String, pinned: String| {
        if absent != pinned {
            differences.push(format!(
                "{what}: a name no member holds is answered {absent}, pinned {pinned}"
            ));
        }
        if masked(&hidden, HIDDEN) != masked(&absent, ABSENT) {
            differences.push(format!(
                "{what}: the hidden member is answered {hidden}, a name no member holds {absent}"
            ));
        }
    };
    for (posture, state) in console.postures() {
        let keyless = |name: &str| with(send_to(name), "idempotency_key", json!(""));
        let app = console_json_router_with_state(state.clone());
        compare(
            format!("a send with no idempotency key, {posture}"),
            post_rest(&app, &bob, "/console/send", keyless(HIDDEN)).await,
            post_rest(&app, &bob, "/console/send", keyless(ABSENT)).await,
            if posture == REFUSED {
                refused.clone()
            } else {
                format!(
                    "{} {}",
                    StatusCode::BAD_REQUEST,
                    json!({
                        "error": "invalid_request",
                        "message": "invalid request: idempotency_key must be non-empty",
                    })
                )
            },
        );

        let mut governed = state;
        governed.governed_ingress = Default::default();
        governed.governed_ingress.install(Arc::new(InstalledHost));
        let app = console_json_router_with_state(governed);
        compare(
            format!("a send on a Console whose sends are governed, {posture}"),
            post_rest(&app, &bob, "/console/send", send_to(HIDDEN)).await,
            post_rest(&app, &bob, "/console/send", send_to(ABSENT)).await,
            if posture == REFUSED {
                refused.clone()
            } else {
                format!(
                    "{} {}",
                    StatusCode::FORBIDDEN,
                    json!({
                        "error": "governed_send_refused",
                        "message": format!(
                            "governed send refused: Console sends are governed and {ABSENT} \
                             has no identity-first admission"
                        ),
                    })
                )
            },
        );
    }
    assert!(differences.is_empty(), "{differences:#?}");
    Ok(())
}

/// `POST /console/send/recover` for `identity` under `key` as the holder of
/// `bearer`.
async fn recover_under(app: &Router, bearer: &str, identity: &str, key: &str) -> String {
    post_rest(
        app,
        bearer,
        "/console/send/recover",
        json!({ "identity": identity, "origin": "console", "idempotency_key": key }),
    )
    .await
}

/// A recovery that names a member hidden from the caller finds no send, as
/// one that names a name no member holds, also under the key of a send the
/// caller made to the member while the Console showed it to them.
#[tokio::test]
async fn a_recovery_for_a_hidden_member_finds_no_send_under_a_key_the_caller_used() -> TestResult {
    let console = PrivateSideChat::new("absent-answer-recovery-own-send").await?;
    let bob = bearer(bob_claims());
    // Bob as an access administrator is shown every agent, and sends.
    let mut shown = console.state.clone();
    shown.access = Some(
        AccessController::new(crate::access::AccessControlConfig {
            enabled: true,
            admins: vec![BOB.to_string()],
            ..crate::access::AccessControlConfig::default()
        })
        .expect("access controller"),
    );
    let key = format!("send-{HIDDEN}");
    let sent = post_rest(
        &console_json_router_with_state(shown),
        &bob,
        "/console/send",
        send_to(HIDDEN),
    )
    .await;
    assert!(
        sent.starts_with(&StatusCode::OK.to_string()),
        "the Console accepts the administrator's send: {sent}"
    );

    // Without the grant the member is hidden from him again.
    let app = console_json_router_with_state(console.state.clone());
    let hidden = recover_under(&app, &bob, HIDDEN, &key).await;
    let absent = recover_under(&app, &bob, ABSENT, &key).await;
    assert_eq!(
        absent,
        rest_answers_for_an_absent_name()[1],
        "a name no member holds"
    );
    assert_eq!(masked(&hidden, HIDDEN), masked(&absent, ABSENT));
    Ok(())
}

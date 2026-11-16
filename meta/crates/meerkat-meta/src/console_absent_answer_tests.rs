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

use super::console_human_admission_tests::authenticate;
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
        principal: caller.principal.clone(),
        service_identity: caller.service_identity,
        access_view: caller.access_view.clone(),
        human: caller.human.clone(),
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

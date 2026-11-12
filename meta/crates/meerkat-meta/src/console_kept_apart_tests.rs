//! The callers a console keeps apart from other people. A console that can
//! identify a person keeps every caller who is not an administrator apart
//! from the others, and only a console that says it identifies nobody has
//! one operator who is not kept apart. Each route that takes a view for a
//! caller it did not identify states its console's own answer, and a view
//! taken with no console to ask is kept apart.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;

use serde_json::json;

use super::console_private_side_chat_tests::{
    ROOT, TestResult, alice_claims, bearer as person_bearer, enforced_console,
    everyone_drives_every_agent, fork_on_console,
};
use super::console_side_chat_tests::{persistent_gateway_runtime_in, seeded_member};
use super::console_token_lapse_tests::console_rpc_as;
use super::console_viewer_resolver_tests::{BOB, CAROL, bearer, decisions, rules_off, with_no_key};
use super::*;
use crate::access::{
    ACTION_AGENT_SEND, ACTION_GATING_VIEW, AccessControlConfig, AccessEffect, AccessRule,
    AgentResourceAttributes, CapturedLineageLink, PRIVATE_TO_LABEL,
};
use crate::auth::session_fence::ConsoleSessionFences;
use crate::console_aggregator::{
    AllowAllConsoleVisibilityPolicy, ConsoleFrameSource, ConsoleFrameSourceKind,
    ConsoleFrameStatus, ConsoleTimelineQuery, MetaConsoleAggregator, NewConsoleFrame,
    timeline_frame_visible_to,
};

/// A console where sign-in is optional, and whether it can identify a
/// person: it can with an access controller and a key a token verifies
/// against, and with nothing less.
struct OpenConsole {
    name: &'static str,
    decisions: RuntimeDecisionState,
    access: Option<AccessController>,
    identifies_people: bool,
}

/// The consoles where sign-in is optional. Carol is the one administrator of
/// each access controller, and its rules are off.
fn open_consoles() -> Vec<OpenConsole> {
    let open = decisions(false);
    let controller = || Some(AccessController::new(rules_off()).expect("access controller"));
    vec![
        OpenConsole {
            name: "an access controller and a key that verifies a token",
            decisions: open.clone(),
            access: controller(),
            identifies_people: true,
        },
        OpenConsole {
            name: "an access controller and no key",
            decisions: with_no_key(open.clone()),
            access: controller(),
            identifies_people: false,
        },
        OpenConsole {
            name: "a key and no access controller",
            decisions: open.clone(),
            access: None,
            identifies_people: false,
        },
        OpenConsole {
            name: "no key and no access controller",
            decisions: with_no_key(open),
            access: None,
            identifies_people: false,
        },
    ]
}

/// The audience a stream route takes for a request with `headers`.
fn stream_audience(
    decisions: Option<&RuntimeDecisionState>,
    access: Option<&AccessController>,
    headers: &HeaderMap,
) -> AccessView {
    let Ok(caller) = crate::http_sse::sse_access_context(
        decisions,
        &ConsoleSessionFences::detached(),
        access,
        headers,
        &Uri::from_static("/console/gating/stream"),
    ) else {
        panic!("the stream route admits the request");
    };
    caller.audience
}

/// The audience the console's own routes take for a request with `headers`.
fn request_audience(console: &OpenConsole, headers: &HeaderMap) -> AccessView {
    console_request_auth_context_for(
        &console.decisions,
        &ConsoleSessionFences::detached(),
        console.access.as_ref(),
        headers,
        &Uri::from_static("/console/timeline"),
    )
    .expect("the console admits the request")
    .audience
}

/// What the three rules for a kept-apart caller answer `view`: whether it is
/// kept apart, whether it reads a record with no captured member attributes,
/// and whether a member the console does not know is withheld from it.
fn kept_apart_rules(view: &AccessView) -> (bool, bool, bool) {
    (
        view.kept_apart_from_other_people(),
        view.can_view_uncaptured_member("ghost"),
        view.withholds_unknown_member("ghost"),
    )
}

/// Every route that takes a view for a caller it did not identify states
/// what its own console is: on a console that can identify a person the
/// caller who sent no token is kept apart like every person who is not an
/// administrator, and on a console that can identify nobody it is the one
/// operator. The console's routes, the event streams and the view a stream
/// takes again after a policy change all give the same answer, and a stream
/// route with no console configuration, which has nothing to state, keeps
/// its caller apart.
#[test]
fn every_route_tells_the_caller_it_did_not_identify_by_its_console() {
    let no_token = HeaderMap::new();
    for console in open_consoles() {
        let name = console.name;
        let request = request_audience(&console, &no_token);
        let stream = stream_audience(Some(&console.decisions), console.access.as_ref(), &no_token);
        let expected = if console.identifies_people {
            (true, false, true)
        } else {
            (false, true, false)
        };
        for (route, view) in [
            ("the console's routes", &request),
            ("the stream routes", &stream),
            ("a stream's view taken again", &stream.refreshed()),
        ] {
            assert!(view.subject().is_none(), "{name}, {route}");
            assert_eq!(
                kept_apart_rules(view),
                expected,
                "{name}: the caller with no token on {route} (kept apart, reads a record with \
                 no captured attributes, is withheld an unknown member)"
            );
        }
        if !console.identifies_people {
            continue;
        }
        // The people such a console identifies, on every route: a person is
        // kept apart, the administrator is not.
        for (route, person, admin) in [
            (
                "the console's routes",
                request_audience(&console, &bearer(BOB)),
                request_audience(&console, &bearer(CAROL)),
            ),
            (
                "the stream routes",
                stream_audience(
                    Some(&console.decisions),
                    console.access.as_ref(),
                    &bearer(BOB),
                ),
                stream_audience(
                    Some(&console.decisions),
                    console.access.as_ref(),
                    &bearer(CAROL),
                ),
            ),
        ] {
            assert_eq!(person.subject(), Some(BOB), "{name}, {route}");
            assert_eq!(
                kept_apart_rules(&person),
                (true, false, true),
                "{name}, {route}"
            );
            assert!(admin.is_admin(), "{name}, {route}");
            assert_eq!(
                kept_apart_rules(&admin),
                (false, true, false),
                "{name}, {route}"
            );
        }
    }

    // A stream route with no console configuration cannot say what its
    // console is: its caller is kept apart, with a controller or without.
    for access in [
        None,
        Some(AccessController::new(rules_off()).expect("access controller")),
    ] {
        let view = stream_audience(None, access.as_ref(), &no_token);
        assert_eq!(
            kept_apart_rules(&view),
            (true, false, true),
            "a stream route with no console configuration (controller: {})",
            access.is_some()
        );
    }
}

/// A route that takes no audience from the Console (an embedder's own REST
/// route for the experience) tells the caller it did not identify by the
/// same answer as the Console's routes, and identifies the person a
/// volunteered token names only on a console that can identify a person.
/// The console the stock console gateway serves when it is launched with no
/// auth configuration trusts no key, so it identifies nobody, with an
/// access controller too; one that requires sign-in identifies its people.
#[test]
fn a_route_with_no_console_audience_asks_the_same_console() {
    let fences = ConsoleSessionFences::detached();
    let experience = |query: &str| crate::runtime::ConsoleRestJsonRequest {
        method: "GET".to_string(),
        path: format!("/console/experience{query}"),
        auth: None,
    };
    for console in open_consoles() {
        let name = console.name;
        let view = crate::runtime::console_rest_route_view(
            &console.decisions,
            &fences,
            &experience(""),
            None,
            console.access.as_ref(),
        );
        assert!(view.subject().is_none(), "{name}");
        assert_eq!(
            kept_apart_rules(&view),
            if console.identifies_people {
                (true, false, true)
            } else {
                (false, true, false)
            },
            "{name}: the caller with no token on a route with no console audience"
        );
        // The same request with a person's token in its query.
        let token = bearer(BOB)
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .expect("a bearer token")
            .to_string();
        let volunteered = crate::runtime::console_rest_route_view(
            &console.decisions,
            &fences,
            &experience(&format!("?auth_token={token}")),
            None,
            console.access.as_ref(),
        );
        if console.identifies_people {
            assert_eq!(volunteered.subject(), Some(BOB), "{name}");
            assert_eq!(
                kept_apart_rules(&volunteered),
                (true, false, true),
                "{name}"
            );
        } else {
            assert!(
                volunteered.subject().is_none(),
                "{name}: a console that identifies nobody resolves no token"
            );
            assert_eq!(
                kept_apart_rules(&volunteered),
                (false, true, false),
                "{name}"
            );
        }
    }

    // The stock console gateway's console with no auth configuration.
    let local = RuntimeDecisionState::local_console(
        crate::ConsolePolicy {
            require_app_auth: false,
            ..crate::ConsolePolicy::default()
        },
        None,
    );
    let controller = AccessController::new(rules_off()).expect("access controller");
    for access in [None, Some(&controller)] {
        assert!(
            !crate::runtime::console_identifies_people(&local, access),
            "a local console identifies nobody (controller: {})",
            access.is_some()
        );
        let operator = console_request_auth_context_for(
            &local,
            &fences,
            access,
            &HeaderMap::new(),
            &Uri::from_static("/console/timeline"),
        )
        .expect("the console admits its operator")
        .audience;
        assert_eq!(kept_apart_rules(&operator), (false, true, false));
    }
    let mut signed_in = local;
    signed_in.console.require_app_auth = true;
    assert!(crate::runtime::console_identifies_people(&signed_in, None));
}

/// An access configuration that grants every caller every agent action and
/// `gating.view`, with a deny on agents labelled `tier = restricted` when
/// `with_deny`.
fn grants_every_caller(with_deny: bool) -> AccessControlConfig {
    let mut rules = vec![AccessRule {
        id: "every-caller".to_string(),
        actions: vec!["agent.*".to_string(), ACTION_GATING_VIEW.to_string()],
        agents: vec!["*".to_string()],
        ..AccessRule::default()
    }];
    if with_deny {
        rules.push(AccessRule {
            id: "deny-restricted".to_string(),
            effect: AccessEffect::Deny,
            actions: vec!["agent.*".to_string()],
            match_labels: BTreeMap::from([("tier".to_string(), "restricted".to_string())]),
            ..AccessRule::default()
        });
    }
    AccessControlConfig {
        enabled: true,
        admins: vec![CAROL.to_string()],
        rules,
        ..AccessControlConfig::default()
    }
}

fn member(identity: &str, labels: &[(&str, &str)]) -> AgentResourceAttributes {
    AgentResourceAttributes {
        identity: identity.to_string(),
        agent_id: None,
        role: Some("worker".to_string()),
        labels: labels
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect(),
    }
}

/// Only the three rules for a kept-apart caller read what a view says of its
/// console. Every other decision a view gives (whether rules are enforced,
/// an action, an action on an agent, whether an agent is hidden or known, a
/// record with captured member attributes, administration) is the same for a
/// caller with no token whether its console is said to identify people or
/// nobody. So the places that take a view with no console to ask (the stock
/// send gate for a host's viewer, a scheduled send, the access status and
/// administration reads, a topology plan, a held decision) decide exactly
/// what they decided before such a view was kept apart.
#[tokio::test]
async fn only_the_kept_apart_rules_read_what_a_view_says_of_its_console() -> TestResult {
    for (rules, config) in [
        ("rules off", rules_off()),
        ("every caller granted", grants_every_caller(false)),
        (
            "every caller granted, one deny by label",
            grants_every_caller(true),
        ),
    ] {
        let controller = AccessController::new(config).expect("access controller");
        controller.record_agent_attributes(member("alpha", &[]));
        controller.record_agent_attributes(member("vault", &[("tier", "restricted")]));
        controller.record_agent_attributes(member("side", &[(PRIVATE_TO_LABEL, "someone-else")]));
        let unstated = controller.view_for_caller(None, None);
        let identifying = controller.view_for_caller(None, None).on_console(true);
        let operator = controller.view_for_caller(None, None).on_console(false);

        let decisions_of = |view: &AccessView| {
            let mut answers = vec![
                ("enforced".to_string(), view.enforced()),
                ("gating.view".to_string(), view.allows(ACTION_GATING_VIEW)),
                (
                    "agent.send anywhere".to_string(),
                    view.may_perform_anywhere(ACTION_AGENT_SEND),
                ),
                ("can administer".to_string(), view.can_administer()),
                ("is admin".to_string(), view.is_admin()),
            ];
            for agent in ["alpha", "vault", "side", "ghost"] {
                answers.push((format!("view {agent}"), view.can_view_agent(agent)));
                answers.push((
                    format!("send {agent}"),
                    view.allows_agent(ACTION_AGENT_SEND, agent),
                ));
                answers.push((format!("hides {agent}"), view.hides_agent(agent)));
                answers.push((format!("knows {agent}"), view.knows_agent(agent)));
                let labels: &[(&str, &str)] = match agent {
                    "vault" => &[("tier", "restricted")],
                    "side" => &[(PRIVATE_TO_LABEL, "someone-else")],
                    _ => &[],
                };
                answers.push((
                    format!("captured record of {agent}"),
                    view.can_view_captured_member(&[CapturedLineageLink {
                        identity: agent.to_string(),
                        attributes: Some(member(agent, labels)),
                    }]),
                ));
            }
            answers
        };
        let answers = decisions_of(&unstated);
        assert_eq!(
            answers,
            decisions_of(&identifying),
            "{rules}: no other decision reads the console"
        );
        assert_eq!(
            answers,
            decisions_of(&operator),
            "{rules}: no other decision reads the console"
        );
        // Sanity of the fixture: the decisions above do tell agents apart.
        assert!(
            answers.contains(&("view alpha".to_string(), true))
                && answers.contains(&("hides side".to_string(), true))
                && answers.contains(&("view side".to_string(), false))
                && answers.contains(&("knows ghost".to_string(), false)),
            "{rules}: {answers:?}"
        );

        // The three rules that do read it.
        assert_eq!(kept_apart_rules(&unstated), (true, false, true), "{rules}");
        assert_eq!(
            kept_apart_rules(&identifying),
            (true, false, true),
            "{rules}"
        );
        assert_eq!(
            kept_apart_rules(&operator),
            (
                false,
                !operator.has_deny_undecidable_by_identity("agent.view", "ghost"),
                false
            ),
            "{rules}"
        );

        // The access status and administration reads take the caller's view
        // again with no console to ask: their answers hold none of the three.
        let status = access_status_result(Some(&controller), Some(&operator));
        assert_eq!(
            status,
            access_status_result(Some(&controller), Some(&identifying)),
            "{rules}"
        );
        assert_eq!(status["is_admin"], json!(false), "{rules}");
        assert_eq!(status["can_administer"], json!(false), "{rules}");
        assert!(controller.admin_config(None).is_err(), "{rules}");
        let held = controller.decision_fence(None, None);
        assert_eq!(held.enforced(), unstated.enforced(), "{rules}");
        assert_eq!(
            held.can_view_agent("alpha"),
            unstated.can_view_agent("alpha"),
            "{rules}"
        );
        assert!(!held.can_view_agent("side"), "{rules}");
    }

    // The stock send gate for a host's viewer the console did not identify,
    // on a console with no access controller: the gate takes the viewer's
    // audience with no console to ask, and a target the console does not
    // know is not withheld by it.
    let precheck = ConsoleSendPrecheck::new(decisions(false), None, None);
    let viewer = ConsoleViewerContext {
        principal: None,
        service_identity: false,
        access_view: None,
        human: None,
    };
    assert!(
        precheck.check(&viewer, "ghost").await.is_ok(),
        "the send gate decides a viewer with no stated console as before"
    );
    Ok(())
}

/// A stored frame under `identity` that holds `canary` and carries no
/// member witness.
fn no_witness_frame(identity: &str, canary: &str) -> NewConsoleFrame {
    NewConsoleFrame {
        id: None,
        dedupe_key: format!("kept-apart-row:{identity}:{canary}"),
        timestamp_ms: 2_000,
        runtime_key: "kept-apart".to_string(),
        identity: identity.to_string(),
        conversation_id: Some(identity.to_string()),
        session_id: None,
        kind: "user_input".to_string(),
        status: ConsoleFrameStatus::Completed,
        payload: json!({ "content": canary }),
        source: ConsoleFrameSource {
            member_provenance: None,
            kind: ConsoleFrameSourceKind::ConsoleEvent,
            source_cursor: None,
        },
        source_event_id: Some(format!("kept-apart-row:{identity}:{canary}")),
        interaction_id: None,
        turn_id: None,
        run_id: None,
        parent_frame_id: None,
        caused_by_frame_id: None,
    }
}

/// Retire `member` and wait until it has left the roster.
async fn leaves(runtime: &MobRuntime, member: &str) -> TestResult {
    let identity = AgentIdentity::from(member);
    runtime.handle().retire(identity.clone()).await?;
    for _ in 0..100 {
        if runtime.handle().get_member(&identity).await?.is_none() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(format!("{member} is still on the roster").into())
}

/// Whether the console knows `identity` for `view`, and whether `view` reads
/// a frame stored under it with no witness.
async fn knows_and_reads(view: &AccessView, identity: &str) -> (bool, bool) {
    let frame = MetaConsoleAggregator::in_memory()
        .store()
        .append_if_absent(no_witness_frame(identity, "no-witness-record"))
        .await
        .expect("the frame is stored")
        .frame;
    (
        view.knows_agent(identity),
        timeline_frame_visible_to(view, &frame),
    )
}

/// What a console with sign-in left behind: the state directory of its mob,
/// which holds the shared member `alpha` and held `alpha-side`, a side chat
/// private to the person who forked it, and `beta`, an ordinary member, with
/// the session each of the two ran on. Both ended before the console
/// stopped.
struct LeftBehind {
    state: tempfile::TempDir,
    side_chat_session: meerkat_core::types::SessionId,
    beta_session: meerkat_core::types::SessionId,
}

/// The session `member` runs on.
async fn session_of(
    runtime: &MobRuntime,
    member: &str,
) -> Result<meerkat_core::types::SessionId, Box<dyn std::error::Error + Send + Sync>> {
    runtime
        .handle()
        .get_member(&AgentIdentity::from(member))
        .await?
        .and_then(|entry| entry.bridge_session_id().cloned())
        .ok_or_else(|| format!("{member} has a session").into())
}

async fn state_after_a_console_with_sign_in(
    mob: &str,
) -> Result<LeftBehind, Box<dyn std::error::Error + Send + Sync>> {
    let state = tempfile::tempdir()?;
    let runtime = persistent_gateway_runtime_in(state.path(), mob, "", "").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    seeded_member(&runtime, "worker", "beta", &[]).await?;
    let app = console_json_router_with_state(enforced_console(
        &runtime,
        Some(everyone_drives_every_agent()),
    ));
    let forked = fork_on_console(
        &app,
        Some(&person_bearer(alice_claims())),
        "alpha",
        "alpha-side",
    )
    .await;
    assert!(forked["error"].is_null(), "{forked:#}");
    drop(app);
    let side_chat_session = session_of(&runtime, "alpha-side").await?;
    let beta_session = session_of(&runtime, "beta").await?;
    leaves(&runtime, "alpha-side").await?;
    leaves(&runtime, "beta").await?;
    runtime.handle().shutdown().await?;
    let _ = runtime
        .stand_down_child_mobs(std::time::Instant::now() + Duration::from_secs(90))
        .await;
    drop(runtime);
    Ok(LeftBehind {
        state,
        side_chat_session,
        beta_session,
    })
}

/// The console of `runtime` started with an access controller whose rules
/// are off and no key to verify a token with: it identifies nobody, and its
/// one operator sends no token.
fn console_that_identifies_nobody(runtime: &MobRuntime) -> ConsoleJsonState {
    let mut console = enforced_console(
        runtime,
        Some(
            AccessController::new(AccessControlConfig {
                enabled: false,
                admins: vec![ROOT.to_string()],
                ..AccessControlConfig::default()
            })
            .expect("access controller"),
        ),
    );
    console.decisions.console.require_app_auth = false;
    console.decisions.trusted_oidc.jwks_json = r#"{"keys":[]}"#.to_string();
    console
}

/// A console that had sign-in is started again as one that identifies
/// nobody. A member that was private to a person stays hidden from its one
/// operator once the console has read who its members are private to: a
/// frame under the private member's name with no witness is refused like a
/// frame that carries its witness, while an ordinary ended member's frame
/// with no witness and one under a name no member ever held are read. A
/// frame with no witness cannot tell two holders of one name apart: once a
/// shared member holds the private member's name, it is decided by that
/// member.
#[tokio::test]
async fn a_console_restarted_as_one_that_identifies_nobody_keeps_a_known_private_member_hidden()
-> TestResult {
    const MOB: &str = "kept-apart-restart";
    let left = state_after_a_console_with_sign_in(MOB).await?;
    let runtime = persistent_gateway_runtime_in(left.state.path(), MOB, "", "").await?;
    let console = console_that_identifies_nobody(&runtime);
    let operator = console_request_auth_context(
        &console,
        &HeaderMap::new(),
        &Uri::from_static("/console/timeline"),
    )
    .expect("the console admits its operator")
    .audience;
    assert!(operator.subject().is_none() && !operator.kept_apart_from_other_people());

    // The record of who each member is private to is read, and the
    // operator's audience primed from it as every route primes it.
    let handle = runtime.handle();
    assert!(runtime.private_owners().seeded(&handle).await.landed());
    let known = runtime.private_owners().memory_members();
    assert!(
        known.get("alpha-side").is_some_and(Option::is_some)
            && known.get("beta").is_some_and(Option::is_none),
        "the record names the side chat's owner and knows beta as shared: {known:?}"
    );
    prime_access_cache_from_runtime(&runtime, &operator.controller()).await;
    for (name, expected, what) in [
        (
            "alpha-side",
            (true, false),
            "the ended private member: known, refused",
        ),
        (
            "beta",
            (true, true),
            "an ordinary ended member: known, read",
        ),
        (
            "ghost",
            (false, true),
            "a name no member ever held: not known, read",
        ),
    ] {
        assert_eq!(knows_and_reads(&operator, name).await, expected, "{what}");
    }
    assert!(
        !operator.can_view_captured_member(&[CapturedLineageLink {
            identity: "alpha-side".to_string(),
            attributes: Some(member(
                "alpha-side",
                &[(PRIVATE_TO_LABEL, "the-key-of-the-person-it-was-private-to")],
            )),
        }]),
        "a frame that carries the private member's witness is refused as well"
    );

    // A shared member takes the ended private member's name: the console
    // now knows the name as that member's, and a frame with no witness
    // under it is decided by the member that holds the name.
    seeded_member(&runtime, "worker", "alpha-side", &[]).await?;
    prime_access_cache_from_runtime(&runtime, &operator.controller()).await;
    assert_eq!(
        knows_and_reads(&operator, "alpha-side").await,
        (true, true),
        "a later shared holder of the name"
    );
    Ok(())
}

/// The record of who a runtime's members are private to, unreadable: every
/// read of it fails.
struct UnreadableRecord;

#[async_trait::async_trait]
impl crate::private_owners::MemberSpawnLog for UnreadableRecord {
    async fn spawns_after(
        &self,
        _cursor: u64,
        _limit: usize,
    ) -> Result<Option<crate::private_owners::MemberSpawnPage>, String> {
        Err("the mob's event log cannot be read".to_string())
    }
}

/// The frames a console's timeline routes serve a caller with no token: the
/// RPC page and the REST window, each as the text of its answer.
async fn timeline_routes(app: &Router) -> Vec<(&'static str, String)> {
    use tower::ServiceExt;
    let page = console_rpc_as(
        app,
        None,
        "meta/console/query_timeline",
        json!({ "limit": 500 }),
    )
    .await;
    assert!(page["error"].is_null(), "{page:#}");
    let response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/console/timeline?limit=500")
                .body(axum::body::Body::empty())
                .expect("window request"),
        )
        .await
        .expect("window response");
    assert_eq!(response.status(), StatusCode::OK);
    let window = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .expect("window body");
    vec![
        ("the RPC page", page["result"]["frames"].to_string()),
        (
            "the REST window",
            String::from_utf8_lossy(&window).into_owned(),
        ),
    ]
}

/// Until a console has read who its runtime's members are private to, it
/// cannot tell an ended private member from any other name: right after a
/// restart, and for as long as that read fails, a frame with no witness is
/// served to nobody but an administrator, on a console that identifies
/// nobody too. Once the read has succeeded the same routes serve the
/// ordinary ended member's frame and keep the private member's hidden.
#[tokio::test]
async fn a_frame_with_no_witness_waits_for_the_first_read_of_who_members_are_private_to()
-> TestResult {
    const MOB: &str = "kept-apart-first-read";
    let left = state_after_a_console_with_sign_in(MOB).await?;
    let restarted = persistent_gateway_runtime_in(left.state.path(), MOB, "", "").await?;
    let runtime = runtime_whose_first_read_failed(&restarted).await;
    let aggregator = MetaConsoleAggregator::in_memory();
    for (name, canary) in [
        ("alpha-side", "record-of-the-private-member"),
        ("beta", "record-of-the-ordinary-member"),
    ] {
        aggregator
            .store()
            .append_if_absent(no_witness_frame(name, canary))
            .await?;
    }
    let mut console = console_that_identifies_nobody(&runtime);
    console.console_aggregator = Some(aggregator);
    let app = console_json_router_with_state(console);

    for (route, served) in timeline_routes(&app).await {
        for canary in [
            "record-of-the-private-member",
            "record-of-the-ordinary-member",
        ] {
            assert!(
                !served.contains(canary),
                "before the first read succeeds {route} serves no frame with no witness: \
                 {canary}"
            );
        }
    }

    // The read succeeds.
    let handle = runtime.handle();
    assert!(runtime.private_owners().seeded(&handle).await.landed());
    for (route, served) in timeline_routes(&app).await {
        assert!(
            served.contains("record-of-the-ordinary-member"),
            "once the record is read {route} serves the ordinary ended member's frame"
        );
        assert!(
            !served.contains("record-of-the-private-member"),
            "and {route} keeps the private member's frame hidden"
        );
    }
    Ok(())
}

/// The console's runtime over the mob of `restarted`, with the first read of
/// who its members are private to held open: the read was tried and failed.
async fn runtime_whose_first_read_failed(restarted: &MobRuntime) -> MobRuntime {
    let runtime = MobRuntime::from_handle(restarted.handle());
    assert!(
        !runtime
            .private_owners()
            .seeded(&UnreadableRecord)
            .await
            .landed(),
        "the first read has not succeeded"
    );
    runtime
}

/// The audience of the one operator of the console of `runtime` (a console
/// that identifies nobody), primed from the runtime as every route primes
/// it before it decides.
async fn primed_operator(runtime: &MobRuntime) -> AccessView {
    let operator = console_request_auth_context(
        &console_that_identifies_nobody(runtime),
        &HeaderMap::new(),
        &Uri::from_static("/console/rpc"),
    )
    .expect("the console admits its operator")
    .audience;
    prime_access_cache_from_runtime(runtime, &operator.controller()).await;
    operator
}

/// Until the first read of who members are private to has succeeded, the
/// memory scope of a member that ended is shown to nobody but an
/// administrator, on a console that identifies nobody too: a read of it is
/// answered as a read of a member that does not exist. A member on the
/// roster is withheld from nobody. Once the read has succeeded the ordinary
/// ended member's scope is read and the private member's stays withheld.
#[tokio::test]
async fn an_ended_members_memory_scope_waits_for_the_first_read_of_who_members_are_private_to()
-> TestResult {
    /// Whether a read of `member`'s memory scope reaches that member for
    /// `view`, or is answered as a read of a member that does not exist.
    fn reads_memory_scope_of(view: &AccessView, member: &str) -> bool {
        let mut params = json!({ "identity": member });
        console_rpc_resolve_members(Some(view), "meta/agent_memory/manifest", &mut params)
            .expect("a member a method reads is never refused");
        params["identity"] == json!(member)
    }
    const MOB: &str = "kept-apart-first-read-scope";
    let left = state_after_a_console_with_sign_in(MOB).await?;
    let restarted = persistent_gateway_runtime_in(left.state.path(), MOB, "", "").await?;
    let runtime = runtime_whose_first_read_failed(&restarted).await;

    let operator = primed_operator(&runtime).await;
    for ended in ["alpha-side", "beta"] {
        assert!(
            !reads_memory_scope_of(&operator, ended),
            "before the first read succeeds the memory scope of {ended} is shown to nobody"
        );
    }
    assert!(
        reads_memory_scope_of(&operator, "alpha"),
        "a member on the roster is withheld from nobody"
    );

    // The read succeeds.
    let handle = runtime.handle();
    assert!(runtime.private_owners().seeded(&handle).await.landed());
    let operator = primed_operator(&runtime).await;
    assert!(
        reads_memory_scope_of(&operator, "beta"),
        "once the record is read the ordinary ended member's scope is read"
    );
    assert!(
        !reads_memory_scope_of(&operator, "alpha-side"),
        "and the private member's stays withheld"
    );
    Ok(())
}

/// Until the first read of who members are private to has succeeded, the
/// session of a member that ended, like a session no member ever ran on,
/// is answered to everyone but an administrator as a session that does not
/// exist, on a console that identifies nobody too. Once the read has
/// succeeded that console's operator attaches the ordinary ended member's
/// session as before.
#[tokio::test]
async fn an_ended_members_session_waits_for_the_first_read_of_who_members_are_private_to()
-> TestResult {
    async fn attach(
        view: &AccessView,
        runtime: &MobRuntime,
        session: &meerkat_core::types::SessionId,
    ) -> &'static str {
        match attached_session_decision(Some(view), runtime, None, session).await {
            SessionAttachDecision::Allowed => "allowed",
            SessionAttachDecision::Absent => "absent",
            SessionAttachDecision::Denied(_) => "denied",
        }
    }
    const MOB: &str = "kept-apart-first-read-attach";
    let left = state_after_a_console_with_sign_in(MOB).await?;
    let restarted = persistent_gateway_runtime_in(left.state.path(), MOB, "", "").await?;
    let runtime = runtime_whose_first_read_failed(&restarted).await;
    let never_owned = meerkat_core::types::SessionId::new();

    let operator = primed_operator(&runtime).await;
    for (session, whose) in [
        (&left.beta_session, "the ordinary ended member's session"),
        (&never_owned, "a session no member ran on"),
    ] {
        assert_eq!(
            attach(&operator, &runtime, session).await,
            "absent",
            "before the first read succeeds: {whose}"
        );
    }

    // The read succeeds.
    let handle = runtime.handle();
    assert!(runtime.private_owners().seeded(&handle).await.landed());
    let operator = primed_operator(&runtime).await;
    for (session, whose) in [
        (&left.beta_session, "the ordinary ended member's session"),
        (&never_owned, "a session no member ran on"),
    ] {
        assert_eq!(
            attach(&operator, &runtime, session).await,
            "allowed",
            "once the record is read: {whose}"
        );
    }
    assert_ne!(left.side_chat_session, left.beta_session);
    Ok(())
}

/// The record of who the members of `mob` are private to, read through a
/// gate: every read waits until the gate opens, then reads the mob's event
/// log. Counts the reads that start (a read starts at the log's beginning).
struct GatedRecord {
    gate: tokio::sync::watch::Receiver<bool>,
    mob: meerkat_mob::MobHandle,
    reads: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl crate::private_owners::MemberSpawnLog for GatedRecord {
    async fn spawns_after(
        &self,
        cursor: u64,
        limit: usize,
    ) -> Result<Option<crate::private_owners::MemberSpawnPage>, String> {
        if cursor == 0 {
            self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        let mut gate = self.gate.clone();
        let _ = gate.wait_for(|open| *open).await;
        crate::private_owners::MemberSpawnLog::spawns_after(&self.mob, cursor, limit).await
    }
}

/// The console over the mob of `restarted` whose first read of who members
/// are private to is in flight behind `gate` (as the background read is
/// right after a restart), holding a frame with no witness of the ended
/// private member and one of the ordinary ended member. Returns the
/// console's router and the record being read.
async fn console_whose_first_read_is_in_flight(
    restarted: &MobRuntime,
    gate: tokio::sync::watch::Receiver<bool>,
) -> Result<(Router, Arc<GatedRecord>), Box<dyn std::error::Error + Send + Sync>> {
    let runtime = MobRuntime::from_handle(restarted.handle());
    let record = Arc::new(GatedRecord {
        gate,
        mob: restarted.handle(),
        reads: std::sync::atomic::AtomicUsize::new(0),
    });
    // The background read, as a runtime starts it: one task, until it lands.
    let owners = runtime.private_owners().clone();
    let reading = Arc::clone(&record);
    tokio::spawn(async move {
        owners.seeded(reading.as_ref()).await;
    });
    for _ in 0..200 {
        if record.reads.load(std::sync::atomic::Ordering::SeqCst) > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        record.reads.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the first read is in flight"
    );
    let aggregator = MetaConsoleAggregator::in_memory();
    for (name, canary) in [
        ("alpha-side", "record-of-the-private-member"),
        ("beta", "record-of-the-ordinary-member"),
    ] {
        aggregator
            .store()
            .append_if_absent(no_witness_frame(name, canary))
            .await?;
    }
    let mut console = console_that_identifies_nobody(&runtime);
    console.console_aggregator = Some(aggregator);
    Ok((console_json_router_with_state(console), record))
}

/// The frames the RPC timeline page serves a caller with no token, as text.
async fn rpc_page(app: &Router) -> String {
    let page = console_rpc_as(
        app,
        None,
        "meta/console/query_timeline",
        json!({ "limit": 500 }),
    )
    .await;
    assert!(page["error"].is_null(), "{page:#}");
    page["result"]["frames"].to_string()
}

/// A route waits for a first read that is in flight, but not for ever: when
/// the read does not return, the route answers within its bound, and
/// answers as it does while the read has not succeeded: no frame with no
/// witness for anybody but an administrator.
#[tokio::test]
async fn a_route_waits_a_bounded_time_for_a_first_read_that_does_not_return() -> TestResult {
    const MOB: &str = "kept-apart-first-read-bound";
    let left = state_after_a_console_with_sign_in(MOB).await?;
    let restarted = persistent_gateway_runtime_in(left.state.path(), MOB, "", "").await?;
    // The gate never opens.
    let (_closed, gate) = tokio::sync::watch::channel(false);
    let (app, record) = console_whose_first_read_is_in_flight(&restarted, gate).await?;

    let bound = crate::private_owners::FIRST_READ_WAIT;
    let asked = std::time::Instant::now();
    let served = tokio::time::timeout(bound * 6, rpc_page(&app))
        .await
        .map_err(|_| "the route answers although the read never returns")?;
    let waited = asked.elapsed();
    assert!(
        waited >= bound.mul_f32(0.9),
        "the route waited for the read in flight: {waited:?}"
    );
    for canary in [
        "record-of-the-private-member",
        "record-of-the-ordinary-member",
    ] {
        assert!(
            !served.contains(canary),
            "after the bound the route serves no frame with no witness: {canary}"
        );
    }
    assert_eq!(
        record.reads.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the waiting route started no read of its own"
    );
    Ok(())
}

/// Many route calls made while the first read is in flight wait for that
/// one read: none starts a read of its own, and once the read lands each of
/// them answers from what it read, the ordinary ended member's frame served
/// and the private member's hidden.
#[tokio::test]
async fn route_calls_made_while_the_first_read_is_in_flight_wait_for_that_one_read() -> TestResult {
    const MOB: &str = "kept-apart-first-read-waiters";
    let left = state_after_a_console_with_sign_in(MOB).await?;
    let restarted = persistent_gateway_runtime_in(left.state.path(), MOB, "", "").await?;
    let (open, gate) = tokio::sync::watch::channel(false);
    let (app, record) = console_whose_first_read_is_in_flight(&restarted, gate).await?;

    let mut calls = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let app = app.clone();
        calls.spawn(async move { rpc_page(&app).await });
    }
    // Every call has had time to reach its wait; the read is still the one.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(record.reads.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(
        calls.try_join_next().is_none(),
        "no call answers while the read is in flight"
    );
    open.send(true)?;
    let mut answers = 0;
    while let Some(served) = calls.join_next().await {
        let served = served?;
        assert!(
            served.contains("record-of-the-ordinary-member"),
            "a waiting call serves the ordinary ended member's frame once the read lands"
        );
        assert!(
            !served.contains("record-of-the-private-member"),
            "and keeps the private member's frame hidden"
        );
        answers += 1;
    }
    assert_eq!(answers, 8);
    assert_eq!(
        record.reads.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "eight calls, one read"
    );
    Ok(())
}

/// A member whose run is in flight is retired. The run ends on the console
/// with a failure record that names the run's session: it carries the
/// member's witness, so everyone who may view the member reads that the run
/// ended. The retire notice itself names no session and carries no witness:
/// an administrator reads it, no other person does.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_run_in_flight_at_a_retire_ends_with_a_record_the_members_viewers_read() -> TestResult {
    use crate::unified_runtime::in_flight_drain_tests::{ModelCalls, gated_runtime, seat_mid_turn};

    let (_gate, gate) = tokio::sync::watch::channel(false);
    let calls = Arc::new(ModelCalls::default());
    let runtime = Arc::new(gated_runtime("kept-apart-cut-run", gate, Arc::clone(&calls)).await);
    // The gateways drain runtime events on a task of their own.
    let drain = Arc::clone(&runtime).spawn_event_drain_task();
    let aggregator = MetaConsoleAggregator::in_memory();
    aggregator.register_runtime_handles_with_policy(
        "kept-apart",
        "",
        runtime.mob_runtime().clone(),
        None,
        runtime.console_events(),
        Arc::new(AllowAllConsoleVisibilityPolicy),
    );
    let identity = seat_mid_turn(&runtime.mob_handle(), &calls, "alpha").await;
    let frames = || async {
        aggregator
            .store()
            .query_frames(ConsoleTimelineQuery {
                identity: Some("alpha".to_string()),
                limit: 1_000,
                ..Default::default()
            })
            .await
            .expect("stored frames")
            .frames
    };
    // The run's start reaches the console, with the member's witness.
    let mut started = None;
    for _ in 0..200 {
        started = frames()
            .await
            .into_iter()
            .find(|frame| frame.kind == "run_started");
        if started.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let started = started.ok_or("the run's start reaches the console")?;
    let session = started
        .session_id
        .clone()
        .ok_or("the run's start names its session")?;
    assert!(started.source.member_provenance.is_some());

    runtime.mob_handle().retire(identity.clone()).await?;
    runtime
        .record_console_lifecycle("alpha", "member_retired", json!({}))
        .await;
    let (mut ended, mut notice) = (None, None);
    for _ in 0..200 {
        let stored = frames().await;
        ended = stored
            .iter()
            .find(|frame| matches!(frame.kind.as_str(), "interaction_failed" | "run_failed"))
            .cloned();
        notice = stored
            .iter()
            .find(|frame| frame.kind == "member_retired")
            .cloned();
        if ended.is_some() && notice.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let ended = ended.ok_or("the run in flight ends with a failure record")?;
    let notice = notice.ok_or("the retire notice is stored")?;
    let controller = everyone_drives_every_agent();
    let person = controller.view_for_caller(Some(BOB), Some("bob-key"));
    let admin = controller.view_for_caller(Some(ROOT), None);
    assert_eq!(
        ended.session_id.as_deref(),
        Some(session.as_str()),
        "the failure record names the run's session"
    );
    assert!(
        ended.source.member_provenance.is_some() && timeline_frame_visible_to(&person, &ended),
        "it carries the member's witness and a person who may view the member reads it"
    );
    assert!(
        notice.session_id.is_none() && notice.source.member_provenance.is_none(),
        "the retire notice names no session and carries no witness"
    );
    assert!(!timeline_frame_visible_to(&person, &notice));
    assert!(timeline_frame_visible_to(&admin, &notice));
    drain.abort();
    let _ = runtime.shutdown().await;
    Ok(())
}

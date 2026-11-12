//! The callers a console keeps apart from other people. A console that can
//! identify a person keeps every caller who is not an administrator apart
//! from the others, and only a console that says it identifies nobody has
//! one operator who is not kept apart. Each route that takes a view for a
//! caller it did not identify states its console's own answer, and a view
//! taken with no console to ask is kept apart.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;

use serde_json::json;

use super::console_private_side_chat_tests::TestResult;
use super::console_viewer_resolver_tests::{BOB, CAROL, bearer, decisions, rules_off, with_no_key};
use super::*;
use crate::access::{
    ACTION_AGENT_SEND, ACTION_GATING_VIEW, AccessControlConfig, AccessEffect, AccessRule,
    AgentResourceAttributes, CapturedLineageLink, PRIVATE_TO_LABEL,
};
use crate::auth::session_fence::ConsoleSessionFences;

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

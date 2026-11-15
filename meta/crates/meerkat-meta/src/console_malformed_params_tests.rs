//! The Console's request routes refuse a request whose params are
//! malformed, as invalid params, before any method runs: whatever the
//! method, whoever the caller and whatever agent the request names. A
//! refused request changes nothing, and a request is answered alike for an
//! agent the caller may not know and for a name no agent holds.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use meerkat_mob::SpawnMemberSpec;
use serde_json::{Value, json};

use super::console_private_audience_tests::fully_composed_console;
use super::console_private_side_chat_tests::{
    PRIVATE_TO, ROOT, TestResult, alice_claims, bearer, bob_claims, enforced_console,
    fork_on_console, root_claims, runtime,
};
use super::console_side_chat_tests::seeded_member;
use super::console_token_lapse_tests::console_rpc_as;
use super::*;
use crate::rpc::mob_methods::{
    ADOPT_MEMBER_IDENTITY_DECLARATION, APPLY_MEMBER_TOOL_DECLARATION, MEMBER_TOOL_DECLARATION,
};

/// Malformed params that carry `name`.
fn malformed(name: &str) -> Value {
    json!([name])
}

/// `answer` with each of `names` replaced by a placeholder and the time of
/// each observation left out, so the answers a method gives two names
/// compare as answers.
fn masked(answer: &Value, names: &[&str]) -> String {
    fn without_times(value: &mut Value) {
        match value {
            Value::Object(members) => {
                members.remove("observed_at_ms");
                members.values_mut().for_each(without_times);
            }
            Value::Array(items) => items.iter_mut().for_each(without_times),
            _ => {}
        }
    }
    let mut answer = answer.clone();
    without_times(&mut answer);
    names.iter().fold(answer.to_string(), |text, name| {
        text.replace(name, "<name>")
    })
}

/// Whether `answer` is the routes' refusal of malformed params and nothing
/// else: invalid params, with no result, no data and no name.
fn refused_as_malformed(answer: &Value, id: &Value) -> bool {
    *answer
        == json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32602, "message": "Invalid params" },
        })
}

/// The caller scope the console reports for the holder of `bearer`.
async fn caller_scope(app: &Router, bearer: &str) -> String {
    console_rpc_as(app, Some(bearer), "meta/console/whoami", json!({})).await["result"]
        ["caller_scope"]
        .as_str()
        .expect("the caller scope")
        .to_string()
}

/// `body` on `POST /console/rpc` as the holder of `bearer`: the status and
/// the JSON answer.
async fn rpc_body(app: &Router, bearer: &str, body: &Value) -> (StatusCode, Value) {
    use tower::ServiceExt;

    let response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/console/rpc")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
                .body(axum::body::Body::from(body.to_string()))
                .expect("console request"),
        )
        .await
        .expect("console response");
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("console body");
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

/// A request on `POST /console/rpc/multipart` whose payload part is
/// `payload`, as the holder of `bearer`: its JSON answer.
async fn multipart_rpc(app: &Router, bearer: &str, payload: &Value) -> Value {
    use tower::ServiceExt;

    let boundary = "malformed-params-multipart";
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"payload\"\r\n");
    body.extend_from_slice(b"Content-Type: application/json\r\n\r\n");
    body.extend_from_slice(payload.to_string().as_bytes());
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    let response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/console/rpc/multipart")
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
                .body(axum::body::Body::from(body))
                .expect("multipart request"),
        )
        .await
        .expect("multipart response");
    let body = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("multipart body");
    serde_json::from_slice(&body).unwrap_or(Value::Null)
}

/// A seated member's session and its roster labels: the baseline an
/// adoption of it states.
struct Seat {
    session: String,
    labels: BTreeMap<String, String>,
}

/// Seat `name` as a turn-driven worker with `labels`.
async fn seat(
    runtime: &MobRuntime,
    name: &str,
    labels: &[(&str, &str)],
) -> Result<Seat, Box<dyn std::error::Error + Send + Sync>> {
    let identity = AgentIdentity::from(name);
    let labels = labels
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect::<BTreeMap<_, _>>();
    Box::pin(
        runtime.handle().spawn_spec(
            SpawnMemberSpec::new("worker", identity.clone())
                .with_labels(labels)
                .with_runtime_mode(meerkat_mob::MobRuntimeMode::TurnDriven),
        ),
    )
    .await?;
    Ok(Seat {
        session: runtime
            .handle()
            .resolve_bridge_session_id(&identity)
            .await
            .ok_or_else(|| format!("{name} has a session"))?
            .to_string(),
        labels: runtime
            .handle()
            .get_member(&identity)
            .await?
            .ok_or_else(|| format!("{name} is seated"))?
            .labels,
    })
}

fn read_of(mob_id: &str, name: &str) -> Value {
    json!({ "mob_id": mob_id, "agent_identity": name })
}

fn adoption_of(mob_id: &str, name: &str, seat: &Seat, request_id: &str) -> Value {
    let session = &seat.session;
    json!({
        "mob_id": mob_id,
        "agent_identity": name,
        "request_id": request_id,
        "precondition": "expected_absent",
        "declaration_scope": "console-malformed-params",
        "declaration_revision": 1,
        "session": {
            "session_id": session,
            "lineage_id": format!("session:{session}"),
            "lineage_generation": 0,
            "authority_policy": "require_existing"
        },
        "member": {
            "profile_name": "worker",
            "labels": seat.labels,
            "runtime_mode": "turn_driven",
            "execution": { "execution": "controlling_session" }
        },
        "wiring_custody": "external_managed",
        "owned_wiring": [],
        "convergence": { "kind": "drain", "max_wait_ms": 5000 }
    })
}

fn apply_of(mob_id: &str, name: &str, request_id: &str, declaration: &Value) -> Value {
    json!({
        "mob_id": mob_id,
        "agent_identity": name,
        "request_id": request_id,
        "expected_intent_revision": 1,
        "declaration": declaration,
        "convergence": { "kind": "drain", "max_wait_ms": 5000 }
    })
}

/// What a change of a declaration would alter in its read: its revision
/// and its content (the convergence beside them moves on its own).
fn declared(read: &Value) -> (Value, Value) {
    (
        read["result"]["desired_intent_revision"].clone(),
        read["result"]["declaration"].clone(),
    )
}

/// On a console under `access` (none: sign-in only), with two agents
/// private to Alice, one of them holding a declaration: every way in which
/// Bob's answers for Alice's agents differ from his answers for a name no
/// agent holds, per method, for well-formed and for malformed params, and
/// everything his requests changed. Empty where every request is answered
/// alike and changes nothing.
async fn differences_from_an_absent_name(
    mob: &str,
    access: Option<AccessController>,
) -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>> {
    const OWN: &str = "alpha-own";
    const SECOND: &str = "alpha-second";
    const GONE: &str = "alpha-gone";
    let (_temp_dir, runtime) = runtime(mob).await?;
    let app = console_json_router_with_state(enforced_console(&runtime, access));
    let alice = bearer(alice_claims());
    let bob = bearer(bob_claims());
    let alice_scope = caller_scope(&app, &alice).await;
    let mob_id = runtime.handle().mob_id().to_string();
    let owner_label = [(PRIVATE_TO, alice_scope.as_str())];
    let own_seat = seat(&runtime, OWN, &owner_label).await?;
    let second_seat = seat(&runtime, SECOND, &owner_label).await?;

    let adopted = console_rpc_as(
        &app,
        Some(&alice),
        ADOPT_MEMBER_IDENTITY_DECLARATION,
        adoption_of(&mob_id, OWN, &own_seat, "adopt-own"),
    )
    .await;
    assert!(adopted["error"].is_null(), "the owner adopts: {adopted:#}");
    let owners = console_rpc_as(
        &app,
        Some(&alice),
        MEMBER_TOOL_DECLARATION,
        read_of(&mob_id, OWN),
    )
    .await;
    assert_eq!(
        owners["result"]["desired_intent_revision"],
        json!(1),
        "the owner reads her agent's declaration: {owners:#}"
    );
    let declaration = owners["result"]["declaration"].clone();

    // Each method with the agent it names and the same request for a name
    // no agent holds.
    let requests = [
        (
            MEMBER_TOOL_DECLARATION,
            OWN,
            read_of(&mob_id, OWN),
            read_of(&mob_id, GONE),
        ),
        (
            APPLY_MEMBER_TOOL_DECLARATION,
            OWN,
            apply_of(&mob_id, OWN, "apply-1", &declaration),
            apply_of(&mob_id, GONE, "apply-1", &declaration),
        ),
        (
            ADOPT_MEMBER_IDENTITY_DECLARATION,
            SECOND,
            adoption_of(&mob_id, SECOND, &second_seat, "adopt-1"),
            adoption_of(&mob_id, GONE, &second_seat, "adopt-1"),
        ),
    ];
    let mut differences = Vec::new();
    for (method, name, private, absent) in requests {
        let private = console_rpc_as(&app, Some(&bob), method, private).await;
        let absent = console_rpc_as(&app, Some(&bob), method, absent).await;
        if masked(&private, &[name]) != masked(&absent, &[GONE]) {
            differences.push(format!(
                "{method}, well-formed: the answers for the two names differ: \
                 {private} / {absent}"
            ));
        }
        let absent = console_rpc_as(&app, Some(&bob), method, malformed(GONE)).await;
        let private = console_rpc_as(&app, Some(&bob), method, malformed(name)).await;
        if masked(&private, &[name]) != masked(&absent, &[GONE]) {
            differences.push(format!(
                "{method}, malformed: the answers for the two names differ: \
                 {private} / {absent}"
            ));
        }
    }

    let after = console_rpc_as(
        &app,
        Some(&alice),
        MEMBER_TOOL_DECLARATION,
        read_of(&mob_id, OWN),
    )
    .await;
    if declared(&after) != declared(&owners) {
        differences.push(format!(
            "the declaration of {OWN} changed: {} -> {}",
            owners["result"], after["result"]
        ));
    }
    let second = console_rpc_as(
        &app,
        Some(&alice),
        MEMBER_TOOL_DECLARATION,
        read_of(&mob_id, SECOND),
    )
    .await;
    if !second["result"].is_null() {
        differences.push(format!("{SECOND} holds a declaration: {second}"));
    }
    Ok(differences)
}

/// On a console with sign-in and no access configuration, a request is
/// answered alike for an agent private to another person and for a name no
/// agent holds, whether its params are well-formed or malformed, and a
/// refused request changes nothing.
#[tokio::test]
async fn a_request_is_answered_alike_for_a_private_agent_and_an_absent_name() -> TestResult {
    let differences = differences_from_an_absent_name("malformed-params-private", None).await?;
    assert!(differences.is_empty(), "{differences:#?}");
    Ok(())
}

/// The same under an enforced access configuration.
#[tokio::test]
async fn a_request_is_answered_alike_for_a_private_agent_and_an_absent_name_under_access_rules()
-> TestResult {
    let access = AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![ROOT.to_string()],
        rules: vec![AccessRule {
            id: "everyone-administers-the-runtime".to_string(),
            subjects: vec!["*".to_string()],
            actions: vec![
                "agent.*".to_string(),
                crate::access::ACTION_RUNTIME_ADMIN.to_string(),
            ],
            ..AccessRule::default()
        }],
        ..crate::access::AccessControlConfig::default()
    })?;
    let differences =
        differences_from_an_absent_name("malformed-params-runtime-admin", Some(access)).await?;
    assert!(differences.is_empty(), "{differences:#?}");
    Ok(())
}

/// A well-formed adoption by one person changes nothing of an agent private
/// to another person. One that names the private agent in the wiring it
/// owns is refused as naming an unknown agent, and the private agent's
/// stored declaration and convergence are what they were, as is its
/// owner's next change. One on the private agent's session is answered as
/// one on a session no agent holds.
#[tokio::test]
async fn an_adoption_changes_nothing_of_an_agent_private_to_another_person() -> TestResult {
    const OWN: &str = "alpha-own";
    let (_temp_dir, runtime) = runtime("malformed-params-adoption").await?;
    let app = console_json_router_with_state(enforced_console(&runtime, None));
    let alice = bearer(alice_claims());
    let bob = bearer(bob_claims());
    let alice_scope = caller_scope(&app, &alice).await;
    let mob_id = runtime.handle().mob_id().to_string();
    let own_seat = seat(&runtime, OWN, &[(PRIVATE_TO, alice_scope.as_str())]).await?;
    let adopted = console_rpc_as(
        &app,
        Some(&alice),
        ADOPT_MEMBER_IDENTITY_DECLARATION,
        adoption_of(&mob_id, OWN, &own_seat, "adopt-own"),
    )
    .await;
    assert!(adopted["error"].is_null(), "the owner adopts: {adopted:#}");
    let owners = console_rpc_as(
        &app,
        Some(&alice),
        MEMBER_TOOL_DECLARATION,
        read_of(&mob_id, OWN),
    )
    .await;
    let declaration = owners["result"]["declaration"].clone();
    let owners_apply = |request_id: &'static str| {
        console_rpc_as(
            &app,
            Some(&alice),
            APPLY_MEMBER_TOOL_DECLARATION,
            apply_of(&mob_id, OWN, request_id, &declaration),
        )
    };
    let applied_before = owners_apply("apply-before").await;
    assert!(
        applied_before["error"].is_null(),
        "the owner's change is served: {applied_before:#}"
    );
    let identity = AgentIdentity::from(OWN);
    // The agent's stored declaration, whole, and the revisions its
    // convergence stands at (its condition moves on its own).
    let stored = || async {
        let handle = runtime.handle();
        let convergence = match handle.identity_convergence_status(&identity).await {
            Ok(meerkat_mob::identity::IdentityStoredObservation::Valid(status)) => format!(
                "{:?} {:?}",
                status.intent_revision, status.active_intent_revision
            ),
            other => format!("{other:?}"),
        };
        (
            format!("{:?}", handle.identity_intent(&identity).await),
            convergence,
        )
    };
    let before = stored().await;

    // Another person's adoption of their own agent, owning an edge to the
    // private agent, to a name no agent holds, and to an agent they may
    // view. The adopting agent is the first end of each edge it owns.
    const ADOPTER: &str = "alpha-adopter";
    let adopter_seat = seat(&runtime, ADOPTER, &[]).await?;
    seat(&runtime, "alpha-peer", &[]).await?;
    let adopter = AgentIdentity::from(ADOPTER);
    let adopters_declaration =
        || async { format!("{:?}", runtime.handle().identity_intent(&adopter).await) };
    let undeclared = adopters_declaration().await;
    let wired_to = |peer: &'static str| {
        let mut wired = adoption_of(&mob_id, ADOPTER, &adopter_seat, "adopt-adopter");
        wired["wiring_custody"] = json!("identity_owned");
        wired["owned_wiring"] = json!([{ "a": ADOPTER, "b": peer }]);
        console_rpc_as(&app, Some(&bob), ADOPT_MEMBER_IDENTITY_DECLARATION, wired)
    };
    let refused = wired_to(OWN).await;
    let after = stored().await;
    let applied_after = owners_apply("apply-after").await;
    assert_eq!(
        (&refused["error"]["code"], &refused["result"]),
        (&json!(-32001), &Value::Null),
        "an adoption that owns wiring to the private agent is refused as naming an unknown \
         agent: {refused:#}"
    );
    let absent = wired_to("alpha-gone").await;
    assert_eq!(
        masked(&refused, &[OWN]),
        masked(&absent, &["alpha-gone"]),
        "and so is one that owns wiring to a name no agent holds: {absent:#}"
    );
    let two_edges = |first: &'static str, second: &'static str| {
        let mut wired = adoption_of(&mob_id, ADOPTER, &adopter_seat, "adopt-adopter");
        wired["wiring_custody"] = json!("identity_owned");
        wired["owned_wiring"] = json!([
            { "a": ADOPTER, "b": first },
            { "a": ADOPTER, "b": second },
        ]);
        console_rpc_as(&app, Some(&bob), ADOPT_MEMBER_IDENTITY_DECLARATION, wired)
    };
    assert_eq!(
        two_edges("alpha-gone", OWN).await,
        two_edges("alpha-gone", "alpha-lost").await,
        "an adoption that owns wiring to a name no agent holds and to the private agent is \
         answered as one that owns wiring to two such names"
    );
    assert_eq!(
        adopters_declaration().await,
        undeclared,
        "a refused adoption records nothing"
    );
    let viewed = wired_to("alpha-peer").await;
    assert!(
        viewed["error"].is_null(),
        "an adoption that owns wiring to an agent the caller may view is served: {viewed:#}"
    );
    let declared = adopters_declaration().await;
    assert!(
        declared.contains("alpha-peer") && !declared.contains(OWN),
        "and records that edge: {declared}"
    );
    assert_eq!(
        after, before,
        "the private agent's stored declaration and convergence are what they were"
    );
    assert_eq!(
        (&applied_after["error"], &applied_after["result"]["commit"]),
        (
            &applied_before["error"],
            &applied_before["result"]["commit"]
        ),
        "the owner's next change is served as before: {applied_after:#}"
    );

    // An adoption on the private agent's session, and one on a session no
    // agent holds.
    let no_session = "01a1b8c0-96e4-71d4-8671-d7a73527e863";
    let mut answers = Vec::new();
    for (name, session) in [
        ("beta-two", own_seat.session.as_str()),
        ("beta-three", no_session),
    ] {
        let seat = seat(&runtime, name, &[]).await?;
        let mut adoption = adoption_of(&mob_id, name, &seat, &format!("adopt-{name}"));
        adoption["session"]["session_id"] = json!(session);
        adoption["session"]["lineage_id"] = json!(format!("session:{session}"));
        let answer = console_rpc_as(
            &app,
            Some(&bob),
            ADOPT_MEMBER_IDENTITY_DECLARATION,
            adoption,
        )
        .await;
        answers.push(masked(&answer, &[name, session]));
    }
    assert_eq!(
        answers[0], answers[1],
        "an adoption on the private agent's session is answered as one on a session no agent \
         holds"
    );
    assert_eq!(
        stored().await,
        before,
        "the private agent's stored declaration and convergence are what they were"
    );
    Ok(())
}

/// The same for a caller the access rules do not let adopt: an adoption
/// that owns wiring to an agent private to another person is answered as
/// one that owns wiring to a name no agent holds, and one whose wiring
/// names an agent the caller may view is refused by the rules.
#[tokio::test]
async fn an_adoption_is_answered_alike_for_a_private_agent_and_an_absent_name_under_access_rules()
-> TestResult {
    const OWN: &str = "alpha-own";
    let access = AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![ROOT.to_string()],
        rules: vec![AccessRule {
            id: "everyone-drives-every-agent".to_string(),
            subjects: vec!["*".to_string()],
            actions: vec!["agent.*".to_string()],
            ..AccessRule::default()
        }],
        ..crate::access::AccessControlConfig::default()
    })?;
    let (_temp_dir, runtime) = runtime("malformed-params-adoption-rules").await?;
    let app = console_json_router_with_state(enforced_console(&runtime, Some(access)));
    let alice = bearer(alice_claims());
    let bob = bearer(bob_claims());
    let alice_scope = caller_scope(&app, &alice).await;
    let mob_id = runtime.handle().mob_id().to_string();
    seat(&runtime, OWN, &[(PRIVATE_TO, alice_scope.as_str())]).await?;
    seat(&runtime, "alpha-peer", &[]).await?;
    let adopter_seat = seat(&runtime, "alpha-adopter", &[]).await?;
    let wired_to = |peer: &'static str| {
        let mut wired = adoption_of(&mob_id, "alpha-adopter", &adopter_seat, "adopt-adopter");
        wired["wiring_custody"] = json!("identity_owned");
        wired["owned_wiring"] = json!([{ "a": "alpha-adopter", "b": peer }]);
        console_rpc_as(&app, Some(&bob), ADOPT_MEMBER_IDENTITY_DECLARATION, wired)
    };
    let private = wired_to(OWN).await;
    let absent = wired_to("alpha-gone").await;
    assert_eq!(
        masked(&private, &[OWN]),
        masked(&absent, &["alpha-gone"]),
        "the private agent and a name no agent holds are answered alike: {private:#} / \
         {absent:#}"
    );
    assert_eq!(
        (&private["error"]["code"], &private["result"]),
        (&json!(-32001), &Value::Null),
        "as naming an unknown agent: {private:#}"
    );
    let viewed = wired_to("alpha-peer").await;
    assert_eq!(
        viewed["error"]["code"],
        json!(ACCESS_DENIED_RPC_CODE),
        "the rules refuse the caller an adoption whose wiring names agents it may view: \
         {viewed:#}"
    );
    Ok(())
}

/// Under an access rule that keeps a caller from an agent, a well-formed
/// request for that agent is answered the rule's refusal, and a request
/// with malformed params the refusal of malformed params, whichever agent
/// it names.
#[tokio::test]
async fn a_request_for_an_agent_an_access_rule_denies_is_refused() -> TestResult {
    const GUARDED: &str = "alpha-guarded";
    const OPEN: &str = "alpha-open";
    let actions = || {
        vec![
            crate::access::ACTION_AGENT_VIEW.to_string(),
            crate::access::ACTION_AGENT_SEND.to_string(),
        ]
    };
    let access = AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![ROOT.to_string()],
        rules: vec![
            AccessRule {
                id: "everyone-views-and-sends".to_string(),
                subjects: vec!["*".to_string()],
                actions: actions(),
                ..AccessRule::default()
            },
            AccessRule {
                id: "no-one-reaches-the-restricted".to_string(),
                effect: crate::access::AccessEffect::Deny,
                subjects: vec!["*".to_string()],
                actions: actions(),
                match_labels: BTreeMap::from([("org".to_string(), "restricted".to_string())]),
                ..AccessRule::default()
            },
        ],
        ..crate::access::AccessControlConfig::default()
    })?;
    let (_temp_dir, runtime) = runtime("malformed-params-rules").await?;
    let app = console_json_router_with_state(enforced_console(&runtime, Some(access)));
    let root = bearer(root_claims());
    let bob = bearer(bob_claims());
    let mob_id = runtime.handle().mob_id().to_string();
    let guarded_seat = seat(&runtime, GUARDED, &[("org", "restricted")]).await?;
    let open_seat = seat(&runtime, OPEN, &[]).await?;
    for (name, seat) in [(GUARDED, &guarded_seat), (OPEN, &open_seat)] {
        let adopted = console_rpc_as(
            &app,
            Some(&root),
            ADOPT_MEMBER_IDENTITY_DECLARATION,
            adoption_of(&mob_id, name, seat, &format!("adopt-{name}")),
        )
        .await;
        assert!(
            adopted["error"].is_null(),
            "the admin adopts {name}: {adopted:#}"
        );
    }

    let open = console_rpc_as(
        &app,
        Some(&bob),
        MEMBER_TOOL_DECLARATION,
        read_of(&mob_id, OPEN),
    )
    .await;
    assert_eq!(
        open["result"]["desired_intent_revision"],
        json!(1),
        "a well-formed read of an agent the caller may view is answered: {open:#}"
    );
    let id = json!("test");
    for (method, well_formed) in [
        (MEMBER_TOOL_DECLARATION, read_of(&mob_id, GUARDED)),
        (
            "meta/console/send",
            json!({
                "identity": GUARDED,
                "content": "hello",
                "origin": "console",
                "idempotency_key": "malformed-params-rules",
            }),
        ),
    ] {
        let denied = console_rpc_as(&app, Some(&bob), method, well_formed).await;
        assert_eq!(
            (
                &denied["error"]["code"],
                &denied["error"]["data"]["resource"]
            ),
            (&json!(ACCESS_DENIED_RPC_CODE), &json!(GUARDED)),
            "{method}: the rule refuses a well-formed request: {denied:#}"
        );
        for name in [GUARDED, OPEN] {
            let answer = console_rpc_as(&app, Some(&bob), method, malformed(name)).await;
            assert!(
                refused_as_malformed(&answer, &id),
                "{method}: malformed params are refused for {name}: {answer:#}"
            );
        }
    }
    Ok(())
}

/// A refused request changes nothing: for a member's owner and for another
/// person, malformed params are refused alike for the member and for a
/// name no member holds, and afterwards the member's state is what it was.
#[tokio::test]
async fn a_refused_request_changes_nothing() -> TestResult {
    const SIDE: &str = "alpha-side";
    const GONE: &str = "alpha-gone";
    let (_temp_dir, runtime) = runtime("malformed-params-effects").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let alice = bearer(alice_claims());
    let bob = bearer(bob_claims());
    let forking = console_json_router_with_state(enforced_console(&runtime, None));
    let forked = fork_on_console(&forking, Some(&alice), "alpha", SIDE).await;
    assert!(forked["error"].is_null(), "{forked:#}");
    let memory_dir = tempfile::tempdir()?;
    let app = console_json_router_with_state(
        fully_composed_console(&runtime, SIDE, memory_dir.path()).await?,
    );
    // What the owner reads of her side chat: its grants and its seat.
    let owners_reads = || async {
        let grants = console_rpc_as(
            &app,
            Some(&alice),
            "meta/tool_grants/list",
            json!({ "identity": SIDE }),
        )
        .await;
        let member = console_rpc_as(
            &app,
            Some(&alice),
            "meta/get_member",
            json!({ "member_id": SIDE }),
        )
        .await;
        assert!(
            grants["error"].is_null() && member["error"].is_null(),
            "the owner reads her side chat: {grants:#} {member:#}"
        );
        (
            grants["result"]["grants"].clone(),
            member["result"]["state"].clone(),
        )
    };
    let before = owners_reads().await;

    let id = json!("test");
    let mut unrefused = Vec::new();
    for method in [
        "meta/tool_grants/list",
        "meta/tool_grants/audit",
        "meta/tool_grants/grant",
        "meta/console/query_timeline",
        "meta/mob_events/query",
        "meta/mob_events/subscribe",
        "meta/console/recover_send",
        "meta/console/send",
        "meta/retire_member",
    ] {
        for (caller, name) in [(&bob, GONE), (&bob, SIDE), (&alice, SIDE)] {
            let answer = console_rpc_as(&app, Some(caller), method, malformed(name)).await;
            if !refused_as_malformed(&answer, &id) {
                unrefused.push(format!("{method} for {name}: {answer}"));
            }
        }
    }
    assert!(unrefused.is_empty(), "{unrefused:#?}");
    assert_eq!(
        owners_reads().await,
        before,
        "the side chat's grants and seat are what they were"
    );
    Ok(())
}

/// Params that carry `name` at `path`.
fn naming(path: &[&str], name: &str) -> Value {
    match path.split_first() {
        None => json!(name),
        Some((&"*", rest)) => json!([naming(rest, name)]),
        Some((key, rest)) => Value::Object(serde_json::Map::from_iter([(
            (*key).to_string(),
            naming(rest, name),
        )])),
    }
}

/// `params` with the value `depth` steps down `path` replaced by malformed
/// params that carry `name`; `None` where that value is none to replace.
fn malformed_at(params: &Value, path: &[&str], depth: usize, name: &str) -> Option<Value> {
    if depth == 0 {
        return params.is_object().then(|| malformed(name));
    }
    let (head, rest) = path.split_first()?;
    match (*head, params) {
        ("*", Value::Array(entries)) => {
            let entry = malformed_at(entries.first()?, rest, depth - 1, name)?;
            Some(json!([entry]))
        }
        (key, Value::Object(members)) => {
            let inner = malformed_at(members.get(key)?, rest, depth - 1, name)?;
            let mut members = members.clone();
            members.insert(key.to_string(), inner);
            Some(Value::Object(members))
        }
        _ => None,
    }
}

/// Malformed params are refused wherever a request names an agent: for
/// every method whose params name an agent below their top level, params
/// that are malformed on the way to the name are answered the one
/// invalid-params refusal and nothing else, before any method runs, and
/// the member resolution and the access check refuse them too. The same
/// params well-formed are not answered that refusal.
#[tokio::test]
async fn malformed_params_are_refused_wherever_a_request_names_an_agent() -> TestResult {
    const SIDE: &str = "alpha-side";
    let (_temp_dir, runtime) = runtime("malformed-params-agents").await?;
    seeded_member(&runtime, "worker", "alpha", &[]).await?;
    let app = console_json_router_with_state(enforced_console(&runtime, None));
    let alice = bearer(alice_claims());
    let bob = bearer(bob_claims());
    let forked = fork_on_console(&app, Some(&alice), "alpha", SIDE).await;
    assert!(forked["error"].is_null(), "{forked:#}");
    let view = AccessController::audience_only().view_for_subject(None);
    let id = json!("test");

    let mut methods = console_methods();
    methods.extend_from_slice(&[
        "meta/run_flow",
        "meta/fork_member",
        "meta/cross_mob/wire_local",
        "meta/workgraph/goal/create",
        "meta/workgraph/attention/reassign",
        "meta/workgraph/attention/break_glass_reassign",
    ]);
    let mut paths = std::collections::BTreeSet::new();
    let mut unrefused = Vec::new();
    let mut refused = Vec::new();
    for method in methods {
        for field in console_method_member_fields(method).unwrap_or_default() {
            if field.path.len() < 2 {
                continue;
            }
            paths.insert(field.path.join("/"));
            let well_formed = naming(field.path, SIDE);
            let answer = console_rpc_as(&app, Some(&bob), method, well_formed.clone()).await;
            if refused_as_malformed(&answer, &id) {
                refused.push(format!("{method} {well_formed}: {answer}"));
            }
            for depth in 1..field.path.len() {
                let Some(params) = malformed_at(&well_formed, field.path, depth, SIDE) else {
                    continue;
                };
                let answer = console_rpc_as(&app, Some(&bob), method, params.clone()).await;
                if !refused_as_malformed(&answer, &id) {
                    unrefused.push(format!("{method} {params}: {answer}"));
                }
                let mut resolved = params.clone();
                let resolution = console_rpc_resolve_members(Some(&view), method, &mut resolved);
                if !matches!(&resolution, Err(error) if error.code == -32602) {
                    unrefused.push(format!("{method} {params}: resolved as {resolution:?}"));
                }
            }
        }
    }
    assert_eq!(
        paths.iter().map(String::as_str).collect::<Vec<_>>(),
        [
            "agents/*/name",
            "member/identity",
            "operations/*/edge/a/identity",
            "operations/*/edge/b/identity",
            "owned_wiring/*/a",
            "owned_wiring/*/b",
            "role_targets/*/*",
            "target/identity",
        ],
        "the paths below the top level at which a method's params name an agent"
    );
    assert!(unrefused.is_empty(), "{unrefused:#?}");
    assert!(refused.is_empty(), "{refused:#?}");
    Ok(())
}

/// The methods of the console's method families, and the console's own
/// methods beside them.
fn console_methods() -> Vec<&'static str> {
    let mut methods: Vec<&'static str> = Vec::new();
    methods.extend_from_slice(crate::rpc::mob_methods::MEMBER_DECLARATION_METHODS);
    methods.extend_from_slice(crate::rpc::tool_grant_methods::TOOL_GRANT_METHODS);
    methods.extend_from_slice(crate::rpc::identity_methods::IDENTITY_FAMILY_METHODS);
    methods.extend_from_slice(crate::mob_announcements::rpc::ANNOUNCEMENT_METHODS);
    methods.extend_from_slice(&[
        crate::rpc::topology_methods::TOPOLOGY_QUERY_METHOD,
        crate::rpc::topology_methods::TOPOLOGY_PLAN_METHOD,
        crate::rpc::topology_methods::TOPOLOGY_APPLY_METHOD,
        crate::rpc::topology_methods::TOPOLOGY_OPERATION_METHOD,
        crate::rpc::topology_methods::TOPOLOGY_AUDIT_METHOD,
        crate::console_voice::VOICE_OPEN_METHOD,
        crate::console_voice::VOICE_READINESS_METHOD,
        crate::console_voice::VOICE_CLOSE_METHOD,
        crate::console_voice::VOICE_ANSWER_RECEIVED_METHOD,
        crate::console_voice::VOICE_REPLACEMENT_METHOD,
        crate::console_voice::VOICE_ACTIVITY_METHOD,
        crate::console_voice::VOICE_CONTEXT_STATUS_METHOD,
        crate::console_voice::VOICE_CAPTIONS_METHOD,
        "meta/live/status",
        "meta/live/close",
        "meta/live/refresh",
        "meta/live/interrupt",
        "meta/live/playback_owner/register",
        "meta/live/playback_owner/revoke",
        "live/webrtc/answer",
        "meta/console/send",
        "meta/console/recover_send",
        "meta/console/query_timeline",
        "meta/mob_events/query",
        "meta/mob_events/subscribe",
        "meta/decisions/list",
        "meta/decisions/get",
        "meta/decisions/rendering",
        "meta/decisions/withdraw",
        "meta/workgraph/items/list",
        "meta/access/get",
        "meta/get_member",
        "meta/list_members",
        "meta/retire_member",
        "meta/status",
        "meta/capabilities",
        crate::rpc::CONSOLE_WHOAMI_METHOD,
        crate::rpc::CONSOLE_AUDIT_METHOD,
        crate::rpc::CONSOLE_ATTENTION_METHOD,
        crate::rpc::CONSOLE_ATTENTION_DISMISS_METHOD,
        crate::rpc::CONSOLE_REVOKE_SESSIONS_METHOD,
        crate::rpc::CONSOLE_SIGN_OUT_METHOD,
    ]);
    methods
}

/// Malformed params are refused before any method runs. On
/// `POST /console/rpc`, every method answers malformed params with the one
/// invalid-params refusal and nothing else (no result, no data, no name),
/// with or without a request id; the same method with well-formed params,
/// or with none, is not answered that refusal. `POST /console/rpc/multipart`
/// refuses the same way. A batch of requests is no request to either route.
#[tokio::test]
async fn malformed_params_are_refused_before_any_method_runs() -> TestResult {
    const NAME: &str = "alpha-own";
    let (_temp_dir, runtime) = runtime("malformed-params-routes").await?;
    let app = console_json_router_with_state(enforced_console(&runtime, None));
    let bob = bearer(bob_claims());
    seat(&runtime, NAME, &[]).await?;
    let id = json!("test");
    let malformed_values = || {
        [
            malformed(NAME),
            json!([]),
            json!(NAME),
            json!(7),
            json!(true),
        ]
    };
    let mut unrefused = Vec::new();
    for method in console_methods() {
        for params in malformed_values() {
            let answer = console_rpc_as(&app, Some(&bob), method, params.clone()).await;
            if !refused_as_malformed(&answer, &id) {
                unrefused.push(format!("{method} {params}: {answer}"));
            }
        }
        let (status, no_id) = rpc_body(
            &app,
            &bob,
            &json!({ "jsonrpc": "2.0", "method": method, "params": malformed(NAME) }),
        )
        .await;
        if status != StatusCode::OK || !refused_as_malformed(&no_id, &Value::Null) {
            unrefused.push(format!("{method} without an id: {status} {no_id}"));
        }
    }
    for method in ["meta/console/send", "meta/blob/upload", "meta/get_member"] {
        for params in malformed_values() {
            let payload =
                json!({ "jsonrpc": "2.0", "id": "test", "method": method, "params": params });
            let answer = multipart_rpc(&app, &bob, &payload).await;
            if !refused_as_malformed(&answer, &id) {
                unrefused.push(format!("multipart {method} {params}: {answer}"));
            }
        }
    }
    assert!(unrefused.is_empty(), "{unrefused:#?}");

    // With well-formed params, and with none, no method is answered that
    // refusal. (The two methods that end the caller's own login are left
    // out: the caller makes the requests after them.)
    let mut refused = Vec::new();
    for method in console_methods() {
        if matches!(
            method,
            crate::rpc::CONSOLE_REVOKE_SESSIONS_METHOD | crate::rpc::CONSOLE_SIGN_OUT_METHOD
        ) {
            continue;
        }
        for params in [json!({}), Value::Null] {
            let answer = console_rpc_as(&app, Some(&bob), method, params.clone()).await;
            if refused_as_malformed(&answer, &id) {
                refused.push(format!("{method} {params}: {answer}"));
            }
        }
    }
    assert!(refused.is_empty(), "{refused:#?}");
    let member = console_rpc_as(
        &app,
        Some(&bob),
        "meta/get_member",
        json!({ "member_id": NAME }),
    )
    .await;
    assert!(
        member["error"].is_null(),
        "a request with well-formed params is handled: {member:#}"
    );
    let capabilities = console_rpc_as(&app, Some(&bob), "meta/capabilities", Value::Null).await;
    assert!(
        capabilities["result"]["methods"].is_array(),
        "a request without params is handled: {capabilities:#}"
    );

    let batch = json!([{ "jsonrpc": "2.0", "id": "test", "method": "meta/capabilities" }]);
    let (status, answer) = rpc_body(&app, &bob, &batch).await;
    assert_eq!(
        (status, &answer["error"]["code"], &answer["result"]),
        (StatusCode::OK, &json!(-32600), &Value::Null),
        "a batch is no request: {answer:#}"
    );
    let answer = multipart_rpc(&app, &bob, &batch).await;
    assert_eq!(
        (&answer["error"]["code"], &answer["result"]),
        (&json!(-32600), &Value::Null),
        "a batch is no request to the multipart route: {answer:#}"
    );
    Ok(())
}

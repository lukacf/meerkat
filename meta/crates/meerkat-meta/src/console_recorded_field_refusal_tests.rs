//! The console methods that seat a member under a name or record something
//! under one (a new member, a fork's child, the agents of a create, a
//! WorkGraph target) answer a caller the access rules refuse with the rules'
//! refusal, alike for a name a member hidden from the caller holds and for
//! a name no member holds, each spelled as the caller spelled it. A caller
//! the rules allow is answered `unknown identity` for the hidden name (for
//! a topology apply's end, in the topology's words for an end no member
//! holds), and nothing is seated or recorded under it for either caller.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use meerkat_core::types::SessionId;
use serde_json::{Value, json};

use super::console_private_side_chat_tests::{
    alice_claims, bearer, bob_claims, enforced_console, fork_on_console, root_claims,
};
use super::console_side_chat_tests::{persistent_gateway_runtime_in, seeded_member};
use super::*;
use crate::access::{AccessControlConfig, AccessRule};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const ALICE: &str = "alice@example.test";
const BOB: &str = "bob@example.test";
/// The member every row forks from, visible to both people.
const SOURCE: &str = "alpha";
/// A side chat private to Alice: hidden from Bob.
const HIDDEN: &str = "alpha-side";
/// A name no member holds and no record knows.
const NOBODY: &str = "never-recorded";

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

/// Enforced rules: Alice may do everything to every agent, with or without
/// one; Bob is given `bob`.
fn rules_with(bob: Vec<AccessRule>) -> AccessControlConfig {
    let mut rules = vec![AccessRule {
        id: "alice-does-everything".to_string(),
        subjects: vec![ALICE.to_string()],
        actions: vec!["*".to_string()],
        ..AccessRule::default()
    }];
    rules.extend(bob);
    AccessControlConfig {
        enabled: true,
        admins: vec!["root@example.test".to_string()],
        rules,
        ..AccessControlConfig::default()
    }
}

/// Bob may view every agent and nothing else: the rules refuse him a seat,
/// a create and a WorkGraph change.
fn bob_is_refused() -> AccessControlConfig {
    rules_with(vec![AccessRule {
        id: "bob-views-every-agent".to_string(),
        subjects: vec![BOB.to_string()],
        actions: vec!["agent.view".to_string()],
        agents: vec!["*".to_string()],
        ..AccessRule::default()
    }])
}

/// Bob may do everything, with or without an agent.
fn bob_is_allowed() -> AccessControlConfig {
    rules_with(vec![AccessRule {
        id: "bob-does-everything".to_string(),
        subjects: vec![BOB.to_string()],
        actions: vec!["*".to_string()],
        ..AccessRule::default()
    }])
}

/// Bob may view every agent, and seat a member under exactly `name`.
fn bob_may_seat_exactly(name: &str) -> AccessControlConfig {
    let mut config = bob_is_refused();
    config.rules.push(AccessRule {
        id: "bob-seats-one-name".to_string(),
        subjects: vec![BOB.to_string()],
        actions: vec!["agent.spawn".to_string()],
        agents: vec![name.to_string()],
        ..AccessRule::default()
    });
    config
}

/// One runtime with [`SOURCE`] seated and [`HIDDEN`] forked from it by
/// Alice, and the console of that runtime under `config` for each of
/// `configs`, in order. Every console requires sign-in.
async fn consoles(
    mob_id: &str,
    configs: Vec<AccessControlConfig>,
) -> TestResult<(tempfile::TempDir, MobRuntime, Vec<Router>)> {
    let state = tempfile::tempdir()?;
    let runtime = persistent_gateway_runtime_in(state.path(), mob_id, "", "").await?;
    seeded_member(&runtime, "worker", SOURCE, &[]).await?;
    let mut apps = Vec::new();
    for config in configs {
        apps.push(console_json_router_with_state(enforced_console(
            &runtime,
            Some(AccessController::new(config)?),
        )));
    }
    let first = apps.first().ok_or("at least one console")?;
    let forked = fork_on_console(first, Some(&bearer(alice_claims())), SOURCE, HIDDEN).await;
    if !forked["error"].is_null() {
        return Err(format!("the side chat was not forked: {forked}").into());
    }
    Ok((state, runtime, apps))
}

/// The requests of one method that name `name` in its recorded field: once,
/// and where the method takes one, also under a second spelling of the same
/// member.
fn requests_naming(method: &str, name: &str) -> Vec<Value> {
    match method {
        "meta/ensure_member" => vec![
            json!({ "role": "worker", "agent_identity": name }),
            json!({ "role": "worker", "agent_identity": name, "member_id": name }),
        ],
        "meta/spawn_helper" => {
            let helper = |second: bool| {
                let mut params = json!({
                    "agent_identity": name,
                    "task": "Say ok.",
                    "options": { "role": "worker" },
                    "result_label": "answer",
                    "max_text_bytes": 1024,
                });
                if second {
                    params["member_id"] = json!(name);
                }
                params
            };
            vec![helper(false), helper(true)]
        }
        "meta/attach_existing_session" => {
            let session = SessionId::new().to_string();
            vec![
                json!({ "role": "worker", "agent_identity": name, "session_id": session }),
                json!({
                    "role": "worker",
                    "agent_identity": name,
                    "agent_id": name,
                    "session_id": session,
                }),
            ]
        }
        "meta/fork_member" => vec![json!({ "source_member_id": SOURCE, "agent_identity": name })],
        "meta/fork_helper" => vec![json!({
            "source_member_id": SOURCE,
            "agent_identity": name,
            "task": "Say ok.",
            "result_label": "answer",
            "max_text_bytes": 1024,
        })],
        crate::rpc::identity_methods::IDENTITY_CREATE => vec![
            json!({ "agents": [{ "name": name, "profile": "worker" }] }),
            json!({ "agents": [
                { "name": "a-new-name", "profile": "worker" },
                { "name": name, "profile": "worker" },
            ]}),
        ],
        "meta/workgraph/goal/create" => vec![json!({
            "title": "A goal",
            "target": { "identity": name },
        })],
        "meta/workgraph/attention/reassign" | "meta/workgraph/attention/break_glass_reassign" => {
            vec![json!({
                "item_id": "item-1",
                "target": { "identity": name },
                "reason": "handover",
            })]
        }
        other => panic!("no request for {other}"),
    }
}

/// For each of `methods`, Bob's answers on `app` to the same request naming
/// [`HIDDEN`] and naming [`NOBODY`] must be the same bytes once the name is
/// masked, and the answer for [`NOBODY`] must be the rules' refusal of
/// `action`. What differs is added to `wrong`.
async fn compare_refused(app: &Router, methods: &[&str], action: &str, wrong: &mut Vec<String>) {
    let bob = bearer(bob_claims());
    for method in methods {
        let hidden_requests = requests_naming(method, HIDDEN);
        let absent_requests = requests_naming(method, NOBODY);
        for (index, (hidden_request, absent_request)) in
            hidden_requests.into_iter().zip(absent_requests).enumerate()
        {
            let hidden = rpc_answer(app, &bob, method, hidden_request).await;
            let absent = rpc_answer(app, &bob, method, absent_request).await;
            if index == 0
                && (absent["error"]["code"] != json!(ACCESS_DENIED_RPC_CODE)
                    || absent["error"]["message"] != json!(format!("access denied: {action}"))
                    || !absent["result"].is_null())
            {
                wrong.push(format!(
                    "{method}: a name no member holds is not refused {action} by the rules: \
                     {absent}"
                ));
            }
            if hidden.to_string().replace(HIDDEN, "<member>")
                != absent.to_string().replace(NOBODY, "<member>")
            {
                wrong.push(format!(
                    "{method} (request {index}): the hidden name is not answered as a name no \
                     member holds: hidden {hidden}, absent {absent}"
                ));
            }
        }
    }
}

/// Whether the mob of `runtime` seats a member named `name`.
async fn seated(runtime: &MobRuntime, name: &str) -> TestResult<bool> {
    Ok(runtime
        .handle()
        .get_member(&AgentIdentity::from(name))
        .await?
        .is_some())
}

const SEAT_METHODS: [&str; 3] = [
    "meta/ensure_member",
    "meta/spawn_helper",
    "meta/attach_existing_session",
];
const FORK_METHODS: [&str; 2] = ["meta/fork_member", "meta/fork_helper"];
const WORKGRAPH_METHODS: [&str; 3] = [
    "meta/workgraph/goal/create",
    "meta/workgraph/attention/reassign",
    "meta/workgraph/attention/break_glass_reassign",
];

/// A seat under a name: an ensure, a helper spawn, an attach.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_seat_under_a_hidden_name_is_refused_by_the_rules_as_under_a_name_no_member_holds()
-> TestResult {
    let (_state, _runtime, apps) = consoles("seat-refused", vec![bob_is_refused()]).await?;
    let mut wrong = Vec::new();
    compare_refused(&apps[0], &SEAT_METHODS, "agent.spawn", &mut wrong).await;
    assert!(wrong.is_empty(), "a seat under a name: {wrong:#?}");
    Ok(())
}

/// A fork whose child takes the name.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fork_whose_child_takes_a_hidden_name_is_refused_by_the_rules_as_for_a_name_no_member_holds()
-> TestResult {
    let (_state, _runtime, apps) = consoles("fork-refused", vec![bob_is_refused()]).await?;
    let mut wrong = Vec::new();
    compare_refused(&apps[0], &FORK_METHODS, "agent.spawn", &mut wrong).await;
    assert!(wrong.is_empty(), "a fork's child: {wrong:#?}");
    Ok(())
}

/// A create of long-term agents, one of them under the name.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_agent_create_under_a_hidden_name_is_refused_by_the_rules_as_under_a_name_no_agent_holds()
-> TestResult {
    let (_state, _runtime, apps) = consoles("create-refused", vec![bob_is_refused()]).await?;
    let mut wrong = Vec::new();
    compare_refused(
        &apps[0],
        &[crate::rpc::identity_methods::IDENTITY_CREATE],
        crate::access::ACTION_AGENT_IDENTITY_CREATE,
        &mut wrong,
    )
    .await;
    assert!(wrong.is_empty(), "an agent create: {wrong:#?}");
    Ok(())
}

/// A WorkGraph goal or attention binding aimed at the name, in its identity
/// form.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_workgraph_target_naming_a_hidden_member_is_refused_by_the_rules_as_a_name_no_member_holds()
-> TestResult {
    let (_state, _runtime, apps) = consoles("workgraph-refused", vec![bob_is_refused()]).await?;
    let mut wrong = Vec::new();
    compare_refused(
        &apps[0],
        &WORKGRAPH_METHODS,
        crate::access::ACTION_WORKGRAPH_MANAGE,
        &mut wrong,
    )
    .await;
    assert!(wrong.is_empty(), "a WorkGraph target: {wrong:#?}");
    Ok(())
}

/// What stays: a caller the rules allow, and a caller of a console whose
/// rules are off, is answered `unknown identity` for the hidden name by
/// every one of these methods, with the name as the caller spelled it and
/// nothing else in the error; the hidden member stays as it was.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_caller_the_rules_allow_is_answered_unknown_identity_for_a_hidden_name_as_before()
-> TestResult {
    let (_state, runtime, apps) = consoles(
        "allowed-unknown",
        vec![bob_is_allowed(), AccessControlConfig::default()],
    )
    .await?;
    let bob = bearer(bob_claims());
    let owner_of_hidden = |runtime: &MobRuntime| {
        let handle = runtime.handle();
        async move {
            handle
                .list_members()
                .await
                .into_iter()
                .find(|member| member.agent_identity.as_str() == HIDDEN)
                .and_then(|member| {
                    member
                        .labels
                        .get(super::console_private_side_chat_tests::PRIVATE_TO)
                        .cloned()
                })
        }
    };
    let owner_before = owner_of_hidden(&runtime).await;
    assert!(
        owner_before.is_some(),
        "the hidden member is private to a person"
    );
    let expected = serde_json::to_value(unknown_identity_rpc_error(HIDDEN))?;
    let mut wrong = Vec::new();
    let every_method: Vec<&str> = SEAT_METHODS
        .iter()
        .chain(&FORK_METHODS)
        .chain(&WORKGRAPH_METHODS)
        .copied()
        .chain([crate::rpc::identity_methods::IDENTITY_CREATE])
        .collect();
    for (console, app) in ["rules that allow", "rules that are off"].iter().zip(&apps) {
        for method in &every_method {
            for request in requests_naming(method, HIDDEN) {
                let answer = rpc_answer(app, &bob, method, request).await;
                if answer["error"] != expected || !answer["result"].is_null() {
                    wrong.push(format!("{console}, {method}: {answer}"));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "an allowed caller: {wrong:#?}");
    assert_eq!(
        owner_of_hidden(&runtime).await,
        owner_before,
        "the hidden member stays its owner's"
    );
    Ok(())
}

/// A topology apply that wires `name` to the source member.
fn apply_naming(name: &str) -> Value {
    apply_under(name, &format!("wire-{name}"))
}

/// A topology apply that wires `name` to the source member, under the
/// idempotency key `key`.
fn apply_under(name: &str, key: &str) -> Value {
    json!({
        "expected_revision": 0,
        "idempotency_key": key,
        "operations": [{
            "action": "connect",
            "edge": { "a": { "identity": name }, "b": { "identity": SOURCE } },
        }],
    })
}

/// Bob's answers on `app` to a topology apply naming [`HIDDEN`] and naming
/// [`NOBODY`], each with the name masked.
async fn apply_answers(app: &Router) -> (String, String) {
    let bob = bearer(bob_claims());
    let method = crate::rpc::topology_methods::TOPOLOGY_APPLY_METHOD;
    let hidden = rpc_answer(app, &bob, method, apply_naming(HIDDEN)).await;
    let absent = rpc_answer(app, &bob, method, apply_naming(NOBODY)).await;
    (
        hidden.to_string().replace(HIDDEN, "<member>"),
        absent.to_string().replace(NOBODY, "<member>"),
    )
}

/// `config` with one more rule for Bob: `actions` on `agents`.
fn bob_also(
    mut config: AccessControlConfig,
    id: &str,
    actions: &[&str],
    agents: &[&str],
) -> AccessControlConfig {
    config.rules.push(AccessRule {
        id: id.to_string(),
        subjects: vec![BOB.to_string()],
        actions: actions.iter().map(ToString::to_string).collect(),
        agents: agents.iter().map(ToString::to_string).collect(),
        ..AccessRule::default()
    });
    config
}

/// One runtime with [`SOURCE`] seated and [`HIDDEN`] forked from it by
/// Alice, its console under `config` with topology control by `controller`,
/// and that control's handle. The console requires sign-in.
async fn topology_console(
    mob_id: &str,
    config: AccessControlConfig,
    controller: crate::topology_control::TopologyController,
) -> TestResult<(
    tempfile::TempDir,
    MobRuntime,
    Router,
    crate::topology_control::TopologyRuntimeHandle,
)> {
    let state = tempfile::tempdir()?;
    let runtime = persistent_gateway_runtime_in(state.path(), mob_id, "", "").await?;
    seeded_member(&runtime, "worker", SOURCE, &[]).await?;
    let topology = crate::topology_control::TopologyRuntimeHandle::new(
        runtime.handle(),
        None,
        std::sync::Arc::new(tokio::sync::RwLock::new(std::collections::BTreeSet::new())),
        controller,
        None,
        false,
    );
    let mut console = enforced_console(&runtime, Some(AccessController::new(config)?));
    console.topology = Some(topology.clone());
    let app = console_json_router_with_state(console);
    let forked = fork_on_console(&app, Some(&bearer(alice_claims())), SOURCE, HIDDEN).await;
    if !forked["error"].is_null() {
        return Err(format!("the side chat was not forked: {forked}").into());
    }
    Ok((state, runtime, app, topology))
}

/// The page of the topology's operation records the holder of `bearer`
/// reads on `app`.
async fn operation_records_page(app: &Router, bearer: &str) -> TestResult<Value> {
    let answer = rpc_answer(
        app,
        bearer,
        crate::rpc::topology_methods::TOPOLOGY_AUDIT_METHOD,
        json!({}),
    )
    .await;
    if !answer["error"].is_null() {
        return Err(format!("the operation records were not read: {answer}").into());
    }
    Ok(answer["result"].clone())
}

/// The names of the ends of every operation record on `page`.
fn page_endpoints(page: &Value) -> Vec<String> {
    let mut names = Vec::new();
    for record in page["records"].as_array().into_iter().flatten() {
        for operation in record["operations"].as_array().into_iter().flatten() {
            for end in ["a", "b"] {
                if let Some(name) = operation["edge"][end]["identity"].as_str() {
                    names.push(name.to_string());
                }
            }
        }
    }
    names
}

/// A topology apply on a console with topology control: a caller the rules
/// refuse reads the rules' refusal for an endpoint a hidden member holds as
/// for a name no member holds. Each refused apply is kept in the topology's
/// operation records as a denied one that holds no operation: neither name
/// is stored, and nothing derived from one (the two names under one
/// idempotency key leave records of one operation id).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_topology_apply_naming_a_hidden_endpoint_is_refused_by_the_rules_as_a_name_no_member_holds()
-> TestResult {
    let (_state, _runtime, app, topology) = topology_console(
        "apply-refused",
        bob_is_refused(),
        crate::topology_control::TopologyController::default(),
    )
    .await?;

    let (hidden, absent) = apply_answers(&app).await;
    assert!(
        absent.contains("\"code\":-32030") && absent.contains("access denied: topology."),
        "a name no member holds is refused by the rules: {absent}"
    );
    assert_eq!(
        hidden, absent,
        "the hidden endpoint against a name no member holds"
    );
    let bob = bearer(bob_claims());
    let method = crate::rpc::topology_methods::TOPOLOGY_APPLY_METHOD;
    for name in [HIDDEN, NOBODY] {
        rpc_answer(&app, &bob, method, apply_under(name, "one-key")).await;
    }

    let records = topology
        .controller()
        .all_operation_records_after(None)
        .await
        .map_err(|error| error.to_string())?
        .records;
    assert_eq!(
        records.len(),
        4,
        "each refused apply leaves one record, the hidden member's alike: {records:?}"
    );
    assert!(
        records.iter().all(|record| {
            record.operations.is_empty()
                && record.status == crate::topology_control::TopologyOperationRecordStatus::Denied
        }),
        "each is a denied record that holds no operation: {records:?}"
    );
    let stored = serde_json::to_string(&records)?;
    assert!(
        !stored.contains(HIDDEN) && !stored.contains(NOBODY),
        "no record stores a name: {stored}"
    );
    let under_one_key: Vec<&str> = records
        .iter()
        .skip(2)
        .map(|record| record.operation_id.as_str())
        .collect();
    assert_eq!(
        under_one_key.first(),
        under_one_key.last(),
        "the two names under one idempotency key leave records of one operation id"
    );
    Ok(())
}

/// A caller the rules refuse the apply, who may read the topology's
/// operation records for some other agent, reads the same count of records
/// after an apply naming an endpoint a hidden member holds as after one
/// naming a name no member holds.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_topology_apply_moves_the_operation_records_alike_for_a_hidden_endpoint_and_a_name_no_member_holds()
-> TestResult {
    let (_state, _runtime, app, _topology) = topology_console(
        "apply-record-count",
        bob_also(
            bob_is_refused(),
            "bob-reads-the-records-of-another-agent",
            &["topology.audit"],
            &["elsewhere"],
        ),
        crate::topology_control::TopologyController::default(),
    )
    .await?;
    let bob = bearer(bob_claims());
    let method = crate::rpc::topology_methods::TOPOLOGY_APPLY_METHOD;
    let latest = |page: Value| page["latest_seq"].as_u64().ok_or("the page's latest_seq");

    let before = latest(operation_records_page(&app, &bob).await?)?;
    let refused = rpc_answer(&app, &bob, method, apply_naming(NOBODY)).await;
    assert_eq!(
        refused["error"]["code"],
        json!(ACCESS_DENIED_RPC_CODE),
        "{refused}"
    );
    let after_a_free_name = latest(operation_records_page(&app, &bob).await?)?;
    rpc_answer(&app, &bob, method, apply_naming(HIDDEN)).await;
    let page = operation_records_page(&app, &bob).await?;
    let after_a_hidden_name = latest(page.clone())?;

    assert_eq!(
        after_a_free_name - before,
        1,
        "control: the refused apply of a name no member holds leaves one record"
    );
    assert_eq!(
        after_a_hidden_name - after_a_free_name,
        after_a_free_name - before,
        "the refused apply of a hidden endpoint moves the records as that of a name no member \
         holds"
    );
    assert!(
        page_endpoints(&page).is_empty(),
        "the caller is shown neither record: {page}"
    );
    Ok(())
}

/// A caller who may view the topology and read its operation records, and
/// may not wire a name, reads the rules' refusal, which carries the name as
/// sent, for an endpoint a hidden member holds as for a name no member
/// holds, and its page of the records shows the same after either: neither
/// record, both counted. The two records hold no operation and are shown
/// to an access administrator, and to nobody for seeing the member: not to
/// the person the hidden member is private to. A refused apply whose ends
/// all name agents the records know keeps its record with its names, shown
/// as before.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_topology_apply_of_a_name_the_records_do_not_know_is_recorded_without_it_for_administrators_only()
-> TestResult {
    let mut config = bob_also(
        bob_also(
            bob_is_refused(),
            "bob-reads-the-topology",
            &["topology.view", "topology.audit"],
            &["*"],
        ),
        "bob-wires-the-source",
        &["topology.connect"],
        &[SOURCE],
    );
    // Alice, whom the hidden member is private to, may do everything to
    // every agent and read every record of the topology, and administers
    // no access.
    if let Some(alice) = config.rules.first_mut() {
        alice.actions = vec!["agent.*".to_string(), "topology.*".to_string()];
    }
    let (_state, runtime, app, _topology) = topology_console(
        "apply-record-readers",
        config,
        crate::topology_control::TopologyController::default(),
    )
    .await?;
    let bob = bearer(bob_claims());
    let method = crate::rpc::topology_methods::TOPOLOGY_APPLY_METHOD;

    let hidden = rpc_answer(&app, &bob, method, apply_naming(HIDDEN)).await;
    let after_hidden = operation_records_page(&app, &bob).await?;
    let absent = rpc_answer(&app, &bob, method, apply_naming(NOBODY)).await;
    let after_absent = operation_records_page(&app, &bob).await?;
    let absent = absent.to_string().replace(NOBODY, "<member>");
    assert!(
        absent.contains("\"code\":-32030")
            && absent.contains("access denied: topology.connect")
            && absent.contains("\"resource\":\"<member>\""),
        "a name no member holds is refused by the rules, by name: {absent}"
    );
    assert_eq!(
        hidden.to_string().replace(HIDDEN, "<member>"),
        absent,
        "the hidden endpoint against a name no member holds"
    );
    assert_eq!(
        (
            after_hidden["records"].clone(),
            after_hidden["latest_seq"].clone(),
            after_absent["records"].clone(),
            after_absent["latest_seq"].clone(),
        ),
        (json!([]), json!(1), json!([]), json!(2)),
        "the caller's page shows neither record and counts each: {after_hidden} {after_absent}"
    );

    let owners = operation_records_page(&app, &bearer(alice_claims())).await?;
    assert_eq!(
        owners["records"],
        json!([]),
        "seeing the member shows nobody its record: {owners}"
    );
    let administrators = operation_records_page(&app, &bearer(root_claims())).await?;
    let shown = administrators["records"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        shown.len(),
        2,
        "an administrator is shown both: {administrators}"
    );
    assert!(
        shown
            .iter()
            .all(|record| record["operations"] == json!([]) && record["status"] == json!("denied")),
        "each is a denied record that holds no operation: {administrators}"
    );
    let page = administrators.to_string();
    assert!(
        !page.contains(HIDDEN) && !page.contains(NOBODY),
        "no record stores a name: {page}"
    );

    // What stays: both ends name agents the records know.
    seeded_member(&runtime, "worker", "beta", &[]).await?;
    let known = rpc_answer(&app, &bob, method, apply_naming("beta")).await;
    assert_eq!(known["error"]["data"]["resource"], json!("beta"), "{known}");
    let callers = page_endpoints(&operation_records_page(&app, &bob).await?);
    assert!(
        callers.iter().any(|name| name == "beta") && callers.iter().any(|name| name == SOURCE),
        "the caller is shown its refused apply between two agents the records know, by name: \
         {callers:?}"
    );
    Ok(())
}

/// A person the rules grant the administration of access, from whom the
/// member is hidden all the same, is shown the records of both refused
/// applies: each holds no operation and no name.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_access_administrator_by_rule_reads_the_record_of_a_refused_topology_apply_without_a_name()
-> TestResult {
    let config = bob_also(
        bob_also(
            bob_is_refused(),
            "bob-administers-access",
            &["access.admin"],
            &[],
        ),
        "bob-reads-the-records-of-another-agent",
        &["topology.audit"],
        &["elsewhere"],
    );
    let (_state, _runtime, app, _topology) = topology_console(
        "apply-record-rule-administrator",
        config,
        crate::topology_control::TopologyController::default(),
    )
    .await?;
    let bob = bearer(bob_claims());
    let member = |name: &str| json!({ "identity": name });
    assert_eq!(
        rpc_answer(&app, &bob, "meta/get_member", member(HIDDEN))
            .await
            .to_string()
            .replace(HIDDEN, "<member>"),
        rpc_answer(&app, &bob, "meta/get_member", member(NOBODY))
            .await
            .to_string()
            .replace(NOBODY, "<member>"),
        "control: the member is hidden from this administrator"
    );

    let (hidden, absent) = apply_answers(&app).await;
    assert!(
        absent.contains("\"code\":-32030") && absent.contains("access denied: topology."),
        "a name no member holds is refused by the rules: {absent}"
    );
    assert_eq!(
        hidden, absent,
        "the hidden endpoint against a name no member holds"
    );

    let page = operation_records_page(&app, &bob).await?;
    let shown = page["records"].as_array().cloned().unwrap_or_default();
    assert_eq!(shown.len(), 2, "the administrator is shown both: {page}");
    assert!(
        shown
            .iter()
            .all(|record| record["operations"] == json!([]) && record["status"] == json!("denied")),
        "each is a denied record that holds no operation: {page}"
    );
    let page = page.to_string();
    assert!(
        !page.contains(HIDDEN) && !page.contains(NOBODY),
        "no record stores a name: {page}"
    );
    Ok(())
}

/// When the record of a refused apply cannot be written, the caller reads
/// that failure for an endpoint a hidden member holds as for a name no
/// member holds.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_topology_apply_whose_record_cannot_be_written_answers_alike_for_a_hidden_endpoint()
-> TestResult {
    let records = tempfile::tempdir()?;
    let store = records.path().join("topology");
    let controller = crate::topology_control::TopologyController::load_or_default(
        crate::topology_control::TopologyControlPolicy::default(),
        store.join("intent.json"),
    )
    .map_err(|error| error.to_string())?;
    let (_state, _runtime, app, _topology) =
        topology_console("apply-record-write-fails", bob_is_refused(), controller).await?;
    // The directory of the records becomes a file: no record can be written.
    std::fs::remove_dir_all(&store)?;
    std::fs::write(&store, b"")?;

    let (hidden, absent) = apply_answers(&app).await;
    assert!(
        absent.contains("\"error\"") && !absent.contains("access denied"),
        "control: the caller reads the failed write, not the rules' refusal: {absent}"
    );
    assert_eq!(
        hidden, absent,
        "the hidden endpoint against a name no member holds"
    );
    Ok(())
}

/// Rules that allow the topology's actions under exactly the name a hidden
/// member holds do not allow the name no member holds that stands in for
/// it: the caller reads the rules' refusal, as for a name no member holds.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_caller_allowed_the_topology_actions_under_exactly_a_hidden_name_reads_the_rules_refusal()
-> TestResult {
    let (_state, _runtime, app, _topology) = topology_console(
        "apply-exact-name",
        bob_also(
            bob_is_refused(),
            "bob-wires-two-names",
            &["topology.*"],
            &[HIDDEN, SOURCE],
        ),
        crate::topology_control::TopologyController::default(),
    )
    .await?;
    let (hidden, absent) = apply_answers(&app).await;
    assert!(
        hidden.contains("\"code\":-32030") && hidden.contains("access denied: topology."),
        "the hidden endpoint is refused by the rules: {hidden}"
    );
    assert_eq!(
        hidden, absent,
        "the hidden endpoint against a name no member holds"
    );
    Ok(())
}

/// A topology apply on a console with no topology control: the method is
/// not served, and every caller, refused by the rules or allowed, reads
/// that for an endpoint a hidden member holds as for a name no member
/// holds.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_topology_apply_naming_a_hidden_endpoint_is_answered_as_a_name_no_member_holds_where_it_is_not_served()
-> TestResult {
    let (_state, _runtime, apps) =
        consoles("apply-not-served", vec![bob_is_refused(), bob_is_allowed()]).await?;
    for (caller, app) in ["refused", "allowed"].iter().zip(&apps) {
        let (hidden, absent) = apply_answers(app).await;
        assert!(absent.contains("Method not found"), "{caller}: {absent}");
        assert_eq!(hidden, absent, "{caller}");
    }
    Ok(())
}

/// What stays: on a console with topology control a caller the rules allow
/// is answered, for an endpoint a hidden member holds, in the words the
/// topology answers an end no member holds in, under the name as sent, and
/// no operation is recorded.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_caller_the_rules_allow_is_answered_for_a_hidden_endpoint_in_the_words_of_an_end_no_member_holds()
-> TestResult {
    let (_state, _runtime, app, topology) = topology_console(
        "apply-allowed",
        bob_is_allowed(),
        crate::topology_control::TopologyController::default(),
    )
    .await?;

    let answer = rpc_answer(
        &app,
        &bearer(bob_claims()),
        crate::rpc::topology_methods::TOPOLOGY_APPLY_METHOD,
        apply_naming(HIDDEN),
    )
    .await;
    assert_eq!(
        answer["error"],
        serde_json::to_value(crate::rpc::topology_methods::member_not_found_rpc_error(
            HIDDEN
        ))?,
        "{answer}"
    );
    let records = topology
        .controller()
        .all_operation_records_after(None)
        .await
        .map_err(|error| error.to_string())?
        .records;
    assert!(records.is_empty(), "no operation is recorded: {records:?}");
    Ok(())
}

/// A console with no runtime of its own serves none of these methods. A
/// caller the rules refuse reads their refusal for a seat under a hidden
/// name as under a name no member holds; a caller they allow reads
/// `unknown identity` for the hidden name; and a topology apply, which the
/// shared check does not judge, is answered alike for both names to both
/// callers.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_console_with_no_runtime_answers_a_hidden_name_in_a_recorded_field_by_the_same_order()
-> TestResult {
    async fn answer(controller: &AccessController, method: &str, params: Value) -> Value {
        let view = controller.view_for_caller(Some(BOB), Some("person:bob"));
        handle_console_aggregator_rpc(
            None,
            JsonRpcRequest {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: Some(json!(1)),
                method: method.to_string(),
                params,
            },
            true,
            false,
            Some(controller),
            Some(&view),
            &crate::console_aggregator::ConsoleSendScope::Anonymous,
            None,
        )
        .await
    }
    let controller_under = |config: AccessControlConfig| -> TestResult<AccessController> {
        let controller = AccessController::new(config)?;
        controller.record_agent_attributes(crate::access::AgentResourceAttributes {
            identity: HIDDEN.to_string(),
            agent_id: None,
            role: None,
            labels: std::collections::BTreeMap::from([(
                crate::access::PRIVATE_TO_LABEL.to_string(),
                "person:alice".to_string(),
            )]),
        });
        Ok(controller)
    };
    let refused = controller_under(bob_is_refused())?;
    let allowed = controller_under(bob_is_allowed())?;
    assert!(
        refused
            .view_for_caller(Some(BOB), Some("person:bob"))
            .hides_agent(HIDDEN),
        "the member is hidden from the caller"
    );
    let seat = |name: &str| json!({ "role": "worker", "agent_identity": name });
    let masked = |answer: &Value, name: &str| answer.to_string().replace(name, "<member>");

    // A seat, for the caller the rules refuse.
    let hidden = answer(&refused, "meta/ensure_member", seat(HIDDEN)).await;
    let absent = answer(&refused, "meta/ensure_member", seat(NOBODY)).await;
    assert_eq!(
        absent["error"]["code"],
        json!(ACCESS_DENIED_RPC_CODE),
        "{absent}"
    );
    assert_eq!(masked(&hidden, HIDDEN), masked(&absent, NOBODY));

    // A seat, for the caller the rules allow.
    let hidden = answer(&allowed, "meta/ensure_member", seat(HIDDEN)).await;
    assert_eq!(
        hidden["error"],
        serde_json::to_value(unknown_identity_rpc_error(HIDDEN))?,
        "{hidden}"
    );

    // A topology apply, for both callers.
    for (caller, controller) in [("refused", &refused), ("allowed", &allowed)] {
        let method = crate::rpc::topology_methods::TOPOLOGY_APPLY_METHOD;
        let hidden = answer(controller, method, apply_naming(HIDDEN)).await;
        let absent = answer(controller, method, apply_naming(NOBODY)).await;
        assert_eq!(
            masked(&hidden, HIDDEN),
            masked(&absent, NOBODY),
            "{caller}: a topology apply"
        );
    }
    Ok(())
}

/// Every function that resolves the members of a request either gives the
/// refusal a recorded field is owed, after its access check, or hands the
/// resolution only methods that have no recorded field. A function that
/// calls the resolution and is not listed here fails the row until it is.
#[test]
fn every_caller_of_the_member_resolution_gives_the_recorded_refusal_or_names_no_recorded_field() {
    let source = include_str!("http_console.rs");
    let production = &source[..source
        .find("\nmod tests {")
        .expect("the inline test module")];
    // Each call site with the function that holds it and that function's
    // text from the call to its end.
    let lines: Vec<&str> = production.lines().collect();
    let function_at = |line: usize| {
        lines[..=line].iter().rev().find_map(|text| {
            let text = text
                .trim_start_matches("pub(crate) ")
                .trim_start_matches("pub ")
                .trim_start_matches("async ");
            text.strip_prefix("fn ")
                .and_then(|rest| rest.split(['(', '<']).next())
        })
    };
    let mut callers = std::collections::BTreeMap::new();
    for (index, text) in lines.iter().enumerate() {
        let code = text.trim_start();
        let calls = code.contains("console_rpc_resolve_members(")
            || code.contains("console_rpc_resolve_members_with(");
        if !calls || code.starts_with("//") || code.contains("fn console_rpc_resolve_members") {
            continue;
        }
        let function = function_at(index).expect("a call site is inside a function");
        let end = lines[index..]
            .iter()
            .position(|text| *text == "}")
            .map_or(lines.len(), |offset| index + offset);
        callers.insert(function, lines[index..end].join("\n"));
    }
    let gives_the_refusal = [
        "handle_console_aggregator_rpc",
        "handle_console_runtime_rpc_with_visibility",
    ];
    let names_no_recorded_field = [
        // The wrapper of the resolution itself.
        "console_rpc_resolve_members",
        // The voice verbs.
        "handle_console_voice_rpc",
        // The multipart send: `meta/console/send`.
        "console_rpc_multipart_answer",
    ];
    assert_eq!(
        callers.keys().copied().collect::<Vec<_>>(),
        {
            let mut expected: Vec<&str> = gives_the_refusal
                .iter()
                .chain(&names_no_recorded_field)
                .copied()
                .collect();
            expected.sort_unstable();
            expected
        },
        "the functions that resolve a request's members"
    );
    for function in gives_the_refusal {
        assert!(
            callers[function].contains(".recorded_refusal("),
            "{function} gives the refusal a recorded field is owed"
        );
    }
    // No method with a recorded field is a voice verb or the send.
    let recorded_methods = SEAT_METHODS
        .iter()
        .chain(&FORK_METHODS)
        .chain(&WORKGRAPH_METHODS)
        .copied()
        .chain([
            crate::rpc::identity_methods::IDENTITY_CREATE,
            crate::rpc::mob_methods::ADOPT_MEMBER_IDENTITY_DECLARATION,
            crate::rpc::topology_methods::TOPOLOGY_APPLY_METHOD,
        ]);
    let voice_verbs = [
        crate::console_voice::VOICE_OPEN_METHOD,
        crate::console_voice::VOICE_READINESS_METHOD,
        crate::console_voice::VOICE_CLOSE_METHOD,
        crate::console_voice::VOICE_ANSWER_RECEIVED_METHOD,
        crate::console_voice::VOICE_REPLACEMENT_METHOD,
        crate::console_voice::VOICE_ACTIVITY_METHOD,
        crate::console_voice::VOICE_CONTEXT_STATUS_METHOD,
        crate::console_voice::VOICE_CAPTIONS_METHOD,
    ];
    for method in recorded_methods {
        assert!(
            console_method_member_fields(method).is_some_and(|fields| fields
                .iter()
                .any(|field| field.kind == ConsoleMemberFieldKind::Recorded)),
            "{method} names a member in a recorded field"
        );
        assert!(
            method != "meta/console/send"
                && !voice_verbs.contains(&method)
                && !crate::console_voice::is_channel_method(method),
            "{method} is not a method of a caller that gives no refusal"
        );
    }
    assert!(
        callers["console_rpc_multipart_answer"].contains("\"meta/console/send\","),
        "the multipart send resolves the members of the send alone"
    );
    assert!(
        console_method_member_fields("meta/console/send").is_some_and(|fields| fields
            .iter()
            .all(|field| field.kind == ConsoleMemberFieldKind::Subject)),
        "the send names its member in a subject field"
    );
}

/// The one answer that changes for a caller the rules allow: rules that
/// allow the seat under exactly the hidden name. The caller reads the
/// rules' refusal, with the name as the caller spelled it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_caller_allowed_a_seat_under_exactly_a_hidden_name_reads_the_rules_refusal() -> TestResult
{
    let (_state, runtime, apps) =
        consoles("exact-name", vec![bob_may_seat_exactly(HIDDEN)]).await?;
    let bob = bearer(bob_claims());
    let mut wrong = Vec::new();
    for method in SEAT_METHODS {
        let request = requests_naming(method, HIDDEN).swap_remove(0);
        let answer = rpc_answer(&apps[0], &bob, method, request).await;
        if answer["error"]["code"] != json!(ACCESS_DENIED_RPC_CODE)
            || answer["error"]["message"] != json!("access denied: agent.spawn")
            || answer["error"]["data"]["resource"] != json!(HIDDEN)
        {
            wrong.push(format!("{method}: {answer}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "a rule naming exactly the name: {wrong:#?}"
    );
    assert!(seated(&runtime, HIDDEN).await?);
    Ok(())
}

/// No request of a caller the rules refuse seats or records anything: one
/// method of each kind, under the hidden name and under a name no member
/// holds, leaves the roster as it was.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_request_seats_and_records_nothing() -> TestResult {
    let (_state, runtime, apps) = consoles("refused-no-effect", vec![bob_is_refused()]).await?;
    let bob = bearer(bob_claims());
    let roster = |runtime: &MobRuntime| {
        let handle = runtime.handle();
        async move {
            let mut names: Vec<String> = handle
                .list_members()
                .await
                .into_iter()
                .map(|member| member.agent_identity.to_string())
                .collect();
            names.sort();
            names
        }
    };
    let before = roster(&runtime).await;
    for method in [
        "meta/ensure_member",
        "meta/fork_member",
        crate::rpc::identity_methods::IDENTITY_CREATE,
        "meta/workgraph/goal/create",
    ] {
        for name in [HIDDEN, NOBODY] {
            for request in requests_naming(method, name) {
                let answer = rpc_answer(&apps[0], &bob, method, request).await;
                assert!(
                    answer["result"].is_null() && !answer["error"].is_null(),
                    "{method} naming {name}: {answer}"
                );
            }
        }
    }
    assert_eq!(roster(&runtime).await, before, "the roster is as it was");
    assert!(!seated(&runtime, NOBODY).await?);
    assert!(!seated(&runtime, "a-new-name").await?);
    Ok(())
}

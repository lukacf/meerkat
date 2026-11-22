//! A console of a host that serves more than one mob: what a person is
//! listed and answered of a mob beside the launch mob.
//!
//! The rows speak the wire: routes, methods and JSON pointers of the
//! console's contract (`docs/rct/console-rest-sse-contract-v0.6.0.json`,
//! `mob_addressing`). The host of the fixture serves a second mob through a
//! test resolver; every person signs in, and the access rules say who may
//! address the second mob.
//!
//! What the rows hold:
//! - the experience read over every mob (`mobs=*`) lists the second mob,
//!   its members and its counts to those who may address it, and answers
//!   everybody else exactly what a host without that mob answers;
//! - that read loads no mob;
//! - a member row is keyed, and a request addresses a member, as the shared
//!   cases say (`console_row_key_cases.json`);
//! - the gating lists and the gating decision are the host's: a mob named
//!   beside them selects nothing and loads nothing; a list answers, of
//!   the entries members made, the launch mob's members' alone to a
//!   request that does not ask for every mob, and with `mobs` an entry is
//!   listed to those who may see its member in the entry's own mob;
//! - the gating requests of a mob, and their audit rows, are those of the
//!   mob that stands under its id;
//! - no answer of a method of the host names a mob to a person who may not
//!   address it;
//! - every name a member row carries of its member is its row key, and an
//!   entry of a host list names its member by its mob and its name;
//! - an access preview about a member is judged in that member's own mob,
//!   and a key the launch mob's door knows whole names that member;
//! - a request that names a mob and nothing else of it is what loads it;
//! - the gateway's methods of the host and its member words are the
//!   contract document's.

#![allow(clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use serde_json::{Value, json};

use super::*;

/// The secret the fixture console's sign-in tokens are signed with.
const SIGNING_SECRET: &str = "console-hosted-mobs-rows-signing";
/// An administrator of the fixture's access configuration.
const ADMIN: &str = "root@example.test";
/// A person whose rules allow viewing agents and gating entries in the
/// launch mob and in the second mob.
const OF_BOTH_MOBS: &str = "both@example.test";
/// A person whose rules allow viewing agents and gating entries, and name
/// no mob: they hold in the launch mob only.
const OF_THE_LAUNCH_MOB: &str = "launch@example.test";
/// A person who signs in and whom no access rule names.
const WITHOUT_RULES: &str = "nobody@example.test";

/// The member both mobs have under one name, and each mob's other member.
const SHARED_NAME: &str = "reviewer";
const ONLY_IN_THE_LAUNCH_MOB: &str = "greeter";
const ONLY_IN_THE_SECOND_MOB: &str = "analyst";

/// The shared cases of the member row key and of the door of a member.
const ROW_KEY_CASES: &str = include_str!("console_row_key_cases.json");

/// A runtime over one mob whose id starts with `tag`, composed as a gateway
/// composes one over the state directory `state_dir`, with the members
/// `members` of its one profile.
async fn runtime_with_members(
    state_dir: &std::path::Path,
    tag: &str,
    members: &[&str],
) -> crate::UnifiedRuntime {
    runtime_of_the_mob(
        state_dir,
        &format!("hosted-rows-{tag}-{}", uuid::Uuid::new_v4().simple()),
        members,
    )
    .await
}

/// [`runtime_with_members`] over the mob whose id is `mob_id`.
async fn runtime_of_the_mob(
    state_dir: &std::path::Path,
    mob_id: &str,
    members: &[&str],
) -> crate::UnifiedRuntime {
    let definition = meerkat_mob::MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "{mob_id}"
[profiles.worker]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"
[profiles.worker.tools]
comms = true
"#
    ))
    .expect("mob definition");
    let runtime = Box::pin(
        crate::UnifiedRuntime::builder()
            .definition(definition)
            .persistent_state(state_dir)
            .comms(true)
            .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
            .build(),
    )
    .await
    .unwrap_or_else(|error| panic!("the fixture runtime builds: {error}"));
    for member in members {
        runtime
            .mob_handle()
            .spawn_spec(meerkat_mob::SpawnMemberSpec::from_wire(
                "worker".to_string(),
                (*member).to_string(),
                None,
                None,
                None,
            ))
            .await
            .unwrap_or_else(|error| panic!("the fixture spawns {member}: {error}"));
    }
    runtime
}

/// A sign-in token of `email` for the fixture console.
fn token(email: &str) -> String {
    jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
        &json!({
            "iss": "http://127.0.0.1/meta-gateway",
            "aud": "persistent-gateway",
            "sub": email,
            "email": email,
            "exp": chrono::Utc::now().timestamp() + 300,
        }),
        &jsonwebtoken::EncodingKey::from_secret(SIGNING_SECRET.as_bytes()),
    )
    .expect("token")
}

/// The fixture: one launch runtime and one runtime whose mob is the host's
/// second mob, and two consoles over the launch runtime's one state. The
/// consoles differ in their resolver and in nothing else: `with_second`
/// serves the second mob too, `without_second` serves the launch mob alone.
struct Hosts {
    launch: crate::UnifiedRuntime,
    second: crate::UnifiedRuntime,
    /// The two runtimes' state directories, kept for the row's life.
    _state_dirs: [tempfile::TempDir; 2],
    with_second: Router,
    without_second: Router,
    /// The resolver of `with_second`: it records the mob each of its
    /// questions named.
    resolver: crate::unified_runtime::MobResolver,
    launch_mob: String,
    second_mob: String,
}

impl Hosts {
    async fn start() -> Self {
        let state_dirs = [
            tempfile::tempdir().expect("state directory"),
            tempfile::tempdir().expect("state directory"),
        ];
        let launch = runtime_with_members(
            state_dirs[0].path(),
            "launch",
            &[SHARED_NAME, ONLY_IN_THE_LAUNCH_MOB],
        )
        .await;
        let second = runtime_with_members(
            state_dirs[1].path(),
            "second",
            &[SHARED_NAME, ONLY_IN_THE_SECOND_MOB],
        )
        .await;
        let launch_mob = launch.mob_runtime().mob_id().as_str().to_string();
        let second_mob = second.mob_runtime().mob_id().as_str().to_string();
        let view_actions = vec![
            crate::access::ACTION_AGENT_VIEW.to_string(),
            crate::access::ACTION_GATING_VIEW.to_string(),
        ];
        let controller = AccessController::new(crate::access::AccessControlConfig {
            enabled: true,
            admins: vec![ADMIN.to_string()],
            rules: vec![
                crate::access::AccessRule {
                    id: "views-in-the-launch-mob".to_string(),
                    subjects: vec![OF_BOTH_MOBS.to_string(), OF_THE_LAUNCH_MOB.to_string()],
                    actions: view_actions.clone(),
                    ..crate::access::AccessRule::default()
                },
                crate::access::AccessRule {
                    id: "views-in-the-second-mob".to_string(),
                    subjects: vec![OF_BOTH_MOBS.to_string()],
                    actions: view_actions,
                    mobs: vec![second_mob.clone()],
                    ..crate::access::AccessRule::default()
                },
            ],
            ..crate::access::AccessControlConfig::default()
        })
        .expect("access configuration");
        let decisions = crate::console_auth_config::parse_console_auth_config(&json!({
            "shared_secret": SIGNING_SECRET,
            "email_allowlist": [ADMIN, OF_BOTH_MOBS, OF_THE_LAUNCH_MOB, WITHOUT_RULES],
        }))
        .expect("console sign-in configuration");
        // The host's values of the launch runtime; a mob's own values are
        // the ones its resolver answers at each request.
        let state = ConsoleJsonState {
            decisions,
            runtime: Some(launch.mob_runtime().clone()),
            module_runtime: Some(launch.module_runtime_handle()),
            access: Some(controller),
            ..super::console_human_admission_tests::state()
        };
        let resolver = launch
            .mob_resolver()
            .serving_a_second_mob_in_tests(&second.mob_resolver());
        let with_second =
            console_json_router_with_state(state.clone()).layer(axum::Extension(resolver.clone()));
        let without_second =
            console_json_router_with_state(state).layer(axum::Extension(launch.mob_resolver()));
        Self {
            launch,
            second,
            _state_dirs: state_dirs,
            with_second,
            without_second,
            resolver,
            launch_mob,
            second_mob,
        }
    }

    async fn stop(self) {
        self.launch.shutdown().await;
        self.second.shutdown().await;
    }

    /// The mobs the resolver of `with_second` was asked to resolve that are
    /// not the launch mob: a mob it resolved is a mob it loaded.
    fn mobs_loaded(&self) -> Vec<String> {
        self.resolver
            .resolved_for_in_tests()
            .into_iter()
            .flatten()
            .map(|mob| mob.as_str().to_string())
            .filter(|mob| mob != &self.launch_mob)
            .collect()
    }
}

/// What a console answered: its status, its headers (sorted, without the
/// time of the answer) and its body's bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Answer {
    status: StatusCode,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Answer {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "the answer is JSON ({error}): {} {}",
                self.status,
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// The answer of `app` for `request`, signed in as `email`.
async fn answer_as(
    app: &Router,
    mut request: axum::http::Request<axum::body::Body>,
    email: &str,
) -> Answer {
    use tower::ServiceExt;
    request.headers_mut().insert(
        header::AUTHORIZATION,
        format!("Bearer {}", token(email))
            .parse()
            .expect("header value"),
    );
    let response = tokio::time::timeout(
        std::time::Duration::from_mins(1),
        app.clone().oneshot(request),
    )
    .await
    .expect("the console answers in time")
    .expect("the console answers");
    let status = response.status();
    let mut headers = response
        .headers()
        .iter()
        .filter(|(name, _)| *name != header::DATE)
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                String::from_utf8_lossy(value.as_bytes()).into_owned(),
            )
        })
        .collect::<Vec<_>>();
    headers.sort();
    let body = axum::body::to_bytes(response.into_body(), 1 << 24)
        .await
        .expect("the answer's body")
        .to_vec();
    Answer {
        status,
        headers,
        body,
    }
}

/// `GET uri` on `app` as `email`.
async fn get_as(app: &Router, uri: &str, email: &str) -> Answer {
    let request = axum::http::Request::builder()
        .method("GET")
        .uri(uri)
        .body(axum::body::Body::empty())
        .expect("GET request");
    answer_as(app, request, email).await
}

/// The answer of `method` with `params` on the console's request door of
/// `app`, as `email`.
async fn rpc_as(app: &Router, method: &str, params: Value, email: &str) -> Value {
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/console/rpc")
        .header(header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(
            json!({"jsonrpc": "2.0", "id": "row", "method": method, "params": params}).to_string(),
        ))
        .expect("POST request");
    answer_as(app, request, email).await.json()
}

/// The entries of the array at `pointer` of `document`, or none.
fn entries<'a>(document: &'a Value, pointer: &str) -> Vec<&'a Value> {
    document
        .pointer(pointer)
        .and_then(Value::as_array)
        .map(|entries| entries.iter().collect())
        .unwrap_or_default()
}

/// The ids of the mobs the experience `document` lists.
fn listed_mobs(document: &Value) -> Vec<String> {
    entries(document, "/topology/live_snapshot/mobs")
        .into_iter()
        .filter_map(|mob| mob["mob_id"].as_str().map(str::to_string))
        .collect()
}

/// The row keys of the member rows the experience `document` lists.
fn listed_rows(document: &Value) -> Vec<String> {
    let mut rows = entries(document, "/agent_sidebar/live_snapshot/agents")
        .into_iter()
        .filter_map(|row| row["identity"].as_str().map(str::to_string))
        .collect::<Vec<_>>();
    rows.sort();
    rows
}

/// The experience read over every mob the caller may address.
const EXPERIENCE_OVER_EVERY_MOB: &str = "/console/experience?mobs=*";

/// The experience read over every mob lists the second mob, with its
/// members, to an administrator and to a person whose rule selects it; and
/// it answers a person whose rules hold in the launch mob only, and a
/// person no rule names, exactly what a host without that mob answers:
/// the same status, the same headers and the same bytes, counts included.
#[tokio::test]
async fn the_experience_over_every_mob_lists_a_second_mob_only_to_those_who_may_address_it() {
    let hosts = Hosts::start().await;

    // The fixture's own ground: a person whose rules hold in the launch mob
    // is answered the launch mob's members.
    let own = get_as(
        &hosts.with_second,
        EXPERIENCE_OVER_EVERY_MOB,
        OF_THE_LAUNCH_MOB,
    )
    .await;
    assert_eq!(own.status, StatusCode::OK, "{}", own.text());
    let own_rows = listed_rows(&own.json());
    assert!(
        own_rows.contains(&SHARED_NAME.to_string())
            && own_rows.contains(&ONLY_IN_THE_LAUNCH_MOB.to_string()),
        "a person whose rules hold in the launch mob is listed its members, got {own_rows:?}"
    );

    for email in [OF_THE_LAUNCH_MOB, WITHOUT_RULES] {
        for uri in [EXPERIENCE_OVER_EVERY_MOB, "/console/experience"] {
            let with_second = get_as(&hosts.with_second, uri, email).await;
            let without_second = get_as(&hosts.without_second, uri, email).await;
            assert_eq!(
                with_second.status, without_second.status,
                "{email} {uri}: one status with a second mob and without"
            );
            assert_eq!(
                with_second.headers, without_second.headers,
                "{email} {uri}: one set of headers with a second mob and without"
            );
            assert!(
                with_second.body == without_second.body,
                "{email} {uri}: one body with a second mob and without:\n{}\n{}",
                with_second.text(),
                without_second.text()
            );
            assert!(
                !with_second.text().contains(&hosts.second_mob),
                "{email} {uri}: the answer names no mob the person may not address"
            );
        }
    }

    for email in [ADMIN, OF_BOTH_MOBS] {
        let document = get_as(&hosts.with_second, EXPERIENCE_OVER_EVERY_MOB, email)
            .await
            .json();
        let mobs = listed_mobs(&document);
        assert!(
            mobs.contains(&hosts.launch_mob) && mobs.contains(&hosts.second_mob),
            "{email} is listed the launch mob and the second mob, got {mobs:?}"
        );
        let rows = listed_rows(&document);
        for row in [
            SHARED_NAME.to_string(),
            ONLY_IN_THE_LAUNCH_MOB.to_string(),
            format!("{}/{SHARED_NAME}", hosts.second_mob),
            format!("{}/{ONLY_IN_THE_SECOND_MOB}", hosts.second_mob),
        ] {
            assert!(
                rows.contains(&row),
                "{email} is listed the member row {row}, got {rows:?}"
            );
        }
    }
    hosts.stop().await;
}

/// Every count of the experience read over every mob counts what the
/// document lists to its reader: a listed mob's `member_count` is the
/// number of its member rows in the same document, the launch mob is the
/// one marked `default`, and reading it loads no mob.
#[tokio::test]
async fn every_count_of_the_experience_over_every_mob_counts_what_it_lists_and_the_read_loads_no_mob()
 {
    let hosts = Hosts::start().await;
    let document = get_as(&hosts.with_second, EXPERIENCE_OVER_EVERY_MOB, ADMIN)
        .await
        .json();
    let mobs = entries(&document, "/topology/live_snapshot/mobs");
    let rows = entries(&document, "/agent_sidebar/live_snapshot/agents");
    for (mob_id, is_default) in [(&hosts.launch_mob, true), (&hosts.second_mob, false)] {
        let mob = mobs
            .iter()
            .find(|mob| mob["mob_id"].as_str() == Some(mob_id.as_str()))
            .unwrap_or_else(|| panic!("the document lists the mob {mob_id}: {mobs:?}"));
        assert_eq!(
            mob["default"].as_bool().unwrap_or(false),
            is_default,
            "the launch mob is the one marked default: {mob}"
        );
        assert!(
            mob["state"].is_string(),
            "a listed mob names its state: {mob}"
        );
        let counted = rows
            .iter()
            .filter(|row| row["mob_id"].as_str() == Some(mob_id.as_str()))
            .count();
        assert_eq!(
            counted, 2,
            "each mob of the fixture has two member rows: {rows:?}"
        );
        assert_eq!(
            mob["member_count"].as_u64(),
            Some(counted as u64),
            "member_count is the number of the mob's rows in the document: {mob}"
        );
    }
    assert_eq!(
        hosts.mobs_loaded(),
        Vec::<String>::new(),
        "the read over every mob loads no mob"
    );
    hosts.stop().await;
}

/// A member row is keyed as the shared cases say, and a request addresses
/// a member with the pair the cases give: the launch mob's member by its
/// bare name and no mob, the second mob's member of the same name by that
/// mob's id and its bare name. The two are different members.
#[tokio::test]
async fn member_rows_are_keyed_and_members_addressed_as_the_shared_cases_say() {
    let cases: Value = serde_json::from_str(ROW_KEY_CASES).expect("the shared cases are JSON");
    // The cases' launch mob and their first hosted mob stand for the
    // fixture's two mobs.
    let listed = cases["mobs"].as_array().expect("the cases list mobs");
    let case_launch = listed
        .iter()
        .find(|mob| mob["default"] == json!(true))
        .and_then(|mob| mob["mob_id"].as_str())
        .expect("the cases have a launch mob");
    let case_hosted = listed
        .iter()
        .find(|mob| mob["provenance"] == json!("host") && mob["default"] != json!(true))
        .and_then(|mob| mob["mob_id"].as_str())
        .expect("the cases have a hosted mob");
    let hosts = Hosts::start().await;
    let in_fixture = |text: &str| {
        text.replace(case_hosted, &hosts.second_mob)
            .replace(case_launch, &hosts.launch_mob)
    };

    let document = get_as(&hosts.with_second, EXPERIENCE_OVER_EVERY_MOB, ADMIN)
        .await
        .json();
    let rows = entries(&document, "/agent_sidebar/live_snapshot/agents");
    let mut checked = 0;
    let mut row_faults = Vec::new();
    for case in cases["door"].as_array().expect("the cases of the door") {
        let mob = case["mob_id"].as_str().expect("a case names its mob");
        if case["name"] != json!(SHARED_NAME) || (mob != case_launch && mob != case_hosted) {
            continue;
        }
        let mob_id = in_fixture(mob);
        let row_key = in_fixture(case["row_key"].as_str().expect("a row key"));
        match rows
            .iter()
            .find(|row| row["identity"].as_str() == Some(row_key.as_str()))
        {
            None => row_faults.push(format!("the document has no row {row_key}")),
            Some(row) if row["mob_id"].as_str() != Some(mob_id.as_str()) => {
                row_faults.push(format!(
                    "the row {row_key} names the mob {} for {mob_id}",
                    row["mob_id"]
                ));
            }
            Some(_) => {}
        }

        // The request pair of the case reaches that mob's members.
        let mut params = json!({});
        if let Some(door) = case["request"]["mob_id"].as_str() {
            params["mob_id"] = json!(in_fixture(door));
        }
        let members = rpc_as(
            &hosts.with_second,
            "meta/list_members",
            params.clone(),
            ADMIN,
        )
        .await;
        let text = members.to_string();
        let (own, other) = if mob == case_launch {
            (ONLY_IN_THE_LAUNCH_MOB, ONLY_IN_THE_SECOND_MOB)
        } else {
            (ONLY_IN_THE_SECOND_MOB, ONLY_IN_THE_LAUNCH_MOB)
        };
        assert!(
            members["error"].is_null() && text.contains(own) && !text.contains(other),
            "a request with {params} is served for the mob of the row {row_key}: {members}"
        );
        let identity = case["request"]["identity"]
            .as_str()
            .expect("a request names its member");
        assert_eq!(
            identity, SHARED_NAME,
            "the door of each of the two mobs knows its member by the bare name"
        );
        checked += 1;
    }
    assert_eq!(
        checked, 2,
        "the launch mob's case and the hosted mob's case"
    );
    assert!(
        row_faults.is_empty(),
        "member rows that are not keyed as the shared cases say:\n{}",
        row_faults.join("\n")
    );
    hosts.stop().await;
}

/// A pending gating entry whose action came from the member `identity` of
/// the mob `mob` (none: the launch mob), recorded in the host's one gating
/// state. Its pending id.
async fn pending_entry_of(hosts: &Hosts, mob: Option<&str>, identity: &str) -> String {
    let request = json!({
        "action": "Publish the reviewed report",
        "actor_id": "agent:writer",
        "risk_tier": "r3",
        "approval_timeout_ms": 600_000,
    });
    hosts
        .launch
        .module_runtime_handle()
        .lock()
        .await
        .evaluate_gating_action_with_origin(
            serde_json::from_value(request).expect("gating request"),
            Some(crate::runtime::GatingOrigin {
                mob_id: mob.map(meerkat_mob::MobId::from),
                identity: identity.to_string(),
                conversation_id: None,
                interaction_id: None,
            }),
        )
        .pending_id
        .expect("an R3 gate waits for a decision")
}

/// The pending ids `meta/gating/pending` with `params` lists to `email`,
/// sorted; the answer itself when it is no list.
async fn pending_ids(app: &Router, params: Value, email: &str) -> Result<Vec<String>, Value> {
    let answer = rpc_as(app, "meta/gating/pending", params, email).await;
    let Some(pending) = answer["result"]["pending"].as_array() else {
        return Err(answer);
    };
    let mut ids = pending
        .iter()
        .filter_map(|entry| entry["pending_id"].as_str().map(str::to_string))
        .collect::<Vec<_>>();
    ids.sort();
    Ok(ids)
}

/// The gating lists and the gating decision are the host's. A request of
/// one of them that names a mob the caller may address is answered what
/// the same request without a mob is answered, and loads no mob.
#[tokio::test]
async fn a_mob_named_beside_a_gating_method_selects_nothing_and_loads_no_mob() {
    let hosts = Hosts::start().await;
    let of_launch = pending_entry_of(&hosts, None, SHARED_NAME).await;
    let of_second = pending_entry_of(&hosts, Some(&hosts.second_mob), SHARED_NAME).await;
    assert_ne!(of_launch, of_second);

    let naming = json!({ "mob_id": hosts.second_mob });
    for (method, params) in [
        ("meta/gating/pending", json!({})),
        ("meta/gating/audit", json!({})),
        (
            "meta/gating/decide",
            json!({ "pending_id": "no-such-pending-entry", "decision": "approve" }),
        ),
    ] {
        let mut with_mob = params.clone();
        with_mob["mob_id"] = naming["mob_id"].clone();
        let unnamed = rpc_as(&hosts.with_second, method, params, ADMIN).await;
        let named = rpc_as(&hosts.with_second, method, with_mob, ADMIN).await;
        assert_eq!(
            named, unnamed,
            "{method}: a mob named beside it selects nothing"
        );
    }
    assert_eq!(
        hosts.mobs_loaded(),
        Vec::<String>::new(),
        "a gating method loads no mob"
    );
    hosts.stop().await;
}

/// One list of the host for a person: a pending gating entry is listed to
/// those who may see its member in the entry's own mob, on a request that
/// asks for every mob (`mobs`). The two mobs'
/// members of one name are told apart: a person whose rules hold in both
/// mobs is listed both entries, a person whose rules hold in the launch
/// mob only is listed the launch mob's entry alone, with a second mob on
/// the host and without, whatever mob the request names.
#[tokio::test]
async fn a_gating_entry_is_listed_to_those_who_may_see_its_member_in_its_own_mob() {
    let hosts = Hosts::start().await;
    let of_launch = pending_entry_of(&hosts, None, SHARED_NAME).await;
    let of_second = pending_entry_of(&hosts, Some(&hosts.second_mob), SHARED_NAME).await;
    let mut both = vec![of_launch.clone(), of_second.clone()];
    both.sort();

    assert_eq!(
        pending_ids(&hosts.with_second, json!({ "mobs": "*" }), ADMIN).await,
        Ok(both.clone()),
        "an administrator is listed the entry of each mob's member"
    );
    for app in [&hosts.with_second, &hosts.without_second] {
        assert_eq!(
            pending_ids(app, json!({}), OF_THE_LAUNCH_MOB).await,
            Ok(vec![of_launch.clone()]),
            "a person whose rules hold in the launch mob is listed its entry alone"
        );
    }
    // The answers of a person who may not address the second mob are equal
    // with that mob on the host and without, and name it nowhere.
    for email in [OF_THE_LAUNCH_MOB, WITHOUT_RULES] {
        for method in ["meta/gating/pending", "meta/gating/audit"] {
            let with_second = rpc_as(&hosts.with_second, method, json!({}), email).await;
            let without_second = rpc_as(&hosts.without_second, method, json!({}), email).await;
            assert_eq!(
                with_second, without_second,
                "{email} {method}: one answer with a second mob and without"
            );
            assert!(
                !with_second.to_string().contains(&hosts.second_mob),
                "{email} {method}: the answer names no mob the person may not address: {with_second}"
            );
        }
    }
    // The second mob runs: a person whose rules hold in both mobs is listed
    // the entry of each mob's member without asking anything of that mob
    // first.
    assert_eq!(
        pending_ids(&hosts.with_second, json!({ "mobs": "*" }), OF_BOTH_MOBS).await,
        Ok(both),
        "a person whose rules hold in both mobs is listed the entry of each mob's member"
    );
    hosts.stop().await;
}

/// A person whose rules hold in both mobs is listed the pending gating
/// entry of the second mob's member once they asked something of that mob:
/// a request that names a mob makes its members known to the console.
#[tokio::test]
async fn a_gating_entry_of_a_second_mobs_member_is_listed_once_the_person_asked_of_that_mob() {
    let hosts = Hosts::start().await;
    let of_launch = pending_entry_of(&hosts, None, SHARED_NAME).await;
    let of_second = pending_entry_of(&hosts, Some(&hosts.second_mob), SHARED_NAME).await;
    let mut both = vec![of_launch, of_second];
    both.sort();

    let members = rpc_as(
        &hosts.with_second,
        "meta/list_members",
        json!({ "mob_id": hosts.second_mob }),
        OF_BOTH_MOBS,
    )
    .await;
    assert!(
        members["error"].is_null() && members.to_string().contains(ONLY_IN_THE_SECOND_MOB),
        "the person is answered the second mob's members: {members}"
    );
    assert_eq!(
        pending_ids(&hosts.with_second, json!({ "mobs": "*" }), OF_BOTH_MOBS).await,
        Ok(both),
        "the person is listed the entry of each mob's member"
    );
    hosts.stop().await;
}

/// The two gating requests the host's gating owner takes now of the mob
/// `mob_id`, each waiting for a decision: one made for the mob with no
/// origin, and one whose action came from the mob's member `identity`. The
/// pending id and the action id of each, in that order.
async fn two_requests_of_the_mob(
    host: &crate::UnifiedRuntime,
    mob_id: &str,
    identity: &str,
) -> [(String, String); 2] {
    let request = || {
        serde_json::from_value(json!({
            "action": "Publish the reviewed report",
            "actor_id": "agent:writer",
            "risk_tier": "r3",
            "approval_timeout_ms": 600_000,
        }))
        .expect("gating request")
    };
    let mob = meerkat_mob::MobId::from(mob_id);
    let owner = host.module_runtime_handle();
    let mut owner = owner.lock().await;
    let for_the_mob = owner.evaluate_gating_action_of_mob(request(), Some(&mob));
    let by_its_member = owner.evaluate_gating_action_with_origin(
        request(),
        Some(crate::runtime::GatingOrigin {
            mob_id: Some(mob),
            identity: identity.to_string(),
            conversation_id: None,
            interaction_id: None,
        }),
    );
    [for_the_mob, by_its_member].map(|evaluated| {
        (
            evaluated
                .pending_id
                .expect("an R3 gate waits for a decision"),
            evaluated.action_id,
        )
    })
}

/// The action ids of the rows `meta/gating/audit` over every mob serves
/// `email`; the answer itself when it is no list.
async fn audit_actions(
    app: &Router,
    email: &str,
) -> Result<std::collections::BTreeSet<String>, Value> {
    let answer = rpc_as(
        app,
        "meta/gating/audit",
        json!({ "limit": 200, "mobs": "*" }),
        email,
    )
    .await;
    let Some(entries) = answer["result"]["entries"].as_array() else {
        return Err(answer);
    };
    Ok(entries
        .iter()
        .filter_map(|entry| entry["action_id"].as_str().map(str::to_string))
        .collect())
}

/// The gating requests of a mob are those of the mob that stands under its
/// id. A mob stands under an id beside the launch mob, and the host's
/// gating owner takes two requests of it that wait: one made for the mob
/// with no origin, one made by its member. That mob ends. A mob is created
/// under the id, with a member of the same name, and the owner takes the
/// same two requests of it, and one of the launch mob's member.
///
/// A person whose rules hold in the mob of that id is listed the two
/// requests made since the mob that stands under the id was created, with
/// the launch mob's, and reads the audit rows of those three. A decide by
/// that person of one of the other two is answered as an id that is not
/// pending, and the request waits on. An administrator is listed all five
/// and reads the rows of all five. A person whose rules hold in the launch
/// mob only is listed the launch mob's request and reads its rows alone.
#[tokio::test]
async fn the_gating_requests_of_a_mob_are_those_of_the_mob_that_stands_under_its_id() {
    let state_dirs = [
        tempfile::tempdir().expect("state directory"),
        tempfile::tempdir().expect("state directory"),
        tempfile::tempdir().expect("state directory"),
    ];
    let launch = runtime_with_members(state_dirs[0].path(), "launch", &[SHARED_NAME]).await;
    let second_mob = format!("hosted-rows-standing-{}", uuid::Uuid::new_v4().simple());
    let a_while = || tokio::time::sleep(std::time::Duration::from_millis(40));

    // A mob under the id, its two requests, and its end.
    let before = {
        let earlier = runtime_of_the_mob(state_dirs[1].path(), &second_mob, &[SHARED_NAME]).await;
        let before = two_requests_of_the_mob(&launch, &second_mob, SHARED_NAME).await;
        earlier.shutdown().await;
        before
    };
    a_while().await;
    // The mob that stands under the id, and its two requests.
    let standing = runtime_of_the_mob(state_dirs[2].path(), &second_mob, &[SHARED_NAME]).await;
    a_while().await;
    let since = two_requests_of_the_mob(&launch, &second_mob, SHARED_NAME).await;
    let of_launch = {
        let owner = launch.module_runtime_handle();
        let evaluated = owner.lock().await.evaluate_gating_action_with_origin(
            serde_json::from_value(json!({
                "action": "Publish the reviewed report",
                "actor_id": "agent:writer",
                "risk_tier": "r3",
                "approval_timeout_ms": 600_000,
            }))
            .expect("gating request"),
            Some(crate::runtime::GatingOrigin {
                mob_id: None,
                identity: SHARED_NAME.to_string(),
                conversation_id: None,
                interaction_id: None,
            }),
        );
        (
            evaluated
                .pending_id
                .expect("an R3 gate waits for a decision"),
            evaluated.action_id,
        )
    };

    let actions = vec![
        crate::access::ACTION_AGENT_VIEW.to_string(),
        crate::access::ACTION_GATING_VIEW.to_string(),
        crate::access::ACTION_GATING_DECIDE.to_string(),
    ];
    let controller = AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![ADMIN.to_string()],
        rules: vec![
            crate::access::AccessRule {
                id: "gating-in-the-launch-mob".to_string(),
                subjects: vec![OF_BOTH_MOBS.to_string(), OF_THE_LAUNCH_MOB.to_string()],
                actions: actions.clone(),
                ..crate::access::AccessRule::default()
            },
            crate::access::AccessRule {
                id: "gating-in-the-second-mob".to_string(),
                subjects: vec![OF_BOTH_MOBS.to_string()],
                actions,
                mobs: vec![second_mob.clone()],
                ..crate::access::AccessRule::default()
            },
        ],
        ..crate::access::AccessControlConfig::default()
    })
    .expect("access configuration");
    let decisions = crate::console_auth_config::parse_console_auth_config(&json!({
        "shared_secret": SIGNING_SECRET,
        "email_allowlist": [ADMIN, OF_BOTH_MOBS, OF_THE_LAUNCH_MOB, WITHOUT_RULES],
    }))
    .expect("console sign-in configuration");
    let state = ConsoleJsonState {
        decisions,
        runtime: Some(launch.mob_runtime().clone()),
        module_runtime: Some(launch.module_runtime_handle()),
        access: Some(controller),
        ..super::console_human_admission_tests::state()
    };
    let resolver = launch
        .mob_resolver()
        .serving_a_second_mob_in_tests(&standing.mob_resolver());
    let app = console_json_router_with_state(state).layer(axum::Extension(resolver));

    let pending_of = |requests: &[&(String, String)]| {
        let mut ids = requests
            .iter()
            .map(|(pending_id, _)| pending_id.clone())
            .collect::<Vec<_>>();
        ids.sort();
        Ok::<_, Value>(ids)
    };
    let actions_of = |requests: &[&(String, String)]| {
        Ok::<_, Value>(
            requests
                .iter()
                .map(|(_, action_id)| action_id.clone())
                .collect::<std::collections::BTreeSet<_>>(),
        )
    };
    let mut wrong = Vec::new();
    for (who, email, shown) in [
        (
            "an administrator",
            ADMIN,
            vec![&before[0], &before[1], &since[0], &since[1], &of_launch],
        ),
        (
            "a person whose rules hold in the mob of that id",
            OF_BOTH_MOBS,
            vec![&since[0], &since[1], &of_launch],
        ),
        (
            "a person whose rules hold in the launch mob only",
            OF_THE_LAUNCH_MOB,
            vec![&of_launch],
        ),
    ] {
        let pending = pending_ids(&app, json!({ "mobs": "*" }), email).await;
        if pending != pending_of(&shown) {
            wrong.push(format!(
                "{who} is listed the requests {pending:?}, not {:?}",
                pending_of(&shown)
            ));
        }
        let rows = audit_actions(&app, email).await;
        if rows != actions_of(&shown) {
            wrong.push(format!(
                "{who} reads the rows of the actions {rows:?}, not {:?}",
                actions_of(&shown)
            ));
        }
    }

    // A decide of a request the person is not listed: the answer of an id
    // that is not pending, and the request waits on.
    {
        let decide = |pending_id: &str| {
            rpc_as(
                &app,
                "meta/gating/decide",
                json!({ "pending_id": pending_id, "decision": "approve" }),
                OF_BOTH_MOBS,
            )
        };
        let of_an_id_that_is_not_pending = decide("no-such-request").await;
        for (which, (pending_id, _)) in [
            ("made for the mob with no origin", &before[0]),
            ("made by the mob's member", &before[1]),
        ] {
            let answered = decide(pending_id).await;
            if answered != of_an_id_that_is_not_pending {
                wrong.push(format!(
                    "a decide of the request {which}: {answered}, and of an id that is not \
                     pending: {of_an_id_that_is_not_pending}"
                ));
            }
            let waits = launch
                .module_runtime_handle()
                .lock()
                .await
                .list_gating_pending()
                .iter()
                .any(|entry| &entry.pending_id == pending_id);
            if !waits {
                wrong.push(format!(
                    "the request {which} waits no more after that decide"
                ));
            }
        }
    }

    drop(app);
    launch.shutdown().await;
    standing.shutdown().await;
    assert!(
        wrong.is_empty(),
        "the gating requests of the mob that stands under an id:\n{}",
        wrong.join("\n")
    );
}

/// The pending entry an escalation of the request `pending_id` leaves:
/// the successor's pending id.
async fn successor_of(host: &crate::UnifiedRuntime, pending_id: &str) -> String {
    let owner = host.module_runtime_handle();
    let escalated = owner
        .lock()
        .await
        .decide_gating_action(crate::runtime::GatingDecideRequest {
            pending_id: pending_id.to_string(),
            approver_id: "person:owner".to_string(),
            decision: crate::runtime::GatingDecision::Escalate,
            reason: None,
        });
    match escalated {
        Ok(escalated) => escalated
            .next_pending_id
            .expect("an escalated request waits on under a successor"),
        Err(refused) => panic!("the request is escalated: {refused:?}"),
    }
}

/// An escalated request waits on under a new pending entry and is the
/// request it was: its time is the time it was made, not the time it was
/// escalated. A mob stands under an id and the host's gating owner takes
/// two requests of it; that mob ends and a mob is created under the id,
/// of which the owner takes the same two requests. All four are then
/// escalated.
///
/// A person whose rules hold in the mob of that id is listed the
/// successors of the two requests made since the standing mob was created
/// and reads the rows of those two actions; a decide by that person of one
/// of the other two successors is answered as an id that is not pending,
/// and it waits on. An administrator is listed the four successors and
/// reads the rows of the four actions.
#[tokio::test]
async fn an_escalated_request_is_judged_by_the_time_it_was_made() {
    let state_dirs = [
        tempfile::tempdir().expect("state directory"),
        tempfile::tempdir().expect("state directory"),
        tempfile::tempdir().expect("state directory"),
    ];
    let launch = runtime_with_members(state_dirs[0].path(), "launch", &[SHARED_NAME]).await;
    let second_mob = format!("hosted-rows-escalated-{}", uuid::Uuid::new_v4().simple());
    let a_while = || tokio::time::sleep(std::time::Duration::from_millis(40));
    launch
        .module_runtime_handle()
        .lock()
        .await
        .set_gating_escalation_routes(crate::runtime::GatingEscalationRoutes {
            default: Some("person:deputy".to_string()),
            ..crate::runtime::GatingEscalationRoutes::default()
        });

    // A mob under the id, its two requests, and its end.
    let before = {
        let earlier = runtime_of_the_mob(state_dirs[1].path(), &second_mob, &[SHARED_NAME]).await;
        let before = two_requests_of_the_mob(&launch, &second_mob, SHARED_NAME).await;
        earlier.shutdown().await;
        before
    };
    a_while().await;
    // The mob that stands under the id, and its two requests.
    let standing = runtime_of_the_mob(state_dirs[2].path(), &second_mob, &[SHARED_NAME]).await;
    a_while().await;
    let since = two_requests_of_the_mob(&launch, &second_mob, SHARED_NAME).await;
    a_while().await;
    // The four requests are escalated: each waits on under a successor.
    let mut escalated = Vec::new();
    for (pending_id, action_id) in before.iter().chain(&since) {
        escalated.push((successor_of(&launch, pending_id).await, action_id.clone()));
    }
    let (before, since) = escalated.split_at(2);

    let actions = vec![
        crate::access::ACTION_AGENT_VIEW.to_string(),
        crate::access::ACTION_GATING_VIEW.to_string(),
        crate::access::ACTION_GATING_DECIDE.to_string(),
    ];
    let controller = AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![ADMIN.to_string()],
        rules: vec![
            crate::access::AccessRule {
                id: "gating-in-the-launch-mob".to_string(),
                subjects: vec![OF_BOTH_MOBS.to_string()],
                actions: actions.clone(),
                ..crate::access::AccessRule::default()
            },
            crate::access::AccessRule {
                id: "gating-in-the-second-mob".to_string(),
                subjects: vec![OF_BOTH_MOBS.to_string()],
                actions,
                mobs: vec![second_mob.clone()],
                ..crate::access::AccessRule::default()
            },
        ],
        ..crate::access::AccessControlConfig::default()
    })
    .expect("access configuration");
    let decisions = crate::console_auth_config::parse_console_auth_config(&json!({
        "shared_secret": SIGNING_SECRET,
        "email_allowlist": [ADMIN, OF_BOTH_MOBS, OF_THE_LAUNCH_MOB, WITHOUT_RULES],
    }))
    .expect("console sign-in configuration");
    let state = ConsoleJsonState {
        decisions,
        runtime: Some(launch.mob_runtime().clone()),
        module_runtime: Some(launch.module_runtime_handle()),
        access: Some(controller),
        ..super::console_human_admission_tests::state()
    };
    let resolver = launch
        .mob_resolver()
        .serving_a_second_mob_in_tests(&standing.mob_resolver());
    let app = console_json_router_with_state(state).layer(axum::Extension(resolver));

    let pending_of = |requests: &[&(String, String)]| {
        let mut ids = requests
            .iter()
            .map(|(pending_id, _)| pending_id.clone())
            .collect::<Vec<_>>();
        ids.sort();
        Ok::<_, Value>(ids)
    };
    let actions_of = |requests: &[&(String, String)]| {
        Ok::<_, Value>(
            requests
                .iter()
                .map(|(_, action_id)| action_id.clone())
                .collect::<std::collections::BTreeSet<_>>(),
        )
    };
    let mut wrong = Vec::new();
    for (who, email, shown) in [
        (
            "an administrator",
            ADMIN,
            vec![&before[0], &before[1], &since[0], &since[1]],
        ),
        (
            "a person whose rules hold in the mob of that id",
            OF_BOTH_MOBS,
            vec![&since[0], &since[1]],
        ),
    ] {
        let pending = pending_ids(&app, json!({ "mobs": "*" }), email).await;
        if pending != pending_of(&shown) {
            wrong.push(format!(
                "{who} is listed the successors {pending:?}, not {:?}",
                pending_of(&shown)
            ));
        }
        let rows = audit_actions(&app, email).await;
        if rows != actions_of(&shown) {
            wrong.push(format!(
                "{who} reads the rows of the actions {rows:?}, not {:?}",
                actions_of(&shown)
            ));
        }
    }
    // A decide of a successor the person is not listed: the answer of an
    // id that is not pending, and the successor waits on.
    {
        let decide = |pending_id: &str| {
            rpc_as(
                &app,
                "meta/gating/decide",
                json!({ "pending_id": pending_id, "decision": "approve" }),
                OF_BOTH_MOBS,
            )
        };
        let of_an_id_that_is_not_pending = decide("no-such-request").await;
        for (pending_id, _) in before {
            let answered = decide(pending_id).await;
            if answered != of_an_id_that_is_not_pending {
                wrong.push(format!(
                    "a decide of a successor the person is not listed: {answered}, and of an id \
                     that is not pending: {of_an_id_that_is_not_pending}"
                ));
            }
            let waits = launch
                .module_runtime_handle()
                .lock()
                .await
                .list_gating_pending()
                .iter()
                .any(|entry| &entry.pending_id == pending_id);
            if !waits {
                wrong.push("a successor waits no more after that decide".to_string());
            }
        }
    }

    drop(app);
    launch.shutdown().await;
    standing.shutdown().await;
    assert!(
        wrong.is_empty(),
        "an escalated request, by the time it was made:\n{}",
        wrong.join("\n")
    );
}

/// No answer of a method of the host names a mob to a person who may not
/// address it: every method of the host, asked with no parameters by a
/// person whose rules hold in the launch mob only and by a person no rule
/// names, on a host with a second mob whose member has a pending gating
/// entry.
#[tokio::test]
async fn no_answer_of_a_method_of_the_host_names_a_mob_the_person_may_not_address() {
    let hosts = Hosts::start().await;
    pending_entry_of(&hosts, Some(&hosts.second_mob), SHARED_NAME).await;
    let mut naming = Vec::new();
    let mut asked = 0;
    for method in crate::rpc::mob_addressing::HOST_SCOPED_CONSOLE_METHODS {
        // A sign-out ends the caller's login: it answers nothing of a mob
        // and would end the row's own sign-in.
        if matches!(
            *method,
            "meta/console/sign_out" | "meta/console/revoke_sessions"
        ) {
            continue;
        }
        for email in [OF_THE_LAUNCH_MOB, WITHOUT_RULES] {
            let answer = rpc_as(&hosts.with_second, method, json!({}), email).await;
            asked += 1;
            if answer.to_string().contains(&hosts.second_mob) {
                naming.push(format!("{method} as {email}: {answer}"));
            }
        }
    }
    assert!(asked > 60, "the row asks the methods of the host ({asked})");
    assert!(
        naming.is_empty(),
        "answers of methods of the host that name a mob the person may not address:\n{}",
        naming.join("\n")
    );
    hosts.stop().await;
}

/// Every name a member row carries of its own member is the row key:
/// `identity`, `member_id` and `agent_id` are equal, and so is the label
/// `agent_identity` where a row carries one. A row of the second mob's
/// member carries the bare name its own mob knows it by in none of them, so
/// no reader takes it for the launch mob's member of that name.
#[tokio::test]
async fn every_name_of_a_member_row_is_its_row_key() {
    let hosts = Hosts::start().await;
    let document = get_as(&hosts.with_second, EXPERIENCE_OVER_EVERY_MOB, ADMIN)
        .await
        .json();
    let rows = entries(&document, "/agent_sidebar/live_snapshot/agents");
    assert!(!rows.is_empty(), "the document lists member rows");
    let mut faults = Vec::new();
    for row in &rows {
        let identity = row["identity"].as_str().unwrap_or_default();
        for name in ["member_id", "agent_id"] {
            if row[name].as_str() != Some(identity) {
                faults.push(format!("the row {identity} carries {name} = {}", row[name]));
            }
        }
        if let Some(label) = row
            .pointer("/labels/agent_identity")
            .and_then(Value::as_str)
            && label != identity
        {
            faults.push(format!(
                "the row {identity} carries the label agent_identity = {label}"
            ));
        }
    }
    assert!(
        faults.is_empty(),
        "member rows whose names are not their row key:\n{}",
        faults.join("\n")
    );
    let of_second = format!("{}/", hosts.second_mob);
    let second_rows = rows
        .iter()
        .filter(|row| {
            row["identity"]
                .as_str()
                .is_some_and(|identity| identity.starts_with(&of_second))
        })
        .count();
    assert_eq!(
        second_rows, 2,
        "the document lists the second mob's two members under their row keys: {rows:?}"
    );
    hosts.stop().await;
}

/// Whether the access preview answers that `subject` is allowed to view the
/// member `identity` names, asked by an administrator.
async fn preview_allows_viewing(hosts: &Hosts, subject: &str, identity: &str) -> bool {
    let answer = rpc_as(
        &hosts.with_second,
        "meta/access/preview",
        json!({ "subject": subject, "action": "agent.view", "identity": identity }),
        ADMIN,
    )
    .await;
    answer["result"]["allowed"]
        .as_bool()
        .unwrap_or_else(|| panic!("the preview answers whether it is allowed: {answer}"))
}

/// An access preview about a member is judged in that member's own mob. The
/// preview is a method of the host: it names a member by its row key and
/// takes no mob. A rule that lists no mob holds in the launch mob only, so
/// it allows the launch mob's member and not the second mob's member of the
/// same name; a rule that selects the second mob allows that one.
#[tokio::test]
async fn an_access_preview_about_a_member_is_judged_in_the_members_own_mob() {
    let hosts = Hosts::start().await;
    let of_second = format!("{}/{SHARED_NAME}", hosts.second_mob);

    assert!(
        preview_allows_viewing(&hosts, OF_THE_LAUNCH_MOB, SHARED_NAME).await,
        "a rule that lists no mob allows the launch mob's member"
    );
    assert!(
        !preview_allows_viewing(&hosts, WITHOUT_RULES, SHARED_NAME).await,
        "a person no rule names is not allowed the launch mob's member"
    );
    assert!(
        !preview_allows_viewing(&hosts, WITHOUT_RULES, &of_second).await,
        "a person no rule names is not allowed the second mob's member"
    );
    assert!(
        preview_allows_viewing(&hosts, OF_BOTH_MOBS, &of_second).await,
        "a rule that selects the second mob allows its member"
    );
    assert!(
        !preview_allows_viewing(&hosts, OF_THE_LAUNCH_MOB, &of_second).await,
        "a rule that lists no mob does not allow the second mob's member"
    );
    hosts.stop().await;
}

/// A row key that is the whole name of a member of the launch mob names
/// that member, whatever the name begins with. A launch mob member's name
/// may hold the separator, so it can begin with the id of a mob the host
/// serves beside its launch mob. Here the launch mob seats a member named
/// `<the second mob's id>/drafter`, and the second mob has no member
/// `drafter`. Both consoles list the member under that name: the one that
/// serves the launch mob alone too, so it is the launch mob's. An access
/// preview about that key is judged in the launch mob: a rule that lists no
/// mob holds in the launch mob only, and it allows the member; a person no
/// rule names is not allowed it. A key the launch mob's door does not know
/// whole names the second mob's member: the rule that lists no mob does
/// not allow that one, and a rule that selects the second mob does.
#[tokio::test]
async fn an_access_preview_about_a_key_the_launch_mob_knows_whole_is_judged_in_the_launch_mob() {
    let hosts = Hosts::start().await;
    let spelled = format!("{}/drafter", hosts.second_mob);
    hosts
        .launch
        .mob_handle()
        .spawn_spec(meerkat_mob::SpawnMemberSpec::from_wire(
            "worker".to_string(),
            crate::member_comms_id::mob_member_id_str(&spelled).into_owned(),
            None,
            None,
            None,
        ))
        .await
        .unwrap_or_else(|error| panic!("the launch mob seats {spelled}: {error}"));

    // The scene: the launch mob's door knows the name whole, on the console
    // that serves the second mob and on the one that does not; and the
    // second mob is served, with its own members under its id.
    let with_second = get_as(&hosts.with_second, EXPERIENCE_OVER_EVERY_MOB, ADMIN)
        .await
        .json();
    let without_second = get_as(&hosts.without_second, EXPERIENCE_OVER_EVERY_MOB, ADMIN)
        .await
        .json();
    let of_second = format!("{}/{SHARED_NAME}", hosts.second_mob);
    assert!(
        listed_rows(&without_second).contains(&spelled),
        "the console of the launch mob alone lists its member {spelled}: {:?}",
        listed_rows(&without_second)
    );
    assert!(
        listed_rows(&with_second).contains(&spelled)
            && listed_rows(&with_second).contains(&of_second),
        "the console of both mobs lists {spelled} and {of_second}: {:?}",
        listed_rows(&with_second)
    );

    assert!(
        preview_allows_viewing(&hosts, OF_THE_LAUNCH_MOB, &spelled).await,
        "a rule that lists no mob allows the launch mob's member whose name begins with the \
         second mob's id"
    );
    assert!(
        !preview_allows_viewing(&hosts, WITHOUT_RULES, &spelled).await,
        "a person no rule names is not allowed that member"
    );
    assert!(
        !preview_allows_viewing(&hosts, OF_THE_LAUNCH_MOB, &of_second).await,
        "a key the launch mob's door does not know whole names the second mob's member, \
         which a rule that lists no mob does not allow"
    );
    assert!(
        preview_allows_viewing(&hosts, OF_BOTH_MOBS, &of_second).await,
        "a rule that selects the second mob allows its member"
    );
    hosts.stop().await;
}

/// The launch mob's door answers for a key by the roster it has at the
/// request. The launch mob seats a member named
/// `<the second mob's id>/editor`, and the first request after the seating
/// is an access preview about that key: nothing but the preview's own
/// reading of the key has read the launch mob's roster since the member
/// was seated. The preview is judged in the launch mob, where a rule that
/// lists no mob allows the member. The console of the launch mob alone
/// lists the member under that name afterwards.
#[tokio::test]
async fn an_access_preview_as_the_first_request_after_a_seating_reads_the_whole_name() {
    let hosts = Hosts::start().await;
    let spelled = format!("{}/editor", hosts.second_mob);
    hosts
        .launch
        .mob_handle()
        .spawn_spec(meerkat_mob::SpawnMemberSpec::from_wire(
            "worker".to_string(),
            crate::member_comms_id::mob_member_id_str(&spelled).into_owned(),
            None,
            None,
            None,
        ))
        .await
        .unwrap_or_else(|error| panic!("the launch mob seats {spelled}: {error}"));

    assert!(
        preview_allows_viewing(&hosts, OF_THE_LAUNCH_MOB, &spelled).await,
        "the first request after the seating reads the new member's whole name, and a rule \
         that lists no mob allows it"
    );
    // The scene, read after the preview: the member is the launch mob's.
    let without_second = get_as(&hosts.without_second, EXPERIENCE_OVER_EVERY_MOB, ADMIN)
        .await
        .json();
    assert!(
        listed_rows(&without_second).contains(&spelled),
        "the console of the launch mob alone lists its member {spelled}: {:?}",
        listed_rows(&without_second)
    );
    hosts.stop().await;
}

/// An entry of a list of the host names its member by its mob and its
/// name: a pending gating entry of the second mob's member carries that
/// mob's id beside the name, and an entry of the launch mob's member
/// carries the name and no mob.
#[tokio::test]
async fn a_gating_entry_names_its_member_by_its_mob_and_its_name() {
    let hosts = Hosts::start().await;
    let of_launch = pending_entry_of(&hosts, None, SHARED_NAME).await;
    let of_second = pending_entry_of(&hosts, Some(&hosts.second_mob), SHARED_NAME).await;
    let answer = rpc_as(
        &hosts.with_second,
        "meta/gating/pending",
        json!({ "mobs": "*" }),
        ADMIN,
    )
    .await;
    let pending = entries(&answer, "/result/pending");
    let entry = |id: &str| {
        pending
            .iter()
            .find(|entry| entry["pending_id"].as_str() == Some(id))
            .copied()
            .unwrap_or_else(|| panic!("the list has the entry {id}: {answer}"))
    };
    let launch_entry = entry(&of_launch);
    assert_eq!(launch_entry["origin"]["identity"], json!(SHARED_NAME));
    assert!(
        launch_entry["origin"].get("mob_id").is_none(),
        "an entry of the launch mob's member names no mob: {launch_entry}"
    );
    let second_entry = entry(&of_second);
    assert_eq!(second_entry["origin"]["identity"], json!(SHARED_NAME));
    assert_eq!(
        second_entry["origin"]["mob_id"],
        json!(hosts.second_mob),
        "an entry of the second mob's member names that mob: {second_entry}"
    );
    hosts.stop().await;
}

/// A request that names a mob and nothing else of it (`meta/status`) is
/// answered to those who may address the mob, and it is what loads the
/// mob; for everybody else it is the one refusal of a mob, and loads
/// nothing.
#[tokio::test]
async fn a_status_request_that_names_a_mob_is_answered_to_those_who_may_address_it() {
    let hosts = Hosts::start().await;
    let naming = json!({ "mob_id": hosts.second_mob });
    for email in [OF_THE_LAUNCH_MOB, WITHOUT_RULES] {
        let answer = rpc_as(&hosts.with_second, "meta/status", naming.clone(), email).await;
        assert_eq!(
            (
                &answer["error"]["code"],
                &answer["error"]["data"]["kind"],
                &answer["error"]["data"]["requested_mob_id"],
            ),
            (
                &json!(-32602),
                &json!("foreign_mob_target"),
                &json!(hosts.second_mob)
            ),
            "{email}: {answer}"
        );
    }
    assert_eq!(
        hosts.mobs_loaded(),
        Vec::<String>::new(),
        "a refused request loads no mob"
    );
    let answer = rpc_as(&hosts.with_second, "meta/status", naming, OF_BOTH_MOBS).await;
    assert!(
        answer["error"].is_null() && answer["result"].is_object(),
        "a person who may address the mob is answered its status: {answer}"
    );
    let loaded = hosts.mobs_loaded();
    assert!(
        !loaded.is_empty() && loaded.iter().all(|mob| mob == &hosts.second_mob),
        "the request that names the mob is the one that loads it: {loaded:?}"
    );
    hosts.stop().await;
}

/// The gateway and the contract document agree, method by method: a method
/// is the host's in the document exactly when the console's door serves it
/// as a method of the host (the list the door that takes a request's mob
/// reads), and the member of `params` by which the document says a method
/// names a member is one the gateway's member field table reads. Every
/// method the door serves as a method of the host is a method of the
/// document. A method of the document serves the mob a request names
/// exactly when the door serves it per mob. A console
/// decides from the document whether a request carries a mob and under
/// which word it names a member, so a difference would send a request to
/// the wrong mob's member.
#[test]
fn the_gateways_methods_of_the_host_and_member_words_are_the_contract_documents() {
    let manifest_dir =
        std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets the crate directory");
    let path = std::path::Path::new(&manifest_dir)
        .join("../../docs/rct/console-rest-sse-contract-v0.6.0.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "this tree has no console contract document v0.6.0 ({}): {error}",
            path.display()
        )
    });
    let document: Value = serde_json::from_str(&text).expect("the contract document is JSON");
    let methods = document["surfaces"]["rpc"]["methods"]
        .as_object()
        .expect("the document lists the console's methods");
    assert!(methods.len() > 80, "{}", methods.len());
    let mut faults = Vec::new();
    let scopes = &document["mob_addressing"]["scopes"];
    let not_yet_served: Vec<&str> = scopes["one_mob_not_yet_served"]["methods"]
        .as_array()
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    for (method, entry) in methods {
        let of_host_in_document = entry["mob_scope"] == json!("host");
        let of_host_in_gateway = super::mobs_selector::console_method_is_of_the_host(method);
        if of_host_in_document != of_host_in_gateway {
            faults.push(format!(
                "{method}: of the host in the document: {of_host_in_document}, in the gateway: {of_host_in_gateway}"
            ));
        }
        // A method of the document serves the mob a request names exactly
        // when the door serves it per mob. A method the document marks as
        // not served yet is not counted.
        let per_mob_in_document =
            entry["mob_scope"] == json!("one_mob") && !not_yet_served.contains(&method.as_str());
        let per_mob_in_gateway = super::console_method_is_served_per_mob(method);
        if per_mob_in_document != per_mob_in_gateway {
            faults.push(format!(
                "{method}: served per mob in the document: {per_mob_in_document}, in the gateway: {per_mob_in_gateway}"
            ));
        }
        let words = entry["member_params"]
            .as_array()
            .map(|words| {
                words
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let table = console_method_member_fields(method).map(|fields| {
            fields
                .iter()
                .map(|field| field.path.join("."))
                .collect::<Vec<_>>()
        });
        match table {
            None if !words.is_empty() => faults.push(format!(
                "{method}: the document names {words:?}, the gateway's table does not list the method"
            )),
            None => {}
            Some(paths) => {
                for word in &words {
                    if !paths.contains(word) {
                        faults.push(format!(
                            "{method}: the document names {word}, the gateway reads {paths:?}"
                        ));
                    }
                }
                if words.is_empty() && !paths.is_empty() {
                    faults.push(format!(
                        "{method}: the gateway reads {paths:?}, the document names no member"
                    ));
                }
            }
        }
    }
    // The door's list is walked too: a method the door serves as a method
    // of the host is a method of the document, and of the host there.
    for method in super::mobs_selector::CONSOLE_METHODS_OF_THE_HOST {
        let of_host_in_document = methods
            .get(*method)
            .is_some_and(|entry| entry["mob_scope"] == json!("host"));
        if !of_host_in_document {
            faults.push(format!(
                "{method}: a method of the host for the console's door, and not one in the document"
            ));
        }
    }
    assert!(
        faults.is_empty(),
        "methods on which the gateway and the contract document differ:\n{}",
        faults.join("\n")
    );
}

/// A list of the host answers, of the entries members made, those of the
/// launch mob's members alone to a request that does not ask for every
/// mob, whoever asks and whatever mob stands beside the method; a request
/// that asks (`mobs`) is answered the entries of every mob the caller may
/// address. So a console that does not ask is never answered another mob's
/// member as the launch mob's member of the same name.
#[tokio::test]
async fn a_host_list_without_the_selector_answers_the_launch_mobs_entries_alone() {
    let hosts = Hosts::start().await;
    let of_launch = pending_entry_of(&hosts, None, SHARED_NAME).await;
    let of_second = pending_entry_of(&hosts, Some(&hosts.second_mob), SHARED_NAME).await;
    let mut both = vec![of_launch.clone(), of_second];
    both.sort();

    assert_eq!(
        pending_ids(&hosts.with_second, json!({ "mobs": "*" }), ADMIN).await,
        Ok(both),
        "an administrator who asks for every mob is listed the entry of each mob's member"
    );
    for email in [ADMIN, OF_BOTH_MOBS, OF_THE_LAUNCH_MOB] {
        assert_eq!(
            pending_ids(&hosts.with_second, json!({}), email).await,
            Ok(vec![of_launch.clone()]),
            "{email}: a request that does not ask for every mob is listed the launch mob's entry alone"
        );
    }
    assert_eq!(
        pending_ids(
            &hosts.with_second,
            json!({ "mob_id": hosts.second_mob }),
            ADMIN
        )
        .await,
        Ok(vec![of_launch]),
        "a mob named beside the method selects nothing"
    );
    hosts.stop().await;
}

/// The experience a console without sign-in serves over a snapshot of the
/// mob `door` with the members `members`, each with the mob it sits in when
/// that is a mob nested under `door` (the console's walk names such a
/// member `<its mob>/<its name>` and writes its mob on it).
fn experience_of_door(door: &str, members: &[(&str, Option<&str>)]) -> Value {
    let members = members
        .iter()
        .map(|(name, nested)| {
            let (identity, labels) = match nested {
                Some(mob) => (
                    crate::console_member_ref::nested_member_identity(mob, name)
                        .expect("a nested member's identity"),
                    json!({ "source_mob_id": mob }),
                ),
                None => ((*name).to_string(), json!({})),
            };
            serde_json::from_value::<crate::runtime::ConsoleMember>(json!({
                "agent_identity": identity,
                "role": "worker",
                "state": "active",
                "wired_to": [],
                "labels": labels,
            }))
            .expect("a member row of a snapshot")
        })
        .collect();
    let snapshot = crate::runtime::ConsoleLiveSnapshot::new(
        None,
        Some(true),
        Vec::new(),
        Vec::new(),
        members,
        true,
    )
    .with_mob_id(Some(door.to_string()));
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
        &crate::runtime::ConsoleRestJsonRequest {
            method: "GET".to_string(),
            path: "/console/experience".to_string(),
            auth: None,
        },
        None,
        Some(&snapshot),
        crate::runtime::SnapshotRows::OfRosters,
        None,
        None,
        false,
        false,
        &std::collections::BTreeSet::new(),
        None,
    );
    assert_eq!(response.status, 200, "experience: {:?}", response.body);
    response.body
}

/// The member rows and the topology nodes the door writes are the shared
/// cases' (`console_row_key_cases.json`), member for member.
///
/// A row names the mob whose door it was read on in `mob_id`, the mob its
/// member sits in in `member_mob_id` and the member's name there in
/// `member_name`, on the plain read and on the read over every mob alike,
/// and for a member of the door's own mob as for a member of a mob nested
/// under it. A row read on a door other than the launch mob's is keyed by
/// that door (`key_member_row`). A node is written by its own door alone
/// and carries the name as that door knows it, with the same three
/// members. For the case that states a member whole, its frame's pair
/// joined is its row's `identity`, its request is that pair, and its
/// request by mob is its row's three values.
///
/// Two members of one door can spell one identity: a member of the door's
/// own mob whose name is `<a mob's id>/<a name>`, and the member of that
/// name in that mob nested under the door. The door writes a row and a
/// node for each, alike in every name, and `member_mob_id` and
/// `member_name` alone tell them apart: no reading of the identity does.
/// The fixture's launch mob has both, so every row and node of that
/// identity is found by the mob its member sits in.
#[test]
fn the_rows_and_nodes_the_door_writes_are_the_shared_cases() {
    const ROWS: &str = "/agent_sidebar/live_snapshot/agents";
    const NODES: &str = "/topology/live_snapshot/nodes";
    let cases: Value = serde_json::from_str(ROW_KEY_CASES).expect("the shared cases are JSON");
    let listed = cases["mobs"].as_array().expect("the cases list mobs");
    let launch = listed
        .iter()
        .find(|mob| mob["default"] == json!(true))
        .and_then(|mob| mob["mob_id"].as_str())
        .expect("the cases have a launch mob");
    // The door of a mob: the hosted mob at the top of its tree, the launch
    // mob for a mob nested under the launch mob.
    let door_of = |mob: &str| -> String {
        let mut at = mob.to_string();
        loop {
            let parent = listed
                .iter()
                .find(|listed| listed["mob_id"] == json!(at))
                .and_then(|listed| listed["parent_mob_id"].as_str());
            match parent {
                Some(parent) => at = parent.to_string(),
                None => return at,
            }
        }
    };
    let text_of = |value: &Value, what: &str| -> String {
        value
            .as_str()
            .unwrap_or_else(|| panic!("a case names {what}: {value}"))
            .to_string()
    };
    let shared = cases["shared_spelling"]
        .as_array()
        .expect("the cases of a shared spelling");
    assert!(!shared.is_empty(), "shared_spelling: no case");
    // Every member the cases state: the mob it sits in and its name there.
    let mut stated: Vec<(String, String)> = ["row", "plain_row", "whole", "node"]
        .into_iter()
        .flat_map(|section| cases[section].as_array().cloned().unwrap_or_default())
        .map(|case| {
            (
                text_of(&case["mob_id"], "its mob"),
                text_of(&case["name"], "its member"),
            )
        })
        .collect();
    for case in shared {
        for member in case["members"]
            .as_array()
            .expect("the members that share it")
        {
            stated.push((
                text_of(&member["mob_id"], "its mob"),
                text_of(&member["name"], "its member"),
            ));
        }
    }
    // By its door: each member's name and, when it sits in a mob nested
    // under the door, that mob.
    let mut members_of: std::collections::BTreeMap<String, Vec<(String, Option<String>)>> =
        std::collections::BTreeMap::new();
    for (mob, name) in &stated {
        let door = door_of(mob);
        let member = (name.clone(), (&door != mob).then(|| mob.clone()));
        let members = members_of.entry(door).or_default();
        if !members.contains(&member) {
            members.push(member);
        }
    }
    let documents: std::collections::BTreeMap<String, Value> = members_of
        .iter()
        .map(|(door, members)| {
            let members: Vec<(&str, Option<&str>)> = members
                .iter()
                .map(|(name, nested)| (name.as_str(), nested.as_deref()))
                .collect();
            (door.clone(), experience_of_door(door, &members))
        })
        .collect();
    // The entry (a row or a node, at `pointer`) the door writes for the
    // member `name` of the mob `mob`, which the cases state as `stated`:
    // the one entry of the name as the door knows it, or, where two
    // members of the door spell that identity, the one that names the mob
    // the stated member sits in. A row of a door other than the launch
    // mob's is keyed as the read over every mob keys it.
    let entry_of = |pointer: &str,
                    mob: &str,
                    name: &str,
                    stated: &Value|
     -> Result<Value, String> {
        let door = door_of(mob);
        let known_as = if door == mob {
            name.to_string()
        } else {
            crate::console_member_ref::nested_member_identity(mob, name)
                .expect("a nested member's identity")
        };
        let written: Vec<&Value> = entries(&documents[&door], pointer)
            .into_iter()
            .filter(|entry| entry["identity"].as_str() == Some(known_as.as_str()))
            .collect();
        let mut entry = match written.as_slice() {
            [] => return Err("the door writes no such entry".to_string()),
            [only] => (*only).clone(),
            several => several
                .iter()
                .find(|entry| entry["member_mob_id"] == stated["member_mob_id"])
                .map(|entry| (*entry).clone())
                .ok_or_else(|| {
                    format!(
                        "the door writes {} entries of the identity {known_as} and none names \
                         the mob {} its member sits in",
                        several.len(),
                        stated["member_mob_id"]
                    )
                })?,
        };
        if pointer == ROWS && door != launch {
            super::row_key::key_member_row(&mut entry, &meerkat_mob::MobId::from(door.as_str()));
        }
        Ok(entry)
    };
    // What is wrong with the entry the door writes for a stated member.
    let held = |pointer: &str, mob: &Value, name: &Value, stated: &Value| -> Option<String> {
        match entry_of(
            pointer,
            &text_of(mob, "its mob"),
            &text_of(name, "its member"),
            stated,
        ) {
            Err(fault) => Some(fault),
            Ok(written) => {
                let differs: Vec<String> = stated
                    .as_object()
                    .expect("a case states an object")
                    .iter()
                    .filter(|(member, value)| &written[member.as_str()] != *value)
                    .map(|(member, value)| {
                        format!("{member}: {} for {value}", written[member.as_str()])
                    })
                    .collect();
                (!differs.is_empty()).then(|| differs.join("; "))
            }
        }
    };
    // Whether a request by mob is the three values of a row: a request on
    // the launch mob's door names no mob.
    let by_mob_is_the_rows = |by_mob: &Value, row: &Value| -> bool {
        (by_mob["mob_id"] == row["mob_id"]
            || (by_mob["mob_id"].is_null() && row["mob_id"] == json!(launch)))
            && by_mob["member_mob_id"] == row["member_mob_id"]
            && by_mob["identity"] == row["member_name"]
    };

    let mut wrong = Vec::new();
    for section in ["row", "plain_row", "whole"] {
        let stated = cases[section].as_array().expect("the cases of a row");
        assert!(!stated.is_empty(), "{section}: no case");
        for case in stated {
            if let Some(fault) = held(ROWS, &case["mob_id"], &case["name"], &case["row"]) {
                wrong.push(format!("{section}, {}: {fault}", case["case"]));
            }
        }
    }
    for case in cases["whole"].as_array().expect("the whole cases") {
        let (row, frame) = (&case["row"], &case["frame"]);
        let joined = match frame["mob_id"].as_str() {
            Some(mob) => format!("{mob}/{}", frame["identity"].as_str().unwrap_or_default()),
            None => frame["identity"].as_str().unwrap_or_default().to_string(),
        };
        if row["identity"] != json!(joined) {
            wrong.push(format!(
                "whole, {}: the frame's pair joined is {joined}, the row's identity {}",
                case["case"], row["identity"]
            ));
        }
        if case["request"] != *frame {
            wrong.push(format!(
                "whole, {}: the request is not the frame's pair",
                case["case"]
            ));
        }
        if !by_mob_is_the_rows(&case["request_by_mob"], row) {
            wrong.push(format!(
                "whole, {}: the request by mob {} is not the row's three values",
                case["case"], case["request_by_mob"]
            ));
        }
    }
    let nodes = cases["node"].as_array().expect("the cases of a node");
    assert!(!nodes.is_empty(), "node: no case");
    for case in nodes {
        if let Some(fault) = held(NODES, &case["mob_id"], &case["name"], &case["node"]) {
            wrong.push(format!("node, {}: {fault}", case["case"]));
        }
    }
    for case in shared {
        let members = case["members"]
            .as_array()
            .expect("the members that share it");
        if members.len() != 2 || !case["request"].is_null() {
            wrong.push(format!(
                "shared_spelling, {}: two members and no request by the one string are stated",
                case["case"]
            ));
        }
        let mut told_apart = std::collections::BTreeSet::new();
        for member in members {
            let row = &member["row"];
            if row["identity"] != case["identity"] {
                wrong.push(format!(
                    "shared_spelling, {}: the row of {} is not of the shared identity",
                    case["case"], member["name"]
                ));
            }
            if let Some(fault) = held(ROWS, &member["mob_id"], &member["name"], row) {
                wrong.push(format!(
                    "shared_spelling, {}, the row of {}: {fault}",
                    case["case"], member["name"]
                ));
            }
            // Its node holds the same three members as its row, and is
            // found under the name as its door knows it: a node is not
            // keyed by its door.
            let node = json!({
                "mob_id": row["mob_id"],
                "member_mob_id": row["member_mob_id"],
                "member_name": row["member_name"],
            });
            if let Some(fault) = held(NODES, &member["mob_id"], &member["name"], &node) {
                wrong.push(format!(
                    "shared_spelling, {}, the node of {}: {fault}",
                    case["case"], member["name"]
                ));
            }
            if !by_mob_is_the_rows(&member["request_by_mob"], row) {
                wrong.push(format!(
                    "shared_spelling, {}: the request by mob {} is not the row's three values",
                    case["case"], member["request_by_mob"]
                ));
            }
            told_apart.insert((
                row["member_mob_id"].to_string(),
                row["member_name"].to_string(),
            ));
        }
        if told_apart.len() != members.len() {
            wrong.push(format!(
                "shared_spelling, {}: the mob a member sits in and its name there do not tell \
                 the members apart",
                case["case"]
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "the rows and nodes the door writes are not the shared cases':\n{}",
        wrong.join("\n")
    );
}

/// A `member_mob_id` that names a mob of another door reaches no member
/// and loads no mob. The launch mob's door, asked for the second mob's
/// member of a name both mobs have by the second mob's id, answers what it
/// answers a mob the host does not have and a name no member holds, the
/// names apart: never its own member of that name; a person with no right
/// in the second mob is answered for it as for a mob the host does not
/// have. A read of the roster of
/// the door's own mob refuses the other mob in its one sentence. The second
/// mob's door answers the launch mob's id the same way, and its own mob's
/// id as it answers the bare name.
#[tokio::test]
async fn a_member_mob_id_that_names_a_mob_of_another_door_reaches_no_member() {
    let hosts = Hosts::start().await;
    let nowhere = format!("{}-nowhere", hosts.launch_mob);
    let apart = |answer: &Value| {
        answer
            .to_string()
            .replace(&nowhere, "<mob>")
            .replace(&hosts.second_mob, "<mob>")
            .replace(&hosts.launch_mob, "<mob>")
            .replace(SHARED_NAME, "<member>")
            .replace("nobody", "<member>")
    };
    let roster_refusal = |method: &str| {
        json!({
            "code": -32602,
            "message": format!(
                "{method} reads the roster of the mob the request is served for; \
                 member_mob_id names another mob"
            ),
        })
    };
    let mut wrong = Vec::new();

    // The launch mob's door.
    for method in ["meta/status_identity", "meta/console/inspect_identity"] {
        let absent = rpc_as(
            &hosts.with_second,
            method,
            json!({ "identity": "nobody", "member_mob_id": nowhere }),
            ADMIN,
        )
        .await;
        let other_door = rpc_as(
            &hosts.with_second,
            method,
            json!({ "identity": SHARED_NAME, "member_mob_id": hosts.second_mob }),
            ADMIN,
        )
        .await;
        if apart(&other_door) != apart(&absent) {
            wrong.push(format!(
                "{method} on the launch mob's door, the second mob's member: {other_door}\n  \
                 no member: {absent}"
            ));
        }
    }
    let refused = rpc_as(
        &hosts.with_second,
        "meta/get_member",
        json!({ "member_id": SHARED_NAME, "member_mob_id": hosts.second_mob }),
        ADMIN,
    )
    .await;
    if refused["error"] != roster_refusal("meta/get_member") {
        wrong.push(format!(
            "meta/get_member on the launch mob's door, the second mob: {refused}"
        ));
    }
    let absent = rpc_as(
        &hosts.with_second,
        "meta/console/query_timeline",
        json!({ "identity": "nobody", "member_mob_id": nowhere }),
        ADMIN,
    )
    .await;
    let other_door = rpc_as(
        &hosts.with_second,
        "meta/console/query_timeline",
        json!({ "identity": SHARED_NAME, "member_mob_id": hosts.second_mob }),
        ADMIN,
    )
    .await;
    if apart(&other_door) != apart(&absent) {
        wrong.push(format!(
            "meta/console/query_timeline on the launch mob's door, the second mob's member: \
             {other_door}\n  no member: {absent}"
        ));
    }
    // A person with no right in the second mob is answered for it as for
    // a mob the host does not have.
    for method in ["meta/status_identity", "meta/console/query_timeline"] {
        let absent = rpc_as(
            &hosts.with_second,
            method,
            json!({ "identity": SHARED_NAME, "member_mob_id": nowhere }),
            OF_THE_LAUNCH_MOB,
        )
        .await;
        let other_door = rpc_as(
            &hosts.with_second,
            method,
            json!({ "identity": SHARED_NAME, "member_mob_id": hosts.second_mob }),
            OF_THE_LAUNCH_MOB,
        )
        .await;
        if apart(&other_door) != apart(&absent) {
            wrong.push(format!(
                "{method} on the launch mob's door, asked by a person with no right in the \
                 second mob: {other_door}\n  a mob the host does not have: {absent}"
            ));
        }
    }
    let loaded = hosts.mobs_loaded();
    if !loaded.is_empty() {
        wrong.push(format!(
            "a request on the launch mob's door loaded {loaded:?}"
        ));
    }

    // The second mob's door.
    let refused = rpc_as(
        &hosts.with_second,
        "meta/get_member",
        json!({
            "mob_id": hosts.second_mob,
            "member_id": SHARED_NAME,
            "member_mob_id": hosts.launch_mob,
        }),
        ADMIN,
    )
    .await;
    if refused["error"] != roster_refusal("meta/get_member") {
        wrong.push(format!(
            "meta/get_member on the second mob's door, the launch mob: {refused}"
        ));
    }
    let by_name = rpc_as(
        &hosts.with_second,
        "meta/get_member",
        json!({ "mob_id": hosts.second_mob, "member_id": SHARED_NAME }),
        ADMIN,
    )
    .await;
    let by_own_mob = rpc_as(
        &hosts.with_second,
        "meta/get_member",
        json!({
            "mob_id": hosts.second_mob,
            "member_id": SHARED_NAME,
            "member_mob_id": hosts.second_mob,
        }),
        ADMIN,
    )
    .await;
    if !by_name["error"].is_null() || by_own_mob != by_name {
        wrong.push(format!(
            "meta/get_member on the second mob's door, its own mob: {by_own_mob}\n  by the \
             name: {by_name}"
        ));
    }
    let absent = rpc_as(
        &hosts.with_second,
        "meta/console/query_timeline",
        json!({ "mob_id": hosts.second_mob, "identity": "nobody", "member_mob_id": nowhere }),
        ADMIN,
    )
    .await;
    let other_door = rpc_as(
        &hosts.with_second,
        "meta/console/query_timeline",
        json!({
            "mob_id": hosts.second_mob,
            "identity": SHARED_NAME,
            "member_mob_id": hosts.launch_mob,
        }),
        ADMIN,
    )
    .await;
    if apart(&other_door) != apart(&absent) {
        wrong.push(format!(
            "meta/console/query_timeline on the second mob's door, the launch mob's member: \
             {other_door}\n  no member: {absent}"
        ));
    }
    assert!(
        wrong.is_empty(),
        "a `member_mob_id` that names a mob of another door reaches a member or loads a \
         mob:\n{}",
        wrong.join("\n")
    );
    hosts.stop().await;
}

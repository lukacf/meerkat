//! Rows for the experience over every mob a caller may address, asked of
//! the console's own doors over a host whose resolver serves a second mob
//! with members of its own.

#![allow(clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use serde_json::{Value, json};

use super::*;

/// The secret the fixture console's sign-in tokens are signed with.
const SIGNING_SECRET: &str = "console-every-mob-rows-signing";
/// An administrator of the fixture's access configuration.
const ADMIN: &str = "root@example.test";
/// A person whose one rule allows viewing agents of the role `worker` and
/// lists no mob: it holds in the launch mob only.
const OF_THE_LAUNCH_MOBS_WORKERS: &str = "workers@example.test";
/// A person whose one rule allows viewing agents of the role `analyst` in
/// the second mob.
const OF_THE_SECOND_MOBS_ANALYSTS: &str = "analysts@example.test";
/// The member both mobs have: a `worker` in the launch mob, an `analyst`
/// in the second mob.
const SHARED_NAME: &str = "reviewer";

/// A runtime over one mob whose id starts with `tag`, composed over the
/// state directory `state_dir`, with the member `reviewer` of its one
/// profile `role`.
async fn runtime_with_a_reviewer(
    state_dir: &std::path::Path,
    tag: &str,
    role: &str,
) -> crate::UnifiedRuntime {
    let definition = meerkat_mob::MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "every-mob-{tag}-{}"
[profiles.{role}]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"
[profiles.{role}.tools]
comms = true
"#,
        uuid::Uuid::new_v4().simple()
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
    runtime
        .mob_handle()
        .spawn_spec(meerkat_mob::SpawnMemberSpec::from_wire(
            role.to_string(),
            SHARED_NAME.to_string(),
            None,
            None,
            None,
        ))
        .await
        .unwrap_or_else(|error| panic!("the fixture spawns its member: {error}"));
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

/// The answer of `request` on `app` as an administrator, as JSON.
async fn asked(app: &Router, mut request: axum::http::Request<axum::body::Body>) -> Value {
    use tower::ServiceExt;

    request.headers_mut().insert(
        header::AUTHORIZATION,
        format!("Bearer {}", token(ADMIN))
            .parse()
            .expect("header value"),
    );
    let response = tokio::time::timeout(
        std::time::Duration::from_mins(1),
        app.clone().oneshot(request),
    )
    .await
    .expect("the door answers in time")
    .expect("the door answers");
    let body = axum::body::to_bytes(response.into_body(), 1 << 24)
        .await
        .expect("the answer's body");
    serde_json::from_slice(&body)
        .unwrap_or_else(|_| panic!("no JSON: {}", String::from_utf8_lossy(&body)))
}

/// Whether the access preview answers an administrator that `subject` is
/// allowed to view the member the row key `row_key` names.
async fn preview_allows_viewing(app: &Router, subject: &str, row_key: &str) -> bool {
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/console/rpc")
        .header(header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(
            json!({
                "jsonrpc": "2.0",
                "id": "row",
                "method": "meta/access/preview",
                "params": { "subject": subject, "action": "agent.view", "identity": row_key },
            })
            .to_string(),
        ))
        .expect("POST request");
    let answer = asked(app, request).await;
    answer["result"]["allowed"]
        .as_bool()
        .unwrap_or_else(|| panic!("the preview answers whether it is allowed: {answer}"))
}

/// The experience document `uri` answers an administrator.
async fn experience(app: &Router, uri: &str) -> Value {
    let request = axum::http::Request::builder()
        .method("GET")
        .uri(uri)
        .body(axum::body::Body::empty())
        .expect("GET request");
    asked(app, request).await
}

/// The experience over every mob reads the members of a second mob that is
/// loaded, and records them as that mob's agents and never as the launch
/// mob's. The two mobs each have a member `reviewer`: a `worker` in the
/// launch mob, an `analyst` in the second mob. After an administrator read
/// the experience over every mob,
///
/// - a rule about the role `worker` that lists no mob still allows the
///   launch mob's `reviewer`, and still does not allow the second mob's;
/// - a rule about the role `analyst` in the second mob allows the second
///   mob's `reviewer`, and does not allow the launch mob's.
///
/// The second mob's member is told from the launch mob's member of the
/// same name wherever the console judges a member by what it recorded.
#[tokio::test]
async fn the_experience_over_every_mob_records_a_mobs_members_as_that_mobs_agents() {
    let state_dirs = [
        tempfile::tempdir().expect("state directory"),
        tempfile::tempdir().expect("state directory"),
    ];
    let launch = runtime_with_a_reviewer(state_dirs[0].path(), "launch", "worker").await;
    let second = runtime_with_a_reviewer(state_dirs[1].path(), "second", "analyst").await;
    let second_mob = second.mob_runtime().mob_id().as_str().to_string();
    let rule = |id: &str, subject: &str, role: &str, mobs: Vec<String>| crate::access::AccessRule {
        id: id.to_string(),
        subjects: vec![subject.to_string()],
        actions: vec![crate::access::ACTION_AGENT_VIEW.to_string()],
        roles: vec![role.to_string()],
        mobs,
        ..crate::access::AccessRule::default()
    };
    let controller = AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![ADMIN.to_string()],
        rules: vec![
            rule(
                "workers-of-the-launch-mob",
                OF_THE_LAUNCH_MOBS_WORKERS,
                "worker",
                Vec::new(),
            ),
            rule(
                "analysts-of-the-second-mob",
                OF_THE_SECOND_MOBS_ANALYSTS,
                "analyst",
                vec![second_mob.clone()],
            ),
        ],
        ..crate::access::AccessControlConfig::default()
    })
    .expect("access configuration");
    controller.bind_default_mob(&crate::member_key::DefaultMob::new(
        launch.mob_runtime().mob_id().clone(),
    ));
    let decisions = crate::console_auth_config::parse_console_auth_config(&json!({
        "shared_secret": SIGNING_SECRET,
        "email_allowlist": [ADMIN, OF_THE_LAUNCH_MOBS_WORKERS, OF_THE_SECOND_MOBS_ANALYSTS],
    }))
    .expect("console sign-in configuration");
    let state = ConsoleJsonState {
        decisions,
        runtime: Some(launch.mob_runtime().clone()),
        access: Some(controller),
        ..super::console_human_admission_tests::state()
    };
    let resolver = launch
        .mob_resolver()
        .serving_a_second_mob_in_tests(&second.mob_resolver());
    let app = console_json_router_with_state(state).layer(axum::Extension(resolver.clone()));
    let of_the_second_mob = format!("{second_mob}/{SHARED_NAME}");

    // The launch mob's own read records its members.
    let of_the_launch_mob_alone = experience(&app, "/console/experience").await;
    let rows_of = |document: &Value| -> Vec<String> {
        document
            .pointer("/agent_sidebar/live_snapshot/agents")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|row| row["identity"].as_str().map(str::to_string))
            .collect()
    };
    assert_eq!(
        rows_of(&of_the_launch_mob_alone),
        vec![SHARED_NAME.to_string()],
        "the fixture's launch mob has its one member"
    );
    assert!(
        preview_allows_viewing(&app, OF_THE_LAUNCH_MOBS_WORKERS, SHARED_NAME).await,
        "the rule about workers allows the launch mob's member"
    );

    let over_every_mob = experience(&app, "/console/experience?mobs=*").await;
    let rows = rows_of(&over_every_mob);
    let mut wrong = Vec::new();
    if !rows.contains(&SHARED_NAME.to_string()) || !rows.contains(&of_the_second_mob) {
        wrong.push(format!(
            "the experience over every mob does not list both members: {rows:?}"
        ));
    }
    for (subject, row_key, allowed) in [
        (OF_THE_LAUNCH_MOBS_WORKERS, SHARED_NAME, true),
        (
            OF_THE_LAUNCH_MOBS_WORKERS,
            of_the_second_mob.as_str(),
            false,
        ),
        (OF_THE_SECOND_MOBS_ANALYSTS, SHARED_NAME, false),
        (
            OF_THE_SECOND_MOBS_ANALYSTS,
            of_the_second_mob.as_str(),
            true,
        ),
    ] {
        if preview_allows_viewing(&app, subject, row_key).await != allowed {
            wrong.push(format!(
                "{subject} viewing {row_key}: the preview does not answer allowed = {allowed}"
            ));
        }
    }
    let resolved = resolver.resolved_for_in_tests();
    if resolved.iter().any(Option::is_some) {
        wrong.push(format!("a read resolved the second mob: {resolved:?}"));
    }
    drop(app);
    launch.shutdown().await;
    second.shutdown().await;
    assert!(
        wrong.is_empty(),
        "a mob's members are not recorded as that mob's agents:\n{}",
        wrong.join("\n")
    );
}

/// A row of the launch mob offers the stop of a run of its member exactly
/// where it offers a retirement, so a console that reads `can_stop_run`
/// alone offers Stop where it offered it before. A plain member is offered
/// both; a singleton member, which no retirement removes, is offered
/// neither.
#[tokio::test]
async fn a_row_of_the_launch_mob_offers_the_stop_where_it_offers_a_retirement() {
    const SINGLETON: &str = "keeper";
    let state_dir = tempfile::tempdir().expect("state directory");
    let launch = runtime_with_a_reviewer(state_dir.path(), "stop-rows", "worker").await;
    launch
        .mob_handle()
        .spawn_spec(
            meerkat_mob::SpawnMemberSpec::from_wire(
                "worker".to_string(),
                SINGLETON.to_string(),
                None,
                None,
                None,
            )
            .with_labels(std::collections::BTreeMap::from([(
                "singleton".to_string(),
                "true".to_string(),
            )])),
        )
        .await
        .unwrap_or_else(|error| panic!("the singleton member spawns: {error}"));
    let controller = AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![ADMIN.to_string()],
        ..crate::access::AccessControlConfig::default()
    })
    .expect("access configuration");
    controller.bind_default_mob(&crate::member_key::DefaultMob::new(
        launch.mob_runtime().mob_id().clone(),
    ));
    let decisions = crate::console_auth_config::parse_console_auth_config(&json!({
        "shared_secret": SIGNING_SECRET,
        "email_allowlist": [ADMIN],
    }))
    .expect("console sign-in configuration");
    let state = ConsoleJsonState {
        decisions,
        runtime: Some(launch.mob_runtime().clone()),
        access: Some(controller),
        ..super::console_human_admission_tests::state()
    };
    let app = console_json_router_with_state(state);
    let document = experience(&app, "/console/experience").await;
    drop(app);
    launch.shutdown().await;

    let mut wrong = Vec::new();
    for (member, offered) in [(SHARED_NAME, true), (SINGLETON, false)] {
        let affordances = document
            .pointer("/agent_sidebar/live_snapshot/agents")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|row| row["identity"] == json!(member))
            .map_or(Value::Null, |row| row["affordances"].clone());
        if affordances["can_retire"] != json!(offered)
            || affordances["can_stop_run"] != json!(offered)
        {
            wrong.push(format!(
                "{member}: a retirement and the stop are not both {offered}: {affordances}"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "a row of the launch mob does not offer the stop where it offers a retirement:\n{}",
        wrong.join("\n")
    );
}

/// The rows of another mob in the experience over every mob offer no
/// lifecycle action the door does not serve for that mob. The launch mob
/// and a second mob each have a member `reviewer`. In the experience over
/// every mob an administrator reads, the second mob's row offers each
/// lifecycle action exactly where the door serves its method for the mob a
/// request names: the stop of a run is served per mob and a retirement is
/// not, so the row offers the stop and no retirement. The launch mob's row
/// is the row of the launch mob's own experience.
#[tokio::test]
async fn a_row_of_another_mob_offers_no_lifecycle_action_the_door_does_not_serve_for_it() {
    // Each lifecycle action of a member row, with the method that acts.
    const ACTIONS: [(&str, &str); 5] = [
        ("can_retire", "meta/retire"),
        ("can_respawn", "meta/respawn"),
        ("can_reset", "meta/reset"),
        ("can_stop_run", "meta/stop_member_run"),
        ("can_fork_side_chat", "meta/fork_member"),
    ];
    let state_dirs = [
        tempfile::tempdir().expect("state directory"),
        tempfile::tempdir().expect("state directory"),
    ];
    let launch = runtime_with_a_reviewer(state_dirs[0].path(), "launch", "worker").await;
    let second = runtime_with_a_reviewer(state_dirs[1].path(), "second", "worker").await;
    let second_mob = second.mob_runtime().mob_id().as_str().to_string();
    let controller = AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![ADMIN.to_string()],
        ..crate::access::AccessControlConfig::default()
    })
    .expect("access configuration");
    controller.bind_default_mob(&crate::member_key::DefaultMob::new(
        launch.mob_runtime().mob_id().clone(),
    ));
    let decisions = crate::console_auth_config::parse_console_auth_config(&json!({
        "shared_secret": SIGNING_SECRET,
        "email_allowlist": [ADMIN],
    }))
    .expect("console sign-in configuration");
    let state = ConsoleJsonState {
        decisions,
        runtime: Some(launch.mob_runtime().clone()),
        access: Some(controller),
        ..super::console_human_admission_tests::state()
    };
    let resolver = launch
        .mob_resolver()
        .serving_a_second_mob_in_tests(&second.mob_resolver());
    let app = console_json_router_with_state(state).layer(axum::Extension(resolver));
    let of_the_second_mob = format!("{second_mob}/{SHARED_NAME}");
    // The affordances of the row `key` of a document, or null.
    let affordances_of = |document: &Value, key: &str| -> Value {
        document
            .pointer("/agent_sidebar/live_snapshot/agents")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|row| row["identity"] == json!(key))
            .map_or(Value::Null, |row| row["affordances"].clone())
    };

    let of_the_launch_mob_alone = experience(&app, "/console/experience").await;
    let over_every_mob = experience(&app, "/console/experience?mobs=*").await;
    drop(app);
    launch.shutdown().await;
    second.shutdown().await;

    let launch_row = affordances_of(&over_every_mob, SHARED_NAME);
    let second_row = affordances_of(&over_every_mob, &of_the_second_mob);
    let mut wrong = Vec::new();
    if launch_row.is_null() || launch_row != affordances_of(&of_the_launch_mob_alone, SHARED_NAME) {
        wrong.push(format!(
            "the launch mob's row is not the row of its own experience: {launch_row}"
        ));
    }
    if second_row["can_retire"] != json!(false) || second_row["can_stop_run"] != json!(true) {
        wrong.push(format!(
            "the second mob's row does not offer the stop and no retirement: {second_row}"
        ));
    }
    for (affordance, method) in ACTIONS {
        let served = CONSOLE_METHODS_SERVED_PER_MOB.contains(&method);
        if second_row
            .get(affordance)
            .is_some_and(|offered| offered.as_bool() != Some(served))
        {
            wrong.push(format!(
                "the second mob's row says {affordance} {}, and {method} is served per mob: \
                 {served}",
                second_row[affordance]
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "a row of another mob offers a lifecycle action the door does not serve for it:\n{}",
        wrong.join("\n")
    );
}

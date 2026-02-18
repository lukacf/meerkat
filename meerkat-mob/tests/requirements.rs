use std::collections::BTreeMap;
use std::sync::Arc;

use meerkat_mob::definition::WiringRules;
use meerkat_mob::profile::{Profile, ToolConfig};
use meerkat_mob::{
    MeerkatId, MobBuilder, MobDefinition, MobId, MobState, MobStorage, ProfileName, TaskStatus,
};

mod support;
use support::{TestCommsRuntime, TestSessionService};

fn base_definition() -> MobDefinition {
    let mut profiles = BTreeMap::new();
    profiles.insert(
        ProfileName::from("worker"),
        Profile {
            model: "gpt-5.2".to_string(),
            skills: Vec::new(),
            tools: ToolConfig {
                builtins: true,
                shell: false,
                comms: true,
                memory: true,
                mob: true,
                mob_tasks: true,
                mcp: Vec::new(),
                rust_bundles: Vec::new(),
            },
            peer_description: "worker".to_string(),
            external_addressable: true,
        },
    );

    MobDefinition {
        id: MobId::from("mob-test"),
        orchestrator: None,
        profiles,
        mcp_servers: BTreeMap::new(),
        wiring: WiringRules::default(),
        skills: BTreeMap::new(),
    }
}

#[tokio::test]
async fn req_mob_102_blocked_by_prevents_claiming() {
    let storage = MobStorage::in_memory_with_id(MobId::from("mob-102"));
    let comms = Arc::new(TestCommsRuntime::default());
    let sessions = Arc::new(TestSessionService::new(
        storage.sessions.clone(),
        comms.clone(),
    ));
    let handle = MobBuilder::new(base_definition(), storage)
        .with_session_service(sessions)
        .with_comms_runtime(comms)
        .create()
        .await
        .expect("mob created");

    let blocker = handle
        .task_create("blocker".to_string(), "blocker".to_string(), Vec::new())
        .await
        .expect("blocker created");
    let blocked = handle
        .task_create(
            "blocked".to_string(),
            "blocked".to_string(),
            vec![blocker.clone()],
        )
        .await
        .expect("blocked created");

    let err = handle
        .task_update(
            blocked.clone(),
            Some(TaskStatus::InProgress),
            Some(MeerkatId::from("worker-1")),
        )
        .await
        .expect_err("claim should fail while blocker incomplete");
    let message = err.to_string();
    assert!(
        message.contains("is blocked by"),
        "unexpected error: {message}"
    );

    handle
        .task_update(
            blocker,
            Some(TaskStatus::Done),
            Some(MeerkatId::from("worker-1")),
        )
        .await
        .expect("blocker marked done");
    handle
        .task_update(
            blocked,
            Some(TaskStatus::InProgress),
            Some(MeerkatId::from("worker-2")),
        )
        .await
        .expect("claim succeeds after blocker done");
}

#[tokio::test]
async fn req_mob_110_missing_rust_bundle_fails_spawn() {
    let mut definition = base_definition();
    if let Some(profile) = definition.profiles.get_mut(&ProfileName::from("worker")) {
        profile.tools.rust_bundles = vec!["missing_bundle".to_string()];
    }
    let storage = MobStorage::in_memory_with_id(MobId::from("mob-110"));
    let comms = Arc::new(TestCommsRuntime::default());
    let sessions = Arc::new(TestSessionService::new(
        storage.sessions.clone(),
        comms.clone(),
    ));
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(sessions)
        .with_comms_runtime(comms)
        .create()
        .await
        .expect("mob created");

    let err = handle
        .spawn(&ProfileName::from("worker"), MeerkatId::from("wk-1"), None)
        .await
        .expect_err("spawn should fail without registered bundle");
    assert!(
        err.to_string().contains("missing rust tool bundle"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn complete_archives_sessions_and_destroy_sets_state() {
    let storage = MobStorage::in_memory_with_id(MobId::from("mob-lifecycle"));
    let comms = Arc::new(TestCommsRuntime::default());
    let sessions = Arc::new(TestSessionService::new(
        storage.sessions.clone(),
        comms.clone(),
    ));
    let handle = MobBuilder::new(base_definition(), storage)
        .with_session_service(sessions)
        .with_comms_runtime(comms)
        .create()
        .await
        .expect("mob created");
    let id = handle
        .spawn(&ProfileName::from("worker"), MeerkatId::from("wk-1"), None)
        .await
        .expect("spawned");

    handle.complete().await.expect("complete succeeds");
    let turn_err = handle
        .external_turn(&id, "ping".to_string())
        .await
        .expect_err("archived session should not accept turns");
    assert!(
        turn_err.to_string().contains("invalid state transition"),
        "unexpected error after complete: {turn_err}"
    );

    handle.destroy().await.expect("destroy succeeds");
    assert_eq!(handle.status(), MobState::Destroyed);
}

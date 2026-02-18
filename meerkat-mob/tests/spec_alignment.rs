use std::collections::BTreeMap;

use meerkat_mob::definition::{OrchestratorConfig, RolePair, WiringRules};
use meerkat_mob::event::MobEventKind;
use meerkat_mob::profile::{Profile, ToolConfig};
use meerkat_mob::{
    MeerkatId, MobBuilder, MobDefinition, MobId, MobStorage, ProfileName, TaskStatus,
};

fn profile_with_tools() -> Profile {
    Profile {
        model: "gpt-4o-mini".to_string(),
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
    }
}

fn base_definition(mob_id: &str) -> MobDefinition {
    let mut profiles = BTreeMap::new();
    profiles.insert(ProfileName::from("coordinator"), profile_with_tools());
    profiles.insert(ProfileName::from("worker"), profile_with_tools());
    MobDefinition {
        id: MobId::from(mob_id),
        orchestrator: Some(OrchestratorConfig {
            profile: ProfileName::from("coordinator"),
            startup_message: Some("coordinate".to_string()),
        }),
        profiles,
        mcp_servers: BTreeMap::new(),
        wiring: WiringRules {
            auto_wire_orchestrator: true,
            role_wiring: vec![RolePair {
                a: ProfileName::from("coordinator"),
                b: ProfileName::from("worker"),
            }],
        },
        skills: BTreeMap::new(),
    }
}

#[tokio::test]
async fn choke_mob_002_and_004_events_and_roster_projection() {
    let handle = MobBuilder::new(
        base_definition("choke-2-4"),
        MobStorage::in_memory_with_id(MobId::from("choke-2-4")),
    )
    .create()
    .await
    .expect("mob created");

    let a = handle
        .spawn(&ProfileName::from("coordinator"), MeerkatId::from("coord"), None)
        .await
        .expect("coordinator spawned");
    let b = handle
        .spawn(&ProfileName::from("worker"), MeerkatId::from("worker"), None)
        .await
        .expect("worker spawned");

    handle.wire(&a, &b).await.expect("wired");
    handle.retire(&b).await.expect("retired");

    let ids = handle
        .list_meerkats(None)
        .into_iter()
        .map(|entry| entry.meerkat_id.as_str().to_string())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["coord".to_string()]);

    let events = handle.poll_events(None, None).await.expect("events");
    assert!(events.iter().any(|event| matches!(event.kind, MobEventKind::MeerkatSpawned { .. })));
    assert!(events.iter().any(|event| matches!(event.kind, MobEventKind::PeersWired { .. })));
    assert!(events.iter().any(|event| matches!(event.kind, MobEventKind::MeerkatRetired { .. })));
}

#[tokio::test]
async fn choke_mob_003_taskboard_projection() {
    let handle = MobBuilder::new(
        base_definition("choke-3"),
        MobStorage::in_memory_with_id(MobId::from("choke-3")),
    )
    .create()
    .await
    .expect("mob created");

    let task = handle
        .task_create("subject".to_string(), "desc".to_string(), Vec::new())
        .await
        .expect("task created");
    handle
        .task_update(task.clone(), Some(TaskStatus::InProgress), Some(MeerkatId::from("coord")))
        .await
        .expect("task updated");

    let projected = handle.task_list().await.expect("task list");
    let entry = projected
        .into_iter()
        .find(|t| t.id == task)
        .expect("projected task");
    assert!(matches!(entry.status, TaskStatus::InProgress));
    assert_eq!(entry.owner.as_ref().map(|id| id.as_str()), Some("coord"));
}

#[tokio::test]
async fn choke_mob_005_concurrent_handle_commands_are_serialized() {
    let handle = MobBuilder::new(
        base_definition("choke-5"),
        MobStorage::in_memory_with_id(MobId::from("choke-5")),
    )
    .create()
    .await
    .expect("mob created");

    let h1 = handle.clone();
    let h2 = handle.clone();
    let spawn_1 = tokio::spawn(async move {
        h1.spawn(&ProfileName::from("worker"), MeerkatId::from("w1"), None)
            .await
    });
    let spawn_2 = tokio::spawn(async move {
        h2.spawn(&ProfileName::from("worker"), MeerkatId::from("w2"), None)
            .await
    });

    spawn_1.await.expect("join 1").expect("spawn 1");
    spawn_2.await.expect("join 2").expect("spawn 2");

    assert_eq!(handle.list_meerkats(None).len(), 2);
}

#[tokio::test]
async fn choke_mob_006_and_e2e_mob_003_resume_roundtrip() {
    let storage = MobStorage::in_memory_with_id(MobId::from("resume-3"));
    let definition = base_definition("resume-3");
    let handle = MobBuilder::new(definition.clone(), storage.clone())
        .create()
        .await
        .expect("mob created");

    let coord = handle
        .spawn(&ProfileName::from("coordinator"), MeerkatId::from("coord"), None)
        .await
        .expect("coord spawned");
    let w1 = handle
        .spawn(&ProfileName::from("worker"), MeerkatId::from("w1"), None)
        .await
        .expect("worker spawned");
    handle.wire(&coord, &w1).await.expect("wired");
    handle.stop().await.expect("stopped");

    let resumed = MobBuilder::for_resume(storage.clone())
        .resume()
        .await
        .expect("resumed");
    let entries = resumed.list_meerkats(None);
    assert_eq!(entries.len(), 2);
    let coord_entry = entries
        .iter()
        .find(|entry| entry.meerkat_id.as_str() == "coord")
        .expect("coord entry");
    assert!(coord_entry.wired_to.iter().any(|peer| peer.as_str() == "w1"));
}

#[tokio::test]
async fn e2e_mob_001_and_004_spawn_wire_and_external_addressability() {
    let mut definition = base_definition("e2e-1-4");
    definition.profiles.insert(
        ProfileName::from("hidden"),
        Profile {
            external_addressable: false,
            ..profile_with_tools()
        },
    );
    let handle = MobBuilder::new(
        definition,
        MobStorage::in_memory_with_id(MobId::from("e2e-1-4")),
    )
    .create()
    .await
    .expect("mob created");

    let coord = handle
        .spawn(&ProfileName::from("coordinator"), MeerkatId::from("coord"), None)
        .await
        .expect("coord spawned");
    let worker = handle
        .spawn(&ProfileName::from("worker"), MeerkatId::from("worker"), None)
        .await
        .expect("worker spawned");
    let hidden = handle
        .spawn(&ProfileName::from("hidden"), MeerkatId::from("hidden"), None)
        .await
        .expect("hidden spawned");

    handle.wire(&coord, &worker).await.expect("wired");
    let ok = handle
        .external_turn(&coord, "hello".to_string())
        .await
        .expect("external turn ok");
    assert!(ok.text.contains("hello"));

    let err = handle
        .external_turn(&hidden, "nope".to_string())
        .await
        .expect_err("hidden should reject external turn");
    assert!(err.to_string().contains("not externally addressable"));
}

#[tokio::test]
async fn e2e_mob_002_and_005_retire_complete_destroy() {
    let storage = MobStorage::in_memory_with_id(MobId::from("e2e-2-5"));
    let definition = base_definition("e2e-2-5");
    let handle = MobBuilder::new(definition, storage.clone())
        .create()
        .await
        .expect("mob created");

    let a = handle
        .spawn(&ProfileName::from("coordinator"), MeerkatId::from("a"), None)
        .await
        .expect("a");
    let b = handle
        .spawn(&ProfileName::from("worker"), MeerkatId::from("b"), None)
        .await
        .expect("b");
    handle.wire(&a, &b).await.expect("wire");
    handle.retire(&b).await.expect("retire");
    assert_eq!(handle.list_meerkats(None).len(), 1);

    handle.complete().await.expect("complete");
    let resume_err = match MobBuilder::for_resume(storage.clone()).resume().await {
        Ok(_) => panic!("resume completed should fail"),
        Err(err) => err,
    };
    assert!(resume_err.to_string().contains("invalid state transition"));

    handle.destroy().await.expect("destroy");
}

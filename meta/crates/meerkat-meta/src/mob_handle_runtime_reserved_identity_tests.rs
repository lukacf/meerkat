//! A member created under one of the console's runtime-plane identities
//! before every creation path refused them still comes up after a restart.
//! A mob built without the guard stands for a build from before the check:
//! the restore admits its members, the runtime names them, a respawn is
//! refused, and the console records none of their events under a
//! runtime-plane identity.

use std::sync::atomic::{AtomicU64, Ordering};

use meerkat_mob::AgentIdentity;
use serde_json::json;

use super::*;
use crate::console_spawn::ConsoleSpawnSeed;
use crate::types::{EventEnvelope, UnifiedEvent};
use crate::unified_runtime::console_events::ConsoleEventStore;

static NEXT_MOB: AtomicU64 = AtomicU64::new(0);

fn definition() -> MobDefinition {
    MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "legacy-reserved-{}-{}"

[profiles.worker]
model = "gpt-5.5"

[profiles.worker.tools]
comms = true
"#,
        std::process::id(),
        NEXT_MOB.fetch_add(1, Ordering::Relaxed)
    ))
    .expect("mob definition")
}

/// One durable lifetime over `dir`; a second call over the same directory
/// restores what the first one left.
async fn boot(dir: &Path, definition: MobDefinition) -> MobRuntime {
    let (storage, provenance) =
        crate::mob_composition_manifest::persistent_mob_storage(dir.join("mob.sqlite3"))
            .expect("mob storage");
    let spec = MobBootstrapSpec::persistent(definition, storage, provenance, dir.to_path_buf(), 4)
        .await
        .expect("persistent spec")
        .with_options(MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(meerkat_client::TestClient::default())),
        });
    Box::pin(MobRuntime::bootstrap(spec))
        .await
        .expect("bootstrap the mob runtime")
}

fn worker(member: &str) -> SpawnMemberSpec {
    SpawnMemberSpec::new(ProfileName::from("worker"), AgentIdentity::from(member))
}

fn agent_event(event_id: String, agent_id: String) -> EventEnvelope<UnifiedEvent> {
    EventEnvelope {
        event_id,
        source: "test".to_string(),
        timestamp_ms: 1,
        event: UnifiedEvent::Agent {
            agent_id,
            event_type: "text_delta".to_string(),
            payload: Some(json!({ "delta": "hello" })),
        },
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_legacy_runtime_plane_member_comes_up_and_the_console_records_none_of_its_events() {
    let dir = tempfile::tempdir().expect("state dir");
    let definition = definition();

    // A build from before the checks: it creates both shapes.
    crate::member_comms_id::set_pre_check_mob_for_test(definition.id.as_str(), true);
    let first = boot(dir.path(), definition.clone()).await;
    let handle = first.handle();
    handle
        .ensure_member(worker("alice"))
        .await
        .expect("seat the fork source");
    // An ordinary member the console attributes to `__console__`.
    Box::pin(
        handle.spawn_spec(worker("labelled").with_labels(BTreeMap::from([(
            "agent_identity".to_string(),
            "__console__".to_string(),
        )]))),
    )
    .await
    .expect("labelled member");
    // A fork child named `_system`: restored through its own overlay, the
    // path where a refusal would stop the whole mob.
    Box::pin(handle.fork_member(&AgentIdentity::from("alice"), worker("_system"), None))
        .await
        .expect("fork-derived member");
    handle.shutdown().await.expect("first lifetime shuts down");
    first.stand_down_child_mobs(stand_down_deadline()).await;
    drop(handle);
    drop(first);
    crate::member_comms_id::set_pre_check_mob_for_test(definition.id.as_str(), false);

    // The guarded build restores them.
    let second = boot(dir.path(), definition).await;
    let handle = second.handle();
    let roster = handle.roster().await;
    for member in ["alice", "labelled", "_system"] {
        assert!(
            roster
                .get_by_identity(&AgentIdentity::from(member))
                .is_some(),
            "{member} is restored"
        );
    }
    let members = handle.list_members().await;
    for member in ["labelled", "_system"] {
        let entry = members
            .iter()
            .find(|entry| entry.agent_identity.as_str() == member)
            .unwrap_or_else(|| panic!("{member} is listed"));
        assert!(
            entry.error.is_none(),
            "{member} keeps running: {:?}",
            entry.error
        );
    }

    // The runtime names each one: the startup warn and `meta/status` list
    // member id, runtime id and reserved identity, nothing else.
    let status = crate::member_comms_id::reserved_identity_members_status(&handle)
        .await
        .expect("meta/status carries the field");
    let mut listed = status
        .as_array()
        .expect("a list")
        .iter()
        .map(|entry| {
            assert_eq!(
                entry.as_object().map(serde_json::Map::len),
                Some(3),
                "{entry}"
            );
            (
                entry["member_id"].as_str().expect("member_id").to_string(),
                entry["runtime_id"]
                    .as_str()
                    .expect("runtime_id")
                    .to_string(),
                entry["identity"].as_str().expect("identity").to_string(),
            )
        })
        .collect::<Vec<_>>();
    listed.sort();
    let runtime_id_of = |member: &str| {
        crate::member_comms_id::runtime_event_alias(
            &roster
                .get_by_identity(&AgentIdentity::from(member))
                .expect("on the roster")
                .agent_runtime_id,
        )
    };
    assert_eq!(
        listed,
        vec![
            (
                "_system".to_string(),
                runtime_id_of("_system"),
                "_system".to_string()
            ),
            (
                "labelled".to_string(),
                runtime_id_of("labelled"),
                "__console__".to_string()
            ),
        ],
        "{status}"
    );

    // The console records none of their events under a runtime-plane
    // identity: their agent events, their spawn projection and their memory
    // events. An ordinary member's events still land.
    let store = ConsoleEventStore::new();
    let sink = crate::console_spawn::ConsoleSpawnSink::new(store.clone());
    for (index, entry) in roster.list().enumerate() {
        store
            .project_unified_event(&agent_event(
                format!("legacy-{index}"),
                crate::member_comms_id::runtime_event_alias(&entry.agent_runtime_id),
            ))
            .await;
        let identity = crate::console_spawn::spawn_seed_console_identity(
            entry.agent_identity.as_str(),
            &entry.labels,
        );
        sink.project_spawned_member(&ConsoleSpawnSeed {
            mob_id: None,
            member_id: entry.agent_identity.to_string(),
            identity: identity.clone(),
            initial_message: Some(json!("kickoff")),
            labels: entry.labels.clone(),
            spawned_by: None,
            spawned_by_session: None,
            via_tool: "spawn_member".to_string(),
        })
        .await;
        store
            .append(identity, None, "memory.harvest.completed", json!({}))
            .await;
    }
    let replay = store.replay_all(None).await.expect("replay");
    let under_runtime_plane = replay
        .iter()
        .filter(|event| crate::console_contracts::is_runtime_plane_identity(&event.identity))
        .map(|event| event.event_type.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        under_runtime_plane,
        vec!["runtime_bootstrapped"],
        "only the store's own bootstrap event carries a runtime-plane identity: {replay:#?}"
    );
    assert!(
        replay
            .iter()
            .any(|event| event.identity == "alice" && event.event_type == "text_delta"),
        "an ordinary member's events still land: {replay:#?}"
    );

    // Creating sources stay refused for them: a respawn mints a new
    // incarnation.
    let respawned = handle.respawn(AgentIdentity::from("labelled"), None).await;
    match &respawned {
        Err(error) => assert!(
            error.to_string().contains("reserved"),
            "the labelled member's respawn is refused for the reservation: {error}"
        ),
        Ok(_) => panic!("a member labelled `__console__` is not respawned"),
    }
    let respawned = handle.respawn(AgentIdentity::from("_system"), None).await;
    assert!(
        respawned.is_err(),
        "a `_system` member is not respawned: {respawned:?}"
    );

    second
        .handle()
        .shutdown()
        .await
        .expect("second lifetime shuts down");
    second.stand_down_child_mobs(stand_down_deadline()).await;
}

fn stand_down_deadline() -> std::time::Instant {
    std::time::Instant::now() + std::time::Duration::from_secs(90)
}

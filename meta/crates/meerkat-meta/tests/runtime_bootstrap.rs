#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args,
    clippy::collapsible_if,
    clippy::redundant_clone,
    clippy::needless_raw_string_hashes,
    clippy::single_match,
    clippy::redundant_closure_for_method_calls,
    clippy::redundant_pattern_matching,
    clippy::ignored_unit_patterns,
    clippy::clone_on_copy,
    clippy::manual_assert,
    clippy::unwrap_in_result,
    clippy::useless_vec
)]
use std::sync::Arc;

use meerkat::{AgentFactory, Config, build_ephemeral_service};
use meerkat_client::TestClient;
use meerkat_meta::{MobBootstrapOptions, MobBootstrapSpec, MobRuntime, RealMobRuntime};
use meerkat_mob::{MobDefinition, MobState, MobStorage, SpawnMemberSpec};
use tempfile::TempDir;

struct RuntimeFixture {
    _temp_dir: TempDir,
    runtime: MobRuntime,
}

/// Verify that `MobRuntime` is the canonical name and `RealMobRuntime` is a
/// backward-compatible alias pointing to the same type.
#[test]
fn mob_runtime_rename_alias_compat() {
    // Both names must resolve to the same concrete type.
    let a = std::any::type_name::<MobRuntime>();
    let b = std::any::type_name::<RealMobRuntime>();
    assert_eq!(a, b);
}

fn spawn_spec(profile: &str, member_id: &str) -> SpawnMemberSpec {
    SpawnMemberSpec::from_wire(
        profile.to_string(),
        member_id.to_string(),
        Some(format!("You are {member_id}. Keep responses concise.").into()),
        None,
        None,
    )
}

async fn build_runtime_fixture() -> RuntimeFixture {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let session_path = temp_dir.path().join("sessions");
    std::fs::create_dir_all(&session_path).expect("session path");

    let factory = AgentFactory::new(&session_path).comms(true);
    let session_service = Arc::new(build_ephemeral_service(factory, Config::default(), 16));

    let definition = MobDefinition::from_toml(
        r#"
[mob]
id = "phase-a-mob"

[profiles.lead]
model = "gpt-5.5"
external_addressable = true

[profiles.lead.tools]
comms = true

[profiles.worker]
model = "gpt-5.5"
external_addressable = true

[profiles.worker.tools]
comms = true
"#,
    )
    .expect("parse test mob definition");

    let runtime = RealMobRuntime::bootstrap(
        MobBootstrapSpec::new(definition, MobStorage::in_memory(), session_service).with_options(
            MobBootstrapOptions {
                allow_ephemeral_sessions: true,
                notify_orchestrator_on_resume: true,
                default_llm_client: Some(Arc::new(TestClient::default())),
            },
        ),
    )
    .await
    .expect("bootstrap runtime");

    RuntimeFixture {
        _temp_dir: temp_dir,
        runtime,
    }
}

#[tokio::test]
#[ignore]
async fn phase_a_runtime_001_bootstrap_discovery_reconcile_spawn_resume_real_mob_path() {
    use meerkat_mob::runtime::reconcile::ReconcileOptions;

    let fixture = build_runtime_fixture().await;
    let handle = fixture.runtime.handle();
    assert_eq!(handle.status().await.unwrap(), MobState::Running);
    assert!(handle.list_members_including_retiring().await.is_empty());

    Box::pin(handle.spawn_spec(spawn_spec("lead", "lead-1")))
        .await
        .expect("spawn lead");

    let discovered_after_spawn = handle.list_members_including_retiring().await;
    assert_eq!(discovered_after_spawn.len(), 1);
    assert_eq!(discovered_after_spawn[0].agent_identity.as_str(), "lead-1");
    assert_eq!(discovered_after_spawn[0].role.as_str(), "lead");
    // meerkat 0.7: roster lifecycle is the machine-projected `status`
    // (MobMemberStatus), not the removed roster-owned MemberState.
    assert_eq!(
        discovered_after_spawn[0].status,
        meerkat_mob::MobMemberStatus::Active
    );

    let reconcile = handle
        .reconcile(
            vec![
                spawn_spec("lead", "lead-1"),
                spawn_spec("worker", "worker-1"),
            ],
            ReconcileOptions { retire_stale: true },
        )
        .await
        .expect("reconcile");

    let desired_ids: Vec<String> = reconcile.desired.iter().map(|id| id.to_string()).collect();
    let retained_ids: Vec<String> = reconcile.retained.iter().map(|id| id.to_string()).collect();
    let spawned_ids: Vec<String> = reconcile
        .spawned
        .iter()
        .map(|r| r.agent_identity.to_string())
        .collect();
    let retired_ids: Vec<String> = reconcile.retired.iter().map(|id| id.to_string()).collect();
    assert_eq!(desired_ids, vec!["lead-1", "worker-1"]);
    assert_eq!(retained_ids, vec!["lead-1"]);
    assert_eq!(spawned_ids, vec!["worker-1"]);
    assert!(retired_ids.is_empty());

    let discovered_after_reconcile = handle.list_members_including_retiring().await;
    assert_eq!(discovered_after_reconcile.len(), 2);
    assert!(
        discovered_after_reconcile
            .iter()
            .any(|member| member.agent_identity.as_str() == "worker-1")
    );

    handle.stop().await.expect("stop runtime");
    assert_eq!(handle.status().await.unwrap(), MobState::Stopped);
    handle.resume().await.expect("resume runtime");
    assert_eq!(handle.status().await.unwrap(), MobState::Running);

    handle.retire_all().await.expect("retire all");
}

#[tokio::test]
#[ignore]
async fn phase_a_runtime_002_reconcile_retires_stale_members_by_default() {
    use meerkat_mob::runtime::reconcile::ReconcileOptions;

    let fixture = build_runtime_fixture().await;
    let handle = fixture.runtime.handle();
    Box::pin(handle.spawn_spec(spawn_spec("lead", "lead-1")))
        .await
        .expect("spawn lead");
    Box::pin(handle.spawn_spec(spawn_spec("worker", "worker-1")))
        .await
        .expect("spawn worker");

    let reconcile = handle
        .reconcile(
            vec![spawn_spec("lead", "lead-1")],
            ReconcileOptions { retire_stale: true },
        )
        .await
        .expect("reconcile");

    let desired_ids: Vec<String> = reconcile.desired.iter().map(|id| id.to_string()).collect();
    let retained_ids: Vec<String> = reconcile.retained.iter().map(|id| id.to_string()).collect();
    let spawned_ids: Vec<String> = reconcile
        .spawned
        .iter()
        .map(|r| r.agent_identity.to_string())
        .collect();
    let retired_ids: Vec<String> = reconcile.retired.iter().map(|id| id.to_string()).collect();
    assert_eq!(desired_ids, vec!["lead-1"]);
    assert_eq!(retained_ids, vec!["lead-1"]);
    assert!(spawned_ids.is_empty());
    assert_eq!(retired_ids, vec!["worker-1"]);

    let discovered = handle.list_members_including_retiring().await;
    assert_eq!(discovered.len(), 1);
    assert_eq!(discovered[0].agent_identity.as_str(), "lead-1");
    handle.retire_all().await.expect("retire all");
}

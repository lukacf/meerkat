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
use std::time::Duration;

use meerkat::{AgentFactory, Config, build_ephemeral_service};
use meerkat_client::TestClient;
use meerkat_meta::{
    DiscoverySpec, ErrorEvent, ErrorHook, MetaConfig, MobBootstrapOptions, MobBootstrapSpec,
    PostReconcileHook, PostSpawnHook, UnifiedRuntime, UnifiedRuntimeReconcileReport,
};
use meerkat_mob::MobHandle;
use meerkat_mob::{MobDefinition, MobSessionService, MobState, MobStorage, SpawnMemberSpec};
use tokio::sync::Mutex;

fn spawn_spec(profile: &str, member_id: &str) -> SpawnMemberSpec {
    SpawnMemberSpec::from_wire(
        profile.to_string(),
        member_id.to_string(),
        Some(format!("You are {member_id}. Keep responses concise.").into()),
        None,
        None,
    )
}

fn build_session_service(temp_dir: &tempfile::TempDir) -> Arc<dyn MobSessionService> {
    let session_path = temp_dir.path().join("sessions");
    std::fs::create_dir_all(&session_path).expect("session path");
    let factory = AgentFactory::new(&session_path).comms(true);
    Arc::new(build_ephemeral_service(factory, Config::default(), 16))
}

fn build_mob_spec(temp_dir: &tempfile::TempDir) -> MobBootstrapSpec {
    let definition = MobDefinition::from_toml(
        r#"
[mob]
id = "hooks-test-mob"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true

[profiles.worker.tools]
comms = true
"#,
    )
    .expect("parse mob definition");

    MobBootstrapSpec::new(
        definition,
        MobStorage::in_memory(),
        build_session_service(temp_dir),
    )
    .with_options(MobBootstrapOptions {
        allow_ephemeral_sessions: true,
        notify_orchestrator_on_resume: true,
        default_llm_client: Some(Arc::new(TestClient::default())),
    })
}

fn empty_module_config() -> MetaConfig {
    MetaConfig {
        modules: vec![],
        discovery: DiscoverySpec {
            namespace: "hooks-test".to_string(),
            modules: vec![],
        },
        pre_spawn: vec![],
    }
}

#[tokio::test]
#[ignore]
async fn post_spawn_hook_receives_spawned_member_id() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let spawned_ids: Arc<Mutex<Vec<Vec<String>>>> = Arc::new(Mutex::new(Vec::new()));
    let captured = spawned_ids.clone();

    let hook: PostSpawnHook = Arc::new(move |ids| {
        let captured = captured.clone();
        Box::pin(async move {
            captured.lock().await.push(ids);
        })
    });

    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .mob_spec(build_mob_spec(&temp_dir))
            .module_config(empty_module_config())
            .timeout(Duration::from_secs(2))
            .post_spawn_hook(hook)
            .build(),
    )
    .await
    .expect("build unified runtime");

    assert_eq!(
        runtime.mob_handle().status().await.unwrap(),
        MobState::Running
    );

    runtime
        .spawn(spawn_spec("worker", "hook-worker-1"))
        .await
        .expect("spawn member");

    let captured = spawned_ids.lock().await;
    assert_eq!(captured.len(), 1, "post-spawn hook should be called once");
    assert_eq!(
        captured[0],
        vec!["hook-worker-1".to_string()],
        "post-spawn hook should receive the spawned member id"
    );

    runtime.shutdown().await;
}

#[tokio::test]
#[ignore]
async fn post_reconcile_hook_receives_reconcile_report() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let reports: Arc<Mutex<Vec<UnifiedRuntimeReconcileReport>>> = Arc::new(Mutex::new(Vec::new()));
    let captured = reports.clone();

    let hook: PostReconcileHook = Arc::new(move |report| {
        let captured = captured.clone();
        Box::pin(async move {
            captured.lock().await.push(report);
        })
    });

    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .mob_spec(build_mob_spec(&temp_dir))
            .module_config(empty_module_config())
            .timeout(Duration::from_secs(2))
            .post_reconcile_hook(hook)
            .build(),
    )
    .await
    .expect("build unified runtime");

    let desired = vec![
        spawn_spec("worker", "reconcile-a"),
        spawn_spec("worker", "reconcile-b"),
    ];
    let report = runtime
        .reconcile(desired)
        .await
        .expect("reconcile should succeed");

    let captured = reports.lock().await;
    assert_eq!(
        captured.len(),
        1,
        "post-reconcile hook should be called once"
    );
    assert_eq!(
        captured[0], report,
        "post-reconcile hook should receive the same report returned by reconcile"
    );
    assert_eq!(captured[0].mob.spawned.len(), 2);
    let spawned_identities: Vec<&str> = captured[0]
        .mob
        .spawned
        .iter()
        .map(|receipt| receipt.agent_identity.as_str())
        .collect();
    assert!(spawned_identities.contains(&"reconcile-a"));
    assert!(spawned_identities.contains(&"reconcile-b"));

    runtime.shutdown().await;
}

#[tokio::test]
#[ignore]
async fn mob_handle_returns_working_handle() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .mob_spec(build_mob_spec(&temp_dir))
            .module_config(empty_module_config())
            .timeout(Duration::from_secs(2))
            .build(),
    )
    .await
    .expect("build unified runtime");

    let handle: MobHandle = runtime.mob_handle();

    // The handle should report running state
    assert_eq!(handle.status().await.unwrap(), MobState::Running);

    // Spawn via the handle directly
    Box::pin(handle.spawn_spec(spawn_spec("worker", "handle-worker-1")))
        .await
        .expect("spawn via mob_handle");

    // Verify the member is visible through the handle
    let members = handle.list_members().await;
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].agent_identity.to_string(), "handle-worker-1");

    runtime.shutdown().await;
}

#[tokio::test]
#[ignore]
async fn no_hook_still_works() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .mob_spec(build_mob_spec(&temp_dir))
            .module_config(empty_module_config())
            .timeout(Duration::from_secs(2))
            .build(),
    )
    .await
    .expect("build unified runtime");

    // spawn without hooks set should work fine
    runtime
        .spawn(spawn_spec("worker", "no-hook-worker"))
        .await
        .expect("spawn without hook");

    // reconcile without hooks set should work fine
    let report = runtime
        .reconcile(vec![
            spawn_spec("worker", "no-hook-worker"),
            spawn_spec("worker", "no-hook-worker-2"),
        ])
        .await
        .expect("reconcile without hook");

    assert_eq!(report.mob.spawned.len(), 1);
    assert_eq!(
        report.mob.spawned[0].agent_identity.as_str(),
        "no-hook-worker-2"
    );

    runtime.shutdown().await;
}

#[tokio::test]
#[ignore]
async fn error_hook_fires_on_spawn_failure() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let errors: Arc<Mutex<Vec<ErrorEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let captured = errors.clone();

    let hook: ErrorHook = Arc::new(move |event| {
        let captured = captured.clone();
        Box::pin(async move {
            captured.lock().await.push(event);
        })
    });

    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .mob_spec(build_mob_spec(&temp_dir))
            .module_config(empty_module_config())
            .timeout(Duration::from_secs(2))
            .on_error(hook)
            .build(),
    )
    .await
    .expect("build unified runtime");

    // Spawn with a non-existent profile to trigger a failure
    let result = runtime
        .spawn(SpawnMemberSpec::from_wire(
            "nonexistent-profile".to_string(),
            "error-hook-agent".to_string(),
            None,
            None,
            None,
        ))
        .await;
    assert!(result.is_err(), "spawn should fail for nonexistent profile");

    // Yield to let the fire-and-forget error hook task complete
    tokio::task::yield_now().await;

    let captured = errors.lock().await;
    assert_eq!(captured.len(), 1, "error hook should fire once");
    match &captured[0] {
        ErrorEvent::SpawnFailure {
            member_id,
            profile,
            error,
        } => {
            assert_eq!(member_id, "error-hook-agent");
            assert_eq!(profile, "nonexistent-profile");
            assert!(!error.is_empty(), "error message should not be empty");
        }
        other => panic!("expected SpawnFailure, got {other:?}"),
    }

    runtime.shutdown().await;
}

#[tokio::test]
#[ignore]
async fn list_members_returns_roster() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .mob_spec(build_mob_spec(&temp_dir))
            .module_config(empty_module_config())
            .timeout(Duration::from_secs(2))
            .build(),
    )
    .await
    .expect("build unified runtime");

    runtime
        .spawn(spawn_spec("worker", "roster-a"))
        .await
        .expect("spawn roster-a");
    runtime
        .spawn(spawn_spec("worker", "roster-b"))
        .await
        .expect("spawn roster-b");

    let members = runtime.mob_handle().list_members_including_retiring().await;
    assert_eq!(members.len(), 2, "should list both members");
    let ids: Vec<String> = members
        .iter()
        .map(|m| m.agent_identity.to_string())
        .collect();
    assert!(
        ids.iter().any(|id| id == "roster-a"),
        "should contain roster-a"
    );
    assert!(
        ids.iter().any(|id| id == "roster-b"),
        "should contain roster-b"
    );

    for m in &members {
        assert_eq!(m.role.as_str(), "worker");
        // meerkat 0.7: roster lifecycle is the machine-projected `status`
        // (MobMemberStatus), not the removed roster-owned MemberState.
        assert_eq!(m.status, meerkat_mob::MobMemberStatus::Active);
    }

    runtime.shutdown().await;
}

#[tokio::test]
#[ignore]
async fn get_member_returns_snapshot_or_none() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .mob_spec(build_mob_spec(&temp_dir))
            .module_config(empty_module_config())
            .timeout(Duration::from_secs(2))
            .build(),
    )
    .await
    .expect("build unified runtime");

    runtime
        .spawn(spawn_spec("worker", "get-member-1"))
        .await
        .expect("spawn get-member-1");

    let handle = runtime.mob_handle();
    let entries = handle.list_members_including_retiring().await;
    let found = entries
        .iter()
        .find(|e| e.agent_identity.as_str() == "get-member-1");
    assert!(found.is_some(), "should find spawned member");
    let snapshot = found.unwrap();
    assert_eq!(snapshot.agent_identity.as_str(), "get-member-1");
    assert_eq!(snapshot.role.as_str(), "worker");
    // meerkat 0.7: machine-projected status replaces MemberState.
    assert_eq!(snapshot.status, meerkat_mob::MobMemberStatus::Active);

    let not_found = entries
        .iter()
        .find(|e| e.agent_identity.as_str() == "nonexistent");
    assert!(not_found.is_none(), "should return None for unknown member");

    runtime.shutdown().await;
}

#[tokio::test]
#[ignore]
async fn retire_member_transitions_state() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .mob_spec(build_mob_spec(&temp_dir))
            .module_config(empty_module_config())
            .timeout(Duration::from_secs(2))
            .build(),
    )
    .await
    .expect("build unified runtime");

    runtime
        .spawn(spawn_spec("worker", "retire-me"))
        .await
        .expect("spawn retire-me");

    runtime
        .mob_handle()
        .retire(meerkat_mob::ids::AgentIdentity::from("retire-me"))
        .await
        .expect("retire should succeed");

    // After retire, the member either shows as "retiring" (if it has active
    // work to drain) or is already gone (idle member disposes immediately).
    let members = runtime.mob_handle().list_members_including_retiring().await;
    let retired = members
        .iter()
        .find(|m| m.agent_identity.as_str() == "retire-me");
    match retired {
        // meerkat 0.7: machine-projected status replaces MemberState.
        Some(m) => assert_eq!(
            m.status,
            meerkat_mob::MobMemberStatus::Retiring,
            "if still visible, status should be retiring"
        ),
        None => {} // idle member was immediately disposed — acceptable
    }

    runtime.shutdown().await;
}

#[tokio::test]
#[ignore]
async fn respawn_member_replaces_member() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .mob_spec(build_mob_spec(&temp_dir))
            .module_config(empty_module_config())
            .timeout(Duration::from_secs(2))
            .build(),
    )
    .await
    .expect("build unified runtime");

    runtime
        .spawn(spawn_spec("worker", "respawn-me"))
        .await
        .expect("spawn respawn-me");

    runtime
        .mob_handle()
        .respawn(meerkat_mob::ids::AgentIdentity::from("respawn-me"), None)
        .await
        .expect("respawn should succeed");

    // After respawn, the member should still exist in the roster
    let entries = runtime.mob_handle().list_members_including_retiring().await;
    let found = entries
        .iter()
        .find(|e| e.agent_identity.as_str() == "respawn-me");
    assert!(found.is_some(), "member should still exist after respawn");

    runtime.shutdown().await;
}

/// meerkat-studio ask K4: a mob-wide external-tools provider installed via
/// `MobBootstrapSpec::with_default_external_tools_provider` must be consulted
/// at EVERY member materialization — the initial spawn AND the respawn
/// replacement — unlike the per-spawn `SpawnMemberSpec.external_tools`
/// overlay, which revival drops.
#[tokio::test]
async fn default_external_tools_provider_survives_respawn() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct NoTools;
    #[async_trait::async_trait]
    impl meerkat_core::AgentToolDispatcher for NoTools {
        fn tools(&self) -> Arc<[Arc<meerkat_core::types::ToolDef>]> {
            Vec::<Arc<meerkat_core::types::ToolDef>>::new().into()
        }
        async fn dispatch(
            &self,
            call: meerkat_core::types::ToolCallView<'_>,
        ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
            Ok(meerkat_core::ToolDispatchOutcome::sync_result(
                meerkat_core::types::ToolResult::new(call.id.to_string(), "{}".to_string(), false),
            ))
        }
        fn capabilities(&self) -> meerkat_core::agent::DispatcherCapabilities {
            meerkat_core::agent::DispatcherCapabilities::default()
        }
    }

    let calls = Arc::new(AtomicUsize::new(0));
    let provider_calls = calls.clone();
    let provider: meerkat_mob::ExternalToolsProvider = Arc::new(move || {
        provider_calls.fetch_add(1, Ordering::SeqCst);
        Some(Arc::new(NoTools) as Arc<dyn meerkat_core::AgentToolDispatcher>)
    });

    let temp_dir = tempfile::tempdir().expect("temp dir");
    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .mob_spec(build_mob_spec(&temp_dir).with_default_external_tools_provider(provider))
            .module_config(empty_module_config())
            .timeout(Duration::from_secs(2))
            .build(),
    )
    .await
    .expect("build unified runtime");

    runtime
        .spawn(spawn_spec("worker", "tooled-member"))
        .await
        .expect("spawn tooled-member");
    let after_spawn = calls.load(Ordering::SeqCst);
    assert!(
        after_spawn >= 1,
        "provider must be consulted at initial spawn"
    );

    runtime
        .mob_handle()
        .respawn(meerkat_mob::ids::AgentIdentity::from("tooled-member"), None)
        .await
        .expect("respawn should succeed");
    assert!(
        calls.load(Ordering::SeqCst) > after_spawn,
        "provider must be consulted again at respawn (revival-surviving tools)"
    );

    runtime.shutdown().await;
}

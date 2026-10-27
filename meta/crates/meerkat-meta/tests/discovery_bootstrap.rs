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
//! Integration tests for MK-001 (Discovery trait), MK-002 (Bootstrap integration),
//! and MK-007 (spawn_many).

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use meerkat::{AgentFactory, Config, build_ephemeral_service};
use meerkat_client::TestClient;
use meerkat_meta::{
    AgentDiscoverySpec, Discovery, DiscoverySpec, MetaConfig, MobBootstrapOptions,
    MobBootstrapSpec, UnifiedRuntime, discovery_spec_to_spawn_spec,
};
use meerkat_mob::{MobDefinition, MobStorage, SpawnMemberSpec};
use serde_json::json;

struct MockDiscovery {
    specs: Vec<AgentDiscoverySpec>,
}

impl Discovery for MockDiscovery {
    fn discover(
        &self,
        _context: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Vec<AgentDiscoverySpec>> + Send + '_>> {
        let specs = self.specs.clone();
        Box::pin(async move { specs })
    }
}

fn build_session_service(temp_dir: &tempfile::TempDir) -> Arc<dyn meerkat_mob::MobSessionService> {
    let session_path = temp_dir.path().join("sessions");
    std::fs::create_dir_all(&session_path).expect("session path");
    let factory = AgentFactory::new(&session_path).comms(true);
    Arc::new(build_ephemeral_service(factory, Config::default(), 16))
}

fn build_mob_spec(temp_dir: &tempfile::TempDir) -> MobBootstrapSpec {
    let session_service = build_session_service(temp_dir);
    let definition = MobDefinition::from_toml(
        r#"
[mob]
id = "mk001-discovery-mob"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true

[profiles.worker.tools]
comms = true
"#,
    )
    .expect("parse mob definition");

    MobBootstrapSpec::new(definition, MobStorage::in_memory(), session_service).with_options(
        MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(TestClient::default())),
        },
    )
}

fn empty_module_config() -> MetaConfig {
    MetaConfig {
        modules: vec![],
        discovery: DiscoverySpec {
            namespace: "mk001-test".to_string(),
            modules: vec![],
        },
        pre_spawn: vec![],
    }
}

#[test]
fn mk001_discovery_spec_to_spawn_spec_maps_all_fields() {
    let spec = AgentDiscoverySpec {
        profile: "worker".to_string(),
        meerkat_id: "agent-1".to_string(),
        labels: Some(BTreeMap::from([
            ("team".to_string(), "alpha".to_string()),
            ("role".to_string(), "analyst".to_string()),
        ])),
        context: Some(json!({"env": "staging"})),
        additional_instructions: vec!["Be concise.".to_string()],
        resume_session_id: None,
    };

    let spawn = discovery_spec_to_spawn_spec(&spec);

    assert_eq!(spawn.role_name.as_str(), "worker");
    assert_eq!(spawn.identity.as_str(), "agent-1");
    assert!(
        spawn.initial_message.is_none(),
        "additional_instructions should not map to initial_message"
    );
    // additional_instructions maps directly to SpawnMemberSpec.additional_instructions
    assert_eq!(
        spawn.additional_instructions.as_deref(),
        Some(&["Be concise.".to_string()][..])
    );
    // context passes through unchanged
    let ctx = spawn.context.as_ref().expect("context should be present");
    assert_eq!(ctx["env"], "staging");
    let labels = spawn.labels.as_ref().expect("labels should be present");
    assert_eq!(labels.get("team").map(String::as_str), Some("alpha"));
    assert_eq!(labels.get("role").map(String::as_str), Some("analyst"));
    assert!(matches!(
        spawn.launch_mode,
        meerkat_mob::launch::MemberLaunchMode::Fresh
    ));
    assert!(spawn.runtime_mode.is_none());
    assert!(spawn.backend.is_none());
}

#[test]
fn mk001_discovery_spec_to_spawn_spec_handles_resume_session_id() {
    let session_uuid = "01933ee4-0fc2-7fa9-ae4f-6b2cb9571530";
    let spec = AgentDiscoverySpec {
        profile: "worker".to_string(),
        meerkat_id: "agent-resume".to_string(),
        labels: None,
        context: None,
        additional_instructions: vec![],
        resume_session_id: Some(session_uuid.to_string()),
    };

    let spawn = discovery_spec_to_spawn_spec(&spec);
    let sid = match &spawn.launch_mode {
        meerkat_mob::launch::MemberLaunchMode::Resume {
            bridge_session_id, ..
        } => bridge_session_id.clone(),
        other => panic!("expected Resume launch mode, got {other:?}"),
    };
    assert_eq!(sid.to_string(), session_uuid);
}

#[test]
fn mk001_discovery_spec_to_spawn_spec_ignores_invalid_session_id() {
    let spec = AgentDiscoverySpec {
        profile: "worker".to_string(),
        meerkat_id: "agent-bad-session".to_string(),
        labels: None,
        context: None,
        additional_instructions: vec![],
        resume_session_id: Some("not-a-uuid".to_string()),
    };

    let spawn = discovery_spec_to_spawn_spec(&spec);
    assert!(
        matches!(
            spawn.launch_mode,
            meerkat_mob::launch::MemberLaunchMode::Fresh
        ),
        "invalid session ID should be silently ignored"
    );
}

#[test]
fn mk001_discovery_spec_to_spawn_spec_minimal() {
    let spec = AgentDiscoverySpec {
        profile: "lead".to_string(),
        meerkat_id: "leader".to_string(),
        labels: None,
        context: None,
        additional_instructions: vec![],
        resume_session_id: None,
    };

    let spawn = discovery_spec_to_spawn_spec(&spec);
    assert_eq!(spawn.role_name.as_str(), "lead");
    assert_eq!(spawn.identity.as_str(), "leader");
    assert!(spawn.initial_message.is_none());
    assert!(spawn.context.is_none());
    assert!(spawn.labels.is_none());
    assert!(matches!(
        spawn.launch_mode,
        meerkat_mob::launch::MemberLaunchMode::Fresh
    ));
}

#[test]
fn mk001_discovery_spec_to_spawn_spec_keeps_colon_identity_in_public_alias_space() {
    // The encode to meerkat 0.7's comms-safe roster id happens once, inside
    // `UnifiedRuntime::spawn`/`spawn_many` (and `reconcile`). Encoding here
    // too would double-encode (`mk--` is a reserved, non-idempotent marker).
    let spec = AgentDiscoverySpec {
        profile: "lead".to_string(),
        meerkat_id: "domain:billing".to_string(),
        labels: None,
        context: None,
        additional_instructions: vec![],
        resume_session_id: None,
    };

    let spawn = discovery_spec_to_spawn_spec(&spec);
    assert_eq!(spawn.identity.as_str(), "domain:billing");
}

#[test]
fn mk001_agent_discovery_spec_serde_roundtrip() {
    let spec = AgentDiscoverySpec {
        profile: "worker".to_string(),
        meerkat_id: "agent-serde".to_string(),
        labels: Some(BTreeMap::from([("env".to_string(), "prod".to_string())])),
        context: Some(json!({"key": "value"})),
        additional_instructions: vec!["Follow protocol X.".to_string()],
        resume_session_id: Some("01933ee4-0fc2-7fa9-ae4f-6b2cb9571530".to_string()),
    };

    let json = serde_json::to_string(&spec).expect("serialize");
    let deserialized: AgentDiscoverySpec = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(deserialized, spec);
}

#[test]
fn mk001_agent_discovery_spec_serde_minimal_omits_none_fields() {
    let spec = AgentDiscoverySpec {
        profile: "worker".to_string(),
        meerkat_id: "agent-min".to_string(),
        labels: None,
        context: None,
        additional_instructions: vec![],
        resume_session_id: None,
    };

    let json = serde_json::to_string(&spec).expect("serialize");
    assert!(!json.contains("labels"));
    assert!(!json.contains("context"));
    assert!(!json.contains("additional_instructions"));
    assert!(!json.contains("resume_session_id"));

    let deserialized: AgentDiscoverySpec = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(deserialized, spec);
}

#[tokio::test]
#[ignore]
async fn mk007_spawn_many_spawns_multiple_agents() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let runtime = UnifiedRuntime::bootstrap(
        build_mob_spec(&temp_dir),
        empty_module_config(),
        Duration::from_secs(2),
    )
    .await
    .expect("bootstrap");

    let specs = vec![
        SpawnMemberSpec::from_wire("worker".into(), "w-1".into(), None, None, None),
        SpawnMemberSpec::from_wire("worker".into(), "w-2".into(), None, None, None),
        SpawnMemberSpec::from_wire("worker".into(), "w-3".into(), None, None, None),
    ];

    let refs = runtime.spawn_many(specs).await.expect("spawn_many");
    assert_eq!(refs.len(), 3, "spawn_many should return 3 member refs");

    runtime.shutdown().await;
}

#[tokio::test]
async fn unified_runtime_raw_spawn_apis_reject_authoritative_identity_labels() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let runtime = UnifiedRuntime::bootstrap(
        build_mob_spec(&temp_dir),
        empty_module_config(),
        Duration::from_secs(2),
    )
    .await
    .expect("bootstrap");
    let forged_labels =
        BTreeMap::from([("agent_identity".to_string(), "durable:forged".to_string())]);

    let single_error = runtime
        .spawn(
            SpawnMemberSpec::from_wire("worker".into(), "raw-single".into(), None, None, None)
                .with_labels(forged_labels.clone()),
        )
        .await
        .expect_err("single raw spawn must reject runtime-authoritative labels");
    assert!(single_error.to_string().contains("runtime-authoritative"));

    let batch_error = runtime
        .spawn_many(vec![
            SpawnMemberSpec::from_wire("worker".into(), "raw-batch".into(), None, None, None)
                .with_labels(forged_labels),
        ])
        .await
        .expect_err("batch raw spawn must reject runtime-authoritative labels");
    assert!(batch_error.to_string().contains("runtime-authoritative"));
    assert!(runtime.mob_handle().list_members().await.is_empty());

    runtime.shutdown().await;
}

#[tokio::test]
#[ignore]
async fn mk002_builder_with_discovery_spawns_discovered_agents() {
    let temp_dir = tempfile::tempdir().expect("temp dir");

    let discovery = MockDiscovery {
        specs: vec![
            AgentDiscoverySpec {
                profile: "worker".to_string(),
                meerkat_id: "disc-1".to_string(),
                labels: Some(BTreeMap::from([("tier".to_string(), "1".to_string())])),
                context: Some(json!({"zone": "us-east"})),
                additional_instructions: vec![],
                resume_session_id: None,
            },
            AgentDiscoverySpec {
                profile: "worker".to_string(),
                meerkat_id: "disc-2".to_string(),
                labels: None,
                context: None,
                additional_instructions: vec!["Be brief.".to_string()],
                resume_session_id: None,
            },
        ],
    };

    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .mob_spec(build_mob_spec(&temp_dir))
            .module_config(empty_module_config())
            .timeout(Duration::from_secs(2))
            .discovery(discovery)
            .build(),
    )
    .await
    .expect("build with discovery");

    // Reconcile to get current roster and verify the discovered agents are present.
    let desired: Vec<SpawnMemberSpec> = vec![
        SpawnMemberSpec::from_wire("worker".into(), "disc-1".into(), None, None, None),
        SpawnMemberSpec::from_wire("worker".into(), "disc-2".into(), None, None, None),
    ];
    let report = runtime.reconcile(desired).await.expect("reconcile");
    assert_eq!(
        report.mob.retained,
        vec!["disc-1".to_string(), "disc-2".to_string()],
        "both discovered agents should already be present (retained, not spawned)"
    );
    assert!(
        report.mob.spawned.is_empty(),
        "no new agents should be spawned since they were already discovered"
    );

    runtime.shutdown().await;
}

#[tokio::test]
#[ignore]
async fn mk002_builder_pre_spawn_hook_runs_before_discovery() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let temp_dir = tempfile::tempdir().expect("temp dir");
    let hook_ran = Arc::new(AtomicBool::new(false));
    let hook_ran_clone = hook_ran.clone();

    let hook: meerkat_meta::unified_runtime::PreSpawnHook = Box::new(move || {
        let flag = hook_ran_clone.clone();
        Box::pin(async move {
            flag.store(true, Ordering::SeqCst);
            Ok(serde_json::Value::Null)
        })
    });

    let discovery = MockDiscovery {
        specs: vec![AgentDiscoverySpec {
            profile: "worker".to_string(),
            meerkat_id: "hook-agent".to_string(),
            labels: None,
            context: None,
            additional_instructions: vec![],
            resume_session_id: None,
        }],
    };

    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .mob_spec(build_mob_spec(&temp_dir))
            .module_config(empty_module_config())
            .timeout(Duration::from_secs(2))
            .pre_spawn_hook(hook)
            .discovery(discovery)
            .build(),
    )
    .await
    .expect("build with hook");

    assert!(
        hook_ran.load(Ordering::SeqCst),
        "pre-spawn hook should have executed"
    );

    runtime.shutdown().await;
}

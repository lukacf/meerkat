#![allow(clippy::expect_used)]

use std::sync::Arc;

use indexmap::IndexMap;
use meerkat_core::types::ContentInput;
use meerkat_mob::definition::{
    BackendConfig, CollectionPolicy, DependencyMode, DispatchMode, FlowSpec, FlowStepSpec,
    LimitsSpec, OrchestratorConfig, StepOutputFormat, WiringRules,
};
use meerkat_mob::runtime::FlowRunKernel;
use meerkat_mob::{
    FlowId, FlowRunConfig, InMemoryMobEventStore, InMemoryMobRunStore, MobDefinition, MobEventKind,
    MobEventStore, MobId, MobRunStatus, MobRunStore, MobRuntimeMode, Profile, ProfileName,
    ToolConfig,
};
use std::collections::BTreeMap;

fn owner_test_config() -> FlowRunConfig {
    let mut steps = IndexMap::new();
    steps.insert(
        meerkat_mob::StepId::from("s1"),
        FlowStepSpec {
            role: ProfileName::from("worker"),
            message: ContentInput::from("do it"),
            depends_on: Vec::new(),
            dispatch_mode: DispatchMode::FanOut,
            collection_policy: CollectionPolicy::All,
            condition: None,
            timeout_ms: Some(2_000),
            expected_schema_ref: None,
            branch: None,
            depends_on_mode: DependencyMode::All,
            allowed_tools: None,
            blocked_tools: None,
            output_format: StepOutputFormat::Json,
        },
    );

    let mut flows = BTreeMap::new();
    flows.insert(
        FlowId::from("demo"),
        FlowSpec {
            description: Some("owner test flow".into()),
            steps,
        },
    );

    let mut profiles = BTreeMap::new();
    profiles.insert(
        ProfileName::from("lead"),
        Profile {
            model: "model".into(),
            skills: Vec::new(),
            tools: ToolConfig::default(),
            peer_description: "lead".into(),
            external_addressable: true,
            backend: None,
            runtime_mode: MobRuntimeMode::AutonomousHost,
            max_inline_peer_notifications: None,
            output_schema: None,
            provider_params: None,
        },
    );
    profiles.insert(
        ProfileName::from("worker"),
        Profile {
            model: "model".into(),
            skills: Vec::new(),
            tools: ToolConfig::default(),
            peer_description: "worker".into(),
            external_addressable: false,
            backend: None,
            runtime_mode: MobRuntimeMode::AutonomousHost,
            max_inline_peer_notifications: None,
            output_schema: None,
            provider_params: None,
        },
    );

    let definition = MobDefinition {
        id: MobId::from("mob-owner-test"),
        orchestrator: Some(OrchestratorConfig {
            profile: ProfileName::from("lead"),
        }),
        profiles,
        mcp_servers: BTreeMap::new(),
        wiring: WiringRules::default(),
        skills: BTreeMap::new(),
        backend: BackendConfig::default(),
        flows,
        topology: None,
        supervisor: None,
        limits: Some(LimitsSpec {
            max_flow_duration_ms: Some(60_000),
            max_step_retries: Some(1),
            max_orphaned_turns: Some(8),
            cancel_grace_timeout_ms: None,
        }),
        spawn_policy: None,
        event_router: None,
    };

    FlowRunConfig::from_definition(FlowId::from("demo"), &definition).expect("flow config")
}

#[tokio::test]
async fn flow_run_kernel_persists_pending_and_terminal_truth_for_machine_verify() {
    let run_store = Arc::new(InMemoryMobRunStore::new());
    let events = Arc::new(InMemoryMobEventStore::new());
    let kernel = FlowRunKernel::new(
        MobId::from("mob-owner-test"),
        run_store.clone(),
        events.clone(),
    );
    let config = owner_test_config();

    let run_id = kernel
        .create_pending_run(&config, serde_json::json!({"entry":"owner-test"}))
        .await
        .expect("create pending run");
    let pending = run_store
        .get_run(&run_id)
        .await
        .expect("load pending run")
        .expect("pending run should exist");
    assert_eq!(pending.status, MobRunStatus::Pending);

    let started = kernel.start_run(&run_id).await.expect("start run");
    assert!(started, "pending run should start");

    let step_id = meerkat_mob::StepId::from("s1");
    let dispatched = kernel
        .dispatch_step(&run_id, &step_id)
        .await
        .expect("dispatch step");
    assert!(dispatched, "single-step flow should dispatch");

    let completed_step = kernel
        .complete_step(&run_id, &step_id)
        .await
        .expect("complete step");
    assert!(completed_step, "dispatched step should complete");

    let _ = kernel
        .terminalize_completed(run_id.clone(), FlowId::from("demo"))
        .await
        .expect("terminalize completed");

    let completed = run_store
        .get_run(&run_id)
        .await
        .expect("load completed run")
        .expect("completed run should exist");
    assert_eq!(completed.status, MobRunStatus::Completed);

    let replay = run_store
        .list_runs(&MobId::from("mob-owner-test"), None)
        .await
        .expect("list runs");
    assert_eq!(
        replay.len(),
        1,
        "owner test should observe one durable run record"
    );

    let events = events.replay_all().await.expect("replay events");
    assert!(events.iter().any(|event| matches!(
        &event.kind,
        MobEventKind::FlowCompleted { run_id: event_run_id, .. } if event_run_id == &run_id
    )));
}

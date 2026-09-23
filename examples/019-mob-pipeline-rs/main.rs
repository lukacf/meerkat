//! # 019 — Mob: Pipeline (Rust)
//!
//! Builds and validates a staged mob definition, spawns specialized workers,
//! wires their topology, and manually submits illustrative lint and test turns.
//! It does not execute the declared flow or enforce pass/fail gating.
//!
//! ## What you'll learn
//! - Staged profiles, skills, topology, limits, and a sample flow DAG
//! - Spawning stage-specific workers
//! - Running manual turns on individual stage members
//! - Validating a `MobDefinition` before creation
//!
//! Note: Uses `build_ephemeral_service` (in-memory substrate) for simplicity.
//! Production pipelines use the runtime-backed path.
//!
//! ## Run
//! ```bash
//! ANTHROPIC_API_KEY=... ./scripts/repo-cargo run -p meerkat-mob --example 019-mob-pipeline
//! ```

use std::sync::Arc;
use std::{io::Write, time::Duration};

use meerkat::{AgentFactory, Config, build_ephemeral_service};
use meerkat_mob::{
    AgentIdentity, MobBuilder, MobDefinition, MobEventKind, MobRuntimeMode, MobSessionService,
    MobStorage, SpawnMemberSpec, validate_definition,
};

#[path = "../017-mob-coding-swarm-rs/tracked_turn.rs"]
mod tracked_turn;
use tracked_turn::DemoError;

const PIPELINE_TOML: &str = r#"
[mob]
id = "pipeline"
orchestrator = "lead"

[profiles.lead]
model = "claude-opus-4-8"
skills = ["orchestrator"]
peer_description = "Orchestrator"
external_addressable = true

[profiles.lead.tools]
builtins = true
comms = true
mob = true

[profiles.worker]
model = "claude-sonnet-4-6"
skills = ["worker"]
peer_description = "Worker"
external_addressable = false

[profiles.worker.tools]
builtins = true
shell = true
comms = true

[wiring]
auto_wire_orchestrator = true

[skills.orchestrator]
source = "inline"
content = "Drive staged pipeline execution: advance stages sequentially, collect handoff artifacts."

[skills.worker]
source = "inline"
content = "Execute your stage deterministically and emit handoff artifacts."

[flows.pipeline]
description = "pipeline flow"

[flows.pipeline.steps.start]
role = "lead"
message = "go"
dispatch_mode = "one_to_one"
depends_on_mode = "all"

[flows.pipeline.steps.branch_a]
role = "worker"
message = "a"
depends_on = ["start"]
branch = "choose"
condition = { op = "eq", path = "params.choice", value = "a" }

[flows.pipeline.steps.branch_b]
role = "worker"
message = "b"
depends_on = ["start"]
branch = "choose"
condition = { op = "eq", path = "params.choice", value = "b" }

[flows.pipeline.steps.join]
role = "lead"
message = "join"
depends_on = ["branch_a", "branch_b"]
depends_on_mode = "any"
collection_policy = { type = "quorum", n = 1 }
timeout_ms = 1000
expected_schema_ref = '{"type":"object","properties":{"summary":{"type":"string"}},"required":["summary"],"additionalProperties":false}'

[topology]
mode = "strict"
rules = [{ from_role = "lead", to_role = "worker", allowed = true }]

[supervisor]
role = "lead"
escalation_threshold = 2

[limits]
max_flow_duration_ms = 30000
max_step_retries = 1
max_orphaned_turns = 8
"#;

/// Format a mob event kind into a short human-readable label.
fn event_label(kind: &MobEventKind) -> &'static str {
    match kind {
        MobEventKind::MobCreated { .. } => "MobCreated",
        MobEventKind::MobCompleted => "MobCompleted",
        MobEventKind::MobReset => "MobReset",
        MobEventKind::MemberSpawned(..) => "MemberSpawned",
        MobEventKind::MemberRetired { .. } => "MemberRetired",
        MobEventKind::FlowStarted { .. } => "FlowStarted",
        MobEventKind::FlowCompleted { .. } => "FlowCompleted",
        MobEventKind::FlowFailed { .. } => "FlowFailed",
        MobEventKind::FlowCanceled { .. } => "FlowCanceled",
        MobEventKind::StepCompleted { .. } => "StepCompleted",
        _ => "Other",
    }
}

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    meerkat_runtime::host_stack::run_host("019-mob-pipeline", async_main)?
}

async fn async_main() -> Result<(), DemoError> {
    let _api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| "Set ANTHROPIC_API_KEY to run this example")?;
    let directory = tempfile::tempdir_in(".")?;
    let store_path = directory.path().join("sessions");
    std::fs::create_dir_all(&store_path)?;
    let factory = AgentFactory::new(&store_path).comms(true);
    let service = Arc::new(build_ephemeral_service(factory, Config::default(), 16));
    run_pipeline(service, Duration::from_secs(60), &mut std::io::stdout()).await
}

async fn run_pipeline(
    session_service: Arc<impl MobSessionService + 'static>,
    turn_timeout: Duration,
    output: &mut (impl Write + Send),
) -> Result<(), DemoError> {
    macro_rules! println {
        ($($args:tt)*) => { writeln!(output, $($args)*)? };
    }

    // ── Part 1: Explore the pipeline definition ──────────────────────────────
    println!("=== Mob: Pipeline ===\n");
    println!("{PIPELINE_TOML}\n");

    // ── Part 2: Custom CI/CD pipeline definition (TOML) ──────────────────────

    println!("=== Custom CI/CD Pipeline ===\n");

    let pipeline_toml = r#"
[mob]
id = "cicd-pipeline"
orchestrator = "coordinator"

[profiles.coordinator]
model = "claude-opus-4-8"
skills = ["pipeline-coordinator"]
peer_description = "Pipeline coordinator -- drives sequential stage execution"
external_addressable = true

[profiles.coordinator.tools]
builtins = true
comms = true
mob = true

[profiles.linter]
model = "claude-sonnet-4-6"
skills = ["lint-stage"]
peer_description = "Stage 1: Code linting and style checks"
external_addressable = true

[profiles.linter.tools]
builtins = true
comms = true

[profiles.tester]
model = "claude-sonnet-4-6"
skills = ["test-stage"]
peer_description = "Stage 2: Test execution and coverage analysis"
external_addressable = true

[profiles.tester.tools]
builtins = true
comms = true

[profiles.deployer]
model = "claude-sonnet-4-6"
skills = ["deploy-stage"]
peer_description = "Stage 3: Deployment and smoke testing"

[profiles.deployer.tools]
builtins = true
comms = true

[wiring]
auto_wire_orchestrator = true

[skills.pipeline-coordinator]
source = "inline"
content = """
## Role
Drive staged CI/CD pipeline execution.

## Stages
1. Lint: Code quality checks
2. Test: Unit and integration tests
3. Deploy: Release and smoke tests

## Rules
- Stages execute sequentially
- A stage must PASS before the next starts
- On failure: report and stop
"""

[skills.lint-stage]
source = "inline"
content = "Run code linting: check formatting and style. Report pass/fail."

[skills.test-stage]
source = "inline"
content = "Run test suite: check all tests pass. Report coverage and failures."

[skills.deploy-stage]
source = "inline"
content = "Execute deployment: build release, run smoke tests. Report pass/fail."
"#;

    let definition = MobDefinition::from_toml(pipeline_toml)?;
    println!("Pipeline: {}", definition.id);
    println!("Stages:");
    for (name, binding) in &definition.profiles {
        if let Some(profile) = binding.as_inline()
            && name.as_str() != "coordinator"
        {
            println!("  {name} -- {}", profile.peer_description);
        }
    }

    let diagnostics = validate_definition(&definition);
    println!(
        "Validation: {}\n",
        if diagnostics.is_empty() {
            "PASSED"
        } else {
            "ISSUES FOUND"
        }
    );

    // ── Part 3: Create and run a real pipeline mob ───────────────────────────

    println!("=== Live Pipeline Execution ===\n");

    // Create the pipeline mob.
    let storage = MobStorage::in_memory();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(session_service)
        .allow_ephemeral_sessions(true)
        .create()
        .await?;

    let result = async {
    println!(
        "Pipeline '{}' created (status: {:?})",
        handle.mob_id(),
        handle.status().await?
    );

    // Spawn the coordinator.
    println!("\nSpawning coordinator...");
    let coord_spec = SpawnMemberSpec::new("coordinator", "coordinator-1")
        .with_runtime_mode(MobRuntimeMode::TurnDriven);
    let coord_ref = handle.spawn_spec(coord_spec).await?;
    println!("  Spawned coordinator-1: {coord_ref:?}");

    // Spawn pipeline stage workers sequentially.
    let stages = [("linter", "lint-1"), ("tester", "test-1"), ("deployer", "deploy-1")];

    for (profile, id) in &stages {
        let spec = SpawnMemberSpec::new(*profile, *id)
            .with_runtime_mode(MobRuntimeMode::TurnDriven);
        let spawn_result = handle.spawn_spec(spec).await?;
        println!("  Spawned {id} ({profile}): {spawn_result:?}");
    }

    // Wire coordinator to all stages, and chain stages sequentially.
    for (_, id) in &stages {
        handle
            .wire(
                AgentIdentity::from("coordinator-1"),
                AgentIdentity::from(*id),
            )
            .await?;
    }
    // Chain: lint -> test -> deploy
    handle
        .wire(AgentIdentity::from("lint-1"), AgentIdentity::from("test-1"))
        .await?;
    handle
        .wire(
            AgentIdentity::from("test-1"),
            AgentIdentity::from("deploy-1"),
        )
        .await?;
    println!("  Wired pipeline topology");

    // Show the roster.
    let members = handle.list_members().await;
    println!("\nRoster ({} members):", members.len());
    for m in &members {
        println!(
            "  {} (profile: {}, wired_to: {:?})",
            m.agent_identity, m.role, m.wired_to
        );
    }

    // Run the first pipeline stage: send a lint request to the linter.
    println!("\n--- Stage 1: Lint ---");
    println!("Sending lint request (live LLM call)...");
    tracked_turn::report_turn(
        &handle.member(&AgentIdentity::from("lint-1")).await?,
            "Analyze this Rust function for style issues. Report PASS or FAIL with a one-line reason. \
             Do NOT use any tools -- respond in plain text only.\n\n\
             ```rust\n\
             fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\
             ```",
        turn_timeout,
        output,
    )
        .await?;

    println!("Lint turn completed (manual demonstration; no pass/fail gate).");

    // Run the second stage: send a test request to the tester.
    println!("\n--- Stage 2: Test ---");
    println!("Sending test request (live LLM call)...");
    tracked_turn::report_turn(
        &handle.member(&AgentIdentity::from("test-1")).await?,
            "Independently evaluate the test coverage for this function. \
             Report PASS or FAIL with a one-line summary. \
             Do NOT use any tools -- respond in plain text only.\n\n\
             ```rust\n\
             fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\n\
             #[test]\n\
             fn test_add() {\n    assert_eq!(add(2, 3), 5);\n}\n\
             ```",
        turn_timeout,
        output,
    )
        .await?;

    println!("Test turn completed (manual demonstration; no pass/fail gate).");

    // Poll all mob events.
    let events = handle.poll_events(0, 50).await?;
    println!("\nMob events ({} total):", events.len());
    for event in &events {
        println!("  cursor={}: {}", event.cursor, event_label(&event.kind));
    }

    // Final status.
    println!("\nPipeline status: {:?}", handle.status().await?);
    println!("Members: {}", handle.list_members().await.len());
    Ok::<_, DemoError>(())
    }.await;

    // Cleanup runs on both successful observation and typed failure/timeout.
    let cleanup = handle.retire_all().await;
    let shutdown = handle.shutdown().await;
    if let Err(error) = &cleanup {
        eprintln!("Pipeline cleanup failed: {error}");
    }
    if let Err(error) = &shutdown {
        eprintln!("Pipeline shutdown failed: {error}");
    }
    result?;
    cleanup?;
    shutdown?;
    println!("All stage workers retired. Demonstration complete.");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat::{EphemeralSessionService, FactoryAgentBuilder, SessionService};
    use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
    use meerkat_mob::definition::FlowSchemaRef;
    use meerkat_mob::{BoundedTurnFailure, BoundedTurnWaitError, MemberDeliveryReceipt};
    use std::sync::Mutex;
    use tokio::sync::Notify;

    static PIPELINE_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[derive(Default)]
    struct StageClient {
        entered: Notify,
        release: Notify,
        requests: Mutex<Vec<LlmRequest>>,
        fail_on: Option<usize>,
    }

    #[async_trait::async_trait]
    impl LlmClient for StageClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(&'a self, request: &'a LlmRequest) -> meerkat_client::types::LlmStream<'a> {
            Box::pin(async_stream::stream! {
                let call = {
                    let mut requests = self.requests.lock().unwrap();
                    requests.push(request.clone());
                    requests.len()
                };
                assert_eq!(request.model, "claude-sonnet-4-6");
                self.entered.notify_one();
                self.release.notified().await;
                if self.fail_on == Some(call) {
                    yield Err(LlmError::InvalidRequest { message: "synthetic stage failure".into() });
                    return;
                }
                yield Ok(LlmEvent::TextDelta {
                    delta: match call {
                        1 => "LINT ANSWER: FAIL synthetic style finding",
                        2 => "TEST ANSWER: PASS synthetic coverage finding",
                        _ => panic!("unexpected stage request {call}"),
                    }.into(),
                    meta: None,
                });
                yield Ok(LlmEvent::UsageUpdate {
                    usage: meerkat_core::TurnUsage::host_declared(
                        meerkat_core::Provider::Anthropic,
                        &request.model,
                        meerkat_core::Usage::default(),
                    ),
                });
                yield Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::Success { stop_reason: meerkat_core::StopReason::EndTurn },
                });
            })
        }

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::Anthropic
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    #[derive(Clone, Default)]
    struct Output(Arc<Mutex<Vec<u8>>>);

    impl Output {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    impl Write for Output {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn fixture(
        client: Arc<StageClient>,
    ) -> (
        tempfile::TempDir,
        Arc<EphemeralSessionService<FactoryAgentBuilder>>,
    ) {
        let directory = tempfile::tempdir_in(".").unwrap();
        let path = directory.path().join("sessions");
        std::fs::create_dir_all(&path).unwrap();
        let factory = AgentFactory::new(path).comms(true);
        let mut builder = FactoryAgentBuilder::new(factory, Config::default());
        builder.default_llm_client = Some(client);
        (
            directory,
            Arc::new(EphemeralSessionService::new(builder, 16)),
        )
    }

    fn run_test<F, Fut>(make: F)
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()>,
    {
        meerkat_runtime::host_stack::HostStackBudget::default_budget()
            .run("pipeline-demo-test", || async {
                // The actual body deliberately uses the same fixed mob identity
                // as main, which owns a process-wide comms participant name.
                let _exclusive_pipeline = PIPELINE_TEST_LOCK.lock().await;
                tokio::time::timeout(Duration::from_secs(30), make())
                    .await
                    .unwrap();
            })
            .unwrap();
    }

    async fn expect_entered(
        client: &StageClient,
        task: &mut tokio::task::JoinHandle<Result<(), DemoError>>,
        output: &Output,
    ) {
        tokio::select! {
            () = client.entered.notified() => {}
            result = task => panic!("pipeline ended before provider entry: {result:?}; output: {}", output.text()),
        }
    }

    #[test]
    fn actual_pipeline_waits_for_both_answers_in_order_before_cleanup() {
        run_test(|| async {
            let client = Arc::new(StageClient::default());
            let (directory, service) = fixture(client.clone());
            let path = directory.path().to_owned();
            let output = Output::default();
            let mut writer = output.clone();
            let task_service = service.clone();
            let mut task = tokio::spawn(async move {
                run_pipeline(task_service, Duration::from_secs(10), &mut writer).await
            });
            expect_entered(&client, &mut task, &output).await;
            assert!(!task.is_finished());
            assert!(output.text().contains("Roster (4 members)"));
            assert!(!output.text().contains("Lint turn completed"));
            assert!(!output.text().contains("--- Stage 2:"));
            assert_eq!(client.requests.lock().unwrap().len(), 1);
            client.release.notify_one();
            expect_entered(&client, &mut task, &output).await;
            assert!(!task.is_finished());
            let halfway = output.text();
            assert!(halfway.contains("LINT ANSWER: FAIL"));
            assert!(halfway.contains("Lint turn completed"));
            assert!(!halfway.contains("Test turn completed"));
            assert!(!halfway.contains("All stage workers retired"));
            assert_eq!(service.list(Default::default()).await.unwrap().len(), 4);
            client.release.notify_one();
            task.await.unwrap().unwrap();
            let text = output.text();
            let lint = text.find("LINT ANSWER: FAIL").unwrap();
            let second = text.find("--- Stage 2:").unwrap();
            let test = text.find("TEST ANSWER: PASS").unwrap();
            let cleanup = text.find("All stage workers retired").unwrap();
            assert!(lint < second && second < test && test < cleanup);
            let requests = client.requests.lock().unwrap().clone();
            assert_eq!(
                requests.len(),
                2,
                "idle topology members must not trigger provider calls"
            );
            assert!(
                serde_json::to_string(&requests[0].messages)
                    .unwrap()
                    .contains("Analyze this Rust function")
            );
            assert!(
                serde_json::to_string(&requests[1].messages)
                    .unwrap()
                    .contains("Independently evaluate")
            );
            assert!(service.list(Default::default()).await.unwrap().is_empty());
            drop(directory);
            assert!(!path.exists());
        });
    }

    #[test]
    fn actual_pipeline_propagates_each_stage_failure_without_false_completion() {
        run_test(|| async {
            for fail_on in [1, 2] {
                let client = Arc::new(StageClient {
                    fail_on: Some(fail_on),
                    ..Default::default()
                });
                let (_directory, service) = fixture(client.clone());
                let output = Output::default();
                let mut writer = output.clone();
                let task_service = service.clone();
                let mut task = tokio::spawn(async move {
                    run_pipeline(task_service, Duration::from_secs(10), &mut writer).await
                });
                for _ in 0..fail_on {
                    expect_entered(&client, &mut task, &output).await;
                    assert!(!task.is_finished());
                    client.release.notify_one();
                }
                let error = task.await.unwrap().unwrap_err();
                let typed = error
                    .downcast_ref::<BoundedTurnWaitError<MemberDeliveryReceipt>>()
                    .unwrap_or_else(|| panic!("expected typed exact-turn failure: {error:?}"));
                let BoundedTurnFailure::RuntimeTerminated { error, .. } = typed.failure() else {
                    panic!("expected fatal runtime termination: {typed:?}");
                };
                assert_eq!(
                    error.kind,
                    meerkat_core::TurnTerminalCauseKind::FatalFailure
                );
                assert_eq!(
                    error.outcome,
                    Some(meerkat_core::TurnTerminalOutcome::Failed)
                );
                let text = output.text();
                assert!(!text.contains("Test turn completed"));
                assert!(!text.contains("Demonstration complete"));
                assert_eq!(text.contains("Lint turn completed"), fail_on == 2);
                assert_eq!(client.requests.lock().unwrap().len(), fail_on);
                assert!(service.list(Default::default()).await.unwrap().is_empty());
            }
        });
    }

    #[test]
    fn actual_pipeline_timeout_is_not_completion_and_retires_blocked_workers() {
        run_test(|| async {
            let client = Arc::new(StageClient::default());
            let (_directory, service) = fixture(client.clone());
            let output = Output::default();
            let mut writer = output.clone();
            let error = run_pipeline(service.clone(), Duration::from_secs(1), &mut writer)
                .await
                .unwrap_err();
            assert_eq!(
                error
                    .downcast_ref::<std::io::Error>()
                    .unwrap_or_else(|| panic!(
                        "expected observation timeout, got {error:?}; output: {}",
                        output.text()
                    ))
                    .kind(),
                std::io::ErrorKind::TimedOut,
            );
            assert_eq!(client.requests.lock().unwrap().len(), 1);
            assert!(!output.text().contains("Lint turn completed"));
            assert!(!output.text().contains("--- Stage 2:"));
            assert!(service.list(Default::default()).await.unwrap().is_empty());
        });
    }

    #[test]
    fn printed_flow_schema_is_self_contained_and_validates_join_output() {
        let definition = MobDefinition::from_toml(PIPELINE_TOML).unwrap();
        let flow = definition.flows.values().next().unwrap();
        let join = flow.steps.get(&meerkat_mob::StepId::from("join")).unwrap();
        let Some(FlowSchemaRef::Inline(schema)) = &join.expected_schema_ref else {
            panic!("printed template must not depend on missing schema files");
        };
        let validator = jsonschema::validator_for(schema.as_value()).unwrap();
        assert!(validator.is_valid(&serde_json::json!({"summary":"fixture summary"})));
        assert!(!validator.is_valid(&serde_json::json!({})));
    }
}

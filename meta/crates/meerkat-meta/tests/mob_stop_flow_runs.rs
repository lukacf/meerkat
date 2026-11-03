//! A mob with an active flow run stops at teardown.
//!
//! meerkat 0.8.51's MobMachine refuses `Stop` while a flow run is active
//! (`StopRunning`'s `no_active_runs` guard, meerkat#1593). Teardown cancels
//! the run, awaits its terminal on the mob event ledger, and stops the mob,
//! without re-sending the stop on a timer.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use meerkat::{AgentFactory, Config, build_ephemeral_service};
use meerkat_client::{LlmClient, LlmError, LlmEvent, LlmRequest};
use meerkat_core::Message;
use meerkat_core::types::StopReason;
use meerkat_meta::{
    DiscoverySpec, MetaConfig, MobBootstrapOptions, MobBootstrapSpec, MobStopOutcome,
    UnifiedRuntime,
};
use meerkat_mob::{MobDefinition, MobRunStatus, MobState, MobStorage};

#[path = "support/llm_usage.rs"]
mod llm_usage;

static NEXT_TEST_MOB_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

const BLOCKING_STEP: &str = "MOB-STOP-FLOW-BLOCKING-STEP";

/// Bound for test steps that must happen promptly; it only turns a hang into
/// a failure.
const STEP: Duration = Duration::from_secs(30);

/// Answers every turn at once, except the flow step's, which blocks in the
/// provider (signalling `entered`) so the flow run stays active.
#[derive(Clone, Default)]
struct BlockingStepClient {
    entered: Arc<tokio::sync::Notify>,
}

impl LlmClient for BlockingStepClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        let blocks = serde_json::to_string(&request.messages)
            .is_ok_and(|messages| messages.contains(BLOCKING_STEP));
        let [usage, done] =
            llm_usage::usage_then_done(request, LlmClient::provider(self), StopReason::EndTurn);
        let entered = Arc::clone(&self.entered);
        Box::pin(async_stream::stream! {
            if blocks {
                entered.notify_one();
                std::future::pending::<()>().await;
            }
            yield Ok(LlmEvent::TextDelta { delta: "ok".to_string(), meta: None });
            yield Ok(usage);
            yield Ok(done);
        })
    }

    fn provider(&self) -> meerkat::Provider {
        meerkat::Provider::OpenAI
    }

    fn health_check<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn Future<Output = Result<(), LlmError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }
}

async fn runtime(client: &BlockingStepClient, state: &std::path::Path) -> UnifiedRuntime {
    let mob_id = format!(
        "mob-stop-flow-{}",
        NEXT_TEST_MOB_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let definition = MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "{mob_id}"

[limits]
cancel_grace_timeout_ms = 200

[profiles.lead]
model = "gpt-5.5"
external_addressable = true

[profiles.lead.tools]
comms = true

[flows.blocking]
description = "a flow whose only step blocks in the provider"

[flows.blocking.steps.first]
role = "lead"
message = "{BLOCKING_STEP}"
"#
    ))
    .expect("parse the mob definition");
    let factory = AgentFactory::new(state).comms(true);
    let session_service = Arc::new(build_ephemeral_service(factory, Config::default(), 8));
    let spec = MobBootstrapSpec::new(definition, MobStorage::in_memory(), session_service)
        .with_options(MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(client.clone())),
        });
    UnifiedRuntime::bootstrap(
        spec,
        MetaConfig {
            modules: Vec::new(),
            discovery: DiscoverySpec {
                namespace: String::new(),
                modules: Vec::new(),
            },
            pre_spawn: Vec::new(),
        },
        Duration::from_secs(10),
    )
    .await
    .expect("bootstrap the runtime")
}

#[tokio::test(flavor = "multi_thread")]
async fn teardown_cancels_an_active_flow_run_and_stops_the_mob() {
    let state = tempfile::tempdir().expect("state dir");
    let client = BlockingStepClient::default();
    let runtime = runtime(&client, state.path()).await;
    let handle = runtime.mob_handle();

    let entered = client.entered.notified();
    tokio::pin!(entered);
    entered.as_mut().enable();
    let run_id = handle
        .run_flow("blocking".into(), serde_json::Value::Null)
        .await
        .expect("start the flow");
    tokio::time::timeout(STEP, entered)
        .await
        .expect("the flow step reaches the provider");

    // The flow run holds the Stop: MobMachine refuses it outright.
    assert!(
        matches!(
            handle.stop().await,
            Err(meerkat_mob::MobError::InvalidTransition { .. })
        ),
        "an active flow run refuses a plain Stop"
    );

    let outcome = tokio::time::timeout(STEP, runtime.stop_mob_for_teardown())
        .await
        .expect("teardown settles the flow run within its budget");
    assert!(
        matches!(outcome, MobStopOutcome::Stopped),
        "teardown stops a mob with an active flow run: {outcome:?}"
    );
    assert_eq!(handle.status().await.unwrap(), MobState::Stopped);
    let run = handle
        .flow_status(run_id)
        .await
        .expect("flow status")
        .expect("the run exists");
    assert_eq!(
        run.status,
        MobRunStatus::Canceled,
        "teardown cancelled the run"
    );

    let _ = runtime.shutdown().await;
}

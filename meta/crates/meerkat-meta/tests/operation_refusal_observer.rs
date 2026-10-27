//! A host's operation refusal observer, installed with
//! `UnifiedRuntimeBuilder::operation_refusal_observer`, reaches the member
//! agents of each builder-owned launch: persistent state, a scratch
//! directory (the identity-first composition) and the default temp-dir
//! ephemeral one. A member's tool owner refuses the one call its model
//! makes, and the observer receives exactly that refusal as the call's tool
//! result.
//!
//! A member can run more than one turn (an autonomous startup or a spawn
//! kickoff, then the test's own input), and Core reports the refused call of
//! each run. So the test model calls the refused tool only once the test
//! arms its probe, right before the input it reads, and answers every other
//! request with text: each launch makes exactly one refused call.
//!
//! Each test reads the observations once the model is asked again with
//! the refused call's result, a typed watch the test client sets, bounded
//! only by a named failure timeout. Core reports a call's refusal before
//! that result returns to the model, and no later request calls the tool,
//! so the read sees every observation there will be, and a duplicate fails
//! the exact comparison.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::types::{ToolCallView, ToolDef};
use meerkat_core::{
    AgentToolDispatcher, Message, OperationRefusalDisposition, OperationRefusalKind,
    OperationRefusalObservation, OperationRefusalObserver, OperationRefusalOutcome,
    OperationRefusalSurface, OperationRefused, Provider, RefusalDispatch, StopReason,
    ToolDispatchOutcome, ToolError,
};
use meerkat_meta::UnifiedRuntime;
use meerkat_meta::identity_first::{
    AgentAddressability, AgentIdentity, DurableAgentSpec, LocalContinuityStore, LocalLeaseProvider,
    MutableRosterProvider,
};
use meerkat_mob::{MobDefinition, SpawnMemberSpec};

/// How long a member turn may take to reach the observer before the test
/// fails.
const OBSERVATION_FAILURE_BOUND: Duration = Duration::from_secs(30);
const REFUSED_TOOL: &str = "refused_probe";
const REFUSED_CALL: &str = "refused-call";
const MEMBER: &str = "helper";

fn definition(mob_id: &str) -> MobDefinition {
    MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "{mob_id}"

[profiles.worker]
model = "gpt-5.5"
runtime_mode = "autonomous_host"
external_addressable = true

[profiles.worker.tools]
comms = true
rust_bundles = ["refusing"]
"#
    ))
    .expect("parse mob definition")
}

/// One tool whose owner refuses every call.
struct RefusingBundle;

#[async_trait::async_trait]
impl AgentToolDispatcher for RefusingBundle {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        vec![Arc::new(ToolDef::new(
            REFUSED_TOOL,
            "A tool whose owner refuses every call.",
            serde_json::json!({"type": "object", "properties": {}}),
        ))]
        .into()
    }

    async fn dispatch(&self, _call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        Err(ToolError::AuthorizationRefused {
            refusal: OperationRefused::new(OperationRefusalKind::Denied),
        })
    }
}

/// Calls the refused tool exactly once after the host arms the probe.
/// It marks `answered` when the model is asked again with that result.
struct RefusedCallClient {
    answered: tokio::sync::watch::Sender<bool>,
    probe_pending: AtomicBool,
}

impl RefusedCallClient {
    fn new() -> (Arc<Self>, tokio::sync::watch::Receiver<bool>) {
        let (answered, seen) = tokio::sync::watch::channel(false);
        (
            Arc::new(Self {
                answered,
                probe_pending: AtomicBool::new(false),
            }),
            seen,
        )
    }

    fn arm_probe(&self) {
        self.probe_pending.store(true, Ordering::Release);
    }
}

#[async_trait::async_trait]
impl LlmClient for RefusedCallClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(&'a self, request: &'a LlmRequest) -> meerkat_client::types::LlmStream<'a> {
        let after_tool = request.messages.iter().any(|message| {
            matches!(message, Message::ToolResults { results, .. }
                if results.iter().any(|result| result.tool_use_id == REFUSED_CALL))
        });
        if after_tool {
            self.answered.send_replace(true);
        }
        // Autonomous startup or an additional host turn is not another probe.
        // One issued call still must produce exactly one observer notification.
        let (event, stop_reason) = if self.probe_pending.swap(false, Ordering::AcqRel) {
            (
                LlmEvent::ToolCallComplete {
                    id: REFUSED_CALL.to_string(),
                    name: REFUSED_TOOL.to_string(),
                    args: serde_json::json!({}),
                    meta: None,
                },
                StopReason::ToolUse,
            )
        } else {
            (
                LlmEvent::TextDelta {
                    delta: "done".to_string(),
                    meta: None,
                },
                StopReason::EndTurn,
            )
        };
        Box::pin(futures::stream::iter(vec![
            Ok(event),
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::success(stop_reason),
            }),
        ]))
    }

    fn provider(&self) -> Provider {
        Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

/// Records every observation.
struct RecordingObserver {
    observed: Mutex<Vec<OperationRefusalObservation>>,
}

impl RecordingObserver {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            observed: Mutex::new(Vec::new()),
        })
    }

    /// Surface, outcome and disposition of every observation, read once the
    /// model has been asked again with the call's result.
    async fn observed(
        &self,
        mut answered: tokio::sync::watch::Receiver<bool>,
    ) -> Vec<(
        OperationRefusalSurface,
        OperationRefusalOutcome,
        OperationRefusalDisposition,
    )> {
        tokio::time::timeout(
            OBSERVATION_FAILURE_BOUND,
            answered.wait_for(|answered| *answered),
        )
        .await
        .expect("the model got the refused call's result within the failure bound")
        .expect("the test client is alive");
        self.observed
            .lock()
            .unwrap()
            .iter()
            .map(|observation| {
                (
                    observation.surface().clone(),
                    observation.outcome(),
                    observation.disposition(),
                )
            })
            .collect()
    }
}

impl OperationRefusalObserver for RecordingObserver {
    fn observe(&self, observation: &OperationRefusalObservation) {
        self.observed.lock().unwrap().push(observation.clone());
    }
}

/// The one observation every launch must produce: the refused call, returned
/// to the model as its tool result.
fn refused_call() -> Vec<(
    OperationRefusalSurface,
    OperationRefusalOutcome,
    OperationRefusalDisposition,
)> {
    vec![(
        OperationRefusalSurface::ToolCall {
            tool_use_id: REFUSED_CALL.to_string(),
        },
        OperationRefusalOutcome::Refused {
            kind: OperationRefusalKind::Denied,
            dispatch: RefusalDispatch::Unknown,
        },
        OperationRefusalDisposition::ToolResult,
    )]
}

/// Spawns the member and sends it one message; its turn calls the refused
/// tool.
async fn spawn_and_send(runtime: &UnifiedRuntime, client: &RefusedCallClient) {
    runtime
        .spawn(SpawnMemberSpec::from_wire(
            "worker".to_string(),
            MEMBER.to_string(),
            None,
            None,
            None,
        ))
        .await
        .expect("spawn");
    client.arm_probe();
    meerkat_meta::send_message_on_mob(&runtime.mob_handle(), MEMBER, "hello".to_string())
        .await
        .expect("send message");
}

/// The persistent launch (`persistent_state`).
#[tokio::test(flavor = "multi_thread")]
async fn a_persistent_member_reports_its_refused_call_to_the_host_observer() {
    let state = tempfile::tempdir().unwrap();
    let observer = RecordingObserver::new();
    let (client, answered) = RefusedCallClient::new();
    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .definition(definition("refusal-observer-persistent"))
            .persistent_state(state.path().join("state"))
            .default_llm_client(client.clone())
            .register_tool_bundle("refusing", Arc::new(RefusingBundle))
            .operation_refusal_observer(observer.clone())
            .build(),
    )
    .await
    .expect("runtime builds");
    spawn_and_send(&runtime, &client).await;
    assert_eq!(observer.observed(answered).await, refused_call());
    runtime.shutdown().await;
}

/// The default launch: neither persistent state nor a scratch directory,
/// so the builder owns a temp-dir ephemeral composition.
#[tokio::test(flavor = "multi_thread")]
async fn a_temp_dir_member_reports_its_refused_call_to_the_host_observer() {
    let observer = RecordingObserver::new();
    let (client, answered) = RefusedCallClient::new();
    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .definition(definition("refusal-observer-temp-dir"))
            .default_llm_client(client.clone())
            .register_tool_bundle("refusing", Arc::new(RefusingBundle))
            .operation_refusal_observer(observer.clone())
            .build(),
    )
    .await
    .expect("runtime builds");
    spawn_and_send(&runtime, &client).await;
    assert_eq!(observer.observed(answered).await, refused_call());
    runtime.shutdown().await;
}

/// The scratch launch: a scratch directory selects the identity-first
/// composition over the runtime-backed ephemeral spec. The roster member's
/// armed turn runs through the mob's host input.
#[tokio::test(flavor = "multi_thread")]
async fn a_scratch_member_reports_its_refused_call_to_the_host_observer() {
    let scratch = tempfile::tempdir().unwrap();
    let observer = RecordingObserver::new();
    let (client, answered) = RefusedCallClient::new();
    let identity = AgentIdentity::parse(MEMBER).unwrap();
    let roster = Arc::new(MutableRosterProvider::new(vec![DurableAgentSpec {
        identity: identity.clone(),
        profile: "worker".into(),
        addressability: AgentAddressability::Addressable,
        display_name: None,
        labels: BTreeMap::new(),
        context: None,
        additional_instructions: Vec::new(),
        initial_message: None,
        runtime_mode_override: Some(meerkat_mob::MobRuntimeMode::AutonomousHost),
        backend: None,
        binding: None,
        placement: None,
    }]));
    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .definition(definition("refusal-observer-scratch"))
            .continuity_store(Arc::new(LocalContinuityStore::in_memory().unwrap()))
            .lease_provider(Arc::new(LocalLeaseProvider::new()))
            .roster_provider(roster)
            .scratch_dir(scratch.path())
            .ephemeral_runtime_store(true)
            .identity_runtime_instance_id("refusal-observer-scratch")
            .default_llm_client(client.clone())
            .register_tool_bundle("refusing", Arc::new(RefusingBundle))
            .operation_refusal_observer(observer.clone())
            .build(),
    )
    .await
    .expect("runtime builds");
    client.arm_probe();
    run_host_turn(&runtime, &identity).await;
    assert_eq!(observer.observed(answered).await, refused_call());
    runtime.mob_handle().stop().await.unwrap();
}

/// One host input turn for the roster member, waited to its end.
async fn run_host_turn(runtime: &UnifiedRuntime, identity: &AgentIdentity) {
    let handle = runtime.mob_handle();
    let member = handle
        .get_member(&identity.as_str().into())
        .await
        .unwrap()
        .expect("the roster member is live");
    let interaction = uuid::Uuid::new_v4();
    let turn = handle
        .start_host_human_input_bounded(
            member.agent_runtime_id,
            member.fence_token,
            meerkat_mob::WorkSpec::new("Call the tool.", meerkat_mob::WorkOrigin::External)
                .with_interaction_id(meerkat_core::interaction::InteractionId(interaction)),
            meerkat_core::types::HandlingMode::Queue,
            meerkat_mob::MobDeliveryIdentity::new(interaction.to_string(), interaction.to_string())
                .unwrap(),
            std::time::Instant::now() + OBSERVATION_FAILURE_BOUND,
        )
        .await
        .unwrap();
    tokio::time::timeout(OBSERVATION_FAILURE_BOUND, turn.wait())
        .await
        .unwrap()
        .unwrap();
}

//! Promotion contract for the run-local model-routing handoff slot.
//!
//! These tests pin the one rule the whole feature rests on: staged intent
//! becomes a committed request if and only if the run that staged it reached a
//! clean terminal boundary, and the committed record names that exact run.
//!
//! They are deliberately written against the real agent loop rather than the
//! staging slot in isolation. The slot's own behaviour is already unit-tested;
//! what is worth proving here is that the loop calls it at the right two places
//! and at no others, which is a property of the loop, not of the slot.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use crate as meerkat_core;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use meerkat_core::image_generation::SwitchTurnRequestId;
use meerkat_core::lifecycle::run_primitive::ModelId;
use meerkat_core::session::model_routing_control::{
    ModelRoutingIntentRecordDisposition, SessionModelRoutingControlRecord,
};
use meerkat_core::session::model_routing_handoff_staging::ModelRoutingHandoffStagingSlot;
use meerkat_core::{
    AgentBuilder, AgentError, AgentLlmClient, AgentSessionStore, AgentToolDispatcher,
    LlmStreamResult, Message, StopReason, ToolCallView, ToolDef, ToolResult, Usage,
};
use serde_json::Value;
use serde_json::value::RawValue;

type DynAgent =
    meerkat_core::Agent<dyn AgentLlmClient, dyn AgentToolDispatcher, dyn AgentSessionStore>;

const STAGING_TOOL: &str = "stage_swap";

fn empty_tool_schema() -> Value {
    let mut schema = serde_json::Map::new();
    schema.insert("type".to_string(), Value::String("object".to_string()));
    Value::Object(schema)
}

fn normalized_usage(client: &dyn AgentLlmClient) -> Usage {
    meerkat_core::TurnUsage::host_declared(client.provider(), client.model(), Usage::default())
        .into_inner()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scenario {
    /// One assistant turn, no tools, clean completion.
    TextOnly,
    /// Tool call, then a second provider call, then clean completion.
    ToolThenText,
    /// Provider error on the first call.
    FailImmediately,
}

/// Records every provider call so a test can prove the *current* run never
/// switched model mid-flight.
struct RecordingClient {
    scenario: Scenario,
    calls: Arc<std::sync::Mutex<Vec<&'static str>>>,
}

impl RecordingClient {
    fn new(scenario: Scenario, calls: Arc<std::sync::Mutex<Vec<&'static str>>>) -> Self {
        Self { scenario, calls }
    }
}

#[async_trait]
impl AgentLlmClient for RecordingClient {
    async fn stream_response(
        &self,
        _messages: &[Message],
        _tools: &[Arc<ToolDef>],
        _max_tokens: u32,
        _temperature: Option<f32>,
        _provider_params: Option<&meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
    ) -> Result<LlmStreamResult, AgentError> {
        let index = {
            let mut calls = self
                .calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            calls.push("model-a");
            calls.len() - 1
        };
        match self.scenario {
            Scenario::FailImmediately => {
                Err(AgentError::InternalError("provider failed".to_string()))
            }
            Scenario::TextOnly => Ok(LlmStreamResult::new(
                vec![meerkat_core::AssistantBlock::Text {
                    text: "done".to_string(),
                    meta: None,
                }],
                StopReason::EndTurn,
                normalized_usage(self),
            )),
            Scenario::ToolThenText => {
                if index == 0 {
                    let args = RawValue::from_string("{}".to_string())
                        .map_err(|error| AgentError::InternalError(error.to_string()))?;
                    Ok(LlmStreamResult::new(
                        vec![meerkat_core::AssistantBlock::ToolUse {
                            id: "tc_1".to_string(),
                            name: STAGING_TOOL.into(),
                            args,
                            meta: None,
                        }],
                        StopReason::ToolUse,
                        normalized_usage(self),
                    ))
                } else {
                    Ok(LlmStreamResult::new(
                        vec![meerkat_core::AssistantBlock::Text {
                            text: "done".to_string(),
                            meta: None,
                        }],
                        StopReason::EndTurn,
                        normalized_usage(self),
                    ))
                }
            }
        }
    }

    fn provider(&self) -> crate::provider::Provider {
        crate::provider::Provider::Other
    }

    fn model(&self) -> &'static str {
        "model-a"
    }
}

/// A dispatcher whose single tool stages a permanent switch, exactly as the
/// real `brain_swap` builtin does.
struct StagingToolDispatcher {
    staging: Arc<ModelRoutingHandoffStagingSlot>,
    request_ids: Arc<std::sync::Mutex<Vec<SwitchTurnRequestId>>>,
    next_request_byte: AtomicUsize,
    target: String,
}

impl StagingToolDispatcher {
    fn new(staging: Arc<ModelRoutingHandoffStagingSlot>, target: &str) -> Self {
        Self {
            staging,
            request_ids: Arc::new(std::sync::Mutex::new(Vec::new())),
            next_request_byte: AtomicUsize::new(1),
            target: target.to_string(),
        }
    }
}

#[async_trait]
impl AgentToolDispatcher for StagingToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::new([Arc::new(ToolDef {
            name: STAGING_TOOL.into(),
            description: "stage a permanent model switch".to_string(),
            input_schema: empty_tool_schema(),
            provenance: None,
        })])
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        let byte = self.next_request_byte.fetch_add(1, Ordering::SeqCst) as u8;
        let request_id = SwitchTurnRequestId::new(uuid::Uuid::from_bytes([byte; 16]));
        let outcome = self
            .staging
            .stage(request_id, ModelId::new(self.target.clone()))
            .map_err(|error| meerkat_core::ToolError::execution_failed(error.to_string()))?;
        self.request_ids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(*outcome.request_id());
        Ok(ToolResult::new(call.id.to_string(), "staged".to_string(), false).into())
    }
}

struct NoopStore;

#[async_trait]
impl AgentSessionStore for NoopStore {
    async fn save(&self, _session: &meerkat_core::Session) -> Result<(), AgentError> {
        Ok(())
    }

    async fn load(&self, _id: &str) -> Result<Option<meerkat_core::Session>, AgentError> {
        Ok(None)
    }
}

/// A store whose every write fails, and that counts the attempts.
struct FailingStore {
    attempts: Arc<std::sync::Mutex<usize>>,
}

impl FailingStore {
    fn new(attempts: Arc<std::sync::Mutex<usize>>) -> Self {
        Self { attempts }
    }
}

#[async_trait]
impl AgentSessionStore for FailingStore {
    async fn save(&self, _session: &meerkat_core::Session) -> Result<(), AgentError> {
        *self
            .attempts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) += 1;
        Err(AgentError::StoreError("disk is on fire".to_string()))
    }

    async fn load(&self, _id: &str) -> Result<Option<meerkat_core::Session>, AgentError> {
        Ok(None)
    }
}

fn test_session() -> meerkat_core::Session {
    let mut session = meerkat_core::Session::new();
    session
        .set_session_metadata(meerkat_core::SessionMetadata {
            model_fallback: None,
            schema_version: meerkat_core::session_metadata_schema_version(),
            model: "model-a".to_string(),
            max_tokens: 1024,
            structured_output_retries: 2,
            provider: meerkat_core::Provider::Other,
            self_hosted_server_id: None,
            provider_params: None,
            tooling: meerkat_core::SessionTooling::default(),
            keep_alive: false,
            comms_name: None,
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: None,
            mob_member_binding: None,
        })
        .expect("session metadata serializes");
    session
}

async fn build_agent(
    scenario: Scenario,
    staging: Arc<ModelRoutingHandoffStagingSlot>,
    calls: Arc<std::sync::Mutex<Vec<&'static str>>>,
    dispatcher: Arc<StagingToolDispatcher>,
) -> DynAgent {
    build_agent_with_store(
        scenario,
        staging,
        calls,
        dispatcher,
        Arc::new(NoopStore) as Arc<dyn AgentSessionStore>,
    )
    .await
}

async fn build_agent_with_store(
    scenario: Scenario,
    staging: Arc<ModelRoutingHandoffStagingSlot>,
    calls: Arc<std::sync::Mutex<Vec<&'static str>>>,
    dispatcher: Arc<StagingToolDispatcher>,
    store: Arc<dyn AgentSessionStore>,
) -> DynAgent {
    let client: Arc<dyn AgentLlmClient> = Arc::new(RecordingClient::new(scenario, calls));
    let tools: Arc<dyn AgentToolDispatcher> = dispatcher;
    AgentBuilder::new()
        .resume_session(test_session())
        .with_turn_state_handle(Arc::new(
            crate::agent::test_turn_state_handle::TestTurnStateHandle::new(),
        ))
        .with_model_routing_handoff_staging(staging)
        .build_standalone(client, tools, store)
        .await
}

fn requested_records(agent: &DynAgent) -> Vec<&SessionModelRoutingControlRecord> {
    agent
        .session()
        .model_routing_control()
        .records()
        .iter()
        .filter(|record| record.disposition() == ModelRoutingIntentRecordDisposition::Requested)
        .collect()
}

/// The no-deadlock property, stated as an observable fact rather than an
/// absence of hangs: the run that calls the staging tool makes a provider call
/// BEFORE the tool and another AFTER it, both on model A, and then completes.
///
/// If the tool had tried to switch identity synchronously it would either have
/// blocked on the session it is running inside, or the second call would have
/// been served by model B. Both are visible here.
#[tokio::test]
async fn staging_tool_never_switches_the_run_that_called_it() {
    let staging = Arc::new(ModelRoutingHandoffStagingSlot::new());
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let dispatcher = Arc::new(StagingToolDispatcher::new(Arc::clone(&staging), "model-b"));
    let mut agent = build_agent(
        Scenario::ToolThenText,
        Arc::clone(&staging),
        Arc::clone(&calls),
        Arc::clone(&dispatcher),
    )
    .await;

    let result = agent
        .run(meerkat_core::ContentInput::Text("go".to_string()))
        .await
        .expect("run completes");
    assert_eq!(result.tool_calls, 1, "the staging tool must have executed");

    let observed = calls
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    assert_eq!(
        observed,
        vec!["model-a", "model-a"],
        "every provider call in the staging run must stay on the original model"
    );

    let records = requested_records(&agent);
    assert_eq!(
        records.len(),
        1,
        "a clean run must commit exactly one request"
    );
    assert_eq!(
        records[0].intent().target_model,
        ModelId::new("model-b"),
        "the committed request must name the staged target"
    );
}

/// A run that never reaches a clean boundary commits nothing.
#[tokio::test]
async fn failed_run_commits_no_request() {
    let staging = Arc::new(ModelRoutingHandoffStagingSlot::new());
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let dispatcher = Arc::new(StagingToolDispatcher::new(Arc::clone(&staging), "model-b"));
    let mut agent = build_agent(
        Scenario::FailImmediately,
        Arc::clone(&staging),
        Arc::clone(&calls),
        Arc::clone(&dispatcher),
    )
    .await;

    // Stage directly: the provider fails before any tool could run, and the
    // point is that ALREADY-staged intent is still discarded.
    staging
        .stage(
            SwitchTurnRequestId::new(uuid::Uuid::from_bytes([9u8; 16])),
            ModelId::new("model-b"),
        )
        .expect("stage");

    let outcome = agent
        .run(meerkat_core::ContentInput::Text("go".to_string()))
        .await;
    assert!(
        outcome.is_err(),
        "the scripted provider failure must surface"
    );
    assert!(
        requested_records(&agent).is_empty(),
        "a failed run must not commit a request the model was never told succeeded"
    );
}

/// Intent staged by a run that then failed must not be inherited by the next
/// run. Run start clears the slot, so the second (clean) run commits nothing.
#[tokio::test]
async fn intent_staged_by_a_failed_run_is_not_inherited() {
    let staging = Arc::new(ModelRoutingHandoffStagingSlot::new());
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let dispatcher = Arc::new(StagingToolDispatcher::new(Arc::clone(&staging), "model-b"));
    let mut failing = build_agent(
        Scenario::FailImmediately,
        Arc::clone(&staging),
        Arc::clone(&calls),
        Arc::clone(&dispatcher),
    )
    .await;
    staging
        .stage(
            SwitchTurnRequestId::new(uuid::Uuid::from_bytes([7u8; 16])),
            ModelId::new("model-b"),
        )
        .expect("stage");
    let _ = failing
        .run(meerkat_core::ContentInput::Text("go".to_string()))
        .await;
    drop(failing);

    let clean_calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut clean = build_agent(
        Scenario::TextOnly,
        Arc::clone(&staging),
        clean_calls,
        Arc::clone(&dispatcher),
    )
    .await;
    clean
        .run(meerkat_core::ContentInput::Text("go".to_string()))
        .await
        .expect("clean run completes");
    assert!(
        requested_records(&clean).is_empty(),
        "a later run must not commit intent it never staged"
    );
}

/// Two clean runs that each stage bind to two DIFFERENT originating runs.
///
/// This is the per-run binding assertion: if the record captured the run at
/// staging time, or reused a cached id, the two would collide.
#[tokio::test]
async fn each_committed_request_binds_its_own_originating_run() {
    let staging = Arc::new(ModelRoutingHandoffStagingSlot::new());
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let dispatcher = Arc::new(StagingToolDispatcher::new(Arc::clone(&staging), "model-b"));
    let mut agent = build_agent(
        Scenario::ToolThenText,
        Arc::clone(&staging),
        Arc::clone(&calls),
        Arc::clone(&dispatcher),
    )
    .await;

    agent
        .run(meerkat_core::ContentInput::Text("first".to_string()))
        .await
        .expect("first run completes");
    // The scripted client keys its tool-call turn off the call index, which is
    // shared across runs. Reset it so the second run replays the same shape;
    // without this the second run would never call the tool and the test would
    // assert nothing.
    calls
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
    agent
        .run(meerkat_core::ContentInput::Text("second".to_string()))
        .await
        .expect("second run completes");

    let records = requested_records(&agent);
    assert_eq!(records.len(), 2, "each staging run commits one request");
    assert_ne!(
        records[0].originating_run_id(),
        records[1].originating_run_id(),
        "each committed request must name its own originating run"
    );
    assert_ne!(
        records[0].request_id(),
        records[1].request_id(),
        "each staging run mints its own request identity"
    );
}

/// A run with nothing staged commits nothing, so the promotion hook is not a
/// per-run write of any kind.
#[tokio::test]
async fn a_run_that_stages_nothing_commits_nothing() {
    let staging = Arc::new(ModelRoutingHandoffStagingSlot::new());
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let dispatcher = Arc::new(StagingToolDispatcher::new(Arc::clone(&staging), "model-b"));
    let mut agent = build_agent(
        Scenario::TextOnly,
        Arc::clone(&staging),
        Arc::clone(&calls),
        Arc::clone(&dispatcher),
    )
    .await;
    agent
        .run(meerkat_core::ContentInput::Text("go".to_string()))
        .await
        .expect("run completes");
    assert!(
        agent.session().model_routing_control().is_empty(),
        "an ordinary run must leave the handoff log byte-identical"
    );
}

/// An agent with no staging slot wired cannot commit anything, which is the
/// standalone/WASM shape: the tool is never registered there either.
#[tokio::test]
async fn an_agent_without_a_staging_slot_commits_nothing() {
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let client: Arc<dyn AgentLlmClient> = Arc::new(RecordingClient::new(Scenario::TextOnly, calls));
    let tools: Arc<dyn AgentToolDispatcher> = Arc::new(StagingToolDispatcher::new(
        Arc::new(ModelRoutingHandoffStagingSlot::new()),
        "model-b",
    ));
    let store: Arc<dyn AgentSessionStore> = Arc::new(NoopStore);
    let mut agent: DynAgent = AgentBuilder::new()
        .resume_session(test_session())
        .with_turn_state_handle(Arc::new(
            crate::agent::test_turn_state_handle::TestTurnStateHandle::new(),
        ))
        .build_standalone(client, tools, store)
        .await;
    agent
        .run(meerkat_core::ContentInput::Text("go".to_string()))
        .await
        .expect("run completes");
    assert!(agent.session().model_routing_control().is_empty());
}

// ---------------------------------------------------------------------------
// Promotion durability.
//
// A run that promotes a staged handoff has told the model its permanent switch
// was accepted. If the save that carries that request then fails quietly, the
// run still reports success and nothing on disk owes the switch — the next
// session answers on the old identity with no record that anything was asked.
// So promotion upgrades the ordinary best-effort save into a required one.
// ---------------------------------------------------------------------------

/// A promoting run whose store write fails must FAIL, not report success.
#[tokio::test]
async fn a_promoting_run_fails_when_the_session_cannot_be_persisted() {
    let staging = Arc::new(ModelRoutingHandoffStagingSlot::new());
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let dispatcher = Arc::new(StagingToolDispatcher::new(Arc::clone(&staging), "model-b"));
    let attempts = Arc::new(std::sync::Mutex::new(0usize));
    let mut agent = build_agent_with_store(
        Scenario::ToolThenText,
        Arc::clone(&staging),
        Arc::clone(&calls),
        Arc::clone(&dispatcher),
        Arc::new(FailingStore::new(Arc::clone(&attempts))) as Arc<dyn AgentSessionStore>,
    )
    .await;

    let error = agent
        .run(meerkat_core::ContentInput::Text("go".to_string()))
        .await
        .expect_err("a promoting run must not report success it cannot back with a durable write");
    assert!(
        matches!(&error, AgentError::StoreError(message) if message == "disk is on fire"),
        "the store's own typed error must surface unchanged, not restated: {error:?}"
    );
    assert!(
        *attempts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            > 0,
        "the run must have attempted the write it is failing over"
    );

    // The request is still on the in-memory session: nothing rolls it back,
    // because the caller's recovery is to retry the save, not to pretend the
    // model was never told.
    assert_eq!(
        requested_records(&agent).len(),
        1,
        "the promoted request stays on the live session for the caller to retry"
    );
}

/// The ordinary path is unchanged: a run that stages nothing keeps the
/// historical best-effort behaviour even when the store is failing.
#[tokio::test]
async fn a_run_without_a_staged_handoff_still_tolerates_a_failing_store() {
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let attempts = Arc::new(std::sync::Mutex::new(0usize));
    let client: Arc<dyn AgentLlmClient> =
        Arc::new(RecordingClient::new(Scenario::TextOnly, Arc::clone(&calls)));
    // A dispatcher whose staging slot is never reached, so the run ends with
    // nothing staged.
    let tools: Arc<dyn AgentToolDispatcher> = Arc::new(StagingToolDispatcher::new(
        Arc::new(ModelRoutingHandoffStagingSlot::new()),
        "model-b",
    ));
    let store: Arc<dyn AgentSessionStore> = Arc::new(FailingStore::new(Arc::clone(&attempts)));
    let mut agent: DynAgent = AgentBuilder::new()
        .resume_session(test_session())
        .with_turn_state_handle(Arc::new(
            crate::agent::test_turn_state_handle::TestTurnStateHandle::new(),
        ))
        .with_model_routing_handoff_staging(Arc::new(ModelRoutingHandoffStagingSlot::new()))
        .build_standalone(client, tools, store)
        .await;

    agent
        .run(meerkat_core::ContentInput::Text("go".to_string()))
        .await
        .expect("a run that promoted nothing keeps the historical best-effort save");
    assert!(
        *attempts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            > 0,
        "the best-effort write must still have been attempted"
    );
    assert!(
        agent.session().model_routing_control().is_empty(),
        "nothing was staged, so nothing may be committed"
    );
}

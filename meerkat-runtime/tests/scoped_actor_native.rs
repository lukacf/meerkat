#![cfg(all(feature = "live", feature = "sqlite-store"))]

use meerkat_core::execution_scope::{
    RunExecutionAuthority, RunExecutionContext, ScopedToolEffectSupport,
};
use meerkat_core::live_execution::LiveChannelId;
use meerkat_core::live_execution::evidence::LiveObservationInterval;
use meerkat_core::live_execution::request::{LiveApplicationRequestId, LiveSourceKey};
use meerkat_core::turn_execution_authority::{ContentShape, TurnPrimitiveKind};
use meerkat_core::{AgentToolDispatcher, BlobStore, Session};
use meerkat_runtime::live_grant::{LiveExecutionGrantIssuer, LiveGrantActivationRequest};
use meerkat_runtime::live_ledger::completion::{LiveCompletionEvent, LivePhysicalEffectOutcome};
use meerkat_runtime::live_ledger::record::LiveLedgerRecord;
use meerkat_runtime::live_ledger::transcript_authority::{
    LiveSourceReservationOutcome, LiveTranscriptStoreOwner,
};
use meerkat_runtime::live_source::LiveSourceEntryRecord;
use meerkat_runtime::store::live_history::LiveHistoryReadRequest;
use meerkat_runtime::store::{
    RuntimeStore, RuntimeStoreError, RuntimeStoreWriteFence, RuntimeStoreWriteFenceOutcome,
    SerializedSessionSnapshot, SqliteRuntimeStore,
};
use meerkat_runtime::{LogicalRuntimeId, MeerkatMachine};
use meerkat_store::SessionStore;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Mutex;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

#[path = "scoped_actor_native/callback_admission.rs"]
mod callback_admission;

struct CurrentFence;

impl RuntimeStoreWriteFence for CurrentFence {
    fn execute_if_current(
        &self,
        operation: Box<dyn FnOnce() -> Result<(), RuntimeStoreError> + '_>,
    ) -> Result<RuntimeStoreWriteFenceOutcome, RuntimeStoreError> {
        operation()?;
        Ok(RuntimeStoreWriteFenceOutcome::Applied)
    }
}

struct NoTools;

#[async_trait::async_trait]
impl AgentToolDispatcher for NoTools {
    fn scoped_tool_effect_support(&self) -> ScopedToolEffectSupport {
        ScopedToolEffectSupport::RejectsAllCalls
    }

    fn tools(&self) -> Arc<[Arc<meerkat_core::ToolDef>]> {
        Arc::from([])
    }

    async fn dispatch(
        &self,
        call: meerkat_core::ToolCallView<'_>,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, meerkat_core::ToolError> {
        Err(meerkat_core::ToolError::NotFound {
            name: call.name.into(),
        })
    }
}

struct ReadTool {
    calls: Arc<AtomicUsize>,
    scenario: ActorScenario,
    sibling_blocks: Vec<meerkat_core::AssistantBlock>,
}

#[async_trait::async_trait]
impl AgentToolDispatcher for ReadTool {
    fn scoped_tool_effect_support(&self) -> ScopedToolEffectSupport {
        ScopedToolEffectSupport::RequiresOuterClaim
    }

    fn tools(&self) -> Arc<[Arc<meerkat_core::ToolDef>]> {
        vec![Arc::new(meerkat_core::ToolDef::new(
            "allowed_tool",
            "Read a fixture value",
            serde_json::json!({"type":"object","properties":{},"additionalProperties":false}),
        ))]
        .into()
    }

    fn tool_mutation_class(&self, _name: &str) -> meerkat_core::ToolMutationClass {
        meerkat_core::ToolMutationClass::ReadOnly
    }

    async fn dispatch(
        &self,
        call: meerkat_core::ToolCallView<'_>,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, meerkat_core::ToolError> {
        if call.name != "allowed_tool" {
            return Err(meerkat_core::ToolError::not_found(call.name));
        }
        if self.scenario == ActorScenario::CallbackTranscriptSibling && call.id == "fc_sibling" {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let mut outcome = meerkat_core::ops::ToolDispatchOutcome::sync_result(
                meerkat_core::ToolResult::new(call.id.into(), "sibling result".into(), false),
            );
            outcome
                .session_effects
                .push(meerkat_core::ops::SessionEffect::AppendAssistantBlocks {
                    blocks: self.sibling_blocks.clone(),
                });
            return Ok(outcome);
        }
        let previous = self.calls.fetch_add(1, Ordering::SeqCst);
        if matches!(
            self.scenario,
            ActorScenario::Callback
                | ActorScenario::CallbackAdmission
                | ActorScenario::CallbackAdmissionStale
                | ActorScenario::CallbackResume
                | ActorScenario::CallbackTranscriptSibling
                | ActorScenario::CallbackRecoveryHold
                | ActorScenario::CallbackAppliedFailure
                | ActorScenario::CallbackCancelledBeforeAdmission
                | ActorScenario::CallbackCancelledBeforeStage
                | ActorScenario::CallbackCancelledBeforeStageAppendFailure
        ) && (previous == 1
            || (self.scenario == ActorScenario::CallbackTranscriptSibling
                && call.id == "fc_0"
                && previous > 1))
        {
            return Err(meerkat_core::ToolError::callback_pending(
                call.name,
                serde_json::json!({"question":"scoped callback"}),
            ));
        }
        Ok(meerkat_core::ToolResult::new(call.id.into(), "actual read".into(), false).into())
    }
}

#[tokio::test]
async fn native_scoped_factory_core_runner_commits_model_feedback_and_restores_session_policy()
-> TestResult {
    run_scoped_actor_scenario(ActorScenario::Success).await
}

#[tokio::test]
async fn native_scoped_client_delegation_freezes_and_admits_before_actor_execution() -> TestResult {
    run_scoped_actor_scenario(ActorScenario::ClientDelegation).await
}

#[tokio::test]
async fn native_scoped_factory_core_runner_retries_only_committed_rejection() -> TestResult {
    run_scoped_actor_scenario(ActorScenario::Rejected).await
}

#[tokio::test]
async fn native_scoped_factory_core_runner_retries_committed_empty_success() -> TestResult {
    run_scoped_actor_scenario(ActorScenario::EmptySuccess).await
}

#[tokio::test]
async fn native_scoped_factory_core_runner_drop_records_unknown_and_restores_session_policy()
-> TestResult {
    run_scoped_actor_scenario(ActorScenario::Dropped).await
}

#[tokio::test]
async fn native_scoped_factory_core_runner_claims_reused_tool_ids_at_each_boundary() -> TestResult {
    run_scoped_actor_scenario(ActorScenario::ToolBoundaries).await
}

#[tokio::test]
async fn native_scoped_actor_keeps_completed_answer_at_observed_token_limit() -> TestResult {
    run_scoped_actor_scenario(ActorScenario::TokenThreshold).await
}

#[tokio::test]
async fn native_scoped_actor_unmeasured_usage_is_advisory_across_model_boundaries() -> TestResult {
    run_scoped_actor_scenario(ActorScenario::UnmeasuredToolBoundaries).await
}

#[tokio::test]
async fn native_scoped_actor_token_limit_stops_next_boundary_without_provider_retry() -> TestResult
{
    run_scoped_actor_scenario(ActorScenario::TokenToolBoundary).await
}

#[tokio::test]
async fn native_scoped_callback_producer_retains_scope_and_refuses_ordinary_resume() -> TestResult {
    run_scoped_actor_scenario(ActorScenario::Callback).await
}

#[tokio::test]
async fn native_scoped_callback_admission_uses_finalized_actor_boundary() -> TestResult {
    run_scoped_actor_scenario(ActorScenario::CallbackAdmission).await
}

#[tokio::test]
async fn native_scoped_callback_admission_rejects_stale_actor_body_atomically() -> TestResult {
    run_scoped_actor_scenario(ActorScenario::CallbackAdmissionStale).await
}

#[tokio::test]
async fn native_scoped_callback_continuation_applies_results_once_without_another_prompt()
-> TestResult {
    run_scoped_actor_scenario(ActorScenario::CallbackResume).await
}

#[tokio::test]
async fn native_scoped_callback_continuation_retains_adjacent_sibling_transcript() -> TestResult {
    run_scoped_actor_scenario(ActorScenario::CallbackTranscriptSibling).await
}

#[tokio::test]
async fn native_scoped_callback_failure_holds_original_application_without_replay() -> TestResult {
    run_scoped_actor_scenario(ActorScenario::CallbackRecoveryHold).await
}

#[tokio::test]
async fn native_scoped_callback_applied_failure_keeps_terminal_body_and_ordinary_receipt()
-> TestResult {
    run_scoped_actor_scenario(ActorScenario::CallbackAppliedFailure).await
}

#[tokio::test]
async fn native_scoped_callback_cancelled_before_admission_holds_without_fabricating_result()
-> TestResult {
    run_scoped_actor_scenario(ActorScenario::CallbackCancelledBeforeAdmission).await
}

#[tokio::test]
async fn native_scoped_callback_cancelled_before_stage_closes_exact_continuation() -> TestResult {
    run_scoped_actor_scenario(ActorScenario::CallbackCancelledBeforeStage).await
}

#[tokio::test]
async fn native_scoped_callback_cancelled_continuation_recovers_after_append_failure_and_full_reopen()
-> TestResult {
    run_scoped_actor_scenario(ActorScenario::CallbackCancelledBeforeStageAppendFailure).await
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ActorScenario {
    Success,
    ClientDelegation,
    Rejected,
    EmptySuccess,
    Dropped,
    ToolBoundaries,
    TokenThreshold,
    TokenToolBoundary,
    UnmeasuredToolBoundaries,
    Callback,
    CallbackAdmission,
    CallbackAdmissionStale,
    CallbackResume,
    CallbackTranscriptSibling,
    CallbackRecoveryHold,
    CallbackAppliedFailure,
    CallbackCancelledBeforeAdmission,
    CallbackCancelledBeforeStage,
    CallbackCancelledBeforeStageAppendFailure,
}

impl ActorScenario {
    fn has_tools(self) -> bool {
        self.tool_boundaries() != 0
    }

    fn tool_boundaries(self) -> usize {
        match self {
            Self::ToolBoundaries
            | Self::UnmeasuredToolBoundaries
            | Self::Callback
            | Self::CallbackAdmission
            | Self::CallbackAdmissionStale => 2,
            Self::CallbackResume
            | Self::CallbackTranscriptSibling
            | Self::CallbackRecoveryHold
            | Self::CallbackAppliedFailure
            | Self::CallbackCancelledBeforeAdmission
            | Self::CallbackCancelledBeforeStage
            | Self::CallbackCancelledBeforeStageAppendFailure => 2,
            Self::TokenToolBoundary => 1,
            _ => 0,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ActorBackend {
    Memory,
    WholeBlob,
    HeadCanonical,
}

async fn run_scoped_actor_scenario(scenario: ActorScenario) -> TestResult {
    let backends: &[ActorBackend] =
        if scenario == ActorScenario::CallbackCancelledBeforeStageAppendFailure {
            &[ActorBackend::WholeBlob, ActorBackend::HeadCanonical]
        } else if matches!(
            scenario,
            ActorScenario::CallbackAdmission
                | ActorScenario::CallbackAdmissionStale
                | ActorScenario::CallbackResume
                | ActorScenario::CallbackTranscriptSibling
                | ActorScenario::CallbackRecoveryHold
                | ActorScenario::CallbackAppliedFailure
                | ActorScenario::CallbackCancelledBeforeAdmission
                | ActorScenario::CallbackCancelledBeforeStage
                | ActorScenario::CallbackCancelledBeforeStageAppendFailure
        ) {
            &[
                ActorBackend::Memory,
                ActorBackend::WholeBlob,
                ActorBackend::HeadCanonical,
            ]
        } else {
            &[ActorBackend::WholeBlob, ActorBackend::HeadCanonical]
        };
    for backend in backends {
        let head_canonical = *backend == ActorBackend::HeadCanonical;
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("runtime.sqlite3");
        let session = Session::new();
        if head_canonical {
            meerkat_store::SqliteSessionStore::open(&path)?
                .save(&session)
                .await?;
        }
        let initial: Arc<dyn RuntimeStore> = if *backend == ActorBackend::Memory {
            Arc::new(meerkat_runtime::store::InMemoryRuntimeStore::new())
        } else {
            Arc::new(SqliteRuntimeStore::new_whole_blob(&path)?)
        };
        initial
            .commit_session_snapshot(
                &LogicalRuntimeId::for_session(session.id()),
                SerializedSessionSnapshot {
                    session_snapshot: Arc::new(serde_json::to_vec(&session)?),
                },
            )
            .await?;
        let store: Arc<dyn RuntimeStore> = if head_canonical {
            drop(initial);
            Arc::new(SqliteRuntimeStore::new_head_canonical(&path)?)
        } else {
            initial
        };
        let blobs: Arc<dyn BlobStore> = Arc::new(meerkat_store::MemoryBlobStore::new());
        let machine = Arc::new(MeerkatMachine::persistent(store.clone(), blobs.clone()));
        let bindings = machine.prepare_bindings(session.id().clone()).await?;
        assert!(meerkat_runtime::session_runtime_bindings_have_machine_authority(&bindings));
        let actor_session = callback_admission::read_body(store.as_ref(), &path, session.id())
            .await?
            .into_session();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let client = meerkat_openai::OpenAiClient::new("fixture-key".into())
            .with_base_url(format!("http://{}", listener.local_addr()?));
        let tool_calls = Arc::new(AtomicUsize::new(0));
        let sibling_blocks = if scenario == ActorScenario::CallbackTranscriptSibling {
            let blob_ref = blobs.put_image(
                "image/png",
                "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aL8sAAAAASUVORK5CYII=",
            ).await?;
            vec![
                meerkat_core::AssistantBlock::Text {
                    text: "retained sibling transcript".into(),
                    meta: None,
                },
                meerkat_core::AssistantBlock::Image {
                    image_id: meerkat_core::AssistantImageId::new(uuid::Uuid::new_v4()),
                    blob_ref,
                    media_type: meerkat_core::MediaType::new("image/png"),
                    width: 1,
                    height: 1,
                    revised_prompt: meerkat_core::RevisedPromptDisposition::NotRequested,
                    meta: meerkat_core::ProviderImageMetadata::NotEmitted,
                },
            ]
        } else {
            Vec::new()
        };
        let tools: Arc<dyn AgentToolDispatcher> = if scenario.has_tools() {
            Arc::new(ReadTool {
                calls: tool_calls.clone(),
                scenario,
                sibling_blocks,
            })
        } else {
            Arc::new(NoTools)
        };
        let build = meerkat::AgentBuildConfig {
            provider: Some(meerkat_core::Provider::OpenAI),
            custom_models: std::collections::BTreeMap::from([(
                "fixture-model".into(),
                serde_json::from_value(serde_json::json!({
                    "provider": "openai", "context_window": 100_000, "max_output_tokens": 1024,
                    "vision": scenario == ActorScenario::CallbackTranscriptSibling,
                }))?,
            )]),
            max_tokens: Some(128),
            blob_store_override: (scenario == ActorScenario::CallbackTranscriptSibling)
                .then_some(blobs),
            provider_params: scenario.has_tools().then(|| {
                use meerkat_core::lifecycle::run_primitive::{
                    OpaqueProviderBody, OpenAiProviderTag, ProviderParamsOverride, ProviderTag,
                };
                ProviderParamsOverride {
                    provider_tag: Some(ProviderTag::OpenAi(OpenAiProviderTag {
                        web_search: Some(OpaqueProviderBody::from_value(
                            &serde_json::json!({"type":"web_search"}),
                        )),
                        ..Default::default()
                    })),
                    ..Default::default()
                }
            }),
            resume_session: Some(actor_session),
            runtime_build_mode: meerkat_core::RuntimeBuildMode::SessionOwned(bindings.clone()),
            llm_client_override: Some(Arc::new(client)),
            tool_dispatcher_override: Some(tools),
            session_store_override: Some(Arc::new(meerkat_store::StoreAdapter::new(Arc::new(
                meerkat_store::MemoryStore::new(),
            )))),
            system_prompt: meerkat::SystemPromptOverride::Disable,
            ..meerkat::AgentBuildConfig::new("fixture-model")
        };
        let mut config = meerkat::Config::default();
        config.skills.enabled = false;
        let mut agent = meerkat::AgentFactory::minimal()
            .build_agent(build, &config)
            .await?;
        let grant = LiveExecutionGrantIssuer::new(store.clone()).activate(
            LiveGrantActivationRequest::<()> {
                activation_id: meerkat_core::live_execution::activation::LiveActivationId::parse("actor-fixture")?,
                declaration: serde_json::from_value(serde_json::json!({
                    "issuer_realm":"owner", "profile_id":"voice", "profile_revision":vec![2;32],
                    "requesting_realms":["caller"], "executor":{"kind":"session","session_id":session.id()},
                    "allowed_evidence":["application_snapshot"],
                    "permission": {
                        "allowed_mutations":["read_only"],
                        "tools":{"kind":"allow_listed","names":["allowed_tool"]},
                        "limits":{"max_requests":2,"max_concurrent_requests":1,
                            "max_effects_per_request":if scenario == ActorScenario::CallbackTranscriptSibling {
                                6
                            } else if scenario.has_tools() { 5 } else { 2 },
                            "max_tokens_per_request": match scenario { ActorScenario::TokenThreshold => 1, ActorScenario::TokenToolBoundary => 2, ActorScenario::UnmeasuredToolBoundaries => 3, _ => 1000 },"max_duration_ms":10_000}
                    },
                    "generation":1,"revoke_policy":"cancel_pending_and_request_running_cancellation"
                }))?,
                requesting_realm: serde_json::from_value(serde_json::json!("caller"))?,
                executor: serde_json::from_value(serde_json::json!({
                    "selector":{"kind":"session","session_id":session.id()},
                    "binding":{"session_id":session.id(),"realm":"executor",
                        "runtime_epoch":bindings.epoch_id(),"binding_generation":0}
                }))?,
            }, Arc::new(CurrentFence),
        ).await?;
        use meerkat_core::live_observation::{
            LiveTranscriptDirection, LiveTranscriptObservation, LiveTranscriptRange,
        };
        let lifecycle = store
            .observe_machine_lifecycle(&LogicalRuntimeId::for_session(session.id()))
            .await?
            .version()
            .ok_or("current lifecycle")?
            .clone();
        let mut channel = LiveTranscriptStoreOwner::new(
            store.clone(),
            session.id().clone(),
            Arc::new(CurrentFence),
        )
        .activate_channel(LiveChannelId::new("voice"), lifecycle)
        .await?;
        let activated = store
            .live_ledger_ops()
            .ok_or("ledger")?
            .load_live_head(session.id())
            .await?
            .ok_or("activation")?;
        let observed = channel
            .append(LiveTranscriptObservation::new(
                LiveTranscriptDirection::Input,
                LiveTranscriptRange::new(0.0, 1.0)?,
                "scoped model",
            ))
            .await?;
        let (source, client_admission) = if scenario == ActorScenario::ClientDelegation {
            use meerkat_core::live_execution::request::LiveProviderReference;
            use meerkat_runtime::live_ledger::transcript_authority::LiveClientDelegationOutcome;
            let outcome = channel
                .reserve_and_admit_client_source(
                    &machine,
                    LiveProviderReference::new("client-delegation")?,
                    0.0,
                    Some(&grant),
                )
                .await?;
            let LiveClientDelegationOutcome::Admitted {
                authority,
                completion,
            } = outcome
            else {
                return Err("client source was not admitted".into());
            };
            let replay = channel
                .reserve_and_admit_client_source(
                    &machine,
                    LiveProviderReference::new("client-delegation")?,
                    0.0,
                    None,
                )
                .await?;
            let LiveClientDelegationOutcome::Source(LiveSourceReservationOutcome::Retained(entry)) =
                replay
            else {
                return Err("source replay minted another admission".into());
            };
            let LiveSourceEntryRecord::Reservation { record } = *entry else {
                return Err("client source lost".into());
            };
            assert!(matches!(record.disposition(),
                meerkat_runtime::live_source::LiveSourceDisposition::Admitted { receipt }
                    if receipt.as_ref() == authority.record()));
            (
                authority.record().source().clone(),
                Some((authority, completion)),
            )
        } else {
            let interval = LiveObservationInterval::new(
                activated.reference.event_count,
                observed.event_count,
            )?;
            let LiveSourceReservationOutcome::Retained(entry) = channel
                .reserve_application_source(
                    LiveApplicationRequestId::from_uuid(uuid::Uuid::new_v4()),
                    interval,
                    Some(&grant),
                )
                .await?
            else {
                return Err("production source was not retained".into());
            };
            let LiveSourceEntryRecord::Reservation { record } = *entry else {
                return Err("production source unexpectedly cancelled".into());
            };
            assert!(matches!(
                record.disposition(),
                meerkat_runtime::live_source::LiveSourceDisposition::Reserved {}
            ));
            (record.source().clone(), None)
        };
        let ops = store.live_ledger_ops().ok_or("ledger")?;
        if matches!(
            scenario,
            ActorScenario::CallbackAdmission
                | ActorScenario::CallbackAdmissionStale
                | ActorScenario::CallbackResume
                | ActorScenario::CallbackTranscriptSibling
                | ActorScenario::CallbackRecoveryHold
                | ActorScenario::CallbackAppliedFailure
                | ActorScenario::CallbackCancelledBeforeAdmission
                | ActorScenario::CallbackCancelledBeforeStage
                | ActorScenario::CallbackCancelledBeforeStageAppendFailure
        ) {
            // The actor owns its bindings; the outer fixture must not keep a
            // second durable-store owner alive across the cold-reopen proof.
            drop(bindings);
            callback_admission::run(callback_admission::Fixture {
                machine,
                store,
                path,
                agent,
                grant,
                source,
                channel,
                listener,
                stale_body: scenario == ActorScenario::CallbackAdmissionStale,
                resume: matches!(
                    scenario,
                    ActorScenario::CallbackResume
                        | ActorScenario::CallbackTranscriptSibling
                        | ActorScenario::CallbackRecoveryHold
                        | ActorScenario::CallbackAppliedFailure
                ),
                transcript_sibling: scenario == ActorScenario::CallbackTranscriptSibling,
                reopen_cancelled_callback: scenario
                    == ActorScenario::CallbackCancelledBeforeAdmission
                    && *backend != ActorBackend::Memory,
                cancellation: match scenario {
                    ActorScenario::CallbackCancelledBeforeAdmission => {
                        callback_admission::CallbackCancellation::BeforeAdmission
                    }
                    ActorScenario::CallbackCancelledBeforeStage => {
                        callback_admission::CallbackCancellation::BeforeStage
                    }
                    ActorScenario::CallbackCancelledBeforeStageAppendFailure => {
                        callback_admission::CallbackCancellation::BeforeStageAppendFailure
                    }
                    _ => callback_admission::CallbackCancellation::None,
                },
                failure: match scenario {
                    ActorScenario::CallbackRecoveryHold => {
                        callback_admission::CallbackFailure::WithoutAppliedWitness
                    }
                    ActorScenario::CallbackAppliedFailure => {
                        callback_admission::CallbackFailure::AppliedTerminal
                    }
                    _ => callback_admission::CallbackFailure::None,
                },
            })
            .await?;
            continue;
        }
        let (admitted, _completion) = match client_admission {
            Some(admission) => admission,
            None => machine.commit_live_input_admission(source, &grant).await?,
        };
        channel.close().await?;
        drop(channel);
        let (run_id, authority) = machine
            .prepare_next_batch_for_live_scope_authority_test(
                session.id(),
                admitted.record().input_id(),
            )
            .await?;
        let RunExecutionAuthority::Scoped(scope) = authority else {
            return Err("scope lost".into());
        };
        bindings.turn_state().start_conversation_run(
            run_id.clone(),
            TurnPrimitiveKind::ConversationTurn,
            ContentShape::Conversation,
            false,
            false,
            0,
        )?;
        let context = machine
            .scoped_actor_execution_context(scope.clone())
            .await?;
        let captured = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let capture = captured.clone();
        let first_captured = Arc::new(tokio::sync::Notify::new());
        let release_first = Arc::new(tokio::sync::Notify::new());
        let entered = first_captured.clone();
        let release = release_first.clone();
        let router = axum::Router::new().route("/v1/responses", axum::routing::post(
            move |axum::Json(body): axum::Json<serde_json::Value>| {
                let capture = capture.clone();
                let entered = entered.clone();
                let release = release.clone();
                async move {
                    let index = {
                        let mut bodies = capture.lock().await;
                        bodies.push(body);
                        bodies.len()
                    };
                    if index == 1 && scenario == ActorScenario::Rejected {
                        return (
                            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                            [("content-type", "application/json")],
                            r#"{"error":{"type":"server_error","message":"explicit rejected request"}}"#.into(),
                        );
                    }
                    if index == 1 && scenario == ActorScenario::Dropped {
                        entered.notify_one();
                        release.notified().await;
                    }
                    let output = if scenario == ActorScenario::EmptySuccess && index == 1 {
                        serde_json::json!([])
                    } else if index <= scenario.tool_boundaries() {
                        serde_json::json!([{"id":"item_fixture","type":"function_call",
                            "call_id":"fc_0","name":"allowed_tool","arguments":"{}","status":"completed"}])
                    } else {
                        serde_json::json!([{"id":"msg_fixture","type":"message","role":"assistant","status":"completed",
                            "content":[{"type":"output_text","text":"ok","annotations":[]}]}])
                    };
                    let response = if scenario == ActorScenario::UnmeasuredToolBoundaries && (2..=3).contains(&index) {
                        serde_json::json!({"id":"resp_fixture","status":"completed","output":output})
                    } else {
                        serde_json::json!({"id":"resp_fixture","status":"completed","output":output,
                            "usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}})
                    };
                    let event = serde_json::json!({"type":"response.completed","response":response});
                    (axum::http::StatusCode::OK, [("content-type","text/event-stream")], format!("data: {event}\n\n"))
                }
            },
        ));
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let server = async {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await?;
            TestResult::Ok(())
        };
        let (events, mut receiver) = tokio::sync::mpsc::channel(256);
        let runs = async {
            agent.set_runtime_execution_kind(Some(
                meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn,
            ));
            let scoped = agent.run_with_events_and_execution_context(
                "scoped model".into(),
                Vec::new(),
                Vec::new(),
                None,
                RunExecutionContext::Scoped(context),
                events.clone(),
            );
            if scenario == ActorScenario::Dropped {
                let mut scoped = Box::pin(scoped);
                tokio::select! {
                    () = first_captured.notified() => {}
                    result = &mut scoped => return Err(format!(
                        "scoped run finished before its physical request was held: {result:?}",
                    ).into()),
                }
                drop(scoped);
                release_first.notify_one();
                bindings.turn_state().cancel_now(run_id.clone())?;
                bindings.turn_state().cancellation_observed(run_id)?;
            } else if scenario == ActorScenario::Callback {
                assert!(matches!(
                    scoped.await,
                    Err(meerkat_core::AgentError::CallbackPending { tool_use_id, .. })
                        if tool_use_id == "fc_0"
                ));
            } else if scenario == ActorScenario::TokenToolBoundary {
                let result = scoped.await?;
                assert_eq!(
                    result.terminal_cause_kind,
                    Some(meerkat_core::TurnTerminalCauseKind::BudgetExhausted)
                );
                assert_eq!(result.turns, 1);
                assert_eq!(result.tool_calls, 1);
            } else {
                assert_eq!(scoped.await?.text, "ok");
            }
            let expected = match scenario {
                ActorScenario::Success
                | ActorScenario::ClientDelegation
                | ActorScenario::TokenThreshold => {
                    vec![LivePhysicalEffectOutcome::Succeeded]
                }
                ActorScenario::Rejected => vec![
                    LivePhysicalEffectOutcome::Failed,
                    LivePhysicalEffectOutcome::Succeeded,
                ],
                ActorScenario::Dropped => vec![LivePhysicalEffectOutcome::Unknown],
                ActorScenario::EmptySuccess | ActorScenario::TokenToolBoundary => {
                    vec![LivePhysicalEffectOutcome::Succeeded; 2]
                }
                ActorScenario::ToolBoundaries | ActorScenario::UnmeasuredToolBoundaries => {
                    vec![LivePhysicalEffectOutcome::Succeeded; 5]
                }
                ActorScenario::Callback
                | ActorScenario::CallbackAdmission
                | ActorScenario::CallbackAdmissionStale => vec![
                    LivePhysicalEffectOutcome::Succeeded,
                    LivePhysicalEffectOutcome::Succeeded,
                    LivePhysicalEffectOutcome::Succeeded,
                    LivePhysicalEffectOutcome::Unknown,
                ],
                ActorScenario::CallbackResume
                | ActorScenario::CallbackTranscriptSibling
                | ActorScenario::CallbackRecoveryHold
                | ActorScenario::CallbackAppliedFailure
                | ActorScenario::CallbackCancelledBeforeAdmission
                | ActorScenario::CallbackCancelledBeforeStage
                | ActorScenario::CallbackCancelledBeforeStageAppendFailure => {
                    return Err("callback resume must use its runtime-loop fixture".into());
                }
            };
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                loop {
                    let head = ops.load_live_head(session.id()).await?.ok_or("head")?;
                    let history = ops
                        .read_live_history(&LiveHistoryReadRequest::new(
                            head.reference,
                            None,
                            0,
                            64,
                        )?)
                        .await?;
                    let actual = history
                        .records()
                        .iter()
                        .filter_map(|record| {
                            if let LiveLedgerRecord::Completion(record) = record
                                && let LiveCompletionEvent::EffectTerminal { outcome, .. } =
                                    record.event
                            {
                                Some(outcome)
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>();
                    if actual.len() >= expected.len() {
                        assert_eq!(actual, expected);
                        return TestResult::Ok(());
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                }
            })
            .await??;
            let committed = ops.load_live_head(session.id()).await?.ok_or("head")?;
            if scenario == ActorScenario::Callback {
                let Some(meerkat_core::session::CallbackBatchObservation::Pending {
                    identity,
                    pending_tool_use_ids,
                }) = agent.session().callback_batch_observation()?
                else {
                    return Err("actual scoped actor lost its callback batch".into());
                };
                assert_eq!(identity.session_id(), session.id());
                assert_eq!(identity.run_id(), &scope.record().run_id);
                assert_eq!(identity.execution_scope(), Some(scope.scope_id()));
                assert_eq!(pending_tool_use_ids, ["fc_0"]);
                let effect_id = identity
                    .scoped_tool_effect_id("fc_0")?
                    .ok_or("missing callback effect identity")?;
                let snapshot: serde_json::Value =
                    serde_json::from_slice(&committed.payload.request_snapshot)?;
                let mut callback_claims = 0;
                let mut tool_claims = 0;
                for (claim_id, record) in snapshot["claim_records"]
                    .as_object()
                    .ok_or("missing claims")?
                {
                    let claim: meerkat_core::execution_scope::ScopedEffectClaimRecord<
                        meerkat_core::execution_scope::ScopedEffectTarget,
                    > = serde_json::from_str(
                        record.as_str().ok_or("claim must retain encoded record")?,
                    )?;
                    if matches!(
                        claim.target,
                        meerkat_core::execution_scope::ScopedEffectTarget::ToolDispatch { .. }
                    ) {
                        tool_claims += 1;
                    }
                    if claim.effect_id == effect_id {
                        callback_claims += 1;
                        assert_eq!(claim.scope_id, scope.scope_id());
                        assert_eq!(claim.run_id, scope.record().run_id);
                        assert_eq!(snapshot["claim_phases"][claim_id], "Unknown");
                    }
                }
                assert_eq!(
                    tool_claims, 2,
                    "reused call ID must still have distinct boundary claims"
                );
                assert_eq!(
                    callback_claims, 1,
                    "callback must identify only its actual boundary claim"
                );
                let before = serde_json::to_vec(agent.session())?;
                let Err(error) =
                    agent.apply_pending_callback_tool_results(vec![meerkat_core::ToolResult::new(
                        "fc_0".into(),
                        "answer".into(),
                        false,
                    )])
                else {
                    return Err("ordinary callback application accepted scoped custody".into());
                };
                assert!(
                    matches!(error, meerkat_core::AgentError::ConfigError(message)
                    if message.contains("generated continuation authority"))
                );
                assert_eq!(serde_json::to_vec(agent.session())?, before);
                *agent.session_mut() = serde_json::from_slice(&before)?;
                let Err(error) = agent.run_pending_with_events(events).await else {
                    return Err("ordinary pending run stripped restored callback scope".into());
                };
                assert!(
                    matches!(error, meerkat_core::AgentError::ConfigError(message)
                    if message.contains("generated continuation authority"))
                );
                assert_eq!(serde_json::to_vec(agent.session())?, before);
                assert_eq!(ops.load_live_head(session.id()).await?, Some(committed));
                assert_eq!(tool_calls.load(Ordering::SeqCst), 2);
                let bodies = captured.lock().await;
                assert_eq!(bodies.len(), 2);
                assert!(
                    bodies
                        .iter()
                        .all(|body| body["tools"]
                            .as_array()
                            .is_some_and(|tools| tools.len() == 1
                                && tools[0]["type"] == "function"
                                && tools[0]["name"] == "allowed_tool"))
                );
                stop.send(()).map_err(|()| "server stopped early")?;
                return TestResult::Ok(());
            }
            if matches!(
                scenario,
                ActorScenario::TokenThreshold | ActorScenario::UnmeasuredToolBoundaries
            ) {
                let snapshot: serde_json::Value =
                    serde_json::from_slice(&committed.payload.request_snapshot)?;
                assert_eq!(
                    snapshot["request_known_tokens"][scope.record().request_id.to_string()],
                    2
                );
                let mut unmeasured = 0;
                while let Ok(event) = receiver.try_recv() {
                    if matches!(
                        event,
                        meerkat_core::AgentEvent::TurnUsageAccountingUnmeasured { .. }
                    ) {
                        unmeasured += 1;
                    }
                }
                if scenario == ActorScenario::UnmeasuredToolBoundaries {
                    assert_eq!(unmeasured, 2);
                } else {
                    assert_eq!(unmeasured, 0);
                    assert_eq!(
                        machine.scoped_effect_host()?.resolve_model_attempt(scope.clone(), meerkat_core::ops::OperationId::new()).await?,
                        meerkat_core::execution_scope::ScopedModelAttemptResolution::TokenBudgetExhausted { used: 2,                         limit: 1 },
                    );
                    let request = Arc::new(meerkat_core::execution_scope::ScopedModelRequest::new(
                        machine.scoped_execution_context(scope.clone())?,
                        meerkat_core::ops::OperationId::new(),
                        meerkat_core::SessionLlmIdentity {
                            provider: meerkat_core::Provider::OpenAI,
                            model: "fixture-model".into(),
                            provider_params: None,
                            self_hosted_server_id: None,
                            auth_binding: None,
                        },
                    )?);
                    assert!(
                        request
                            .claim_request(
                                "fixture-model",
                                "forbidden-next-request",
                                meerkat_core::LoweredRequestProvenance::from_body(
                                    meerkat_core::Provider::OpenAI,
                                    meerkat_core::LoweredRequestEncoding::OpenAiResponsesJson,
                                    b"{}"
                                ),
                                meerkat_core::ProviderNativeToolPolicy::DisableAll
                            )
                            .await
                            .is_err()
                    );
                    assert_eq!(captured.lock().await.len(), 1);
                    assert_eq!(
                        ops.load_live_head(session.id()).await?,
                        Some(committed.clone())
                    );
                }
            }
            agent.set_runtime_execution_kind(Some(
                meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn,
            ));
            let ordinary = agent
                .run_with_events("ordinary model".into(), events)
                .await?;
            assert_eq!(ordinary.text, "ok");
            assert_eq!(
                Some(committed),
                ops.load_live_head(session.id()).await?,
                "ordinary follow-up must not retain Live scope or mint another claim"
            );
            let bodies = captured.lock().await;
            let expected_requests = if scenario == ActorScenario::TokenToolBoundary {
                2
            } else if scenario.has_tools() {
                4
            } else {
                expected.len() + 1
            };
            assert_eq!(bodies.len(), expected_requests);
            assert!(bodies.iter().all(|body| body["model"] == "fixture-model"));
            if scenario.has_tools() {
                assert_eq!(
                    tool_calls.load(Ordering::SeqCst),
                    scenario.tool_boundaries()
                );
                let scoped_requests = expected_requests - 1;
                assert!(bodies[..scoped_requests].iter().all(|body| {
                    body["tools"].as_array().is_some_and(|tools| {
                        tools.len() == 1
                            && tools[0]["type"] == "function"
                            && tools[0]["name"] == "allowed_tool"
                    })
                }));
                assert!(
                    bodies[scoped_requests]["tools"]
                        .as_array()
                        .is_some_and(
                            |tools| tools.iter().any(|tool| tool["type"] == "web_search")
                                && tools.iter().any(|tool| tool["type"] == "function"
                                    && tool["name"] == "allowed_tool")
                        ),
                    "ordinary model request must restore configured native tools"
                );
                let results = bodies[scenario.tool_boundaries()]["input"]
                    .as_array()
                    .ok_or("request input")?
                    .iter()
                    .filter(|item| {
                        item["type"] == "function_call_output"
                            && item["call_id"] == "fc_0"
                            && item["output"] == "actual read"
                    })
                    .count();
                assert_eq!(results, scenario.tool_boundaries());
            } else {
                assert_eq!(tool_calls.load(Ordering::SeqCst), 0);
                assert_eq!(bodies[0]["tools"], serde_json::json!([]));
            }
            stop.send(()).map_err(|()| "server stopped early")?;
            TestResult::Ok(())
        };
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            tokio::try_join!(runs, server)
        })
        .await??;
    }
    Ok(())
}

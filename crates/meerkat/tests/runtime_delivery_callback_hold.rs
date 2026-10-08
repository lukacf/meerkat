//! A job delivery that arrives while a callback tool batch awaits its results
//! is held, not appended: the session refuses it as busy, the row stays
//! pending, and the transcript keeps its assistant tool-use tail. Once the
//! host stages the results and the run resumes and settles, the delivery
//! owner applies the held row exactly once, after the tool results.

#![cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use meerkat::surface::{
    SessionServiceDeliveryHost, build_runtime_backed_service, default_persistent_executor,
};
use meerkat::{
    AgentFactory, AttemptClaim, CanonicalArgumentsHash, Config, CreateSessionRequest,
    DetachedJobService, FactoryAgentBuilder, InteractionLineageId, JobId, JobResultRef, JobSpec,
    JobSubmissionKey, PersistentSessionService, RestartClass, RunnerHandleRef, RunnerIdentity,
    RuntimeDeliveryPass, ToolIdentity, WorkerId,
};
use meerkat_client::types::LlmStream;
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::service::{SessionServiceControlExt as _, StageToolResultsRequest};
use meerkat_core::{Message, SessionBuildOptions, ToolResult};
use meerkat_runtime::completion::CompletionOutcome;
use meerkat_runtime::{ContinuationInput, Input, MeerkatMachine, PromptInput};
use tokio::sync::watch;
use tokio::time::Duration;

/// Hang guard for awaited typed events; never a pacing mechanism.
const EVENT_GUARD: Duration = Duration::from_secs(20);
const CALLBACK_TOOL_USE_ID: &str = "toolu_external_callback";

/// First model call asks for the host callback; every later call ends the
/// turn with plain text.
struct CallbackThenTextClient {
    calls: AtomicUsize,
}

fn usage(request: &LlmRequest) -> LlmEvent {
    LlmEvent::UsageUpdate {
        usage: meerkat_core::TurnUsage::host_declared(
            meerkat_core::Provider::OpenAI,
            &request.model,
            meerkat_core::Usage::default(),
        ),
    }
}

#[async_trait::async_trait]
impl LlmClient for CallbackThenTextClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        let events = if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            vec![
                LlmEvent::ToolCallComplete {
                    id: CALLBACK_TOOL_USE_ID.to_string(),
                    name: "external_callback".to_string(),
                    args: serde_json::json!({ "key": "approve" }),
                    meta: None,
                },
                usage(request),
                LlmEvent::Done {
                    outcome: LlmDoneOutcome::Success {
                        stop_reason: meerkat_core::StopReason::ToolUse,
                    },
                },
            ]
        } else {
            vec![
                LlmEvent::TextDelta {
                    delta: "done".to_string(),
                    meta: None,
                },
                usage(request),
                LlmEvent::Done {
                    outcome: LlmDoneOutcome::Success {
                        stop_reason: meerkat_core::StopReason::EndTurn,
                    },
                },
            ]
        };
        Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::Other
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

/// A host tool whose result arrives later through a callback.
struct CallbackPendingDispatcher;

#[async_trait::async_trait]
impl meerkat_core::AgentToolDispatcher for CallbackPendingDispatcher {
    fn tools(&self) -> Arc<[Arc<meerkat_core::ToolDef>]> {
        Arc::from([Arc::new(meerkat_core::ToolDef::new(
            "external_callback",
            "external callback test tool",
            serde_json::json!({
                "type": "object",
                "properties": { "key": { "type": "string" } }
            }),
        ))])
    }

    async fn dispatch(
        &self,
        call: meerkat_core::ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        let args = serde_json::from_str(call.args.get()).unwrap_or(serde_json::Value::Null);
        Err(meerkat_core::ToolError::callback_pending(call.name, args))
    }
}

fn create_request() -> CreateSessionRequest {
    let build = SessionBuildOptions {
        external_tools: Some(Arc::new(CallbackPendingDispatcher)),
        ..SessionBuildOptions::default()
    };
    CreateSessionRequest {
        injected_context: Vec::new(),
        model: "gpt-5.4".to_string(),
        prompt: meerkat_core::ContentInput::Text(String::new()),
        system_prompt: meerkat::SystemPromptOverride::Set("callback hold contract".to_string()),
        max_tokens: None,
        event_tx: None,
        initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
        deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
        build: Some(build),
        labels: None,
    }
}

async fn completed_job(
    jobs: &DetachedJobService,
    realm_id: &str,
    session_id: meerkat::SessionId,
) -> JobId {
    let receipt = jobs
        .submit(JobSpec::new(
            realm_id,
            session_id,
            meerkat::ExecutionIntentId::new(),
            InteractionLineageId::new(),
            ToolIdentity::new("scan", "1").expect("tool"),
            RunnerIdentity::new("scan-runner", "1").expect("runner"),
            RestartClass::Adoptable,
            CanonicalArgumentsHash::new("hash-held-delivery").expect("hash"),
            JobSubmissionKey::new("held-delivery").expect("submission key"),
        ))
        .await
        .expect("submit");
    let claim = jobs
        .claim_attempt(
            &receipt.job_id,
            AttemptClaim::new(
                WorkerId::new("worker").expect("worker"),
                1,
                100,
                RunnerHandleRef::new("runner-handle").expect("handle"),
            ),
        )
        .await
        .expect("claim");
    jobs.complete_attempt(
        &receipt.job_id,
        (&claim).into(),
        2,
        Some(JobResultRef::new("result").expect("result")),
    )
    .await
    .expect("complete");
    receipt.job_id
}

async fn wait_for_pass(
    passes: &mut watch::Receiver<RuntimeDeliveryPass>,
    what: &str,
    predicate: impl FnMut(&RuntimeDeliveryPass) -> bool,
) -> RuntimeDeliveryPass {
    tokio::time::timeout(EVENT_GUARD, passes.wait_for(predicate))
        .await
        .unwrap_or_else(|_| panic!("delivery owner never reached: {what}"))
        .expect("owner pass channel open")
        .clone()
}

async fn authoritative_messages(
    service: &PersistentSessionService<FactoryAgentBuilder>,
    session_id: &meerkat::SessionId,
) -> Vec<Message> {
    service
        .load_authoritative_session(session_id)
        .await
        .expect("load authoritative session")
        .expect("session exists")
        .messages()
        .to_vec()
}

fn is_callback_tool_use(message: &Message) -> bool {
    matches!(message, Message::BlockAssistant(assistant)
        if assistant.tool_calls().any(|call| call.id == CALLBACK_TOOL_USE_ID))
}

fn is_job_delivery(message: &Message, job_id: &JobId) -> bool {
    matches!(message, Message::System(system)
        if system.identity.as_ref().and_then(|identity| identity.source.as_deref())
            == Some(format!("detached_job:{job_id}").as_str()))
}

#[tokio::test]
async fn a_delivery_held_by_a_pending_callback_batch_applies_once_after_its_results() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (manifest, persistence) = meerkat::open_realm_persistence_in(
        temp.path(),
        "callback-hold-realm",
        Some(meerkat_store::RealmBackend::Sqlite),
        Some(meerkat_store::RealmOrigin::Explicit),
    )
    .await
    .expect("open realm persistence");
    let realm_id = manifest.realm.to_string();
    let observer = persistence.clone();
    let jobs = DetachedJobService::new(observer.job_store());
    let owner = observer.runtime_delivery_owner();
    let factory = AgentFactory::new(temp.path().join("sessions"));
    let mut builder = FactoryAgentBuilder::new(factory, Config::default());
    builder.default_llm_client = Some(Arc::new(CallbackThenTextClient {
        calls: AtomicUsize::new(0),
    }));
    // The low-level composition arms no owner, so this test holds the
    // handle and observes every pass.
    let (service, adapter) = build_runtime_backed_service(builder, 4, persistence);
    let service = Arc::new(service);
    let adapter: Arc<MeerkatMachine> = adapter;
    let handle = owner
        .arm(Arc::new(SessionServiceDeliveryHost::new(
            &service,
            &adapter,
            DetachedJobService::new(observer.job_store()),
            Some(realm_id.clone()),
        )))
        .expect("arm the delivery owner");
    let mut passes = handle.subscribe_passes();

    let session = meerkat::Session::new();
    let session_id = session.id().clone();
    let service_for_executor = Arc::clone(&service);
    let adapter_for_executor = Arc::clone(&adapter);
    Box::pin(meerkat::surface::materialize_session(
        &service,
        &adapter,
        session,
        create_request(),
        move |session_id| {
            default_persistent_executor(service_for_executor, adapter_for_executor, session_id)
        },
    ))
    .await
    .expect("materialize session");

    // 1. The turn suspends on the host callback.
    let (_outcome, completion) = adapter
        .accept_input_with_completion(
            &session_id,
            Input::Prompt(PromptInput::new("ask the host", None)),
        )
        .await
        .expect("accept prompt");
    let suspended = tokio::time::timeout(EVENT_GUARD, completion.expect("completion").wait())
        .await
        .expect("prompt settles")
        .expect("completion resolves");
    assert!(
        matches!(&suspended, CompletionOutcome::CallbackPending { tool_name, .. } if tool_name == "external_callback"),
        "the turn suspends on the callback: {suspended:?}"
    );
    let suspended_transcript = authoritative_messages(&service, &session_id).await;
    assert!(
        suspended_transcript
            .last()
            .is_some_and(is_callback_tool_use),
        "the transcript ends in the callback tool-use batch"
    );

    // 2. A job delivery arrives while the batch is pending: refused, held.
    let job_id = completed_job(&jobs, &realm_id, session_id.clone()).await;
    let held = wait_for_pass(&mut passes, "the refused delivery", |pass| {
        pass.blocked_sessions.contains(&session_id)
    })
    .await;
    assert!(
        held.failures
            .iter()
            .any(|failure| failure.contains(&format!("session is busy: {session_id}"))),
        "the refusal is the retryable busy: {:?}",
        held.failures
    );
    // A host append meets the same typed refusal.
    let host_append = service
        .append_system_context(
            &session_id,
            meerkat_core::service::AppendSystemContextRequest::from_text("host note"),
        )
        .await;
    assert!(
        matches!(
            host_append,
            Err(meerkat_core::service::SessionControlError::Session(
                meerkat_core::service::SessionError::Busy { .. }
            ))
        ),
        "a host append during the pending batch is a retryable busy: {host_append:?}"
    );
    assert_eq!(
        authoritative_messages(&service, &session_id).await,
        suspended_transcript,
        "a refused delivery leaves the transcript untouched"
    );

    // 3. The host stages the callback result and resumes with an ordinary
    //    continuation: the resumed run applies the staged results before it
    //    calls the model.
    service
        .stage_tool_results(
            &session_id,
            StageToolResultsRequest {
                results: vec![ToolResult::new(
                    CALLBACK_TOOL_USE_ID.to_string(),
                    "approved".to_string(),
                    false,
                )],
            },
        )
        .await
        .expect("stage the callback result");
    let (_outcome, completion) = adapter
        .accept_input_with_completion(
            &session_id,
            Input::Continuation(ContinuationInput::detached_background_op_completed()),
        )
        .await
        .expect("accept the resume continuation");
    let resumed = tokio::time::timeout(EVENT_GUARD, completion.expect("completion").wait())
        .await
        .expect("resumed run settles")
        .expect("completion resolves");
    assert!(
        !matches!(resumed, CompletionOutcome::CallbackPending { .. }),
        "the resumed run completes: {resumed:?}"
    );

    // 4. The run settlement retries the held row; it applies exactly once.
    //    A pass after the held one that no longer reports the session blocked
    //    drained its row. Not `applied > 0` on the observed pass: the passes
    //    channel keeps only the latest pass, and the store-watch wake that
    //    follows the delivery's own commit (#1813) can replace the applying
    //    pass before this waiter reads it. The transcript and the backlog
    //    below prove the single application.
    wait_for_pass(&mut passes, "the held delivery applied", |pass| {
        pass.generation > held.generation && !pass.blocked_sessions.contains(&session_id)
    })
    .await;
    let transcript = authoritative_messages(&service, &session_id).await;
    let tool_use = transcript
        .iter()
        .position(is_callback_tool_use)
        .expect("the callback tool use stays in the transcript");
    assert!(
        matches!(&transcript[tool_use + 1], Message::ToolResults { results, .. }
            if results.iter().any(|result| result.tool_use_id == CALLBACK_TOOL_USE_ID)),
        "the tool results directly follow their tool use: {:?}",
        &transcript[tool_use..]
    );
    let deliveries: Vec<usize> = transcript
        .iter()
        .enumerate()
        .filter(|(_, message)| is_job_delivery(message, &job_id))
        .map(|(index, _)| index)
        .collect();
    assert_eq!(
        deliveries.len(),
        1,
        "the held delivery applies exactly once"
    );
    assert!(
        deliveries[0] > tool_use + 1,
        "the delivery lands after the tool results, never inside the batch"
    );
    assert_eq!(
        observer
            .runtime_delivery_inbox()
            .pending_delivery_total()
            .await
            .expect("backlog read"),
        0,
        "no delivery row stays pending"
    );
}

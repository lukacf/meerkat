//! Actual native input and model/tool feedback, with the real Forge dispatcher
//! and protected presenter. HTTP, the model and sibling are controlled fixtures.
//! It includes the exact CLI presentation-outcome helper, not the full main loop.
//! No installed host or live authentication is claimed.
use super::*;
use async_trait::async_trait;
use futures_util::{FutureExt, Stream};
use meerkat::{LlmClient, LlmError, LlmEvent, LlmRequest};
use meerkat_connect::{
    RealmOptions, SessionOptions,
    approval::LocalApprovals,
    forge::FORGE_PREPARE,
};
use meerkat_core::{
    ApprovalActionKind, ApprovalDecision, ApprovalOwnerRef, ApprovalPrincipalId,
    ApprovalProposedAction, ApprovalRequest, ApprovalResourceId, ApprovalResourceKind,
    ApprovalResourceRef, ApprovalRisk, ApprovalStatus, Message, Provider,
    SessionBuildOptions, SessionId, ToolDef, ToolDispatchContext, ToolResult,
    ops::ToolDispatchOutcome,
};
use std::{
    collections::BTreeMap,
    future::Future,
    panic::{AssertUnwindSafe, resume_unwind},
    pin::Pin,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::SystemTime,
};

const SIBLING: &str = "forge_refusal_permitted_sibling";
const INPUT_BOUND: Duration = Duration::from_secs(30);

struct Tools {
    forge: ForgeFrontend,
    sibling: AtomicUsize,
    sessions: Mutex<Vec<SessionId>>,
    refusals: Mutex<Vec<String>>,
}

#[async_trait]
impl AgentToolDispatcher for Tools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        let mut tools = self.forge.tools().to_vec();
        tools.push(Arc::new(ToolDef::new(SIBLING, "Inert same-input continuity observation",
            json!({"type":"object","properties":{},"additionalProperties":false}))));
        tools.into()
    }
    fn tool_mutation_class(&self, name: &str) -> ToolMutationClass {
        if name == SIBLING { ToolMutationClass::ReadOnly }
        else { self.forge.tool_mutation_class(name) }
    }
    async fn dispatch(&self, _call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        Err(ToolError::execution_failed("the native session context is required"))
    }
    async fn dispatch_with_context(&self, call: ToolCallView<'_>, context: &ToolDispatchContext)
        -> Result<ToolDispatchOutcome, ToolError>
    {
        let session = context.origin_session_id()
            .ok_or_else(|| ToolError::execution_failed("the native session context is missing"))?;
        self.sessions.lock().unwrap().push(session.clone());
        if call.name == SIBLING {
            self.sibling.fetch_add(1, Ordering::SeqCst);
            return Ok(ToolResult::new(call.id.into(), json!({"sibling_completed":true}).to_string(), false).into());
        }
        let result = self.forge.dispatch_with_context(call, context).await;
        if let Err(ToolError::ExecutionFailed { message }) = &result {
            self.refusals.lock().unwrap().push(message.clone());
        }
        result
    }
}

struct Model {
    inner: meerkat_client::TestClient,
    issued: AtomicUsize,
    results: Mutex<BTreeMap<String, ToolResult>>,
    expected: &'static str,
}

#[async_trait]
impl LlmClient for Model {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        self.inner.project_replay_messages(messages)
    }
    fn stream<'a>(&'a self, request: &'a LlmRequest)
        -> Pin<Box<dyn Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>>
    {
        for message in &request.messages {
            if let Message::ToolResults { results, .. } = message {
                for result in results {
                    self.results.lock().unwrap().insert(result.tool_use_id.clone(), result.clone());
                }
            }
        }
        let next = self.issued.fetch_add(1, Ordering::SeqCst);
        let (id, name) = match next {
            0 => ("forge-refusal", FORGE_SELECT_HOST),
            1 => {
                let results = self.results.lock().unwrap();
                // The sibling is emitted only after the original refusal has
                // actually returned through native model messages. A wrong
                // result holds this exact input for the owner's cancel/join.
                if !results.get("forge-refusal").is_some_and(|result|
                    result.is_error && result.text_content().contains(self.expected))
                {
                    return Box::pin(futures_util::stream::pending());
                }
                ("sibling", SIBLING)
            }
            _ => return self.inner.stream(request),
        };
        Box::pin(futures_util::stream::iter([
            Ok(LlmEvent::ToolCallComplete { id:id.into(), name:name.into(), args:json!({}), meta:None }),
            Ok(LlmEvent::UsageUpdate { usage:meerkat_core::TurnUsage::host_declared(
                Provider::OpenAI, &request.model, meerkat_core::Usage::default()) }),
            Ok(LlmEvent::Done { outcome:meerkat::LlmDoneOutcome::success(meerkat::StopReason::ToolUse) }),
        ]))
    }
    fn provider(&self) -> Provider { Provider::OpenAI }
    async fn health_check(&self) -> Result<(), LlmError> { Ok(()) }
}

// Exercise the public review owner as a liveness observation. Denying this
// inert request creates no grant, dispatch or Forge configuration effect.
fn liveness_request(session: &SessionId) -> ApprovalRequest {
    ApprovalRequest {
        requester: ApprovalPrincipalId::new("fixture-requester").unwrap(),
        owner: ApprovalOwnerRef::Session { session_id:session.clone() },
        resource: ApprovalResourceRef { kind:ApprovalResourceKind::Runtime,
            id:ApprovalResourceId::new("fixture-selected-realm") },
        proposed_action: ApprovalProposedAction { kind:ApprovalActionKind::Other,
            summary:"Observe the same invocation's review eligibility".into(),
            body:Some(json!({"fixture":"review-owner-liveness","effect":"none"})) },
        risk:ApprovalRisk::Medium,
        request_body:Some(json!({"fixture":"review-owner-liveness"})),
        allowed_decisions:[ApprovalDecision::Approve, ApprovalDecision::Deny].into_iter().collect(),
        expires_at:Some((SystemTime::now() + Duration::from_secs(60)).into()),
        metadata:Default::default(),
        request_provenance:Some(json!({"source":"fixture-host-owner"})),
    }
}

// Box the enclosing schedule as in the existing profile native-turn controls.
// Keep the completion and presenter futures owned outside the timeout observer.
#[inline(never)]
fn same_input(scenario: &'static str) -> Pin<Box<dyn Future<Output = ()>>> {
    Box::pin(async move {
        let server = Server::new(scenario).await;
        let origin = server.host.origin.clone();
        let directory = tempfile::tempdir().unwrap();
        let storage = RealmOptions { context_root:directory.path().to_owned(),
            state_root:Some(directory.path().join("state")),
            user_config_root:Some(directory.path().join("home")) }.resolve("work").unwrap();
        let expected = if scenario == "wrong" { "Forge Console sign-in was refused" }
            else { "The Forge Console response was not the supported login or frontend-support contract" };
        let (front, mut presenter) = super::main_connector::standalone_forge(None).unwrap();
        let schema = front.tools().iter().find(|tool| tool.name == FORGE_PREPARE)
            .unwrap().input_schema.clone();
        let tools = Arc::new(Tools { forge:front, sibling:AtomicUsize::new(0),
            sessions:Mutex::new(Vec::new()), refusals:Mutex::new(Vec::new()) });
        let model = Arc::new(Model { inner:meerkat_client::TestClient::for_provider(Provider::OpenAI),
            issued:AtomicUsize::new(0), results:Mutex::new(BTreeMap::new()), expected });
        let options = SessionOptions {
            model:meerkat::CreateSessionModelResolutionRequest { model:Some("gpt-5.4".into()), ..Default::default() },
            build:SessionBuildOptions { external_tools:Some(tools.clone()),
                llm_client_override:Some(meerkat::encode_llm_client_override_for_service(model.clone())),
                ..Default::default() },
        };
        let session = Box::pin(ConnectSession::open_with_config(storage, meerkat_core::Config::default(), options))
            .await.unwrap();
        let session_id = session.id().clone();
        let (approvals, mut decisions) = LocalApprovals::create_paired_host(directory.path()).unwrap();
        approvals.bind_session(session.id()).unwrap();
        // Exactly one native input. No replacement input/session is submitted
        // after the Forge selection fails.
        let (admission, completion) = session.submit(
            "Select my Forge host; if it is refused, complete the permitted sibling.".into()).await.unwrap();
        let mut pty = Pty::new();
        let mut output = Output::default();
        let observed = output.clone();
        let (stop, stopped) = oneshot::channel();
        let mut stop = Some(stop);
        let presenting = async {
            let interrupt = async { stopped.await.map_err(|_| io::Error::other("fixture stop channel ended")) };
            tokio::pin!(interrupt);
            let prompt = tokio::select! { biased;
                _ = interrupt.as_mut() => return Ok(false),
                prompt = presenter.next() => prompt,
            };
            match prompt {
                Some(prompt) => present_forge_selection(prompt, &mut pty.input, &mut output, interrupt.as_mut()).await,
                None => Err(meerkat_connect::ConnectError::InvalidInput("fixture presenter ended".into())),
            }
        };
        let entering = AssertUnwindSafe(enter(&pty.master, &pty.observer, &observed, &origin, PASSWORD)).catch_unwind();
        let completion_result = Arc::new(Mutex::new(None));
        let retained_result = completion_result.clone();
        let mut waiting = Box::pin(async move {
            let result = completion.wait().await;
            *retained_result.lock().unwrap() = Some(result);
        });
        let kept = Arc::new(AtomicBool::new(false));
        let entered_route = kept.clone();
        let mut joined = Box::pin(async {
            let (presented, entered) = tokio::join!(presenting, entering);
            // Include and call the actual Toolkit main helper with this same
            // native admission, completion and approval owner. No copied
            // retirement/cancellation policy substitutes for that branch.
            let routed = super::main_connector::continue_after_presentation(
                presented, &session, &admission, &approvals, waiting.as_mut()).await;
            if matches!(&routed, Ok(true)) {
                entered_route.store(true, Ordering::SeqCst);
                waiting.as_mut().await;
            }
            (routed, entered)
        });
        let mut timed_out = false;
        let mut cancelled = None;
        let (routed, entered) = match timeout(INPUT_BOUND, joined.as_mut()).await {
            Ok(result) => result,
            Err(_) => {
                timed_out = true;
                let _ = stop.take().unwrap().send(());
                // Before the keep branch returns, the exact main helper owns
                // stop/cancel/join. After it keeps the input, a wrong model
                // result can still require cancellation of that same input.
                if kept.load(Ordering::SeqCst) {
                    cancelled = Some(session.cancel(&admission).await);
                }
                joined.as_mut().await
            }
        };
        drop(joined);
        drop(waiting);

        let completed = matches!(completion_result.lock().unwrap().as_ref(),
            Some(Ok(meerkat_runtime::completion::CompletionOutcome::Completed(_))));
        let liveness = if matches!(&routed, Ok(true)) && completed
        {
            let probe = async {
                tokio::join!(approvals.review_access(liveness_request(session.id())), async {
                    match decisions.next().await {
                        Some(mut prompt) => prompt.decide(ApprovalDecision::Deny),
                        None => Err(meerkat_connect::ConnectError::InvalidInput("review owner retired".into())),
                    }
                })
            };
            tokio::pin!(probe);
            match timeout(BOUND, probe.as_mut()).await {
                Ok(result) => Some((false, result)),
                Err(_) => { approvals.retire(); Some((true, probe.as_mut().await)) }
            }
        } else { None };
        let closed = session.close().await;
        let calls = server.finish().await;
        // All original work and the loopback server are joined before any
        // regression assertion or saved PTY-entry panic is propagated.
        if let Err(panic) = entered { resume_unwind(panic); }
        pty.restored();
        assert!(!timed_out, "same admitted input did not settle: cancellation={cancelled:?}");
        closed.unwrap();
        assert!(matches!(routed, Ok(true)), "ordinary refusal must leave the main route open: {routed:?}");
        let outcome = completion_result.lock().unwrap().take().expect("the original completion was joined");
        assert!(matches!(outcome.unwrap(), meerkat_runtime::completion::CompletionOutcome::Completed(_)));
        assert_eq!(tools.sibling.load(Ordering::SeqCst), 1);
        assert_eq!(*tools.sessions.lock().unwrap(), [session_id.clone(), session_id]);
        let refusals = tools.refusals.lock().unwrap();
        assert_eq!(refusals.len(), 1);
        assert!(refusals[0].contains(expected));
        let results = model.results.lock().unwrap();
        assert!(results["forge-refusal"].is_error);
        assert!(results["forge-refusal"].text_content().contains(expected));
        assert!(!results["sibling"].is_error);
        assert!(results["sibling"].text_content().contains("sibling_completed"));
        let (probe_timed_out, (record, decision)) = liveness.expect("review owner was checked after the sibling");
        assert!(!probe_timed_out);
        decision.unwrap();
        assert_eq!(record.unwrap().status, ApprovalStatus::Denied);
        assert!(!approvals.has_uncertain_dispatch());
        assert_eq!(tools.forge.tools().iter().find(|tool| tool.name == FORGE_PREPARE)
            .unwrap().input_schema, schema, "refusal must preserve generic setup mode");
        let schema = schema.to_string();
        assert!(schema.contains("configuration") && schema.contains("expected_revision"));
        assert!(!schema.contains("telegram_user_id") && !schema.contains("participant_id"));
        for private in [PASSWORD, NONCE, SESSION, origin.as_str(), "canary"] {
            assert!(!refusals[0].contains(private));
            assert!(!results["forge-refusal"].text_content().contains(private));
            assert!(!output.text().contains(private));
        }
        let expected_calls = if scenario == "wrong" { vec!["GET /login", "POST /login"] }
            else { vec!["GET /login", "POST /login", "GET /console/forge/frontends/descriptor"] };
        assert_eq!(calls, expected_calls, "one sign-in, no retry/Reading fallback and no Forge prepare/submit");
    })
}

#[tokio::test]
async fn forge_refusal_reaches_same_native_input_and_permitted_sibling_with_reviews_alive() {
    for scenario in ["wrong", "descriptor-missing", "descriptor-malformed"] {
        same_input(scenario).await;
    }
}

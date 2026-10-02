//! Collector regression, not a second native-ingress proof. Uses the existing
//! fixture's application owners plus real grants, Agent, runtime turn authority,
//! and RuntimeOpsLifecycleRegistry. Only audit failure and model output are fake.
use super::*;
use meerkat_authorization::grant_policy::GrantBackedWorkPolicy;
use meerkat_authorization_contracts::audit::{
    AuthorizationAuditObservation, AuthorizationAuditSink,
};
use meerkat_core::AssistantBlock;
use meerkat_core::agent::{BindOutcome, DispatcherCapabilities, OpsLifecycleBindError};
use meerkat_core::authorization::{OperationObservationError, WorkAuthorizationContext};
use meerkat_core::ops_lifecycle::*;
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::Poll;

const BLOCKED: &str = "audit_blocked";
const HEALTHY: &str = "healthy";
const CALLBACK: &str = "ask_user";
const EFFECT: &str = "healthy deferred sibling effect";

#[derive(Default)]
struct FailingEntrySink {
    blocked_operation: Mutex<Option<meerkat_core::OperationId>>,
    failed_entries: AtomicUsize,
    observations: Mutex<Vec<AuthorizationAuditObservation>>,
}
impl AuthorizationAuditSink for FailingEntrySink {
    fn append(
        &self,
        event: AuthorizationAuditObservation,
    ) -> Result<(), OperationObservationError> {
        if let AuditObservation::Prepared { target, .. } = &event.observation
            && matches!(target.as_ref(), AuditTarget::Tool { tool_name, .. } if tool_name == BLOCKED)
        {
            *self.blocked_operation.lock().expect("injected target") =
                Some(event.operation_id.clone());
        }
        if matches!(event.observation, AuditObservation::Entry)
            && self
                .blocked_operation
                .lock()
                .expect("injected target")
                .as_ref()
                == Some(&event.operation_id)
        {
            self.failed_entries.fetch_add(1, Ordering::SeqCst);
            return Err(OperationObservationError);
        }
        self.observations.lock().expect("observations").push(event);
        Ok(())
    }
}

struct DeferredResourceOwner;
impl OperationPolicyOwner for DeferredResourceOwner {
    fn authorize_controller_admission(
        &self,
        association: &InputAuthorityAssociation,
        facts: &meerkat_core::ControllerModelFacts,
        now_ms: u64,
    ) -> Result<
        meerkat_authorization::grant_policy::ControllerAdmissionAllowance,
        meerkat_core::OperationAuthorizationError,
    > {
        RecordOwner.authorize_controller_admission(association, facts, now_ms)
    }

    fn authorize_operation(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        if matches!(&binding.facts().operation, AuthorizationOperation::Model(_)) {
            return RecordOwner.authorize_operation(association, binding, purpose, now_ms);
        }
        let AuthorizationOperation::Tool(facts) = &binding.facts().operation else {
            return Err(denied().into());
        };
        if purpose != LocalPolicyPurpose::Operation
            || ![BLOCKED, HEALTHY, CALLBACK].contains(&facts.name.as_str())
        {
            return Err(denied().into());
        }
        Ok(LocalPolicyAllowance {
            operation_values: vec![LocalOperationValues {
                action: action("read"),
                resource_domain: domain(),
                processor: ProcessorRef::Principal {
                    principal: principal("executor"),
                },
                audience: AudienceRef::Principal {
                    principal: principal("requester"),
                },
            }],
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: now_ms + 60_000,
        })
    }
}

struct DeferredProvider {
    callback: bool,
    requests: AtomicUsize,
}
#[async_trait]
impl LlmClient for DeferredProvider {
    fn plain_model_route(
        &self,
        logical_model: &str,
    ) -> Result<meerkat_llm_core::PlainModelRoute, meerkat_core::ControllerFactsUnavailable> {
        if logical_model != MODEL {
            return Err(meerkat_core::ControllerFactsUnavailable);
        }
        meerkat_llm_core::PlainModelRoute::new(ENDPOINT, MODEL)
    }

    fn controller_model_selection(&self) -> Option<ControllerModelSelection> {
        Some(selection())
    }
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }
    fn stream<'a>(&'a self, _: &'a LlmRequest) -> LlmStream<'a> {
        panic!("authorization must be forwarded")
    }
    fn stream_prepared<'a>(&'a self, request: &'a PreparedLlmRequest) -> LlmStream<'a> {
        let checked = request
            .authorization()
            .expect("work context reaches controller")
            .prepare(model_facts())
            .expect("actual grant and application policy permit controller")
            .current()
            .expect("current controller");
        checked
            .observe_entry()
            .expect("only named tool entry is injected");
        checked
            .observe_outcome(OperationObservedOutcome::HttpResponse { status: 200 })
            .expect("model outcome");
        let index = self.requests.fetch_add(1, Ordering::SeqCst);
        let events = if index == 0 {
            let mut names = vec![BLOCKED, HEALTHY];
            if self.callback {
                names.push(CALLBACK);
            }
            let mut events = names
                .into_iter()
                .map(|name| {
                    Ok(LlmEvent::ToolCallComplete {
                        id: format!("call-{name}"),
                        name: name.into(),
                        args: serde_json::json!({}),
                        meta: None,
                    })
                })
                .collect::<Vec<_>>();
            events.push(done(StopReason::ToolUse));
            events
        } else {
            // Permit the known baseline bug to finish, so completed sibling
            // and native operation assertions run before the decisive RED.
            vec![
                Ok(LlmEvent::TextDelta {
                    delta: "unexpected controller continuation".into(),
                    meta: None,
                }),
                done(StopReason::EndTurn),
            ]
        };
        Box::pin(futures::stream::iter(events))
    }
    fn provider(&self) -> Provider {
        Provider::Anthropic
    }
    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

struct DeferredTools {
    binding: std::sync::OnceLock<(Arc<dyn OpsLifecycleRegistry>, SessionId)>,
    barrier: OperationId,
    detached: OperationId,
    blocked_bodies: AtomicUsize,
    healthy_bodies: AtomicUsize,
    callback_bodies: AtomicUsize,
}
#[async_trait]
impl AgentToolDispatcher for DeferredTools {
    fn capabilities(&self) -> DispatcherCapabilities {
        DispatcherCapabilities {
            ops_lifecycle: true,
        }
    }
    fn bind_ops_lifecycle(
        self: Arc<Self>,
        registry: Arc<dyn OpsLifecycleRegistry>,
        session: SessionId,
    ) -> Result<BindOutcome, OpsLifecycleBindError> {
        assert!(
            self.binding.set((registry, session)).is_ok(),
            "one actual factory registry binding"
        );
        Ok(BindOutcome::Bound(self))
    }
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        [BLOCKED, HEALTHY, CALLBACK]
            .into_iter()
            .map(|name| {
                Arc::new(ToolDef::new(
                    name,
                    "test tool",
                    serde_json::json!({"type":"object"}),
                ))
            })
            .collect()
    }
    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        match call.name {
            BLOCKED => {
                self.blocked_bodies.fetch_add(1, Ordering::SeqCst);
                Ok(ToolResult::new(call.id.into(), "unexpected body".into(), false).into())
            }
            CALLBACK => {
                self.callback_bodies.fetch_add(1, Ordering::SeqCst);
                Err(ToolError::callback_pending(
                    call.name,
                    serde_json::json!({"question":"continue?"}),
                ))
            }
            HEALTHY => {
                self.healthy_bodies.fetch_add(1, Ordering::SeqCst);
                let (registry, owner_session) =
                    self.binding.get().expect("actual factory registry");
                for (id, label) in [(&self.barrier, "barrier"), (&self.detached, "detached")] {
                    registry
                        .register_operation(OperationSpec {
                            id: id.clone(),
                            kind: OperationKind::BackgroundToolOp,
                            owner_session_id: owner_session.clone(),
                            display_name: label.into(),
                            source_label: "observation-test".into(),
                            operation_source: None,
                            child_session_id: None,
                            expect_peer_channel: false,
                        })
                        .expect("real generated registration");
                    registry
                        .provisioning_succeeded(id)
                        .expect("real generated start");
                }
                Ok(ToolDispatchOutcome::new(
                    ToolResult::new(call.id.into(), "healthy returned result".into(), false),
                    vec![
                        meerkat_core::ops::AsyncOpRef::barrier(self.barrier.clone()),
                        meerkat_core::ops::AsyncOpRef::detached(self.detached.clone()),
                    ],
                    vec![meerkat_core::ops::SessionEffect::AppendAssistantBlocks {
                        blocks: vec![AssistantBlock::Text {
                            text: EFFECT.into(),
                            meta: None,
                        }],
                    }],
                ))
            }
            _ => panic!("unexpected fixture tool"),
        }
    }
}

async fn deferred_case(callback: bool) {
    let grants = Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: principal("grant-owner"),
                namespace: id("deferred-grants"),
                generation: 1,
            },
            LocalAuthorizationPublication::new(),
            Arc::new(HostAuthorizationClock),
        )
        .expect("real grant owner"),
    );
    let controller = grants
        .issue_root(
            &principal("grant-owner"),
            id("controller"),
            principal("executor"),
            None,
            ceiling("infer"),
        )
        .expect("controller grant");
    let operation = grants
        .issue_root(
            &principal("grant-owner"),
            id("read"),
            principal("executor"),
            None,
            ceiling("read"),
        )
        .expect("operation grant");
    let provider = Arc::new(DeferredProvider {
        callback,
        requests: AtomicUsize::new(0),
    });
    let tools = Arc::new(DeferredTools {
        binding: std::sync::OnceLock::new(),
        barrier: OperationId::new(),
        detached: OperationId::new(),
        blocked_bodies: AtomicUsize::new(0),
        healthy_bodies: AtomicUsize::new(0),
        callback_bodies: AtomicUsize::new(0),
    });
    let mut build = meerkat::AgentBuildConfig::new(MODEL);
    build.llm_client_override = Some(provider.clone());
    build.tool_dispatcher_override = Some(tools.clone());
    build.session_store_override = Some(Arc::new(RecordingStore::default()));
    let (mut agent, pin) = AgentFactory::minimal()
        .build_agent_with_controller(build, &Config::default())
        .await
        .expect("actual standalone facade build and runnable controller pin");
    let turn = agent
        .turn_state_handle()
        .expect("actual standalone runtime turn owner");
    let registry = tools
        .binding
        .get()
        .expect("actual factory registry binding")
        .0
        .clone();
    let association = association(
        &LogicalRuntimeId::for_session(agent.session().id()),
        controller,
        operation,
        pin.selection().clone(),
    );
    let policy = Arc::new(GrantBackedWorkPolicy::new(
        grants,
        Arc::new(InvocationOwner),
        Arc::new(DeferredResourceOwner),
    ));
    let sink = Arc::new(FailingEntrySink::default());
    let context: WorkAuthorizationContext = policy
        .audited_work_context(
            vec![association].into(),
            OperationExecutionScope::Domain,
            sink.clone(),
        )
        .expect("fixture admitted application work")
        .with_controller_client(pin)
        .expect("exact selected client and policy agree");

    if callback {
        let (tx, _rx) = tokio::sync::mpsc::channel(128);
        let first = tokio::time::timeout(
            Duration::from_secs(20),
            agent.run_with_events_and_work_authorization(
                "run the mixed tool batch".to_string().into(),
                vec![],
                vec![],
                None,
                tx,
                Some(context.clone()),
            ),
        )
        .await
        .expect("callback batch returns without hanging")
        .expect_err("callback remains externally pending");
        assert!(matches!(
            first,
            AgentError::CallbackPending { .. } | AgentError::CallbackBatchPending { .. }
        ));
        assert_eq!(tools.callback_bodies.load(Ordering::SeqCst), 1);
        assert_eq!(provider.requests.load(Ordering::SeqCst), 1);
        assert!(
            agent
                .session()
                .messages()
                .iter()
                .all(|message| !matches!(message, Message::ToolResults { .. })),
            "do not publish a partial tool-result batch"
        );
        let staged = agent
            .session()
            .metadata()
            .get("session_pending_callback_batch_v1")
            .cloned()
            .expect("existing durable callback owner");
        let staged_text = serde_json::to_string(&staged).expect("staged record");
        assert!(staged_text.contains("healthy returned result") && staged_text.contains(EFFECT));
        assert!(
            staged_text.contains(&tools.barrier.to_string())
                && staged_text.contains(&tools.detached.to_string())
        );
        assert_eq!(
            staged["batch"]["deferred_failure"]["kind"],
            "operation_observation_unavailable"
        );
        assert_eq!(
            staged["batch"]["deferred_failure"]["source_run_id"],
            staged["batch"]["run_id"]
        );
        let restored: Session = serde_json::from_value(
            serde_json::to_value(agent.session()).expect("serialize staged owner"),
        )
        .expect("restore staged owner");
        *agent.session_mut() = restored;
        let before_bad = serde_json::to_value(agent.session()).expect("before invalid callback");
        assert!(
            agent
                .apply_pending_callback_tool_results(vec![ToolResult::new(
                    "wrong-call".into(),
                    "wrong".into(),
                    false
                )])
                .is_err()
        );
        assert_eq!(
            serde_json::to_value(agent.session()).expect("after invalid callback"),
            before_bad
        );
        let result = ToolResult::new(
            format!("call-{CALLBACK}"),
            "callback answered".into(),
            false,
        );
        agent
            .apply_pending_callback_tool_results(vec![result.clone()])
            .expect("apply exact callback");
        let after_apply = serde_json::to_value(agent.session()).expect("applied batch");
        let applied = agent
            .session()
            .metadata()
            .get("session_pending_callback_batch_v1")
            .expect("applied receipt");
        assert_eq!(
            applied["deferred_failure"], staged["batch"]["deferred_failure"],
            "the exact failure marker survives Pending to Applied, not just an Agent field"
        );
        agent
            .apply_pending_callback_tool_results(vec![result])
            .expect("exact callback redelivery");
        assert_eq!(
            serde_json::to_value(agent.session()).expect("redelivered batch"),
            after_apply
        );
    }

    let (tx, _rx) = tokio::sync::mpsc::channel(128);
    let outcome = {
        let run = async {
            if callback {
                agent
                    .run_pending_with_events_and_work_authorization(tx, Some(context.clone()))
                    .await
            } else {
                agent
                    .run_with_events_and_work_authorization(
                        "run the mixed tool batch".to_string().into(),
                        vec![],
                        vec![],
                        None,
                        tx,
                        Some(context.clone()),
                    )
                    .await
            }
        };
        tokio::pin!(run);
        // Poll the actual Agent until it has both entered the generated
        // WaitingForOps phase and returned Pending. On this path its first
        // suspension after entering that phase is the real registry wait_all.
        // No timing-only signal or fabricated WaitAllSatisfied is involved.
        tokio::time::timeout(
            Duration::from_secs(20),
            futures::future::poll_fn(|cx| {
                if let Poll::Ready(result) = run.as_mut().poll(cx) {
                    panic!(
                        "collector returned before real barrier ownership could settle: {result:?}"
                    );
                }
                if turn.snapshot().turn_phase
                    == meerkat_core::turn_execution_authority::TurnPhase::WaitingForOps
                {
                    Poll::Ready(())
                } else {
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
            }),
        )
        .await
        .expect("actual Agent reached and suspended in its barrier wait");
        let state = turn.snapshot();
        assert_eq!(
            state.turn_phase,
            meerkat_core::turn_execution_authority::TurnPhase::WaitingForOps
        );
        assert_eq!(
            state.barrier_operation_ids,
            std::collections::BTreeSet::from([tools.barrier.clone()])
        );
        assert!(
            state
                .pending_op_refs
                .iter()
                .any(|item| item.operation_id == tools.detached)
        );
        assert_eq!(tools.blocked_bodies.load(Ordering::SeqCst), 0);
        assert_eq!(tools.healthy_bodies.load(Ordering::SeqCst), 1);
        assert_eq!(
            registry
                .snapshot(&tools.barrier)
                .expect("barrier snapshot")
                .expect("registered")
                .status,
            OperationStatus::Running
        );
        assert_eq!(
            registry
                .snapshot(&tools.detached)
                .expect("detached snapshot")
                .expect("registered")
                .status,
            OperationStatus::Running
        );
        registry
            .complete_operation(
                &tools.barrier,
                OperationResult {
                    id: tools.barrier.clone(),
                    content: "barrier physical completion".into(),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .expect("only real operation owner may satisfy its barrier");
        tokio::time::timeout(Duration::from_secs(20), &mut run)
            .await
            .expect("settled barrier must release the collector")
    };

    assert_eq!(sink.failed_entries.load(Ordering::SeqCst), 1);
    assert_eq!(tools.blocked_bodies.load(Ordering::SeqCst), 0);
    assert_eq!(tools.healthy_bodies.load(Ordering::SeqCst), 1);
    assert_eq!(
        registry
            .snapshot(&tools.barrier)
            .expect("completed barrier")
            .expect("retained")
            .status,
        OperationStatus::Completed
    );
    assert_eq!(
        registry
            .snapshot(&tools.detached)
            .expect("detached ownership")
            .expect("retained")
            .status,
        OperationStatus::Running,
        "an unrelated infrastructure failure cannot fabricate detached cancellation"
    );
    let results = agent
        .session()
        .messages()
        .iter()
        .filter_map(|message| match message {
            Message::ToolResults { results, .. } => Some(results),
            _ => None,
        })
        .flatten()
        .collect::<Vec<_>>();
    assert_eq!(results.len(), if callback { 3 } else { 2 });
    let healthy = results
        .iter()
        .find(|result| result.tool_use_id == format!("call-{HEALTHY}"))
        .expect("healthy retained result");
    assert_eq!(healthy.text_content(), "healthy returned result");
    assert!(!healthy.is_error);
    if callback {
        assert_eq!(
            results
                .iter()
                .find(|result| result.tool_use_id == format!("call-{CALLBACK}"))
                .expect("callback result")
                .text_content(),
            "callback answered"
        );
    }
    let effects = agent
        .session()
        .messages()
        .iter()
        .filter_map(|message| match message {
            Message::BlockAssistant(message) => Some(message.blocks.iter()),
            _ => None,
        })
        .flatten()
        .filter(|block| matches!(block, AssistantBlock::Text { text, .. } if text == EFFECT))
        .count();
    assert_eq!(
        results
            .iter()
            .map(|result| result.tool_use_id.as_str())
            .collect::<Vec<_>>(),
        if callback {
            vec!["call-audit_blocked", "call-healthy", "call-ask_user"]
        } else {
            vec!["call-audit_blocked", "call-healthy"]
        }
    );
    assert!(
        !sink
            .observations
            .lock()
            .expect("audit records")
            .iter()
            .any(|event| matches!(event.observation, AuditObservation::Refused { .. })),
        "infrastructure failure cannot fabricate a policy refusal observation"
    );
    assert_eq!(
        effects, 1,
        "healthy session effect commits exactly once across callback resume/barrier"
    );
    assert_eq!(
        provider.requests.load(Ordering::SeqCst),
        1,
        "retained infrastructure failure prevents the next model request"
    );
    let error =
        outcome.expect_err("deferred infrastructure is an engine error after sibling settlement");
    assert!(error.operation_refusal().is_none());
    assert!(matches!(error, AgentError::Llm {
        reason: meerkat_core::error::LlmFailureReason::ProviderError(ref provider), ..
    } if provider.kind == meerkat_core::error::LlmProviderErrorKind::OperationObservationUnavailable));
    let failed = results
        .iter()
        .find(|result| result.tool_use_id == format!("call-{BLOCKED}"))
        .expect("matched failed call");
    assert!(failed.is_error);
    assert_eq!(
        failed.text_content(),
        ToolError::OperationObservationUnavailable.to_transcript_content()
    );
    if callback {
        assert!(
            agent.session().metadata()["session_pending_callback_batch_v1"]
                .get("deferred_failure")
                .is_none(),
            "only the exact terminalized receipt clears its pending failure"
        );
    }

    let (tx, _rx) = tokio::sync::mpsc::channel(128);
    let independent = tokio::time::timeout(
        Duration::from_secs(20),
        agent.run_with_events_and_work_authorization(
            "new independent work".to_string().into(),
            vec![],
            vec![],
            None,
            tx,
            Some(context),
        ),
    )
    .await
    .expect("fresh work remains live")
    .expect("old failure cannot attach to independent work");
    assert!(!independent.text.is_empty());
    assert_eq!(
        provider.requests.load(Ordering::SeqCst),
        2,
        "only explicit fresh work adds a model request"
    );
    assert_eq!(tools.blocked_bodies.load(Ordering::SeqCst), 0);
    assert_eq!(tools.healthy_bodies.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn observation_infrastructure_callback_resume_retains_siblings_and_prevents_model() {
    deferred_case(true).await;
}

#[tokio::test]
async fn observation_infrastructure_barrier_settlement_retains_siblings_and_prevents_model() {
    deferred_case(false).await;
}

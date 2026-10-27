//! Real installed continuation reconciliation against a native machine's input
//! ledger. The existing governed fixture scripts model transport; a plan-time
//! probe captures its actual staged context and refuses before tool execution.
use super::*;
use crate::private_originals::ContinuationFixture;
use futures::{FutureExt, StreamExt};
use meerkat::{
    ContinuationHostBindings, DetachedJobService, MemoryDetachedJobStore,
    RuntimeDeliveryOwner, RuntimeDeliveryPass,
};
use meerkat::surface::SessionServiceDeliveryHost;
use meerkat_core::{
    AgentToolDispatcher, ControllerModelSelection, Message, Provider, SessionId, StopReason,
    ToolCallView, ToolDef, ToolDispatchContext,
};
use meerkat_llm_core::{
    LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest, LlmStream,
    PlainModelRoute, PreparedLlmRequest,
};
use meerkat_mob_mcp::HostJobSource;
use meerkat_runtime::{
    InMemoryRuntimeStore, RuntimeDeliveryInbox, SessionServiceRuntimeExt,
};
use meerkat_toolkit::{
    authority::{Action, ActionPermission, Ceiling, SetupProposalConstraints,
        SetupRequestEvidence, SetupUseContext},
    request::{ContextReferenceOwner, FindAndInstallRequest, HostSetupRequest,
        RequestError, SetupLimits, validate_request},
    setup::{Resolution, SetupAccess},
};
use std::{panic::AssertUnwindSafe, sync::{Mutex, atomic::{AtomicBool, Ordering}}, time::Duration};

const EVENT_GUARD: Duration = Duration::from_secs(10);
const PROBE: &str = "continuation_context_probe";
fn id(value: &str) -> Id { Id::new(value).unwrap() }
fn revision(value: &str) -> Revision { Revision::new(value).unwrap() }

/// Uses the existing native pre-execution resolver, which receives the actual
/// staged context. It grants no operation and never enters a tool body.
#[derive(Default)]
pub(crate) struct ContextProbe {
    captured: Mutex<Option<ToolDispatchContext>>,
    exact_catalog: bool,
    next_input: Arc<AtomicBool>,
}
impl ContextProbe {
    pub(crate) fn with_exact_catalog() -> Self {
        // This probe owns one immutable definition and the default catalog
        // mirrors that complete list. It never supplies a deferred entry.
        Self { captured: Mutex::new(None), exact_catalog: true, next_input: Arc::new(AtomicBool::new(false)) }
    }
    pub(crate) fn definition() -> ToolDef {
        ToolDef::new(PROBE, "Capture the staged continuation fixture context", serde_json::json!({
            "type": "object", "additionalProperties": false,
        }))
    }
    pub(crate) fn captured(&self) -> ToolDispatchContext {
        self.captured.lock().unwrap().clone().expect("native plan resolver captured the staged context")
    }
    /// The owning test has joined the prior turn and retained its actual
    /// context before explicitly requesting a probe for one additional input.
    pub(crate) fn arm_next_input(&self) {
        self.captured.lock().unwrap().take().expect("prior input was captured");
        self.next_input.store(true, Ordering::SeqCst);
    }
}
#[async_trait]
impl AgentToolDispatcher for ContextProbe {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> { Arc::from([Arc::new(Self::definition())]) }
    fn tool_catalog_capabilities(&self) -> meerkat_core::ToolCatalogCapabilities {
        meerkat_core::ToolCatalogCapabilities {
            exact_catalog: self.exact_catalog,
            may_require_catalog_control_plane: false,
        }
    }
    fn resolve_execution_plan(&self, call: ToolCallView<'_>, context: &ToolDispatchContext,
        _: &meerkat_core::ToolExecutionResolutionContext)
        -> Result<meerkat_core::ResolvedToolExecutionPlan, meerkat_core::ToolExecutionResolutionError>
    {
        assert_eq!(call.name, PROBE);
        assert!(context.retained_work().is_some());
        assert!(context.work_authorization().is_some());
        assert!(self.captured.lock().unwrap().replace(context.clone()).is_none(), "probe runs once");
        Err(meerkat_core::ToolExecutionResolutionError::NotFound { tool_name: call.name.into() })
    }
    async fn dispatch(&self, _: ToolCallView<'_>)
        -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError>
    {
        panic!("the context probe must refuse during plan resolution")
    }
}

/// The existing provider still performs its native model authorization and
/// observation. Only its first scripted response is changed to a probe call.
pub(crate) struct ProbeProvider { inner: Arc<dyn LlmClient>, first: Arc<AtomicBool> }
impl ProbeProvider {
    pub(crate) fn new(inner: Arc<dyn LlmClient>) -> Self {
        Self { inner, first: Arc::new(AtomicBool::new(true)) }
    }
    pub(crate) fn with_probe(inner: Arc<dyn LlmClient>, probe: &ContextProbe) -> Self {
        probe.next_input.store(true, Ordering::SeqCst);
        Self { inner, first: probe.next_input.clone() }
    }
}
#[async_trait]
impl LlmClient for ProbeProvider {
    async fn prepare_controller_credential(&self) -> Result<(), meerkat_core::auth::AuthError> {
        self.inner.prepare_controller_credential().await
    }

    fn plain_model_route(&self, model: &str) -> Result<PlainModelRoute, meerkat_core::ControllerFactsUnavailable> {
        self.inner.plain_model_route(model)
    }
    fn controller_model_selection(&self) -> Option<ControllerModelSelection> {
        self.inner.controller_model_selection()
    }
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        self.inner.project_replay_messages(messages)
    }
    fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> { self.inner.stream(request) }
    fn stream_prepared<'a>(&'a self, request: &'a PreparedLlmRequest) -> LlmStream<'a> {
        let stream = self.inner.stream_prepared(request);
        if !self.first.swap(false, Ordering::SeqCst) { return stream; }
        Box::pin(stream.map(|event| event.map(|event| match event {
            LlmEvent::TextDelta { .. } => LlmEvent::ToolCallComplete {
                id: "continuation-probe-call".into(), name: PROBE.into(),
                args: serde_json::json!({}), meta: None,
            },
            LlmEvent::Done { outcome: LlmDoneOutcome::Success { .. } } => LlmEvent::Done {
                outcome: LlmDoneOutcome::success(StopReason::ToolUse),
            },
            other => other,
        })))
    }
    fn provider(&self) -> Provider { self.inner.provider() }
    async fn health_check(&self) -> Result<(), LlmError> { self.inner.health_check().await }
}

struct References(ContextReference);
impl ContextReferenceOwner for References {
    fn verify_access(&self, value: &ContextReference, authority: &SetupRequestEvidence,
        context: &SetupUseContext<'_>) -> Result<(), RequestError>
    {
        authority.check_context(context)?;
        if value == &self.0 { Ok(()) } else { Err(RequestError::ContextUnavailable) }
    }
}

async fn terminal_result<S>(adapter: &InstalledNativeContinuation, fixture: &ContinuationFixture<S>,
    dispatch: &ToolDispatchContext) -> ContinuationObligation {
    terminal_result_with_work(adapter, fixture, dispatch, None).await
}

async fn terminal_result_with_work<S>(adapter: &InstalledNativeContinuation, fixture: &ContinuationFixture<S>,
    dispatch: &ToolDispatchContext, selected: Option<meerkat_core::retained_work::RetainedWorkIdentity>) -> ContinuationObligation {
    let tenant = id(&adapter.source_scope.realm);
    let deployment = id(&adapter.source_scope.owner);
    let receiver = id("agent");
    let policy = revision("test-policy");
    let association = fixture.binding().await.unwrap().unwrap();
    let target = association.candidate().target.clone();
    let authorization = dispatch.work_authorization().unwrap();
    let retained = selected.unwrap_or_else(|| dispatch.retained_work().unwrap().clone());
    assert_eq!(dispatch.origin_session_id(), Some(&fixture.session));
    assert_eq!(dispatch.run_id(), Some(dispatch.retained_work().unwrap().run_id()));
    let meerkat_core::exact_operation::OperationExecutionScope::RuntimeInput {
        owner_session_id, canonical_input_id, ..
    } = authorization.execution_scope() else { panic!("actual staged input context required") };
    assert_eq!(owner_session_id, &fixture.session);
    let now = u64::try_from(adapter.store.now_ms().await.unwrap()).unwrap();
    let evidence = SetupRequestEvidence::new(association, SetupProposalConstraints {
        tenant: tenant.clone(), deployment: deployment.clone(), policy_revision: policy.clone(),
        issued_at_ms: now, expires_at_ms: now + 60_000,
        ceiling: Ceiling {
            actions: [ActionPermission { action: Action::FindAndInstall, resource: receiver.clone() }].into(),
            services: BTreeSet::new(), frontends: BTreeSet::new(), output_audiences: adapter.audience.clone(),
        },
    }).unwrap();
    let context = SetupUseContext {
        tenant: &tenant, deployment: &deployment,
        realm: &evidence.association().candidate().ingress_namespace.realm,
        target: &target, policy_revision: &policy, now_ms: now, authorization,
    };
    let references = References(ContextReference { owner: id("test-detail-owner"),
        id: id("cancelled"), revision: revision("1"), content_digest: ContentDigest::of_bytes(b"cancelled") });
    let request = validate_request(FindAndInstallRequest {
        request: "Find a service for this original task".into(), user_context: vec![], caller_observations: vec![],
    }, HostSetupRequest {
        setup_helper: None,
        target: receiver, correlation: id("original-call"), parent_operation: None,
        continuation: TaskContinuation {
            owner: adapter.namespace.clone(), task: id(&retained.run_id().to_string()), revision: revision(&canonical_input_id.to_string()),
            delivery_key: id("host:installed-regression:original-call"), audience: adapter.audience.clone(),
            native_original: Some(NativeOriginalTask { origin_session_id: fixture.session.clone(), retained_work: retained, member: None }),
        },
        environment_revision: revision("test-environment"),
        limits: SetupLimits { deadline_ms: now + 60_000, max_workers: 1, max_effect_attempts: 2 },
    }, &evidence, &context, &references).unwrap();
    let store = adapter.store().unwrap();
    let admitted = store.admit(&request, &evidence, &context, &references).await.unwrap().snapshot;
    let terminal = store.resolve(&admitted, Resolution::Cancelled(references.0.clone()), None,
        SetupAccess { evidence: &evidence, context: &context, references: &references }).await.unwrap();
    store.committed_continuation(&id(&terminal.state().operation_id)).await.unwrap().unwrap()
}

async fn wait_applied(service: &ContinuationOwnerService, owner: &ContinuationOwner,
    key: &ContinuationKey, passes: &mut tokio::sync::watch::Receiver<RuntimeDeliveryPass>) -> ContinuationStatus
{
    tokio::time::timeout(EVENT_GUARD, async {
        loop {
            let status = service.continuation_status(owner, key).await.unwrap();
            if matches!(status, ContinuationStatus::Applied { .. }) { return status; }
            passes.changed().await.unwrap();
        }
    }).await.expect("native continuation becomes Applied")
}

/// Move only the test's close schedule across the real canonical read. This
/// wrapper returns the exact owner result and supplies no job facts itself.
struct CloseAfterFound {
    source: Arc<InstalledNativeContinuation>,
    close: Mutex<Option<Arc<meerkat_toolkit::host_lifetime::NativeHostLifetime>>>,
}
#[async_trait]
impl RetainedJobSource for CloseAfterFound {
    async fn retained_job(&self, producer: &ContinuationProducer, job_id: &str) -> RetainedJobLookup {
        let result = self.source.retained_job(producer, job_id).await;
        if matches!(&result, RetainedJobLookup::Found(_)) {
            if let Some(lifetime) = self.close.lock().unwrap().take() {
                use meerkat_toolkit::host_lifetime::control::{PrivateHostLifetimeControl, HostCloseReason};
                lifetime.begin_close_activation(HostCloseReason::OperatorDisable).unwrap();
            }
        }
        result
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ResumeCase { Exact, OtherBody, ClosedHost, ClosedBeforeJobRead, ReleasedSource }

async fn applied_case(case: ResumeCase) {
    let fixture = ContinuationFixture::for_continuation().await;
    let mut owner_handle = None;
    let mut owner_passes = None;
    // Separate the continuation exercise from the fixture-construction frame.
    // Keep the same task, machine, source owners and joined cleanup.
    let result = AssertUnwindSafe(Box::pin(async {
        assert_eq!(fixture.inputs().await, Ok(0));
        let admitted = fixture.submit().await;
        fixture.finish(&admitted).await.unwrap();
        assert_eq!(fixture.inputs().await, Ok(1));
        let first_requests = fixture.model_requests();
        let replay = fixture.submit().await;
        fixture.finish(&replay).await.unwrap();
        assert!(matches!(replay, Ok(crate::private_originals::PrivateOriginalSubmission::Admitted {
            deduplicated: true, ..
        })), "the exact original replays before retained resume");
        assert_eq!(fixture.inputs().await, Ok(1));
        assert_eq!(fixture.model_requests(), first_requests);
        let context = fixture.probe.as_ref().unwrap().captured();
        let adapter = InstalledNativeContinuation::new(fixture.store.clone(), fixture.scope.clone(),
            fixture.source.clone(), id("installed-regression"), [id("person")].into()).unwrap();
        let close_after_found = Arc::new(CloseAfterFound {
            source: adapter.clone(), close: Mutex::new(None),
        });
        let job_source: Arc<dyn RetainedJobSource> = if case == ResumeCase::ClosedHost {
            close_after_found.clone()
        } else { adapter.clone() };
        let state = MobMcpState::new_with_runtime_adapter(fixture.service.clone(), Some(fixture.machine.clone()),
            meerkat_mob::MobControlPrincipal::Owner).unwrap()
            .with_host_job_source(HostJobSource::new([adapter.namespace()], job_source).unwrap()).into_shared();
        let inbox = RuntimeDeliveryInbox::new(Arc::new(InMemoryRuntimeStore::new()));
        let bindings = Arc::new(ContinuationHostBindings::default());
        state.bind_continuations(inbox.clone(), &bindings).unwrap();
        let service = state.continuation_service().unwrap();
        adapter.bind_fixture_native(service.clone(), state.clone()).unwrap();
        let obligation = terminal_result(&adapter, &fixture, &context).await;
        let before = adapter.store().unwrap().committed_continuation(obligation.operation())
            .await.unwrap().unwrap().digest().unwrap();
        let job = RetainedJobRecord::from_owner(adapter.as_ref(), adapter.producer(), obligation.operation().as_str())
            .await.unwrap().unwrap();
        let binding = adapter.binding(&obligation).unwrap();
        let owner = InstalledNativeContinuation::owner(binding);
        let mut delivery = adapter.delivery(&obligation).unwrap();
        if case == ResumeCase::OtherBody { delivery.body = "a different result under this exact owner and key".into(); }
        let key = delivery.key.clone();
        let commit_time = u64::try_from(fixture.store.now_ms().await.unwrap()).unwrap();
        let receipt = service.submit_retained_completion(&owner, delivery, &job, commit_time).await.unwrap();
        let expected = adapter.receipt_reference(&receipt).unwrap();
        let before_requests = fixture.model_requests();
        match case {
            ResumeCase::ClosedHost => {
                // Let the actual private owner disclose its committed facts
                // first. Native must still recheck host authority afterward.
                *close_after_found.close.lock().unwrap() = Some(fixture.lifetime().clone());
            }
            ResumeCase::ClosedBeforeJobRead => {
                use meerkat_toolkit::host_lifetime::control::{PrivateHostLifetimeControl, HostCloseReason};
                fixture.lifetime().begin_close_activation(HostCloseReason::OperatorDisable).unwrap();
            }
            ResumeCase::ReleasedSource => fixture.release_source_custody().unwrap(),
            _ => {}
        }
        let jobs = Arc::new(MemoryDetachedJobStore::new());
        let host = SessionServiceDeliveryHost::new(&fixture.service, &fixture.machine,
            DetachedJobService::new(jobs.clone()), None).with_continuation_bindings(bindings.clone());
        let handle = RuntimeDeliveryOwner::new(jobs, inbox)
            .with_source_changes(bindings.subscribe_source_changes())
            .arm(Arc::new(host)).unwrap();
        owner_passes = Some(handle.subscribe_passes());
        owner_handle = Some(handle);
        if matches!(case, ResumeCase::ClosedHost | ResumeCase::ClosedBeforeJobRead | ResumeCase::ReleasedSource) {
            let passes = owner_passes.as_mut().unwrap();
            let pass = tokio::time::timeout(EVENT_GUARD, async {
                loop {
                    let observed = passes.borrow_and_update().clone();
                    if observed.generation > 0 { break observed; }
                    passes.changed().await.unwrap();
                }
            }).await.expect("native owner publishes the attempted resume");
            let status = service.continuation_status(&owner, &key).await.unwrap();
            if matches!(case, ResumeCase::ClosedHost | ResumeCase::ReleasedSource) {
                // An actual source release is NotRetained. The wrapper must
                // preserve native's terminal verdict before looking for a pin.
                assert!(matches!(status, ContinuationStatus::Refused {
                    receipt: ref observed,
                    reason: meerkat_runtime::RuntimeDeliveryRefusalReason::AuthorityDenied, ..
                } if observed == &receipt), "{status:?}; {pass:?}");
                assert_eq!(pass.refused, 1, "{pass:?}");
                if case == ResumeCase::ClosedHost {
                    assert!(close_after_found.close.lock().unwrap().is_none(),
                        "the real canonical Found result must precede this close");
                }
            } else {
                assert!(matches!(status, ContinuationStatus::Pending { admitted: None, .. }),
                    "{status:?}; {pass:?}");
                assert_eq!(pass.refused, 0, "{pass:?}");
                assert!(pass.failures.iter().any(|failure| failure.contains("canonical setup result is unavailable")),
                    "closed private disclosure must remain unavailable, not fabricate missing job facts: {pass:?}");
                assert_eq!(pass.blocked_sessions, vec![fixture.session.clone()], "{pass:?}");
            }
            assert_eq!(pass.applied, 0, "{pass:?}");
            assert_eq!(fixture.inputs().await, Ok(1));
            assert_eq!(fixture.model_requests(), before_requests);
            return;
        }
        let status = wait_applied(&service, &owner, &key, owner_passes.as_mut().unwrap()).await;
        let ContinuationStatus::Applied { receipt: actual, session: session_id, input: input_id } = status
            else { panic!("actual native input admission required") };
        assert_eq!(actual, receipt);
        assert_eq!(session_id, fixture.session);
        // The actual machine owns this row and its admitted retained binding.
        // No fixture constructs StoredInputState or fabricates Applied/count.
        let stored = tokio::time::timeout(EVENT_GUARD, fixture.machine.input_state(&session_id, &input_id))
            .await.unwrap().unwrap().unwrap();
        let resume = stored.state.retained_resume.as_ref().expect("machine retained resume evidence");
        assert_eq!(&resume.identity, &binding.retained_work);
        assert_eq!(fixture.inputs().await, Ok(2));
        // This is the former path's discriminating precondition: key-only
        // status really IS Applied. The actual installed adapter must ask the
        // native owner to compare this canonical obligation's exact content.
        if case == ResumeCase::OtherBody {
            assert!(matches!(adapter.reconcile(&obligation).await, Err(ContinuationError::InvalidReceipt)));
            assert!(matches!(adapter.verify_admission(&obligation, &expected).await, Err(ContinuationError::InvalidReceipt)));
        } else {
            let ContinuationReconciliation::Applied(first) = adapter.reconcile(&obligation).await.unwrap()
                else { panic!("exact Applied result must reconcile") };
            let next = adapter.verify_admission(&obligation, &expected).await.unwrap().unwrap();
            assert!(first.receipt == next.receipt, "exact receipt is preserved");
            assert!(first.durable_input == next.durable_input, "exact durable input is preserved");
            assert_eq!(first.durable_input.id, id(&input_id.to_string()));
            assert_eq!(first.session, id(&binding.origin_session_id.to_string()));
            assert_eq!(first.runtime, id(binding.retained_work.runtime_id()));
            assert_eq!(first.obligation_digest, obligation.digest().unwrap());
            assert!(next.observed_at_ms >= first.observed_at_ms);
        }
        assert_eq!(fixture.inputs().await, Ok(2), "read-only reconciliation never admits again");
        assert_eq!(adapter.store().unwrap().committed_continuation(obligation.operation())
            .await.unwrap().unwrap().digest().unwrap(), before, "observation does not acknowledge or rewrite the setup result");
    })).catch_unwind().await;
    // The native owner's handle owns abort. Observe its worker sender closing
    // before the existing fixture joins shutdown of the same session/machine.
    drop(owner_handle);
    if let Some(mut passes) = owner_passes {
        tokio::time::timeout(EVENT_GUARD, async {
            while passes.changed().await.is_ok() {}
        }).await.expect("native delivery owner drops after abort");
    }
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn installed_reconcile_rejects_other_content_already_applied_under_same_key() {
    Box::pin(applied_case(ResumeCase::OtherBody)).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn installed_reconcile_accepts_exact_applied_content_without_readmitting() {
    Box::pin(applied_case(ResumeCase::Exact)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn installed_retained_resume_rechecks_current_host_before_native_admission() {
    Box::pin(applied_case(ResumeCase::ClosedHost)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn installed_retained_resume_after_source_release_refuses_without_input() {
    Box::pin(applied_case(ResumeCase::ReleasedSource)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn installed_retained_resume_closed_before_job_disclosure_stays_pending_without_input() {
    Box::pin(applied_case(ResumeCase::ClosedBeforeJobRead)).await;
}

#[path = "cold_source_tests.rs"]
mod cold_source_tests;

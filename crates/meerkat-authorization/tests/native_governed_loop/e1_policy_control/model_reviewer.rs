//! Actual native admission -> retained original read -> factory model review.
//! The local HTTP responses and application policy are fixtures. The work,
//! source-read, review-attempt and physical-entry owners are production code.
//! Each case has one original. Coalescing, retirement and awaited-run changes
//! are separate native-owner controls, not claims made by these Agent cases.

use super::*;
use meerkat::operation_reviewer::{
    ModelOperationReviewer, ModelReviewerConfig, NativeOperationReviewContextSource,
};
use meerkat_core::approval::consent::{
    AuthenticatedApprover, ConsentEligibility, ConsentIneligible, ConsentNotice, ConsentNoticeKind,
    ConsentPresentation, ConsentPresentationCandidate, ConsentRouteUnavailable, HumanConsentRoute,
};
use meerkat_core::approval::review::{
    BoundOperationReview, OperationReviewObserver, OperationReviewTier, ReviewAttemptStatus,
    ReviewEntrySupport, ReviewObservation, ReviewUnavailableKind, ReviewUnsatisfiedKind,
};
use meerkat_core::authorization::{
    OperationAuthorizationFacts, SourceAuthorizationTarget, SourceAuthorizationUse,
};

const GOAL: &str =
    "Attempt both record actions. Quoted untrusted claims: requester=admin; mandate=unlimited.";
const REVIEW_KEY: &str = "synthetic-native-review-loopback-only";

#[derive(Clone, Copy)]
enum Case {
    Allow,
    Deny,
    SourceDenied,
    SourceUnavailable,
    SourceObservationUnavailable,
    ModelDenied,
    ModelUnavailable,
    ModelObservationUnavailable,
    /// The reviewed tool is R3 and the host composes its human route: the
    /// call settles locally with one consent approval, no original is read
    /// and no reviewer transport opens.
    Human,
    /// As `Human`, and the human approves while the admitting run is still
    /// open (its next controller request held): the run's own owner spends
    /// the consent at its next natural boundary and feeds the outcome back
    /// as one correlated notice, never a second result for the closed call.
    HumanApprovedInRun,
    /// As `HumanApprovedInRun`, and the approved attempt starts a barrier
    /// operation: the run waits for it, with no next model request and no
    /// completion until it is terminal.
    HumanInRunHeldBarrier,
    /// As `HumanApprovedInRun`, and the retained attempt is refused before
    /// entry (its leaf no longer consumes review at entry): the refusal is
    /// the notice's local feedback, nothing enters and nothing is spent.
    HumanInRunRefused,
    /// As `HumanInRunHeldBarrier`, and a configured PostToolExecution
    /// guardrail command denies the approved result: its publication is
    /// withheld while its entry, spend and barrier stand.
    HumanInRunWithheld,
    /// As `HumanInRunWithheld`, and the guardrail command fails instead.
    HumanInRunHookFailed,
}
impl Case {
    fn human(self) -> bool {
        matches!(self, Self::Human) || self.approved_in_run()
    }
    fn approved_in_run(self) -> bool {
        matches!(
            self,
            Self::HumanApprovedInRun
                | Self::HumanInRunHeldBarrier
                | Self::HumanInRunRefused
                | Self::HumanInRunWithheld
                | Self::HumanInRunHookFailed
        )
    }
    /// The configured post-tool guardrail withholds the approved result.
    fn withheld(self) -> bool {
        matches!(self, Self::HumanInRunWithheld | Self::HumanInRunHookFailed)
    }
    /// The approved in-run attempt starts a barrier operation.
    fn holds_barrier(self) -> bool {
        matches!(self, Self::HumanInRunHeldBarrier) || self.withheld()
    }
    fn source_allowed(self) -> bool {
        matches!(
            self,
            Self::Allow
                | Self::Deny
                | Self::ModelDenied
                | Self::ModelUnavailable
                | Self::ModelObservationUnavailable
        )
    }
    fn model_allowed(self) -> bool {
        matches!(self, Self::Allow | Self::Deny)
    }
    fn model_fault(self) -> bool {
        matches!(
            self,
            Self::ModelDenied | Self::ModelUnavailable | Self::ModelObservationUnavailable
        )
    }
    fn infrastructure_failure(self) -> bool {
        matches!(
            self,
            Self::SourceObservationUnavailable | Self::ModelObservationUnavailable
        )
    }
    fn enters(self) -> bool {
        matches!(self, Self::Allow)
    }
    fn feedback(self) -> Option<Value> {
        match self {
            Self::Allow => None,
            Self::Deny => Some(
                ToolError::ReviewUnsatisfied {
                    kind: ReviewUnsatisfiedKind::Denied,
                    approval_id: None,
                }
                .to_error_payload(),
            ),
            Self::SourceDenied
            | Self::SourceUnavailable
            | Self::ModelDenied
            | Self::ModelUnavailable => Some(
                ToolError::ReviewUnavailable {
                    kind: ReviewUnavailableKind::ReviewerFailed,
                }
                .to_error_payload(),
            ),
            Self::SourceObservationUnavailable | Self::ModelObservationUnavailable => {
                Some(ToolError::OperationObservationUnavailable.to_error_payload())
            }
            // Its feedback names the approval the route was told about.
            Self::Human
            | Self::HumanApprovedInRun
            | Self::HumanInRunHeldBarrier
            | Self::HumanInRunRefused
            | Self::HumanInRunWithheld
            | Self::HumanInRunHookFailed => None,
        }
    }
}

fn review_response(case: Case) -> String {
    let verdict = if matches!(case, Case::Deny) {
        "deny"
    } else {
        "allow"
    };
    sse(vec![
        start_message(),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":json!({"verdict":verdict}).to_string()}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"message_delta","usage":{"output_tokens":4},"delta":{"stop_reason":"end_turn"}}),
        json!({"type":"message_stop"}),
    ])
}

fn input_domain() -> ResourceDomain {
    ResourceDomain {
        authority: principal("native-input-owner"),
        namespace: "originals".into(),
    }
}

/// Installed application policy independently maps the exact native source,
/// reviewed tool, and actual reviewer route. The native work owner still
/// validates the real ledger/run and generated grant for every operation.
struct ReviewPolicy {
    controller: HttpRecordOwner,
    reviewer: ControllerModelSelection,
    reviewer_endpoint: String,
    original: meerkat_core::InputId,
    source_case: Case,
    source_checks: Arc<AtomicUsize>,
    model_probe: Option<Arc<ModelProbe>>,
}

/// Only the source-observation and model-failure cases record preparation calls.
/// Positive and concurrent cases retain their original policy path and counts.
#[derive(Default)]
struct ModelProbe {
    // Passive facts avoid retaining the binding's work owner back into this
    // policy fixture. They are assertions only, never an authorization input.
    source: Mutex<Option<OperationAuthorizationFacts>>,
    models: Mutex<Vec<OperationAuthorizationFacts>>,
}

impl OperationPolicyOwner for ReviewPolicy {
    /// The reviewed fixture record has no separately selected action account.
    fn review_action_account(
        &self,
        _: &InputAuthorityAssociation,
        _: &PreparedAuthorizationBinding,
        _: u64,
    ) -> Result<Option<String>, meerkat_core::OperationAuthorizationError> {
        Ok(None)
    }

    fn authorize_controller_admission(
        &self,
        association: &InputAuthorityAssociation,
        facts: &meerkat_core::ControllerModelFacts,
        now_ms: u64,
    ) -> Result<ControllerAdmissionAllowance, meerkat_core::OperationAuthorizationError> {
        self.controller
            .authorize_controller_admission(association, facts, now_ms)
    }

    fn authorize_operation(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        match &binding.facts().operation {
            AuthorizationOperation::Tool(facts) => {
                let mut allowed =
                    RecordOwner.authorize_operation(association, binding, purpose, now_ms)?;
                allowed.review_tier = if facts.name != "delete_record" {
                    OperationReviewTier::R1
                } else if self.source_case.human() {
                    OperationReviewTier::R3
                } else {
                    OperationReviewTier::R2
                };
                Ok(allowed)
            }
            AuthorizationOperation::Model(facts)
                if facts.usage == ModelAuthorizationUse::ControllerInference =>
            {
                self.controller
                    .authorize_operation(association, binding, purpose, now_ms)
            }
            AuthorizationOperation::Model(facts)
                if purpose == LocalPolicyPurpose::Operation
                    && facts.usage == ModelAuthorizationUse::Inference
                    && self.reviewer.matches_model_facts(facts)
                    && facts.endpoint.as_ref() == self.reviewer_endpoint
                    && facts.wire_model.as_ref() == E1_MODEL
                    && facts.hosted_capabilities.is_empty()
                    && facts.live_channel.is_none() =>
            {
                if let Some(probe) = &self.model_probe {
                    let source = probe.source.lock().unwrap();
                    let source = source
                        .as_ref()
                        .expect("actual original read prepared before reviewer inference");
                    assert_eq!(binding.facts().execution_scope, source.execution_scope);
                    assert_eq!(binding.facts().run_id, source.run_id);
                    assert!(binding.facts().run_id.is_some());
                    assert_eq!(binding.facts().context_revision, source.context_revision);
                    assert_ne!(binding.facts().operation_id, source.operation_id);
                    probe.models.lock().unwrap().push(binding.facts().clone());
                    // Fault only after the actual factory route/credential and
                    // native source coordinates have passed the checks above.
                    // This is preparation failure, not a failed audit-store write.
                    return Err(match self.source_case {
                        Case::ModelDenied => denied().into(),
                        Case::ModelUnavailable => {
                            meerkat_core::OperationAuthorizationError::Unavailable
                        }
                        Case::ModelObservationUnavailable => {
                            meerkat_core::OperationAuthorizationError::ObservationUnavailable(
                                meerkat_core::authorization::OperationObservationError,
                            )
                        }
                        _ => panic!("model probe is installed only for model failure cases"),
                    });
                }
                Ok(LocalPolicyAllowance {
                    operation_values: self.controller.values(),
                    restrictions: ExecutionRestrictions::unrestricted(),
                    expires_at_ms: now_ms + 60_000,
                    review_tier: OperationReviewTier::R1,
                })
            }
            AuthorizationOperation::Source(facts)
                if purpose == LocalPolicyPurpose::Operation
                    && facts.usage == SourceAuthorizationUse::Read =>
            {
                let SourceAuthorizationTarget::RuntimeInput {
                    owner_session_id,
                    runtime_epoch_id,
                    input_id,
                } = &facts.target
                else {
                    return Err(denied().into());
                };
                let OperationExecutionScope::RuntimeInput {
                    owner_session_id: scope_session,
                    runtime_epoch_id: scope_epoch,
                    submitted_input_id,
                    canonical_input_id,
                } = &binding.facts().execution_scope
                else {
                    return Err(denied().into());
                };
                if input_id != &self.original
                    || scope_session != owner_session_id
                    || scope_epoch != runtime_epoch_id
                    || submitted_input_id != input_id
                    || canonical_input_id != input_id
                    || association.candidate().target.logical_runtime
                        != id(&LogicalRuntimeId::for_session(owner_session_id).to_string())
                {
                    return Err(denied().into());
                }
                self.source_checks.fetch_add(1, Ordering::SeqCst);
                if let Some(probe) = &self.model_probe {
                    assert!(
                        probe
                            .source
                            .lock()
                            .unwrap()
                            .replace(binding.facts().clone())
                            .is_none(),
                        "one actual source preparation attempt, not a retry"
                    );
                }
                // Inject at the installed application SourceRead owner, after
                // checking the actual native original and destination scope.
                // This is not a native audit-buffer or SQLite failure hook.
                match self.source_case {
                    Case::SourceDenied => return Err(denied().into()),
                    Case::SourceUnavailable => {
                        return Err(meerkat_core::OperationAuthorizationError::Unavailable);
                    }
                    Case::SourceObservationUnavailable => {
                        return Err(
                            meerkat_core::OperationAuthorizationError::ObservationUnavailable(
                                meerkat_core::authorization::OperationObservationError,
                            ),
                        );
                    }
                    Case::Allow
                    | Case::Deny
                    | Case::ModelDenied
                    | Case::ModelUnavailable
                    | Case::ModelObservationUnavailable
                    | Case::Human
                    | Case::HumanApprovedInRun
                    | Case::HumanInRunHeldBarrier
                    | Case::HumanInRunRefused
                    | Case::HumanInRunWithheld
                    | Case::HumanInRunHookFailed => {}
                }
                Ok(LocalPolicyAllowance {
                    operation_values: vec![LocalOperationValues {
                        action: action("read"),
                        resource_domain: input_domain(),
                        processor: ProcessorRef::Principal {
                            principal: principal("executor"),
                        },
                        audience: AudienceRef::Principal {
                            principal: principal("requester"),
                        },
                    }],
                    restrictions: ExecutionRestrictions::unrestricted(),
                    expires_at_ms: now_ms + 60_000,
                    review_tier: OperationReviewTier::R1,
                })
            }
            _ => Err(denied().into()),
        }
    }
}

/// A barrier operation the approved in-run attempt starts in the registry
/// the factory actually binds to the fixture; the test completes it.
struct HeldBarrier {
    binding: std::sync::OnceLock<(
        Arc<dyn meerkat_core::ops_lifecycle::OpsLifecycleRegistry>,
        SessionId,
    )>,
    operation: meerkat_core::ops::OperationId,
}

impl HeldBarrier {
    fn new() -> Self {
        Self {
            binding: std::sync::OnceLock::new(),
            operation: meerkat_core::ops::OperationId::new(),
        }
    }
}

/// The approved delete_record result of the withheld cases: a distinct
/// sentinel the guardrail selects and that must never be published.
const WITHHELD_SENTINEL: &str = "withheld-consent-result-q4";
/// The configured post-tool guardrail's hook id.
const GUARD_HOOK: &str = "consent-result-guard";

/// The recording leaf; with a held barrier its R3 body starts that barrier
/// operation, once its flag is set its R3 leaf no longer consumes review at
/// entry, and with a sentinel its R3 body returns that sentinel.
#[derive(Default)]
struct ReviewedTools(
    RecordingTools,
    Option<Arc<HeldBarrier>>,
    std::sync::atomic::AtomicBool,
    Option<&'static str>,
);
#[async_trait]
impl AgentToolDispatcher for ReviewedTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.0.tools()
    }
    fn capabilities(&self) -> meerkat_core::DispatcherCapabilities {
        meerkat_core::DispatcherCapabilities {
            ops_lifecycle: self.1.is_some(),
        }
    }
    fn bind_ops_lifecycle(
        self: Arc<Self>,
        registry: Arc<dyn meerkat_core::ops_lifecycle::OpsLifecycleRegistry>,
        session: SessionId,
    ) -> Result<meerkat_core::BindOutcome, meerkat_core::agent::OpsLifecycleBindError> {
        if let Some(barrier) = &self.1 {
            assert!(
                barrier.binding.set((registry, session)).is_ok(),
                "one actual factory registry binding"
            );
        }
        Ok(meerkat_core::BindOutcome::Bound(self))
    }
    fn review_entry_support(&self, name: &str) -> ReviewEntrySupport {
        if name == "delete_record" && self.2.load(Ordering::SeqCst) {
            ReviewEntrySupport::Unsupported
        } else {
            ReviewEntrySupport::ConsumesAtEntry
        }
    }
    async fn dispatch(&self, _: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        panic!("native reviewed leaf must retain its dispatch context")
    }
    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        assert!(context.work_authorization().is_some());
        let _entering = context.enter_reviewed_effect(call, None)?;
        // No preparation/await precedes this synchronous body recorder inside
        // RecordingTools::dispatch. No synthetic review token is installed.
        let mut outcome = self.0.dispatch(call).await?;
        if call.name == "delete_record"
            && let Some(sentinel) = self.3
        {
            outcome.result =
                meerkat_core::ToolResult::new(call.id.to_string(), sentinel.to_string(), false);
        }
        if call.name == "delete_record"
            && let Some(barrier) = &self.1
        {
            let (registry, owner) = barrier
                .binding
                .get()
                .expect("the actual factory registry binding");
            registry
                .register_operation(meerkat_core::ops_lifecycle::OperationSpec {
                    id: barrier.operation.clone(),
                    kind: meerkat_core::ops_lifecycle::OperationKind::BackgroundToolOp,
                    owner_session_id: owner.clone(),
                    display_name: "held barrier".into(),
                    source_label: "in-run-consent-test".into(),
                    operation_source: None,
                    child_session_id: None,
                    expect_peer_channel: false,
                })
                .expect("real generated registration");
            registry
                .provisioning_succeeded(&barrier.operation)
                .expect("real generated start");
            outcome
                .async_ops
                .push(meerkat_core::ops::AsyncOpRef::barrier(
                    barrier.operation.clone(),
                ));
        }
        Ok(outcome)
    }
}

/// The host's human route for the R3 case: records the exact operation it
/// was shown and the approvals it was told about. `lead-1` is the eligible
/// approver, published through one local authorization publication.
struct HumanRoute {
    presented: Mutex<Vec<OperationAuthorizationFacts>>,
    notices: Mutex<Vec<(meerkat_core::ApprovalId, ConsentNoticeKind)>>,
    publication: LocalAuthorizationPublication,
}

impl HumanRoute {
    fn new() -> Self {
        Self {
            presented: Mutex::default(),
            notices: Mutex::default(),
            publication: LocalAuthorizationPublication::new(),
        }
    }

    /// The approval of the last `Requested` notice; a later withdrawal
    /// notice names the same approval.
    fn requested(&self) -> meerkat_core::ApprovalId {
        self.notices
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(_, kind)| *kind == ConsentNoticeKind::Requested)
            .map(|(approval_id, _)| approval_id.clone())
            .expect("a requested notice")
    }
}

impl HumanConsentRoute for HumanRoute {
    fn present(
        &self,
        candidate: &ConsentPresentationCandidate<'_>,
    ) -> Result<ConsentPresentation, ConsentRouteUnavailable> {
        self.presented
            .lock()
            .unwrap()
            .push(candidate.facts().clone());
        Ok(ConsentPresentation {
            requester: meerkat_core::ApprovalPrincipalId::new("human:requester").unwrap(),
            risk: meerkat_core::ApprovalRisk::High,
            metadata: meerkat_core::SurfaceMetadata::default(),
        })
    }

    fn eligibility(
        &self,
        approver: &AuthenticatedApprover,
        _approval: &meerkat_core::ApprovalRecord,
    ) -> Result<Arc<dyn ConsentEligibility>, ConsentIneligible> {
        let eligible = approver.principal().id.as_str() == "lead-1";
        meerkat_authorization::publication::PublishedConsentEligibility::observe(
            &self.publication,
            || eligible,
        )
    }

    fn notice(&self, notice: ConsentNotice) {
        self.notices
            .lock()
            .unwrap()
            .push((notice.approval_id, notice.kind));
    }
}

#[derive(Default)]
struct Reviews(Mutex<Vec<ReviewObservation>>);
impl OperationReviewObserver for Reviews {
    fn observe(&self, observation: ReviewObservation) {
        self.0.lock().unwrap().push(observation);
    }
}

fn assert_review_fault_audit(
    case: Case,
    probe: Option<&ModelProbe>,
    audit: &[StoredAuthorizationAuditObservation],
    input: &meerkat_core::InputId,
    reviews: &Reviews,
) {
    let Some(probe) = probe else {
        assert!(!case.model_fault() && !matches!(case, Case::SourceObservationUnavailable));
        return;
    };
    let source = probe.source.lock().unwrap();
    let source = source.as_ref().expect("real native original source");
    let models = probe.models.lock().unwrap();
    if matches!(case, Case::SourceObservationUnavailable) {
        let source_records: Vec<_> = audit
            .iter()
            .filter(|record| record.observation.operation_id == source.operation_id)
            .collect();
        assert!(
            source_records.is_empty(),
            "source observation failure cannot invent records for {:?}: {:?}",
            source.operation_id,
            source_records
                .iter()
                .map(|record| record.observation.safe_projection())
                .collect::<Vec<_>>()
        );
        assert!(
            models.is_empty(),
            "failed source cannot prepare reviewer inference"
        );
        return;
    }
    assert_eq!(
        models.len(),
        1,
        "one exact reviewer-model owner call, no retry or fallback"
    );
    let model = &models[0];
    let starts: Vec<_> = audit
        .iter()
        .filter_map(|record| {
            if let AuditObservation::ReviewAttemptStarted { attempt_ref } =
                &record.observation.observation
            {
                Some((record, attempt_ref))
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        starts.len(),
        1,
        "only the reviewed R2 tool opens an attempt"
    );
    let (started, attempt) = starts[0];
    let source_records: Vec<_> = audit
        .iter()
        .filter(|record| record.observation.operation_id == source.operation_id)
        .collect();
    assert_eq!(
        source_records.len(),
        3,
        "source Prepared, Entry and materialized Outcome survive the later model failure"
    );
    assert_eq!(
        source_records
            .iter()
            .filter(|record| matches!(
                record.observation.observation,
                AuditObservation::Prepared { .. }
            ))
            .count(),
        1
    );
    assert_eq!(
        source_records
            .iter()
            .filter(|record| matches!(
                record.observation.observation,
                AuditObservation::Entry { .. }
            ))
            .count(),
        1
    );
    assert_eq!(
        source_records
            .iter()
            .filter(|record| matches!(
                record.observation.observation,
                AuditObservation::Outcome {
                    outcome: OperationObservedOutcome::SourceReadMaterialized
                }
            ))
            .count(),
        1
    );
    let model_records: Vec<_> = audit
        .iter()
        .filter(|record| record.observation.operation_id == model.operation_id)
        .collect();
    match case {
        Case::ModelDenied => {
            assert_eq!(model_records.len(), 1);
            assert!(matches!(
                model_records[0].observation.observation,
                AuditObservation::Refused {
                    reason: OperationRefusalKind::Denied,
                    ..
                }
            ));
        }
        Case::ModelUnavailable => {
            assert_eq!(model_records.len(), 1);
            assert!(matches!(
                model_records[0].observation.observation,
                AuditObservation::AuthorizationUnavailable { .. }
            ));
        }
        Case::ModelObservationUnavailable => {
            assert!(
                model_records.is_empty(),
                "infrastructure failure cannot invent records for {:?}: {:?}",
                model.operation_id,
                model_records
                    .iter()
                    .map(|record| record.observation.safe_projection())
                    .collect::<Vec<_>>()
            );
        }
        _ => panic!("model probe installed on a non-model-failure case"),
    }
    for (records, role) in [
        (
            &source_records,
            meerkat_authorization_contracts::audit::AuditReviewRole::ContextRead,
        ),
        (
            &model_records,
            meerkat_authorization_contracts::audit::AuditReviewRole::ReviewerInference,
        ),
    ] {
        for record in records {
            assert_eq!(record.contributors.len(), 1);
            assert_eq!(&record.contributors[0].input_id, input);
            assert_eq!(record.contributors[0].requester, principal("requester"));
            assert_eq!(record.observation.execution_scope, source.execution_scope);
            assert_eq!(record.observation.run_id, source.run_id);
            assert_eq!(
                record.observation.context_revision,
                source
                    .context_revision
                    .as_ref()
                    .map(|revision| revision.as_str().to_owned())
            );
            let child = record
                .observation
                .review_attribution
                .as_ref()
                .expect("actual protected child link");
            assert_eq!(
                child.candidate_operation_id,
                started.observation.operation_id
            );
            assert_eq!(&child.attempt_ref, attempt);
            assert_eq!(child.role, role);
        }
    }
    let observations = reviews.0.lock().unwrap();
    assert_eq!(
        observations.len(),
        2,
        "one pending attempt and one terminal observation"
    );
    assert!(
        observations
            .iter()
            .all(|record| record.tool_call_id.as_ref() == DENIED_CALL)
    );
    assert_eq!(observations[0].status, ReviewAttemptStatus::Pending);
    assert!(observations[0].retirement.is_none());
    assert_eq!(observations[0].attempt, observations[1].attempt);
    if case.infrastructure_failure() {
        assert_eq!(observations[1].status, ReviewAttemptStatus::Retired);
        assert_eq!(
            observations[1].retirement,
            Some(meerkat_core::approval::review::ReviewRetirementReason::Abandoned)
        );
    } else {
        assert_eq!(observations[1].status, ReviewAttemptStatus::Unavailable);
        assert!(observations[1].retirement.is_none());
    }
}

async fn exercise_review(controller_server: &Server, review_server: &Server, case: Case) {
    let client = http_client(controller_server);
    let selected = client
        .controller_model_selection()
        .expect("actual controller selection");
    let mut config = Config::default();
    let mut realm =
        meerkat_core::RealmConfigSection::from_inline_api_keys(&[("anthropic", REVIEW_KEY)]);
    realm.backend.get_mut("default_anthropic").unwrap().base_url =
        Some(review_server.base_url.clone());
    config.realm.insert("native_review".into(), realm);
    let reviewer_identity = SessionLlmIdentity {
        model: E1_MODEL.into(),
        provider: Provider::Anthropic,
        self_hosted_server_id: None,
        provider_params: None,
        auth_binding: Some(AuthBindingRef {
            realm: RealmId::parse("native_review").unwrap(),
            binding: BindingId::parse("default_anthropic").unwrap(),
            profile: None,
            origin: BindingOrigin::Configured,
        }),
    };
    let factory = AgentFactory::minimal().without_provider_auth_persistence();
    let reviewer_selection = factory
        .build_llm_client_for_identity(&config, &reviewer_identity)
        .await
        .unwrap()
        .controller_model_selection()
        .expect("actual factory reviewer selection");
    let reviewer = ModelOperationReviewer::build(
        &factory,
        &config,
        ModelReviewerConfig::new(reviewer_identity, 73).unwrap(),
        Arc::new(NativeOperationReviewContextSource),
    )
    .await
    .unwrap();
    let reviews = Arc::new(Reviews::default());
    let approvals = meerkat_core::ApprovalService::new();
    let human = case.human().then(|| Arc::new(HumanRoute::new()));
    let mut composed = BoundOperationReview::new(
        Arc::new(reviewer),
        approvals.clone(),
        Duration::from_secs(5),
    )
    .with_observer(reviews.clone());
    if let Some(route) = &human {
        composed = composed.with_human_consent(route.clone(), Duration::from_hours(4));
    }
    let review = Arc::new(composed);

    let grants = Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: principal("grant-owner"),
                namespace: id("native-review-grants"),
                generation: 1,
            },
            LocalAuthorizationPublication::new(),
            Arc::new(HostAuthorizationClock),
        )
        .unwrap(),
    );
    let controller = grants
        .issue_root(
            &principal("grant-owner"),
            id("controller"),
            principal("executor"),
            None,
            ceiling("infer"),
        )
        .unwrap();
    // Both tool operations are allowed before review. Source and reviewer
    // inference also need explicit ordinary operation grants, independently.
    let mut operations = ceiling("read");
    operations.actions =
        ExactRestriction::exact([action("read"), action("delete"), action("infer")]);
    operations.resource_domains = DomainRestriction::exact([domain(), input_domain()]);
    let operation = grants
        .issue_root(
            &principal("grant-owner"),
            id("reviewable-operations"),
            principal("executor"),
            None,
            operations,
        )
        .unwrap();
    let expected_controller = controller.clone();
    let expected_operation = operation.clone();
    let expected_selection = selected.clone();
    let ingress: Arc<NativeIngressCheck> = Arc::new(move |runtime, _, current, claimed| {
        let candidate = claimed.candidate();
        if current.requester() != &principal("requester")
            || current.ingress_actor() != &principal("ingress")
            || current.realm() != &RealmId::parse("native-loop").unwrap()
            || candidate.logical_executor != principal("executor")
            || candidate.represented_subject.is_some()
            || candidate.target.logical_runtime != id(&runtime.to_string())
            || candidate.controller_grant_lineage != [expected_controller.clone()]
            || candidate.authority_basis
                != (WorkAuthorityBasis::GrantLineage {
                    lineage: vec![expected_operation.clone()],
                })
            || candidate.controller_model.as_ref() != Some(&expected_selection)
            || candidate.controller_ceiling != ceiling("infer")
        {
            return Err(denied().into());
        }
        Ok(())
    });
    let mut prompt = PromptInput::new(GOAL, None);
    let input_id = prompt.header.id.clone();
    let source_checks = Arc::new(AtomicUsize::new(0));
    let model_probe = (case.model_fault() || matches!(case, Case::SourceObservationUnavailable))
        .then(|| Arc::new(ModelProbe::default()));
    let machine = Arc::new(
        MeerkatMachine::ephemeral()
            .with_local_grant_authorization(NativeGrantWorkConfiguration {
                retained_resume: None,
                require_scoped_destination: false,
                grants,
                ingress,
                invocation_owner: Arc::new(InvocationOwner),
                operation_owner: Arc::new(ReviewPolicy {
                    controller: HttpRecordOwner {
                        selection: selected.clone(),
                        endpoint: format!("{}/v1/messages", controller_server.base_url),
                    },
                    reviewer: reviewer_selection.clone(),
                    reviewer_endpoint: format!("{}/v1/messages", review_server.base_url),
                    original: input_id.clone(),
                    source_case: case,
                    source_checks: source_checks.clone(),
                    model_probe: model_probe.clone(),
                }),
                configured_mcp_attachment: None,
            })
            .unwrap(),
    );
    let held_barrier = case.holds_barrier().then(|| Arc::new(HeldBarrier::new()));
    let tools = Arc::new(ReviewedTools(
        RecordingTools::default(),
        held_barrier.clone(),
        std::sync::atomic::AtomicBool::new(false),
        case.withheld().then_some(WITHHELD_SENTINEL),
    ));
    // The withheld cases configure one bounded foreground PostToolExecution
    // guardrail command through the native config path. It records each
    // entry that sees the sentinel, then denies it or fails. Only these cases
    // own a marker, each at its own fresh path, so no other case or
    // concurrently running test can create or remove it.
    let guard_marker = case.withheld().then(|| {
        std::env::temp_dir().join(format!(
            "meerkat-e1-consent-guard-{}-{}.log",
            std::process::id(),
            SessionId::new()
        ))
    });
    if let Some(marker) = &guard_marker {
        let script = r#"payload=$(cat)
case "$payload" in
  *"$3"*)
    printf 'entered\n' >> "$1"
    if [ "$2" = fail ]; then exit 3; fi
    printf '%s' '{"decision":{"decision":"deny","hook_id":"consent-result-guard","reason_code":"policy_violation","message":"consent result withheld"}}'
    ;;
  *) printf '%s' '{}' ;;
esac"#;
        config.hooks.entries.push(meerkat_core::HookEntryConfig {
            id: meerkat_core::HookId::new(GUARD_HOOK),
            point: meerkat_core::HookPoint::PostToolExecution,
            mode: meerkat_core::HookExecutionMode::Foreground,
            capability: meerkat_core::HookCapability::Guardrail,
            timeout_ms: Some(5_000),
            runtime: meerkat_core::HookAdapterConfig::command(
                "/bin/sh",
                vec![
                    "-c".into(),
                    script.into(),
                    GUARD_HOOK.into(),
                    marker.display().to_string(),
                    if matches!(case, Case::HumanInRunHookFailed) {
                        "fail".into()
                    } else {
                        "deny".into()
                    },
                    WITHHELD_SENTINEL.into(),
                ],
                std::collections::HashMap::new(),
            ),
            ..Default::default()
        });
    }
    let sessions = Arc::new(RecordingStore::default());
    let mut builder = FactoryAgentBuilder::new(factory.with_operation_review(review), config);
    builder.default_llm_client = Some(client);
    builder.default_tool_dispatcher = Some(tools.clone());
    builder.default_session_store = Some(sessions.clone());
    let service = Arc::new(EphemeralSessionService::new(builder, 2));
    let created = meerkat::surface::materialize_ephemeral_runtime_session(
        &service,
        &machine,
        CreateSessionRequest {
            model: E1_MODEL.into(),
            prompt: "".into(),
            injected_context: Vec::new(),
            system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(meerkat_core::service::SessionBuildOptions {
                auth_binding: selected.auth_binding().cloned(),
                ..Default::default()
            }),
            labels: None,
        },
        false,
    )
    .await
    .unwrap();
    let session_id = created.session_id;
    // Every check below runs inside the bounded cleanup: a failing assertion
    // or the checks' own deadline still releases the held request and
    // unregisters the native session.
    let checks = async {
        let mut events = service
            .subscribe_session_events(&session_id)
            .await
            .expect("the session's native event stream");
        let actor = service
            .live_session_actor_witness(&session_id)
            .await
            .unwrap();
        let pin = service
            .pin_controller_client_for_actor(&actor)
            .await
            .unwrap();
        assert_eq!(pin.selection(), &selected);
        meerkat_auth_core::save_tokens_and_publish_lifecycle(
            meerkat_core::auth::ProviderAuthPersistence::new(
                Arc::new(meerkat_auth_core::EphemeralTokenStore::new()),
                Arc::new(meerkat_auth_core::InMemoryCoordinator::new()),
            ),
            machine.generated_auth_lease_handle(),
            selected.credential().clone(),
            meerkat_core::auth::PersistedTokens::api_key("synthetic-e1-loopback-only"),
        )
        .await
        .unwrap();
        let runtime = LogicalRuntimeId::for_session(&session_id);
        let claims = association(&runtime, controller, operation, selected.clone());
        prompt.header.authority_association = Some(claims.clone());
        let input = Input::Prompt(prompt);
        let current = NativeIngressContext::from_trusted_ingress(
            &input,
            principal("requester"),
            principal("ingress"),
            RealmId::parse("native-loop").unwrap(),
            super::super::evidence("native-review-authentication"),
        )
        .unwrap()
        .with_controller_client(&input, pin)
        .unwrap();
        let input = input.with_ingress_context(current).unwrap();
        let expected_input = serde_json::to_value(&input).unwrap();
        let (_, completion) = machine
            .accept_input_with_completion(&session_id, input)
            .await
            .unwrap();
        let completion = completion.expect("actual native completion");
        if case.infrastructure_failure() {
            // E2's configured engine contract settles the healthy sibling, then
            // ends this attempt with typed infrastructure failure before another
            // model call. Release an accidental continuation so a taxonomy mutant
            // fails an assertion after draining, rather than hanging at the server.
            controller_server.receiver.finish.notify_one();
            let outcome = tokio::time::timeout(Duration::from_secs(20), completion.wait())
                .await
                .unwrap()
                .unwrap();
            let entries = tools.0.0.lock().unwrap().clone();
            assert_eq!(
                entries
                    .iter()
                    .filter(|name| name.as_str() == "delete_record")
                    .count(),
                0
            );
            assert_eq!(
                entries
                    .iter()
                    .filter(|name| name.as_str() == "read_record")
                    .count(),
                1,
                "the healthy R1 sibling settles exactly once before infrastructure is reported"
            );
            assert_eq!(
                source_checks.load(Ordering::SeqCst),
                1,
                "no source-policy retry or fallback"
            );
            assert!(
                review_server.receiver.bodies.lock().unwrap().is_empty(),
                "failed source or model preparation cannot send reviewer HTTP"
            );
            assert!(
                !reviews
                    .0
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|observation| observation.status == ReviewAttemptStatus::Used)
            );
            // A fatal attempt does not take the successful-run save path. Read
            // the settled transcript from its live session owner after completion.
            let retained =
                tokio::time::timeout(Duration::from_secs(10), service.export_session(&session_id))
                    .await
                    .expect("failed-run transcript export completes")
                    .expect("the same native session retains its failed attempt");
            assert!(retained.messages().iter().any(
                |message| matches!(message, Message::User(user) if user.text_content() == GOAL)
            ));
            assert_eq!(
                tool_feedback(retained.messages(), PERMITTED_CALL)
                    .unwrap()
                    .text_content(),
                "record-7 value"
            );
            let failed = tool_feedback(retained.messages(), DENIED_CALL).unwrap();
            assert!(failed.is_error);
            // The typed carrier preserves the actual source/model owner failure;
            // it cannot become ordinary ReviewerFailed feedback.
            assert_eq!(
                failed.text_content(),
                ToolError::OperationObservationUnavailable.to_transcript_content()
            );
            let CompletionOutcome::AbandonedWithError { error, .. } = outcome else {
                panic!("required observation must retain the exact engine failure: {outcome:?}");
            };
            assert!(error.terminal);
            assert_eq!(error.retryable, Some(false));
            assert!(matches!(
                error.reason,
                Some(meerkat_core::event::AgentErrorReason::LlmProviderError {
                    provider_error_kind:
                        meerkat_core::error::LlmProviderErrorKind::OperationObservationUnavailable,
                    provider_error_retryability:
                        meerkat_core::error::LlmProviderErrorRetryability::NonRetryable,
                    ..
                })
            ));
            assert_eq!(
                controller_server.receiver.bodies.lock().unwrap().len(),
                1,
                "required-audit infrastructure cannot become next-model permission feedback"
            );
            let failed_row = machine
                .input_state(&session_id, &input_id)
                .await
                .unwrap()
                .unwrap();
            let failed_run = failed_row
                .seed
                .last_run_id
                .clone()
                .expect("failed attempt retains its actual run");
            let failed_audit: Vec<StoredAuthorizationAuditObservation> = serde_json::from_value(
                serde_json::to_value(&failed_row).unwrap()["authorization_audit"].clone(),
            )
            .unwrap();
            assert!(failed_audit.iter().any(|record| matches!(
                record.observation.observation,
                AuditObservation::ReviewAttemptStarted { .. }
            )));
            // This prefix also contains other operations. Check the failed source
            // or reviewer inference by its actual operation ID below.
            assert!(!failed_audit.iter().any(|record| matches!(&record.observation.observation,
                AuditObservation::Prepared { target, .. } if matches!(target.as_ref(), AuditTarget::Model(model)
                    if model.usage == meerkat_authorization_contracts::audit::AuditModelUse::Inference))));
            assert_review_fault_audit(
                case,
                model_probe.as_deref(),
                &failed_audit,
                &input_id,
                &reviews,
            );

            // A fresh actual admission on this same session is not a retry of the
            // failed operation. It retains the real controller and original goal;
            // the next deterministic response completes without a reviewed tool.
            const FRESH: &str = "Fresh independent work after the observation failure.";
            let mut fresh_prompt = PromptInput::new(FRESH, None);
            fresh_prompt.header.authority_association = Some(claims);
            let fresh = Input::Prompt(fresh_prompt);
            let fresh_id = fresh.id().clone();
            let actor = service
                .live_session_actor_witness(&session_id)
                .await
                .unwrap();
            let pin = service
                .pin_controller_client_for_actor(&actor)
                .await
                .unwrap();
            assert_eq!(pin.selection(), &selected);
            let ingress = NativeIngressContext::from_trusted_ingress(
                &fresh,
                principal("requester"),
                principal("ingress"),
                RealmId::parse("native-loop").unwrap(),
                super::super::evidence("native-review-fresh-authentication"),
            )
            .unwrap()
            .with_controller_client(&fresh, pin)
            .unwrap();
            let (_, fresh_completion) = machine
                .accept_input_with_completion(
                    &session_id,
                    fresh.with_ingress_context(ingress).unwrap(),
                )
                .await
                .unwrap();
            let fresh_outcome =
                tokio::time::timeout(Duration::from_secs(20), fresh_completion.unwrap().wait())
                    .await
                    .unwrap()
                    .unwrap();
            let CompletionOutcome::Completed(result) = fresh_outcome else {
                panic!(
                    "the prior observation fault cannot poison a fresh native admission: {fresh_outcome:?}"
                );
            };
            assert_eq!(result.session_id, session_id);
            assert_eq!(result.text, FINISHED);
            assert!(result.terminal_cause_kind.is_none());
            let fresh_row = machine
                .input_state(&session_id, &fresh_id)
                .await
                .unwrap()
                .unwrap();
            assert!(
                fresh_row
                    .seed
                    .last_run_id
                    .as_ref()
                    .is_some_and(|run| run != &failed_run)
            );
            let fresh_audit: Vec<StoredAuthorizationAuditObservation> = serde_json::from_value(
                serde_json::to_value(&fresh_row).unwrap()["authorization_audit"].clone(),
            )
            .unwrap();
            assert!(!fresh_audit.is_empty());
            assert!(
                fresh_audit
                    .iter()
                    .all(|record| record.contributors.len() == 1
                        && record.contributors[0].input_id == fresh_id)
            );
            assert!(!fresh_audit.iter().any(
                |record| record.observation.review_attribution.is_some()
                    || matches!(
                        record.observation.observation,
                        AuditObservation::ReviewAttemptStarted { .. }
                    )
            ));
            let bodies = controller_server.receiver.bodies.lock().unwrap().clone();
            assert_eq!(
                bodies.len(),
                2,
                "only explicit fresh work permits the second model call"
            );
            assert!(
                bodies[1]["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|message| {
                        if message["role"] != "user" {
                            return false;
                        }
                        // Retained tool-result messages are not prompt-text candidates.
                        let content = &message["content"];
                        match content {
                            Value::String(text) => text == FRESH,
                            Value::Array(blocks) => {
                                blocks.iter().all(|block| {
                                    block["type"] == "text" && block["text"].is_string()
                                }) && wire_text(content) == FRESH
                            }
                            _ => false,
                        }
                    })
            );
            assert_eq!(source_checks.load(Ordering::SeqCst), 1);
            if let Some(probe) = &model_probe {
                assert_eq!(
                    probe.models.lock().unwrap().len(),
                    usize::from(case.model_fault()),
                    "fresh controller work does not retry reviewer inference"
                );
            }
            assert!(review_server.receiver.bodies.lock().unwrap().is_empty());
            assert_eq!(
                *tools.0.0.lock().unwrap(),
                entries,
                "fresh final-only work cannot repeat either physical tool"
            );
            return;
        }
        tokio::time::timeout(
            Duration::from_secs(20),
            controller_server.receiver.second_request.notified(),
        )
        .await
        .expect("local review feedback and permitted sibling reach the next controller turn");

        let bodies = controller_server.receiver.bodies.lock().unwrap().clone();
        assert_eq!(bodies.len(), 2);
        let first_goal = bodies[0]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["role"] == "user" && wire_text(&message["content"]) == GOAL)
            .unwrap();
        assert!(
            bodies[1]["messages"]
                .as_array()
                .unwrap()
                .contains(first_goal),
            "exact original goal retained in follow-up"
        );
        let results: Vec<_> = bodies[1]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["role"] == "user")
            .filter_map(|message| message["content"].as_array())
            .flatten()
            .filter(|block| block["type"] == "tool_result")
            .collect();
        assert_eq!(results.len(), 2);
        let reviewed = results
            .iter()
            .find(|result| result["tool_use_id"] == DENIED_CALL)
            .unwrap();
        let sibling = results
            .iter()
            .find(|result| result["tool_use_id"] == PERMITTED_CALL)
            .unwrap();
        if let Some(route) = &human {
            // Same-run model -> tool -> model: the R3 call settles locally with
            // its one consent approval and the next controller turn sees it.
            assert_eq!(reviewed["is_error"], true);
            assert_eq!(
                serde_json::from_str::<Value>(&wire_text(&reviewed["content"])).unwrap(),
                ToolError::ReviewUnsatisfied {
                    kind: ReviewUnsatisfiedKind::HumanConsentRequired,
                    approval_id: Some(route.requested()),
                }
                .to_error_payload()
            );
        } else if let Some(feedback) = case.feedback() {
            assert_eq!(reviewed["is_error"], true);
            assert_eq!(
                serde_json::from_str::<Value>(&wire_text(&reviewed["content"])).unwrap(),
                feedback
            );
        } else {
            assert_ne!(reviewed["is_error"], true);
            assert_eq!(wire_text(&reviewed["content"]), "record-7 value");
        }
        assert_ne!(sibling["is_error"], true);
        assert_eq!(wire_text(&sibling["content"]), "record-7 value");
        let entries = tools.0.0.lock().unwrap().clone();
        assert_eq!(
            entries
                .iter()
                .filter(|name| name.as_str() == "delete_record")
                .count(),
            usize::from(case.enters())
        );
        assert_eq!(
            entries
                .iter()
                .filter(|name| name.as_str() == "read_record")
                .count(),
            1
        );

        let reviewer_bodies = review_server.receiver.bodies.lock().unwrap().clone();
        assert_eq!(
            reviewer_bodies.len(),
            usize::from(case.model_allowed()),
            "source and model-owner refusal both precede reviewer transport"
        );
        if let Some(body) = reviewer_bodies.first() {
            assert_eq!(body["model"], E1_MODEL);
            assert_eq!(body["max_tokens"], 73);
            assert!(
                body.get("tools")
                    .is_none_or(|tools| tools.as_array().is_some_and(Vec::is_empty))
            );
            let messages = body["messages"].as_array().unwrap();
            assert_eq!(
                messages.len(),
                1,
                "system instructions are separate from owner context"
            );
            let request: Value = serde_json::from_str(&wire_text(&messages[0]["content"])).unwrap();
            assert_eq!(
                request["proposed_operation"],
                json!({"tool":"delete_record", "call_id":DENIED_CALL, "arguments":{"record":"record-7"}})
            );
            let context: Value =
                serde_json::from_str(request["owner_supplied_context"].as_str().unwrap()).unwrap();
            let originals = context["original_inputs"].as_array().unwrap();
            assert_eq!(
                originals.len(),
                1,
                "every original in this admitted batch, no transcript substitute"
            );
            assert_eq!(
                originals[0]["input_id"],
                serde_json::to_value(&input_id).unwrap()
            );
            assert_eq!(originals[0]["input"], expected_input);
            assert_eq!(
                originals[0]["authenticated_association"],
                serde_json::to_value(&claims).unwrap()
            );
            assert_eq!(claims.candidate().requester, principal("requester"));
            assert_eq!(claims.candidate().ingress_actor, principal("ingress"));
            assert_eq!(claims.candidate().logical_executor, principal("executor"));
        }

        let stored = machine
            .input_state(&session_id, &input_id)
            .await
            .unwrap()
            .unwrap();
        let admitted_run = stored.seed.last_run_id.clone();
        let audit: Vec<StoredAuthorizationAuditObservation> = serde_json::from_value(
            serde_json::to_value(stored).unwrap()["authorization_audit"].clone(),
        )
        .unwrap();
        assert_review_fault_audit(case, model_probe.as_deref(), &audit, &input_id, &reviews);
        if let Some(route) = &human {
            // R3 reads no original and opens no reviewer transport; the human
            // route was shown exactly this call of this native run.
            assert!(
                !audit
                    .iter()
                    .any(|record| matches!(&record.observation.observation,
                    AuditObservation::Prepared { target, .. }
                        if matches!(target.as_ref(), AuditTarget::RuntimeInput { .. }))),
                "the human route reads no original"
            );
            let presented = route.presented.lock().unwrap();
            assert_eq!(presented.len(), 1, "one presentation for the one consent");
            // Exactly the operation the native owner prepared for the R3 call,
            // in the admitted input's own native run and execution scope.
            let prepared: Vec<_> = audit
                .iter()
                .filter(|record| {
                    matches!(&record.observation.observation,
                        AuditObservation::Prepared { target, .. }
                            if matches!(target.as_ref(), AuditTarget::Tool { call_id, tool_name, .. }
                                if call_id == DENIED_CALL && tool_name == "delete_record"))
                })
                .collect();
            assert_eq!(prepared.len(), 1, "one Prepared record of the R3 call");
            let prepared = &prepared[0].observation;
            assert_eq!(
                presented[0].operation_id, prepared.operation_id,
                "the prepared operation"
            );
            assert_eq!(
                presented[0].execution_scope, prepared.execution_scope,
                "the prepared operation's full execution scope"
            );
            assert_eq!(
                presented[0].run_id, prepared.run_id,
                "the prepared operation's run"
            );
            assert!(admitted_run.is_some());
            assert_eq!(
                presented[0].run_id, admitted_run,
                "the admitted input's native run"
            );
            assert_eq!(
                presented[0]
                    .context_revision
                    .as_ref()
                    .map(|revision| revision.as_str()),
                prepared.context_revision.as_deref(),
                "the prepared operation's context revision"
            );
            assert!(matches!(&presented[0].execution_scope,
                OperationExecutionScope::RuntimeInput { canonical_input_id, .. }
                    if canonical_input_id == &input_id));
            assert!(matches!(&presented[0].operation,
                AuthorizationOperation::Tool(tool)
                    if tool.name == "delete_record" && &*tool.call_id == DENIED_CALL));
        } else {
            let source = audit.iter().find(|record| match &record.observation.observation {
                AuditObservation::Prepared { target, .. } | AuditObservation::Refused { target, .. }
                    | AuditObservation::AuthorizationUnavailable { target } => matches!(target.as_ref(),
                    AuditTarget::RuntimeInput { owner_session_id, input_id: actual, .. } if owner_session_id == &session_id && actual == &input_id),
                _ => false,
            }).expect("actual native SourceRead has protected attribution");
            if case.source_allowed() {
                assert!(audit.iter().any(|record| record.observation.operation_id
                    == source.observation.operation_id
                    && matches!(
                        record.observation.observation,
                        AuditObservation::Outcome {
                            outcome: OperationObservedOutcome::SourceReadMaterialized
                        }
                    )));
            } else {
                if matches!(case, Case::SourceUnavailable) {
                    assert!(matches!(
                        source.observation.observation,
                        AuditObservation::AuthorizationUnavailable { .. }
                    ));
                    assert!(
                        !audit.iter().any(|record| record.observation.operation_id
                            == source.observation.operation_id
                            && matches!(
                                record.observation.observation,
                                AuditObservation::Refused { .. }
                            )),
                        "this unavailable source operation cannot also be recorded as a policy refusal"
                    );
                } else {
                    assert!(matches!(
                        source.observation.observation,
                        AuditObservation::Refused {
                            reason: OperationRefusalKind::Denied,
                            ..
                        }
                    ));
                }
                assert!(!audit.iter().any(|record| record.observation.operation_id
                    == source.observation.operation_id
                    && matches!(
                        record.observation.observation,
                        AuditObservation::Entry { .. } | AuditObservation::Outcome { .. }
                    )));
            }
            let reviewer_prepares: Vec<_> = audit.iter().filter(|record| matches!(&record.observation.observation,
                AuditObservation::Prepared { target, .. } if matches!(target.as_ref(), AuditTarget::Model(model)
                    if model.usage == meerkat_authorization_contracts::audit::AuditModelUse::Inference
                        && model.endpoint == format!("{}/v1/messages", review_server.base_url)
                        && model.credential.as_ref() == Some(reviewer_selection.credential())))).collect();
            assert_eq!(reviewer_prepares.len(), usize::from(case.model_allowed()));
            for review_model in reviewer_prepares {
                assert_ne!(
                    review_model.observation.operation_id,
                    source.observation.operation_id
                );
                for record in audit.iter().filter(|record| {
                    record.observation.operation_id == review_model.observation.operation_id
                }) {
                    assert_eq!(record.contributors.len(), 1);
                    assert_eq!(record.contributors[0].input_id, input_id);
                    assert_eq!(record.contributors[0].requester, principal("requester"));
                    assert_eq!(
                        record.observation.execution_scope,
                        source.observation.execution_scope
                    );
                    assert_eq!(record.observation.run_id, source.observation.run_id);
                }
                assert!(audit.iter().any(|record| record.observation.operation_id
                    == review_model.observation.operation_id
                    && matches!(
                        record.observation.observation,
                        AuditObservation::Outcome {
                            outcome: OperationObservedOutcome::HttpResponse { status: 200 }
                        }
                    )));
            }
        }
        let observations = reviews.0.lock().unwrap().clone();
        assert!(
            !observations.is_empty(),
            "native R2 actually reached the installed reviewer"
        );
        assert!(
            observations
                .iter()
                .all(|observation| observation.tool_call_id.as_ref() == DENIED_CALL),
            "R1 sibling never reads context or opens a review attempt"
        );
        assert_eq!(
            observations
                .iter()
                .filter(|observation| observation.status == ReviewAttemptStatus::Used)
                .count(),
            usize::from(case.enters())
        );

        if let Some(route) = &human
            && case.approved_in_run()
        {
            // The human approves while the admitting run is still open, its next
            // controller request held. The decision alone executes nothing.
            approvals
                .decide_consent(&route.requested(), approve_as("lead-1"))
                .expect("the authenticated approval commits");
            if matches!(case, Case::HumanInRunRefused) {
                // The retained attempt will be refused before entry.
                tools.2.store(true, Ordering::SeqCst);
            }
            assert_eq!(
                tools
                    .0
                    .0
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|name| name.as_str() == "delete_record")
                    .count(),
                0,
                "a decision alone enters nothing"
            );
        }
        controller_server.receiver.finish.notify_one();
        let mut seen_events: Vec<meerkat_core::AgentEvent> = Vec::new();
        let wait = completion.wait();
        tokio::pin!(wait);
        if let Some(barrier) = &held_barrier {
            // The approved attempt enters and starts a barrier operation. The run
            // waits for it: no next model request and no completion until the
            // operation is terminal.
            tokio::time::timeout(Duration::from_secs(20), async {
                loop {
                    let entered = tools
                        .0
                        .0
                        .lock()
                        .unwrap()
                        .iter()
                        .any(|name| name.as_str() == "delete_record");
                    if entered {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .expect("the approved attempt enters");
            if case.withheld() {
                // The guardrail's own settlement first, so the pending
                // completion below shows the barrier, not a slow hook.
                tokio::time::timeout(Duration::from_secs(20), async {
                    while let Some(envelope) = futures::StreamExt::next(&mut events).await {
                        let settled = guard_settled(case, &envelope.payload);
                        seen_events.push(envelope.payload);
                        if settled {
                            break;
                        }
                    }
                })
                .await
                .expect("the guardrail settles the approved result");
                assert!(
                    seen_events.iter().any(|event| guard_settled(case, event)),
                    "the guardrail's own Deny or ExecutionFailed precedes the barrier checks"
                );
            }
            assert!(
                tokio::time::timeout(Duration::from_millis(500), &mut wait)
                    .await
                    .is_err(),
                "the run cannot complete before its barrier operation is terminal"
            );
            assert_eq!(
                controller_server.receiver.bodies.lock().unwrap().len(),
                2,
                "no next model request before the barrier operation is terminal"
            );
            barrier
                .binding
                .get()
                .expect("the actual factory registry binding")
                .0
                .complete_operation(
                    &barrier.operation,
                    meerkat_core::ops_lifecycle::OperationResult {
                        id: barrier.operation.clone(),
                        content: "held barrier completion".into(),
                        is_error: false,
                        duration_ms: 1,
                        tokens_used: 0,
                    },
                )
                .expect("the barrier's owner completes it");
        }
        let outcome = tokio::time::timeout(Duration::from_secs(20), wait)
            .await
            .unwrap()
            .unwrap();
        let CompletionOutcome::Completed(result) = outcome else {
            panic!("review settles locally in the original run: {outcome:?}")
        };
        if let Some(route) = &human
            && case.approved_in_run()
        {
            let approval_id = route.requested();
            let refused = matches!(case, Case::HumanInRunRefused);
            let notices = route.notices.lock().unwrap().clone();
            let consent = approvals
                .get(&approval_id)
                .unwrap()
                .consent
                .expect("consent history");
            if refused {
                // Refused before entry: decided, never used; nothing is spent.
                assert_eq!(
                    notices[..2],
                    [
                        (approval_id.clone(), ConsentNoticeKind::Requested),
                        (approval_id.clone(), ConsentNoticeKind::Approved),
                    ],
                    "decided before the refused attempt"
                );
                assert!(
                    !notices
                        .iter()
                        .any(|(_, kind)| *kind == ConsentNoticeKind::Used),
                    "a refused attempt spends nothing: {notices:?}"
                );
                assert!(consent.consumed_at.is_none(), "never spent");
            } else {
                // The run's own owner spent the approved consent at its next
                // natural boundary, through the production runner: decided, then
                // used once, never withdrawn.
                assert_eq!(
                    notices,
                    vec![
                        (approval_id.clone(), ConsentNoticeKind::Requested),
                        (approval_id.clone(), ConsentNoticeKind::Approved),
                        (approval_id.clone(), ConsentNoticeKind::Used),
                    ],
                    "exactly Requested, Approved, then Used"
                );
                assert_eq!(
                    approvals.get(&approval_id).unwrap().status,
                    meerkat_core::ApprovalStatus::Approved
                );
                assert!(consent.consumed_at.is_some(), "spent by the open run");
                assert!(consent.withdrawal.is_none(), "never withdrawn");
            }
            let entries = tools.0.0.lock().unwrap().clone();
            assert_eq!(
                entries
                    .iter()
                    .filter(|name| name.as_str() == "delete_record")
                    .count(),
                usize::from(!refused),
                "the approved call entered exactly once, the refused one never"
            );
            // The original call keeps its one native completion and result
            // event; the in-run attempt emits neither again.
            seen_events.extend(drain_events(&mut events).await);
            let events = std::mem::take(&mut seen_events);
            for (kind, count) in [
                (
                    "ToolExecutionCompleted",
                    events
                        .iter()
                        .filter(|event| matches!(event, meerkat_core::AgentEvent::ToolExecutionCompleted { id, .. } if id == DENIED_CALL))
                        .count(),
                ),
                (
                    "ToolResultReceived",
                    events
                        .iter()
                        .filter(|event| matches!(event, meerkat_core::AgentEvent::ToolResultReceived { id, .. } if id == DENIED_CALL))
                        .count(),
                ),
            ] {
                assert_eq!(count, 1, "one native {kind} for the closed call");
            }
            if case.withheld() {
                // The configured guardrail command actually ran for the
                // approved result, once, and that exact hook denied or failed.
                assert_eq!(
                    guard_marker
                        .as_ref()
                        .and_then(|marker| std::fs::read_to_string(marker).ok())
                        .unwrap_or_default(),
                    "entered\n",
                    "the guardrail command entered once for the approved result"
                );
                let decided = events
                    .iter()
                    .filter(|event| guard_settled(case, event))
                    .count();
                assert_eq!(
                    decided, 1,
                    "the guardrail's own Deny or ExecutionFailed, once"
                );
                // The withheld result never reaches a later request.
                for body in controller_server.receiver.bodies.lock().unwrap().iter() {
                    assert!(
                        !body.to_string().contains(WITHHELD_SENTINEL),
                        "a withheld result reached a model request"
                    );
                }
            }
            let bodies = controller_server.receiver.bodies.lock().unwrap().clone();
            let third = bodies[2]["messages"].as_array().unwrap();
            assert_eq!(
                third
                    .iter()
                    .filter_map(|message| message["content"].as_array())
                    .flatten()
                    .filter(|block| block["type"] == "tool_result"
                        && block["tool_use_id"] == DENIED_CALL)
                    .count(),
                1,
                "the closed call keeps its one local result"
            );
            // The notice text (naming the closed call), then the published
            // result as the next part, in order.
            let texts: Vec<String> = third
                .iter()
                .flat_map(|message| match &message["content"] {
                    Value::String(text) => vec![text.clone()],
                    Value::Array(blocks) => blocks
                        .iter()
                        .filter(|block| block["type"] == "text")
                        .filter_map(|block| block["text"].as_str().map(str::to_owned))
                        .collect(),
                    _ => Vec::new(),
                })
                .collect();
            let at: Vec<usize> = texts
                .iter()
                .enumerate()
                .filter(|(_, text)| text.contains("human-approved delete_record call"))
                .map(|(index, _)| index)
                .collect();
            assert_eq!(at.len(), 1, "one correlated outcome: {texts:?}");
            assert!(
                texts[at[0]].contains(DENIED_CALL),
                "the notice names the closed call: {texts:?}"
            );
            assert_eq!(
                texts.get(at[0] + 1).cloned(),
                Some(in_run_outcome_text(case)),
                "the published result (or refusal) follows the notice text: {texts:?}"
            );
        } else if let Some(route) = &human {
            // The run that admitted the candidate closed: the owner published
            // the withdrawal of its unspent consent before anything reads or
            // reconciles the approval, and only then is it read as Cancelled.
            let approval_id = route.requested();
            assert_eq!(
                *route.notices.lock().unwrap(),
                vec![
                    (approval_id.clone(), ConsentNoticeKind::Requested),
                    (approval_id.clone(), ConsentNoticeKind::Withdrawn),
                ],
                "exactly Requested then Withdrawn for the captured consent"
            );
            let record = approvals.get(&approval_id).unwrap();
            assert_eq!(record.status, meerkat_core::ApprovalStatus::Cancelled);
            assert_eq!(
                record
                    .consent
                    .and_then(|consent| consent.withdrawal)
                    .map(|withdrawal| withdrawal.reason),
                Some(meerkat_core::approval::ApprovalConsentWithdrawalReason::RunEnded),
                "withdrawn, never denied, and carried to no new run"
            );
        }
        assert_eq!(result.text, FINISHED);
        assert_eq!(result.session_id, session_id);
        assert!(result.terminal_cause_kind.is_none());
        let saved = sessions
            .0
            .lock()
            .unwrap()
            .get(&session_id)
            .cloned()
            .unwrap();
        assert!(
            saved.messages().iter().any(
                |message| matches!(message, Message::User(user) if user.text_content() == GOAL)
            )
        );
        assert_eq!(
            tool_feedback(saved.messages(), PERMITTED_CALL)
                .unwrap()
                .text_content(),
            "record-7 value"
        );
        assert_eq!(
            tool_feedback(saved.messages(), DENIED_CALL)
                .unwrap()
                .is_error,
            !case.enters()
        );
        assert_eq!(
            controller_server.receiver.bodies.lock().unwrap().len(),
            if case.approved_in_run() { 3 } else { 2 }
        );
        if let Some(route) = &human
            && case.approved_in_run()
        {
            let payloads: Vec<(Value, Vec<meerkat_core::ContentBlock>)> = saved
                .messages()
                .iter()
                .filter_map(|message| match message {
                    Message::SystemNotice(notice) => {
                        notice.blocks.iter().find_map(|block| match block {
                            meerkat_core::SystemNoticeBlock::RuntimeNotice {
                                category,
                                payload,
                                content,
                                ..
                            } if category == "consent_attempt" => {
                                payload.clone().map(|payload| (payload, content.clone()))
                            }
                            _ => None,
                        })
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(
                payloads,
                vec![(
                    json!({
                        "approval_id": route.requested(),
                        "tool_use_id": DENIED_CALL,
                        "tool_name": "delete_record",
                        "is_error": matches!(case, Case::HumanInRunRefused) || case.withheld(),
                    }),
                    vec![meerkat_core::ContentBlock::Text {
                        text: in_run_outcome_text(case),
                    }],
                )],
                "one typed, correlated runtime notice with the published result"
            );
            if case.withheld() {
                assert!(
                    !serde_json::to_string(saved.messages())
                        .unwrap()
                        .contains(WITHHELD_SENTINEL),
                    "a withheld result never reaches the persisted transcript"
                );
            }
            // The scripted extra read the held request asked for ran, after the
            // decision, as an ordinary permitted call.
            let read =
                tool_feedback(saved.messages(), IN_RUN_READ_CALL).expect("the scripted read");
            assert!(!read.is_error);
            assert_eq!(read.text_content(), "record-7 value");
        }
        assert_eq!(
            review_server.receiver.bodies.lock().unwrap().len(),
            usize::from(case.model_allowed())
        );
        assert_eq!(
            source_checks.load(Ordering::SeqCst),
            usize::from(human.is_none()),
            "one native original read for model review, with no policy retry"
        );
    };
    let checks = std::panic::AssertUnwindSafe(tokio::time::timeout(Duration::from_mins(1), checks))
        .catch_unwind()
        .await;
    // Bounded native cleanup, whether or not the checks passed or finished:
    // release a held request, then unregister the native session.
    controller_server.receiver.finish.notify_one();
    let cleanup = tokio::time::timeout(
        Duration::from_secs(10),
        machine.unregister_current_session_registration_until_terminal(&session_id),
    )
    .await;
    if let Some(marker) = &guard_marker {
        let _ = std::fs::remove_file(marker);
    }
    match checks {
        Ok(Ok(())) => {}
        Ok(Err(elapsed)) => panic!("native reviewer case timed out: {elapsed}"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
    cleanup.unwrap().unwrap();
}

/// The in-run attempt's model-facing outcome text in `case`: the published
/// result, the local refusal of an attempt refused before entry, or the
/// withheld publication's refusal.
fn in_run_outcome_text(case: Case) -> String {
    match case {
        Case::HumanInRunRefused => ToolError::ReviewUnavailable {
            kind: ReviewUnavailableKind::UnsupportedEntry,
        }
        .to_transcript_content(),
        Case::HumanInRunWithheld => ToolError::HookDenied {
            denial: Box::new(meerkat_core::HookDenial {
                hook_id: meerkat_core::HookId::new(GUARD_HOOK),
                point: meerkat_core::HookPoint::PostToolExecution,
                reason_code: meerkat_core::HookReasonCode::PolicyViolation,
                message: "Tool result publication was withheld; this does not undo any entered tool execution."
                    .to_string(),
                payload: None,
            }),
        }
        .to_transcript_content(),
        Case::HumanInRunHookFailed => ToolError::Other(
            "Tool result publication was withheld because a required post-tool hook failed; this does not undo any entered tool execution."
                .to_string(),
        )
        .to_transcript_content(),
        _ => "record-7 value".to_string(),
    }
}

/// The configured guardrail's own settlement of the approved result: its
/// Deny in the withheld case, its ExecutionFailed in the failed one.
fn guard_settled(case: Case, event: &meerkat_core::AgentEvent) -> bool {
    let guard = meerkat_core::HookId::new(GUARD_HOOK);
    match event {
        meerkat_core::AgentEvent::HookDenied { hook_id, point, .. } => {
            matches!(case, Case::HumanInRunWithheld)
                && *hook_id == guard
                && *point == meerkat_core::HookPoint::PostToolExecution
        }
        meerkat_core::AgentEvent::HookFailed {
            hook_id,
            point,
            reason,
        } => {
            matches!(case, Case::HumanInRunHookFailed)
                && *hook_id == guard
                && *point == meerkat_core::HookPoint::PostToolExecution
                && matches!(
                    reason,
                    meerkat_core::HookFailureReason::ExecutionFailed { .. }
                )
        }
        _ => false,
    }
}

/// Every native event the session published so far.
async fn drain_events(
    events: &mut meerkat_core::comms::EventStream,
) -> Vec<meerkat_core::AgentEvent> {
    let mut drained = Vec::new();
    while let Ok(Some(envelope)) =
        tokio::time::timeout(Duration::from_millis(300), futures::StreamExt::next(events)).await
    {
        drained.push(envelope.payload);
    }
    drained
}

/// One more R1 read the held second request asks for, so the run reaches
/// one more natural boundary before its third request.
const IN_RUN_READ_CALL: &str = "attempt-read-again";

fn in_run_read_response() -> String {
    sse(vec![
        start_message(),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":IN_RUN_READ_CALL,"name":"read_record","input":{}}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"record\":\"record-7\"}"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"message_delta","usage":{"output_tokens":2},"delta":{"stop_reason":"tool_use"}}),
        json!({"type":"message_stop"}),
    ])
}

/// An authenticated approval by `id`, as the host's ingress adapter makes it.
fn approve_as(id: &str) -> meerkat_core::approval::consent::AuthenticatedConsentDecision {
    meerkat_core::approval::consent::AuthenticatedConsentDecision {
        approver: AuthenticatedApprover::new(
            meerkat_core::PrincipalRef::in_domain(
                meerkat_core::PrincipalKind::Human,
                id,
                meerkat_core::TrustDomainId::new("team").unwrap(),
            )
            .unwrap(),
            meerkat_core::ApprovalAssurance::PersonAuthenticated,
        )
        .unwrap(),
        decision: meerkat_core::ApprovalDecision::Approve,
        reason: None,
    }
}

async fn run_case(case: Case) {
    let mut controller = if case.approved_in_run() {
        Server::start_with_receiver(Receiver {
            first_responses: vec![sibling_response(), final_response()],
            held_tool_response: Some(in_run_read_response()),
            model: Some(E1_MODEL),
            ..Receiver::default()
        })
        .await
    } else {
        Server::start().await
    };
    let mut reviewer = Server::start_with_receiver(Receiver {
        first_responses: vec![review_response(case)],
        api_key: Some(REVIEW_KEY),
        ..Receiver::default()
    })
    .await;
    // The checks carry their own deadline inside exercise_review's bounded
    // native cleanup; this outer bound only stops a setup that never ends.
    let outcome = std::panic::AssertUnwindSafe(tokio::time::timeout(
        Duration::from_mins(2),
        exercise_review(&controller, &reviewer, case),
    ))
    .catch_unwind()
    .await;
    controller.reap().await;
    reviewer.reap().await;
    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(error)) => panic!("native reviewer case setup timed out: {error}"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_original_context_reaches_model_review_and_verdict_preserves_continuation() {
    run_case(Case::Allow).await;
    run_case(Case::Deny).await;
    run_case(Case::Human).await;
    run_case(Case::HumanApprovedInRun).await;
    run_case(Case::HumanInRunHeldBarrier).await;
    run_case(Case::HumanInRunRefused).await;
    run_case(Case::HumanInRunWithheld).await;
    run_case(Case::HumanInRunHookFailed).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_original_source_denial_skips_reviewer_transport_and_preserves_continuation() {
    run_case(Case::SourceDenied).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_original_source_unavailable_keeps_local_review_feedback_and_continuation() {
    run_case(Case::SourceUnavailable).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_original_source_observation_failure_preserves_infrastructure_and_fresh_work() {
    run_case(Case::SourceObservationUnavailable).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_reviewer_model_denial_or_unavailability_preserves_source_and_continuation() {
    run_case(Case::ModelDenied).await;
    run_case(Case::ModelUnavailable).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_reviewer_model_observation_failure_preserves_source_sibling_and_fresh_work() {
    run_case(Case::ModelObservationUnavailable).await;
}

#[path = "model_reviewer/correlation.rs"]
mod correlation;

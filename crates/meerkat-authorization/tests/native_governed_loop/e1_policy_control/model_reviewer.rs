//! Actual native admission -> retained original read -> factory model review.
//! The local HTTP responses and application policy are fixtures. The work,
//! source-read, review-attempt and physical-entry owners are production code.
//! Each case has one original. Coalescing, retirement and awaited-run changes
//! are separate native-owner controls, not claims made by these Agent cases.

use super::*;
use meerkat::operation_reviewer::{
    ModelOperationReviewer, ModelReviewerConfig, NativeOperationReviewContextSource,
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
}
impl Case {
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
                allowed.review_tier = if facts.name == "delete_record" {
                    OperationReviewTier::R2
                } else {
                    OperationReviewTier::R1
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
                    | Case::ModelObservationUnavailable => {}
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

#[derive(Default)]
struct ReviewedTools(RecordingTools);
#[async_trait]
impl AgentToolDispatcher for ReviewedTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.0.tools()
    }
    fn review_entry_support(&self, _: &str) -> ReviewEntrySupport {
        ReviewEntrySupport::ConsumesAtEntry
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
        self.0.dispatch(call).await
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
            .filter(|record| matches!(record.observation.observation, AuditObservation::Entry))
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
            assert!(record.contributors[0].requester == principal("requester"));
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
    assert!(observations[0].attempt == observations[1].attempt);
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
    let review = Arc::new(
        BoundOperationReview::new(
            Arc::new(reviewer),
            meerkat_core::ApprovalService::new(),
            Duration::from_secs(5),
        )
        .with_observer(reviews.clone()),
    );

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
    operations.resource_domains = ExactRestriction::exact([domain(), input_domain()]);
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
            })
            .unwrap(),
    );
    let tools = Arc::new(ReviewedTools::default());
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
    let actor = service
        .live_session_actor_witness(&session_id)
        .await
        .unwrap();
    let pin = service
        .pin_controller_client_for_actor(&actor)
        .await
        .unwrap();
    assert!(pin.selection() == &selected);
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
        assert!(
            retained.messages().iter().any(
                |message| matches!(message, Message::User(user) if user.text_content() == GOAL)
            )
        );
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
        assert!(pin.selection() == &selected);
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
            .accept_input_with_completion(&session_id, fresh.with_ingress_context(ingress).unwrap())
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
        assert!(
            !fresh_audit
                .iter()
                .any(|record| record.observation.review_attribution.is_some()
                    || matches!(
                        record.observation.observation,
                        AuditObservation::ReviewAttemptStarted { .. }
                    ))
        );
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
                            blocks
                                .iter()
                                .all(|block| block["type"] == "text" && block["text"].is_string())
                                && wire_text(content) == FRESH
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
        tokio::time::timeout(
            Duration::from_secs(10),
            machine.unregister_current_session_registration_until_terminal(&session_id),
        )
        .await
        .unwrap()
        .unwrap();
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
    if let Some(feedback) = case.feedback() {
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
        assert!(claims.candidate().requester == principal("requester"));
        assert!(claims.candidate().ingress_actor == principal("ingress"));
        assert!(claims.candidate().logical_executor == principal("executor"));
    }

    let stored = machine
        .input_state(&session_id, &input_id)
        .await
        .unwrap()
        .unwrap();
    let audit: Vec<StoredAuthorizationAuditObservation> = serde_json::from_value(
        serde_json::to_value(stored).unwrap()["authorization_audit"].clone(),
    )
    .unwrap();
    assert_review_fault_audit(case, model_probe.as_deref(), &audit, &input_id, &reviews);
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
                AuditObservation::Entry | AuditObservation::Outcome { .. }
            )));
    }
    let reviewer_prepares: Vec<_> = audit.iter().filter(|record| matches!(&record.observation.observation,
        AuditObservation::Prepared { target, .. } if matches!(target.as_ref(), AuditTarget::Model(model)
            if model.usage == meerkat_authorization_contracts::audit::AuditModelUse::Inference
                && model.endpoint == format!("{}/v1/messages", review_server.base_url)
                && model.credential.as_ref() == Some(reviewer_selection.credential())))).collect();
    assert_eq!(reviewer_prepares.len(), usize::from(case.model_allowed()));
    for review_model in reviewer_prepares {
        assert!(review_model.observation.operation_id != source.observation.operation_id);
        for record in audit.iter().filter(|record| {
            record.observation.operation_id == review_model.observation.operation_id
        }) {
            assert_eq!(record.contributors.len(), 1);
            assert_eq!(record.contributors[0].input_id, input_id);
            assert!(record.contributors[0].requester == principal("requester"));
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

    controller_server.receiver.finish.notify_one();
    let outcome = tokio::time::timeout(Duration::from_secs(20), completion.wait())
        .await
        .unwrap()
        .unwrap();
    let CompletionOutcome::Completed(result) = outcome else {
        panic!("review settles locally in the original run: {outcome:?}")
    };
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
        saved
            .messages()
            .iter()
            .any(|message| matches!(message, Message::User(user) if user.text_content() == GOAL))
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
    assert_eq!(controller_server.receiver.bodies.lock().unwrap().len(), 2);
    assert_eq!(
        review_server.receiver.bodies.lock().unwrap().len(),
        usize::from(case.model_allowed())
    );
    assert_eq!(
        source_checks.load(Ordering::SeqCst),
        1,
        "one native original read, with no policy retry"
    );
    tokio::time::timeout(
        Duration::from_secs(10),
        machine.unregister_current_session_registration_until_terminal(&session_id),
    )
    .await
    .unwrap()
    .unwrap();
}

async fn run_case(case: Case) {
    let mut controller = Server::start().await;
    let mut reviewer = Server::start_with_receiver(Receiver {
        first_responses: vec![review_response(case)],
        api_key: Some(REVIEW_KEY),
        ..Receiver::default()
    })
    .await;
    let outcome = std::panic::AssertUnwindSafe(tokio::time::timeout(
        Duration::from_secs(60),
        exercise_review(&controller, &reviewer, case),
    ))
    .catch_unwind()
    .await;
    controller.reap().await;
    reviewer.reap().await;
    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(error)) => panic!("native reviewer case timed out: {error}"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_original_context_reaches_model_review_and_verdict_preserves_continuation() {
    run_case(Case::Allow).await;
    run_case(Case::Deny).await;
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

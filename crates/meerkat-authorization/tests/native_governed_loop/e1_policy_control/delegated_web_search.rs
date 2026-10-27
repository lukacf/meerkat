//! The governed builtin web_search through the native loop (the model
//! reviewer's pattern): the real composite produces and revalidates the
//! call's typed builtin target, a real Anthropic web-search helper sends on
//! its own loopback route, and the host owner maps exactly one typed target
//! per call. The helper is the typed delegate of its own call and is decided
//! by its own route's lineage only; every refusal is ordinary model feedback
//! while the permitted sibling and the run continue.
use super::*;
use meerkat_authorization::grant_policy::single_tool_operation_target;
use meerkat_authorization_contracts::audit::{AuditDelegateAttribution, AuditDelegateRole};
use meerkat_authorization_contracts::work_association::{GrantTarget, TargetGrantLineage};
use meerkat_core::authorization::{ToolAuthorizationFacts, ToolDelegateRole};
use meerkat_core::{
    BuiltinToolOperation, OperationId, ReviewUnavailableKind, ReviewUnsatisfiedKind,
    ServerToolKind, ToolOperationTarget,
};

const SEARCH_CALL: &str = "governed-search";
const SECOND_SEARCH_CALL: &str = "governed-search-again";
const SEARCH_ANSWER: &str = "governed search answer";
const SEARCH_URL: &str = "https://search-result.invalid/governed";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Case {
    /// Every lineage selected and the owner allows the helper: one send.
    Allowed,
    /// The work selects no lineage for the helper's route.
    DelegateUnselected,
    /// The owner cannot decide the helper now.
    OwnerUnavailable,
    /// The call's own lineage is revoked after the call entered and before
    /// its helper: the entered invocation's helper still sends on its own
    /// lineage, and the next web_search call is refused at its own entry.
    OuterRevokedInFlight,
    /// The helper's own lineage is revoked after the call entered and before
    /// the helper's entry: nothing is sent.
    DelegateRevokedBeforeEntry,
    /// The helper's own lineage is revoked after its request was prepared,
    /// while its client authorizes the request and before the physical send:
    /// the send's own current check refuses it and nothing is sent.
    DelegateRevokedAfterPrepare,
    /// The owner rates the helper's inference R2 while a reviewer is
    /// installed. A model request carries no operation review, so it settles
    /// at its model entry as `unsupported_entry` and reaches the model as the
    /// same typed `review_unavailable` feedback any other tool gives: nothing
    /// is sent and the reviewer is never asked.
    HelperReviewRequired,
    /// The owner rates the helper's inference R3 while a reviewer is
    /// installed: it settles at its model entry as `human_consent_required`
    /// and reaches the model as typed `review_unsatisfied` feedback, with
    /// nothing sent and the reviewer never asked.
    HelperConsentRequired,
    /// A host tool merely named web_search: no typed target, no delegate.
    HostToolNamedWebSearch,
}

fn search_domain() -> ResourceDomain {
    ResourceDomain {
        authority: principal("resource-owner"),
        namespace: "public-search".into(),
    }
}

fn values(domain: ResourceDomain, actions: &[&str]) -> Vec<LocalOperationValues> {
    actions
        .iter()
        .map(|name| LocalOperationValues {
            action: action(name),
            resource_domain: domain.clone(),
            processor: ProcessorRef::Principal {
                principal: principal("executor"),
            },
            audience: AudienceRef::Principal {
                principal: principal("requester"),
            },
        })
        .collect()
}

fn search_bounds(actions: &[&str]) -> ExecutionRestrictions {
    let mut bounds = ExecutionRestrictions::unrestricted();
    bounds.actions = ExactRestriction::exact(actions.iter().map(|name| action(name)));
    bounds.resource_domains = DomainRestriction::exact([search_domain()]);
    bounds
}

fn allowance(
    operation_values: Vec<LocalOperationValues>,
    now_ms: u64,
) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
    Ok(LocalPolicyAllowance {
        operation_values,
        restrictions: ExecutionRestrictions::unrestricted(),
        expires_at_ms: now_ms + 60_000,
        review_tier: meerkat_core::authorization::OperationReviewTier::R1,
    })
}

fn is_builtin_search(target: Option<&ToolOperationTarget>) -> bool {
    matches!(
        target,
        Some(ToolOperationTarget::BuiltinOperation {
            operation: BuiltinToolOperation::WebSearch,
        })
    )
}

/// The host's policy owner. The controller route exactly; read_record by its
/// correlated facts; web_search only by the typed builtin target its owner
/// produced; a host tool merely named web_search as an ordinary read; the
/// search helper only as the typed delegate of that call, on its own
/// approved route, with exactly the web search capability. No name selects
/// web search, and no tuple is the controller's.
struct SearchOwner {
    controller: HttpRecordOwner,
    helper_route: String,
    helper_unavailable: bool,
    helper_review_tier: meerkat_core::authorization::OperationReviewTier,
    helper_seen: AtomicUsize,
    calls: Mutex<Vec<(String, Option<ToolOperationTarget>)>>,
}

impl OperationPolicyOwner for SearchOwner {
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
            AuthorizationOperation::Tool(facts) if purpose == LocalPolicyPurpose::Operation => {
                let ToolAuthorizationTarget::Dispatcher(plan) = &facts.target else {
                    return Err(denied().into());
                };
                let target = single_tool_operation_target(plan)?.cloned();
                self.calls
                    .lock()
                    .unwrap()
                    .push((facts.name.to_string(), target.clone()));
                match target {
                    target if is_builtin_search(target.as_ref()) => {
                        allowance(values(search_domain(), &["web-search"]), now_ms)
                    }
                    None if facts.name.as_str() == "web_search" => {
                        allowance(values(domain(), &["read"]), now_ms)
                    }
                    None => RecordOwner.authorize_operation(association, binding, purpose, now_ms),
                    Some(_) => Err(denied().into()),
                }
            }
            AuthorizationOperation::Model(facts) if binding.delegate_attribution().is_some() => {
                self.helper_seen.fetch_add(1, Ordering::SeqCst);
                let link = binding.delegate_attribution().expect("checked above");
                let parent_is_search = matches!(
                    &link.parent_binding().facts().operation,
                    AuthorizationOperation::Tool(ToolAuthorizationFacts {
                        target: ToolAuthorizationTarget::Dispatcher(plan),
                        ..
                    }) if single_tool_operation_target(plan)
                        .is_ok_and(is_builtin_search)
                );
                if purpose != LocalPolicyPurpose::Operation
                    || link.role() != ToolDelegateRole::HostedSearchInference
                    || !parent_is_search
                    || facts.usage != ModelAuthorizationUse::Inference
                    || facts.hosted_capabilities.as_ref() != [ServerToolKind::WebSearch]
                    || facts
                        .controller_route_key()
                        .is_none_or(|route| route.as_str() != self.helper_route)
                {
                    return Err(denied().into());
                }
                if self.helper_unavailable {
                    return Err(meerkat_core::OperationAuthorizationError::Unavailable);
                }
                allowance(
                    values(search_domain(), &["search-infer", "hosted-web-search"]),
                    now_ms,
                )
                .map(|allowance| LocalPolicyAllowance {
                    review_tier: self.helper_review_tier,
                    ..allowance
                })
            }
            _ => self
                .controller
                .authorize_operation(association, binding, purpose, now_ms),
        }
    }
}

/// An installed reviewer that counts every review it is asked for.
#[derive(Default)]
struct CountingReviewer(AtomicUsize);

#[async_trait]
impl meerkat_core::approval::review::OperationReviewer for CountingReviewer {
    async fn review(
        &self,
        _candidate: &meerkat_core::approval::review::ReviewCandidate<'_>,
    ) -> Result<
        meerkat_core::approval::review::ReviewVerdict,
        meerkat_core::approval::review::ReviewerFailure,
    > {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(meerkat_core::approval::review::ReviewVerdict::Allow)
    }
}

/// The installed search executor with one change of the grant owner's
/// state run immediately before its own (helper) work: after the call
/// entered, before the helper prepares or sends.
struct BeforeHelper {
    inner: Arc<dyn meerkat_llm_core::WebSearchExecutor>,
    before: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

#[async_trait]
impl meerkat_llm_core::WebSearchExecutor for BeforeHelper {
    async fn execute_web_search(
        &self,
        request: meerkat_core::web_search::WebSearchRequest,
    ) -> Result<meerkat_core::web_search::WebSearchResult, meerkat_llm_core::LlmError> {
        self.execute_web_search_authorized(request, None).await
    }

    async fn execute_web_search_authorized(
        &self,
        request: meerkat_core::web_search::WebSearchRequest,
        authorization: Option<meerkat_core::LlmRequestAuthorization>,
    ) -> Result<meerkat_core::web_search::WebSearchResult, meerkat_llm_core::LlmError> {
        let before = self.before.lock().unwrap().take();
        if let Some(before) = before {
            before();
        }
        self.inner
            .execute_web_search_authorized(request, authorization)
            .await
    }
}

/// The helper client's authorizer with one change of the grant owner's state
/// run while it authorizes the helper's request: after the provider client
/// prepared the request's authorization, before the physical send.
struct BeforeSend {
    inner: FixtureAuthorizer,
    before: Arc<Mutex<Option<Box<dyn FnOnce() + Send>>>>,
}

#[async_trait]
impl HttpAuthorizer for BeforeSend {
    async fn authorize(
        &self,
        request: &mut meerkat_core::HttpAuthorizationRequest<'_>,
    ) -> Result<(), meerkat_core::AuthError> {
        let before = self.before.lock().unwrap().take();
        if let Some(before) = before {
            before();
        }
        self.inner.authorize(request).await
    }

    fn label(&self) -> &'static str {
        "e1-private-loopback"
    }
}

/// A host dispatcher whose own tool is merely named web_search, beside the
/// fixture's record tools. Its body asks the dispatch for a delegate, as a
/// helper of its own would.
#[derive(Default)]
struct HostSearchTools {
    records: RecordingTools,
    minted: Mutex<Vec<Result<bool, ToolError>>>,
}

#[async_trait]
impl AgentToolDispatcher for HostSearchTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        let mut tools = self.records.tools().to_vec();
        tools.push(Arc::new(ToolDef::new(
            "web_search",
            "host search",
            json!({
                "type": "object", "properties": {"query": {"type": "string"}},
                "required": ["query"], "additionalProperties": false,
            }),
        )));
        tools.into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        self.records.dispatch(call).await
    }

    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        if call.name != "web_search" {
            return self.records.dispatch_with_context(call, context).await;
        }
        let minted = context
            .tool_delegate_attribution(call, ToolDelegateRole::HostedSearchInference)
            .map(|attribution| attribution.is_some());
        self.minted.lock().unwrap().push(minted);
        Ok(ToolResult::new(
            call.id.into(),
            "host search ran without a delegate".into(),
            false,
        )
        .into())
    }
}

fn search_batch(search_call: &str) -> String {
    let mut events = vec![start_message()];
    for (index, (id, name, input)) in [
        (search_call, "web_search", r#"{"query":"governed query"}"#),
        (PERMITTED_CALL, "read_record", r#"{"record":"record-7"}"#),
    ]
    .into_iter()
    .enumerate()
    {
        events.extend([
            json!({"type":"content_block_start","index":index,"content_block":{"type":"tool_use","id":id,"name":name,"input":{}}}),
            json!({"type":"content_block_delta","index":index,"delta":{"type":"input_json_delta","partial_json":input}}),
            json!({"type":"content_block_stop","index":index}),
        ]);
    }
    events.extend([
        json!({"type":"message_delta","usage":{"output_tokens":2},"delta":{"stop_reason":"tool_use"}}),
        json!({"type":"message_stop"}),
    ]);
    sse(events)
}

fn second_search() -> String {
    sse(vec![
        start_message(),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":SECOND_SEARCH_CALL,"name":"web_search","input":{}}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"query\":\"governed query again\"}"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"message_delta","usage":{"output_tokens":2},"delta":{"stop_reason":"tool_use"}}),
        json!({"type":"message_stop"}),
    ])
}

/// The helper's provider answer: one native web search and its result.
fn search_answer() -> String {
    sse(vec![
        start_message(),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"server_tool_use","id":"srvtoolu_governed","name":"web_search","input":{}}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"query\":\"governed query\"}"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"web_search_tool_result","tool_use_id":"srvtoolu_governed","content":[{"type":"web_search_result","url":SEARCH_URL,"title":"Governed result"}]}}),
        json!({"type":"content_block_stop","index":1}),
        json!({"type":"content_block_start","index":2,"content_block":{"type":"text","text":""}}),
        json!({"type":"content_block_delta","index":2,"delta":{"type":"text_delta","text":SEARCH_ANSWER}}),
        json!({"type":"content_block_stop","index":2}),
        json!({"type":"message_delta","usage":{"output_tokens":3},"delta":{"stop_reason":"end_turn"}}),
        json!({"type":"message_stop"}),
    ])
}

/// One call's own committed result in a request body.
fn wire_result(body: &Value, call_id: &str) -> Value {
    body["messages"]
        .as_array()
        .expect("request messages")
        .iter()
        .filter(|message| message["role"] == "user")
        .filter_map(|message| message["content"].as_array())
        .flat_map(|blocks| blocks.iter())
        .find(|block| block["type"] == "tool_result" && block["tool_use_id"] == call_id)
        .cloned()
        .unwrap_or_else(|| panic!("the committed result of {call_id}"))
}

fn assert_refusal_feedback(body: &Value, call_id: &str, expected: ToolError) {
    let result = wire_result(body, call_id);
    assert_eq!(result["is_error"], true, "{call_id}");
    let diagnostic: Value =
        serde_json::from_str(&wire_text(&result["content"])).expect("typed safe refusal JSON");
    assert_eq!(diagnostic, expected.to_error_payload(), "{call_id}");
}

fn assert_sibling_completed(body: &Value) {
    let read = wire_result(body, PERMITTED_CALL);
    assert_ne!(read["is_error"], true);
    assert_eq!(wire_text(&read["content"]), "record-7 value");
}

/// The protected records of the operations whose target is a model request
/// on `endpoint`, in order, with every later record of those operations.
fn model_records<'a>(
    audit: &'a [StoredAuthorizationAuditObservation],
    endpoint: &str,
) -> Vec<&'a StoredAuthorizationAuditObservation> {
    let targeted =
        |record: &StoredAuthorizationAuditObservation| match &record.observation.observation {
            AuditObservation::Prepared { target, .. }
            | AuditObservation::Refused { target, .. }
            | AuditObservation::AuthorizationUnavailable { target } => {
                matches!(target.as_ref(), AuditTarget::Model(model) if model.endpoint == endpoint)
            }
            _ => false,
        };
    let ids: Vec<&OperationId> = audit
        .iter()
        .filter(|record| targeted(record))
        .map(|record| &record.observation.operation_id)
        .collect();
    audit
        .iter()
        .filter(|record| ids.contains(&&record.observation.operation_id))
        .collect()
}

/// The operation of the prepared tool call `call_id`.
fn tool_operation(audit: &[StoredAuthorizationAuditObservation], call_id: &str) -> OperationId {
    audit
        .iter()
        .find_map(|record| match &record.observation.observation {
            AuditObservation::Prepared { target, .. }
                if matches!(target.as_ref(), AuditTarget::Tool { call_id: id, .. } if id == call_id) =>
            {
                Some(record.observation.operation_id.clone())
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("the prepared call {call_id}"))
}

fn audit_rows(stored: &impl serde::Serialize) -> Vec<StoredAuthorizationAuditObservation> {
    serde_json::from_value(serde_json::to_value(stored).unwrap()["authorization_audit"].clone())
        .expect("actual row-bound audit observations")
}

async fn exercise_search(controller_server: &Server, helper_server: &Server, case: Case) {
    let client = http_client(controller_server);
    let selected = client
        .controller_model_selection()
        .expect("actual registry selection");
    let endpoint = format!("{}/v1/messages", controller_server.base_url);
    let helper_binding = AuthBindingRef {
        realm: RealmId::parse("native-loop").unwrap(),
        binding: BindingId::parse(format!("e1-search-{}", SessionId::new())).unwrap(),
        profile: None,
        origin: BindingOrigin::Configured,
    };
    let before_send: Arc<Mutex<Option<Box<dyn FnOnce() + Send>>>> = Arc::default();
    let helper_client = http_client_with_authorizer(
        helper_server,
        helper_binding,
        Arc::new(BeforeSend {
            inner: FixtureAuthorizer(helper_server.receiver.clone()),
            before: Arc::clone(&before_send),
        }),
    );
    let helper_endpoint = format!("{}/v1/messages", helper_server.base_url);
    // The key the grant selection names: the helper's own resolved route,
    // exactly as its requests report it.
    let helper_route = meerkat_core::ControllerModelFacts::new(
        helper_client
            .controller_model_selection()
            .expect("the helper's own registry selection"),
        helper_endpoint.clone().into(),
        E1_MODEL.into(),
    )
    .route_key()
    .as_str()
    .to_owned();
    let controller_route = meerkat_core::ControllerModelFacts::new(
        selected.clone(),
        endpoint.clone().into(),
        E1_MODEL.into(),
    )
    .route_key();
    assert_ne!(helper_route, controller_route.as_str());

    let grants = Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: principal("grant-owner"),
                namespace: id("e1-search-grants"),
                generation: 1,
            },
            LocalAuthorizationPublication::new(),
            Arc::new(HostAuthorizationClock),
        )
        .unwrap(),
    );
    let issue = |name: &str, bounds: ExecutionRestrictions| {
        grants
            .issue_root(
                &principal("grant-owner"),
                id(name),
                principal("executor"),
                None,
                bounds,
            )
            .unwrap()
    };
    let controller = issue("controller", ceiling("infer"));
    let read_only = issue("read-only", ceiling("read"));
    let search_call = issue("web-search-call", search_bounds(&["web-search"]));
    let delegated = issue(
        "delegated-search",
        search_bounds(&["search-infer", "hosted-web-search"]),
    );
    let mut targets = vec![TargetGrantLineage {
        target: GrantTarget::BuiltinToolOperation {
            operation: BuiltinToolOperation::WebSearch,
        },
        lineage: vec![search_call.clone()],
    }];
    if case != Case::DelegateUnselected {
        targets.push(TargetGrantLineage {
            target: GrantTarget::DelegatedHostedSearch {
                route: helper_route.clone(),
                capability: ServerToolKind::WebSearch,
            },
            lineage: vec![delegated.clone()],
        });
    }
    let basis = WorkAuthorityBasis::TargetedGrantLineages {
        base: Some(vec![read_only.clone()]),
        targets,
    };
    let expected_controller = controller.clone();
    let expected_basis = basis.clone();
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
            || candidate.authority_basis != expected_basis
            || candidate.controller_model.as_ref() != Some(&expected_selection)
            || candidate.controller_ceiling != ceiling("infer")
        {
            return Err(denied().into());
        }
        Ok(())
    });
    let owner = Arc::new(SearchOwner {
        controller: HttpRecordOwner {
            selection: selected.clone(),
            endpoint: endpoint.clone(),
        },
        helper_route: helper_route.clone(),
        helper_unavailable: case == Case::OwnerUnavailable,
        helper_review_tier: match case {
            Case::HelperReviewRequired => meerkat_core::authorization::OperationReviewTier::R2,
            Case::HelperConsentRequired => meerkat_core::authorization::OperationReviewTier::R3,
            _ => meerkat_core::authorization::OperationReviewTier::R1,
        },
        helper_seen: AtomicUsize::new(0),
        calls: Mutex::new(Vec::new()),
    });
    let machine = Arc::new(
        MeerkatMachine::ephemeral()
            .with_local_grant_authorization(NativeGrantWorkConfiguration {
                retained_resume: None,
                require_scoped_destination: false,
                grants: Arc::clone(&grants),
                ingress,
                invocation_owner: Arc::new(InvocationOwner),
                operation_owner: owner.clone(),
                configured_mcp_attachment: None,
            })
            .unwrap(),
    );

    // The real composite with the real web_search builtin over the real
    // Anthropic helper; the host's record tools are its external tools.
    let adapted: Arc<dyn meerkat_core::AgentLlmClient> = Arc::new(
        meerkat_llm_core::LlmClientAdapter::try_for_provider_identity(
            helper_client,
            E1_MODEL.into(),
            Provider::Anthropic,
        )
        .unwrap(),
    );
    let revoke = |lineage: GrantLineageRef| -> Box<dyn FnOnce() + Send> {
        let grants = Arc::clone(&grants);
        Box::new(move || {
            struct NoController;
            impl meerkat_authorization_contracts::grant_mutation::ControllerGrantMutationCustody
                for NoController
            {
                fn with_unreferenced_controller_grant<T, E>(
                    &mut self,
                    _: &GrantLineageRef,
                    mutate: impl FnOnce() -> Result<T, E>,
                ) -> Result<
                    Result<T, E>,
                    meerkat_authorization_contracts::grant_mutation::ControllerCustodyRefusal,
                > {
                    Ok(mutate())
                }
            }
            grants
                .revoke(&principal("grant-owner"), &lineage, &mut NoController)
                .expect("revocation before the helper");
        })
    };
    let executor = Arc::new(BeforeHelper {
        inner: Arc::new(meerkat_anthropic::AnthropicWebSearchExecutor::new(
            E1_MODEL.into(),
            adapted,
        )),
        before: Mutex::new(match case {
            Case::OuterRevokedInFlight => Some(revoke(search_call.clone())),
            Case::DelegateRevokedBeforeEntry => Some(revoke(delegated.clone())),
            _ => None,
        }),
    });
    if case == Case::DelegateRevokedAfterPrepare {
        *before_send.lock().unwrap() = Some(revoke(delegated.clone()));
    }
    let host_tools = Arc::new(HostSearchTools::default());
    let records = Arc::new(RecordingTools::default());
    // The record tools that actually serve this case's sibling read.
    let entered = |case: Case| {
        if case == Case::HostToolNamedWebSearch {
            host_tools.records.0.lock().unwrap().clone()
        } else {
            records.0.lock().unwrap().clone()
        }
    };
    let project = tempfile::tempdir().unwrap();
    let external: Arc<dyn AgentToolDispatcher> = if case == Case::HostToolNamedWebSearch {
        host_tools.clone()
    } else {
        records.clone()
    };
    let mut composite = meerkat_tools::builtin::CompositeDispatcher::new(
        Arc::new(meerkat_tools::builtin::MemoryTaskStore::new()),
        &meerkat_tools::builtin::BuiltinToolConfig {
            policy: meerkat_tools::builtin::ToolPolicyLayer::new()
                .with_mode(meerkat_tools::builtin::ToolMode::AllowList),
            ..Default::default()
        },
        Some(project.path().to_path_buf()),
        None,
        Some(external),
        None,
    )
    .unwrap();
    if case != Case::HostToolNamedWebSearch {
        composite.register_web_search_tool(executor, meerkat_core::ToolCategoryOverride::Enable);
    }

    let sessions = Arc::new(RecordingStore::default());
    let reviewer = Arc::new(CountingReviewer::default());
    let factory = if matches!(
        case,
        Case::HelperReviewRequired | Case::HelperConsentRequired
    ) {
        AgentFactory::minimal().with_operation_review(Arc::new(
            meerkat_core::approval::review::BoundOperationReview::new(
                reviewer.clone(),
                meerkat_core::ApprovalService::new(),
                Duration::from_secs(5),
            ),
        ))
    } else {
        AgentFactory::minimal()
    };
    let mut builder = FactoryAgentBuilder::new(factory, Config::default());
    builder.default_llm_client = Some(client);
    builder.default_tool_dispatcher = Some(Arc::new(composite));
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
                auth_binding: Some(
                    selected
                        .auth_binding()
                        .expect("registered fixture binding")
                        .clone(),
                ),
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
    assert_eq!(pin.selection(), &selected);
    let vault = Arc::new(meerkat_auth_core::EphemeralTokenStore::new());
    meerkat_auth_core::save_tokens_and_publish_lifecycle(
        meerkat_core::auth::ProviderAuthPersistence::new(
            vault,
            Arc::new(meerkat_auth_core::InMemoryCoordinator::new()),
        ),
        machine.generated_auth_lease_handle(),
        selected.credential().clone(),
        meerkat_core::auth::PersistedTokens::api_key("synthetic-e1-loopback-only"),
    )
    .await
    .unwrap();
    let runtime = LogicalRuntimeId::for_session(&session_id);
    let mut candidate = association(&runtime, controller, read_only, selected.clone())
        .candidate()
        .clone();
    candidate.authority_basis = basis;
    let mut prompt = PromptInput::new("Search the web, then read the record", None);
    prompt.header.authority_association =
        Some(InputAuthorityAssociation::new(candidate).expect("targeted claims"));
    let input = Input::Prompt(prompt);
    let input_id = input.id().clone();
    let current = NativeIngressContext::from_trusted_ingress(
        &input,
        principal("requester"),
        principal("ingress"),
        RealmId::parse("native-loop").unwrap(),
        super::evidence("e1-search-authentication"),
    )
    .unwrap()
    .with_controller_client(&input, pin)
    .unwrap();
    let input = input.with_ingress_context(current).unwrap();
    let (_, completion) = machine
        .accept_input_with_completion(&session_id, input)
        .await
        .unwrap();
    let completion = completion.expect("actual native completion receipt");
    tokio::time::timeout(
        Duration::from_secs(20),
        controller_server.receiver.second_request.notified(),
    )
    .await
    .expect("the same native run returns both results in request two");

    let bodies = controller_server.receiver.bodies.lock().unwrap().clone();
    assert_eq!(bodies.len(), 2);
    // The permitted sibling always completes beside the search's own result.
    assert_sibling_completed(&bodies[1]);
    assert_eq!(entered(case), ["read_record"]);
    let helper_sends = helper_server.receiver.bodies.lock().unwrap().len();
    let audit = audit_rows(
        &machine
            .input_state(&session_id, &input_id)
            .await
            .unwrap()
            .expect("actual unfinished native row"),
    );
    let helper_records = model_records(&audit, &helper_endpoint);
    let calls = owner.calls.lock().unwrap().clone();
    let search_calls: Vec<_> = calls
        .iter()
        .filter(|(name, _)| name == "web_search")
        .collect();

    if case == Case::HostToolNamedWebSearch {
        let result = wire_result(&bodies[1], SEARCH_CALL);
        assert_ne!(result["is_error"], true);
        assert_eq!(
            wire_text(&result["content"]),
            "host search ran without a delegate"
        );
        assert_eq!(search_calls.len(), 1);
        assert!(
            search_calls[0].1.is_none(),
            "a host tool named web_search carries no typed target"
        );
        let minted = host_tools.minted.lock().unwrap();
        assert!(
            matches!(minted.as_slice(), [Err(ToolError::AuthorizationRefused { refusal })]
                if refusal.kind() == OperationRefusalKind::MalformedFacts),
            "it has no delegate: {minted:?}"
        );
        assert!(helper_records.is_empty());
        assert_eq!(owner.helper_seen.load(Ordering::SeqCst), 0);
    } else {
        assert_eq!(search_calls.len(), 1);
        assert!(
            is_builtin_search(search_calls[0].1.as_ref()),
            "the composite's own builtin target reached the owner"
        );
        let parent = tool_operation(&audit, SEARCH_CALL);
        let expected_link = AuditDelegateAttribution {
            parent_operation_id: parent.clone(),
            role: AuditDelegateRole::HostedSearchInference,
        };
        for record in &helper_records {
            assert_eq!(
                record.observation.delegate_attribution.as_ref(),
                Some(&expected_link),
                "every helper record names its parent call"
            );
            assert!(record.observation.review_attribution.is_none());
        }
        let has = |predicate: fn(&AuditObservation) -> bool| {
            helper_records
                .iter()
                .any(|record| predicate(&record.observation.observation))
        };
        match case {
            Case::Allowed | Case::OuterRevokedInFlight => {
                let result = wire_result(&bodies[1], SEARCH_CALL);
                assert_ne!(result["is_error"], true, "{result}");
                let text = wire_text(&result["content"]);
                assert!(text.contains(SEARCH_ANSWER), "{text}");
                assert!(text.contains(SEARCH_URL), "{text}");
                assert_eq!(helper_sends, 1, "one helper send");
                assert_eq!(owner.helper_seen.load(Ordering::SeqCst), 1);
                assert!(has(|observation| matches!(
                    observation,
                    AuditObservation::Prepared { .. }
                )));
                assert!(has(|observation| matches!(
                    observation,
                    AuditObservation::Entry { .. }
                )));
                assert!(has(|observation| matches!(
                    observation,
                    AuditObservation::Outcome {
                        outcome: OperationObservedOutcome::HttpResponse { status: 200 }
                    }
                )));
                let helper_body = &helper_server.receiver.bodies.lock().unwrap()[0];
                assert_eq!(helper_body["model"], E1_MODEL);
                assert!(
                    helper_body["tools"]
                        .as_array()
                        .is_some_and(|tools| tools.iter().any(|tool| tool["name"] == "web_search")),
                    "the helper's own hosted search: {helper_body}"
                );
            }
            Case::DelegateUnselected | Case::DelegateRevokedBeforeEntry => {
                assert_refusal_feedback(
                    &bodies[1],
                    SEARCH_CALL,
                    ToolError::AuthorizationRefused { refusal: denied() },
                );
                assert_eq!(helper_sends, 0, "nothing is sent for a refused helper");
                assert_eq!(
                    owner.helper_seen.load(Ordering::SeqCst),
                    0,
                    "the grant owner refuses before the operation owner"
                );
                assert!(has(|observation| matches!(
                    observation,
                    AuditObservation::Refused {
                        reason: OperationRefusalKind::Denied,
                        ..
                    }
                )));
                assert!(!has(|observation| matches!(
                    observation,
                    AuditObservation::Prepared { .. }
                        | AuditObservation::Entry { .. }
                        | AuditObservation::Outcome { .. }
                )));
            }
            Case::DelegateRevokedAfterPrepare => {
                assert_refusal_feedback(
                    &bodies[1],
                    SEARCH_CALL,
                    ToolError::AuthorizationRefused { refusal: denied() },
                );
                assert_eq!(
                    helper_server
                        .receiver
                        .authorized_requests
                        .load(Ordering::SeqCst),
                    1,
                    "the client authorized the prepared request"
                );
                assert_eq!(
                    helper_sends, 0,
                    "nothing is sent once its lineage is revoked"
                );
                assert_eq!(
                    owner.helper_seen.load(Ordering::SeqCst),
                    1,
                    "the owner decided the prepared request once; its refresh is refused by the grant owner"
                );
                assert!(has(|observation| matches!(
                    observation,
                    AuditObservation::Prepared { .. }
                )));
                assert!(has(|observation| matches!(
                    observation,
                    AuditObservation::Refused {
                        reason: OperationRefusalKind::Denied,
                        ..
                    }
                )));
                assert!(!has(|observation| matches!(
                    observation,
                    AuditObservation::Entry { .. } | AuditObservation::Outcome { .. }
                )));
            }
            Case::OwnerUnavailable => {
                assert_refusal_feedback(
                    &bodies[1],
                    SEARCH_CALL,
                    ToolError::OperationAuthorizationUnavailable,
                );
                assert_eq!(helper_sends, 0);
                assert_eq!(owner.helper_seen.load(Ordering::SeqCst), 1, "asked once");
                assert!(has(|observation| matches!(
                    observation,
                    AuditObservation::AuthorizationUnavailable { .. }
                )));
                assert!(!has(|observation| matches!(
                    observation,
                    AuditObservation::Prepared { .. }
                        | AuditObservation::Entry { .. }
                        | AuditObservation::Outcome { .. }
                )));
            }
            Case::HelperReviewRequired | Case::HelperConsentRequired => {
                // The model entry settles the required review locally; the
                // executor seam keeps that typed settlement, so the model
                // gets exactly the review feedback any other tool gives.
                assert_refusal_feedback(
                    &bodies[1],
                    SEARCH_CALL,
                    if case == Case::HelperReviewRequired {
                        ToolError::ReviewUnavailable {
                            kind: ReviewUnavailableKind::UnsupportedEntry,
                        }
                    } else {
                        ToolError::ReviewUnsatisfied {
                            kind: ReviewUnsatisfiedKind::HumanConsentRequired,
                            approval_id: None,
                        }
                    },
                );
                assert_eq!(helper_sends, 0, "nothing is sent at R2 or R3");
                assert_eq!(
                    reviewer.0.load(Ordering::SeqCst),
                    0,
                    "a model request never asks the installed reviewer"
                );
                assert_eq!(owner.helper_seen.load(Ordering::SeqCst), 1);
                assert!(has(|observation| matches!(
                    observation,
                    AuditObservation::Prepared { .. }
                )));
                assert!(
                    !has(|observation| matches!(
                        observation,
                        AuditObservation::Entry { .. }
                            | AuditObservation::Outcome { .. }
                            | AuditObservation::Refused { .. }
                    )),
                    "the review refusal at the model entry writes no audit row"
                );
            }
            Case::HostToolNamedWebSearch => unreachable!(),
        }
        // The call itself entered and returned its own result.
        assert!(
            audit
                .iter()
                .any(|record| record.observation.operation_id == parent
                    && matches!(
                        record.observation.observation,
                        AuditObservation::Entry { .. }
                    ))
        );
    }

    controller_server.receiver.finish.notify_one();
    let outcome = tokio::time::timeout(Duration::from_secs(20), completion.wait())
        .await
        .unwrap()
        .unwrap();
    let CompletionOutcome::Completed(result) = outcome else {
        panic!("a refused search must not terminate admitted work: {outcome:?}")
    };
    assert_eq!(result.text, FINISHED);
    assert_eq!(result.session_id, session_id);
    assert!(result.terminal_cause_kind.is_none());
    let bodies = controller_server.receiver.bodies.lock().unwrap().clone();
    if case == Case::OuterRevokedInFlight {
        // Outer revocation does not cancel the entered invocation (its helper
        // sent above); it refuses the next web_search call at its own entry,
        // before any helper.
        assert_eq!(bodies.len(), 3);
        assert_refusal_feedback(
            &bodies[2],
            SECOND_SEARCH_CALL,
            ToolError::AuthorizationRefused {
                refusal: denied_before_dispatch(),
            },
        );
        assert_eq!(helper_server.receiver.bodies.lock().unwrap().len(), 1);
        assert_eq!(owner.helper_seen.load(Ordering::SeqCst), 1);
    } else {
        assert_eq!(bodies.len(), 2);
    }
    assert_eq!(entered(case), ["read_record"]);
}

async fn run_case(case: Case) {
    let mut controller = Server::start_with_receiver(Receiver {
        first_responses: if case == Case::OuterRevokedInFlight {
            vec![search_batch(SEARCH_CALL), final_response()]
        } else {
            vec![search_batch(SEARCH_CALL)]
        },
        model: Some(E1_MODEL),
        held_tool_response: (case == Case::OuterRevokedInFlight).then(second_search),
        ..Receiver::default()
    })
    .await;
    let mut helper = Server::start_with_receiver(Receiver {
        first_responses: vec![search_answer()],
        model: Some(E1_MODEL),
        ..Receiver::default()
    })
    .await;
    let outcome = std::panic::AssertUnwindSafe(tokio::time::timeout(
        Duration::from_mins(1),
        exercise_search(&controller, &helper, case),
    ))
    .catch_unwind()
    .await;
    controller.reap().await;
    helper.reap().await;
    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(error)) => panic!("{case:?} timed out: {error}"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn governed_web_search_runs_its_helper_as_the_typed_delegate_of_its_call() {
    run_case(Case::Allowed).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unselected_delegate_is_refused_feedback_before_any_helper_send() {
    run_case(Case::DelegateUnselected).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unavailable_helper_decision_is_local_feedback_after_one_owner_read() {
    run_case(Case::OwnerUnavailable).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn outer_revocation_keeps_the_entered_invocation_and_refuses_the_next_call() {
    run_case(Case::OuterRevokedInFlight).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delegate_revocation_before_helper_entry_prevents_its_send() {
    run_case(Case::DelegateRevokedBeforeEntry).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delegate_revocation_after_its_preparation_prevents_its_send() {
    run_case(Case::DelegateRevokedAfterPrepare).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_helper_rated_r2_settles_at_its_model_entry_without_a_send_or_review() {
    run_case(Case::HelperReviewRequired).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_helper_rated_r3_settles_at_its_model_entry_without_a_send_or_review() {
    run_case(Case::HelperConsentRequired).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_host_tool_named_web_search_has_no_typed_target_or_delegate() {
    run_case(Case::HostToolNamedWebSearch).await;
}

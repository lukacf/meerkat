//! A governed web_search call and its hosted-search delegate each resolve
//! only their own exact lineage in a targeted basis: the call the lineage
//! selected for its typed builtin target, the delegate the one selected for
//! its own route and hosted capability. The base, the catalog and the
//! controller's lineage and permission never stand in for either. The
//! attribution keys only that selection; it grants nothing by itself.
use super::*;
use meerkat_authorization_contracts::grant_mutation::{
    ControllerCustodyRefusal, ControllerGrantMutationCustody,
};
use meerkat_authorization_contracts::work_association::{GrantTarget, TargetGrantLineage};
use meerkat_core::authorization::{
    PreparedOperationCheck, ToolAuthorizationFacts, ToolAuthorizationTarget, ToolDelegateRole,
};
use meerkat_core::tool_execution::{BuiltinToolOperation, ToolOperationTarget};

const SEARCH_ENDPOINT: &str = "https://search.invalid/v1/responses";

/// The helper's own route: its own model, credential, profile and endpoint.
fn helper(provider: Provider, capability: ServerToolKind) -> ModelAuthorizationFacts {
    let credential: AuthCredentialIdentity =
        serde_json::from_str(r#"{"realm":"fixture","account":"search-account"}"#)
            .expect("credential identity");
    ModelAuthorizationFacts {
        identity: Arc::new(SessionLlmIdentity {
            model: "search-model".into(),
            provider,
            self_hosted_server_id: None,
            auth_binding: None,
            provider_params: None,
        }),
        wire_model: "search-wire-model".into(),
        hosted_capabilities: Arc::from([capability]),
        backend_profile_id: Some("search-profile".into()),
        backend_kind: "search-backend".into(),
        endpoint: SEARCH_ENDPOINT.into(),
        credential: Some(credential),
        usage: ModelAuthorizationUse::Inference,
        live_channel: None,
    }
}

fn route(facts: &ModelAuthorizationFacts) -> String {
    facts
        .controller_route_key()
        .expect("a complete route")
        .as_str()
        .to_owned()
}

fn builtin() -> GrantTarget {
    GrantTarget::BuiltinToolOperation {
        operation: BuiltinToolOperation::WebSearch,
    }
}

fn delegated_search(route: String, capability: ServerToolKind) -> GrantTarget {
    GrantTarget::DelegatedHostedSearch { route, capability }
}

struct Allowed;
impl meerkat_core::authorization::PreparedOperationAuthorization for Allowed {
    fn review_tier(&self) -> meerkat_core::authorization::OperationReviewTier {
        meerkat_core::authorization::OperationReviewTier::R1
    }

    fn check_current(
        &self,
        _binding: &PreparedAuthorizationBinding,
    ) -> Result<(), meerkat_core::OperationAuthorizationError> {
        Ok(())
    }

    fn spend_at_entry(&self) -> Result<(), meerkat_core::OperationAuthorizationError> {
        Ok(())
    }
}

/// Allows everything: only the selection keys are under test with it.
struct AllowAll;
impl meerkat_core::authorization::WorkAuthorization for AllowAll {
    fn prepare(
        &self,
        _binding: &PreparedAuthorizationBinding,
    ) -> Result<
        Arc<dyn meerkat_core::PreparedOperationAuthorization>,
        meerkat_core::OperationAuthorizationError,
    > {
        Ok(Arc::new(Allowed))
    }
}

/// Admits every original of the fixture's exact scope and records the
/// purposes it is asked about.
#[derive(Default)]
struct RecordingAdmitted(Mutex<Vec<LocalPolicyPurpose>>);
impl AdmittedWorkPolicyOwner for RecordingAdmitted {
    fn authorize_admitted_work(
        &self,
        _association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        _now: u64,
    ) -> Result<WorkOwnerAllowance, meerkat_core::OperationAuthorizationError> {
        self.0.lock().expect("purposes").push(purpose);
        if binding.facts().execution_scope != scope() {
            return Err(denied().into());
        }
        Ok(WorkOwnerAllowance {
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: 1_000,
        })
    }
}

/// Maps the builtin web_search call by its typed target only, and a
/// hosted-search delegate by its own route and hosted capability. Records
/// the purpose of every model operation it is asked about.
#[derive(Default)]
struct SearchOperations {
    helper_unavailable: AtomicBool,
    models: Mutex<Vec<LocalPolicyPurpose>>,
}
impl OperationPolicyOwner for SearchOperations {
    fn authorize_operation(
        &self,
        _association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        _now: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        let operation_values = match &binding.facts().operation {
            AuthorizationOperation::Tool(ToolAuthorizationFacts {
                target: ToolAuthorizationTarget::Dispatcher(plan),
                ..
            }) => match single_tool_operation_target(plan)? {
                Some(ToolOperationTarget::BuiltinOperation {
                    operation: BuiltinToolOperation::WebSearch,
                }) => vec![tuple("web_search", "web")],
                _ => return Err(denied().into()),
            },
            AuthorizationOperation::Model(facts) => {
                self.models.lock().expect("models").push(purpose);
                let delegate = binding.delegate_attribution().ok_or_else(denied)?;
                if delegate.role() != ToolDelegateRole::HostedSearchInference
                    || purpose != LocalPolicyPurpose::Operation
                {
                    return Err(denied().into());
                }
                if self.helper_unavailable.load(Ordering::SeqCst) {
                    return Err(meerkat_core::OperationAuthorizationError::Unavailable);
                }
                let mut values = vec![tuple("infer", "search-models")];
                for capability in facts.hosted_capabilities.iter() {
                    values.push(tuple(
                        match capability {
                            ServerToolKind::GoogleSearch => "google_search",
                            _ => "web_search",
                        },
                        "web",
                    ));
                }
                values
            }
            _ => return Err(denied().into()),
        };
        Ok(LocalPolicyAllowance {
            operation_values,
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: 800,
            review_tier: meerkat_core::authorization::OperationReviewTier::R1,
        })
    }
}

/// No controller work holds these grants.
struct NoCustody;
impl ControllerGrantMutationCustody for NoCustody {
    fn with_unreferenced_controller_grant<T, E>(
        &mut self,
        _: &GrantLineageRef,
        mutate: impl FnOnce() -> Result<T, E>,
    ) -> Result<Result<T, E>, ControllerCustodyRefusal> {
        Ok(mutate())
    }
}

/// A root grant to the fixture's executor for `actions` (every action when
/// empty), on every domain.
fn grant(fixture: &Fixture, id_name: &str, actions: &[&str]) -> GrantLineageRef {
    let actions = if actions.is_empty() {
        ExecutionRestrictions::unrestricted().actions
    } else {
        ExactRestriction::exact(actions.iter().map(|name| action(name)))
    };
    fixture
        .grants
        .issue_root(
            &who("root"),
            id(id_name),
            who("executor"),
            Some(who("subject")),
            ExecutionRestrictions {
                actions,
                ..ExecutionRestrictions::unrestricted()
            },
        )
        .expect("issued grant")
}

/// A targeted selection naming `targets`, in canonical order.
fn selecting(
    base: Option<GrantLineageRef>,
    mut targets: Vec<(GrantTarget, GrantLineageRef)>,
) -> WorkAuthorityBasis {
    targets.sort_by(|left, right| left.0.cmp(&right.0));
    WorkAuthorityBasis::TargetedGrantLineages {
        base: base.map(|grant| vec![grant]),
        targets: targets
            .into_iter()
            .map(|(target, grant)| TargetGrantLineage {
                target,
                lineage: vec![grant],
            })
            .collect(),
    }
}

/// The fixture's original under `basis` and `controller`, with an
/// unrestricted admitted ceiling so only grants bound the operations.
fn work_with(
    fixture: &Fixture,
    basis: WorkAuthorityBasis,
    controller: &GrantLineageRef,
) -> InputAuthorityAssociation {
    let mut candidate = fixture.association().candidate().clone();
    candidate.authority_basis = basis;
    candidate.controller_grant_lineage = vec![controller.clone()];
    candidate.admitted_ceiling = ExecutionRestrictions::unrestricted();
    InputAuthorityAssociation::new(candidate).expect("association")
}

fn search_policy(
    fixture: &Fixture,
) -> (
    Arc<GrantBackedWorkPolicy>,
    Arc<RecordingAdmitted>,
    Arc<SearchOperations>,
) {
    let admitted = Arc::new(RecordingAdmitted::default());
    let operations = Arc::new(SearchOperations::default());
    (
        Arc::new(GrantBackedWorkPolicy::new(
            fixture.grants.clone(),
            admitted.clone(),
            operations.clone(),
        )),
        admitted,
        operations,
    )
}

/// The builtin web_search call dispatched for real under `context`, and its
/// delegate's preparation of `facts`.
async fn delegate(
    context: &WorkAuthorizationContext,
    facts: ModelAuthorizationFacts,
) -> Result<PreparedOperationCheck, meerkat_core::OperationAuthorizationError> {
    let attribution = meerkat_core::authorization::__test_mint_tool_delegate(
        context,
        None,
        ToolDelegateRole::HostedSearchInference,
    )
    .await
    .expect("the call itself is allowed, so its dispatch mints the delegate");
    meerkat_core::LlmRequestAuthorization::for_tool_delegate(attribution).prepare(facts)
}

fn is_denied(
    result: &Result<PreparedOperationCheck, meerkat_core::OperationAuthorizationError>,
) -> bool {
    matches!(
        result,
        Err(meerkat_core::OperationAuthorizationError::Refused(refused))
            if refused.kind() == OperationRefusalKind::Denied
    )
}

/// The builtin call selects its own builtin target and the delegate its own
/// route and hosted capability, keyed from the helper's own facts (a Gemini
/// helper's grounding is its own capability); an ordinary inference with
/// identical facts carries no attribution and keeps the base.
#[tokio::test]
async fn the_builtin_call_and_its_delegate_select_their_own_targets() {
    let work = WorkAuthorizationContext::new(Arc::new(AllowAll), scope());
    let attribution = meerkat_core::authorization::__test_mint_tool_delegate(
        &work,
        None,
        ToolDelegateRole::HostedSearchInference,
    )
    .await
    .expect("minted");
    assert_eq!(
        operation_grant_target(attribution.parent_binding()),
        Ok(Some(builtin()))
    );
    let openai = helper(Provider::OpenAI, ServerToolKind::WebSearch);
    let search = meerkat_core::LlmRequestAuthorization::for_tool_delegate(attribution.clone())
        .prepare(openai.clone())
        .expect("the delegate");
    assert_eq!(
        operation_grant_target(search.binding()),
        Ok(Some(delegated_search(
            route(&openai),
            ServerToolKind::WebSearch
        )))
    );
    assert_ne!(
        route(&openai),
        route(&model(ModelAuthorizationUse::ControllerInference, vec![])),
        "the helper's own route, not the controller's"
    );
    let gemini = helper(Provider::Gemini, ServerToolKind::GoogleSearch);
    let grounded = meerkat_core::LlmRequestAuthorization::for_tool_delegate(attribution)
        .prepare(gemini.clone())
        .expect("the delegate");
    assert_eq!(
        operation_grant_target(grounded.binding()),
        Ok(Some(delegated_search(
            route(&gemini),
            ServerToolKind::GoogleSearch
        )))
    );
    assert_ne!(route(&gemini), route(&openai));
    let plain = meerkat_core::LlmRequestAuthorization::new(
        work,
        meerkat_core::OperationId::new(),
        ModelAuthorizationUse::Inference,
    )
    .prepare(openai)
    .expect("an ordinary inference");
    assert!(plain.binding().delegate_attribution().is_none());
    assert_eq!(operation_grant_target(plain.binding()), Ok(None));
}

/// A delegate is allowed by the lineage selected for exactly its route and
/// capability. Base only, another route or another capability: Denied by
/// the grant owner, and the operation owner is never asked about the
/// helper, although the broad base would cover its tuples.
#[tokio::test]
async fn a_delegated_search_resolves_only_its_own_route_and_capability_lineage() {
    let fixture = Fixture::new();
    let call = grant(&fixture, "builtin-search", &["web_search"]);
    let broad = grant(&fixture, "broad-base", &[]);
    let selected = grant(&fixture, "delegated-search", &["infer", "web_search"]);
    let openai = helper(Provider::OpenAI, ServerToolKind::WebSearch);
    let other_route = route(&helper(Provider::Anthropic, ServerToolKind::WebSearch));
    assert_ne!(other_route, route(&openai));
    let (policy, _admitted, operations) = search_policy(&fixture);
    let helper_result = |targets: Vec<(GrantTarget, GrantLineageRef)>| {
        let association = work_with(
            &fixture,
            selecting(Some(broad.clone()), targets),
            &fixture.controller,
        );
        let context = fixture.context(&policy, &association);
        let facts = openai.clone();
        async move { delegate(&context, facts).await }
    };

    helper_result(vec![
        (builtin(), call.clone()),
        (
            delegated_search(route(&openai), ServerToolKind::WebSearch),
            selected.clone(),
        ),
    ])
    .await
    .expect("its own route and capability's lineage allows it");
    assert_eq!(
        operations.models.lock().expect("models").as_slice(),
        [LocalPolicyPurpose::Operation]
    );

    for (label, targets) in [
        ("the base alone", vec![(builtin(), call.clone())]),
        (
            "another route",
            vec![
                (builtin(), call.clone()),
                (
                    delegated_search(other_route.clone(), ServerToolKind::WebSearch),
                    selected.clone(),
                ),
            ],
        ),
        (
            "another capability",
            vec![
                (builtin(), call.clone()),
                (
                    delegated_search(route(&openai), ServerToolKind::GoogleSearch),
                    selected.clone(),
                ),
            ],
        ),
    ] {
        let result = helper_result(targets).await;
        assert!(is_denied(&result), "{label}: {result:?}");
        assert_eq!(
            operations.models.lock().expect("models").len(),
            1,
            "{label}: the operation owner is not asked about the helper"
        );
    }
}

/// A helper on exactly the admitted controller's route is still only an
/// operation: without its own delegate selection it is Denied before the
/// operation owner, even with a controller grant naming that route; the
/// Controller purpose refuses it before the work owner; with a selection the
/// owners see only the Operation purpose. Revoking the controller grant
/// leaves it allowed; revoking its own grant refuses it.
#[tokio::test]
async fn a_delegated_search_never_uses_the_controller_route_permission() {
    let fixture = Fixture::new();
    let same_route = model(
        ModelAuthorizationUse::Inference,
        vec![ServerToolKind::WebSearch],
    );
    let key = route(&same_route);
    assert_eq!(
        key,
        meerkat_core::ControllerModelFacts::new(
            selection(),
            same_route.endpoint.clone(),
            same_route.wire_model.clone(),
        )
        .route_key()
        .as_str(),
        "the helper runs on exactly the admitted controller's route"
    );
    let call = grant(&fixture, "builtin-search", &["web_search"]);
    let broad = grant(&fixture, "broad-base", &[]);
    let controller = fixture
        .grants
        .issue_root(
            &who("root"),
            id("controller-route"),
            who("executor"),
            Some(who("subject")),
            ExecutionRestrictions {
                resource_domains: DomainRestriction::exact([domain(&key)]),
                ..ExecutionRestrictions::unrestricted()
            },
        )
        .expect("a controller grant naming the route");
    let own = grant(&fixture, "delegated-search", &["infer", "web_search"]);
    let (policy, admitted, operations) = search_policy(&fixture);
    let unselected = work_with(
        &fixture,
        selecting(Some(broad.clone()), vec![(builtin(), call.clone())]),
        &controller,
    );
    let result = delegate(&fixture.context(&policy, &unselected), same_route.clone()).await;
    assert!(is_denied(&result), "{result:?}");
    assert!(
        operations.models.lock().expect("models").is_empty(),
        "the operation owner is never asked"
    );

    let selected = work_with(
        &fixture,
        selecting(
            Some(broad.clone()),
            vec![
                (builtin(), call.clone()),
                (
                    delegated_search(key.clone(), ServerToolKind::WebSearch),
                    own.clone(),
                ),
            ],
        ),
        &controller,
    );
    let before = admitted.0.lock().expect("purposes").len();
    let prepared = delegate(&fixture.context(&policy, &selected), same_route.clone())
        .await
        .expect("its own selection allows it");
    assert_eq!(
        operations.models.lock().expect("models").as_slice(),
        [LocalPolicyPurpose::Operation],
        "the operation owner sees only the Operation purpose"
    );
    let asked = admitted.0.lock().expect("purposes").clone();
    assert!(
        asked[before..]
            .iter()
            .all(|purpose| *purpose == LocalPolicyPurpose::Operation),
        "{asked:?}"
    );
    let before = admitted.0.lock().expect("purposes").len();
    assert!(matches!(
        policy.evaluate_for(
            &selected,
            prepared.binding(),
            LocalPolicyPurpose::Controller,
            100,
        ),
        Err(meerkat_core::OperationAuthorizationError::Refused(refused))
            if refused.kind() == OperationRefusalKind::Denied
    ));
    assert_eq!(
        admitted.0.lock().expect("purposes").len(),
        before,
        "the Controller purpose is refused before the work owner"
    );

    fixture
        .grants
        .revoke(&who("root"), &controller, &mut NoCustody)
        .expect("revoke the controller grant");
    delegate(&fixture.context(&policy, &selected), same_route.clone())
        .await
        .expect("the controller grant never authorized it");
    fixture
        .grants
        .revoke(&who("root"), &own, &mut NoCustody)
        .expect("revoke its own grant");
    let result = delegate(&fixture.context(&policy, &selected), same_route).await;
    assert!(is_denied(&result), "{result:?}");
}

/// An owner that cannot decide the helper now is Unavailable after it was
/// asked once. A helper request whose route has no key (no credential
/// owner) is Unavailable before any lineage is selected or the operation
/// owner is asked; with no selection for it, a selected key would have been
/// Denied instead.
#[tokio::test]
async fn a_delegated_search_unavailable_names_its_guard() {
    let fixture = Fixture::new();
    let call = grant(&fixture, "builtin-search", &["web_search"]);
    let own = grant(&fixture, "delegated-search", &["infer", "web_search"]);
    let openai = helper(Provider::OpenAI, ServerToolKind::WebSearch);
    let (policy, _admitted, operations) = search_policy(&fixture);
    let selected = work_with(
        &fixture,
        selecting(
            None,
            vec![
                (builtin(), call.clone()),
                (
                    delegated_search(route(&openai), ServerToolKind::WebSearch),
                    own,
                ),
            ],
        ),
        &fixture.controller,
    );
    operations.helper_unavailable.store(true, Ordering::SeqCst);
    let result = delegate(&fixture.context(&policy, &selected), openai.clone()).await;
    assert!(
        matches!(
            result,
            Err(meerkat_core::OperationAuthorizationError::Unavailable)
        ),
        "{result:?}"
    );
    assert_eq!(
        operations.models.lock().expect("models").len(),
        1,
        "the owner was asked once"
    );
    operations.helper_unavailable.store(false, Ordering::SeqCst);

    let mut keyless = openai;
    keyless.credential = None;
    let unselected = work_with(
        &fixture,
        selecting(None, vec![(builtin(), call)]),
        &fixture.controller,
    );
    let result = delegate(&fixture.context(&policy, &unselected), keyless).await;
    assert!(
        matches!(
            result,
            Err(meerkat_core::OperationAuthorizationError::Unavailable)
        ),
        "no route key: {result:?}"
    );
    assert_eq!(
        operations.models.lock().expect("models").len(),
        1,
        "the operation owner is not asked without a route key"
    );
}

/// Already-entered-call semantics: revoking the call's own lineage refuses
/// the next call at its own preparation, but the helper of an invocation
/// that already entered is decided by its own lineage only. Revoking the
/// helper's own lineage stops a helper already prepared at its entry check.
#[tokio::test]
async fn revoking_the_call_lineage_keeps_an_entered_invocations_helper() {
    let fixture = Fixture::new();
    let call = grant(&fixture, "builtin-search", &["web_search"]);
    let own = grant(&fixture, "delegated-search", &["infer", "web_search"]);
    let openai = helper(Provider::OpenAI, ServerToolKind::WebSearch);
    let (policy, _admitted, _operations) = search_policy(&fixture);
    let association = work_with(
        &fixture,
        selecting(
            None,
            vec![
                (builtin(), call.clone()),
                (
                    delegated_search(route(&openai), ServerToolKind::WebSearch),
                    own.clone(),
                ),
            ],
        ),
        &fixture.controller,
    );
    let context = fixture.context(&policy, &association);
    let entered = meerkat_core::authorization::__test_mint_tool_delegate(
        &context,
        None,
        ToolDelegateRole::HostedSearchInference,
    )
    .await
    .expect("the call entered and minted its delegate");
    fixture
        .grants
        .revoke(&who("root"), &call, &mut NoCustody)
        .expect("revoke the call's own lineage");
    assert!(
        matches!(
            meerkat_core::authorization::__test_mint_tool_delegate(
                &context,
                None,
                ToolDelegateRole::HostedSearchInference,
            )
            .await,
            Err(meerkat_core::ToolError::AuthorizationRefused { refusal })
                if refusal.kind() == OperationRefusalKind::Denied
        ),
        "the next call is refused at its own preparation"
    );
    let prepared = meerkat_core::LlmRequestAuthorization::for_tool_delegate(entered)
        .prepare(openai)
        .expect("the entered invocation's helper is decided by its own lineage");
    prepared
        .current()
        .expect("still current at its own entry check");
    fixture
        .grants
        .revoke(&who("root"), &own, &mut NoCustody)
        .expect("revoke the helper's own lineage");
    let result = prepared.current();
    assert!(
        is_denied(&result),
        "a prepared helper is refused at its entry check: {result:?}"
    );
}

/// One restriction variant of a selected lineage: its label, its bounds,
/// the decision time and whether the operation is allowed under it.
type Bound = (&'static str, ExecutionRestrictions, u64, bool);

/// `within` and one variant per restriction dimension: time (decided after
/// the lifetime ends), resource domain, private data (another processor,
/// another audience) and operation (a read-only action set that lacks the
/// operation's actions; read-only is an action set, as
/// `ExecutionRestrictions` has no read-only flag) each exclude the
/// operation. Delegation depth bounds only re-delegation, so a lineage that
/// can delegate no further still allows it.
fn bounds(within: ExecutionRestrictions) -> Vec<Bound> {
    vec![
        ("within every bound", within.clone(), 100, true),
        ("after its lifetime", within.clone(), 200, false),
        (
            "another resource domain",
            ExecutionRestrictions {
                resource_domains: DomainRestriction::exact([domain("elsewhere")]),
                ..within.clone()
            },
            100,
            false,
        ),
        (
            "no further delegation",
            ExecutionRestrictions {
                delegation_depth: DelegationDepth::remaining(0),
                ..within.clone()
            },
            100,
            true,
        ),
        (
            "another processor",
            ExecutionRestrictions {
                processors: ExactRestriction::exact([ProcessorRef::Principal {
                    principal: who("elsewhere"),
                }]),
                ..within.clone()
            },
            100,
            false,
        ),
        (
            "another audience",
            ExecutionRestrictions {
                audiences: ExactRestriction::exact([AudienceRef::Principal {
                    principal: who("someone-else"),
                }]),
                ..within.clone()
            },
            100,
            false,
        ),
        (
            "a read-only action set",
            ExecutionRestrictions {
                actions: ExactRestriction::exact([action("read")]),
                ..within
            },
            100,
            false,
        ),
    ]
}

/// Choosing a target never erases a restriction (ruling Q5): the call's and
/// the delegate's selected lineages carry their own time, resource-domain,
/// operation and private-data bounds, and each bound refuses its operation
/// although the broad base would allow it. An allowed delegate expires with the earliest
/// of its own lineage's lifetime, the operation owner's allowance and the
/// admitted work.
#[tokio::test]
async fn a_selected_lineage_keeps_its_time_operation_and_data_restrictions() {
    let fixture = Fixture::new();
    let broad = grant(&fixture, "broad-base", &[]);
    let openai = helper(Provider::OpenAI, ServerToolKind::WebSearch);
    let (policy, _admitted, _operations) = search_policy(&fixture);
    let issue = |name: String, restrictions: ExecutionRestrictions| {
        fixture
            .grants
            .issue_root(
                &who("root"),
                id(&name),
                who("executor"),
                Some(who("subject")),
                restrictions,
            )
            .expect("issued grant")
    };
    let within = |actions: &[&str], domains: &[&str]| ExecutionRestrictions {
        actions: ExactRestriction::exact(actions.iter().map(|name| action(name))),
        resource_domains: DomainRestriction::exact(domains.iter().map(|name| domain(name))),
        processors: ExactRestriction::exact([ProcessorRef::Principal {
            principal: who("executor"),
        }]),
        audiences: ExactRestriction::exact([AudienceRef::Principal {
            principal: who("requester"),
        }]),
        lifetime: LifetimeRestriction::window(50, 150),
        ..ExecutionRestrictions::unrestricted()
    };
    let open_call = || ExecutionRestrictions {
        actions: ExactRestriction::exact([action("web_search")]),
        ..ExecutionRestrictions::unrestricted()
    };
    let at = |now_ms: u64| fixture.clock.0.store(now_ms, Ordering::SeqCst);

    for (index, (label, call_bounds, now_ms, allowed)) in bounds(within(&["web_search"], &["web"]))
        .into_iter()
        .enumerate()
    {
        at(100);
        let association = work_with(
            &fixture,
            selecting(
                Some(broad.clone()),
                vec![(builtin(), issue(format!("call-{index}"), call_bounds))],
            ),
            &fixture.controller,
        );
        let context = fixture.context(&policy, &association);
        at(now_ms);
        let minted = meerkat_core::authorization::__test_mint_tool_delegate(
            &context,
            None,
            ToolDelegateRole::HostedSearchInference,
        )
        .await;
        if allowed {
            minted.expect("the call within every bound is allowed");
        } else {
            assert!(
                matches!(
                    &minted,
                    Err(meerkat_core::ToolError::AuthorizationRefused { refusal })
                        if refusal.kind() == OperationRefusalKind::Denied
                ),
                "the call {label}: {minted:?}"
            );
        }
    }

    for (index, (label, delegate_bounds, now_ms, allowed)) in
        bounds(within(&["infer", "web_search"], &["search-models", "web"]))
            .into_iter()
            .enumerate()
    {
        at(100);
        let association = work_with(
            &fixture,
            selecting(
                Some(broad.clone()),
                vec![
                    (builtin(), issue(format!("open-call-{index}"), open_call())),
                    (
                        delegated_search(route(&openai), ServerToolKind::WebSearch),
                        issue(format!("delegate-{index}"), delegate_bounds),
                    ),
                ],
            ),
            &fixture.controller,
        );
        let context = fixture.context(&policy, &association);
        let attribution = meerkat_core::authorization::__test_mint_tool_delegate(
            &context,
            None,
            ToolDelegateRole::HostedSearchInference,
        )
        .await
        .expect("the call itself is allowed");
        at(now_ms);
        let result = meerkat_core::LlmRequestAuthorization::for_tool_delegate(attribution)
            .prepare(openai.clone());
        if allowed {
            let prepared = result.expect("the delegate within every bound is allowed");
            assert_eq!(
                prepared
                    .validity()
                    .expect("a bounded decision")
                    .expires_at_ms,
                150,
                "the earliest bound is the selected lineage's own lifetime"
            );
        } else {
            assert!(is_denied(&result), "the delegate {label}: {result:?}");
        }
    }
    at(100);
}

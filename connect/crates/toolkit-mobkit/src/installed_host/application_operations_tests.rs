//! Real operator review, durable native grants, retained Toolkit source and
//! native work compiler. No provider, native input admission, controller pin,
//! effect execution or resumed run is exercised by these component tests.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use meerkat_authorization_contracts::constraints::DomainRestriction;
use super::operator_setup::{
    OperatorOwnerStartup, OperatorParentOutcome, OperatorParentProposal, OperatorParentPurpose,
    OperatorSetup,
};
use super::*;
use meerkat_authorization::grant_policy::GrantBackedWorkPolicy;
use meerkat_authorization_contracts::{
    constraints::{ActionRef, AudienceRef, DelegationDepth, ExactRestriction, ProcessorRef},
    evidence::{EvidenceDigest, EvidenceId, HistoricalEvidenceRef},
    protocol::ContractRequirements,
    resource::ResourceRef,
    work_association::{
        GrantLineageRef, InputAuthorityAssociationCandidate, NativeWorkTarget, OriginalWorkRef,
        QualifiedIngressNamespace, TargetGrantLineage,
    },
};
use meerkat_core::{
    ApprovalActionKind, ApprovalDecision, ApprovalOwnerRef, ApprovalPrincipalId,
    ApprovalProposedAction, ApprovalRecord, ApprovalRequest, ApprovalResourceId,
    ApprovalResourceKind, ApprovalResourceRef, ApprovalRisk, ApprovalService, ControllerModelFacts,
    ControllerModelSelection, Provider, SessionId, SessionLlmIdentity,
    authorization::{
        ModelAuthorizationFacts, OperationAuthorizationFacts,
        SourceAuthorizationFacts, WorkAuthorizationContext,
    },
    connection::{AuthCredentialIdentity, CredentialAccountId, CredentialAccountRef},
    exact_operation::OperationExecutionScope,
    memory::MemorySearchScope,
};
use meerkat_runtime::input::{Input, PromptInput};
use meerkat_toolkit::host_lifetime::{
    control::{HostCloseReason, PrivateHostLifetimeControl},
    policy::CompiledLocalPolicy,
};

fn id(value: &str) -> EvidenceId {
    EvidenceId::new(value).unwrap()
}

fn model() -> ModelAuthorizationFacts {
    ModelAuthorizationFacts {
        identity: Arc::new(SessionLlmIdentity {
            model: "reviewed-model".into(),
            provider: Provider::OpenAI,
            self_hosted_server_id: None,
            auth_binding: None,
            provider_params: None,
        }),
        wire_model: "reviewed-wire-model".into(),
        hosted_capabilities: Vec::new().into(),
        backend_profile_id: Some("reviewed-profile".into()),
        backend_kind: "reviewed-backend".into(),
        endpoint: "https://reviewed.invalid/v1/responses".into(),
        credential: Some(AuthCredentialIdentity::Account(CredentialAccountRef {
            realm: meerkat_core::mob_realm_id("factory").unwrap(),
            account: CredentialAccountId::parse("reviewed-account").unwrap(),
        })),
        usage: ModelAuthorizationUse::ControllerInference,
        live_channel: None,
    }
}

fn selected(facts: &ModelAuthorizationFacts) -> ControllerModelSelection {
    ControllerModelSelection::new(
        facts.identity.as_ref().clone(),
        facts.credential.clone().unwrap(),
        facts.backend_profile_id.as_ref().unwrap().to_string(),
        facts.backend_kind.to_string(),
    )
}

fn plain(facts: &ModelAuthorizationFacts) -> ControllerModelFacts {
    ControllerModelFacts::new(
        selected(facts),
        facts.endpoint.clone(),
        facts.wire_model.clone(),
    )
}

fn request() -> ApprovalRequest {
    ApprovalRequest {
        requester: ApprovalPrincipalId::new("application-setup-fixture").unwrap(),
        owner: ApprovalOwnerRef::Session {
            session_id: SessionId::new(),
        },
        resource: ApprovalResourceRef {
            kind: ApprovalResourceKind::Runtime,
            id: ApprovalResourceId::new("selected-application"),
        },
        proposed_action: ApprovalProposedAction {
            kind: ApprovalActionKind::Other,
            summary: "owner supplies the exact reviewed access".into(),
            body: None,
        },
        risk: ApprovalRisk::High,
        request_body: None,
        allowed_decisions: [ApprovalDecision::Approve, ApprovalDecision::Deny]
            .into_iter()
            .collect(),
        expires_at: None,
        metadata: Default::default(),
        request_provenance: None,
    }
}

fn approve(proposal: &OperatorParentProposal) -> ApprovalRecord {
    // Actual native decision custody, not a JSON body decoded as authority.
    // The protected terminal/panel presenter is not executed by this fixture.
    let approvals = ApprovalService::new();
    let pending = approvals.request(proposal.request().clone()).unwrap();
    approvals
        .decide(
            &pending.approval_id,
            ApprovalDecision::Approve,
            ApprovalPrincipalId::new("protected-presenter-fixture").unwrap(),
            None,
            None,
        )
        .unwrap()
}

async fn issue(
    setup: &OperatorSetup,
    proposal: OperatorParentProposal,
    purpose: OperatorParentPurpose,
) -> Vec<GrantLineageRef> {
    let approval = approve(&proposal);
    match setup
        .apply_parent_decision(proposal, approval)
        .await
        .unwrap()
    {
        OperatorParentOutcome::Selected {
            route,
            purpose: selected,
            lineage,
        } => {
            assert_eq!(route, toolkit_package::Id::new("console").unwrap());
            assert_eq!(selected, purpose);
            lineage
        }
        _ => panic!("approved native parent was not selected"),
    }
}

/// The only operation parent is a human-reviewed own-session read. It does
/// not grant a model action or a second memory scope. This is fixture policy,
/// not a duplicate of the production operation classifier.
fn memory_bounds(native: &CompiledNativeHost, session: &SessionId) -> ExecutionRestrictions {
    ExecutionRestrictions {
        actions: ExactRestriction::exact([ActionRef {
            feature: "toolkit".into(),
            action: "memory-read".into(),
        }]),
        resource_domains: DomainRestriction::exact([ResourceDomain {
            authority: native.grant_administrator.clone(),
            namespace: format!("toolkit-session-memory/{session}"),
        }]),
        processors: ExactRestriction::exact([ProcessorRef::Principal {
            principal: native.routes[0].logical_executor.clone(),
        }]),
        audiences: ExactRestriction::exact([native.members[0].audience.clone()]),
        delegation_depth: DelegationDepth::remaining(1),
        ..ExecutionRestrictions::unrestricted()
    }
}

struct Fixture {
    _directory: tempfile::TempDir,
    lifetime: Arc<NativeHostLifetime>,
    source: Arc<ToolkitOriginalOwner>,
    retained: Arc<crate::original_ingress::RetainedToolkitOriginal>,
    reference: crate::private_originals::PrivateOriginalRef,
    application: Arc<InstalledApplicationPolicy>,
    policy: Arc<GrantBackedWorkPolicy>,
    association: InputAuthorityAssociation,
    context: WorkAuthorizationContext,
    session: SessionId,
    facts: ModelAuthorizationFacts,
}

impl Fixture {
    async fn new() -> Self {
        Self::with_operations(memory_bounds).await
    }

    async fn with_operations(
        bounds: impl FnOnce(&CompiledNativeHost, &SessionId) -> ExecutionRestrictions,
    ) -> Self {
        Self::with_operations_review(bounds, false, false).await
    }

    async fn with_setup_request() -> Self {
        Self::with_operations_review(setup_request_bounds, true, false).await
    }

    async fn with_targeted_setup() -> Self {
        Self::with_operations_review_basis(setup_request_bounds, true, false, true).await
    }

    async fn with_operations_review(
        bounds: impl FnOnce(&CompiledNativeHost, &SessionId) -> ExecutionRestrictions,
        setup_request_review: bool,
        frontend_source: bool,
    ) -> Self {
        Self::with_operations_review_basis(bounds, setup_request_review, frontend_source, false).await
    }

    async fn with_operations_review_basis(
        bounds: impl FnOnce(&CompiledNativeHost, &SessionId) -> ExecutionRestrictions,
        setup_request_review: bool,
        frontend_source: bool,
        targeted_setup: bool,
    ) -> Self {
        let (directory, mut selection) = super::tests::fixture();
        let mut local: CompiledLocalPolicy =
            serde_json::from_slice(&fs::read(&selection.local_policy.path).unwrap()).unwrap();
        if frontend_source {
            let (member, requester, actor, realm) = {
                let native = local.native_host.as_ref().unwrap();
                let (member, requester, actor) = native.routes[0].source_binding(&local).unwrap();
                (member.clone(), requester.clone(), actor.clone(), native.realm.clone())
            };
            let component = toolkit_package::Id::new("frontend").unwrap();
            local.components = vec![component.clone()];
            local.channels = vec![meerkat_toolkit::host_lifetime::policy::CompiledChannelBinding {
                component, channel_id: "fixture-channel".into(), service_subject: "fixture-service".into(),
                provider_account: "fixture-account".into(), sender_id: "fixture-sender".into(),
                requester, ingress_actor: actor, target_identity: "agent".into(),
                member, realm, required_surfaces: vec!["channel:fixture-channel".into()],
            }];
            local.native_host.as_mut().unwrap().routes[0].origin =
                meerkat_toolkit::host_lifetime::policy::CompiledNativeSourceOrigin::ProviderEvent {
                    channel_id: "fixture-channel".into(), sender_id: "fixture-sender".into(), target_identity: "agent".into(),
                };
            super::tests::replace_local(&mut selection, &local);
        }
        let native = local.native_host.as_mut().unwrap();
        let session = SessionId::new();
        let facts = model();
        let operations = bounds(native, &session);
        let setup = OperatorSetup::open(
            selection.clone(),
            OperatorOwnerStartup::Initialize {
                namespace: id("application-policy-tests"),
                generation: 1,
            },
        )
        .unwrap();
        let controller_proposal = setup
            .prepare_controller_parent_review(
                toolkit_package::Id::new("console").unwrap(),
                &plain(&facts),
                request(),
            )
            .unwrap();
        let controller_parent = issue(
            &setup,
            controller_proposal,
            OperatorParentPurpose::Controller,
        )
        .await;
        let operator =
            meerkat::governed_realm::stock::StockPrincipals::for_local_operator(&native.realm)
                .unwrap()
                .operator;
        assert_eq!(controller_parent[0].root_authority, operator);
        assert_ne!(operator, native.grant_administrator);

        let operation_proposal = if setup_request_review {
            let proposal = setup
                .prepare_setup_parent_review(
                    toolkit_package::Id::new("console").unwrap(),
                    &session,
                    request(),
                )
                .unwrap();
            assert_eq!(
                proposal.request().proposed_action.body.as_ref().unwrap()["setup_request"]["session"],
                session.to_string()
            );
            proposal
        } else {
            setup
                .prepare_parent_review(
                    toolkit_package::Id::new("console").unwrap(),
                    operations.clone(),
                    request(),
                )
                .unwrap()
        };
        let operation_parent = issue(
            &setup,
            operation_proposal,
            if setup_request_review { OperatorParentPurpose::SetupRequest } else { OperatorParentPurpose::Operations },
        )
        .await;
        // The component fixture selects a populated base and a distinct find
        // target from independently reviewed native parents. Full configured
        // setup production is exercised by the existing selection controls.
        let base_parent = if targeted_setup {
            let proposal = setup.prepare_parent_review(
                toolkit_package::Id::new("console").unwrap(),
                memory_bounds(native, &session), request(),
            ).unwrap();
            Some(issue(&setup, proposal, OperatorParentPurpose::Operations).await)
        } else { None };
        // Release actual operator owner/store exclusion before ordinary restore.
        drop(setup);
        native.routes[0].application_parent = Some(operation_parent.clone());
        native.routes[0].controller_parent = Some(controller_parent.clone());
        let encoded = serde_json::to_vec(&local).unwrap();
        let roundtrip: CompiledLocalPolicy = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(roundtrip, local);
        let native = roundtrip.native_host.as_ref().unwrap().clone();
        super::tests::replace_local(&mut selection, &local);

        let mut lifetime = NativeHostLifetime::bootstrap(selection.clone()).unwrap();
        let path = state_directory(&selection, &native, NativeOwnerStartup::Restore).unwrap();
        let source = Arc::new(
            ToolkitOriginalOwner::restore(
                Arc::new(FileGrantOwnerStore::new(path.join("grants.jsonl"))),
                Arc::new(HostAuthorizationClock),
                native.ingress_domain.clone(),
            )
            .unwrap(),
        );
        lifetime
            .bind_native_authorization_publication(source.publication())
            .unwrap();
        let lifetime = Arc::new(lifetime);
        if frontend_source {
            lifetime.open_component(&toolkit_package::Id::new("frontend").unwrap()).unwrap();
        }
        let route = &native.routes[0];
        let (_, requester, actor) = route.source_binding(&local).unwrap();
        let originals = Arc::new(
            PrivateOriginals::new(
                StoreScope {
                    realm: native.realm.to_string(),
                    owner: local.deployment.to_string(),
                },
                Arc::new(SqliteStore::open_existing(path.join("originals.sqlite3")).unwrap()),
                source.clone(),
                vec![PrivateOriginalRoute {
                    id: route.id.clone(),
                    grant_caller: native.grant_administrator.clone(),
                    application_parent: route.application_parent.clone(),
                    controller_parent: route.controller_parent.clone(),
                    grant_target: toolkit_package::Id::new("agent").unwrap(),
                    grant_audience: native.members[0].audience.clone(),
                    realm: native.realm.clone(),
                    requester: requester.clone(),
                    ingress_actor: actor.clone(),
                    logical_executor: route.logical_executor.clone(),
                    represented_subject: route.represented_subject.clone(),
                    source_authority: route.source_authority.clone(),
                    authentication_domain: route.authentication_domain.clone(),
                    event_domain: route.event_domain.clone(),
                }],
            )
            .await
            .unwrap(),
        );
        let child = |parent: &[GrantLineageRef], name: &str| {
            let leaf = source
                .grants()
                .issue_child(
                    &native.grant_administrator,
                    parent.last().unwrap(),
                    id(name),
                    route.logical_executor.clone(),
                    ExecutionRestrictions {
                        delegation_depth: DelegationDepth::remaining(0),
                        ..ExecutionRestrictions::unrestricted()
                    },
                )
                .unwrap();
            let mut lineage = parent.to_vec();
            lineage.push(leaf);
            lineage
        };
        let selected_controller = route.controller_parent.as_deref().unwrap();
        let selected_operations = route.application_parent.as_deref().unwrap();
        assert_ne!(selected_controller, selected_operations);
        let controller_lineage = child(selected_controller, "controller-agent");
        let operation_lineage = child(selected_operations, "operation-agent");
        let operation_basis = if let Some(parent) = &base_parent {
            WorkAuthorityBasis::TargetedGrantLineages {
                base: Some(child(parent, "base-agent")),
                targets: vec![TargetGrantLineage {
                    target: super::operator_setup::setup_targets::find_target(),
                    lineage: operation_lineage,
                }],
            }
        } else { WorkAuthorityBasis::GrantLineage { lineage: operation_lineage } };
        let selected_parent = source
            .grants()
            .resolve_lineage(selected_controller, &native.grant_administrator, None)
            .unwrap();
        assert_eq!(
            selected_parent.restrictions().resource_domains,
            DomainRestriction::exact([ResourceDomain {
                authority: native.grant_administrator.clone(),
                namespace: format!("toolkit-model-route/{}", plain(&facts).route_key()),
            }])
        );
        drop(selected_parent);
        // A parsed selected reference is evidence only. The actual restored
        // native owner rejects a changed leaf and a different application.
        let mut forged = selected_controller.to_vec();
        forged.last_mut().unwrap().grant_id = id("unissued-controller-parent");
        assert!(
            source
                .grants()
                .resolve_lineage(&forged, &native.grant_administrator, None,)
                .is_err()
        );
        assert!(
            source
                .grants()
                .resolve_lineage(selected_controller, &route.logical_executor, None,)
                .is_err()
        );
        let controller_ceiling = source
            .grants()
            .resolve_controller_lineage(&controller_lineage, &route.logical_executor, None)
            .unwrap()
            .restrictions()
            .clone();
        let mut event = crate::original_ingress::tests::private_event("policy-input");
        event.route = route.id.clone();
        let reference = originals.retain_event(event.clone()).await.unwrap();
        let evidence = |domain: &ResourceDomain| HistoricalEvidenceRef {
            resource: ResourceRef {
                domain: domain.clone(),
                resource_id: "policy-input".into(),
            },
            revision: id("1"),
            digest: EvidenceDigest::of_bytes(b"policy-input"),
        };
        let association = InputAuthorityAssociation::new(InputAuthorityAssociationCandidate {
            requester: requester.clone(),
            ingress_actor: actor.clone(),
            represented_subject: None,
            original_authentication: evidence(&route.authentication_domain),
            logical_executor: route.logical_executor.clone(),
            target: NativeWorkTarget {
                logical_owner: native.members[0].logical_owner.clone(),
                logical_runtime: id(
                    &meerkat_runtime::LogicalRuntimeId::for_session(&session).to_string()
                ),
                context: id(&session.to_string()),
                context_generation: native.members[0].context_generation,
                audience: native.members[0].audience.clone(),
            },
            original_work: OriginalWorkRef {
                authority: route.source_authority.clone(),
                work: event.original,
            },
            root_event: evidence(&route.event_domain),
            contributing_work: vec![],
            authority_basis: operation_basis,
            controller_grant_lineage: controller_lineage,
            controller_model: Some(selected(&facts)),
            controller_ceiling,
            admitted_ceiling: if targeted_setup { ExecutionRestrictions::unrestricted() } else { operations },
            source_observations: vec![],
            ingress_namespace: QualifiedIngressNamespace {
                realm: native.realm.clone(),
                ingress: native.ingress_domain.clone(),
                occurrence_scope: event.occurrence,
            },
            contract: ContractRequirements::local_governed_v1(),
        })
        .unwrap();
        let persist_originals = &originals;
        let persist_reference = &reference;
        let retained = source
            .bind_issued_access_async(
                association.clone(),
                crate::original_ingress::tests::source(&association),
                requester,
                actor,
                targeted_setup.then_some(&native.grant_administrator),
                |association| async move {
                    persist_originals
                        .persist_binding(persist_reference, &association)
                        .await
                },
            )
            .await
            .unwrap();
        originals.hold_binding(reference.clone(), retained.clone()).await.unwrap();
        let application = Arc::new(InstalledApplicationPolicy {
            originals,
            lifetime: lifetime.clone(),
            native,
            selected: local,
        });
        // Consume the same real composition used by E1, including its native
        // source/contributor wrapper. No permissive fixture owner is installed.
        let configuration = source.configuration(
            ScopedInvocation::from_policy(application.clone()),
            application.clone(),
            None,
        );
        let policy = Arc::new(GrantBackedWorkPolicy::new(
            configuration.grants,
            configuration.invocation_owner,
            configuration.operation_owner,
        ));
        let context = policy
            .work_context(vec![association.clone()].into(), scope(&session))
            .unwrap();
        Self {
            _directory: directory,
            lifetime,
            source,
            retained,
            reference,
            application,
            policy,
            association,
            context,
            session,
            facts,
        }
    }

    fn model_binding(&self, facts: ModelAuthorizationFacts) -> PreparedAuthorizationBinding {
        self.binding(AuthorizationOperation::Model(facts))
    }

    fn memory(
        &self,
        session: SessionId,
        usage: SourceAuthorizationUse,
    ) -> PreparedAuthorizationBinding {
        self.binding(AuthorizationOperation::Source(SourceAuthorizationFacts {
            usage,
            target: SourceAuthorizationTarget::Memory(MemorySearchScope::for_session(session)),
        }))
    }

    fn binding(&self, operation: AuthorizationOperation) -> PreparedAuthorizationBinding {
        PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
            operation_id: serde_json::from_str("\"00000000-0000-0000-0000-000000000004\"").unwrap(),
            execution_scope: scope(&self.session),
            run_id: None,
            context_revision: None,
            operation,
        })
    }

    fn ingress(&self) -> NativeIngressContext {
        let c = self.association.candidate();
        let mut prompt = PromptInput::new("fixture input", None);
        prompt.header.authority_association = Some(self.association.clone());
        NativeIngressContext::from_trusted_ingress(
            &Input::Prompt(prompt),
            c.requester.clone(),
            c.ingress_actor.clone(),
            c.ingress_namespace.realm.clone(),
            c.original_authentication.clone(),
        )
        .unwrap()
    }
}

fn scope(session: &SessionId) -> OperationExecutionScope {
    OperationExecutionScope::RuntimeInput {
        owner_session_id: session.clone(),
        runtime_epoch_id: serde_json::from_str("\"00000000-0000-0000-0000-000000000002\"").unwrap(),
        submitted_input_id: serde_json::from_str("\"00000000-0000-0000-0000-000000000003\"")
            .unwrap(),
        canonical_input_id: serde_json::from_str("\"00000000-0000-0000-0000-000000000003\"")
            .unwrap(),
    }
}

fn denied<T>(result: Result<T, OperationAuthorizationError>) {
    assert!(
        matches!(result, Err(OperationAuthorizationError::Refused(refusal))
        if refusal.kind() == OperationRefusalKind::Denied)
    );
}

#[tokio::test]
async fn approved_controller_and_own_memory_use_distinct_actual_native_lineages() {
    let f = Fixture::new().await;
    f.policy
        .validate_controller_admission(&f.association, &plain(&f.facts))
        .unwrap();
    f.application
        .authorize_invocation(&f.ingress(), &f.association)
        .unwrap();
    for usage in [
        ModelAuthorizationUse::ControllerInference,
        ModelAuthorizationUse::Compaction,
    ] {
        let mut facts = f.facts.clone();
        facts.usage = usage;
        let binding = f.model_binding(facts);
        f.context
            .authorization()
            .prepare(&binding)
            .unwrap()
            .check_current(&binding)
            .unwrap();
    }
    let memory = f.memory(f.session.clone(), SourceAuthorizationUse::Read);
    f.context
        .authorization()
        .prepare(&memory)
        .unwrap()
        .check_current(&memory)
        .unwrap();
    // Generic requested inference cannot spend the retained controller grant.
    let mut requested = f.facts.clone();
    requested.usage = ModelAuthorizationUse::Inference;
    assert!(
        f.context
            .authorization()
            .prepare(&f.model_binding(requested))
            .is_err()
    );
}

#[tokio::test]
async fn controller_grant_rejects_changed_endpoint_wire_model_identity_and_resource_authority() {
    let f = Fixture::new().await;
    let good = f.model_binding(f.facts.clone());
    f.context.authorization().prepare(&good).unwrap();
    for dimension in 0..3 {
        let mut facts = f.facts.clone();
        match dimension {
            0 => facts.endpoint = "https://other.invalid/v1/responses".into(),
            1 => facts.wire_model = "different-wire-model".into(),
            _ => Arc::make_mut(&mut facts.identity).model = "different-model".into(),
        }
        denied(
            f.policy
                .validate_controller_admission(&f.association, &plain(&facts)),
        );
        denied(f.context.authorization().prepare(&f.model_binding(facts)));
    }
    let mut changed = f.application.native.clone();
    changed.grant_administrator = super::tests::principal("different-application");
    let changed = Arc::new(InstalledApplicationPolicy {
        originals: f.application.originals.clone(),
        lifetime: f.lifetime.clone(),
        native: changed,
        selected: f.application.selected.clone(),
    });
    let configuration = f.source.configuration(
        ScopedInvocation::from_policy(changed.clone()),
        changed,
        None,
    );
    let policy = Arc::new(GrantBackedWorkPolicy::new(
        configuration.grants,
        configuration.invocation_owner,
        configuration.operation_owner,
    ));
    denied(policy.validate_controller_admission(&f.association, &plain(&f.facts)));
    denied(
        policy
            .work_context(vec![f.association.clone()].into(), scope(&f.session))
            .unwrap()
            .authorization()
            .prepare(&good),
    );
}

#[tokio::test]
async fn held_source_does_not_authorize_changed_requester_executor_or_audience() {
    let f = Fixture::new().await;
    let binding = f.model_binding(f.facts.clone());
    f.context.authorization().prepare(&binding).unwrap();
    for dimension in 0..3 {
        let mut claim = f.association.candidate().clone();
        match dimension {
            0 => claim.requester = super::tests::principal("other-requester"),
            1 => claim.logical_executor = super::tests::principal("other-executor"),
            _ => {
                claim.target.audience = AudienceRef::Principal {
                    principal: super::tests::principal("other-audience"),
                }
            }
        }
        let changed = InputAuthorityAssociation::new(claim).unwrap();
        // A changed source binding has no retained route. This is unavailable
        // source custody, not an assertion that a new requester was admitted.
        assert!(
            f.policy
                .validate_controller_admission(&changed, &plain(&f.facts))
                .is_err()
        );
        let context = f
            .policy
            .work_context(vec![changed].into(), scope(&f.session))
            .unwrap();
        assert!(context.authorization().prepare(&binding).is_err());
    }
}

#[tokio::test]
async fn memory_scope_and_use_are_checked_before_any_native_entry() {
    let f = Fixture::new().await;
    let own = f.memory(f.session.clone(), SourceAuthorizationUse::Read);
    f.context.authorization().prepare(&own).unwrap();
    denied(
        f.context
            .authorization()
            .prepare(&f.memory(SessionId::new(), SourceAuthorizationUse::Read)),
    );
    for usage in [
        SourceAuthorizationUse::Hydrate,
        SourceAuthorizationUse::Retain,
    ] {
        assert!(
            f.context
                .authorization()
                .prepare(&f.memory(f.session.clone(), usage))
                .is_err()
        );
    }
}

#[tokio::test]
async fn close_invalidates_prepared_memory_and_refuses_new_invocation_but_retains_controller_policy()
 {
    let f = Fixture::new().await;
    let memory = f.memory(f.session.clone(), SourceAuthorizationUse::Read);
    let prepared = f.context.authorization().prepare(&memory).unwrap();
    let controller = f.model_binding(f.facts.clone());
    f.context.authorization().prepare(&controller).unwrap();
    f.application
        .authorize_invocation(&f.ingress(), &f.association)
        .unwrap();
    f.lifetime
        .begin_close_activation(HostCloseReason::OperatorDisable)
        .unwrap();
    assert!(matches!(prepared.check_current(&memory),
        Err(OperationAuthorizationError::Refused(refusal))
            if refusal.kind() == OperationRefusalKind::ReprepareRequired));
    denied(f.context.authorization().prepare(&memory));
    assert!(
        f.application
            .authorize_invocation(&f.ingress(), &f.association)
            .is_err()
    );
    // This is retained controller policy, not permission for a fresh input.
    // Actual controller-pin custody and run settlement are native owner tests.
    for usage in [
        ModelAuthorizationUse::ControllerInference,
        ModelAuthorizationUse::Compaction,
    ] {
        let mut facts = f.facts.clone();
        facts.usage = usage;
        let binding = f.model_binding(facts);
        f.context
            .authorization()
            .prepare(&binding)
            .unwrap()
            .check_current(&binding)
            .unwrap();
    }
    let c = f.association.candidate();
    f.source
        .grants()
        .resolve_controller_lineage(
            &c.controller_grant_lineage,
            &c.logical_executor,
            c.represented_subject.as_ref(),
        )
        .unwrap()
        .check_current()
        .unwrap();
}

// Catalog coverage resolves real native plans, then uses the same reviewed
// parent, issued child and work compiler as the model/memory controls above.
// It does not execute a load effect or claim a loaded tool can subsequently run.
fn catalog_bounds(native: &CompiledNativeHost, session: &SessionId) -> ExecutionRestrictions {
    ExecutionRestrictions {
        actions: ExactRestriction::exact([ActionRef {
            feature: "toolkit".into(),
            action: "catalog-read".into(),
        }]),
        resource_domains: DomainRestriction::exact([ResourceDomain {
            authority: native.grant_administrator.clone(),
            namespace: format!("toolkit-session-catalog/{session}"),
        }]),
        processors: ExactRestriction::exact([ProcessorRef::Principal {
            principal: native.routes[0].logical_executor.clone(),
        }]),
        audiences: ExactRestriction::exact([native.members[0].audience.clone()]),
        delegation_depth: DelegationDepth::remaining(1),
        ..ExecutionRestrictions::unrestricted()
    }
}

struct CatalogCall {
    // Keep the real resolving leaf alive alongside the root retained by plan.
    control: Arc<meerkat_tools::CatalogControlDispatcher>,
    facts: meerkat_core::authorization::ToolAuthorizationFacts,
}

impl CatalogCall {
    fn new(name: &str) -> Self {
        let control = Arc::new(meerkat_tools::CatalogControlDispatcher::new(
            Arc::new(meerkat_tools::EmptyToolDispatcher),
            Arc::new(meerkat_tools::CatalogControlVisibilityProvider::new()),
        ));
        let root: Arc<dyn meerkat_core::AgentToolDispatcher> =
            Arc::new(meerkat_core::DynamicToolComposite::new(vec![
                control.clone(),
            ]));
        let arguments: Arc<serde_json::value::RawValue> =
            serde_json::value::RawValue::from_string("{}".into())
                .unwrap()
                .into();
        let plan = meerkat_core::resolve_tool_execution_plan_fenced(
            &root,
            meerkat_core::ToolCallView {
                id: "catalog-call",
                name,
                args: &arguments,
            },
            &meerkat_core::ToolDispatchContext::default(),
            &meerkat_core::ToolExecutionResolutionContext::new(catalog_deadlines()),
        )
        .unwrap();
        Self {
            control,
            facts: meerkat_core::authorization::ToolAuthorizationFacts {
                call_id: Arc::from("catalog-call"),
                name: name.into(),
                arguments,
                target: meerkat_core::authorization::ToolAuthorizationTarget::Dispatcher(Arc::new(
                    plan,
                )),
            },
        }
    }

    fn plan(&self) -> &meerkat_core::ResolvedToolExecutionPlan {
        let meerkat_core::authorization::ToolAuthorizationTarget::Dispatcher(plan) =
            &self.facts.target
        else {
            unreachable!("fixture resolves only dispatcher plans")
        };
        plan
    }

    fn binding(&self, f: &Fixture) -> PreparedAuthorizationBinding {
        f.binding(AuthorizationOperation::Tool(self.facts.clone()))
    }

    fn with_plan(
        &self,
        f: &Fixture,
        plan: meerkat_core::ResolvedToolExecutionPlan,
    ) -> PreparedAuthorizationBinding {
        let mut facts = self.facts.clone();
        facts.target =
            meerkat_core::authorization::ToolAuthorizationTarget::Dispatcher(Arc::new(plan));
        f.binding(AuthorizationOperation::Tool(facts))
    }
}

fn catalog_deadlines() -> meerkat_core::ToolDeadlineChain {
    meerkat_core::ToolDeadlineChain::new(vec![meerkat_core::ToolDeadlineContributor::finite(
        meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
        std::time::Duration::from_secs(60),
    )])
    .unwrap()
}

#[tokio::test]
async fn catalog_search_and_load_require_a_catalog_grant_and_actual_resolved_target() {
    let catalog = Fixture::with_operations(catalog_bounds).await;
    let memory = Fixture::new().await;
    for (name, operation) in [
        (
            "tool_catalog_search",
            meerkat_core::CatalogOperation::Search,
        ),
        ("tool_catalog_load", meerkat_core::CatalogOperation::Load),
    ] {
        let call = CatalogCall::new(name);
        let targets: Vec<_> = call
            .plan()
            .owner_witnesses()
            .iter()
            .filter_map(|witness| witness.operation_target())
            .collect();
        assert_eq!(
            targets,
            vec![&meerkat_core::ToolOperationTarget::CatalogControl { operation }]
        );
        let binding = call.binding(&catalog);
        catalog
            .context
            .authorization()
            .prepare(&binding)
            .unwrap()
            .check_current(&binding)
            .unwrap();
        // The same native catalog target cannot spend a memory-only lineage.
        denied(
            memory
                .context
                .authorization()
                .prepare(&call.binding(&memory)),
        );
    }
}

#[tokio::test]
async fn catalog_grants_remain_bound_to_session_authority_executor_and_audience() {
    for dimension in 0..4 {
        let f = Fixture::with_operations(|native, session| {
            let mut bounds = catalog_bounds(native, session);
            match dimension {
                0 => {
                    bounds.resource_domains = DomainRestriction::exact([ResourceDomain {
                        authority: native.grant_administrator.clone(),
                        namespace: format!("toolkit-session-catalog/{}", SessionId::new()),
                    }])
                }
                1 => {
                    bounds.resource_domains = DomainRestriction::exact([ResourceDomain {
                        authority: super::tests::principal("other-application"),
                        namespace: format!("toolkit-session-catalog/{session}"),
                    }])
                }
                2 => {
                    bounds.processors = ExactRestriction::exact([ProcessorRef::Principal {
                        principal: super::tests::principal("other-executor"),
                    }])
                }
                _ => {
                    bounds.audiences = ExactRestriction::exact([AudienceRef::Principal {
                        principal: super::tests::principal("other-audience"),
                    }])
                }
            }
            bounds
        })
        .await;
        for name in ["tool_catalog_search", "tool_catalog_load"] {
            denied(
                f.context
                    .authorization()
                    .prepare(&CatalogCall::new(name).binding(&f)),
            );
        }
    }
}

#[tokio::test]
async fn catalog_names_missing_targets_and_ambiguous_targets_refuse_mcp_scope() {
    let f = Fixture::with_operations(catalog_bounds).await;
    for name in ["tool_catalog_search", "tool_catalog_load"] {
        let call = CatalogCall::new(name);
        f.context
            .authorization()
            .prepare(&call.binding(&f))
            .unwrap();
        let bare = meerkat_core::ToolExecutionContract::default()
            .resolve_default(catalog_deadlines())
            .unwrap();
        let missing = call.with_plan(&f, bare.clone());
        assert!(matches!(
            f.context.authorization().prepare(&missing),
            Err(OperationAuthorizationError::Unavailable)
        ));

        // A second operation target is not a larger catalog permission. This
        // synthetic malformed plan is only a negative classifier input.
        let native_witness = call
            .plan()
            .owner_witnesses()
            .iter()
            .find(|witness| witness.operation_target().is_some())
            .unwrap();
        let other = meerkat_core::ToolExecutionOwnerWitness::new(
            "other-native-owner",
            name,
            native_witness.binding_fingerprint().clone(),
        )
        .unwrap()
        .with_operation_target(meerkat_core::ToolOperationTarget::McpServerOperation {
            server: "reviewed-connector".into(),
            connection_generation: 1,
            raw_operation: "read".into(),
            connector: meerkat_core::mcp_config::McpConnectorApproval::NoConnector,
            account: Arc::new(meerkat_core::OperationAccount::Unavailable),
        });
        let ambiguous = call.with_plan(
            &f,
            call.plan()
                .clone()
                .with_owner_witness(other.clone())
                .unwrap(),
        );
        assert!(matches!(
            f.context.authorization().prepare(&ambiguous),
            Err(OperationAuthorizationError::Unavailable)
        ));
        // Even a sole typed MCP target is outside this catalog grant. A load
        // result cannot replace its separate account/operation owner and grant.
        let mcp = call.with_plan(&f, bare.with_owner_witness(other).unwrap());
        denied(f.context.authorization().prepare(&mcp));
    }
}

#[tokio::test]
async fn catalog_native_leaf_still_refuses_a_plan_from_another_instance() {
    use meerkat_core::AgentToolDispatcher;
    let f = Fixture::with_operations(catalog_bounds).await;
    for name in ["tool_catalog_search", "tool_catalog_load"] {
        let call = CatalogCall::new(name);
        let binding = call.binding(&f);
        f.context.authorization().prepare(&binding).unwrap();
        let other = meerkat_tools::CatalogControlDispatcher::new(
            Arc::new(meerkat_tools::EmptyToolDispatcher),
            Arc::new(meerkat_tools::CatalogControlVisibilityProvider::new()),
        );
        // Direct leaf dispatch ensures the root fence cannot satisfy this
        // negative on the leaf's behalf. It stops before catalog I/O/effects.
        let error = other
            .dispatch_resolved_with_context(
                call.facts.call(),
                &meerkat_core::ToolDispatchContext::default(),
                call.plan(),
            )
            .await
            .unwrap_err();
        assert_eq!(
            error,
            meerkat_core::ToolError::unavailable(
                name,
                meerkat_core::ToolUnavailableReason::ExecutionOwnerChanged,
            )
        );
        assert!(!call.control.tools().is_empty());
    }
}

#[tokio::test]
async fn catalog_close_invalidates_prepared_load_and_preserves_controller_policy() {
    let f = Fixture::with_operations(catalog_bounds).await;
    let load = CatalogCall::new("tool_catalog_load").binding(&f);
    let prepared = f.context.authorization().prepare(&load).unwrap();
    let controller = f.model_binding(f.facts.clone());
    f.context.authorization().prepare(&controller).unwrap();
    f.lifetime
        .begin_close_activation(HostCloseReason::OperatorDisable)
        .unwrap();
    assert!(matches!(prepared.check_current(&load),
        Err(OperationAuthorizationError::Refused(refusal))
            if refusal.kind() == OperationRefusalKind::ReprepareRequired));
    denied(f.context.authorization().prepare(&load));
    f.context
        .authorization()
        .prepare(&controller)
        .unwrap()
        .check_current(&controller)
        .unwrap();
}

// Native527f registration creates the positive owner witness. These component
// controls inspect actual grants and policy compilation without launching any
// setup helper, protected auth UI, provider operation or original continuation.
fn setup_request_bounds(native: &CompiledNativeHost, session: &SessionId) -> ExecutionRestrictions {
    application_operations::parent_restrictions(application_operations::find_and_install(
        &native.grant_administrator,
        &native.routes[0].logical_executor,
        &native.members[0].audience,
        session,
    ))
}

fn operation_unavailable<T>(result: Result<T, OperationAuthorizationError>) {
    assert!(matches!(
        result,
        Err(OperationAuthorizationError::Unavailable)
    ));
}
struct BundleFixture {
    tools: Arc<[Arc<meerkat_core::ToolDef>]>,
    calls: Arc<std::sync::atomic::AtomicUsize>,
}
#[async_trait::async_trait]
impl meerkat_core::AgentToolDispatcher for BundleFixture {
    fn tools(&self) -> Arc<[Arc<meerkat_core::ToolDef>]> {
        self.tools.clone()
    }
    async fn dispatch(
        &self,
        call: meerkat_core::ToolCallView<'_>,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, meerkat_core::ToolError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err(meerkat_core::ToolError::unavailable(
            call.name,
            meerkat_core::ToolUnavailableReason::ExecutionOwnerChanged,
        ))
    }
}
struct SetupBundleCall {
    registered: Arc<meerkat_core::RegisteredToolBundleDispatcher>,
    calls: Arc<std::sync::atomic::AtomicUsize>,
    facts: meerkat_core::authorization::ToolAuthorizationFacts,
}
impl SetupBundleCall {
    fn new(bundle: &str, operation: &str) -> Self {
        Self::with_account(bundle, operation, None)
    }

    fn with_account(
        bundle: &str,
        operation: &str,
        account: Option<meerkat_core::OperationAccount>,
    ) -> Self {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let inner: Arc<dyn meerkat_core::AgentToolDispatcher> = Arc::new(BundleFixture {
            calls: calls.clone(),
            tools: vec![Arc::new(meerkat_core::ToolDef {
                name: operation.into(),
                description: "inert source fixture".into(),
                input_schema: serde_json::json!({"type":"object"}),
                provenance: None,
                audience: meerkat_core::ToolAudience::Model,
            })]
            .into(),
        });
        let registered = meerkat_core::RegisteredToolBundleDispatcher::new(bundle, inner);
        let registered = Arc::new(match account {
            Some(account) => registered.with_account(account),
            None => registered,
        });
        let root: Arc<dyn meerkat_core::AgentToolDispatcher> =
            Arc::new(meerkat_core::DynamicToolComposite::new(vec![
                registered.clone(),
            ]));
        let arguments: Arc<serde_json::value::RawValue> =
            serde_json::value::RawValue::from_string("{}".into())
                .unwrap()
                .into();
        let plan = meerkat_core::resolve_tool_execution_plan_fenced(
            &root,
            meerkat_core::ToolCallView {
                id: "setup-call",
                name: operation,
                args: &arguments,
            },
            &meerkat_core::ToolDispatchContext::default(),
            &meerkat_core::ToolExecutionResolutionContext::new(catalog_deadlines()),
        )
        .unwrap();
        Self {
            registered,
            calls,
            facts: meerkat_core::authorization::ToolAuthorizationFacts {
                call_id: Arc::from("setup-call"),
                name: operation.into(),
                arguments,
                target: meerkat_core::authorization::ToolAuthorizationTarget::Dispatcher(Arc::new(
                    plan,
                )),
            },
        }
    }
    fn plan(&self) -> &meerkat_core::ResolvedToolExecutionPlan {
        let meerkat_core::authorization::ToolAuthorizationTarget::Dispatcher(plan) =
            &self.facts.target
        else {
            unreachable!()
        };
        plan
    }
    fn binding(&self, f: &Fixture) -> PreparedAuthorizationBinding {
        f.binding(AuthorizationOperation::Tool(self.facts.clone()))
    }
    fn with_plan(
        &self,
        f: &Fixture,
        plan: meerkat_core::ResolvedToolExecutionPlan,
    ) -> PreparedAuthorizationBinding {
        let mut facts = self.facts.clone();
        facts.target =
            meerkat_core::authorization::ToolAuthorizationTarget::Dispatcher(Arc::new(plan));
        f.binding(AuthorizationOperation::Tool(facts))
    }
}

#[tokio::test]
async fn retained_targeted_setup_preserves_native_target_and_controller_authority() {
    let f = Fixture::with_targeted_setup().await;
    let claims = f.association.candidate();
    let WorkAuthorityBasis::TargetedGrantLineages { base: Some(base), targets } = &claims.authority_basis
        else { panic!("expected separately issued base and targeted setup authority") };
    assert_eq!(base.len(), 2);
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].lineage.len(), 2);
    assert_ne!(base, &targets[0].lineage);
    assert_eq!(f.retained.association(), &f.association);
    f.application.authorize_invocation(&f.ingress(), &f.association).unwrap();
    f.policy.validate_controller_admission(&f.association, &plain(&f.facts)).unwrap();

    let call = SetupBundleCall::new(crate::FIND_AND_INSTALL_BUNDLE, crate::FIND_AND_INSTALL);
    let find = call.binding(&f);
    let memory = f.memory(f.session.clone(), SourceAuthorizationUse::Read);
    let controller = f.model_binding(f.facts.clone());
    for binding in [&find, &memory, &controller] {
        f.context.authorization().prepare(binding).unwrap().check_current(binding).unwrap();
    }
    // Neither a valid base nor the selected find target permits this sibling.
    let report = SetupBundleCall::new(meerkat_toolkit::setup::SETUP_RESULTS_BUNDLE,
        meerkat_toolkit::setup::SETUP_REPORT);
    denied(f.context.authorization().prepare(&report.binding(&f)));
    denied(f.context.authorization().prepare(&CatalogCall::new("tool_catalog_load").binding(&f)));

    // Revoke only the actual find leaf under native mutation custody. The same
    // real installed policy is installed in this idle native machine; no input
    // is admitted and no operation is executed by this component control.
    let machine = meerkat_runtime::MeerkatMachine::ephemeral().with_local_grant_authorization(
        f.source.configuration(ScopedInvocation::from_policy(f.application.clone()),
            f.application.clone(), None),
    ).unwrap();
    {
        let mut custody = machine.try_controller_grant_mutation().unwrap();
        f.source.grants().revoke(&f.application.native.grant_administrator,
            targets[0].lineage.last().unwrap(), &mut custody).unwrap();
    }
    denied(f.context.authorization().prepare(&find));
    f.application.authorize_invocation(&f.ingress(), &f.association).unwrap();
    f.policy.validate_controller_admission(&f.association, &plain(&f.facts)).unwrap();
    for binding in [&memory, &controller] {
        f.context.authorization().prepare(binding).unwrap().check_current(binding).unwrap();
    }
    assert_eq!(call.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(report.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn targeted_setup_requires_exact_retained_source_and_refuses_non_grant_bases() {
    let f = Fixture::with_targeted_setup().await;
    f.application.authorize_invocation(&f.ingress(), &f.association).unwrap();
    for altered_target in [true, false] {
        let mut claim = f.association.candidate().clone();
        if altered_target {
            let WorkAuthorityBasis::TargetedGrantLineages { base: Some(base), targets } = &mut claim.authority_basis
                else { unreachable!() };
            targets[0].lineage = base.clone();
        } else {
            claim.original_work.work = id("unretained-targeted-original");
        }
        let changed = InputAuthorityAssociation::new(claim).unwrap();
        // Valid shape/native grant references do not create source custody.
        operation_unavailable(f.application.member(&changed));
        operation_unavailable(f.policy.validate_controller_admission(&changed, &plain(&f.facts)));
        let context = f.policy.work_context(vec![changed].into(), scope(&f.session)).unwrap();
        operation_unavailable(context.authorization().prepare(&f.model_binding(f.facts.clone())));
    }
    let claims = f.association.candidate();
    for basis in [
        WorkAuthorityBasis::HostPolicy { policy: claims.original_authentication.clone() },
        WorkAuthorityBasis::ServiceMandate {
            mandate: claims.original_authentication.clone(),
            commissioning_actor: claims.requester.clone(), occurrence: id("foreign-mandate"),
        },
    ] {
        let mut claim = claims.clone();
        claim.authority_basis = basis;
        let changed = InputAuthorityAssociation::new(claim).unwrap();
        denied(f.application.member(&changed));
        denied(f.policy.validate_controller_admission(&changed, &plain(&f.facts)));
    }
    f.application.authorize_invocation(&f.ingress(), &f.association).unwrap();
    f.policy.validate_controller_admission(&f.association, &plain(&f.facts)).unwrap();
}

#[tokio::test]
async fn registered_setup_request_uses_the_same_operator_review_and_native_operation_tuple() {
    let setup = Fixture::with_setup_request().await;
    let call = SetupBundleCall::new(crate::FIND_AND_INSTALL_BUNDLE, crate::FIND_AND_INSTALL);
    let targets: Vec<_> = call
        .plan()
        .owner_witnesses()
        .iter()
        .filter_map(|w| w.operation_target())
        .collect();
    assert_eq!(
        targets,
        vec![
            &meerkat_core::ToolOperationTarget::RegisteredToolBundleOperation {
                bundle: crate::FIND_AND_INSTALL_BUNDLE.into(),
                operation: crate::FIND_AND_INSTALL.into(),
                account: Arc::new(meerkat_core::OperationAccount::Unavailable),
            }
        ]
    );
    let binding = call.binding(&setup);
    setup
        .context
        .authorization()
        .prepare(&binding)
        .unwrap()
        .check_current(&binding)
        .unwrap();
    let catalog = Fixture::with_operations(catalog_bounds).await;
    denied(
        catalog
            .context
            .authorization()
            .prepare(&call.binding(&catalog)),
    );
    let memory = Fixture::new().await;
    denied(
        memory
            .context
            .authorization()
            .prepare(&call.binding(&memory)),
    );
    // Account presentation comes from the actual registration owner. It
    // neither replaces a missing operation grant nor invalidates an R1 grant.
    let account = meerkat_core::OperationAccount::selected(
        "https://account-fixture.invalid",
        "fixture-user",
    );
    assert!(matches!(&account, meerkat_core::OperationAccount::Selected(_)));
    let selected = SetupBundleCall::with_account(
        crate::FIND_AND_INSTALL_BUNDLE,
        crate::FIND_AND_INSTALL,
        Some(account.clone()),
    );
    let selected_accounts: Vec<_> = selected
        .plan()
        .owner_witnesses()
        .iter()
        .filter_map(|witness| witness.operation_target())
        .map(|target| target.account())
        .collect();
    assert_eq!(selected_accounts, vec![Some(&account)]);
    let selected_binding = selected.binding(&setup);
    setup
        .context
        .authorization()
        .prepare(&selected_binding)
        .unwrap()
        .check_current(&selected_binding)
        .unwrap();
    for insufficient in [&catalog, &memory] {
        denied(
            insufficient
                .context
                .authorization()
                .prepare(&selected.binding(insufficient)),
        );
    }
    assert_eq!(selected.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    let catalog_call = CatalogCall::new("tool_catalog_load");
    denied(
        setup
            .context
            .authorization()
            .prepare(&catalog_call.binding(&setup)),
    );
}

#[tokio::test]
async fn setup_witness_refuses_wrong_bundle_operation_missing_and_ambiguous_target() {
    let f = Fixture::with_setup_request().await;
    for (bundle, operation) in [
        ("other.bundle", crate::FIND_AND_INSTALL),
        (crate::FIND_AND_INSTALL_BUNDLE, "other_operation"),
    ] {
        let call = SetupBundleCall::new(bundle, operation);
        operation_unavailable(f.context.authorization().prepare(&call.binding(&f)));
    }
    let call = SetupBundleCall::new(crate::FIND_AND_INSTALL_BUNDLE, crate::FIND_AND_INSTALL);
    let bare = meerkat_core::ToolExecutionContract::default()
        .resolve_default(catalog_deadlines())
        .unwrap();
    operation_unavailable(f.context.authorization().prepare(&call.with_plan(&f, bare)));
    // Only this negative fixture constructs a duplicate witness. A positive
    // plan always comes from the actual native registration owner above.
    let native = call
        .plan()
        .owner_witnesses()
        .iter()
        .find(|w| w.operation_target().is_some())
        .unwrap();
    let duplicate = meerkat_core::ToolExecutionOwnerWitness::new(
        "conflicting-owner",
        crate::FIND_AND_INSTALL,
        native.binding_fingerprint().clone(),
    )
    .unwrap()
    .with_operation_target(
        meerkat_core::ToolOperationTarget::RegisteredToolBundleOperation {
            bundle: crate::FIND_AND_INSTALL_BUNDLE.into(),
            operation: crate::FIND_AND_INSTALL.into(),
            account: Arc::new(meerkat_core::OperationAccount::Unavailable),
        },
    );
    operation_unavailable(f.context.authorization().prepare(&call.with_plan(
        &f,
        call.plan().clone().with_owner_witness(duplicate).unwrap(),
    )));
}

#[tokio::test]
async fn setup_parent_is_bound_to_native_session_authority_executor_and_audience() {
    for wrong in 0..4 {
        let f = Fixture::with_operations(|native, session| {
            let mut bounds = setup_request_bounds(native, session);
            match wrong {
                0 => {
                    bounds.resource_domains = DomainRestriction::exact([ResourceDomain {
                        authority: native.grant_administrator.clone(),
                        namespace: format!("toolkit-session-setup/{}", SessionId::new()),
                    }])
                }
                1 => {
                    bounds.resource_domains = DomainRestriction::exact([ResourceDomain {
                        authority: super::tests::principal("foreign-application"),
                        namespace: format!("toolkit-session-setup/{session}"),
                    }])
                }
                2 => {
                    bounds.processors = ExactRestriction::exact([ProcessorRef::Principal {
                        principal: super::tests::principal("foreign-agent"),
                    }])
                }
                _ => {
                    bounds.audiences = ExactRestriction::exact([AudienceRef::Principal {
                        principal: super::tests::principal("foreign-audience"),
                    }])
                }
            }
            bounds
        })
        .await;
        let call = SetupBundleCall::new(crate::FIND_AND_INSTALL_BUNDLE, crate::FIND_AND_INSTALL);
        denied(f.context.authorization().prepare(&call.binding(&f)));
    }
}

#[tokio::test]
async fn native_setup_wrapper_refuses_another_registration_and_closed_host_refuses_current_use() {
    use meerkat_core::AgentToolDispatcher;
    let f = Fixture::with_setup_request().await;
    let first = SetupBundleCall::new(crate::FIND_AND_INSTALL_BUNDLE, crate::FIND_AND_INSTALL);
    let other = SetupBundleCall::new(crate::FIND_AND_INSTALL_BUNDLE, crate::FIND_AND_INSTALL);
    assert!(matches!(
        other
            .registered
            .dispatch_resolved_with_context(
                meerkat_core::ToolCallView {
                    id: "setup-call",
                    name: crate::FIND_AND_INSTALL,
                    args: &first.facts.arguments
                },
                &meerkat_core::ToolDispatchContext::default(),
                first.plan(),
            )
            .await,
        Err(meerkat_core::ToolError::Unavailable { .. })
    ));
    assert_eq!(other.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    let binding = first.binding(&f);
    let prepared = f.context.authorization().prepare(&binding).unwrap();
    f.lifetime
        .begin_close_activation(HostCloseReason::Shutdown)
        .unwrap();
    assert!(prepared.check_current(&binding).is_err());
}

// Policy-component fixtures only. The synthetic witness lets these controls
// vary the exact tuple without starting an MCP server; it is not evidence of
// physical attachment, native witness production or generation revalidation.
fn mcp_policy_binding(
    fixture: &Fixture,
    server: &str,
    raw_operation: &str,
    generation: u64,
) -> PreparedAuthorizationBinding {
    let call = CatalogCall::new("tool_catalog_search");
    let plan = meerkat_core::ToolExecutionContract::default()
        .resolve_default(catalog_deadlines()).unwrap();
    let catalog_witness = call.plan().owner_witnesses().iter()
        .find(|witness| witness.operation_target().is_some()).unwrap();
    let witness = meerkat_core::ToolExecutionOwnerWitness::new(
        "synthetic-mcp-policy-fixture", "exact-server-operation",
        catalog_witness.binding_fingerprint().clone(),
    ).unwrap().with_operation_target(meerkat_core::ToolOperationTarget::McpServerOperation {
        server: server.into(), connection_generation: generation, raw_operation: raw_operation.into(),
        connector: meerkat_core::mcp_config::McpConnectorApproval::NoConnector,
        account: Arc::new(meerkat_core::OperationAccount::Unavailable),
    });
    // The exposed tool name deliberately remains a catalog name: the policy
    // reads the native typed target, never a name-based permission heuristic.
    call.with_plan(fixture, plan.with_owner_witness(witness).unwrap())
}

#[tokio::test]
async fn mcp_policy_preserves_exact_registration_raw_action_executor_and_audience() {
    let fixture = Fixture::with_operations(|native, _| {
        let mut bounds = application_operations::mcp_access_restrictions(
            &native.grant_administrator, &native.routes[0].logical_executor,
            &native.members[0].audience, "reviewed-connector", ["read"],
            &meerkat_core::mcp_config::McpConnectorApproval::NoConnector,
        );
        bounds.delegation_depth = DelegationDepth::remaining(1);
        bounds
    }).await;
    for generation in [1, 2] {
        let binding = mcp_policy_binding(&fixture, "reviewed-connector", "read", generation);
        let allowance = fixture.application.authorize_operation(
            &fixture.association, &binding, LocalPolicyPurpose::Operation, 5,
        ).unwrap();
        assert_eq!(allowance.operation_values.len(), 2);
        assert_eq!(allowance.operation_values[1].action,
            meerkat_authorization::mcp_installation::mcp_connector_action(
                "reviewed-connector", &meerkat_core::mcp_config::McpConnectorApproval::NoConnector,
            ));
        let values = &allowance.operation_values[0];
        assert_eq!(values.action.feature, "mcp");
        assert_eq!(values.action.action, "read");
        assert_eq!(values.resource_domain.namespace, "mcp-server/reviewed-connector");
        assert_eq!(values.resource_domain.authority, fixture.application.native.grant_administrator);
        assert_eq!(values.processor, ProcessorRef::Principal {
            principal: fixture.association.candidate().logical_executor.clone(),
        });
        assert_eq!(values.audience, fixture.association.candidate().target.audience);
        fixture.context.authorization().prepare(&binding).unwrap()
            .check_current(&binding).unwrap();
    }
    for (server, operation) in [("other-connector", "read"), ("reviewed-connector", "write")] {
        denied(fixture.context.authorization().prepare(
            &mcp_policy_binding(&fixture, server, operation, 1),
        ));
    }
    // Mapping raw MCP tuples does not enlarge catalog grants or authorize the
    // registered setup request. Both require their separately reviewed tuples.
    denied(fixture.context.authorization().prepare(
        &CatalogCall::new("tool_catalog_search").binding(&fixture),
    ));
    denied(fixture.context.authorization().prepare(
        &SetupBundleCall::new(crate::FIND_AND_INSTALL_BUNDLE, crate::FIND_AND_INSTALL)
            .binding(&fixture),
    ));
}

#[tokio::test]
async fn mcp_policy_wildcard_parent_does_not_invent_no_connector_approval() {
    let fixture = Fixture::with_operations(|_, _| ExecutionRestrictions {
        delegation_depth: DelegationDepth::remaining(1),
        ..ExecutionRestrictions::unrestricted()
    }).await;
    let binding = mcp_policy_binding(&fixture, "reviewed-connector", "read", 1);
    assert!(matches!(fixture.context.authorization().prepare(&binding),
        Err(OperationAuthorizationError::Unavailable)));
}

#[tokio::test]
async fn internal_helper_policy_stops_when_its_flattened_frontend_ancestor_closes() {
    use crate::original_ingress::helper_originals::{
        HelperAdmissionSelection, HelperNativeBinding, retained_policy_fixture,
    };
    use meerkat_core::service::WorkGraphNamespaceGrant;
    use meerkat_workgraph::{SqliteWorkGraphStore, WorkGraphService};

    let fixture = Fixture::with_operations_review(memory_bounds, false, true).await;
    let native = &fixture.application.native;
    let private = &fixture.application.originals;
    let component = toolkit_package::Id::new("frontend").unwrap();
    fixture.lifetime.open_component(&component).unwrap();
    fixture.application.current_invocation(&fixture.association).unwrap();

    let controller_parent = native.routes[0].controller_parent.clone().unwrap();
    let operation_parent = native.routes[0].application_parent.clone().unwrap();
    let executor = native.routes[0].logical_executor.clone();
    let child_grant = |parent: &[GrantLineageRef], label: &str| {
        let leaf = fixture.source.grants().issue_child(
            &native.grant_administrator, parent.last().unwrap(), id(label), executor.clone(),
            ExecutionRestrictions { delegation_depth: DelegationDepth::remaining(0), ..ExecutionRestrictions::unrestricted() },
        ).unwrap();
        let mut lineage = parent.to_vec(); lineage.push(leaf); lineage
    };
    // Distinct native-issued helper children from explicitly selected operator
    // parents. No original operation/controller leaf is copied into the helper.
    let controller = child_grant(&controller_parent, "policy-helper-controller");
    let basis = WorkAuthorityBasis::GrantLineage {
        lineage: child_grant(&operation_parent, "policy-helper-operation"),
    };
    let member = native.members[0].clone();
    let selection = HelperAdmissionSelection {
        helper: super::acquisition::InstalledSetupSelection {
            catalog_capability: None,
            report_capability: None,
            member: member.member.clone(), ingress_actor: super::tests::principal("helper-owner"),
            logical_executor: executor, represented_subject: None,
            controller_parent, operation_basis: basis.clone(),
        },
        member, grant_caller: native.grant_administrator.clone(),
    };
    let binding = HelperNativeBinding {
        ingress_actor: selection.helper.ingress_actor.clone(),
        logical_executor: selection.helper.logical_executor.clone(), represented_subject: None,
        target: fixture.association.candidate().target.clone(), authority_basis: basis,
        controller_grant_lineage: controller, controller_model: selected(&fixture.facts),
        controller_ceiling: fixture.association.candidate().controller_ceiling.clone(),
        admitted_ceiling: fixture.association.candidate().admitted_ceiling.clone(),
    };
    let service = WorkGraphService::with_namespace_grant(
        Arc::new(SqliteWorkGraphStore::open(fixture._directory.path().join("helper-policy-work.sqlite3")).unwrap()),
        WorkGraphNamespaceGrant::new(native.realm.to_string(), "setup").unwrap(),
    ).unwrap();
    let source_scope = StoreScope {
        realm: native.realm.to_string(), owner: fixture.application.selected.deployment.to_string(),
    };
    // First retain a real internal child of the frontend source. The outer
    // helper then retains both this child and its exact flattened ancestor.
    assert!(fixture.association.candidate().original_work.authority != native.ingress_domain.authority);
    let inner = retained_policy_fixture(
        &fixture.source, private.canonical_store_for_policy_fixture(), source_scope.clone(),
        &service, "source-policy-inner", fixture.retained.clone(), vec![fixture.retained.clone()],
        selection.clone(), binding.clone(),
    ).await;
    let held_child = inner.original.clone();
    let child = held_child.association().clone();
    assert!(child.candidate().original_work.authority == native.ingress_domain.authority);
    assert!(child.candidate().original_work.authority != fixture.association.candidate().original_work.authority);
    assert!(inner.policy_source().unwrap().origin.original_authority
        == fixture.association.candidate().original_work.authority);
    private.retain_helper_policy_fixture(inner);
    let retained = retained_policy_fixture(
        &fixture.source, private.canonical_store_for_policy_fixture(), source_scope,
        &service, "source-policy-outer", held_child.clone(), vec![held_child, fixture.retained.clone()],
        selection, binding,
    ).await;
    let helper = retained.original.association().clone();
    assert!(retained.policy_source().unwrap().origin.original_authority == child.candidate().original_work.authority);
    assert_eq!(helper.candidate().contributing_work, vec![
        child.candidate().original_work.clone(), fixture.association.candidate().original_work.clone(),
    ]);
    private.retain_helper_policy_fixture(retained);
    // The helper is an internal source with no external route of its own.
    assert!(private.route_for_binding(&helper).is_err());
    fixture.application.current_invocation(&helper).unwrap();
    fixture.lifetime.begin_close_component(&component, HostCloseReason::OperatorDisable).unwrap();
    fixture.lifetime.validate_native_entry(None).unwrap();
    denied(fixture.application.current_invocation(&helper));
    denied(fixture.application.current_invocation(&child));
    denied(fixture.application.current_invocation(&fixture.association));
}

#[path = "application_operations_tests/service_instruction.rs"]
mod service_instruction;

#[path = "application_operations_tests/helper_origin.rs"]
mod helper_origin;

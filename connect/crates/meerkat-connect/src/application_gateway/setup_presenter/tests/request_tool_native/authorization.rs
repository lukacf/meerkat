//! Test-only trusted ingress and resource policy over the actual native grant
//! owner. The native machine still creates all work authorization and tickets.
//! This owns one immutable original, not a production source/permission store.

use meerkat_authorization_contracts::constraints::DomainRestriction;
use std::sync::{Arc, OnceLock};

use meerkat_authorization::{
    clock::HostAuthorizationClock,
    grant_policy::{AdmittedWorkPolicyOwner, ControllerAdmissionAllowance,
        OperationPolicyOwner, WorkOwnerAllowance, single_tool_operation_target},
    grants::{LocalGrantAuthority, LocalGrantConfiguration},
    policy::{LocalOperationValues, LocalPolicyAllowance, LocalPolicyPurpose},
    publication::LocalAuthorizationPublication,
};
use meerkat_authorization_contracts::{
    constraints::{ActionRef, AudienceRef, DelegationDepth, ExactRestriction,
        ExecutionRestrictions, ProcessorRef, ResourceDomain},
    evidence::{EvidenceDigest, EvidenceId, HistoricalEvidenceRef},
    protocol::ContractRequirements,
    resource::ResourceRef,
    work_association::{GrantLineageRef, InputAuthorityAssociation,
        InputAuthorityAssociationCandidate, NativeWorkTarget, OriginalWorkRef,
        QualifiedIngressNamespace, WorkAuthorityBasis},
};
use meerkat_core::{ControllerModelClient, ControllerModelFacts, ControllerRouteKey,
    InputId, PrincipalKind, PrincipalRef, SessionId, TrustDomainId,
    authorization::{AuthorizationOperation, ModelAuthorizationUse,
        OperationAuthorizationError, OperationRefusalKind, OperationRefused,
        OperationReviewTier, PreparedAuthorizationBinding, PublicationRecipient,
        SourceAuthorizationTarget, SourceAuthorizationUse, ToolAuthorizationTarget},
    connection::RealmId, exact_operation::OperationExecutionScope,
    tool_execution::ToolOperationTarget};
use meerkat_runtime::{identifiers::IdempotencyKey, input::Input,
    input_authority::NativeAdmissionError,
    meerkat_machine::{NativeGrantWorkConfiguration, bind_scoped_original}};
use meerkat_toolkit::setup::{SETUP_ACCOUNTS_BUNDLE, SETUP_ACCOUNT_REQUEST};

pub(super) struct FixtureAuthorization(Arc<FixtureOwner>);

struct Original {
    session: SessionId,
    input: InputId,
    association: InputAuthorityAssociation,
    route: ControllerRouteKey,
}

struct FixtureOwner {
    realm: RealmId,
    requester: PrincipalRef,
    ingress: PrincipalRef,
    executor: PrincipalRef,
    domain: ResourceDomain,
    sibling: String,
    controller: GrantLineageRef,
    operations: GrantLineageRef,
    original: OnceLock<Original>,
}

impl FixtureAuthorization {
    /// Pass the returned configuration to the existing
    /// PersistenceBundle::new_with_local_grant_authorization with fresh stores,
    /// before publishing SessionStartup. Do not reuse another live machine's
    /// runtime store. The fixture makes no configured MCP attachment available.
    pub(super) fn new(realm: RealmId, harmless_sibling: &str)
        -> (Self, NativeGrantWorkConfiguration)
    {
        assert_ne!(harmless_sibling, SETUP_ACCOUNT_REQUEST);
        assert!(!harmless_sibling.is_empty());
        let trust = TrustDomainId::new(format!("requesttool-fixture-{}",
            meerkat_core::time_compat::new_uuid_v7())).unwrap();
        let principal = |name| PrincipalRef::in_domain(
            PrincipalKind::ServiceAccount, name, trust.clone()).unwrap();
        let root = principal("issuer");
        let requester = principal("requester");
        let ingress = principal("ingress");
        let executor = principal("session-agent");
        let domain = ResourceDomain { authority: ingress.clone(),
            namespace: "one-original".into() };
        let grants = Arc::new(LocalGrantAuthority::new(LocalGrantConfiguration {
            root: root.clone(), namespace: id("fixture-grants"), generation: 1,
        }, LocalAuthorizationPublication::new(), Arc::new(HostAuthorizationClock)).unwrap());
        let controller = grants.issue_root(&root, id("controller"), executor.clone(), None,
            bounds(&domain, &executor, &requester, ["infer"])).unwrap();
        let operations = grants.issue_root(&root, id("operations"), executor.clone(), None,
            bounds(&domain, &executor, &requester,
                [SETUP_ACCOUNT_REQUEST, harmless_sibling, "read-original", "publish"])).unwrap();
        let owner = Arc::new(FixtureOwner { realm, requester, ingress, executor,
            domain, sibling: harmless_sibling.into(), controller, operations,
            original: OnceLock::new() });
        let ingress_owner = owner.clone();
        let configuration = NativeGrantWorkConfiguration {
            grants,
            ingress: Arc::new(move |_, input, observation, association| {
                let original = ingress_owner.original.get().ok_or_else(||
                    NativeAdmissionError::from(OperationAuthorizationError::Unavailable))?;
                let claims = original.association.candidate();
                if input.id() != &original.input || association != &original.association
                    || observation.requester() != &ingress_owner.requester
                    || observation.ingress_actor() != &ingress_owner.ingress
                    || observation.realm() != &ingress_owner.realm
                    || observation.authentication() != &claims.original_authentication
                {
                    return Err(NativeAdmissionError::Refused(refused()));
                }
                Ok(())
            }),
            invocation_owner: owner.clone(), operation_owner: owner.clone(),
            retained_resume: None,
            // This single-session integration fixture supplies exact ingress
            // and checks native session/input coordinates at every use. It does
            // not claim the separate scoped-destination registration contract.
            require_scoped_destination: false,
            configured_mcp_attachment: None,
        };
        (Self(owner), configuration)
    }

    /// The caller supplies the actual session and its pinned factory client,
    /// after releasing the native turn-boundary lease and before first submit.
    /// Rebinding or reconstructing a successor original is deliberately absent.
    pub(super) fn bind_input(&self, session: &SessionId, input: Input,
        controller: ControllerModelClient) -> Input
    {
        let owner = &self.0;
        let facts = controller.plain_facts().expect("actual retained controller facts");
        let authentication = owner.evidence("fixture-authenticated-ingress");
        let association = InputAuthorityAssociation::new(InputAuthorityAssociationCandidate {
            requester: owner.requester.clone(), ingress_actor: owner.ingress.clone(),
            represented_subject: None, original_authentication: authentication.clone(),
            logical_executor: owner.executor.clone(),
            target: NativeWorkTarget { logical_owner: owner.executor.clone(),
                logical_runtime: id(&meerkat_runtime::LogicalRuntimeId::for_session(session).to_string()),
                context: id(&session.to_string()),
                context_generation: 1,
                audience: AudienceRef::Principal { principal: owner.requester.clone() } },
            original_work: OriginalWorkRef { authority: owner.ingress.clone(),
                work: id(&input.id().to_string()) },
            root_event: owner.evidence("original-event"), contributing_work: Vec::new(),
            authority_basis: WorkAuthorityBasis::GrantLineage {
                lineage: vec![owner.operations.clone()] },
            controller_grant_lineage: vec![owner.controller.clone()],
            controller_model: Some(controller.selection().clone()),
            controller_ceiling: owner.bounds(["infer"]),
            admitted_ceiling: owner.bounds([SETUP_ACCOUNT_REQUEST, owner.sibling.as_str(),
                "read-original", "publish"]),
            source_observations: Vec::new(),
            ingress_namespace: QualifiedIngressNamespace { realm: owner.realm.clone(),
                ingress: owner.domain.clone(), occurrence_scope: id("one-occurrence") },
            contract: ContractRequirements::local_governed_v1(),
        }).expect("well-formed association is still only a claim");
        let original = Original { session: session.clone(), input: input.id().clone(),
            association: association.clone(), route: facts.route_key() };
        assert!(owner.original.set(original).is_ok(), "one immutable fixture original");
        bind_scoped_original(input, association, IdempotencyKey::new("one-original"),
            owner.requester.clone(), owner.ingress.clone(), owner.realm.clone(),
            authentication, controller).expect("bind actual input and pinned controller")
    }
}

impl FixtureOwner {
    fn evidence(&self, name: &str) -> HistoricalEvidenceRef {
        HistoricalEvidenceRef { resource: ResourceRef { domain: self.domain.clone(),
            resource_id: name.into() }, revision: id("1"),
            digest: EvidenceDigest::of_bytes(name.as_bytes()) }
    }

    fn bounds<const N: usize>(&self, actions: [&str; N]) -> ExecutionRestrictions {
        bounds(&self.domain, &self.executor, &self.requester, actions)
    }

    fn exact<'a>(&'a self, association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding) -> Result<&'a Original, OperationAuthorizationError>
    {
        let original = self.original.get().ok_or(OperationAuthorizationError::Unavailable)?;
        if association != &original.association { return Err(refused().into()); }
        match &binding.facts().execution_scope {
            OperationExecutionScope::RuntimeInput { owner_session_id,
                submitted_input_id, canonical_input_id, .. }
                if owner_session_id == &original.session
                    && submitted_input_id == &original.input
                    && canonical_input_id == &original.input => Ok(original),
            _ => Err(refused().into()),
        }
    }

    fn values(&self, action: &str) -> LocalOperationValues {
        LocalOperationValues { action: action_ref(action), resource_domain: self.domain.clone(),
            processor: ProcessorRef::Principal { principal: self.executor.clone() },
            audience: AudienceRef::Principal { principal: self.requester.clone() } }
    }
}

impl AdmittedWorkPolicyOwner for FixtureOwner {
    fn authorize_admitted_work(&self, association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding, purpose: LocalPolicyPurpose, now_ms: u64)
        -> Result<WorkOwnerAllowance, OperationAuthorizationError>
    {
        self.exact(association, binding)?;
        let restrictions = match purpose {
            LocalPolicyPurpose::Controller => self.bounds(["infer"]),
            LocalPolicyPurpose::Operation => self.bounds([SETUP_ACCOUNT_REQUEST,
                self.sibling.as_str(), "read-original", "publish"]),
        };
        Ok(WorkOwnerAllowance { restrictions, expires_at_ms: now_ms.saturating_add(60_000) })
    }
}

impl OperationPolicyOwner for FixtureOwner {
    fn authorize_controller_admission(&self, association: &InputAuthorityAssociation,
        facts: &ControllerModelFacts, _: u64)
        -> Result<ControllerAdmissionAllowance, OperationAuthorizationError>
    {
        let original = self.original.get().ok_or(OperationAuthorizationError::Unavailable)?;
        if association != &original.association || facts.route_key() != original.route {
            return Err(refused().into());
        }
        Ok(ControllerAdmissionAllowance { operation_values: vec![self.values("infer")],
            restrictions: self.bounds(["infer"]) })
    }

    fn authorize_operation(&self, association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding, purpose: LocalPolicyPurpose, now_ms: u64)
        -> Result<LocalPolicyAllowance, OperationAuthorizationError>
    {
        let original = self.exact(association, binding)?;
        let action = match &binding.facts().operation {
            AuthorizationOperation::Model(facts)
                if purpose == LocalPolicyPurpose::Controller
                    && matches!(facts.usage, ModelAuthorizationUse::ControllerInference)
                    && facts.hosted_capabilities.is_empty() && facts.live_channel.is_none()
                    && facts.controller_route_key().as_ref() == Some(&original.route) => "infer",
            AuthorizationOperation::Tool(facts) if purpose == LocalPolicyPurpose::Operation => {
                let ToolAuthorizationTarget::Dispatcher(plan) = &facts.target else {
                    return Err(refused().into());
                };
                match single_tool_operation_target(plan)? {
                    Some(ToolOperationTarget::RegisteredToolBundleOperation { bundle, operation, account: _ })
                        if bundle == SETUP_ACCOUNTS_BUNDLE && operation == SETUP_ACCOUNT_REQUEST
                            && facts.name.as_str() == SETUP_ACCOUNT_REQUEST => SETUP_ACCOUNT_REQUEST,
                    // The caller supplies exactly one no-effect fixture leaf.
                    // It must stay untargeted, so another registered bundle or
                    // MCP/catalog operation with the same name cannot match it.
                    None if facts.name.as_str() == self.sibling => self.sibling.as_str(),
                    _ => return Err(refused().into()),
                }
            }
            AuthorizationOperation::Source(facts) if purpose == LocalPolicyPurpose::Operation
                && matches!(facts.usage, SourceAuthorizationUse::Read) => {
                match (&facts.target, &binding.facts().execution_scope) {
                    (SourceAuthorizationTarget::RuntimeInput { owner_session_id, runtime_epoch_id,
                        input_id }, OperationExecutionScope::RuntimeInput { runtime_epoch_id: epoch, .. })
                        if owner_session_id == &original.session && input_id == &original.input
                            && runtime_epoch_id == epoch => "read-original",
                    _ => return Err(refused().into()),
                }
            }
            AuthorizationOperation::Publication(facts) if purpose == LocalPolicyPurpose::Operation
                && facts.live_channel.is_none()
                && matches!(&facts.recipient, PublicationRecipient::Principal(p) if p == &self.requester)
                    => "publish",
            _ => return Err(refused().into()),
        };
        Ok(LocalPolicyAllowance { operation_values: vec![self.values(action)],
            restrictions: self.bounds([action]), expires_at_ms: now_ms.saturating_add(60_000),
            review_tier: OperationReviewTier::R1 })
    }
}

fn id(value: &str) -> EvidenceId { EvidenceId::new(value).unwrap() }
fn action_ref(action: &str) -> ActionRef {
    ActionRef { feature: "requesttool-locality-fixture".into(), action: action.into() }
}
fn refused() -> OperationRefused { OperationRefused::new(OperationRefusalKind::Denied) }
fn bounds<const N: usize>(domain: &ResourceDomain, executor: &PrincipalRef,
    audience: &PrincipalRef, actions: [&str; N]) -> ExecutionRestrictions
{
    let mut restrictions = ExecutionRestrictions::unrestricted();
    restrictions.actions = ExactRestriction::exact(actions.into_iter().map(action_ref));
    restrictions.resource_domains = DomainRestriction::exact([domain.clone()]);
    restrictions.processors = ExactRestriction::exact([ProcessorRef::Principal { principal: executor.clone() }]);
    restrictions.audiences = ExactRestriction::exact([AudienceRef::Principal { principal: audience.clone() }]);
    restrictions.delegation_depth = DelegationDepth::remaining(0);
    restrictions
}

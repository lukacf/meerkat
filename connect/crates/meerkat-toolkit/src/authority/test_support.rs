//! Shared data fixtures. Every native operation is denied. These fixtures do
//! not authenticate ingress or qualify a runtime, provider, or grant owner.
use meerkat_authorization_contracts::{
    constraints::{AudienceRef, ExecutionRestrictions, ResourceDomain},
    evidence::{EvidenceDigest, EvidenceId, HistoricalEvidenceRef},
    protocol::ContractRequirements,
    resource::ResourceRef,
    work_association::{
        InputAuthorityAssociation, InputAuthorityAssociationCandidate, NativeWorkTarget,
        OriginalWorkRef, QualifiedIngressNamespace, WorkAuthorityBasis,
    },
};
use meerkat_core::{
    auth::{PrincipalKind, PrincipalRef, TrustDomainId},
    authorization::{
        OperationAuthorizationError, PreparedAuthorizationBinding, PreparedOperationAuthorization,
        WorkAuthorization, WorkAuthorizationContext,
    },
    connection::RealmId,
    exact_operation::OperationExecutionScope,
    lifecycle::{InputId, RunId},
    retained_work::{RetainedContributorRef, RetainedWorkIdentity},
};
use std::sync::Arc;
use toolkit_package::ContentDigest;

pub(crate) fn principal(kind: PrincipalKind, value: &str) -> PrincipalRef {
    PrincipalRef::in_domain(kind, value, TrustDomainId::new("toolkit-fixture").unwrap()).unwrap()
}
pub(crate) fn event(value: &str) -> HistoricalEvidenceRef {
    HistoricalEvidenceRef {
        resource: ResourceRef {
            domain: ResourceDomain {
                authority: principal(PrincipalKind::ServiceAccount, "source"),
                namespace: "receipts".into(),
            },
            resource_id: value.into(),
        },
        revision: EvidenceId::new("1").unwrap(),
        digest: EvidenceDigest::of_bytes(value.as_bytes()),
    }
}
pub(crate) fn target(value: &str, requester: &PrincipalRef) -> NativeWorkTarget {
    NativeWorkTarget {
        logical_owner: principal(PrincipalKind::SharedAgent, value),
        logical_runtime: EvidenceId::new("fixture-runtime").unwrap(),
        context: EvidenceId::new(value).unwrap(),
        context_generation: 1,
        audience: AudienceRef::Principal {
            principal: requester.clone(),
        },
    }
}
pub(crate) fn original(value: &str) -> OriginalWorkRef {
    OriginalWorkRef {
        authority: principal(PrincipalKind::ServiceAccount, "source"),
        work: EvidenceId::new(value).unwrap(),
    }
}
pub(crate) fn association(
    requester: PrincipalRef,
    target: NativeWorkTarget,
    work: &str,
    root_event: &str,
) -> InputAuthorityAssociation {
    InputAuthorityAssociation::new(InputAuthorityAssociationCandidate {
        requester,
        ingress_actor: principal(PrincipalKind::ServiceAccount, "ingress"),
        represented_subject: None,
        original_authentication: event("authenticated-at-ingress"),
        logical_executor: target.logical_owner.clone(),
        target,
        original_work: original(work),
        root_event: event(root_event),
        contributing_work: vec![original(work)],
        authority_basis: WorkAuthorityBasis::HostPolicy {
            policy: event("policy"),
        },
        controller_grant_lineage: vec![],
        controller_model: None,
        controller_ceiling: ExecutionRestrictions::unrestricted(),
        admitted_ceiling: ExecutionRestrictions::unrestricted(),
        source_observations: vec![event("source")],
        ingress_namespace: QualifiedIngressNamespace {
            realm: RealmId::parse("core-realm").unwrap(),
            ingress: ResourceDomain {
                authority: principal(PrincipalKind::ServiceAccount, "source"),
                namespace: "input".into(),
            },
            occurrence_scope: EvidenceId::new("fixture-occurrence").unwrap(),
        },
        contract: ContractRequirements::local_governed_v1(),
    })
    .unwrap()
}
struct DenyOperations;
impl WorkAuthorization for DenyOperations {
    fn prepare(
        &self,
        _: &PreparedAuthorizationBinding,
    ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError> {
        Err(OperationAuthorizationError::Unavailable)
    }
}
pub(crate) fn current(association: &InputAuthorityAssociation) -> WorkAuthorizationContext {
    WorkAuthorizationContext::new(Arc::new(DenyOperations), OperationExecutionScope::Domain)
        .with_retained_work(RetainedWorkIdentity::new(
            "fixture-runtime",
            RunId::new(),
            vec![RetainedContributorRef {
                input_id: InputId::new(),
                association_digest: ContentDigest::of_bytes(
                    &serde_json::to_vec(association).unwrap(),
                )
                .to_string(),
                submission_digest: ContentDigest::of_bytes(b"fixture-input").to_string(),
            }],
            Default::default(),
            None,
        ))
}

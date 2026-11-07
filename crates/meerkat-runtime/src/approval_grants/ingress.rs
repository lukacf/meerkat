//! The approved work of an executed `grant.child` decision, bound by Core
//! (`ApprovalCarriedIngress`), in the pattern of a host outcome binding.
//!
//! The association keeps the request's requester, names the runtime's
//! decision ingress as its ingress actor, acts for the grant's represented
//! subject, runs on the approval branch, and is the approved work itself
//! (derived from the decision id, so a replayed admission is the same work).
//! It names no contributor. Its basis selects one target, the bound
//! operation's, whose lineage ends at the approval-carried grant, and no
//! base: every other target has no lineage. Its controller lineage is the
//! grantee's own extended by the branch's controller grant. It is bound in
//! the origin's realm and ingress domain, with the approved work as its
//! occurrence, as a governed peer work is.

use meerkat_authorization_contracts::constraints::ResourceDomain;
use meerkat_authorization_contracts::evidence::{
    EvidenceDigest, EvidenceId, HistoricalEvidenceRef,
};
use meerkat_authorization_contracts::protocol::ContractRequirements;
use meerkat_authorization_contracts::resource::ResourceRef;
use meerkat_authorization_contracts::work_association::{
    GrantLineageRef, GrantTarget, InputAuthorityAssociation, InputAuthorityAssociationCandidate,
    OriginalWorkRef, QualifiedIngressNamespace, TargetGrantLineage, WorkAuthorityBasis,
};
use meerkat_core::PrincipalRef;
use meerkat_core::approval::decision::DecisionId;
use sha2::{Digest, Sha256};

use super::body::{GrantChildRequest, approved_work_id, approved_work_prompt};
use super::seams::ApprovalBranchSeat;
use crate::identifiers::IdempotencyKey;
use crate::input::{Input, PromptInput};

/// The approved work of one executed `grant.child` decision, bound to its
/// final input: the association Core minted, which the host enrolls before
/// admission, and the input the branch admits.
#[derive(Debug)]
#[non_exhaustive]
pub struct ApprovalCarriedIngress {
    pub association: InputAuthorityAssociation,
    pub input: Input,
}

/// Why the approved work could not be bound.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the approved work cannot be bound: {0}")]
pub struct ApprovalCarriedIngressError(pub(crate) String);

impl ApprovalCarriedIngress {
    /// Bind the approved work of `decision`, whose held request is
    /// `request`, on the branch `seat` of the executor `branch`: its
    /// operation lineage `approval_lineage` (the offering grant's lineage
    /// and the approval-carried grant), its controller lineage
    /// `controller_lineage` (the grantee's and the branch's controller
    /// grant), with `decision_ingress` as its ingress actor.
    ///
    /// # Errors
    /// [`ApprovalCarriedIngressError`] when the request's facts cannot form
    /// an association or the input cannot be bound to the branch's
    /// controller.
    pub(crate) fn bind(
        decision: &DecisionId,
        request: &GrantChildRequest,
        branch: &PrincipalRef,
        approval_lineage: Vec<GrantLineageRef>,
        controller_lineage: Vec<GrantLineageRef>,
        seat: ApprovalBranchSeat,
        decision_ingress: &PrincipalRef,
    ) -> Result<Self, ApprovalCarriedIngressError> {
        let failed =
            |detail: &dyn std::fmt::Display| ApprovalCarriedIngressError(detail.to_string());
        let work_id = approved_work_id(decision).ok_or_else(|| failed(&"no approved work id"))?;
        let original_work = OriginalWorkRef {
            authority: request.origin.ingress.authority.clone(),
            work: work_id.clone(),
        };
        if original_work != request.terms.work {
            return Err(failed(&"the terms name another work"));
        }
        let target = request
            .terms
            .operation
            .as_ref()
            .map(|operation| operation.target.clone())
            .unwrap_or(GrantTarget::McpServer {
                server: request.operation.server.clone(),
            });
        let evidence =
            |revision: &str, digest: EvidenceDigest| -> Result<_, ApprovalCarriedIngressError> {
                Ok(HistoricalEvidenceRef {
                    resource: ResourceRef {
                        domain: ResourceDomain {
                            authority: request.origin.ingress.authority.clone(),
                            namespace: "decisions".into(),
                        },
                        resource_id: decision.as_str().into(),
                    },
                    revision: EvidenceId::new(revision).map_err(|error| failed(&error))?,
                    digest,
                })
            };
        let original_authentication = evidence(
            "executed",
            digest(&[
                b"meerkat.approved-work-decision.v1",
                decision.as_str().as_bytes(),
            ]),
        )?;
        let root_event = evidence(
            "approved-work",
            digest(&[
                b"meerkat.approved-work-event.v1",
                work_id.as_str().as_bytes(),
            ]),
        )?;
        let association = InputAuthorityAssociation::new(InputAuthorityAssociationCandidate {
            requester: request.terms.requester.clone(),
            ingress_actor: decision_ingress.clone(),
            represented_subject: request.terms.represented_subject.clone(),
            original_authentication: original_authentication.clone(),
            logical_executor: branch.clone(),
            target: seat.target.clone(),
            original_work,
            root_event,
            contributing_work: Vec::new(),
            authority_basis: WorkAuthorityBasis::TargetedGrantLineages {
                base: None,
                targets: vec![TargetGrantLineage {
                    target,
                    lineage: approval_lineage,
                }],
            },
            controller_grant_lineage: controller_lineage,
            controller_model: Some(seat.controller.selection().clone()),
            controller_ceiling: seat.controller_ceiling.clone(),
            admitted_ceiling: seat.admitted_ceiling.clone(),
            source_observations: Vec::new(),
            ingress_namespace: QualifiedIngressNamespace {
                realm: request.origin.realm.clone(),
                ingress: request.origin.ingress.clone(),
                occurrence_scope: work_id.clone(),
            },
            contract: ContractRequirements::local_governed_v1(),
        })
        .map_err(|error| failed(&error))?;
        let prompt =
            approved_work_prompt(request).ok_or_else(|| failed(&"no approved work input"))?;
        let input = crate::input_authority::bind_original_input(
            Input::Prompt(PromptInput::new(prompt, None)),
            association.clone(),
            IdempotencyKey::new(work_id.as_str()),
            request.terms.requester.clone(),
            decision_ingress.clone(),
            request.origin.realm.clone(),
            original_authentication,
            seat.controller,
        )
        .map_err(|error| failed(&error))?;
        Ok(Self { association, input })
    }
}

/// SHA-256 over length-delimited `parts`.
fn digest(parts: &[&[u8]]) -> EvidenceDigest {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part);
    }
    EvidenceDigest::from_array(hasher.finalize().into())
}

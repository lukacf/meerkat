//! Holding an access offer: the owner logic of `access_offer`.
//!
//! An agent offers the agent that asked it (for the requester of the work it
//! runs) one exact operation of a governed source it can reach, under the
//! source owner's decision. Core resolves the offering grant from the
//! calling work's own lineage for the operation's server, takes the
//! requester and the origin from the calling work, computes the digest of
//! the operation from its exact arguments, and holds a `grant.child`
//! decision for the offering grant's child rule. Nothing is approved that
//! cannot run: an offer without a child rule, without a reply route or
//! without a branch for its grantee is refused at hold, typed.

use meerkat_authorization::grants::{
    ChildGrantIntent, ChildGrantRefusal, GrantChildTerms, GrantRefusal, RetainedGrant,
    grant_child_terms_digest, mcp_operation_binding,
};
use meerkat_authorization_contracts::constraints::{
    AudienceRef, BoundMatch, DelegationDepth, ExactRestriction, ExecutionRestrictions,
    LifetimeBound, LifetimeRestriction, ProcessorRef, ResourceDomain,
};
use meerkat_authorization_contracts::derived_child::DerivedChildRestrictions;
use meerkat_authorization_contracts::work_association::{
    GrantLineageRef, GrantTarget, OriginalWorkRef, WorkAuthorityBasis,
};
use meerkat_core::approval::decision::{
    DecisionHoldRequest, DecisionId, DecisionKind, DecisionPrincipalKey, StoredDecision,
};
use meerkat_core::connection::RealmId;
use meerkat_core::{PrincipalRef, ToolDispatchContext};
use serde::{Deserialize, Serialize};

use super::body::{
    GrantChildRequest, OfferedOperation, approval_branch_principal, approved_work_id,
    branch_controller_grant_id, grant_child_operation_id, grant_child_subject_key, render,
};
use super::seams::{ApprovalGranteeBasis, ApprovalOrigin};
use super::{ApprovalGrants, GRANT_CHILD_REQUEST_FORMAT};
use crate::decision::DecisionError;
use crate::input_authority::GovernedPeerOrigin;

/// Longest summary an access offer carries, in bytes.
pub const ACCESS_OFFER_SUMMARY_MAX_BYTES: usize = 4096;

/// The calling work of a `decisions` tool, as Core admitted it: read from the
/// governed origin the native runtime attached to the work's authorization,
/// never from a tool's arguments.
#[derive(Debug, Clone)]
pub(crate) struct CallingWork {
    pub(crate) requester: PrincipalRef,
    /// The calling agent: the work's executor.
    pub(crate) executor: PrincipalRef,
    pub(crate) represented_subject: Option<PrincipalRef>,
    pub(crate) realm: RealmId,
    pub(crate) ingress: ResourceDomain,
    pub(crate) original_work: OriginalWorkRef,
    pub(crate) ingress_actor: PrincipalRef,
    pub(crate) basis: WorkAuthorityBasis,
    pub(crate) contributing_work: Vec<OriginalWorkRef>,
}

impl CallingWork {
    /// The work `origin` was minted for, when it staged exactly one
    /// original.
    pub(crate) fn of(origin: &GovernedPeerOrigin) -> Option<Self> {
        let original = origin.single_original()?;
        Some(Self {
            requester: origin.requester().clone(),
            executor: origin.sender_executor().clone(),
            represented_subject: origin.represented_subject().cloned(),
            realm: original.namespace.realm.clone(),
            ingress: original.namespace.ingress.clone(),
            original_work: original.original_work.clone(),
            ingress_actor: original.ingress_actor.clone(),
            basis: original.authority_basis.clone(),
            contributing_work: original.contributing_work.to_vec(),
        })
    }

    /// The calling work of a dispatch, when it runs in governed work that
    /// staged exactly one original.
    pub(crate) fn of_dispatch(context: &ToolDispatchContext) -> Option<Self> {
        let carrier = context.work_authorization()?.governed_send_origin()?;
        Self::of(&GovernedPeerOrigin::from_send_origin(carrier)?)
    }

    /// Where the request came from: the calling work's first contributing
    /// work when it runs in a peer work, else the calling work itself.
    pub(crate) fn origin_work(&self) -> &OriginalWorkRef {
        self.contributing_work
            .first()
            .unwrap_or(&self.original_work)
    }

    /// The agent whose approval branch would run the approved work: in a
    /// peer work the agent that asked (its ingress actor), else the calling
    /// agent itself.
    pub(crate) fn grantee(&self) -> &PrincipalRef {
        if self.contributing_work.is_empty() {
            &self.executor
        } else {
            &self.ingress_actor
        }
    }

    /// The calling work's own lineage for `server`'s operations: the
    /// offering grant's.
    fn parent_lineage(&self, server: &str) -> Option<&[GrantLineageRef]> {
        self.basis.lineage_for(Some(&GrantTarget::McpServer {
            server: server.to_owned(),
        }))
    }
}

/// The exact operation `access_offer` offers.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OperationArguments {
    pub(crate) server: String,
    pub(crate) tool: String,
    pub(crate) arguments: serde_json::Value,
}

/// The arguments of `access_offer`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AccessOfferArguments {
    pub(crate) operation: OperationArguments,
    #[serde(default)]
    pub(crate) uses: Option<u64>,
    #[serde(default)]
    pub(crate) duration_ms: Option<u64>,
    pub(crate) summary: String,
}

/// Why an access offer was refused when it was held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AccessOfferRefusalKind {
    /// No grant of the calling work for the operation carries a child
    /// approval rule (or none covers the operation at all).
    NoChildApprovalRule,
    /// The host cannot deliver a reply to the request's origin.
    ReplyRouteUnavailable,
    /// The agent that asked cannot run the approved work in an approval
    /// branch here.
    ApprovalBranchUnavailable,
    /// More uses than the host's policy allows.
    UsesOverPolicy,
    /// A longer window than the host's policy allows.
    WindowOverPolicy,
    /// The offering grant's window ends before the requested window would:
    /// the grant could never be issued with the presented terms.
    WindowOverParent,
    /// The offering grant's bounds exclude the approved work: its processor
    /// (the approval branch) or its audience (the requester).
    OutsideParentBounds,
}

impl AccessOfferRefusalKind {
    /// The refusal's stable token (`no_child_approval_rule`, ...).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoChildApprovalRule => "no_child_approval_rule",
            Self::ReplyRouteUnavailable => "reply_route_unavailable",
            Self::ApprovalBranchUnavailable => "approval_branch_unavailable",
            Self::UsesOverPolicy => "uses_over_policy",
            Self::WindowOverPolicy => "window_over_policy",
            Self::WindowOverParent => "window_over_parent",
            Self::OutsideParentBounds => "outside_parent_bounds",
        }
    }
}

/// The typed reason of an [`AccessOfferRefusalKind`], where it has one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AccessOfferRefusalReason {
    /// No grant of the calling work's agent covers the operation's server.
    NoParentGrant,
    /// The offering grant is not current now.
    ParentNotCurrent,
    /// The offering grant may not be delegated further.
    ParentCannotDelegate,
    /// The host has no approval branch for the agent that asked.
    NotBranchable,
    /// The asking agent's controller grant cannot admit a controller grant
    /// for its branch.
    ControllerCannotAdmitChild,
    /// The offering grant acts for another principal than the asking
    /// agent's controller grant: an approved work has one represented
    /// subject.
    SubjectMismatch,
    /// The offering grant's processors exclude the approval branch.
    ProcessorExcluded,
    /// The offering grant's audiences exclude the requester.
    AudienceExcluded,
}

impl AccessOfferRefusalReason {
    /// The reason's stable token (`no_parent_grant`, ...).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoParentGrant => "no_parent_grant",
            Self::ParentNotCurrent => "parent_not_current",
            Self::ParentCannotDelegate => "parent_cannot_delegate",
            Self::NotBranchable => "not_branchable",
            Self::ControllerCannotAdmitChild => "controller_cannot_admit_child",
            Self::SubjectMismatch => "subject_mismatch",
            Self::ProcessorExcluded => "processor_excluded",
            Self::AudienceExcluded => "audience_excluded",
        }
    }
}

/// An access offer refused at hold: its kind and, where it has one, its
/// typed reason. Nothing was held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AccessOfferRefusal {
    pub refusal: AccessOfferRefusalKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<AccessOfferRefusalReason>,
}

impl AccessOfferRefusal {
    fn of(refusal: AccessOfferRefusalKind) -> Self {
        Self {
            refusal,
            reason: None,
        }
    }

    fn because(refusal: AccessOfferRefusalKind, reason: AccessOfferRefusalReason) -> Self {
        Self {
            refusal,
            reason: Some(reason),
        }
    }
}

impl std::fmt::Display for AccessOfferRefusal {
    /// The kind and, where it has one, the reason:
    /// `no_child_approval_rule (no_parent_grant)`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.reason {
            Some(reason) => write!(f, "{} ({})", self.refusal.as_str(), reason.as_str()),
            None => f.write_str(self.refusal.as_str()),
        }
    }
}

/// Why an access offer held nothing.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum AccessOfferError {
    #[error("access offer refused: {0}")]
    Refused(AccessOfferRefusal),
    #[error("invalid access offer: {0}")]
    InvalidArguments(String),
    #[error("access offer unavailable: {0}")]
    Unavailable(String),
    #[error(transparent)]
    Decision(#[from] DecisionError),
}

/// The requested bounds of an approval-carried child: everything its parent
/// allows, for a window of `duration_ms` from `now_ms`, with no further
/// delegation. The exact operation is bound by the child's approval.
pub(crate) fn approval_child_request(now_ms: u64, duration_ms: u64) -> ExecutionRestrictions {
    let mut requested = ExecutionRestrictions::unrestricted();
    requested.lifetime = LifetimeRestriction::window(now_ms, now_ms.saturating_add(duration_ms));
    requested.delegation_depth = DelegationDepth::remaining(0);
    requested
}

/// The requested bounds of an approval branch's controller grant: processed
/// by the branch only, with no further delegation, and the controller's
/// unrestricted lifetime.
pub(crate) fn branch_controller_request(branch: &PrincipalRef) -> ExecutionRestrictions {
    let mut requested = ExecutionRestrictions::unrestricted();
    requested.processors = ExactRestriction::exact([ProcessorRef::Principal {
        principal: branch.clone(),
    }]);
    requested.delegation_depth = DelegationDepth::remaining(0);
    requested
}

/// The intent of the controller grant of `branch`, the approval branch of
/// the grantee `grantee` (whose own side is `basis`) for `decision`.
pub(crate) fn branch_controller_intent(
    decision: &DecisionId,
    grantee: &PrincipalRef,
    basis: &ApprovalGranteeBasis,
    branch: &PrincipalRef,
) -> Option<ChildGrantIntent> {
    Some(ChildGrantIntent {
        issuer: grantee.clone(),
        parent_lineage: basis.controller_grant_lineage.clone(),
        id: branch_controller_grant_id(decision)?,
        grantee: branch.clone(),
        represented_subject: basis.represented_subject.clone(),
        requested: branch_controller_request(branch),
    })
}

/// The terms of the approval-carried child of `parent` the offer `request`
/// asks for (bar the window and the parent's bounds, which `effective`
/// carries): the effective child bounds of now.
#[allow(clippy::too_many_arguments)]
pub(crate) fn child_terms(
    parent_id: &meerkat_authorization_contracts::evidence::EvidenceId,
    parent: &RetainedGrant,
    issuer: &PrincipalRef,
    grantee: &PrincipalRef,
    effective: &ExecutionRestrictions,
    operation: &OfferedOperation,
    uses: u64,
    requester: &PrincipalRef,
    work: &OriginalWorkRef,
) -> Option<GrantChildTerms> {
    let LifetimeBound::Window {
        not_before_ms,
        expires_at_ms,
    } = effective.lifetime.bound()
    else {
        return None;
    };
    Some(GrantChildTerms {
        parent: parent_id.clone(),
        issuer: issuer.clone(),
        grantee: grantee.clone(),
        represented_subject: parent.represented_subject.clone(),
        actions: effective.actions.clone(),
        resource_domains: effective.resource_domains.clone(),
        processors: effective.processors.clone(),
        audiences: effective.audiences.clone(),
        window_ms: expires_at_ms.checked_sub(not_before_ms)?,
        operation: Some(mcp_operation_binding(
            GrantTarget::McpServer {
                server: operation.server.clone(),
            },
            &operation.tool,
            operation.request_digest.clone(),
        )),
        uses,
        requester: requester.clone(),
        work: work.clone(),
        review_before_return: None,
    })
}

impl ApprovalGrants {
    /// The approver id a hold of an offer of `server`'s operations in the
    /// calling work `calling` asks: the child rule's of the offering grant
    /// (the calling work's own lineage for `server`, held by the calling
    /// agent).
    pub(crate) fn offer_approver(
        &self,
        calling: &CallingWork,
        server: &str,
    ) -> Result<meerkat_core::approval::decision::ApproverId, AccessOfferError> {
        let no_parent = || {
            AccessOfferError::Refused(AccessOfferRefusal::because(
                AccessOfferRefusalKind::NoChildApprovalRule,
                AccessOfferRefusalReason::NoParentGrant,
            ))
        };
        let parent_ref = calling
            .parent_lineage(server)
            .and_then(<[GrantLineageRef]>::last)
            .ok_or_else(no_parent)?;
        let parent = self
            .0
            .grants
            .retained_grant(parent_ref)
            .map_err(|refusal| match refusal {
                GrantRefusal::Unavailable => {
                    AccessOfferError::Unavailable("the grant owner cannot say now".into())
                }
                _ => no_parent(),
            })?;
        if parent.grantee != calling.executor {
            return Err(no_parent());
        }
        parent
            .child_approval
            .map(|rule| rule.approver_id)
            .ok_or(AccessOfferError::Refused(AccessOfferRefusal::of(
                AccessOfferRefusalKind::NoChildApprovalRule,
            )))
    }

    /// Hold the `grant.child` decision the access offer `arguments` asks
    /// for in the calling work `calling`: the decision as stored, or the
    /// typed refusal at hold.
    pub(crate) fn offer(
        &self,
        calling: &CallingWork,
        arguments: AccessOfferArguments,
    ) -> Result<StoredDecision, AccessOfferError> {
        let inner = &self.0;
        let policy = &inner.policy;
        let uses = arguments.uses.unwrap_or(1);
        if uses == 0 {
            return Err(AccessOfferError::InvalidArguments(
                "uses must be at least 1".into(),
            ));
        }
        if uses > policy.max_uses {
            return Err(AccessOfferError::Refused(AccessOfferRefusal::of(
                AccessOfferRefusalKind::UsesOverPolicy,
            )));
        }
        let duration_ms = arguments.duration_ms.unwrap_or(policy.max_window_ms);
        if duration_ms == 0 {
            return Err(AccessOfferError::InvalidArguments(
                "duration_ms must be at least 1".into(),
            ));
        }
        if duration_ms > policy.max_window_ms {
            return Err(AccessOfferError::Refused(AccessOfferRefusal::of(
                AccessOfferRefusalKind::WindowOverPolicy,
            )));
        }
        if arguments.summary.len() > ACCESS_OFFER_SUMMARY_MAX_BYTES {
            return Err(AccessOfferError::InvalidArguments(format!(
                "summary is longer than {ACCESS_OFFER_SUMMARY_MAX_BYTES} bytes"
            )));
        }
        let OperationArguments {
            server,
            tool,
            arguments: call_arguments,
        } = arguments.operation;
        if server.is_empty() || tool.is_empty() || !call_arguments.is_object() {
            return Err(AccessOfferError::InvalidArguments(
                "operation needs a server, a tool and an arguments object".into(),
            ));
        }
        let request_digest = meerkat_core::mcp_request_digest(&server, &tool, &call_arguments)
            .ok_or_else(|| {
                AccessOfferError::InvalidArguments(
                    "the operation's arguments have no canonical form".into(),
                )
            })?;
        let operation = OfferedOperation {
            server,
            tool,
            arguments: call_arguments,
            request_digest,
        };

        // The offering grant: the calling work's own lineage for the
        // server, held by the calling agent, under a child rule, current.
        let no_rule = |reason| {
            AccessOfferError::Refused(AccessOfferRefusal::because(
                AccessOfferRefusalKind::NoChildApprovalRule,
                reason,
            ))
        };
        let parent_lineage = calling
            .parent_lineage(&operation.server)
            .ok_or_else(|| no_rule(AccessOfferRefusalReason::NoParentGrant))?
            .to_vec();
        let parent_ref = parent_lineage
            .last()
            .ok_or_else(|| no_rule(AccessOfferRefusalReason::NoParentGrant))?
            .clone();
        let parent = inner
            .grants
            .retained_grant(&parent_ref)
            .map_err(|refusal| match refusal {
                GrantRefusal::Unavailable => {
                    AccessOfferError::Unavailable("the grant owner cannot say now".into())
                }
                _ => no_rule(AccessOfferRefusalReason::NoParentGrant),
            })?;
        if parent.grantee != calling.executor {
            return Err(no_rule(AccessOfferRefusalReason::NoParentGrant));
        }
        let Some(rule) = parent.child_approval.clone() else {
            return Err(AccessOfferError::Refused(AccessOfferRefusal::of(
                AccessOfferRefusalKind::NoChildApprovalRule,
            )));
        };
        inner
            .grants
            .resolve_lineage(
                &parent_lineage,
                &calling.executor,
                parent.represented_subject.as_ref(),
            )
            .map_err(|refusal| match refusal {
                GrantRefusal::Unavailable => {
                    AccessOfferError::Unavailable("the grant owner cannot say now".into())
                }
                _ => no_rule(AccessOfferRefusalReason::ParentNotCurrent),
            })?;

        // Where the answer goes.
        let origin = ApprovalOrigin {
            realm: calling.realm.clone(),
            ingress: calling.ingress.clone(),
            work: calling.origin_work().clone(),
            requester: calling.requester.clone(),
        };
        if !inner.replies.can_deliver(&origin) {
            return Err(AccessOfferError::Refused(AccessOfferRefusal::of(
                AccessOfferRefusalKind::ReplyRouteUnavailable,
            )));
        }

        // Where it runs: an approval branch of the agent that asked, under a
        // controller grant its own controller grant can admit, for the one
        // subject the offering grant acts for.
        let grantee = calling.grantee().clone();
        let branch_unavailable = |reason| {
            AccessOfferError::Refused(AccessOfferRefusal::because(
                AccessOfferRefusalKind::ApprovalBranchUnavailable,
                reason,
            ))
        };
        let basis = inner
            .branches
            .grantee_basis(&grantee)
            .ok_or_else(|| branch_unavailable(AccessOfferRefusalReason::NotBranchable))?;
        if basis.represented_subject != parent.represented_subject {
            return Err(branch_unavailable(
                AccessOfferRefusalReason::SubjectMismatch,
            ));
        }
        let kind = inner.kind.clone();
        let subject_key = grant_child_subject_key(
            &parent_ref.grant_id,
            &grantee,
            calling.origin_work(),
            &operation.request_digest,
        )
        .ok_or_else(|| {
            AccessOfferError::InvalidArguments("the offer's identifiers are too long".into())
        })?;
        let decision_id = DecisionId::derive(&kind, &subject_key);
        let branch = approval_branch_principal(&grantee, &decision_id).ok_or_else(|| {
            AccessOfferError::InvalidArguments("the asking agent has no qualified principal".into())
        })?;
        let intent = branch_controller_intent(&decision_id, &grantee, &basis, &branch)
            .ok_or_else(|| AccessOfferError::Unavailable("no branch controller id".into()))?;
        inner
            .grants
            .child_admissible(
                &intent,
                &[ProcessorRef::Principal {
                    principal: branch.clone(),
                }],
            )
            .map_err(|refusal| match refusal {
                ChildGrantRefusal::Denied => {
                    branch_unavailable(AccessOfferRefusalReason::ControllerCannotAdmitChild)
                }
                _ => AccessOfferError::Unavailable("the grant owner cannot say now".into()),
            })?;

        // The exact terms the grant would be issued with now.
        let now_ms = self
            .now_ms()
            .ok_or_else(|| AccessOfferError::Unavailable("the clock cannot be read".into()))?;
        let effective = DerivedChildRestrictions::new(
            parent.restrictions.clone(),
            approval_child_request(now_ms, duration_ms),
        )
        .map_err(|_| no_rule(AccessOfferRefusalReason::ParentCannotDelegate))?
        .effective()
        .clone();
        // An offer whose grant could never be issued with its terms, or
        // whose approved work every use would be denied, is never held: the
        // offering grant's window must cover the requested one, and its
        // bounds must admit the approval branch as processor and the
        // requester as audience.
        if let LifetimeBound::Window {
            not_before_ms,
            expires_at_ms,
        } = effective.lifetime.bound()
            && expires_at_ms.saturating_sub(not_before_ms) < duration_ms
        {
            return Err(AccessOfferError::Refused(AccessOfferRefusal::of(
                AccessOfferRefusalKind::WindowOverParent,
            )));
        }
        let outside = |reason| {
            AccessOfferError::Refused(AccessOfferRefusal::because(
                AccessOfferRefusalKind::OutsideParentBounds,
                reason,
            ))
        };
        if matches!(
            effective.processors.check(&ProcessorRef::Principal {
                principal: branch.clone(),
            }),
            BoundMatch::OutsideBounds
        ) {
            return Err(outside(AccessOfferRefusalReason::ProcessorExcluded));
        }
        if matches!(
            effective.audiences.check(&AudienceRef::Principal {
                principal: calling.requester.clone(),
            }),
            BoundMatch::OutsideBounds
        ) {
            return Err(outside(AccessOfferRefusalReason::AudienceExcluded));
        }
        let work = OriginalWorkRef {
            authority: calling.ingress.authority.clone(),
            work: approved_work_id(&decision_id)
                .ok_or_else(|| AccessOfferError::Unavailable("no approved work id".into()))?,
        };
        let terms = child_terms(
            &parent_ref.grant_id,
            &parent,
            &calling.executor,
            &branch,
            &effective,
            &operation,
            uses,
            &calling.requester,
            &work,
        )
        .ok_or_else(|| no_rule(AccessOfferRefusalReason::ParentNotCurrent))?;
        let request_digest = grant_child_terms_digest(&terms)
            .ok_or_else(|| AccessOfferError::Unavailable("the terms cannot be encoded".into()))?;
        let request = GrantChildRequest {
            format: GRANT_CHILD_REQUEST_FORMAT.to_owned(),
            parent_lineage,
            approval_branch_of: grantee,
            duration_ms,
            terms,
            operation,
            origin,
            summary: arguments.summary,
        };
        let body = request
            .body()
            .map_err(|error| AccessOfferError::InvalidArguments(error.to_string()))?;
        let rendering = render(&request, &rule.approver_id).ok_or_else(|| {
            AccessOfferError::InvalidArguments("the offer cannot be rendered".into())
        })?;
        let requester = DecisionPrincipalKey::of(&calling.requester).map_err(|error| {
            AccessOfferError::InvalidArguments(format!("the requester has no key: {error}"))
        })?;
        let operation_id = grant_child_operation_id(&decision_id)
            .ok_or_else(|| AccessOfferError::Unavailable("no operation id".into()))?;
        let hold = DecisionHoldRequest::new(
            kind,
            subject_key,
            operation_id,
            request_digest,
            body,
            requester,
            rule.approver_id,
            now_ms.saturating_add(policy.decision_window_ms),
            rendering,
            inner.renderer.clone(),
        );
        Ok(inner.decisions.hold(hold, true)?)
    }
}

/// The decision kind of access offers.
pub(crate) fn grant_child_kind() -> Option<DecisionKind> {
    DecisionKind::new(super::GRANT_CHILD_KIND).ok()
}

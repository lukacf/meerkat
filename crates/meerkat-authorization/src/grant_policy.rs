//! Current generated grants composed with the actual native and resource owners.
//!
//! This module keeps no admission, identity, resource or grant registry. The
//! trusted owner interfaces below must read their existing canonical facts. A
//! decoded association, selected credential or successful constructor is never
//! accepted work or permission. All owner mutations participate in the grant
//! authority's publication; evaluation is local and performs no I/O.

use std::sync::Arc;

use meerkat_authorization_contracts::constraints::{ExecutionRestrictions, LifetimeBound};
use meerkat_authorization_contracts::work_association::{
    InputAuthorityAssociation, WorkAuthorityBasis,
};
use meerkat_core::authorization::{
    AuthorizationOperation, ModelAuthorizationUse, OperationRefusalKind, OperationRefused,
    PreparedAuthorizationBinding, WorkAuthorizationContext,
};
use meerkat_core::exact_operation::OperationExecutionScope;

use crate::grants::{GrantRefusal, LocalGrantAuthority, ResolvedGrant};
use crate::policy::{
    LocalOperationValues, LocalPolicyAllowance, LocalPolicyPurpose, LocalWorkPolicy,
};
use crate::publication::{LocalPublicationGuard, PublicationError};

/// Current owner bounds, not a capability, cached acceptance or wire DTO.
///
/// Only an installed trusted owner implementation supplies this value. Its
/// expiry covers the current caller/mandate/policy decision for the requested
/// purpose. The generated grant's independent expiry is conjoined afterwards.
pub struct WorkOwnerAllowance {
    pub restrictions: ExecutionRestrictions,
    pub expires_at_ms: u64,
}

impl std::fmt::Debug for WorkOwnerAllowance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkOwnerAllowance").finish_non_exhaustive()
    }
}

/// Existing native input/mandate owner, installed by the trusted embedding.
///
/// Resolve the complete association against the actual accepted input and exact
/// binding execution scope, run and context. Check the actual requester may
/// currently invoke this delegation for its executor and represented subject;
/// an agent holding the grant is insufficient. Do not infer the requester from
/// an agent owner, credential/account, InputOrigin or the association itself.
///
/// For Operation, HostPolicy and ServiceMandate must resolve the exact current
/// owner decision/occurrence here. Their presence in a claim is not a fallback
/// permission. GrantLineage is subsequently resolved by the generated owner.
/// For Controller, independently resolve the admitted controller mandate and
/// route; ordinary operation authority is not a prerequisite or substitute.
/// Missing ownership, unsupported scope and any failed conjunct must refuse.
///
/// Every relevant accepted mutation must use the same publication as the grant
/// authority. This synchronous method cannot perform I/O or hold a native lock
/// that is already held by its caller. Real runtime/schedule adapters must bind
/// this contract to their own admitted records; this trait creates none.
pub trait AdmittedWorkPolicyOwner: Send + Sync {
    fn authorize_admitted_work(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<WorkOwnerAllowance, meerkat_core::OperationAuthorizationError>;
}

/// Existing resource, peer, account and sink policy composition.
///
/// Resolve the actual bound operation against its canonical target owners and
/// require every relevant owner to allow. Preserve correlated rule semantics
/// when mapping actual action/resource/processor/audience tuples. In particular,
/// endpoint, wire model, hosted capabilities and account access are independent
/// of controller selection and credential possession. A resource's metadata or
/// historical source observation cannot replace its access decision.
///
/// For Controller return only the actual model-route tuples. For Operation on
/// ControllerInference return every actual hosted-capability tuple; do not add
/// the separately authorized controller tuple or omit an enabled capability.
/// For ordinary operations return all their actual tuples.
///
/// The returned bounds and deadline are current owner facts, read under the
/// enclosing publication observation. This is a trusted extension seam for
/// application ABAC, not a generic policy language or an allow-by-default hook.
pub trait OperationPolicyOwner: Send + Sync {
    /// Authorize this exact plain controller before native admission. The native
    /// ingress owner separately checks the current authenticated caller's right
    /// to invoke the mandate. No operation, run or transcript exists yet.
    ///
    /// Resolve current correlated account/endpoint/wire-model rules. This
    /// allowance must last for admitted work: explicit unrestricted lifetime,
    /// with removal protected by complete native controller custody and this
    /// grant authority's publication. A finite operation-cache deadline cannot
    /// stand in for that contract. Do not perform I/O or infer account authority
    /// from possession of credentials. Unsupported owners fail closed.
    fn authorize_controller_admission(
        &self,
        _association: &InputAuthorityAssociation,
        _facts: &meerkat_core::ControllerModelFacts,
        _now_ms: u64,
    ) -> Result<ControllerAdmissionAllowance, meerkat_core::OperationAuthorizationError> {
        Err(meerkat_core::OperationAuthorizationError::Unavailable)
    }

    fn authorize_operation(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError>;
}

/// Current correlated controller rules lasting for admitted work.
/// This is trusted owner data, not a serialized permission or entry permit.
pub struct ControllerAdmissionAllowance {
    pub operation_values: Vec<LocalOperationValues>,
    pub restrictions: ExecutionRestrictions,
}

impl std::fmt::Debug for ControllerAdmissionAllowance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControllerAdmissionAllowance")
            .finish_non_exhaustive()
    }
}

/// The exact grant publication's reserved writer, not policy authority.
///
/// The host must first hold complete native controller custody, then reserve
/// this writer, then lock its actual policy state. Validate the proposal under
/// those locks. On success replace state and publish before unlocking policy;
/// on refusal change nothing. Do not hold this guard across I/O or await.
/// A panic poisons publication; an unchanged rejected proposal does not.
pub struct ControllerPolicyChange<'a> {
    publication: LocalPublicationGuard<'a>,
}

impl ControllerPolicyChange<'_> {
    /// Publish only after successful replacement, while the policy mutex still
    /// hides the changed facts. Retain native custody until this guard drops.
    pub fn publish(&mut self) {
        self.publication.publish();
    }
}

/// Conjunction using one explicitly selected actual generated grant authority.
/// References naming another authority refuse; claims never select a new owner.
pub struct GrantBackedWorkPolicy {
    grants: Arc<LocalGrantAuthority>,
    work_owner: Arc<dyn AdmittedWorkPolicyOwner>,
    operation_owner: Arc<dyn OperationPolicyOwner>,
}

impl std::fmt::Debug for GrantBackedWorkPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GrantBackedWorkPolicy")
            .finish_non_exhaustive()
    }
}

impl GrantBackedWorkPolicy {
    /// Native production factory. The actual admitted row supplies this sink;
    /// a logging-only exporter is not a substitute. Entry staging must succeed
    /// before a body runs, and observations join that row's next existing commit.
    pub fn audited_work_context(
        self: &Arc<Self>,
        associations: Arc<[InputAuthorityAssociation]>,
        execution_scope: OperationExecutionScope,
        audit_sink: Arc<dyn meerkat_authorization_contracts::audit::AuthorizationAuditSink>,
    ) -> Result<WorkAuthorizationContext, OperationRefused> {
        let policy: Arc<dyn LocalWorkPolicy> = self.clone();
        self.grants
            .audited_work_context(associations, execution_scope, policy, audit_sink)
    }

    /// Install trusted existing owner adapters, never adapters selected by a
    /// wire claim. Construction does not accept work or resolve any grant.
    pub fn new(
        grants: Arc<LocalGrantAuthority>,
        work_owner: Arc<dyn AdmittedWorkPolicyOwner>,
        operation_owner: Arc<dyn OperationPolicyOwner>,
    ) -> Self {
        Self {
            grants,
            work_owner,
            operation_owner,
        }
    }

    /// Rebuild a context from the native owner's complete retained contributors.
    /// The compiler uses this grant authority's own publication and clock;
    /// callers cannot substitute an unrelated invalidation source here.
    ///
    /// This validates batch shape only. Each preparation still resolves every
    /// original association and exact execution scope through `work_owner`.
    pub fn work_context(
        self: &Arc<Self>,
        associations: Arc<[InputAuthorityAssociation]>,
        execution_scope: OperationExecutionScope,
    ) -> Result<WorkAuthorizationContext, OperationRefused> {
        let policy: Arc<dyn LocalWorkPolicy> = self.clone();
        self.grants
            .work_context(associations, execution_scope, policy)
    }

    /// Validate current controller grant/account policy before acceptance.
    /// This never authenticates a caller, admits work or returns an entry permit.
    /// The native ingress owner performs its independent current invocation
    /// check and excludes a protected policy removal until acceptance commits.
    pub fn validate_controller_admission(
        &self,
        association: &InputAuthorityAssociation,
        facts: &meerkat_core::ControllerModelFacts,
    ) -> Result<(), meerkat_core::OperationAuthorizationError> {
        self.grants.observe_controller_admission(|now_ms| {
            let candidate = association.candidate();
            if candidate.controller_model.as_ref() != Some(facts.selection()) {
                return Err(denied().into());
            }
            let grant = self
                .grants
                .resolve_controller_lineage(
                    &candidate.controller_grant_lineage,
                    &candidate.logical_executor,
                    candidate.represented_subject.as_ref(),
                )
                .map_err(grant_refusal)?;
            let allowance =
                self.operation_owner
                    .authorize_controller_admission(association, facts, now_ms)?;
            if allowance.operation_values.is_empty()
                || !matches!(
                    allowance.restrictions.lifetime.bound(),
                    LifetimeBound::Unrestricted
                )
                || !matches!(
                    candidate.controller_ceiling.lifetime.bound(),
                    LifetimeBound::Unrestricted
                )
            {
                return Err(denied().into());
            }
            let restrictions = allowance
                .restrictions
                .conjoin(&candidate.controller_ceiling)
                .conjoin(grant.restrictions());
            for values in &allowance.operation_values {
                restrictions
                    .check_bounds(values.at(now_ms))
                    .map_err(|_| denied())?;
            }
            Ok(())
        })
    }

    /// Reserve this policy's exact publication for a controller-preserving
    /// proposed change. The caller still owns all policy decisions and must
    /// follow `ControllerPolicyChange`'s native/policy lock and commit contract.
    pub fn reserve_controller_policy_change(
        &self,
    ) -> Result<ControllerPolicyChange<'_>, PublicationError> {
        Ok(ControllerPolicyChange {
            publication: self.grants.reserve_controller_policy_change()?,
        })
    }

    fn resolve_grant(
        &self,
        association: &InputAuthorityAssociation,
        controller: bool,
    ) -> Result<Option<ResolvedGrant>, meerkat_core::OperationAuthorizationError> {
        let candidate = association.candidate();
        let lineage = if controller {
            &candidate.controller_grant_lineage
        } else {
            match &candidate.authority_basis {
                WorkAuthorityBasis::GrantLineage { lineage } => lineage,
                // Only reached after the trusted work owner resolved this
                // exact policy or occurrence. No implicit grant is synthesized.
                WorkAuthorityBasis::HostPolicy { .. }
                | WorkAuthorityBasis::ServiceMandate { .. } => {
                    return Ok(None);
                }
            }
        };
        let resolved = if controller {
            self.grants.resolve_controller_lineage(
                lineage,
                &candidate.logical_executor,
                candidate.represented_subject.as_ref(),
            )
        } else {
            self.grants.resolve_lineage(
                lineage,
                &candidate.logical_executor,
                candidate.represented_subject.as_ref(),
            )
        };
        resolved.map(Some).map_err(grant_refusal)
    }
}

impl LocalWorkPolicy for GrantBackedWorkPolicy {
    fn evaluate(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        now_ms: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        let purpose = match &binding.facts().operation {
            AuthorizationOperation::Model(facts)
                if facts.usage == ModelAuthorizationUse::ControllerInference =>
            {
                // The legacy single-allowance method cannot encode independent
                // model and hosted-capability bounds. The compiler invokes
                // evaluate_for twice for this exact same binding instead.
                if !facts.hosted_capabilities.is_empty() {
                    return Err(denied().into());
                }
                LocalPolicyPurpose::Controller
            }
            _ => LocalPolicyPurpose::Operation,
        };
        self.evaluate_for(association, binding, purpose, now_ms)
    }

    fn evaluate_for(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        let controller = match &binding.facts().operation {
            AuthorizationOperation::Model(facts)
                if facts.usage == ModelAuthorizationUse::ControllerInference =>
            {
                Some(facts)
            }
            _ => None,
        };
        if purpose == LocalPolicyPurpose::Controller && controller.is_none()
            || purpose == LocalPolicyPurpose::Operation
                && controller.is_some_and(|facts| facts.hosted_capabilities.is_empty())
        {
            return Err(denied().into());
        }
        // Authentication and actual caller entitlement precede grant lookup.
        // Each role is resolved independently; one cannot stand in for another.
        let owner =
            self.work_owner
                .authorize_admitted_work(association, binding, purpose, now_ms)?;
        if purpose == LocalPolicyPurpose::Controller {
            let facts = controller.ok_or_else(denied)?;
            let selected = association
                .candidate()
                .controller_model
                .as_ref()
                .ok_or_else(denied)?;
            if !selected.matches_model_facts(facts) {
                return Err(denied().into());
            }
        }
        let grant = self.resolve_grant(association, purpose == LocalPolicyPurpose::Controller)?;
        let mut operation =
            self.operation_owner
                .authorize_operation(association, binding, purpose, now_ms)?;
        operation.restrictions = owner.restrictions.conjoin(&operation.restrictions);
        operation.expires_at_ms = owner.expires_at_ms.min(operation.expires_at_ms);
        if let Some(grant) = grant {
            operation.restrictions = operation.restrictions.conjoin(grant.restrictions());
            operation.expires_at_ms = operation.expires_at_ms.min(grant.expires_at_ms());
        }
        if operation.operation_values.is_empty() || operation.expires_at_ms <= now_ms {
            return Err(denied().into());
        }
        // A direct policy caller also receives no allowance for known exclusion
        // or unresolved required facts. The work compiler separately conjoins
        // the purpose's retained admitted ceiling and every original input.
        for values in &operation.operation_values {
            operation
                .restrictions
                .check_bounds(values.at(now_ms))
                .map_err(|_| denied())?;
        }
        Ok(operation)
    }
}

fn denied() -> OperationRefused {
    OperationRefused::new(OperationRefusalKind::Denied)
}

fn grant_refusal(refusal: GrantRefusal) -> meerkat_core::OperationAuthorizationError {
    match refusal {
        GrantRefusal::Denied => denied().into(),
        // ControllerInUse belongs to administration and is preserved by revoke.
        // Current resolution cannot produce it; never invent policy denial if
        // a future resolver reports an unsupported operational condition.
        GrantRefusal::ControllerInUse | GrantRefusal::Unavailable => {
            meerkat_core::OperationAuthorizationError::Unavailable
        }
    }
}

#[cfg(test)]
mod tests;

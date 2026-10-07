//! The feature-owned contract for current local policy composition.
//!
//! A policy implementation reads the canonical work, grant, resource and sink
//! owners. It is a trusted host component, not a caller-supplied permit and not
//! a registry populated from serialized source claims. Application ABAC remains
//! at those owners; this seam supplies their conjunction to the shared gates.

use meerkat_authorization_contracts::constraints::{
    ActionRef, AudienceRef, ExecutionRestrictions, OperationRestrictionValues, ProcessorRef,
    ResourceDomain,
};
use meerkat_authorization_contracts::work_association::InputAuthorityAssociation;
use meerkat_core::authorization::PreparedAuthorizationBinding;

/// Exact correlated operation values selected from actual resolved owner facts.
///
/// Keep alternative tuples separate. Turning `(action A, source A)` and
/// `(action B, source B)` into independent sets would wrongly allow the cross
/// combinations. Neither model-authored arguments nor an unverified endpoint
/// name can establish the authority of a target.
#[derive(Clone, PartialEq, Eq)]
pub struct LocalOperationValues {
    pub action: ActionRef,
    pub resource_domain: ResourceDomain,
    pub processor: ProcessorRef,
    pub audience: AudienceRef,
}

impl std::fmt::Debug for LocalOperationValues {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalOperationValues")
            .finish_non_exhaustive()
    }
}

impl LocalOperationValues {
    pub(crate) fn at(&self, now_ms: u64) -> OperationRestrictionValues<'_> {
        OperationRestrictionValues {
            action: &self.action,
            resource_domain: &self.resource_domain,
            processor: &self.processor,
            audience: &self.audience,
            now_ms,
        }
    }
}

/// Freshly conjoined policy-owner facts for one exact bound operation.
///
/// This nonserializable value is only an input to compilation. It does not
/// authorize entry. The compiler also enforces the retained work ceiling and
/// the same coherent publication under which these facts were read.
pub struct LocalPolicyAllowance {
    /// Every actual source/processor/recipient tuple, including implicit targets.
    /// An empty list is refused; it cannot mean unrestricted operation.
    pub operation_values: Vec<LocalOperationValues>,
    /// Conjunction of current work, grant-ancestor, domain and sink ceilings.
    pub restrictions: ExecutionRestrictions,
    /// Earliest exclusive expiry among the authorities resolved for this actual
    /// operation, in trusted local Unix milliseconds. Preparation cannot renew
    /// an authority or convert historical audit observations into permissions.
    pub expires_at_ms: u64,
}

/// Independent authority conjunct for one exact prepared operation.
///
/// A controller model request with hosted capabilities checks both purposes.
/// Controller tuples describe the model route; Operation tuples describe its
/// actual hosted capabilities. Ordinary requests use only Operation. Neither
/// role may flatten or union the correlated tuples belonging to the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalPolicyPurpose {
    Controller,
    Operation,
}

impl std::fmt::Debug for LocalPolicyAllowance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalPolicyAllowance")
            .finish_non_exhaustive()
    }
}

/// Canonical policy composition invoked once when a bound operation is prepared.
///
/// Implementations must resolve the retained association from its real native
/// input/mandate owner and obtain current requester, executor, represented
/// subject, scoped delegation, grant ancestors, resource-access and actual sink
/// decisions. The actual requester must be entitled to invoke the exact mandate;
/// an agent holding another user's delegation is not sufficient. For controller
/// inference, resolve the separate controller lineage and its actual model route.
/// Provider-hosted capabilities additionally require ordinary operation grants;
/// a controller grant cannot authorize an enabled hosted tool. Compaction and
/// live operations continue to require ordinary operation authority in this slice.
/// Every owner must allow;
/// deny or missing authority refuses only the affected operation. Historical
/// source observations in the association are audit data, never current access.
///
/// All local mutations that can change the answer must participate in this
/// context's `LocalAuthorizationPublication`. Evaluation is synchronous and
/// local: it may not add a refresh round trip or disk commit. The implementation
/// must retain correlated rule meaning and return the earliest authority expiry.
/// It does not infer semantic dependencies or constrain model-derived content.
/// A host cannot advertise LocalGovernedV1 merely by implementing this trait.
pub trait LocalWorkPolicy: Send + Sync {
    fn evaluate(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        now_ms: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError>;

    /// Resolve the requested authority purpose for the same immutable binding.
    /// Existing single-role policies retain their conservative behavior through
    /// this default. Compositions with distinct controller/hosted permissions
    /// must return only that purpose's actual tuples and bounds.
    fn evaluate_for(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        _purpose: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        self.evaluate(association, binding, now_ms)
    }
}

/// Exact correlated rules for policy owners whose complete domain rule really
/// is a finite relation. This helper does not authenticate a principal, issue a
/// grant, flatten an ABAC expression or supply missing policy-owner decisions.
pub struct ExactOperationRelation {
    allowed: Vec<LocalOperationValues>,
}

impl ExactOperationRelation {
    #[must_use]
    pub fn new(allowed: Vec<LocalOperationValues>) -> Self {
        Self { allowed }
    }

    #[must_use]
    pub fn contains(&self, actual: &LocalOperationValues) -> bool {
        self.allowed.contains(actual)
    }
}

impl std::fmt::Debug for ExactOperationRelation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExactOperationRelation")
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_core::{PrincipalKind, PrincipalRef, TrustDomainId};

    fn tuple(action: &str, namespace: &str) -> LocalOperationValues {
        let owner = PrincipalRef::in_domain(
            PrincipalKind::ServiceAccount,
            "owner",
            TrustDomainId::new("test").expect("domain"),
        )
        .expect("owner");
        LocalOperationValues {
            action: ActionRef {
                feature: "test".into(),
                action: action.into(),
            },
            resource_domain: ResourceDomain {
                authority: owner.clone(),
                namespace: namespace.into(),
            },
            processor: ProcessorRef::Principal {
                principal: owner.clone(),
            },
            audience: AudienceRef::Principal { principal: owner },
        }
    }

    #[test]
    fn alternatives_never_widen_into_cartesian_permissions() {
        let rules =
            ExactOperationRelation::new(vec![tuple("read", "public"), tuple("write", "private")]);
        assert!(rules.contains(&tuple("read", "public")));
        assert!(rules.contains(&tuple("write", "private")));
        assert!(!rules.contains(&tuple("write", "public")));
        assert!(!rules.contains(&tuple("read", "private")));
        assert!(!ExactOperationRelation::new(vec![]).contains(&tuple("read", "public")));
    }
}

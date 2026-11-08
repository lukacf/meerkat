//! Explicit process-owned review requirements for installed tool operations.
//! Tool metadata, Console consent rules and the model do not select this owner.
use meerkat_authorization::policy::LocalPolicyAllowance;
use meerkat_authorization_contracts::work_association::InputAuthorityAssociation;
use meerkat_core::{
    OperationAuthorizationError, ToolOperationTarget,
    authorization::{OperationReviewTier, PreparedAuthorizationBinding},
};
use meerkat_toolkit::host_lifetime::policy::{CompiledLocalPolicy, CompiledNativeHost};

/// Borrowed facts from the actual installed operation policy, after its normal
/// local authorization and workflow narrowing. No constructor is exposed.
/// These facts alone are not a grant or proof that native entry has occurred.
pub struct InstalledOperationReviewContext<'a> {
    pub(super) selected: &'a CompiledLocalPolicy,
    pub(super) native: &'a CompiledNativeHost,
    pub(super) association: &'a InputAuthorityAssociation,
    pub(super) binding: &'a PreparedAuthorizationBinding,
    pub(super) target: &'a ToolOperationTarget,
    pub(super) allowance: &'a LocalPolicyAllowance,
}
impl InstalledOperationReviewContext<'_> {
    pub fn selected(&self) -> &CompiledLocalPolicy {
        self.selected
    }
    pub fn native(&self) -> &CompiledNativeHost {
        self.native
    }
    pub fn association(&self) -> &InputAuthorityAssociation {
        self.association
    }
    pub fn binding(&self) -> &PreparedAuthorizationBinding {
        self.binding
    }
    pub fn target(&self) -> &ToolOperationTarget {
        self.target
    }
    pub fn allowance(&self) -> &LocalPolicyAllowance {
        self.allowance
    }
}

/// Trusted synchronous application policy. Implementations may inspect the
/// supplied typed native target and exact owner selection. They must not call
/// back into the host lifetime or perform asynchronous work while native policy
/// custody is held. The selected review requirements must stay immutable for
/// the composed host's lifetime. Changing them requires a new trusted host
/// composition; this API does not enroll an independent policy publication.
///
/// Returning a tier cannot grant an operation, change values or restrictions,
/// extend a deadline, lower existing review, or replace current entry checks.
/// Returning an error preserves that refusal. R3 still requires a separately
/// selected real BoundOperationReview and HumanConsentRoute to be usable.
pub trait InstalledOperationReviewPolicy: Send + Sync {
    fn review_floor(
        &self,
        context: InstalledOperationReviewContext<'_>,
    ) -> Result<OperationReviewTier, OperationAuthorizationError>;
}

//! Forward the native configuration's retained policy to its final owner.
//! No invocation is authorized during construction. Adoption installs the
//! actual current installed policy once, without changing the source owner.

use super::*;
use std::sync::OnceLock;

pub struct PreparedApplicationPolicy {
    originals: Arc<PrivateOriginals>,
    installed: OnceLock<InstalledApplicationPolicy>,
}

impl PreparedApplicationPolicy {
    pub(super) fn new(originals: Arc<PrivateOriginals>) -> Self {
        Self { originals, installed: OnceLock::new() }
    }

    pub(super) fn adopt(&self, installed: InstalledApplicationPolicy)
        -> Result<(), HostLifetimeError>
    {
        if !Arc::ptr_eq(&self.originals, &installed.originals) {
            return Err(HostLifetimeError::WrongOwner);
        }
        installed.lifetime.validate_current_selection()?;
        self.installed.set(installed).map_err(|_| HostLifetimeError::InvalidSelection)
    }

    fn installed(&self) -> Result<&InstalledApplicationPolicy, OperationAuthorizationError> {
        self.installed.get().ok_or(OperationAuthorizationError::Unavailable)
    }

    fn admission(&self) -> Result<&InstalledApplicationPolicy, NativeAdmissionError> {
        self.installed.get().ok_or(NativeAdmissionError::Readiness(
            meerkat_core::authorization::ControllerReadinessFailure::PolicyUnavailable,
        ))
    }
}

impl ScopedMandateOwner for PreparedApplicationPolicy {
    fn authorize_invocation(&self, ingress: &NativeIngressContext,
        association: &InputAuthorityAssociation) -> Result<(), NativeAdmissionError>
    {
        self.admission()?.authorize_invocation(ingress, association)
    }
}

impl RetainedInvocationOwner for PreparedApplicationPolicy {
    fn reauthorize_retained(&self, association: &InputAuthorityAssociation)
        -> Result<(), NativeAdmissionError>
    {
        self.admission()?.reauthorize_retained(association)
    }
}

impl AdmittedWorkPolicyOwner for PreparedApplicationPolicy {
    fn authorize_mcp_attachment(&self,
        control: &meerkat_core::mcp_attachment_control::McpAttachmentControlRequest,
        binding: &PreparedAuthorizationBinding, now_ms: u64,
    ) -> Result<WorkOwnerAllowance, OperationAuthorizationError> {
        AdmittedWorkPolicyOwner::authorize_mcp_attachment(self.installed()?, control, binding, now_ms)
    }

    fn authorize_admitted_work(&self, association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding, purpose: LocalPolicyPurpose, now_ms: u64,
    ) -> Result<WorkOwnerAllowance, OperationAuthorizationError> {
        self.installed()?.authorize_admitted_work(association, binding, purpose, now_ms)
    }
}

impl OperationPolicyOwner for PreparedApplicationPolicy {
    fn authorize_mcp_attachment(&self,
        control: &meerkat_core::mcp_attachment_control::McpAttachmentControlRequest,
        binding: &PreparedAuthorizationBinding, now_ms: u64,
    ) -> Result<LocalPolicyAllowance, OperationAuthorizationError> {
        OperationPolicyOwner::authorize_mcp_attachment(self.installed()?, control, binding, now_ms)
    }

    fn authorize_controller_admission(&self, association: &InputAuthorityAssociation,
        facts: &meerkat_core::ControllerModelFacts, now_ms: u64,
    ) -> Result<ControllerAdmissionAllowance, OperationAuthorizationError> {
        self.installed()?.authorize_controller_admission(association, facts, now_ms)
    }

    fn authorize_operation(&self, association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding, purpose: LocalPolicyPurpose, now_ms: u64,
    ) -> Result<LocalPolicyAllowance, OperationAuthorizationError> {
        self.installed()?.authorize_operation(association, binding, purpose, now_ms)
    }
}

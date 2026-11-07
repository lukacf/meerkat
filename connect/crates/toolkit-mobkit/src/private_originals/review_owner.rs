//! Observe Core's final invocation-owner mandate without changing its answer.
//! This sits outside NamedContributorWork, not around the installed policy.

use crate::installed_host::review_diagnostics::ReviewObservations;
use meerkat_authorization::{
    grant_policy::{AdmittedWorkPolicyOwner, WorkOwnerAllowance},
    policy::LocalPolicyPurpose,
};
use meerkat_authorization_contracts::work_association::{
    InputAuthorityAssociation, OriginalWorkRef, QualifiedIngressNamespace,
};
use meerkat_core::{OperationAuthorizationError, authorization::PreparedAuthorizationBinding};
use meerkat_runtime::meerkat_machine::NativeGrantWorkConfiguration;
use std::sync::Arc;

pub(crate) fn observe_configuration(
    mut configuration: NativeGrantWorkConfiguration,
    observations: Arc<ReviewObservations>,
) -> NativeGrantWorkConfiguration {
    // Keep Core's exact owner, including its original-source/contributor checks.
    // All other configuration owners and requirements remain the same objects.
    configuration.invocation_owner = Arc::new(ObservedInvocationOwner {
        inner: configuration.invocation_owner,
        observations,
    });
    configuration
}

struct ObservedInvocationOwner {
    inner: Arc<dyn AdmittedWorkPolicyOwner>,
    observations: Arc<ReviewObservations>,
}

impl AdmittedWorkPolicyOwner for ObservedInvocationOwner {
    fn authorize_tool_application(
        &self,
        control: &meerkat_core::ToolApplicationControlRequest,
        binding: &PreparedAuthorizationBinding,
        now_ms: u64,
    ) -> Result<WorkOwnerAllowance, OperationAuthorizationError> {
        self.inner
            .authorize_tool_application(control, binding, now_ms)
    }

    fn authorize_context_control(
        &self,
        control: &meerkat_core::service::SystemContextControlRequest,
        binding: &PreparedAuthorizationBinding,
        now_ms: u64,
    ) -> Result<WorkOwnerAllowance, OperationAuthorizationError> {
        self.inner
            .authorize_context_control(control, binding, now_ms)
    }

    fn authorize_mcp_attachment(
        &self,
        control: &meerkat_core::mcp_attachment_control::McpAttachmentControlRequest,
        binding: &PreparedAuthorizationBinding,
        now_ms: u64,
    ) -> Result<WorkOwnerAllowance, OperationAuthorizationError> {
        self.inner
            .authorize_mcp_attachment(control, binding, now_ms)
    }

    fn authorize_admitted_work(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<WorkOwnerAllowance, OperationAuthorizationError> {
        self.inner
            .authorize_admitted_work(association, binding, purpose, now_ms)
    }

    fn review_mandate(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        now_ms: u64,
    ) -> Result<Option<String>, OperationAuthorizationError> {
        let result = self.inner.review_mandate(association, binding, now_ms);
        crate::installed_host::review_diagnostics::native_mandate(&self.observations, result)
    }

    fn enrolled_service_original(
        &self,
        namespace: &QualifiedIngressNamespace,
        original: &OriginalWorkRef,
    ) -> Result<Option<InputAuthorityAssociation>, OperationAuthorizationError> {
        self.inner.enrolled_service_original(namespace, original)
    }
}

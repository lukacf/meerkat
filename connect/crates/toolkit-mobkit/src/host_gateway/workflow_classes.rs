//! Shared workflow construction through the existing class Add owner.
use super::*;
use crate::{
    installed_host::{dynamic_members::AdmittedMemberClass, preparation::PreparedControllerIntent},
    private_originals::{PreparedWorkflowClassSource, PrivateAuthenticatedEvent},
};
use meerkat_mobkit::identity_first::{AgentAddressability, DurableAgentSpec};
use meerkat_toolkit::{
    host_lifetime::policy::{
        CompiledMemberClassInstance, CompiledMemberClassPurpose, CompiledNativeSourceRoute,
    },
    setup::SetupWorkflowTerms,
};
use std::future::Future;
use toolkit_package::Id;
fn refused(error: PrivateOriginalError) -> HostLifetimeError {
    match error {
        PrivateOriginalError::Unavailable => HostLifetimeError::Unavailable,
        _ => HostLifetimeError::Denied,
    }
}
/// A read of actual native Created publication, not a caller-supplied surface.
/// Resource owners must recheck the originating admission before using it.
pub struct CompletedWorkflowMemberClass {
    pub admission: crate::installed_host::dynamic_members::MemberClassAdmission,
    pub member: meerkat_toolkit::host_lifetime::policy::PreparedNativeMember,
    pub native_surface: Vec<meerkat_core::ToolIdentity>,
    pub workflow_member: Arc<crate::private_originals::InstalledWorkflowMember>,
}
impl ToolkitGatewayHost {
    /// This returns the already selected source owner, not a new commissioner
    /// or authenticated source. Every source operation retains its own checks.
    pub fn workflow_source(&self) -> Result<Arc<PrivateOriginals>, HostLifetimeError> {
        self.owner.validate_current_selection()?;
        if self.private_closed.is_closed() {
            return Err(HostLifetimeError::Denied);
        }
        self.private_originals
            .clone()
            .ok_or(HostLifetimeError::Unavailable)
    }
    /// Read the exact installed declaration under the current host selection.
    /// This carries no controller, source, grant or native admission custody.
    pub fn workflow_native_selection(
        &self,
    ) -> Result<meerkat_toolkit::host_lifetime::policy::CompiledNativeHost, HostLifetimeError> {
        self.workflow_source()?;
        self.owner
            .selected_policy()
            .native_host
            .clone()
            .ok_or(HostLifetimeError::Unavailable)
    }
    /// A supplied fresh commissioner must not prepare a second physical peer
    /// for a replayed admission. Only the original newly reserved handle has
    /// the existing one-shot Add token. Actual transfer still happens in Add.
    pub async fn reserve_fresh_workflow_member_class(
        &self,
        id: Id,
        source: &PreparedWorkflowClassSource,
        terms: &SetupWorkflowTerms,
    ) -> Result<Arc<AdmittedMemberClass>, HostLifetimeError> {
        let admitted = self
            .reserve_workflow_member_class(id, source, terms)
            .await?;
        if admitted.add_token.is_none() {
            return Err(HostLifetimeError::Denied);
        }
        Ok(admitted)
    }
    pub async fn retain_workflow_member_source(
        &self,
        instance: CompiledMemberClassInstance,
        parent: &PrivateOriginalRef,
        route: CompiledNativeSourceRoute,
        event: PrivateAuthenticatedEvent,
    ) -> Result<PreparedWorkflowClassSource, HostLifetimeError> {
        self.workflow_source()?
            .retain_workflow_class_source(instance, parent, route, event)
            .await
            .map_err(refused)
    }
    /// Reserve exact prepared source/terms under the same application policy.
    /// The returned handle is caller-owned until Add accepts its one-shot token.
    pub async fn reserve_workflow_member_class(
        &self,
        id: Id,
        source: &PreparedWorkflowClassSource,
        terms: &SetupWorkflowTerms,
    ) -> Result<Arc<AdmittedMemberClass>, HostLifetimeError> {
        let originals = self.workflow_source()?;
        // Fresh reservation permits physical peer preparation before Add.
        // Admit that call before its canonical write, independently of Add's
        // later ownership transfer. Entered reservation retains caller cleanup.
        originals.with_new_work_admission(|| ()).map_err(refused)?;
        Ok(Arc::new(
            originals.reserve_workflow_class(id, source, terms).await?,
        ))
    }
    pub fn completed_workflow_member_class(
        &self,
        admitted: &AdmittedMemberClass,
    ) -> Result<CompletedWorkflowMemberClass, HostLifetimeError> {
        let (admission, workflow_member) =
            self.workflow_source()?.completed_workflow_class(admitted)?;
        Ok(CompletedWorkflowMemberClass {
            member: admission.selected_member()?,
            native_surface: admission
                .native_surface
                .clone()
                .ok_or(HostLifetimeError::Unavailable)?,
            admission,
            workflow_member,
        })
    }
    /// Use the first context retained by this installed gateway. Shared G never
    /// acquires the ordinary Add's Console path or application MCP review path.
    pub async fn add_workflow_member_class<F, Fut>(
        self: &Arc<Self>,
        admitted: Arc<AdmittedMemberClass>,
        spec: DurableAgentSpec,
        route: CompiledNativeSourceRoute,
        controller: PreparedControllerIntent,
        deadline: meerkat_mob::HeldSpawnDeadline,
        finalizer: F,
        cleanup: Option<Arc<dyn MemberClassResourceCleanup>>,
    ) -> Result<MemberClassAddStart, MemberClassAddError>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), HostLifetimeError>> + Send + 'static,
    {
        if admitted.class().purpose != CompiledMemberClassPurpose::SharedWorkflow
            || admitted.workflow().is_none()
            || spec.addressability != AgentAddressability::InternalOnly
            || spec.context.is_some()
            || spec.binding.is_some()
        {
            return Err(MemberClassAddError::ReservationNotAccepted {
                error: HostLifetimeError::Denied,
            });
        }
        let context =
            self.member_class_context
                .get()
                .ok_or(MemberClassAddError::ReservationNotAccepted {
                    error: HostLifetimeError::Unavailable,
                })?;
        self.add_member_class_selected(
            context,
            admitted,
            spec,
            route,
            controller,
            deadline,
            move |_| finalizer(),
            None,
            cleanup,
        )
        .await
    }
}

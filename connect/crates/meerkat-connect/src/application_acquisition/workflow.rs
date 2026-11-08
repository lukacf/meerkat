//! Host-selected workflow invocation composition. The application that actually
//! launched the work retains the input-to-child association. This adapter does
//! not group ordinary Console calls, launch a scheduler, or own another result
//! store. Existing native continuation and source owners retain those effects.

use async_trait::async_trait;
use meerkat_authorization_contracts::work_association::InputAuthorityAssociation;
use meerkat_mobkit::identity_first::DurableAgentSpec;
use meerkat_toolkit::{
    continuation::{
        ContinuationAdmissionEvidence, ContinuationError, ContinuationObligation,
        ContinuationReconciliation, SetupContinuationDelivery, SetupContinuationOwner,
    },
    host_lifetime::control::HostLifetimeError,
    request::{ContextReference, TaskContinuation},
    setup::{SetupError, SetupWorkBackend, SetupWorkContext},
};
use std::{collections::BTreeSet, sync::Arc};
use toolkit_mobkit::{
    ToolAdmissionError,
    host_gateway::{
        MemberClassAddError, MemberClassAddOutcome, MemberClassAddStart,
        MemberClassCleanupObservation, MemberClassResourceCleanup, ToolkitGatewayHost,
    },
    installed_host::acquisition::{
        InstalledNativeContinuation, InstalledOriginalInvocation, InstalledOriginalTaskBinding,
        InstalledOriginalTaskOwner,
    },
    installed_host::{dynamic_members::AdmittedMemberClass, preparation::PreparedControllerIntent},
    private_originals::{InstalledWorkflowPlanning, PrivateOriginalRef, PrivateOriginals},
};
use toolkit_package::Id;
mod private_launch;
pub use private_launch::PrivateApplicationWorkflowLauncher;
mod installed;
pub use installed::InstalledWorkflowComposition;

/// The actual launch owner's retained child selection. Deserialized graph IDs
/// alone cannot produce the planning handle. The full native association and
/// original must match the current Find invocation, and the graph is reread.
pub struct ApplicationWorkflowInvocation {
    pub original: PrivateOriginalRef,
    pub association: InputAuthorityAssociation,
    pub work: SetupWorkContext,
    pub backend: SetupWorkBackend,
    pub planning: Arc<InstalledWorkflowPlanning>,
    pub output_audiences: BTreeSet<Id>,
}

/// Implemented by the application that issued workflow correlation before
/// launching its real children. An unavailable selected workflow is an error;
/// None is reserved for inputs the owner never selected into shared work.
#[async_trait]
pub trait ApplicationWorkflowLaunchOwner: Send + Sync {
    async fn prepare_console_child(
        &self,
        _request: toolkit_mobkit::private_originals::ConsoleWorkflowChildRequest,
        _login: toolkit_mobkit::private_host::ConsoleLoginHandle,
    ) -> Result<Option<PrivateOriginalRef>, toolkit_mobkit::private_host::PrivateWorkflowChildError>
    {
        Err(HostLifetimeError::Unavailable.into())
    }
    /// Retain the actual installed host and native context for later requested
    /// workflow commissioning. This synchronous binding must not wait for a
    /// future Console input during native bootstrap.
    fn bind_runtime(&self, runtime: ApplicationWorkflowRuntime) -> Result<(), HostLifetimeError>;

    /// Optional trusted private launch operation. Ordinary application owners
    /// do not expose their runtime or accept workflow wire selections by default.
    async fn prepare_private_child(
        &self,
        _request: toolkit_mobkit::private_originals::WorkflowChildLaunchRequest,
    ) -> Result<(), toolkit_mobkit::private_host::PrivateWorkflowChildError> {
        Err(HostLifetimeError::Unavailable.into())
    }

    async fn prepare_private_workflow_parent(
        &self,
        _request: toolkit_mobkit::private_host::WorkflowParentRequest,
    ) -> Result<(), HostLifetimeError> {
        Err(HostLifetimeError::Unavailable)
    }

    /// Optional supplied-host commissioning. The publication capability is
    /// supplied only by the existing private pipe's application ACK owner.
    async fn commission_private_workflow(
        &self,
        _request: toolkit_mobkit::private_host::WorkflowCommissionRequest,
        _publication: toolkit_mobkit::private_host::WorkflowMemberPublication,
    ) -> Result<toolkit_mobkit::private_host::WorkflowCommissionResponse, HostLifetimeError> {
        Err(HostLifetimeError::Unavailable)
    }

    async fn invocation(
        &self,
        invocation: &InstalledOriginalInvocation<'_>,
    ) -> Result<Option<ApplicationWorkflowInvocation>, ToolAdmissionError>;
}

/// The actual installed host and its already composed native resource slots.
/// A launch owner first retains authentic P/G source and reserves the selected
/// class through `host()`. It then transfers a real resource here before Add.
/// This handle allocates no member identity, grant, task or workflow itself.
pub struct ApplicationWorkflowRuntime {
    host: Arc<ToolkitGatewayHost>,
    resources: Arc<crate::installed_service_review::composer::LiveSharedSetup>,
    presenters: crate::application_gateway::LocalSetupApprovals,
    policy: Arc<super::selected_policy::SelectedPolicyBinding>,
    tasks: toolkit_mobkit::installed_host::acquisition::InstalledServiceTaskSource,
    output_audiences: BTreeSet<Id>,
    realm: crate::application_setup::ApplicationSetupRealm,
}
impl ApplicationWorkflowRuntime {
    pub(crate) fn new(
        host: Arc<ToolkitGatewayHost>,
        resources: Arc<crate::installed_service_review::composer::LiveSharedSetup>,
        presenters: crate::application_gateway::LocalSetupApprovals,
        policy: Arc<super::selected_policy::SelectedPolicyBinding>,
        tasks: toolkit_mobkit::installed_host::acquisition::InstalledServiceTaskSource,
        output_audiences: BTreeSet<Id>,
        realm: crate::application_setup::ApplicationSetupRealm,
    ) -> Self {
        Self {
            host,
            resources,
            presenters,
            policy,
            tasks,
            output_audiences,
            realm,
        }
    }

    pub fn host(&self) -> &Arc<ToolkitGatewayHost> {
        &self.host
    }

    /// The already-open backend selected at this driver's native bootstrap.
    /// It is the same WorkGraph and provider-open census used by Find, never a
    /// second graph inferred from a storage path.
    pub fn work_backend(&self) -> Result<SetupWorkBackend, HostLifetimeError> {
        let source = self.host.workflow_source()?;
        self.tasks
            .work_backend(&source)
            .map_err(|error| match error {
                toolkit_mobkit::private_originals::PrivateOriginalError::Unavailable => {
                    HostLifetimeError::Unavailable
                }
                _ => HostLifetimeError::Denied,
            })
    }

    pub fn authority_revision(&self) -> Result<toolkit_package::Revision, HostLifetimeError> {
        self.host.workflow_source()?.workflow_authority_revision()
    }

    /// Exact retained physical peer descriptor for an explicitly selected G
    /// envelope. This read is not permission to use the server.
    pub fn helper_server(
        &self,
        admitted: &Arc<AdmittedMemberClass>,
    ) -> Result<meerkat_core::McpServerConfig, HostLifetimeError> {
        self.host.workflow_source()?;
        self.resources.workflow_server(admitted)
    }

    /// Independently commission the completed G through this application's
    /// existing authenticated operator, using the real retained setup peer.
    /// The caller retains exact issuance IDs and the selected finite envelope.
    pub async fn configure_helper(
        &self,
        operator: &toolkit_mobkit::installed_host::operator_setup::OperatorSetup,
        admitted: Arc<AdmittedMemberClass>,
        request: &toolkit_mobkit::installed_host::operator_setup::WorkflowHelperAuthorityRequest,
    ) -> Result<toolkit_mobkit::private_originals::InstalledLateWorkflowHelper, HostLifetimeError>
    {
        self.host.completed_workflow_member_class(&admitted)?;
        let source = self.host.workflow_source()?;
        let server = self.resources.workflow_server(&admitted)?;
        operator
            .configure_workflow_helper(&source, admitted, &server, request)
            .await
    }

    /// Transfer one real prepared Controller/peer to this driver's joined
    /// resource custody. On refusal the unchanged physical owner is returned;
    /// the caller must close/join it before releasing its workflow operation.
    /// This does not start G or authorize its independent planning input.
    #[allow(
        clippy::result_large_err,
        reason = "the refused owner returns by value; boxing would change this public signature"
    )]
    pub fn retain_helper_resources(
        &self,
        admitted: Arc<AdmittedMemberClass>,
        prepared: crate::resources::PreparedSharedSetup,
    ) -> Result<(), (crate::resources::SharedSetupOwner, HostLifetimeError)> {
        if let Err(error) = self.resources.check_reservation(&admitted) {
            return Err((prepared.owner, error));
        }
        let approvals = match self
            .presenters
            .create_shared(self.tasks.clone(), self.policy.clone())
        {
            Ok(approvals) => approvals,
            Err(_) => return Err((prepared.owner, HostLifetimeError::Unavailable)),
        };
        self.resources.retain(admitted, prepared, approvals)
    }

    /// Explicitly abandon a reservation that no Add has accepted. The native
    /// host atomically takes the original one-shot token and retains its exact
    /// retirement in the existing operation owner before any await. A reused
    /// or accepted handle refuses without closing its potentially live peer.
    pub async fn abandon_helper(
        &self,
        admitted: Arc<AdmittedMemberClass>,
    ) -> Result<(), HostLifetimeError> {
        self.resources.require(&admitted)?;
        self.host
            .retire_unaccepted_member_class_with_cleanup(
                admitted.clone(),
                self.resource_cleanup(admitted),
            )
            .await
    }

    /// Enter the existing bounded held-Add owner only after the exact resource
    /// transfer. The application still supplies its real durable publication
    /// callback. The existing accepted native task retains exact resource
    /// cleanup before returning an observer, so losing that observer cannot
    /// strand a known refused Add until whole-driver shutdown.
    pub async fn add_helper<F, Fut>(
        &self,
        admitted: Arc<AdmittedMemberClass>,
        spec: DurableAgentSpec,
        route: meerkat_toolkit::host_lifetime::policy::CompiledNativeSourceRoute,
        controller: PreparedControllerIntent,
        deadline: meerkat_mob::HeldSpawnDeadline,
        finalizer: F,
    ) -> Result<MemberClassAddStart, MemberClassAddError>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<(), HostLifetimeError>> + Send + 'static,
    {
        self.resources
            .require(&admitted)
            .map_err(|error| MemberClassAddError::ReservationNotAccepted { error })?;
        let resources = self.resources.clone();
        let held_admission = admitted.clone();
        self.host
            .add_workflow_member_class(
                admitted.clone(),
                spec,
                route,
                controller,
                deadline,
                move || async move {
                    resources.retain_held_baseline(&held_admission).await?;
                    finalizer().await
                },
                Some(self.resource_cleanup(admitted)),
            )
            .await
    }

    fn resource_cleanup(
        &self,
        admitted: Arc<AdmittedMemberClass>,
    ) -> Arc<dyn MemberClassResourceCleanup> {
        Arc::new(WorkflowResourceCleanup {
            resources: self.resources.clone(),
            admitted,
        })
    }
}

#[async_trait]
impl toolkit_mobkit::private_host::PrivateWorkflowChildLauncher
    for ApplicationWorkflowCommissioner
{
    async fn prepare_console_child(
        &self,
        request: toolkit_mobkit::private_originals::ConsoleWorkflowChildRequest,
        login: toolkit_mobkit::private_host::ConsoleLoginHandle,
    ) -> Result<Option<PrivateOriginalRef>, toolkit_mobkit::private_host::PrivateWorkflowChildError>
    {
        self.launch.prepare_console_child(request, login).await
    }
    async fn prepare_workflow_parent(
        &self,
        request: toolkit_mobkit::private_host::WorkflowParentRequest,
    ) -> Result<(), HostLifetimeError> {
        self.launch.prepare_private_workflow_parent(request).await
    }
    async fn commission_workflow(
        &self,
        request: toolkit_mobkit::private_host::WorkflowCommissionRequest,
        publication: toolkit_mobkit::private_host::WorkflowMemberPublication,
    ) -> Result<toolkit_mobkit::private_host::WorkflowCommissionResponse, HostLifetimeError> {
        self.launch
            .commission_private_workflow(request, publication)
            .await
    }
    async fn prepare_child(
        &self,
        request: toolkit_mobkit::private_originals::WorkflowChildLaunchRequest,
    ) -> Result<(), toolkit_mobkit::private_host::PrivateWorkflowChildError> {
        self.launch.prepare_private_child(request).await
    }
}

// Only the existing accepted Add/retirement task invokes this. It conveys no
// new retirement authority, and retains the exact physical resource through
// cleanup even when the application's result receiver has gone away.
struct WorkflowResourceCleanup {
    resources: Arc<crate::installed_service_review::composer::LiveSharedSetup>,
    admitted: Arc<AdmittedMemberClass>,
}
#[async_trait]
impl MemberClassResourceCleanup for WorkflowResourceCleanup {
    async fn cleanup(
        &self,
        observation: MemberClassCleanupObservation<'_>,
    ) -> Result<(), HostLifetimeError> {
        let settled_absence = match observation {
            MemberClassCleanupObservation::Preflight { cleanup, .. }
            | MemberClassCleanupObservation::UnacceptedRetirement(cleanup) => cleanup.is_ok(),
            MemberClassCleanupObservation::Settled(outcome) => refused_without_member(outcome),
        };
        if settled_absence {
            self.resources
                .release_settled_refusal(&self.admitted)
                .await?;
        }
        Ok(())
    }
}

fn refused_without_member(outcome: &MemberClassAddOutcome) -> bool {
    use meerkat_mobkit::identity_first::{HeldBootstrapRefusal, IdentityRuntimeError};
    if outcome.authorization.is_ok() || outcome.cleanup.is_err() {
        return false;
    }
    match &outcome.native {
        // These roster preconditions refuse before native construction.
        Err(IdentityRuntimeError::HeldBootstrapRefused(
            HeldBootstrapRefusal::LazyMode
            | HeldBootstrapRefusal::Placed { .. }
            | HeldBootstrapRefusal::ExternallyBound { .. }
            | HeldBootstrapRefusal::InitialMessage { .. }
            | HeldBootstrapRefusal::ExistingContinuity { .. },
        )) => true,
        // The callback was reached with an actual held member; settlement has
        // joined its terminal Abort and callback. Physical close still waits
        // for that resource's entered native guards, rather than inferring it.
        Err(IdentityRuntimeError::HeldBootstrapAborted { .. }) => true,
        // NotHeld can mean another embodiment outside this pass. Generic
        // errors, incomplete observations and all Created results keep custody.
        _ => false,
    }
}

/// Production composition of an actual application launch owner with the
/// installed source and native continuation already selected by the Console
/// driver. It retains no second copy of workflow membership or native inputs.
pub struct ApplicationWorkflowCommissioner {
    launch: Arc<dyn ApplicationWorkflowLaunchOwner>,
    source: Arc<PrivateOriginals>,
    native: Arc<InstalledNativeContinuation>,
}
impl ApplicationWorkflowCommissioner {
    pub(crate) fn new(
        launch: Arc<dyn ApplicationWorkflowLaunchOwner>,
        source: Arc<PrivateOriginals>,
        native: Arc<InstalledNativeContinuation>,
    ) -> Self {
        Self {
            launch,
            source,
            native,
        }
    }

    pub(crate) fn bind_runtime(
        &self,
        runtime: ApplicationWorkflowRuntime,
    ) -> Result<(), HostLifetimeError> {
        self.launch.bind_runtime(runtime)
    }
}

#[async_trait]
impl InstalledOriginalTaskOwner for ApplicationWorkflowCommissioner {
    async fn original_task(
        &self,
        invocation: InstalledOriginalInvocation<'_>,
    ) -> Result<TaskContinuation, ToolAdmissionError> {
        self.native.original_task(invocation).await
    }

    async fn original_binding(
        &self,
        invocation: InstalledOriginalInvocation<'_>,
    ) -> Result<InstalledOriginalTaskBinding, ToolAdmissionError> {
        let Some(selected) = self
            .launch
            .invocation(&invocation)
            .await
            .inspect_err(|error| workflow_admission_failure("workflow.launch_invocation", error))?
        else {
            return self
                .native
                .original_binding(invocation)
                .await
                .inspect_err(|error| {
                    workflow_admission_failure("workflow.ordinary_binding", error)
                });
        };
        if &selected.original != invocation.original()
            || &selected.association != invocation.association()
            || selected.output_audiences.is_empty()
        {
            return Err(ToolAdmissionError::Denied);
        }
        selected
            .work
            .check_current(&selected.backend)
            .await
            .map_err(work_error)
            .inspect_err(|error| workflow_admission_failure("workflow.work_before", error))?;
        // Scope the same backend to the existing G/P recipient namespace.
        // This read capability does not authorize disclosure; the source
        // validates the real invocation and fences the subsequent transfer.
        let recipient = meerkat::WorkGraphService::with_scope(
            selected.backend.service().store().clone(),
            selected.backend.service().default_realm_id(),
            selected
                .planning
                .context_namespace()
                .map_err(|_| ToolAdmissionError::Denied)?,
        );
        let transfer = self
            .source
            .prepare_workflow_context_transfer(
                &invocation,
                selected.planning.clone(),
                recipient,
                &selected.backend,
                selected.output_audiences,
            )
            .await
            .map_err(|error| match error {
                toolkit_mobkit::private_originals::PrivateOriginalError::Unavailable => {
                    ToolAdmissionError::Unavailable
                }
                _ => ToolAdmissionError::Denied,
            })
            .inspect_err(|error| workflow_admission_failure("workflow.context_prepare", error))?;
        let continuation = self
            .native
            .original_task(invocation)
            .await
            .inspect_err(|error| {
                workflow_admission_failure("workflow.native_original_task", error)
            })?;
        selected
            .work
            .check_current(&selected.backend)
            .await
            .map_err(work_error)
            .inspect_err(|error| workflow_admission_failure("workflow.work_after", error))?;
        Ok(InstalledOriginalTaskBinding {
            continuation,
            work_context: Some(selected.work),
            planning: Some(selected.planning),
            context_transfer: Some(transfer),
        })
    }
}

// Match the installed leaf's failure-only labels without serializing the
// invocation, source selection or inner error payload.
fn workflow_admission_failure(stage: &'static str, error: &ToolAdmissionError) {
    let variant = match error {
        ToolAdmissionError::Denied => "ToolAdmissionError::Denied",
        ToolAdmissionError::Unavailable => "ToolAdmissionError::Unavailable",
    };
    use std::io::Write;
    let _ = writeln!(
        std::io::stderr().lock(),
        "AA1 failure: stage={stage} variant={variant}"
    );
}

fn work_error(error: SetupError) -> ToolAdmissionError {
    match error {
        SetupError::Store(_)
        | SetupError::WorkGraph(_)
        | SetupError::NativeExecutionUnavailable => ToolAdmissionError::Unavailable,
        _ => ToolAdmissionError::Denied,
    }
}

// Result publication, delivery and reconciliation remain with the exact
// installed native continuation used by the ordinary Console composition.
#[async_trait]
impl SetupContinuationOwner for ApplicationWorkflowCommissioner {
    async fn verify_admission(
        &self,
        obligation: &ContinuationObligation,
        receipt: &ContextReference,
    ) -> Result<Option<ContinuationAdmissionEvidence>, ContinuationError> {
        self.native.verify_admission(obligation, receipt).await
    }
}
#[async_trait]
impl SetupContinuationDelivery for ApplicationWorkflowCommissioner {
    async fn authorize_delivery(
        &self,
        obligation: &ContinuationObligation,
    ) -> Result<(), ContinuationError> {
        self.native.authorize_delivery(obligation).await
    }
    async fn admit(
        &self,
        obligation: &ContinuationObligation,
    ) -> Result<ContinuationAdmissionEvidence, ContinuationError> {
        self.native.admit(obligation).await
    }
    async fn reconcile(
        &self,
        obligation: &ContinuationObligation,
    ) -> Result<ContinuationReconciliation, ContinuationError> {
        self.native.reconcile(obligation).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_mobkit::identity_first::{
        AgentIdentity, HeldBootstrapRefusal, HeldCompletionOutcome, IdentityRuntimeError,
    };

    #[test]
    fn only_settled_unpublished_add_refusal_releases_resource() {
        let mut outcome = MemberClassAddOutcome {
            native: Err(IdentityRuntimeError::HeldBootstrapAborted {
                refusal: HeldBootstrapRefusal::HostRefused(
                    "exact fixture finalizer refusal".into(),
                ),
                completion: HeldCompletionOutcome::Refused(
                    "exact fixture finalizer refusal".into(),
                ),
            }),
            authorization: Err(HostLifetimeError::Denied),
            cleanup: Ok(()),
        };
        assert!(refused_without_member(&outcome));
        outcome.cleanup = Err(HostLifetimeError::NotSettled);
        assert!(
            !refused_without_member(&outcome),
            "uncertain canonical cleanup must retain peer"
        );
        outcome.cleanup = Ok(());
        outcome.authorization = Ok(());
        assert!(
            !refused_without_member(&outcome),
            "contradictory authorization cannot release"
        );
        outcome.authorization = Err(HostLifetimeError::Denied);
        for refusal in [
            HeldBootstrapRefusal::PassEnded,
            HeldBootstrapRefusal::DeadlineElapsed,
            HeldBootstrapRefusal::NotHeld {
                identity: AgentIdentity::parse("unobserved").unwrap(),
            },
        ] {
            outcome.native = Err(IdentityRuntimeError::HeldBootstrapRefused(refusal));
            assert!(
                !refused_without_member(&outcome),
                "an unobserved embodiment is not settled absence"
            );
        }
        outcome.native = Err(IdentityRuntimeError::HeldBootstrapRefused(
            HeldBootstrapRefusal::LazyMode,
        ));
        assert!(refused_without_member(&outcome));
    }
}

//! Supplied private child launcher over the existing source and Driver owners.
use super::*;
use std::sync::{OnceLock, Weak};
mod commission;
use toolkit_mobkit::private_host::PrivateWorkflowChildError;
use toolkit_mobkit::private_originals::{
    PrivateOriginalError, WorkflowChildLaunchRequest, WorkflowChildPreparationError,
};

struct Selected {
    host: Weak<ToolkitGatewayHost>,
    backend: SetupWorkBackend,
    output_audiences: BTreeSet<Id>,
    resources: Arc<crate::installed_service_review::composer::LiveSharedSetup>,
    presenters: crate::application_gateway::LocalSetupApprovals,
    policy: Arc<super::super::selected_policy::SelectedPolicyBinding>,
    tasks: toolkit_mobkit::installed_host::acquisition::InstalledServiceTaskSource,
    realm: crate::application_setup::ApplicationSetupRealm,
}

/// Explicit fresh-host opt-in using current Driver/source owners. Selected
/// interest recovery can restore planning, but cold child launch still lacks
/// the exact restored workflow-class source and late-helper planning custody.
#[derive(Default)]
pub struct PrivateApplicationWorkflowLauncher(OnceLock<Selected>);
impl PrivateApplicationWorkflowLauncher {
    /// Compose the supplied adapter into an application's actual launch owner.
    /// Only weak runtime custody is retained; the Driver still owns shutdown.
    pub fn bind(&self, runtime: &ApplicationWorkflowRuntime) -> Result<(), HostLifetimeError> {
        let backend = runtime.work_backend()?;
        self.0
            .set(Selected {
                host: Arc::downgrade(runtime.host()),
                backend,
                output_audiences: runtime.output_audiences.clone(),
                resources: runtime.resources.clone(),
                presenters: runtime.presenters.clone(),
                policy: runtime.policy.clone(),
                tasks: runtime.tasks.clone(),
                realm: runtime.realm.clone(),
            })
            .map_err(|_| HostLifetimeError::WrongOwner)
    }
    fn selected(&self) -> Result<(&Selected, Arc<ToolkitGatewayHost>), HostLifetimeError> {
        let selected = self.0.get().ok_or(HostLifetimeError::Unavailable)?;
        let host = selected
            .host
            .upgrade()
            .ok_or(HostLifetimeError::Unavailable)?;
        host.workflow_source()?;
        Ok((selected, host))
    }
}
fn source_error(error: PrivateOriginalError) -> HostLifetimeError {
    match error {
        PrivateOriginalError::Unavailable => HostLifetimeError::Unavailable,
        PrivateOriginalError::Denied | PrivateOriginalError::Conflict => HostLifetimeError::Denied,
    }
}
#[async_trait]
impl ApplicationWorkflowLaunchOwner for PrivateApplicationWorkflowLauncher {
    async fn prepare_console_child(
        &self,
        request: toolkit_mobkit::private_originals::ConsoleWorkflowChildRequest,
        login: toolkit_mobkit::private_host::ConsoleLoginHandle,
    ) -> Result<Option<PrivateOriginalRef>, PrivateWorkflowChildError> {
        let (selected, host) = self.selected()?;
        host.workflow_source()?
            .prepare_console_workflow_child(
                request,
                login,
                &selected.backend,
                &selected.output_audiences,
                Arc::new(ConsoleChildPreparer {
                    host: Arc::downgrade(&host),
                    backend: selected.backend.clone(),
                    output_audiences: selected.output_audiences.clone(),
                }),
            )
            .await
            .map_err(|error| PrivateWorkflowChildError::Refused(source_error(error)))
    }
    fn bind_runtime(&self, runtime: ApplicationWorkflowRuntime) -> Result<(), HostLifetimeError> {
        self.bind(&runtime)
    }

    async fn prepare_private_workflow_parent(
        &self,
        request: toolkit_mobkit::private_host::WorkflowParentRequest,
    ) -> Result<(), HostLifetimeError> {
        self.prepare_parent(request).await
    }

    async fn commission_private_workflow(
        &self,
        request: toolkit_mobkit::private_host::WorkflowCommissionRequest,
        publication: toolkit_mobkit::private_host::WorkflowMemberPublication,
    ) -> Result<toolkit_mobkit::private_host::WorkflowCommissionResponse, HostLifetimeError> {
        self.commission(request, publication).await
    }

    async fn prepare_private_child(
        &self,
        request: WorkflowChildLaunchRequest,
    ) -> Result<(), PrivateWorkflowChildError> {
        let (selected, host) = self
            .selected()
            .inspect_err(|error| private_child_failure("private_child.selected", error))?;
        // Actual source authentication and actual native controller/session.
        // No submission occurs, and no serialized controller is accepted.
        let controller = host
            .prepare_local_controller_setup(request.submission.clone())
            .await
            .map_err(source_error)
            .inspect_err(|error| private_child_failure("private_child.controller", error))?;
        let operator = host
            .application_operator_setup_for(&controller)
            .inspect_err(|error| private_child_failure("private_child.operator", error))?;
        let source = host
            .workflow_source()
            .inspect_err(|error| private_child_failure("private_child.source", error))?;
        source
            .prepare_workflow_child_launch(
                &selected.backend,
                &operator,
                controller.session_id(),
                &request,
                &selected.output_audiences,
            )
            .await
            .map_err(|error| match error {
                WorkflowChildPreparationError::Refused(error) => PrivateWorkflowChildError::Refused(source_error(error)),
                WorkflowChildPreparationError::ObservationChanged => PrivateWorkflowChildError::ObservationChanged,
            })
            .inspect_err(|error| match error {
                PrivateWorkflowChildError::Refused(error) => private_child_failure("private_child.canonical", error),
                PrivateWorkflowChildError::ObservationChanged => {
                    use std::io::Write;
                    let _ = writeln!(std::io::stderr().lock(), "AA1 failure: stage=private_child.canonical variant=PrivateWorkflowChildError::ObservationChanged");
                }
            })
    }

    async fn invocation(
        &self,
        invocation: &InstalledOriginalInvocation<'_>,
    ) -> Result<Option<ApplicationWorkflowInvocation>, ToolAdmissionError> {
        let (selected, host) = self
            .selected()
            .map_err(|_| ToolAdmissionError::Unavailable)?;
        let source = host
            .workflow_source()
            .map_err(|_| ToolAdmissionError::Unavailable)?;
        let Some(child) = source
            .workflow_child_invocation(
                invocation.original(),
                invocation.association(),
                &selected.backend,
            )
            .await
            .map_err(|error| match error {
                PrivateOriginalError::Unavailable => ToolAdmissionError::Unavailable,
                _ => ToolAdmissionError::Denied,
            })?
        else {
            return Ok(None);
        };
        if !child.output_audiences.is_subset(&selected.output_audiences) {
            return Err(ToolAdmissionError::Denied);
        }
        Ok(Some(ApplicationWorkflowInvocation {
            original: invocation.original().clone(),
            association: invocation.association().clone(),
            work: child.work,
            backend: selected.backend.clone(),
            planning: child.planning,
            output_audiences: child.output_audiences,
        }))
    }
}

struct ConsoleChildPreparer {
    host: Weak<ToolkitGatewayHost>,
    backend: SetupWorkBackend,
    output_audiences: BTreeSet<Id>,
}
#[async_trait]
impl toolkit_mobkit::private_originals::ConsoleWorkflowChildPreparer for ConsoleChildPreparer {
    async fn prepare(
        &self,
        input: &toolkit_mobkit::private_originals::ConsoleWorkflowChildInput,
    ) -> Result<(), WorkflowChildPreparationError> {
        let host = self
            .host
            .upgrade()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let operator = host
            .application_operator_setup_for(input.controller())
            .map_err(|error| match error {
                HostLifetimeError::Unavailable | HostLifetimeError::NotSettled => {
                    PrivateOriginalError::Unavailable
                }
                _ => PrivateOriginalError::Denied,
            })?;
        host.workflow_source()
            .map_err(|error| match error {
                HostLifetimeError::Unavailable | HostLifetimeError::NotSettled => {
                    PrivateOriginalError::Unavailable
                }
                _ => PrivateOriginalError::Denied,
            })?
            .prepare_console_workflow_child_launch(
                &self.backend,
                &operator,
                input,
                &self.output_audiences,
            )
            .await
    }
}

// Fixed failure labels only; native/source error payloads stay private.
fn private_child_failure(stage: &'static str, error: &HostLifetimeError) {
    let variant = match error {
        HostLifetimeError::InvalidSelection => "HostLifetimeError::InvalidSelection",
        HostLifetimeError::Denied => "HostLifetimeError::Denied",
        HostLifetimeError::WrongOwner => "HostLifetimeError::WrongOwner",
        HostLifetimeError::Unavailable => "HostLifetimeError::Unavailable",
        HostLifetimeError::NotSettled => "HostLifetimeError::NotSettled",
    };
    use std::io::Write;
    let _ = writeln!(
        std::io::stderr().lock(),
        "AA1 failure: stage={stage} variant={variant}"
    );
}

impl Selected {
    fn runtime(&self, host: Arc<ToolkitGatewayHost>) -> ApplicationWorkflowRuntime {
        ApplicationWorkflowRuntime::new(
            host,
            self.resources.clone(),
            self.presenters.clone(),
            self.policy.clone(),
            self.tasks.clone(),
            self.output_audiences.clone(),
            self.realm.clone(),
        )
    }
}

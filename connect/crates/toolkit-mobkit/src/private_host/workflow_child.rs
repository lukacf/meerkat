//! One optional private launch operation, retained by the existing pipe JoinSet.
use super::*;
use crate::private_originals::WorkflowChildLaunchRequest;
use std::sync::{OnceLock, Weak};

/// Preparation can have already issued or retained effects before its final
/// observation changes. Neither variant proves no effects or authorizes replay.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PrivateWorkflowChildError {
    #[error(transparent)]
    Refused(#[from] HostLifetimeError),
    #[error("workflow child authority observation changed")]
    ObservationChanged,
}

pub(super) fn child_response(
    id: u64,
    result: Result<Reply, PrivateWorkflowChildError>,
) -> Response {
    match result {
        Ok(reply) => response(id, Ok(reply)),
        Err(PrivateWorkflowChildError::Refused(error)) => response(id, Err(error)),
        Err(PrivateWorkflowChildError::ObservationChanged) => Response {
            version: PRIVATE_HOST_PROTOCOL_VERSION,
            id,
            result: Reply::Error {
                error: WireError::ObservationChanged,
            },
        },
    }
}

#[async_trait::async_trait]
pub trait PrivateWorkflowChildLauncher: Send + Sync {
    async fn prepare_console_child(
        &self,
        _request: crate::private_originals::ConsoleWorkflowChildRequest,
        _login: ConsoleLoginHandle,
    ) -> Result<Option<PrivateOriginalRef>, PrivateWorkflowChildError> {
        Err(HostLifetimeError::Unavailable.into())
    }
    async fn prepare_workflow_parent(
        &self,
        _request: WorkflowParentRequest,
    ) -> Result<(), HostLifetimeError> {
        Err(HostLifetimeError::Unavailable)
    }
    async fn commission_workflow(
        &self,
        _request: WorkflowCommissionRequest,
        _publication: WorkflowMemberPublication,
    ) -> Result<WorkflowCommissionResponse, HostLifetimeError> {
        Err(HostLifetimeError::Unavailable)
    }
    async fn prepare_child(
        &self,
        request: WorkflowChildLaunchRequest,
    ) -> Result<(), PrivateWorkflowChildError>;
}

#[derive(Clone, Default)]
pub struct WorkflowChildEndpoint(Arc<OnceLock<Weak<dyn PrivateWorkflowChildLauncher>>>);
impl WorkflowChildEndpoint {
    pub(super) async fn prepare_console(
        &self,
        request: crate::private_originals::ConsoleWorkflowChildRequest,
        login: ConsoleLoginHandle,
    ) -> Result<Option<PrivateOriginalRef>, PrivateWorkflowChildError> {
        let launcher = self
            .0
            .get()
            .and_then(Weak::upgrade)
            .ok_or(HostLifetimeError::Unavailable)?;
        launcher.prepare_console_child(request, login).await
    }
    /// Bind the actual adopted Driver participant. The endpoint cannot keep a
    /// host alive or reconstruct a launcher from wire selections.
    pub fn bind(
        &self,
        launcher: Arc<dyn PrivateWorkflowChildLauncher>,
    ) -> Result<(), HostLifetimeError> {
        self.0
            .set(Arc::downgrade(&launcher))
            .map_err(|_| HostLifetimeError::WrongOwner)
    }
    pub(super) async fn prepare_parent(
        &self,
        request: WorkflowParentRequest,
    ) -> Result<(), HostLifetimeError> {
        let launcher = self
            .0
            .get()
            .and_then(Weak::upgrade)
            .ok_or(HostLifetimeError::Unavailable)?;
        launcher.prepare_workflow_parent(request).await
    }
    pub(super) async fn commission(
        &self,
        request: WorkflowCommissionRequest,
        publication: WorkflowMemberPublication,
    ) -> Result<WorkflowCommissionResponse, HostLifetimeError> {
        let launcher = self
            .0
            .get()
            .and_then(Weak::upgrade)
            .ok_or(HostLifetimeError::Unavailable)?;
        launcher.commission_workflow(request, publication).await
    }
    pub(super) async fn prepare(
        &self,
        request: WorkflowChildLaunchRequest,
    ) -> Result<(), PrivateWorkflowChildError> {
        let launcher = self
            .0
            .get()
            .and_then(Weak::upgrade)
            .ok_or(HostLifetimeError::Unavailable)?;
        launcher.prepare_child(request).await
    }
}

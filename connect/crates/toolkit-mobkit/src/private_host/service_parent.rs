//! One endpoint of the existing private review actor. No second task registry,
//! approval owner or native grant authority is created here.
use super::*;
use crate::private_originals::{ApplicationServiceOperation, ApplicationServiceOperationControl,
    ApplicationServiceParentAuthorizer, ServiceParentAuthorization, ServiceParentAuthorizationRequest,
    ServiceParentAuthorizationProposal, ServiceParentOperationObserver, ServiceParentOperationObservation};
use meerkat_toolkit::host_lifetime::NativeHostLifetime;

mod wire;
pub use wire::{ServiceParentAdministrationRequest, ServiceParentDecision, ServiceParentDecisionSource};

impl ServiceParentAdministrationRequest {
    /// Validate the actual full existing wire envelope before the review actor
    /// retains/delivers an exchange. The largest supported request ID avoids
    /// estimating JSON overhead; no proposal truncation or new framing exists.
    pub fn validate_private_response(&self) -> Result<(), HostLifetimeError> {
        let reply = response(9_007_199_254_740_991, Ok(Reply::ServiceParentAdministrationRequest {
            exchange_id: self.exchange_id, proposal_sha256: self.proposal_sha256.clone(),
            proposal: self.proposal.clone(),
        }));
        if private_reply_fits(&reply) { Ok(()) } else { Err(HostLifetimeError::Unavailable) }
    }
}

pub type ServiceParentResponse<T> = oneshot::Sender<Result<T, HostLifetimeError>>;
/// Internal Adopt and Reserve cannot be created by a wire command. The actual
/// native caller transfers its owned operation; reserve carries its observer.
pub enum ServiceParentCommand {
    Adopt { operation: ApplicationServiceOperation },
    Reserve { proposal: ServiceParentAuthorizationProposal, observer: ServiceParentOperationObserver,
        response: ServiceParentResponse<ServiceParentAuthorization> },
    Next { response: ServiceParentResponse<ServiceParentAdministrationRequest> },
    Submit { exchange_id: u64, proposal_sha256: String, decision: ServiceParentDecision,
        response: ServiceParentResponse<()> },
    Result { exchange_id: u64, response: ServiceParentResponse<ServiceParentOperationObservation> },
    Retire { exchange_id: u64, response: ServiceParentResponse<ServiceParentOperationObservation> },
}
impl ServiceParentCommand {
    pub async fn refuse(self) {
        match self {
            Self::Adopt { mut operation } => { operation.retire().await; }
            Self::Reserve { response, .. } => { let _ = response.send(Err(HostLifetimeError::Unavailable)); }
            Self::Next { response } => { let _ = response.send(Err(HostLifetimeError::Unavailable)); }
            Self::Submit { response, .. } => { let _ = response.send(Err(HostLifetimeError::Unavailable)); }
            Self::Result { response, .. } | Self::Retire { response, .. } => { let _ = response.send(Err(HostLifetimeError::Unavailable)); }
        }
    }
}

/// Clones only this actual connection/owner. The parent must separately supply
/// its real authenticated administrative exclusion through the protected SDK.
#[derive(Clone)]
pub struct ServiceParentEndpoint {
    pub(super) sender: mpsc::Sender<RemoteControllerCommand>,
    pub(super) owner: Arc<dyn PrivateHostLifetimeControl>,
    pub(super) closed: PrivateHostClosed,
    pub(super) logins: ConsoleLogins,
}
impl ServiceParentEndpoint {
    pub fn belongs_to(&self, owner: &Arc<NativeHostLifetime>) -> bool {
        let erased: Arc<dyn PrivateHostLifetimeControl> = owner.clone();
        Arc::ptr_eq(&self.owner, &erased)
    }
    /// Transfer synchronously, before any administrative wait or possible
    /// entry. On refusal the caller still owns and must retire this operation.
    pub fn adopt(&self, operation: ApplicationServiceOperation)
        -> Result<ApplicationServiceOperationControl, ApplicationServiceOperation> {
        if self.closed.is_closed() { return Err(operation); }
        let control = operation.control();
        match self.sender.try_send(RemoteControllerCommand::ServiceParent(ServiceParentCommand::Adopt { operation })) {
            Ok(()) => Ok(control),
            Err(error) => match error.into_inner() {
                RemoteControllerCommand::ServiceParent(ServiceParentCommand::Adopt { operation }) => Err(operation),
                _ => unreachable!("only the actual submitted operation can be returned"),
            },
        }
    }
    /// A parent enrollment is lifetime-only. This API deliberately does not
    /// call the verified-carrier resolver or manufacture a native Human.
    pub fn parent_login_lifetime(&self, enrollment_id: u64) -> Result<ConsoleLoginHandle, HostLifetimeError> {
        self.logins.parent_enrollment(enrollment_id)
    }
    pub fn check_parent_login_lifetime(handle: &ConsoleLoginHandle) -> Result<(), HostLifetimeError> {
        handle.try_check().map_err(|error| match error {
            ConsoleLoginRefusal::Unavailable => HostLifetimeError::Unavailable,
            _ => HostLifetimeError::Denied,
        })
    }
    pub fn is_closed(&self) -> bool { self.closed.is_closed() }
}
#[async_trait::async_trait]
impl ApplicationServiceParentAuthorizer for ServiceParentEndpoint {
    async fn reserve(&self, request: &ServiceParentAuthorizationRequest<'_>)
        -> Result<ServiceParentAuthorization, HostLifetimeError> {
        if self.closed.is_closed() { return Err(HostLifetimeError::Unavailable); }
        self.owner.validate_current_selection()?;
        let (response, result) = oneshot::channel();
        self.sender.send(RemoteControllerCommand::ServiceParent(ServiceParentCommand::Reserve {
            proposal: request.proposal().clone(), observer: request.observer(), response,
        })).await.map_err(|_| HostLifetimeError::Unavailable)?;
        result.await.map_err(|_| HostLifetimeError::Unavailable)?
    }
}

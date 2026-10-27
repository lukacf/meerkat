//! Administrative authorization is supplied by the real application owner.
//! Selected policy fields and serialized proposal values supply no authority.
use super::*;
use meerkat_authorization_contracts::constraints::ExecutionRestrictions;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceParentAuthorizationProposal {
    pub operator: PrincipalRef,
    pub rule: ServiceParentAdministration,
    pub issuance_id: EvidenceId,
    pub deployment: Id,
    pub source_policy_sha256: String,
    pub original: PrivateOriginalRef,
    pub acquisition: StoreKey,
    pub instruction_sha256: String,
    pub expires_in_ms: Option<u64>,
    pub application: PrincipalRef,
    pub represented_subject: Option<PrincipalRef>,
    pub decision: PrivateServiceMcpAccessDecision,
    pub connector_selection: serde_json::Value,
    pub restrictions: ExecutionRestrictions,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_policy: Option<ServiceToolPolicyProposal>,
}
impl ServiceParentAuthorizationProposal {
    pub fn proposal_sha256(&self) -> Result<String, HostLifetimeError> {
        canonical_json(self).map(|bytes| digest(&bytes)).map_err(|_| HostLifetimeError::InvalidSelection)
    }
}

/// Only the actual operation constructs a request. The configuration is
/// available to trusted Rust composition but never enters the wire proposal.
pub struct ServiceParentAuthorizationRequest<'a> {
    pub(super) proposal: &'a ServiceParentAuthorizationProposal,
    pub(super) configuration: &'a meerkat_core::McpServerConfig,
    pub(super) observation: ServiceParentOperationObserver,
}
impl ServiceParentAuthorizationRequest<'_> {
    pub fn proposal(&self) -> &ServiceParentAuthorizationProposal { self.proposal }
    pub fn configuration(&self) -> &meerkat_core::McpServerConfig { self.configuration }
    pub fn observer(&self) -> ServiceParentOperationObserver { self.observation.clone() }
}

/// A pending producer must observe request.observer().wait_retiring() (and its
/// own application deadline), cancel and settle its actual authorization wait,
/// then return. The operation continues driving this future through retirement;
/// dropping a result waiter cannot manufacture a settled authorization. Do not
/// await observer.result() inside reserve: operation completion needs reserve
/// to return. The observer may be retained for later completion observation.
#[async_trait::async_trait]
pub trait ApplicationServiceParentAuthorizer: Send + Sync {
    async fn reserve(&self, request: &ServiceParentAuthorizationRequest<'_>)
        -> Result<ServiceParentAuthorization, HostLifetimeError>;
}
pub enum ServiceParentAuthorization {
    RequiresHuman,
    Reserved(Box<dyn ServiceParentAdministrationReservation>),
}
/// The actual application decision/rule exclusion survives through operation
/// join. check_current must not acquire native phase/publication locks or do
/// network I/O. A login count or matching principal is not this authorization.
pub trait ServiceParentAdministrationReservation: Send + Sync {
    fn operator(&self) -> &PrincipalRef;
    fn rule(&self) -> &ServiceParentAdministration;
    fn proposal_sha256(&self) -> &str;
    fn provenance(&self) -> &serde_json::Map<String, serde_json::Value>;
    fn check_current(&self) -> Result<(), HostLifetimeError>;
    fn tool_policy_approval(&self) -> Option<&ServiceToolPolicyApproval> { None }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ServiceParentAuthorizationEvidence {
    pub proposal_sha256: String,
    pub provenance: serde_json::Map<String, serde_json::Value>,
}
impl ServiceParentAuthorizationEvidence {
    pub(crate) fn validate(&self, proposal: &ServiceParentAuthorizationProposal) -> Result<(), HostLifetimeError> {
        if self.proposal_sha256 != proposal.proposal_sha256()? || self.provenance.is_empty()
            || canonical_json(&self.provenance).map_err(|_| HostLifetimeError::InvalidSelection)?.len() > 16 * 1024
        { return Err(HostLifetimeError::Denied); }
        Ok(())
    }
}

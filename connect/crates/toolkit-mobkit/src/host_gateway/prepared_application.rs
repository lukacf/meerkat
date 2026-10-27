//! Exact prepared bytes selected by the existing trusted application factory.
//! The payload identity is not permission; the current host remains the owner.
use super::{ToolkitGatewayHost, host_error};
use crate::private_host::PrivateHostClosed;
use meerkat_mob::MobDefinition;
use meerkat_mobkit::{
    identity_first::{DurableAgentSpec, RosterContext, contracts::RosterProjection},
    rpc_gateway::{PreparedApplicationRequest, PreparedGatewayApplication},
};
use meerkat_toolkit::host_lifetime::{
    NativeHostLifetime,
    control::HostLifetimeError,
    policy::CompiledArtifactReference,
};
use sha2::{Digest, Sha256};
use std::sync::Arc;

/// Process-local projection facts from the authenticated application bundle.
/// This is neither a public wire decoder nor a grant/activation credential.
/// The trusted factory must authenticate the artifact and validate its typed
/// tuple before construction, as prepare_gateway_acquisition does. The digest
/// check here verifies consistency only; it cannot authenticate arbitrary bytes.
pub struct PreparedSetupApplication {
    reference: CompiledArtifactReference,
    bytes: Arc<[u8]>,
    definition: MobDefinition,
    roster: Arc<dyn RosterProjection>,
}
impl PreparedSetupApplication {
    pub fn from_verified_artifact(
        reference: CompiledArtifactReference,
        bytes: Vec<u8>,
        definition: MobDefinition,
        roster: Arc<dyn RosterProjection>,
    ) -> Result<Self, HostLifetimeError> {
        reference.validate()?;
        if format!("{:x}", Sha256::digest(&bytes)) != reference.sha256 {
            return Err(HostLifetimeError::InvalidSelection);
        }
        Ok(Self { reference, bytes: bytes.into(), definition, roster })
    }

    pub(super) fn select(
        &self,
        request: &PreparedApplicationRequest,
        owner: Arc<NativeHostLifetime>,
        private_closed: PrivateHostClosed,
    ) -> Result<PreparedGatewayApplication, HostLifetimeError> {
        // Compare literal bytes as well as their selected identity. A supplied
        // digest is never sufficient and equivalent re-encoding is not replay.
        if request.declared_digest != self.reference.sha256
            || request.bundle.as_slice() != self.bytes.as_ref()
        { return Err(HostLifetimeError::InvalidSelection); }
        Ok(PreparedGatewayApplication::new(self.reference.sha256.clone(), self.definition.clone())
            .with_roster_projection(Arc::new(CurrentApplicationRoster {
                owner, private_closed, inner: self.roster.clone(),
            })))
    }
}

struct CurrentApplicationRoster {
    owner: Arc<NativeHostLifetime>,
    private_closed: PrivateHostClosed,
    inner: Arc<dyn RosterProjection>,
}
impl RosterProjection for CurrentApplicationRoster {
    fn project(&self, context: &RosterContext, roster: Vec<DurableAgentSpec>)
        -> Result<Vec<DurableAgentSpec>, String>
    {
        // The existing activation/selection fence encloses only this bounded
        // synchronous projection. It never awaits the application's provider,
        // reenters the owner, opens a runtime or obtains a new permission.
        self.owner.with_current_setup(|| {
            if self.private_closed.is_closed() { return Err(HostLifetimeError::Denied); }
            let projected = self.inner.project(context, roster)
                .map_err(|_| HostLifetimeError::InvalidSelection)?;
            if self.private_closed.is_closed() { return Err(HostLifetimeError::Denied); }
            Ok(projected)
        }).map_err(|error| host_error(error).to_string())
    }
}

impl ToolkitGatewayHost {
    pub(super) fn require_prepared_application(&self) -> Result<(), HostLifetimeError> {
        if self.setup_application.is_none() { return Ok(()); }
        self.owner.with_current_setup(|| {
            if self.private_closed.is_closed() || self.prepared_application_selected.get().is_none() {
                return Err(HostLifetimeError::Denied);
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests;

//! The real immutable leaves may be registered before their final owner exists.
//! Enrollment carries no source, store, native admission or installation grant.
use super::*;
use std::sync::{OnceLock, Weak};

pub(crate) struct AcquisitionToolBinding {
    owner: OnceLock<Weak<InstalledAcquisition>>,
}
impl AcquisitionToolBinding {
    pub(crate) fn owner(&self) -> Option<Arc<InstalledAcquisition>> {
        let owner = self.owner.get().and_then(Weak::upgrade)?;
        // This is only the existing private owner lifetime precondition. Actual
        // invocation/source/native authorization is still checked by the leaf.
        owner.host.originals.validate_private_setup_owner(
            owner.host.tenant.as_str(), owner.host.deployment.as_str(),
        ).ok()?;
        Some(owner)
    }

    /// The installed leaf requires the actual identity owner bound by this
    /// gateway, in addition to the current source lifetime above. This is an
    /// entry dependency, not a cached readiness or invocation permission bit.
    /// Original admission checks its exact native continuation service; result
    /// admission checks its retained helper input and canonical source again.
    pub(crate) fn execution_owner(&self) -> Option<Arc<InstalledAcquisition>> {
        let owner = self.owner()?;
        owner.require_native_runtime().ok()?;
        Some(owner)
    }
}

struct EnrolledTools {
    binding: Arc<AcquisitionToolBinding>,
    acquisition: Arc<FindAndInstallTool>,
    results: Arc<results::SetupResultsTool>,
    admission_timeout: Duration,
}

/// Exact dispatcher objects for the existing native bundle registry. Cloning
/// retains the same leaves and one binding. The final installed acquisition
/// adopts this value; an unbound, retired or dropped owner cannot serve calls.
/// This does not install a native bundle, select a policy, or authorize MCP.
#[derive(Clone)]
pub struct InstalledAcquisitionTools(Arc<EnrolledTools>);
impl InstalledAcquisitionTools {
    pub fn new(admission_timeout: Duration) -> Result<Self, HostLifetimeError> {
        let binding = Arc::new(AcquisitionToolBinding { owner: OnceLock::new() });
        let acquisition = Arc::new(FindAndInstallTool::enrolled(binding.clone(), admission_timeout)
            .map_err(|_| HostLifetimeError::InvalidSelection)?);
        let results = Arc::new(results::SetupResultsTool::new(binding.clone()));
        Ok(Self(Arc::new(EnrolledTools {
            binding, acquisition, results, admission_timeout,
        })))
    }

    pub fn acquisition_tool(&self) -> Arc<dyn AgentToolDispatcher> {
        self.0.acquisition.clone()
    }

    pub fn result_tool(&self) -> Arc<dyn AgentToolDispatcher> {
        self.0.results.clone()
    }

    pub(super) fn require_unbound(&self, timeout: Duration) -> Result<(), HostLifetimeError> {
        if self.0.admission_timeout != timeout || self.0.binding.owner.get().is_some() {
            return Err(HostLifetimeError::InvalidSelection);
        }
        Ok(())
    }

    pub(super) fn bind(&self, owner: &Arc<InstalledAcquisition>) -> Result<(), HostLifetimeError> {
        // The creating acquisition must retain this exact pair. A different
        // pair cannot redirect already registered leaves to another owner.
        if !Arc::ptr_eq(&self.0, &owner.tools.0) {
            return Err(HostLifetimeError::InvalidSelection);
        }
        self.0.binding.owner.set(Arc::downgrade(owner))
            .map_err(|_| HostLifetimeError::InvalidSelection)
    }
}

#[cfg(test)]
mod tests;

impl InstalledAcquisition {
    pub(crate) fn require_native_runtime(&self) -> Result<(), PrivateOriginalError> {
        self.host.originals.validate_private_setup_owner(
            self.host.tenant.as_str(), self.host.deployment.as_str(),
        )?;
        let _runtime = self.host.originals.bound_identity_runtime()?;
        Ok(())
    }
}

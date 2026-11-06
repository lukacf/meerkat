//! Linux entry for an application-owned protected service host.
//!
//! The application adopts its four or five original descriptors before creating a
//! runtime or helper. The root launch owner supplies the installed mapping,
//! execution identity, namespaces and original login reservation. This module
//! verifies that parent's actual packets and retains the native invocation.
//! Neither descriptor possession nor public Rust construction authenticates a
//! caller. The application still owns its compiled collection catalog.
#![cfg(target_os = "linux")]

mod binding;
mod execution;
mod lifecycle;
mod mapping;
pub(crate) mod selected_input;
mod transport;
mod wire;

use crate::{ConnectError, service_collection::RequiredCollectionOperation};
use std::{
    os::fd::{AsFd, OwnedFd},
    path::Path,
    sync::Arc,
};

fn refused() -> ConnectError {
    ConnectError::InvalidInput("The protected native parent could not be retained; no replacement authority or successful disposal was created".into())
}

/// Compiled application policy. Implementations must be bounded, synchronous
/// selectors with no helper/process launch or native admission. JSON selects
/// an existing compiled program; it cannot supply a program or Ready object.
/// Requirements retain program intent. The actual installed connection is
/// available later through CollectionTarget, when native collection starts.
pub trait ProtectedCollectionCatalog: Send + Sync + 'static {
    fn accepts(&self, selection: &CatalogSelection<'_>) -> bool;
    fn requirement(
        &self,
        selection: &CatalogSelection<'_>,
        source: &CatalogSource<'_>,
        requirement: &CatalogRequirement<'_>,
    ) -> Result<RequiredCollectionOperation, ConnectError>;
}

/// Validated installation intent, not authentication or a native receipt.
pub struct CatalogSelection<'a>(&'a mapping::ParsedMapping);
impl CatalogSelection<'_> {
    pub fn selected_input(&self) -> Option<CatalogSelectedInput<'_>> {
        self.0.selected_input.as_ref().map(CatalogSelectedInput)
    }
    pub fn service_ids(&self) -> impl Iterator<Item = &str> {
        self.0.service_groups.iter().map(|(id, _)| id.as_str())
    }
    pub fn application_id(&self) -> &str {
        &self.0.application.application_id
    }
    pub fn origin(&self) -> &str {
        &self.0.application.origin
    }
    pub fn source_selection(&self) -> &str {
        &self.0.source_selection
    }
    pub fn catalog_id(&self) -> &str {
        &self.0.catalog_id
    }
    pub fn resource_root(&self) -> &Path {
        &self.0.installation.resource_root
    }
    pub fn realm_id(&self) -> &str {
        self.0.realm.realm_id.as_str()
    }
    pub fn state_root(&self) -> &Path {
        &self.0.realm.state_root
    }
    pub fn context_root(&self) -> &Path {
        &self.0.realm.context_root
    }
    pub fn user_config_root(&self) -> &Path {
        &self.0.realm.user_config_root
    }
}
/// Borrowed selection intent only, with no descriptor or staging capability.
pub struct CatalogSelectedInput<'a>(&'a selected_input::Identity);
impl CatalogSelectedInput<'_> {
    pub fn service_id(&self) -> &str {
        &self.0.service_id
    }
    /// Exact lowercase hexadecimal SHA-256 identity (64 ASCII characters).
    /// This borrowed observation does not authorize staging or installation.
    pub fn bundle_sha256(&self) -> &str {
        &self.0.bundle_sha256
    }
}
pub struct CatalogSource<'a>(&'a mapping::CollectionDescriptor);
impl CatalogSource<'_> {
    pub fn service_id(&self) -> &str {
        &self.0.service_id
    }
    pub fn source_id(&self) -> &str {
        &self.0.source_id
    }
    pub fn family(&self) -> &str {
        &self.0.family
    }
    pub fn profile(&self) -> &str {
        &self.0.profile
    }
    pub fn agent(&self) -> &str {
        &self.0.agent
    }
    pub fn calls(&self) -> usize {
        self.0.limits.calls
    }
    pub fn argument_bytes(&self) -> usize {
        self.0.limits.argument_bytes
    }
    pub fn result_bytes(&self) -> usize {
        self.0.limits.result_bytes
    }
}
pub struct CatalogRequirement<'a>(&'a mapping::RequirementDescriptor);
impl CatalogRequirement<'_> {
    pub fn id(&self) -> &str {
        &self.0.id
    }
    pub fn program_id(&self) -> &str {
        &self.0.program_id
    }
    pub fn resource(&self) -> &serde_json::Value {
        &self.0.resource
    }
    pub fn raw_operation_ceiling(&self) -> &[String] {
        &self.0.raw_operation_ceiling
    }
}

/// Original input custody, acquired before creating threads or helpers.
/// The application must transfer actual OwnedFd values exactly once. Connect
/// has no unsafe raw-number conversion, descriptor duplication or FD fallback.
pub struct ProtectedServiceInputs {
    parent: OwnedFd,
    input: OwnedFd,
    output: OwnedFd,
    sealed: transport::SealedMapping,
    mapping: mapping::ParsedMapping,
    peer: transport::VerifiedParentPeer,
    provider_store_root: std::path::PathBuf,
    _resources: crate::resources::InstalledResources,
    selected_input: Option<Arc<selected_input::SelectedInput>>,
}
impl ProtectedServiceInputs {
    pub fn adopt(
        parent: OwnedFd,
        mapping: OwnedFd,
        input: OwnedFd,
        output: OwnedFd,
    ) -> Result<Self, ConnectError> {
        Self::adopt_originals(parent, mapping, input, output, None)
    }
    /// Retain the fifth original descriptor under the authenticated v3 mapping.
    /// Existing four-descriptor adoption refuses v3 rather than discarding it.
    pub fn adopt_with_selected_input(
        parent: OwnedFd,
        mapping: OwnedFd,
        input: OwnedFd,
        output: OwnedFd,
        selected: OwnedFd,
    ) -> Result<Self, ConnectError> {
        Self::adopt_originals(parent, mapping, input, output, Some(selected))
    }
    fn adopt_originals(
        parent: OwnedFd,
        mapping: OwnedFd,
        input: OwnedFd,
        output: OwnedFd,
        selected: Option<OwnedFd>,
    ) -> Result<Self, ConnectError> {
        execution::protect_process()?;
        for descriptor in [&parent, &mapping, &input, &output] {
            transport::protect(descriptor.as_fd(), false).map_err(|_| refused())?;
        }
        if let Some(fd) = selected.as_ref() {
            transport::protect(fd.as_fd(), false).map_err(|_| refused())?;
            transport::require_distinct(&[
                fd.as_fd(),
                parent.as_fd(),
                mapping.as_fd(),
                input.as_fd(),
                output.as_fd(),
            ])
            .map_err(|_| refused())?;
        }
        let peer = transport::inspect_parent(parent.as_fd()).map_err(|_| refused())?;
        let sealed = transport::SealedMapping::read_descriptors(
            mapping,
            parent.as_fd(),
            input.as_fd(),
            output.as_fd(),
        )
        .map_err(|_| refused())?;
        let mapping = mapping::ParsedMapping::parse(sealed.bytes()).map_err(|_| refused())?;
        let selected_input = match (selected, mapping.selected_input.as_ref()) {
            (None, None) => None,
            (Some(fd), Some(identity)) => Some(selected_input::SelectedInput::adopt(
                fd,
                &[parent.as_fd(), input.as_fd(), output.as_fd()],
                identity,
            )?),
            _ => return Err(refused()),
        };
        execution::verify_identity(&mapping.execution)?;
        let resources = execution::verify_installation(&mapping.installation)?;
        let provider_store_root = execution::verify_roots(&mapping)?;
        Ok(Self {
            parent,
            input,
            output,
            sealed,
            mapping,
            peer,
            provider_store_root,
            _resources: resources,
            selected_input,
        })
    }

    /// Begin the single retained owner after entering the application's runtime.
    /// Dropping its observer does not abort this owner or acknowledge disposal.
    /// The original parent can fence it through the same retained channel.
    pub fn start(
        self,
        catalog: Arc<dyn ProtectedCollectionCatalog>,
    ) -> Result<ProtectedServiceRun, ConnectError> {
        let runtime = tokio::runtime::Handle::try_current().map_err(|_| refused())?;
        Ok(ProtectedServiceRun {
            task: runtime.spawn(lifecycle::run(self, catalog)),
        })
    }
}

#[must_use = "Join the retained protected service owner; dropping the observer is not closure"]
pub struct ProtectedServiceRun {
    task: tokio::task::JoinHandle<Result<(), ConnectError>>,
}
impl ProtectedServiceRun {
    pub async fn join(self) -> Result<(), ConnectError> {
        self.task.await.map_err(|_| refused())?
    }
}

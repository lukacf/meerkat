//! Declaration-only remote portability. No host snapshot, grant or receipt.

use crate::*;
use serde::{Deserialize, Serialize};
use url::Url;

pub const REMOTE_EXPORT_DECLARATION: &str = "remote_declaration";

/// The independent component/export facts needed to copy one remote recipe.
/// Host configuration and policy snapshots are not inputs to this inert effect.
/// Schema and lifecycle fields remain declarations, not verified observations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteExportDeclaration {
    pub schema_version: u16,
    pub selection_kind: String,
    pub package: PackageIdentity,
    pub package_content_digest: ContentDigest,
    pub strategy: Id,
    pub components: Vec<RemoteExportComponent>,
    pub exports: Vec<Export>,
    #[serde(default)]
    pub required_contracts: Vec<ContractRequirement>,
    #[serde(default)]
    pub host_services: Vec<HostServiceRequirement>,
    #[serde(default)]
    pub portable_mcp_entries: Vec<PortableMcpEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteExportComponent {
    pub id: Id,
    pub version: Revision,
    pub source: RemoteExportSource,
    pub runtime: RuntimeKind,
    pub placement: Id,
    pub lifetime: ComponentLifetime,
    pub activation_scope: ActivationScope,
    #[serde(default)]
    pub dependencies: Vec<ComponentDependency>,
    #[serde(default)]
    pub required_contracts: Vec<ContractRequirement>,
    #[serde(default)]
    pub host_services: Vec<HostServiceRequirement>,
    #[serde(default)]
    pub state: Vec<StateContract>,
    pub readiness: ReadinessContract,
    pub shutdown: ShutdownContract,
    pub rollback: RollbackContract,
}


/// A remote source declaration cannot name a host configuration revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RemoteExportSource {
    RemoteService {
        endpoint: Url,
        publisher: Id,
        observed_contract: ContentDigest,
        transport: RemoteTransport,
    },
}

/// Only the shared contract validator constructs this internal proof.
/// It is not an observation, activation or native authorization receipt.
pub(crate) struct ValidatedRemoteExportDeclaration {
    declaration: RemoteExportDeclaration,
    digest: ContentDigest,
}

impl ValidatedRemoteExportDeclaration {
    pub(crate) fn declaration(&self) -> &RemoteExportDeclaration { &self.declaration }
    pub(crate) fn digest(&self) -> &ContentDigest { &self.digest }
}

impl RemoteExportDeclaration {
    pub(crate) fn validate(self, package: &IntegrationPackage)
        -> Result<ValidatedRemoteExportDeclaration, ValidationError>
    {
        if self.selection_kind != REMOTE_EXPORT_DECLARATION
            || self.components.len() != 1 || self.exports.len() != 1
            || !matches!(&self.exports[0], Export::ServiceConnector(_))
        {
            return Err(ValidationError::new("remote_declaration",
                "remote declaration requires its explicit kind, one component and one service export"));
        }
        let components = self.components.iter().map(crate::manifest::ComponentContract::from).collect::<Vec<_>>();
        crate::manifest::validate_selection_contract(self.schema_version, &self.package,
            &self.package_content_digest, &self.strategy, &components, &self.exports,
            &self.required_contracts, &self.host_services, &self.portable_mcp_entries, package)?;
        let digest = ContentDigest::of_bytes(&canonical_json(&self)
            .map_err(|_| ValidationError::new("remote_declaration", "canonical serialization failed"))?);
        Ok(ValidatedRemoteExportDeclaration { declaration: self, digest })
    }
}

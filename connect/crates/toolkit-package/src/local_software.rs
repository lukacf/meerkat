//! Inert software selection for one plain local Python or Node MCP service.
//!
//! This record selects package bytes and launch declarations before a host has
//! selected configuration, policy or lifecycle. It cannot represent custom
//! host requirements and does not establish installation or service readiness.

use crate::{
    ArtifactDependencies, AuthenticationRequirement, ContentDigest, Entrypoint, Id,
    IntegrationPackage, OperationDeclaration, PackageIdentity, PortableMcpEntry,
    ResourceContract, Revision, RuntimeKind, StrategyKind, ValidationError, canonical_json,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalSoftwareDocumentKind {
    LocalSoftwareDeclaration,
}

/// All fields are source declarations. Unknown host/custom fields are refused,
/// not discarded or filled with invented owner references.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalSoftwareDeclaration {
    pub schema_version: u16,
    pub document_kind: LocalSoftwareDocumentKind,
    pub package: PackageIdentity,
    pub package_content_digest: ContentDigest,
    pub strategy: Id,
    pub components: Vec<LocalSoftwareComponent>,
    pub exports: Vec<LocalSoftwareExport>,
    pub portable_mcp_entries: Vec<PortableMcpEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalSoftwareComponent {
    pub id: Id,
    pub version: Revision,
    pub source: LocalSoftwareSource,
    pub runtime: RuntimeKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LocalSoftwareSource {
    Artifact {
        location: Url,
        content_digest: ContentDigest,
        dependencies: ArtifactDependencies,
        entrypoint: Entrypoint,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case", deny_unknown_fields)]
pub enum LocalSoftwareExport {
    ServiceConnector(LocalSoftwareService),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalSoftwareService {
    pub id: Id,
    pub components: BTreeSet<Id>,
    pub authentication: AuthenticationRequirement,
    pub operations: Vec<OperationDeclaration>,
}

/// Only validation constructs this proof. It is not a host compatibility,
/// artifact verification, schema observation, approval or installation receipt.
#[derive(Debug, Clone)]
pub struct ValidatedLocalSoftware {
    declaration: LocalSoftwareDeclaration,
    digest: ContentDigest,
}

impl ValidatedLocalSoftware {
    pub fn declaration(&self) -> &LocalSoftwareDeclaration { &self.declaration }
    pub fn digest(&self) -> &ContentDigest { &self.digest }
}

impl LocalSoftwareDeclaration {
    pub fn validate(self, package: &IntegrationPackage) -> Result<ValidatedLocalSoftware, ValidationError> {
        use crate::manifest::{validate_artifact_source, validate_operation_names,
            validate_export_role, validate_operations, validate_package_selection};
        let strategy = validate_package_selection(self.schema_version, &self.package,
            &self.package_content_digest, &self.strategy, package)?;
        let invalid = |message| ValidationError::new("local_software", message);
        if strategy.kind != StrategyKind::LocalMcp {
            return Err(invalid("plain software requires a local MCP strategy"));
        }
        if !strategy.known_requirements.is_empty() || !strategy.unresolved_details.is_empty() {
            return Err(invalid("known or unresolved strategy requirements need full host composition"));
        }
        if self.components.len() != 1 || self.exports.len() != 1 || self.portable_mcp_entries.len() != 1 {
            return Err(invalid("plain software requires one component, service export and portable entry"));
        }
        let component = &self.components[0];
        let LocalSoftwareExport::ServiceConnector(service) = &self.exports[0];
        let portable = &self.portable_mcp_entries[0];
        if !matches!(component.runtime, RuntimeKind::Python | RuntimeKind::Node) {
            return Err(invalid("plain software supports only Python and Node artifacts"));
        }
        if service.components.len() != 1 || !service.components.contains(&component.id)
            || portable.component != component.id {
            return Err(invalid("service and portable entry must select the sole component"));
        }
        let LocalSoftwareSource::Artifact { location, content_digest, dependencies, entrypoint } = &component.source;
        validate_artifact_source(location, content_digest, dependencies, entrypoint,
            &component.runtime, &format!("component:{}", component.id))?;
        if !matches!(dependencies, ArtifactDependencies::PinnedClosure { .. }) {
            return Err(invalid("plain software requires the exact pinned dependency closure"));
        }
        if service.operations.is_empty() {
            return Err(invalid("plain software requires at least one service operation"));
        }
        validate_export_role(package, &service.id, crate::ExportRole::ServiceConnector)?;
        validate_operations(&service.operations, &format!("export:{}", service.id))?;
        if service.operations.iter().any(|operation| !matches!(operation.resources, ResourceContract::ConnectionWide)) {
            return Err(invalid("selected resources and their enforcement owner require full host composition"));
        }
        if let Some(names) = &portable.operation_tools {
            validate_operation_names(names, &service.operations)?;
        }
        crate::manifest::validate_environment_secret_scope(portable, &service.authentication, true)?;
        let digest = ContentDigest::of_bytes(&canonical_json(&self)
            .map_err(|_| invalid("canonical serialization failed"))?);
        Ok(ValidatedLocalSoftware { declaration: self, digest })
    }
}

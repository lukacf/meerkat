//! Inert Node/npm installation projection of an existing validated selection.
//!
//! This is not an installer, grant, readiness check or new manifest authority.
//! A consumer must invoke the trusted validator against its frozen package and
//! selection inputs, then verify every selected archive before ordinary npm
//! realizes the environment. Serialized projection data cannot authorize work.

use crate::local_install::{DeclaredMcpService, local_artifact_selection, software_artifact_selection};
use crate::{
    ContentDigest, Entrypoint, Id, IntegrationPackage, PackageIdentity, RuntimeKind,
    ValidatedManifest, ValidatedLocalSoftware, LocalSoftwareDocumentKind, ValidationError,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// Derived facts for one explicitly selected local Node MCP installation.
///
/// Only validated constructors create this value; it has no deserializer.
/// Package and manifest digests bind the existing owners' canonical content.
/// The caller separately pins raw input bytes and admits the local effect.
/// `closure` contains every pinned file, including `package-json` and
/// `package-lock`. Remaining safe flat IDs name archive blobs. The installer
/// owns npm format, integrity, graph and installed-payload verification.
/// Request/Deployment lifetimes, authentication and policy remain unfulfilled
/// declarations. A generic client receives the whole server and owns launch,
/// environment, authentication, operation admission and joined shutdown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NodeInstallPlan {
    schema_version: u16,
    package: PackageIdentity,
    package_content_digest: ContentDigest,
    #[serde(skip_serializing_if = "Option::is_none")]
    manifest_digest: Option<ContentDigest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    document_kind: Option<LocalSoftwareDocumentKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    declaration_digest: Option<ContentDigest>,
    strategy: Id,
    component: Id,
    export: Id,
    artifact_digest: ContentDigest,
    closure: BTreeMap<Id, ContentDigest>,
    entrypoint: Entrypoint,
    mcp_name: Id,
    #[serde(skip_serializing_if = "Option::is_none")]
    declared_service: Option<DeclaredMcpService>,
}

impl NodeInstallPlan {
    pub fn new(
        package: &IntegrationPackage,
        validated: &ValidatedManifest,
        component_id: &Id,
    ) -> Result<Self, ValidationError> {
        let local = local_artifact_selection(
            package,
            validated,
            component_id,
            RuntimeKind::Node,
            "Node",
            "node_install",
        )?;
        let selected = validated.manifest();
        let artifacts = local.artifacts;
        node_closure(artifacts)?;
        Ok(Self {
            schema_version: 1,
            package: package.identity.clone(),
            package_content_digest: selected.package_content_digest.clone(),
            manifest_digest: Some(validated.digest().clone()),
            document_kind: None,
            declaration_digest: None,
            strategy: selected.strategy.clone(),
            component: local.component.id.clone(),
            export: local.export.id.clone(),
            artifact_digest: local.content_digest.clone(),
            closure: artifacts.clone(),
            entrypoint: local.entrypoint.clone(),
            mcp_name: local.portable.name.clone(),
            declared_service: DeclaredMcpService::from_selection(&local),
        })
    }

    /// Select software bytes without fabricating host configuration or policy.
    /// The same ecosystem checks apply; installation and native access are later.
    pub fn from_declaration(
        package: &IntegrationPackage,
        validated: &ValidatedLocalSoftware,
        component_id: &Id,
    ) -> Result<Self, ValidationError> {
        let local = software_artifact_selection(package, validated, component_id,
            RuntimeKind::Node, "node_install")?;
        let selected = validated.declaration();
        node_closure(local.artifacts)?;
        Ok(Self {
            schema_version: 1,
            package: package.identity.clone(),
            package_content_digest: selected.package_content_digest.clone(),
            manifest_digest: None,
            document_kind: Some(LocalSoftwareDocumentKind::LocalSoftwareDeclaration),
            declaration_digest: Some(validated.digest().clone()),
            strategy: selected.strategy.clone(),
            component: local.component.id.clone(),
            export: local.export.id.clone(),
            artifact_digest: local.content_digest.clone(),
            closure: local.artifacts.clone(),
            entrypoint: local.entrypoint.clone(),
            mcp_name: local.portable.name.clone(),
            declared_service: DeclaredMcpService::from_software(&local),
        })
    }
}

fn node_closure(artifacts: &BTreeMap<Id, ContentDigest>) -> Result<(), ValidationError> {
    if artifacts.len() > 256
        || !artifacts.keys().any(|id| id.as_str() == "package-json")
        || !artifacts.keys().any(|id| id.as_str() == "package-lock")
    {
        return Err(invalid(
            "closure must include both npm project files within its limit",
        ));
    }
    let mut names = BTreeSet::new();
    for id in artifacts.keys() {
        let name = id.as_str();
        if !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
            || !names.insert(name.to_ascii_lowercase())
        {
            return Err(invalid("closure IDs must be distinct safe flat filenames"));
        }
    }
    Ok(())
}

fn invalid(message: &str) -> ValidationError {
    ValidationError::new("node_install", message)
}

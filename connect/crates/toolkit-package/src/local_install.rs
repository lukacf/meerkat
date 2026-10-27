//! Shared restrictions on one independently launched local MCP selection.
//! Package and activation validation remain the canonical metadata owners.

use crate::{
    ActivationScope, ArtifactDependencies, Component, ComponentLifetime, ComponentSource,
    ContentDigest, Entrypoint, Export, Id, IntegrationPackage, PortableMcpEntry, RuntimeKind,
    ServiceConnectorExport, StrategyKind, ValidatedManifest, ValidationError,
};
use std::collections::BTreeMap;

/// A projection of the selected manifest, not observed MCP tool publication.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct DeclaredMcpService {
    authentication: crate::AuthenticationRequirement,
    operation_tools: BTreeMap<Id, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    environment_secret_slots: Option<crate::EnvironmentSecretSlots>,
}

impl DeclaredMcpService {
    pub(crate) fn from_selection(local: &LocalArtifactSelection<'_>) -> Option<Self> {
        Self::from_export(local.export, local.portable)
    }

    pub(crate) fn from_export(
        export: &ServiceConnectorExport,
        portable: &PortableMcpEntry,
    ) -> Option<Self> {
        portable.operation_tools.as_ref().map(|names| Self {
            authentication: export.authentication.clone(),
            operation_tools: names.clone(),
            environment_secret_slots: portable.environment_secret_slots.clone(),
        })
    }

    pub(crate) fn from_software(local: &LocalSoftwareArtifactSelection<'_>) -> Option<Self> {
        local.portable.operation_tools.as_ref().map(|names| Self {
            authentication: local.export.authentication.clone(),
            operation_tools: names.clone(),
            environment_secret_slots: local.portable.environment_secret_slots.clone(),
        })
    }
}
use url::Url;

pub(crate) struct LocalArtifactSelection<'a> {
    pub component: &'a Component,
    pub export: &'a ServiceConnectorExport,
    pub portable: &'a PortableMcpEntry,
    pub location: &'a Url,
    pub content_digest: &'a ContentDigest,
    pub artifacts: &'a BTreeMap<Id, ContentDigest>,
    pub entrypoint: &'a Entrypoint,
}

pub(crate) struct IndependentLocalSelection<'a> {
    pub component: &'a Component,
    pub export: &'a ServiceConnectorExport,
    pub portable: &'a PortableMcpEntry,
}

pub(crate) struct LocalSoftwareArtifactSelection<'a> {
    pub component: &'a crate::LocalSoftwareComponent,
    pub export: &'a crate::LocalSoftwareService,
    pub portable: &'a PortableMcpEntry,
    pub location: &'a Url,
    pub content_digest: &'a ContentDigest,
    pub artifacts: &'a BTreeMap<Id, ContentDigest>,
    pub entrypoint: &'a Entrypoint,
}

pub(crate) fn software_artifact_selection<'a>(
    package: &IntegrationPackage,
    validated: &'a crate::ValidatedLocalSoftware,
    component_id: &Id,
    runtime: RuntimeKind,
    error_path: &str,
) -> Result<LocalSoftwareArtifactSelection<'a>, ValidationError> {
    let invalid = |message| ValidationError::new(error_path, message);
    let selected = validated.declaration();
    if selected.package != package.identity
        || selected.package_content_digest != package.content_digest()? {
        return Err(invalid("selected package differs from the validated package"));
    }
    // Cardinality and plain-service restrictions belong to the private proof.
    let component = &selected.components[0];
    let crate::LocalSoftwareExport::ServiceConnector(export) = &selected.exports[0];
    let portable = &selected.portable_mcp_entries[0];
    if &component.id != component_id || component.runtime != runtime {
        return Err(invalid("selected component or ecosystem differs from the declaration"));
    }
    let crate::LocalSoftwareSource::Artifact { location, content_digest,
        dependencies: ArtifactDependencies::PinnedClosure { artifacts }, entrypoint } = &component.source
    else { return Err(invalid("software declaration requires a pinned artifact closure")); };
    Ok(LocalSoftwareArtifactSelection { component, export, portable, location,
        content_digest, artifacts, entrypoint })
}

pub(crate) fn local_artifact_selection<'a>(
    package: &IntegrationPackage,
    validated: &'a ValidatedManifest,
    component_id: &Id,
    runtime: RuntimeKind,
    profile: &str,
    error_path: &str,
) -> Result<LocalArtifactSelection<'a>, ValidationError> {
    let local = independent_local_selection(
        package,
        validated,
        component_id,
        runtime,
        profile,
        error_path,
    )?;
    let ComponentSource::Artifact {
        location,
        content_digest,
        dependencies: ArtifactDependencies::PinnedClosure { artifacts },
        entrypoint,
    } = &local.component.source
    else {
        return Err(ValidationError::new(
            error_path,
            format!("{profile} installation requires an artifact and pinned closure"),
        ));
    };
    Ok(LocalArtifactSelection {
        component: local.component,
        export: local.export,
        portable: local.portable,
        location,
        content_digest,
        artifacts,
        entrypoint,
    })
}

pub(crate) fn independent_local_selection<'a>(
    package: &IntegrationPackage,
    validated: &'a ValidatedManifest,
    component_id: &Id,
    runtime: RuntimeKind,
    profile: &str,
    error_path: &str,
) -> Result<IndependentLocalSelection<'a>, ValidationError> {
    let invalid = |message: &str| ValidationError::new(error_path, message);
    let selected = validated.manifest();
    if selected.package != package.identity
        || selected.package_content_digest != package.content_digest()?
    {
        return Err(invalid(
            "selected package differs from the validated package",
        ));
    }
    let strategy = package
        .strategies
        .iter()
        .find(|strategy| strategy.id == selected.strategy)
        .ok_or_else(|| invalid("selected strategy is absent"))?;
    if strategy.kind != StrategyKind::LocalMcp {
        return Err(invalid("selected strategy is not local MCP"));
    }
    if selected.components.len() != 1
        || selected.exports.len() != 1
        || selected.portable_mcp_entries.len() != 1
    {
        return Err(invalid(&format!(
            "{profile} installation requires one component, service export and portable entry"
        )));
    }
    let component = &selected.components[0];
    let portable = &selected.portable_mcp_entries[0];
    if &component.id != component_id || portable.component != component.id {
        return Err(invalid("selected component and portable entry differ"));
    }
    // The existing manifest validator owns portable-entry restrictions on
    // state, dependencies, Toolkit contracts and required host services.
    if component.runtime != runtime
        || component.placement.as_str() != "local"
        || component.activation_scope != ActivationScope::Independent
        || component.lifetime == ComponentLifetime::ExternallyManaged
    {
        return Err(invalid(&format!(
            "component is not an independent local {profile} process"
        )));
    }
    let Export::ServiceConnector(export) = &selected.exports[0] else {
        return Err(invalid(&format!(
            "{profile} MCP installation cannot expose an agent frontend"
        )));
    };
    if export.components.len() != 1
        || !export.components.contains(&component.id)
        || !export.events.is_empty()
    {
        return Err(invalid(
            "service export requires another component or event route",
        ));
    }
    Ok(IndependentLocalSelection {
        component,
        export,
        portable,
    })
}

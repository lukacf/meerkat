//! Inert Python/wheel installation projection of an existing validated selection.
//! The installer invokes this owner against frozen documents; serialized output
//! is not authorization or an independently trusted validation receipt.

use crate::local_install::{DeclaredMcpService, local_artifact_selection, software_artifact_selection};
use crate::{
    ContentDigest, Entrypoint, Id, IntegrationPackage, PackageIdentity, RuntimeKind,
    ValidatedManifest, ValidatedLocalSoftware, LocalSoftwareDocumentKind, ValidationError,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// Derived facts for one explicitly selected local Python MCP installation.
///
/// `entrypoint` remains the sole launch declaration. `module` derives from its
/// relative `.py` path for the installer's isolated `python -I -m` profile.
/// `artifact_filename` is the original wheel basename from the artifact URL;
/// closure IDs are `requirements` and original dependency wheel basenames.
/// The installer verifies wheel contents, filename/metadata agreement, module
/// ownership, import shadowing, runtime compatibility and the installed graph.
/// This value has no deserializer and cannot establish those checks or readiness.
/// Request/Deployment lifetimes, authentication and policy remain declarations;
/// the generic MCP client receives the whole endpoint and owns launch/cleanup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PythonInstallPlan {
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
    artifact_filename: String,
    module: String,
}

impl PythonInstallPlan {
    pub fn new(
        package: &IntegrationPackage,
        validated: &ValidatedManifest,
        component_id: &Id,
    ) -> Result<Self, ValidationError> {
        let local = local_artifact_selection(
            package,
            validated,
            component_id,
            RuntimeKind::Python,
            "Python",
            "python_install",
        )?;
        let selected = validated.manifest();
        let (artifact_filename, module) = python_artifact(local.location, local.artifacts, local.entrypoint)?;
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
            closure: local.artifacts.clone(),
            entrypoint: local.entrypoint.clone(),
            mcp_name: local.portable.name.clone(),
            declared_service: DeclaredMcpService::from_selection(&local),
            artifact_filename,
            module,
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
            RuntimeKind::Python, "python_install")?;
        let selected = validated.declaration();
        let (artifact_filename, module) = python_artifact(local.location, local.artifacts, local.entrypoint)?;
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
            artifact_filename,
            module,
        })
    }
}

fn python_artifact(
    location: &url::Url, artifacts: &BTreeMap<Id, ContentDigest>, entrypoint: &Entrypoint,
) -> Result<(String, String), ValidationError> {
    let artifact_filename = location
        .path_segments()
        .and_then(|mut parts| parts.next_back())
        .filter(|name| wheel_filename(name))
        .ok_or_else(|| invalid("artifact URL must end in a safe original wheel filename"))?;
    if artifacts.len() > 256
        || !artifacts
            .keys()
            .any(|id| id.as_str() == "requirements")
    {
        return Err(invalid(
            "closure must include requirements within its limit",
        ));
    }
    // The primary wheel is a separate pinned artifact, not a second closure
    // file. Aliases would make the eventual flat wheelhouse ambiguous.
    let mut names = BTreeSet::from([artifact_filename.to_ascii_lowercase()]);
    for id in artifacts.keys() {
        let name = id.as_str();
        if (name != "requirements" && !wheel_filename(name))
            || !names.insert(name.to_ascii_lowercase())
        {
            return Err(invalid(
                "closure IDs must be distinct original wheel filenames",
            ));
        }
    }
    let module = module_from_entrypoint(
        &entrypoint.executable,
        entrypoint.python_console_script.is_some(),
    )?;
    Ok((artifact_filename.to_owned(), module))
}

fn wheel_filename(name: &str) -> bool {
    name.len() <= 192
        && name.ends_with(".whl")
        && name
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

fn module_from_entrypoint(executable: &str, console_script: bool) -> Result<String, ValidationError> {
    let path = executable
        .strip_suffix(".py")
        .ok_or_else(|| invalid("Python entrypoint must be a relative .py module path"))?;
    let mut parts: Vec<_> = path.split('/').collect();
    if console_script && parts.last() == Some(&"__init__") {
        parts.pop();
    }
    if parts.is_empty()
        || (!console_script && parts.last() == Some(&"__init__"))
        || parts.iter().any(|part| !identifier(part))
    {
        return Err(invalid(
            "Python entrypoint must use module identifiers; package initializers require a console script",
        ));
    }
    Ok(parts.join("."))
}

fn identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn invalid(message: &str) -> ValidationError {
    ValidationError::new("python_install", message)
}

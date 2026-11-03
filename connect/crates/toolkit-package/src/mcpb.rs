//! MCPB 0.3 manifest import into inert setup evidence. Bundle extraction,
//! executable verification, credential binding and process launch belong to the
//! artifact/auth/deployment owners and are not implied by successful parsing.
//! Source: https://github.com/modelcontextprotocol/mcpb/blob/main/MANIFEST.md

use crate::{ContentDigest, Id, RuntimeKind, ValidationError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpbManifest {
    pub manifest_version: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: McpbAuthor,
    pub server: McpbServer,
    #[serde(default)]
    pub user_config: BTreeMap<String, McpbUserConfiguration>,
    #[serde(default)]
    pub compatibility: BTreeMap<String, Value>,
    pub license: Option<String>,
    /// Other upstream metadata is retained as inert data, not interpreted as
    /// activation permissions, executable configuration, or tool schemas.
    #[serde(flatten)]
    pub metadata: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpbAuthor {
    pub name: String,
    pub email: Option<String>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpbServer {
    #[serde(rename = "type")]
    pub runtime: McpbRuntime,
    pub entry_point: String,
    pub mcp_config: McpbProcessConfiguration,
    #[serde(flatten)]
    pub metadata: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpbRuntime {
    Node,
    Python,
    Binary,
}

impl From<McpbRuntime> for RuntimeKind {
    fn from(value: McpbRuntime) -> Self {
        match value {
            McpbRuntime::Node => Self::Node,
            McpbRuntime::Python => Self::Python,
            McpbRuntime::Binary => Self::Native,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpbProcessConfiguration {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub platform_overrides: BTreeMap<String, Value>,
    #[serde(flatten)]
    pub metadata: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpbUserConfiguration {
    #[serde(rename = "type")]
    pub value_type: McpbValueType,
    pub title: String,
    pub description: Option<String>,
    #[serde(default)]
    pub sensitive: bool,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub multiple: bool,
    pub default: Option<Value>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    #[serde(flatten)]
    pub metadata: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpbValueType {
    String,
    Number,
    Boolean,
    Directory,
    File,
}

#[derive(Debug, Clone)]
pub struct ImportedMcpbManifest {
    manifest: McpbManifest,
    source_digest: ContentDigest,
}

impl ImportedMcpbManifest {
    pub fn parse(bytes: &[u8]) -> Result<Self, ValidationError> {
        // Reject duplicate decoded keys before typed maps can erase declarations.
        crate::strict_mcpb_json(bytes)
            .map_err(|_| ValidationError::new("manifest.json", "invalid MCPB manifest"))?;
        let manifest: McpbManifest = serde_json::from_slice(bytes)
            .map_err(|_| ValidationError::new("manifest.json", "invalid MCPB manifest"))?;
        if manifest.manifest_version != "0.3" {
            return Err(ValidationError::new(
                "manifest_version",
                "unsupported MCPB version",
            ));
        }
        for (field, value) in [
            ("name", &manifest.name),
            ("version", &manifest.version),
            ("description", &manifest.description),
            ("author.name", &manifest.author.name),
        ] {
            crate::manifest::nonempty(value, field, "required MCPB metadata is empty")?;
        }
        validate_bundle_path(&manifest.server.entry_point)?;
        if manifest.server.mcp_config.command.trim().is_empty()
            || manifest.server.mcp_config.command.contains('\0')
        {
            return Err(ValidationError::new(
                "server.mcp_config.command",
                "missing MCPB executable",
            ));
        }
        for (name, configuration) in &manifest.user_config {
            Id::new(name)
                .map_err(|_| ValidationError::new("user_config", "invalid configuration key"))?;
            crate::manifest::nonempty(
                &configuration.title,
                "user_config.title",
                "configuration title is empty",
            )?;
            if configuration.sensitive && configuration.default.is_some() {
                return Err(ValidationError::new(
                    "user_config.default",
                    "secret defaults must not be imported into package metadata",
                ));
            }
            if configuration
                .min
                .zip(configuration.max)
                .is_some_and(|(min, max)| min > max)
            {
                return Err(ValidationError::new(
                    "user_config",
                    "minimum exceeds maximum",
                ));
            }
        }
        // Preserve provider/platform overrides as explicit unresolved evidence.
        // This parser cannot turn those opaque values into a validated selection.
        Ok(Self {
            manifest,
            source_digest: ContentDigest::of_bytes(bytes),
        })
    }

    pub fn manifest(&self) -> &McpbManifest {
        &self.manifest
    }

    /// Digest of supplied manifest bytes, not the bundle or executable bytes.
    pub fn source_digest(&self) -> &ContentDigest {
        &self.source_digest
    }

    pub fn sensitive_fields(&self) -> impl Iterator<Item = (&str, &McpbUserConfiguration)> {
        self.manifest
            .user_config
            .iter()
            .filter(|(_, field)| field.sensitive)
            .map(|(key, field)| (key.as_str(), field))
    }

    /// A host must resolve these before preparing an exact activation manifest.
    pub fn unresolved_contracts(&self) -> Vec<McpbUnresolvedContract> {
        let mut unresolved = vec![
            McpbUnresolvedContract::BundleArtifactAndDependencyPins,
            McpbUnresolvedContract::OperationSchemasAndClassification,
            McpbUnresolvedContract::AuthenticationAndAuthority,
            McpbUnresolvedContract::LifecycleAndReadiness,
        ];
        if !self.manifest.compatibility.is_empty()
            || !self
                .manifest
                .server
                .mcp_config
                .platform_overrides
                .is_empty()
        {
            unresolved.push(McpbUnresolvedContract::SelectedPlatformCompatibility);
        }
        if !self.manifest.user_config.is_empty() {
            unresolved.push(McpbUnresolvedContract::HostOwnedConfigurationAndSecretBindings);
        }
        unresolved
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpbUnresolvedContract {
    BundleArtifactAndDependencyPins,
    OperationSchemasAndClassification,
    AuthenticationAndAuthority,
    LifecycleAndReadiness,
    SelectedPlatformCompatibility,
    HostOwnedConfigurationAndSecretBindings,
}

fn validate_bundle_path(path: &str) -> Result<(), ValidationError> {
    if path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path.contains(':')
        || path.chars().any(char::is_control)
        || path
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err(ValidationError::new(
            "server.entry_point",
            "entry point must stay within the verified bundle",
        ));
    }
    Ok(())
}

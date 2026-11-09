//! Launch projection for a verified, self-contained Node MCPB 0.3 archive.
//! Artifact preparation and package validation remain the existing owners.

#[cfg(all(
    feature = "local-artifacts",
    any(target_os = "linux", target_os = "macos")
))]
use crate::local_artifacts::{LocalArtifactLayout, PreparedLocalArtifact};
use crate::local_install::independent_local_selection;
use crate::{
    ArtifactDependencies, AuthenticationRequirement, ComponentSource, ContentDigest, Entrypoint,
    Id, IntegrationPackage, PackageIdentity, RuntimeKind, ValidatedManifest, ValidationError,
};
#[cfg(all(
    feature = "local-artifacts",
    any(target_os = "linux", target_os = "macos")
))]
use crate::{ImportedMcpbManifest, McpbRuntime, McpbValueType};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
#[cfg(all(
    feature = "local-artifacts",
    any(target_os = "linux", target_os = "macos")
))]
use std::collections::BTreeSet;

/// Serialized observations cannot grant, activate or filter the MCP endpoint.
#[derive(Debug, Clone, Serialize)]
pub struct McpbInstallPlan {
    pub schema_version: u16,
    pub package: PackageIdentity,
    pub package_content_digest: ContentDigest,
    pub manifest_digest: ContentDigest,
    pub component: Id,
    pub artifact_digest: ContentDigest,
    pub mcpb_manifest_digest: ContentDigest,
    pub configuration_digest: ContentDigest,
    pub prepared_evidence_digest: ContentDigest,
    pub entrypoint: Entrypoint,
    pub mcp_name: Id,
    pub minimum_node_version: [u32; 3],
}

impl McpbInstallPlan {
    pub fn validate_selection(
        package: &IntegrationPackage,
        selected: &ValidatedManifest,
        component: &Id,
    ) -> Result<(), ValidationError> {
        let local = independent_local_selection(
            package,
            selected,
            component,
            RuntimeKind::Node,
            "MCPB",
            "mcpb_install",
        )?;
        if local.export.authentication != AuthenticationRequirement::None
            || !matches!(
                local.component.source,
                ComponentSource::Artifact {
                    dependencies: ArtifactDependencies::SelfContained,
                    ..
                }
            )
        {
            return Err(invalid(
                "MCPB profile requires a self-contained artifact without authentication",
            ));
        }
        Ok(())
    }

    /// Validate explicit caller-designated nonsecret configuration metadata.
    /// This cannot check field sensitivity, completeness or argument expansion
    /// against an archive. Those checks require `new` and verified bytes.
    pub fn validate_configuration(
        selected: &ValidatedManifest,
        configuration: &[u8],
    ) -> Result<BTreeMap<String, String>, ValidationError> {
        if ContentDigest::of_bytes(configuration)
            != selected.manifest().configuration.content_digest
        {
            return Err(invalid("configuration bytes differ from selection"));
        }
        let config: BTreeMap<String, String> =
            serde_json::from_value(strict_mcpb_json(configuration)?)
                .map_err(|_| invalid("configuration must contain explicit nonsecret strings"))?;
        if config.len() > 64 {
            return Err(invalid(
                "configuration must explicitly supply every declared field",
            ));
        }
        for (name, value) in &config {
            if !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            {
                return Err(invalid("unsupported MCPB configuration field"));
            }
            if value.len() > 4096 || value.contains('$') || value.chars().any(char::is_control) {
                return Err(invalid("unsupported MCPB configuration value"));
            }
        }
        Ok(config)
    }

    #[cfg(all(
        feature = "local-artifacts",
        any(target_os = "linux", target_os = "macos")
    ))]
    pub fn new(
        package: &IntegrationPackage,
        selected: &ValidatedManifest,
        component: &Id,
        prepared: &PreparedLocalArtifact,
        configuration: &[u8],
    ) -> Result<Self, ValidationError> {
        Self::validate_selection(package, selected, component)?;
        let local = independent_local_selection(
            package,
            selected,
            component,
            RuntimeKind::Node,
            "MCPB",
            "mcpb_install",
        )?;
        let evidence = prepared.evidence();
        if evidence.manifest_digest() != selected.digest()
            || evidence.component() != component
            || evidence.layout() != &LocalArtifactLayout::Zip
        {
            return Err(invalid("prepared bytes belong to another selection"));
        }
        let ComponentSource::Artifact {
            content_digest,
            entrypoint,
            ..
        } = &local.component.source
        else {
            unreachable!()
        };
        let configuration_digest = ContentDigest::of_bytes(configuration);
        let config = Self::validate_configuration(selected, configuration)?;
        let raw = prepared
            .read("manifest.json", 1024 * 1024)
            .map_err(|_| invalid("verified manifest is unavailable"))?;
        strict_mcpb_json(&raw)?;
        let imported = ImportedMcpbManifest::parse(&raw)?;
        let manifest = imported.manifest();
        if manifest.name != package.identity.name.as_str()
            || manifest.version != package.identity.version.as_str()
            || manifest.version != local.component.version.as_str()
            || manifest.server.runtime != McpbRuntime::Node
            || manifest.server.entry_point != entrypoint.executable
            || manifest.server.mcp_config.command != "node"
            || !manifest.server.metadata.is_empty()
            || !manifest.server.mcp_config.metadata.is_empty()
            || !manifest.server.mcp_config.env.is_empty()
            || !manifest.server.mcp_config.platform_overrides.is_empty()
        {
            return Err(invalid(
                "unsupported or inconsistent MCPB launch declaration",
            ));
        }
        // Unknown executable/setup semantics are refused, not silently ignored.
        let descriptive = [
            "display_name",
            "long_description",
            "icon",
            "tools",
            "tools_generated",
            "prompts",
            "prompts_generated",
            "keywords",
            "repository",
            "homepage",
            "documentation",
            "support",
        ];
        if manifest
            .metadata
            .keys()
            .any(|key| !descriptive.contains(&key.as_str()))
        {
            return Err(invalid("unsupported MCPB metadata"));
        }
        safe_script(&entrypoint.executable)?;
        prepared
            .read(&entrypoint.executable, 64 * 1024 * 1024)
            .map_err(|_| invalid("selected script is not verified"))?;
        if config.keys().ne(manifest.user_config.keys()) {
            return Err(invalid(
                "configuration must explicitly supply every declared field",
            ));
        }
        for field in manifest.user_config.values() {
            if field.value_type != McpbValueType::String
                || field.sensitive
                || field.multiple
                || field.min.is_some()
                || field.max.is_some()
                || !field.metadata.is_empty()
                || field
                    .default
                    .as_ref()
                    .is_some_and(|value| !value.is_string())
            {
                return Err(invalid("unsupported MCPB configuration field"));
            }
        }
        let args = &manifest.server.mcp_config.args;
        if args.is_empty()
            || args.len() > 129
            || args[0] != format!("${{__dirname}}/{}", entrypoint.executable)
        {
            return Err(invalid(
                "MCPB script argument differs from selected entrypoint",
            ));
        }
        let arguments = args[1..]
            .iter()
            .map(|arg| substitute(arg, &config))
            .collect::<Result<Vec<_>, _>>()?;
        if arguments != entrypoint.arguments {
            return Err(invalid("MCPB arguments differ from selected literal argv"));
        }
        let compatibility = &manifest.compatibility;
        if compatibility.len() != 2
            || !compatibility.contains_key("platforms")
            || !compatibility.contains_key("runtimes")
        {
            return Err(invalid("unsupported MCPB compatibility declaration"));
        }
        let platforms: Vec<String> = serde_json::from_value(compatibility["platforms"].clone())
            .map_err(|_| invalid("invalid MCPB platforms"))?;
        let host = if cfg!(target_os = "macos") {
            "darwin"
        } else {
            "linux"
        };
        if platforms.is_empty()
            || platforms.len() > 3
            || !platforms.iter().any(|value| value == host)
            || platforms.iter().collect::<BTreeSet<_>>().len() != platforms.len()
            || platforms
                .iter()
                .any(|value| !["darwin", "linux", "win32"].contains(&value.as_str()))
        {
            return Err(invalid("unsupported MCPB platform"));
        }
        let runtimes: BTreeMap<String, String> =
            serde_json::from_value(compatibility["runtimes"].clone())
                .map_err(|_| invalid("invalid MCPB runtime requirements"))?;
        let range = runtimes
            .get("node")
            .filter(|_| runtimes.len() == 1)
            .and_then(|value| value.strip_prefix(">="))
            .ok_or_else(|| invalid("only one explicit minimum Node version is supported"))?;
        let minimum_node_version = node_version(range)?;
        Ok(Self {
            schema_version: 1,
            package: package.identity.clone(),
            package_content_digest: selected.manifest().package_content_digest.clone(),
            manifest_digest: selected.digest().clone(),
            component: component.clone(),
            artifact_digest: content_digest.clone(),
            mcpb_manifest_digest: imported.source_digest().clone(),
            configuration_digest,
            prepared_evidence_digest: evidence.digest().clone(),
            entrypoint: entrypoint.clone(),
            mcp_name: local.portable.name.clone(),
            minimum_node_version,
        })
    }

    pub fn check_node_version(&self, version: &str) -> Result<(), ValidationError> {
        let actual = node_version(
            version
                .strip_prefix('v')
                .ok_or_else(|| invalid("invalid Node version"))?,
        )?;
        if actual < self.minimum_node_version {
            return Err(invalid("Node version is below selected requirement"));
        }
        Ok(())
    }
}

#[cfg(all(
    feature = "local-artifacts",
    any(target_os = "linux", target_os = "macos")
))]
fn safe_script(path: &str) -> Result<(), ValidationError> {
    if path.len() > 240
        || !(path.ends_with(".js") || path.ends_with(".mjs") || path.ends_with(".cjs"))
        || path.split('/').any(|part| {
            part.is_empty()
                || part == "."
                || part == ".."
                || !part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        })
    {
        return Err(invalid("unsupported MCPB script path"));
    }
    Ok(())
}

#[cfg(all(
    feature = "local-artifacts",
    any(target_os = "linux", target_os = "macos")
))]
fn substitute(
    template: &str,
    config: &BTreeMap<String, String>,
) -> Result<String, ValidationError> {
    if template.len() > 8192 || template.chars().any(char::is_control) {
        return Err(invalid("invalid MCPB argument"));
    }
    let mut output = String::new();
    let mut rest = template;
    while let Some(index) = rest.find('$') {
        output.push_str(&rest[..index]);
        let variable = rest[index..]
            .strip_prefix("${user_config.")
            .ok_or_else(|| invalid("unsupported MCPB substitution"))?;
        let (name, suffix) = variable
            .split_once('}')
            .ok_or_else(|| invalid("invalid MCPB substitution"))?;
        output.push_str(
            config
                .get(name)
                .ok_or_else(|| invalid("unknown MCPB configuration field"))?,
        );
        rest = suffix;
        if output.len() > 8192 {
            return Err(invalid("MCPB argument exceeds limit"));
        }
    }
    output.push_str(rest);
    if output.len() > 8192 {
        return Err(invalid("MCPB argument exceeds limit"));
    }
    Ok(output)
}

fn node_version(value: &str) -> Result<[u32; 3], ValidationError> {
    let parts = value.split('.').collect::<Vec<_>>();
    if parts.len() != 3
        || parts.iter().any(|part| {
            part.is_empty()
                || part.len() > 9
                || !part.bytes().all(|byte| byte.is_ascii_digit())
                || (part.len() > 1 && part.starts_with('0'))
        })
    {
        return Err(invalid("unsupported Node version syntax"));
    }
    Ok([
        parts[0]
            .parse()
            .map_err(|_| invalid("invalid Node version"))?,
        parts[1]
            .parse()
            .map_err(|_| invalid("invalid Node version"))?,
        parts[2]
            .parse()
            .map_err(|_| invalid("invalid Node version"))?,
    ])
}

/// Bounded callers use this before typed parsing to refuse duplicate JSON keys.
pub fn strict_mcpb_json(bytes: &[u8]) -> Result<Value, ValidationError> {
    struct Unique(Value);
    impl<'de> Deserialize<'de> for Unique {
        fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct Visitor;
            impl<'de> serde::de::Visitor<'de> for Visitor {
                type Value = Unique;
                fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    formatter.write_str("unique-key JSON")
                }
                fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Unique, E> {
                    Ok(Unique(v.into()))
                }
                fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Unique, E> {
                    Ok(Unique(v.into()))
                }
                fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Unique, E> {
                    Ok(Unique(v.into()))
                }
                fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Unique, E> {
                    serde_json::Number::from_f64(v)
                        .map(|n| Unique(Value::Number(n)))
                        .ok_or_else(|| E::custom("invalid number"))
                }
                fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Unique, E> {
                    Ok(Unique(v.into()))
                }
                fn visit_unit<E: serde::de::Error>(self) -> Result<Unique, E> {
                    Ok(Unique(Value::Null))
                }
                fn visit_seq<A: serde::de::SeqAccess<'de>>(
                    self,
                    mut access: A,
                ) -> Result<Unique, A::Error> {
                    let mut values = Vec::new();
                    while let Some(Unique(v)) = access.next_element()? {
                        values.push(v);
                    }
                    Ok(Unique(Value::Array(values)))
                }
                fn visit_map<A: serde::de::MapAccess<'de>>(
                    self,
                    mut access: A,
                ) -> Result<Unique, A::Error> {
                    let mut values = serde_json::Map::new();
                    while let Some((key, Unique(v))) = access.next_entry::<String, Unique>()? {
                        if values.insert(key, v).is_some() {
                            return Err(serde::de::Error::custom("duplicate key"));
                        }
                    }
                    Ok(Unique(Value::Object(values)))
                }
            }
            deserializer.deserialize_any(Visitor)
        }
    }
    serde_json::from_slice::<Unique>(bytes)
        .map(|value| value.0)
        .map_err(|_| invalid("invalid or duplicate-key JSON"))
}

fn invalid(message: &str) -> ValidationError {
    ValidationError::new("mcpb_install", message)
}

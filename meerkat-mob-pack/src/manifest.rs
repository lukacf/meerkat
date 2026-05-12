use serde::de;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{BTreeMap, BTreeSet};

use meerkat_contracts::capability::{MobpackCapabilityId, MobpackCapabilityRequirement};
use meerkat_mob::MobDefinition;

use crate::targz::normalize_for_archive;
use crate::validate::PackValidationError;

macro_rules! manifest_string_id {
    ($name:ident, $parse_fn:path, $expectation:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn parse(value: impl Into<String>) -> Result<Self, String> {
                let value = value.into();
                if $parse_fn(&value) {
                    Ok(Self(value))
                } else {
                    Err($expectation.to_string())
                }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::parse(value).map_err(de::Error::custom)
            }
        }
    };
}

manifest_string_id!(
    MobpackModelAlias,
    valid_manifest_identifier,
    "must be a non-empty identifier starting with a letter or underscore"
);
manifest_string_id!(
    MobpackModelRef,
    valid_model_ref,
    "must be a non-empty model reference"
);
manifest_string_id!(
    MobpackProfileSelector,
    meerkat_mob::validate::is_valid_profile_name,
    "must be a valid mob profile name"
);
manifest_string_id!(
    MobpackSkillPath,
    valid_manifest_skill_path,
    "must be a canonical archive path under skills/"
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MobpackSurfaceSelector {
    Cli,
    Rpc,
    Rest,
    Mcp,
    Web,
}

impl MobpackSurfaceSelector {
    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw {
            "cli" => Ok(Self::Cli),
            "rpc" => Ok(Self::Rpc),
            "rest" => Ok(Self::Rest),
            "mcp" => Ok(Self::Mcp),
            "web" => Ok(Self::Web),
            _ => Err("must be one of cli, rpc, rest, mcp, or web".to_string()),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Rpc => "rpc",
            Self::Rest => "rest",
            Self::Mcp => "mcp",
            Self::Web => "web",
        }
    }
}

impl std::fmt::Display for MobpackSurfaceSelector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for MobpackSurfaceSelector {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for MobpackSurfaceSelector {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobpackManifest {
    pub mobpack: MobpackSection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires: Option<RequiresSection>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub models: BTreeMap<MobpackModelAlias, MobpackModelRef>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub profiles: BTreeMap<MobpackProfileSelector, ProfileSection>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub surfaces: BTreeSet<MobpackSurfaceSelector>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobpackSection {
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequiresSection {
    #[serde(default)]
    pub capabilities: Vec<MobpackManifestCapability>,
}

impl RequiresSection {
    pub fn typed_capabilities(&self) -> impl Iterator<Item = &MobpackManifestCapability> + '_ {
        self.capabilities.iter()
    }
}

/// Capability requirement from `manifest.toml`.
///
/// The archive format keeps the historical string-array shape, but the parsed
/// manifest carries the classified capability identity as the operative value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobpackManifestCapability {
    raw: String,
    id: MobpackCapabilityId,
}

impl MobpackManifestCapability {
    pub fn parse(raw: impl Into<String>) -> Self {
        let raw = raw.into();
        let id = MobpackCapabilityRequirement::parse(&raw).id();
        Self { raw, id }
    }

    pub fn raw(&self) -> &str {
        &self.raw
    }

    pub fn id(&self) -> MobpackCapabilityId {
        self.id
    }
}

impl Serialize for MobpackManifestCapability {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.raw)
    }
}

impl<'de> Deserialize<'de> for MobpackManifestCapability {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(Self::parse(String::deserialize(deserializer)?))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileSection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<MobpackModelAlias>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<MobpackSkillPath>,
}

impl MobpackManifest {
    pub fn validate_contract(
        &self,
        definition: &MobDefinition,
        files: &BTreeMap<String, Vec<u8>>,
    ) -> Result<(), PackValidationError> {
        for (profile_name, profile) in &self.profiles {
            if !definition.profiles.contains_key(profile_name.as_str()) {
                return Err(PackValidationError::InvalidManifestField {
                    field: format!("profiles.{profile_name}"),
                    reason: "profile selector is not defined in definition.json".to_string(),
                });
            }

            if let Some(model_alias) = &profile.model {
                if self.models.is_empty() {
                    return Err(PackValidationError::InvalidManifestField {
                        field: format!("profiles.{profile_name}.model"),
                        reason: "profile model references require a [models] alias table"
                            .to_string(),
                    });
                }
                if !self.models.contains_key(model_alias) {
                    return Err(PackValidationError::InvalidManifestField {
                        field: format!("profiles.{profile_name}.model"),
                        reason: format!("unknown model alias '{model_alias}'"),
                    });
                }
            }

            for (index, skill_path) in profile.skills.iter().enumerate() {
                let Some(bytes) = files.get(skill_path.as_str()) else {
                    return Err(PackValidationError::InvalidManifestField {
                        field: format!("profiles.{profile_name}.skills[{index}]"),
                        reason: format!("skill file '{skill_path}' missing from archive"),
                    });
                };
                std::str::from_utf8(bytes).map_err(|err| {
                    PackValidationError::InvalidManifestField {
                        field: format!("profiles.{profile_name}.skills[{index}]"),
                        reason: format!("skill file '{skill_path}' is not valid UTF-8: {err}"),
                    }
                })?;
            }
        }
        Ok(())
    }
}

fn valid_manifest_identifier(value: &str) -> bool {
    meerkat_mob::validate::is_valid_profile_name(value)
}

fn valid_model_ref(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty() && trimmed == value && !trimmed.chars().any(char::is_control)
}

fn valid_manifest_skill_path(value: &str) -> bool {
    match normalize_for_archive(value) {
        Ok(normalized) => normalized == value && normalized.starts_with("skills/"),
        Err(_) => false,
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use meerkat_contracts::capability::{
        CapabilityId, HostProcessCapabilityId, MobpackCapabilityId,
    };

    #[test]
    fn test_manifest_toml_roundtrip() {
        let mut models = BTreeMap::new();
        models.insert(
            MobpackModelAlias::parse("planner").unwrap(),
            MobpackModelRef::parse("gpt-5").unwrap(),
        );
        models.insert(
            MobpackModelAlias::parse("reviewer").unwrap(),
            MobpackModelRef::parse("gpt-5-mini").unwrap(),
        );

        let mut profiles = BTreeMap::new();
        profiles.insert(
            MobpackProfileSelector::parse("lead").unwrap(),
            ProfileSection {
                model: Some(MobpackModelAlias::parse("planner").unwrap()),
                skills: vec![MobpackSkillPath::parse("skills/lead.md").unwrap()],
            },
        );
        profiles.insert(
            MobpackProfileSelector::parse("reviewer").unwrap(),
            ProfileSection {
                model: Some(MobpackModelAlias::parse("reviewer").unwrap()),
                skills: vec![MobpackSkillPath::parse("skills/review.md").unwrap()],
            },
        );

        let manifest = MobpackManifest {
            mobpack: MobpackSection {
                name: "code-review".to_string(),
                version: "1.0.0".to_string(),
                description: Some("Review assistant".to_string()),
            },
            requires: Some(RequiresSection {
                capabilities: vec![
                    MobpackManifestCapability::parse("comms"),
                    MobpackManifestCapability::parse("shell"),
                ],
            }),
            models,
            profiles,
            surfaces: BTreeSet::from([MobpackSurfaceSelector::Cli, MobpackSurfaceSelector::Rpc]),
        };

        let encoded = toml::to_string(&manifest).unwrap();
        let decoded: MobpackManifest = toml::from_str(&encoded).unwrap();
        assert_eq!(decoded, manifest);
    }

    #[test]
    fn test_requires_section_exposes_typed_capabilities_and_serializes_compat_strings() {
        let manifest: MobpackManifest = toml::from_str(
            r#"
[mobpack]
name = "browser-test"
version = "1.0.0"

[requires]
capabilities = ["comms", "shell", "mcp_stdio", "vendor.custom"]
"#,
        )
        .unwrap();
        let requires = manifest.requires.unwrap();

        assert_eq!(
            requires
                .capabilities
                .iter()
                .map(MobpackManifestCapability::raw)
                .collect::<Vec<_>>(),
            vec!["comms", "shell", "mcp_stdio", "vendor.custom"]
        );

        let typed = requires
            .typed_capabilities()
            .map(MobpackManifestCapability::id)
            .collect::<Vec<_>>();
        assert_eq!(
            typed,
            vec![
                MobpackCapabilityId::Known(CapabilityId::Comms),
                MobpackCapabilityId::Known(CapabilityId::Shell),
                MobpackCapabilityId::HostProcess(HostProcessCapabilityId::McpStdio),
                MobpackCapabilityId::Unknown,
            ]
        );

        let encoded = toml::to_string(&RequiresSection {
            capabilities: requires.capabilities,
        })
        .unwrap();
        assert!(
            encoded.contains(
                "capabilities = [\"comms\", \"shell\", \"mcp_stdio\", \"vendor.custom\"]"
            )
        );
    }

    #[test]
    fn manifest_rejects_unknown_surface_selectors() {
        let err = toml::from_str::<MobpackManifest>(
            r#"
surfaces = ["cli", "spaceship"]

[mobpack]
name = "bad-surface"
version = "1.0.0"
"#,
        )
        .expect_err("unknown surface should be rejected by the manifest owner");

        assert!(err.to_string().contains("cli, rpc, rest, mcp, or web"));
    }

    #[test]
    fn manifest_rejects_raw_profile_skill_paths() {
        let err = toml::from_str::<MobpackManifest>(
            r#"
[mobpack]
name = "bad-skill"
version = "1.0.0"

[profiles.worker]
skills = ["../review.md"]
"#,
        )
        .expect_err("path traversal must be rejected while parsing manifest skills");

        assert!(
            err.to_string()
                .contains("canonical archive path under skills/")
        );
    }
}

use serde::de::Error as DeError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{BTreeMap, BTreeSet};

use meerkat_contracts::capability::{MobpackCapabilityId, MobpackCapabilityRequirement};

use crate::vocabulary::{ModelAlias, ModelRef, SkillPath, SurfaceSelector};

/// A capability token declared in a mobpack manifest, parsed once at the TOML
/// boundary.
///
/// The typed [`MobpackCapabilityId`] is the stored shape — it is classified at
/// deserialization, not re-parsed on every read. The original token is retained
/// so the value round-trips exactly and so unknown vendor tokens keep their
/// surface text for diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobpackCapability {
    token: String,
    id: MobpackCapabilityId,
}

impl MobpackCapability {
    /// Classify a raw capability token into its typed identity, storing both.
    pub fn parse(token: impl Into<String>) -> Self {
        let token = token.into();
        let id = MobpackCapabilityRequirement::parse(&token).id();
        Self { token, id }
    }

    /// The typed identity decided once at the TOML boundary.
    pub fn id(&self) -> MobpackCapabilityId {
        self.id
    }

    /// The original token text, retained for round-trip and diagnostics.
    pub fn token(&self) -> &str {
        &self.token
    }
}

impl std::fmt::Display for MobpackCapability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.token)
    }
}

impl Serialize for MobpackCapability {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.token)
    }
}

impl<'de> Deserialize<'de> for MobpackCapability {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let token = String::deserialize(deserializer)?;
        if token.trim().is_empty() {
            return Err(D::Error::custom("capability token must not be empty"));
        }
        Ok(MobpackCapability::parse(token))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobpackManifest {
    pub mobpack: MobpackSection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires: Option<RequiresSection>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub models: BTreeMap<ModelAlias, ModelRef>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub profiles: BTreeMap<String, ProfileSection>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub surfaces: BTreeSet<SurfaceSelector>,
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
    pub capabilities: Vec<MobpackCapability>,
}

impl RequiresSection {
    /// The typed capability ids, read directly from the stored shape (classified
    /// once at deserialization, not re-parsed here).
    pub fn capability_ids(&self) -> impl Iterator<Item = MobpackCapabilityId> + '_ {
        self.capabilities.iter().map(MobpackCapability::id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileSection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelAlias>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<SkillPath>,
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use meerkat_contracts::capability::{CapabilityId, HostProcessCapabilityId};
    use std::str::FromStr;

    fn alias(raw: &str) -> ModelAlias {
        ModelAlias::from_str(raw).unwrap()
    }

    fn model(raw: &str) -> ModelRef {
        ModelRef::from_str(raw).unwrap()
    }

    fn skill(raw: &str) -> SkillPath {
        SkillPath::from_str(raw).unwrap()
    }

    #[test]
    fn test_manifest_toml_roundtrip() {
        let mut models = BTreeMap::new();
        models.insert(alias("planner"), model("gpt-5"));
        models.insert(alias("reviewer"), model("gpt-5-mini"));

        let mut profiles = BTreeMap::new();
        profiles.insert(
            "lead".to_string(),
            ProfileSection {
                model: Some(alias("planner")),
                skills: vec![skill("skills/lead.md")],
            },
        );
        profiles.insert(
            "reviewer".to_string(),
            ProfileSection {
                model: Some(alias("reviewer")),
                skills: vec![skill("skills/review.md")],
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
                    MobpackCapability::parse("comms"),
                    MobpackCapability::parse("shell"),
                ],
            }),
            models,
            profiles,
            surfaces: BTreeSet::from([SurfaceSelector::Cli, SurfaceSelector::Rpc]),
        };

        let encoded = toml::to_string(&manifest).unwrap();
        let decoded: MobpackManifest = toml::from_str(&encoded).unwrap();
        assert_eq!(decoded, manifest);
    }

    #[test]
    fn test_manifest_rejects_unknown_surface_selector() {
        // An unknown surface selector fails closed at deserialization rather
        // than surviving as opaque manifest text. `surfaces` is a top-level
        // manifest field, so it precedes the `[mobpack]` table.
        let err = toml::from_str::<MobpackManifest>(
            r#"
surfaces = ["cli", "ftp"]

[mobpack]
name = "x"
version = "1.0.0"
"#,
        );
        assert!(err.is_err());
    }

    #[test]
    fn test_manifest_rejects_non_skills_profile_skill_path() {
        // A profile skill path outside skills/ fails closed at deserialization.
        let err = toml::from_str::<MobpackManifest>(
            r#"
[mobpack]
name = "x"
version = "1.0.0"

[profiles.lead]
skills = ["config/lead.md"]
"#,
        );
        assert!(err.is_err());
    }

    #[test]
    fn test_requires_section_preserves_tokens_and_stores_typed_capabilities() {
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
        let requires = manifest.requires.as_ref().unwrap();

        // Tokens are retained for round-trip / diagnostics.
        let tokens = requires
            .capabilities
            .iter()
            .map(|cap| cap.token().to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            tokens,
            vec![
                "comms".to_string(),
                "shell".to_string(),
                "mcp_stdio".to_string(),
                "vendor.custom".to_string(),
            ]
        );

        // The typed id is the STORED shape, read directly (not re-parsed).
        let typed = requires.capability_ids().collect::<Vec<_>>();
        assert_eq!(
            typed,
            vec![
                MobpackCapabilityId::Known(CapabilityId::Comms),
                MobpackCapabilityId::Known(CapabilityId::Shell),
                MobpackCapabilityId::HostProcess(HostProcessCapabilityId::McpStdio),
                MobpackCapabilityId::Unknown,
            ]
        );

        // Round-trip preserves the exact tokens.
        let encoded = toml::to_string(&manifest).unwrap();
        let decoded: MobpackManifest = toml::from_str(&encoded).unwrap();
        assert_eq!(decoded, manifest);
    }

    #[test]
    fn test_requires_section_rejects_empty_capability_token() {
        // An empty capability token fails closed at deserialization rather than
        // surviving as an unparseable String.
        let err = toml::from_str::<MobpackManifest>(
            r#"
[mobpack]
name = "x"
version = "1.0.0"

[requires]
capabilities = ["comms", ""]
"#,
        );
        assert!(err.is_err());
    }
}

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobpackManifest {
    pub mobpack: MobpackSection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires: Option<RequiresSection>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub models: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub profiles: BTreeMap<String, ProfileSection>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub surfaces: Vec<String>,
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
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileSection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn test_manifest_toml_roundtrip() {
        let mut models = BTreeMap::new();
        models.insert("planner".to_string(), "gpt-5".to_string());
        models.insert("reviewer".to_string(), "gpt-5-mini".to_string());

        let mut profiles = BTreeMap::new();
        profiles.insert(
            "lead".to_string(),
            ProfileSection {
                model: Some("planner".to_string()),
                skills: vec!["skills/lead.md".to_string()],
            },
        );
        profiles.insert(
            "reviewer".to_string(),
            ProfileSection {
                model: Some("reviewer".to_string()),
                skills: vec!["skills/review.md".to_string()],
            },
        );

        let manifest = MobpackManifest {
            mobpack: MobpackSection {
                name: "code-review".to_string(),
                version: "1.0.0".to_string(),
                description: Some("Review assistant".to_string()),
            },
            requires: Some(RequiresSection {
                capabilities: vec!["comms".to_string(), "shell".to_string()],
            }),
            models,
            profiles,
            surfaces: vec!["cli".to_string(), "rpc".to_string()],
        };

        let encoded = toml::to_string(&manifest).unwrap();
        let decoded: MobpackManifest = toml::from_str(&encoded).unwrap();
        assert_eq!(decoded, manifest);
    }
}

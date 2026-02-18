//! Mob definition types and TOML parsing.
//!
//! A `MobDefinition` describes the complete structure of a mob: profiles,
//! MCP servers, wiring rules, and skill sources. Definitions are serializable
//! so they can be stored in `MobCreated` events for resume recovery.

use crate::ids::{MobId, ProfileName};
use crate::profile::Profile;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Orchestrator configuration within a mob definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrchestratorConfig {
    /// Profile name of the orchestrator.
    pub profile: ProfileName,
}

/// MCP server configuration for a mob.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Stdio command to launch the server (mutually exclusive with `url`).
    #[serde(default)]
    pub command: Option<Vec<String>>,
    /// HTTP/SSE URL for the server (mutually exclusive with `command`).
    #[serde(default)]
    pub url: Option<String>,
    /// Environment variables passed to the server process.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// Source for a skill definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum SkillSource {
    /// Inline skill content.
    Inline {
        /// Skill content text.
        content: String,
    },
    /// Skill loaded from a file path.
    Path {
        /// Path to the skill file.
        path: String,
    },
}

/// Wiring rule between two profile roles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleWiringRule {
    /// First profile name.
    pub a: ProfileName,
    /// Second profile name.
    pub b: ProfileName,
}

/// Wiring rules controlling automatic peer connections.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WiringRules {
    /// Automatically wire every spawned meerkat to the orchestrator.
    #[serde(default)]
    pub auto_wire_orchestrator: bool,
    /// Fan-out wiring rules between profile roles.
    #[serde(default)]
    pub role_wiring: Vec<RoleWiringRule>,
}

/// Complete mob definition.
///
/// Describes profiles, MCP servers, wiring rules, and skill sources.
/// Serializable for storage in `MobCreated` events. `rust_bundles` in
/// `ToolConfig` are stored as string names only; actual dispatchers
/// must be re-registered on resume.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobDefinition {
    /// Unique mob identifier.
    pub id: MobId,
    /// Optional orchestrator configuration.
    #[serde(default)]
    pub orchestrator: Option<OrchestratorConfig>,
    /// Named profiles for spawning meerkats.
    #[serde(default)]
    pub profiles: BTreeMap<ProfileName, Profile>,
    /// Named MCP server configurations.
    #[serde(default)]
    pub mcp_servers: BTreeMap<String, McpServerConfig>,
    /// Wiring rules for automatic peer connections.
    #[serde(default)]
    pub wiring: WiringRules,
    /// Named skill sources.
    #[serde(default)]
    pub skills: BTreeMap<String, SkillSource>,
}

/// Helper struct for TOML deserialization of the `[mob]` section.
#[derive(Deserialize)]
struct TomlMob {
    id: MobId,
    orchestrator: Option<String>,
}

/// Top-level TOML structure for mob definition files.
#[derive(Deserialize)]
struct TomlDefinition {
    mob: TomlMob,
    #[serde(default)]
    profiles: BTreeMap<ProfileName, Profile>,
    #[serde(default)]
    mcp: BTreeMap<String, McpServerConfig>,
    #[serde(default)]
    wiring: WiringRules,
    #[serde(default)]
    skills: BTreeMap<String, SkillSource>,
}

impl MobDefinition {
    /// Parse a mob definition from TOML content.
    pub fn from_toml(content: &str) -> Result<Self, toml::de::Error> {
        let raw: TomlDefinition = toml::from_str(content)?;
        let orchestrator = raw.mob.orchestrator.map(|profile| OrchestratorConfig {
            profile: ProfileName::from(profile),
        });
        Ok(Self {
            id: raw.mob.id,
            orchestrator,
            profiles: raw.profiles,
            mcp_servers: raw.mcp,
            wiring: raw.wiring,
            skills: raw.skills,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::ToolConfig;

    fn example_toml() -> &'static str {
        r#"
[mob]
id = "code-review"
orchestrator = "lead"

[profiles.lead]
model = "claude-opus-4-6"
skills = ["orchestrator-skill"]
peer_description = "Coordinates code review"
external_addressable = true

[profiles.lead.tools]
builtins = true
comms = true
mob = true
mob_tasks = true

[profiles.reviewer]
model = "claude-sonnet-4-5"
skills = ["reviewer-skill"]
peer_description = "Reviews code for quality"

[profiles.reviewer.tools]
builtins = true
shell = true
comms = true
mob_tasks = true
mcp = ["code-server"]

[mcp.code-server]
command = ["npx", "-y", "@mcp/code-server"]

[mcp.docs-server]
url = "https://docs.example.com/mcp"

[wiring]
auto_wire_orchestrator = true

[[wiring.role_wiring]]
a = "reviewer"
b = "reviewer"

[skills.orchestrator-skill]
source = "inline"
content = "You are the lead code reviewer."

[skills.reviewer-skill]
source = "path"
path = "skills/reviewer.md"
"#
    }

    #[test]
    fn test_mob_definition_from_toml() {
        let def = MobDefinition::from_toml(example_toml()).unwrap();
        assert_eq!(def.id.as_str(), "code-review");
        assert_eq!(def.orchestrator.as_ref().unwrap().profile.as_str(), "lead");
        assert_eq!(def.profiles.len(), 2);
        assert!(def.profiles.contains_key(&ProfileName::from("lead")));
        assert!(def.profiles.contains_key(&ProfileName::from("reviewer")));

        let lead = &def.profiles[&ProfileName::from("lead")];
        assert_eq!(lead.model, "claude-opus-4-6");
        assert!(lead.tools.mob);
        assert!(lead.tools.mob_tasks);
        assert!(lead.tools.comms);
        assert!(lead.external_addressable);

        let reviewer = &def.profiles[&ProfileName::from("reviewer")];
        assert_eq!(reviewer.model, "claude-sonnet-4-5");
        assert!(reviewer.tools.shell);
        assert_eq!(reviewer.tools.mcp, vec!["code-server"]);

        assert_eq!(def.mcp_servers.len(), 2);
        assert!(def.mcp_servers.contains_key("code-server"));
        let code_server = &def.mcp_servers["code-server"];
        assert_eq!(
            code_server.command.as_ref().unwrap(),
            &vec![
                "npx".to_string(),
                "-y".to_string(),
                "@mcp/code-server".to_string()
            ]
        );
        let docs_server = &def.mcp_servers["docs-server"];
        assert_eq!(
            docs_server.url.as_ref().unwrap(),
            "https://docs.example.com/mcp"
        );

        assert!(def.wiring.auto_wire_orchestrator);
        assert_eq!(def.wiring.role_wiring.len(), 1);
        assert_eq!(def.wiring.role_wiring[0].a.as_str(), "reviewer");
        assert_eq!(def.wiring.role_wiring[0].b.as_str(), "reviewer");

        assert_eq!(def.skills.len(), 2);
        match &def.skills["orchestrator-skill"] {
            SkillSource::Inline { content } => {
                assert_eq!(content, "You are the lead code reviewer.");
            }
            _ => panic!("expected inline skill"),
        }
        match &def.skills["reviewer-skill"] {
            SkillSource::Path { path } => {
                assert_eq!(path, "skills/reviewer.md");
            }
            _ => panic!("expected path skill"),
        }
    }

    #[test]
    fn test_mob_definition_toml_roundtrip() {
        let def = MobDefinition::from_toml(example_toml()).unwrap();
        // Serialize to JSON (stable format for roundtrip)
        let json = serde_json::to_string(&def).unwrap();
        let parsed: MobDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, def);
    }

    #[test]
    fn test_mob_definition_json_roundtrip() {
        let def = MobDefinition {
            id: MobId::from("test-mob"),
            orchestrator: Some(OrchestratorConfig {
                profile: ProfileName::from("lead"),
            }),
            profiles: {
                let mut m = BTreeMap::new();
                m.insert(
                    ProfileName::from("lead"),
                    Profile {
                        model: "claude-opus-4-6".to_string(),
                        skills: vec!["skill-a".to_string()],
                        tools: ToolConfig::default(),
                        peer_description: "The leader".to_string(),
                        external_addressable: true,
                    },
                );
                m
            },
            mcp_servers: BTreeMap::new(),
            wiring: WiringRules::default(),
            skills: BTreeMap::new(),
        };
        let json = serde_json::to_string_pretty(&def).unwrap();
        let parsed: MobDefinition = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, def);
    }

    #[test]
    fn test_minimal_toml() {
        let toml_str = r#"
[mob]
id = "minimal"
"#;
        let def = MobDefinition::from_toml(toml_str).unwrap();
        assert_eq!(def.id.as_str(), "minimal");
        assert!(def.orchestrator.is_none());
        assert!(def.profiles.is_empty());
        assert!(def.mcp_servers.is_empty());
        assert!(!def.wiring.auto_wire_orchestrator);
        assert!(def.wiring.role_wiring.is_empty());
        assert!(def.skills.is_empty());
    }

    #[test]
    fn test_wiring_rules_serde_roundtrip() {
        let rules = WiringRules {
            auto_wire_orchestrator: true,
            role_wiring: vec![RoleWiringRule {
                a: ProfileName::from("worker"),
                b: ProfileName::from("reviewer"),
            }],
        };
        let json = serde_json::to_string(&rules).unwrap();
        let parsed: WiringRules = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, rules);
    }

    #[test]
    fn test_mcp_server_config_serde() {
        let stdio = McpServerConfig {
            command: Some(vec!["node".to_string(), "server.js".to_string()]),
            url: None,
            env: {
                let mut m = BTreeMap::new();
                m.insert("API_KEY".to_string(), "secret".to_string());
                m
            },
        };
        let json = serde_json::to_string(&stdio).unwrap();
        let parsed: McpServerConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, stdio);
    }

    #[test]
    fn test_skill_source_serde() {
        let inline = SkillSource::Inline {
            content: "You are a helper.".to_string(),
        };
        let json = serde_json::to_string(&inline).unwrap();
        let parsed: SkillSource = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, inline);

        let path = SkillSource::Path {
            path: "skills/helper.md".to_string(),
        };
        let json = serde_json::to_string(&path).unwrap();
        let parsed: SkillSource = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, path);
    }
}

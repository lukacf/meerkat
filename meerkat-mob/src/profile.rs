//! Profile and tool configuration for mob meerkats.
//!
//! A `Profile` defines the template for spawning a meerkat: which model to use,
//! which skills to load, tool configuration, and communication settings.

use serde::{Deserialize, Serialize};

/// Tool configuration for a meerkat profile.
///
/// Controls which tool categories are enabled for meerkats spawned
/// from this profile.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolConfig {
    /// Enable built-in tools (file read, etc.).
    #[serde(default)]
    pub builtins: bool,
    /// Enable shell execution tool.
    #[serde(default)]
    pub shell: bool,
    /// Enable comms tools (peer messaging).
    #[serde(default)]
    pub comms: bool,
    /// Enable memory/semantic search tools.
    #[serde(default)]
    pub memory: bool,
    /// Enable mob management tools (spawn, retire, wire, unwire, list).
    #[serde(default)]
    pub mob: bool,
    /// Enable shared task list tools (create, list, update, get).
    #[serde(default)]
    pub mob_tasks: bool,
    /// MCP server names this profile connects to.
    #[serde(default)]
    pub mcp: Vec<String>,
    /// Named Rust tool bundles wired by the mob runtime.
    ///
    /// String names referencing `Arc<dyn AgentToolDispatcher>` instances
    /// registered at mob construction time. Not serializable — must be
    /// re-registered on resume.
    #[serde(default)]
    pub rust_bundles: Vec<String>,
}

/// Profile template for spawning meerkats.
///
/// Each profile defines the model, skills, tool configuration, and
/// communication properties for a class of meerkat agents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    /// LLM model name (e.g. "claude-opus-4-6").
    pub model: String,
    /// Skill references to load for this profile.
    #[serde(default)]
    pub skills: Vec<String>,
    /// Tool configuration.
    #[serde(default)]
    pub tools: ToolConfig,
    /// Human-readable description of this meerkat's role, visible to peers.
    #[serde(default)]
    pub peer_description: String,
    /// Whether this meerkat can receive turns from external callers.
    #[serde(default)]
    pub external_addressable: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_config_serde_roundtrip() {
        let config = ToolConfig {
            builtins: true,
            shell: false,
            comms: true,
            memory: false,
            mob: true,
            mob_tasks: true,
            mcp: vec!["server-a".to_string(), "server-b".to_string()],
            rust_bundles: vec!["custom-tools".to_string()],
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: ToolConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, config);
    }

    #[test]
    fn test_tool_config_toml_roundtrip() {
        let config = ToolConfig {
            builtins: true,
            shell: true,
            comms: false,
            memory: false,
            mob: false,
            mob_tasks: false,
            mcp: vec!["mcp-server".to_string()],
            rust_bundles: Vec::new(),
        };
        let toml_str = toml::to_string(&config).unwrap();
        let parsed: ToolConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed, config);
    }

    #[test]
    fn test_profile_serde_roundtrip() {
        let profile = Profile {
            model: "claude-opus-4-6".to_string(),
            skills: vec!["orchestrator-skill".to_string()],
            tools: ToolConfig {
                builtins: true,
                shell: false,
                comms: true,
                memory: false,
                mob: true,
                mob_tasks: true,
                mcp: vec![],
                rust_bundles: vec![],
            },
            peer_description: "Orchestrates worker agents".to_string(),
            external_addressable: true,
        };
        let json = serde_json::to_string(&profile).unwrap();
        let parsed: Profile = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, profile);
    }

    #[test]
    fn test_profile_toml_roundtrip() {
        let profile = Profile {
            model: "gpt-5.2".to_string(),
            skills: vec!["worker-skill".to_string()],
            tools: ToolConfig {
                builtins: false,
                shell: true,
                comms: true,
                memory: false,
                mob: false,
                mob_tasks: true,
                mcp: vec!["code-server".to_string()],
                rust_bundles: vec!["custom".to_string()],
            },
            peer_description: "Writes code".to_string(),
            external_addressable: false,
        };
        let toml_str = toml::to_string(&profile).unwrap();
        let parsed: Profile = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed, profile);
    }

    #[test]
    fn test_tool_config_defaults() {
        let config = ToolConfig::default();
        assert!(!config.builtins);
        assert!(!config.shell);
        assert!(!config.comms);
        assert!(!config.memory);
        assert!(!config.mob);
        assert!(!config.mob_tasks);
        assert!(config.mcp.is_empty());
        assert!(config.rust_bundles.is_empty());
    }

    #[test]
    fn test_profile_default_fields_from_toml() {
        let toml_str = r#"
model = "claude-sonnet-4-5"
"#;
        let profile: Profile = toml::from_str(toml_str).unwrap();
        assert_eq!(profile.model, "claude-sonnet-4-5");
        assert!(profile.skills.is_empty());
        assert_eq!(profile.tools, ToolConfig::default());
        assert_eq!(profile.peer_description, "");
        assert!(!profile.external_addressable);
    }
}

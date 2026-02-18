use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub model: String,
    pub skills: Vec<String>,
    pub tools: ToolConfig,
    pub peer_description: String,
    pub external_addressable: bool,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            model: "gpt-5.2".to_string(),
            skills: Vec::new(),
            tools: ToolConfig::default(),
            peer_description: String::new(),
            external_addressable: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolConfig {
    pub builtins: bool,
    pub shell: bool,
    pub comms: bool,
    pub memory: bool,
    pub mob: bool,
    pub mob_tasks: bool,
    pub mcp: Vec<String>,
    pub rust_bundles: Vec<String>,
}

impl Default for ToolConfig {
    fn default() -> Self {
        Self {
            builtins: true,
            shell: false,
            comms: false,
            memory: true,
            mob: false,
            mob_tasks: false,
            mcp: Vec::new(),
            rust_bundles: Vec::new(),
        }
    }
}

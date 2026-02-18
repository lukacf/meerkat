use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ids::{MobId, ProfileName};
use crate::profile::Profile;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MobDefinition {
    pub id: MobId,
    pub orchestrator: Option<OrchestratorConfig>,
    pub profiles: BTreeMap<ProfileName, Profile>,
    pub mcp_servers: BTreeMap<String, McpServerConfig>,
    pub wiring: WiringRules,
    pub skills: BTreeMap<String, SkillSource>,
}

impl MobDefinition {
    pub fn from_toml(content: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(content)
    }

    pub fn as_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }

    pub fn orchestrator_profile(&self) -> Option<&ProfileName> {
        self.orchestrator.as_ref().map(|o| &o.profile)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrchestratorConfig {
    pub profile: ProfileName,
    pub startup_message: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub command: Option<String>,
    pub args: Option<Vec<String>>,
    pub env: Option<BTreeMap<String, String>>,
    pub url: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WiringRules {
    pub auto_wire_orchestrator: bool,
    pub role_wiring: Vec<RolePair>,
}

impl Default for WiringRules {
    fn default() -> Self {
        Self {
            auto_wire_orchestrator: true,
            role_wiring: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RolePair {
    pub a: ProfileName,
    pub b: ProfileName,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SkillSource {
    Inline(String),
    File { file: String },
}

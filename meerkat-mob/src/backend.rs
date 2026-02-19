use serde::{Deserialize, Serialize};

/// Supported mob member provisioning backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MobBackendKind {
    Subagent,
    External,
}

impl MobBackendKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Subagent => "subagent",
            Self::External => "external",
        }
    }
}

impl Default for MobBackendKind {
    fn default() -> Self {
        Self::Subagent
    }
}

use serde::{Deserialize, Serialize};

/// Supported mob member provisioning backends.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MobBackendKind {
    #[default]
    #[serde(rename = "session", alias = "subagent")]
    Session,
    External,
}

impl MobBackendKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::External => "external",
        }
    }
}

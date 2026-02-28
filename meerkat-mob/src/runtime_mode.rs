use serde::{Deserialize, Serialize};

/// Runtime execution mode for a mob member.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MobRuntimeMode {
    /// Long-lived autonomous host loop (default).
    #[default]
    AutonomousHost,
    /// Explicit turn-by-turn execution.
    TurnDriven,
}

impl std::fmt::Display for MobRuntimeMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AutonomousHost => f.write_str("autonomous_host"),
            Self::TurnDriven => f.write_str("turn_driven"),
        }
    }
}

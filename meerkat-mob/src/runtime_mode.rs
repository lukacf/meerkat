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

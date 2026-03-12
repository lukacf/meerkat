//! §20 Out-of-band run control commands.
//!
//! These are delivered to core on a SEPARATE channel from `RunPrimitive`.
//! They interrupt or stop the current run without going through the input queue.

use serde::{Deserialize, Serialize};

/// Out-of-band control commands for the core executor.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum RunControlCommand {
    /// Cancel the currently executing run.
    /// The run should terminate gracefully (cleanup, emit RunCancelled).
    CancelCurrentRun {
        /// Why the run is being cancelled.
        #[serde(default)]
        reason: String,
    },
    /// Stop the runtime executor entirely.
    /// No further runs will be started.
    StopRuntimeExecutor {
        /// Why the executor is being stopped.
        #[serde(default)]
        reason: String,
    },
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn cancel_serde_roundtrip() {
        let cmd = RunControlCommand::CancelCurrentRun {
            reason: "user request".into(),
        };
        let json = serde_json::to_value(&cmd).unwrap();
        assert_eq!(json["command"], "cancel_current_run");
        let parsed: RunControlCommand = serde_json::from_value(json).unwrap();
        assert_eq!(cmd, parsed);
    }

    #[test]
    fn stop_serde_roundtrip() {
        let cmd = RunControlCommand::StopRuntimeExecutor {
            reason: "shutdown".into(),
        };
        let json = serde_json::to_value(&cmd).unwrap();
        assert_eq!(json["command"], "stop_runtime_executor");
        let parsed: RunControlCommand = serde_json::from_value(json).unwrap();
        assert_eq!(cmd, parsed);
    }
}

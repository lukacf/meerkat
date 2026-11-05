//! Shared contract for messages typed while a turn is running.
//!
//! Such a message is *steered* into the running turn: the runner admits it to
//! the session's runtime as a durable `Steer` input. The runtime writes it
//! into the conversation as an ordinary user message at the running turn's
//! next model boundary, or runs it as one follow-up turn when no boundary
//! opens in time; either way it is written exactly once
//! (`docs/reference/session-contracts.mdx`, "Durable steer"). The types here
//! are the only thing the UI and the runner both depend on.

use std::fmt;

/// Identifier of one queued message, allocated by the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PendingId(pub u64);

impl fmt::Display for PendingId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "pending#{}", self.0)
    }
}

/// Why a steered message was not admitted to the runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SteerFailure {
    /// No turn is running; the message should be sent as a normal turn.
    NotRunning,
    /// The runtime refused the input; the text never reached the session.
    Rejected(String),
}

impl fmt::Display for SteerFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRunning => f.write_str("no turn is running"),
            Self::Rejected(reason) => write!(f, "the runtime refused it: {reason}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_display() {
        assert_eq!(SteerFailure::NotRunning.to_string(), "no turn is running");
        assert_eq!(
            SteerFailure::Rejected("closed".into()).to_string(),
            "the runtime refused it: closed"
        );
        assert_eq!(PendingId(3).to_string(), "pending#3");
    }
}

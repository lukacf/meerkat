//! Shared contract for mid-turn message delivery between the UI and the
//! integrator.
//!
//! A message typed while a turn is running is *injected* into the agent's
//! comms inbox and picked up at the next turn boundary. Nothing in the engine
//! reports delivery, so after every turn the integrator reconciles what it
//! injected against the session and reports back with a [`DeliveryReport`].
//! The types here are the only thing both sides depend on.

use std::fmt;

/// Prefix the model sees in front of a message that arrived mid-turn.
/// (The comms runtime adds its own `[EVENT via <source>] ` in front of this.)
pub const FRAME: &str = "User (sent while you were working): ";

/// Prompt used for a continuation turn when an injected message was recorded
/// in the session but the run ended before the model could answer it.
pub const CONTINUATION_PROMPT: &str =
    "(Please respond to my message above that arrived while you were finishing.)";

/// Identifier of one queued message, allocated by the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PendingId(pub u64);

impl fmt::Display for PendingId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "pending#{}", self.0)
    }
}

/// Why an injection could not be performed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InjectFailure {
    /// No turn is running; the message should be sent as a normal prompt.
    NotRunning,
    /// The session has no comms runtime (injection unavailable).
    Unavailable,
    /// The inbox is full.
    Full,
    /// The inbox is closed (agent gone).
    Closed,
}

impl fmt::Display for InjectFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotRunning => "no turn is running",
            Self::Unavailable => "mid-turn delivery is unavailable for this session",
            Self::Full => "the agent's inbox is full",
            Self::Closed => "the agent's inbox is closed",
        })
    }
}

/// What happened to each injected message by the time a turn ended.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DeliveryReport {
    /// Never reached the model; safe to re-send as a prompt.
    pub undelivered: Vec<PendingId>,
    /// Recorded in the session as a user message, but the run ended without an
    /// assistant reply after it.
    pub unanswered: Vec<PendingId>,
    /// Recorded and followed by an assistant message.
    pub answered: Vec<PendingId>,
}

impl DeliveryReport {
    /// Whether nothing was pending.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.undelivered.is_empty() && self.unanswered.is_empty() && self.answered.is_empty()
    }
}

/// Wrap user text in the mid-turn frame.
#[must_use]
pub fn frame(text: &str) -> String {
    format!("{FRAME}{text}")
}

/// Strip the comms event prefix and the mid-turn frame from a recorded user
/// message, returning the original text when the message was an injection.
#[must_use]
pub fn strip_frame(user_text: &str) -> Option<&str> {
    let rest = user_text.strip_prefix("[EVENT via ")?;
    let (_, rest) = rest.split_once("] ")?;
    rest.strip_prefix(FRAME)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn frame_round_trips_through_the_comms_prefix() {
        let framed = frame("fix the typo");
        let recorded = format!("[EVENT via stdin] {framed}");
        assert_eq!(strip_frame(&recorded), Some("fix the typo"));
        assert_eq!(strip_frame("plain user text"), None);
        assert_eq!(strip_frame("[EVENT via rpc] something else"), None);
    }

    #[test]
    fn report_emptiness() {
        assert!(DeliveryReport::default().is_empty());
        let report = DeliveryReport {
            answered: vec![PendingId(1)],
            ..DeliveryReport::default()
        };
        assert!(!report.is_empty());
    }
}

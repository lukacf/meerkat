//! Launch mode and fork context for mob member provisioning.
//!
//! These types define how a member is launched within a mob — fresh, resuming
//! an existing session, or forking from another member's conversation.
//!
//! `ForkContext` and `BudgetSplitPolicy` are mob-native types defined here
//! (not in `meerkat-core`) because fork and budget splitting are mob-level
//! orchestration concepts.

use crate::ids::MeerkatId;
use meerkat_core::types::SessionId;
use serde::{Deserialize, Serialize};

/// How a mob member should be launched.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
#[non_exhaustive]
pub(crate) enum MemberLaunchMode {
    /// Start with a brand-new session (default).
    #[default]
    Fresh,
    /// Resume an existing bridge session binding by ID.
    Resume {
        #[serde(alias = "session_id")]
        bridge_session_id: SessionId,
    },
    /// Fork from another member's conversation history.
    Fork {
        source_member_id: MeerkatId,
        #[serde(default)]
        fork_context: ForkContext,
    },
}

impl MemberLaunchMode {
    pub(crate) fn resume_bridge_session_id(&self) -> Option<&SessionId> {
        match self {
            Self::Resume { bridge_session_id } => Some(bridge_session_id),
            _ => None,
        }
    }
}

/// Controls how much conversation history is included when forking.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ForkContext {
    /// Full conversation history via `Session::fork()` (O(1) CoW).
    #[default]
    FullHistory,
    /// Last N messages from the source session.
    LastMessages { count: u32 },
}

/// How to split the orchestrator's remaining budget among spawned members.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
#[non_exhaustive]
pub enum BudgetSplitPolicy {
    /// Split remaining budget equally among members.
    #[default]
    Equal,
    /// Proportional split (no-op unless weights are implemented).
    Proportional,
    /// Give all remaining budget to the new member.
    Remaining,
    /// Fixed token budget for the new member.
    Fixed(u64),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resume_launch_mode_bridge_session_accessors_stay_additive() {
        let sid = SessionId::new();
        let mode = MemberLaunchMode::Resume {
            bridge_session_id: sid.clone(),
        };

        assert_eq!(mode.resume_bridge_session_id(), Some(&sid));
    }

    #[test]
    fn resume_launch_mode_deserializes_bridge_session_id_alias() {
        let sid = SessionId::new();
        let payload = serde_json::json!({
            "mode": "resume",
            "bridge_session_id": sid,
        });

        let mode: MemberLaunchMode =
            serde_json::from_value(payload).expect("resume launch mode should deserialize");

        assert_eq!(mode.resume_bridge_session_id(), Some(&sid));
    }
}

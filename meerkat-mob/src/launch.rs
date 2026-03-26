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
pub enum MemberLaunchMode {
    /// Start with a brand-new session (default).
    #[default]
    Fresh,
    /// Resume an existing session by ID.
    Resume { session_id: SessionId },
    /// Fork from another member's conversation history.
    Fork {
        source_member_id: MeerkatId,
        #[serde(default)]
        fork_context: ForkContext,
    },
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

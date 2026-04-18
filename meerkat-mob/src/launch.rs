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
///
/// Public spawn-policy enum exposed so that external consumers of
/// [`crate::runtime::SpawnMemberSpec`] can configure session adoption
/// (resume an existing bridge session, fork from a sibling member's
/// history) without reaching into `pub(crate)` internals.
///
/// `Fork::source_member_id` is typed as [`MeerkatId`] to match the
/// legacy DSL identifier carried inside
/// [`crate::mob_machine::MobMachineCommand`] variants. End-user code
/// holding an `AgentIdentity` can convert directly via
/// `MeerkatId::from(&identity)` — see DELETE_ME A5 (identity-first
/// hot-path conversions). Full DSL-schema migration to
/// `AgentIdentity` is tracked separately; until then, the public
/// spawn-policy surface accepts `MeerkatId` for consistency with
/// every other DSL-command-bearing field.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
#[non_exhaustive]
pub enum MemberLaunchMode {
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
    /// If this launch mode resumes an existing bridge session, return
    /// the session id. Returns `None` for `Fresh` and `Fork` modes.
    pub fn resume_bridge_session_id(&self) -> Option<&SessionId> {
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

    /// DELETE_ME A3 + C1 regression: `MemberLaunchMode` and its variants
    /// (Fresh, Resume, Fork) must be reachable from outside the crate
    /// so external consumers can configure session adoption without
    /// reaching into `pub(crate)` internals. This test exercises each
    /// variant at the public API level — if anyone ever re-hides the
    /// enum or its constructors, this fails to compile instead of
    /// breaking downstream crates silently.
    #[test]
    fn member_launch_mode_public_seam_covers_all_variants() {
        use crate::ids::MeerkatId;

        // Fresh is the default.
        let fresh = MemberLaunchMode::default();
        assert!(fresh.resume_bridge_session_id().is_none());

        // Resume carries a SessionId; accessor exposed as pub.
        let sid = SessionId::new();
        let resume = MemberLaunchMode::Resume {
            bridge_session_id: sid.clone(),
        };
        assert_eq!(resume.resume_bridge_session_id(), Some(&sid));

        // Fork carries a source member id (+ fork context). ForkContext
        // variants are both reachable — FullHistory (default) and
        // LastMessages { count }.
        let fork_full = MemberLaunchMode::Fork {
            source_member_id: MeerkatId::from("lead"),
            fork_context: ForkContext::default(),
        };
        assert!(fork_full.resume_bridge_session_id().is_none());

        let fork_last = MemberLaunchMode::Fork {
            source_member_id: MeerkatId::from("lead"),
            fork_context: ForkContext::LastMessages { count: 5 },
        };
        assert!(fork_last.resume_bridge_session_id().is_none());

        // Round-trip serde on all three shapes to catch accidental
        // `#[serde(skip)]` or visibility regressions on constructor
        // fields.
        for mode in [fresh, resume, fork_full, fork_last] {
            let encoded = serde_json::to_value(&mode).expect("mode serializes");
            let decoded: MemberLaunchMode =
                serde_json::from_value(encoded).expect("mode roundtrips");
            assert_eq!(
                mode.resume_bridge_session_id().is_some(),
                decoded.resume_bridge_session_id().is_some(),
            );
        }
    }
}

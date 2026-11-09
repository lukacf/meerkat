//! Wire contract of a host's compaction request (`session/compact`,
//! `mob/compact_member`).
//!
//! The outcome is a transport projection of the runtime's
//! `CompactionRequestReceipt`: MeerkatMachine decides it. A refusal (a
//! session that is not found, not live in this process, retired or stopped)
//! is an error from the existing vocabulary, never an outcome here.

use serde::{Deserialize, Serialize};

/// Parameters for `session/compact`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CompactSessionParams {
    pub session_id: String,
}

/// What a compaction request recorded on the session's registration.
///
/// `requested`: the session's next compaction check compacts, before the
/// next model request of its running turn or of its next run, whatever the
/// compaction thresholds would decide. `already_requested`: a request no
/// check consumed yet was already recorded; nothing changed, and the next
/// check compacts once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireCompactionRequestOutcome {
    Requested,
    AlreadyRequested,
}

/// Result for `session/compact`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CompactSessionResult {
    pub session_id: String,
    pub outcome: WireCompactionRequestOutcome,
}

/// Parameters for `mob/compact_member`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobCompactMemberParams {
    pub mob_id: String,
    pub agent_identity: String,
}

/// Result for `mob/compact_member`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobCompactMemberResult {
    pub mob_id: String,
    pub agent_identity: String,
    pub outcome: WireCompactionRequestOutcome,
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn outcome_round_trips_in_snake_case() {
        let result = CompactSessionResult {
            session_id: "s".into(),
            outcome: WireCompactionRequestOutcome::AlreadyRequested,
        };
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"session_id": "s", "outcome": "already_requested"})
        );
        assert_eq!(
            serde_json::from_value::<CompactSessionResult>(value).unwrap(),
            result
        );
        assert_eq!(
            serde_json::to_value(WireCompactionRequestOutcome::Requested).unwrap(),
            serde_json::json!("requested")
        );
    }

    #[test]
    fn params_refuse_unknown_fields() {
        assert!(
            serde_json::from_value::<CompactSessionParams>(
                serde_json::json!({"session_id": "s", "threshold": 1})
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<MobCompactMemberParams>(
                serde_json::json!({"mob_id": "m", "agent_identity": "a", "extra": true})
            )
            .is_err()
        );
        assert_eq!(
            serde_json::from_value::<MobCompactMemberParams>(
                serde_json::json!({"mob_id": "m", "agent_identity": "a"})
            )
            .unwrap(),
            MobCompactMemberParams {
                mob_id: "m".into(),
                agent_identity: "a".into(),
            }
        );
    }
}

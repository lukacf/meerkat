//! Mob events for structural state changes.
//!
//! Events are the source of truth for mob state. The roster and task board
//! are projections rebuilt by replaying events.

use crate::definition::MobDefinition;
use crate::ids::{MeerkatId, MobId, ProfileName};
use chrono::{DateTime, Utc};
use meerkat_core::types::SessionId;
use serde::{Deserialize, Serialize};

/// A mob event with metadata assigned by the event store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MobEvent {
    /// Monotonically increasing cursor assigned by the store.
    pub cursor: u64,
    /// Timestamp when the event was appended.
    pub timestamp: DateTime<Utc>,
    /// Mob this event belongs to.
    pub mob_id: MobId,
    /// Event payload.
    pub kind: MobEventKind,
}

/// A new event before store assignment (no cursor or timestamp).
#[derive(Debug, Clone)]
pub struct NewMobEvent {
    /// Mob this event belongs to.
    pub mob_id: MobId,
    /// Event payload.
    pub kind: MobEventKind,
}

/// Structural event kinds covering all mob state transitions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MobEventKind {
    /// Mob was created with the given definition.
    MobCreated {
        /// Full mob definition (serializable form, excluding runtime-only state).
        definition: MobDefinition,
    },
    /// Mob reached terminal completed state.
    MobCompleted,
    /// A meerkat was spawned from a profile.
    MeerkatSpawned {
        /// Unique meerkat identifier.
        meerkat_id: MeerkatId,
        /// Profile name used to spawn.
        role: ProfileName,
        /// Session ID created for this meerkat.
        session_id: SessionId,
    },
    /// A meerkat was retired and its session archived.
    MeerkatRetired {
        /// Meerkat that was retired.
        meerkat_id: MeerkatId,
        /// Profile name of the retired meerkat.
        role: ProfileName,
        /// Session ID that was archived.
        session_id: SessionId,
    },
    /// Bidirectional trust was established between two meerkats.
    PeersWired {
        /// First meerkat.
        a: MeerkatId,
        /// Second meerkat.
        b: MeerkatId,
    },
    /// Bidirectional trust was removed between two meerkats.
    PeersUnwired {
        /// First meerkat.
        a: MeerkatId,
        /// Second meerkat.
        b: MeerkatId,
    },
    /// A task was created on the shared task board.
    TaskCreated {
        /// Unique task identifier.
        task_id: String,
        /// Short subject line.
        subject: String,
        /// Detailed description.
        description: String,
        /// Task IDs that must be completed before this task can be claimed.
        blocked_by: Vec<String>,
    },
    /// A task's status or owner was updated.
    TaskUpdated {
        /// Task being updated.
        task_id: String,
        /// New status.
        status: super::tasks::TaskStatus,
        /// New owner (if assigned).
        owner: Option<MeerkatId>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definition::{MobDefinition, WiringRules};
    use crate::ids::MobId;
    use crate::profile::{Profile, ToolConfig};
    use crate::tasks::TaskStatus;
    use std::collections::BTreeMap;
    use uuid::Uuid;

    fn sample_definition() -> MobDefinition {
        MobDefinition {
            id: MobId::from("test-mob"),
            orchestrator: None,
            profiles: {
                let mut m = BTreeMap::new();
                m.insert(
                    ProfileName::from("worker"),
                    Profile {
                        model: "claude-sonnet-4-5".to_string(),
                        skills: vec![],
                        tools: ToolConfig::default(),
                        peer_description: "A worker".to_string(),
                        external_addressable: false,
                    },
                );
                m
            },
            mcp_servers: BTreeMap::new(),
            wiring: WiringRules::default(),
            skills: BTreeMap::new(),
        }
    }

    fn roundtrip(kind: &MobEventKind) {
        let json = serde_json::to_string(kind).unwrap();
        let parsed: MobEventKind = serde_json::from_str(&json).unwrap();
        assert_eq!(&parsed, kind);
    }

    #[test]
    fn test_mob_created_roundtrip() {
        roundtrip(&MobEventKind::MobCreated {
            definition: sample_definition(),
        });
    }

    #[test]
    fn test_mob_completed_roundtrip() {
        roundtrip(&MobEventKind::MobCompleted);
    }

    #[test]
    fn test_meerkat_spawned_roundtrip() {
        roundtrip(&MobEventKind::MeerkatSpawned {
            meerkat_id: MeerkatId::from("agent-1"),
            role: ProfileName::from("worker"),
            session_id: SessionId::from_uuid(Uuid::nil()),
        });
    }

    #[test]
    fn test_meerkat_retired_roundtrip() {
        roundtrip(&MobEventKind::MeerkatRetired {
            meerkat_id: MeerkatId::from("agent-1"),
            role: ProfileName::from("worker"),
            session_id: SessionId::from_uuid(Uuid::nil()),
        });
    }

    #[test]
    fn test_peers_wired_roundtrip() {
        roundtrip(&MobEventKind::PeersWired {
            a: MeerkatId::from("agent-1"),
            b: MeerkatId::from("agent-2"),
        });
    }

    #[test]
    fn test_peers_unwired_roundtrip() {
        roundtrip(&MobEventKind::PeersUnwired {
            a: MeerkatId::from("agent-1"),
            b: MeerkatId::from("agent-2"),
        });
    }

    #[test]
    fn test_task_created_roundtrip() {
        roundtrip(&MobEventKind::TaskCreated {
            task_id: "task-001".to_string(),
            subject: "Implement feature".to_string(),
            description: "Build the widget factory".to_string(),
            blocked_by: vec!["task-000".to_string()],
        });
    }

    #[test]
    fn test_task_updated_roundtrip() {
        roundtrip(&MobEventKind::TaskUpdated {
            task_id: "task-001".to_string(),
            status: TaskStatus::InProgress,
            owner: Some(MeerkatId::from("agent-1")),
        });
    }

    #[test]
    fn test_mob_event_full_roundtrip() {
        let event = MobEvent {
            cursor: 42,
            timestamp: Utc::now(),
            mob_id: MobId::from("test-mob"),
            kind: MobEventKind::MobCompleted,
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: MobEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.cursor, 42);
        assert_eq!(parsed.mob_id.as_str(), "test-mob");
    }
}

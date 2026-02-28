//! Mob events for structural state changes.
//!
//! Events are the source of truth for mob state. The roster and task board
//! are projections rebuilt by replaying events.

use crate::definition::MobDefinition;
use crate::ids::{FlowId, MeerkatId, MobId, ProfileName, RunId, StepId, TaskId};
use crate::runtime_mode::MobRuntimeMode;
use chrono::{DateTime, Utc};
use meerkat_core::event::{AgentEvent, EventEnvelope};
use meerkat_core::types::SessionId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// A mob event with metadata assigned by the event store.
#[derive(Debug, Clone, Serialize)]
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

#[derive(Debug, Clone, Deserialize)]
struct MobEventCanonical {
    pub cursor: u64,
    pub timestamp: DateTime<Utc>,
    pub mob_id: MobId,
    pub kind: MobEventKind,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum MobEventWire {
    Canonical(MobEventCanonical),
    Compat(MobEventCompat),
}

impl<'de> Deserialize<'de> for MobEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = MobEventWire::deserialize(deserializer)?;
        match wire {
            MobEventWire::Canonical(event) => Ok(Self {
                cursor: event.cursor,
                timestamp: event.timestamp,
                mob_id: event.mob_id,
                kind: event.kind,
            }),
            MobEventWire::Compat(event) => Self::try_from(event).map_err(serde::de::Error::custom),
        }
    }
}

/// A new event before store assignment (no cursor).
#[derive(Debug, Clone)]
pub struct NewMobEvent {
    /// Mob this event belongs to.
    pub mob_id: MobId,
    /// Optional timestamp override (primarily for deterministic/backdated tests).
    pub timestamp: Option<DateTime<Utc>>,
    /// Event payload.
    pub kind: MobEventKind,
}

/// Backend-neutral reference to a mob member.
///
/// Canonical serialization is `{"kind":"...", ...}` while deserialization
/// also accepts legacy session-only payloads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MemberRef {
    /// Session-backed member identity (legacy/current runtime form).
    Session {
        /// Session ID for this member.
        session_id: SessionId,
    },
    /// Backend-provided identity and address (future external form).
    BackendPeer {
        /// Backend-unique peer id.
        peer_id: String,
        /// Backend-provided address string.
        address: String,
        /// Optional session id if this member is bridged to a local session.
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<SessionId>,
    },
}

impl MemberRef {
    pub fn from_session_id(session_id: SessionId) -> Self {
        Self::Session { session_id }
    }

    pub fn session_id(&self) -> Option<&SessionId> {
        match self {
            Self::Session { session_id } => Some(session_id),
            Self::BackendPeer { session_id, .. } => session_id.as_ref(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum MemberRefWire {
    Canonical(MemberRefCanonical),
    LegacySessionOnly { session_id: SessionId },
    LegacySessionId(SessionId),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum MemberRefCanonical {
    Session {
        session_id: SessionId,
    },
    BackendPeer {
        peer_id: String,
        address: String,
        #[serde(default)]
        session_id: Option<SessionId>,
    },
}

impl<'de> Deserialize<'de> for MemberRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = MemberRefWire::deserialize(deserializer)?;
        Ok(match wire {
            MemberRefWire::LegacySessionOnly { session_id }
            | MemberRefWire::LegacySessionId(session_id) => Self::Session { session_id },
            MemberRefWire::Canonical(MemberRefCanonical::Session { session_id }) => {
                Self::Session { session_id }
            }
            MemberRefWire::Canonical(MemberRefCanonical::BackendPeer {
                peer_id,
                address,
                session_id,
            }) => Self::BackendPeer {
                peer_id,
                address,
                session_id,
            },
        })
    }
}

/// Compatibility wire model for old/new event payloads.
#[derive(Debug, Clone, Deserialize)]
pub struct MobEventCompat {
    pub cursor: u64,
    pub timestamp: DateTime<Utc>,
    pub mob_id: MobId,
    pub kind: MobEventKindCompat,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MobEventKindCompat {
    MobCreated {
        definition: MobDefinition,
    },
    MobCompleted,
    MobReset,
    MeerkatSpawned {
        meerkat_id: MeerkatId,
        role: ProfileName,
        #[serde(default)]
        runtime_mode: MobRuntimeMode,
        #[serde(default)]
        session_id: Option<SessionId>,
        #[serde(default)]
        member_ref: Option<MemberRef>,
        #[serde(default)]
        labels: BTreeMap<String, String>,
    },
    MeerkatRetired {
        meerkat_id: MeerkatId,
        role: ProfileName,
        #[serde(default)]
        session_id: Option<SessionId>,
        #[serde(default)]
        member_ref: Option<MemberRef>,
    },
    PeersWired {
        a: MeerkatId,
        b: MeerkatId,
    },
    PeersUnwired {
        a: MeerkatId,
        b: MeerkatId,
    },
    TaskCreated {
        task_id: TaskId,
        subject: String,
        description: String,
        blocked_by: Vec<TaskId>,
    },
    TaskUpdated {
        task_id: TaskId,
        status: super::tasks::TaskStatus,
        owner: Option<MeerkatId>,
    },
    FlowStarted {
        run_id: RunId,
        flow_id: FlowId,
        params: serde_json::Value,
    },
    FlowCompleted {
        run_id: RunId,
        flow_id: FlowId,
    },
    FlowFailed {
        run_id: RunId,
        flow_id: FlowId,
        reason: String,
    },
    FlowCanceled {
        run_id: RunId,
        flow_id: FlowId,
    },
    StepDispatched {
        run_id: RunId,
        step_id: StepId,
        meerkat_id: MeerkatId,
    },
    StepTargetCompleted {
        run_id: RunId,
        step_id: StepId,
        meerkat_id: MeerkatId,
    },
    StepTargetFailed {
        run_id: RunId,
        step_id: StepId,
        meerkat_id: MeerkatId,
        reason: String,
    },
    StepCompleted {
        run_id: RunId,
        step_id: StepId,
    },
    StepFailed {
        run_id: RunId,
        step_id: StepId,
        reason: String,
    },
    StepSkipped {
        run_id: RunId,
        step_id: StepId,
        reason: String,
    },
    TopologyViolation {
        from_role: ProfileName,
        to_role: ProfileName,
    },
    SupervisorEscalation {
        run_id: RunId,
        step_id: StepId,
        escalated_to: MeerkatId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MobEventCompatError {
    MissingMemberRef {
        event_kind: &'static str,
        meerkat_id: MeerkatId,
    },
}

impl fmt::Display for MobEventCompatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingMemberRef {
                event_kind,
                meerkat_id,
            } => write!(
                f,
                "{event_kind} for '{meerkat_id}' is missing a member reference"
            ),
        }
    }
}

impl std::error::Error for MobEventCompatError {}

fn upcast_member_ref(
    event_kind: &'static str,
    meerkat_id: &MeerkatId,
    member_ref: Option<MemberRef>,
    session_id: Option<SessionId>,
) -> Result<MemberRef, MobEventCompatError> {
    if let Some(member_ref) = member_ref {
        return Ok(member_ref);
    }

    if let Some(session_id) = session_id {
        return Ok(MemberRef::from_session_id(session_id));
    }

    Err(MobEventCompatError::MissingMemberRef {
        event_kind,
        meerkat_id: meerkat_id.clone(),
    })
}

impl TryFrom<MobEventCompat> for MobEvent {
    type Error = MobEventCompatError;

    fn try_from(value: MobEventCompat) -> Result<Self, Self::Error> {
        let kind = match value.kind {
            MobEventKindCompat::MobCreated { definition } => {
                MobEventKind::MobCreated { definition }
            }
            MobEventKindCompat::MobCompleted => MobEventKind::MobCompleted,
            MobEventKindCompat::MobReset => MobEventKind::MobReset,
            MobEventKindCompat::MeerkatSpawned {
                meerkat_id,
                role,
                runtime_mode,
                session_id,
                member_ref,
                labels,
            } => MobEventKind::MeerkatSpawned {
                member_ref: upcast_member_ref(
                    "meerkat_spawned",
                    &meerkat_id,
                    member_ref,
                    session_id,
                )?,
                meerkat_id,
                role,
                runtime_mode,
                labels,
            },
            MobEventKindCompat::MeerkatRetired {
                meerkat_id,
                role,
                session_id,
                member_ref,
            } => MobEventKind::MeerkatRetired {
                member_ref: upcast_member_ref(
                    "meerkat_retired",
                    &meerkat_id,
                    member_ref,
                    session_id,
                )?,
                meerkat_id,
                role,
            },
            MobEventKindCompat::PeersWired { a, b } => MobEventKind::PeersWired { a, b },
            MobEventKindCompat::PeersUnwired { a, b } => MobEventKind::PeersUnwired { a, b },
            MobEventKindCompat::TaskCreated {
                task_id,
                subject,
                description,
                blocked_by,
            } => MobEventKind::TaskCreated {
                task_id,
                subject,
                description,
                blocked_by,
            },
            MobEventKindCompat::TaskUpdated {
                task_id,
                status,
                owner,
            } => MobEventKind::TaskUpdated {
                task_id,
                status,
                owner,
            },
            MobEventKindCompat::FlowStarted {
                run_id,
                flow_id,
                params,
            } => MobEventKind::FlowStarted {
                run_id,
                flow_id,
                params,
            },
            MobEventKindCompat::FlowCompleted { run_id, flow_id } => {
                MobEventKind::FlowCompleted { run_id, flow_id }
            }
            MobEventKindCompat::FlowFailed {
                run_id,
                flow_id,
                reason,
            } => MobEventKind::FlowFailed {
                run_id,
                flow_id,
                reason,
            },
            MobEventKindCompat::FlowCanceled { run_id, flow_id } => {
                MobEventKind::FlowCanceled { run_id, flow_id }
            }
            MobEventKindCompat::StepDispatched {
                run_id,
                step_id,
                meerkat_id,
            } => MobEventKind::StepDispatched {
                run_id,
                step_id,
                meerkat_id,
            },
            MobEventKindCompat::StepTargetCompleted {
                run_id,
                step_id,
                meerkat_id,
            } => MobEventKind::StepTargetCompleted {
                run_id,
                step_id,
                meerkat_id,
            },
            MobEventKindCompat::StepTargetFailed {
                run_id,
                step_id,
                meerkat_id,
                reason,
            } => MobEventKind::StepTargetFailed {
                run_id,
                step_id,
                meerkat_id,
                reason,
            },
            MobEventKindCompat::StepCompleted { run_id, step_id } => {
                MobEventKind::StepCompleted { run_id, step_id }
            }
            MobEventKindCompat::StepFailed {
                run_id,
                step_id,
                reason,
            } => MobEventKind::StepFailed {
                run_id,
                step_id,
                reason,
            },
            MobEventKindCompat::StepSkipped {
                run_id,
                step_id,
                reason,
            } => MobEventKind::StepSkipped {
                run_id,
                step_id,
                reason,
            },
            MobEventKindCompat::TopologyViolation { from_role, to_role } => {
                MobEventKind::TopologyViolation { from_role, to_role }
            }
            MobEventKindCompat::SupervisorEscalation {
                run_id,
                step_id,
                escalated_to,
            } => MobEventKind::SupervisorEscalation {
                run_id,
                step_id,
                escalated_to,
            },
        };

        Ok(Self {
            cursor: value.cursor,
            timestamp: value.timestamp,
            mob_id: value.mob_id,
            kind,
        })
    }
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
    /// Mob was reset to initial running state (all members retired, events cleared).
    MobReset,
    /// A meerkat was spawned from a profile.
    MeerkatSpawned {
        /// Unique meerkat identifier.
        meerkat_id: MeerkatId,
        /// Profile name used to spawn.
        role: ProfileName,
        /// Runtime mode for this spawned member.
        #[serde(default)]
        runtime_mode: MobRuntimeMode,
        /// Backend-neutral member identity created for this meerkat.
        member_ref: MemberRef,
        /// Application-defined labels for this member.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        labels: BTreeMap<String, String>,
    },
    /// A meerkat was retired and its session archived.
    MeerkatRetired {
        /// Meerkat that was retired.
        meerkat_id: MeerkatId,
        /// Profile name of the retired meerkat.
        role: ProfileName,
        /// Backend-neutral member identity that was retired.
        member_ref: MemberRef,
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
        task_id: TaskId,
        /// Short subject line.
        subject: String,
        /// Detailed description.
        description: String,
        /// Task IDs that must be completed before this task can be claimed.
        blocked_by: Vec<TaskId>,
    },
    /// A task's status or owner was updated.
    TaskUpdated {
        /// Task being updated.
        task_id: TaskId,
        /// New status.
        status: super::tasks::TaskStatus,
        /// New owner (if assigned).
        owner: Option<MeerkatId>,
    },
    /// Flow run started.
    FlowStarted {
        run_id: RunId,
        flow_id: FlowId,
        params: serde_json::Value,
    },
    /// Flow run completed.
    FlowCompleted { run_id: RunId, flow_id: FlowId },
    /// Flow run failed.
    FlowFailed {
        run_id: RunId,
        flow_id: FlowId,
        reason: String,
    },
    /// Flow run canceled.
    FlowCanceled { run_id: RunId, flow_id: FlowId },
    /// Per-target step dispatch event.
    StepDispatched {
        run_id: RunId,
        step_id: StepId,
        meerkat_id: MeerkatId,
    },
    /// Per-target successful completion event.
    StepTargetCompleted {
        run_id: RunId,
        step_id: StepId,
        meerkat_id: MeerkatId,
    },
    /// Per-target failure event.
    StepTargetFailed {
        run_id: RunId,
        step_id: StepId,
        meerkat_id: MeerkatId,
        reason: String,
    },
    /// Aggregate step completion event.
    StepCompleted { run_id: RunId, step_id: StepId },
    /// Aggregate step failure event.
    StepFailed {
        run_id: RunId,
        step_id: StepId,
        reason: String,
    },
    /// Step skipped event.
    StepSkipped {
        run_id: RunId,
        step_id: StepId,
        reason: String,
    },
    /// Topology violation event.
    TopologyViolation {
        from_role: ProfileName,
        to_role: ProfileName,
    },
    /// Supervisor escalation event.
    SupervisorEscalation {
        run_id: RunId,
        step_id: StepId,
        escalated_to: MeerkatId,
    },
}

/// An agent event attributed to a specific mob member.
///
/// Wraps the full [`EventEnvelope<AgentEvent>`] to preserve event metadata
/// (`event_id`, `source_id`, `seq`, `mob_id`, `timestamp_ms`) without
/// information loss. Used by the mob event bus to tag session-level events
/// with mob-level attribution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttributedEvent {
    /// The meerkat that produced this event.
    pub source: MeerkatId,
    /// Profile name of the source meerkat.
    pub profile: ProfileName,
    /// The original enveloped agent event from the session stream.
    pub envelope: EventEnvelope<AgentEvent>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definition::{BackendConfig, MobDefinition, WiringRules};
    use crate::ids::MobId;
    use crate::profile::{Profile, ToolConfig};
    use crate::roster::Roster;
    use crate::tasks::TaskStatus;
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::collections::BTreeSet;
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
                        backend: None,
                        runtime_mode: MobRuntimeMode::AutonomousHost,
                        max_inline_peer_notifications: None,
                        output_schema: None,
                    },
                );
                m
            },
            mcp_servers: BTreeMap::new(),
            wiring: WiringRules::default(),
            skills: BTreeMap::new(),
            backend: BackendConfig::default(),
            flows: BTreeMap::new(),
            topology: None,
            supervisor: None,
            limits: None,
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
    fn test_mob_reset_roundtrip() {
        roundtrip(&MobEventKind::MobReset);
    }

    #[test]
    fn test_meerkat_spawned_roundtrip() {
        roundtrip(&MobEventKind::MeerkatSpawned {
            meerkat_id: MeerkatId::from("agent-1"),
            role: ProfileName::from("worker"),
            runtime_mode: MobRuntimeMode::AutonomousHost,
            member_ref: MemberRef::from_session_id(SessionId::from_uuid(Uuid::nil())),
            labels: BTreeMap::new(),
        });
    }

    #[test]
    fn test_legacy_meerkat_spawned_defaults_runtime_mode() {
        let event: MobEvent = serde_json::from_value(json!({
            "cursor": 1,
            "timestamp": "2026-02-19T00:00:00Z",
            "mob_id": "test-mob",
            "kind": {
                "type": "meerkat_spawned",
                "meerkat_id": "a",
                "role": "worker",
                "session_id": SessionId::from_uuid(Uuid::nil()),
            },
        }))
        .expect("legacy event should parse");
        match event.kind {
            MobEventKind::MeerkatSpawned { runtime_mode, .. } => {
                assert_eq!(runtime_mode, MobRuntimeMode::AutonomousHost);
            }
            other => panic!("expected MeerkatSpawned, got {other:?}"),
        }
    }

    #[test]
    fn test_meerkat_retired_roundtrip() {
        roundtrip(&MobEventKind::MeerkatRetired {
            meerkat_id: MeerkatId::from("agent-1"),
            role: ProfileName::from("worker"),
            member_ref: MemberRef::from_session_id(SessionId::from_uuid(Uuid::nil())),
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
            task_id: TaskId::from("task-001"),
            subject: "Implement feature".to_string(),
            description: "Build the widget factory".to_string(),
            blocked_by: vec![TaskId::from("task-000")],
        });
    }

    #[test]
    fn test_task_updated_roundtrip() {
        roundtrip(&MobEventKind::TaskUpdated {
            task_id: TaskId::from("task-001"),
            status: TaskStatus::InProgress,
            owner: Some(MeerkatId::from("agent-1")),
        });
    }

    #[test]
    fn test_flow_variants_roundtrip() {
        let run_id = RunId::new();
        let flow_id = FlowId::from("flow-a");
        let step_id = StepId::from("step-a");
        let meerkat_id = MeerkatId::from("worker-1");

        roundtrip(&MobEventKind::FlowStarted {
            run_id: run_id.clone(),
            flow_id: flow_id.clone(),
            params: serde_json::json!({"k":"v"}),
        });
        roundtrip(&MobEventKind::FlowCompleted {
            run_id: run_id.clone(),
            flow_id: flow_id.clone(),
        });
        roundtrip(&MobEventKind::FlowFailed {
            run_id: run_id.clone(),
            flow_id: flow_id.clone(),
            reason: "boom".to_string(),
        });
        roundtrip(&MobEventKind::FlowCanceled {
            run_id: run_id.clone(),
            flow_id: flow_id.clone(),
        });
        roundtrip(&MobEventKind::StepDispatched {
            run_id: run_id.clone(),
            step_id: step_id.clone(),
            meerkat_id: meerkat_id.clone(),
        });
        roundtrip(&MobEventKind::StepTargetCompleted {
            run_id: run_id.clone(),
            step_id: step_id.clone(),
            meerkat_id: meerkat_id.clone(),
        });
        roundtrip(&MobEventKind::StepTargetFailed {
            run_id: run_id.clone(),
            step_id: step_id.clone(),
            meerkat_id: meerkat_id.clone(),
            reason: "fail".to_string(),
        });
        roundtrip(&MobEventKind::StepCompleted {
            run_id: run_id.clone(),
            step_id: step_id.clone(),
        });
        roundtrip(&MobEventKind::StepFailed {
            run_id: run_id.clone(),
            step_id: step_id.clone(),
            reason: "timeout after 1000ms".to_string(),
        });
        roundtrip(&MobEventKind::StepSkipped {
            run_id: run_id.clone(),
            step_id: step_id.clone(),
            reason: "branch lost".to_string(),
        });
        roundtrip(&MobEventKind::TopologyViolation {
            from_role: ProfileName::from("lead"),
            to_role: ProfileName::from("worker"),
        });
        roundtrip(&MobEventKind::SupervisorEscalation {
            run_id,
            step_id,
            escalated_to: meerkat_id,
        });
    }

    #[test]
    fn test_event_compat_mapping_for_new_variants() {
        let run_id = RunId::new();
        let flow_id = FlowId::from("flow-a");
        let compat = MobEventCompat {
            cursor: 7,
            timestamp: Utc::now(),
            mob_id: MobId::from("mob"),
            kind: MobEventKindCompat::FlowStarted {
                run_id: run_id.clone(),
                flow_id: flow_id.clone(),
                params: serde_json::json!({"x":1}),
            },
        };

        let canonical = MobEvent::try_from(compat).expect("compat mapping");
        assert_eq!(canonical.cursor, 7);
        assert_eq!(
            canonical.kind,
            MobEventKind::FlowStarted {
                run_id,
                flow_id,
                params: serde_json::json!({"x":1}),
            }
        );
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

    #[test]
    fn test_member_ref_legacy_session_only_deserializes_to_compat_representation() {
        let sid = SessionId::from_uuid(Uuid::nil());
        let parsed: MemberRef = serde_json::from_value(json!({
            "session_id": sid,
        }))
        .unwrap();

        assert_eq!(parsed, MemberRef::from_session_id(sid));
    }

    #[test]
    fn test_member_ref_serializes_deterministically() {
        let sid = SessionId::from_uuid(Uuid::nil());
        let member_ref = MemberRef::from_session_id(sid);

        let first = serde_json::to_string(&member_ref).unwrap();
        let second = serde_json::to_string(&member_ref).unwrap();
        assert_eq!(first, second);
        assert_eq!(
            first,
            r#"{"kind":"session","session_id":"00000000-0000-0000-0000-000000000000"}"#
        );
    }

    #[test]
    fn test_member_ref_roundtrip_backend_peer() {
        let member_ref = MemberRef::BackendPeer {
            peer_id: "peer-123".to_string(),
            address: "https://backend.example/peers/peer-123".to_string(),
            session_id: Some(SessionId::from_uuid(Uuid::nil())),
        };

        let json = serde_json::to_string(&member_ref).unwrap();
        let parsed: MemberRef = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, member_ref);
    }

    fn parse_compat_events(events: serde_json::Value) -> Vec<MobEvent> {
        let compat: Vec<MobEventCompat> = serde_json::from_value(events).unwrap();
        compat
            .into_iter()
            .map(|event| MobEvent::try_from(event).unwrap())
            .collect()
    }

    fn roster_snapshot(roster: &Roster) -> BTreeMap<String, (String, String, BTreeSet<String>)> {
        roster
            .list()
            .map(|entry| {
                (
                    entry.meerkat_id.as_str().to_string(),
                    (
                        entry.profile.as_str().to_string(),
                        entry
                            .session_id()
                            .expect("session-backed member ref")
                            .to_string(),
                        entry
                            .wired_to
                            .iter()
                            .map(|peer| peer.as_str().to_string())
                            .collect(),
                    ),
                )
            })
            .collect()
    }

    #[test]
    fn test_event_compat_upcast_replay_legacy_and_new_member_refs_match_projection() {
        let sid_a = SessionId::from_uuid(Uuid::nil());
        let sid_b = SessionId::from_uuid(Uuid::from_u128(1));

        let legacy = parse_compat_events(json!([
            {
                "cursor": 1,
                "timestamp": "2026-02-19T00:00:00Z",
                "mob_id": "test-mob",
                "kind": {
                    "type": "meerkat_spawned",
                    "meerkat_id": "a",
                    "role": "worker",
                    "session_id": sid_a,
                },
            },
            {
                "cursor": 2,
                "timestamp": "2026-02-19T00:00:01Z",
                "mob_id": "test-mob",
                "kind": {
                    "type": "meerkat_spawned",
                    "meerkat_id": "b",
                    "role": "worker",
                    "session_id": sid_b,
                },
            },
            {
                "cursor": 3,
                "timestamp": "2026-02-19T00:00:02Z",
                "mob_id": "test-mob",
                "kind": {
                    "type": "peers_wired",
                    "a": "a",
                    "b": "b",
                },
            },
        ]));

        let new_form = parse_compat_events(json!([
            {
                "cursor": 1,
                "timestamp": "2026-02-19T00:00:00Z",
                "mob_id": "test-mob",
                "kind": {
                    "type": "meerkat_spawned",
                    "meerkat_id": "a",
                    "role": "worker",
                    "member_ref": {
                        "kind": "session",
                        "session_id": sid_a,
                    },
                },
            },
            {
                "cursor": 2,
                "timestamp": "2026-02-19T00:00:01Z",
                "mob_id": "test-mob",
                "kind": {
                    "type": "meerkat_spawned",
                    "meerkat_id": "b",
                    "role": "worker",
                    "member_ref": {
                        "kind": "session",
                        "session_id": sid_b,
                    },
                },
            },
            {
                "cursor": 3,
                "timestamp": "2026-02-19T00:00:02Z",
                "mob_id": "test-mob",
                "kind": {
                    "type": "peers_wired",
                    "a": "a",
                    "b": "b",
                },
            },
        ]));

        let legacy_roster = Roster::project(&legacy);
        let new_roster = Roster::project(&new_form);
        assert_eq!(
            roster_snapshot(&legacy_roster),
            roster_snapshot(&new_roster)
        );
    }
}

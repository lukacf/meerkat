//! Mob events for structural state changes.
//!
//! Events are the source of truth for mob state. The roster and task board
//! are projections rebuilt by replaying events.

use crate::definition::MobDefinition;
use crate::ids::{FlowId, MeerkatId, MobId, ProfileName, RunId, StepId, TaskId};
use crate::runtime_mode::MobRuntimeMode;
use chrono::{DateTime, Utc};
use meerkat_core::comms::TrustedPeerSpec;
use meerkat_core::event::{AgentEvent, EventEnvelope};
use meerkat_core::types::SessionId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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

impl<'de> Deserialize<'de> for MobEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let canonical = MobEventCanonical::deserialize(deserializer)?;
        Ok(Self {
            cursor: canonical.cursor,
            timestamp: canonical.timestamp,
            mob_id: canonical.mob_id,
            kind: canonical.kind,
        })
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

/// Structural event kinds covering all mob state transitions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MobEventKind {
    /// Mob was created with the given definition.
    MobCreated {
        /// Full mob definition (serializable form, excluding runtime-only state).
        definition: Box<MobDefinition>,
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
    /// Trust was established from a local member to an external peer.
    ExternalPeerWired {
        /// Local meerkat that trusts the external peer.
        local: MeerkatId,
        /// Full trusted-peer specification for replay/respawn restore.
        spec: TrustedPeerSpec,
    },
    /// Trust was removed from a local member to an external peer.
    ExternalPeerUnwired {
        /// Local meerkat removing trust.
        local: MeerkatId,
        /// External peer name that was removed from the local projection.
        peer_name: MeerkatId,
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
    use crate::tasks::TaskStatus;
    use serde_json::json;
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
                        backend: None,
                        runtime_mode: MobRuntimeMode::AutonomousHost,
                        max_inline_peer_notifications: None,
                        output_schema: None,
                        provider_params: None,
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
            spawn_policy: None,
            event_router: None,
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
            definition: Box::new(sample_definition()),
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
    fn test_meerkat_spawned_defaults_runtime_mode() {
        let event: MobEvent = serde_json::from_value(json!({
            "cursor": 1,
            "timestamp": "2026-02-19T00:00:00Z",
            "mob_id": "test-mob",
            "kind": {
                "type": "meerkat_spawned",
                "meerkat_id": "a",
                "role": "worker",
                "member_ref": {
                    "kind": "session",
                    "session_id": SessionId::from_uuid(Uuid::nil()),
                },
            },
        }))
        .expect("event without runtime_mode should parse");
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
    fn test_external_peer_wired_roundtrip() {
        roundtrip(&MobEventKind::ExternalPeerWired {
            local: MeerkatId::from("agent-1"),
            spec: TrustedPeerSpec::new(
                "remote-mob/worker/agent-2",
                "ed25519:remote-agent-2",
                "inproc://remote-mob/worker/agent-2",
            )
            .expect("valid trusted peer spec"),
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
    fn test_external_peer_unwired_roundtrip() {
        roundtrip(&MobEventKind::ExternalPeerUnwired {
            local: MeerkatId::from("agent-1"),
            peer_name: MeerkatId::from("remote-mob/worker/agent-2"),
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
}

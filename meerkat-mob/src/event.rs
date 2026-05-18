//! Mob events for structural state changes.
//!
//! Events are the source of truth for mob state. The roster is rebuilt by
//! replaying events.

use crate::definition::MobDefinition;
use crate::ids::{
    AgentIdentity, AgentRuntimeId, FenceToken, FlowId, Generation, MobId, ProfileName, RunId,
    StepId,
};
use crate::roster::MobMemberKickoffSnapshot;
use crate::runtime_mode::MobRuntimeMode;
use chrono::{DateTime, Utc};
use meerkat_contracts::wire::supervisor_bridge::BridgeBootstrapToken;
use meerkat_core::comms::{PeerName, TrustedPeerDescriptor};
use meerkat_core::event::{AgentEvent, EventEnvelope};
use meerkat_core::service::{MobToolCallerProvenance, OpaquePrincipalToken};
use meerkat_core::types::SessionId;
use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize};
#[cfg(not(target_arch = "wasm32"))]
use serde_json::Value;
use std::collections::BTreeMap;
#[cfg(not(target_arch = "wasm32"))]
use std::io::{Error as IoError, ErrorKind as IoErrorKind};

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
/// Legacy bridge-level identity retained for internal dispatch. Not part of
/// the public 0.6 mob contract — use [`AgentIdentity`] and [`AgentRuntimeId`]
/// for all public surfaces.
///
/// Finding A7: this enum was previously `#[doc(hidden)] pub` with only
/// `pub(crate)` constructors, making it reachable in the crate's public
/// namespace but effectively useless externally. Making it `pub(crate)` both
/// enforces that (no external reachability) and matches the reality that no
/// public `MobHandle` method uses it as a parameter or return type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MemberRef {
    /// Session-backed member identity for the current bridge binding.
    Session {
        /// Compatibility carrier for the canonical bridge session ID.
        session_id: SessionId,
    },
    /// Backend-provided identity and address (future external form).
    BackendPeer {
        /// Backend-unique peer id.
        peer_id: String,
        /// Backend-provided address string.
        address: String,
        /// Ed25519 signing pubkey bytes for the backend peer. This is the
        /// trust subject for comms admission; `peer_id` must derive from it.
        pubkey: Option<[u8; 32]>,
        /// Optional bootstrap proof for re-establishing supervisor control.
        /// Not serialized on the wire (intentionally elided from `Serialize`
        /// below); kept in memory as a redacting newtype so it does not leak
        /// through `Debug` or `tracing` fields.
        bootstrap_token: Option<BridgeBootstrapToken>,
        /// Optional bridge session binding when this member is bridged to a
        /// local session. Serialized additively as both `session_id` and
        /// `bridge_session_id` for compatibility.
        session_id: Option<SessionId>,
    },
}

impl MemberRef {
    pub(crate) fn from_bridge_session_id(session_id: SessionId) -> Self {
        Self::Session { session_id }
    }

    pub(crate) fn bridge_session_id(&self) -> Option<&SessionId> {
        match self {
            Self::Session { session_id } => Some(session_id),
            Self::BackendPeer { session_id, .. } => session_id.as_ref(),
        }
    }
}

impl Serialize for MemberRef {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Session { session_id } => {
                let mut map = serializer.serialize_map(Some(3))?;
                map.serialize_entry("kind", "session")?;
                map.serialize_entry("session_id", session_id)?;
                map.serialize_entry("bridge_session_id", session_id)?;
                map.end()
            }
            Self::BackendPeer {
                peer_id,
                address,
                pubkey,
                session_id,
                ..
            } => {
                let mut map = serializer.serialize_map(None)?;
                map.serialize_entry("kind", "backend_peer")?;
                map.serialize_entry("peer_id", peer_id)?;
                map.serialize_entry("address", address)?;
                if let Some(pubkey) = pubkey {
                    map.serialize_entry("pubkey", pubkey)?;
                }
                if let Some(session_id) = session_id {
                    map.serialize_entry("session_id", session_id)?;
                    map.serialize_entry("bridge_session_id", session_id)?;
                }
                map.end()
            }
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum MemberRefWire {
    Canonical(MemberRefCanonical),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum MemberRefCanonical {
    Session {
        #[serde(default)]
        session_id: Option<SessionId>,
        #[serde(default)]
        bridge_session_id: Option<SessionId>,
    },
    BackendPeer {
        peer_id: String,
        address: String,
        #[serde(default)]
        pubkey: Option<[u8; 32]>,
        #[serde(default)]
        bootstrap_token: Option<BridgeBootstrapToken>,
        #[serde(default)]
        session_id: Option<SessionId>,
        #[serde(default)]
        bridge_session_id: Option<SessionId>,
    },
}

impl<'de> Deserialize<'de> for MemberRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = MemberRefWire::deserialize(deserializer)?;
        Ok(match wire {
            MemberRefWire::Canonical(MemberRefCanonical::Session {
                session_id,
                bridge_session_id,
            }) => Self::Session {
                session_id: bridge_session_id.or(session_id).ok_or_else(|| {
                    serde::de::Error::custom(
                        "session member_ref requires bridge_session_id or session_id",
                    )
                })?,
            },
            MemberRefWire::Canonical(MemberRefCanonical::BackendPeer {
                peer_id,
                address,
                pubkey,
                bootstrap_token,
                session_id,
                bridge_session_id,
            }) => Self::BackendPeer {
                peer_id,
                address,
                pubkey,
                bootstrap_token,
                session_id: bridge_session_id.or(session_id),
            },
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
const CURRENT_STORED_MOB_EVENT_SCHEMA_VERSION: u32 = 6;

#[cfg(not(target_arch = "wasm32"))]
fn stored_mob_event_format_error(message: impl Into<String>) -> serde_json::Error {
    serde_json::Error::io(IoError::new(IoErrorKind::InvalidData, message.into()))
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
    /// Mob destroy was admitted and is retaining authority until cleanup finishes.
    MobDestroying,
    /// Destroy finished member/runtime cleanup and is finalizing durable storage.
    ///
    /// This marker is a crash-recovery fence: if runtime metadata was scrubbed
    /// but the event log was not yet cleared, resume must not recreate
    /// supervisor authority as though the mob were still live.
    MobDestroyStorageFinalizing,
    /// Mob was reset to initial running state (all members retired, events cleared).
    MobReset,
    // ---------------------------------------------------------------
    // Identity-native lifecycle events (0.6)
    // ---------------------------------------------------------------
    /// A member was spawned with identity-native metadata.
    ///
    /// Replaces `MeerkatSpawned` in the public contract. Carries
    /// [`AgentIdentity`], [`Generation`], [`FenceToken`], and
    /// [`AgentRuntimeId`] instead of [`MeerkatId`] / [`MemberRef`].
    ///
    /// Unlike the other `Member*` variants which use inline struct fields,
    /// this variant wraps a named [`MemberSpawnedEvent`] struct. The reason
    /// is load-bearing and not cosmetic: [`MemberSpawnedEvent`] carries a
    /// `#[serde(skip)] pub(crate) bridge_member_ref: Option<MemberRef>`
    /// field used by in-crate event replay (see
    /// [`encode_stored_mob_event`] / [`decode_stored_mob_event`]) that is
    /// deliberately omitted from the public wire shape. A named struct
    /// keeps the internal replay pointer, its `#[serde(skip)]` attribute,
    /// and the constructor/`with_*` helpers self-contained. Inline variant
    /// fields would force the replay plumbing into this enum and leak
    /// "shell owns mechanics, not meaning" internal state into the public
    /// event contract. Finding A6 (DELETE_ME) flagged the shape difference;
    /// the difference is intentional and regression-pinned by
    /// `member_spawned_public_wire_shape_excludes_bridge_member_ref`.
    MemberSpawned(MemberSpawnedEvent),
    /// A member was retired.
    ///
    /// Identity-native replacement for `MeerkatRetired`.
    MemberRetired {
        /// Stable member identity.
        agent_identity: AgentIdentity,
        /// Generation at time of retirement.
        generation: Generation,
        /// Profile name of the retired member.
        role: ProfileName,
    },
    /// A member was reset to a new generation.
    ///
    /// Preserves the [`AgentIdentity`] but advances the [`Generation`]
    /// counter and issues a new [`FenceToken`] and [`AgentRuntimeId`].
    MemberReset {
        /// Stable member identity (unchanged across reset).
        agent_identity: AgentIdentity,
        /// Previous generation before reset.
        previous_generation: Generation,
        /// New generation after reset.
        new_generation: Generation,
        /// New fence token for the reset incarnation.
        fence_token: FenceToken,
        /// New composite runtime id.
        agent_runtime_id: AgentRuntimeId,
    },

    /// Kickoff state for an existing member changed.
    MemberKickoffUpdated {
        /// Member whose kickoff state changed.
        member: AgentIdentity,
        /// Current kickoff snapshot.
        kickoff: MobMemberKickoffSnapshot,
    },
    /// Bidirectional wiring edge established between two local members.
    ///
    /// DSL-emit-driven observability: the shell records this variant on
    /// successful `MobMachineInput::WireMembers` acceptance, mirroring the
    /// DSL's `EmitWiringLifecycleNotice { kind: Wired, edge }` effect. The
    /// DSL authority (`wiring_edges` in `MobMachine`) is the single source
    /// of truth; this event is observability only, never authority.
    ///
    /// Fields use the normalized `(a, b)` edge ordering (`a <= b`) produced
    /// by `WiringEdge::new`, so equal edges yield equal events regardless
    /// of caller argument order.
    MembersWired {
        /// First member of the edge (lexicographically smaller identity).
        a: AgentIdentity,
        /// Second member of the edge.
        b: AgentIdentity,
    },
    /// Multiple bidirectional wiring edges were established between local members.
    ///
    /// This is the compact projection event for batch topology materialization.
    /// MobMachine still owns the canonical `wiring_edges` graph; the event is
    /// replay data for roster/read-model consumers.
    MembersWiredBatch {
        /// Normalized `(a, b)` member edges admitted by the MobMachine authority.
        edges: Vec<MemberWireEdge>,
    },
    /// Bidirectional wiring edge removed between two local members.
    ///
    /// DSL-emit-driven observability counterpart of [`Self::MembersWired`].
    /// Emitted by the shell on successful `MobMachineInput::UnwireMembers`
    /// acceptance, mirroring `EmitWiringLifecycleNotice { kind: Unwired }`.
    /// Uses the same normalized `(a, b)` edge ordering.
    MembersUnwired {
        /// First member of the edge (lexicographically smaller identity).
        a: AgentIdentity,
        /// Second member of the edge.
        b: AgentIdentity,
    },
    /// A local member was wired to an external trusted peer.
    ///
    /// Restored post-#31 D-external-peer: external peer wiring carries the
    /// full [`TrustedPeerDescriptor`] through the event log so roster
    /// projection and resume can reinstate trust without consulting any
    /// live comms runtime. `local` is the local session-backed member that
    /// initiated the wire; `spec` is the full descriptor used to install
    /// trust on the local's session comms runtime.
    ExternalPeerWired {
        /// Local member that initiated the wire.
        local: AgentIdentity,
        /// Full trusted peer descriptor installed as trust on the local
        /// member's comms runtime.
        spec: TrustedPeerDescriptor,
    },
    /// A local member was unwired from a previously-wired external peer.
    ///
    /// Restored post-#31 D-external-peer: companion of
    /// [`Self::ExternalPeerWired`]. `peer_name` is the canonical
    /// [`PeerName`] of the external peer, matching the
    /// [`TrustedPeerDescriptor::name`] of the previously-wired descriptor.
    ExternalPeerUnwired {
        /// Local member that initiated the unwire.
        local: AgentIdentity,
        /// Canonical name of the external peer removed from local trust.
        peer_name: PeerName,
    },
    /// Flow run started.
    FlowStarted {
        run_id: RunId,
        flow_id: FlowId,
        params: serde_json::Value,
    },
    /// Flow run completed.
    FlowCompleted {
        run_id: RunId,
        flow_id: FlowId,
        /// Typed flow outputs captured at completion time.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        structured_output: Option<serde_json::Value>,
    },
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
        target: AgentRuntimeId,
    },
    /// Per-target successful completion event.
    StepTargetCompleted {
        run_id: RunId,
        step_id: StepId,
        target: AgentRuntimeId,
    },
    /// Per-target failure event.
    StepTargetFailed {
        run_id: RunId,
        step_id: StepId,
        target: AgentRuntimeId,
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error_report: Option<meerkat_core::event::AgentErrorReport>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<meerkat_core::event::TurnErrorMetadata>,
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
        escalated_to: AgentIdentity,
    },
    /// Dispatcher-owned projection of a successful operator mutation/control action.
    ///
    /// This event is provenance/audit only. It is never authorization truth.
    OperatorActionRecorded {
        tool_name: String,
        principal_token: OpaquePrincipalToken,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caller_provenance: Option<MobToolCallerProvenance>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        audit_invocation_id: Option<String>,
    },
}

/// Normalized local-member wiring edge carried by compact topology events.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemberWireEdge {
    /// First member of the edge (lexicographically smaller identity).
    pub a: AgentIdentity,
    /// Second member of the edge.
    pub b: AgentIdentity,
}

/// An agent event attributed to a specific mob member.
///
/// Wraps the full [`EventEnvelope<AgentEvent>`] to preserve event metadata
/// (`event_id`, `source_id`, `seq`, `mob_id`, `timestamp_ms`) without
/// information loss. Used by the mob event bus to tag session-level events
/// with mob-level attribution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttributedEvent {
    /// Identity-native runtime ID of the member that produced this event.
    pub source: AgentRuntimeId,
    /// Fence token for the emitting incarnation.
    pub source_fence_token: FenceToken,
    /// Profile name (role) of the source member.
    pub role: ProfileName,
    /// The original enveloped agent event from the session stream.
    pub envelope: EventEnvelope<AgentEvent>,
}

/// Public identity-native member spawn payload.
///
/// The bridge/session carrier is kept as crate-internal replay metadata only;
/// it is intentionally not serialized on the public event surface.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemberSpawnedEvent {
    /// Stable member identity.
    pub agent_identity: AgentIdentity,
    /// Generation counter (0 for initial spawn).
    pub generation: Generation,
    /// Fence token for stale-command rejection.
    pub fence_token: FenceToken,
    /// Composite runtime id.
    pub agent_runtime_id: AgentRuntimeId,
    /// Profile name used to spawn.
    pub role: ProfileName,
    /// Runtime mode for this spawned member.
    ///
    /// `#[serde(default)]` is load-bearing and intentional: pre-0.6 persisted
    /// events predate the `runtime_mode` field entirely. The default
    /// [`MobRuntimeMode::AutonomousHost`] matches pre-field semantics (the
    /// only mode that existed then), so replay on legacy data coerces to
    /// the historically correct value. Finding B7 (DELETE_ME) flagged this
    /// as "silent semantic coercion"; the coercion is deliberate and the
    /// regression test `mob_event_legacy_member_spawned_runtime_mode_defaults_to_autonomous_host`
    /// pins it so a future schema change cannot silently flip the default.
    #[serde(default)]
    pub runtime_mode: MobRuntimeMode,
    /// Application-defined labels for this member.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    /// Typed continuity evidence emitted at spawn finalization.
    #[serde(default, skip_serializing_if = "crate::event::is_ephemeral_continuity")]
    pub continuity_intent: crate::runtime::SpawnContinuityIntent,
    /// Bridge-internal member reference needed for event replay.
    /// Not part of the public identity-native contract.
    #[serde(skip, default)]
    pub(crate) bridge_member_ref: Option<MemberRef>,
}

impl MemberSpawnedEvent {
    pub fn new(
        agent_identity: AgentIdentity,
        generation: Generation,
        fence_token: FenceToken,
        agent_runtime_id: AgentRuntimeId,
        role: ProfileName,
    ) -> Self {
        Self {
            agent_identity,
            generation,
            fence_token,
            agent_runtime_id,
            role,
            runtime_mode: MobRuntimeMode::AutonomousHost,
            labels: BTreeMap::new(),
            continuity_intent: crate::runtime::SpawnContinuityIntent::Ephemeral,
            bridge_member_ref: None,
        }
    }

    pub(crate) fn with_bridge_member_ref(mut self, bridge_member_ref: Option<MemberRef>) -> Self {
        self.bridge_member_ref = bridge_member_ref;
        self
    }

    #[cfg(any(not(target_arch = "wasm32"), test))]
    pub(crate) fn bridge_member_ref(&self) -> Option<&MemberRef> {
        self.bridge_member_ref.as_ref()
    }
}

fn is_ephemeral_continuity(intent: &crate::runtime::SpawnContinuityIntent) -> bool {
    matches!(intent, crate::runtime::SpawnContinuityIntent::Ephemeral)
}

#[cfg(any(not(target_arch = "wasm32"), test))]
impl MobEventKind {
    pub(crate) fn member_spawned(&self) -> Option<&MemberSpawnedEvent> {
        match self {
            Self::MemberSpawned(event) => Some(event),
            _ => None,
        }
    }

    pub(crate) fn member_spawned_mut(&mut self) -> Option<&mut MemberSpawnedEvent> {
        match self {
            Self::MemberSpawned(event) => Some(event),
            _ => None,
        }
    }
}

/// Encode a stored mob event, preserving internal replay-only fields that are
/// intentionally omitted from the public event contract.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn encode_stored_mob_event(event: &MobEvent) -> Result<Vec<u8>, serde_json::Error> {
    let mut value = serde_json::to_value(event)?;
    if let Some(member_spawned) = event.kind.member_spawned()
        && let Some(bridge_member_ref) = member_spawned.bridge_member_ref()
        && let Some(kind) = value.get_mut("kind").and_then(Value::as_object_mut)
    {
        kind.insert(
            "bridge_member_ref".to_string(),
            serde_json::to_value(bridge_member_ref)?,
        );
    }
    serde_json::to_vec(&serde_json::json!({
        "schema_version": CURRENT_STORED_MOB_EVENT_SCHEMA_VERSION,
        "event": value,
    }))
}

/// Decode a stored mob event, restoring internal replay-only fields that are
/// not part of the public event contract.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn decode_stored_mob_event(bytes: &[u8]) -> Result<MobEvent, serde_json::Error> {
    let mut encoded: Value = serde_json::from_slice(bytes)?;
    let encoded_object = encoded.as_object_mut().ok_or_else(|| {
        stored_mob_event_format_error("stored mob event envelope must be an object")
    })?;
    let schema_version = encoded_object
        .remove("schema_version")
        .and_then(|value| value.as_u64())
        .ok_or_else(|| {
            stored_mob_event_format_error(
                "stored mob event missing schema_version; pre-0.6 mob event history is unsupported",
            )
        })?;
    if schema_version != CURRENT_STORED_MOB_EVENT_SCHEMA_VERSION as u64 {
        return Err(stored_mob_event_format_error(format!(
            "unsupported stored mob event schema_version={schema_version}; expected {CURRENT_STORED_MOB_EVENT_SCHEMA_VERSION}",
        )));
    }
    let mut value = encoded_object
        .remove("event")
        .ok_or_else(|| stored_mob_event_format_error("stored mob event missing event payload"))?;
    let bridge_member_ref = value
        .get_mut("kind")
        .and_then(Value::as_object_mut)
        .and_then(|kind| kind.remove("bridge_member_ref"))
        .map(serde_json::from_value)
        .transpose()?;
    let mut event: MobEvent = serde_json::from_value(value)?;
    if let Some(bridge_member_ref) = bridge_member_ref
        && let Some(member_spawned) = event.kind.member_spawned_mut()
    {
        member_spawned.bridge_member_ref = Some(bridge_member_ref);
    }
    Ok(event)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definition::{BackendConfig, MobDefinition, WiringRules};
    use crate::ids::MobId;
    use crate::profile::{Profile, ProfileBinding, ToolConfig};
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
                    ProfileBinding::Inline(Profile {
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
                    }),
                );
                m
            },
            wiring: WiringRules::default(),
            skills: BTreeMap::new(),
            backend: BackendConfig::default(),
            flows: BTreeMap::new(),
            topology: None,
            supervisor: None,
            limits: None,
            spawn_policy: None,
            event_router: None,
            owner_bridge_session_id: None,
            session_cleanup_policy: crate::definition::SessionCleanupPolicy::Manual,
            is_implicit: false,
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
    fn test_mob_destroying_roundtrip() {
        roundtrip(&MobEventKind::MobDestroying);
    }

    #[test]
    fn test_mob_destroy_storage_finalizing_roundtrip() {
        roundtrip(&MobEventKind::MobDestroyStorageFinalizing);
    }

    #[test]
    fn test_mob_reset_roundtrip() {
        roundtrip(&MobEventKind::MobReset);
    }

    #[test]
    fn test_flow_variants_roundtrip() {
        let run_id = RunId::new();
        let flow_id = FlowId::from("flow-a");
        let step_id = StepId::from("step-a");
        let runtime_id = AgentRuntimeId::initial(AgentIdentity::from("worker-1"));
        let escalated_identity = AgentIdentity::from("worker-1");

        roundtrip(&MobEventKind::FlowStarted {
            run_id: run_id.clone(),
            flow_id: flow_id.clone(),
            params: serde_json::json!({"k":"v"}),
        });
        roundtrip(&MobEventKind::FlowCompleted {
            run_id: run_id.clone(),
            flow_id: flow_id.clone(),
            structured_output: Some(serde_json::json!({
                "steps": {
                    "step-a": {
                        "ok": true
                    }
                }
            })),
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
            target: runtime_id.clone(),
        });
        roundtrip(&MobEventKind::StepTargetCompleted {
            run_id: run_id.clone(),
            step_id: step_id.clone(),
            target: runtime_id.clone(),
        });
        roundtrip(&MobEventKind::StepTargetFailed {
            run_id: run_id.clone(),
            step_id: step_id.clone(),
            target: runtime_id.clone(),
            reason: "fail".to_string(),
            error_report: None,
            error: None,
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
            escalated_to: escalated_identity,
        });
    }

    #[test]
    fn test_members_wired_roundtrip() {
        roundtrip(&MobEventKind::MembersWired {
            a: AgentIdentity::from("l-1"),
            b: AgentIdentity::from("w-2"),
        });
    }

    #[test]
    fn test_members_wired_batch_roundtrip() {
        roundtrip(&MobEventKind::MembersWiredBatch {
            edges: vec![
                MemberWireEdge {
                    a: AgentIdentity::from("l-1"),
                    b: AgentIdentity::from("w-2"),
                },
                MemberWireEdge {
                    a: AgentIdentity::from("l-1"),
                    b: AgentIdentity::from("w-3"),
                },
            ],
        });
    }

    #[test]
    fn test_members_unwired_roundtrip() {
        roundtrip(&MobEventKind::MembersUnwired {
            a: AgentIdentity::from("l-1"),
            b: AgentIdentity::from("w-2"),
        });
    }

    #[test]
    fn test_external_peer_wired_roundtrip() {
        let pubkey = [8u8; 32];
        let peer_id = meerkat_core::comms::PeerId::from_ed25519_pubkey(&pubkey);
        let spec = TrustedPeerDescriptor::unsigned_with_pubkey(
            "remote-mob/worker/agent-b",
            peer_id.to_string(),
            pubkey,
            "inproc://remote-mob/worker/agent-b",
        )
        .expect("valid external peer");
        roundtrip(&MobEventKind::ExternalPeerWired {
            local: AgentIdentity::from("l-1"),
            spec,
        });
    }

    #[test]
    fn test_external_peer_unwired_roundtrip() {
        roundtrip(&MobEventKind::ExternalPeerUnwired {
            local: AgentIdentity::from("l-1"),
            peer_name: PeerName::new("remote-mob/worker/agent-b").unwrap(),
        });
    }

    #[test]
    fn test_operator_action_recorded_roundtrip() {
        roundtrip(&MobEventKind::OperatorActionRecorded {
            tool_name: "mob_create".to_string(),
            principal_token: OpaquePrincipalToken::new("opaque-principal"),
            caller_provenance: Some(
                MobToolCallerProvenance::new()
                    .with_session_id(SessionId::from_uuid(Uuid::nil()))
                    .with_mob_id("test-mob")
                    .with_member_id("lead-1"),
            ),
            audit_invocation_id: Some("audit-123".to_string()),
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
    fn test_member_ref_rejects_legacy_session_only_payload() {
        let sid = SessionId::from_uuid(Uuid::nil());
        let parsed = serde_json::from_value::<MemberRef>(json!({
            "session_id": sid,
        }));

        assert!(parsed.is_err());
    }

    #[test]
    fn test_member_ref_serializes_deterministically() {
        let sid = SessionId::from_uuid(Uuid::nil());
        let member_ref = MemberRef::from_bridge_session_id(sid);

        let first = serde_json::to_string(&member_ref).unwrap();
        let second = serde_json::to_string(&member_ref).unwrap();
        assert_eq!(first, second);
        assert_eq!(
            first,
            r#"{"kind":"session","session_id":"00000000-0000-0000-0000-000000000000","bridge_session_id":"00000000-0000-0000-0000-000000000000"}"#
        );
    }

    #[test]
    fn test_member_ref_deserializes_bridge_session_only_payload() {
        let sid = SessionId::from_uuid(Uuid::nil());
        let parsed: MemberRef = serde_json::from_value(json!({
            "kind": "session",
            "bridge_session_id": sid,
        }))
        .unwrap();

        assert_eq!(parsed, MemberRef::from_bridge_session_id(sid));
    }

    #[test]
    fn test_member_ref_roundtrip_backend_peer() {
        let member_ref = MemberRef::BackendPeer {
            peer_id: "peer-123".to_string(),
            address: "https://backend.example/peers/peer-123".to_string(),
            pubkey: Some([9u8; 32]),
            bootstrap_token: None,
            session_id: Some(SessionId::from_uuid(Uuid::nil())),
        };

        let json = serde_json::to_string(&member_ref).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            value["bridge_session_id"],
            "00000000-0000-0000-0000-000000000000"
        );
        let parsed: MemberRef = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, member_ref);
    }

    #[test]
    fn test_member_ref_backend_peer_omits_bootstrap_token_in_serialized_output() {
        let member_ref = MemberRef::BackendPeer {
            peer_id: "peer-123".to_string(),
            address: "https://backend.example/peers/peer-123".to_string(),
            pubkey: None,
            bootstrap_token: Some("secret-bootstrap-proof".into()),
            session_id: None,
        };

        let value = serde_json::to_value(&member_ref).unwrap();
        assert_eq!(value["kind"], "backend_peer");
        assert_eq!(value["peer_id"], "peer-123");
        assert_eq!(value["address"], "https://backend.example/peers/peer-123");
        assert!(
            value.get("bootstrap_token").is_none(),
            "bootstrap proof must not be exposed through serialized member refs"
        );
    }

    #[test]
    fn test_stored_mob_event_roundtrip_preserves_bridge_member_ref() {
        let sid = SessionId::from_uuid(Uuid::nil());
        let identity = AgentIdentity::from("researcher");
        let event = MobEvent {
            cursor: 1,
            timestamp: Utc::now(),
            mob_id: MobId::from("test-mob"),
            kind: MobEventKind::MemberSpawned(
                MemberSpawnedEvent::new(
                    identity.clone(),
                    Generation::INITIAL,
                    FenceToken::new(1),
                    AgentRuntimeId::initial(identity),
                    ProfileName::from("worker"),
                )
                .with_bridge_member_ref(Some(MemberRef::from_bridge_session_id(sid.clone()))),
            ),
        };

        let encoded = encode_stored_mob_event(&event).unwrap();
        let decoded = decode_stored_mob_event(&encoded).unwrap();

        match decoded.kind {
            MobEventKind::MemberSpawned(member_spawned) => {
                assert_eq!(
                    member_spawned
                        .bridge_member_ref()
                        .and_then(MemberRef::bridge_session_id),
                    Some(&sid)
                );
            }
            other => panic!("expected MemberSpawned, got {other:?}"),
        }
    }

    /// DELETE_ME A6 regression: `MemberSpawned(MemberSpawnedEvent)` uses
    /// a named struct variant while every other `Member*` variant uses
    /// inline fields. The reason is load-bearing: `bridge_member_ref` is
    /// crate-internal replay metadata gated by `#[serde(skip)]` that must
    /// never leak onto the public wire shape. This test pins the public
    /// `MobEventKind::MemberSpawned` serialized form to exclude
    /// `bridge_member_ref` so a future refactor cannot silently promote
    /// the internal replay pointer into the public event contract.
    #[test]
    fn member_spawned_public_wire_shape_excludes_bridge_member_ref() {
        let identity = AgentIdentity::from("researcher");
        let sid = SessionId::from_uuid(Uuid::nil());
        let kind = MobEventKind::MemberSpawned(
            MemberSpawnedEvent::new(
                identity.clone(),
                Generation::INITIAL,
                FenceToken::new(1),
                AgentRuntimeId::initial(identity),
                ProfileName::from("worker"),
            )
            // set the internal pointer so we know the exclusion is real,
            // not a side-effect of it being None.
            .with_bridge_member_ref(Some(MemberRef::from_bridge_session_id(sid))),
        );

        let value = serde_json::to_value(&kind).expect("serialize mob event kind");
        let object = value
            .as_object()
            .expect("MemberSpawned serializes as an object");

        // The public wire shape is {"type":"MemberSpawned", ...fields...}.
        // We don't assert the exact tag convention (that may be internally
        // tagged or adjacent); we assert the inner payload does NOT carry
        // bridge_member_ref under any key path.
        let serialized = serde_json::to_string(&value).unwrap();
        assert!(
            !serialized.contains("bridge_member_ref"),
            "public MemberSpawned wire shape must never expose bridge_member_ref; got: {serialized}",
        );

        // And the standard struct fields ARE present.
        let payload_carrier = object.values().find(|v| v.is_object()).unwrap_or(&value);
        let payload_object = payload_carrier.as_object().unwrap_or(object);
        assert!(
            payload_object.contains_key("agent_identity") || serialized.contains("agent_identity"),
            "public MemberSpawned must carry agent_identity: {serialized}",
        );
    }

    #[test]
    fn test_decode_stored_mob_event_rejects_unversioned_payload() {
        let raw = serde_json::to_vec(&MobEvent {
            cursor: 1,
            timestamp: Utc::now(),
            mob_id: MobId::from("test-mob"),
            kind: MobEventKind::MobCompleted,
        })
        .unwrap();

        let error =
            decode_stored_mob_event(&raw).expect_err("unversioned payload must be rejected");
        assert!(
            error
                .to_string()
                .contains("pre-0.6 mob event history is unsupported")
        );
    }

    #[test]
    fn test_decode_stored_mob_event_rejects_unsupported_schema_version() {
        let raw = serde_json::to_vec(&json!({
            "schema_version": CURRENT_STORED_MOB_EVENT_SCHEMA_VERSION + 1,
            "event": {
                "cursor": 1,
                "timestamp": "2026-02-19T00:00:00Z",
                "mob_id": "test-mob",
                "kind": {
                    "type": "mob_completed"
                }
            }
        }))
        .unwrap();

        let error =
            decode_stored_mob_event(&raw).expect_err("unsupported schema version must be rejected");
        assert!(
            error
                .to_string()
                .contains("unsupported stored mob event schema_version")
        );
    }

    // --- Identity-native event variant tests ---

    #[test]
    fn test_member_spawned_roundtrip() {
        let identity = AgentIdentity::from("researcher");
        roundtrip(&MobEventKind::MemberSpawned(MemberSpawnedEvent::new(
            identity.clone(),
            Generation::INITIAL,
            FenceToken::new(1),
            AgentRuntimeId::initial(identity),
            ProfileName::from("worker"),
        )));
    }

    #[test]
    fn test_member_spawned_with_labels_roundtrip() {
        let identity = AgentIdentity::from("coder");
        let mut labels = BTreeMap::new();
        labels.insert("team".to_string(), "backend".to_string());
        let mut event = MemberSpawnedEvent::new(
            identity.clone(),
            Generation::INITIAL,
            FenceToken::new(42),
            AgentRuntimeId::initial(identity),
            ProfileName::from("coder"),
        );
        event.runtime_mode = MobRuntimeMode::TurnDriven;
        event.labels = labels;
        roundtrip(&MobEventKind::MemberSpawned(event));
    }

    #[test]
    fn test_member_spawned_defaults_runtime_mode() {
        let event: MobEvent = serde_json::from_value(json!({
            "cursor": 1,
            "timestamp": "2026-02-19T00:00:00Z",
            "mob_id": "test-mob",
            "kind": {
                "type": "member_spawned",
                "agent_identity": "researcher",
                "generation": 0,
                "fence_token": 1,
                "agent_runtime_id": {
                    "identity": "researcher",
                    "generation": 0
                },
                "role": "worker"
            },
        }))
        .expect("member_spawned without runtime_mode should parse");
        match event.kind {
            MobEventKind::MemberSpawned(member_spawned) => {
                assert_eq!(member_spawned.runtime_mode, MobRuntimeMode::AutonomousHost);
            }
            other => panic!("expected MemberSpawned, got {other:?}"),
        }
    }

    #[test]
    fn test_member_retired_roundtrip() {
        roundtrip(&MobEventKind::MemberRetired {
            agent_identity: AgentIdentity::from("researcher"),
            generation: Generation::new(2),
            role: ProfileName::from("worker"),
        });
    }

    #[test]
    fn test_member_reset_roundtrip() {
        let identity = AgentIdentity::from("worker-1");
        roundtrip(&MobEventKind::MemberReset {
            agent_identity: identity.clone(),
            previous_generation: Generation::new(0),
            new_generation: Generation::new(1),
            fence_token: FenceToken::new(2),
            agent_runtime_id: AgentRuntimeId::new(identity, Generation::new(1)),
        });
    }

    #[test]
    fn test_member_reset_generation_advancement() {
        let identity = AgentIdentity::from("agent-x");
        let event = MobEventKind::MemberReset {
            agent_identity: identity.clone(),
            previous_generation: Generation::new(3),
            new_generation: Generation::new(4),
            fence_token: FenceToken::new(99),
            agent_runtime_id: AgentRuntimeId::new(identity, Generation::new(4)),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: MobEventKind = serde_json::from_str(&json).unwrap();
        if let MobEventKind::MemberReset {
            previous_generation,
            new_generation,
            ..
        } = parsed
        {
            assert!(new_generation > previous_generation);
        } else {
            panic!("expected MemberReset");
        }
    }

    #[test]
    fn mob_event_legacy_member_spawned_runtime_mode_defaults_to_autonomous_host() {
        // Finding B7 (DELETE_ME): MemberSpawnedEvent.runtime_mode carries
        // `#[serde(default)]` so pre-0.6 persisted events without the field
        // deserialize to MobRuntimeMode::AutonomousHost. That matches the
        // pre-field semantics (AutonomousHost was the only mode that
        // existed). Lock this coercion in so a future schema change cannot
        // silently flip the default to TurnDriven and corrupt replay of
        // legacy data.
        // MobEventKind uses `#[serde(tag = "type", rename_all = "snake_case")]`
        // so MemberSpawned lands as `{"type": "member_spawned", ...}` with the
        // wrapped MemberSpawnedEvent's fields at the same level.
        let legacy_json = serde_json::json!({
            "type": "member_spawned",
            "agent_identity": "legacy-worker",
            "generation": 0,
            "fence_token": 0,
            "agent_runtime_id": {
                "identity": "legacy-worker",
                "generation": 0
            },
            "role": "worker",
            // runtime_mode intentionally omitted — the whole point of this
            // regression is "missing field" replay.
            "labels": {}
        });
        let parsed: MobEventKind =
            serde_json::from_value(legacy_json).expect("legacy event must deserialize");
        match parsed {
            MobEventKind::MemberSpawned(event) => {
                assert_eq!(
                    event.runtime_mode,
                    MobRuntimeMode::AutonomousHost,
                    "legacy MemberSpawnedEvent without runtime_mode must coerce to AutonomousHost",
                );
                assert_eq!(event.agent_identity, AgentIdentity::from("legacy-worker"));
            }
            other => panic!("expected MemberSpawned, got {other:?}"),
        }
    }
}

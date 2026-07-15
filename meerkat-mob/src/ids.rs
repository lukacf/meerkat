//! Newtype identifiers for mob entities.
//!
//! These types wrap concrete primitives for compile-time safety.

use serde::{Deserialize, Serialize};
use std::borrow::Borrow;
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

/// Unique identifier for a flow run.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RunId(Uuid);

impl RunId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for RunId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for RunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for RunId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

macro_rules! string_newtype {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl Borrow<str> for $name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }

        impl Borrow<String> for $name {
            fn borrow(&self) -> &String {
                &self.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl PartialEq<String> for $name {
            fn eq(&self, other: &String) -> bool {
                &self.0 == other
            }
        }

        impl PartialEq<&String> for $name {
            fn eq(&self, other: &&String) -> bool {
                &self.0 == *other
            }
        }

        impl PartialEq<str> for $name {
            fn eq(&self, other: &str) -> bool {
                self.0.as_str() == other
            }
        }

        impl PartialEq<&str> for $name {
            fn eq(&self, other: &&str) -> bool {
                self.0.as_str() == *other
            }
        }
    };
}

string_newtype!(
    /// Unique identifier for a mob instance.
    MobId
);

string_newtype!(
    /// Unique identifier for a flow definition.
    FlowId
);

string_newtype!(
    /// Unique identifier for a step in a flow definition.
    StepId
);

string_newtype!(
    /// Branch group identifier used by mutually-exclusive flow steps.
    BranchId
);

string_newtype!(
    /// Profile name within a mob definition.
    ProfileName
);

string_newtype!(
    /// Runtime identifier for a flow execution frame. One per FrameSpec invocation.
    FrameId
);

string_newtype!(
    /// Runtime identifier for one instance of a repeat_until loop.
    LoopInstanceId
);

string_newtype!(
    /// Lexical identifier for a node within a FrameSpec.
    FlowNodeId
);

string_newtype!(
    /// Lexical identifier for a loop definition within a FrameSpec.
    LoopId
);

// ---------------------------------------------------------------------------
// Identity-first mob model types (0.6)
// ---------------------------------------------------------------------------

string_newtype!(
    /// Stable, human-meaningful identity for a mob member.
    ///
    /// An `AgentIdentity` is assigned at spawn and persists across respawns and
    /// resets. It is the canonical key for all public mob APIs.
    AgentIdentity
);

string_newtype!(
    /// Canonical peer identity carried in respawn topology-restore feedback.
    ///
    /// Local member edges use the member's [`AgentIdentity`] string. External
    /// peer edges use the comms [`meerkat_core::comms::PeerId`] string, never
    /// the display-only peer name.
    RespawnTopologyPeerId
);

impl AgentIdentity {
    /// Stable synthetic lead principal for an owner bridge session that is
    /// not itself a roster member (implicit delegation mobs).
    pub fn objective_lead_for_session(session_id: &meerkat_core::SessionId) -> Self {
        Self::from(format!("objective-lead-session:{session_id}"))
    }

    /// Returns `true` when this identity falls inside the reserved
    /// flow-owned member namespace.
    ///
    /// This is the single canonical predicate for "is this a system-owned
    /// identity the public spawn/wire surfaces must reject"; all admission and
    /// validation sites route through it rather than re-deriving the meaning
    /// from the underlying string prefix.
    pub(crate) fn is_system_reserved(&self) -> bool {
        self.as_str()
            .starts_with(crate::runtime::FLOW_MEMBER_ID_PREFIX)
    }

    /// Returns `true` when this identity falls inside the flow-owned member
    /// namespace. Only MobMachine-owned flow provisioning may mint these
    /// identities; public spawn surfaces must reject them.
    pub(crate) fn is_flow_member_namespace(&self) -> bool {
        self.is_system_reserved()
    }

    /// Constructs the reserved provenance identity used to attribute
    /// flow-system-authored step ledger entries.
    pub(crate) fn flow_system_provenance() -> Self {
        Self::from(crate::run::FLOW_RUN_PROVENANCE_AGENT_ID)
    }

    /// Returns `true` when this identity is exactly the reserved flow-system
    /// provenance identity (stricter than [`Self::is_system_reserved`], which
    /// matches the whole reserved namespace).
    pub(crate) fn is_flow_system_provenance(&self) -> bool {
        self.as_str() == crate::run::FLOW_RUN_PROVENANCE_AGENT_ID
    }
}

/// Monotonically increasing generation counter for a mob member.
///
/// Starts at 0 on first spawn, advances on each reset. The generation is
/// part of [`AgentRuntimeId`] and disambiguates successive incarnations of
/// the same [`AgentIdentity`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Generation(u64);

impl Generation {
    /// The initial generation assigned to a freshly spawned member.
    pub const INITIAL: Self = Self(0);

    /// Create a generation from a raw value.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Return the underlying value.
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Advance to the next generation.
    pub const fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

impl fmt::Display for Generation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Unique runtime identity for a specific incarnation of a mob member.
///
/// Combines the stable [`AgentIdentity`] with a [`Generation`] counter that
/// advances on reset. Two `AgentRuntimeId` values with the same identity but
/// different generations represent successive incarnations of the same member.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AgentRuntimeId {
    /// Stable member identity.
    pub identity: AgentIdentity,
    /// Generation counter for this incarnation.
    pub generation: Generation,
}

impl AgentRuntimeId {
    /// Create a new runtime id.
    pub fn new(identity: AgentIdentity, generation: Generation) -> Self {
        Self {
            identity,
            generation,
        }
    }

    /// Create an initial runtime id (generation 0).
    pub fn initial(identity: AgentIdentity) -> Self {
        Self {
            identity,
            generation: Generation::INITIAL,
        }
    }
}

impl fmt::Display for AgentRuntimeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.identity, self.generation.get())
    }
}

/// Opaque fence token used to reject stale commands.
///
/// A new `FenceToken` is issued at spawn, respawn, and reset. Commands
/// carrying a stale token are rejected, preventing races where a delayed
/// message targets an incarnation that has already been replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FenceToken(u64);

impl FenceToken {
    /// Create a fence token from a raw value.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Return the underlying value.
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for FenceToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "fence:{}", self.0)
    }
}

/// Unique identifier for a unit of work submitted to a mob member.
///
/// Analogous to [`RunId`] but scoped to the work-lane abstraction introduced
/// in the identity-first model.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorkRef(Uuid);

impl WorkRef {
    /// Generate a new random work reference.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Return the underlying UUID.
    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for WorkRef {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for WorkRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for WorkRef {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

/// Describes a unit of work to be executed by a mob member.
///
/// `WorkSpec` is submitted alongside a [`WorkRef`] and [`FenceToken`] through
/// the work lane. It captures the content and delivery semantics without
/// exposing session-level details.
///
/// DELETE_ME C6: `content` is a full [`meerkat_core::types::ContentInput`]
/// (multimodal) rather than `String`, matching the rest of the platform's
/// content-carrying types. Prior to this change the work lane was silently
/// text-only, which was a capability regression vs. every other member-
/// delivery surface. `impl From<String> for ContentInput` / `From<&str>` in
/// `meerkat_core` means existing String call sites upgrade without
/// per-call-site conversion noise.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkSpec {
    /// The content to deliver to the member.
    pub content: meerkat_core::types::ContentInput,
    /// Whether this is an externally-originated turn (user input) or an
    /// internally-originated turn (mob coordination).
    pub origin: WorkOrigin,
    /// Host-attached injected context delivered alongside (not inside) the
    /// work content. Each entry materializes as a separate typed
    /// injected-context transcript message immediately before the work
    /// content, in order. Deliverable on queue-mode turn-driven dispatch
    /// (local and remote); autonomous inbox delivery and steer-mode dispatch
    /// reject it fail-closed with a typed [`crate::MobError`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub injected_context: Vec<meerkat_core::types::ContentInput>,
    /// Ephemeral tool visibility and dispatch context for this turn.
    ///
    /// This carrier is supported only by turn-driven members. Autonomous
    /// inbox delivery cannot install a per-turn overlay and rejects a present
    /// value before the MobMachine admits the work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_tool_overlay: Option<meerkat_core::service::TurnToolOverlay>,
    /// Host-supplied interaction identity for this unit of work.
    ///
    /// Ask-15 addendum: the delivery path carries this id to
    /// `RuntimeTurnMetadata.transcript_identity` (turn-driven dispatch) or
    /// stamps it onto the injected inbox event (autonomous dispatch), so the
    /// transcript messages committed for this turn persist the SAME
    /// interaction id the host's live `interaction_complete` frames carry —
    /// history backfill can then join persisted and live frames without
    /// content heuristics. Serde-additive: absent on old stored shapes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction_id: Option<meerkat_core::interaction::InteractionId>,
    /// Durable objective correlation propagated through the work lane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objective_id: Option<meerkat_core::interaction::ObjectiveId>,
}

impl WorkSpec {
    /// Create a new work spec. Accepts anything that implements
    /// `Into<ContentInput>` — including `String` and `&str` — so existing
    /// text-only call sites upgrade without churn.
    pub fn new(content: impl Into<meerkat_core::types::ContentInput>, origin: WorkOrigin) -> Self {
        Self {
            content: content.into(),
            origin,
            injected_context: Vec::new(),
            turn_tool_overlay: None,
            interaction_id: None,
            objective_id: None,
        }
    }

    #[must_use]
    pub fn with_objective_id(
        mut self,
        objective_id: meerkat_core::interaction::ObjectiveId,
    ) -> Self {
        self.objective_id = Some(objective_id);
        self
    }

    /// Attach host-attached injected context to this work spec.
    pub fn with_injected_context(
        mut self,
        injected_context: Vec<meerkat_core::types::ContentInput>,
    ) -> Self {
        self.injected_context = injected_context;
        self
    }

    /// Attach an ephemeral per-turn tool overlay to this work spec.
    pub fn with_turn_tool_overlay(
        mut self,
        turn_tool_overlay: meerkat_core::service::TurnToolOverlay,
    ) -> Self {
        self.turn_tool_overlay = Some(turn_tool_overlay);
        self
    }

    /// Attach a host-supplied interaction id so the committed transcript
    /// messages for this turn persist the same identity the host's live
    /// interaction frames carry.
    pub fn with_interaction_id(
        mut self,
        interaction_id: meerkat_core::interaction::InteractionId,
    ) -> Self {
        self.interaction_id = Some(interaction_id);
        self
    }
}

/// Origin classification for a [`WorkSpec`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WorkOrigin {
    /// Externally-originated work (user or API surface).
    External,
    /// Internally-originated work (mob orchestration, flow engine).
    Internal,
}

impl WorkOrigin {
    /// Stable string label consumed by `MobMachine` DSL guards.
    pub const fn as_str(self) -> &'static str {
        match self {
            WorkOrigin::External => "External",
            WorkOrigin::Internal => "Internal",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_spec_interaction_id_is_serde_additive() {
        // Absent field (the pre-ask-15 stored shape) deserializes to None,
        // and None is skipped on serialization so old readers see the old
        // shape unchanged.
        let json = serde_json::to_value(WorkSpec::new(
            "do the thing".to_string(),
            WorkOrigin::Internal,
        ))
        .unwrap();
        assert!(
            json.get("interaction_id").is_none(),
            "None must not serialize (serde-additive)"
        );
        let spec: WorkSpec = serde_json::from_value(json).unwrap();
        assert!(spec.interaction_id.is_none());

        // A carried id round-trips.
        let id = meerkat_core::interaction::InteractionId(Uuid::new_v4());
        let keyed =
            WorkSpec::new("do the thing".to_string(), WorkOrigin::External).with_interaction_id(id);
        let json = serde_json::to_value(&keyed).unwrap();
        let parsed: WorkSpec = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.interaction_id, Some(id));
    }

    #[test]
    fn work_spec_turn_tool_overlay_is_serde_additive() {
        let plain = WorkSpec::new("do the thing", WorkOrigin::External);
        let plain_json = serde_json::to_value(&plain).unwrap();
        assert!(
            plain_json.get("turn_tool_overlay").is_none(),
            "None must preserve the pre-field serialized shape"
        );
        let decoded: WorkSpec = serde_json::from_value(plain_json).unwrap();
        assert!(decoded.turn_tool_overlay.is_none());

        let overlay = meerkat_core::service::TurnToolOverlay {
            allowed_tools: Some(vec!["host_tool_alpha".into()]),
            blocked_tools: Some(vec!["shell".into()]),
            dispatch_context: std::collections::BTreeMap::from([(
                "host_tool_connection".to_string(),
                serde_json::json!({"connection_id": "host-1"}),
            )]),
        };
        let carried = WorkSpec::new("do the thing", WorkOrigin::External)
            .with_turn_tool_overlay(overlay.clone());
        let round_trip: WorkSpec =
            serde_json::from_value(serde_json::to_value(&carried).unwrap()).unwrap();
        assert_eq!(round_trip.turn_tool_overlay, Some(overlay));
    }

    #[test]
    fn test_run_id_roundtrip_json() {
        let run_id = RunId::new();
        let encoded = serde_json::to_string(&run_id).unwrap();
        let decoded: RunId = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, run_id);
    }

    #[test]
    fn test_run_id_roundtrip_parse_display() {
        let run_id = RunId::new();
        let rendered = run_id.to_string();
        let reparsed = RunId::from_str(&rendered).unwrap();
        assert_eq!(reparsed, run_id);
    }

    #[test]
    fn test_flow_id_roundtrip_json() {
        let id = FlowId::from("flow-a");
        let encoded = serde_json::to_string(&id).unwrap();
        let decoded: FlowId = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, id);
    }

    #[test]
    fn test_step_id_roundtrip_json() {
        let id = StepId::from("step-a");
        let encoded = serde_json::to_string(&id).unwrap();
        let decoded: StepId = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, id);
    }

    #[test]
    fn test_branch_id_roundtrip_json() {
        let id = BranchId::from("branch-a");
        let encoded = serde_json::to_string(&id).unwrap();
        let decoded: BranchId = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, id);
    }

    #[test]
    fn test_frame_id_roundtrip_json() {
        let id = FrameId::from("frame-a");
        let encoded = serde_json::to_string(&id).unwrap();
        let decoded: FrameId = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, id);
    }

    #[test]
    fn test_loop_instance_id_roundtrip_json() {
        let id = LoopInstanceId::from("loop-instance-a");
        let encoded = serde_json::to_string(&id).unwrap();
        let decoded: LoopInstanceId = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, id);
    }

    #[test]
    fn test_flow_node_id_roundtrip_json() {
        let id = FlowNodeId::from("node-a");
        let encoded = serde_json::to_string(&id).unwrap();
        let decoded: FlowNodeId = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, id);
    }

    #[test]
    fn test_loop_id_roundtrip_json() {
        let id = LoopId::from("loop-a");
        let encoded = serde_json::to_string(&id).unwrap();
        let decoded: LoopId = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, id);
    }

    #[test]
    fn test_existing_ids_roundtrip() {
        let mob = MobId::from("mob-a");
        let profile = ProfileName::from("lead");
        assert_eq!(
            serde_json::from_str::<MobId>(&serde_json::to_string(&mob).unwrap()).unwrap(),
            mob
        );
        assert_eq!(
            serde_json::from_str::<ProfileName>(&serde_json::to_string(&profile).unwrap()).unwrap(),
            profile
        );
    }

    // --- Identity-first model types ---

    #[test]
    fn test_agent_identity_roundtrip_json() {
        let id = AgentIdentity::from("researcher");
        let encoded = serde_json::to_string(&id).unwrap();
        assert_eq!(encoded, "\"researcher\"");
        let decoded: AgentIdentity = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, id);
    }

    #[test]
    fn test_agent_identity_display() {
        let id = AgentIdentity::from("lead-agent");
        assert_eq!(id.to_string(), "lead-agent");
        assert_eq!(id.as_str(), "lead-agent");
    }

    #[test]
    fn test_generation_roundtrip_json() {
        let generation = Generation::new(42);
        let encoded = serde_json::to_string(&generation).unwrap();
        assert_eq!(encoded, "42");
        let decoded: Generation = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, generation);
    }

    #[test]
    fn test_generation_initial_and_next() {
        assert_eq!(Generation::INITIAL.get(), 0);
        assert_eq!(Generation::INITIAL.next().get(), 1);
        assert_eq!(Generation::new(5).next().get(), 6);
    }

    #[test]
    fn test_generation_ordering() {
        assert!(Generation::new(0) < Generation::new(1));
        assert!(Generation::new(1) < Generation::new(100));
    }

    #[test]
    fn test_agent_runtime_id_roundtrip_json() {
        let rid = AgentRuntimeId::new(AgentIdentity::from("worker"), Generation::new(3));
        let encoded = serde_json::to_string(&rid).unwrap();
        let decoded: AgentRuntimeId = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, rid);
    }

    #[test]
    fn test_agent_runtime_id_initial() {
        let rid = AgentRuntimeId::initial(AgentIdentity::from("worker"));
        assert_eq!(rid.identity, AgentIdentity::from("worker"));
        assert_eq!(rid.generation, Generation::INITIAL);
    }

    #[test]
    fn test_agent_runtime_id_display() {
        let rid = AgentRuntimeId::new(AgentIdentity::from("coder"), Generation::new(2));
        assert_eq!(rid.to_string(), "coder:2");
    }

    #[test]
    fn test_fence_token_roundtrip_json() {
        let ft = FenceToken::new(99);
        let encoded = serde_json::to_string(&ft).unwrap();
        assert_eq!(encoded, "99");
        let decoded: FenceToken = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, ft);
    }

    #[test]
    fn test_fence_token_display() {
        assert_eq!(FenceToken::new(7).to_string(), "fence:7");
    }

    #[test]
    fn test_fence_token_ordering() {
        assert!(FenceToken::new(1) < FenceToken::new(2));
    }

    #[test]
    fn test_work_ref_roundtrip_json() {
        let wr = WorkRef::new();
        let encoded = serde_json::to_string(&wr).unwrap();
        let decoded: WorkRef = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, wr);
    }

    #[test]
    fn test_work_ref_roundtrip_parse_display() {
        let wr = WorkRef::new();
        let rendered = wr.to_string();
        let reparsed = WorkRef::from_str(&rendered).unwrap();
        assert_eq!(reparsed, wr);
    }

    #[test]
    fn test_work_spec_roundtrip_json() {
        let spec = WorkSpec::new("do something".to_owned(), WorkOrigin::External);
        let encoded = serde_json::to_string(&spec).unwrap();
        let decoded: WorkSpec = serde_json::from_str(&encoded).unwrap();
        assert_eq!(
            decoded.content,
            meerkat_core::types::ContentInput::from("do something".to_string()),
        );
        assert_eq!(decoded.origin, WorkOrigin::External);
    }

    #[test]
    fn test_work_origin_variants_roundtrip_json() {
        for origin in [WorkOrigin::External, WorkOrigin::Internal] {
            let encoded = serde_json::to_string(&origin).unwrap();
            let decoded: WorkOrigin = serde_json::from_str(&encoded).unwrap();
            assert_eq!(decoded, origin);
        }
    }

    #[test]
    fn test_work_spec_injected_context_serde_default_and_omission() {
        // Absent field defaults to empty (pre-field payloads stay readable).
        let decoded: WorkSpec =
            serde_json::from_str(r#"{"content":"do something","origin":"External"}"#).unwrap();
        assert!(decoded.injected_context.is_empty());

        // Empty is omitted; non-empty round-trips in order.
        let empty = WorkSpec::new("do something".to_owned(), WorkOrigin::External);
        let value = serde_json::to_value(&empty).unwrap();
        assert!(value.get("injected_context").is_none());

        let spec = WorkSpec::new("do something".to_owned(), WorkOrigin::External)
            .with_injected_context(vec![
                meerkat_core::types::ContentInput::Text("ambient alpha".to_string()),
                meerkat_core::types::ContentInput::Text("ambient beta".to_string()),
            ]);
        let encoded = serde_json::to_string(&spec).unwrap();
        let decoded: WorkSpec = serde_json::from_str(&encoded).unwrap();
        assert_eq!(
            decoded.injected_context,
            vec![
                meerkat_core::types::ContentInput::Text("ambient alpha".to_string()),
                meerkat_core::types::ContentInput::Text("ambient beta".to_string()),
            ],
        );
    }

    #[test]
    fn test_work_spec_internal_origin() {
        let spec = WorkSpec::new("coordinate".to_owned(), WorkOrigin::Internal);
        assert_eq!(spec.origin, WorkOrigin::Internal);
        assert_eq!(
            spec.content,
            meerkat_core::types::ContentInput::from("coordinate".to_string()),
        );
    }

    #[test]
    fn test_work_spec_accepts_multimodal_content() {
        // DELETE_ME C6 regression: WorkSpec.content must be ContentInput
        // (multimodal), not String. This test locks in that non-text
        // ContentInput variants (e.g. image blocks) can be submitted as
        // work content without string-coercing them first.
        let image_block = meerkat_core::types::ContentBlock::Image {
            media_type: "image/png".to_string(),
            data: meerkat_core::ImageData::Inline {
                data: "iVBORw0KGgo=".to_string(),
            },
        };
        let content = meerkat_core::types::ContentInput::Blocks(vec![
            meerkat_core::types::ContentBlock::Text {
                text: "analyse this".to_string(),
            },
            image_block.clone(),
        ]);
        let spec = WorkSpec::new(content.clone(), WorkOrigin::External);
        assert_eq!(spec.content, content);
    }
}

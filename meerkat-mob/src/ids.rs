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

/// Legacy carrier-name alias for [`AgentIdentity`].
///
/// DELETE_ME A5 DSL-schema migration: the 0.6 identity-first cascade
/// unifies the "member identifier string" fact under a single type.
/// `MeerkatId` was a separate `string_newtype!` wrapper over `String`,
/// structurally identical to `AgentIdentity` but nominally distinct —
/// that was parallel truth under dogma principle #1 ("one semantic
/// fact, one owner").
///
/// Collapsing the type (`pub type MeerkatId = AgentIdentity;`) unifies
/// the ownership without forcing a rename of every generated DSL
/// command variant field (`Retire { agent_identity }`, `Wire { local }`,
/// etc.) in a single pass — those field names are now just aliases
/// that read as `AgentIdentity`. Follow-up passes can rename the
/// fields to `agent_identity` incrementally without breaking the
/// type-level invariant.
///
/// Existing call sites that used `MeerkatId::from(s)` still work
/// (forwarded to `AgentIdentity::from(s)`). Existing
/// `impl From<AgentIdentity> for MeerkatId` / `From<&AgentIdentity>`
/// impls become reflexive conversions that rustc auto-provides.
pub type MeerkatId = AgentIdentity;

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

// DELETE_ME A5 DSL-schema migration: `MeerkatId` is now a type alias
// for `AgentIdentity` (declared above the `AgentIdentity` definition
// at the top of the "identity-first" section). The previous explicit
// `From<MeerkatId> for AgentIdentity` / `From<AgentIdentity> for
// MeerkatId` bridges become reflexive `impl<T> From<T> for T`
// (auto-provided by core), so they are no longer defined here.
// Shell-hot-path borrowed conversion `MeerkatId::from(&identity)` is
// preserved because `AgentIdentity: From<&AgentIdentity>` is an impl
// we provide below (via the shared string-newtype macro's `From<&str>`
// plus `AsRef<str>`).
impl From<&AgentIdentity> for AgentIdentity {
    fn from(identity: &AgentIdentity) -> Self {
        Self::from(identity.as_str())
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
}

impl WorkSpec {
    /// Create a new work spec. Accepts anything that implements
    /// `Into<ContentInput>` — including `String` and `&str` — so existing
    /// text-only call sites upgrade without churn.
    pub fn new(content: impl Into<meerkat_core::types::ContentInput>, origin: WorkOrigin) -> Self {
        Self {
            content: content.into(),
            origin,
        }
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

    /// DELETE_ME A5 regression: identity-first hot paths and the
    /// DSL-schema migration unify under a single type.
    ///
    /// Originally `MeerkatId` and `AgentIdentity` were two distinct
    /// `string_newtype!` wrappers, and this test pinned that the
    /// shell conversion between them preserved the underlying string
    /// without semantic change. Post-A5-DSL-migration `MeerkatId` is
    /// a type alias for `AgentIdentity`, so "conversion" is now a
    /// no-op at the type level — there is only one owner of the
    /// member-identifier-string fact. The test stays to pin the
    /// invariant that `MeerkatId::from("…").as_str()` round-trips to
    /// the expected string on both the owned and borrowed shell-hot
    /// paths (`MeerkatId::from(&identity)`) and that the two names
    /// continue to refer to the same value identity.
    #[test]
    fn agent_identity_to_meerkat_id_conversion_preserves_identity_string() {
        let identity = AgentIdentity::from("singer");

        // Owned conversion (now a type-level no-op).
        let by_owned: MeerkatId = identity.clone();
        assert_eq!(by_owned.as_str(), "singer");

        // Borrowed conversion — the hot-path shape used by
        // `MobHandle::wire`, `internal_turn`, `realtime_attach`, etc.
        let by_borrow: MeerkatId = (&identity).into();
        assert_eq!(by_borrow.as_str(), "singer");

        // Round trip: MeerkatId and AgentIdentity are the same type
        // post-A5-DSL-migration, so equality compares the shared
        // newtype value.
        let back: AgentIdentity = by_owned;
        assert_eq!(back, identity);
    }

    #[test]
    fn test_existing_ids_roundtrip() {
        let mob = MobId::from("mob-a");
        let meerkat = MeerkatId::from("meerkat-a");
        let profile = ProfileName::from("lead");
        assert_eq!(
            serde_json::from_str::<MobId>(&serde_json::to_string(&mob).unwrap()).unwrap(),
            mob
        );
        assert_eq!(
            serde_json::from_str::<MeerkatId>(&serde_json::to_string(&meerkat).unwrap()).unwrap(),
            meerkat
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

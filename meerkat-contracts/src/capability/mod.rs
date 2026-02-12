//! Typed capability model for Meerkat.
//!
//! Every capability known to Meerkat is a variant of [`CapabilityId`].
//! Feature-gated crates self-register via [`inventory::submit!`] with
//! [`CapabilityRegistration`].

pub mod query;
mod registry;

pub use query::{CapabilitiesResponse, CapabilityEntry};
pub use registry::{CapabilityRegistration, build_capabilities};

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use crate::Protocol;

/// Every capability known to Meerkat. Adding a variant forces updates to
/// the registry, error mappings, and codegen templates.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    strum::EnumIter,
    strum::EnumString,
    strum::Display,
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum CapabilityId {
    Sessions,
    Streaming,
    StructuredOutput,
    Hooks,
    Builtins,
    Shell,
    Comms,
    SubAgents,
    MemoryStore,
    SessionStore,
    SessionCompaction,
    Skills,
}

/// Where a capability applies.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum CapabilityScope {
    /// Available on all protocol surfaces.
    Universal,
    /// Available only on specific protocols.
    Extension { protocols: Cow<'static, [Protocol]> },
}

/// Runtime status of a capability.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum CapabilityStatus {
    /// Compiled in, config-enabled, protocol supports it.
    Available,
    /// Compiled in but disabled by policy.
    ///
    /// `description` summarizes why — this is intentionally a human-readable
    /// string, not a config path, because the reality is multi-layered:
    /// always-compiled features like `Builtins` and `Shell` are controlled by
    /// `AgentFactory` flags, per-build `AgentBuildConfig` overrides, and
    /// `BuiltinToolConfig` policy layers (soft + enforced). A single
    /// "config_path" can't represent that resolution chain.
    DisabledByPolicy { description: Cow<'static, str> },
    /// Not compiled into this build (feature flag absent).
    NotCompiled { feature: Cow<'static, str> },
    /// This protocol surface doesn't support it.
    NotSupportedByProtocol { reason: Cow<'static, str> },
}

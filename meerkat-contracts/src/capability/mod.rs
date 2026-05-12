//! Typed capability model for Meerkat.
//!
//! Every capability known to Meerkat is a variant of [`CapabilityId`].
//! Feature-gated crates self-register via [`inventory::submit!`] with
//! [`CapabilityRegistration`].

pub mod query;
mod registry;

pub use query::{CapabilitiesResponse, CapabilityEntry};
pub use registry::{
    CapabilityRegistration, available_capabilities, build_capabilities, resolve_capabilities,
};

use std::{borrow::Cow, str::FromStr};

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
    MemoryStore,
    WorkGraph,
    SessionStore,
    SessionCompaction,
    Skills,
    McpLive,
}

/// Capability tokens that appear in mobpack manifests.
///
/// Manifests remain string-based for compatibility, but policy checks should
/// classify those strings before making allow/forbid decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MobpackCapabilityRequirement<'a> {
    raw: &'a str,
    id: MobpackCapabilityId,
}

impl<'a> MobpackCapabilityRequirement<'a> {
    pub fn parse(raw: &'a str) -> Self {
        let id = CapabilityId::from_str(raw).map_or_else(
            |_| {
                HostProcessCapabilityId::parse(raw).map_or(
                    MobpackCapabilityId::Unknown,
                    MobpackCapabilityId::HostProcess,
                )
            },
            MobpackCapabilityId::Known,
        );
        Self { raw, id }
    }

    pub fn raw(self) -> &'a str {
        self.raw
    }

    pub fn id(self) -> MobpackCapabilityId {
        self.id
    }
}

/// Typed identity for a mobpack capability requirement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MobpackCapabilityId {
    Known(CapabilityId),
    HostProcess(HostProcessCapabilityId),
    Unknown,
}

/// Host process capabilities named by existing mobpack manifests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostProcessCapabilityId {
    McpStdio,
    ProcessSpawn,
}

impl HostProcessCapabilityId {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "mcp_stdio" => Some(Self::McpStdio),
            "process_spawn" => Some(Self::ProcessSpawn),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::McpStdio => "mcp_stdio",
            Self::ProcessSpawn => "process_spawn",
        }
    }
}

/// Browser mobpack policy decision for a typed capability requirement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserMobpackCapabilityDecision {
    Allowed,
    Forbidden { capability: MobpackCapabilityId },
}

impl BrowserMobpackCapabilityDecision {
    pub fn is_forbidden(self) -> bool {
        matches!(self, Self::Forbidden { .. })
    }
}

pub fn browser_mobpack_capability_decision(
    requirement: MobpackCapabilityRequirement<'_>,
) -> BrowserMobpackCapabilityDecision {
    match requirement.id() {
        MobpackCapabilityId::Known(CapabilityId::Shell) | MobpackCapabilityId::HostProcess(_) => {
            BrowserMobpackCapabilityDecision::Forbidden {
                capability: requirement.id(),
            }
        }
        MobpackCapabilityId::Known(_) | MobpackCapabilityId::Unknown => {
            BrowserMobpackCapabilityDecision::Allowed
        }
    }
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

#[cfg(test)]
mod mobpack_policy_tests {
    use super::{
        BrowserMobpackCapabilityDecision, CapabilityId, HostProcessCapabilityId,
        MobpackCapabilityId, MobpackCapabilityRequirement, browser_mobpack_capability_decision,
    };

    #[test]
    fn mobpack_capability_requirement_classifies_known_capabilities() {
        let requirement = MobpackCapabilityRequirement::parse("comms");

        assert_eq!(
            requirement.id(),
            MobpackCapabilityId::Known(CapabilityId::Comms)
        );
        assert_eq!(requirement.raw(), "comms");
    }

    #[test]
    fn mobpack_capability_requirement_classifies_host_process_capabilities() {
        assert_eq!(
            MobpackCapabilityRequirement::parse("mcp_stdio").id(),
            MobpackCapabilityId::HostProcess(HostProcessCapabilityId::McpStdio)
        );
        assert_eq!(
            MobpackCapabilityRequirement::parse("process_spawn").id(),
            MobpackCapabilityId::HostProcess(HostProcessCapabilityId::ProcessSpawn)
        );
    }

    #[test]
    fn browser_mobpack_policy_forbids_shell_and_host_process_capabilities() {
        for raw in ["shell", "mcp_stdio", "process_spawn"] {
            assert!(
                browser_mobpack_capability_decision(MobpackCapabilityRequirement::parse(raw))
                    .is_forbidden(),
                "{raw} should be forbidden in browser mobpacks"
            );
        }
    }

    #[test]
    fn browser_mobpack_policy_allows_safe_known_and_unknown_capabilities() {
        assert_eq!(
            browser_mobpack_capability_decision(MobpackCapabilityRequirement::parse("comms")),
            BrowserMobpackCapabilityDecision::Allowed
        );
        assert_eq!(
            browser_mobpack_capability_decision(MobpackCapabilityRequirement::parse(
                "vendor.custom"
            )),
            BrowserMobpackCapabilityDecision::Allowed
        );
    }
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

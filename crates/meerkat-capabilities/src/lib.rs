//! Feature-owned capability declarations and registry for Meerkat.

use std::{borrow::Cow, str::FromStr};

use meerkat_core::Config;
use serde::{Deserialize, Serialize};

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
    Schedule,
    WorkGraph,
    SessionStore,
    SessionCompaction,
    Skills,
    McpLive,
    /// Adaptive mobpack flow execution (FlowMaster planning loop, layer
    /// compilation, policy composition). Stamped into a mobpack's
    /// `[requires]` section by the pack builder whenever the manifest
    /// declares an `[adaptive]` section, so hosts that do not know this
    /// capability fail closed instead of silently downgrading the pack.
    AdaptiveFlow,
}

/// A mobpack manifest capability token paired with its typed classification.
///
/// This is the parse-once classifier used at the manifest boundary: a raw
/// token is classified into a [`MobpackCapabilityId`] exactly once, and policy
/// decisions take the typed id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MobpackCapabilityRequirement<'a> {
    raw: &'a str,
    id: MobpackCapabilityId,
}

impl<'a> MobpackCapabilityRequirement<'a> {
    pub fn parse(raw: &'a str) -> Self {
        let id = CapabilityId::from_str(raw).map_or_else(
            |_| {
                HostProcessCapabilityId::parse(raw)
                    .map(MobpackCapabilityId::HostProcess)
                    .or_else(|| {
                        DeploySurfaceCapabilityId::parse(raw)
                            .map(MobpackCapabilityId::DeploySurface)
                    })
                    .unwrap_or(MobpackCapabilityId::Unknown)
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
    DeploySurface(DeploySurfaceCapabilityId),
    Unknown,
}

/// Whether a typed mobpack capability requirement is known to this host
/// build's capability vocabulary.
///
/// This is the fail-closed knowledge gate used at mobpack load: a pack that
/// requires a capability this build cannot even name (a future or vendor
/// token classifying as [`MobpackCapabilityId::Unknown`]) must be rejected
/// rather than silently downgraded. Satisfaction against the *runtime*
/// capability set of a concrete deploy surface is a separate, stricter check
/// owned by the deploying surface.
///
/// The match is exhaustive on purpose: adding a new requirement family to
/// [`MobpackCapabilityId`] forces an explicit decision here.
pub fn mobpack_capability_known_to_host(capability: MobpackCapabilityId) -> bool {
    match capability {
        MobpackCapabilityId::Known(_)
        | MobpackCapabilityId::HostProcess(_)
        | MobpackCapabilityId::DeploySurface(_) => true,
        MobpackCapabilityId::Unknown => false,
    }
}

/// Every mobpack capability token known to this host build, for diagnostics
/// when a pack requires a capability outside the vocabulary.
///
/// Driven by the enum iterators so a new variant in any of the three
/// requirement families is included automatically (its token spelling is
/// already forced by the exhaustive `Display`/`as_str` matches).
pub fn known_mobpack_capability_tokens() -> Vec<String> {
    let mut tokens: Vec<String> = <CapabilityId as strum::IntoEnumIterator>::iter()
        .map(|id| id.to_string())
        .collect();
    tokens.extend(
        <HostProcessCapabilityId as strum::IntoEnumIterator>::iter()
            .map(|id| id.as_str().to_string()),
    );
    tokens.extend(
        <DeploySurfaceCapabilityId as strum::IntoEnumIterator>::iter()
            .map(|id| id.as_str().to_string()),
    );
    tokens
}

/// Deploy-surface capabilities named by mobpack manifests.
///
/// These name the runtime surface a deployed mob is hosted on (`core`,
/// `mcp`, `rpc`), distinct from the feature-capability vocabulary in
/// [`CapabilityId`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumIter)]
pub enum DeploySurfaceCapabilityId {
    Core,
    Mcp,
    Rpc,
}

impl DeploySurfaceCapabilityId {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "core" => Some(Self::Core),
            "mcp" => Some(Self::Mcp),
            "rpc" => Some(Self::Rpc),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::Mcp => "mcp",
            Self::Rpc => "rpc",
        }
    }
}

/// Host process capabilities named by existing mobpack manifests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumIter)]
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
    capability: MobpackCapabilityId,
) -> BrowserMobpackCapabilityDecision {
    match capability {
        MobpackCapabilityId::Known(CapabilityId::Shell) | MobpackCapabilityId::HostProcess(_) => {
            BrowserMobpackCapabilityDecision::Forbidden { capability }
        }
        MobpackCapabilityId::Known(_)
        | MobpackCapabilityId::DeploySurface(_)
        | MobpackCapabilityId::Unknown => BrowserMobpackCapabilityDecision::Allowed,
    }
}

/// Protocol surfaces used only for capability declaration metadata.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    strum::EnumString,
    strum::Display,
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum CapabilityProtocol {
    Rpc,
    Rest,
    Mcp,
    Cli,
}

/// Where a capability applies.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum CapabilityScope {
    /// Available on all protocol surfaces.
    Universal,
    /// Available only on specific protocols.
    Extension {
        protocols: Cow<'static, [CapabilityProtocol]>,
    },
}

/// Runtime status of a capability.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum CapabilityStatus {
    /// Compiled in, config-enabled, protocol supports it.
    Available,
    /// Compiled in but disabled by policy.
    DisabledByPolicy { description: Cow<'static, str> },
    /// Not compiled into this build (feature flag absent).
    NotCompiled { feature: Cow<'static, str> },
    /// This protocol surface doesn't support it.
    NotSupportedByProtocol { reason: Cow<'static, str> },
}

#[derive(Clone, Copy)]
pub struct FeatureCapabilityPolicy {
    enabled: fn(&Config) -> bool,
    disabled_description: &'static str,
}

impl FeatureCapabilityPolicy {
    pub const fn new(enabled: fn(&Config) -> bool, disabled_description: &'static str) -> Self {
        Self {
            enabled,
            disabled_description,
        }
    }

    pub fn is_enabled(self, config: &Config) -> bool {
        (self.enabled)(config)
    }

    pub const fn disabled_description(self) -> &'static str {
        self.disabled_description
    }
}

/// Self-registration entry for a capability.
///
/// Feature crates submit these via `inventory::submit!`.
pub struct CapabilityRegistration {
    pub id: CapabilityId,
    pub description: &'static str,
    pub scope: CapabilityScope,
    pub requires_feature: Option<&'static str>,
    pub prerequisites: &'static [CapabilityId],
    pub status_resolver: Option<fn(&Config) -> CapabilityStatus>,
}

inventory::collect!(CapabilityRegistration);

// Always-present capabilities (no feature gate, always compiled)
inventory::submit! {
    CapabilityRegistration {
        id: CapabilityId::Sessions,
        description: "Agent loop and session lifecycle",
        scope: CapabilityScope::Universal,
        requires_feature: None,
        prerequisites: &[],
        status_resolver: None,
    }
}

inventory::submit! {
    CapabilityRegistration {
        id: CapabilityId::Streaming,
        description: "Event streaming during agent execution",
        scope: CapabilityScope::Universal,
        requires_feature: None,
        prerequisites: &[],
        status_resolver: None,
    }
}

inventory::submit! {
    CapabilityRegistration {
        id: CapabilityId::StructuredOutput,
        description: "Schema-validated JSON output extraction",
        scope: CapabilityScope::Universal,
        requires_feature: None,
        prerequisites: &[],
        status_resolver: None,
    }
}

/// Collect all registered capabilities, sorted by [`CapabilityId`] ordinal
/// for deterministic ordering regardless of `inventory` collection order.
pub fn build_capabilities() -> Vec<&'static CapabilityRegistration> {
    let mut caps: Vec<&'static CapabilityRegistration> = inventory::iter::<CapabilityRegistration>
        .into_iter()
        .collect();
    caps.sort_by_key(|r| r.id);
    caps
}

/// Resolve runtime status for every registered capability against the current
/// config. This is the single config-aware capability truth used by both
/// surface reporting and skill filtering.
pub fn resolve_capabilities(
    config: &Config,
) -> Vec<(&'static CapabilityRegistration, CapabilityStatus)> {
    build_capabilities()
        .into_iter()
        .map(|reg| {
            let status = match reg.status_resolver {
                Some(resolver) => resolver(config),
                None => CapabilityStatus::Available,
            };
            (reg, status)
        })
        .collect()
}

/// Return the capability ids that are effectively available after config-level
/// status resolution has been applied.
pub fn available_capabilities(config: &Config) -> Vec<CapabilityId> {
    resolve_capabilities(config)
        .into_iter()
        .filter_map(|(reg, status)| matches!(status, CapabilityStatus::Available).then_some(reg.id))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::Config;

    #[test]
    fn test_build_capabilities_finds_registered() {
        let caps = build_capabilities();
        assert!(
            caps.iter().any(|c| c.id == CapabilityId::Sessions),
            "Should find the test-registered Sessions capability"
        );
    }

    #[test]
    fn test_build_capabilities_sorted() {
        let caps = build_capabilities();
        if caps.len() >= 2 {
            for window in caps.windows(2) {
                assert!(
                    window[0].id <= window[1].id,
                    "Capabilities should be sorted by ordinal"
                );
            }
        }
    }

    #[test]
    fn available_capabilities_always_include_unconditional_entries() {
        let config = Config::default();
        let caps = available_capabilities(&config);
        assert!(caps.contains(&CapabilityId::Sessions));
        assert!(caps.contains(&CapabilityId::Streaming));
        assert!(caps.contains(&CapabilityId::StructuredOutput));
    }

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
                browser_mobpack_capability_decision(MobpackCapabilityRequirement::parse(raw).id())
                    .is_forbidden(),
                "{raw} should be forbidden in browser mobpacks"
            );
        }
    }

    #[test]
    fn adaptive_flow_classifies_as_known_capability() {
        let requirement = MobpackCapabilityRequirement::parse("adaptive_flow");
        assert_eq!(
            requirement.id(),
            MobpackCapabilityId::Known(CapabilityId::AdaptiveFlow)
        );
        assert_eq!(CapabilityId::AdaptiveFlow.to_string(), "adaptive_flow");
    }

    #[test]
    fn host_knows_every_typed_capability_and_rejects_unknown() {
        for raw in ["comms", "adaptive_flow", "mcp_stdio", "core"] {
            assert!(
                mobpack_capability_known_to_host(MobpackCapabilityRequirement::parse(raw).id()),
                "{raw} must be known to this host build"
            );
        }
        assert!(!mobpack_capability_known_to_host(
            MobpackCapabilityRequirement::parse("capability-from-the-future").id()
        ));
    }

    #[test]
    fn known_tokens_cover_all_requirement_families_and_round_trip() {
        let tokens = known_mobpack_capability_tokens();
        for expected in [
            "sessions",
            "adaptive_flow",
            "mcp_stdio",
            "process_spawn",
            "core",
        ] {
            assert!(
                tokens.iter().any(|t| t == expected),
                "known token set must contain {expected}: {tokens:?}"
            );
        }
        // Every advertised token must classify back into a known typed id.
        for token in &tokens {
            assert!(
                mobpack_capability_known_to_host(MobpackCapabilityRequirement::parse(token).id()),
                "advertised token {token} must round-trip as known"
            );
        }
    }

    #[test]
    fn browser_mobpack_policy_allows_safe_known_and_unknown_capabilities() {
        assert_eq!(
            browser_mobpack_capability_decision(MobpackCapabilityRequirement::parse("comms").id()),
            BrowserMobpackCapabilityDecision::Allowed
        );
        assert_eq!(
            browser_mobpack_capability_decision(
                MobpackCapabilityRequirement::parse("vendor.custom").id()
            ),
            BrowserMobpackCapabilityDecision::Allowed
        );
    }
}

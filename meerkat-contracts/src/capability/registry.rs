//! Distributed capability registration via `inventory`.

use super::{CapabilityId, CapabilityScope, CapabilityStatus};
use meerkat_core::Config;

/// Self-registration entry for a capability.
///
/// Feature-gated crates submit these via `inventory::submit!`.
/// Adding a capability = add enum variant + `inventory::submit!` in the providing crate.
///
/// Each registration can optionally provide a `status_resolver` function
/// that checks config/policy to determine whether the capability is actually
/// available at runtime (vs. merely compiled in). This keeps policy knowledge
/// in the crate that owns the feature.
pub struct CapabilityRegistration {
    pub id: CapabilityId,
    pub description: &'static str,
    pub scope: CapabilityScope,
    pub requires_feature: Option<&'static str>,
    pub prerequisites: &'static [CapabilityId],
    /// Optional config-based status resolver. When `None`, capability is
    /// reported as `Available` if compiled in. When `Some`, the function
    /// is called with the runtime config to determine actual status.
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

inventory::submit! {
    CapabilityRegistration {
        id: CapabilityId::WorkGraph,
        description: "Realm-scoped dependency-aware durable work graph",
        scope: CapabilityScope::Universal,
        requires_feature: None,
        prerequisites: &[],
        status_resolver: Some(|config| {
            if config.tools.workgraph_enabled {
                CapabilityStatus::Available
            } else {
                CapabilityStatus::DisabledByPolicy {
                    description: "config.tools.workgraph_enabled is false".into(),
                }
            }
        }),
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
}

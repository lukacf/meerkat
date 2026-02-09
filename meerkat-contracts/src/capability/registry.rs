//! Distributed capability registration via `inventory`.

use super::{CapabilityId, CapabilityScope};

/// Self-registration entry for a capability.
///
/// Feature-gated crates submit these via `inventory::submit!`.
/// Adding a capability = add enum variant + `inventory::submit!` in the providing crate.
pub struct CapabilityRegistration {
    pub id: CapabilityId,
    pub description: &'static str,
    pub scope: CapabilityScope,
    pub requires_feature: Option<&'static str>,
    pub prerequisites: &'static [CapabilityId],
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
    }
}

inventory::submit! {
    CapabilityRegistration {
        id: CapabilityId::Streaming,
        description: "Event streaming during agent execution",
        scope: CapabilityScope::Universal,
        requires_feature: None,
        prerequisites: &[],
    }
}

inventory::submit! {
    CapabilityRegistration {
        id: CapabilityId::StructuredOutput,
        description: "Schema-validated JSON output extraction",
        scope: CapabilityScope::Universal,
        requires_feature: None,
        prerequisites: &[],
    }
}

/// Collect all registered capabilities, sorted by [`CapabilityId`] ordinal
/// for deterministic ordering regardless of `inventory` collection order.
pub fn build_capabilities() -> Vec<&'static CapabilityRegistration> {
    let mut caps: Vec<&'static CapabilityRegistration> =
        inventory::iter::<CapabilityRegistration>.into_iter().collect();
    caps.sort_by_key(|r| r.id);
    caps
}

#[cfg(test)]
mod tests {
    use super::*;

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
}

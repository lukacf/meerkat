#[cfg(any(feature = "session-store", feature = "session-compaction"))]
use meerkat::{Config, surface::build_capabilities_response};
#[cfg(any(feature = "session-store", feature = "session-compaction"))]
use meerkat_contracts::CapabilityId;

#[cfg(any(feature = "session-store", feature = "session-compaction"))]
fn capability_ids(config: &Config) -> Vec<CapabilityId> {
    build_capabilities_response(config)
        .capabilities
        .into_iter()
        .map(|capability| capability.id)
        .collect()
}

#[cfg(feature = "session-store")]
#[test]
fn session_capability_contract_session_store_is_advertised_without_skills() {
    let ids = capability_ids(&Config::default());
    assert!(
        ids.contains(&CapabilityId::SessionStore),
        "session_store capability should remain visible when session-store is enabled without skills"
    );
}

#[cfg(feature = "session-compaction")]
#[test]
fn session_capability_contract_session_compaction_is_advertised_without_skills() {
    let ids = capability_ids(&Config::default());
    assert!(
        ids.contains(&CapabilityId::SessionCompaction),
        "session_compaction capability should remain visible when session-compaction is enabled without skills"
    );
}

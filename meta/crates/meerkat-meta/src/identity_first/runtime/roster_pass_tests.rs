//! Rows for the roster pass over fake Core members: one pass answers each
//! identity on its own, and one identity's failure never fails the others.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::Arc;

use meerkat_core::types::SessionId;

use crate::identity_first::core_test_bridge::{CoreReadsBridge, register_live};
use crate::identity_first::local_lease::LocalLeaseProvider;
use crate::identity_first::local_store::LocalContinuityStore;
use crate::identity_first::{
    AgentAddressability, AgentIdentity, DurabilityPolicy, DurableAgentSpec,
    IdentityFirstRuntimeContext, IdentityRuntime, IdentityRuntimeConfig, MutableRosterProvider,
};

fn id(name: &str) -> AgentIdentity {
    AgentIdentity::parse(name).expect("valid identity")
}

fn spec(identity: &AgentIdentity) -> DurableAgentSpec {
    DurableAgentSpec {
        identity: identity.clone(),
        profile: "worker".into(),
        addressability: AgentAddressability::Addressable,
        display_name: None,
        labels: BTreeMap::new(),
        context: None,
        additional_instructions: Vec::new(),
        initial_message: None,
        runtime_mode_override: Some(meerkat_mob::MobRuntimeMode::TurnDriven),
        backend: None,
        binding: None,
        placement: None,
    }
}

/// An identity runtime over `bridge`'s fake Core members.
fn runtime_over(bridge: &Arc<CoreReadsBridge>, instance: &str) -> Arc<IdentityRuntime> {
    Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
        continuity_store: Arc::new(LocalContinuityStore::in_memory().expect("continuity store")),
        lease_provider: Arc::new(LocalLeaseProvider::new()),
        runtime_instance_id: instance.to_string(),
        has_runtime_store: true,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge: Some(bridge.clone()),
        default_timeout: None,
    }))
}

/// The roster no longer lists two of three live identities, and Core fails
/// the retirement of one of them. The pass answers each identity on its
/// own: it does not fail, the other unlisted identity's member is retired,
/// the listed identity's member is untouched, and the member Core failed to
/// retire stays live, so a later pass retires it again. The pass used to
/// fail as a whole at the first retirement that failed.
#[tokio::test]
async fn an_unlisted_retire_failure_is_typed_and_per_identity() {
    let bridge = Arc::new(CoreReadsBridge::default());
    bridge.core.keep_journal();
    let identities = runtime_over(&bridge, "roster-pass-unlisted-failure");
    let kept = id("roster-pass-kept");
    let dropped = id("roster-pass-dropped");
    let refused = id("roster-pass-refused");
    for identity in [&kept, &dropped, &refused] {
        register_live(
            &identities,
            &bridge.core,
            spec(identity),
            1,
            SessionId::new(),
        )
        .await
        .expect("a live registered member");
    }
    bridge.refuse_retire_of(&refused);
    let context = IdentityFirstRuntimeContext::new(
        Arc::clone(&identities),
        Arc::new(MutableRosterProvider::new(vec![spec(&kept)])),
        None,
        None,
        None,
    );

    let pass = context.refresh_desired_topology().await;
    assert!(
        pass.is_ok(),
        "one identity's failed unlisted retirement must never fail the pass: {pass:?}"
    );
    assert!(
        bridge
            .core
            .live_member(&dropped)
            .expect("Core's member read")
            .is_none(),
        "the pass retires the other unlisted identity's member"
    );
    assert!(
        bridge
            .core
            .live_member(&refused)
            .expect("Core's member read")
            .is_some(),
        "the member Core failed to retire stays live for a later pass"
    );
    assert!(
        bridge
            .core
            .live_member(&kept)
            .expect("Core's member read")
            .is_some(),
        "the listed identity's member is untouched"
    );
}

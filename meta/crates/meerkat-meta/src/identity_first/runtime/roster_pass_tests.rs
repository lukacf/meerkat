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

/// A roster pass reads a row of the mob's identity store only for an
/// identity it has not registered yet: the first pass over three identities
/// whose record is their row reads three rows and registers them, a second
/// pass reads none, and a pass after a fourth one appears reads one more.
/// The pass tracks them in the bootstrap status beside the roster's
/// identity, and never retires one.
///
/// RED before the pass read Core's entry pages: every pass read every row,
/// and the bootstrap status tracked the roster's identities only.
#[tokio::test]
async fn a_roster_pass_reads_a_row_only_for_an_identity_it_has_not_registered() {
    use std::sync::atomic::Ordering;

    let bridge = Arc::new(CoreReadsBridge::default());
    let identities = runtime_over(&bridge, "roster-pass-row-reads");
    let listed = id("roster-pass-listed");
    register_live(
        &identities,
        &bridge.core,
        spec(&listed),
        1,
        SessionId::new(),
    )
    .await
    .expect("a live registered member");
    let rows = [id("row-one"), id("row-two"), id("row-three")];
    let hold = |name: &AgentIdentity| {
        crate::identity_first::core_test_bridge::bind_live(
            &bridge.core,
            name,
            &meerkat_mob::ProfileName::from("worker"),
            1,
            Some(SessionId::new()),
        )
        .expect("Core seats the identity");
        bridge.hold_created(name, "worker");
    };
    for name in &rows {
        hold(name);
    }
    let context = IdentityFirstRuntimeContext::new(
        Arc::clone(&identities),
        Arc::new(MutableRosterProvider::new(vec![spec(&listed)])),
        None,
        None,
        None,
    );

    context.refresh_desired_topology().await.expect("the pass");
    assert_eq!(
        bridge.row_reads.load(Ordering::SeqCst),
        3,
        "the first pass reads one row per identity it has not registered"
    );
    let tracked = identities.identity_bootstrap_status();
    for name in rows.iter().chain([&listed]) {
        assert!(identities.contains(name).await, "{name} is served");
        assert!(
            tracked.identities.contains_key(name),
            "{name} is tracked by the bootstrap status: {tracked:?}"
        );
    }

    context.refresh_desired_topology().await.expect("the pass");
    assert_eq!(
        bridge.row_reads.load(Ordering::SeqCst),
        3,
        "a pass over identities it serves already reads no row"
    );

    let fourth = id("row-four");
    hold(&fourth);
    context.refresh_desired_topology().await.expect("the pass");
    assert_eq!(
        bridge.row_reads.load(Ordering::SeqCst),
        4,
        "a pass reads the row of the identity that appeared since"
    );
    assert!(identities.contains(&fourth).await);
    for name in rows.iter().chain([&fourth]) {
        assert!(
            bridge
                .core
                .live_member(name)
                .expect("Core's member read")
                .is_some(),
            "{name}: no pass retired the identity the roster does not list"
        );
    }
}

/// The registration follows a row after a change of it committed in Core,
/// with no roster pass and no door: a new row is registered with one row
/// read, and a name Core holds for no identity registers nothing.
#[tokio::test]
async fn the_registration_follows_a_row_after_its_change() {
    use std::sync::atomic::Ordering;

    let bridge = Arc::new(CoreReadsBridge::default());
    let identities = runtime_over(&bridge, "registration-follows-row");
    let name = id("row-changed");
    crate::identity_first::core_test_bridge::bind_live(
        &bridge.core,
        &name,
        &meerkat_mob::ProfileName::from("worker"),
        1,
        Some(SessionId::new()),
    )
    .expect("Core seats the identity");
    bridge.hold_created(&name, "worker");

    assert!(
        identities
            .register_from_row(&name)
            .await
            .expect("the row reads"),
        "the new row is registered"
    );
    assert!(identities.contains(&name).await);
    assert_eq!(bridge.row_reads.load(Ordering::SeqCst), 1);

    let unknown = id("row-of-no-identity");
    assert!(
        !identities
            .register_from_row(&unknown)
            .await
            .expect("Core's entry reads"),
        "a name Core holds for no identity registers nothing"
    );
    assert!(!identities.contains(&unknown).await);
}

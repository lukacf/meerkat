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

/// A cold start reads the rows of the identities whose record is their row
/// in pages, never one store read per identity: the first roster pass over
/// K such identities makes a number of row reads that does not grow with K
/// beyond pages, and registers every one. Before any door opened, each
/// identity's status and registered spec answer exactly what they answer
/// for an identity registered through its own row read. A second pass reads
/// no row, and a pass after one more identity appeared reads one more page.
/// The pass tracks them in the bootstrap status beside the roster's
/// identity, and never retires one.
///
/// RED before the pass read the rows in pages: the first pass made one row
/// read per identity.
#[tokio::test]
async fn a_cold_start_reads_the_rows_in_pages_and_answers_as_a_row_read_does() {
    use std::sync::atomic::Ordering;

    let count = meerkat_mob::IDENTITY_ENTRY_PAGE_MAX + 44;
    let bridge = Arc::new(CoreReadsBridge::default());
    let identities = runtime_over(&bridge, "roster-pass-row-pages");
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
    let rows = (0..count)
        .map(|index| id(&format!("row-{index:03}")))
        .collect::<Vec<_>>();
    let hold = |bridge: &CoreReadsBridge, name: &AgentIdentity| {
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
        hold(&bridge, name);
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
        (
            bridge.row_reads.load(Ordering::SeqCst),
            bridge.row_page_reads.load(Ordering::SeqCst)
        ),
        (0, 2),
        "the first pass over {count} identities reads their rows in two pages, and no row by \
         itself"
    );
    let tracked = identities.identity_bootstrap_status();
    for name in rows.iter().chain([&listed]) {
        assert!(identities.contains(name).await, "{name} is served");
        assert!(
            tracked.identities.contains_key(name),
            "{name} is tracked by the bootstrap status"
        );
    }

    // Before any door opened, the answers are those of an identity
    // registered through its own row read.
    let twin_bridge = Arc::new(CoreReadsBridge::default());
    let twin = runtime_over(&twin_bridge, "roster-pass-row-pages-twin");
    for name in [&rows[0], &rows[count - 1]] {
        hold(&twin_bridge, name);
        assert!(
            twin.register_from_row(name)
                .await
                .expect("the twin's row reads")
        );
        let (paged, single) = (
            identities
                .registered_spec(name)
                .await
                .expect("registered by the pass"),
            twin.registered_spec(name)
                .await
                .expect("registered by its row read"),
        );
        assert_eq!(
            (paged.spec, paged.spec_digest),
            (single.spec, single.spec_digest),
            "{name}: the registered spec is the one a row read registers"
        );
        let (paged, single) = (
            identities.status(name).await.expect("the status reads"),
            twin.status(name).await.expect("the twin's status reads"),
        );
        assert_eq!(
            (paged.profile, paged.display_name, paged.labels),
            (single.profile, single.display_name, single.labels),
            "{name}: the status answers what a row read's registration answers"
        );
    }

    context.refresh_desired_topology().await.expect("the pass");
    assert_eq!(
        (
            bridge.row_reads.load(Ordering::SeqCst),
            bridge.row_page_reads.load(Ordering::SeqCst)
        ),
        (0, 2),
        "a pass over identities it serves already reads no row"
    );

    let late = id("row-late");
    hold(&bridge, &late);
    context.refresh_desired_topology().await.expect("the pass");
    assert_eq!(
        (
            bridge.row_reads.load(Ordering::SeqCst),
            bridge.row_page_reads.load(Ordering::SeqCst)
        ),
        (0, 3),
        "a pass reads one page for the identity that appeared since"
    );
    assert!(identities.contains(&late).await);
    for name in [&rows[0], &rows[count - 1], &late] {
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

    // An update: the row's spec replaces the registered one, with one more
    // row read.
    bridge.hold_created(&name, "analyst");
    assert!(
        identities
            .register_from_row(&name)
            .await
            .expect("the row reads"),
        "the updated row is served"
    );
    assert_eq!(
        registered_profile(&identities, &name).await,
        Some("analyst".to_string()),
        "the registration follows the row's update"
    );
    assert_eq!(bridge.row_reads.load(Ordering::SeqCst), 2);

    // A removal: the registration is gone, and the seam answers that the
    // runtime no longer serves the name, not an error.
    bridge.remove_created(&name);
    assert!(
        !identities
            .register_from_row(&name)
            .await
            .expect("a committed removal is no error"),
        "the removed identity is no longer served"
    );
    assert!(!identities.contains(&name).await);
}

/// The profile of `name`'s registration, if it is registered.
async fn registered_profile(identities: &IdentityRuntime, name: &AgentIdentity) -> Option<String> {
    identities
        .registered_spec(name)
        .await
        .ok()
        .map(|registered| registered.spec.profile.as_str().to_string())
}

/// After an adoption into the store the row is the identity's one owner,
/// also while the host's roster still lists its name: the registration the
/// roster made gives way to the row's spec when the change path follows the
/// row, and a roster pass that still lists the name keeps the row's.
#[tokio::test]
async fn after_an_adoption_the_row_is_registered_and_the_rosters_listing_gives_way() {
    let bridge = Arc::new(CoreReadsBridge::default());
    let identities = runtime_over(&bridge, "adopted-row-wins");
    let name = id("adopted");
    register_live(&identities, &bridge.core, spec(&name), 1, SessionId::new())
        .await
        .expect("the roster's identity is registered and live");
    assert_eq!(
        registered_profile(&identities, &name).await,
        Some(spec(&name).profile.as_str().to_string()),
        "registered from the roster"
    );

    // The host adopts it into the store, and its row then moves on.
    bridge.hold_created(&name, "analyst");
    assert!(
        identities
            .register_from_row(&name)
            .await
            .expect("the row reads"),
        "the adopted identity is served"
    );
    assert_eq!(
        registered_profile(&identities, &name).await,
        Some("analyst".to_string()),
        "the row's spec is the registration, though the roster registered the name"
    );
    assert_eq!(
        identities
            .roster_without_row_identities(&[spec(&name)])
            .await
            .len(),
        0,
        "a pass no longer registers or declares the roster's listing of it"
    );
    assert_eq!(
        identities
            .tracked_row_identities()
            .await
            .iter()
            .map(|spec| spec.identity.clone())
            .collect::<Vec<_>>(),
        vec![name.clone()],
        "and tracks it from its row"
    );
}

/// A roster's listing of a name its row serves is marked on the
/// registration and noted once: the row's registration that replaces the
/// roster's carries the mark at once (an adoption between two passes), a
/// pass whose roster lists the name keeps it, a pass whose roster no longer
/// lists it clears it, and an identity the roster never listed carries
/// none. The log names each such identity once per process, however many
/// passes list it.
#[tokio::test]
async fn a_roster_listing_its_row_supersedes_is_marked_and_noted_once() {
    let bridge = Arc::new(CoreReadsBridge::default());
    let identities = runtime_over(&bridge, "listing-superseded");
    let adopted = id("adopted");
    let unlisted = id("created-unlisted");
    register_live(
        &identities,
        &bridge.core,
        spec(&adopted),
        1,
        SessionId::new(),
    )
    .await
    .expect("the roster's identity is registered and live");
    assert!(
        !identities.roster_listing_superseded(&adopted).await,
        "a configured identity's listing is applied"
    );

    // The host adopts it into the store between two passes: the roster's
    // registration gives way, and its listing is superseded from now.
    bridge.hold_created(&adopted, "analyst");
    assert!(
        identities
            .register_from_row(&adopted)
            .await
            .expect("the row reads")
    );
    assert!(
        identities.roster_listing_superseded(&adopted).await,
        "the roster listed the name its row serves now"
    );
    // The row moves on: its own registration keeps the mark.
    bridge.hold_created(&adopted, "reviewer");
    assert!(
        identities
            .register_from_row(&adopted)
            .await
            .expect("the row reads")
    );
    assert!(identities.roster_listing_superseded(&adopted).await);

    // An identity created under a name the roster never listed.
    bridge.hold_created(&unlisted, "analyst");
    assert!(
        identities
            .register_from_row(&unlisted)
            .await
            .expect("the row reads")
    );
    assert!(!identities.roster_listing_superseded(&unlisted).await);

    // A pass whose roster lists the name: marked, and noted.
    for _ in 0..2 {
        assert!(
            identities
                .roster_without_row_identities(&[spec(&adopted)])
                .await
                .is_empty(),
            "the pass leaves the roster's listing out"
        );
        assert!(identities.roster_listing_superseded(&adopted).await);
        assert!(!identities.roster_listing_superseded(&unlisted).await);
    }
    assert!(
        !identities.first_note_of_superseded_listing(&adopted),
        "the passes noted the name: a later note is not its first"
    );
    assert!(
        identities.first_note_of_superseded_listing(&unlisted),
        "no pass noted an identity the roster does not list"
    );

    // A pass whose roster no longer lists it: the mark is cleared.
    assert!(
        identities
            .roster_without_row_identities(&[])
            .await
            .is_empty()
    );
    assert!(
        !identities.roster_listing_superseded(&adopted).await,
        "a roster that no longer lists the name supersedes nothing"
    );
}

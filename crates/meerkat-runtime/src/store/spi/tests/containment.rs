//! Containing one conflicted host store scope (CONTAINMENT-NOTE r7).
//!
//! A store that meets obligation 4 at load answers a typed conflict for a
//! scope two batches hold one version of (a superseded writer's late batch,
//! folded at that version). Such a scope is contained: the open quarantines
//! it and opens the rest, every call that needs it refuses typed with the
//! conflict and the repair in its cause, no listing leaves it out, and a
//! re-acquire whose listing shows only the recorded conflicts keeps the
//! realm.

#![allow(clippy::panic)]

use meerkat_core::host_store::ScopeVersion;

use super::*;

/// The cause a quarantined scope's refusal carries, in part.
const QUARANTINED: &str = "conflicted in the store at version";
/// What lifts the refusal, as the cause says it.
const REPAIR: &str = "repair the scope in the store, then reopen the realm";

/// `result` is the typed refusal of the quarantined scope `scope`
/// (`<tag>:<key>`), whose cause names the conflict, the repair and every
/// name in `names`.
fn assert_quarantined<T: std::fmt::Debug>(
    result: Result<T, RuntimeStoreError>,
    scope: &str,
    names: &[&str],
) {
    match result {
        Err(RuntimeStoreError::ScopeRefused { scopes }) => {
            let refused = scopes
                .iter()
                .find(|refused| refused.scope == scope)
                .unwrap_or_else(|| panic!("the refusal names {scope}: {scopes:?}"));
            assert!(
                refused.cause.contains(QUARANTINED) && refused.cause.contains(REPAIR),
                "the cause names the conflict and the repair: {}",
                refused.cause
            );
            for name in names {
                assert!(
                    refused.cause.contains(name),
                    "the cause names {name}: {}",
                    refused.cause
                );
            }
        }
        other => panic!("{scope} refuses typed as quarantined: {other:?}"),
    }
}

/// Persist an accepted input for each runtime, so each has a runtime scope
/// that recovery loads at the next open.
async fn accept_inputs(store: &SpiRuntimeStore, runtimes: &[&LogicalRuntimeId]) {
    for runtime in runtimes {
        store
            .persist_input_state(
                runtime,
                &persistable(StoredInputState::new_accepted(InputId::new())),
            )
            .await
            .expect("the input is written");
    }
}

/// Open a realm and its runtime store over `host` once the last writer's
/// lease ended, each failure named.
async fn open_contained(host: &Arc<TestHost>) -> (Arc<SpiRealm>, SpiRuntimeStore) {
    let realm = SpiRealm::open(Arc::clone(host) as _, SpiRealmConfig::default())
        .await
        .expect("the realm opens: a conflicted scope is contained, not a failed open");
    let store = SpiRuntimeStore::open(Arc::clone(&realm))
        .await
        .expect("the runtime store opens beside the contained scope");
    (realm, store)
}

/// The visible incarnation of mob `mob_id`'s host binding scope: its name and
/// version.
fn binding_incarnation(host: &TestHost, mob_id: &str) -> (String, ScopeVersion) {
    let prefix = format!("mhb:{}~", codec::key_part(mob_id));
    let scopes = host.scopes_of(ScopeKind::Record);
    let (name, stored) = scopes
        .iter()
        .find(|(name, _)| name.starts_with(&prefix))
        .expect("the binding scope is visible");
    (name.clone(), stored.version)
}

// One runtime's scope loads as a typed conflict: the open quarantines it and
// opens the rest. Every other runtime serves, and every call on the
// quarantined one, a read or a write, refuses typed with the conflict and
// the repair in its cause; the realm keeps its fence. RED before
// containment: the listing's page fails on the conflict and so does the
// open.
#[tokio::test]
async fn an_open_contains_a_conflicted_runtime_and_serves_the_others() {
    let host = Arc::new(TestHost::new());
    let kept = LogicalRuntimeId::new("kept");
    let conflicted = LogicalRuntimeId::new("conflicted");
    {
        let (_realm, store) = open(&host).await;
        accept_inputs(&store, &[&kept, &conflicted]).await;
    }
    host.expire_lease();
    let name = host
        .conflict_scope(ScopeKind::Session, "rt:rconflicted~")
        .expect("one conflicted scope");

    let (realm, store) = open_contained(&host).await;
    assert_eq!(
        store
            .load_input_states(&kept)
            .await
            .expect("another runtime serves")
            .len(),
        1
    );
    assert_quarantined(
        store.load_input_states(&conflicted).await,
        "rt:conflicted",
        &[&name],
    );
    assert_quarantined(
        store
            .persist_input_state(
                &conflicted,
                &persistable(StoredInputState::new_accepted(InputId::new())),
            )
            .await,
        "rt:conflicted",
        &[&name],
    );
    assert!(
        realm.realm_poisoned().is_none(),
        "the realm keeps its fence: {:?}",
        realm.realm_poisoned()
    );
}

// A runtime that recovery must load meets a typed conflict when the open
// loads its body (the conflict arose after the listing): that runtime is
// quarantined, and every other runtime serves. RED before containment: the
// first failed read failed the runtime store's open.
#[tokio::test]
async fn an_open_contains_a_runtime_whose_recovery_load_answers_a_conflict() {
    let host = Arc::new(TestHost::new());
    let kept = LogicalRuntimeId::new("kept");
    let conflicted = LogicalRuntimeId::new("conflicted");
    {
        let (_realm, store) = open(&host).await;
        accept_inputs(&store, &[&kept, &conflicted]).await;
    }
    host.expire_lease();
    let name = host
        .conflict_scope_after_listing(ScopeKind::Session, "rt:rconflicted~")
        .expect("one conflicted scope");

    let (realm, store) = open_contained(&host).await;
    assert_eq!(
        store
            .load_input_states(&kept)
            .await
            .expect("another runtime serves")
            .len(),
        1
    );
    assert_quarantined(
        store.load_input_states(&conflicted).await,
        "rt:conflicted",
        &[&name],
    );
    assert!(realm.realm_poisoned().is_none());
}

// A runtime id too long for a plain key part is hashed in its scope name,
// so the open knows its conflicted scope only by that name. It is still
// quarantined under the key a call names it by: a call naming the runtime
// refuses typed. RED before containment: the open fails.
#[tokio::test]
async fn a_conflicted_scope_with_a_hashed_key_is_refused_by_its_runtime_id() {
    let host = Arc::new(TestHost::new());
    let long = LogicalRuntimeId::new("r".repeat(250));
    let kept = LogicalRuntimeId::new("kept");
    {
        let (_realm, store) = open(&host).await;
        accept_inputs(&store, &[&kept, &long]).await;
    }
    host.expire_lease();
    let prefix = format!("rt:{}~", codec::key_part(&long.0));
    assert!(prefix.starts_with("rt:h"), "the key part is hashed");
    let name = host
        .conflict_scope(ScopeKind::Session, &prefix)
        .expect("one conflicted scope");

    let (_realm, store) = open_contained(&host).await;
    store
        .load_input_states(&kept)
        .await
        .expect("another runtime serves");
    assert_quarantined(
        store.load_input_states(&long).await,
        &format!("rt:{}", long.0),
        &[&name],
    );
}

// No listing leaves a quarantined runtime out: the recovery-work scan lists
// it (its work cannot be read), the delivery authority listing names it
// unavailable, typed, and the session catalog listing refuses typed. RED
// before containment: the open fails.
#[tokio::test]
async fn no_listing_leaves_a_quarantined_runtime_out() {
    let host = Arc::new(TestHost::new());
    let kept = LogicalRuntimeId::new("kept");
    let conflicted = LogicalRuntimeId::new("conflicted");
    {
        let (_realm, store) = open(&host).await;
        accept_inputs(&store, &[&kept, &conflicted]).await;
    }
    host.expire_lease();
    let name = host
        .conflict_scope(ScopeKind::Session, "rt:rconflicted~")
        .expect("one conflicted scope");

    let (_realm, store) = open_contained(&host).await;
    let with_work = store
        .list_runtimes_with_recovery_work()
        .await
        .expect("the scan answers");
    assert!(
        with_work.contains(&conflicted),
        "the scan lists the quarantined runtime: {with_work:?}"
    );
    let authorities = store
        .list_runtime_delivery_authorities_or_unavailable()
        .await
        .expect("the listing answers");
    let unavailable: Vec<String> = authorities
        .unavailable
        .iter()
        .map(|runtime| runtime.runtime_id.0.clone())
        .collect();
    assert!(
        unavailable.contains(&conflicted.0),
        "the delivery listing names the quarantined runtime unavailable: {unavailable:?}"
    );
    assert_quarantined(
        RuntimeStore::list_runtime_session_catalog_entries(
            &store,
            meerkat_core::SessionFilter::default(),
        )
        .await,
        "rt:conflicted",
        &[&name],
    );
}

// A mob's binding scope is revoked (deleted) and bound again (a fresh
// incarnation of the same key), and a superseded writer's late batch then
// lands on the deleted incarnation at the version its delete took. The next
// open contains it: the mob's binding key refuses typed, its cause naming
// both incarnations, and a neighbour mob's binding reads and writes
// normally. RED before containment: the listing fails on the conflict (and
// containment by key alone would refuse the live incarnation's claim as a
// second live incarnation of the key).
#[tokio::test]
async fn a_late_batch_on_a_deleted_incarnation_beside_a_live_one_is_contained() {
    let host = Arc::new(TestHost::new());
    let (deleted, deleted_version) = {
        let (_realm, store) = open(&host).await;
        assert!(
            store
                .put_mob_host_binding_if_absent("mob-a", b"first")
                .await
                .expect("mob-a binds")
        );
        assert!(
            store
                .put_mob_host_binding_if_absent("mob-b", b"neighbour")
                .await
                .expect("mob-b binds")
        );
        let (name, version) = binding_incarnation(&host, "mob-a");
        assert!(
            store
                .revoke_mob_host_binding("mob-a", b"first", b"receipt")
                .await
                .expect("mob-a is revoked")
        );
        assert!(
            store
                .put_mob_host_binding_if_absent("mob-a", b"second")
                .await
                .expect("mob-a binds again")
        );
        (name, version.next().expect("a next version"))
    };
    let (live, _) = binding_incarnation(&host, "mob-a");
    assert_ne!(live, deleted, "binding again created a fresh incarnation");
    host.expire_lease();
    host.conflict_deleted(ScopeKind::Record, &deleted, deleted_version);

    let (realm, store) = open_contained(&host).await;
    assert_quarantined(
        store.load_mob_host_binding("mob-a").await,
        "mhb:mob-a",
        &[&deleted, &live],
    );
    assert_eq!(
        store
            .load_mob_host_binding("mob-b")
            .await
            .expect("the neighbour reads"),
        Some(b"neighbour".to_vec())
    );
    assert!(
        store
            .compare_and_put_mob_host_binding("mob-b", b"neighbour", b"next")
            .await
            .expect("the neighbour writes")
    );
    assert!(realm.realm_poisoned().is_none());
}

// One key with two conflicted incarnations (each a deleted incarnation a
// late batch landed on) and one live incarnation: the open contains all of
// it, and a re-acquire whose listing shows the same two conflicts keeps the
// realm (each conflict matches its own recorded name). RED before
// containment: the open fails; with one recorded name per key, the second
// incarnation's conflict would read foreign and lose the realm.
#[tokio::test(start_paused = true)]
async fn a_key_with_two_conflicted_incarnations_opens_contained_and_keeps_its_realm() {
    let timing = FenceTiming::try_new(Duration::from_secs(1), Duration::from_secs(3)).unwrap();
    let host = Arc::new(TestHost::with_timing(timing));
    let mut deleted = Vec::new();
    {
        let (_realm, store) = open(&host).await;
        for record in [b"first".as_slice(), b"second".as_slice()] {
            assert!(
                store
                    .put_mob_host_binding_if_absent("mob-a", record)
                    .await
                    .expect("mob-a binds")
            );
            let (name, version) = binding_incarnation(&host, "mob-a");
            assert!(
                store
                    .revoke_mob_host_binding("mob-a", record, b"receipt")
                    .await
                    .expect("mob-a is revoked")
            );
            deleted.push((name, version.next().expect("a next version")));
        }
        assert!(
            store
                .put_mob_host_binding_if_absent("mob-a", b"third")
                .await
                .expect("mob-a binds a third time")
        );
    }
    host.expire_lease();
    for (name, version) in &deleted {
        host.conflict_deleted(ScopeKind::Record, name, *version);
    }

    let (realm, store) = open_contained(&host).await;
    let names: Vec<&str> = deleted.iter().map(|(name, _)| name.as_str()).collect();
    assert_quarantined(
        store.load_mob_host_binding("mob-a").await,
        "mhb:mob-a",
        &names,
    );

    // A renew fails; the next tick re-acquires over the writer's own lease,
    // and its listing shows the two recorded conflicts unchanged.
    host.faults.lock().unwrap().renew_unavailable = 1;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(!realm.lease_is_live(), "the writer stopped");
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(
        realm.realm_poisoned().is_none(),
        "an unchanged listing keeps the realm: {:?}",
        realm.realm_poisoned()
    );
    assert!(realm.lease_is_live(), "the writer resumed");
    assert_quarantined(
        store.load_mob_host_binding("mob-a").await,
        "mhb:mob-a",
        &names,
    );
}

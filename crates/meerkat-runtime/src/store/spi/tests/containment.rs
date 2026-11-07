//! Containing one conflicted host store scope.
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

use super::super::writer_fence::ScopePoisonCause;
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
        store.refuses_runtime_until_reopen(&conflicted),
        "the quarantined runtime refuses until the realm is reopened"
    );
    assert!(
        !store.refuses_runtime_until_reopen(&kept),
        "another runtime does not"
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
// refuses typed. Until a call names it, the reads that list runtimes by id
// cannot name it, so they refuse typed, naming its scope, rather than leave
// it out; once a call named it, they name it as any quarantined runtime.
// RED before containment: the open fails.
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
    let hashed = prefix.trim_end_matches('~');

    let (_realm, store) = open_contained(&host).await;
    store
        .load_input_states(&kept)
        .await
        .expect("another runtime serves");
    assert_quarantined(
        store.list_runtimes_with_recovery_work().await,
        hashed,
        &[&name],
    );
    assert_quarantined(
        store
            .list_runtime_delivery_authorities_or_unavailable()
            .await,
        hashed,
        &[&name],
    );
    assert!(
        store.refuses_runtime_until_reopen(&long),
        "the quarantined runtime refuses until the realm is reopened"
    );
    assert_quarantined(
        store.load_input_states(&long).await,
        &format!("rt:{}", long.0),
        &[&name],
    );

    let with_work = store
        .list_runtimes_with_recovery_work()
        .await
        .expect("the scan names the runtime a call named");
    assert!(
        with_work.contains(&long),
        "the scan lists the quarantined runtime: {with_work:?}"
    );
    let authorities = store
        .list_runtime_delivery_authorities_or_unavailable()
        .await
        .expect("the listing names the runtime a call named");
    assert!(
        authorities
            .unavailable
            .iter()
            .any(|runtime| runtime.runtime_id == long),
        "the delivery listing names the quarantined runtime unavailable"
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

/// A host whose writer re-acquires within a row on tokio's paused clock: a
/// one second renew interval and a three second lease.
fn reacquiring_host() -> Arc<TestHost> {
    let timing = FenceTiming::try_new(Duration::from_secs(1), Duration::from_secs(3)).unwrap();
    Arc::new(TestHost::with_timing(timing))
}

/// Fail the writer's next renew and let the tick after it re-acquire. The
/// row's checkpoints fall between ticks, which come once a second from the
/// realm's open.
async fn stop_and_reacquire(host: &TestHost, realm: &SpiRealm) {
    host.faults.lock().unwrap().renew_unavailable = 1;
    tokio::time::sleep(Duration::from_millis(1250)).await;
    assert!(!realm.lease_is_live(), "the writer stopped");
    tokio::time::sleep(Duration::from_secs(1)).await;
}

/// The visible incarnation of a test family scope whose name starts with
/// `prefix`: its name and version.
fn incarnation(host: &TestHost, prefix: &str) -> (String, ScopeVersion) {
    let scopes = host.scopes_of(ScopeKind::Record);
    let (name, stored) = scopes
        .iter()
        .find(|(name, _)| name.starts_with(prefix))
        .expect("the scope is visible");
    (name.clone(), stored.version)
}

/// A one-entry change of a test family scope.
fn pin(value: &str) -> ScopeDelta {
    ScopeDelta::Changes(vec![(
        "pin".to_string(),
        Some(Arc::new(value.as_bytes().to_vec())),
    )])
}

/// Open a bare realm over `host`, with no adapter, the last writer's lease
/// ended.
async fn open_realm(host: &Arc<TestHost>) -> Arc<SpiRealm> {
    SpiRealm::open(Arc::clone(host) as _, SpiRealmConfig::default())
        .await
        .expect("the realm opens")
}

// A scope quarantined at the open stays quarantined through a stop: the
// re-acquire's listing shows its recorded conflict unchanged and keeps the
// realm, and the scope still refuses. A new conflict on another scope at the
// next re-acquire is a change this writer did not make: the realm is lost,
// and the next open contains both scopes and serves the rest.
#[tokio::test(start_paused = true)]
async fn a_recorded_conflict_keeps_the_realm_and_a_new_one_loses_it() {
    let host = reacquiring_host();
    let first = LogicalRuntimeId::new("first");
    let second = LogicalRuntimeId::new("second");
    let kept = LogicalRuntimeId::new("kept");
    {
        let (_realm, store) = open(&host).await;
        accept_inputs(&store, &[&first, &second, &kept]).await;
    }
    host.expire_lease();
    let first_name = host
        .conflict_scope(ScopeKind::Session, "rt:rfirst~")
        .expect("one conflicted scope");

    let (realm, store) = open_contained(&host).await;
    stop_and_reacquire(&host, &realm).await;
    assert!(
        realm.realm_poisoned().is_none(),
        "a listing that shows only the recorded conflict keeps the realm: {:?}",
        realm.realm_poisoned()
    );
    assert!(realm.lease_is_live(), "the writer resumed");
    assert_quarantined(
        store.load_input_states(&first).await,
        "rt:first",
        &[&first_name],
    );

    let second_name = host
        .conflict_scope(ScopeKind::Session, "rt:rsecond~")
        .expect("one conflicted scope");
    stop_and_reacquire(&host, &realm).await;
    assert!(
        realm.realm_poisoned().is_some(),
        "a conflict the writer did not record is a batch it did not write"
    );
    drop(store);
    drop(realm);
    host.expire_lease();

    let (_realm, store) = open_contained(&host).await;
    assert_quarantined(
        store.load_input_states(&first).await,
        "rt:first",
        &[&first_name],
    );
    assert_quarantined(
        store.load_input_states(&second).await,
        "rt:second",
        &[&second_name],
    );
    assert_eq!(
        store
            .load_input_states(&kept)
            .await
            .expect("another runtime serves")
            .len(),
        1
    );
}

// An unchanged conflict listed again with its commit ids in another order,
// and once with none, still matches its record: each re-acquire keeps the
// realm. The same conflict listed with other ids, where both sides name
// them, is another batch: the realm is lost.
#[tokio::test(start_paused = true)]
async fn a_recorded_conflict_matches_with_its_ids_reordered_or_unnamed_and_not_with_others() {
    let host = reacquiring_host();
    let runtime = LogicalRuntimeId::new("conflicted");
    {
        let (_realm, store) = open(&host).await;
        accept_inputs(&store, &[&runtime]).await;
    }
    host.expire_lease();
    let name = host
        .conflict_scope(ScopeKind::Session, "rt:rconflicted~")
        .expect("one conflicted scope");
    let stored = host
        .scopes_of(ScopeKind::Session)
        .get(&name)
        .expect("the scope is visible")
        .commit_id
        .as_str()
        .to_string();
    let late = format!("late-{name}");
    let mut descending = [stored, late];
    descending.sort();
    descending.reverse();

    let (realm, _store) = open_contained(&host).await;
    host.set_conflict_ids(
        ScopeKind::Session,
        &name,
        &[descending[0].as_str(), descending[1].as_str()],
    );
    stop_and_reacquire(&host, &realm).await;
    assert!(
        realm.realm_poisoned().is_none() && realm.lease_is_live(),
        "the same ids in another order match: {:?}",
        realm.realm_poisoned()
    );
    host.set_conflict_ids(ScopeKind::Session, &name, &[]);
    stop_and_reacquire(&host, &realm).await;
    assert!(
        realm.realm_poisoned().is_none() && realm.lease_is_live(),
        "ids named on one side only match: {:?}",
        realm.realm_poisoned()
    );
    host.set_conflict_ids(ScopeKind::Session, &name, &["another-batch"]);
    stop_and_reacquire(&host, &realm).await;
    assert!(
        realm.realm_poisoned().is_some(),
        "other ids where both sides name them are another batch"
    );
}

// A recorded conflict that a re-acquire's listing shows in neither list (the
// deleted incarnation it was recorded for is gone) is a change this writer
// did not make: the realm is lost.
#[tokio::test(start_paused = true)]
async fn a_recorded_conflict_missing_from_a_reacquires_listing_loses_the_realm() {
    let host = reacquiring_host();
    let doomed = ScopeId::new(TEST_FAMILY, "doomed");
    let (deleted, version) = {
        let realm = open_realm(&host).await;
        commit_one(&realm, &doomed, pin("v1"))
            .await
            .expect("the scope is written");
        let (name, version) = incarnation(&host, "tst:rdoomed~");
        commit_one(&realm, &doomed, ScopeDelta::Full(BTreeMap::new()))
            .await
            .expect("the scope is deleted");
        (name, version.next().expect("a next version"))
    };
    host.expire_lease();
    host.conflict_deleted(ScopeKind::Record, &deleted, version);

    let realm = open_realm(&host).await;
    host.repair_conflict(ScopeKind::Record, &deleted);
    stop_and_reacquire(&host, &realm).await;
    assert!(
        realm.realm_poisoned().is_some(),
        "a recorded conflict listed in neither list is a change"
    );
}

// A recorded conflict that a re-acquire's listing shows among the heads (the
// scope repaired meanwhile) is a change this writer did not make: the realm
// is lost. The writer cannot tell an operator's repair from another writer's
// commit, so a repair is followed by a reopen.
#[tokio::test(start_paused = true)]
async fn a_recorded_conflict_listed_among_the_heads_loses_the_realm() {
    let host = reacquiring_host();
    let runtime = LogicalRuntimeId::new("conflicted");
    {
        let (_realm, store) = open(&host).await;
        accept_inputs(&store, &[&runtime]).await;
    }
    host.expire_lease();
    let name = host
        .conflict_scope(ScopeKind::Session, "rt:rconflicted~")
        .expect("one conflicted scope");

    let (realm, _store) = open_contained(&host).await;
    host.repair_conflict(ScopeKind::Session, &name);
    stop_and_reacquire(&host, &realm).await;
    assert!(
        realm.realm_poisoned().is_some(),
        "a recorded conflict listed among the heads is a change"
    );
}

// An owner whose chunk is conflicted: the open's listing quarantines the
// chunk by its full store name, and the owner's first load meets that
// conflict and quarantines the owner as well, refused typed; neither the
// realm nor any scope is poisoned. A commit on the owner writes nothing, and
// a re-acquire whose listing shows the owner's head unchanged and the
// chunk's recorded conflict keeps the realm.
#[tokio::test(start_paused = true)]
async fn an_owner_whose_chunk_is_conflicted_is_quarantined_at_its_first_load() {
    let host = reacquiring_host();
    let owner = ScopeId::new(TEST_FAMILY, "owner");
    {
        let realm = open_realm(&host).await;
        let body = Arc::new(vec![7_u8; super::super::realm::INLINE_VALUE_LIMIT + 1]);
        commit_one(
            &realm,
            &owner,
            ScopeDelta::Changes(vec![("body".to_string(), Some(body))]),
        )
        .await
        .expect("the body is written to a chunk");
    }
    host.expire_lease();
    let chunk = host
        .conflict_scope(ScopeKind::Record, "ch:")
        .expect("one chunk");

    let realm = open_realm(&host).await;
    let quarantined: Vec<String> = realm
        .quarantined_scopes()
        .into_iter()
        .map(|scope| scope.scope)
        .collect();
    assert_eq!(
        quarantined,
        vec![chunk.clone()],
        "the open quarantined the chunk by its full name"
    );
    let cells = realm.claim(TEST_FAMILY).expect("the family claims");
    {
        let mut state = cells[0].lock().await;
        match realm.load_body(&mut state).await {
            Err(SpiError::ScopeQuarantined { scope, cause }) => {
                assert_eq!(scope, "tst:owner");
                assert!(
                    cause.contains(QUARANTINED) && cause.contains(REPAIR) && cause.contains(&chunk),
                    "the cause names the chunk's conflict and the repair: {cause}"
                );
            }
            other => panic!("the owner's first load refuses typed: {other:?}"),
        }
    }
    let quarantined = realm.quarantined_scopes();
    assert!(
        quarantined
            .iter()
            .any(|scope| scope.scope == "tst:owner" && scope.names == vec![chunk.clone()]),
        "the owner is quarantined for its chunk's conflict: {quarantined:?}"
    );
    assert!(realm.realm_poisoned().is_none());
    assert!(realm.poisoned_scopes().is_empty());

    let commits = host.commit_count();
    assert!(matches!(
        commit_one(&realm, &owner, pin("next")).await,
        Err(SpiError::ScopeQuarantined { .. })
    ));
    assert_eq!(
        host.commit_count(),
        commits,
        "a commit on the owner writes nothing"
    );

    stop_and_reacquire(&host, &realm).await;
    assert!(
        realm.realm_poisoned().is_none(),
        "the owner's unchanged head and the chunk's recorded conflict keep the realm: {:?}",
        realm.realm_poisoned()
    );
    assert!(realm.lease_is_live(), "the writer resumed");
}

// A quarantined scope still owns its chunks: the open's sweep of chunks
// whose owner is not listed keeps them.
#[tokio::test]
async fn the_chunk_sweep_keeps_a_quarantined_owners_chunks() {
    let host = Arc::new(TestHost::new());
    let owner = ScopeId::new(TEST_FAMILY, "owner");
    {
        let realm = open_realm(&host).await;
        let body = Arc::new(vec![7_u8; super::super::realm::INLINE_VALUE_LIMIT + 1]);
        commit_one(
            &realm,
            &owner,
            ScopeDelta::Changes(vec![("body".to_string(), Some(body))]),
        )
        .await
        .expect("the body is written to a chunk");
    }
    host.expire_lease();
    let name = host
        .conflict_scope(ScopeKind::Record, "tst:rowner~")
        .expect("the owner");

    let realm = open_realm(&host).await;
    let chunks: Vec<String> = host
        .scopes_of(ScopeKind::Record)
        .into_keys()
        .filter(|name| name.starts_with("ch:"))
        .collect();
    assert_eq!(chunks.len(), 1, "the owner's chunk is kept: {chunks:?}");
    let cells = realm.claim(TEST_FAMILY).expect("the family claims");
    let state = cells[0].lock().await;
    assert!(
        state
            .quarantine()
            .is_some_and(|cause| cause.contains(&name)),
        "the owner is quarantined: {:?}",
        state.quarantine()
    );
}

// What still fails the whole open: a store's Unavailable listing (a pending
// acquire cut), which a later open gets past, and a head no format decodes.
#[tokio::test]
async fn an_unavailable_listing_stays_retryable_and_an_undecodable_head_fails_the_open() {
    let host = Arc::new(TestHost::new());
    let kept = LogicalRuntimeId::new("kept");
    {
        let (_realm, store) = open(&host).await;
        accept_inputs(&store, &[&kept]).await;
    }
    host.expire_lease();
    host.faults.lock().unwrap().list_unavailable = 1;
    match SpiRealm::open(Arc::clone(&host) as _, SpiRealmConfig::default()).await {
        Err(SpiError::Host(HostStoreError::Unavailable(_))) => {}
        other => panic!("an unavailable listing fails the open as an outage: {other:?}"),
    }
    host.expire_lease();
    drop(open_contained(&host).await);
    host.expire_lease();
    host.corrupt_head(ScopeKind::Session, "rt:rkept~")
        .expect("one runtime");
    match SpiRealm::open(Arc::clone(&host) as _, SpiRealmConfig::default()).await {
        Err(SpiError::Corrupt(_)) => {}
        other => panic!("a head no format decodes fails the open: {other:?}"),
    }
}

// A scope a dropped call poisoned is listed by `poisoned_scopes` and still
// refuses after a renew failure's re-acquire: the writer fence keeps the
// poison through a stop, a re-acquire and a resume.
#[tokio::test(start_paused = true)]
async fn a_dropped_calls_poison_is_listed_and_refuses_after_a_reacquire() {
    let host = reacquiring_host();
    let realm = open_realm(&host).await;
    let armed = ScopeId::new(TEST_FAMILY, "armed");
    {
        let guards = realm.lock_scopes(std::slice::from_ref(&armed)).await;
        let mut scopes = ArmedScopes::new(Arc::clone(&realm), guards);
        scopes.arm();
    }
    let listed: Vec<ScopeId> = realm
        .poisoned_scopes()
        .into_iter()
        .map(|scope| scope.id)
        .collect();
    assert_eq!(listed, vec![armed.clone()]);

    stop_and_reacquire(&host, &realm).await;
    assert!(realm.lease_is_live(), "the writer resumed");
    assert!(realm.scope_poisoned(&armed));
    assert!(matches!(
        commit_one(&realm, &armed, pin("v1")).await,
        Err(SpiError::ScopePoisoned { .. })
    ));
}

// A mob's host revocation scope quarantined at the open: a read of the
// revocation and a binding write that would supersede it refuse typed, and
// nothing is written. The open claims the scope with no incarnation and its
// cell starts quarantined; created usable, its write would land a second
// live incarnation of the key.
#[tokio::test]
async fn a_quarantined_revocation_scope_refuses_its_read_and_a_write_that_supersedes_it() {
    let host = Arc::new(TestHost::new());
    {
        let (_realm, store) = open(&host).await;
        assert!(
            store
                .put_mob_host_binding_if_absent("mob-a", b"first")
                .await
                .expect("mob-a binds")
        );
        assert!(
            store
                .revoke_mob_host_binding("mob-a", b"first", b"receipt")
                .await
                .expect("mob-a is revoked")
        );
    }
    host.expire_lease();
    let name = host
        .conflict_scope(ScopeKind::Record, "mhr:rmob-a~")
        .expect("the revocation scope");

    let (_realm, store) = open_contained(&host).await;
    assert_quarantined(
        store.load_mob_host_revocation("mob-a").await,
        "mhr:mob-a",
        &[&name],
    );
    let commits = host.commit_count();
    assert_quarantined(
        store
            .put_mob_host_binding_if_absent("mob-a", b"again")
            .await,
        "mhr:mob-a",
        &[&name],
    );
    assert_eq!(host.commit_count(), commits, "nothing was written");
}

// A tool grant scope quarantined at the open: a load of that grant refuses
// typed, and so does the grant listing of every subject, since the
// quarantined grant's head, and so the subject it is filed under, is
// unknown. Another grant still loads.
#[tokio::test]
async fn a_quarantined_tool_grant_scope_refuses_its_load_and_every_grant_listing() {
    let host = Arc::new(TestHost::new());
    {
        let (_realm, store) = open(&host).await;
        for (grant, subject) in [("g1", "s:a"), ("g2", "s:b")] {
            assert!(
                store
                    .put_tool_grant_if_absent(grant, subject, b"grant", &[])
                    .await
                    .expect("the grant is written")
            );
        }
    }
    host.expire_lease();
    let name = host
        .conflict_scope(ScopeKind::Record, "tg:rg1~")
        .expect("the grant scope");

    let (_realm, store) = open_contained(&host).await;
    assert_quarantined(store.load_tool_grant("g1").await, "tg:g1", &[&name]);
    for subject in ["s:a", "s:b"] {
        assert_quarantined(store.list_tool_grants(subject).await, "tg:g1", &[&name]);
    }
    assert_eq!(
        store
            .load_tool_grant("g2")
            .await
            .expect("another grant loads"),
        Some(b"grant".to_vec())
    );
}

// A poison report keeps a quarantine: an adapter's poison of a scope
// quarantined at the open, before the scope has a cell, and a stray touch of
// another, change nothing. Each refuses as quarantined at its next call,
// nothing is listed poisoned, and the next re-acquire keeps the realm.
#[tokio::test(start_paused = true)]
async fn a_poison_report_keeps_a_quarantine_through_a_reacquire() {
    let host = reacquiring_host();
    let reported = ScopeId::new(TEST_FAMILY, "reported");
    let touched = ScopeId::new(TEST_FAMILY, "touched");
    {
        let realm = open_realm(&host).await;
        for id in [&reported, &touched] {
            commit_one(&realm, id, pin("v1"))
                .await
                .expect("the scope is written");
        }
    }
    host.expire_lease();
    let reported_name = host
        .conflict_scope(ScopeKind::Record, "tst:rreported~")
        .expect("one scope");
    let touched_name = host
        .conflict_scope(ScopeKind::Record, "tst:rtouched~")
        .expect("one scope");

    // No adapter claimed the family: neither scope has a cell yet.
    let realm = open_realm(&host).await;
    {
        let cell = realm.cell(&reported);
        let mut state = cell.lock().await;
        realm.poison(&mut state, "an adapter's rule broke");
        assert!(state.quarantine().is_some(), "the scope stays quarantined");
    }
    {
        let cell = realm.cell(&touched);
        let mut state = cell.lock().await;
        realm.poison_scope(
            &mut state,
            ScopePoisonCause::StrayTouch,
            "a call changed it outside the scopes it locked",
        );
        assert!(state.quarantine().is_some(), "the scope stays quarantined");
    }
    assert!(realm.poisoned_scopes().is_empty(), "nothing is poisoned");
    for (id, name) in [(&reported, &reported_name), (&touched, &touched_name)] {
        match commit_one(&realm, id, pin("v2")).await {
            Err(SpiError::ScopeQuarantined { cause, .. }) => {
                assert!(cause.contains(name.as_str()), "{cause}");
            }
            other => panic!("{id} refuses as quarantined: {other:?}"),
        }
    }

    stop_and_reacquire(&host, &realm).await;
    assert!(
        realm.realm_poisoned().is_none(),
        "the recorded conflicts keep the realm: {:?}",
        realm.realm_poisoned()
    );
    assert!(realm.lease_is_live(), "the writer resumed");
}

// A poisoned scope that a re-acquire's listing shows conflicted is a change
// this writer did not make (a poisoned scope sends no further batch): the
// realm is lost. The next open quarantines the scope.
#[tokio::test(start_paused = true)]
async fn a_poisoned_scope_listed_conflicted_at_a_reacquire_loses_the_realm() {
    let host = reacquiring_host();
    let scope = ScopeId::new(TEST_FAMILY, "poisoned");
    let realm = open_realm(&host).await;
    commit_one(&realm, &scope, pin("v1"))
        .await
        .expect("the scope is written");
    {
        let cell = realm.cell(&scope);
        let mut state = cell.lock().await;
        realm.poison(&mut state, "an adapter's rule broke");
    }
    let name = host
        .conflict_scope(ScopeKind::Record, "tst:rpoisoned~")
        .expect("one scope");
    stop_and_reacquire(&host, &realm).await;
    assert!(
        realm.realm_poisoned().is_some(),
        "a conflict on a scope not quarantined is a change"
    );
    drop(realm);
    host.expire_lease();

    let realm = open_realm(&host).await;
    let quarantined = realm.quarantined_scopes();
    assert!(
        quarantined
            .iter()
            .any(|scope| scope.scope == "tst:poisoned" && scope.names == vec![name.clone()]),
        "the next open quarantines it: {quarantined:?}"
    );
}

// A realm loss the writer fence refuses before the realm opened is a misuse,
// answered so; a loss it records, or a realm lost already, answers the
// recorded cause.
#[test]
fn a_realm_loss_refused_before_the_open_answers_a_misuse() {
    use super::super::writer_fence::{
        HostStoreWriterFenceEffect, WriterFenceLostCause, WriterFenceRefusal,
    };
    let refused = super::super::realm::refused_loss(
        &[HostStoreWriterFenceEffect::RequestRefused {
            reason: WriterFenceRefusal::NotOpened,
        }],
        "an adapter's rule broke",
    );
    assert!(
        matches!(&refused, Some(SpiError::Misuse(text)) if text.contains("before the realm opened")),
        "{refused:?}"
    );
    for effect in [
        HostStoreWriterFenceEffect::RealmLost {
            cause: WriterFenceLostCause::AdapterRule,
        },
        HostStoreWriterFenceEffect::RequestRefused {
            reason: WriterFenceRefusal::Lost,
        },
    ] {
        assert!(super::super::realm::refused_loss(&[effect], "lost").is_none());
    }
}

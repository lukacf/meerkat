//! Rows for the host store adapters, unrun until the candidate's
//! validation.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use meerkat_core::completion_feed::CompletionFeed;
use meerkat_core::host_store::{AcquireMode, FenceTiming, ScopeKind};
use meerkat_core::lifecycle::InputId;
use meerkat_core::ops_lifecycle::{
    CompletionCursorConsumer, OperationId, OperationKind, OperationResult, OperationSource,
    OperationSpec, OpsLifecycleRegistry,
};
use meerkat_core::types::SessionId;
use meerkat_core::{EpochCursorState, RuntimeEpochId};

use super::codec::{self, RowOp, ScopeHeadV1};
use super::realm::{
    CHECKPOINT_FACTOR, CHECKPOINT_MIN_BYTES, INLINE_VALUE_LIMIT, LIST_PAGE_HEADS, READ_PAGE_ROWS,
};
use super::test_host::{HostCall, TestHost};
use super::{
    ArmedScopes, ScopeDelta, ScopeFamily, ScopeId, SpiError, SpiRealm, SpiRealmConfig,
    SpiRuntimeStore,
};
use crate::identifiers::LogicalRuntimeId;
use crate::input_state::{
    InputLifecycleState, InputStatePersistenceRecord, InputTerminalOutcome, StoredInputState,
};
use crate::ops_lifecycle::{OpsLifecycleConfig, PersistedOpsSnapshot, RuntimeOpsLifecycleRegistry};
use crate::runtime_state::RuntimeState;
use crate::store::{
    MachineLifecycleBindingFacts, MachineLifecycleCommit, MachineLifecyclePreRunPhase,
    MachineLifecycleRunFacts, RuntimeDeliveryAuthorityCasOutcome, RuntimeDeliveryAuthorityRecord,
    RuntimeDeliveryStoreRecord, RuntimeStore, RuntimeStoreError, SupervisorAuthoritySnapshot,
};

const TEST_FAMILY: ScopeFamily = ScopeFamily {
    tag: "tst",
    kind: ScopeKind::Record,
    pinned: &["pin"],
};

fn persistable(bundle: StoredInputState) -> InputStatePersistenceRecord {
    InputStatePersistenceRecord::from_machine_snapshot(bundle).expect("machine snapshot")
}

async fn open(host: &Arc<TestHost>) -> (Arc<SpiRealm>, SpiRuntimeStore) {
    let realm = SpiRealm::open(Arc::clone(host) as _, SpiRealmConfig::default())
        .await
        .expect("realm opens");
    let store = SpiRuntimeStore::open(Arc::clone(&realm))
        .await
        .expect("store opens");
    (realm, store)
}

/// Drop a process and open another over the same host, as a crash restart
/// whose lease ran out.
async fn reopen(
    host: &Arc<TestHost>,
    old: (Arc<SpiRealm>, SpiRuntimeStore),
) -> (Arc<SpiRealm>, SpiRuntimeStore) {
    drop(old);
    host.expire_lease();
    open(host).await
}

fn delivery_record(sequence: u64) -> RuntimeDeliveryStoreRecord {
    RuntimeDeliveryStoreRecord::from_parts(
        format!("delivery-{sequence}"),
        sequence,
        br#"{"kind":"test"}"#.to_vec(),
    )
}

// Unrun until the candidate's validation.
#[tokio::test]
async fn writes_reach_the_host_and_a_reopen_sees_the_same_full_export() {
    let host = Arc::new(TestHost::new());
    let opened = open(&host).await;
    let alpha = LogicalRuntimeId::new("alpha");
    let beta = LogicalRuntimeId::new("beta");
    for runtime in [&alpha, &beta] {
        for _ in 0..3 {
            let bundle = StoredInputState::new_accepted(InputId::new());
            opened
                .1
                .persist_input_state(runtime, &persistable(bundle))
                .await
                .expect("persist");
        }
    }
    let outcome = opened
        .1
        .compare_and_swap_runtime_delivery_authority(
            &beta,
            None,
            RuntimeDeliveryAuthorityRecord::from_parts(1, br#"{"state":1}"#.to_vec()),
            Some(delivery_record(1)),
        )
        .await
        .expect("delivery cas");
    assert!(matches!(
        outcome,
        RuntimeDeliveryAuthorityCasOutcome::Applied(_)
    ));
    let high_water = meerkat_contracts::wire::supervisor_bridge::BridgeDirectMemberIncarnation {
        mob_id: "mob".into(),
        agent_identity: "member".into(),
        generation: 3,
        fence_token: 7,
    };
    opened
        .1
        .admit_direct_member_incarnation_high_water("member-session", &high_water)
        .await
        .expect("high water");
    let before_alpha = opened.1.export_runtime_for_test("alpha").await;
    let before_beta = opened.1.export_runtime_for_test("beta").await;
    assert_eq!(
        host.scopes_of(ScopeKind::Session).len(),
        2,
        "one session scope per runtime"
    );

    let reopened = reopen(&host, opened).await;
    // Touch both bodies so they are resident, then compare everything.
    reopened.1.load_input_states(&alpha).await.expect("alpha");
    reopened.1.load_input_states(&beta).await.expect("beta");
    assert_eq!(
        reopened.1.export_runtime_for_test("alpha").await,
        before_alpha
    );
    assert_eq!(
        reopened.1.export_runtime_for_test("beta").await,
        before_beta
    );
    assert!(
        host.scopes_of(ScopeKind::Record)
            .keys()
            .any(|name| name.starts_with("dmh:")),
        "the high water is a record scope"
    );
    let authorities = reopened
        .1
        .list_runtime_delivery_authorities()
        .await
        .expect("listing");
    assert_eq!(authorities.len(), 1);
}

// Unrun until the candidate's validation.
#[tokio::test]
async fn a_body_recovery_does_not_need_stays_on_the_host_until_first_use() {
    let host = Arc::new(TestHost::new());
    let opened = open(&host).await;
    let quiet = LogicalRuntimeId::new("quiet");
    opened
        .1
        .compare_and_swap_runtime_delivery_authority(
            &quiet,
            None,
            RuntimeDeliveryAuthorityRecord::from_parts(1, br#"{"state":1}"#.to_vec()),
            Some(delivery_record(1)),
        )
        .await
        .expect("delivery cas");
    let busy = LogicalRuntimeId::new("busy");
    opened
        .1
        .persist_input_state(
            &busy,
            &persistable(StoredInputState::new_accepted(InputId::new())),
        )
        .await
        .expect("nonterminal input");

    let reopened = reopen(&host, opened).await;
    assert!(
        reopened.1.runtime_loaded_for_test("busy").await,
        "a runtime with nonterminal inputs loads at boot"
    );
    assert!(
        !reopened.1.runtime_loaded_for_test("quiet").await,
        "a runtime recovery need not act on stays on the host"
    );
    // The pinned delivery authority is served without the body.
    assert!(
        reopened
            .1
            .load_runtime_delivery_authority(&quiet)
            .await
            .expect("authority")
            .is_some()
    );
    assert!(!reopened.1.runtime_loaded_for_test("quiet").await);
    // A body read loads it.
    let records = reopened
        .1
        .list_runtime_delivery_records(&quiet, 0, 10)
        .await
        .expect("records");
    assert_eq!(records.len(), 1);
    assert!(reopened.1.runtime_loaded_for_test("quiet").await);
}

// Unrun until the candidate's validation.
#[tokio::test]
async fn a_failed_batch_poisons_its_scopes_and_nothing_else() {
    let host = Arc::new(TestHost::new());
    let (_realm, store) = open(&host).await;
    let doomed = LogicalRuntimeId::new("doomed");
    let healthy = LogicalRuntimeId::new("healthy");
    host.faults.lock().unwrap().conflict = 1;
    let refused = store
        .persist_input_state(
            &doomed,
            &persistable(StoredInputState::new_accepted(InputId::new())),
        )
        .await;
    assert!(matches!(refused, Err(RuntimeStoreError::WriteFailed(_))));
    // The poisoned scope refuses reads too: its memory may be ahead.
    assert!(store.load_input_states(&doomed).await.is_err());
    // Another runtime keeps working.
    store
        .persist_input_state(
            &healthy,
            &persistable(StoredInputState::new_accepted(InputId::new())),
        )
        .await
        .expect("independent scope");
    // Cross-scope reads fail closed while a scope is poisoned.
    assert!(
        RuntimeStore::list_runtime_session_catalog_entries(
            &store,
            meerkat_core::SessionFilter::default()
        )
        .await
        .is_err()
    );
}

// Unrun until the candidate's validation.
#[tokio::test]
async fn an_unknown_outcome_that_landed_settles_from_the_head() {
    let host = Arc::new(TestHost::new());
    let (_realm, store) = open(&host).await;
    host.faults.lock().unwrap().indeterminate_after_apply = 1;
    let runtime = LogicalRuntimeId::new("settled");
    store
        .persist_input_state(
            &runtime,
            &persistable(StoredInputState::new_accepted(InputId::new())),
        )
        .await
        .expect("the landed batch is acknowledged");
    assert_eq!(host.commit_count(), 1, "the batch landed once");
    // The scope is healthy: a second write uses the next version.
    store
        .persist_input_state(
            &runtime,
            &persistable(StoredInputState::new_accepted(InputId::new())),
        )
        .await
        .expect("next version");
}

// Unrun until the candidate's validation.
#[tokio::test]
async fn an_unavailable_store_is_retried_with_the_same_commit_id() {
    let host = Arc::new(TestHost::new());
    let (_realm, store) = open(&host).await;
    host.faults.lock().unwrap().unavailable = 2;
    store
        .persist_input_state(
            &LogicalRuntimeId::new("retried"),
            &persistable(StoredInputState::new_accepted(InputId::new())),
        )
        .await
        .expect("retried within the budget");
    assert_eq!(host.commit_count(), 1);
}

// Unrun until the candidate's validation.
#[tokio::test(start_paused = true)]
async fn a_failed_renew_stops_writes_before_memory_changes() {
    let timing = FenceTiming::try_new(Duration::from_secs(1), Duration::from_secs(3)).unwrap();
    let host = Arc::new(TestHost::with_timing(timing));
    let (realm, store) = open(&host).await;
    // The background renew fails, then the re-acquire fails too.
    {
        let mut faults = host.faults.lock().unwrap();
        faults.renew_unavailable = 1;
        faults.acquire_unavailable = 1;
    }
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(
        !realm.lease_is_live(),
        "a failed renew stops writes at once"
    );
    let runtime = LogicalRuntimeId::new("fenced");
    let refused = store
        .persist_input_state(
            &runtime,
            &persistable(StoredInputState::new_accepted(InputId::new())),
        )
        .await;
    assert!(refused.is_err());
    assert_eq!(host.commit_count(), 0);
    // Memory did not change: once a renew succeeds the runtime is empty.
    assert!(
        store
            .load_input_states(&runtime)
            .await
            .expect("read")
            .is_empty()
    );
}

// Unrun until the candidate's validation.
#[tokio::test(start_paused = true)]
async fn an_exclusive_open_refuses_a_live_holder_and_a_takeover_waits_out_its_lease() {
    let host = Arc::new(TestHost::new());
    let (first, _store) = open(&host).await;
    let held = SpiRealm::open(Arc::clone(&host) as _, SpiRealmConfig::default()).await;
    assert!(matches!(held, Err(SpiError::Held { .. })));
    let started = tokio::time::Instant::now();
    let config = SpiRealmConfig {
        acquire: AcquireMode::Takeover,
        ..SpiRealmConfig::default()
    };
    let second = SpiRealm::open(Arc::clone(&host) as _, config)
        .await
        .expect("takeover");
    assert!(
        started.elapsed() > Duration::from_secs(1),
        "waited out the lease"
    );
    assert!(second.fence() > first.fence());
}

// Unrun until the candidate's validation.
#[tokio::test]
async fn both_adapters_share_one_fence() {
    let host = Arc::new(TestHost::new());
    let (realm, store) = open(&host).await;
    store
        .persist_input_state(
            &LogicalRuntimeId::new("runtime"),
            &persistable(StoredInputState::new_accepted(InputId::new())),
        )
        .await
        .expect("runtime adapter");
    // A second adapter (as the mob adapter does) commits through the same
    // realm: no second acquire, nobody fenced out.
    let id = ScopeId::new(TEST_FAMILY, "other");
    let mut guards = realm.lock_scopes(std::slice::from_ref(&id)).await;
    let mut states: Vec<_> = guards.iter_mut().map(|guard| &mut **guard).collect();
    realm
        .commit(
            &mut states,
            vec![ScopeDelta::Changes(vec![(
                "pin".into(),
                Some(Arc::new(b"one".to_vec())),
            )])],
        )
        .await
        .expect("second adapter");
    drop(states);
    drop(guards);
    store
        .persist_input_state(
            &LogicalRuntimeId::new("runtime"),
            &persistable(StoredInputState::new_accepted(InputId::new())),
        )
        .await
        .expect("the runtime adapter still holds the fence");
}

async fn commit_one(realm: &SpiRealm, id: &ScopeId, delta: ScopeDelta) -> Result<(), SpiError> {
    let mut guards = realm.lock_scopes(std::slice::from_ref(id)).await;
    let mut states: Vec<_> = guards.iter_mut().map(|guard| &mut **guard).collect();
    realm.commit(&mut states, vec![delta]).await
}

async fn reload(
    host: &Arc<TestHost>,
    realm: Arc<SpiRealm>,
) -> (Arc<SpiRealm>, BTreeMap<String, Arc<Vec<u8>>>) {
    drop(realm);
    host.expire_lease();
    let realm = SpiRealm::open(Arc::clone(host) as _, SpiRealmConfig::default())
        .await
        .unwrap();
    let cells = realm.claim(TEST_FAMILY).unwrap();
    let mut image = BTreeMap::new();
    for cell in cells {
        let mut state = cell.lock().await;
        image.extend(realm.pinned_values(&state).await.unwrap());
        image.extend(realm.load_body(&mut state).await.unwrap());
        // The test is the adapter here: it holds the loaded values.
        state.mark_loaded();
    }
    (realm, image)
}

// Unrun until the candidate's validation.
#[tokio::test]
async fn an_oversized_value_lives_in_a_chunk_that_dies_with_it() {
    let host = Arc::new(TestHost::new());
    let realm = SpiRealm::open(Arc::clone(&host) as _, SpiRealmConfig::default())
        .await
        .unwrap();
    let id = ScopeId::new(TEST_FAMILY, "big");
    let big = Arc::new(vec![7_u8; 3 * 1024 * 1024 + 11]);
    commit_one(
        &realm,
        &id,
        ScopeDelta::Changes(vec![("body".into(), Some(Arc::clone(&big)))]),
    )
    .await
    .unwrap();
    let chunks = |host: &TestHost| {
        host.scopes_of(ScopeKind::Record)
            .keys()
            .filter(|name| name.starts_with("ch:"))
            .count()
    };
    assert_eq!(chunks(&host), 1);
    let (realm, image) = reload(&host, realm).await;
    assert_eq!(image.get("body"), Some(&big));
    commit_one(
        &realm,
        &id,
        ScopeDelta::Changes(vec![("body".into(), Some(Arc::new(b"small".to_vec())))]),
    )
    .await
    .unwrap();
    assert_eq!(
        chunks(&host),
        0,
        "the superseded chunk is deleted in the same batch"
    );
}

// A chunked value replaced by another reloads from the host: the replacing
// batch deleted the first chunk, and the load reads only the chunk of the
// entry that survives the fold, not the one its superseded row names.
#[tokio::test]
async fn a_replaced_chunked_value_reloads_without_its_deleted_chunk() {
    let host = Arc::new(TestHost::new());
    let realm = SpiRealm::open(Arc::clone(&host) as _, SpiRealmConfig::default())
        .await
        .unwrap();
    let id = ScopeId::new(TEST_FAMILY, "big");
    let first = Arc::new(vec![7_u8; INLINE_VALUE_LIMIT + 1]);
    let second = Arc::new(vec![8_u8; INLINE_VALUE_LIMIT + 1]);
    for value in [&first, &second] {
        commit_one(
            &realm,
            &id,
            ScopeDelta::Changes(vec![("body".into(), Some(Arc::clone(value)))]),
        )
        .await
        .unwrap();
    }
    let chunks = host
        .scopes_of(ScopeKind::Record)
        .keys()
        .filter(|name| name.starts_with("ch:"))
        .count();
    assert_eq!(chunks, 1, "the first chunk died with the replacing batch");
    let (_realm, image) = reload(&host, realm).await;
    assert_eq!(image.get("body"), Some(&second));
}

// Unrun until the candidate's validation.
#[tokio::test]
async fn a_change_set_over_the_batch_limit_spills_and_reloads() {
    let host = Arc::new(TestHost::new());
    let realm = SpiRealm::open(Arc::clone(&host) as _, SpiRealmConfig::default())
        .await
        .unwrap();
    let id = ScopeId::new(TEST_FAMILY, "wide");
    let changes: Vec<_> = (0..40)
        .map(|index| {
            (
                format!("entry/{index}"),
                Some(Arc::new(vec![index as u8; INLINE_VALUE_LIMIT - 64])),
            )
        })
        .collect();
    let expected: BTreeMap<String, Arc<Vec<u8>>> = changes
        .iter()
        .map(|(key, value)| (key.clone(), Arc::clone(value.as_ref().unwrap())))
        .collect();
    commit_one(&realm, &id, ScopeDelta::Changes(changes))
        .await
        .unwrap();
    let (_realm, image) = reload(&host, realm).await;
    assert_eq!(image, expected);
}

// Unrun until the candidate's validation.
#[tokio::test]
async fn a_checkpoint_drops_every_earlier_row_in_one_batch() {
    let host = Arc::new(TestHost::new());
    let realm = SpiRealm::open(Arc::clone(&host) as _, SpiRealmConfig::default())
        .await
        .unwrap();
    let id = ScopeId::new(TEST_FAMILY, "churn");
    let value = |round: u8| Arc::new(vec![round; 200 * 1024]);
    let mut round = 0_u8;
    loop {
        round = round.wrapping_add(1);
        let wants_full = realm.cell(&id).lock().await.wants_full_image();
        let delta = if wants_full {
            let mut image = BTreeMap::new();
            image.insert("hot".to_string(), value(round));
            ScopeDelta::Full(image)
        } else {
            ScopeDelta::Changes(vec![("hot".into(), Some(value(round)))])
        };
        commit_one(&realm, &id, delta).await.unwrap();
        if wants_full {
            break;
        }
        assert!(u64::from(round) * 200 * 1024 < 4 * CHECKPOINT_MIN_BYTES);
    }
    let stored = host.scopes_of(ScopeKind::Record);
    let scope = stored
        .iter()
        .find(|(name, _)| name.starts_with("tst:"))
        .map(|(_, scope)| scope.clone())
        .unwrap();
    assert!(scope.start > 0, "the checkpoint dropped the earlier rows");
    let (_realm, image) = reload(&host, realm).await;
    assert_eq!(image.get("hot"), Some(&value(round)));
}

// Unrun until the candidate's validation.
#[tokio::test]
async fn a_deleted_scope_comes_back_under_a_new_name() {
    let host = Arc::new(TestHost::new());
    let realm = SpiRealm::open(Arc::clone(&host) as _, SpiRealmConfig::default())
        .await
        .unwrap();
    let id = ScopeId::new(TEST_FAMILY, "phoenix");
    let set = |value: Option<&'static [u8]>| {
        ScopeDelta::Changes(vec![(
            "pin".into(),
            value.map(|bytes| Arc::new(bytes.to_vec())),
        )])
    };
    commit_one(&realm, &id, set(Some(b"one"))).await.unwrap();
    let first: Vec<String> = host.scopes_of(ScopeKind::Record).into_keys().collect();
    commit_one(&realm, &id, set(None)).await.unwrap();
    assert!(host.scopes_of(ScopeKind::Record).is_empty());
    // The test host refuses to create a deleted name again.
    commit_one(&realm, &id, set(Some(b"two"))).await.unwrap();
    let second: Vec<String> = host.scopes_of(ScopeKind::Record).into_keys().collect();
    assert_ne!(first, second);
}

// Unrun until the candidate's validation.
#[tokio::test]
async fn mob_host_bindings_follow_the_sqlite_store() {
    let host = Arc::new(TestHost::new());
    let opened = open(&host).await;
    let store = &opened.1;
    assert!(
        store
            .put_mob_host_binding_if_absent("mob", b"one")
            .await
            .unwrap()
    );
    assert!(
        !store
            .put_mob_host_binding_if_absent("mob", b"two")
            .await
            .unwrap()
    );
    assert!(
        !store
            .compare_and_put_mob_host_binding("mob", b"two", b"three")
            .await
            .unwrap()
    );
    assert!(
        store
            .compare_and_put_mob_host_binding("mob", b"one", b"three")
            .await
            .unwrap()
    );
    assert!(
        !store
            .revoke_mob_host_binding("mob", b"one", b"receipt")
            .await
            .unwrap()
    );
    assert_eq!(store.load_mob_host_revocation("mob").await.unwrap(), None);
    assert!(
        store
            .revoke_mob_host_binding("mob", b"three", b"receipt")
            .await
            .unwrap()
    );
    assert_eq!(store.load_mob_host_binding("mob").await.unwrap(), None);
    assert_eq!(
        store.load_mob_host_revocation("mob").await.unwrap(),
        Some(b"receipt".to_vec())
    );
    // A fresh binding supersedes the receipt in the same batch.
    assert!(
        store
            .put_mob_host_binding_if_absent("mob", b"four")
            .await
            .unwrap()
    );
    assert_eq!(store.load_mob_host_revocation("mob").await.unwrap(), None);
    assert!(
        store
            .put_mob_host_binding_if_absent("another", b"five")
            .await
            .unwrap()
    );
    assert!(
        !store
            .delete_mob_host_binding("another", b"wrong")
            .await
            .unwrap()
    );
    let reopened = reopen(&host, opened).await;
    let store = &reopened.1;
    assert_eq!(
        store.list_mob_host_bindings().await.unwrap(),
        vec![
            ("another".to_string(), b"five".to_vec()),
            ("mob".to_string(), b"four".to_vec()),
        ]
    );
    assert!(store.list_mob_host_revocations().await.unwrap().is_empty());
    assert!(
        store
            .delete_mob_host_binding("another", b"five")
            .await
            .unwrap()
    );
    assert_eq!(store.load_mob_host_binding("another").await.unwrap(), None);
}

// Unrun until the candidate's validation.
#[tokio::test]
async fn tool_grant_rows_follow_the_sqlite_store_and_survive_a_reopen() {
    let host = Arc::new(TestHost::new());
    let opened = open(&host).await;
    crate::store::tool_grant_store_tests::tool_grant_rows_conform(&opened.1).await;
    let reopened = reopen(&host, opened).await;
    let store = &reopened.1;
    assert_eq!(
        store.load_tool_grant("g1").await.unwrap(),
        Some(b"next".to_vec())
    );
    assert_eq!(
        store.list_tool_grants("s:a").await.unwrap(),
        vec![
            ("g1".to_string(), b"next".to_vec()),
            ("g2".to_string(), b"two".to_vec()),
        ]
    );
    // The audit log loads on its first use after the reopen, and an append
    // continues after its rows.
    store
        .append_tool_grant_audit("s:a", &[b"after-reopen".to_vec()])
        .await
        .unwrap();
    assert_eq!(
        store
            .read_tool_grant_audit("s:a", Some(4), 10)
            .await
            .unwrap(),
        vec![(5, b"after-reopen".to_vec())]
    );
    assert_eq!(
        store
            .read_tool_grant_audit("s:a", None, 10)
            .await
            .unwrap()
            .len(),
        6
    );
}

// Unrun until the candidate's validation.
#[tokio::test]
async fn controller_custody_refuses_while_a_commit_is_in_flight() {
    let host = Arc::new(TestHost::new());
    let (_realm, store) = open(&host).await;
    let store = Arc::new(store);
    let gate = Arc::new(tokio::sync::Notify::new());
    *host.commit_gate.lock().unwrap() = Some(Arc::clone(&gate));
    let writer = {
        let store = Arc::clone(&store);
        tokio::spawn(async move {
            store
                .persist_input_state(
                    &LogicalRuntimeId::new("in-flight"),
                    &persistable(StoredInputState::new_accepted(InputId::new())),
                )
                .await
        })
    };
    tokio::task::yield_now().await;
    let claim = RuntimeStore::execution_custody(&*store)
        .expect("custody")
        .try_acquire_governed()
        .expect("claim");
    assert!(
        RuntimeStore::try_controller_mutation_custody(&*store, &claim).is_err(),
        "an in-flight commit holds the gate"
    );
    *host.commit_gate.lock().unwrap() = None;
    gate.notify_one();
    writer.await.unwrap().expect("the commit lands");
    assert!(RuntimeStore::try_controller_mutation_custody(&*store, &claim).is_ok());
}

// Unrun until the candidate's validation.
#[tokio::test]
async fn a_host_declared_self_fence_margin_bounds_the_write_window() {
    use meerkat_core::host_store::SelfFence;
    let timing = FenceTiming::try_with_margin(
        Duration::from_secs(5),
        Duration::from_secs(30),
        Duration::from_secs(12),
    )
    .expect("a 12 s margin on a 30 s lease at 5 s renews");
    let host = Arc::new(TestHost::with_timing(timing));
    let (realm, store) = open(&host).await;
    let window = realm.window();
    assert_eq!(window.window, Duration::from_secs(18));
    assert!(window.is_open());
    let fence = RuntimeStore::self_fence(&store).expect("a host store realm fences itself");
    assert_eq!(fence.window().window, Duration::from_secs(18));
}

// Unrun until the candidate's validation.
#[tokio::test]
async fn a_run_rebase_lands_as_one_batch_and_survives_a_reopen() {
    use meerkat_core::types::{Message, UserMessage};
    let host = Arc::new(TestHost::new());
    let opened = open(&host).await;
    let mut original = meerkat_core::Session::new();
    original.push(Message::User(UserMessage::text("hello".to_string())));
    original.push(Message::User(UserMessage::text("again".to_string())));
    let runtime_id = LogicalRuntimeId::for_session(original.id());
    opened
        .1
        .commit_session_snapshot(
            &runtime_id,
            crate::store::SerializedSessionSnapshot {
                session_snapshot: serde_json::to_vec(&original).unwrap().into(),
            },
        )
        .await
        .unwrap();
    let committed = opened
        .1
        .load_committed_whole_blob_snapshot(&runtime_id)
        .await
        .unwrap()
        .unwrap();
    let payload =
        crate::store::VerifiedCommittedWholeBlobPayload::from_committed(original.id(), committed)
            .unwrap();
    let run_id = meerkat_core::lifecycle::RunId::new();
    let mut live = original.clone();
    live.push(Message::User(UserMessage::text("next".to_string())));
    live.set_run_cursor(&meerkat_core::SessionRunCursor::new(
        run_id.clone(),
        Vec::new(),
        0,
        meerkat_core::RunResumePoint::AwaitingModel,
    ))
    .unwrap();
    let mut compacted = live.clone();
    let parent = compacted.transcript_revision().unwrap();
    let commit = compacted
        .commit_transcript_rewrite(
            meerkat_core::TranscriptRewriteSelection::MessageRange { start: 0, end: 2 },
            vec![Message::User(UserMessage::compaction_summary("compacted"))],
            meerkat_core::TranscriptRewriteReason::new("compaction"),
            Some("spi-rebase-test".to_string()),
            Some(parent),
        )
        .unwrap();
    let rebase = crate::store::PreparedWholeBlobRunRebase::prepare(
        payload,
        None,
        run_id,
        compacted,
        std::slice::from_ref(&commit),
    )
    .unwrap();
    let before = host.commit_count();
    let outcome = opened
        .1
        .commit_prepared_whole_blob_run_rebase(&runtime_id, rebase.store_parts())
        .await
        .unwrap();
    let crate::store::WholeBlobRunRebaseOutcome::Applied { committed, tail } = outcome else {
        unreachable!("the re-base applies");
    };
    assert_eq!(
        host.commit_count(),
        before + 1,
        "one batch carries the successor, its outbox rows and the new tail"
    );
    let reopened = reopen(&host, opened).await;
    let loaded_tail = reopened
        .1
        .load_whole_blob_provisional_tail(&runtime_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded_tail.authority(), &tail);
    assert_eq!(
        reopened
            .1
            .load_whole_blob_store_authority(&runtime_id)
            .await
            .unwrap()
            .unwrap(),
        committed
    );
}

// Unrun until the candidate's validation. The host store runtime store
// answers the session service's re-base check (a WholeBlob store that
// re-bases runs), so a run's closing save after a final compaction takes the
// re-base delegate, one host batch, and not the fallback, which takes no
// round commit after a rewrite.
#[tokio::test]
async fn a_closing_rebase_takes_the_delegate_and_not_the_fallback() {
    use meerkat_core::types::{Message, UserMessage};
    let host = Arc::new(TestHost::new());
    let opened = open(&host).await;
    // What the session service's checkpointer reads before it re-bases.
    assert_eq!(
        <SpiRuntimeStore as crate::store::RuntimeSessionAuthorityOps>::session_persistence_profile(
            &opened.1
        ),
        crate::store::RuntimeSessionPersistenceProfile::WholeBlobV1
    );
    assert!(RuntimeStore::supports_whole_blob_run_rebase(&opened.1));
    // A single-turn run whose one boundary compacts: its closing save
    // carries no cursor, and no earlier tail exists.
    let mut original = meerkat_core::Session::new();
    original.push(Message::User(UserMessage::text("hello".to_string())));
    original.push(Message::User(UserMessage::text("again".to_string())));
    let runtime_id = LogicalRuntimeId::for_session(original.id());
    opened
        .1
        .commit_session_snapshot(
            &runtime_id,
            crate::store::SerializedSessionSnapshot {
                session_snapshot: serde_json::to_vec(&original).unwrap().into(),
            },
        )
        .await
        .unwrap();
    let committed = opened
        .1
        .load_committed_whole_blob_snapshot(&runtime_id)
        .await
        .unwrap()
        .unwrap();
    let payload =
        crate::store::VerifiedCommittedWholeBlobPayload::from_committed(original.id(), committed)
            .unwrap();
    let run_id = meerkat_core::lifecycle::RunId::new();
    let mut live = original;
    live.push(Message::User(UserMessage::text("the question".to_string())));
    let parent = live.transcript_revision().unwrap();
    let commit = live
        .commit_transcript_rewrite(
            meerkat_core::TranscriptRewriteSelection::MessageRange { start: 0, end: 2 },
            vec![Message::User(UserMessage::compaction_summary("compacted"))],
            meerkat_core::TranscriptRewriteReason::new("compaction"),
            Some("spi-closing-rebase-test".to_string()),
            Some(parent),
        )
        .unwrap();
    assert!(live.try_run_cursor().unwrap().is_none());
    let rebase = crate::store::PreparedWholeBlobRunRebase::prepare(
        payload,
        None,
        run_id.clone(),
        live,
        std::slice::from_ref(&commit),
    )
    .unwrap();
    let before = host.commit_count();
    let outcome = opened
        .1
        .commit_prepared_whole_blob_run_rebase(&runtime_id, rebase.store_parts())
        .await
        .unwrap();
    let crate::store::WholeBlobRunRebaseOutcome::Applied { committed, tail } = outcome else {
        unreachable!("the closing save re-bases");
    };
    assert_eq!(host.commit_count(), before + 1, "one host batch");
    assert_eq!(tail.run_id(), &run_id);
    assert_eq!(tail.candidate_sequence(), 1);
    assert_eq!(tail.candidate_blob_sha256(), committed.blob_sha256());
    let reopened = reopen(&host, opened).await;
    assert_eq!(
        reopened
            .1
            .load_whole_blob_provisional_tail(&runtime_id)
            .await
            .unwrap()
            .unwrap()
            .authority(),
        &tail
    );
    assert_eq!(
        reopened
            .1
            .load_whole_blob_store_authority(&runtime_id)
            .await
            .unwrap()
            .unwrap(),
        committed
    );
}

// Unrun until the candidate's validation. The re-acquire rule, with the
// visibility-lag counterexample: renews fail from some point, the lease
// lapses, another process acquires, and the old writer's connection comes
// back. A renew that now succeeded would prove nothing (the new acquire may
// have missed the last renew), so the old writer never resumes on a renew:
// it tries a fresh acquire, finds the new holder, and stays down.
#[tokio::test(start_paused = true)]
async fn a_writer_that_stopped_never_resumes_on_a_later_renew() {
    let timing = FenceTiming::try_new(Duration::from_secs(1), Duration::from_secs(3)).unwrap();
    let host = Arc::new(TestHost::with_timing(timing));
    let (old_realm, old_store) = open(&host).await;
    host.faults.lock().unwrap().renew_unavailable = 1;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(
        !old_realm.lease_is_live(),
        "the failed renew stopped the old writer"
    );
    // Its lease lapses and a new process acquires.
    host.expire_lease();
    let (new_realm, _new_store) = open(&host).await;
    assert!(new_realm.fence() > old_realm.fence());
    // The old writer's next tick re-acquires, meets the new holder, and
    // poisons itself; it never wrote again.
    let before = host.commit_count();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(!old_realm.lease_is_live());
    let refused = old_store
        .persist_input_state(
            &LogicalRuntimeId::new("late"),
            &persistable(StoredInputState::new_accepted(InputId::new())),
        )
        .await;
    assert!(refused.is_err());
    assert!(old_realm.realm_poisoned().is_some());
    assert_eq!(host.commit_count(), before);
}

// Unrun until the candidate's validation.
#[tokio::test(start_paused = true)]
async fn a_stopped_writer_reacquires_and_resumes_when_nothing_changed() {
    let timing = FenceTiming::try_new(Duration::from_secs(1), Duration::from_secs(3)).unwrap();
    let host = Arc::new(TestHost::with_timing(timing));
    let (realm, store) = open(&host).await;
    let first = realm.fence();
    host.faults.lock().unwrap().renew_unavailable = 1;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(!realm.lease_is_live());
    // The next tick re-acquires over its own still-recorded lease.
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(realm.lease_is_live());
    assert!(realm.fence() > first, "a fresh token");
    store
        .persist_input_state(
            &LogicalRuntimeId::new("resumed"),
            &persistable(StoredInputState::new_accepted(InputId::new())),
        )
        .await
        .expect("writes resume under the new token");
}

// Unrun until the candidate's validation.
#[tokio::test(start_paused = true)]
async fn a_stopped_writer_finds_a_foreign_write_and_stays_down() {
    let timing = FenceTiming::try_new(Duration::from_secs(1), Duration::from_secs(3)).unwrap();
    let host = Arc::new(TestHost::with_timing(timing));
    let (old_realm, old_store) = open(&host).await;
    host.faults.lock().unwrap().renew_unavailable = 1;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    // Another process writes while the old writer is stopped, then dies.
    host.expire_lease();
    {
        let (other_realm, other_store) = open(&host).await;
        other_store
            .persist_input_state(
                &LogicalRuntimeId::new("foreign"),
                &persistable(StoredInputState::new_accepted(InputId::new())),
            )
            .await
            .unwrap();
        drop(other_store);
        drop(other_realm);
    }
    host.expire_lease();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(
        old_realm.realm_poisoned().is_some(),
        "a foreign batch since the stop keeps the old writer down"
    );
    assert!(
        old_store
            .load_input_states(&LogicalRuntimeId::new("foreign"))
            .await
            .is_err()
    );
}

// Unrun until the candidate's validation. A renew slower than the
// renew interval counts as failed even under a longer configured timeout,
// so renew starts stay one interval apart.
#[tokio::test(start_paused = true)]
async fn a_renew_slower_than_the_interval_stops_the_writer() {
    let timing = FenceTiming::try_new(Duration::from_secs(1), Duration::from_secs(3)).unwrap();
    let host = Arc::new(TestHost::with_timing(timing));
    let config = SpiRealmConfig {
        renew_timeout: Duration::from_secs(5),
        ..SpiRealmConfig::default()
    };
    let realm = SpiRealm::open(Arc::clone(&host) as _, config)
        .await
        .unwrap();
    host.faults.lock().unwrap().renew_delay = Some(Duration::from_millis(1500));
    // The renew that starts at 1 s is cut off at 2 s; the configured 5 s
    // would have let it succeed at 2.5 s.
    tokio::time::sleep(Duration::from_millis(2100)).await;
    assert!(!realm.lease_is_live(), "the slow renew counts as failed");
}

// Unrun until the candidate's validation. A re-acquire whose listing
// outlasts the window renews once with the new token before it resumes, so
// the writer resumes with an open window instead of stopping again.
#[tokio::test(start_paused = true)]
async fn a_slow_reacquire_listing_still_resumes_with_an_open_window() {
    let timing = FenceTiming::try_new(Duration::from_secs(1), Duration::from_secs(3)).unwrap();
    let host = Arc::new(TestHost::with_timing(timing));
    let (realm, store) = open(&host).await;
    let first = realm.fence();
    {
        let mut faults = host.faults.lock().unwrap();
        faults.renew_unavailable = 1;
        faults.list_delay = Some(Duration::from_millis(2500));
    }
    // 1 s: the renew fails. 2 s: the re-acquire starts, and its listing
    // takes until 4.5 s, past the 2 s window counted from the acquire.
    tokio::time::sleep(Duration::from_millis(4600)).await;
    assert!(realm.fence() > first, "a fresh token");
    assert!(
        realm.lease_is_live(),
        "the renew after the listing restarted the window"
    );
    store
        .persist_input_state(
            &LogicalRuntimeId::new("resumed"),
            &persistable(StoredInputState::new_accepted(InputId::new())),
        )
        .await
        .expect("writes resume");
}

// Unrun until the candidate's validation. A batch that landed while
// its commit kept answering errors poisons its scope only; the next
// re-acquire accepts that scope at either version instead of reading the
// batch as another writer's and poisoning the realm.
#[tokio::test(start_paused = true)]
async fn a_batch_of_unknown_outcome_does_not_poison_the_realm_at_a_reacquire() {
    let timing = FenceTiming::try_new(Duration::from_secs(1), Duration::from_secs(3)).unwrap();
    let host = Arc::new(TestHost::with_timing(timing));
    let config = SpiRealmConfig {
        commit_retry_budget: Duration::from_millis(500),
        ..SpiRealmConfig::default()
    };
    let realm = SpiRealm::open(Arc::clone(&host) as _, config)
        .await
        .unwrap();
    let store = SpiRuntimeStore::open(Arc::clone(&realm)).await.unwrap();
    {
        let mut faults = host.faults.lock().unwrap();
        faults.indeterminate_after_apply = 1;
        faults.load_head_unavailable = 1000;
        faults.landed_retry_unavailable = 1000;
    }
    let unknown = LogicalRuntimeId::new("unknown");
    let refused = store
        .persist_input_state(
            &unknown,
            &persistable(StoredInputState::new_accepted(InputId::new())),
        )
        .await;
    assert!(refused.is_err(), "the outcome stayed unknown");
    {
        let mut faults = host.faults.lock().unwrap();
        faults.load_head_unavailable = 0;
        faults.landed_retry_unavailable = 0;
        faults.renew_unavailable = 1;
    }
    // The renew at 1 s fails; the tick at 2 s re-acquires.
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert!(
        realm.realm_poisoned().is_none(),
        "{:?}",
        realm.realm_poisoned()
    );
    assert!(realm.lease_is_live());
    assert!(
        store.load_input_states(&unknown).await.is_err(),
        "the unknown batch's scope stays poisoned"
    );
    store
        .persist_input_state(
            &LogicalRuntimeId::new("other"),
            &persistable(StoredInputState::new_accepted(InputId::new())),
        )
        .await
        .expect("another runtime writes");
}

// Unrun until the candidate's validation. A call dropped while its
// batch is in flight still lands it and adopts the new version, so the next
// call on the scope commits on top of it instead of conflicting.
#[tokio::test]
async fn a_call_dropped_during_its_commit_still_lands_and_adopts_it() {
    let host = Arc::new(TestHost::new());
    let (realm, store) = open(&host).await;
    let store = Arc::new(store);
    let gate = Arc::new(tokio::sync::Notify::new());
    *host.commit_gate.lock().unwrap() = Some(Arc::clone(&gate));
    let runtime = LogicalRuntimeId::new("dropped");
    let call = {
        let store = Arc::clone(&store);
        let runtime = runtime.clone();
        tokio::spawn(async move {
            store
                .persist_input_state(
                    &runtime,
                    &persistable(StoredInputState::new_accepted(InputId::new())),
                )
                .await
        })
    };
    while host.commits_started.load(Ordering::Acquire) == 0 {
        tokio::task::yield_now().await;
    }
    call.abort();
    assert!(call.await.unwrap_err().is_cancelled());
    *host.commit_gate.lock().unwrap() = None;
    gate.notify_one();
    // The handed-off commit settles on its own task; the next call waits
    // for the scope's lock, then commits on top of it.
    store
        .persist_input_state(
            &runtime,
            &persistable(StoredInputState::new_accepted(InputId::new())),
        )
        .await
        .expect("the next call commits on top of the dropped call's batch");
    assert!(realm.realm_poisoned().is_none());
    let store = Arc::try_unwrap(store).expect("the dropped call released the store");
    let (_realm, reopened) = reopen(&host, (realm, store)).await;
    assert_eq!(
        reopened.load_input_states(&runtime).await.unwrap().len(),
        2,
        "both batches are on the host"
    );
}

// Unrun until the candidate's validation. Scopes dropped while armed
// (a call dropped during its in-memory change) are poisoned; disarmed, they
// are not.
#[tokio::test]
async fn armed_scopes_poison_when_dropped_and_only_then() {
    let host = Arc::new(TestHost::new());
    let realm = SpiRealm::open(Arc::clone(&host) as _, SpiRealmConfig::default())
        .await
        .unwrap();
    let armed = ScopeId::new(TEST_FAMILY, "armed");
    let calm = ScopeId::new(TEST_FAMILY, "calm");
    let mut scopes = ArmedScopes::new(
        Arc::clone(&realm),
        realm.lock_scopes(std::slice::from_ref(&armed)).await,
    );
    scopes.arm();
    drop(scopes);
    let mut scopes = ArmedScopes::new(
        Arc::clone(&realm),
        realm.lock_scopes(std::slice::from_ref(&calm)).await,
    );
    scopes.arm();
    scopes.disarm();
    drop(scopes);
    assert!(realm.cell(&armed).lock().await.poison().is_some());
    assert!(realm.cell(&calm).lock().await.poison().is_none());
}

// Unrun until the candidate's validation. A large value moved to
// another entry in one commit (a boundary promotion moves the provisional
// tail's body to the session entry) keeps its chunk instead of uploading
// the bytes again.
#[tokio::test]
async fn a_large_value_moved_between_entries_keeps_its_chunk() {
    let host = Arc::new(TestHost::new());
    let realm = SpiRealm::open(Arc::clone(&host) as _, SpiRealmConfig::default())
        .await
        .unwrap();
    let id = ScopeId::new(TEST_FAMILY, "promote");
    let body = Arc::new(vec![3_u8; INLINE_VALUE_LIMIT + 1]);
    commit_one(
        &realm,
        &id,
        ScopeDelta::Changes(vec![("tail".into(), Some(Arc::clone(&body)))]),
    )
    .await
    .unwrap();
    let chunk_names = |host: &TestHost| -> Vec<String> {
        host.scopes_of(ScopeKind::Record)
            .keys()
            .filter(|name| name.starts_with("ch:"))
            .cloned()
            .collect()
    };
    let before = chunk_names(&host);
    assert_eq!(before.len(), 1);
    let commits = host.commit_count();
    // Equal bytes in another allocation: the digest decides.
    commit_one(
        &realm,
        &id,
        ScopeDelta::Changes(vec![
            ("tail".into(), None),
            ("session".into(), Some(Arc::new(body.as_ref().clone()))),
        ]),
    )
    .await
    .unwrap();
    assert_eq!(
        chunk_names(&host),
        before,
        "the session entry took the chunk over"
    );
    assert_eq!(
        host.commit_count(),
        commits + 1,
        "one batch and no chunk upload"
    );
    let (_realm, image) = reload(&host, realm).await;
    assert_eq!(image.get("session"), Some(&body));
    assert!(!image.contains_key("tail"));
}

// Unrun until the candidate's validation. A change that cannot fit one
// batch, even with every large row set spilled, is refused typed, and the
// scopes it would have changed are poisoned.
#[tokio::test]
async fn a_change_that_cannot_fit_one_batch_is_refused_typed() {
    let host = Arc::new(TestHost::new());
    let realm = SpiRealm::open(Arc::clone(&host) as _, SpiRealmConfig::default())
        .await
        .unwrap();
    // 40 scopes with a 200 KiB row each: no row set is large enough to
    // spill, and together they pass the batch budget.
    let ids: Vec<ScopeId> = (0..40)
        .map(|n| ScopeId::new(TEST_FAMILY, format!("wide-{n:02}")))
        .collect();
    let mut guards = realm.lock_scopes(&ids).await;
    let mut states: Vec<_> = guards.iter_mut().map(|guard| &mut **guard).collect();
    let value = Arc::new(vec![1_u8; 200 * 1024]);
    let deltas = ids
        .iter()
        .map(|_| ScopeDelta::Changes(vec![("row".into(), Some(Arc::clone(&value)))]))
        .collect();
    let refused = realm.commit(&mut states, deltas).await;
    assert!(
        matches!(refused, Err(SpiError::TooLarge { .. })),
        "{refused:?}"
    );
    drop(states);
    assert!(guards.iter().all(|guard| guard.poison().is_some()));
    assert_eq!(host.commit_count(), 0);
}

// Unrun until the candidate's validation. A failed batch poisons the
// scopes it would have changed, not a scope locked with it whose delta
// changes nothing.
#[tokio::test]
async fn a_failed_batch_leaves_an_unchanged_locked_scope_serving() {
    let host = Arc::new(TestHost::new());
    let realm = SpiRealm::open(Arc::clone(&host) as _, SpiRealmConfig::default())
        .await
        .unwrap();
    let read = ScopeId::new(TEST_FAMILY, "read");
    let written = ScopeId::new(TEST_FAMILY, "written");
    let same = || ScopeDelta::Changes(vec![("pin".into(), Some(Arc::new(b"same".to_vec())))]);
    commit_one(&realm, &read, same()).await.unwrap();
    host.faults.lock().unwrap().conflict = 1;
    // Locked in scope order: "read" before "written".
    let mut guards = realm.lock_scopes(&[read, written]).await;
    let mut states: Vec<_> = guards.iter_mut().map(|guard| &mut **guard).collect();
    let refused = realm
        .commit(
            &mut states,
            vec![
                same(),
                ScopeDelta::Changes(vec![("pin".into(), Some(Arc::new(b"new".to_vec())))]),
            ],
        )
        .await;
    assert!(refused.is_err());
    drop(states);
    assert!(
        guards[0].poison().is_none(),
        "the unchanged scope still serves"
    );
    assert!(guards[1].poison().is_some());
}

// Unrun until the candidate's validation. The check a host runs against its
// own store passes over Core's own test host. Each open after the first ends
// the dropped process's lease, as a crash restart whose lease ran out, so
// the restarts take over without waiting. Two worker threads keep the
// realm's renew task running beside the check's 20 MiB encodings.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_large_document_check_passes_over_the_test_host() {
    use std::sync::atomic::AtomicU64;

    use meerkat_core::host_store::{HostStore, HostStoreError};

    use super::chapter::{HostStoreOpener, large_documents};

    struct Opener {
        host: Arc<TestHost>,
        opens: AtomicU64,
    }

    #[async_trait::async_trait]
    impl HostStoreOpener for Opener {
        async fn open(&self) -> Result<Arc<dyn HostStore>, HostStoreError> {
            if self.opens.fetch_add(1, Ordering::SeqCst) > 0 {
                self.host.expire_lease();
            }
            Ok(Arc::clone(&self.host) as Arc<dyn HostStore>)
        }
    }

    let opener = Opener {
        host: Arc::new(TestHost::new()),
        opens: AtomicU64::new(0),
    };
    assert_eq!(large_documents(&opener).await, Ok(()));
    assert_eq!(
        opener.opens.load(Ordering::SeqCst),
        3,
        "the check opens a process, reopens after the round commit and restarts after the re-base"
    );
}

/// The host store adapter commits a session boundary through the in-memory
/// store's own implementation, so a run's mid-run checkpoint receipt commits
/// beside the run's WholeBlob provisional tail over a host store as well, and
/// a reopen reads both from the host; a receipt-only success and another
/// run's checkpoint stay refused beside the tail.
#[tokio::test]
async fn a_mid_run_checkpoint_commits_beside_its_runs_tail_over_the_host_store() {
    use meerkat_core::lifecycle::run_primitive::RunApplyBoundary;
    use meerkat_core::lifecycle::{RunBoundaryReceipt, RunId};
    use meerkat_core::types::{Message, UserMessage};

    use crate::store::{
        PreparedRuntimeSessionCommit, PreparedWholeBlobProvisionalTail, SerializedSessionSnapshot,
    };

    let checkpoint = |run_id: &RunId, sequence: u64| RunBoundaryReceipt {
        run_id: run_id.clone(),
        boundary: RunApplyBoundary::RunCheckpoint,
        contributing_input_ids: Vec::new(),
        conversation_digest: None,
        message_count: 0,
        sequence,
        owner_contributions: vec!["live-delegation-steer:continuation-1".to_string()],
    };
    let host = Arc::new(TestHost::new());
    let opened = open(&host).await;
    let mut session = meerkat_core::Session::new();
    session.push(Message::User(UserMessage::text("the committed base")));
    let session_id = session.id().clone();
    let runtime_id = LogicalRuntimeId::for_session(&session_id);
    opened
        .1
        .commit_session_snapshot(
            &runtime_id,
            SerializedSessionSnapshot {
                session_snapshot: Arc::new(serde_json::to_vec(&session).unwrap()),
            },
        )
        .await
        .expect("the committed base");
    let base = opened
        .1
        .load_whole_blob_store_authority(&runtime_id)
        .await
        .unwrap()
        .expect("a committed base");
    session.push(Message::User(UserMessage::text("the run's first round")));
    let run_id = RunId::new();
    let tail = opened
        .1
        .write_prepared_whole_blob_provisional_tail(
            &runtime_id,
            PreparedWholeBlobProvisionalTail::prepare_from_session(
                base,
                run_id.clone(),
                1,
                &session,
            )
            .unwrap(),
        )
        .await
        .expect("the round commit's tail");
    let receipt = checkpoint(&run_id, 1);
    opened
        .1
        .commit_prepared_session_boundary(
            &runtime_id,
            PreparedRuntimeSessionCommit::run_checkpoint(
                receipt.clone(),
                Vec::new(),
                session_id.clone(),
            )
            .unwrap(),
        )
        .await
        .expect("a mid-run checkpoint commits beside its run's tail");
    for refused in [
        PreparedRuntimeSessionCommit::success(
            None,
            checkpoint(&run_id, 2),
            Vec::new(),
            Some(session_id.clone()),
        ),
        PreparedRuntimeSessionCommit::run_checkpoint(
            checkpoint(&RunId::new(), 1),
            Vec::new(),
            session_id.clone(),
        )
        .unwrap(),
    ] {
        let error = opened
            .1
            .commit_prepared_session_boundary(&runtime_id, refused)
            .await
            .expect_err("this receipt-only boundary would bypass the candidate");
        assert!(
            matches!(
                error,
                RuntimeStoreError::SessionPersistenceAuthorityConflict { .. }
            ),
            "{error:?}"
        );
    }

    let reopened = reopen(&host, opened).await;
    assert_eq!(
        reopened
            .1
            .load_boundary_receipt(&runtime_id, &run_id, 1)
            .await
            .unwrap(),
        Some(receipt),
        "the checkpoint reached the host"
    );
    assert_eq!(
        reopened
            .1
            .load_boundary_receipt(&runtime_id, &run_id, 2)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        reopened
            .1
            .load_whole_blob_provisional_tail(&runtime_id)
            .await
            .unwrap()
            .map(|loaded| loaded.authority().clone()),
        Some(tail),
        "the tail stands on the host"
    );
}

// Unrun until the candidate's validation. A create-only import through the
// host store adapter, which forwards the in-memory store's import: one batch
// carries the document, its catalog entry and its pending compaction
// intent's outbox row, and a reopen reads them all from the host. Importing
// the same bytes again, or other bytes, writes no batch.
#[tokio::test]
async fn an_import_lands_as_one_batch_with_its_outbox_row_and_survives_a_reopen() {
    use meerkat_core::types::{Message, UserMessage};

    use crate::store::{SerializedSessionSnapshot, WholeBlobSessionImport};

    let host = Arc::new(TestHost::new());
    let opened = open(&host).await;
    let (session, intent) = crate::store::memory::tests::session_with_compaction_intent();
    let runtime_id = LogicalRuntimeId::for_session(session.id());
    let bytes = serde_json::to_vec(&session).unwrap();
    let before = host.commit_count();
    let outcome = opened
        .1
        .import_whole_blob_session(
            &runtime_id,
            SerializedSessionSnapshot {
                session_snapshot: Arc::new(bytes.clone()),
            },
        )
        .await
        .expect("the import lands");
    let WholeBlobSessionImport::Created(authority) = outcome else {
        unreachable!("the first import creates the session");
    };
    assert_eq!(
        host.commit_count(),
        before + 1,
        "one batch carries the document, its catalog entry and its outbox row"
    );
    assert_eq!(
        opened
            .1
            .import_whole_blob_session(
                &runtime_id,
                SerializedSessionSnapshot {
                    session_snapshot: Arc::new(bytes.clone()),
                },
            )
            .await
            .expect("the same bytes again"),
        WholeBlobSessionImport::AlreadyCurrent(authority.clone())
    );
    let mut other = session.clone();
    other.push(Message::User(UserMessage::text("other bytes")));
    let refused = opened
        .1
        .import_whole_blob_session(
            &runtime_id,
            SerializedSessionSnapshot {
                session_snapshot: Arc::new(serde_json::to_vec(&other).unwrap()),
            },
        )
        .await;
    assert!(
        matches!(
            refused,
            Err(RuntimeStoreError::SessionPersistenceAuthorityConflict { .. })
        ),
        "{refused:?}"
    );
    assert_eq!(
        host.commit_count(),
        before + 1,
        "neither the repeat nor the refusal wrote a batch"
    );

    let reopened = reopen(&host, opened).await;
    let committed = reopened
        .1
        .load_committed_whole_blob_snapshot(&runtime_id)
        .await
        .unwrap()
        .expect("the imported document is on the host");
    assert_eq!(committed.bytes(), bytes.as_slice());
    assert_eq!(committed.authority(), &authority);
    assert_eq!(
        reopened
            .1
            .load_pending_compaction_projections(&runtime_id)
            .await
            .unwrap(),
        vec![intent],
        "the pending intent's outbox row is on the host"
    );
    assert!(
        reopened
            .1
            .load_runtime_session_catalog_entry(&runtime_id)
            .await
            .unwrap()
            .is_some(),
        "the catalog entry is on the host"
    );
}

// Unrun until the candidate's validation. The obligation scan answers from
// memory: before and after a restart it lists exactly the runtimes with
// recovery work and reads nothing from the host, and after the restart the
// open has loaded the body of exactly the runtimes it lists.
#[tokio::test]
async fn boot_obligation_scan_spi_store_lists_the_runtimes_with_work_without_host_reads() {
    use crate::store::recovery_obligation::scan_contract;

    let host = Arc::new(TestHost::new());
    let opened = open(&host).await;
    let fixture = scan_contract::seed(&opened.1).await;
    let reads = host.read_count();
    scan_contract::assert_lists_exactly(&opened.1, &fixture).await;
    assert_eq!(
        host.read_count(),
        reads,
        "the scan reads nothing from the host"
    );

    let reopened = reopen(&host, opened).await;
    let reads = host.read_count();
    scan_contract::assert_lists_exactly(&reopened.1, &fixture).await;
    assert_eq!(
        host.read_count(),
        reads,
        "after a restart the scan still reads nothing from the host"
    );
    for runtime in fixture.runtimes() {
        assert_eq!(
            reopened.1.runtime_loaded_for_test(&runtime.0).await,
            fixture.listed.contains(runtime),
            "the open loads the body of {runtime:?} exactly when the scan lists it"
        );
    }
}

// Unrun until the candidate's validation. Boot facts without the folded
// form's rollback-compatibility marker (an earlier writer's compact form, or
// a JSON normaliser's rewrite, which drops it) may leave the ops lifecycle's
// work out: the scan lists their runtime as holding work, fail closed,
// without the open loading its body, until the runtime's next commit pins
// the folded form. The runtime holds rows (a terminal input under an idle
// lifecycle: no recovery work), so its body is one the open would have to
// load: a runtime of pinned entries alone is resident once they are read.
#[tokio::test]
async fn boot_obligation_scan_spi_store_fails_closed_on_a_head_pinned_before_the_ops_fold() {
    let host = Arc::new(TestHost::new());
    let opened = open(&host).await;
    let quiet = LogicalRuntimeId::new("quiet");
    opened
        .1
        .compare_and_swap_runtime_delivery_authority(
            &quiet,
            None,
            RuntimeDeliveryAuthorityRecord::from_parts(1, br#"{"state":1}"#.to_vec()),
            None,
        )
        .await
        .expect("delivery cas");
    opened
        .1
        .commit_machine_lifecycle(&quiet, idle_lifecycle(&quiet), &[])
        .await
        .expect("the runtime is registered");
    let mut terminal = StoredInputState::new_accepted(InputId::new());
    terminal.seed.phase = InputLifecycleState::Consumed;
    terminal.seed.terminal_outcome = Some(InputTerminalOutcome::Consumed);
    terminal.seed.recovery_lane = None;
    opened
        .1
        .persist_input_state(&quiet, &persistable(terminal))
        .await
        .expect("a terminal input row");
    assert!(
        !opened
            .1
            .list_runtimes_with_recovery_work()
            .await
            .expect("the scan answers")
            .contains(&quiet),
        "a folded head with no work is not listed"
    );

    // The same facts as a JSON normaliser rewrites them: the marker is
    // gone, as in the compact form an earlier writer pinned.
    let id = ScopeId::new(super::RUNTIME_FAMILY, quiet.0.clone());
    let folded = {
        let cell = opened.0.cell(&id);
        let state = cell.lock().await;
        let pinned = opened
            .0
            .pinned_values(&state)
            .await
            .expect("the pinned head");
        Arc::clone(pinned.get("boot").expect("the boot facts are pinned"))
    };
    assert_eq!(
        folded.last(),
        Some(&b'\n'),
        "this writer pins the folded form"
    );
    let normalised =
        serde_json::to_vec(&serde_json::from_slice::<serde_json::Value>(&folded).unwrap()).unwrap();
    commit_one(
        &opened.0,
        &id,
        ScopeDelta::Changes(vec![("boot".into(), Some(Arc::new(normalised)))]),
    )
    .await
    .expect("a head that lost the marker");

    let reopened = reopen(&host, opened).await;
    assert!(
        reopened
            .1
            .list_runtimes_with_recovery_work()
            .await
            .expect("the scan answers")
            .contains(&quiet),
        "a head without the marker is listed, fail closed"
    );
    let window = host
        .window_of(ScopeKind::Session, &runtime_scope_prefix(&quiet))
        .expect("the runtime's scope is on the host");
    assert!(
        window.end > window.start,
        "the runtime's head names rows, so its body is not resident unless loaded"
    );
    assert!(
        !reopened.1.runtime_loaded_for_test("quiet").await,
        "the open does not load its body for it"
    );

    reopened
        .1
        .compare_and_swap_runtime_delivery_authority(
            &quiet,
            Some(1),
            RuntimeDeliveryAuthorityRecord::from_parts(2, br#"{"state":2}"#.to_vec()),
            None,
        )
        .await
        .expect("the runtime's next commit");
    assert!(
        !reopened
            .1
            .list_runtimes_with_recovery_work()
            .await
            .expect("the scan answers")
            .contains(&quiet),
        "its next commit pins the folded form"
    );
    let restarted = reopen(&host, reopened).await;
    assert!(
        !restarted
            .1
            .list_runtimes_with_recovery_work()
            .await
            .expect("the scan answers")
            .contains(&quiet),
        "and the head stays folded across a restart"
    );
}

// Unrun until the candidate's validation. A retired runtime is archived: its
// lifecycle names Retired (a retire never writes the catalog terminal). So a
// head of it whose boot facts lost the folded form's marker is not listed
// fail closed: an archived runtime holds no recovery work, whatever its
// facts might leave out.
#[tokio::test]
async fn boot_obligation_scan_spi_store_does_not_fail_closed_on_a_retired_runtimes_head() {
    use crate::store::recovery_obligation::scan_contract;

    let host = Arc::new(TestHost::new());
    let opened = open(&host).await;
    let retired = LogicalRuntimeId::new("retired");
    opened
        .1
        .commit_machine_lifecycle(
            &retired,
            scan_contract::ended_lifecycle(RuntimeState::Retired),
            &[],
        )
        .await
        .expect("the retired lifecycle");

    // The facts as a JSON normaliser rewrites them: the marker is gone.
    let id = ScopeId::new(super::RUNTIME_FAMILY, retired.0.clone());
    let folded = {
        let cell = opened.0.cell(&id);
        let state = cell.lock().await;
        let pinned = opened
            .0
            .pinned_values(&state)
            .await
            .expect("the pinned head");
        Arc::clone(pinned.get("boot").expect("the boot facts are pinned"))
    };
    let normalised =
        serde_json::to_vec(&serde_json::from_slice::<serde_json::Value>(&folded).unwrap()).unwrap();
    commit_one(
        &opened.0,
        &id,
        ScopeDelta::Changes(vec![("boot".into(), Some(Arc::new(normalised)))]),
    )
    .await
    .expect("a head that lost the marker");

    let reopened = reopen(&host, opened).await;
    assert!(
        !reopened
            .1
            .list_runtimes_with_recovery_work()
            .await
            .expect("the scan answers")
            .contains(&retired),
        "a retired runtime's head is not listed, though its facts predate the fold"
    );
}

// Unrun until the candidate's validation. The open's body-load decision
// over a runtime's pinned head and the scan answer the same predicate.
#[tokio::test]
async fn boot_obligation_scan_spi_store_agrees_with_the_open_on_each_pinned_head() {
    use crate::store::recovery_obligation::scan_contract;

    let host = Arc::new(TestHost::new());
    let opened = open(&host).await;
    let fixture = scan_contract::seed(&opened.1).await;
    let (realm, store) = reopen(&host, opened).await;
    let listed: std::collections::HashSet<LogicalRuntimeId> = store
        .list_runtimes_with_recovery_work()
        .await
        .expect("the scan answers")
        .into_iter()
        .collect();
    for runtime in fixture.runtimes() {
        let cell = realm.cell(&ScopeId::new(super::RUNTIME_FAMILY, runtime.0.clone()));
        let state = cell.lock().await;
        let pinned = realm.pinned_values(&state).await.expect("the pinned head");
        assert_eq!(
            super::runtime::needs_body_at_boot(&pinned).expect("the pinned head decodes"),
            listed.contains(runtime),
            "the open and the scan decide {runtime:?} alike"
        );
    }
}

// ---------------------------------------------------------------------------
// What a realm's history costs a host store: the reads of an open and the
// rows a runtime keeps, over a host that records every call.
// ---------------------------------------------------------------------------

/// Completed operations the settling registry keeps (a runtime keeps 256 by
/// default). With [`SETTLED_RESULT_BYTES`] of result each, a snapshot weighs
/// some tens of KiB, as a busy runtime's does, and stays inline.
const SETTLED_RETAINED: usize = 24;

/// The result text of each settled operation, in bytes.
const SETTLED_RESULT_BYTES: usize = 512;

/// Settled operations between two persisted snapshots in a history. The
/// runtime persists one at every terminal transition (and one more when it
/// injects the completion); each snapshot supersedes every earlier one, so a
/// history of fewer, equal snapshots grows the rows the same way and costs a
/// row less time.
const SETTLED_PER_SNAPSHOT: usize = 50;

/// The quiet runtimes of a history row: registered, with settled operations
/// and nothing else.
const QUIET_RUNTIMES: usize = 2;

/// The retention rule these rows hold the realm to: a scope's visible rows
/// stay within `max(REBASE_FACTOR x live bytes, REBASE_FLOOR)` plus its last
/// commit's rows. The rows pin the values instead of reading the realm's
/// constants, so a floor that grows back fails them.
const REBASE_FACTOR: u64 = 4;

/// See [`REBASE_FACTOR`].
const REBASE_FLOOR: u64 = 256 * 1024;

/// A runtime's ops lifecycle registry settling background operations one
/// after another: each is registered, runs, completes, and has its
/// completion injected into its owner, so a snapshot taken between two
/// operations holds no recovery work.
struct SettlingOps {
    registry: RuntimeOpsLifecycleRegistry,
    feed: Arc<dyn CompletionFeed>,
    epoch: RuntimeEpochId,
    owner: SessionId,
    completed: usize,
}

impl SettlingOps {
    fn new() -> Self {
        let registry = RuntimeOpsLifecycleRegistry::with_config(OpsLifecycleConfig {
            max_completed: SETTLED_RETAINED,
            max_concurrent: None,
        });
        let feed = registry.completion_feed_handle();
        Self {
            registry,
            feed,
            epoch: RuntimeEpochId::new(),
            owner: SessionId::new(),
            completed: 0,
        }
    }

    /// Register, run and complete one background operation.
    fn complete_one(&mut self) {
        let id = OperationId::new();
        self.registry
            .register_operation(OperationSpec {
                id: id.clone(),
                kind: OperationKind::BackgroundToolOp,
                owner_session_id: self.owner.clone(),
                display_name: format!("settled operation {}", self.completed),
                source_label: "host-store-history".into(),
                operation_source: None,
                child_session_id: None,
                expect_peer_channel: false,
            })
            .expect("register the operation");
        self.registry
            .provisioning_succeeded(&id)
            .expect("start the operation");
        self.registry
            .complete_operation(
                &id,
                OperationResult {
                    id: id.clone(),
                    content: "r".repeat(SETTLED_RESULT_BYTES),
                    is_error: false,
                    duration_ms: 1,
                    tokens_used: 0,
                },
            )
            .expect("complete the operation");
        self.completed += 1;
    }

    /// Settle one more operation: complete it and inject its completion.
    fn settle_one(&mut self) {
        self.complete_one();
        let completion = self.feed.watermark();
        self.registry
            .advance_completion_cursor(CompletionCursorConsumer::RuntimeInjected, completion, None)
            .expect("inject the completion");
    }

    /// Settle `count` more operations.
    fn settle(&mut self, count: usize) {
        for _ in 0..count {
            self.settle_one();
        }
    }

    /// Owe the work a restart must act on: one completion no consumer took,
    /// and one running detached job wait.
    fn owe_work(&mut self) {
        self.complete_one();
        let wait = OperationId::for_detached_job_wait(&self.owner, "history-realm", "history-job");
        self.registry
            .register_operation(OperationSpec {
                id: wait.clone(),
                kind: OperationKind::DetachedJobWait,
                owner_session_id: self.owner.clone(),
                display_name: "await detached job".into(),
                source_label: "await_job".into(),
                operation_source: Some(OperationSource::detached_job(
                    "history-realm",
                    "history-job",
                )),
                child_session_id: None,
                expect_peer_channel: false,
            })
            .expect("register the job wait");
        self.registry
            .provisioning_succeeded(&wait)
            .expect("the job wait runs");
    }

    /// The snapshot the runtime persists now.
    fn snapshot(&self) -> PersistedOpsSnapshot {
        self.registry
            .capture_persistence_snapshot(self.epoch.clone(), &EpochCursorState::new())
            .expect("the ops lifecycle snapshot")
    }
}

/// An idle lifecycle bound to `runtime`: what a registered runtime with no
/// run commits.
fn idle_lifecycle(runtime: &LogicalRuntimeId) -> MachineLifecycleCommit {
    MachineLifecycleCommit::new_with_binding_run_and_unregister_progress(
        RuntimeState::Idle,
        MachineLifecycleBindingFacts::new(
            Some(runtime.0.clone()),
            Some(1),
            Some(1),
            Some("epoch-1".to_string()),
        ),
        MachineLifecycleRunFacts::new(None, Some(MachineLifecyclePreRunPhase::Idle)),
        SupervisorAuthoritySnapshot::UnboundNoReceipt,
        None,
    )
}

/// Write the history of `settled` settled operations into `runtime`: a
/// snapshot every [`SETTLED_PER_SNAPSHOT`] operations and one after the last.
async fn write_settled_history(
    store: &SpiRuntimeStore,
    runtime: &LogicalRuntimeId,
    ops: &mut SettlingOps,
    settled: usize,
) {
    let mut done = 0;
    while done < settled {
        let batch = SETTLED_PER_SNAPSHOT.min(settled - done);
        ops.settle(batch);
        done += batch;
        store
            .persist_ops_lifecycle(runtime, &ops.snapshot())
            .await
            .expect("the snapshot persists");
    }
}

/// [`QUIET_RUNTIMES`] registered runtimes, each with a history of `settled`
/// settled operations and nothing else.
async fn quiet_runtimes_with_history(
    store: &SpiRuntimeStore,
    settled: usize,
) -> Vec<LogicalRuntimeId> {
    let mut runtimes = Vec::with_capacity(QUIET_RUNTIMES);
    for index in 0..QUIET_RUNTIMES {
        let runtime = LogicalRuntimeId::new(format!("quiet-{index}"));
        store
            .commit_machine_lifecycle(&runtime, idle_lifecycle(&runtime), &[])
            .await
            .expect("the runtime is registered");
        write_settled_history(store, &runtime, &mut SettlingOps::new(), settled).await;
        runtimes.push(runtime);
    }
    runtimes
}

/// The start of the host name of `runtime`'s session scope.
fn runtime_scope_prefix(runtime: &LogicalRuntimeId) -> String {
    format!(
        "{}:{}~",
        super::RUNTIME_FAMILY.tag,
        codec::key_part(&runtime.0)
    )
}

/// A runtime scope's window as the host holds it.
struct HostWindow {
    name: String,
    start: u64,
    end: u64,
    /// Bytes of the visible rows.
    bytes: u64,
    /// The head's count of live row entry bytes.
    live: u64,
    row_lens: BTreeMap<u64, usize>,
}

/// `runtime`'s window on the host, checked against its head: the head names
/// exactly the visible rows and counts their bytes (the counter the re-base
/// rule reads).
fn host_window(host: &TestHost, runtime: &LogicalRuntimeId) -> HostWindow {
    let scope = host
        .window_of(ScopeKind::Session, &runtime_scope_prefix(runtime))
        .expect("the runtime's scope is on the host");
    let head = ScopeHeadV1::decode(&scope.head).expect("the head decodes");
    let bytes: u64 = scope.row_lens.values().map(|len| *len as u64).sum();
    assert_eq!(
        (head.w.s, head.w.e),
        (scope.start, scope.end),
        "the head names {}'s visible rows",
        scope.name
    );
    assert_eq!(
        head.sb, bytes,
        "the head counts {}'s visible row bytes",
        scope.name
    );
    HostWindow {
        name: scope.name,
        start: scope.start,
        end: scope.end,
        bytes,
        live: head.lb,
        row_lens: scope.row_lens,
    }
}

/// The retention rule after one commit: the visible rows stay within
/// `max(REBASE_FACTOR x the live bytes before the commit, REBASE_FLOOR)`
/// plus the rows the commit appended, and a commit that re-based the scope
/// did it in its one batch, a reset first, with every row before the
/// image's first row dropped. Answers whether it re-based.
fn assert_retention(
    host: &TestHost,
    before: &HostWindow,
    after: &HostWindow,
    commits_before: u64,
) -> bool {
    let appended: u64 = after
        .row_lens
        .range(before.end..)
        .map(|(_, len)| *len as u64)
        .sum();
    let bound = (REBASE_FACTOR * before.live).max(REBASE_FLOOR) + appended;
    assert!(
        after.bytes <= bound,
        "{} holds {} row bytes, over max({REBASE_FACTOR} x {} live bytes, \
         {REBASE_FLOOR}) + {appended} appended",
        after.name,
        after.bytes,
        before.live
    );
    if after.start == before.start {
        return false;
    }
    assert_eq!(
        after.start, before.end,
        "the re-base drops every row before the image's first row"
    );
    assert_eq!(
        host.commit_count(),
        commits_before + 1,
        "the re-base lands in the commit's one batch"
    );
    let first = host
        .row_at(ScopeKind::Session, &after.name, after.start)
        .expect("the image's first row");
    assert!(
        matches!(
            codec::decode_row(&first).expect("the row decodes").first(),
            Some(RowOp::Reset)
        ),
        "the image starts with a reset"
    );
    true
}

/// Persist `snapshot` to `runtime` and check the retention rule against the
/// window before. Answers the window after, and whether the commit re-based.
async fn persist_checked(
    host: &TestHost,
    store: &SpiRuntimeStore,
    runtime: &LogicalRuntimeId,
    snapshot: &PersistedOpsSnapshot,
    before: &HostWindow,
) -> (HostWindow, bool) {
    let commits = host.commit_count();
    store
        .persist_ops_lifecycle(runtime, snapshot)
        .await
        .expect("the snapshot persists");
    let after = host_window(host, runtime);
    let rebased = assert_retention(host, before, &after, commits);
    (after, rebased)
}

/// Drop a process and open another over the same host, with the host's call
/// record cleared in between: the calls recorded next are the new open's.
async fn reopen_recording(
    host: &Arc<TestHost>,
    old: (Arc<SpiRealm>, SpiRuntimeStore),
) -> (Arc<SpiRealm>, SpiRuntimeStore) {
    drop(old);
    host.expire_lease();
    host.take_calls();
    open(host).await
}

/// The calls a row counts: renews run on the realm's own timer, not on the
/// path a row measures.
fn counted_calls(host: &TestHost) -> Vec<HostCall> {
    host.take_calls()
        .into_iter()
        .filter(|call| *call != HostCall::RenewFence)
        .collect()
}

// Unrun until the candidate's validation.
//
// Runtimes with long settled histories and no work cost an open their heads
// only: one acquire, `list_heads` per kind at 2048 heads per call, and no
// `load_head` or `read_rows` at all, the obligation scan included. Every
// count is the same for 0, 1,000 and 10,000 settled operations per runtime.
#[tokio::test]
async fn an_open_reads_only_heads_for_quiet_runtimes_whatever_their_history() {
    let mut recorded = Vec::new();
    for settled in [0, 1_000, 10_000] {
        let host = Arc::new(TestHost::new());
        let opened = open(&host).await;
        let quiet = quiet_runtimes_with_history(&opened.1, settled).await;
        let reopened = reopen_recording(&host, opened).await;
        let listed = reopened
            .1
            .list_runtimes_with_recovery_work()
            .await
            .expect("the scan answers");
        let calls = counted_calls(&host);
        assert!(listed.is_empty(), "no quiet runtime holds work: {listed:?}");
        // A runtime with no rows has nothing to load and reads as loaded.
        if settled > 0 {
            for runtime in &quiet {
                assert!(
                    !reopened.1.runtime_loaded_for_test(&runtime.0).await,
                    "{runtime:?} stays on the host after {settled} settled operations"
                );
            }
        }
        assert_eq!(
            calls
                .iter()
                .filter(|call| **call == HostCall::AcquireFence)
                .count(),
            1,
            "one acquire: {calls:?}"
        );
        for kind in [
            ScopeKind::Session,
            ScopeKind::Identity,
            ScopeKind::Mob,
            ScopeKind::Record,
        ] {
            assert_eq!(
                calls
                    .iter()
                    .filter(|call| **call == HostCall::ListHeads { kind })
                    .count(),
                host.scope_count(kind).div_ceil(LIST_PAGE_HEADS).max(1),
                "the {kind} heads, {LIST_PAGE_HEADS} per call"
            );
        }
        assert!(
            calls
                .iter()
                .all(|call| matches!(call, HostCall::AcquireFence | HostCall::ListHeads { .. })),
            "the open reads heads only after {settled} settled operations: {calls:?}"
        );
        recorded.push(calls);
    }
    assert!(
        recorded.windows(2).all(|pair| pair[0] == pair[1]),
        "every open makes the same calls whatever the history: {recorded:?}"
    );
}

/// The heads a listing asks one `list_heads` call for, as the host store
/// reference names it (256 before 0.9.7). The rows below count calls
/// against this number, not against the realm's constant.
const DOCUMENTED_LIST_PAGE_HEADS: usize = 2048;

/// Create `count` test scopes, `listed-00000` onwards, each holding one
/// small pinned entry, committed a few hundred scopes per batch. Answers
/// their keys.
async fn seed_listed_scopes(realm: &SpiRealm, count: usize) -> BTreeSet<String> {
    let ids: Vec<ScopeId> = (0..count)
        .map(|n| ScopeId::new(TEST_FAMILY, format!("listed-{n:05}")))
        .collect();
    for batch in ids.chunks(256) {
        let mut guards = realm.lock_scopes(batch).await;
        let mut states: Vec<_> = guards.iter_mut().map(|guard| &mut **guard).collect();
        let deltas = batch
            .iter()
            .map(|_| ScopeDelta::Changes(vec![("pin".into(), Some(Arc::new(b"listed".to_vec())))]))
            .collect();
        realm
            .commit(&mut states, deltas)
            .await
            .expect("the test scopes land");
    }
    ids.into_iter().map(|id| id.key).collect()
}

/// The keys of every test scope `realm` claims from its listing.
async fn claimed_test_keys(realm: &SpiRealm) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    for cell in realm.claim(TEST_FAMILY).expect("the test scopes claim") {
        keys.insert(cell.lock().await.id().key.clone());
    }
    keys
}

/// How many `list_heads` calls of `kind` `calls` holds.
fn listings_of(calls: &[HostCall], kind: ScopeKind) -> usize {
    calls
        .iter()
        .filter(|call| **call == HostCall::ListHeads { kind })
        .count()
}

/// A 1 s renew interval with a 30 s lease: the self-fence window (29 s, on
/// the wall clock) stays open however long a row's open takes, so only the
/// injected renew failure stops the writer.
fn listing_row_timing() -> FenceTiming {
    FenceTiming::try_new(Duration::from_secs(1), Duration::from_secs(30)).unwrap()
}

/// Wait until `realm`'s writer stops on a failed renew and acquires the
/// fence again ([`listing_row_timing`]: the renew at 1 s fails, the tick at
/// 2 s re-acquires over its own recorded lease), and answer the calls of
/// that re-acquire.
async fn reacquire_recording(host: &TestHost, realm: &SpiRealm) -> Vec<HostCall> {
    let first = realm.fence();
    host.faults.lock().unwrap().renew_unavailable = 1;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(
        !realm.lease_is_live(),
        "the failed renew stopped the writer"
    );
    host.take_calls();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(
        realm.lease_is_live() && realm.fence() > first,
        "the writer acquired again and resumed: {:?}",
        realm.realm_poisoned()
    );
    counted_calls(host)
}

// Unrun until the candidate's validation.
//
// An open and a stopped writer's re-acquire list a scope kind in pages of
// 2048 heads: `ceil(n / 2048)` `list_heads` calls for n heads, one for a
// kind with none. 2048 heads take one call, 2049 two and 4097 three (at 256
// heads per call they took 8, 9 and 17), and the open claims every head.
#[tokio::test(start_paused = true)]
async fn an_open_and_a_reacquire_list_a_kind_in_pages_of_2048_heads() {
    let timing = listing_row_timing();
    for scopes in [
        DOCUMENTED_LIST_PAGE_HEADS,
        DOCUMENTED_LIST_PAGE_HEADS + 1,
        2 * DOCUMENTED_LIST_PAGE_HEADS + 1,
    ] {
        let host = Arc::new(TestHost::with_timing(timing));
        let realm = SpiRealm::open(Arc::clone(&host) as _, SpiRealmConfig::default())
            .await
            .unwrap();
        let keys = seed_listed_scopes(&realm, scopes).await;
        assert_eq!(
            host.scope_count(ScopeKind::Record),
            scopes,
            "the test scopes are the only record scopes"
        );
        drop(realm);
        host.expire_lease();
        host.take_calls();
        let realm = SpiRealm::open(Arc::clone(&host) as _, SpiRealmConfig::default())
            .await
            .unwrap();
        let opened = counted_calls(&host);
        let reacquired = reacquire_recording(&host, &realm).await;
        for (step, calls) in [("open", &opened), ("re-acquire", &reacquired)] {
            assert_eq!(
                listings_of(calls, ScopeKind::Record),
                scopes.div_ceil(DOCUMENTED_LIST_PAGE_HEADS),
                "the {step} lists {scopes} record heads {DOCUMENTED_LIST_PAGE_HEADS} per call"
            );
            for kind in [ScopeKind::Session, ScopeKind::Identity, ScopeKind::Mob] {
                assert_eq!(
                    listings_of(calls, kind),
                    1,
                    "the {step} lists the empty {kind} kind in one call"
                );
            }
        }
        assert_eq!(
            claimed_test_keys(&realm).await,
            keys,
            "the open claims every head it listed"
        );
    }
}

// Unrun until the candidate's validation.
//
// A store may answer fewer heads than a listing asks for, with `next` set.
// Over a store whose pages hold 100 heads, an open lists 250 record heads in
// three calls and claims every one, and a stopped writer's re-acquire lists
// them the same way and finds every version it knows, so it resumes.
#[tokio::test(start_paused = true)]
async fn a_store_with_shorter_pages_still_lists_every_head() {
    let host = Arc::new(TestHost::with_timing(listing_row_timing()));
    let realm = SpiRealm::open(Arc::clone(&host) as _, SpiRealmConfig::default())
        .await
        .unwrap();
    let keys = seed_listed_scopes(&realm, 250).await;
    drop(realm);
    host.expire_lease();
    host.faults.lock().unwrap().list_page_cap = Some(100);
    host.take_calls();
    let realm = SpiRealm::open(Arc::clone(&host) as _, SpiRealmConfig::default())
        .await
        .unwrap();
    assert_eq!(
        listings_of(&counted_calls(&host), ScopeKind::Record),
        3,
        "the open reads on from each short page"
    );
    assert_eq!(
        claimed_test_keys(&realm).await,
        keys,
        "the open claims every head"
    );
    let reacquired = reacquire_recording(&host, &realm).await;
    assert_eq!(
        listings_of(&reacquired, ScopeKind::Record),
        3,
        "the re-acquire reads on from each short page"
    );
    assert!(realm.realm_poisoned().is_none());
}

// Unrun until the candidate's validation.
//
// One runtime holds work (an unsettled input) beside quiet runtimes, and all
// of them settled the same number of operations. The open reads rows of that
// runtime only: `ceil(window rows / 16)` `read_rows` calls, and no
// `load_head` since no live entry is chunked. Its window stays under
// `max(4 x live bytes, 256 KiB)` plus one commit after every commit, so the
// reads are the same for 1,000 and 10,000 settled operations.
#[tokio::test]
async fn an_open_reads_only_the_obligated_runtimes_bounded_window() {
    let mut row_reads = Vec::new();
    for settled in [1_000, 10_000] {
        let host = Arc::new(TestHost::new());
        let opened = open(&host).await;
        let quiet = quiet_runtimes_with_history(&opened.1, settled).await;
        let obligated = LogicalRuntimeId::new("obligated");
        opened
            .1
            .commit_machine_lifecycle(&obligated, idle_lifecycle(&obligated), &[])
            .await
            .expect("the runtime is registered");
        opened
            .1
            .persist_input_state(
                &obligated,
                &persistable(StoredInputState::new_accepted(InputId::new())),
            )
            .await
            .expect("an unsettled input");
        let mut ops = SettlingOps::new();
        let mut window = host_window(&host, &obligated);
        let mut done = 0;
        while done < settled {
            let batch = SETTLED_PER_SNAPSHOT.min(settled - done);
            ops.settle(batch);
            done += batch;
            window = persist_checked(&host, &opened.1, &obligated, &ops.snapshot(), &window)
                .await
                .0;
        }
        assert!(
            host.window_of(ScopeKind::Record, "ch:").is_none(),
            "every snapshot is inline: no live entry is chunked"
        );

        let reopened = reopen_recording(&host, opened).await;
        let reads: Vec<HostCall> = counted_calls(&host)
            .into_iter()
            .filter(|call| matches!(call, HostCall::LoadHead { .. } | HostCall::ReadRows { .. }))
            .collect();
        for call in &reads {
            assert!(
                matches!(
                    call,
                    HostCall::ReadRows { kind: ScopeKind::Session, name } if *name == window.name
                ),
                "the open reads the obligated runtime's rows only: {call:?}"
            );
        }
        assert_eq!(
            reads.len() as u64,
            (window.end - window.start).div_ceil(READ_PAGE_ROWS as u64),
            "one read_rows per {READ_PAGE_ROWS} rows of the window {}..{}",
            window.start,
            window.end
        );
        assert!(
            reopened.1.runtime_loaded_for_test(&obligated.0).await,
            "the runtime with work loads at open"
        );
        for runtime in &quiet {
            assert!(
                !reopened.1.runtime_loaded_for_test(&runtime.0).await,
                "{runtime:?} stays on the host"
            );
        }
        row_reads.push(reads.len());
    }
    assert_eq!(
        row_reads[0], row_reads[1],
        "the open reads as many rows after 1,000 settled operations as after 10,000"
    );
}

// Unrun until the candidate's validation.
//
// A runtime settles operations one after another and persists its ops
// lifecycle snapshot after each, as the runtime does at every terminal
// transition. After every commit its visible rows stay within
// `max(4 x live bytes, 256 KiB)` plus that commit's rows; each re-base is the
// commit's one batch, a reset and the live image, with every earlier row
// dropped at the image's first row; and a reopen folds the same entries: the
// newest snapshot with the work it owes, the unsettled input, the committed
// document and the run's provisional tail. Every entry folds byte for byte
// but the ops snapshot, which memory encodes again from its hash maps: it
// must fold to the same snapshot, compared decoded.
#[tokio::test]
async fn a_settling_runtime_keeps_its_window_under_the_rebase_bound() {
    use meerkat_core::lifecycle::RunId;
    use meerkat_core::types::{Message, UserMessage};

    use crate::store::{PreparedWholeBlobProvisionalTail, SerializedSessionSnapshot};

    const SETTLED: usize = 100;

    assert_eq!(
        (CHECKPOINT_FACTOR, CHECKPOINT_MIN_BYTES),
        (REBASE_FACTOR, REBASE_FLOOR),
        "the realm re-bases by the rule these rows hold it to"
    );
    let host = Arc::new(TestHost::new());
    let opened = open(&host).await;
    let mut session = meerkat_core::Session::new();
    session.push(Message::User(UserMessage::text("the committed base")));
    let runtime = LogicalRuntimeId::for_session(session.id());
    opened
        .1
        .commit_session_snapshot(
            &runtime,
            SerializedSessionSnapshot {
                session_snapshot: Arc::new(serde_json::to_vec(&session).unwrap()),
            },
        )
        .await
        .expect("the committed document");
    let base = opened
        .1
        .load_whole_blob_store_authority(&runtime)
        .await
        .unwrap()
        .expect("a committed base");
    session.push(Message::User(UserMessage::text("the run's first round")));
    opened
        .1
        .write_prepared_whole_blob_provisional_tail(
            &runtime,
            PreparedWholeBlobProvisionalTail::prepare_from_session(base, RunId::new(), 1, &session)
                .unwrap(),
        )
        .await
        .expect("the run's provisional tail");
    opened
        .1
        .persist_input_state(
            &runtime,
            &persistable(StoredInputState::new_accepted(InputId::new())),
        )
        .await
        .expect("an unsettled input");

    let mut ops = SettlingOps::new();
    let mut window = host_window(&host, &runtime);
    let mut rebases = 0;
    for _ in 0..SETTLED {
        ops.settle_one();
        let (after, rebased) =
            persist_checked(&host, &opened.1, &runtime, &ops.snapshot(), &window).await;
        rebases += usize::from(rebased);
        window = after;
    }
    ops.owe_work();
    let (_, rebased) = persist_checked(&host, &opened.1, &runtime, &ops.snapshot(), &window).await;
    rebases += usize::from(rebased);
    assert!(
        rebases >= 2,
        "{rebases} re-bases over {SETTLED} settled operations"
    );
    assert!(
        host.window_of(ScopeKind::Record, "ch:").is_none(),
        "every snapshot is inline"
    );

    let image = opened.1.export_runtime_for_test(&runtime.0).await;
    let reopened = reopen(&host, opened).await;
    assert!(
        reopened.1.runtime_loaded_for_test(&runtime.0).await,
        "its work loads it at open"
    );
    let folded = reopened.1.export_runtime_for_test(&runtime.0).await;
    assert_eq!(
        folded.keys().collect::<Vec<_>>(),
        image.keys().collect::<Vec<_>>(),
        "the reopen folds the same entries"
    );
    for (key, value) in &image {
        if key == "ops" {
            // Memory keeps the ops snapshot decoded and encodes it again for
            // each export. Its operations, specs and feed entries are hash
            // maps, so two encodings of the same snapshot may order their
            // keys differently: the reopen must fold the same snapshot, not
            // the same bytes.
            let decode = |bytes: &[u8]| {
                serde_json::from_slice::<serde_json::Value>(bytes)
                    .expect("the ops snapshot is JSON")
            };
            assert_eq!(
                decode(folded[key].as_slice()),
                decode(value.as_slice()),
                "the reopen folds the same ops snapshot"
            );
        } else {
            assert_eq!(&folded[key], value, "the reopen folds {key} byte for byte");
        }
    }
    assert!(
        reopened
            .1
            .list_runtimes_with_recovery_work()
            .await
            .expect("the scan answers")
            .contains(&runtime),
        "the owed work is still owed"
    );
}

// ---------------------------------------------------------------------------
// The open's loads of the runtimes recovery must act on: up to
// BOOT_LOAD_CONCURRENCY runtimes at once, and nothing installed when one
// fails.
// ---------------------------------------------------------------------------

/// How long a session row read takes in these rows, on the paused clock.
const BOOT_READ_DELAY: Duration = Duration::from_millis(50);

/// `count` runtimes, each holding an unsettled input, so the open loads every
/// one of their bodies. Their keys sort in creation order.
async fn obligated_runtimes(store: &SpiRuntimeStore, count: usize) -> Vec<LogicalRuntimeId> {
    let mut runtimes = Vec::with_capacity(count);
    for index in 0..count {
        let runtime = LogicalRuntimeId::new(format!("obligated-{index:03}"));
        store
            .persist_input_state(
                &runtime,
                &persistable(StoredInputState::new_accepted(InputId::new())),
            )
            .await
            .expect("an unsettled input");
        runtimes.push(runtime);
    }
    runtimes
}

// Unrun until the candidate's validation.
//
// The open loads the runtimes recovery must act on concurrently, up to
// `BOOT_LOAD_CONCURRENCY` at once. Every session row read waits the same
// delay on the paused clock, and the host counts the reads waiting at once:
// an open of K such runtimes peaks at min(K, bound) reads in flight, never
// more, and loads every one of them. The row counts the peak; it does not
// time the open (ceil(K / bound) delays, where a serial open took K).
//
// RED on the base (e294b75f29, with the row and its test host hooks applied):
// the open loaded the runtimes one at a time, so the peak is 1 for every K.
#[tokio::test(start_paused = true)]
async fn an_open_loads_the_runtimes_with_recovery_work_up_to_the_bound_at_once() {
    let bound = super::runtime::BOOT_LOAD_CONCURRENCY;
    assert_eq!(bound, 16, "the bound the docs and the CHANGELOG state");
    for count in [5, 2 * bound + 3] {
        let host = Arc::new(TestHost::new());
        let opened = open(&host).await;
        let runtimes = obligated_runtimes(&opened.1, count).await;
        host.faults.lock().unwrap().session_read_delay = Some(BOOT_READ_DELAY);
        let (_realm, store) = reopen(&host, opened).await;
        assert_eq!(
            host.session_read_peak(),
            count.min(bound),
            "an open of {count} runtimes with work reads min({count}, {bound}) of them at once"
        );
        for runtime in &runtimes {
            assert!(
                store.runtime_loaded_for_test(&runtime.0).await,
                "{runtime:?} holds work and loads at open"
            );
        }
    }
}

// Unrun until the candidate's validation.
//
// A failed body load fails the open with the host's error, and the open
// installs nothing: no runtime scope of the realm it leaves counts as
// loaded, and it commits nothing. Once the host reads again, the next open
// loads and serves every runtime as it was. The failing runtime is the last
// in key order and its read answers last, after the other loads finished, so
// an open that installed each runtime as its read completed, in key order or
// not, would have installed the other two.
//
// RED on the base (e294b75f29, with the row and its test host hooks applied):
// the serial open imported the first two runtimes and marked their scopes
// loaded before it failed at the third.
#[tokio::test(start_paused = true)]
async fn a_failed_recovery_load_fails_the_open_and_installs_nothing() {
    let host = Arc::new(TestHost::new());
    let opened = open(&host).await;
    let runtimes = obligated_runtimes(&opened.1, 3).await;
    let mut images = Vec::with_capacity(runtimes.len());
    for runtime in &runtimes {
        images.push(opened.1.export_runtime_for_test(&runtime.0).await);
    }
    drop(opened);
    host.expire_lease();
    host.faults.lock().unwrap().failing_session_reads =
        Some((runtime_scope_prefix(&runtimes[2]), BOOT_READ_DELAY));
    let commits = host.commit_count();
    let realm = SpiRealm::open(Arc::clone(&host) as _, SpiRealmConfig::default())
        .await
        .expect("the realm opens");
    let failed = SpiRuntimeStore::open(Arc::clone(&realm)).await;
    assert!(
        matches!(
            failed,
            Err(SpiError::Host(
                meerkat_core::host_store::HostStoreError::Unavailable(_)
            ))
        ),
        "the failed load fails the open with the host's error: {failed:?}"
    );
    for runtime in &runtimes {
        let cell = realm.cell(&ScopeId::new(super::RUNTIME_FAMILY, runtime.0.clone()));
        assert!(
            !cell.lock().await.is_loaded(),
            "the failed open installed nothing, {runtime:?} included"
        );
    }
    assert_eq!(
        host.commit_count(),
        commits,
        "the failed open committed nothing"
    );

    host.faults.lock().unwrap().failing_session_reads = None;
    drop((failed, realm));
    host.expire_lease();
    let (_realm, store) = open(&host).await;
    for (runtime, image) in runtimes.iter().zip(&images) {
        assert!(
            store.runtime_loaded_for_test(&runtime.0).await,
            "{runtime:?} loads at the next open"
        );
        assert_eq!(
            &store.export_runtime_for_test(&runtime.0).await,
            image,
            "the next open serves {runtime:?} as it was"
        );
    }
}

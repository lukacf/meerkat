//! Rows for the host store adapters, unrun until the candidate's
//! validation.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use meerkat_core::host_store::{AcquireMode, FenceTiming, ScopeKind};
use meerkat_core::lifecycle::InputId;

use super::realm::{CHECKPOINT_MIN_BYTES, INLINE_VALUE_LIMIT};
use super::test_host::TestHost;
use super::{
    ArmedScopes, ScopeDelta, ScopeFamily, ScopeId, SpiError, SpiRealm, SpiRealmConfig,
    SpiRuntimeStore,
};
use crate::identifiers::LogicalRuntimeId;
use crate::input_state::{InputStatePersistenceRecord, StoredInputState};
use crate::store::{
    RuntimeDeliveryAuthorityCasOutcome, RuntimeDeliveryAuthorityRecord, RuntimeDeliveryStoreRecord,
    RuntimeStore, RuntimeStoreError,
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
    tokio::time::sleep(Duration::from_millis(1000)).await;
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
    tokio::time::sleep(Duration::from_millis(1000)).await;
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
    tokio::time::sleep(Duration::from_millis(1000)).await;
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

//! The store-wide delivery generation (#1813) is a wake hint, never part of
//! any runtime's delivery-authority comparand: a delivery committed for one
//! runtime must never fail another runtime's compare-and-swap.

#![allow(clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use meerkat_runtime::LogicalRuntimeId;
use meerkat_runtime::store::{
    RuntimeDeliveryAuthorityCasOutcome, RuntimeDeliveryAuthorityRecord, RuntimeStore,
    SqliteRuntimeStore,
};

fn record(revision: u64) -> RuntimeDeliveryAuthorityRecord {
    RuntimeDeliveryAuthorityRecord::from_parts(revision, br#"{"opaque":true}"#.to_vec())
}

fn applied(outcome: RuntimeDeliveryAuthorityCasOutcome) {
    assert!(
        matches!(outcome, RuntimeDeliveryAuthorityCasOutcome::Applied(_)),
        "the CAS applied on its first attempt: {outcome:?}"
    );
}

/// One runtime's chain of delivery commits, each a compare-and-swap of that
/// runtime's own revision only.
async fn commit_chain(store: Arc<dyn RuntimeStore>, runtime: &'static str, commits: u64) {
    let runtime_id = LogicalRuntimeId::new(runtime);
    let mut observed = None;
    for revision in 1..=commits {
        applied(
            store
                .compare_and_swap_runtime_delivery_authority(
                    &runtime_id,
                    observed,
                    record(revision),
                    None,
                )
                .await
                .expect("delivery commit"),
        );
        observed = Some(revision);
    }
}

/// tlc-gate's constraint on the generation: two runtimes commit deliveries
/// concurrently, through two store handles over one database (two
/// processes' connections) on separate worker threads. Every
/// compare-and-swap applies on its first attempt (the store-wide bump, made
/// in each delivery's own transaction, never fails another runtime's
/// comparand), and the generation counts every commit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_delivery_commits_never_fail_another_runtimes_cas() {
    const COMMITS: u64 = 25;
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("runtime.sqlite3");
    let first: Arc<dyn RuntimeStore> =
        Arc::new(SqliteRuntimeStore::new(path.clone()).expect("open first handle"));
    let second: Arc<dyn RuntimeStore> =
        Arc::new(SqliteRuntimeStore::new(path).expect("open second handle"));
    assert_eq!(first.load_delivery_generation().await.expect("read"), 0);

    let (a, b) = tokio::join!(
        tokio::spawn(commit_chain(
            Arc::clone(&first),
            "generation-runtime-a",
            COMMITS
        )),
        tokio::spawn(commit_chain(
            Arc::clone(&second),
            "generation-runtime-b",
            COMMITS
        )),
    );
    a.expect("runtime a's chain");
    b.expect("runtime b's chain");
    for store in [&first, &second] {
        assert_eq!(
            store.load_delivery_generation().await.expect("read"),
            2 * COMMITS,
            "every delivery commit advanced the generation, in its own transaction"
        );
    }
}

/// The interleaving spelled out: another runtime's commit between this
/// runtime's read and its CAS moves the generation, and this runtime's CAS
/// still applies, because it compares only its own revision.
#[tokio::test]
async fn another_runtimes_commit_never_fails_this_runtimes_cas() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store =
        Arc::new(SqliteRuntimeStore::new(dir.path().join("runtime.sqlite3")).expect("open store"));
    let a = LogicalRuntimeId::new("generation-runtime-a");
    let b = LogicalRuntimeId::new("generation-runtime-b");

    applied(
        store
            .compare_and_swap_runtime_delivery_authority(&a, None, record(1), None)
            .await
            .expect("first commit for a"),
    );
    let observed_a = store
        .load_runtime_delivery_authority(&a)
        .await
        .expect("read a")
        .expect("a has authority")
        .revision();
    applied(
        store
            .compare_and_swap_runtime_delivery_authority(&b, None, record(1), None)
            .await
            .expect("commit for b"),
    );
    applied(
        store
            .compare_and_swap_runtime_delivery_authority(&a, Some(observed_a), record(2), None)
            .await
            .expect("second commit for a"),
    );
    assert_eq!(store.load_delivery_generation().await.expect("read"), 3);
}

#[tokio::test]
async fn a_store_that_never_committed_a_delivery_reads_generation_zero() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteRuntimeStore::new(dir.path().join("runtime.sqlite3")).expect("open store");
    assert_eq!(store.load_delivery_generation().await.expect("read"), 0);
}

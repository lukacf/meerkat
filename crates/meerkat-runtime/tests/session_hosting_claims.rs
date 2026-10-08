//! Session hosting claims (#1813): owner-scoped, read from the local claim
//! registry without touching a session's lock, and held inside a store-only
//! write's blocking operations.
//!
//! A peer process is simulated by a separate open file on the session's lock
//! file: flock conflicts per open file, exactly as across processes.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use meerkat_core::SessionId;
use meerkat_core::session_hosting::session_hosting_lock_path;
use meerkat_runtime::{
    ColdDeliveryOwnership, HostingCapability, HostingOwner, HostingPaths, HostingRefused,
    ServedElsewhere, SessionServing, grant_session_hosting, local_session_serving,
    spawn_blocking_holding_claim, try_cold_delivery_ownership, with_write_hosting,
};

fn capability(root: &std::path::Path) -> HostingCapability {
    HostingCapability::OsLock(Arc::new(HostingPaths {
        hosting_lock_dir: root.join("hosting"),
        cold_delivery_lock: root.join("delivery").join("cold-delivery.lock"),
        database: None,
    }))
}

fn lock_dir(capability: &HostingCapability) -> std::path::PathBuf {
    capability
        .paths()
        .expect("an OsLock capability names its paths")
        .hosting_lock_dir
        .clone()
}

fn open_lock_file(capability: &HostingCapability, session: &SessionId) -> std::fs::File {
    let lock_dir = lock_dir(capability);
    std::fs::create_dir_all(&lock_dir).expect("lock dir");
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(session_hosting_lock_path(&lock_dir, session))
        .expect("open lock file")
}

/// A peer process's attempt at the session's lock (released at once).
fn peer_can_lock(capability: &HostingCapability, session: &SessionId) -> bool {
    open_lock_file(capability, session).try_lock().is_ok()
}

#[test]
fn a_claim_is_shared_within_its_owner_and_released_with_its_last_holder() {
    let dir = tempfile::tempdir().expect("tempdir");
    let capability = capability(dir.path());
    let owner = HostingOwner::mint();
    let session = SessionId::new();
    let first = grant_session_hosting(&capability, &owner, &session).expect("first claim");
    assert!(first.is_cross_process());
    let second = grant_session_hosting(&capability, &owner, &session).expect("lineage shares it");
    drop(first);
    assert_eq!(
        local_session_serving(&capability, &owner, &session),
        SessionServing::HeldHere,
        "a remaining holder keeps the claim"
    );
    assert!(!peer_can_lock(&capability, &session));
    drop(second);
    assert_eq!(
        local_session_serving(&capability, &owner, &session),
        SessionServing::NotHeldInThisProcess
    );
    assert!(
        peer_can_lock(&capability, &session),
        "the last holder released it"
    );
    assert!(
        session_hosting_lock_path(&lock_dir(&capability), &session).exists(),
        "the lock file is never unlinked"
    );
}

/// ADR's owner scope: another runtime owner of the same process is refused
/// exactly like another process, and sees the session served by another
/// local owner; it gets the claim once the holder's lineage is gone.
#[test]
fn another_local_owner_is_refused_like_another_process() {
    let dir = tempfile::tempdir().expect("tempdir");
    let capability = capability(dir.path());
    let (holder, other) = (HostingOwner::mint(), HostingOwner::mint());
    assert_ne!(holder, other);
    let session = SessionId::new();
    let claim = grant_session_hosting(&capability, &holder, &session).expect("claim");
    assert_eq!(
        grant_session_hosting(&capability, &other, &session).expect_err("another owner"),
        HostingRefused::ServedElsewhere(ServedElsewhere {
            session_id: session.clone()
        })
    );
    assert_eq!(
        local_session_serving(&capability, &other, &session),
        SessionServing::HeldByAnotherLocalOwner
    );
    assert_eq!(
        local_session_serving(&capability, &holder, &session),
        SessionServing::HeldHere
    );
    drop(claim);
    let taken = grant_session_hosting(&capability, &other, &session)
        .expect("free once the holder's lineage is gone");
    assert_eq!(taken.owner(), &other);
}

/// Serving is read from the registry alone: asking never takes the lock (a
/// peer process holding it is undisturbed and invisible) and never creates
/// the lock file. Only a grant consults the OS lock.
#[test]
fn local_serving_reads_the_registry_and_never_takes_the_lock() {
    let dir = tempfile::tempdir().expect("tempdir");
    let capability = capability(dir.path());
    let owner = HostingOwner::mint();
    let session = SessionId::new();
    assert_eq!(
        local_session_serving(&capability, &owner, &session),
        SessionServing::NotHeldInThisProcess
    );
    assert!(
        !session_hosting_lock_path(&lock_dir(&capability), &session).exists(),
        "asking never creates the lock file"
    );

    let peer = open_lock_file(&capability, &session);
    peer.try_lock().expect("the peer takes the lock");
    assert_eq!(
        local_session_serving(&capability, &owner, &session),
        SessionServing::NotHeldInThisProcess,
        "another process's claim is invisible without a grant"
    );
    assert_eq!(
        grant_session_hosting(&capability, &owner, &session).expect_err("the peer holds it"),
        HostingRefused::ServedElsewhere(ServedElsewhere {
            session_id: session.clone()
        })
    );
    drop(peer);
    grant_session_hosting(&capability, &owner, &session).expect("free once the peer released it");
}

#[test]
fn process_local_and_claimless_stores_grant_unclaimed_and_own_cold_delivery() {
    let session = SessionId::new();
    let owner = HostingOwner::mint();
    for capability in [HostingCapability::ProcessLocal, HostingCapability::None] {
        let claim = grant_session_hosting(&capability, &owner, &session).expect("granted");
        assert!(!claim.is_cross_process());
        assert_eq!(
            local_session_serving(&capability, &owner, &session),
            SessionServing::HeldHere,
            "a store without cross-process claims is served here"
        );
        assert!(try_cold_delivery_ownership(&capability).applies_cold_deliveries());
    }
}

/// ADR's #1813 B correction: a store that selected cross-process claims never
/// weakens a claim it cannot take to an unclaimed grant, nor a cold-delivery
/// lock it cannot take to sole ownership. Here both lock locations become
/// unusable after the capability was selected (a regular file where each
/// directory belongs). The grant is refused typed, nothing is served here,
/// and cold delivery is ineligible. Paired: once the locations are usable
/// again the same owner's claim and cold ownership are granted, and ordinary
/// contention is still `ServedElsewhere`.
#[test]
fn an_unavailable_cross_process_lock_is_refused_never_weakened() {
    let dir = tempfile::tempdir().expect("tempdir");
    let capability = capability(dir.path());
    let hosting_dir = lock_dir(&capability);
    let cold_dir = capability
        .paths()
        .expect("an OsLock capability names its paths")
        .cold_delivery_lock
        .parent()
        .expect("cold lock parent")
        .to_path_buf();
    std::fs::write(&hosting_dir, b"not a directory").expect("block the lock dir");
    std::fs::write(&cold_dir, b"not a directory").expect("block the cold lock dir");
    let owner = HostingOwner::mint();
    let session = SessionId::new();

    match grant_session_hosting(&capability, &owner, &session) {
        Err(HostingRefused::Unavailable(unavailable)) => {
            assert_eq!(unavailable.session_id, session);
        }
        other => panic!("an unavailable claim must be refused typed, got {other:?}"),
    }
    assert_eq!(
        local_session_serving(&capability, &owner, &session),
        SessionServing::NotHeldInThisProcess,
        "nothing is served here without the claim"
    );
    let cold = try_cold_delivery_ownership(&capability);
    assert!(
        matches!(cold, ColdDeliveryOwnership::Unavailable),
        "{cold:?}"
    );
    assert!(!cold.applies_cold_deliveries());

    std::fs::remove_file(&hosting_dir).expect("unblock the lock dir");
    std::fs::remove_file(&cold_dir).expect("unblock the cold lock dir");
    let claim = grant_session_hosting(&capability, &owner, &session).expect("usable again");
    assert!(claim.is_cross_process());
    assert_eq!(
        grant_session_hosting(&capability, &HostingOwner::mint(), &session)
            .expect_err("contention is unchanged"),
        HostingRefused::ServedElsewhere(ServedElsewhere {
            session_id: session.clone()
        })
    );
    let cold = try_cold_delivery_ownership(&capability);
    assert!(matches!(cold, ColdDeliveryOwnership::Owner(_)), "{cold:?}");
}

#[test]
fn one_cold_delivery_owner_per_store() {
    let dir = tempfile::tempdir().expect("tempdir");
    let capability = capability(dir.path());
    let owner = try_cold_delivery_ownership(&capability);
    assert!(matches!(owner, ColdDeliveryOwnership::Owner(_)));
    assert!(!try_cold_delivery_ownership(&capability).applies_cold_deliveries());
    drop(owner);
    assert!(try_cold_delivery_ownership(&capability).applies_cold_deliveries());
}

/// ADR's blocker 3: the transient claim of a store-only write lives in the
/// blocking closure. The awaiting future is cancelled while the blocking
/// write still runs; the lock stays held until the write returns.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_blocking_write_keeps_the_scoped_claim_until_it_returns() {
    let dir = tempfile::tempdir().expect("tempdir");
    let capability = capability(dir.path());
    let owner = HostingOwner::mint();
    let session = SessionId::new();
    let claim = grant_session_hosting(&capability, &owner, &session).expect("claim");
    let (started_tx, started_rx) = std::sync::mpsc::channel::<()>();
    let (finish_tx, finish_rx) = std::sync::mpsc::channel::<()>();
    let write = tokio::spawn(with_write_hosting(claim, async move {
        let blocking = spawn_blocking_holding_claim(move || {
            started_tx.send(()).expect("signal start");
            finish_rx.recv().expect("released by the test");
        });
        let _ = blocking.await;
    }));
    tokio::task::spawn_blocking(move || started_rx.recv())
        .await
        .expect("join")
        .expect("the blocking write started");
    write.abort();
    assert!(write.await.expect_err("aborted").is_cancelled());
    assert!(
        !peer_can_lock(&capability, &session),
        "the cancelled write's blocking closure still holds the claim"
    );

    // A peer blocks on the lock; the kernel wakes it when the closure's
    // clone, the lineage's last, is dropped as the write returns.
    let peer = open_lock_file(&capability, &session);
    finish_tx.send(()).expect("finish the write");
    tokio::time::timeout(
        std::time::Duration::from_secs(20),
        tokio::task::spawn_blocking(move || peer.lock()),
    )
    .await
    .expect("the claim is released once the blocking write returns (hang guard)")
    .expect("join")
    .expect("the peer takes the released lock");
}

/// Outside a write scope a blocking store operation captures no claim: the
/// task's own claim, dropped while the operation runs, is released at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_blocking_operation_outside_a_scope_holds_no_claim() {
    let dir = tempfile::tempdir().expect("tempdir");
    let capability = capability(dir.path());
    let owner = HostingOwner::mint();
    let session = SessionId::new();
    let claim = grant_session_hosting(&capability, &owner, &session).expect("claim");
    let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
    let operation = spawn_blocking_holding_claim({
        let capability = capability.clone();
        let session = session.clone();
        move || {
            go_rx.recv().expect("go");
            peer_can_lock(&capability, &session)
        }
    });
    drop(claim);
    go_tx.send(()).expect("go");
    assert!(
        operation.await.expect("join"),
        "an operation started outside a write scope captured no claim"
    );
}

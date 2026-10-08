//! Delivery owners on a store several processes share (#1813).
//!
//! A peer process is simulated by holding its OS locks through separate file
//! handles in this process (flock conflicts per open file, exactly as across
//! processes): the hosting claim of a session it serves, or the store's
//! cold-delivery lock. A peer's "death" is closing those handles, which is
//! what the kernel does when a process exits. A peer's death raises no file
//! notification, so the owner notices it at its store watch's sweep (here a
//! short one; 5 s by default).

// `session-store` enables the runtime's SQLite store, and the crate's own
// test dev-dependency enables `session-store`, so this runs in every lane.
#![cfg(feature = "session-store")]
#![allow(clippy::expect_used, clippy::panic)]

use std::fs::File;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use meerkat::{
    AttemptClaim, CanonicalArgumentsHash, DeliveryRoute, DetachedJobService, InteractionLineageId,
    JobDeliveryApplication, JobDeliveryApplyError, JobDeliveryKind, JobDeliverySink, JobResultRef,
    JobSpec, JobSubmissionKey, JobSubscription, JobSubscriptionId, MemoryDetachedJobStore,
    RestartClass, RunnerHandleRef, RunnerIdentity, RuntimeDeliveryHost, RuntimeDeliveryOwner,
    RuntimeDeliveryPass, SessionId, ToolIdentity, WorkerId,
};
use meerkat_runtime::{
    HostingCapability, HostingClaim, HostingOwner, HostingPaths, HostingRefused,
    RuntimeDeliveryInbox, SessionHostingAuthority, SessionServing, SqliteRuntimeStore,
};
use tokio::sync::{Mutex, watch};

/// Hang guard for awaited typed events; never a pacing mechanism.
const EVENT_GUARD: Duration = Duration::from_secs(20);
/// The owner's store-watch sweep in these tests.
const TEST_SWEEP: Duration = Duration::from_millis(50);

/// A peer process's hold on one lock file: an exclusive OS lock through its
/// own open file. Dropping it is the peer's death.
struct PeerLock {
    _file: File,
}

fn peer_lock(path: &Path) -> PeerLock {
    std::fs::create_dir_all(path.parent().expect("lock parent")).expect("lock dir");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .expect("open lock file");
    file.try_lock().expect("the peer takes the lock");
    PeerLock { _file: file }
}

struct Shared {
    _dir: tempfile::TempDir,
    paths: HostingPaths,
    capability: HostingCapability,
    inbox: RuntimeDeliveryInbox,
}

fn shared_store() -> Shared {
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = HostingPaths {
        hosting_lock_dir: dir.path().join("hosting"),
        cold_delivery_lock: dir.path().join("delivery").join("cold-delivery.lock"),
        database: None,
    };
    let store = Arc::new(
        SqliteRuntimeStore::new(dir.path().join("runtime.sqlite3"))
            .expect("open store")
            .with_hosting_paths(paths.clone()),
    );
    let capability = meerkat_runtime::RuntimeStore::hosting_capability(store.as_ref());
    assert!(capability.is_cross_process());
    Shared {
        _dir: dir,
        paths,
        capability,
        inbox: RuntimeDeliveryInbox::new(store),
    }
}

/// A completed job whose notification is subscribed by `recipient`.
async fn completed_job_for(recipient: &SessionId, key: &str) -> Arc<MemoryDetachedJobStore> {
    let job_store = Arc::new(MemoryDetachedJobStore::new());
    let jobs = DetachedJobService::new(job_store.clone());
    let receipt = jobs
        .submit(JobSpec::new(
            "default",
            SessionId::new(),
            meerkat::ExecutionIntentId::new(),
            InteractionLineageId::new(),
            ToolIdentity::new("shell", "1").expect("tool"),
            RunnerIdentity::new("durable-shell", "1").expect("runner"),
            RestartClass::Adoptable,
            CanonicalArgumentsHash::new(format!("hash-{key}")).expect("hash"),
            JobSubmissionKey::new(key).expect("submission key"),
        ))
        .await
        .expect("submit");
    jobs.subscribe(
        &receipt.job_id,
        JobSubscription::new(
            JobSubscriptionId::new("to-recipient").expect("subscription id"),
            recipient.clone(),
            JobDeliveryKind::Notification,
        ),
    )
    .await
    .expect("subscribe");
    let claim = jobs
        .claim_attempt(
            &receipt.job_id,
            AttemptClaim::new(
                WorkerId::new("worker").expect("worker"),
                1,
                100,
                RunnerHandleRef::new("runner-handle").expect("handle"),
            ),
        )
        .await
        .expect("claim");
    jobs.complete_attempt(
        &receipt.job_id,
        (&claim).into(),
        2,
        Some(JobResultRef::new("result").expect("result")),
    )
    .await
    .expect("complete");
    job_store
}

#[derive(Default)]
struct RecordingSink {
    applied: Mutex<Vec<SessionId>>,
}

#[async_trait::async_trait]
impl JobDeliverySink for RecordingSink {
    async fn apply(
        &self,
        application: JobDeliveryApplication,
    ) -> Result<(), JobDeliveryApplyError> {
        let session_id = match &application {
            JobDeliveryApplication::Record { subscription, .. }
            | JobDeliveryApplication::Notification { subscription, .. }
            | JobDeliveryApplication::Event { subscription, .. } => {
                subscription.session_id().clone()
            }
        };
        self.applied.lock().await.push(session_id);
        Ok(())
    }
}

/// Routes by its runtime owner's hosting claims, as a real host does: the
/// claim registry for routing, a real grant for a cold claim.
struct ClaimHost {
    authority: SessionHostingAuthority,
    sink: Arc<RecordingSink>,
}

impl ClaimHost {
    fn new(capability: &HostingCapability, sink: Arc<RecordingSink>) -> Self {
        Self {
            authority: SessionHostingAuthority::new(capability.clone(), HostingOwner::mint()),
            sink,
        }
    }
}

#[async_trait::async_trait]
impl RuntimeDeliveryHost for ClaimHost {
    async fn delivery_route(&self, session_id: &SessionId) -> Option<DeliveryRoute> {
        Some(match self.authority.serving(session_id) {
            SessionServing::HeldHere => DeliveryRoute::ServedHere(self.sink.clone()),
            SessionServing::HeldByAnotherLocalOwner => DeliveryRoute::ServedElsewhere,
            SessionServing::NotHeldInThisProcess => DeliveryRoute::Unserved(self.sink.clone()),
        })
    }

    async fn claim_cold_delivery(
        &self,
        session_id: &SessionId,
    ) -> Option<Result<HostingClaim, HostingRefused>> {
        Some(self.authority.grant(session_id))
    }
}

async fn wait_for_pass(
    passes: &mut watch::Receiver<RuntimeDeliveryPass>,
    what: &str,
    predicate: impl FnMut(&RuntimeDeliveryPass) -> bool,
) -> RuntimeDeliveryPass {
    tokio::time::timeout(EVENT_GUARD, passes.wait_for(predicate))
        .await
        .unwrap_or_else(|_| panic!("delivery owner never reached: {what}"))
        .expect("owner pass channel open")
        .clone()
}

/// A peer's death hands its sessions' deliveries to the cold-delivery owner:
/// the row of a session the peer hosts waits while the peer lives (the cold
/// owner's claim attempt is refused, with no delivery-authority input); once
/// the kernel releases the peer's claim, the owner's next store-watch sweep
/// retries the waiting cold recipient, its claim succeeds, and it
/// cold-delivers the row exactly once. Routing never touches the peer's lock.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_peers_death_hands_its_recipients_to_the_cold_owner() {
    let shared = shared_store();
    let peer_session = SessionId::new();
    let peer_claim = peer_lock(&meerkat_core::session_hosting::session_hosting_lock_path(
        &shared.paths.hosting_lock_dir,
        &peer_session,
    ));
    let job_store = completed_job_for(&peer_session, "peer-death").await;

    let sink = Arc::new(RecordingSink::default());
    let handle = RuntimeDeliveryOwner::new(job_store, shared.inbox.clone())
        .with_store_sweep(TEST_SWEEP)
        .arm(Arc::new(ClaimHost::new(&shared.capability, sink.clone())))
        .expect("arm owner");
    let mut passes = handle.subscribe_passes();
    let first = wait_for_pass(&mut passes, "the reconcile pass", |pass| {
        pass.generation >= 1
    })
    .await;
    assert!(
        first.applies_cold_deliveries,
        "the only owner holds cold delivery"
    );
    assert!(first.cross_process_wake_unavailable.is_none(), "{first:?}");
    assert_eq!(
        first.awaiting_other_hosts, 1,
        "the live peer serves the recipient: {first:?}"
    );
    assert!(sink.applied.lock().await.is_empty());

    drop(peer_claim);
    wait_for_pass(
        &mut passes,
        "cold delivery after the peer's death",
        |pass| pass.applied >= 1,
    )
    .await;
    assert_eq!(sink.applied.lock().await.as_slice(), &[peer_session]);
}

/// The cold-delivery owner's death passes cold ownership on: while another
/// process holds the store's cold-delivery lock, a row for a session no
/// process hosts waits; once that lock is released, the owner's next sweep
/// takes it and applies the row exactly once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_cold_owners_death_passes_cold_delivery_on() {
    let shared = shared_store();
    let other_cold_owner = peer_lock(&shared.paths.cold_delivery_lock);
    let cold_session = SessionId::new();
    let job_store = completed_job_for(&cold_session, "cold-owner-death").await;

    let sink = Arc::new(RecordingSink::default());
    let handle = RuntimeDeliveryOwner::new(job_store, shared.inbox.clone())
        .with_store_sweep(TEST_SWEEP)
        .arm(Arc::new(ClaimHost::new(&shared.capability, sink.clone())))
        .expect("arm owner");
    let mut passes = handle.subscribe_passes();
    let first = wait_for_pass(&mut passes, "the reconcile pass", |pass| {
        pass.generation >= 1
    })
    .await;
    assert!(
        !first.applies_cold_deliveries,
        "another process is the cold owner"
    );
    assert_eq!(first.awaiting_other_hosts, 1, "{first:?}");
    assert!(sink.applied.lock().await.is_empty());

    drop(other_cold_owner);
    let applied = wait_for_pass(&mut passes, "cold delivery after succession", |pass| {
        pass.applied >= 1
    })
    .await;
    assert!(applied.applies_cold_deliveries, "{applied:?}");
    assert_eq!(sink.applied.lock().await.as_slice(), &[cold_session]);
}

/// ADR's #1813 B correction, delivery level: the store selected OS-locked
/// claims, and both lock locations become unusable after startup (a regular
/// file where each directory belongs). The owner is NOT the sole cold owner
/// and delivers nothing: no unclaimed cold delivery. With the cold-delivery
/// lock usable again it becomes the cold owner at its next sweep, but the
/// recipient's own claim is still unavailable, so the row still waits with
/// nothing applied. Paired: once the session's claim can be taken, the same
/// owner delivers the row exactly once under it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lock_unavailable_after_startup_hosts_and_delivers_nothing_unclaimed() {
    let shared = shared_store();
    let hosting_dir = shared.paths.hosting_lock_dir.clone();
    let cold_dir = shared
        .paths
        .cold_delivery_lock
        .parent()
        .expect("cold lock parent")
        .to_path_buf();
    std::fs::write(&hosting_dir, b"not a directory").expect("block the lock dir");
    std::fs::write(&cold_dir, b"not a directory").expect("block the cold lock dir");
    let cold_session = SessionId::new();
    let host = Arc::new(ClaimHost::new(
        &shared.capability,
        Arc::new(RecordingSink::default()),
    ));
    assert!(
        matches!(
            host.authority.grant(&cold_session),
            Err(HostingRefused::Unavailable(_))
        ),
        "an unavailable claim is refused, never granted unclaimed"
    );
    let job_store = completed_job_for(&cold_session, "lock-unavailable").await;

    let sink = host.sink.clone();
    let handle = RuntimeDeliveryOwner::new(job_store, shared.inbox.clone())
        .with_store_sweep(TEST_SWEEP)
        .arm(host)
        .expect("arm owner");
    let mut passes = handle.subscribe_passes();
    let first = wait_for_pass(&mut passes, "the reconcile pass", |pass| {
        pass.generation >= 1
    })
    .await;
    assert!(
        !first.applies_cold_deliveries,
        "an unavailable cold-delivery lock is not sole ownership: {first:?}"
    );
    assert_eq!(first.applied, 0, "{first:?}");
    assert_eq!(first.awaiting_other_hosts, 1, "{first:?}");
    assert!(sink.applied.lock().await.is_empty());

    std::fs::remove_file(&cold_dir).expect("unblock the cold lock dir");
    let cold_owner = wait_for_pass(
        &mut passes,
        "cold ownership once its lock is usable",
        |pass| pass.applies_cold_deliveries,
    )
    .await;
    assert_eq!(
        cold_owner.applied, 0,
        "the recipient's claim is still unavailable: {cold_owner:?}"
    );
    assert!(sink.applied.lock().await.is_empty());

    std::fs::remove_file(&hosting_dir).expect("unblock the lock dir");
    wait_for_pass(
        &mut passes,
        "cold delivery once the claim is usable",
        |pass| pass.applied >= 1,
    )
    .await;
    assert_eq!(sink.applied.lock().await.as_slice(), &[cold_session]);
}

//! A host's pure observation of a work's own publication while another actor
//! issues grants on it, through the real owners: a durable grant owner whose
//! appends can be held, and the native work owner, composed on one
//! publication. An issuance holds the publication's writer reserved, not yet
//! published, across its acknowledged append, and the grant owner's mutex
//! with it, and publishes before it releases that mutex. A lineage read that
//! reaches the mutex meanwhile waits there and ends stale once the issuance
//! published, usually after it completed.
//!
//! Channels order every step. The waits run on paused virtual time, and the
//! issuer's thread never touches the runtime.

use std::sync::mpsc;

use meerkat_authorization_contracts::grant::GrantLineageRef;
use meerkat_core::authorization::{OWNER_CHANGE_WAIT_BOUND, OwnerChangeChurn};

use super::*;
use crate::grants::{
    DurableGrantOwner, GrantOwnerChange, GrantOwnerIdentity, GrantOwnerStore, GrantStoreError,
    InMemoryGrantOwnerStore, LineageResolutionRefusal, LocalGrantAuthority,
    LocalGrantConfiguration,
};

const SITE: &str = "stale_read";

/// The grant owner's store. While `holding` is set, each append reports
/// that it entered and writes only once it is released.
struct HeldAppends {
    inner: InMemoryGrantOwnerStore,
    holding: AtomicBool,
    entered: mpsc::Sender<()>,
    released: Mutex<mpsc::Receiver<()>>,
}

impl GrantOwnerStore for HeldAppends {
    fn load(&self) -> Result<Option<DurableGrantOwner>, GrantStoreError> {
        self.inner.load()
    }

    fn create(&self, identity: &GrantOwnerIdentity) -> Result<(), GrantStoreError> {
        self.inner.create(identity)
    }

    fn append(&self, change: &GrantOwnerChange) -> Result<(), GrantStoreError> {
        if self.holding.load(Ordering::SeqCst) {
            self.entered
                .send(())
                .expect("the test waits for the held append");
            self.released
                .lock()
                .expect("release channel")
                .recv()
                .expect("the test releases the held append");
        }
        self.inner.append(change)
    }

    fn release(&self) {
        self.inner.release();
    }
}

/// The grant owner's clock. A lineage read takes it just before it waits for
/// the grant owner's mutex, so a release armed here is sent once the read
/// reached that mutex.
#[derive(Default)]
struct ReleasingClock {
    armed: Mutex<Option<mpsc::Sender<()>>>,
}

impl ReleasingClock {
    /// The next read of this clock sends one release on `release`.
    fn release_at_next_read(&self, release: mpsc::Sender<()>) {
        *self.armed.lock().expect("armed release") = Some(release);
    }
}

impl LocalAuthorizationClock for ReleasingClock {
    fn now(&self) -> Result<LocalAuthorizationTime, LocalClockError> {
        let armed = self.armed.lock().expect("armed release").take();
        if let Some(release) = armed {
            release
                .send(())
                .expect("the held append waits for its release");
        }
        Ok(LocalAuthorizationTime {
            unix_ms: 1_000,
            monotonic: Instant::now(),
        })
    }
}

/// What the host's pure observation answered.
#[derive(Debug, PartialEq, Eq)]
enum Read {
    /// The lineage resolved within one coherent observation.
    Current,
    /// An owner change published during the read: stale, never a verdict.
    Stale,
    /// The owner's refusal or an unavailable publication: final.
    Final,
}

/// The native work owner and a durable grant owner on one publication, as a
/// host composes every owner, with the observer's own grant and the parent
/// grant of another actor's issuances.
struct World {
    fixture: Fixture,
    grants: Arc<LocalGrantAuthority>,
    store: Arc<HeldAppends>,
    clock: Arc<ReleasingClock>,
    entered: mpsc::Receiver<()>,
    release: mpsc::Sender<()>,
    observed: GrantLineageRef,
    parent: GrantLineageRef,
}

impl World {
    fn new() -> Self {
        let fixture = Fixture::new(ExecutionRestrictions::unrestricted());
        let (entered_tx, entered) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let store = Arc::new(HeldAppends {
            inner: InMemoryGrantOwnerStore::new(),
            holding: AtomicBool::new(false),
            entered: entered_tx,
            released: Mutex::new(released),
        });
        let clock = Arc::new(ReleasingClock::default());
        let grants = Arc::new(
            LocalGrantAuthority::new_durable(
                LocalGrantConfiguration {
                    root: principal("grant-root"),
                    namespace: id("grants"),
                    generation: 1,
                },
                store.clone(),
                fixture.publication.clone(),
                clock.clone(),
            )
            .expect("a durable grant owner on the work's publication"),
        );
        let root = |name: &str, grantee: &str| {
            grants
                .issue_root(
                    &principal("grant-root"),
                    id(name),
                    principal(grantee),
                    None,
                    ExecutionRestrictions::unrestricted(),
                )
                .expect("a root grant")
        };
        let observed = root("observed", "reader");
        let parent = root("parent", "issuer");
        Self {
            fixture,
            grants,
            store,
            clock,
            entered,
            release,
            observed,
            parent,
        }
    }

    /// From now on every append of the grant owner holds until released.
    fn hold_appends(&self) {
        self.store.holding.store(true, Ordering::SeqCst);
    }

    /// Another actor issues a child of its parent grant for each of
    /// `children` in turn, on its own thread.
    fn issue(&self, children: &'static [&'static str]) -> std::thread::JoinHandle<()> {
        let grants = Arc::clone(&self.grants);
        let parent = self.parent.clone();
        std::thread::spawn(move || {
            for child in children {
                grants
                    .issue_child(
                        &principal("issuer"),
                        &parent,
                        id(child),
                        principal("helper"),
                        ExecutionRestrictions::unrestricted(),
                    )
                    .expect("the child is issued");
            }
        })
    }

    /// Returns once the issuance in progress holds its append: the writer
    /// reserved, nothing published yet.
    fn await_held_append(&self) {
        self.entered.recv().expect("an issuance holds its append");
    }

    fn release_append(&self) {
        self.release.send(()).expect("the held append waits");
    }

    /// The host's pure observation: its own observation of the work's
    /// publication around an observed lineage resolution of the observer's
    /// grant, which takes the grant owner's mutex. It reads owners and runs
    /// no effect.
    fn read(&self) -> Read {
        let observed = self.fixture.publication.observe(|| {
            self.grants.resolve_lineage_observed(
                std::slice::from_ref(&self.observed),
                &principal("reader"),
                None,
            )
        });
        match observed {
            Ok((Ok(_), _)) => Read::Current,
            Ok((Err(LineageResolutionRefusal::Changed), _)) | Err(PublicationError::Changed) => {
                Read::Stale
            }
            Ok((Err(LineageResolutionRefusal::Refused(_)), _))
            | Err(PublicationError::Unavailable) => Read::Final,
        }
    }

    /// [`Self::read`] while an issuance holds its append: the append is
    /// released only once the read reached the grant owner's mutex, so the
    /// read waits there until the issuance published.
    fn read_meeting_the_held_issuance(&self) -> Read {
        self.clock.release_at_next_read(self.release.clone());
        self.read()
    }
}

/// A grant issuance holds the publication's writer reserved, not yet
/// published, across its acknowledged append. The native work owner names
/// it as an owner change in progress from then until the issuance
/// completes; `change_in_progress` names only a published change.
///
/// RED at v0.9.7: the work named nothing before the issuance published, so
/// a wait had no holder to wait for.
#[test]
fn a_held_issuance_is_named_in_progress_before_it_publishes_and_until_it_completes() {
    let world = World::new();
    let work = &world.fixture.authorization;
    assert_eq!(work.owner_change_in_progress(), None, "nothing is held yet");
    world.hold_appends();
    let issuer = world.issue(&["held-child"]);
    world.await_held_append();
    assert!(
        work.owner_change_in_progress().is_some(),
        "the unpublished issuance is named while it holds the writer"
    );
    assert_eq!(
        world.fixture.publication.change_in_progress(),
        None,
        "nothing has published yet"
    );
    world.release_append();
    issuer.join().expect("the issuer");
    assert_eq!(
        work.owner_change_in_progress(),
        None,
        "completed: named no longer"
    );
    assert!(
        world
            .grants
            .issued_reference(&id("held-child"))
            .expect("lookup")
            .is_some(),
        "the child is issued"
    );
}

/// Another actor issues two child grants on the work's own publication. The
/// host's pure observation reaches the grant owner's mutex during each
/// issuance's append and ends stale once that issuance published: the first
/// read and its one rebuild both end stale. By the time the host asks, the
/// second issuance has completed and nothing is in progress, so the
/// settled-change wait has nothing to wait for. The stale-read wait pauses
/// one backoff step and settles as one free rebuild, the read made again is
/// current, and both children are issued.
///
/// RED on the base: no public wait settled this read, so the host ended
/// stale.
#[tokio::test(start_paused = true)]
async fn a_pure_observation_behind_completed_issuances_reads_again_within_the_bound() {
    let world = World::new();
    world.hold_appends();
    let issuer = world.issue(&["first-child", "second-child"]);
    for read in ["the first read", "its one rebuild"] {
        world.await_held_append();
        assert_eq!(
            world.read_meeting_the_held_issuance(),
            Read::Stale,
            "{read} meets an issuance"
        );
    }
    issuer.join().expect("the issuer");

    let work = &world.fixture.authorization;
    assert_eq!(
        work.owner_change_in_progress(),
        None,
        "both issuances completed"
    );
    let mut churn = OwnerChangeChurn::new();
    assert_eq!(
        churn.wait_for_settled_change(work, None, SITE, None).await,
        None,
        "the settled-change wait has nothing to wait for"
    );
    let wait = churn.wait_after_stale_read(work, None, SITE, None).await;
    assert!(
        wait.settled() && wait.waited() < OWNER_CHANGE_WAIT_BOUND,
        "one free rebuild, within the bound: {wait:?}"
    );
    assert_eq!(
        world.read(),
        Read::Current,
        "the read made again is current"
    );
    for child in ["first-child", "second-child"] {
        assert!(
            world
                .grants
                .issued_reference(&id(child))
                .expect("lookup")
                .is_some(),
            "{child} is issued"
        );
    }
}

/// The bounds, while another actor's issuance holds its append past them:
/// the work names the issuance throughout, so no wait settles. Each ends at
/// its churn's one deadline (the bound, or the caller's budget when less)
/// and not past it, and a later wait of the same churn starts nothing: the
/// host keeps its stale answer.
///
/// RED at v0.9.7: nothing named the unpublished issuance, so the wait
/// settled after its pause while the issuance still held.
#[tokio::test(start_paused = true)]
async fn a_wait_after_a_stale_read_ends_at_its_deadline_while_an_issuance_holds() {
    let world = World::new();
    world.hold_appends();
    let issuer = world.issue(&["stuck-child"]);
    world.await_held_append();
    let work = &world.fixture.authorization;
    for (budget, deadline) in [
        (None, OWNER_CHANGE_WAIT_BOUND),
        (Some(Duration::from_millis(300)), Duration::from_millis(300)),
    ] {
        let mut churn = OwnerChangeChurn::new();
        let started = tokio::time::Instant::now();
        let wait = churn.wait_after_stale_read(work, budget, SITE, None).await;
        assert!(
            !wait.settled() && wait.waited() >= deadline && started.elapsed() <= deadline,
            "held throughout: it ends at the deadline, not past it: {wait:?}"
        );
        let again = churn.wait_after_stale_read(work, budget, SITE, None).await;
        assert_eq!(
            (again.settled(), again.waited()),
            (false, Duration::ZERO),
            "no wait starts past the deadline"
        );
    }
    world.release_append();
    issuer.join().expect("the issuer");
}

/// Cancellation: the wait holds nothing, so dropping it mid-wait releases
/// nothing. The held issuance completes on its own; afterwards nothing is in
/// progress, the writer is free, and an observation made then stays
/// current: the dropped wait published nothing.
#[tokio::test(start_paused = true)]
async fn a_cancelled_wait_after_a_stale_read_releases_nothing_because_it_holds_nothing() {
    let world = World::new();
    world.hold_appends();
    let issuer = world.issue(&["held-child"]);
    world.await_held_append();
    let work = &world.fixture.authorization;
    let mut churn = OwnerChangeChurn::new();
    let mut wait = Box::pin(churn.wait_after_stale_read(work, None, SITE, None));
    assert!(
        futures::poll!(wait.as_mut()).is_pending(),
        "waiting for the held issuance"
    );
    drop(wait);

    world.release_append();
    issuer.join().expect("the issuance completes on its own");
    assert_eq!(
        work.owner_change_in_progress(),
        None,
        "nothing left in progress"
    );
    let ((), stamp) = world
        .fixture
        .publication
        .observe(|| ())
        .expect("a coherent observation");
    assert_eq!(
        world.grants.try_with_owner_changes_excluded(|| ()),
        Ok(()),
        "the writer is free"
    );
    assert_eq!(stamp.check_current(), Ok(()), "nothing published since");
}

//! The waiting form of the governed helper admission
//! (`GovernedHelperAdmissionFence::create_idempotent_after_owner_change`):
//! every attempt is a new fence and the same idempotent create. A refusal
//! that met contention (another writer's reservation of the publication that
//! has not published, or an owner change in progress on it), and on the
//! dispatch paths every refusal to rebuild, as Core's waiting step forms
//! treat it, is waited out outside every lock; every other answer comes at
//! once.
//!
//! The rows run on paused virtual time. The other writer holds the
//! publication on its own thread, which never touches the runtime, and every
//! wait and deadline is on the runtime's clock, so the timing is exact. A
//! waiting admission's first poll is the witness: pending means its first
//! attempt met the hold and it waits; ready means it answered at once.

use super::*;
use meerkat_authorization::grants::{
    DurableGrantOwner, GrantOwnerChange, GrantOwnerIdentity, GrantOwnerStore, GrantStoreError,
    InMemoryGrantOwnerStore,
};
use meerkat_core::authorization::{OWNER_CHANGE_FREE_REBUILDS, OWNER_CHANGE_WAIT_BOUND};
use meerkat_workgraph::{WorkGraphError, WorkItemFilter};
use std::task::Poll;
use std::time::Duration;

/// One armed append: it reports that it began, then waits for its release.
struct Gate {
    began: std::sync::mpsc::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

/// A grant owner store whose armed append waits, as a slow acknowledged
/// append does.
struct GatedAppends {
    inner: InMemoryGrantOwnerStore,
    gate: Mutex<Option<Gate>>,
}

impl GrantOwnerStore for GatedAppends {
    fn load(&self) -> Result<Option<DurableGrantOwner>, GrantStoreError> {
        self.inner.load()
    }

    fn create(&self, identity: &GrantOwnerIdentity) -> Result<(), GrantStoreError> {
        self.inner.create(identity)
    }

    fn append(&self, change: &GrantOwnerChange) -> Result<(), GrantStoreError> {
        let gate = self.gate.lock().expect("the gate").take();
        if let Some(gate) = gate {
            gate.began.send(()).expect("the append is reported");
            // Released, or the sender dropped: `recv` returns either way.
            let _ = gate.release.recv();
        }
        self.inner.append(change)
    }

    fn release(&self) {
        self.inner.release();
    }
}

/// Another writer of the fixture's publication: a root issuance of a
/// second, durable grant owner on that publication, on its own thread.
/// Through its acknowledged append it holds the publication's writer,
/// reserved and not yet published; once released, the append is
/// acknowledged and the issuance publishes and completes.
struct GatedIssuance {
    release: std::sync::mpsc::Sender<()>,
    thread: std::thread::JoinHandle<()>,
}

impl GatedIssuance {
    /// Returns once the issuance holds the writer, inside its append.
    fn begin(f: &Fixture, name: &str) -> Self {
        let (began, appending) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let store = Arc::new(GatedAppends {
            inner: InMemoryGrantOwnerStore::new(),
            gate: Mutex::new(None),
        });
        let root = principal("writer-owner");
        let writer = LocalGrantAuthority::new_durable(
            LocalGrantConfiguration {
                root: root.clone(),
                namespace: id(&format!("writer-{name}")),
                generation: 1,
            },
            Arc::clone(&store) as Arc<dyn GrantOwnerStore>,
            f.publication.clone(),
            Arc::clone(&f.clock) as Arc<dyn LocalAuthorizationClock>,
        )
        .expect("a second durable grant owner on the publication");
        *store.gate.lock().expect("the gate") = Some(Gate {
            began,
            release: released,
        });
        let grant = id(&format!("writer-grant-{name}"));
        let thread = std::thread::spawn(move || {
            writer
                .issue_root(
                    &root,
                    grant,
                    principal("writer-grantee"),
                    None,
                    ExecutionRestrictions::unrestricted(),
                )
                .expect("the issuance publishes once its append is acknowledged");
        });
        appending
            .recv()
            .expect("the issuance holds the writer, unpublished");
        Self { release, thread }
    }

    /// The append is acknowledged: the issuance publishes and completes.
    fn publish(self) {
        drop(self.release);
        self.thread.join().expect("the writer's thread");
    }
}

/// How a host builds the helper's fence.
#[derive(Clone, Copy, Debug)]
enum FenceFrom {
    /// The prepared setup operation with its native composition.
    Prepared,
    /// The governed tool dispatch's own operation.
    Dispatch,
    /// A step of that dispatch's entered operation.
    Entered,
}

impl FenceFrom {
    fn label(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Dispatch => "dispatch",
            Self::Entered => "entered",
        }
    }
}

const PATHS: [FenceFrom; 3] = [FenceFrom::Prepared, FenceFrom::Dispatch, FenceFrom::Entered];

fn admission_key(value: &str) -> WorkAdmissionKey {
    WorkAdmissionKey::new(value).expect("key")
}

/// One native composition with a governed dispatch in the child's work and
/// its entered operation, the retained original `s` every fence names, and a
/// WorkGraph.
struct Admissions {
    f: Fixture,
    dispatch: Dispatch,
    entered: meerkat_core::EnteredDispatchOperation,
    service: WorkGraphService,
}

impl Admissions {
    fn new() -> Self {
        Self::of(Fixture::new())
    }

    fn of(f: Fixture) -> Self {
        f.source("s", ExecutionRestrictions::unrestricted());
        let dispatch = Dispatch::new(&f.context);
        let entered = dispatch.enter();
        Self {
            f,
            dispatch,
            entered,
            service: WorkGraphService::new(Arc::new(MemoryWorkGraphStore::new())),
        }
    }

    /// The fence `from` the given path for the admission of `key`.
    fn fence(&self, from: FenceFrom, key: &str) -> Arc<GovernedHelperAdmissionFence> {
        let admission = self
            .service
            .admission_identity(admission_key(key), setup_request())
            .expect("identity");
        match from {
            FenceFrom::Prepared => GovernedHelperAdmissionFence::new(
                Arc::clone(&self.f.grants),
                self.f.ledger.clone(),
                self.f.child.clone(),
                retained_originals(&["s"]),
                self.f.setup.clone(),
                Arc::clone(&self.f.policy),
                Arc::clone(&self.f.clock) as Arc<dyn LocalAuthorizationClock>,
                "default",
                WorkNamespace::default(),
                admission,
            ),
            FenceFrom::Dispatch => GovernedHelperAdmissionFence::from_dispatch(
                self.dispatch.call(),
                &self.dispatch.context,
                self.f.ledger.clone(),
                retained_originals(&["s"]),
                "default",
                WorkNamespace::default(),
                admission,
            )
            .expect("the dispatch's own operation builds the fence"),
            FenceFrom::Entered => GovernedHelperAdmissionFence::from_entered(
                &self.entered,
                self.f.ledger.clone(),
                retained_originals(&["s"]),
                "default",
                WorkNamespace::default(),
                admission,
            ),
        }
    }

    /// The direct single attempt of `key` through `fence`.
    async fn create_once(
        &self,
        fence: &Arc<GovernedHelperAdmissionFence>,
        key: &str,
    ) -> Result<WorkAdmissionOutcome, WorkGraphError> {
        self.service
            .create_idempotent_with_fence(admission_key(key), setup_request(), fence.clone())
            .await
    }

    /// The waiting admission of `key` through `fence`, with no budget of its
    /// own.
    async fn create(
        &self,
        fence: &GovernedHelperAdmissionFence,
        key: &str,
    ) -> Result<WorkAdmissionOutcome, WorkGraphError> {
        fence
            .create_idempotent_after_owner_change(
                &self.service,
                admission_key(key),
                setup_request(),
                None,
            )
            .await
    }

    /// Every item the WorkGraph holds.
    async fn items(&self) -> usize {
        self.service
            .list(WorkItemFilter {
                include_terminal: true,
                ..Default::default()
            })
            .await
            .expect("list")
            .len()
    }
}

/// RED-1 (writer contention): another writer holds the publication, reserved
/// and not yet published (a durable grant issuance in its acknowledged
/// append). A direct single attempt answers `Unavailable` and commits
/// nothing. The waiting admission meets the same hold and waits for it (its
/// first poll is pending); once the writer publishes, its next attempt, a
/// new fence, creates the item. On the prepared, dispatch and entered paths
/// alike.
///
/// RED on the single-attempt form: the waiting admission answered
/// `Unavailable` at once, as the direct attempt does.
#[tokio::test(start_paused = true)]
async fn a_create_refused_by_another_writers_reservation_completes_once_it_publishes() {
    let admissions = Admissions::new();
    for from in PATHS {
        let key = format!("{}-held", from.label());
        let writer = GatedIssuance::begin(&admissions.f, &key);

        let direct = admissions.fence(from, &key);
        assert!(
            matches!(
                admissions.create_once(&direct, &key).await,
                Err(WorkGraphError::Conflict(_))
            ),
            "{from:?}: the direct single attempt refuses"
        );
        assert_eq!(
            direct.refusal(),
            Some(HelperAdmissionRefusal::Unavailable),
            "{from:?}: as unavailable"
        );

        let template = admissions.fence(from, &key);
        let mut waiting = std::pin::pin!(admissions.create(&template, &key));
        assert!(
            futures::poll!(waiting.as_mut()).is_pending(),
            "RED-1 {from:?}: the waiting admission answered at once instead of waiting for the writer"
        );
        writer.publish();
        assert!(
            matches!(waiting.await, Ok(WorkAdmissionOutcome::Created(_))),
            "{from:?}: created once the writer published"
        );
        assert_eq!(
            template.refusal(),
            None,
            "{from:?}: the answered attempt was admitted"
        );
        assert_eq!(
            template.attempts().len(),
            2,
            "{from:?}: one refused attempt, then one that committed"
        );
    }
    assert_eq!(
        admissions.items().await,
        PATHS.len(),
        "each admission committed once"
    );
}

/// GUARD-2 (`Denied`): a denial is final for the admission, so the waiting
/// admission answers it at once, after exactly one attempt: for a fence
/// bound to another admission, denied before its check even while another
/// writer holds the publication, and for a retained original cancelled
/// since the setup operation was prepared, denied by the check. On every
/// path.
///
/// Passes on the single-attempt form too; it fails for a form that waits
/// whenever the publication names a hold, or that retries every refusal.
#[tokio::test(start_paused = true)]
async fn a_denied_admission_answers_at_once_after_one_attempt() {
    let admissions = Admissions::new();
    let holder = HeldExclusion::begin(&admissions.f);
    for from in PATHS {
        let template = admissions.fence(from, "bound-elsewhere");
        let key = format!("{}-denied", from.label());
        let answer = {
            let mut waiting = std::pin::pin!(admissions.create(&template, &key));
            futures::poll!(waiting.as_mut())
        };
        assert!(
            matches!(answer, Poll::Ready(Err(WorkGraphError::Conflict(_)))),
            "{from:?}: another admission is answered at once, while the writer is held"
        );
        assert_eq!(
            template.refusal(),
            Some(HelperAdmissionRefusal::Denied),
            "{from:?}: denied"
        );
        assert_eq!(
            template.attempts().len(),
            1,
            "{from:?}: exactly one attempt"
        );
    }
    holder.release();

    admissions
        .f
        .ledger
        .cancel_operations(&namespace("s"), &work("s"), evidence("withdrawn"))
        .expect("the retained original is cancelled");
    for from in PATHS {
        let key = format!("{}-cancelled", from.label());
        let template = admissions.fence(from, &key);
        let answer = {
            let mut waiting = std::pin::pin!(admissions.create(&template, &key));
            futures::poll!(waiting.as_mut())
        };
        assert!(
            matches!(answer, Poll::Ready(Err(WorkGraphError::Conflict(_)))),
            "{from:?}: a cancelled original is answered at once"
        );
        assert_eq!(
            template.refusal(),
            Some(HelperAdmissionRefusal::Denied),
            "{from:?}: denied by the check"
        );
        assert_eq!(
            template.attempts().len(),
            1,
            "{from:?}: exactly one attempt"
        );
    }
    assert_eq!(admissions.items().await, 0, "nothing committed");
}

/// GUARD-3 (no contention): an `Unavailable` that met no other writer is
/// answered at once, after exactly one attempt: a poisoned publication, on
/// every path, and on the prepared path the setup check's refusal to rebuild
/// (a host policy answering `ReprepareRequired`), which ran inside the
/// fence's own exclusion. A dispatch-path refusal to rebuild is retried
/// instead (RED-7).
///
/// Passes on the single-attempt form too; it fails for a form that waits
/// after every `Unavailable`.
#[tokio::test(start_paused = true)]
async fn an_unavailable_admission_without_contention_answers_at_once_after_one_attempt() {
    let owner = Arc::new(ChurnedRecords(std::sync::atomic::AtomicU8::new(0)));
    let admissions = Admissions::of(Fixture::new_with(owner.clone()));
    owner.0.store(1, Ordering::SeqCst);
    for (cause, poison, paths) in [
        ("rebuild", false, &[FenceFrom::Prepared][..]),
        ("poisoned", true, &PATHS[..]),
    ] {
        if poison {
            owner.0.store(0, Ordering::SeqCst);
            admissions.f.publication.poison();
        }
        for &from in paths {
            let key = format!("{}-{cause}", from.label());
            let template = admissions.fence(from, &key);
            let answer = {
                let mut waiting = std::pin::pin!(admissions.create(&template, &key));
                futures::poll!(waiting.as_mut())
            };
            assert!(
                matches!(answer, Poll::Ready(Err(WorkGraphError::Conflict(_)))),
                "{from:?}, {cause}: answered at once"
            );
            assert_eq!(
                template.refusal(),
                Some(HelperAdmissionRefusal::Unavailable),
                "{from:?}, {cause}: unavailable"
            );
            assert_eq!(
                template.attempts().len(),
                1,
                "{from:?}, {cause}: exactly one attempt"
            );
        }
    }
    assert_eq!(admissions.items().await, 0, "nothing committed");
}

/// RED-4 (the bound): a hold that outlives the wait's deadline answers
/// `Unavailable` once the wait is not settled. The waiting admission waits
/// exactly to its deadline (the owner-change bound, or the budget passed
/// when that is less), makes no attempt after it, before or after the
/// holder releases, and commits nothing.
///
/// RED on the single-attempt form: it answered at once, without a wait.
#[tokio::test(start_paused = true)]
async fn a_hold_past_the_bound_answers_unavailable_with_no_attempt_after_the_deadline() {
    let admissions = Admissions::new();
    for (budget, deadline) in [
        (None, OWNER_CHANGE_WAIT_BOUND),
        (Some(Duration::from_millis(300)), Duration::from_millis(300)),
    ] {
        let key = format!("held-past-{}", deadline.as_millis());
        let holder = HeldExclusion::begin(&admissions.f);
        let template = admissions.fence(FenceFrom::Prepared, &key);
        let started = tokio::time::Instant::now();
        let answer = template
            .create_idempotent_after_owner_change(
                &admissions.service,
                admission_key(&key),
                setup_request(),
                budget,
            )
            .await;
        let waited = started.elapsed();
        assert!(
            matches!(answer, Err(WorkGraphError::Conflict(_))),
            "budget {budget:?}: refused"
        );
        assert_eq!(
            template.refusal(),
            Some(HelperAdmissionRefusal::Unavailable),
            "budget {budget:?}: the last attempt's unavailability"
        );
        assert!(
            (deadline..deadline + Duration::from_millis(2)).contains(&waited),
            "RED-4 budget {budget:?}: it waits to its deadline and not past it: {waited:?}"
        );
        assert_eq!(
            template.attempts().len(),
            1,
            "budget {budget:?}: no attempt after the deadline"
        );
        holder.release();
        tokio::time::advance(OWNER_CHANGE_WAIT_BOUND).await;
        assert_eq!(
            template.attempts().len(),
            1,
            "budget {budget:?}: nor once the holder released"
        );
    }
    assert_eq!(admissions.items().await, 0, "nothing committed");
}

/// RED-5 (fresh fences): every attempt is a new fence. The fence the host
/// built is never handed to the store: the waiting admission's attempts are
/// fences of their own, each attempted exactly once, the first refused
/// while another writer held the publication and the second admitted once
/// it released. The host's fence serves this one waiting admission: asked
/// for another, it refuses at once and makes no attempt.
///
/// RED on the single-attempt form: it made one attempt and answered at
/// once.
#[tokio::test(start_paused = true)]
async fn every_attempt_of_a_waiting_admission_is_a_fresh_fence() {
    let admissions = Admissions::new();
    let holder = HeldExclusion::begin(&admissions.f);
    let template = admissions.fence(FenceFrom::Prepared, "fresh");
    let mut waiting = std::pin::pin!(admissions.create(&template, "fresh"));
    let first = futures::poll!(waiting.as_mut());
    holder.release();
    assert!(
        first.is_pending(),
        "RED-5: the waiting admission answered at once after one attempt"
    );
    assert!(
        matches!(waiting.await, Ok(WorkAdmissionOutcome::Created(_))),
        "created once the holder released"
    );

    let attempts = template.attempts();
    assert_eq!(attempts.len(), 2, "one fence per attempt");
    for (index, attempt) in attempts.iter().enumerate() {
        assert!(
            !Arc::ptr_eq(attempt, &template),
            "attempt {index} is a fence of its own, not the host's"
        );
        assert!(
            attempt.was_attempted(),
            "attempt {index} served its one attempt"
        );
    }
    assert!(
        !Arc::ptr_eq(&attempts[0], &attempts[1]),
        "each attempt is a new fence"
    );
    assert_eq!(
        attempts[0].refusal(),
        Some(HelperAdmissionRefusal::Unavailable),
        "the first met the hold"
    );
    assert_eq!(attempts[1].refusal(), None, "the second was admitted");

    assert!(
        matches!(
            admissions.create(&template, "fresh").await,
            Err(WorkGraphError::Conflict(_))
        ),
        "the host's fence served its one waiting admission"
    );
    assert_eq!(template.attempts().len(), 2, "and makes no further attempt");
    assert_eq!(template.refusal(), None, "its admission's outcome is kept");
    assert_eq!(admissions.items().await, 1, "one item");
}

/// RED-6 (cancellation): dropping the waiting admission while it waits
/// commits nothing and uses no further fence. After the holder released and
/// the bound passed, the one attempt it made (refused, nothing run) is all
/// there is, the WorkGraph holds no item and the host's fence records no
/// outcome. A new waiting admission of the same key then creates the item,
/// not a replay: nothing was committed before.
///
/// RED on the single-attempt form: it answered at once, so nothing waited
/// to be dropped.
#[tokio::test(start_paused = true)]
async fn a_dropped_waiting_admission_creates_nothing_and_uses_no_further_fence() {
    let admissions = Admissions::new();
    let holder = HeldExclusion::begin(&admissions.f);
    let template = admissions.fence(FenceFrom::Prepared, "dropped");
    let first = {
        let mut waiting = std::pin::pin!(admissions.create(&template, "dropped"));
        futures::poll!(waiting.as_mut()).is_pending()
    };
    holder.release();
    assert!(
        first,
        "RED-6: the waiting admission answered at once, before it could be dropped"
    );
    tokio::time::advance(OWNER_CHANGE_WAIT_BOUND).await;

    let attempts = template.attempts();
    assert_eq!(attempts.len(), 1, "no fence used after the drop");
    assert_eq!(
        attempts[0].refusal(),
        Some(HelperAdmissionRefusal::Unavailable),
        "its one attempt refused and ran nothing"
    );
    assert_eq!(
        template.refusal(),
        None,
        "the dropped admission answered nothing"
    );
    assert_eq!(admissions.items().await, 0, "no item created");

    let again = admissions.fence(FenceFrom::Prepared, "dropped");
    assert!(
        matches!(
            admissions.create(&again, "dropped").await,
            Ok(WorkAdmissionOutcome::Created(_))
        ),
        "a new admission of the key creates the item"
    );
}

/// RED-7 (a dispatch-path refusal to rebuild): on the dispatch and entered
/// paths a refusal to rebuild with no change in progress (a host policy
/// answering `ReprepareRequired` inside the work owners' exclusion) is
/// retried as Core's waiting step forms retry it: after a 1 ms pause, as one
/// of the call's free rebuilds. Once the owners answer again, the next
/// attempt creates the item. While they keep refusing, the waiting admission
/// makes one attempt per free rebuild, each after its pause, and answers the
/// last `Unavailable` once they are spent. The prepared path answers such a
/// refusal at once (GUARD-3).
///
/// RED before the dispatch-path rule: the refusal named no change or hold, so
/// it was answered at once, after one attempt.
#[tokio::test(start_paused = true)]
async fn a_dispatch_refusal_to_rebuild_is_retried_once_per_free_rebuild() {
    let owner = Arc::new(ChurnedRecords(std::sync::atomic::AtomicU8::new(0)));
    let admissions = Admissions::of(Fixture::new_with(owner.clone()));
    let pauses = Duration::from_millis(u64::from(OWNER_CHANGE_FREE_REBUILDS));
    for from in [FenceFrom::Dispatch, FenceFrom::Entered] {
        owner.0.store(1, Ordering::SeqCst);
        let key = format!("{}-rebuilt", from.label());
        let template = admissions.fence(from, &key);
        let mut waiting = std::pin::pin!(admissions.create(&template, &key));
        assert!(
            futures::poll!(waiting.as_mut()).is_pending(),
            "RED-7 {from:?}: the refusal to rebuild was answered at once"
        );
        owner.0.store(0, Ordering::SeqCst);
        assert!(
            matches!(waiting.await, Ok(WorkAdmissionOutcome::Created(_))),
            "{from:?}: created once the owners answer again"
        );
        assert_eq!(
            template.attempts().len(),
            2,
            "{from:?}: one retry after the pause"
        );

        owner.0.store(1, Ordering::SeqCst);
        let key = format!("{}-still-refused", from.label());
        let template = admissions.fence(from, &key);
        let started = tokio::time::Instant::now();
        let answer = admissions.create(&template, &key).await;
        let waited = started.elapsed();
        assert!(
            matches!(answer, Err(WorkGraphError::Conflict(_))),
            "{from:?}: refused"
        );
        assert_eq!(
            template.refusal(),
            Some(HelperAdmissionRefusal::Unavailable),
            "{from:?}: the last attempt's unavailability"
        );
        assert_eq!(
            template.attempts().len(),
            1 + usize::from(OWNER_CHANGE_FREE_REBUILDS),
            "RED-7 {from:?}: one attempt per free rebuild"
        );
        assert!(
            (pauses..pauses + Duration::from_millis(2)).contains(&waited),
            "{from:?}: a 1 ms pause before each retry: {waited:?}"
        );
    }
    owner.0.store(0, Ordering::SeqCst);
    assert_eq!(
        admissions.items().await,
        2,
        "one item per path whose owners answered again"
    );
}

//! The actual held report dispatch prepares a service task through the real
//! installed task source while an owner change of the work's own publication
//! is in progress exactly at one of the preparation's current-authorization
//! checks. Such a check refuses to rebuild: it ran nothing, so it waits out
//! the change within Core's bound and checks again. It never answers the
//! change as a denial, and it repeats only the check, never the preparation.
use super::*;
use crate::installed_host::acquisition::AcquisitionBootstrap;
use meerkat_authorization::publication::LocalAuthorizationPublication;
use meerkat_core::authorization::OWNER_CHANGE_WAIT_BOUND;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// An owner change held on a thread of its own, as a writer holds one. It is
/// published when `hold` elapses, or when released.
struct HeldChange {
    release: std::sync::mpsc::Sender<()>,
    thread: std::thread::JoinHandle<()>,
}
impl HeldChange {
    fn begin(publication: LocalAuthorizationPublication, hold: Option<Duration>) -> Self {
        let (began, started) = std::sync::mpsc::sync_channel(0);
        let (release, released) = std::sync::mpsc::channel::<()>();
        let thread = std::thread::spawn(move || {
            let change = publication.begin_owner_change().unwrap();
            began.send(()).unwrap();
            match hold {
                Some(hold) => {
                    let _ = released.recv_timeout(hold);
                }
                None => {
                    let _ = released.recv();
                }
            }
            drop(change);
        });
        started.recv().unwrap();
        Self { release, thread }
    }
    fn release(self) {
        let _ = self.release.send(());
        self.thread.join().unwrap();
    }
}

/// What the preparation reached: each stage's count, and the change begun at
/// the first arrival at the selected stage.
#[derive(Default)]
struct Reached {
    stages: Mutex<BTreeMap<&'static str, usize>>,
    change: Mutex<Option<HeldChange>>,
}
impl Reached {
    fn count(&self, stage: &str) -> usize {
        self.stages.lock().unwrap().get(stage).copied().unwrap_or(0)
    }
}

enum AtCheck {
    /// An owner change in progress, published after `hold` or never.
    OwnerChange(Option<Duration>),
    /// The original's operations really cancelled.
    Cancellation,
}

struct Prepared {
    result: Result<InstalledServiceTask, ServiceTaskError>,
    elapsed: Duration,
    remaining: Option<Duration>,
    reached: Arc<Reached>,
}

/// Prepare the actual software task once, acting at the first arrival at
/// `stage`, and return what the preparation answered after every owner
/// joined.
async fn prepare_with(stage: &'static str, act: AtCheck) -> Prepared {
    let script = acquisition_denial::Script::for_approved();
    let report = Report::new(script.tools().result_tool());
    let (fixture, _runtime_store) =
        Fixture::for_configured_helper_report(script.clone(), report.clone()).await;
    let reached = Arc::new(Reached::default());
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(
        WAIT,
        Box::pin(async {
            let first = fixture.submit().await;
            fixture.finish(&first).await.unwrap();
            let context = report.take_context().await;
            let remaining = context.remaining_tool_time();
            let owner = script.installed_owner();
            let mut bootstrap = AcquisitionBootstrap::selected(owner.clone());
            assert!(Arc::ptr_eq(
                &bootstrap.prepare(None, None).unwrap().unwrap(),
                &owner
            ));
            let bootstrap = Arc::new(Mutex::new(bootstrap));
            let tasks = InstalledServiceTaskSource::new(&bootstrap);
            let publication = fixture.source.owner.publication();
            let source = fixture.source.clone();
            let original = fixture.submission.original.clone();
            let probe = reached.clone();
            let act = Mutex::new(Some(act));
            let runtime = tokio::runtime::Handle::current();
            fixture
                .source
                .probe_service_task_stages_for_test(Box::new(move |arrived| {
                    *probe.stages.lock().unwrap().entry(arrived).or_default() += 1;
                    if arrived != stage {
                        return;
                    }
                    let selected = act.lock().unwrap().take();
                    match selected {
                        Some(AtCheck::OwnerChange(hold)) => {
                            let change = HeldChange::begin(publication.clone(), hold);
                            assert!(probe.change.lock().unwrap().replace(change).is_none());
                        }
                        Some(AtCheck::Cancellation) => {
                            // The existing source cancellation owner, run to
                            // completion on the test runtime before the check.
                            let (source, original, runtime) =
                                (source.clone(), original.clone(), runtime.clone());
                            std::thread::spawn(move || {
                                runtime.block_on(async move {
                                    source.record_operation_cancelled(&original).await.unwrap();
                                })
                            })
                            .join()
                            .unwrap();
                        }
                        None => {}
                    }
                }));
            let started = Instant::now();
            let result = report
                .prepare_task(&tasks, ReportPreparation::Software)
                .await;
            let elapsed = started.elapsed();
            if let Some(change) = reached.change.lock().unwrap().take() {
                change.release();
            }
            (result, elapsed, remaining)
        }),
    ))
    .catch_unwind()
    .await;
    if let Some(change) = reached.change.lock().unwrap().take() {
        change.release();
    }
    report.release();
    report.leaf.context.lock().unwrap().take();
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    match checked {
        Ok(Ok((result, elapsed, remaining))) => Prepared {
            result,
            elapsed,
            remaining,
            reached,
        },
        Ok(Err(_)) => panic!("service task preparation exceeded its bounded deadline"),
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

/// The dispatch's own tool time must leave Core's whole wait bound to the
/// check, so these rows observe that bound and not the dispatch deadline.
fn assert_tool_time_covers_the_wait(remaining: Option<Duration>) {
    assert!(
        remaining.is_none_or(|remaining| remaining > OWNER_CHANGE_WAIT_BOUND * 3),
        "the held dispatch must keep more tool time than the wait bound: {remaining:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settled_owner_change_at_the_task_check_is_checked_again_and_admitted() {
    let hold = Duration::from_millis(200);
    let prepared = prepare_with("prepare.authorization", AtCheck::OwnerChange(Some(hold))).await;
    assert_tool_time_covers_the_wait(prepared.remaining);
    let task = prepared
        .result
        .expect("a settled owner change is checked again and admitted");
    assert_eq!(task.consumer_member(), Some(MEMBER));
    assert!(
        prepared.elapsed >= hold - Duration::from_millis(50),
        "the check waited for the change: {:?}",
        prepared.elapsed
    );
    assert_eq!(prepared.reached.count("prepare.authorization"), 1);
    assert_eq!(prepared.reached.count("prepare.service_task"), 1);
    assert_eq!(prepared.reached.count("prepare.final_authorization"), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn persistent_owner_change_at_the_task_check_ends_typed_within_the_bound() {
    let prepared = prepare_with("prepare.authorization", AtCheck::OwnerChange(None)).await;
    assert_tool_time_covers_the_wait(prepared.remaining);
    assert!(
        matches!(prepared.result, Err(ServiceTaskError::ReprepareRequired)),
        "a change that outlasts the bound stays a typed refusal to rebuild, never a denial: {:?}",
        prepared.result.as_ref().err()
    );
    assert!(
        prepared.elapsed >= OWNER_CHANGE_WAIT_BOUND - Duration::from_millis(100)
            && prepared.elapsed < OWNER_CHANGE_WAIT_BOUND + Duration::from_secs(2),
        "the check waits out Core's bound once, never renewed: {:?}",
        prepared.elapsed
    );
    assert_eq!(prepared.reached.count("prepare.authorization"), 1);
    assert_eq!(
        prepared.reached.count("prepare.service_task"),
        0,
        "nothing was prepared"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_denial_at_the_task_check_stays_denied_without_waiting() {
    let prepared = prepare_with("prepare.authorization", AtCheck::Cancellation).await;
    assert!(
        matches!(
            prepared.result,
            Err(ServiceTaskError::Refused(PrivateOriginalError::Denied))
        ),
        "a cancelled original stays a denial: {:?}",
        prepared.result.as_ref().err()
    );
    assert!(
        prepared.elapsed < OWNER_CHANGE_WAIT_BOUND,
        "a denial is final at once: {:?}",
        prepared.elapsed
    );
    assert_eq!(prepared.reached.count("prepare.service_task"), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settled_owner_change_at_the_final_check_keeps_one_preparation() {
    let hold = Duration::from_millis(200);
    let prepared = prepare_with(
        "prepare.final_authorization",
        AtCheck::OwnerChange(Some(hold)),
    )
    .await;
    assert_tool_time_covers_the_wait(prepared.remaining);
    let task = prepared
        .result
        .expect("a settled owner change at the final check is checked again and admitted");
    assert_eq!(task.consumer_member(), Some(MEMBER));
    assert!(prepared.elapsed >= hold - Duration::from_millis(50));
    assert_eq!(
        prepared.reached.count("prepare.service_task"),
        1,
        "the wait repeats the check only, never the preparation"
    );
    assert_eq!(prepared.reached.count("prepare.final_authorization"), 1);
}

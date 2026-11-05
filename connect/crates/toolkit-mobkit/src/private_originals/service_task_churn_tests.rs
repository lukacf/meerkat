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
/// the first arrival at the selected stage, with the record change made
/// behind it.
#[derive(Default)]
struct Reached {
    stages: Mutex<BTreeMap<&'static str, usize>>,
    change: Mutex<Option<HeldChange>>,
    record_change: Mutex<Option<tokio::task::JoinHandle<toolkit_store::StoreResult<()>>>>,
    record_pause: Mutex<Option<Arc<RecordChangePause>>>,
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
    /// A record change of the owner's store holding the store's one
    /// connection, and an owner change in progress, both for `hold`: the
    /// record change then waits for the owner change, so every other
    /// operation of the store waits as long.
    RecordChangeBehind(Duration),
}

/// The owner's next record change, paused once it holds the store's one
/// connection, just before it begins its owner change: it signals that it
/// entered, then waits until released or until `hold` elapsed.
struct RecordChangePause {
    entered: Mutex<Option<std::sync::mpsc::SyncSender<()>>>,
    released: (Mutex<bool>, std::sync::Condvar),
    hold: Duration,
}
impl RecordChangePause {
    fn release(&self) {
        *self.released.0.lock().unwrap() = true;
        self.released.1.notify_all();
    }
}
struct RecordChangePauseGuard;
impl toolkit_store::RecordChangeGuard for RecordChangePauseGuard {}
impl toolkit_store::RecordChangePublication for RecordChangePause {
    fn begin_record_change(
        &self,
    ) -> toolkit_store::StoreResult<Box<dyn toolkit_store::RecordChangeGuard + '_>> {
        if let Some(entered) = self.entered.lock().unwrap().take() {
            let _ = entered.send(());
        }
        let (released, ready) = &self.released;
        let released = released.lock().unwrap();
        drop(
            ready
                .wait_timeout_while(released, self.hold, |released| !*released)
                .unwrap(),
        );
        Ok(Box::new(RecordChangePauseGuard))
    }
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
            // A row of the owner's store for the record change made behind
            // the owner change, written before the preparation starts.
            let record_row = if matches!(act, AtCheck::RecordChangeBehind(_)) {
                let row = fixture
                    .store
                    .admit(toolkit_store::AdmissionRequest {
                        key: toolkit_store::StoreKey {
                            realm: fixture.scope.realm.clone(),
                            owner: "toolkit/test-record-change".into(),
                            id: "behind-owner-change".into(),
                        },
                        admission_key: "test-record-change-behind-owner-change".into(),
                        data: toolkit_store::RecordData {
                            schema: "toolkit.test.record_change".into(),
                            version: 1,
                            body: serde_json::json!({}),
                        },
                        events: vec![],
                        effects: vec![],
                    })
                    .await
                    .unwrap()
                    .record()
                    .clone();
                Some((fixture.store.clone(), row))
            } else {
                None
            };
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
                        Some(AtCheck::RecordChangeBehind(hold)) => {
                            // The pause first: the next record change takes
                            // the store's one connection and pauses there for
                            // the hold; then the held owner change begins.
                            let (entered, entering) = std::sync::mpsc::sync_channel(1);
                            let pause = Arc::new(RecordChangePause {
                                entered: Mutex::new(Some(entered)),
                                released: (Mutex::new(false), std::sync::Condvar::new()),
                                hold,
                            });
                            source
                                .owner
                                .observe_next_record_change_for_test(pause.clone());
                            let (store, row) = record_row.clone().expect("the record's row");
                            // A blocking thread, not a task: this hook blocks
                            // its worker below, and a task spawned from that
                            // worker would wait in its LIFO slot, which no
                            // other worker steals.
                            let writer = runtime.clone();
                            let writing = runtime.spawn_blocking(move || {
                                writer.block_on(async move {
                                    store
                                        .compare_and_swap(toolkit_store::Transition {
                                            key: row.key.clone(),
                                            expected_revision: row.revision,
                                            expected_digest: Some(row.content_digest.clone()),
                                            claim: None,
                                            data: row.data.clone(),
                                            events: vec![],
                                            effects: vec![],
                                        })
                                        .await
                                        .map(|_| ())
                                })
                            });
                            assert!(
                                entering.recv_timeout(Duration::from_secs(10)).is_ok(),
                                "a record change holds the store's connection"
                            );
                            let change = HeldChange::begin(publication.clone(), Some(hold));
                            assert!(probe.change.lock().unwrap().replace(change).is_none());
                            assert!(
                                probe
                                    .record_change
                                    .lock()
                                    .unwrap()
                                    .replace(writing)
                                    .is_none()
                            );
                            assert!(probe.record_pause.lock().unwrap().replace(pause).is_none());
                        }
                        None => {}
                    }
                }));
            let started = Instant::now();
            let result = report
                .prepare_task(&tasks, ReportPreparation::Software)
                .await;
            let elapsed = started.elapsed();
            if let Some(pause) = reached.record_pause.lock().unwrap().take() {
                pause.release();
            }
            if let Some(change) = reached.change.lock().unwrap().take() {
                change.release();
            }
            let record_change = reached.record_change.lock().unwrap().take();
            if let Some(writing) = record_change {
                writing.await.unwrap().unwrap();
            }
            (result, elapsed, remaining)
        }),
    ))
    .catch_unwind()
    .await;
    if let Some(pause) = reached.record_pause.lock().unwrap().take() {
        pause.release();
    }
    if let Some(change) = reached.change.lock().unwrap().take() {
        change.release();
    }
    let record_change = reached.record_change.lock().unwrap().take();
    if let Some(writing) = record_change {
        let _ = writing.await;
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
        matches!(prepared.result, Err(ServiceTaskError::ReprepareRequired(_))),
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

// A selected install's outlasting await, named by the round 20 prints: the
// preparation's snapshot reads the owner's store, whose one connection a
// record change holds while it waits for an owner change of the shared
// publication held past Core's bound. The preparation's one deadline, from
// its start, ends that wait: it answers ReprepareRequired at the bound,
// typed, and nothing past the snapshot is made.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_change_held_behind_a_record_change_at_the_snapshot_ends_typed_within_the_bound() {
    let prepared = prepare_with(
        "prepare.snapshot",
        AtCheck::RecordChangeBehind(OWNER_CHANGE_WAIT_BOUND * 2),
    )
    .await;
    assert_tool_time_covers_the_wait(prepared.remaining);
    assert!(
        matches!(prepared.result, Err(ServiceTaskError::ReprepareRequired(_))),
        "a change that outlasts the preparation's deadline stays a typed refusal to rebuild: {:?}",
        prepared.result.as_ref().err()
    );
    assert!(
        prepared.elapsed >= OWNER_CHANGE_WAIT_BOUND - Duration::from_millis(100)
            && prepared.elapsed < OWNER_CHANGE_WAIT_BOUND + Duration::from_secs(1),
        "the preparation waits until its one deadline, not for the whole hold: {:?}",
        prepared.elapsed
    );
    assert_eq!(prepared.reached.count("prepare.snapshot"), 1);
    assert_eq!(
        prepared.reached.count("prepare.authorization"),
        0,
        "nothing past the snapshot was made"
    );
    assert_eq!(prepared.reached.count("prepare.service_task"), 0);
}

// The same record change behind an owner change that ends within the bound:
// the snapshot's reads wait it out and the preparation completes once, as
// before the deadline (an invariant row, the same before and after it).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_change_settled_behind_a_record_change_at_the_snapshot_is_waited_out() {
    let hold = Duration::from_millis(300);
    let prepared = prepare_with("prepare.snapshot", AtCheck::RecordChangeBehind(hold)).await;
    assert_tool_time_covers_the_wait(prepared.remaining);
    let task = prepared
        .result
        .expect("a record change behind a settled change is waited out");
    assert_eq!(task.consumer_member(), Some(MEMBER));
    assert!(
        prepared.elapsed >= hold - Duration::from_millis(50),
        "the snapshot waited for the record change: {:?}",
        prepared.elapsed
    );
    for stage in [
        "prepare.snapshot",
        "prepare.authorization",
        "prepare.service_task",
        "prepare.final_authorization",
    ] {
        assert_eq!(prepared.reached.count(stage), 1, "{stage}");
    }
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

/// What the task source's check after a shared effect answered on the actual
/// held report dispatch, and how long it took.
struct CheckedAfterEffect {
    result: Result<(), meerkat_core::ToolError>,
    elapsed: Duration,
    remaining: Option<Duration>,
}

/// Run the check after a shared effect once on the actual held report
/// dispatch, acting just before it: an owner change held open on the work's
/// own publication, or the original's operations really cancelled.
async fn check_after_effect_with(act: AtCheck) -> CheckedAfterEffect {
    let script = acquisition_denial::Script::for_approved();
    let report = Report::new(script.tools().result_tool());
    let (fixture, _runtime_store) =
        Fixture::for_configured_helper_report(script.clone(), report.clone()).await;
    let held = Mutex::new(None::<HeldChange>);
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
            match act {
                AtCheck::OwnerChange(hold) => {
                    let change = HeldChange::begin(fixture.source.owner.publication(), hold);
                    *held.lock().unwrap() = Some(change);
                }
                AtCheck::Cancellation => {
                    fixture
                        .source
                        .record_operation_cancelled(&fixture.submission.original)
                        .await
                        .unwrap();
                }
                AtCheck::RecordChangeBehind(_) => {
                    unreachable!(
                        "a record change behind an owner change acts at a preparation stage"
                    )
                }
            }
            let started = Instant::now();
            let result = report.check_after_shared_effect(&tasks).await;
            let elapsed = started.elapsed();
            if let Some(change) = held.lock().unwrap().take() {
                change.release();
            }
            (result, elapsed, remaining)
        }),
    ))
    .catch_unwind()
    .await;
    if let Some(change) = held.lock().unwrap().take() {
        change.release();
    }
    report.release();
    report.leaf.context.lock().unwrap().take();
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    match checked {
        Ok(Ok((result, elapsed, remaining))) => CheckedAfterEffect {
            result,
            elapsed,
            remaining,
        },
        Ok(Err(_)) => panic!("the check after a shared effect exceeded its bounded deadline"),
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

/// The model payload of a refused check: its kind and whether it claims the
/// tool ran.
fn refusal_data(error: &meerkat_core::ToolError) -> serde_json::Value {
    let rendered =
        meerkat_core::ops::terminal_tool_outcome_for_error("shared-check", error.clone()).result;
    serde_json::from_str::<serde_json::Value>(&rendered.text_content()).unwrap()["data"].clone()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settled_owner_change_after_a_shared_effect_is_checked_again() {
    let hold = Duration::from_millis(200);
    let checked = check_after_effect_with(AtCheck::OwnerChange(Some(hold))).await;
    assert_tool_time_covers_the_wait(checked.remaining);
    assert!(
        checked.result.is_ok(),
        "a settled owner change is waited out and the check answers current: {:?}",
        checked.result.as_ref().err().map(refusal_data)
    );
    assert!(
        checked.elapsed >= hold - Duration::from_millis(50),
        "the check waited for the change: {:?}",
        checked.elapsed
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn persistent_owner_change_after_a_shared_effect_ends_typed_and_uncertified() {
    let checked = check_after_effect_with(AtCheck::OwnerChange(None)).await;
    assert_tool_time_covers_the_wait(checked.remaining);
    let error = checked
        .result
        .expect_err("a change that outlasts the bound is refused");
    assert_eq!(
        refusal_data(&error),
        serde_json::json!({"kind": "reprepare_required", "tool_invoked": "unknown"}),
        "after its effect the refusal stays typed and claims nothing about dispatch"
    );
    assert!(
        checked.elapsed >= OWNER_CHANGE_WAIT_BOUND - Duration::from_millis(100)
            && checked.elapsed < OWNER_CHANGE_WAIT_BOUND + Duration::from_secs(2),
        "the check waits out Core's bound once, never renewed: {:?}",
        checked.elapsed
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_denial_after_a_shared_effect_stays_immediate() {
    let checked = check_after_effect_with(AtCheck::Cancellation).await;
    let error = checked
        .result
        .expect_err("a cancelled original's call is refused");
    assert_ne!(
        refusal_data(&error)["kind"],
        "reprepare_required",
        "a denial is never a refusal to rebuild"
    );
    assert!(
        checked.elapsed < OWNER_CHANGE_WAIT_BOUND,
        "a denial is final at once: {:?}",
        checked.elapsed
    );
}

/// What the task source's reservation of the held report call's shared
/// entry answered, and how long it took.
struct Reserved {
    result: Result<(), ServiceTaskError>,
    elapsed: Duration,
    remaining: Option<Duration>,
    reached: Arc<Reached>,
}

/// Reserve the held report call's shared entry once through the actual task
/// source, beginning an owner change of the work's own publication at the
/// first arrival at the reservation's current-authorization check, published
/// after `hold` or never. A reserved entry is settled unused.
async fn reserve_with(hold: Option<Duration>) -> Reserved {
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
            let probe = reached.clone();
            let act = Mutex::new(Some(hold));
            fixture
                .source
                .probe_service_task_stages_for_test(Box::new(move |arrived| {
                    *probe.stages.lock().unwrap().entry(arrived).or_default() += 1;
                    if arrived != "reserve.authorization" {
                        return;
                    }
                    if let Some(hold) = act.lock().unwrap().take() {
                        let change = HeldChange::begin(publication.clone(), hold);
                        assert!(probe.change.lock().unwrap().replace(change).is_none());
                    }
                }));
            let started = Instant::now();
            let result = report.reserve_shared_entry(&tasks).await;
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
        Ok(Ok((result, elapsed, remaining))) => Reserved {
            result,
            elapsed,
            remaining,
            reached,
        },
        Ok(Err(_)) => panic!("the shared entry reservation exceeded its bounded deadline"),
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

// The reservation is source custody a cancellation drains, so its check
// never waits while holding it: a refusal to rebuild settles the unused
// reservation, waits out the change holding nothing, and reserves again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settled_owner_change_at_the_reservation_check_is_reserved_again_after_the_wait() {
    let hold = Duration::from_millis(200);
    let reserved = reserve_with(Some(hold)).await;
    assert_tool_time_covers_the_wait(reserved.remaining);
    assert!(
        reserved.result.is_ok(),
        "a settled owner change at the reservation's check is waited out: {:?}",
        reserved.result
    );
    assert!(
        reserved.elapsed >= hold - Duration::from_millis(50),
        "the reservation waited for the change: {:?}",
        reserved.elapsed
    );
    assert_eq!(
        reserved.reached.count("reserve.authorization"),
        2,
        "the refused reservation was settled and the entry reserved again once"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn persistent_owner_change_at_the_reservation_check_ends_typed_within_the_bound() {
    let reserved = reserve_with(None).await;
    assert_tool_time_covers_the_wait(reserved.remaining);
    assert!(
        matches!(
            reserved.result,
            Err(ServiceTaskError::ReprepareRequired { .. })
        ),
        "a change that outlasts the bound stays a typed refusal to rebuild: {:?}",
        reserved.result
    );
    assert!(
        reserved.elapsed >= OWNER_CHANGE_WAIT_BOUND - Duration::from_millis(100)
            && reserved.elapsed < OWNER_CHANGE_WAIT_BOUND + Duration::from_secs(2),
        "the reservation waits out Core's bound once, never renewed: {:?}",
        reserved.elapsed
    );
    assert_eq!(
        reserved.reached.count("reserve.authorization"),
        1,
        "no reservation after the bound"
    );
    let Err(ServiceTaskError::ReprepareRequired(refusal)) = reserved.result else {
        unreachable!()
    };
    assert_eq!(
        refusal.dispatch(),
        meerkat_core::authorization::RefusalDispatch::NotDispatched,
        "the reservation's check ran before anything was sent: certified not dispatched"
    );
}

/// What the held report call answered once its dispatch resumed, with an
/// owner change of the work's own publication begun at the report's first
/// current-authorization check, and how long the helper's turn took from the
/// resume.
struct Reported {
    feedback: Vec<ToolResult>,
    elapsed: Duration,
    remaining: Option<Duration>,
}

async fn report_with(hold: Option<Duration>) -> Reported {
    let script = acquisition_denial::Script::for_approved();
    let report = Report::new(script.tools().result_tool());
    let (fixture, _runtime_store) =
        Fixture::for_configured_helper_report(script.clone(), report.clone()).await;
    let held = Arc::new(Mutex::new(None::<HeldChange>));
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(
        WAIT,
        Box::pin(async {
            let first = fixture.submit().await;
            fixture.finish(&first).await.unwrap();
            let context = report.take_context().await;
            let remaining = context.remaining_tool_time();
            drop(context);
            let helper =
                AgentIdentity::parse(crate::installed_host::controller_first_input_fixture::HELPER)
                    .unwrap();
            let helper_before = fixture.runtime.completion_cursor(&helper).await;
            let publication = fixture.source.owner.publication();
            let probe = held.clone();
            let act = Mutex::new(Some(hold));
            fixture
                .source
                .probe_service_task_stages_for_test(Box::new(move |arrived| {
                    if arrived != "report.authorization" {
                        return;
                    }
                    if let Some(hold) = act.lock().unwrap().take() {
                        let change = HeldChange::begin(publication.clone(), hold);
                        assert!(probe.lock().unwrap().replace(change).is_none());
                    }
                }));
            let started = Instant::now();
            report.release();
            fixture
                .runtime
                .wait_for_completion(&helper, helper_before, WAIT)
                .await
                .unwrap();
            let elapsed = started.elapsed();
            if let Some(change) = held.lock().unwrap().take() {
                change.release();
            }
            let feedback = report.feedback.lock().unwrap().clone();
            (feedback, elapsed, remaining)
        }),
    ))
    .catch_unwind()
    .await;
    if let Some(change) = held.lock().unwrap().take() {
        change.release();
    }
    report.release();
    report.leaf.context.lock().unwrap().take();
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    match checked {
        Ok(Ok((feedback, elapsed, remaining))) => Reported {
            feedback,
            elapsed,
            remaining,
        },
        Ok(Err(_)) => panic!("the helper's report exceeded its bounded deadline"),
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

// The report's checks hold nothing, so each waits out an owner change in
// progress within the report's one waiting state, and the report goes on.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settled_owner_change_at_the_report_check_is_waited_out_and_reported() {
    let hold = Duration::from_millis(200);
    let reported = report_with(Some(hold)).await;
    assert_tool_time_covers_the_wait(reported.remaining);
    assert_one_report_result(&reported.feedback);
    assert!(
        !reported.feedback[0].is_error,
        "a settled owner change at the report's check is waited out, not refused: {:?}",
        reported.feedback
    );
    let reply: serde_json::Value =
        serde_json::from_str(&reported.feedback[0].text_content()).unwrap();
    assert_eq!(reply["phase"], "unavailable", "{reply}");
    assert!(
        reported.elapsed >= hold - Duration::from_millis(50),
        "the report waited for the change: {:?}",
        reported.elapsed
    );
}

/// Every helper model request after the report carries its one result.
fn assert_one_report_result(feedback: &[ToolResult]) {
    assert!(
        !feedback.is_empty() && feedback.iter().all(|seen| seen == &feedback[0]),
        "the report's one result reached the helper: {feedback:?}"
    );
}

// A change held past Core's bound at the report's first check: the check
// waits out the bound and refuses typed. Nothing of the report ran yet, so the
// refusal is certified not dispatched.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn persistent_owner_change_at_the_report_check_is_refused_certified_after_the_bound() {
    let reported = report_with(Some(OWNER_CHANGE_WAIT_BOUND + Duration::from_millis(500))).await;
    assert_tool_time_covers_the_wait(reported.remaining);
    assert_one_report_result(&reported.feedback);
    assert!(reported.feedback[0].is_error, "{:?}", reported.feedback);
    let refusal: serde_json::Value =
        serde_json::from_str(&reported.feedback[0].text_content()).unwrap();
    assert_eq!(
        refusal["data"],
        serde_json::json!({"kind": "reprepare_required", "tool_invoked": false}),
        "refused typed after the bound, before anything of the report ran: {refusal}"
    );
    assert!(
        reported.elapsed >= OWNER_CHANGE_WAIT_BOUND - Duration::from_millis(100),
        "the report's check waited out its bound: {:?}",
        reported.elapsed
    );
}

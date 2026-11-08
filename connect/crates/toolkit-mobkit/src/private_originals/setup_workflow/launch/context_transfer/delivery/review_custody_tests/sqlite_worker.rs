//! Real workflow I/O with a consumed Core review. The blocked write is the
//! selected waiter's actual departure; it is not a synthetic Find commit.
//! The first control cancels a foreground read before any Meta/delivery task
//! can retain the review. The second covers the existing history-only task.
use super::*;
use std::future::{Future, poll_fn};
use toolkit_store::{RecordChangeGuard, RecordChangePublication, StoreResult};

struct PublicationPause {
    entered: tokio::sync::Notify,
    release: (Mutex<bool>, std::sync::Condvar),
    active: AtomicBool,
}
impl PublicationPause {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            entered: tokio::sync::Notify::new(),
            release: (Mutex::new(false), std::sync::Condvar::new()),
            active: AtomicBool::new(false),
        })
    }
}
struct PausedPublication<'a>(&'a PublicationPause);
impl RecordChangeGuard for PausedPublication<'_> {}
impl Drop for PausedPublication<'_> {
    fn drop(&mut self) {
        self.0.active.store(false, Ordering::SeqCst);
    }
}
impl RecordChangePublication for PublicationPause {
    fn begin_record_change(&self) -> StoreResult<Box<dyn RecordChangeGuard + '_>> {
        assert!(!self.active.swap(true, Ordering::SeqCst));
        self.entered.notify_one();
        let (lock, ready) = &self.release;
        let mut released = lock.lock().unwrap();
        while !*released {
            released = ready.wait(released).unwrap();
        }
        Ok(Box::new(PausedPublication(self)))
    }
}
struct ReleaseWriter(Arc<PublicationPause>);
impl Drop for ReleaseWriter {
    fn drop(&mut self) {
        *self.0.release.0.lock().unwrap() = true;
        self.0.release.1.notify_all();
    }
}

// Observe the actual future's first poll, not a delay before calling it. For
// launch_with_review the first await is the selected SqliteStore get_effect:
// with() captures custody and submits its blocking closure in that same poll.
fn observed_caller(
    future: impl Future<Output = ()> + Send + 'static,
) -> (
    tokio::task::JoinHandle<()>,
    tokio::sync::oneshot::Receiver<bool>,
) {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        tokio::pin!(future);
        let mut sender = Some(sender);
        poll_fn(|cx| {
            let result = future.as_mut().poll(cx);
            if let Some(sender) = sender.take() {
                let _ = sender.send(result.is_pending());
            }
            result
        })
        .await
    });
    (task, receiver)
}

type Departure = tokio::task::JoinHandle<Result<SetupWorkflowSnapshot, PrivateOriginalError>>;

async fn close_after_test(
    c: &Fixture,
    lifecycle: &AcquisitionRecovery,
    release: Option<ReleaseWriter>,
    caller: Option<tokio::task::JoinHandle<()>>,
    departure: Option<Departure>,
    body: Result<(), Box<dyn std::any::Any + Send>>,
) {
    // Release before any failed-assertion cleanup. No JoinHandle is detached,
    // and the real departure is never aborted to manufacture an outcome.
    drop(release);
    let caller_closed = match caller {
        Some(task) => {
            task.abort();
            task.await.is_err_and(|error| error.is_cancelled())
        }
        None => true,
    };
    let writer_closed = match departure {
        Some(task) => matches!(task.await, Ok(Ok(_))),
        None => true,
    };
    lifecycle.stop();
    let transfers = c.f.source.join_workflow_context_transfers(false).await;
    let native = c.close().await;
    assert!(
        caller_closed && writer_closed && transfers.is_ok() && native.is_ok(),
        "caller, actual writer, retained workflow tasks and native owner must all join"
    );
    if let Err(panic) = body {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelled_workflow_launch_read_keeps_review_until_actual_sqlite_worker_finishes() {
    let c = Fixture::persistent_configured_waiter().await;
    let lifecycle = AcquisitionRecovery::default();
    let pause = PublicationPause::new();
    let mut release = Some(ReleaseWriter(pause.clone()));
    let mut departure = None;
    let mut caller = None;
    let body = AssertUnwindSafe(async {
        let data = fixture::prepare(&c).await;
        let snapshot =
            c.f.source
                .workflow_store()
                .unwrap()
                .load_setup_workflow(data.envelope.membership.workflow())
                .await
                .unwrap();
        let before_revision = snapshot.revision();
        let launch_key = snapshot.reference().launch_effect();
        let launch_before = c.f.store.get_effect(&launch_key).await.unwrap().unwrap();
        assert_eq!(launch_before.status, EffectStatus::Succeeded);
        let native_before = fixture::native_rows(&c).await;
        let requests_before = c.f.model_requests();
        assert!(data.planning.context_transfers.lock().unwrap().is_empty());

        c.f.source
            .owner
            .observe_next_record_change_for_test(pause.clone());
        let writer = data.planning.clone();
        let membership = data.envelope.membership.clone();
        departure = Some(tokio::spawn(async move {
            writer.depart_waiter(&membership).await
        }));
        tokio::time::timeout(WAIT, pause.entered.notified())
            .await
            .unwrap();
        assert!(pause.active.load(Ordering::SeqCst));
        // The pause is inside the real validated SQLite transaction, before
        // native publication. It cannot make an unavailable grant look current.
        c.f.source.owner.publication().observe(|| ()).unwrap();
        data.planning
            .check_for_binding(&c.f.source, &c.lead.original)
            .unwrap();

        let (review, observations) = consumed("cancelled-workflow-sqlite-read").await;
        let planning = data.planning.clone();
        let (task, first_poll) = observed_caller(async move {
            let _ = planning.launch_with_review(&snapshot, review).await;
        });
        caller = Some(task);
        assert!(
            tokio::time::timeout(WAIT, first_poll)
                .await
                .unwrap()
                .unwrap(),
            "the real get_effect worker must be submitted and blocked at the first await"
        );
        let task = caller.take().unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(pause.active.load(Ordering::SeqCst));
        assert!(
            data.planning.context_transfers.lock().unwrap().is_empty(),
            "no transfer task, delivery slot or Meta binder can mask worker custody"
        );
        assert_eq!(
            observations.used_count(),
            0,
            "the cancelled foreground is gone; the entered SQLite read still owns the review"
        );

        drop(release.take());
        let written = departure.take().unwrap().await.unwrap().unwrap();
        assert_eq!(
            written.revision(),
            before_revision + 1,
            "one actual selected departure CAS"
        );
        assert!(written.departed(&data.envelope.membership).unwrap());
        assert!(!written.departed(&data.sibling).unwrap());
        observations
            .exactly_used("cancelled-workflow-sqlite-read")
            .await;
        assert!(!pause.active.load(Ordering::SeqCst));
        assert_eq!(
            c.f.store.get_effect(&launch_key).await.unwrap().unwrap(),
            launch_before,
            "cancelling the read never claims, acknowledges or relaunches G"
        );
        assert_eq!(fixture::native_rows(&c).await, native_before);
        assert_eq!(c.f.model_requests(), requests_before);
    })
    .catch_unwind()
    .await;
    close_after_test(&c, &lifecycle, release, caller, departure, body).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn historical_delivery_task_keeps_review_after_caller_cancel_until_actual_join() {
    let c = Fixture::persistent_configured_waiter().await;
    let lifecycle = AcquisitionRecovery::default();
    let pause = PublicationPause::new();
    let mut release = Some(ReleaseWriter(pause.clone()));
    let mut departure = None;
    let mut caller = None;
    let body = AssertUnwindSafe(async {
        let data = fixture::prepare(&c).await;
        fixture::store_envelope(&data).await;
        let WorkflowContextObservation::Committed(context) =
            fixture::restore(&c, &data, &lifecycle).await.unwrap()
        else {
            panic!("actual committed envelope observation")
        };
        let attempt = context.attempt.clone();
        assert!(!attempt.locally_entered());
        assert!(attempt.advance.lock().unwrap().is_none());
        assert!(attempt.delivery.lock().unwrap().is_none());
        let effect_key = data.envelope.membership.delivery_effect().unwrap();
        let effect_before = c.f.store.get_effect(&effect_key).await.unwrap().unwrap();
        assert_eq!(effect_before.attempt, 0);
        let before_revision =
            c.f.source
                .workflow_store()
                .unwrap()
                .load_setup_workflow(data.envelope.membership.workflow())
                .await
                .unwrap()
                .revision();
        let native_before = fixture::native_rows(&c).await;

        c.f.source
            .owner
            .observe_next_record_change_for_test(pause.clone());
        let writer = data.planning.clone();
        let membership = data.envelope.membership.clone();
        departure = Some(tokio::spawn(async move {
            writer.depart_waiter(&membership).await
        }));
        tokio::time::timeout(WAIT, pause.entered.notified())
            .await
            .unwrap();
        let (review, observations) = consumed("cancelled-historical-delivery-reader").await;
        let (task, first_poll) = observed_caller(async move {
            let _ = context.deliver_with_review(review).await;
        });
        caller = Some(task);
        assert!(
            tokio::time::timeout(WAIT, first_poll)
                .await
                .unwrap()
                .unwrap()
        );
        let run = attempt
            .advance
            .lock()
            .unwrap()
            .clone()
            .expect("actual registered historical task");
        let task = caller.take().unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(!run.finished());
        assert!(
            attempt.delivery.lock().unwrap().is_none(),
            "history never invents native delivery custody"
        );
        assert_eq!(
            observations.used_count(),
            0,
            "the existing registered task owns the exact review after its caller is cancelled"
        );

        drop(release.take());
        let written = departure.take().unwrap().await.unwrap().unwrap();
        assert_eq!(
            written.revision(),
            before_revision + 1,
            "one actual selected departure CAS"
        );
        assert!(written.departed(&data.envelope.membership).unwrap());
        assert_eq!(
            run.join().await.unwrap(),
            WorkflowDeliveryObservation::Pending
        );
        observations
            .exactly_used("cancelled-historical-delivery-reader")
            .await;
        assert!(run.joined.load(Ordering::Acquire));
        assert!(run.native.lock().unwrap().is_none());
        assert_eq!(
            c.f.store.get_effect(&effect_key).await.unwrap().unwrap(),
            effect_before
        );
        assert_eq!(fixture::native_rows(&c).await, native_before);
    })
    .catch_unwind()
    .await;
    close_after_test(&c, &lifecycle, release, caller, departure, body).await;
}

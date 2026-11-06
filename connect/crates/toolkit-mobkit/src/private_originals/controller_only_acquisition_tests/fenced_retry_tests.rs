//! Real registered Find, entered native fences, and SQLite transactions.
//! The explicit writer holder is a control, not the retained failure's actor.
use super::*;
use meerkat::governed_realm::helper_admission::{
    GovernedHelperAdmissionFence, HelperAdmissionRefusal,
};
use meerkat_workgraph::{WorkAdmission, WorkGraphEventFilter, WorkGraphEventKind};

#[derive(Clone, Copy)]
enum Mode {
    ReleasedBeforeWait,
    HeldPastBudget,
    Poisoned,
    ExpiredCallback,
}
struct Holder {
    release: std::sync::mpsc::Sender<()>,
    join: Option<std::thread::JoinHandle<bool>>,
}
impl Drop for Holder {
    fn drop(&mut self) {
        let _ = self.release.send(());
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}
type FenceObservation = (
    std::sync::Weak<GovernedHelperAdmissionFence>,
    Option<HelperAdmissionRefusal>,
    bool,
);
struct Probe {
    source: Arc<PrivateOriginals>,
    service: WorkGraphService,
    mode: Mode,
    holder: Mutex<Option<Holder>>,
    attempts: Mutex<Vec<Arc<OwnedTransferCommit>>>,
    observations: Arc<Mutex<Vec<FenceObservation>>>,
    result: Mutex<Option<Result<WorkAdmission, PrivateOriginalError>>>,
    request: Mutex<Option<CreateWorkItemRequest>>,
    elapsed: Mutex<Option<Duration>>,
    done: tokio::sync::Notify,
    calls: AtomicUsize,
}
#[async_trait]
impl FindDispatchProbe for Probe {
    async fn inspect(&self, call: ToolCallView<'_>, context: &ToolDispatchContext) {
        assert_eq!(self.calls.fetch_add(1, Ordering::SeqCst), 0);
        let entered = context.enter_governed_operation(call).unwrap();
        let request = CommitProbe::request(call, context);
        *self.request.lock().unwrap() = Some(request.clone());
        let mut release_after_refusal = None;
        if matches!(self.mode, Mode::ReleasedBeforeWait | Mode::HeldPastBudget) {
            let owner = self.source.owner.clone();
            let (ready_tx, ready_rx) = std::sync::mpsc::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            let (released_tx, released_rx) = std::sync::mpsc::channel();
            let join = std::thread::spawn(move || {
                // Public owner API holds an unpublished writer exclusion.
                let result = owner.grants().try_with_owner_changes_excluded(|| {
                    ready_tx.send(()).unwrap();
                    release_rx.recv_timeout(WAIT).unwrap();
                });
                let _ = released_tx.send(());
                result.is_ok()
            });
            *self.holder.lock().unwrap() = Some(Holder {
                release: release_tx.clone(),
                join: Some(join),
            });
            ready_rx.recv_timeout(WAIT).unwrap();
            if matches!(self.mode, Mode::ReleasedBeforeWait) {
                release_after_refusal = Some((release_tx, Mutex::new(released_rx)));
            }
        } else if matches!(self.mode, Mode::Poisoned) {
            self.source.owner.publication().poison();
        }
        let observations = self.observations.clone();
        let observer = Arc::new(move |fence, refusal, ok| {
            let first = {
                let mut rows = observations.lock().unwrap();
                rows.push((fence, refusal, ok));
                rows.len() == 1
            };
            if first && let Some((release, released)) = &release_after_refusal {
                // The actual failed create has returned before this hook.
                // Wait for the real guard drop to exercise a completed hold.
                release.send(()).unwrap();
                released.lock().unwrap().recv_timeout(WAIT).unwrap();
            }
        });
        let now = self.source.owner.clock().now().unwrap().unix_ms;
        let budget_ms = match self.mode {
            Mode::HeldPastBudget => 25,
            Mode::ExpiredCallback => 0,
            _ => 60_000,
        };
        let started = std::time::Instant::now();
        let commit = Arc::new(
            OwnedTransferCommit::start_entered_probe(
                call,
                &entered,
                context,
                self.source.owner.clone(),
                self.service.clone(),
                WorkAdmissionKey::new("entered-contention").unwrap(),
                request,
                now + budget_ms,
                observer,
            )
            .unwrap(),
        );
        self.attempts.lock().unwrap().push(commit.clone());
        let result = commit.join().await;
        *self.elapsed.lock().unwrap() = Some(started.elapsed());
        assert_eq!(
            commit.join().await,
            result,
            "cached join never repeats the admission"
        );
        *self.result.lock().unwrap() = Some(result);
        self.holder.lock().unwrap().take(); // release and join any held writer before further native work
        self.done.notify_one();
    }
}

async fn exercise(mode: Mode) {
    let script = Script::paused_for_helper_entry();
    let (fixture, _history) = FirstFixture::<meerkat::PersistentSessionService<FactoryAgentBuilder>>::
        for_approved_acquisition(script.clone()).await;
    let service = WorkGraphService::with_scope(
        Arc::new(
            SqliteWorkGraphStore::open(
                fixture
                    .source_path
                    .with_extension("entered-contention.sqlite"),
            )
            .unwrap(),
        ),
        fixture.scope.realm.clone(),
        WorkNamespace::new("entered-contention-control").unwrap(),
    );
    let probe = Arc::new(Probe {
        source: fixture.source.clone(),
        service,
        mode,
        holder: Mutex::new(None),
        attempts: Mutex::new(Vec::new()),
        observations: Arc::new(Mutex::new(Vec::new())),
        result: Mutex::new(None),
        request: Mutex::new(None),
        elapsed: Mutex::new(None),
        done: tokio::sync::Notify::new(),
        calls: AtomicUsize::new(0),
    });
    assert!(
        script
            .dispatch
            .probe
            .lock()
            .unwrap()
            .replace(probe.clone())
            .is_none()
    );
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(WAIT, async {
        acquisition_approved::select_reviewed_setup(&fixture).await;
        let submitted = fixture.submit().await;
        assert!(matches!(
            &submitted,
            Ok(PrivateOriginalSubmission::Admitted { .. })
        ));
        script.first_response_waiting().await;
        script.release_first_response();
        probe.done.notified().await;
        let rows = probe.observations.lock().unwrap().clone();
        let result = probe.result.lock().unwrap().clone().unwrap();
        let request = probe.request.lock().unwrap().clone().unwrap();
        let admission = probe
            .service
            .observe_admission(
                WorkAdmissionKey::new("entered-contention").unwrap(),
                request,
            )
            .await
            .unwrap();
        let events = probe
            .service
            .events(WorkGraphEventFilter::default())
            .await
            .unwrap();
        match mode {
            Mode::ReleasedBeforeWait => {
                assert_eq!(rows.len(), 2, "one refused create and one fresh fence");
                assert_eq!(rows[0].1, Some(HelperAdmissionRefusal::Unavailable));
                assert!(!rows[0].2 && rows[1].2);
                // Retained Weak pointers keep each allocation distinct without
                // retaining its native owner or manufacturing a diagnostic ID.
                assert!(
                    !std::sync::Weak::ptr_eq(&rows[0].0, &rows[1].0),
                    "fresh actual Core fence"
                );
                assert_eq!(rows[1].1, None);
                let committed = result.unwrap();
                assert!(matches!(admission, Some(WorkAdmissionOutcome::Replayed(_))));
                assert_eq!(probe.attempts.lock().unwrap()[0].admission(), &committed);
                assert_eq!(
                    events
                        .iter()
                        .filter(|row| row.kind == WorkGraphEventKind::Created)
                        .count(),
                    1
                );
            }
            Mode::HeldPastBudget | Mode::Poisoned => {
                assert_eq!(rows.len(), 1, "unsettled or poisoned owner is final");
                assert_eq!(rows[0].1, Some(HelperAdmissionRefusal::Unavailable));
                assert_eq!(result, Err(PrivateOriginalError::Unavailable));
                assert!(admission.is_none() && events.is_empty());
            }
            Mode::ExpiredCallback => {
                assert_eq!(rows.len(), 1, "a callback error cannot earn a fresh fence");
                assert_eq!(
                    rows[0].1, None,
                    "the fence admitted the actual expiry callback"
                );
                assert_eq!(result, Err(PrivateOriginalError::Denied));
                assert!(admission.is_none() && events.is_empty());
            }
        }
        if matches!(mode, Mode::HeldPastBudget) {
            assert!(
                probe.elapsed.lock().unwrap().unwrap() < Duration::from_secs(1),
                "the 25 ms request budget must clamp the otherwise two-second owner wait"
            );
        }
        assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
        // The probe owns only its SQLite admission; no whole-Find success claim.
    }))
    .catch_unwind()
    .await;
    script.release_first_response();
    probe.holder.lock().unwrap().take();
    let attempts = probe.attempts.lock().unwrap().clone();
    for attempt in attempts {
        let _ = attempt.join().await;
    }
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "fixture cleanup: {cleanup:?}");
    match checked {
        Ok(Ok(())) => (),
        Ok(Err(_)) => panic!("fenced retry control timed out"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn entered_fence_completed_writer_hold_creates_once_with_fresh_fence() {
    exercise(Mode::ReleasedBeforeWait).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn entered_fence_held_writer_past_budget_keeps_unavailable() {
    exercise(Mode::HeldPastBudget).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn entered_fence_poisoned_publication_never_retries() {
    exercise(Mode::Poisoned).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn entered_fence_callback_error_never_retries() {
    exercise(Mode::ExpiredCallback).await;
}

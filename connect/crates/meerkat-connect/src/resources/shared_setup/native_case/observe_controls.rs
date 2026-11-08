//! A control of the shared SDK case for a Find's live-replay helper
//! observation, the step its admission makes after its work step completed
//! (`advance.observe_helper`). An owner change held open on the host's shared
//! publication there, as another original's first binding holds one, is
//! waited out with the Find's one churn, and the observation is made again
//! with a fresh fence: the Find admits its helper and the case completes.
use super::*;
use std::time::Instant;

/// A's Find: the call the control acts on.
const FIND: &str = "a-original-find";

#[derive(Default)]
struct Held {
    /// When the change began, and when it was published.
    change: Mutex<Option<(Instant, std::thread::JoinHandle<Instant>)>>,
    /// Each time A's Find reached its helper observation.
    reached: Mutex<Vec<Instant>>,
}

#[test]
fn owner_change_at_a_finds_helper_observation_is_waited_out_and_observed_again() {
    let hold = Duration::from_millis(120);
    let (pipes, parent) = SdkParent::pair();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    let held = Arc::new(Held::default());
    let seen = held.clone();
    runtime.block_on(async move {
        let mut case = Case::new(parent.enter());
        let result = std::panic::AssertUnwindSafe(async {
            case.construct(pipes).await;
            let publication = case
                .host
                .as_ref()
                .unwrap()
                .authorization_publication_for_test()
                .expect("the host's private originals share one publication");
            toolkit_mobkit::observe_helper_probe::install(Arc::new(move |call| {
                if call != FIND {
                    return;
                }
                seen.reached.lock().unwrap().push(Instant::now());
                let mut change = seen.change.lock().unwrap();
                if change.is_some() {
                    return;
                }
                let publication = publication.clone();
                let (began, started) = std::sync::mpsc::sync_channel(0);
                let ending = std::thread::spawn(move || {
                    let change = publication.begin_owner_change().unwrap();
                    began.send(()).unwrap();
                    std::thread::sleep(hold);
                    let ended = Instant::now();
                    drop(change);
                    ended
                });
                started.recv().unwrap();
                *change = Some((Instant::now(), ending));
            }));
            case.run().await;
        })
        .catch_unwind()
        .await;
        toolkit_mobkit::observe_helper_probe::clear();
        if result.is_err() {
            case.retain_failed_audits().await;
        }
        let cleanup = case.cleanup_completed_runtime().await;
        assert!(
            cleanup.is_empty(),
            "all SDK/native/physical owners must settle: {cleanup:?}"
        );
        if let Err(payload) = result {
            std::panic::resume_unwind(payload);
        }
    });
    let reached = held.reached.lock().unwrap().clone();
    let (began, ending) = held
        .change
        .lock()
        .unwrap()
        .take()
        .expect("the control held an owner change at A's helper observation");
    let ended = ending.join().unwrap();
    assert_eq!(
        reached.len(),
        1,
        "A's Find reached its helper observation once"
    );
    assert!(
        ended.duration_since(began) >= hold,
        "the change stayed in progress while the observation ran"
    );
}

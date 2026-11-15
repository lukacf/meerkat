//! A control of the shared SDK case for a Find's work step at its helper's
//! publication-source read (`complete.publication_source`), made once the
//! helper's controller grants were issued and its source bound. The owner's
//! originals collection held there by another owner's short section, with no
//! owner change named (as the scratch diagnostic met it), is waited out with
//! bounded pauses within Core's owner-change bound, holding nothing, and the
//! read's check made again: B's Find admits its helper and the case
//! completes. An owner change held at the same read is a toolkit-mobkit row
//! of its own, where no other Find's steps share the change.
//!
//! The control issues B's Find only once A's setup has settled into its held
//! install: the one physical worker has entered, and stays held until the
//! case releases it. Issued beside A's Find, as the case itself issues it, a
//! step of B's Find that does not wait can be refused while a step of A's
//! holds the same reservation, before the read under control is reached:
//! that is the case's own contention between two setups, not this control's
//! subject. B's original is still launched and admitted beside A's, and B's
//! helper still joins A's preparation while the worker is held.
use super::*;
use std::time::Instant;
use toolkit_mobkit::source_hold_probe::{self, Hold, Site};

/// B's Find: the call the control acts on.
const FIND: &str = "b-original-find";
/// What B's Find waits for.
const SETTLED: &str = "A's install holds the one physical worker";

#[test]
fn source_collection_held_at_b_finds_publication_source_is_waited_out_and_read_again() {
    let hold = Duration::from_millis(120);
    let (pipes, parent) = SdkParent::pair();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    let reached: Arc<Mutex<Vec<Instant>>> = Arc::default();
    let seen = reached.clone();
    let failed = reached.clone();
    runtime.block_on(async move {
        let mut case = Case::new(parent.enter());
        let result = std::panic::AssertUnwindSafe(async {
            case.construct(pipes).await;
            source_hold_probe::install(Arc::new(move |site, call| {
                if site != Site::PublicationSource || call != FIND {
                    return None;
                }
                let mut seen = seen.lock().unwrap();
                seen.push(Instant::now());
                (seen.len() == 1).then_some(Hold {
                    owner_change: false,
                    duration: hold,
                })
            }));
            // Only A's install can have entered the worker: B's helper
            // exists only once B's Find has run.
            let entered = case.directory.path().join("physical/WORKER-ENTERED.json");
            assert!(
                case.script
                    .find_gate
                    .set(FindGate {
                        name: SETTLED,
                        open: Box::new(move || entered.exists()),
                        opened: AtomicBool::new(false),
                    })
                    .is_ok()
            );
            case.run().await;
        })
        .catch_unwind()
        .await;
        source_hold_probe::clear();
        let settled = case
            .script
            .find_gate
            .get()
            .is_some_and(|gate| gate.opened.load(Ordering::SeqCst));
        if result.is_err() {
            eprintln!(
                "{}",
                json!({
                    "event":"sdk_held_find_control_failed", "find_gate_opened":settled,
                    "publication_source_reached":failed.lock().unwrap().len(),
                })
            );
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
        assert!(
            settled,
            "B's Find was issued only once this held: {SETTLED}"
        );
    });
    assert_eq!(
        reached.lock().unwrap().len(),
        1,
        "B's Find reached its publication-source read once"
    );
}

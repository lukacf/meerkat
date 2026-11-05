//! A control of the shared SDK case for a Find's work step at its helper's
//! publication-source read (`complete.publication_source`), made once the
//! helper's controller grants were issued and its source bound. The owner's
//! originals collection held there by another owner's short section, with no
//! owner change named (as the scratch diagnostic met it), is waited out with
//! bounded pauses within Core's owner-change bound, holding nothing, and the
//! read's check made again: B's Find admits its helper and the case
//! completes. An owner change held at the same read is a toolkit-mobkit row
//! of its own, where no other Find's steps share the change.
use super::*;
use std::time::Instant;
use toolkit_mobkit::publication_source_probe::{self, Hold};

/// B's Find: the call the control acts on.
const FIND: &str = "b-original-find";

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
    runtime.block_on(async move {
        let mut case = Case::new(parent.enter());
        let result = std::panic::AssertUnwindSafe(async {
            case.construct(pipes).await;
            publication_source_probe::install(Arc::new(move |call| {
                if call != FIND {
                    return None;
                }
                let mut seen = seen.lock().unwrap();
                seen.push(Instant::now());
                (seen.len() == 1).then_some(Hold {
                    owner_change: false,
                    duration: hold,
                })
            }));
            case.run().await;
        })
        .catch_unwind()
        .await;
        publication_source_probe::clear();
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
    assert_eq!(
        reached.lock().unwrap().len(),
        1,
        "B's Find reached its publication-source read once"
    );
}

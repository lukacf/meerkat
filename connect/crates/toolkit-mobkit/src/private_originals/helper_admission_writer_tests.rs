//! The Find's helper admission takes the owner's source writer, which another
//! original's admission or binding holds across its durable write. A short
//! hold is waited for, within Core's owner-change bound, and the admission
//! then admits once; a hold that outlasts the bound stays the existing
//! retryable unavailability, with nothing admitted.
use super::*;
use meerkat_core::authorization::OWNER_CHANGE_WAIT_BOUND;
use std::time::{Duration, Instant};

/// What the original's first Find answered with the owner's source writer
/// held for `hold` from just before its helper admission took it.
struct Found {
    reply: serde_json::Value,
    is_error: bool,
    helpers: usize,
    elapsed: Duration,
}

async fn find_with_writer_held(hold: Duration) -> Found {
    let script = acquisition_denial::Script::for_approved();
    let report = Report::new(script.tools().result_tool());
    let (fixture, _runtime_store) =
        Fixture::for_configured_helper_report(script.clone(), report.clone()).await;
    fixture
        .source
        .owner
        .hold_source_writer_at_next_helper_admission_for_test(hold);
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(
        WAIT,
        Box::pin(async {
            let started = Instant::now();
            let first = fixture.submit().await;
            fixture.finish(&first).await.unwrap();
            let elapsed = started.elapsed();
            let (feedback, _, _) = script.observations();
            assert_eq!(feedback.len(), 1, "one Find result");
            let reply: serde_json::Value =
                serde_json::from_str(&feedback[0].text_content()).unwrap();
            let helpers = fixture.source.helper_sources.lock().unwrap().len();
            Found {
                reply,
                is_error: feedback[0].is_error,
                helpers,
                elapsed,
            }
        }),
    ))
    .catch_unwind()
    .await;
    report.release();
    report.leaf.context.lock().unwrap().take();
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    match checked {
        Ok(Ok(found)) => found,
        Ok(Err(_)) => panic!("the Find did not settle before the fixture deadline"),
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn helper_admission_waits_for_a_held_source_writer() {
    let hold = Duration::from_millis(300);
    let found = find_with_writer_held(hold).await;
    assert!(
        !found.is_error,
        "a held source writer is waited for, not answered unavailable: {}",
        found.reply
    );
    assert_eq!(
        found.reply["worker"]["state"], "input_admitted",
        "{}",
        found.reply
    );
    assert_eq!(found.helpers, 1, "the helper was admitted once");
    assert!(
        found.elapsed >= hold - Duration::from_millis(50),
        "the admission waited for the writer: {:?}",
        found.elapsed
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn helper_admission_past_its_bound_stays_unavailable_with_nothing_admitted() {
    let found = find_with_writer_held(OWNER_CHANGE_WAIT_BOUND + Duration::from_millis(500)).await;
    assert!(found.is_error, "{}", found.reply);
    assert_eq!(
        found.reply["error"], "tool_unavailable",
        "the existing retryable refusal: {}",
        found.reply
    );
    assert_eq!(found.helpers, 0, "nothing was admitted");
    assert!(
        found.elapsed >= OWNER_CHANGE_WAIT_BOUND - Duration::from_millis(100),
        "the admission waited out its bound before refusing: {:?}",
        found.elapsed
    );
}

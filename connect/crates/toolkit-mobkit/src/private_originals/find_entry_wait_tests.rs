//! A Find's own effect entry meets an owner change of the shared publication
//! that another original's first binding holds open right then. At R1 the
//! entry waits the change out within the dispatch attempt's bounds, spends
//! its review once and enters, in the one dispatch of the leaf; a change that
//! outlasts the bound stays the refusal certified before the effect
//! (`tool_invoked: false`), with nothing admitted.
use super::*;
use meerkat_core::authorization::OWNER_CHANGE_WAIT_BOUND;
use std::time::{Duration, Instant};

/// What the original's first Find answered with an owner change held for
/// `hold` from just before its own effect entry.
struct Found {
    result: ToolResult,
    reply: serde_json::Value,
    helpers: usize,
    elapsed: Duration,
    /// Dispatches of the Find leaf.
    dispatched: usize,
}

async fn find_with_change_at_entry(hold: Duration) -> Found {
    let script = acquisition_denial::Script::for_approved();
    let report = Report::new(script.tools().result_tool());
    let (fixture, _runtime_store) =
        Fixture::for_configured_helper_report(script.clone(), report.clone()).await;
    let publication = fixture.source.owner.publication();
    let held = Arc::new(Mutex::new(Vec::new()));
    let threads = held.clone();
    crate::find_entry_probe::install(Arc::new(move |_call| {
        let publication = publication.clone();
        let (began, beginning) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            let change = publication.begin_owner_change().unwrap();
            began.send(()).unwrap();
            std::thread::sleep(hold);
            drop(change);
        });
        beginning.recv().unwrap();
        threads.lock().unwrap().push(thread);
    }));
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(
        WAIT,
        Box::pin(async {
            let started = Instant::now();
            let first = fixture.submit().await;
            fixture.finish(&first).await.unwrap();
            let elapsed = started.elapsed();
            let (feedback, _, dispatched) = script.observations();
            assert_eq!(feedback.len(), 1, "one Find result");
            let result = feedback[0].clone();
            let reply: serde_json::Value = serde_json::from_str(&result.text_content()).unwrap();
            let helpers = fixture.source.helper_sources.lock().unwrap().len();
            Found {
                result,
                reply,
                helpers,
                elapsed,
                dispatched,
            }
        }),
    ))
    .catch_unwind()
    .await;
    crate::find_entry_probe::clear();
    for thread in std::mem::take(&mut *held.lock().unwrap()) {
        thread.join().unwrap();
    }
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
async fn find_entry_waits_out_a_concurrent_owner_change_and_enters() {
    let hold = Duration::from_millis(150);
    let found = find_with_change_at_entry(hold).await;
    assert!(
        !found.result.is_error,
        "an owner change in progress at the Find's entry is waited out, not refused: {}",
        found.reply
    );
    assert_eq!(
        found.reply["worker"]["state"], "input_admitted",
        "{}",
        found.reply
    );
    assert_eq!(found.helpers, 1, "the helper was admitted once");
    assert_eq!(
        found.dispatched, 1,
        "the entry waited inside the one dispatch of the leaf, which never repeated"
    );
    assert!(
        found.elapsed >= hold - Duration::from_millis(50),
        "the entry waited for the change: {:?}",
        found.elapsed
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn find_entry_change_outlasting_the_bound_stays_certified_not_dispatched() {
    let found =
        find_with_change_at_entry(OWNER_CHANGE_WAIT_BOUND + Duration::from_millis(500)).await;
    assert!(found.result.is_error, "{}", found.reply);
    assert_eq!(
        found.reply["data"],
        serde_json::json!({"kind": "reprepare_required", "tool_invoked": false}),
        "a refusal met before the effect stays certified not dispatched: {}",
        found.reply
    );
    assert_eq!(found.helpers, 0, "nothing was admitted");
    assert_eq!(found.dispatched, 1, "one dispatch, refused at its entry");
    assert!(
        found.elapsed >= OWNER_CHANGE_WAIT_BOUND - Duration::from_millis(100),
        "the entry waited out its bound before refusing: {:?}",
        found.elapsed
    );
}

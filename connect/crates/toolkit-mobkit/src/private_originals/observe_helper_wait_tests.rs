//! A Find's live-replay helper observation (`advance.observe_helper`), made
//! after its work step completed, reads its helper's admission through a
//! fresh fence of its own. The fence refuses while an owner change is in
//! progress on the shared publication, as another original's first binding
//! holds one: that change is waited out with the Find's one churn and the
//! observation made again, holding nothing in between. A refusal with no
//! owner change in progress is answered at once, as before.
use super::*;
use meerkat_core::authorization::OWNER_CHANGE_WAIT_BOUND;
use std::time::{Duration, Instant};

/// What the original's first Find answered with `act` run once, just
/// before its helper observation.
struct Found {
    reply: serde_json::Value,
    is_error: bool,
    helpers: usize,
    /// From the observation's start to the Find's settled result.
    observed_for: Duration,
    /// The helper admission steps recorded from the observation's start.
    events: Vec<&'static str>,
}

#[derive(Clone, Copy)]
enum Act {
    /// An owner change on the shared publication, held for this long.
    OwnerChange(Duration),
    /// The owner's source writer, held for this long by another holder.
    SourceWriter(Duration),
}

async fn find_with_act_at_observation(act: Act) -> Found {
    let script = acquisition_denial::Script::for_approved();
    let report = Report::new(script.tools().result_tool());
    let (fixture, _runtime_store) =
        Fixture::for_configured_helper_report(script.clone(), report.clone()).await;
    let owner = fixture.source.owner.clone();
    let publication = owner.publication();
    let reached: Arc<Mutex<Option<(Instant, usize)>>> = Arc::default();
    let threads = Arc::new(Mutex::new(Vec::new()));
    let (seen, held) = (reached.clone(), threads.clone());
    crate::observe_helper_probe::install(Arc::new(move |_call| {
        let mut seen = seen.lock().unwrap();
        if seen.is_some() {
            return;
        }
        *seen = Some((
            Instant::now(),
            owner.helper_admission_events_for_test().len(),
        ));
        match act {
            Act::OwnerChange(hold) => {
                let publication = publication.clone();
                let (began, beginning) = std::sync::mpsc::channel();
                let thread = std::thread::spawn(move || {
                    let change = publication.begin_owner_change().unwrap();
                    began.send(()).unwrap();
                    std::thread::sleep(hold);
                    drop(change);
                });
                beginning.recv().unwrap();
                held.lock().unwrap().push(thread);
            }
            Act::SourceWriter(hold) => {
                owner.hold_source_writer_at_next_helper_admission_for_test(hold);
            }
        }
    }));
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(
        WAIT,
        Box::pin(async {
            let first = fixture.submit().await;
            fixture.finish(&first).await.unwrap();
            let settled = Instant::now();
            let (feedback, _, _) = script.observations();
            assert!(!feedback.is_empty(), "the Find result reached the model");
            assert!(
                feedback.iter().all(|seen| seen == &feedback[0]),
                "one Find result: {feedback:?}"
            );
            let reply: serde_json::Value =
                serde_json::from_str(&feedback[0].text_content()).unwrap();
            let helpers = fixture.source.helper_sources.lock().unwrap().len();
            let (start, from) = reached
                .lock()
                .unwrap()
                .expect("the Find reached its helper observation");
            Found {
                reply,
                is_error: feedback[0].is_error,
                helpers,
                observed_for: settled.duration_since(start),
                events: fixture.source.owner.helper_admission_events_for_test()[from..].to_vec(),
            }
        }),
    ))
    .catch_unwind()
    .await;
    crate::observe_helper_probe::clear();
    for thread in std::mem::take(&mut *threads.lock().unwrap()) {
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

fn count(events: &[&str], event: &str) -> usize {
    events.iter().filter(|seen| **seen == event).count()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn helper_observation_waits_out_an_owner_change_and_observes_again() {
    let hold = Duration::from_millis(150);
    let found = find_with_act_at_observation(Act::OwnerChange(hold)).await;
    assert!(
        !found.is_error,
        "an owner change in progress at the helper observation is waited out, not refused: {}",
        found.reply
    );
    assert_eq!(
        found.reply["worker"]["state"], "input_admitted",
        "{}",
        found.reply
    );
    assert_eq!(found.helpers, 1, "the helper was admitted once");
    assert_eq!(
        count(&found.events, "writer_acquired"),
        2,
        "the observation refused under the change, then was made again once: {:?}",
        found.events
    );
    assert!(
        found.observed_for >= hold - Duration::from_millis(50),
        "the observation waited for the change: {:?}",
        found.observed_for
    );
}

// The negative control: the owner's source writer held past its bound at the
// observation, with no owner change in progress. The observation answers
// unavailable once its writer wait ran out, at once and typed, and it is not
// made again: no writer is taken for it after its holder released it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn helper_observation_refused_with_no_owner_change_is_answered_at_once() {
    let found = find_with_act_at_observation(Act::SourceWriter(
        OWNER_CHANGE_WAIT_BOUND + Duration::from_millis(500),
    ))
    .await;
    assert!(found.is_error, "{}", found.reply);
    assert_eq!(
        found.reply["error"], "tool_unavailable",
        "the observation's own typed refusal: {}",
        found.reply
    );
    assert_eq!(
        count(&found.events, "writer_acquired"),
        0,
        "the refused observation was not made again: {:?}",
        found.events
    );
    assert!(
        found.observed_for >= OWNER_CHANGE_WAIT_BOUND - Duration::from_millis(100),
        "answered when the writer wait ran out: {:?}",
        found.observed_for
    );
}

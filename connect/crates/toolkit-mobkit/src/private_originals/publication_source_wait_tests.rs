//! A Find's work step reads its helper's publication source
//! (`complete.publication_source`) once the helper's controller grants were
//! issued and its source bound. Another owner's short section holds the
//! owner's originals collection there (another helper's first binding write
//! holds it inside an owner change on the shared publication), and the
//! read's source checks then find it contended. That is waited out with
//! bounded pauses within Core's owner-change bound, holding nothing, and an
//! owner change Core names with the work step's one churn; only the read is
//! made again, and nothing is issued or bound again. Held past the bound,
//! the read is refused typed and is not made again. A refusal that the
//! bounds outlast leaves the work to a fresh Find, which recovers the bound
//! source and issues nothing again.
use super::*;
use crate::publication_source_probe::{self, Hold};
use meerkat_core::authorization::OWNER_CHANGE_WAIT_BOUND;
use std::sync::atomic::AtomicUsize;
use std::time::{Duration, Instant};

/// What the original's Finds settled, with `hold` made once, at the first
/// Find's publication-source read.
struct Found {
    /// Each Find's result, in the order the model first saw them.
    feedback: Vec<ToolResult>,
    /// The helpers whose source this owner retains.
    helpers: usize,
    /// The publication-source reads made, from the first on.
    reads: usize,
    /// Controller grants the helper's issuance appended.
    issued: usize,
    /// From the first read to the original's settled turn.
    settled_after: Duration,
    /// The helper's source row and its work effect after the first Find
    /// settled (taken before any fresh Find), and at the end.
    first_row: Option<toolkit_store::StoredRecord>,
    first_effect: Option<toolkit_store::StoredEffect>,
    last_row: toolkit_store::StoredRecord,
    last_effect: toolkit_store::StoredEffect,
}

/// The helper's one canonical source row.
async fn helper_row(fixture: &Fixture) -> toolkit_store::StoredRecord {
    let scope = toolkit_store::StoreScope {
        realm: fixture.scope.realm.clone(),
        owner: format!("toolkit/helper-originals/{}", fixture.scope.owner),
    };
    let rows = fixture.store.list_records(&scope, None, 10).await.unwrap();
    assert_eq!(rows.len(), 1, "one canonical helper source row");
    fixture.store.get(&rows[0].key).await.unwrap().unwrap()
}

/// The helper's work effect, named by its source row.
async fn work_effect(
    fixture: &Fixture,
    row: &toolkit_store::StoredRecord,
) -> toolkit_store::StoredEffect {
    let operation: crate::original_ingress::helper_originals::HelperOperation =
        serde_json::from_value(row.data.body["facts"]["operation"].clone()).unwrap();
    let key = toolkit_store::EffectKey {
        record: operation.acquisition,
        id: operation.effect,
    };
    fixture.store.get_effect(&key).await.unwrap().unwrap()
}

async fn finds_with_hold_at_publication_source(hold: Hold, fresh_finds: usize) -> Found {
    let script = acquisition_denial::Script::for_approved();
    script.arm_fresh_finds(fresh_finds);
    let report = Report::new(script.tools().result_tool());
    let (fixture, _runtime_store) =
        Fixture::for_configured_helper_report(script.clone(), report.clone()).await;
    let issued = Arc::new(AtomicUsize::new(0));
    let appended = issued.clone();
    assert!(
        fixture
            .source
            .helper_controller_appended
            .set(Arc::new(move |_lineage| {
                appended.fetch_add(1, Ordering::SeqCst);
            }))
            .is_ok()
    );
    let reached: Arc<Mutex<Option<(Instant, usize)>>> = Arc::default();
    let seen = reached.clone();
    let owner = fixture.source.owner.clone();
    publication_source_probe::install(Arc::new(move |_call| {
        let mut seen = seen.lock().unwrap();
        if seen.is_some() {
            return None;
        }
        *seen = Some((
            Instant::now(),
            owner.helper_admission_events_for_test().len(),
        ));
        Some(hold)
    }));
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(
        WAIT,
        Box::pin(async {
            let first = fixture.submit().await;
            // Each fresh Find goes once the hold ended and the first Find's
            // settled state was read.
            let fresh = async {
                if fresh_finds == 0 {
                    return (None, None);
                }
                let start = loop {
                    if let Some((start, _)) = *reached.lock().unwrap() {
                        break start;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                };
                tokio::time::sleep_until(
                    tokio::time::Instant::from_std(start + hold.duration)
                        + Duration::from_millis(100),
                )
                .await;
                let row = helper_row(&fixture).await;
                let effect = work_effect(&fixture, &row).await;
                for _ in 0..fresh_finds {
                    script.release_fresh_find();
                }
                (Some(row), Some(effect))
            };
            let (finished, (first_row, first_effect)) = tokio::join!(fixture.finish(&first), fresh);
            finished.unwrap();
            let settled = Instant::now();
            let (feedback, _, _) = script.observations();
            let (start, from) = reached
                .lock()
                .unwrap()
                .expect("the first Find reached its publication-source read");
            let reads = fixture.source.owner.helper_admission_events_for_test()[from..]
                .iter()
                .filter(|event| **event == "publication_source_read")
                .count();
            let last_row = helper_row(&fixture).await;
            let last_effect = work_effect(&fixture, &last_row).await;
            Found {
                feedback,
                helpers: fixture.source.helper_sources.lock().unwrap().len(),
                reads,
                issued: issued.load(Ordering::SeqCst),
                settled_after: settled.duration_since(start),
                first_row,
                first_effect,
                last_row,
                last_effect,
            }
        }),
    ))
    .catch_unwind()
    .await;
    publication_source_probe::clear();
    report.release();
    report.leaf.context.lock().unwrap().take();
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    match checked {
        Ok(Ok(found)) => found,
        Ok(Err(_)) => panic!("the Finds did not settle before the fixture deadline"),
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

fn reply(result: &ToolResult) -> serde_json::Value {
    serde_json::from_str(&result.text_content()).unwrap()
}

/// The Find admitted its helper once the hold was waited out, with the
/// controller grant issued once.
fn assert_waited_out(found: &Found, hold: Duration) {
    assert_eq!(found.feedback.len(), 1, "{:?}", found.feedback);
    let first = &found.feedback[0];
    assert!(
        !first.is_error,
        "a held source collection at the publication-source read is waited out, not refused: {}",
        reply(first)
    );
    assert_eq!(
        reply(first)["worker"]["state"],
        "input_admitted",
        "{}",
        reply(first)
    );
    assert_eq!(found.helpers, 1, "the helper was retained once");
    assert!(found.reads >= 1, "the read was made");
    assert_eq!(found.issued, 1, "the controller grant was issued once");
    assert!(
        found.settled_after >= hold - Duration::from_millis(50),
        "the read waited for the hold: {:?}",
        found.settled_after
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publication_source_read_waits_out_sources_held_inside_an_owner_change() {
    let hold = Duration::from_millis(150);
    let found = finds_with_hold_at_publication_source(
        Hold {
            owner_change: true,
            duration: hold,
        },
        0,
    )
    .await;
    assert_waited_out(&found, hold);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publication_source_read_waits_out_sources_held_with_no_owner_change() {
    let hold = Duration::from_millis(150);
    let found = finds_with_hold_at_publication_source(
        Hold {
            owner_change: false,
            duration: hold,
        },
        0,
    )
    .await;
    assert_waited_out(&found, hold);
}

// The negative control: the owner's originals collection held at the read
// past Core's owner-change bound, with no owner change in progress. The read
// is refused typed once the bound ran out, and is not made again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publication_source_read_held_past_the_bound_is_refused_typed_and_not_read_again() {
    let found = finds_with_hold_at_publication_source(
        Hold {
            owner_change: false,
            duration: OWNER_CHANGE_WAIT_BOUND + Duration::from_millis(500),
        },
        0,
    )
    .await;
    assert_eq!(found.feedback.len(), 1, "{:?}", found.feedback);
    let first = &found.feedback[0];
    assert!(first.is_error, "{}", reply(first));
    assert_eq!(
        reply(first)["error"],
        "tool_unavailable",
        "the read's own typed refusal: {}",
        reply(first)
    );
    assert_eq!(found.reads, 1, "the refused read was not made again");
    assert_eq!(found.issued, 1, "the controller grant was issued once");
    assert!(
        found.settled_after >= OWNER_CHANGE_WAIT_BOUND - Duration::from_millis(100),
        "refused once the bound ran out: {:?}",
        found.settled_after
    );
}

// A refusal that the bounds outlast (the collection held inside an owner
// change past both the contention bound and the churn's): the work step
// marks its claim uncertain and keeps the issued grant and the bound source.
// A fresh Find is answered retry_safe for the uncertain work, and the next
// fresh Find recovers the bound source: the helper is admitted with no grant
// issued and no source bound again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publication_source_refused_past_the_bounds_issues_no_second_grant() {
    let found = finds_with_hold_at_publication_source(
        Hold {
            owner_change: true,
            duration: OWNER_CHANGE_WAIT_BOUND * 2 + Duration::from_secs(1),
        },
        2,
    )
    .await;
    assert_eq!(found.feedback.len(), 3, "{:?}", found.feedback);
    let first = &found.feedback[0];
    assert!(first.is_error, "{}", reply(first));
    assert_eq!(
        reply(first)["error"],
        "tool_unavailable",
        "a refusal past the bounds stays typed: {}",
        reply(first)
    );
    assert_eq!(
        reply(&found.feedback[1])["worker"]["state"],
        "retry_safe",
        "{}",
        reply(&found.feedback[1])
    );
    assert_eq!(
        reply(&found.feedback[2])["worker"]["state"],
        "input_admitted",
        "{}",
        reply(&found.feedback[2])
    );
    assert_eq!(found.helpers, 1, "the helper was retained once");
    assert_eq!(found.issued, 1, "no second grant was issued");
    let first_row = found
        .first_row
        .expect("the first Find's settled source row");
    assert_eq!(
        first_row.data.body["state"]["phase"], "bound",
        "{first_row:?}"
    );
    for part in ["facts", "controller", "state"] {
        assert_eq!(
            found.last_row.data.body[part], first_row.data.body[part],
            "the bound source's {part} is unchanged"
        );
    }
    let first_effect = found
        .first_effect
        .expect("the first Find's settled work effect");
    assert_eq!(first_effect.attempt, 1);
    assert_eq!(first_effect.status, toolkit_store::EffectStatus::Uncertain);
    let Some(toolkit_store::EffectOutcome::Uncertain { evidence }) = &first_effect.outcome else {
        panic!("the refused completion keeps its claim uncertain: {first_effect:?}")
    };
    assert_eq!(evidence.body["kind"], "completion_uncertain");
    assert_eq!(found.last_effect.key, first_effect.key);
    assert_eq!(
        found.last_effect.attempt, 2,
        "the one fresh attempt completed the same work"
    );
}

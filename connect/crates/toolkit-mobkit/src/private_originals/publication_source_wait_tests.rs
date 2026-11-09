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
//! bounds outlast keeps the refused work as it was: a later Find is another
//! model call, so it is its own invocation (Toolkit's retry correlation is
//! the original, its input and the call ID) and makes its own work once. No
//! effect runs twice, and the refused work is never issued again.
//!
//! [`finds_with_hold_at`] makes the same hold at any site of the source hold
//! probe ([`Site`]); the helper source rows of the other sites use it too.
use super::*;
use crate::source_hold_probe::{self, Hold, Site};
use meerkat_core::authorization::OWNER_CHANGE_WAIT_BOUND;
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

/// What the original's Finds settled, with `hold` made once, at the site's
/// hit the control chose.
pub(super) struct Found {
    /// Each Find's result, in the order the model first saw them.
    pub(super) feedback: Vec<ToolResult>,
    /// The helpers whose source this owner retains.
    pub(super) helpers: usize,
    /// The publication-source reads made, from the hold on.
    pub(super) reads: usize,
    /// The controller grants appended, one entry (its lineage) per append.
    pub(super) issued: Vec<String>,
    /// From the hold to the first Find's result reaching the model.
    pub(super) answered_after: Duration,
    /// The first Find's helper source row and its work effect, read after
    /// the hold ended and before any fresh Find.
    pub(super) first_row: Option<toolkit_store::StoredRecord>,
    pub(super) first_effect: Option<toolkit_store::StoredEffect>,
    /// Every helper source row the Finds wrote, with its work effect, at
    /// the end.
    pub(super) rows: Vec<(toolkit_store::StoredRecord, toolkit_store::StoredEffect)>,
    /// How long another actor's owner change, begun on the owner's
    /// publication while the hold lasted, waited to begin.
    pub(super) concurrent_change: Option<Duration>,
    /// Whether the original's turn completed (its native ticket answered).
    pub(super) turn_completed: bool,
    /// Every "not now" a binder answered (its site and key), in order.
    pub(super) not_now: Vec<(Site, String)>,
}

fn helper_scope(fixture: &Fixture) -> toolkit_store::StoreScope {
    toolkit_store::StoreScope {
        realm: fixture.scope.realm.clone(),
        owner: format!("toolkit/helper-originals/{}", fixture.scope.owner),
    }
}

/// The ids of the helper source rows in the owner's helper scope.
async fn helper_rows(fixture: &Fixture) -> BTreeSet<String> {
    let rows = fixture
        .store
        .list_records(&helper_scope(fixture), None, 10)
        .await
        .unwrap();
    assert!(rows.len() < 10);
    rows.into_iter().map(|row| row.key.id).collect()
}

/// The helper source rows written after `before`.
async fn new_rows(
    fixture: &Fixture,
    before: &BTreeSet<String>,
) -> Vec<toolkit_store::StoredRecord> {
    let rows = fixture
        .store
        .list_records(&helper_scope(fixture), None, 10)
        .await
        .unwrap();
    assert!(rows.len() < 10);
    let mut found = vec![];
    for row in rows {
        if !before.contains(&row.key.id) {
            found.push(fixture.store.get(&row.key).await.unwrap().unwrap());
        }
    }
    found
}

/// The one helper source row the first Find's helper wrote.
async fn helper_row(fixture: &Fixture, before: &BTreeSet<String>) -> toolkit_store::StoredRecord {
    let mut rows = new_rows(fixture, before).await;
    assert_eq!(
        rows.len(),
        1,
        "one canonical source row of this helper: {:?}",
        rows.iter().map(|row| &row.key.id).collect::<Vec<_>>()
    );
    rows.remove(0)
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
    finds_with_hold_at(Site::PublicationSource, 1, hold, fresh_finds, false).await
}

/// A control's hits of its site so far, and, once the chosen hit came, the
/// hold's start with the owner's helper admission events then.
#[derive(Default)]
struct Reached {
    hits: usize,
    start: Option<(Instant, usize)>,
}

/// The original's Finds, with `hold` made once, at the `nth` hit (from 1)
/// of `site`, and `fresh_finds` fresh Finds released once the hold ended.
/// With `concurrent_change`, another actor begins and ends an owner change
/// on the owner's publication 30 ms into the hold, timing how long it waited
/// to begin.
pub(super) async fn finds_with_hold_at(
    site: Site,
    nth: usize,
    hold: Hold,
    fresh_finds: usize,
    concurrent_change: bool,
) -> Found {
    let script = acquisition_denial::Script::for_approved();
    script.arm_fresh_finds(fresh_finds);
    let report = Report::new(script.tools().result_tool());
    let (fixture, _runtime_store) =
        Fixture::for_configured_helper_report(script.clone(), report.clone()).await;
    let issued: Arc<Mutex<Vec<String>>> = Arc::default();
    let appended = issued.clone();
    assert!(
        fixture
            .source
            .helper_controller_appended
            .set(Arc::new(move |lineage| {
                appended
                    .lock()
                    .unwrap()
                    .push(serde_json::to_string(lineage).unwrap());
            }))
            .is_ok()
    );
    // The site's hits so far, and the hold's start.
    let reached: Arc<Mutex<Reached>> = Arc::default();
    let seen = reached.clone();
    let owner = fixture.source.owner.clone();
    let concurrent: Arc<Mutex<Option<Duration>>> = Arc::default();
    let measured = concurrent.clone();
    let not_now: Arc<Mutex<Vec<(Site, String)>>> = Arc::default();
    let answered_not_now = not_now.clone();
    source_hold_probe::install(Arc::new(move |at, key| {
        if matches!(at, Site::OriginalNotNow | Site::HelperNotNow) {
            answered_not_now.lock().unwrap().push((at, key.to_owned()));
            return None;
        }
        if at != site {
            return None;
        }
        let mut seen = seen.lock().unwrap();
        seen.hits += 1;
        if seen.hits != nth {
            return None;
        }
        seen.start = Some((
            Instant::now(),
            owner.helper_admission_events_for_test().len(),
        ));
        if concurrent_change {
            let publication = owner.publication_for_test().clone();
            let measured = measured.clone();
            source_hold_probe::retain(std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(30));
                let started = Instant::now();
                let change = publication.begin_owner_change().unwrap();
                *measured.lock().unwrap() = Some(started.elapsed());
                std::thread::sleep(Duration::from_millis(1));
                drop(change);
            }));
        }
        Some(hold)
    }));
    // The production shape: the source's custody recovery runs.
    start_custody_recovery(&fixture);
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(
        WAIT,
        Box::pin(async {
            let before = helper_rows(&fixture).await;
            let first = fixture.submit().await;
            // When the first Find's result reached the original's model.
            let answered = async {
                loop {
                    if !script.observations().0.is_empty() {
                        break Instant::now();
                    }
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
            };
            // Each fresh Find goes once the hold ended and the first Find's
            // settled state was read.
            let fresh = async {
                if fresh_finds == 0 {
                    return (None, None);
                }
                let start = loop {
                    if let Some((start, _)) = reached.lock().unwrap().start {
                        break start;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                };
                tokio::time::sleep_until(
                    tokio::time::Instant::from_std(start + hold.duration)
                        + Duration::from_millis(100),
                )
                .await;
                let row = helper_row(&fixture, &before).await;
                let effect = work_effect(&fixture, &row).await;
                for _ in 0..fresh_finds {
                    script.release_fresh_find();
                }
                (Some(row), Some(effect))
            };
            let (finished, answered, (first_row, first_effect)) =
                tokio::join!(fixture.finish(&first), answered, fresh);
            let turn_completed = finished.is_ok();
            let (feedback, _, _) = script.observations();
            let (start, from) = reached
                .lock()
                .unwrap()
                .start
                .expect("the Find reached the hold's site");
            let reads = fixture.source.owner.helper_admission_events_for_test()[from..]
                .iter()
                .filter(|event| **event == "publication_source_read")
                .count();
            let mut rows = vec![];
            for row in new_rows(&fixture, &before).await {
                let effect = work_effect(&fixture, &row).await;
                rows.push((row, effect));
            }
            Found {
                feedback,
                helpers: fixture.source.helper_sources.lock().unwrap().len(),
                reads,
                issued: issued.lock().unwrap().clone(),
                answered_after: answered.duration_since(start),
                first_row,
                first_effect,
                rows,
                concurrent_change: None,
                turn_completed,
                not_now: not_now.lock().unwrap().clone(),
            }
        }),
    ))
    .catch_unwind()
    .await;
    fixture.source.stop_custody_recovery();
    source_hold_probe::clear();
    report.release();
    report.leaf.context.lock().unwrap().take();
    let cleanup = fixture.shutdown().await;
    let recovery = fixture.source.join_custody_recovery().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    assert!(
        recovery.is_ok(),
        "the custody recovery joined: {recovery:?}"
    );
    match checked {
        Ok(Ok(found)) => Found {
            concurrent_change: *concurrent.lock().unwrap(),
            ..found
        },
        Ok(Err(_)) => panic!("the Finds did not settle before the fixture deadline"),
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

pub(super) fn reply(result: &ToolResult) -> serde_json::Value {
    serde_json::from_str(&result.text_content()).unwrap()
}

/// The Find admitted its helper once the hold was waited out, with the
/// controller grant issued once.
pub(super) fn assert_waited_out(found: &Found, hold: Duration) {
    assert!(found.turn_completed, "the original's turn completed");
    assert_eq!(found.feedback.len(), 1, "{:?}", found.feedback);
    let first = &found.feedback[0];
    assert!(
        !first.is_error,
        "a held source collection is waited out, not refused: {}",
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
    assert_eq!(
        found.issued.len(),
        1,
        "the controller grant was issued once"
    );
    assert_eq!(
        found.rows.len(),
        1,
        "one canonical source row of this helper"
    );
    assert!(
        found.answered_after >= hold - Duration::from_millis(50),
        "the read waited for the hold: {:?}",
        found.answered_after
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
// is refused typed once the bound ran out (before the hold ended), and is not
// made again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publication_source_read_held_past_the_bound_is_refused_typed_and_not_read_again() {
    let hold = OWNER_CHANGE_WAIT_BOUND + Duration::from_millis(500);
    let found = finds_with_hold_at_publication_source(
        Hold {
            owner_change: false,
            duration: hold,
        },
        0,
    )
    .await;
    assert!(found.turn_completed, "the original's turn completed");
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
    assert_eq!(
        found.issued.len(),
        1,
        "the controller grant was issued once"
    );
    assert_eq!(
        found.rows.len(),
        1,
        "one canonical source row of this helper"
    );
    assert!(
        found.answered_after >= OWNER_CHANGE_WAIT_BOUND - Duration::from_millis(100)
            && found.answered_after < hold,
        "refused once the bound ran out, before the hold ended: {:?}",
        found.answered_after
    );
}

// An invariant row, the same before and after the fix: a refusal that the
// bounds outlast (the collection held inside an owner change past both the
// contention bound and the churn's) never makes the refused work's effects
// again. The work step marks its claim uncertain and keeps the issued grant
// and the bound source. A fresh Find, released after the hold ended, is
// another model call: its own invocation, with its own operation, work item
// and grant, each made once. The refused work's row, claim and grant stay as
// the first Find left them.
//
// The scripted model serves each armed fresh Find by model operation: when
// Core prepares the request carrying the first Find's result again (its
// first attempt, prepared during the owner change, was refused before any
// response), that attempt is the same model call and gets the same fresh
// call ID, with no second release.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publication_source_refused_work_is_never_reissued_and_no_effect_runs_twice() {
    let found = finds_with_hold_at_publication_source(
        Hold {
            owner_change: true,
            duration: OWNER_CHANGE_WAIT_BOUND * 2 + Duration::from_secs(1),
        },
        1,
    )
    .await;
    assert!(found.turn_completed, "the original's turn completed");
    assert_eq!(found.feedback.len(), 2, "{:?}", found.feedback);
    let first = &found.feedback[0];
    assert!(first.is_error, "{}", reply(first));
    assert_eq!(
        reply(first)["error"],
        "tool_unavailable",
        "a refusal past the bounds stays typed: {}",
        reply(first)
    );
    let fresh = &found.feedback[1];
    assert!(!fresh.is_error, "{}", reply(fresh));
    assert_eq!(
        reply(fresh)["worker"]["state"],
        "input_admitted",
        "the fresh Find admits its own helper: {}",
        reply(fresh)
    );
    // The refused work as the first Find left it: bound, its claim
    // uncertain at its first attempt.
    let first_row = found
        .first_row
        .expect("the first Find's settled source row");
    assert_eq!(
        first_row.data.body["state"]["phase"], "bound",
        "{first_row:?}"
    );
    let first_effect = found
        .first_effect
        .expect("the first Find's settled work effect");
    assert_eq!(first_effect.attempt, 1);
    assert_eq!(first_effect.status, toolkit_store::EffectStatus::Uncertain);
    let Some(toolkit_store::EffectOutcome::Uncertain { evidence }) = &first_effect.outcome else {
        panic!("the refused completion keeps its claim uncertain: {first_effect:?}")
    };
    assert_eq!(evidence.body["kind"], "completion_uncertain");
    // At the end: the refused work unchanged, and the fresh Find's own work.
    assert_eq!(
        found.rows.len(),
        2,
        "the refused work's row and the fresh Find's own"
    );
    let (refused_row, refused_effect) = found
        .rows
        .iter()
        .find(|(row, _)| row.key == first_row.key)
        .expect("the refused work's row");
    for part in ["facts", "controller", "state"] {
        assert_eq!(
            refused_row.data.body[part], first_row.data.body[part],
            "the refused work's {part} is unchanged"
        );
    }
    assert_eq!(refused_effect.key, first_effect.key);
    assert_eq!(
        refused_effect.attempt, 1,
        "the refused work was not attempted again"
    );
    assert_eq!(
        refused_effect.status,
        toolkit_store::EffectStatus::Uncertain
    );
    let (own_row, own_effect) = found
        .rows
        .iter()
        .find(|(row, _)| row.key != first_row.key)
        .expect("the fresh Find's own row");
    assert_ne!(
        own_row.data.body["facts"]["operation"], first_row.data.body["facts"]["operation"],
        "the fresh Find's own operation"
    );
    assert_ne!(
        own_row.data.body["facts"]["admission"]["key"],
        first_row.data.body["facts"]["admission"]["key"],
        "the fresh Find's own work item"
    );
    assert_eq!(own_row.data.body["state"]["phase"], "bound", "{own_row:?}");
    assert_eq!(own_effect.attempt, 1);
    assert_eq!(own_effect.status, toolkit_store::EffectStatus::Succeeded);
    // One grant per work, each appended once: the refused work's never again.
    assert_eq!(found.issued.len(), 2, "one grant per work");
    assert_ne!(
        found.issued[0], found.issued[1],
        "no grant was appended twice"
    );
    assert_eq!(found.helpers, 2, "each work's helper source retained once");
}

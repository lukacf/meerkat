//! A workflow launch meets its source held at one of the shared
//! binder's input checks (its input preparation's retained-source check, its
//! admitted-custody check).
//!
//! The launch's first bind binds its planning original inside the bind
//! (controller child grant, binding record), so on a first launch both checks
//! are past the attempt's first durable effect and keep the shipped outcome.
//! A launch of an original already bound answers "not now" there and binds
//! once after the release.
use super::*;
use crate::source_hold_probe::{Hold, Site};

/// How long the probe holds the source collection.
const HOLD: Duration = Duration::from_millis(300);

/// The marks a first launch's binding writes record.
const FIRST_BINDING_MARKS: [&str; 2] = ["original.controller_grant", "original.first_binding"];

/// What the probe saw: each site it was asked at, its key, and the
/// attempt's effect mark there.
#[derive(Default)]
struct Seen(Mutex<Vec<(Site, String, Option<&'static str>)>>);

impl Seen {
    fn at(&self, site: Site) -> Vec<(String, Option<&'static str>)> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|(seen, ..)| *seen == site)
            .map(|(_, key, mark)| (key.clone(), *mark))
            .collect()
    }
}

/// Install a probe that holds the source collection at `site` on its first
/// hit, and records every site it is asked at with the effect mark there.
fn hold_at(site: Site) -> Arc<Seen> {
    hold_at_for(site, HOLD)
}

/// [`hold_at`], holding the collection for `duration`.
fn hold_at_for(site: Site, duration: Duration) -> Arc<Seen> {
    let seen = Arc::new(Seen::default());
    let recorder = seen.clone();
    crate::source_hold_probe::install(Arc::new(move |at, key| {
        let first = !recorder
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|(seen, ..)| *seen == at);
        recorder.0.lock().unwrap().push((
            at,
            key.to_owned(),
            meerkat_core::protected_section::effect_recorded(),
        ));
        (at == site && first).then_some(Hold {
            owner_change: false,
            duration,
        })
    }));
    seen
}

async fn clear_probe() {
    tokio::task::spawn_blocking(crate::source_hold_probe::clear)
        .await
        .unwrap();
}

/// One launch attempt of `c`'s planning through a fresh real workflow binder.
async fn launch_once(f: &FirstFixture, c: &Case) -> WorkflowLaunchObservation {
    let prepared = Arc::new(PreparedWorkflowLaunch {
        selection: c.planning.clone(),
        reference: c.snapshot.reference().clone(),
    });
    let route = f
        .source
        .route(&c.planning.submission.original.route)
        .unwrap();
    let binder = prepared.binder(&f.source, &route).await.unwrap();
    prepared
        .submit_with_binder(&f.source, binder, None)
        .await
        .unwrap()
}

fn started_once(observed: &WorkflowLaunchObservation) -> bool {
    matches!(
        observed,
        WorkflowLaunchObservation::Native {
            observation: PrivateOriginalSubmission::Admitted {
                deduplicated: false,
                ..
            }
        }
    )
}

/// A first launch whose source is held at its input preparation keeps its
/// shipped refusal (`NotAdmitted`, nothing started), never a "not now": its
/// bind had already bound the planning original, and the mark there names
/// that write.
///
/// RED with the first-binding marks dropped: the mark there is unset.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_first_launch_held_at_input_preparation_keeps_its_shipped_refusal() {
    let f = FirstFixture::for_continuation().await;
    let result = AssertUnwindSafe(tokio::time::timeout(GUARD, async {
        let c = prepare(&f).await;
        assert_eq!(
            f.inputs().await.unwrap(),
            2,
            "A and independent P are admitted"
        );
        let seen = hold_at(Site::OriginalPrepare);
        let observed = launch_once(&f, &c).await;
        assert!(
            matches!(observed, WorkflowLaunchObservation::NotAdmitted),
            "the shipped refusal: {}",
            serde_json::to_string(&observed).unwrap()
        );
        assert_eq!(f.inputs().await.unwrap(), 2, "nothing started");
        assert!(seen.at(Site::OriginalNotNow).is_empty(), "never not now");
        let mark = seen
            .at(Site::OriginalPrepare)
            .first()
            .and_then(|(_, mark)| *mark);
        assert!(
            mark.is_some_and(|mark| FIRST_BINDING_MARKS.contains(&mark)),
            "the check runs after the first launch's binding write: {mark:?}"
        );
    }))
    .catch_unwind()
    .await;
    clear_probe().await;
    let cleanup = f.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    match result {
        Ok(result) => result.expect("bounded launch control"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

/// A first launch whose source is held at its admitted-custody check waits
/// there as shipped and starts once; no "not now" escapes, and the mark there
/// names its first binding write.
///
/// RED with the first-binding marks dropped: the mark there is unset.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_first_launch_held_at_the_admitted_check_waits_as_shipped_and_starts_once() {
    let f = FirstFixture::for_continuation().await;
    let result = AssertUnwindSafe(tokio::time::timeout(GUARD, async {
        let c = prepare(&f).await;
        assert_eq!(
            f.inputs().await.unwrap(),
            2,
            "A and independent P are admitted"
        );
        let seen = hold_at(Site::OriginalAdmitted);
        let observed = launch_once(&f, &c).await;
        assert!(
            started_once(&observed),
            "the launch waited in place and started once: {}",
            serde_json::to_string(&observed).unwrap()
        );
        assert_eq!(
            f.inputs().await.unwrap(),
            3,
            "the launch input was admitted once"
        );
        assert!(seen.at(Site::OriginalNotNow).is_empty(), "never not now");
        let admitted = seen.at(Site::OriginalAdmitted);
        assert_eq!(admitted.len(), 1, "one bind: the check waited in place");
        assert!(
            admitted[0]
                .1
                .is_some_and(|mark| FIRST_BINDING_MARKS.contains(&mark)),
            "the check runs after the first launch's binding write: {admitted:?}"
        );
    }))
    .catch_unwind()
    .await;
    clear_probe().await;
    let cleanup = f.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    match result {
        Ok(result) => result.expect("bounded launch control"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

/// A launch of a planning original already bound by a first launch that
/// ended without admission, while its source is held at the admitted-custody
/// check: the check answers "not now" (the source is held) with nothing
/// written by this attempt, the launch waits outside the identity's lock,
/// binds again and starts once.
///
/// RED on the base: the check waits in place, inside the bind.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bound_launch_sent_again_while_its_source_is_held_is_not_now_then_starts_once() {
    let f = FirstFixture::for_continuation().await;
    let result = AssertUnwindSafe(tokio::time::timeout(GUARD, async {
        let c = prepare(&f).await;
        assert_eq!(
            f.inputs().await.unwrap(),
            2,
            "A and independent P are admitted"
        );
        let first_seen = hold_at(Site::OriginalPrepare);
        let first = launch_once(&f, &c).await;
        clear_probe().await;
        assert!(
            matches!(first, WorkflowLaunchObservation::NotAdmitted),
            "the first launch ended without admission: {}",
            serde_json::to_string(&first).unwrap()
        );
        assert!(first_seen.at(Site::OriginalNotNow).is_empty());
        let seen = hold_at(Site::OriginalAdmitted);
        let observed = launch_once(&f, &c).await;
        assert_eq!(
            seen.at(Site::OriginalNotNow),
            vec![("source_held".to_string(), None)],
            "the bound original's check answered not now, once"
        );
        let admitted = seen.at(Site::OriginalAdmitted);
        assert_eq!(
            admitted.len(),
            2,
            "bound again after the wait: {admitted:?}"
        );
        assert_eq!(admitted[0].1, None, "no durable effect before the not now");
        assert!(
            started_once(&observed),
            "started once after the release: {}",
            serde_json::to_string(&observed).unwrap()
        );
        assert_eq!(
            f.inputs().await.unwrap(),
            3,
            "the launch input was admitted once"
        );
    }))
    .catch_unwind()
    .await;
    clear_probe().await;
    let cleanup = f.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    match result {
        Ok(result) => result.expect("bounded launch control"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

/// How much tool time the dispatch of [`a_launch_with_little_tool_time_left_waits_only_within_it`]
/// has left.
const TOOL_TIME_LEFT: Duration = Duration::from_millis(300);

/// A launch dispatched with little tool time left waits out its source held at
/// the admitted-custody check only within that time. A planning original
/// already bound, sent again while its source is held far past the dispatch's
/// deadline, answers "not now" once and then its shipped uncertain outcome
/// (`native_outcome_unavailable`, nothing started) within the dispatch's time,
/// never after Core's owner-change bound.
///
/// RED before the fix: the launch's binder waited Core's whole bound.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_launch_with_little_tool_time_left_waits_only_within_it() {
    let f = FirstFixture::for_continuation().await;
    let result = AssertUnwindSafe(tokio::time::timeout(GUARD, async {
        let c = prepare(&f).await;
        assert_eq!(
            f.inputs().await.unwrap(),
            2,
            "A and independent P are admitted"
        );
        let first_seen = hold_at(Site::OriginalPrepare);
        let first = launch_once(&f, &c).await;
        clear_probe().await;
        assert!(
            matches!(first, WorkflowLaunchObservation::NotAdmitted),
            "the first launch ended without admission: {}",
            serde_json::to_string(&first).unwrap()
        );
        assert!(first_seen.at(Site::OriginalNotNow).is_empty());

        // The source is held well past the dispatch's deadline and past
        // Core's owner-change bound.
        let seen = hold_at_for(
            Site::OriginalAdmitted,
            meerkat_core::authorization::OWNER_CHANGE_WAIT_BOUND + Duration::from_secs(1),
        );
        let prepared = Arc::new(PreparedWorkflowLaunch {
            selection: c.planning.clone(),
            reference: c.snapshot.reference().clone(),
        });
        let dispatch = ToolDispatchContext::default()
            .with_work_authorization(c.a_dispatch.work_authorization().cloned());
        let started = tokio::time::Instant::now();
        let observed = prepared
            .submit_with_review(
                &f.source,
                ReviewCustody::default(),
                Some((&dispatch, started + TOOL_TIME_LEFT)),
            )
            .await
            .unwrap();
        let answered = started.elapsed();
        assert!(
            answered < TOOL_TIME_LEFT + Duration::from_millis(700),
            "the launch answered within its dispatch's time: {answered:?}"
        );
        assert!(
            matches!(
                observed,
                WorkflowLaunchObservation::Native {
                    observation: PrivateOriginalSubmission::NativeOutcomeUnavailable
                }
            ),
            "the shipped uncertain outcome: {}",
            serde_json::to_string(&observed).unwrap()
        );
        assert_eq!(
            seen.at(Site::OriginalNotNow),
            vec![("source_held".to_string(), None)],
            "the bound original's check answered not now, once"
        );
        assert_eq!(f.inputs().await.unwrap(), 2, "nothing started");
    }))
    .catch_unwind()
    .await;
    clear_probe().await;
    let cleanup = f.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    match result {
        Ok(result) => result.expect("bounded launch control"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

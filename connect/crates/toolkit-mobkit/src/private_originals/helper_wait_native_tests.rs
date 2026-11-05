//! The Find dispatch's wait before each rebind of its helper, on the actual
//! native owner. The helper's first native admission finds a real owner change
//! of the same source publication in progress (begun after its genuine policy
//! read and held for a while from another thread), and its return is the real
//! same-session PolicyChanged. The dispatch tool time is emulated (Core sets
//! it only inside its own fenced dispatch); the source deadline stays live
//! throughout. A change that outlasts the tool time, or no tool time left at
//! all, stops the dispatch at ReprepareRequired after one native call; one
//! that settles within it lets a rebind go with the same input, after each
//! settled wait of the dispatch's one churn. That churn's one deadline is
//! never re-armed: changes that each settle within it but together outlast
//! it stop the dispatch at the wait that meets it. A contributing login
//! retired while the dispatch waits is refused at the next native call's
//! final entry, and that refusal is never rebound. Each control names the
//! exact stop, and checks that every native call carried the same input and
//! that the completed preparation and its effects stayed unchanged.
//! Without the source's custody recovery the
//! helper's first candidate is unenrolled, and its guard, dropped at the
//! refusal, takes the publication writer and so waits out a change still in
//! progress before the dispatch's own wait. The controls that stop at a wait
//! meeting a change therefore select the recovery, as the completed
//! host does: the candidate is enrolled and retained, the dispatch's wait
//! meets the change in progress, and the joined recovery withdraws the
//! candidate. Its task is held before its first round until the dispatch has
//! answered, as the fixture's other recovery controls hold it, so recovery
//! rounds do not interleave with this foreground.
use super::*;
use crate::private_originals::custody_recovery::HandoffPhase;
use crate::private_originals::governed_submit::SourcePublication;

/// How a helper wait control's dispatch ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WaitEnd {
    /// The native call after the last held change admits.
    Admitted,
    /// The dispatch stops for exactly this reason after its last native
    /// call, which met a held change.
    Stopped(setup_helpers::HelperRebindStop),
    /// The last native call meets no change and is refused at its final
    /// entry; nothing rebinds after it.
    RefusedAtEntry,
}

/// One helper wait control.
struct WaitControl {
    /// How long each held owner change is held.
    hold: Duration,
    /// The dispatch's emulated tool time.
    tool_time: Duration,
    /// How many native calls, from the first, each meet a held change.
    changes: usize,
    /// Whether the source's custody recovery is selected.
    selected: bool,
    /// Retire the contributing login once this many native calls returned,
    /// while the dispatch waits out the change the last of them met.
    retire_after: Option<usize>,
    /// The native calls the dispatch makes.
    calls: usize,
    end: WaitEnd,
}

async fn exercise_helper_wait(control: WaitControl) {
    let admitted = control.end == WaitEnd::Admitted;
    let stop = match control.end {
        WaitEnd::Stopped(stop) => Some(stop),
        WaitEnd::Admitted | WaitEnd::RefusedAtEntry => None,
    };
    let selected = control.selected;
    let (_connection, logins, login) = enrolled_login();
    let script = acquisition_denial::Script::paused_for_helper_entry();
    let mode = if control.changes == 1 {
        DeliveryMode::HelperPolicyChangeHeld(control.hold)
    } else {
        DeliveryMode::HelperPolicyChangesHeld(control.changes, control.hold)
    };
    let fault = Arc::new(DeliveryFault::new(mode));
    let (fixture, history) = Fixture::for_helper_entry(script.clone(), fault.clone()).await;
    let mut first_round = selected.then(|| fixture.source.hold_first_custody_round_for_test());
    let custody = std::sync::Mutex::new(None);
    let deadline = tokio::time::Instant::now() + WAIT * 3;
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout_at(
        deadline,
        Box::pin(async {
            if selected {
                // Select the real source task before either first input.
                fixture
                    .source
                    .start_custody_recovery(
                        fixture.native.mob_runtime().continuation_source_announcer(),
                    )
                    .unwrap();
            }
            let helper_session = fault.helper_session.get().unwrap().clone();
            let (_prepared, _parent) = acquisition_approved::select_reviewed_setup(&fixture).await;
            let first = fixture.submit().await;
            let Ok(PrivateOriginalSubmission::Admitted {
                deduplicated: false,
                source_settlement: PrivateSourceSettlement::FreshAcceptancePromoted,
                ..
            }) = &first
            else {
                panic!(
                    "actual Original admission required: {}",
                    submission_context(&first)
                );
            };
            script.first_response_waiting().await;
            let binding = fixture.binding().await.unwrap().unwrap();
            retain_actual_a_login(&fixture, &binding, &login);
            script.release_first_response();
            fault.entered.notified().await;
            assert_eq!(fault.helper_returns.load(Ordering::SeqCst), 1);
            assert_eq!(
                fault.helper_policy_changed.load(Ordering::SeqCst),
                1,
                "the actual native first return must be same-session PolicyChanged"
            );
            assert_eq!(
                fault.publication_changes.load(Ordering::SeqCst),
                1,
                "one real owner change is held"
            );
            // The issued helper's grant checks would observe the held change;
            // the reprepare controls already cover them. Take the one helper.
            let held = {
                let helpers = fixture.source.helper_sources.lock().unwrap();
                assert_eq!(helpers.len(), 1, "one actual issued helper source");
                helpers[0].clone()
            };
            assert_eq!(
                held.original()
                    .association()
                    .candidate()
                    .target
                    .context
                    .as_str(),
                helper_session.to_string()
            );
            let input_before = fault.helper_inputs.lock().unwrap().clone();
            assert_eq!(input_before.len(), 1);
            let acquisitions_before = fixture
                .store
                .list_records(&acquisition_approved::scope(&fixture), None, 10)
                .await
                .unwrap();
            assert_eq!(
                acquisitions_before.len(),
                1,
                "the completed preparation remains owned by one canonical acquisition"
            );
            let effects_before = fixture
                .store
                .list_effects(&acquisitions_before[0].key, None, 16)
                .await
                .unwrap();
            assert!(
                effects_before.len() < 16,
                "the bounded effect inventory must be complete"
            );
            // Bound the dispatch's tool time before it sees its refusal; the
            // source deadline is far later.
            assert!(
                fixture
                    .source
                    .helper_tool_deadline_for_test
                    .set(tokio::time::Instant::now() + control.tool_time)
                    .is_ok()
            );
            let operation_deadline = fixture
                .source
                .helper_operation_deadline(held.original().association())
                .unwrap()
                .unwrap();
            // A change still in progress after the dispatch stopped would
            // reach the Original's turn: end it exactly at the stop.
            let ending = fault.clone();
            *fixture
                .source
                .helper_rebind_stopped_for_test
                .lock()
                .unwrap() = Some(Box::new(move || ending.end_held_changes()));
            fault.release.add_permits(1);
            // The dispatch waits out the change its last returned native call
            // met: retire the contributing login meanwhile. Its drain runs
            // beside the Original's turn, which owns every later bridge call.
            let retiring = async {
                let Some(after) = control.retire_after else {
                    return;
                };
                while fault.helper_returns.load(Ordering::SeqCst) < after {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
                let retirement = logins.fixture_retire(1).unwrap();
                assert!(login.check().is_err());
                let retired = retirement.await.unwrap();
                assert!(retired.fenced && retired.reservations_drained && !retired.uncertain);
            };
            let (finished, ()) = tokio::join!(fixture.finish(&first), retiring);
            finished.unwrap();
            let ending = fault.clone();
            tokio::task::spawn_blocking(move || ending.end_held_changes())
                .await
                .unwrap();
            let now_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;
            assert!(
                operation_deadline > now_ms,
                "the source deadline stayed live throughout"
            );
            let reply = tool_reply(&script);
            assert_eq!(reply["created"], true);
            let expected = match control.end {
                WaitEnd::Admitted => "input_admitted",
                WaitEnd::Stopped(_) => "retry_safe",
                WaitEnd::RefusedAtEntry => "native_outcome_unavailable",
            };
            assert_eq!(reply["worker"]["state"], expected, "{reply}");
            // Each held change refuses one native call; a rebind follows each
            // settled wait. The call after the last change admits, unless the
            // dispatch stopped first or that call was refused at its entry.
            let inputs = fault.helper_inputs.lock().unwrap().clone();
            assert_eq!(inputs.len(), control.calls, "native calls");
            assert_eq!(fault.helper_returns.load(Ordering::SeqCst), control.calls);
            for input in &inputs {
                assert_eq!(
                    input, &input_before[0],
                    "one actual Input ID, content, authority, timestamp and event key"
                );
            }
            let changed = if stop.is_some() {
                control.calls
            } else {
                control.calls - 1
            };
            assert_eq!(fault.helper_policy_changed.load(Ordering::SeqCst), changed);
            assert_eq!(
                *fixture.source.helper_rebind_stops_for_test.lock().unwrap(),
                Vec::from_iter(stop),
                "the dispatch stopped for exactly this reason, or did not stop"
            );
            assert_eq!(fault.accepted.load(Ordering::SeqCst), usize::from(admitted));
            let rows =
                acquisition_approved::native_history(history.as_ref(), &helper_session).await;
            assert_eq!(rows.len(), usize::from(admitted));
            assert!(
                held.handoffs_for_test().is_empty(),
                "definite refusal or successful acceptance consumes its Box"
            );
            assert_eq!(
                fixture
                    .store
                    .list_records(&acquisition_approved::scope(&fixture), None, 10)
                    .await
                    .unwrap(),
                acquisitions_before,
                "rebinding does not create or replace the completed preparation"
            );
            assert_eq!(
                fixture
                    .store
                    .list_effects(&acquisitions_before[0].key, None, 16)
                    .await
                    .unwrap(),
                effects_before,
                "same succeeded effect identity, attempt, lease, request and receipt after helper attempts"
            );
            // With the recovery selected the first candidate is enrolled and
            // stays retained (a stopped dispatch released its binder, so its
            // foreground handed it to recovery); without it, it is not retained.
            let retained = held.first_custody.lock().unwrap().clone();
            if selected {
                let retained = retained.expect("the helper's enrolled first candidate is retained");
                assert!(retained.handoff.enrolled());
                if !admitted {
                    assert!(retained.handoff.phase() != HandoffPhase::Foreground);
                }
                *custody.lock().unwrap() = Some(Arc::downgrade(&retained));
            } else {
                assert!(
                    retained.is_none(),
                    "an unenrolled first candidate is not retained"
                );
            }
            assert_eq!(fixture.source.helper_sources.lock().unwrap().len(), 1);
        }),
    ))
    .catch_unwind()
    .await;
    // These owners live outside the cancellable body. Release the held round,
    // the script and the bridge, then join native and the source's recovery
    // on every exit.
    drop(first_round.take());
    script.release_first_response();
    fault.release.add_permits(1);
    if selected {
        fixture.source.stop_custody_recovery();
    }
    let cleanup = fixture.shutdown().await;
    let recovery = fixture.source.join_custody_recovery().await;
    assert_eq!(fault.publication_change_failures.load(Ordering::SeqCst), 0);
    assert!(cleanup.is_ok(), "{cleanup:?}");
    assert!(recovery.is_ok(), "{recovery:?}");
    match checked {
        Ok(Ok(())) => {}
        Ok(Err(_)) => panic!("actual helper wait control timed out"),
        Err(payload) => std::panic::resume_unwind(payload),
    }
    // The recovery joined: it withdrew the stopped dispatch's retained
    // candidate, which native never admitted, and settled its source attempt.
    let custody = custody.lock().unwrap().take();
    assert_eq!(
        custody.is_some(),
        selected,
        "a first candidate is retained exactly when the recovery is selected"
    );
    if let Some(custody) = custody {
        // Only a control whose dispatch stops selects the recovery.
        assert!(stop.is_some());
        let pending = custody
            .upgrade()
            .expect("the helper source still retains its first candidate");
        assert!(pending.handoff.phase() == HandoffPhase::Done);
        let publication = pending.publication.lock().unwrap();
        assert!(publication.native_admission.is_none());
        assert!(
            matches!(
                &publication.source,
                SourcePublication::Settled {
                    source_acknowledged: false,
                    result: Err(error),
                } if error == "Toolkit permanently withdrew this source attempt"
            ),
            "the joined recovery withdrew the helper's refused source attempt"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn helper_change_outlasting_the_tool_time_stops_after_one_native_call() {
    Box::pin(exercise_helper_wait(WaitControl {
        hold: Duration::from_secs(5),
        tool_time: Duration::from_millis(300),
        changes: 1,
        selected: true,
        retire_after: None,
        calls: 1,
        end: WaitEnd::Stopped(setup_helpers::HelperRebindStop::WaitUnsettled),
    }))
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn helper_with_no_tool_time_left_stops_after_one_native_call() {
    Box::pin(exercise_helper_wait(WaitControl {
        hold: Duration::from_millis(300),
        tool_time: Duration::ZERO,
        changes: 1,
        selected: false,
        retire_after: None,
        calls: 1,
        end: WaitEnd::Stopped(setup_helpers::HelperRebindStop::NoBudget),
    }))
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn helper_change_settling_within_the_tool_time_rebinds_once() {
    Box::pin(exercise_helper_wait(WaitControl {
        hold: Duration::from_secs(1),
        tool_time: Duration::from_secs(10),
        changes: 1,
        selected: false,
        retire_after: None,
        calls: 2,
        end: WaitEnd::Admitted,
    }))
    .await;
}

// Two changes in a row, each held 300 ms and each settling within the
// dispatch's bounds: the submission rebinds after each settled wait of its one
// churn, with the same input, and the third native call admits.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn helper_changes_settling_in_a_row_rebind_after_each_wait() {
    Box::pin(exercise_helper_wait(WaitControl {
        hold: Duration::from_millis(300),
        tool_time: Duration::from_secs(10),
        changes: 2,
        selected: false,
        retire_after: None,
        calls: 3,
        end: WaitEnd::Admitted,
    }))
    .await;
}

// Two changes in a row, each held 1.5 s: each alone settles within the
// churn's one deadline (Core's 2 s bound from its first wait; the tool time is
// far later), but together they outlast it. The first wait settles and the
// submission rebinds; the second wait meets the deadline the first armed,
// never re-armed, and the dispatch stops there after two native calls.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn helper_changes_outlasting_one_churn_deadline_stop_at_the_second_wait() {
    Box::pin(exercise_helper_wait(WaitControl {
        hold: Duration::from_millis(1500),
        tool_time: Duration::from_secs(10),
        changes: 2,
        selected: true,
        retire_after: None,
        calls: 2,
        end: WaitEnd::Stopped(setup_helpers::HelperRebindStop::WaitUnsettled),
    }))
    .await;
}

// Two changes in a row, each held 600 ms. The contributing login is retired
// once the second native call returned, while the dispatch waits out the
// change it met. That wait settles and the submission rebinds once more with
// the same input; the third native call is refused at its final entry, and
// the submission stops at that refusal: no fourth call, nothing admitted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn helper_login_retired_during_a_later_wait_is_refused_at_the_next_entry() {
    Box::pin(exercise_helper_wait(WaitControl {
        hold: Duration::from_millis(600),
        tool_time: Duration::from_secs(10),
        changes: 2,
        selected: false,
        retire_after: Some(2),
        calls: 3,
        end: WaitEnd::RefusedAtEntry,
    }))
    .await;
}

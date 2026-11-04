//! The Find dispatch's wait before its helper's one rebind, on the actual
//! native owner. The helper's first native admission finds a real owner change
//! of the same source publication in progress (begun after its genuine policy
//! read and held for a while from another thread), and its return is the real
//! same-session PolicyChanged. The dispatch tool time is emulated (Core sets
//! it only inside its own fenced dispatch); the source deadline stays live
//! throughout. A change that outlasts the tool time, or no tool time left at
//! all, stops the dispatch at ReprepareRequired after one native call; one
//! that settles within it lets the one rebind go with the same input. Each
//! control names the exact stop. Without the source's custody recovery the
//! helper's first candidate is unenrolled, and its guard, dropped at the
//! refusal, takes the publication writer and so waits out a change still in
//! progress before the dispatch's own wait. The control for a change
//! outlasting the tool time therefore selects the recovery, as the completed
//! host does: the candidate is enrolled and retained, the dispatch's wait
//! meets the change in progress, and the joined recovery withdraws the
//! candidate. Its task is held before its first round until the dispatch has
//! answered, as the fixture's other recovery controls hold it, so recovery
//! rounds do not interleave with this foreground.
use super::*;
use crate::private_originals::custody_recovery::HandoffPhase;
use crate::private_originals::governed_submit::SourcePublication;

async fn exercise_helper_wait(
    hold: Duration,
    tool_time: Duration,
    stop: Option<setup_helpers::HelperRebindStop>,
    selected: bool,
) {
    let rebind = stop.is_none();
    let (_connection, _logins, login) = enrolled_login();
    let script = acquisition_denial::Script::paused_for_helper_entry();
    let fault = Arc::new(DeliveryFault::new(DeliveryMode::HelperPolicyChangeHeld(
        hold,
    )));
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
            // Bound the dispatch's tool time before it sees its refusal; the
            // source deadline is far later.
            assert!(
                fixture
                    .source
                    .helper_tool_deadline_for_test
                    .set(tokio::time::Instant::now() + tool_time)
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
            fixture.finish(&first).await.unwrap();
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
            let expected = if rebind {
                "input_admitted"
            } else {
                "retry_safe"
            };
            assert_eq!(reply["worker"]["state"], expected, "{reply}");
            let attempts = if rebind { 2 } else { 1 };
            let inputs = fault.helper_inputs.lock().unwrap().clone();
            assert_eq!(inputs.len(), attempts, "native calls");
            assert_eq!(fault.helper_returns.load(Ordering::SeqCst), attempts);
            for input in &inputs {
                assert_eq!(
                    input, &input_before[0],
                    "one actual Input ID, content, authority, timestamp and event key"
                );
            }
            assert_eq!(fault.helper_policy_changed.load(Ordering::SeqCst), 1);
            assert_eq!(
                *fixture.source.helper_rebind_stops_for_test.lock().unwrap(),
                Vec::from_iter(stop),
                "the dispatch stopped for exactly this reason, or rebound"
            );
            assert_eq!(fault.accepted.load(Ordering::SeqCst), usize::from(rebind));
            let rows =
                acquisition_approved::native_history(history.as_ref(), &helper_session).await;
            assert_eq!(rows.len(), usize::from(rebind));
            assert!(
                held.handoffs_for_test().is_empty(),
                "definite refusal or successful acceptance consumes its Box"
            );
            // With the recovery selected the first candidate is enrolled and
            // stays retained (a stopped dispatch released its binder, so its
            // foreground handed it to recovery); without it, it is not retained.
            let retained = held.first_custody.lock().unwrap().clone();
            if selected {
                let retained = retained.expect("the helper's enrolled first candidate is retained");
                assert!(retained.handoff.enrolled());
                if !rebind {
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
        assert!(!rebind);
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
    Box::pin(exercise_helper_wait(
        Duration::from_secs(5),
        Duration::from_millis(300),
        Some(setup_helpers::HelperRebindStop::WaitUnsettled),
        true,
    ))
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn helper_with_no_tool_time_left_stops_after_one_native_call() {
    Box::pin(exercise_helper_wait(
        Duration::from_millis(300),
        Duration::ZERO,
        Some(setup_helpers::HelperRebindStop::NoBudget),
        false,
    ))
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn helper_change_settling_within_the_tool_time_rebinds_once() {
    Box::pin(exercise_helper_wait(
        Duration::from_secs(1),
        Duration::from_secs(10),
        None,
        false,
    ))
    .await;
}

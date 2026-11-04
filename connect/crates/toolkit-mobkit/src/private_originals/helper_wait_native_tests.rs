//! The Find dispatch's wait before its helper's one rebind, on the actual
//! native owner. The helper's first native admission finds a real owner change
//! of the same source publication in progress (begun after its genuine policy
//! read and held for a while from another thread), and its return is the real
//! same-session PolicyChanged. The dispatch tool time is emulated (Core sets
//! it only inside its own fenced dispatch); the source deadline stays live
//! throughout. A change that outlasts the tool time, or no tool time left at
//! all, stops the dispatch at ReprepareRequired after one native call; one
//! that settles within it lets the one rebind go with the same input. (Release
//! paths that take the publication writer wait for the change, so the time of
//! the reply is not the evidence: the native calls and the outcome are.)
use super::*;
use crate::private_originals::custody_recovery::HandoffPhase;

async fn exercise_helper_wait(hold: Duration, tool_time: Duration, rebind: bool) {
    let (_connection, _logins, login) = enrolled_login();
    let script = acquisition_denial::Script::paused_for_helper_entry();
    let fault = Arc::new(DeliveryFault::new(DeliveryMode::HelperPolicyChangeHeld(
        hold,
    )));
    let (fixture, history) = Fixture::for_helper_entry(script.clone(), fault.clone()).await;
    let deadline = tokio::time::Instant::now() + WAIT * 3;
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout_at(
        deadline,
        Box::pin(async {
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
            fault.release.add_permits(1);
            fixture.finish(&first).await.unwrap();
            let changes = std::mem::take(&mut *fault.held_changes.lock().unwrap());
            for change in changes {
                tokio::task::spawn_blocking(move || change.join())
                    .await
                    .unwrap()
                    .unwrap();
            }
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
            assert_eq!(fault.accepted.load(Ordering::SeqCst), usize::from(rebind));
            let rows =
                acquisition_approved::native_history(history.as_ref(), &helper_session).await;
            assert_eq!(rows.len(), usize::from(rebind));
            assert!(
                held.handoffs_for_test().is_empty(),
                "definite refusal or successful acceptance consumes its Box"
            );
            if !rebind {
                // The stopped dispatch released its binder: recovery settles the
                // helper's retained first custody by withdrawal.
                tokio::time::timeout(WAIT, async {
                    loop {
                        let done = held
                            .first_custody
                            .lock()
                            .unwrap()
                            .as_ref()
                            .is_none_or(|pending| pending.handoff.phase() == HandoffPhase::Done);
                        if done {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                })
                .await
                .expect("the helper's refused source attempt is withdrawn");
            }
            assert_eq!(fixture.source.helper_sources.lock().unwrap().len(), 1);
        }),
    ))
    .catch_unwind()
    .await;
    script.release_first_response();
    fault.release.add_permits(1);
    let cleanup = fixture.shutdown().await;
    assert_eq!(fault.publication_change_failures.load(Ordering::SeqCst), 0);
    assert!(cleanup.is_ok(), "{cleanup:?}");
    match checked {
        Ok(Ok(())) => {}
        Ok(Err(_)) => panic!("actual helper wait control timed out"),
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn helper_change_outlasting_the_tool_time_stops_after_one_native_call() {
    Box::pin(exercise_helper_wait(
        Duration::from_secs(3),
        Duration::from_millis(300),
        false,
    ))
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn helper_with_no_tool_time_left_stops_after_one_native_call() {
    Box::pin(exercise_helper_wait(
        Duration::from_secs(1),
        Duration::ZERO,
        false,
    ))
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn helper_change_settling_within_the_tool_time_rebinds_once() {
    Box::pin(exercise_helper_wait(
        Duration::from_secs(1),
        Duration::from_secs(10),
        true,
    ))
    .await;
}

//! Delivery-identity terminal wait over the real autonomous inbox path.
//!
//! Deliveries go through the ordinary generic submit, so they reach the
//! member's comms inbox and become `ExternalEvent` runtime inputs when the
//! member drains it; nothing here admits a completion-bearing prompt.

use super::*;
use crate::runtime::{
    DeliveryNotTerminalCause, DeliveryTerminalResolution, DeliveryTerminalWait,
    DeliveryTerminalWaitError, DeliveryTerminalWaitReport, DeliveryUnknownCause,
};
use meerkat_core::lifecycle::InputId;
use meerkat_runtime::terminal_status::{
    InputTerminalReceiptRead, InteractionSelector, TerminalWitnessSource,
};

fn bound() -> BoundedResultSpec {
    BoundedResultSpec::new("delivery answer", 1024).expect("result bound")
}

async fn wait_delivery(
    fixture: &Fixture,
    delivery: &MobDeliveryIdentity,
    spec: &BoundedResultSpec,
    until: std::time::Instant,
) -> DeliveryTerminalWaitReport {
    tokio::time::timeout(
        WAIT + Duration::from_secs(5),
        fixture
            .handle
            .wait_bounded_work_for_identity_with_delivery_identity(
                &fixture.entry.agent_identity,
                delivery,
                spec,
                until,
            ),
    )
    .await
    .expect("the wait honours its deadline")
    .expect("delivery wait runs")
}

/// Wait until the delivery is admitted as a runtime input and return its
/// input id. The runtime's admission signal is the wake, not a re-read with
/// short deadlines: one short wait can end before its first evidence read
/// under load (`Unknown { NotObservedByDeadline }`). The member's turn is
/// held, so the admitted input is still owed a terminal, which one typed
/// read confirms.
async fn admitted_pending(fixture: &Fixture, delivery: &MobDeliveryIdentity) -> InputId {
    let adapter = fixture
        .service
        .acquire_runtime_adapter(None)
        .expect("acquire runtime authority")
        .expect("runtime owner");
    // The admission is the signal; the deadline only bounds a broken run.
    let input_id = tokio::time::timeout(
        WAIT,
        adapter
            .wait_input_admitted_by_idempotency_key(&fixture.session_id, &delivery.idempotency_key),
    )
    .await
    .expect("delivery is admitted from the inbox")
    .expect("admission wait runs")
    .expect("the member session is live");
    let read = adapter
        .input_terminal_receipt(
            &fixture.session_id,
            InteractionSelector::InputId(input_id.clone()),
        )
        .await
        .expect("receipt read runs")
        .expect("the admitted input is readable");
    assert!(
        matches!(
            read.report,
            InputTerminalReceiptRead::Pending { terminal: None, .. }
        ),
        "the admitted delivery is still owed a terminal: {:?}",
        read.report
    );
    input_id
}

struct ReceiptView<'a> {
    input_id: &'a InputId,
    run_id: &'a meerkat_core::lifecycle::RunId,
    recipients: &'a [InputId],
    result: &'a BoundedTurnResult,
    source: TerminalWitnessSource,
}

fn receipt(report: &DeliveryTerminalWaitReport) -> ReceiptView<'_> {
    let DeliveryTerminalWait::Terminal(record) = report.work() else {
        panic!("expected a terminal delivery, got {:?}", report.work());
    };
    assert_eq!(
        record.terminal(),
        &meerkat_runtime::InputTerminalOutcome::Consumed
    );
    let DeliveryTerminalResolution::Receipt {
        runtime_run_id: Some(run_id),
        owner_input_id,
        recipient_input_ids,
        result,
    } = record.resolution()
    else {
        panic!("expected a run receipt, got {:?}", record.resolution());
    };
    assert_eq!(owner_input_id, &recipient_input_ids[0]);
    assert!(recipient_input_ids.contains(record.input_id()));
    ReceiptView {
        input_id: record.input_id(),
        run_id,
        recipients: recipient_input_ids,
        result: result.as_ref().expect("the run completed with a result"),
        source: record.witness_source(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn autonomous_delivery_returns_its_own_run_receipt_and_bounded_answer() {
    let fixture = Fixture::new().await;
    let requests_before = fixture.client.requests().len();
    let key = delivery("terminal-single");
    let receipt_ack = fixture
        .submit_generic(
            WorkSpec::new("single delivery", WorkOrigin::External)
                .with_interaction_id(interaction(&key)),
            key.clone(),
        )
        .await;
    assert_eq!(receipt_ack.runtime_id, fixture.entry.agent_runtime_id);

    let small = BoundedResultSpec::new("small", 15).expect("small bound");
    let report = wait_delivery(&fixture, &key, &small, deadline()).await;
    assert!(matches!(
        report.member(),
        Some(DurableBoundedMemberState::Active { session_id }) if session_id == &fixture.session_id
    ));
    let view = receipt(&report);
    let admitted = fixture.input_for_delivery(&key).await;
    assert_eq!(view.input_id, &admitted.state().state.input_id);
    assert_eq!(view.recipients, std::slice::from_ref(view.input_id));
    assert_eq!(view.result.session_id(), &fixture.session_id);
    assert_eq!(view.result.result().label(), "small");
    assert_eq!(
        view.result.result().status(),
        BoundedHelperResultStatus::CompletedTruncated
    );
    assert_eq!(
        view.result.result().text(),
        format!("exe{HELPER_RESULT_TRUNCATION_MARKER}")
    );
    assert_eq!(fixture.client.requests().len(), requests_before + 1);

    // The runtime's own durable rows answer the same way to a fresh machine
    // over the same store, with no session registered: restart honesty.
    let restarted = meerkat_runtime::MeerkatMachine::persistent(
        Arc::new(
            meerkat_runtime::SqliteRuntimeStore::new_head_canonical(
                fixture.root.path().join("realm.db"),
            )
            .expect("second view of the canonical runtime store"),
        ),
        Arc::new(meerkat_store::MemoryBlobStore::new()),
    )
    .expect("construct runtime authority");
    let durable = restarted
        .input_terminal_receipt(
            &fixture.session_id,
            InteractionSelector::IdempotencyKey(key.idempotency_key.clone()),
        )
        .await
        .expect("durable read without registration")
        .expect("durable delivery input");
    assert_eq!(durable.source, TerminalWitnessSource::DurableStore);
    let InputTerminalReceiptRead::Finalized(durable) = durable.report else {
        panic!("expected a finalized durable receipt");
    };
    assert_eq!(durable.input_id(), view.input_id);
    assert_eq!(durable.run_id(), Some(view.run_id));
    assert_eq!(durable.recipient_input_ids(), view.recipients);
    match durable.outcome() {
        meerkat_runtime::completion::CompletionOutcome::Completed(result) => {
            assert_eq!(
                result.text,
                format!("executor-answer-{}", requests_before + 1)
            );
        }
        other => panic!("expected the completed run result, got {other:?}"),
    }
    fixture.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn batched_autonomous_deliveries_report_one_shared_run_and_their_batch() {
    let fixture = Fixture::new().await;
    let before = fixture.durable().await;
    let requests_before = fixture.client.requests().len();
    fixture.client.block();
    let head = delivery("terminal-head");
    fixture
        .submit_generic(
            WorkSpec::new("head delivery", WorkOrigin::External),
            head.clone(),
        )
        .await;
    fixture.client.wait_for_requests(requests_before + 1).await;

    let x = delivery("terminal-x");
    let y = delivery("terminal-y");
    const X_TEXT: &str = "first batched delivery";
    fixture
        .submit_generic(
            WorkSpec::new(X_TEXT, WorkOrigin::External).with_interaction_id(interaction(&x)),
            x.clone(),
        )
        .await;
    fixture
        .submit_generic(
            WorkSpec::new("second batched delivery", WorkOrigin::External)
                .with_interaction_id(interaction(&y)),
            y.clone(),
        )
        .await;
    let x_input = admitted_pending(&fixture, &x).await;
    let y_input = admitted_pending(&fixture, &y).await;

    // Autonomous semantics are unchanged: completion-bearing admission is
    // still refused for this member.
    let refused = fixture
        .handle
        .start_work_with_mode(
            fixture.entry.agent_runtime_id.clone(),
            fixture.entry.fence_token,
            WorkRef::new(),
            WorkSpec::new("still refused", WorkOrigin::Internal),
            HandlingMode::Queue,
        )
        .await
        .expect_err("autonomous members keep refusing tracked completion");
    assert!(
        matches!(refused, MobError::UnsupportedForMode { .. }),
        "unexpected refusal {refused:?}"
    );

    // Waiting reads only: no input row and no mob event is added by it.
    let inputs_before_wait = fixture.input_count().await;
    let events_before_wait = fixture
        .handle
        .events()
        .replay_all()
        .await
        .expect("mob events")
        .len();
    let spawn_wait = |delivery: MobDeliveryIdentity| {
        let handle = fixture.handle.clone();
        let identity = fixture.entry.agent_identity.clone();
        tokio::spawn(async move {
            handle
                .wait_bounded_work_for_identity_with_delivery_identity(
                    &identity,
                    &delivery,
                    &bound(),
                    deadline(),
                )
                .await
        })
    };
    let x_wait = spawn_wait(x.clone());
    let y_wait = spawn_wait(y.clone());
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!x_wait.is_finished() && !y_wait.is_finished());
    assert_eq!(fixture.input_count().await, inputs_before_wait);

    fixture.client.release();
    let x_report = tokio::time::timeout(WAIT, x_wait)
        .await
        .expect("x waiter finishes")
        .expect("x waiter joins")
        .expect("x wait runs");
    let y_report = tokio::time::timeout(WAIT, y_wait)
        .await
        .expect("y waiter finishes")
        .expect("y waiter joins")
        .expect("y wait runs");
    let x_view = receipt(&x_report);
    let y_view = receipt(&y_report);
    assert_eq!(x_view.input_id, &x_input);
    assert_eq!(y_view.input_id, &y_input);
    // The live row may already be archived to the store once its durable
    // obligations close; either witness carries the same receipt.
    assert_eq!(x_view.source, y_view.source);
    assert_eq!(x_view.run_id, y_view.run_id, "one run answered both");
    let mut expected_batch = vec![x_input.clone(), y_input.clone()];
    expected_batch.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(x_view.recipients, expected_batch.as_slice());
    assert_eq!(y_view.recipients, expected_batch.as_slice());
    let batch_answer = format!("executor-answer-{}", requests_before + 2);
    assert_eq!(x_view.result.result().text(), batch_answer);
    assert_eq!(y_view.result.result().text(), batch_answer);
    assert_eq!(fixture.client.requests().len(), requests_before + 2);

    assert_eq!(fixture.input_count().await, inputs_before_wait);
    assert_eq!(
        fixture
            .handle
            .events()
            .replay_all()
            .await
            .expect("mob events")
            .len(),
        events_before_wait
    );
    let durable = fixture.durable().await;
    let appended = &durable.messages()[before.messages().len()..];
    assert_eq!(
        external_event_sources(appended, X_TEXT),
        ["rpc"],
        "the delivery still commits as an ExternalEvent notice"
    );
    assert!(
        !appended
            .iter()
            .any(|message| matches!(message, Message::User(user) if user.text_content() == X_TEXT)),
        "waiting must not turn the notice into a user prompt"
    );
    fixture.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn elapsed_wait_reports_not_terminal_and_a_later_wait_returns_the_terminal() {
    let fixture = Fixture::new().await;
    let requests_before = fixture.client.requests().len();
    fixture.client.block();
    let key = delivery("terminal-slow");
    fixture
        .submit_generic(
            WorkSpec::new("slow delivery", WorkOrigin::External),
            key.clone(),
        )
        .await;
    fixture.client.wait_for_requests(requests_before + 1).await;

    let started = std::time::Instant::now();
    let report = wait_delivery(
        &fixture,
        &key,
        &bound(),
        started + Duration::from_millis(600),
    )
    .await;
    let elapsed = started.elapsed();
    let DeliveryTerminalWait::NotTerminal {
        input_id,
        terminal: None,
        attempt_count,
        cause:
            DeliveryNotTerminalCause::DeadlineElapsed | DeliveryNotTerminalCause::EvidenceReadTimedOut,
        ..
    } = report.work()
    else {
        panic!("expected a pending delivery, got {:?}", report.work());
    };
    assert_eq!(*attempt_count, 1);
    // The wait runs until the deadline less the 100 ms evidence floor, not
    // until a quarter of the budget is left.
    assert!(elapsed >= Duration::from_millis(500), "waited {elapsed:?}");
    assert!(
        elapsed < Duration::from_millis(600) + Duration::from_millis(400),
        "returned by the deadline: {elapsed:?}"
    );
    assert!(elapsed < Duration::from_secs(5), "waited {elapsed:?}");
    let input_id = input_id.clone();

    fixture.client.release();
    let report = wait_delivery(&fixture, &key, &bound(), deadline()).await;
    let view = receipt(&report);
    assert_eq!(view.input_id, &input_id, "the first wait cancelled nothing");
    assert_eq!(
        view.result.result().text(),
        format!("executor-answer-{}", requests_before + 1)
    );
    fixture.finish().await;
}

/// #1227: settling a delivery cancels its exact runtime input through the
/// input's own run and reads the terminal that cancellation produced; a
/// settle naming an input the delivery was not admitted as is refused,
/// typed, before anything is cancelled.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn settle_cancels_the_deliverys_exact_input_and_refuses_a_foreign_one() {
    let fixture = Fixture::new().await;
    let requests_before = fixture.client.requests().len();
    fixture.client.block();
    let key = delivery("settle-exact");
    fixture
        .submit_generic(
            WorkSpec::new("delivery to settle", WorkOrigin::External),
            key.clone(),
        )
        .await;
    fixture.client.wait_for_requests(requests_before + 1).await;
    let input_id = admitted_pending(&fixture, &key).await;

    let foreign = InputId::new();
    let refused = fixture
        .handle
        .settle_delivery_input_for_identity(
            &fixture.entry.agent_identity,
            &key,
            &foreign,
            &bound(),
            deadline(),
        )
        .await
        .expect_err("a foreign input is refused");
    assert!(
        matches!(
            &refused,
            DeliveryTerminalWaitError::InputNotOfDelivery { input_id } if input_id == &foreign
        ),
        "{refused:?}"
    );

    let settled = fixture
        .handle
        .settle_delivery_input_for_identity(
            &fixture.entry.agent_identity,
            &key,
            &input_id,
            &bound(),
            deadline(),
        )
        .await
        .expect("settle the delivery");
    let DeliveryTerminalWait::Terminal(record) = settled.work() else {
        panic!("the settled delivery is terminal: {:?}", settled.work());
    };
    assert_eq!(record.input_id(), &input_id);
    assert!(
        !matches!(
            record.resolution(),
            DeliveryTerminalResolution::Receipt { result: Ok(_), .. }
        ),
        "the cancellation, not a completed turn, is the terminal: {record:?}"
    );
    fixture.client.release();
    fixture.finish().await;
}

/// The budget up to the deadline (less the evidence floor) is spent waiting:
/// a terminal that lands in the last quarter of the budget is returned, not
/// reported as an elapsed deadline.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_terminal_late_in_the_budget_is_returned_before_the_deadline() {
    let fixture = Fixture::new().await;
    let requests_before = fixture.client.requests().len();
    fixture.client.block();
    let key = delivery("terminal-late");
    fixture
        .submit_generic(
            WorkSpec::new("late delivery", WorkOrigin::External),
            key.clone(),
        )
        .await;
    fixture.client.wait_for_requests(requests_before + 1).await;

    // The terminal lands at 80% of the budget. The 3 s left after the
    // release covers the stream, the boundary commit and the waiter's read on
    // a loaded runner; 800 ms (3.2 s of 4 s) was not enough.
    let budget = Duration::from_secs(15);
    let started = std::time::Instant::now();
    let release_after = Duration::from_secs(12);
    let release = {
        let client = fixture.client.clone();
        tokio::spawn(async move {
            tokio::time::sleep(release_after.saturating_sub(started.elapsed())).await;
            client.release();
        })
    };
    let report = wait_delivery(&fixture, &key, &bound(), started + budget).await;
    let elapsed = started.elapsed();
    release.await.expect("release task joins");
    let view = receipt(&report);
    assert_eq!(
        view.result.result().text(),
        format!("executor-answer-{}", requests_before + 1)
    );
    assert!(
        elapsed >= release_after,
        "returned before the run: {elapsed:?}"
    );
    assert!(elapsed < budget, "returned by the deadline: {elapsed:?}");
    fixture.finish().await;
}

/// The member-lifecycle read is bounded by the caller's deadline too. For a
/// member the machine state does not know it replays the mob event log; a
/// replay that outlasts the deadline must not hold the call past it.
#[tokio::test]
async fn a_stalled_member_lifecycle_read_returns_by_the_deadline() {
    let events = Arc::new(super::super::FaultInjectedMobEventStore::new());
    let (handle, _service) = super::super::create_test_mob_with_events(
        with_unique_mob_id(sample_definition(), "delivery-wait-stalled-replay"),
        events.clone(),
    )
    .await;
    events.stall_replay();
    let started = std::time::Instant::now();
    let report = tokio::time::timeout(
        Duration::from_secs(5),
        handle.wait_bounded_work_for_identity_with_delivery_identity(
            &AgentIdentity::from("never-spawned-member"),
            &delivery("terminal-stalled-replay"),
            &bound(),
            started + Duration::from_millis(300),
        ),
    )
    .await
    .expect("the wait returns by its deadline despite a stalled replay")
    .expect("delivery wait runs");
    let elapsed = started.elapsed();
    events.release_replay();
    assert!(
        elapsed < Duration::from_millis(300) + Duration::from_millis(250),
        "returned {elapsed:?} after the call"
    );
    assert!(
        report.member().is_none(),
        "the member lifecycle was never read: {:?}",
        report.member()
    );
    assert!(
        matches!(
            report.work(),
            DeliveryTerminalWait::Unknown {
                cause: DeliveryUnknownCause::NotObservedByDeadline
            }
        ),
        "nothing about the delivery was observed: {:?}",
        report.work()
    );
    handle.shutdown().await.expect("mob shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unknown_delivery_reports_typed_unknown_by_deadline_and_for_retired_members() {
    let mut fixture = Fixture::new().await;
    let never_sent = delivery("terminal-never-sent");

    let started = std::time::Instant::now();
    let report = wait_delivery(
        &fixture,
        &never_sent,
        &bound(),
        started + Duration::from_millis(500),
    )
    .await;
    let elapsed = started.elapsed();
    assert!(matches!(
        report.work(),
        DeliveryTerminalWait::Unknown {
            cause: DeliveryUnknownCause::NotAdmittedByDeadline
        }
    ));
    assert!(
        elapsed >= Duration::from_millis(350),
        "an unknown key is polled until the wait slice ends: {elapsed:?}"
    );

    // A past deadline gets one read within the 100 ms evidence floor. The
    // contract names both outcomes: the read finished and found no admitted
    // input, or it could not finish within the floor (a loaded host), so
    // nothing was observed.
    let started = std::time::Instant::now();
    let report = wait_delivery(&fixture, &never_sent, &bound(), started).await;
    assert!(
        matches!(
            report.work(),
            DeliveryTerminalWait::Unknown {
                cause: DeliveryUnknownCause::NotAdmittedByDeadline
                    | DeliveryUnknownCause::NotObservedByDeadline
            }
        ),
        "past-deadline wait: {:?}",
        report.work()
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "a past deadline takes one snapshot read"
    );

    let invalid = MobDeliveryIdentity {
        idempotency_key: " padded ".to_string(),
        correlation_id: Uuid::new_v4().to_string(),
    };
    assert!(matches!(
        fixture
            .handle
            .wait_bounded_work_for_identity_with_delivery_identity(
                &fixture.entry.agent_identity,
                &invalid,
                &bound(),
                deadline(),
            )
            .await,
        Err(DeliveryTerminalWaitError::InvalidDeliveryIdentity(_))
    ));

    // Retiring the member disposes of its session, which the fixture's open
    // live context channel (with live support) would refuse.
    fixture.close_channel().await;
    tokio::time::timeout(
        WAIT,
        crate::runtime::tests::retire_to_terminal(&fixture.handle, &fixture.entry.agent_identity),
    )
    .await
    .expect("retire finishes")
    .expect("retire member");
    let started = std::time::Instant::now();
    let report = wait_delivery(&fixture, &never_sent, &bound(), deadline()).await;
    assert!(
        matches!(
            report.member(),
            Some(DurableBoundedMemberState::Retired { .. })
        ),
        "member lifecycle: {:?}",
        report.member()
    );
    assert!(matches!(
        report.work(),
        DeliveryTerminalWait::Unknown {
            cause: DeliveryUnknownCause::MemberRetired
        }
    ));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "a retired member cannot admit the delivery; the wait returns at once"
    );
    fixture.finish().await;
}

#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)]
fn delivery_wait_future_is_send(
    handle: &MobHandle,
    identity: &AgentIdentity,
    delivery: &MobDeliveryIdentity,
    spec: &BoundedResultSpec,
) {
    fn require_send<T: Send>(_: &T) {}
    let future = handle.wait_bounded_work_for_identity_with_delivery_identity(
        identity,
        delivery,
        spec,
        std::time::Instant::now(),
    );
    require_send(&future);
}

//! A durable steer whose admission was acknowledged before a model request's
//! input is fixed joins that request, also when its boundary preparation has
//! not reached the actor's boundary coordinator yet.
//!
//! Admission answers its caller once the input is committed; the boundary
//! preparation runs afterwards, in a task of its own, and reaches the
//! coordinator through the executor's boundary handle, which may await before
//! it registers anything. The executor below holds a durable preparation at
//! that handle, after the admission answered and before the registration, and
//! lets the runner reach the boundary meanwhile.
use super::*;

/// How long a row gives the runner to reach the boundary and close it while
/// the preparations are held. A runner that waits for them is still at the
/// boundary when this elapses.
const RUNNER_REACHES_THE_BOUNDARY: Duration = Duration::from_millis(250);

/// Holds durable boundary preparations before they reach the coordinator.
#[derive(Default)]
struct RegistrationLag {
    armed: AtomicBool,
    /// Durable preparations that reached the handle while the lag was armed.
    held: AtomicUsize,
    /// How many of them, oldest first, may go on to the coordinator.
    released: AtomicUsize,
    /// Durable preparations handed on to the coordinator, held or not.
    forwarded: AtomicUsize,
}

impl RegistrationLag {
    fn arm(&self) {
        self.armed.store(true, Ordering::SeqCst);
    }

    fn held(&self) -> usize {
        self.held.load(Ordering::SeqCst)
    }

    /// Let the first `count` held preparations go on to the coordinator.
    fn release_through(&self, count: usize) {
        self.released.fetch_max(count, Ordering::SeqCst);
    }

    async fn wait_held(&self, expected: usize) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.held() < expected {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{expected} durable preparations reach the boundary handle"));
    }

    /// Wait until `expected` durable preparations reached the coordinator.
    async fn wait_forwarded(&self, expected: usize) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.forwarded.load(Ordering::SeqCst) < expected {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{expected} durable preparations reach the coordinator"));
    }

    async fn hold(&self) {
        if !self.armed.load(Ordering::SeqCst) {
            return;
        }
        let ticket = self.held.fetch_add(1, Ordering::SeqCst) + 1;
        while self.released.load(Ordering::SeqCst) < ticket {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }
}

struct LaggingRegistrationBoundaryHandle {
    inner: Arc<dyn CoreExecutorBoundaryHandle>,
    lag: Arc<RegistrationLag>,
}

#[async_trait::async_trait]
impl CoreExecutorBoundaryHandle for LaggingRegistrationBoundaryHandle {
    async fn cancel_after_boundary(
        &self,
        expected_run_id: &RunId,
        reason: String,
    ) -> Result<(), CoreExecutorError> {
        self.inner
            .cancel_after_boundary(expected_run_id, reason)
            .await
    }

    async fn prepare_turn_boundary_delivery(
        &self,
        expected_run_id: &RunId,
        delivery: meerkat_core::TurnBoundaryDelivery,
    ) -> Result<
        meerkat_core::lifecycle::CoreBoundaryStageOutput,
        meerkat_core::lifecycle::CoreBoundaryStageError,
    > {
        if matches!(
            delivery,
            meerkat_core::TurnBoundaryDelivery::DurableAppends(_)
        ) {
            self.lag.hold().await;
            // The coordinator registers the delivery in the first poll of
            // the call below, in this same poll of this task.
            self.lag.forwarded.fetch_add(1, Ordering::SeqCst);
        }
        self.inner
            .prepare_turn_boundary_delivery(expected_run_id, delivery)
            .await
    }
}

/// The rig's executor behind a boundary handle that lags.
struct LaggingRegistrationExecutor {
    inner: DurableSteerExecutor,
    lag: Arc<RegistrationLag>,
}

#[async_trait::async_trait]
impl CoreExecutor for LaggingRegistrationExecutor {
    async fn publish_boundary_appends_discarded(
        &mut self,
        discarded: &meerkat_core::event::BoundaryAppendsDiscarded,
    ) -> Result<(), CoreExecutorError> {
        self.inner
            .publish_boundary_appends_discarded(discarded)
            .await
    }

    fn boundary_handle(&self) -> Option<Arc<dyn CoreExecutorBoundaryHandle>> {
        self.inner.boundary_handle().map(|inner| {
            Arc::new(LaggingRegistrationBoundaryHandle {
                inner,
                lag: Arc::clone(&self.lag),
            }) as Arc<dyn CoreExecutorBoundaryHandle>
        })
    }

    fn interrupt_handle(
        &self,
    ) -> Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorInterruptHandle>> {
        self.inner.interrupt_handle()
    }

    fn publication_handle(
        &self,
    ) -> Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle>> {
        self.inner.publication_handle()
    }

    async fn apply(
        &mut self,
        run_id: RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        self.inner.apply(run_id, primitive).await
    }

    async fn cancel_after_boundary(&mut self, reason: String) -> Result<(), CoreExecutorError> {
        self.inner.cancel_after_boundary(reason).await
    }

    async fn stop_runtime_executor(&mut self, reason: String) -> Result<(), CoreExecutorError> {
        self.inner.stop_runtime_executor(reason).await
    }
}

async fn lagging_rig(adapter: Arc<MeerkatMachine>) -> (DurableSteerRig, Arc<RegistrationLag>) {
    let session_id = SessionId::new();
    let bindings = adapter
        .prepare_bindings(session_id.clone())
        .await
        .expect("prepare generated runtime bindings");
    let state = meerkat_core::TransientTurnContextStateHandle::new();
    // As the agent builder does for the coordinator it creates.
    if let Some(admissions) = bindings.turn_state().pending_boundary_admissions() {
        state.bind_pending_boundary_admissions(admissions);
    }
    let script = RunnerScript::new();
    let lag = Arc::new(RegistrationLag::default());
    adapter
        .ensure_session_with_executor(
            session_id.clone(),
            Box::new(LaggingRegistrationExecutor {
                inner: DurableSteerExecutor {
                    turn_state: Arc::clone(bindings.turn_state()),
                    state: state.clone(),
                    script: Arc::clone(&script),
                    terminal_publisher: None,
                },
                lag: Arc::clone(&lag),
            }),
        )
        .await
        .expect("install durable steer executor");
    (
        DurableSteerRig {
            adapter,
            session_id,
            state,
            script,
        },
        lag,
    )
}

async fn ephemeral_lagging_rig() -> (DurableSteerRig, Arc<RegistrationLag>) {
    lagging_rig(Arc::new(MeerkatMachine::ephemeral())).await
}

async fn persistent_lagging_rig() -> (DurableSteerRig, Arc<RegistrationLag>) {
    let store: Arc<dyn crate::store::RuntimeStore> =
        Arc::new(crate::store::InMemoryRuntimeStore::new());
    let (rig, lag) = lagging_rig(Arc::new(
        MeerkatMachine::persistent_without_blobs(Arc::clone(&store)).expect("persistent machine"),
    ))
    .await;
    *rig.script.publication_store.lock().unwrap() =
        Some((store, MeerkatMachine::logical_runtime_id(&rig.session_id)));
    (rig, lag)
}

fn take_ids(take: &[(InputId, String)]) -> Vec<InputId> {
    take.iter().map(|(input_id, _)| input_id.clone()).collect()
}

async fn wait_for_steps_done(rig: &DurableSteerRig, expected: usize) -> bool {
    tokio::time::timeout(Duration::from_secs(5), async {
        while rig.script.steps_done.load(Ordering::SeqCst) < expected {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .is_ok()
}

/// The runner reaches its next boundary while every durable preparation that
/// was held before this call is still held, so none of them has registered.
/// Only then do those preparations go on to the coordinator. Returns what
/// that boundary wrote for the model request that follows it.
async fn runner_reaches_the_boundary_before_the_registration(
    rig: &DurableSteerRig,
    lag: &RegistrationLag,
) -> Vec<(InputId, String)> {
    let held = lag.held();
    let steps_before = rig.script.steps_done.load(Ordering::SeqCst);
    let takes_before = rig.script.boundary_takes().len();
    rig.script.step(RunnerStep::BoundaryThenToolCalls);
    // A runner that does not wait for an acknowledged steer closes the
    // boundary here, within this time, and fixes the request's input.
    let _ = tokio::time::timeout(RUNNER_REACHES_THE_BOUNDARY, async {
        while rig.script.steps_done.load(Ordering::SeqCst) <= steps_before {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await;
    lag.release_through(held);
    assert!(
        wait_for_steps_done(rig, steps_before + 1).await,
        "the runner fixes the request's input once the held preparations registered"
    );
    rig.script.boundary_takes()[takes_before].clone()
}

async fn finish_without_a_follow_up(rig: &DurableSteerRig, delivered: &[InputId]) {
    rig.script.step(RunnerStep::Finish);
    for input_id in delivered {
        rig.wait_for_phase(input_id, InputLifecycleState::Consumed)
            .await;
        assert_eq!(
            contributions(&rig.script.primitives(), input_id),
            0,
            "{input_id} joined the running run; no turn started with it"
        );
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        rig.script.apply_calls.load(Ordering::SeqCst),
        1,
        "every steer joined the running run; no follow-up turn ran"
    );
    assert_eq!(
        rig.script.applied_durable(),
        delivered,
        "each steer was applied exactly once, in admission order"
    );
}

/// The first steer, with nobody holding the boundary. Its admission is
/// acknowledged, its preparation has not registered, and the runner reaches
/// the boundary. The request the runner makes next carries the steer.
async fn acknowledged_first_steer_is_in_the_next_request(
    rig: DurableSteerRig,
    lag: Arc<RegistrationLag>,
) {
    rig.start_busy_turn().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    lag.arm();
    let steer = peer_steer("acknowledged payload");
    let steer_id = steer.id().clone();
    // The admission answers its caller here.
    rig.admit(steer).await;
    lag.wait_held(1).await;
    assert!(
        !rig.state.has_waiting_delivery_for_test(),
        "the acknowledged steer's preparation has not registered yet"
    );

    let next_request = runner_reaches_the_boundary_before_the_registration(&rig, &lag).await;
    assert_eq!(
        take_ids(&next_request),
        vec![steer_id.clone()],
        "the steer was acknowledged before the request's input was fixed: \
         the request carries it"
    );
    assert!(
        next_request[0].1.contains("acknowledged payload"),
        "the steer's payload: {next_request:?}"
    );
    assert_eq!(
        rig.script
            .boundary_takes()
            .iter()
            .map(Vec::len)
            .collect::<Vec<_>>(),
        vec![0, 1],
        "no boundary closed without the acknowledged steer"
    );
    finish_without_a_follow_up(&rig, &[steer_id]).await;
}

#[tokio::test]
async fn acknowledged_first_steer_whose_registration_lags_is_in_the_next_request() {
    let (rig, lag) = ephemeral_lagging_rig().await;
    acknowledged_first_steer_is_in_the_next_request(rig, lag).await;
}

#[tokio::test]
async fn persistent_acknowledged_first_steer_whose_registration_lags_is_in_the_next_request() {
    let (rig, lag) = persistent_lagging_rig().await;
    acknowledged_first_steer_is_in_the_next_request(rig, lag).await;
}

/// A second steer is acknowledged while an older one holds the boundary for
/// the lane. No request is made without a steer that was acknowledged before
/// its input was fixed: the next request carries the holder first, and the
/// second steer is in that request too or, where a boundary carried the
/// holder alone, in the request after it, although its own preparation had
/// not registered when the runner reached that boundary either.
///
/// With `holder_registered` the holder's preparation registered before the
/// second steer arrived; without it the holder's preparation is itself still
/// on its way to the coordinator when the runner reaches the boundary.
async fn acknowledged_second_steer_is_in_the_earliest_request(holder_registered: bool) {
    let (rig, lag) = ephemeral_lagging_rig().await;
    rig.start_busy_turn().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    let holder = peer_steer("holder payload");
    let holder_id = holder.id().clone();
    let mut held = 0;
    if holder_registered {
        rig.admit(holder).await;
        rig.wait_for_waiting_delivery().await;
        lag.arm();
    } else {
        lag.arm();
        rig.admit(holder).await;
        held += 1;
        lag.wait_held(held).await;
        assert!(!rig.state.has_waiting_delivery_for_test());
    }
    let second = peer_steer("second payload");
    let second_id = second.id().clone();
    // The second admission answers its caller here; the steer waits in the
    // steer lane behind the holder.
    rig.admit(second).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(rig.steer_queue().await.contains(&second_id));

    let mut delivered = runner_reaches_the_boundary_before_the_registration(&rig, &lag).await;
    assert_eq!(
        take_ids(&delivered).first(),
        Some(&holder_id),
        "the holder was acknowledged before the request's input was fixed: \
         the request carries it first: {delivered:?}"
    );
    if delivered.len() == 1 {
        // The boundary carried the holder alone. The second steer now
        // prepares for the following boundary, and that preparation is held
        // before it registers while the runner reaches the boundary.
        held += 1;
        lag.wait_held(held).await;
        let following = runner_reaches_the_boundary_before_the_registration(&rig, &lag).await;
        assert_eq!(
            take_ids(&following),
            vec![second_id.clone()],
            "the second steer was acknowledged before this request's input \
             was fixed: the request carries it"
        );
        delivered.extend(following);
    }
    assert_eq!(
        take_ids(&delivered),
        vec![holder_id.clone(), second_id.clone()],
        "both steers reach the model, in admission order"
    );
    assert!(delivered[0].1.contains("holder payload"));
    assert!(delivered[1].1.contains("second payload"));
    assert!(
        rig.script.boundary_takes()[1..]
            .iter()
            .all(|take| !take.is_empty()),
        "no boundary closed without an acknowledged steer: {:?}",
        rig.script.boundary_takes()
    );
    finish_without_a_follow_up(&rig, &[holder_id, second_id]).await;
}

#[tokio::test]
async fn acknowledged_second_steer_behind_a_registered_holder_is_in_the_earliest_request() {
    acknowledged_second_steer_is_in_the_earliest_request(true).await;
}

#[tokio::test]
async fn acknowledged_second_steer_behind_a_lagging_holder_is_in_the_earliest_request() {
    acknowledged_second_steer_is_in_the_earliest_request(false).await;
}

async fn wait_for_prepare_hold(rig: &DurableSteerRig) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !rig.script.prepare_hold_reached.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("a preparation parks the runner and is held before it publishes");
}

/// A steer that reaches the model as request-only context: a continuation
/// with a turn append.
fn request_only_steer(detail: &str) -> Input {
    let mut continuation = crate::input::ContinuationInput::detached_background_op_completed();
    continuation.turn_append = Some(notice_append(detail, ConversationAppendRole::SystemNotice));
    let continuation = Input::Continuation(continuation);
    assert_eq!(
        crate::ingress_types::RuntimeInputSemantics::try_from_generated_admission(
            &continuation,
            false
        )
        .expect("generated admission")
        .live_boundary_delivery(),
        Some(crate::ingress_types::LiveBoundaryDeliveryClass::RequestOnly),
    );
    continuation
}

/// The runner is parked at its boundary with a request-only delivery alone,
/// and that delivery has not been published. A durable steer is acknowledged
/// then: the request's input is not fixed yet, so the request carries it.
async fn durable_steer_acknowledged_during_a_request_only_park_is_in_that_request(
    rig: DurableSteerRig,
    lag: Arc<RegistrationLag>,
) {
    rig.start_busy_turn().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    let context = request_only_steer("request-only context");
    let context_id = context.id().clone();
    rig.admit(context).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !rig.state.has_registered_request_only_delivery_for_test() {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("the request-only steer prepares for the open boundary");

    // The runner parks with the request-only delivery alone; its ingress is
    // held before it publishes, so the park lasts.
    rig.script.prepare_hold_armed.store(true, Ordering::SeqCst);
    let steps_before = rig.script.steps_done.load(Ordering::SeqCst);
    let takes_before = rig.script.boundary_takes().len();
    rig.script.step(RunnerStep::BoundaryThenToolCalls);
    wait_for_prepare_hold(&rig).await;

    let steer = peer_steer("payload during the park");
    let steer_id = steer.id().clone();
    // The admission answers its caller here, and the steer's preparation
    // reaches the coordinator while the runner is still parked.
    rig.admit(steer).await;
    lag.wait_forwarded(1).await;
    assert_eq!(
        rig.script.steps_done.load(Ordering::SeqCst),
        steps_before,
        "the runner is still parked: the request's input is not fixed yet"
    );

    rig.script
        .prepare_hold_released
        .store(true, Ordering::SeqCst);
    assert!(
        wait_for_steps_done(&rig, steps_before + 1).await,
        "the parked boundary ends once its deliveries are published"
    );
    let request = rig.script.boundary_takes()[takes_before].clone();
    assert_eq!(
        take_ids(&request),
        vec![steer_id.clone()],
        "the steer was acknowledged before the request's input was fixed: \
         the request carries it beside the request-only context"
    );
    assert!(
        request[0].1.contains("payload during the park"),
        "the steer's payload: {request:?}"
    );
    assert_eq!(
        *rig.script.request_only_taken.lock().unwrap(),
        vec![1],
        "the request-only context reached the same request"
    );
    rig.wait_for_phase(&context_id, InputLifecycleState::Consumed)
        .await;
    finish_without_a_follow_up(&rig, &[steer_id]).await;
}

#[tokio::test]
async fn durable_steer_acknowledged_while_the_runner_is_parked_on_request_only_context_is_in_that_request()
 {
    let (rig, lag) = ephemeral_lagging_rig().await;
    durable_steer_acknowledged_during_a_request_only_park_is_in_that_request(rig, lag).await;
}

#[tokio::test]
async fn persistent_durable_steer_acknowledged_while_the_runner_is_parked_on_request_only_context_is_in_that_request()
 {
    let (rig, lag) = persistent_lagging_rig().await;
    durable_steer_acknowledged_during_a_request_only_park_is_in_that_request(rig, lag).await;
}

/// The runner is parked with a steer whose publication has not happened.
/// That steer is recalled, and two more steers are acknowledged while the
/// runner is still parked. The boundary neither waits for ever nor ends
/// without them: the request carries the two, in order, and never the
/// recalled one.
#[tokio::test]
async fn steers_acknowledged_after_the_parked_holder_was_recalled_are_in_that_request() {
    let (rig, _lag) = ephemeral_lagging_rig().await;
    rig.start_busy_turn().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    rig.script.prepare_hold_armed.store(true, Ordering::SeqCst);
    let holder = peer_steer("recalled holder payload");
    let holder_id = holder.id().clone();
    rig.admit(holder).await;
    rig.wait_for_waiting_delivery().await;
    let steps_before = rig.script.steps_done.load(Ordering::SeqCst);
    let takes_before = rig.script.boundary_takes().len();
    rig.script.step(RunnerStep::BoundaryThenToolCalls);
    wait_for_prepare_hold(&rig).await;

    let recalled = tokio::time::timeout(
        Duration::from_secs(5),
        rig.adapter.recall_queued_input(&rig.session_id, &holder_id),
    )
    .await
    .expect("a recall answers without waiting for a boundary")
    .expect("recall");
    assert_eq!(
        recalled,
        Some(crate::queued_input_recall::QueuedInputRecall::Abandoned {
            input_id: holder_id.clone(),
            attempt_count: 0,
        })
    );
    let second = peer_steer("second payload after the recall");
    let second_id = second.id().clone();
    let third = peer_steer("third payload after the recall");
    let third_id = third.id().clone();
    rig.admit(second).await;
    rig.admit(third).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let lane = rig.steer_queue().await;
    assert!(lane.contains(&second_id) && lane.contains(&third_id));
    assert_eq!(
        rig.script.steps_done.load(Ordering::SeqCst),
        steps_before,
        "the runner is still parked"
    );

    rig.script
        .prepare_hold_released
        .store(true, Ordering::SeqCst);
    assert!(
        wait_for_steps_done(&rig, steps_before + 1).await,
        "the parked boundary ends: nothing waits for the recalled steer"
    );
    let request = rig.script.boundary_takes()[takes_before].clone();
    assert_eq!(
        take_ids(&request),
        vec![second_id.clone(), third_id.clone()],
        "both steers were acknowledged before the request's input was fixed"
    );
    assert!(request[0].1.contains("second payload after the recall"));
    assert!(request[1].1.contains("third payload after the recall"));
    assert!(
        request
            .iter()
            .all(|(_, payload)| !payload.contains("recalled holder payload")),
        "the recalled steer reaches no request: {request:?}"
    );
    finish_without_a_follow_up(&rig, &[second_id, third_id]).await;
    assert_eq!(contributions(&rig.script.primitives(), &holder_id), 0);
    assert_eq!(
        rig.phase(&holder_id).await,
        Some(InputLifecycleState::Abandoned)
    );
}

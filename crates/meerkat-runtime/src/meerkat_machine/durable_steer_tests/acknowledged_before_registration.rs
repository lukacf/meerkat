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
    /// While set, a held preparation that is let go fails at the handle
    /// instead of reaching the coordinator.
    refuses: AtomicBool,
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
            if self.lag.refuses.load(Ordering::SeqCst) {
                return Err(
                    meerkat_core::lifecycle::CoreBoundaryStageError::unavailable(
                        "the boundary handle failed before the coordinator saw the delivery",
                    ),
                );
            }
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

/// Waits until the runner did one of two things at its boundary: decided to
/// keep it, or its park, for an acknowledged steer whose preparation has not
/// registered (the coordinator's count moved past `waits_before`), or fixed
/// the request's input (its step is done). `true` for the first. No time
/// interval stands in for either: a held preparation is let through only
/// after the runner's own decision was observed.
async fn runner_decides_at_the_boundary(
    rig: &DurableSteerRig,
    steps_before: usize,
    waits_before: u64,
) -> bool {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if rig.script.steps_done.load(Ordering::SeqCst) > steps_before {
                return false;
            }
            if rig.state.pending_admission_waits_entered_for_test() > waits_before {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("the runner reaches its boundary and decides")
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
    let waits_before = rig.state.pending_admission_waits_entered_for_test();
    rig.script.step(RunnerStep::BoundaryThenToolCalls);
    // A runner that does not wait for an acknowledged steer closes the
    // boundary and fixes the request's input; one that waits says so. The
    // held preparations go on only after either was observed.
    let _ = runner_decides_at_the_boundary(rig, steps_before, waits_before).await;
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

/// The door an input is admitted through.
#[derive(Clone, Copy)]
enum AdmissionDoor {
    /// `accept_input_with_completion`.
    AcceptWithCompletion,
    /// The control plane's `ingest`.
    ControlPlaneIngest,
}

async fn admit_through(rig: &DurableSteerRig, door: AdmissionDoor, input: Input) {
    match door {
        AdmissionDoor::AcceptWithCompletion => {
            rig.admit(input).await;
        }
        AdmissionDoor::ControlPlaneIngest => {
            let runtime_id = MeerkatMachine::logical_runtime_id(&rig.session_id);
            let outcome = tokio::time::timeout(
                Duration::from_secs(5),
                crate::traits::RuntimeControlPlane::ingest(
                    rig.adapter.as_ref(),
                    &runtime_id,
                    input,
                ),
            )
            .await
            .expect("an ingest answers without waiting for a boundary")
            .expect("steer input ingested");
            assert!(outcome.is_accepted());
        }
    }
}

/// The first steer, with nobody holding the boundary. Its admission is
/// acknowledged, its preparation has not registered, and the runner reaches
/// the boundary. The request the runner makes next carries the steer.
async fn acknowledged_first_steer_is_in_the_next_request(
    rig: DurableSteerRig,
    lag: Arc<RegistrationLag>,
    door: AdmissionDoor,
) {
    rig.start_busy_turn().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    lag.arm();
    let steer = peer_steer("acknowledged payload");
    let steer_id = steer.id().clone();
    // The admission answers its caller here.
    admit_through(&rig, door, steer).await;
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
    acknowledged_first_steer_is_in_the_next_request(rig, lag, AdmissionDoor::AcceptWithCompletion)
        .await;
}

#[tokio::test]
async fn persistent_acknowledged_first_steer_whose_registration_lags_is_in_the_next_request() {
    let (rig, lag) = persistent_lagging_rig().await;
    acknowledged_first_steer_is_in_the_next_request(rig, lag, AdmissionDoor::AcceptWithCompletion)
        .await;
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
///
/// With `registration_lags` the steer's preparation is still on its way to
/// the coordinator when the request-only delivery is published, so the park
/// has to outlast that publication.
async fn durable_steer_acknowledged_during_a_request_only_park_is_in_that_request(
    rig: DurableSteerRig,
    lag: Arc<RegistrationLag>,
    registration_lags: bool,
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
    if registration_lags {
        lag.arm();
    }
    // The admission answers its caller here.
    rig.admit(steer).await;
    if registration_lags {
        lag.wait_held(1).await;
    } else {
        // The steer's preparation reaches the coordinator while the runner
        // is still parked.
        lag.wait_forwarded(1).await;
    }
    assert_eq!(
        rig.script.steps_done.load(Ordering::SeqCst),
        steps_before,
        "the runner is still parked: the request's input is not fixed yet"
    );

    let waits_before = rig.state.pending_admission_waits_entered_for_test();
    rig.script
        .prepare_hold_released
        .store(true, Ordering::SeqCst);
    if registration_lags {
        // The request-only delivery is published now. A park that ends on
        // it fixes the request's input without the acknowledged steer; one
        // that stays for the steer says so. The steer's preparation goes on
        // only after either was observed, and after the request-only input
        // was consumed by its publication.
        rig.wait_for_phase(&context_id, InputLifecycleState::Consumed)
            .await;
        let _ = runner_decides_at_the_boundary(&rig, steps_before, waits_before).await;
        lag.release_through(1);
    }
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
    durable_steer_acknowledged_during_a_request_only_park_is_in_that_request(rig, lag, false).await;
}

#[tokio::test]
async fn durable_steer_whose_registration_lags_a_request_only_park_is_in_that_request() {
    let (rig, lag) = ephemeral_lagging_rig().await;
    durable_steer_acknowledged_during_a_request_only_park_is_in_that_request(rig, lag, true).await;
}

#[tokio::test]
async fn persistent_durable_steer_whose_registration_lags_a_request_only_park_is_in_that_request() {
    let (rig, lag) = persistent_lagging_rig().await;
    durable_steer_acknowledged_during_a_request_only_park_is_in_that_request(rig, lag, true).await;
}

#[tokio::test]
async fn persistent_durable_steer_acknowledged_while_the_runner_is_parked_on_request_only_context_is_in_that_request()
 {
    let (rig, lag) = persistent_lagging_rig().await;
    durable_steer_acknowledged_during_a_request_only_park_is_in_that_request(rig, lag, false).await;
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

#[tokio::test]
async fn acknowledged_first_steer_ingested_through_the_control_plane_is_in_the_next_request() {
    let (rig, lag) = ephemeral_lagging_rig().await;
    acknowledged_first_steer_is_in_the_next_request(rig, lag, AdmissionDoor::ControlPlaneIngest)
        .await;
}

async fn recall(rig: &DurableSteerRig, input_id: &InputId) {
    let recalled = tokio::time::timeout(
        Duration::from_secs(5),
        rig.adapter.recall_queued_input(&rig.session_id, input_id),
    )
    .await
    .expect("a recall answers without waiting for a boundary")
    .expect("recall");
    assert_eq!(
        recalled,
        Some(crate::queued_input_recall::QueuedInputRecall::Abandoned {
            input_id: input_id.clone(),
            attempt_count: 0,
        })
    );
}

/// The runner reaches its boundary and stays there for an acknowledged steer
/// whose preparation is held. Returns the runner's step count before the
/// step and the index of the take that step makes.
async fn runner_waits_at_the_boundary_for_the_held_steer(rig: &DurableSteerRig) -> (usize, usize) {
    let steps_before = rig.script.steps_done.load(Ordering::SeqCst);
    let takes_before = rig.script.boundary_takes().len();
    let waits_before = rig.state.pending_admission_waits_entered_for_test();
    rig.script.step(RunnerStep::BoundaryThenToolCalls);
    assert!(
        runner_decides_at_the_boundary(rig, steps_before, waits_before).await,
        "the runner keeps its boundary open for the acknowledged steer"
    );
    (steps_before, takes_before)
}

/// An acknowledged steer is recalled before its preparation registers. The
/// boundary the runner kept open for it ends without it: the steer is in no
/// request, has no row and is never delivered.
#[tokio::test]
async fn acknowledged_steer_recalled_before_its_registration_is_in_no_request() {
    let (rig, lag) = ephemeral_lagging_rig().await;
    rig.start_busy_turn().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    lag.arm();
    let steer = peer_steer("recalled before registration");
    let steer_id = steer.id().clone();
    rig.admit(steer).await;
    lag.wait_held(1).await;
    let (steps_before, takes_before) = runner_waits_at_the_boundary_for_the_held_steer(&rig).await;

    recall(&rig, &steer_id).await;
    lag.release_through(1);
    assert!(
        wait_for_steps_done(&rig, steps_before + 1).await,
        "the boundary ends: nothing waits for the recalled steer"
    );
    assert!(
        rig.script.boundary_takes()[takes_before].is_empty(),
        "the recalled steer reaches no request: {:?}",
        rig.script.boundary_takes()
    );
    assert!(
        rig.script.applied_durable().is_empty(),
        "the recalled steer has no row"
    );
    assert_eq!(
        rig.phase(&steer_id).await,
        Some(InputLifecycleState::Abandoned)
    );
    finish_without_a_follow_up(&rig, &[]).await;
    assert_eq!(contributions(&rig.script.primitives(), &steer_id), 0);
}

/// The boundary handle fails for an acknowledged steer before the
/// coordinator sees its delivery. The boundary the runner kept open for it
/// ends, and the steer takes its queued follow-up turn, once.
#[tokio::test]
async fn acknowledged_steer_whose_preparation_fails_takes_one_follow_up() {
    let (rig, lag) = ephemeral_lagging_rig().await;
    rig.start_busy_turn().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    lag.arm();
    let steer = peer_steer("preparation fails");
    let steer_id = steer.id().clone();
    rig.admit(steer).await;
    lag.wait_held(1).await;
    let (steps_before, takes_before) = runner_waits_at_the_boundary_for_the_held_steer(&rig).await;

    lag.refuses.store(true, Ordering::SeqCst);
    lag.release_through(1);
    assert!(
        wait_for_steps_done(&rig, steps_before + 1).await,
        "the boundary ends once the steer's work is over without a preparation"
    );
    assert!(rig.script.boundary_takes()[takes_before].is_empty());
    assert_eq!(
        rig.phase(&steer_id).await,
        Some(InputLifecycleState::Queued)
    );

    rig.script.step(RunnerStep::Finish);
    rig.wait_for_apply_calls(2).await;
    assert_eq!(
        rig.script.primitives()[1],
        vec![steer_id.clone()],
        "the steer starts one follow-up turn"
    );
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_phase(&steer_id, InputLifecycleState::Consumed)
        .await;
    assert!(rig.script.applied_durable().is_empty());
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(rig.script.apply_calls.load(Ordering::SeqCst), 2);
}

/// Request-only context is registered for the boundary and a durable steer
/// is acknowledged, its preparation held, when the runner reaches it. The
/// runner does not park on the request-only context alone: the request
/// carries both.
#[tokio::test]
async fn acknowledged_steer_beside_registered_request_only_context_is_in_that_request() {
    let (rig, lag) = ephemeral_lagging_rig().await;
    rig.start_busy_turn().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    let context = request_only_steer("request-only context first");
    let context_id = context.id().clone();
    rig.admit(context).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !rig.state.has_registered_request_only_delivery_for_test() {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("the request-only steer prepares for the open boundary");
    lag.arm();
    let steer = peer_steer("payload beside request-only context");
    let steer_id = steer.id().clone();
    rig.admit(steer).await;
    lag.wait_held(1).await;

    let request = runner_reaches_the_boundary_before_the_registration(&rig, &lag).await;
    assert_eq!(
        take_ids(&request),
        vec![steer_id.clone()],
        "the steer was acknowledged before the request's input was fixed"
    );
    assert!(request[0].1.contains("payload beside request-only context"));
    assert_eq!(*rig.script.request_only_taken.lock().unwrap(), vec![1]);
    rig.wait_for_phase(&context_id, InputLifecycleState::Consumed)
        .await;
    finish_without_a_follow_up(&rig, &[steer_id]).await;
}

/// A steer is acknowledged while the model streams its last response, and
/// its preparation is still held when the run ends with no further boundary.
/// Nothing hangs: the steer starts one follow-up turn; that turn's first
/// boundary is kept open for the ended run's preparation, and ends once
/// that preparation returns.
#[tokio::test]
async fn acknowledged_steer_still_unregistered_when_the_run_ends_takes_one_follow_up() {
    let (rig, lag) = ephemeral_lagging_rig().await;
    rig.start_busy_turn().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenStream)
        .await;
    lag.arm();
    let steer = peer_steer("run ends first");
    let steer_id = steer.id().clone();
    rig.admit(steer).await;
    lag.wait_held(1).await;

    // The model answers: the run ends with the preparation still held.
    rig.script.step(RunnerStep::Finish);
    rig.wait_for_apply_calls(2).await;
    assert_eq!(
        rig.script.primitives()[1],
        vec![steer_id.clone()],
        "the steer starts one follow-up turn"
    );

    // The follow-up turn reaches its first boundary while the ended run's
    // preparation is still held. The announcement of that preparation still
    // stands (it is counted for the session, not for a run), so this
    // boundary is kept open for it: read here, before the preparation is
    // let go.
    let steps_before = rig.script.steps_done.load(Ordering::SeqCst);
    let takes_before = rig.script.boundary_takes().len();
    let waits_before = rig.state.pending_admission_waits_entered_for_test();
    rig.script.step(RunnerStep::BoundaryThenToolCalls);
    assert!(
        runner_decides_at_the_boundary(&rig, steps_before, waits_before).await,
        "the follow-up turn's first boundary is held open while the ended run's \
         preparation has not returned"
    );
    assert_eq!(
        (lag.held(), lag.forwarded.load(Ordering::SeqCst)),
        (1, 0),
        "the ended run's preparation is still held and has reached no coordinator"
    );
    assert_eq!(
        rig.script.steps_done.load(Ordering::SeqCst),
        steps_before,
        "the boundary is still open when the held preparation is let go"
    );
    assert_eq!(
        rig.script.boundary_takes().len(),
        takes_before,
        "the open boundary has written no take"
    );

    // Let go, the preparation names a run that is no longer active: the
    // coordinator refuses it at once, its work ends, and the boundary
    // closes with nothing.
    lag.release_through(1);
    assert!(
        wait_for_steps_done(&rig, steps_before + 1).await,
        "the follow-up turn's boundary ends once the ended run's preparation returned"
    );
    assert_eq!(
        lag.forwarded.load(Ordering::SeqCst),
        1,
        "the boundary ended after the preparation went on to the coordinator, not before"
    );
    assert!(rig.script.boundary_takes()[takes_before].is_empty());
    assert!(
        !rig.state.has_waiting_delivery_for_test(),
        "the ended run's preparation registered nothing"
    );

    rig.script.step(RunnerStep::Finish);
    rig.wait_for_phase(&steer_id, InputLifecycleState::Consumed)
        .await;
    assert!(
        rig.script.applied_durable().is_empty(),
        "the steer was delivered by its follow-up turn, not at a boundary"
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(rig.script.apply_calls.load(Ordering::SeqCst), 2);
}

/// Two steers join one boundary; the runner then goes through the next
/// boundary at once, with nothing to deliver. It is not held by the two
/// steers' finished work beyond that work's own return.
#[tokio::test]
async fn empty_boundary_right_after_two_steers_joined_ends_promptly() {
    let (rig, _lag) = ephemeral_lagging_rig().await;
    rig.start_busy_turn().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    let first = peer_steer("joined payload one");
    let first_id = first.id().clone();
    let second = peer_steer("joined payload two");
    let second_id = second.id().clone();
    rig.admit(first).await;
    rig.admit(second).await;
    rig.wait_for_waiting_delivery().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    assert_eq!(
        take_ids(rig.script.boundary_takes().last().expect("a take")),
        vec![first_id.clone(), second_id.clone()]
    );

    let started = std::time::Instant::now();
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    let elapsed = started.elapsed();
    eprintln!("empty boundary right after two steers joined: {elapsed:?}");
    assert!(
        rig.script
            .boundary_takes()
            .last()
            .expect("a take")
            .is_empty()
    );
    assert!(
        elapsed < Duration::from_millis(500),
        "the empty boundary ended in {elapsed:?}"
    );
    finish_without_a_follow_up(&rig, &[first_id, second_id]).await;
}

/// A boundary carried one steer. Another steer is acknowledged afterwards
/// and its preparation is held when the runner reaches the following
/// boundary: that request carries it.
#[tokio::test]
async fn steer_acknowledged_after_a_boundary_that_carried_another_is_in_the_following_request() {
    let (rig, lag) = ephemeral_lagging_rig().await;
    rig.start_busy_turn().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    let first = peer_steer("first boundary payload");
    let first_id = first.id().clone();
    rig.admit(first).await;
    rig.wait_for_waiting_delivery().await;
    rig.script
        .step_and_wait(RunnerStep::BoundaryThenToolCalls)
        .await;
    assert_eq!(
        take_ids(rig.script.boundary_takes().last().expect("a take")),
        vec![first_id.clone()]
    );

    lag.arm();
    let later = peer_steer("following boundary payload");
    let later_id = later.id().clone();
    rig.admit(later).await;
    lag.wait_held(1).await;
    let following = runner_reaches_the_boundary_before_the_registration(&rig, &lag).await;
    assert_eq!(
        take_ids(&following),
        vec![later_id.clone()],
        "the steer was acknowledged before this request's input was fixed"
    );
    assert!(following[0].1.contains("following boundary payload"));
    finish_without_a_follow_up(&rig, &[first_id, later_id]).await;
}

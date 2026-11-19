//! An orderly stop with a run in flight on a host store, end to end. A real
//! native agent runs a queued prompt over the store checkpointer of a host
//! store realm. Its first round commits. The host then stops the session in
//! good order, keeping its queued work for the next start, while the run is
//! in its second round: the stop asks the run to suspend, and the run is at
//! that moment inside its round's first commit, waiting where a commit waits
//! for another writer of the store. The process ends, and a restart over the
//! same store, inside the stop's max age, opens the session: the same run
//! goes on from its last round commit, and completes with each round in the
//! transcript once. Beside it, the other endings that can meet the run in
//! that commit, each of which must leave the store usable and end the run
//! as it always did: the same stop restarted after its max age, a stop that
//! keeps no work, and a cancel. And the orderly stop that meets the run in
//! a model call, which keeps it as before, and the one that meets it in its
//! closing save, after the model's last answer.

use super::unsettled_restart::{Durable, Process};
use super::*;
use meerkat_core::lifecycle::core_executor::CoreApplyOutput;
use meerkat_core::lifecycle::{
    CommittedSessionBoundaryAuthority, CoreExecutor, CoreExecutorBoundaryHandle, CoreExecutorError,
    CoreExecutorInterruptHandle, CoreExecutorPostStopCleanupHandle, CoreExecutorPreDequeueHandle,
    CoreExecutorPublicationHandle, CoreExecutorTranscriptNoticeHandle,
    CoreExecutorTurnFinalizationBoundaryHandle, CoreInteractionTerminalPublicationReceipt, RunId,
    RunPrimitive,
};

/// The call id of the run's second round, whose first commit the stop meets.
const SECOND_ROUND: &str = "probe-round-2";

/// The max age of the work an orderly stop keeps, in the rows that restart
/// inside it.
const MAX_AGE: Duration = Duration::from_mins(10);

/// A max age the restart of one row outlasts.
const SHORT_MAX_AGE: Duration = Duration::from_secs(2);

/// How long a restarted process is watched for what it does of its own
/// accord with inputs that are not ended.
const RESTART_WATCH: Duration = Duration::from_secs(20);

/// How long the caller of a run's closing save is watched, while the save
/// is held, for an interrupt that drops it.
const CLOSING_SAVE_WATCH: Duration = Duration::from_secs(2);

/// The row's word over the interrupt that ends the run (a stop's
/// suspension, a stop's or a cancel's hard cancel): while it holds, the
/// first interrupt is delivered only once the row lets it go. It lists
/// every interrupt the run's handle was asked for, by kind, in order.
#[derive(Default)]
struct SuspensionGate {
    holds: AtomicBool,
    asked: tokio::sync::Notify,
    go: tokio::sync::Notify,
    interrupts: StdMutex<Vec<&'static str>>,
}

/// The kind of interrupt a stop that keeps the queued work asks for.
const SUSPENSION: &str = "a suspension";

/// The kind of interrupt a cancel, and a stop that keeps nothing, ask for.
const HARD_CANCEL: &str = "a hard cancel";

/// The session's interrupt handle, its interrupts under the row's gate.
struct GatedInterrupt {
    inner: Arc<dyn CoreExecutorInterruptHandle>,
    gate: Arc<SuspensionGate>,
}

impl GatedInterrupt {
    /// Deliver an interrupt: at once, or, for the first one while the gate
    /// holds, on the row's word. That one is delivered from a task of its
    /// own: a stop bounds its wait for the delivery, and the interrupt
    /// still lands after that bound.
    async fn on_the_rows_word(
        &self,
        kind: &'static str,
        deliver: impl Future<Output = Result<bool, CoreExecutorError>> + Send + 'static,
    ) -> Result<bool, CoreExecutorError> {
        self.gate
            .interrupts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(kind);
        if !self.gate.holds.swap(false, Ordering::SeqCst) {
            return deliver.await;
        }
        let gate = Arc::clone(&self.gate);
        let delivery = tokio::spawn(async move {
            gate.asked.notify_one();
            gate.go.notified().await;
            deliver.await
        });
        match delivery.await {
            Ok(outcome) => outcome,
            Err(error) => Err(CoreExecutorError::Internal(error.to_string())),
        }
    }
}

#[async_trait]
impl CoreExecutorInterruptHandle for GatedInterrupt {
    async fn hard_cancel_run_if_current(
        &self,
        expected_run_id: &RunId,
        reason: String,
    ) -> Result<bool, CoreExecutorError> {
        let inner = Arc::clone(&self.inner);
        let run_id = expected_run_id.clone();
        self.on_the_rows_word(HARD_CANCEL, async move {
            inner.hard_cancel_run_if_current(&run_id, reason).await
        })
        .await
    }

    async fn suspend_run_if_current(
        &self,
        expected_run_id: &RunId,
        reason: String,
    ) -> Result<bool, CoreExecutorError> {
        let inner = Arc::clone(&self.inner);
        let run_id = expected_run_id.clone();
        self.on_the_rows_word(SUSPENSION, async move {
            inner.suspend_run_if_current(&run_id, reason).await
        })
        .await
    }
}

/// The session's executor as a surface builds it, with its interrupt handle
/// under the row's gate. Every other call goes to the executor unchanged.
struct GatedExecutor {
    inner: Box<dyn CoreExecutor>,
    interrupt: Option<Arc<dyn CoreExecutorInterruptHandle>>,
}

impl GatedExecutor {
    fn over(inner: Box<dyn CoreExecutor>, gate: Arc<SuspensionGate>) -> Box<dyn CoreExecutor> {
        let interrupt = inner.interrupt_handle().map(|inner| {
            Arc::new(GatedInterrupt { inner, gate }) as Arc<dyn CoreExecutorInterruptHandle>
        });
        Box::new(Self { inner, interrupt })
    }
}

#[async_trait]
impl CoreExecutor for GatedExecutor {
    fn supports_work_authorization(&self) -> bool {
        self.inner.supports_work_authorization()
    }

    fn boundary_handle(&self) -> Option<Arc<dyn CoreExecutorBoundaryHandle>> {
        self.inner.boundary_handle()
    }

    fn interrupt_handle(&self) -> Option<Arc<dyn CoreExecutorInterruptHandle>> {
        self.interrupt.clone()
    }

    fn publication_handle(&self) -> Option<Arc<dyn CoreExecutorPublicationHandle>> {
        self.inner.publication_handle()
    }

    fn machine_managed_post_stop_unregister(&self) -> bool {
        self.inner.machine_managed_post_stop_unregister()
    }

    fn post_stop_cleanup_handle(&self) -> Option<Arc<dyn CoreExecutorPostStopCleanupHandle>> {
        self.inner.post_stop_cleanup_handle()
    }

    fn turn_finalization_boundary_handle(
        &self,
    ) -> Option<Arc<dyn CoreExecutorTurnFinalizationBoundaryHandle>> {
        self.inner.turn_finalization_boundary_handle()
    }

    fn pre_dequeue_handle(&self) -> Option<Arc<dyn CoreExecutorPreDequeueHandle>> {
        self.inner.pre_dequeue_handle()
    }

    fn transcript_notice_handle(&self) -> Option<Arc<dyn CoreExecutorTranscriptNoticeHandle>> {
        self.inner.transcript_notice_handle()
    }

    async fn apply(
        &mut self,
        run_id: RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        self.inner.apply(run_id, primitive).await
    }

    async fn checkpoint_committed_session_snapshot(
        &mut self,
        session_snapshot: Arc<Vec<u8>>,
    ) -> Result<(), CoreExecutorError> {
        self.inner
            .checkpoint_committed_session_snapshot(session_snapshot)
            .await
    }

    async fn acknowledge_committed_session_boundary(
        &mut self,
        authority: &CommittedSessionBoundaryAuthority,
    ) -> Result<(), CoreExecutorError> {
        self.inner
            .acknowledge_committed_session_boundary(authority)
            .await
    }

    async fn acknowledge_compaction_finalization(
        &mut self,
        authority: &CommittedSessionBoundaryAuthority,
    ) -> Result<(), CoreExecutorError> {
        self.inner
            .acknowledge_compaction_finalization(authority)
            .await
    }

    async fn reconcile_committed_compaction_projections(
        &mut self,
        intents: &[meerkat_core::memory::CompactionProjectionIntent],
    ) -> Result<(), CoreExecutorError> {
        self.inner
            .reconcile_committed_compaction_projections(intents)
            .await
    }

    async fn abort_uncommitted_compaction_projections(&mut self) -> Result<(), CoreExecutorError> {
        self.inner.abort_uncommitted_compaction_projections().await
    }

    async fn abort_rejected_run_projections(&mut self) -> Result<(), CoreExecutorError> {
        self.inner.abort_rejected_run_projections().await
    }

    async fn publish_interaction_terminals(
        &mut self,
        events: &[meerkat_core::AgentEvent],
    ) -> Result<Vec<CoreInteractionTerminalPublicationReceipt>, CoreExecutorError> {
        self.inner.publish_interaction_terminals(events).await
    }

    async fn publish_boundary_appends_discarded(
        &mut self,
        discarded: &meerkat_core::event::BoundaryAppendsDiscarded,
    ) -> Result<(), CoreExecutorError> {
        self.inner
            .publish_boundary_appends_discarded(discarded)
            .await
    }

    async fn cancel_after_boundary(&mut self, reason: String) -> Result<(), CoreExecutorError> {
        self.inner.cancel_after_boundary(reason).await
    }

    async fn stop_runtime_executor(&mut self, reason: String) -> Result<(), CoreExecutorError> {
        self.inner.stop_runtime_executor(reason).await
    }

    async fn cleanup_after_runtime_stop_terminalized(&mut self) -> Result<(), CoreExecutorError> {
        self.inner.cleanup_after_runtime_stop_terminalized().await
    }
}

/// Materialize a new session in `process` as a surface does, its executor's
/// suspension under `gate`.
async fn materialize_gated(
    process: &Process,
    gate: Arc<SuspensionGate>,
) -> meerkat_core::SessionId {
    let (request, _) =
        crate::surface::split_runtime_backed_eager_create_request(CreateSessionRequest {
            model: "claude-sonnet-4-5".into(),
            prompt: "unused".into(),
            injected_context: Vec::new(),
            system_prompt: meerkat_core::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: None,
            labels: None,
        });
    let created = Box::pin(crate::surface::materialize_session(
        &process.service,
        &process.machine,
        meerkat_core::Session::new(),
        request,
        {
            let service = process.service.clone();
            let machine = process.machine.clone();
            move |id| {
                GatedExecutor::over(
                    crate::surface::default_persistent_executor(service, machine, id),
                    gate,
                )
            }
        },
    ))
    .await
    .expect("materialize the session and its executor");
    created.session_id
}

/// How a row's first process ends its run.
#[derive(Debug, Clone, Copy)]
enum Ending {
    /// An orderly stop that keeps the queued work for `max_age`.
    OrderlyStop { max_age: Duration },
    /// A stop that keeps no work.
    StopKeepingNothing,
    /// A cancel of the run.
    Cancel,
}

/// Where the run is when the interrupt that ends it lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Meets {
    /// Inside its second round's first commit, waiting where a commit waits
    /// for another writer of the store.
    ARoundCommit,
    /// Inside its closing save, after the model's last answer, waiting
    /// where a commit waits for another writer of the store.
    ItsClosingSave,
    /// In its second model call.
    AModelCall,
}

impl Meets {
    /// The run's first write after its second model call is where the
    /// interrupt lands.
    fn a_write(self) -> bool {
        matches!(self, Self::ARoundCommit | Self::ItsClosingSave)
    }

    /// What the model answers, request by request: one call of `probe`,
    /// then, on the row's word, one more call or its last answer.
    fn script(self) -> Vec<ModelStep> {
        match self {
            Self::ARoundCommit | Self::AModelCall => {
                vec![ModelStep::Tools, ModelStep::HeldTools(SECOND_ROUND)]
            }
            Self::ItsClosingSave => vec![ModelStep::Tools, ModelStep::HeldText],
        }
    }
}

/// What the store holds of one input: its phase, the run it last ran in,
/// and how it ended.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StoredInput {
    phase: InputLifecycleState,
    last_run: Option<RunId>,
    ended: Option<InputTerminalOutcome>,
}

impl StoredInput {
    /// The input ran to its end.
    fn consumed(&self) -> bool {
        self.phase == InputLifecycleState::Consumed
            && self.ended == Some(InputTerminalOutcome::Consumed)
    }
}

/// The session's inputs as `store` holds them.
async fn stored_inputs_in(
    store: &dyn RuntimeStore,
    session_id: &meerkat_core::SessionId,
) -> Option<Vec<Option<StoredInput>>> {
    store
        .load_input_states(&LogicalRuntimeId::for_session(session_id))
        .await
        .ok()
        .map(|rows| {
            rows.iter()
                .map(|row| match row {
                    InputStateRow::Decoded(stored) => Some(StoredInput {
                        phase: stored.seed.phase,
                        last_run: stored.seed.last_run_id.clone(),
                        ended: stored.seed.terminal_outcome.clone(),
                    }),
                    _ => None,
                })
                .collect()
        })
}

/// How many of `inputs` ran to their end.
fn consumed_among(inputs: Option<&Vec<Option<StoredInput>>>) -> usize {
    inputs.map_or(0, |inputs| {
        inputs
            .iter()
            .flatten()
            .filter(|input| input.consumed())
            .count()
    })
}

/// What a further prompt of the session did.
#[derive(Debug)]
struct NextPrompt {
    /// Its waiter's answer, in `Debug` form, or its refusal.
    answer: Option<String>,
    /// The session's inputs in the store after it.
    inputs: Option<Vec<Option<StoredInput>>>,
    /// How many of them ran to their end before it.
    consumed_before: usize,
    /// How many more times the model was asked.
    asked_more: usize,
}

impl NextPrompt {
    /// The prompt ran: it was answered, the store holds one more input that
    /// ran to its end, and the model was asked once more.
    fn ran(&self) -> bool {
        self.answer
            .as_deref()
            .is_some_and(|answer| answer.starts_with("Ok("))
            && consumed_among(self.inputs.as_ref()) == self.consumed_before + 1
            && self.asked_more == 1
    }
}

/// What the first process did before it ended.
#[derive(Debug)]
struct FirstProcess {
    session_id: meerkat_core::SessionId,
    /// The run of the store's tail after the first round.
    run: Option<RunId>,
    /// Every interrupt the run's handle was asked for, by kind, in order.
    interrupts: Vec<&'static str>,
    /// The ending asked the run's interrupt handle for an interrupt.
    interrupt_asked: bool,
    /// The scopes of the writing calls under way when the row began to
    /// hold writing calls, before the run went on.
    writing_before_the_run_went_on: Vec<String>,
    /// The scopes of the calls whose caller waited in their commit when the
    /// interrupt was let go.
    waiting_in_a_commit: Vec<String>,
    /// The scopes of the calls that stood held right after they began to
    /// change their scopes, when the interrupt was let go.
    held_in_a_commit: Vec<String>,
    /// The caller of that commit was gone before the row let the call go.
    caller_gone: bool,
    /// The scopes of the calls that still stood held once the caller was
    /// gone.
    held_without_a_caller: Vec<String>,
    /// The stop's or the cancel's answer, in `Debug` form, when one came.
    ended: Option<String>,
    /// The input's waiter's answer, in `Debug` form, when one came.
    answer: Option<String>,
    /// After a cancel: what a second prompt did.
    next_turn: Option<NextPrompt>,
    model_requests: usize,
    dispatched: usize,
    /// Each poisoned scope, and why.
    poisoned_scopes: Vec<(String, String)>,
}

/// Wait, at most [`ANSWER_DEADLINE`], until `met` holds; whether it did.
async fn until(met: impl FnMut() -> bool) -> bool {
    within(ANSWER_DEADLINE, met).await
}

/// Wait, at most `limit`, until `met` holds; whether it did.
async fn within(limit: Duration, mut met: impl FnMut() -> bool) -> bool {
    let started = tokio::time::Instant::now();
    while !met() {
        if started.elapsed() >= limit {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    true
}

/// Queue `text` for the session and answer its waiter's answer, in `Debug`
/// form, when the prompt was accepted and answered in time.
async fn prompt_answered(
    process: &Process,
    session_id: &meerkat_core::SessionId,
    text: &str,
) -> Option<String> {
    let accepted = process
        .machine
        .accept_input_with_completion(session_id, Input::Prompt(PromptInput::new(text, None)))
        .await;
    let completion = match accepted {
        Ok((_, Some(completion))) => completion,
        Ok((_, None)) => return Some("accepted, with no waiter".to_string()),
        Err(error) => return Some(format!("refused: {error:?}")),
    };
    tokio::time::timeout(ANSWER_DEADLINE, completion.wait())
        .await
        .ok()
        .map(|answer| format!("{answer:?}"))
}

/// Queue one more prompt for the session in `process` and report what it
/// did, from the store and the model's count: the waiter's answer alone
/// does not say the prompt ran.
async fn next_prompt(process: &Process, session_id: &meerkat_core::SessionId) -> NextPrompt {
    let store: &dyn RuntimeStore = process.runtime_store.as_ref();
    let consumed_before = consumed_among(stored_inputs_in(store, session_id).await.as_ref());
    let asked_before = process.client.requests.load(Ordering::SeqCst);
    let answer = prompt_answered(process, session_id, "probe again").await;
    let started = tokio::time::Instant::now();
    let inputs = loop {
        let inputs = stored_inputs_in(store, session_id).await;
        // A prompt that was refused, or answered with an error, is not
        // waited for.
        if consumed_among(inputs.as_ref()) > consumed_before
            || !answer
                .as_deref()
                .is_some_and(|answer| answer.starts_with("Ok("))
            || started.elapsed() >= ANSWER_DEADLINE
        {
            break inputs;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    NextPrompt {
        answer,
        inputs,
        consumed_before,
        asked_more: process.client.requests.load(Ordering::SeqCst) - asked_before,
    }
}

/// The first process, on a runtime of its own: it queues the prompt, lets
/// the first round commit, ends the run as `ending` says while the run is
/// where `meets` says, and reports. It then lives on, writing nothing more
/// of its own accord, until its runtime is shut down.
async fn first_process(
    durable: Arc<Durable>,
    ending: Ending,
    meets: Meets,
    report: tokio::sync::oneshot::Sender<FirstProcess>,
) {
    let process = Process::open(&durable, AcquireMode::Exclusive, meets.script()).await;
    let gate = Arc::new(SuspensionGate::default());
    let session_id = materialize_gated(&process, Arc::clone(&gate)).await;
    let (_, completion) = process
        .machine
        .accept_input_with_completion(
            &session_id,
            Input::Prompt(PromptInput::new("probe once", None)),
        )
        .await
        .expect("the runtime accepts the prompt");
    let completion = completion.expect("a completion waiter");
    // The first round is committed and the second model call waits.
    until(|| process.client.requests.load(Ordering::SeqCst) >= 2).await;
    let runtime_id = LogicalRuntimeId::for_session(&session_id);
    let run = process
        .runtime_store
        .load_whole_blob_provisional_tail(&runtime_id)
        .await
        .ok()
        .flatten()
        .map(|tail| tail.authority().run_id().clone());
    gate.holds.store(meets.a_write(), Ordering::SeqCst);
    let ender = {
        let machine = Arc::clone(&process.machine);
        let session_id = session_id.clone();
        tokio::spawn(async move {
            match ending {
                Ending::Cancel => format!(
                    "{:?}",
                    machine
                        .hard_cancel_current_run(&session_id, "the user stops the run")
                        .await
                ),
                Ending::StopKeepingNothing => {
                    let registration = machine
                        .current_session_registration_witness(&session_id)
                        .await
                        .expect("the session is registered");
                    format!(
                        "{:?}",
                        machine
                            .unregister_session_registration_until_terminal_if_current(
                                &registration
                            )
                            .await
                    )
                }
                Ending::OrderlyStop { max_age } => {
                    let registration = machine
                        .current_session_registration_witness(&session_id)
                        .await
                        .expect("the session is registered");
                    format!(
                        "{:?}",
                        machine
                            .unregister_session_registration_until_terminal_with_queued_inputs_if_current(
                                &registration,
                                meerkat_runtime::UnregisterQueuedInputs::Requeue { max_age },
                            )
                            .await
                    )
                }
            }
        })
    };
    let mut interrupt_asked = false;
    let mut writing_before_the_run_went_on = Vec::new();
    let mut waiting_in_a_commit = Vec::new();
    let mut held_in_a_commit = Vec::new();
    let mut caller_gone = false;
    let mut held_without_a_caller = Vec::new();
    if meets.a_write() {
        interrupt_asked = tokio::time::timeout(ANSWER_DEADLINE, gate.asked.notified())
            .await
            .is_ok();
        // The run's next commit waits where a commit waits for another
        // writer of the store, and the interrupt lands there. No writing
        // call is under way when the hold begins, so the call that comes
        // to stand held is the run's next commit and no earlier one.
        let store = &process.runtime_store;
        let parked = store.park_armed_calls_for_test().await;
        writing_before_the_run_went_on = store.armed_callers_for_test();
        durable.control.model_release.notify_one();
        until(|| {
            !store.armed_callers_for_test().is_empty()
                && !store.parked_armed_calls_for_test().is_empty()
        })
        .await;
        waiting_in_a_commit = store.armed_callers_for_test();
        held_in_a_commit = store.parked_armed_calls_for_test();
        gate.go.notify_one();
        // The hold is still in place: a caller that leaves now was dropped,
        // its call did not answer. A closing save is watched for a while
        // only: no interrupt reaches its caller.
        let watch = match meets {
            Meets::ItsClosingSave => CLOSING_SAVE_WATCH,
            Meets::ARoundCommit | Meets::AModelCall => ANSWER_DEADLINE,
        };
        caller_gone = within(watch, || store.armed_callers_for_test().is_empty()).await;
        held_without_a_caller = store.parked_armed_calls_for_test();
        drop(parked);
    }
    let ended = tokio::time::timeout(STOP_DEADLINE, ender)
        .await
        .ok()
        .map(|joined| joined.unwrap_or_else(|error| format!("the ending's task failed: {error}")));
    let answer = tokio::time::timeout(ANSWER_DEADLINE, completion.wait())
        .await
        .ok()
        .map(|answer| format!("{answer:?}"));
    // A cancel leaves the session where it is: its next turn runs here.
    let next_turn = match ending {
        Ending::Cancel => Some(next_prompt(&process, &session_id).await),
        Ending::StopKeepingNothing | Ending::OrderlyStop { .. } => None,
    };
    let _ = report.send(FirstProcess {
        session_id,
        run,
        interrupts: gate
            .interrupts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone(),
        interrupt_asked,
        writing_before_the_run_went_on,
        waiting_in_a_commit,
        held_in_a_commit,
        caller_gone,
        held_without_a_caller,
        ended,
        answer,
        next_turn,
        model_requests: process.client.requests.load(Ordering::SeqCst),
        dispatched: process.tools.dispatched.load(Ordering::SeqCst),
        poisoned_scopes: process
            .realm
            .poisoned_scopes()
            .into_iter()
            .map(|scope| (scope.id.to_string(), scope.cause))
            .collect(),
    });
    std::future::pending::<()>().await;
}

/// What a restart found.
struct Restarted {
    first: FirstProcess,
    second: Process,
    /// The run of the store's tail at the restart, before the session opens.
    run_at_the_restart: Option<RunId>,
    /// The session's inputs at the restart, before the session opens.
    inputs_at_the_restart: Option<Vec<Option<StoredInput>>>,
    _durable: Arc<Durable>,
}

impl Restarted {
    /// Wait, at most [`TURN_DEADLINE`], until the session's one input reads
    /// `phase` in the restarted process's store; the last reading.
    async fn input_becomes(&self, phase: InputLifecycleState) -> Option<Vec<Option<StoredInput>>> {
        let started = tokio::time::Instant::now();
        loop {
            let read =
                stored_inputs_in(self.second.runtime_store.as_ref(), &self.first.session_id).await;
            let there = matches!(
                read.as_deref(),
                Some([Some(input)]) if input.phase == phase
            );
            if there || started.elapsed() >= TURN_DEADLINE {
                return read;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// The restarted process's model requests and tool runs.
    fn asked_and_ran(&self) -> (usize, usize) {
        (
            self.second.client.requests.load(Ordering::SeqCst),
            self.second.tools.dispatched.load(Ordering::SeqCst),
        )
    }

    /// The session's transcript in the restarted process.
    async fn rows(&self) -> Vec<String> {
        committed_rows_of(&self.second.service, &self.first.session_id).await
    }

    /// Wait, at most [`RESTART_WATCH`], until every input of the session is
    /// ended, then print what the restarted process did of its own accord,
    /// before any row gives it a prompt: the inputs, the model requests and
    /// tool runs, the store's tail and the transcript.
    async fn report_what_it_did_of_its_own_accord(&self) {
        let store: &dyn RuntimeStore = self.second.runtime_store.as_ref();
        let started = tokio::time::Instant::now();
        let inputs = loop {
            let inputs = stored_inputs_in(store, &self.first.session_id).await;
            let all_ended = inputs.as_ref().is_some_and(|inputs| {
                inputs
                    .iter()
                    .all(|input| input.as_ref().is_some_and(|input| input.ended.is_some()))
            });
            if all_ended || started.elapsed() >= RESTART_WATCH {
                break inputs;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        let tail = self
            .second
            .runtime_store
            .load_whole_blob_provisional_tail(&LogicalRuntimeId::for_session(
                &self.first.session_id,
            ))
            .await
            .map(|tail| tail.map(|tail| tail.authority().run_id().clone()));
        eprintln!(
            "the restarted process, of its own accord, after {:?}: inputs {inputs:?}; model \
             requests and tool runs {:?}; tail of run {tail:?}; transcript {:?}",
            started.elapsed(),
            self.asked_and_ran(),
            self.rows().await
        );
    }
}

/// Run the first process to its `ending` with the run where `meets` says,
/// end the process, wait `pause`, and restart over the same store with the
/// session open again.
async fn ended_then_restarted(ending: Ending, meets: Meets, pause: Duration) -> Restarted {
    let durable = Durable::new();
    let first_runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("the first process's runtime");
    let (report, reported) = tokio::sync::oneshot::channel();
    drop(first_runtime.spawn(first_process(Arc::clone(&durable), ending, meets, report)));
    let first = tokio::time::timeout(TURN_DEADLINE * 3, reported)
        .await
        .expect("the first process reports")
        .expect("the first process sends its report");
    first_runtime.shutdown_background();
    eprintln!("first process: {first:#?}");
    tokio::time::sleep(pause).await;
    let second = Process::open(&durable, AcquireMode::Takeover, Vec::new()).await;
    let runtime_id = LogicalRuntimeId::for_session(&first.session_id);
    let run_at_the_restart = second
        .runtime_store
        .load_whole_blob_provisional_tail(&runtime_id)
        .await
        .expect("the restart reads the run's tail")
        .map(|tail| tail.authority().run_id().clone());
    let inputs_at_the_restart =
        stored_inputs_in(second.runtime_store.as_ref(), &first.session_id).await;
    eprintln!(
        "the store at the restart: tail of run {run_at_the_restart:?}, inputs \
         {inputs_at_the_restart:?}"
    );
    let session = second
        .service
        .load_authoritative_session(&first.session_id)
        .await
        .expect("read the committed session")
        .expect("the session is committed");
    second.materialize(session).await;
    let restarted = Restarted {
        first,
        second,
        run_at_the_restart,
        inputs_at_the_restart,
        _durable: durable,
    };
    restarted.report_what_it_did_of_its_own_accord().await;
    restarted
}

/// What every row whose interrupt lands in one of the run's writes asks of
/// its first process. No writing call of the store was under way when the
/// row began to hold them, so the one call that then stood held, right
/// after it began to change the session's runtime scope, is the run's next
/// write. The ending asked the run for `interrupt` first, while that
/// call's caller waited in it. The caller was gone while the call was still
/// held: it was dropped there. The first process left no scope of the store
/// refusing, and the call the run was dropped in went on without its
/// caller.
fn assert_dropped_in_its_commit_without_a_refusing_scope(
    first: &FirstProcess,
    interrupt: &'static str,
) {
    let scope = assert_met_in_a_write(first, interrupt);
    assert!(
        first.caller_gone,
        "the run's call was dropped in its write, while the write was held: {first:#?}"
    );
    assert_eq!(
        first.poisoned_scopes,
        Vec::<(String, String)>::new(),
        "the ending left no scope of the store refusing: {first:#?}"
    );
    assert_eq!(
        first.held_without_a_caller, scope,
        "the write the run was dropped in was still under way once its caller was gone: \
         {first:#?}"
    );
}

/// What every row whose interrupt meets the run in one of its writes asks
/// first, and the session's runtime scope by its name: no writing call of
/// the store was under way when the row began to hold them; the ending
/// asked the run for `interrupt` first; and that interrupt was let go while
/// one call, on the session's runtime scope, stood held with its caller in
/// it.
fn assert_met_in_a_write(first: &FirstProcess, interrupt: &'static str) -> Vec<String> {
    let scope = vec![format!(
        "rt:{}",
        LogicalRuntimeId::for_session(&first.session_id)
    )];
    assert_eq!(
        first.writing_before_the_run_went_on,
        Vec::<String>::new(),
        "no writing call of the store was under way before the run went on: {first:#?}"
    );
    assert_eq!(
        (first.interrupt_asked, first.interrupts.first().copied()),
        (true, Some(interrupt)),
        "the ending asked the run for {interrupt} first: {first:#?}"
    );
    assert_eq!(
        (&first.waiting_in_a_commit, &first.held_in_a_commit),
        (&scope, &scope),
        "the interrupt was let go while the run's write stood held on its runtime's scope, \
         its caller in it: {first:#?}"
    );
    assert_eq!(
        (first.dispatched, first.model_requests >= 2),
        (1, true),
        "the first round ran and the second was asked for: {first:#?}"
    );
    scope
}

/// The transcript keeps the prompt and the first round, each once, ahead of
/// whatever the ending left after them.
fn assert_keeps_the_first_round(rows: &[String]) {
    eprintln!("the transcript: {rows:#?}");
    assert!(
        rows.starts_with(&[
            "user probe once".to_string(),
            "assistant tool use".to_string(),
            "tool results".to_string(),
        ]) && rows
            .iter()
            .filter(|row| row.as_str() == "user probe once")
            .count()
            == 1,
        "the transcript keeps the prompt and the committed first round, each once: {rows:#?}"
    );
}

/// What a stop that keeps the queued work tells the caller waiting on the
/// run's input: the input stays queued across the stop.
fn assert_the_waiter_is_told_its_input_stays_queued(first: &FirstProcess) {
    assert!(
        first
            .answer
            .as_deref()
            .is_some_and(|answer| answer.starts_with("Err(RequeuedAcrossStop {")),
        "the caller waiting on the run's input is told it stays queued across the stop: \
         {first:#?}"
    );
}

/// What a restart finds of a run an orderly stop kept, before the session
/// opens: the store's tail names the run, and its one input is not ended.
fn assert_kept_at_the_restart(restarted: &Restarted) {
    let first = &restarted.first;
    assert!(
        first.run.is_some() && restarted.run_at_the_restart == first.run,
        "the store's tail names the same run at the restart ({:?}): {first:#?}",
        restarted.run_at_the_restart
    );
    assert!(
        matches!(
            restarted.inputs_at_the_restart.as_deref(),
            Some([Some(input)])
                if input.ended.is_none()
                    && !matches!(
                        input.phase,
                        InputLifecycleState::Consumed | InputLifecycleState::Abandoned
                    )
        ),
        "the kept run's input is not ended at the restart: {:?}",
        restarted.inputs_at_the_restart
    );
}

/// The one input of the session, ended as `reason` says, or `None`.
fn ended_as(
    inputs: Option<&Vec<Option<StoredInput>>>,
    reason: &InputAbandonReason,
) -> Option<bool> {
    match inputs.map(Vec::as_slice) {
        Some([Some(input)]) => Some(
            input.phase == InputLifecycleState::Abandoned
                && input.ended
                    == Some(InputTerminalOutcome::Abandoned {
                        reason: reason.clone(),
                    }),
        ),
        _ => None,
    }
}

/// The run continues after an orderly stop exactly as after a crash, also
/// when the stop's suspension lands while the run is inside a round commit.
/// The first round is committed; the stop keeps the queued work for ten
/// minutes; the run is in its second round's first commit, waiting where a
/// commit waits for another writer, when it is asked to suspend. The stop
/// completes and leaves no scope of the store refusing, and the caller
/// waiting on the input is told it stays queued. A restart over the same
/// store inside the max age finds the tail of the same run and its input
/// not ended, and opens the session: the run goes on from its last round
/// commit and completes, its input is consumed once in the run it began in,
/// the transcript holds the prompt and each round once, and each round's
/// call ran once.
#[tokio::test]
async fn a_run_stopped_in_good_order_during_a_round_commit_goes_on_at_the_restart() {
    let restarted = ended_then_restarted(
        Ending::OrderlyStop { max_age: MAX_AGE },
        Meets::ARoundCommit,
        Duration::ZERO,
    )
    .await;
    let first = &restarted.first;
    assert_dropped_in_its_commit_without_a_refusing_scope(first, SUSPENSION);
    assert_eq!(
        first.ended.as_deref(),
        Some("Ok(true)"),
        "the stop completes: {first:#?}"
    );
    assert_the_waiter_is_told_its_input_stays_queued(first);
    assert_kept_at_the_restart(&restarted);
    let inputs = restarted.input_becomes(InputLifecycleState::Consumed).await;
    assert_eq!(
        inputs,
        Some(vec![Some(StoredInput {
            phase: InputLifecycleState::Consumed,
            last_run: first.run.clone(),
            ended: Some(InputTerminalOutcome::Consumed),
        })]),
        "the same run goes on after the restart and completes: its input is consumed once, in \
         the run it began in (at the restart: {:?})",
        restarted.inputs_at_the_restart
    );
    assert_eq!(
        restarted.rows().await,
        vec![
            "user probe once".to_string(),
            "assistant tool use".to_string(),
            "tool results".to_string(),
            "assistant tool use".to_string(),
            "tool results".to_string(),
            "assistant probed".to_string(),
        ],
        "the prompt and each round are in the transcript once: the commit the stop met is kept"
    );
    assert_eq!(
        restarted.asked_and_ran(),
        (1, 1),
        "the run goes on from its second round's first commit: that round's call runs once, \
         and the model is asked once more, for the answer"
    );
}

/// An orderly stop keeps its queued work for its max age only, also when
/// its suspension lands in a round commit: the stop completes and leaves no
/// scope refusing, a restart later than the max age still finds the tail of
/// the run and its input not ended, ends that input as expired without
/// running it when the session opens, keeps the committed first round, and
/// the session's next prompt runs.
#[tokio::test]
async fn a_run_stopped_in_good_order_during_a_round_commit_does_not_go_on_past_the_max_age() {
    let restarted = ended_then_restarted(
        Ending::OrderlyStop {
            max_age: SHORT_MAX_AGE,
        },
        Meets::ARoundCommit,
        SHORT_MAX_AGE * 2,
    )
    .await;
    let first = &restarted.first;
    assert_dropped_in_its_commit_without_a_refusing_scope(first, SUSPENSION);
    assert_eq!(
        first.ended.as_deref(),
        Some("Ok(true)"),
        "the stop completes: {first:#?}"
    );
    assert_the_waiter_is_told_its_input_stays_queued(first);
    assert_kept_at_the_restart(&restarted);
    let inputs = restarted
        .input_becomes(InputLifecycleState::Abandoned)
        .await;
    assert_eq!(
        (
            ended_as(inputs.as_ref(), &InputAbandonReason::RequeueExpired),
            restarted.asked_and_ran()
        ),
        (Some(true), (0, 0)),
        "work kept past its max age is ended as expired when the session opens, and not run: \
         {inputs:?}"
    );
    assert_keeps_the_first_round(&restarted.rows().await);
    let next = next_prompt(&restarted.second, &first.session_id).await;
    assert!(next.ran(), "the session's next prompt runs: {next:#?}");
}

/// A stop that keeps no work ends the run, also when its interrupt lands in
/// a round commit: the stop completes and leaves no scope refusing, the
/// input is ended, a restart runs nothing of it and keeps the committed
/// first round, and the session's next prompt runs.
#[tokio::test]
async fn a_run_stopped_without_keeping_its_work_during_a_round_commit_is_ended() {
    let restarted = ended_then_restarted(
        Ending::StopKeepingNothing,
        Meets::ARoundCommit,
        Duration::ZERO,
    )
    .await;
    let first = &restarted.first;
    assert_dropped_in_its_commit_without_a_refusing_scope(first, HARD_CANCEL);
    assert_eq!(
        first.ended.as_deref(),
        Some("Ok(true)"),
        "the stop completes: {first:#?}"
    );
    assert_eq!(
        (
            restarted
                .inputs_at_the_restart
                .as_deref()
                .map(|inputs| inputs
                    .iter()
                    .map(|input| input.as_ref().map(|input| input.phase))
                    .collect::<Vec<_>>()),
            restarted.asked_and_ran()
        ),
        (Some(vec![Some(InputLifecycleState::Abandoned)]), (0, 0)),
        "the stopped run's input is ended and a restart runs nothing of it: {:?}",
        restarted.inputs_at_the_restart
    );
    assert_keeps_the_first_round(&restarted.rows().await);
    let next = next_prompt(&restarted.second, &first.session_id).await;
    assert!(next.ran(), "the session's next prompt runs: {next:#?}");
}

/// A cancel ends the run, also when it lands in a round commit: the cancel
/// is taken, no scope is left refusing, the session's next prompt runs in
/// the same process, and a restart finds the cancelled run's input ended,
/// the committed first round kept, and runs nothing of the cancelled run.
#[tokio::test]
async fn a_run_cancelled_during_a_round_commit_is_ended_and_the_next_prompt_runs() {
    let restarted = ended_then_restarted(Ending::Cancel, Meets::ARoundCommit, Duration::ZERO).await;
    let first = &restarted.first;
    assert_dropped_in_its_commit_without_a_refusing_scope(first, HARD_CANCEL);
    assert_eq!(
        first.ended.as_deref(),
        Some("Ok(())"),
        "the cancel is taken: {first:#?}"
    );
    assert!(
        first.next_turn.as_ref().is_some_and(NextPrompt::ran),
        "the session's next prompt runs after the cancel: {first:#?}"
    );
    assert_eq!(
        restarted.inputs_at_the_restart.as_ref().map(|inputs| {
            inputs
                .iter()
                .flatten()
                .filter(|input| input.phase == InputLifecycleState::Abandoned)
                .count()
        }),
        Some(1),
        "the cancelled run's input is ended: {:?}",
        restarted.inputs_at_the_restart
    );
    assert_keeps_the_first_round(&restarted.rows().await);
    assert_eq!(
        restarted.asked_and_ran(),
        (0, 0),
        "a restart runs nothing of the cancelled run"
    );
}

/// An orderly stop that meets the run in its closing save, after the
/// model's last answer, does not interrupt it: the run's first write after
/// that answer stands held when the stop's interrupt is let go, and its
/// caller stays in it. The save lands, the stop completes and leaves no
/// scope refusing, and after a restart the run's input is consumed once,
/// in the run it began in, with the answer in the transcript once, no
/// further model request and no call run again.
#[tokio::test]
async fn a_run_stopped_in_good_order_during_its_closing_save_completes_and_is_consumed_once() {
    let restarted = ended_then_restarted(
        Ending::OrderlyStop { max_age: MAX_AGE },
        Meets::ItsClosingSave,
        Duration::ZERO,
    )
    .await;
    let first = &restarted.first;
    let scope = assert_met_in_a_write(first, SUSPENSION);
    assert_eq!(
        (
            first.caller_gone,
            &first.held_without_a_caller,
            first.poisoned_scopes.len()
        ),
        (false, &scope, 0),
        "no interrupt drops the run's closing save: its caller stays in the held write, and no \
         scope is left refusing: {first:#?}"
    );
    assert_eq!(
        first.ended.as_deref(),
        Some("Ok(true)"),
        "the stop completes: {first:#?}"
    );
    let inputs = restarted.input_becomes(InputLifecycleState::Consumed).await;
    assert_eq!(
        inputs,
        Some(vec![Some(StoredInput {
            phase: InputLifecycleState::Consumed,
            last_run: first.run.clone(),
            ended: Some(InputTerminalOutcome::Consumed),
        })]),
        "the run's input is consumed once, in the run it began in (at the restart: {:?})",
        restarted.inputs_at_the_restart
    );
    assert_eq!(
        (restarted.rows().await, restarted.asked_and_ran()),
        (
            vec![
                "user probe once".to_string(),
                "assistant tool use".to_string(),
                "tool results".to_string(),
                "assistant probed".to_string(),
            ],
            (0, 0)
        ),
        "the answer is in the transcript once, and the restart asks the model for nothing and \
         runs no call again"
    );
}

/// An orderly stop that meets the run in a model call keeps it, and the run
/// goes on at a restart inside the max age, as it always did.
#[tokio::test]
async fn a_run_stopped_in_good_order_during_a_model_call_goes_on_at_the_restart() {
    let restarted = ended_then_restarted(
        Ending::OrderlyStop { max_age: MAX_AGE },
        Meets::AModelCall,
        Duration::ZERO,
    )
    .await;
    let first = &restarted.first;
    assert_eq!(
        (
            first.ended.as_deref(),
            first.poisoned_scopes.len(),
            first.interrupts.first().copied()
        ),
        (Some("Ok(true)"), 0, Some(SUSPENSION)),
        "the stop asks the run for a suspension, completes and leaves no scope refusing: \
         {first:#?}"
    );
    assert_the_waiter_is_told_its_input_stays_queued(first);
    assert_kept_at_the_restart(&restarted);
    let inputs = restarted.input_becomes(InputLifecycleState::Consumed).await;
    assert_eq!(
        inputs,
        Some(vec![Some(StoredInput {
            phase: InputLifecycleState::Consumed,
            last_run: first.run.clone(),
            ended: Some(InputTerminalOutcome::Consumed),
        })]),
        "the same run goes on after the restart and completes: its input is consumed once, in \
         the run it began in (at the restart: {:?})",
        restarted.inputs_at_the_restart
    );
    assert_eq!(
        (restarted.rows().await, restarted.asked_and_ran()),
        (
            vec![
                "user probe once".to_string(),
                "assistant tool use".to_string(),
                "tool results".to_string(),
                "assistant probed".to_string(),
            ],
            (1, 0)
        ),
        "the run goes on from its first round: the model is asked again, and no call runs twice"
    );
}

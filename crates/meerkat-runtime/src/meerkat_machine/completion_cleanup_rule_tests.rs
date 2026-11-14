//! The completion cleanup rule (`ResolveRuntimeCompletionCleanup`), asked on
//! the authority of real sessions in each runtime phase, and the phase in
//! which each way of ending a runtime answers its waiters.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use chrono::Utc;
use meerkat_core::lifecycle::core_executor::{CoreApplyOutput, CoreExecutor, CoreExecutorError};
use meerkat_core::lifecycle::run_primitive::RunApplyBoundary;
use meerkat_core::lifecycle::run_primitive::RunPrimitive;
use meerkat_core::lifecycle::run_receipt::RunBoundaryReceiptDraft;

use crate::completion::{CompletionHandle, CompletionOutcome};
use crate::meerkat_machine::dsl as mm;
use crate::meerkat_machine::dsl_authority::runtime_phase_from_authority;
use crate::runtime_state::RuntimeState;
use crate::traits::RuntimeControlPlane;

const BOUND: Duration = Duration::from_secs(30);

type SharedAuthority = Arc<std::sync::Mutex<mm::MeerkatMachineAuthority>>;

use mm::RuntimeCompletionCleanupAction as Action;
use mm::RuntimeCompletionLiveSessionObservation as Live;
use mm::RuntimeCompletionObservedOutcome as Outcome;
use mm::RuntimeCompletionPreAdmissionAction as PreAdmission;

const OUTCOMES: [Outcome; 9] = [
    Outcome::Completed,
    Outcome::CompletedWithoutResult,
    Outcome::CallbackPending,
    Outcome::Cancelled,
    Outcome::Abandoned,
    Outcome::RuntimeApplyFailed,
    Outcome::FinalizationFailed,
    Outcome::RuntimeTerminated,
    Outcome::OutcomeUnknown,
];

const LIVE: [Live; 3] = [Live::NotObserved, Live::Present, Live::Absent];

/// The phases in which a runtime takes and runs input.
fn serves(phase: RuntimeState) -> bool {
    matches!(
        phase,
        RuntimeState::Idle | RuntimeState::Attached | RuntimeState::Running
    )
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn keyed_event(key: &str) -> Input {
    Input::ExternalEvent(crate::input::ExternalEventInput {
        objective_id: None,
        header: crate::input::InputHeader {
            ingress_context: None,
            retained_resume: None,
            authority_association: None,
            id: InputId::new(),
            timestamp: Utc::now(),
            source: crate::input::InputOrigin::External {
                source_name: "completion-cleanup-rule-test".into(),
            },
            durability: crate::input::InputDurability::Durable,
            visibility: crate::input::InputVisibility::default(),
            idempotency_key: Some(crate::identifiers::IdempotencyKey::new(key)),
            supersession_key: None,
            correlation_id: None,
            requester: None,
        },
        event_type: "completion_cleanup_rule".into(),
        payload: serde_json::json!({ "key": key }),
        blocks: None,
        handling_mode: meerkat_core::types::HandlingMode::Queue,
        render_metadata: None,
    })
}

fn persistent_machine() -> Arc<MeerkatMachine> {
    Arc::new(
        MeerkatMachine::persistent(
            Arc::new(crate::store::InMemoryRuntimeStore::new()),
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        )
        .expect("persistent machine"),
    )
}

async fn registered(machine: &Arc<MeerkatMachine>) -> SessionId {
    let session_id = SessionId::new();
    machine
        .register_session(session_id.clone())
        .await
        .expect("register session");
    session_id
}

async fn accept(
    machine: &MeerkatMachine,
    session_id: &SessionId,
    key: &str,
) -> (InputId, CompletionHandle) {
    let input = keyed_event(key);
    let input_id = input.id().clone();
    let (outcome, handle) = machine
        .accept_input_with_completion(session_id, input)
        .await
        .expect("accept keyed event");
    assert!(outcome.is_accepted());
    (
        input_id,
        handle.expect("an accepted input has a completion handle"),
    )
}

/// Take the session mutation gate and give it back at once, so the accepted
/// boundary work of an admission is done before the test goes on.
async fn take_the_gate_once(machine: &MeerkatMachine, session_id: &SessionId) {
    drop(
        machine
            .session_mutation_gate(session_id)
            .await
            .expect("registered session gate")
            .lock_settled(SettledGateWait::TestFixture)
            .await,
    );
}

async fn session_authority(machine: &MeerkatMachine, session_id: &SessionId) -> SharedAuthority {
    let driver = machine
        .sessions
        .read()
        .await
        .get(session_id)
        .expect("registered session")
        .driver
        .clone();
    let driver = driver.lock().await;
    driver.shared_dsl_authority()
}

fn phase_of(authority: &SharedAuthority) -> RuntimeState {
    let authority = authority
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    runtime_phase_from_authority(&authority)
}

async fn wait_for_phase(authority: &SharedAuthority, wanted: RuntimeState) {
    tokio::time::timeout(BOUND, async {
        while phase_of(authority) != wanted {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "the runtime reaches {wanted}; it is {}",
            phase_of(authority)
        )
    });
}

/// Executor whose every apply waits for one permit. A closed gate ends the
/// apply as stopped.
struct GatedExecutor {
    session_id: SessionId,
    permits: Arc<crate::tokio::sync::Semaphore>,
    applies: Arc<AtomicUsize>,
}

fn run_result(session_id: &SessionId, text: &str) -> meerkat_core::RunResult {
    serde_json::from_value(serde_json::json!({
        "text": text,
        "session_id": session_id,
        "usage": meerkat_core::Usage::default(),
        "turns": 1,
        "tool_calls": 0,
    }))
    .expect("minimal RunResult")
}

#[async_trait::async_trait]
impl CoreExecutor for GatedExecutor {
    async fn apply(
        &mut self,
        run_id: RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        let call = self.applies.fetch_add(1, Ordering::SeqCst) + 1;
        let contributors = primitive.contributing_input_ids().to_vec();
        self.permits
            .acquire()
            .await
            .map_err(|_| CoreExecutorError::Stopped)?
            .forget();
        Ok(CoreApplyOutput::with_run_result(
            RunBoundaryReceiptDraft {
                run_id,
                boundary: RunApplyBoundary::RunStart,
                contributing_input_ids: contributors,
                conversation_digest: None,
                message_count: 0,
            },
            None,
            run_result(&self.session_id, &format!("answer-{call}")),
        ))
    }

    async fn cancel_after_boundary(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        Ok(())
    }

    async fn stop_runtime_executor(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        Ok(())
    }
}

struct Served {
    session_id: SessionId,
    permits: Arc<crate::tokio::sync::Semaphore>,
    applies: Arc<AtomicUsize>,
}

impl Served {
    async fn new(machine: &Arc<MeerkatMachine>) -> Self {
        let session_id = SessionId::new();
        let permits = Arc::new(crate::tokio::sync::Semaphore::new(0));
        let applies = Arc::new(AtomicUsize::new(0));
        machine
            .register_session_with_executor(
                session_id.clone(),
                Box::new(GatedExecutor {
                    session_id: session_id.clone(),
                    permits: Arc::clone(&permits),
                    applies: Arc::clone(&applies),
                }),
            )
            .await
            .expect("register gated executor");
        Self {
            session_id,
            permits,
            applies,
        }
    }

    async fn wait_for_applies(&self, count: usize) {
        tokio::time::timeout(BOUND, async {
            while self.applies.load(Ordering::SeqCst) < count {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("executor apply starts");
    }
}

// ---------------------------------------------------------------------------
// The rule, asked in each phase
// ---------------------------------------------------------------------------

/// The registration an observation names: what the rule's binding guard
/// compares with the authority's own.
#[derive(Clone)]
struct Binding {
    session_id: mm::SessionId,
    runtime_id: Option<mm::AgentRuntimeId>,
    fence_token: Option<mm::FenceToken>,
    generation: Option<mm::Generation>,
    epoch_id: Option<mm::RuntimeEpochId>,
}

fn binding_of(authority: &SharedAuthority, session_id: &SessionId) -> Binding {
    let authority = authority
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let state = authority.state();
    Binding {
        session_id: mm::SessionId::from_domain(session_id),
        runtime_id: state.active_runtime_id.clone(),
        fence_token: state.active_fence_token,
        generation: state.active_runtime_generation,
        epoch_id: state.active_runtime_epoch_id.clone(),
    }
}

/// Ask the rule on a fork of `authority`, as the runtime does.
fn ask(
    authority: &SharedAuthority,
    binding: &Binding,
    outcome: Outcome,
    archived: bool,
    live: Live,
) -> Result<(Action, PreAdmission), String> {
    let input = mm::MeerkatMachineInput::ResolveRuntimeCompletionCleanup {
        session_id: binding.session_id.clone(),
        observation_session_id: binding.session_id.clone(),
        observation_agent_runtime_id: binding.runtime_id.clone(),
        observation_fence_token: binding.fence_token,
        observation_runtime_generation: binding.generation,
        observation_runtime_epoch_id: binding.epoch_id.clone(),
        outcome,
        archived_by_authority: archived,
        live_session: live,
    };
    let effects = MeerkatMachine::preview_dsl_input_on_shared_authority(
        authority,
        input,
        "ResolveRuntimeCompletionCleanup",
    )?;
    effects
        .iter()
        .find_map(|effect| match effect {
            mm::MeerkatMachineEffect::RuntimeCompletionCleanupResolved {
                action,
                pre_admission_action,
                ..
            } => Some((*action, *pre_admission_action)),
            _ => None,
        })
        .ok_or_else(|| "the rule emitted no cleanup effect".to_string())
}

/// One authority in each runtime phase, each the authority of a real session
/// brought there by the runtime's own operations (Initializing: a machine
/// authority before its first signal). The machines are returned so the
/// sessions stay registered while the rows ask.
struct Phases {
    _machines: Vec<Arc<MeerkatMachine>>,
    _served: Vec<Served>,
    authorities: Vec<(RuntimeState, SharedAuthority, Binding)>,
}

impl Phases {
    fn get(&self, phase: RuntimeState) -> (&SharedAuthority, &Binding) {
        self.authorities
            .iter()
            .find(|(candidate, _, _)| *candidate == phase)
            .map(|(_, authority, binding)| (authority, binding))
            .unwrap_or_else(|| panic!("the fixture has an authority in {phase}"))
    }
}

async fn an_authority_in_each_phase() -> Phases {
    let mut machines = Vec::new();
    let mut served_sessions = Vec::new();
    let mut authorities = Vec::new();

    // Initializing: no session is registered yet.
    {
        let session_id = SessionId::new();
        let authority: SharedAuthority =
            Arc::new(std::sync::Mutex::new(mm::MeerkatMachineAuthority::new()));
        let binding = binding_of(&authority, &session_id);
        authorities.push((RuntimeState::Initializing, authority, binding));
    }
    // Idle: registered, no executor.
    {
        let machine = persistent_machine();
        let session_id = registered(&machine).await;
        let authority = session_authority(&machine, &session_id).await;
        let binding = binding_of(&authority, &session_id);
        authorities.push((RuntimeState::Idle, authority, binding));
        machines.push(machine);
    }
    // Attached: an executor serves it, nothing runs.
    {
        let machine = persistent_machine();
        let served = Served::new(&machine).await;
        let authority = session_authority(&machine, &served.session_id).await;
        wait_for_phase(&authority, RuntimeState::Attached).await;
        let binding = binding_of(&authority, &served.session_id);
        authorities.push((RuntimeState::Attached, authority, binding));
        machines.push(machine);
        served_sessions.push(served);
    }
    // Running: its executor holds a run.
    {
        let machine = persistent_machine();
        let served = Served::new(&machine).await;
        let (_input_id, _handle) = accept(&machine, &served.session_id, "running").await;
        served.wait_for_applies(1).await;
        let authority = session_authority(&machine, &served.session_id).await;
        wait_for_phase(&authority, RuntimeState::Running).await;
        let binding = binding_of(&authority, &served.session_id);
        authorities.push((RuntimeState::Running, authority, binding));
        machines.push(machine);
        served_sessions.push(served);
    }
    // Retired.
    {
        let machine = persistent_machine();
        let session_id = registered(&machine).await;
        let authority = session_authority(&machine, &session_id).await;
        <MeerkatMachine as SessionServiceRuntimeExt>::retire_runtime(&machine, &session_id)
            .await
            .expect("retire");
        wait_for_phase(&authority, RuntimeState::Retired).await;
        let binding = binding_of(&authority, &session_id);
        authorities.push((RuntimeState::Retired, authority, binding));
        machines.push(machine);
    }
    // Stopped.
    {
        let machine = persistent_machine();
        let served = Served::new(&machine).await;
        let authority = session_authority(&machine, &served.session_id).await;
        wait_for_phase(&authority, RuntimeState::Attached).await;
        machine
            .stop_runtime_executor(&served.session_id, "stop for the fixture")
            .await
            .expect("stop");
        wait_for_phase(&authority, RuntimeState::Stopped).await;
        let binding = binding_of(&authority, &served.session_id);
        authorities.push((RuntimeState::Stopped, authority, binding));
        machines.push(machine);
        served_sessions.push(served);
    }
    // Destroyed.
    {
        let machine = persistent_machine();
        let session_id = registered(&machine).await;
        let authority = session_authority(&machine, &session_id).await;
        RuntimeControlPlane::destroy(&*machine, &MeerkatMachine::logical_runtime_id(&session_id))
            .await
            .expect("destroy");
        wait_for_phase(&authority, RuntimeState::Destroyed).await;
        let binding = binding_of(&authority, &session_id);
        authorities.push((RuntimeState::Destroyed, authority, binding));
        machines.push(machine);
    }

    for (phase, authority, _) in &authorities {
        assert_eq!(
            phase_of(authority),
            *phase,
            "each fixture authority is in the phase it is listed under"
        );
    }
    Phases {
        _machines: machines,
        _served: served_sessions,
        authorities,
    }
}

const RULE_PHASES: [RuntimeState; 6] = [
    RuntimeState::Initializing,
    RuntimeState::Idle,
    RuntimeState::Attached,
    RuntimeState::Running,
    RuntimeState::Retired,
    RuntimeState::Stopped,
];

/// The rule's answer, as a table over (outcome, archived, live session).
fn the_table(outcome: Outcome, archived: bool, live: Live) -> Action {
    if archived {
        return Action::CleanupArchivedSession;
    }
    match outcome {
        Outcome::RuntimeTerminated | Outcome::FinalizationFailed | Outcome::RuntimeApplyFailed => {
            Action::CleanupRuntime
        }
        Outcome::Abandoned | Outcome::Cancelled => {
            if live == Live::Absent {
                Action::CleanupRuntime
            } else {
                Action::RetainRuntime
            }
        }
        Outcome::Completed
        | Outcome::CompletedWithoutResult
        | Outcome::CallbackPending
        | Outcome::OutcomeUnknown => Action::RetainRuntime,
    }
}

/// An archived session's runtime is cleaned up as the runtime of an archived
/// session (`CleanupArchivedSession`: the cleanup that also ends what the
/// session owns), whatever the outcome, the live-session read and the phase
/// the runtime is in, and the request's pre-admission is released. No other
/// cell of the table names that cleanup.
#[tokio::test]
async fn an_archived_session_is_cleaned_up_in_every_phase() {
    let phases = an_authority_in_each_phase().await;
    let mut asked = 0;
    for phase in RULE_PHASES {
        let (authority, binding) = phases.get(phase);
        for outcome in OUTCOMES {
            for live in LIVE {
                assert_eq!(
                    ask(authority, binding, outcome, true, live),
                    Ok((
                        Action::CleanupArchivedSession,
                        PreAdmission::ReleasePreAdmission
                    )),
                    "archived, {phase}, {outcome:?}, {live:?}"
                );
                let not_archived =
                    ask(authority, binding, outcome, false, live).map(|(action, _)| action);
                assert_ne!(
                    not_archived,
                    Ok(Action::CleanupArchivedSession),
                    "not archived, {phase}, {outcome:?}, {live:?}"
                );
                asked += 1;
            }
        }
    }
    assert_eq!(asked, 6 * 9 * 3);
}

/// A runtime that no longer serves (and one that never started) is cleaned
/// up for a terminated input, for a failed apply and for a failed
/// finalization; a cancelled or abandoned input cleans up only when no live
/// session is left; every other outcome keeps the runtime. In the serving
/// phases the same table holds for every outcome but a terminated input,
/// which this row leaves to its own rows.
#[tokio::test]
async fn the_cleanup_rule_keeps_its_table_in_every_phase() {
    let phases = an_authority_in_each_phase().await;
    let mut asked = 0;
    for phase in RULE_PHASES {
        let (authority, binding) = phases.get(phase);
        for outcome in OUTCOMES {
            for live in LIVE {
                if serves(phase) && outcome == Outcome::RuntimeTerminated {
                    continue;
                }
                assert_eq!(
                    ask(authority, binding, outcome, false, live),
                    Ok((
                        the_table(outcome, false, live),
                        PreAdmission::ReleasePreAdmission
                    )),
                    "not archived, {phase}, {outcome:?}, {live:?}"
                );
                asked += 1;
            }
        }
    }
    assert_eq!(asked, 6 * 9 * 3 - 3 * 3);
}

/// The rule answers nothing for an observation that names another
/// registration (another runtime, fence, generation, epoch or session), in
/// any phase, and nothing at all for a destroyed runtime.
#[tokio::test]
async fn the_cleanup_rule_refuses_another_registration_and_a_destroyed_runtime() {
    let phases = an_authority_in_each_phase().await;
    for phase in RULE_PHASES {
        let (authority, binding) = phases.get(phase);
        let mut others = Vec::new();
        let mut other = binding.clone();
        other.runtime_id = Some(mm::AgentRuntimeId("another-runtime".into()));
        others.push(("runtime", other));
        let mut other = binding.clone();
        other.fence_token = Some(mm::FenceToken(
            binding.fence_token.map_or(7, |token| token.0 + 1),
        ));
        others.push(("fence", other));
        let mut other = binding.clone();
        other.generation = Some(mm::Generation(
            binding.generation.map_or(7, |generation| generation.0 + 1),
        ));
        others.push(("generation", other));
        let mut other = binding.clone();
        other.epoch_id = Some(mm::RuntimeEpochId("another-epoch".into()));
        others.push(("epoch", other));
        for (what, other) in &others {
            for outcome in OUTCOMES {
                for archived in [false, true] {
                    assert!(
                        ask(authority, other, outcome, archived, Live::NotObserved).is_err(),
                        "another {what}, {phase}, {outcome:?}, archived {archived}"
                    );
                }
            }
        }
        // Another session: the observation's session is not the asked one.
        let input = mm::MeerkatMachineInput::ResolveRuntimeCompletionCleanup {
            session_id: binding.session_id.clone(),
            observation_session_id: mm::SessionId::from_domain(&SessionId::new()),
            observation_agent_runtime_id: binding.runtime_id.clone(),
            observation_fence_token: binding.fence_token,
            observation_runtime_generation: binding.generation,
            observation_runtime_epoch_id: binding.epoch_id.clone(),
            outcome: Outcome::RuntimeTerminated,
            archived_by_authority: true,
            live_session: Live::NotObserved,
        };
        assert!(
            MeerkatMachine::preview_dsl_input_on_shared_authority(
                authority,
                input,
                "ResolveRuntimeCompletionCleanup"
            )
            .is_err(),
            "another session, {phase}"
        );
    }

    let (authority, binding) = phases.get(RuntimeState::Destroyed);
    for outcome in OUTCOMES {
        for archived in [false, true] {
            for live in LIVE {
                assert!(
                    ask(authority, binding, outcome, archived, live).is_err(),
                    "destroyed, {outcome:?}, archived {archived}, {live:?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The phase in which a waiter is answered
// ---------------------------------------------------------------------------

/// What the probe read when a waiter was taken out to be answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReleasePhase {
    /// The runtime's lifecycle phase at that moment.
    In(RuntimeState),
    /// The authority was locked at that moment: the phase was not read.
    AuthorityLocked,
}

impl ReleasePhase {
    fn phase(self) -> RuntimeState {
        match self {
            Self::In(phase) => phase,
            Self::AuthorityLocked => {
                panic!("the authority was locked when the waiter left the registry")
            }
        }
    }
}

type Releases = Arc<std::sync::Mutex<Vec<(InputId, ReleasePhase)>>>;

/// Record, for every waiter the session's registry answers from now on, the
/// runtime phase at the moment the waiter is taken out to be answered.
async fn watch_releases(machine: &MeerkatMachine, session_id: &SessionId) -> Releases {
    let (driver, completions) = {
        let sessions = machine.sessions.read().await;
        let entry = sessions.get(session_id).expect("registered session");
        (entry.driver.clone(), entry.completions.clone())
    };
    let authority = driver.lock().await.shared_dsl_authority();
    let releases = Releases::default();
    let seen = Arc::clone(&releases);
    completions
        .lock()
        .await
        .set_waiter_release_probe_for_test(Arc::new(move |input_id: &InputId| {
            let phase = authority
                .try_lock()
                .map_or(ReleasePhase::AuthorityLocked, |authority| {
                    ReleasePhase::In(runtime_phase_from_authority(&authority))
                });
            seen.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((input_id.clone(), phase));
        }));
    releases
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ending {
    Stop,
    Retire,
    Destroy,
    Unregister,
    UnregisterCurrentStop,
    UnregisterCurrentRequeue,
    Reset,
    Recall,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// No executor: one input queued.
    Queued,
    /// An executor holds a run of the first input; a second is queued.
    RunningAndQueued,
    /// As `RunningAndQueued`, and the held run ends as stopped a moment
    /// after the ending is asked for (an executor that gives up its run).
    StoppedRunAndQueued,
}

#[derive(Debug)]
struct Answered {
    label: &'static str,
    /// What the probe read when the waiter was taken out to be answered
    /// (`None`: no waiter of the input was seen leaving the registry).
    phase_at_release: Option<ReleasePhase>,
    /// The outcome's name, or the wait failure.
    answer: String,
    terminated: bool,
    /// What the rule answers a relay that asks right after the waiter's
    /// answer, with the observation it was given.
    rule: Option<Result<Action, String>>,
}

async fn end_and_collect(ending: Ending, shape: Shape, persistent: bool) -> Vec<Answered> {
    let machine = if persistent {
        persistent_machine()
    } else {
        Arc::new(MeerkatMachine::ephemeral())
    };
    let mut waiters: Vec<(&'static str, InputId, CompletionHandle)> = Vec::new();
    let (session_id, served) = match shape {
        Shape::Queued => {
            let session_id = registered(&machine).await;
            (session_id, None)
        }
        Shape::RunningAndQueued | Shape::StoppedRunAndQueued => {
            let served = Served::new(&machine).await;
            (served.session_id.clone(), Some(served))
        }
    };
    let releases = watch_releases(&machine, &session_id).await;
    match &served {
        None => {
            let (input_id, handle) = accept(&machine, &session_id, "queued").await;
            take_the_gate_once(&machine, &session_id).await;
            waiters.push(("queued", input_id, handle));
        }
        Some(served) => {
            let (input_id, handle) = accept(&machine, &session_id, "running").await;
            served.wait_for_applies(1).await;
            waiters.push(("running", input_id, handle));
            let (input_id, handle) = accept(&machine, &session_id, "queued").await;
            waiters.push(("queued", input_id, handle));
        }
    }
    let queued_input = waiters
        .iter()
        .find(|(label, _, _)| *label == "queued")
        .map(|(_, input_id, _)| input_id.clone())
        .expect("a queued input");

    let ending_machine = Arc::clone(&machine);
    let ending_session = session_id.clone();
    let mut task = tokio::spawn(async move {
        let machine = ending_machine;
        let session_id = ending_session;
        match ending {
            Ending::Stop => machine
                .stop_runtime_executor(&session_id, "stop")
                .await
                .map(|()| "stopped".to_string())
                .map_err(|error| error.to_string()),
            Ending::Retire => {
                <MeerkatMachine as SessionServiceRuntimeExt>::retire_runtime(&machine, &session_id)
                    .await
                    .map(|report| format!("{report:?}"))
                    .map_err(|error| error.to_string())
            }
            Ending::Destroy => RuntimeControlPlane::destroy(
                &*machine,
                &MeerkatMachine::logical_runtime_id(&session_id),
            )
            .await
            .map(|report| format!("{report:?}"))
            .map_err(|error| error.to_string()),
            Ending::Unregister => machine
                .unregister_session(&session_id)
                .await
                .map(|()| "unregistered".to_string())
                .map_err(|error| error.to_string()),
            Ending::UnregisterCurrentStop => {
                let witness = machine
                    .current_session_registration_witness(&session_id)
                    .await
                    .expect("a current registration");
                machine
                    .unregister_session_registration_until_terminal_if_current(&witness)
                    .await
                    .map(|done| format!("unregistered {done}"))
                    .map_err(|error| error.to_string())
            }
            Ending::UnregisterCurrentRequeue => {
                let witness = machine
                    .current_session_registration_witness(&session_id)
                    .await
                    .expect("a current registration");
                machine
                    .unregister_session_registration_until_terminal_with_queued_inputs_if_current(
                        &witness,
                        UnregisterQueuedInputs::Requeue {
                            max_age: Duration::from_mins(10),
                        },
                    )
                    .await
                    .map(|done| format!("unregistered {done}"))
                    .map_err(|error| error.to_string())
            }
            Ending::Reset => {
                <MeerkatMachine as SessionServiceRuntimeExt>::reset_runtime(&machine, &session_id)
                    .await
                    .map(|report| format!("{report:?}"))
                    .map_err(|error| error.to_string())
            }
            Ending::Recall => machine
                .recall_queued_input(&session_id, &queued_input)
                .await
                .map(|answer| format!("{answer:?}"))
                .map_err(|error| error.to_string()),
        }
    });
    if shape == Shape::StoppedRunAndQueued {
        // The executor gives up its held run shortly after the ending is
        // asked for: the apply ends as stopped.
        tokio::time::sleep(Duration::from_millis(100)).await;
        if let Some(served) = &served {
            served.permits.close();
        }
    }
    // An ending that waits for the held run gets it: the gate opens after a
    // moment, so the run that was in flight ends by itself.
    let ended = match tokio::time::timeout(Duration::from_secs(2), &mut task).await {
        Ok(joined) => joined.expect("the ending does not panic"),
        Err(_) => {
            if let Some(served) = &served {
                served.permits.add_permits(8);
            }
            tokio::time::timeout(BOUND, task)
                .await
                .expect("the ending returns once the held run is let go")
                .expect("the ending does not panic")
        }
    };
    println!("{ending:?} {shape:?} persistent={persistent}: {ended:?}");
    // Whatever the ending left held ends by itself from here on.
    if let Some(served) = &served
        && !served.permits.is_closed()
    {
        served.permits.add_permits(8);
    }

    let mut answered = Vec::new();
    for (label, input_id, handle) in waiters {
        let waited = tokio::time::timeout(
            Duration::from_secs(5),
            handle.try_wait_with_cleanup_observation(),
        )
        .await;
        let phase_at_release = releases
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .find(|(released, _)| *released == input_id)
            .map(|(_, phase)| *phase);
        let row = match waited {
            Err(_) => Answered {
                label,
                phase_at_release,
                answer: "not answered".into(),
                terminated: false,
                rule: None,
            },
            Ok(Err(error)) => Answered {
                label,
                phase_at_release,
                answer: format!("wait failure: {error:?}"),
                terminated: false,
                rule: None,
            },
            Ok(Ok((release, observation))) => {
                let outcome = release.release().await.expect("the outcome is released");
                let terminated = matches!(outcome, CompletionOutcome::RuntimeTerminated { .. });
                let name = match &outcome {
                    CompletionOutcome::Completed(_) => "Completed".to_string(),
                    CompletionOutcome::RuntimeTerminated { .. } => "RuntimeTerminated".to_string(),
                    other => format!("{other:?}"),
                };
                let rule = machine
                    .resolve_runtime_completion_cleanup(
                        &session_id,
                        observation,
                        false,
                        Live::NotObserved,
                    )
                    .await
                    .map(|authority| authority.action)
                    .map_err(|error| error.to_string());
                Answered {
                    label,
                    phase_at_release,
                    answer: name,
                    terminated,
                    rule: Some(rule),
                }
            }
        };
        println!("    {row:?}");
        answered.push(row);
    }
    if let Some(served) = &served {
        served.permits.close();
    }
    answered
}

const ENDINGS: [Ending; 6] = [
    Ending::Stop,
    Ending::Retire,
    Ending::Destroy,
    Ending::Unregister,
    Ending::UnregisterCurrentStop,
    Ending::UnregisterCurrentRequeue,
];

/// Whatever ends a runtime (a stop, a retire, a destroy, an unregister that
/// stops or requeues), with an input queued alone or behind a held run, on a
/// stored and on a process-only runtime: no waiter is answered `terminated`
/// while the runtime's phase still serves.
#[tokio::test]
async fn no_ending_of_a_runtime_answers_a_waiter_terminated_while_the_runtime_serves() {
    let mut terminated_answers = 0;
    for persistent in [true, false] {
        for shape in [
            Shape::Queued,
            Shape::RunningAndQueued,
            Shape::StoppedRunAndQueued,
        ] {
            for ending in ENDINGS {
                for row in end_and_collect(ending, shape, persistent).await {
                    if !row.terminated {
                        continue;
                    }
                    terminated_answers += 1;
                    let phase = row
                        .phase_at_release
                        .expect("an answered waiter was seen leaving the registry")
                        .phase();
                    assert!(
                        !serves(phase),
                        "{ending:?}, {shape:?}, persistent {persistent}: the {} input's waiter was answered terminated in {phase}",
                        row.label
                    );
                }
            }
        }
    }
    assert!(
        terminated_answers > 0,
        "the row saw no terminated answer at all"
    );
}

/// The other side: taking one queued input back, and a reset of a runtime
/// with nothing running (a reset is refused while a run is in flight),
/// answer the input's waiter `terminated` while the runtime goes on serving.
#[tokio::test]
async fn a_recall_and_a_reset_answer_a_waiter_terminated_while_the_runtime_serves() {
    for persistent in [true, false] {
        for (ending, shape) in [
            (Ending::Recall, Shape::Queued),
            (Ending::Recall, Shape::RunningAndQueued),
            (Ending::Reset, Shape::Queued),
        ] {
            let rows = end_and_collect(ending, shape, persistent).await;
            let queued = rows
                .iter()
                .find(|row| row.label == "queued")
                .expect("the queued input's row");
            assert!(
                queued.terminated,
                "{ending:?}, {shape:?}, persistent {persistent}: {queued:?}"
            );
            let phase = queued
                .phase_at_release
                .expect("the waiter was seen leaving the registry")
                .phase();
            assert!(
                serves(phase),
                "{ending:?}, {shape:?}, persistent {persistent}: answered in {phase}"
            );
        }
    }
}

/// A reset takes every queued input back and ends nothing else: the session
/// stays registered in a serving phase and takes the next input, and a
/// served runtime runs its next input after a reset and answers it.
#[tokio::test]
async fn a_reset_runtime_goes_on_serving() {
    // No executor: the queued input is reset away, the registration stays.
    let machine = persistent_machine();
    let session_id = registered(&machine).await;
    let (_reset_away, handle) = accept(&machine, &session_id, "reset away").await;
    take_the_gate_once(&machine, &session_id).await;
    let report = <MeerkatMachine as SessionServiceRuntimeExt>::reset_runtime(&machine, &session_id)
        .await
        .expect("reset");
    assert_eq!(report.inputs_abandoned, 1);
    let outcome = tokio::time::timeout(BOUND, handle.try_wait())
        .await
        .expect("the reset input's waiter is answered")
        .expect("with an outcome");
    assert!(
        matches!(outcome, CompletionOutcome::RuntimeTerminated { .. }),
        "the reset input is answered terminated: {outcome:?}"
    );
    let authority = session_authority(&machine, &session_id).await;
    assert_eq!(
        phase_of(&authority),
        RuntimeState::Idle,
        "the reset runtime is still registered and serves"
    );
    let (_next, _next_handle) = accept(&machine, &session_id, "after the reset").await;

    // A served runtime: after a reset its next input runs and is answered.
    let machine = persistent_machine();
    let served = Served::new(&machine).await;
    let authority = session_authority(&machine, &served.session_id).await;
    wait_for_phase(&authority, RuntimeState::Attached).await;
    <MeerkatMachine as SessionServiceRuntimeExt>::reset_runtime(&machine, &served.session_id)
        .await
        .expect("reset of a served runtime with nothing running");
    let (_input, handle) = accept(&machine, &served.session_id, "after the reset").await;
    served.wait_for_applies(1).await;
    served.permits.add_permits(1);
    let outcome = tokio::time::timeout(BOUND, handle.try_wait())
        .await
        .expect("the next input's waiter is answered")
        .expect("with an outcome");
    assert!(
        matches!(outcome, CompletionOutcome::Completed(_)),
        "the next input runs and is answered its result: {outcome:?}"
    );
}

// ---------------------------------------------------------------------------
// One terminated input on a runtime that serves
// ---------------------------------------------------------------------------

/// One terminated input does not end a runtime that serves: in Idle,
/// Attached and Running the rule keeps the runtime and releases the
/// request's pre-admission, whatever the live-session read.
#[tokio::test]
async fn one_terminated_input_keeps_a_serving_runtime() {
    let phases = an_authority_in_each_phase().await;
    for phase in [
        RuntimeState::Idle,
        RuntimeState::Attached,
        RuntimeState::Running,
    ] {
        let (authority, binding) = phases.get(phase);
        for live in LIVE {
            assert_eq!(
                ask(authority, binding, Outcome::RuntimeTerminated, false, live),
                Ok((Action::RetainRuntime, PreAdmission::ReleasePreAdmission)),
                "one terminated input, not archived, {phase}, {live:?}"
            );
        }
    }
}

/// A relay that waited on an input a recall or a reset took back is told
/// to keep the runtime, so its cleanup (which retires the registration and
/// what the surface holds for the session) does not run. A run that was in
/// flight next to the recalled input completes and its relay is told the
/// same.
#[tokio::test]
async fn the_relay_of_an_input_taken_back_is_told_to_keep_the_runtime() {
    for persistent in [true, false] {
        for (ending, shape) in [
            (Ending::Recall, Shape::Queued),
            (Ending::Recall, Shape::RunningAndQueued),
            (Ending::Reset, Shape::Queued),
        ] {
            for row in end_and_collect(ending, shape, persistent).await {
                assert_eq!(
                    row.rule,
                    Some(Ok(Action::RetainRuntime)),
                    "{ending:?}, {shape:?}, persistent {persistent}: the relay of the {} input \
                     ({})",
                    row.label,
                    row.answer
                );
            }
        }
    }
}

/// The authority a surface is handed carries the machine's action unchanged.
/// For the waiter of a stopped runtime: the cleanup of an archived session
/// when the session is archived, the runtime's own cleanup when it is not;
/// both release the request's pre-admission.
#[tokio::test]
async fn the_cleanup_authority_carries_the_machines_action() {
    let machine = persistent_machine();
    let session_id = registered(&machine).await;
    let (_input, handle) = accept(&machine, &session_id, "stopped with the runtime").await;
    take_the_gate_once(&machine, &session_id).await;
    machine
        .stop_runtime_executor(&session_id, "stop")
        .await
        .expect("stop");
    let (release, observation) =
        tokio::time::timeout(BOUND, handle.try_wait_with_cleanup_observation())
            .await
            .expect("the stopped input's waiter is answered")
            .expect("with an outcome");
    let outcome = release.release().await.expect("the outcome is released");
    assert!(
        matches!(outcome, CompletionOutcome::RuntimeTerminated { .. }),
        "the stopped input is answered terminated: {outcome:?}"
    );
    for (archived, action) in [
        (false, Action::CleanupRuntime),
        (true, Action::CleanupArchivedSession),
    ] {
        let authority = machine
            .resolve_runtime_completion_cleanup(
                &session_id,
                observation.clone(),
                archived,
                Live::NotObserved,
            )
            .await
            .expect("the rule answers");
        assert_eq!(
            (
                authority.action,
                authority.releases_pre_admission(),
                authority.archived_by_authority
            ),
            (action, true, archived),
            "(action, pre-admission released, archive fact) for archived = {archived}"
        );
    }
}

// ---------------------------------------------------------------------------
// One terminated input on a retired runtime that still drains
// ---------------------------------------------------------------------------

/// A retired runtime whose drain has not started: an executor serves the
/// session, run starts are held, `queued` inputs are queued, and the runtime
/// is then retired. The retire leaves the queued inputs to its drain and the
/// hold keeps the drain from starting, so the phase is Retired with every
/// input still queued.
struct Retiring {
    machine: Arc<MeerkatMachine>,
    served: Served,
    authority: SharedAuthority,
    inputs: Vec<(InputId, CompletionHandle)>,
}

async fn a_retired_runtime_with_queued_inputs(persistent: bool, queued: usize) -> Retiring {
    let machine = if persistent {
        persistent_machine()
    } else {
        Arc::new(MeerkatMachine::ephemeral())
    };
    let served = Served::new(&machine).await;
    let authority = session_authority(&machine, &served.session_id).await;
    wait_for_phase(&authority, RuntimeState::Attached).await;
    machine
        .hold_run_starts(&served.session_id, crate::RunStartHoldReason::MobStop)
        .await
        .expect("hold run starts");
    let mut inputs = Vec::new();
    for index in 0..queued {
        inputs.push(accept(&machine, &served.session_id, &format!("queued-{index}")).await);
    }
    take_the_gate_once(&machine, &served.session_id).await;
    let report =
        <MeerkatMachine as SessionServiceRuntimeExt>::retire_runtime(&machine, &served.session_id)
            .await
            .expect("retire");
    assert_eq!(
        (report.inputs_abandoned, report.inputs_pending_drain),
        (0, queued),
        "(inputs the retire abandoned, inputs it left to its drain)"
    );
    wait_for_phase(&authority, RuntimeState::Retired).await;
    assert_eq!(
        served.applies.load(Ordering::SeqCst),
        0,
        "no run starts while run starts are held"
    );
    Retiring {
        machine,
        served,
        authority,
        inputs,
    }
}

impl Retiring {
    /// Take one queued input back, and ask the rule as the relay that waited
    /// on that input does: with the observation its answer carried.
    async fn recall_and_ask(&self, input: (InputId, CompletionHandle)) -> Result<Action, String> {
        let (input_id, handle) = input;
        let recall = self
            .machine
            .recall_queued_input(&self.served.session_id, &input_id)
            .await
            .expect("the recall answers");
        assert!(
            matches!(
                recall,
                Some(crate::QueuedInputRecall::Abandoned {
                    attempt_count: 0,
                    ..
                })
            ),
            "the queued input is taken back: {recall:?}"
        );
        let (release, observation) =
            tokio::time::timeout(BOUND, handle.try_wait_with_cleanup_observation())
                .await
                .expect("the recalled input's waiter is answered")
                .expect("with an outcome");
        let outcome = release.release().await.expect("the outcome is released");
        assert!(
            matches!(outcome, CompletionOutcome::RuntimeTerminated { .. }),
            "the recalled input is answered terminated: {outcome:?}"
        );
        assert_eq!(
            phase_of(&self.authority),
            RuntimeState::Retired,
            "the runtime is retired when the relay asks"
        );
        self.machine
            .resolve_runtime_completion_cleanup(
                &self.served.session_id,
                observation,
                false,
                Live::NotObserved,
            )
            .await
            .map(|authority| authority.action)
            .map_err(|error| error.to_string())
    }
}

/// Taking back one queued input of a retired runtime that still has an
/// input to drain ends that input and nothing else. The relay that waited
/// on it is told to keep the runtime, so its cleanup (which retires the
/// registration and ends what is queued on it) does not run; when run
/// starts are released the other input runs and is answered its result, and
/// its relay is told to keep the runtime too.
#[tokio::test]
async fn one_terminated_input_keeps_a_retired_runtime_that_still_drains() {
    for persistent in [true, false] {
        let mut retiring = a_retired_runtime_with_queued_inputs(persistent, 2).await;
        let (other, other_handle) = retiring.inputs.pop().expect("the second queued input");
        let recalled = retiring.inputs.pop().expect("the first queued input");
        assert_eq!(
            retiring.recall_and_ask(recalled).await,
            Ok(Action::RetainRuntime),
            "persistent {persistent}: the relay of a recalled input, with another input still \
             to drain"
        );

        retiring
            .machine
            .release_run_starts(
                &retiring.served.session_id,
                crate::RunStartHoldReason::MobStop,
            )
            .await
            .expect("release run starts");
        retiring.served.wait_for_applies(1).await;
        retiring.served.permits.add_permits(1);
        let (release, observation) =
            tokio::time::timeout(BOUND, other_handle.try_wait_with_cleanup_observation())
                .await
                .expect("the other input's waiter is answered")
                .expect("with an outcome");
        let outcome = release.release().await.expect("the outcome is released");
        assert!(
            matches!(outcome, CompletionOutcome::Completed(_)),
            "persistent {persistent}: the other input ({other}) runs and is answered its \
             result: {outcome:?}"
        );
        assert_eq!(
            retiring
                .machine
                .resolve_runtime_completion_cleanup(
                    &retiring.served.session_id,
                    observation,
                    false,
                    Live::NotObserved,
                )
                .await
                .map(|authority| authority.action)
                .map_err(|error| error.to_string()),
            Ok(Action::RetainRuntime),
            "persistent {persistent}: the relay of the drained input"
        );
        retiring.served.permits.close();
    }
}

/// The cleanup of a retired runtime runs when nothing is left to drain: the
/// last queued input taken back, alone or after the others, leaves no work,
/// and its relay is told to clean the runtime up.
#[tokio::test]
async fn the_last_queued_input_taken_back_leaves_a_retired_runtime_to_its_cleanup() {
    for persistent in [true, false] {
        let mut alone = a_retired_runtime_with_queued_inputs(persistent, 1).await;
        let only = alone.inputs.pop().expect("the queued input");
        assert_eq!(
            alone.recall_and_ask(only).await,
            Ok(Action::CleanupRuntime),
            "persistent {persistent}: the relay of the only queued input"
        );
        alone.served.permits.close();

        let mut retiring = a_retired_runtime_with_queued_inputs(persistent, 2).await;
        let last = retiring.inputs.pop().expect("the second queued input");
        let first = retiring.inputs.pop().expect("the first queued input");
        assert_eq!(
            retiring.recall_and_ask(first).await,
            Ok(Action::RetainRuntime),
            "persistent {persistent}: the relay of the first of two queued inputs"
        );
        assert_eq!(
            retiring.recall_and_ask(last).await,
            Ok(Action::CleanupRuntime),
            "persistent {persistent}: the relay of the last queued input"
        );
        retiring.served.permits.close();
    }
}

/// The rule's table on a retired runtime that still has an input to drain:
/// one terminated input keeps the runtime, an archived session is cleaned up
/// as an archived session, and every other outcome is answered as in the
/// other phases.
#[tokio::test]
async fn the_cleanup_rule_keeps_its_table_on_a_retired_runtime_that_still_drains() {
    let retiring = a_retired_runtime_with_queued_inputs(true, 1).await;
    let binding = binding_of(&retiring.authority, &retiring.served.session_id);
    for outcome in OUTCOMES {
        for live in LIVE {
            let expected = if outcome == Outcome::RuntimeTerminated {
                Action::RetainRuntime
            } else {
                the_table(outcome, false, live)
            };
            assert_eq!(
                ask(&retiring.authority, &binding, outcome, false, live),
                Ok((expected, PreAdmission::ReleasePreAdmission)),
                "not archived, retired with an input to drain, {outcome:?}, {live:?}"
            );
            assert_eq!(
                ask(&retiring.authority, &binding, outcome, true, live),
                Ok((
                    Action::CleanupArchivedSession,
                    PreAdmission::ReleasePreAdmission
                )),
                "archived, retired with an input to drain, {outcome:?}, {live:?}"
            );
        }
    }
    retiring.served.permits.close();
}

// ---------------------------------------------------------------------------
// A turn refused at its start
// ---------------------------------------------------------------------------

/// A durable operator prompt: one turn.
fn keyed_prompt(key: &str) -> Input {
    Input::Prompt(crate::input::PromptInput {
        injected_context: Vec::new(),
        header: crate::input::InputHeader {
            ingress_context: None,
            retained_resume: None,
            authority_association: None,
            id: InputId::new(),
            timestamp: Utc::now(),
            source: crate::input::InputOrigin::Operator,
            durability: crate::input::InputDurability::Durable,
            visibility: crate::input::InputVisibility::default(),
            idempotency_key: Some(crate::identifiers::IdempotencyKey::new(key)),
            supersession_key: None,
            correlation_id: None,
            requester: None,
        },
        content: format!("the turn {key}").into(),
        typed_turn_appends: Vec::new(),
        turn_metadata: None,
    })
}

/// The refusal a turn is answered when the model's route does not take the
/// reasoning effort level the turn would run with.
fn effort_refusal() -> meerkat_core::ReasoningEffortRefusal {
    serde_json::from_value(serde_json::json!({
        "requested": "max", "provider": "anthropic", "model": "claude-opus-4-5",
        "route": {"backend": "anthropic_api", "api": "messages"},
        "reason": "unsupported_level", "supported": ["low", "medium", "high"],
        "facts": "catalog",
    }))
    .expect("a reasoning effort refusal")
}

/// Executor whose first apply waits for one permit. An apply that carries
/// the input `refused` names is refused for its reasoning effort level, as
/// the reconfiguration before a turn refuses it: nothing was applied. Every
/// other apply answers a result.
struct RefusingExecutor {
    session_id: SessionId,
    first_apply: Arc<crate::tokio::sync::Semaphore>,
    batches: Arc<std::sync::Mutex<Vec<Vec<InputId>>>>,
    refused: Arc<std::sync::Mutex<Option<InputId>>>,
}

#[async_trait::async_trait]
impl CoreExecutor for RefusingExecutor {
    async fn apply(
        &mut self,
        run_id: RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        let contributors = primitive.contributing_input_ids().to_vec();
        let call = {
            let mut batches = self
                .batches
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            batches.push(contributors.clone());
            batches.len()
        };
        if call == 1 {
            self.first_apply
                .acquire()
                .await
                .map_err(|_| CoreExecutorError::Stopped)?
                .forget();
        }
        let refused = self
            .refused
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if refused.is_some_and(|refused| contributors.contains(&refused)) {
            let refusal = effort_refusal();
            return Err(CoreExecutorError::apply_failed(
                meerkat_core::lifecycle::core_executor::CoreApplyFailureCause::runtime_turn(
                    refusal.to_string(),
                )
                .with_reason(
                    meerkat_core::event::AgentErrorReason::ReasoningEffortRefused {
                        refusal: Box::new(refusal),
                    },
                ),
            ));
        }
        Ok(CoreApplyOutput::with_run_result(
            RunBoundaryReceiptDraft {
                run_id,
                boundary: RunApplyBoundary::RunStart,
                contributing_input_ids: contributors,
                conversation_digest: None,
                message_count: 0,
            },
            None,
            run_result(&self.session_id, &format!("answer-{call}")),
        ))
    }

    async fn cancel_after_boundary(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        Ok(())
    }

    async fn stop_runtime_executor(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        Ok(())
    }
}

/// What a turn refused at its start, with one turn running ahead of it and
/// one queued behind it, left of itself and of the session.
struct RefusedStart {
    /// The inputs of the turns, in the order they were accepted: the three,
    /// then one more accepted once the turn queued behind was answered.
    running: InputId,
    refused: InputId,
    behind: InputId,
    later: InputId,
    /// What each turn's waiter was answered.
    running_answer: String,
    refused_answer: serde_json::Value,
    behind_answer: String,
    later_answer: String,
    /// What the relay that waited on the refused turn is told, by what it
    /// observed of the session: (the outcome the rule was asked about, the
    /// action), for a live session present, not observed and absent.
    relay: Vec<Result<(Outcome, Action), String>>,
    /// The inputs of each apply the executor was handed, in order.
    batches: Vec<Vec<InputId>>,
    /// The refused input's stored record: (phase, terminal record as it is
    /// stored, attempts counted).
    stored: (String, serde_json::Value, u32),
    /// The refused input's stored receipt, as it is stored.
    receipt: serde_json::Value,
    /// Whether the session is served by the registration it had before the
    /// refusal: (the registration has not ended, the same runtime epoch, the
    /// same authority, the same runtime binding).
    same_registration: (bool, bool, bool, bool),
    /// The runtime's phase after the three turns.
    phase: RuntimeState,
}

async fn answered(handle: CompletionHandle) -> Result<CompletionOutcome, String> {
    match tokio::time::timeout(BOUND, handle.try_wait()).await {
        Err(_) => Err("not answered".to_string()),
        Ok(Err(error)) => Err(format!("wait failure: {error}")),
        Ok(Ok(outcome)) => Ok(outcome),
    }
}

/// The name of an answer: its completion type, or why there is none.
fn answer_name(answer: &Result<CompletionOutcome, String>) -> String {
    match answer {
        Err(none) => none.clone(),
        Ok(outcome) => {
            serde_json::to_value(outcome).expect("an outcome serializes")["completion_type"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        }
    }
}

async fn a_turn_refused_at_its_start(persistent: bool) -> RefusedStart {
    let machine = if persistent {
        persistent_machine()
    } else {
        Arc::new(MeerkatMachine::ephemeral())
    };
    let session_id = SessionId::new();
    let first_apply = Arc::new(crate::tokio::sync::Semaphore::new(0));
    let batches = Arc::new(std::sync::Mutex::new(Vec::new()));
    let refused_input = Arc::new(std::sync::Mutex::new(None));
    machine
        .register_session_with_executor(
            session_id.clone(),
            Box::new(RefusingExecutor {
                session_id: session_id.clone(),
                first_apply: Arc::clone(&first_apply),
                batches: Arc::clone(&batches),
                refused: Arc::clone(&refused_input),
            }),
        )
        .await
        .expect("register the refusing executor");
    let authority = session_authority(&machine, &session_id).await;
    let registration = machine
        .current_session_registration_witness(&session_id)
        .await
        .expect("the session is registered");

    // One turn runs; two are queued behind it. The first of the two is the
    // one that will be refused at its start.
    let accept_prompt = |key: &'static str| {
        let machine = Arc::clone(&machine);
        let session_id = session_id.clone();
        async move {
            let input = keyed_prompt(key);
            let input_id = input.id().clone();
            let (outcome, handle) = machine
                .accept_input_with_completion(&session_id, input)
                .await
                .expect("accept the turn");
            assert!(outcome.is_accepted());
            (
                input_id,
                handle.expect("an accepted input has a completion handle"),
            )
        }
    };
    let (running, running_handle) = accept_prompt("running").await;
    tokio::time::timeout(BOUND, async {
        while batches
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the first turn starts");
    let binding = binding_of(&authority, &session_id);
    let (refused, refused_handle) = accept_prompt("refused").await;
    let (behind, behind_handle) = accept_prompt("behind").await;
    *refused_input
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(refused.clone());
    first_apply.add_permits(1);

    let running_answer = answer_name(&answered(running_handle).await);
    let (refused_answer, relay) =
        match tokio::time::timeout(BOUND, refused_handle.try_wait_with_cleanup_observation()).await
        {
            Err(_) => (serde_json::json!("not answered"), Vec::new()),
            Ok(Err(error)) => (
                serde_json::json!(format!("wait failure: {error}")),
                Vec::new(),
            ),
            Ok(Ok((release, observation))) => {
                let outcome = release.release().await.expect("the outcome is released");
                let mut relay = Vec::new();
                for live in [Live::Present, Live::NotObserved, Live::Absent] {
                    relay.push(
                        machine
                            .resolve_runtime_completion_cleanup(
                                &session_id,
                                observation.clone(),
                                false,
                                live,
                            )
                            .await
                            .map(|authority| (authority.outcome, authority.action))
                            .map_err(|error| error.to_string()),
                    );
                }
                (
                    serde_json::to_value(&outcome).expect("an outcome serializes"),
                    relay,
                )
            }
        };
    let behind_answer = answer_name(&answered(behind_handle).await);
    // One more turn, accepted after everything queued so far was answered:
    // a start the runtime still owed any earlier input comes before it.
    let (later, later_handle) = accept_prompt("later").await;
    let later_answer = answer_name(&answered(later_handle).await);

    let stored =
        <MeerkatMachine as SessionServiceRuntimeExt>::input_state(&machine, &session_id, &refused)
            .await
            .expect("the refused input's record is read")
            .expect("the refused input has a record");
    let receipt = <MeerkatMachine as SessionServiceRuntimeExt>::input_terminal_completion(
        &machine,
        &session_id,
        &refused,
    )
    .await
    .map(|receipt| serde_json::to_value(&receipt).expect("a receipt serializes"))
    .unwrap_or_else(|error| serde_json::json!(format!("no receipt: {error}")));
    let now = machine
        .current_session_registration_witness(&session_id)
        .await;
    let authority_now = session_authority(&machine, &session_id).await;
    let binding_now = binding_of(&authority_now, &session_id);
    let result = RefusedStart {
        running,
        refused,
        behind,
        later,
        running_answer,
        refused_answer,
        behind_answer,
        later_answer,
        relay,
        batches: batches
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone(),
        stored: (
            format!("{:?}", stored.seed.phase),
            serde_json::to_value(&stored.seed.terminal_outcome)
                .expect("a terminal record serializes"),
            stored.seed.attempt_count,
        ),
        receipt,
        same_registration: (
            !registration.has_ended(),
            now.is_some_and(|now| now.epoch_id() == registration.epoch_id()),
            Arc::ptr_eq(&authority, &authority_now),
            (
                &binding.runtime_id,
                binding.fence_token,
                binding.generation,
                &binding.epoch_id,
            ) == (
                &binding_now.runtime_id,
                binding_now.fence_token,
                binding_now.generation,
                &binding_now.epoch_id,
            ),
        ),
        phase: phase_of(&authority_now),
    };
    first_apply.close();
    result
}

/// A turn refused at its start for its reasoning effort level, with a turn
/// queued behind it, costs the session nothing else: the relay that waited
/// on the refused turn is told to keep the runtime while the live session is
/// there (the refused reconfiguration left it as it was), the turn queued
/// behind runs and is answered its result, and the session is still served
/// by the registration it had. A relay that finds no live session is told to
/// clean up, as for every abandoned turn.
#[tokio::test]
async fn a_turn_refused_at_its_start_keeps_the_runtime_and_the_turn_queued_behind_it_runs() {
    for persistent in [true, false] {
        let end = a_turn_refused_at_its_start(persistent).await;
        assert_eq!(
            end.relay,
            vec![
                Ok((Outcome::Abandoned, Action::RetainRuntime)),
                Ok((Outcome::Abandoned, Action::RetainRuntime)),
                Ok((Outcome::Abandoned, Action::CleanupRuntime)),
            ],
            "persistent {persistent}: what the refused turn's relay is told, with the live \
             session present, not observed and absent"
        );
        assert_eq!(
            (
                end.running_answer.as_str(),
                end.behind_answer.as_str(),
                end.later_answer.as_str()
            ),
            ("completed", "completed", "completed"),
            "persistent {persistent}: (the turn that ran ahead, the turn queued behind, a turn \
             accepted later)"
        );
        assert_eq!(
            end.same_registration,
            (true, true, true, true),
            "persistent {persistent}: (the registration has not ended, the same runtime epoch, \
             the same authority, the same runtime binding)"
        );
        assert!(
            serves(end.phase),
            "persistent {persistent}: the runtime still serves, it is {}",
            end.phase
        );
    }
}

/// The refused turn ends once and alone: its waiter is answered the typed
/// refusal, it is handed to the executor once and never again, the turns
/// around it are each handed over once in their order, and its stored
/// record says it was never executed.
#[tokio::test]
async fn a_turn_refused_at_its_start_ends_once_with_the_refusal_and_is_not_started_again() {
    let refusal = serde_json::to_value(
        meerkat_core::event::AgentErrorReason::ReasoningEffortRefused {
            refusal: Box::new(effort_refusal()),
        },
    )
    .expect("a reason serializes");
    for persistent in [true, false] {
        let end = a_turn_refused_at_its_start(persistent).await;
        assert_eq!(
            end.batches,
            vec![
                vec![end.running.clone()],
                vec![end.refused.clone()],
                vec![end.behind.clone()],
                vec![end.later.clone()]
            ],
            "persistent {persistent}: the inputs of each start, in order: the refused turn is \
             started once, and no second start of it comes before a turn accepted later"
        );
        assert_eq!(
            (
                end.refused_answer["completion_type"].as_str(),
                &end.refused_answer["error"]["kind"],
                &end.refused_answer["error"]["reason"],
            ),
            (
                Some("abandoned_with_error"),
                &serde_json::json!("runtime_apply_failure"),
                &refusal,
            ),
            "persistent {persistent}: the refused turn's answer: {}",
            end.refused_answer
        );
        assert_eq!(
            end.stored,
            (
                "Abandoned".to_string(),
                serde_json::json!({"outcome_type": "abandoned", "reason": "never_executed"}),
                1,
            ),
            "persistent {persistent}: the refused input's stored record (phase, terminal \
             record, attempts counted)"
        );
    }
}

/// What is stored for the refused turn uses only forms a release without
/// this rule reads: the terminal record is the never-executed one, the
/// stored receipt is the answer the caller got, and neither names the cause
/// the machine keeps for the failed apply.
#[tokio::test]
async fn a_turn_refused_at_its_start_stores_nothing_an_earlier_release_cannot_read() {
    for persistent in [true, false] {
        let end = a_turn_refused_at_its_start(persistent).await;
        assert_eq!(
            end.stored.1,
            serde_json::json!({"outcome_type": "abandoned", "reason": "never_executed"}),
            "persistent {persistent}: the refused input's stored terminal record"
        );
        assert_eq!(
            end.receipt, end.refused_answer,
            "persistent {persistent}: the refused input's stored receipt is the answer its \
             waiter got"
        );
        let stored = format!("{} {}", end.stored.1, end.receipt);
        for cause in ["RequestRefused", "request_refused"] {
            assert!(
                !stored.contains(cause),
                "persistent {persistent}: the stored forms name no cause {cause}: {stored}"
            );
        }
        let error = end.receipt["error"]
            .as_object()
            .expect("the receipt carries the turn's error");
        let mut fields: Vec<&str> = error.keys().map(String::as_str).collect();
        fields.sort_unstable();
        assert_eq!(
            fields,
            vec!["detail", "kind", "outcome", "reason", "terminal"],
            "persistent {persistent}: the fields of the stored turn error"
        );
    }
}

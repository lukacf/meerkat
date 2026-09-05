import re
p='meerkat-mob/src/runtime/actor.rs'
s=open(p).read()

def rep(old, new, count=1):
    global s
    assert s.count(old)==count, (s.count(old), old[:120])
    s=s.replace(old,new)

# ---------------------------------------------------------------- types
rep('''enum SubmitWorkDispatchCompletion {
    Completed,
    AwaitTurnAdmission {
        operation_id: Option<meerkat_core::ops::OperationId>,
        member_ref: MemberRef,
''','''enum SubmitWorkDispatchCompletion {
    Completed,
    AwaitTurnAdmission {
        operation_id: Option<meerkat_core::ops::OperationId>,
        /// Lane key: every admission for one member is single-flight.
        agent_identity: AgentIdentity,
        /// Member-local readiness executed in the lane before admission.
        readiness: Option<LocalTurnAdmissionReadiness>,
        member_ref: MemberRef,
''')
rep('''    AwaitTurnCompletion {
        member_ref: MemberRef,
        req: Box<meerkat_core::service::StartTurnRequest>,
        completion_tx: Option<super::handle::ExactTurnCompletionSender>,
        bounded_result_spec: Option<super::handle::BoundedResultSpec>,
        /// See [`Self::AwaitTurnAdmission::placed_identity`].
        placed_identity: Option<AgentIdentity>,
''','''    /// Local AutonomousHost delivery: readiness, the steer admission-barrier
    /// probe, and the runtime-admit-or-inbox-inject decision all run in the
    /// member's admission lane (#1102). Only the DSL admission stays inline.
    AwaitAutonomousDispatch {
        agent_identity: AgentIdentity,
        readiness: Option<LocalTurnAdmissionReadiness>,
        material: Box<AutonomousDispatchMaterial>,
    },
    AwaitTurnCompletion {
        /// Member-local readiness executed by the detached sender before the
        /// tracked turn starts (same #37 probe as the admission lane).
        readiness: Option<LocalTurnAdmissionReadiness>,
        member_ref: MemberRef,
        req: Box<meerkat_core::service::StartTurnRequest>,
        completion_tx: Option<super::handle::ExactTurnCompletionSender>,
        bounded_result_spec: Option<super::handle::BoundedResultSpec>,
        /// See [`Self::AwaitTurnAdmission::placed_identity`].
        placed_identity: Option<AgentIdentity>,
''')
rep('''            Self::AwaitTurnAdmission { .. } => "AwaitTurnAdmission",
            Self::AwaitTurnCompletion { .. } => "AwaitTurnCompletion",
        }
    }
}
''','''            Self::AwaitTurnAdmission { .. } => "AwaitTurnAdmission",
            Self::AwaitAutonomousDispatch { .. } => "AwaitAutonomousDispatch",
            Self::AwaitTurnCompletion { .. } => "AwaitTurnCompletion",
        }
    }
}

/// Member-local readiness that ran inline on the actor loop before turn
/// admission until #1102 (OB3 fleet-wide delivery stall). It now runs inside
/// the member's detached admission lane, after the DSL SubmitWork transition
/// and before the runtime admission, so one member's slow or wedged step
/// cannot delay another member's dispatch or the liveness probe.
#[derive(Debug, Clone)]
struct LocalTurnAdmissionReadiness {
    bridge_session_id: SessionId,
    /// #37: probe the live session actor; when it is gone, re-enter the actor
    /// for the machine-authorized revival and continue once it replies.
    check_live_session: bool,
    /// AutonomousHost members: mob comms drain + injector capability.
    autonomous_runtime: bool,
}

/// Off-loop autonomous dispatch material. After readiness the lane probes the
/// steer admission barrier and either admits a runtime turn or injects the
/// inbox event; neither step touches actor authority.
struct AutonomousDispatchMaterial {
    member_ref: MemberRef,
    bridge_session_id: SessionId,
    content: ContentInput,
    system_prompt: Option<String>,
    turn_metadata: Option<meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata>,
    handling_mode: meerkat_core::types::HandlingMode,
    external_delivery_identity: Option<crate::store::MobExternalDeliveryIdentity>,
    interaction_id: Option<meerkat_core::interaction::InteractionId>,
    objective_id: Option<meerkat_core::interaction::ObjectiveId>,
    event_tx:
        Option<tokio::sync::mpsc::Sender<meerkat_core::EventEnvelope<meerkat_core::AgentEvent>>>,
    completion_tx: Option<super::handle::ExactTurnCompletionSender>,
    llm_identity_applied_tx: Option<super::handle::MemberTurnLlmIdentityAppliedSender>,
    ack_mode: crate::mob_machine::SubmitWorkAckMode,
}

/// One member turn admission owned by that member's admission lane.
struct PendingMemberTurnAdmission {
    agent_identity: AgentIdentity,
    readiness: Option<LocalTurnAdmissionReadiness>,
    dispatch: PendingTurnDispatch,
}

enum PendingTurnDispatch {
    Admission {
        member_ref: MemberRef,
        req: Box<meerkat_core::service::StartTurnRequest>,
        completion_tx: Option<super::handle::ExactTurnCompletionSender>,
        llm_identity_applied_tx: Option<super::handle::MemberTurnLlmIdentityAppliedSender>,
        placed_identity: Option<AgentIdentity>,
        placed_incarnation: Option<super::bridge_protocol::BridgeMemberIncarnation>,
        placed_input_id: Option<String>,
    },
    Autonomous(Box<AutonomousDispatchMaterial>),
}

struct ParkedMemberTurnAdmission {
    pending: Box<PendingMemberTurnAdmission>,
    reply_tx: oneshot::Sender<Result<(), MobError>>,
}

/// Single-flight admission lane for one member (#1102). The actor runs one
/// detached admission per member at a time; later deliveries for the same
/// member park here in FIFO order and never occupy the actor loop. Ordering
/// per member is therefore DSL admission order; cross-member order is
/// unconstrained by construction.
#[derive(Default)]
pub(super) struct MemberAdmissionLane {
    inflight: Option<u64>,
    parked: VecDeque<ParkedMemberTurnAdmission>,
}

/// Read-only actor material for member-local readiness executed off the loop:
/// comms drain, injector capability, and the steer admission-barrier probe.
/// Placement is decided by the caller on the loop (placed members never
/// reach these), so this context carries no MobMachine state.
#[derive(Clone)]
pub(super) struct DetachedMemberReadinessContext {
    provisioner: Arc<dyn MobProvisioner>,
    #[cfg(feature = "runtime-adapter")]
    runtime_adapter: Option<Arc<meerkat_runtime::MeerkatMachine>>,
    mob_id: MobId,
}

/// One member's bounded readiness result from the concurrent explicit-Resume
/// fan-out. Applied on the actor (StartupMarkReady is machine authority).
pub(super) struct MemberReadinessOutcome {
    pub(super) entry: RosterEntry,
    pub(super) stage: super::state::LifecycleProgressStage,
    pub(super) result: Result<(), MobError>,
}

/// Budget after which one inline actor-loop step is reported as a stall
/// suspect. Any step that legitimately needs longer belongs off the loop.
const ACTOR_INLINE_STEP_BUDGET: Duration = Duration::from_secs(2);

#[derive(Debug, Clone)]
struct ActorInlineStep {
    command_kind: &'static str,
    step: &'static str,
    started: Instant,
    warned: bool,
}

/// Inline-step watchdog (#1102 observability). The loop marks the start of
/// every command dispatch and the arms refine the current step name; a
/// checker task warns once when the running step exceeds
/// [`ACTOR_INLINE_STEP_BUDGET`], naming the command kind and step, and the
/// loop warns again at completion with the total elapsed time. A wedged step
/// is therefore visible while it is wedged, not only after it returns.
pub(super) struct ActorInlineStepWatchdog {
    current: Arc<std::sync::Mutex<Option<ActorInlineStep>>>,
    checker: Option<tokio::task::JoinHandle<()>>,
}

impl ActorInlineStepWatchdog {
    pub(super) fn new() -> Self {
        Self {
            current: Arc::new(std::sync::Mutex::new(None)),
            checker: None,
        }
    }

    fn start_checker(&mut self, mob_id: MobId) {
        if self.checker.is_some() {
            return;
        }
        let current = Arc::clone(&self.current);
        self.checker = Some(tokio::spawn(async move {
            let mut tick = tokio::time::interval(ACTOR_INLINE_STEP_BUDGET / 4);
            loop {
                tick.tick().await;
                let mut guard = current
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Some(step) = guard.as_mut()
                    && !step.warned
                    && step.started.elapsed() >= ACTOR_INLINE_STEP_BUDGET
                {
                    step.warned = true;
                    tracing::warn!(
                        mob_id = %mob_id,
                        command_kind = step.command_kind,
                        step = step.step,
                        elapsed_ms = step.started.elapsed().as_millis() as u64,
                        budget_ms = ACTOR_INLINE_STEP_BUDGET.as_millis() as u64,
                        "actor inline step exceeded its budget; every later command is queued behind it"
                    );
                }
            }
        }));
    }

    fn begin(&self, command_kind: &'static str, step: &'static str) {
        *self
            .current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(ActorInlineStep {
            command_kind,
            step,
            started: Instant::now(),
            warned: false,
        });
    }

    /// Refine the step name of the running command (the command kind and
    /// start time are retained).
    pub(super) fn set_step(&self, step: &'static str) {
        if let Some(current) = self
            .current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_mut()
        {
            current.step = step;
        }
    }

    fn end(&self, mob_id: &MobId) {
        let finished = self
            .current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(step) = finished
            && step.started.elapsed() >= ACTOR_INLINE_STEP_BUDGET
        {
            tracing::warn!(
                mob_id = %mob_id,
                command_kind = step.command_kind,
                step = step.step,
                elapsed_ms = step.started.elapsed().as_millis() as u64,
                "actor inline step completed after exceeding its budget"
            );
        }
    }

    fn stop(&mut self) {
        if let Some(checker) = self.checker.take() {
            checker.abort();
        }
    }
}
''')

# ---------------------------------------------------------------- MobActor fields
rep('''    pub(super) member_revival_locks:
        Arc<tokio::sync::Mutex<HashMap<SessionId, Arc<tokio::sync::Mutex<()>>>>>,
''','''    pub(super) member_revival_locks:
        Arc<tokio::sync::Mutex<HashMap<SessionId, Arc<tokio::sync::Mutex<()>>>>>,
    /// Per-member single-flight admission lanes (#1102). Execution custody
    /// only: the DSL SubmitWork transition already admitted every entry.
    pub(super) member_admission_lanes: HashMap<AgentIdentity, MemberAdmissionLane>,
    pub(super) next_member_admission_ticket: u64,
    /// Handle-readable gauge of parked deliveries per member.
    pub(super) member_admission_backlog: Arc<super::handle::MemberAdmissionBacklogGauge>,
    /// Warns when one inline loop step exceeds its budget (#1102).
    pub(super) inline_step_watchdog: ActorInlineStepWatchdog,
    /// Explicit Resume whose per-member readiness fan-out is running detached;
    /// the loop keeps draining commands until the outcomes re-enter.
    pub(super) pending_resume_lifecycle: Option<PendingResumeLifecycle>,
    pub(super) next_resume_lifecycle_ticket: u64,
''')

open(p,'w').write(s)
print("ok")

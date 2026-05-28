//! Turn admission shell for `EphemeralSessionService`.
//!
//! The mutex/notify/channel code around this type is service mechanics. The
//! semantic facts it carries — phase, transition legality, active projection,
//! interrupt/shutdown acceptance, and start-turn disposition — are owned by the
//! generated `SessionTurnAdmissionMachine` below.

use std::fmt;

use meerkat_core::lifecycle::RuntimeExecutionKind;
use meerkat_machine_dsl::machine;

mod authority {
    use super::machine;
    use meerkat_core::pending_continuation_admission::PendingContinuationDisposition;

    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
    pub(crate) enum StartTurnExecutionKind {
        #[default]
        ContentTurn,
        ResumePending,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
    pub(crate) enum StartTurnDisposition {
        #[default]
        RunContentTurn,
        RunPending,
        NoPendingBoundary,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
    pub(crate) enum StartTurnPublicTerminal {
        #[default]
        NoPendingBoundary,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
    pub(crate) enum StartTurnDispatchAuthorization {
        #[default]
        Authorized,
        Cancelled,
    }

    machine! {
        machine SessionTurnAdmissionMachine {
            version: 1,
            rust: "meerkat-session" / "turn_admission::authority",

            state {
                lifecycle_phase: TurnAdmissionPhase,
                interrupt_pending: bool,
                shutdown_pending: bool,
                last_public_terminal: Option<Enum<StartTurnPublicTerminal>>,
            }

            init(Idle) {
                interrupt_pending = false,
                shutdown_pending = false,
                last_public_terminal = None,
            }

            terminal [ShuttingDown]

            phase TurnAdmissionPhase {
                Idle,
                Admitted,
                Running,
                Completing,
                ShuttingDown,
            }

            input SessionTurnAdmissionInput {
                ProjectTurnAdmission,
                ClaimTurn,
                AbortClaim,
                BeginTurn,
                ResolveTurn,
                FinalizeTurn,
                RequestInterrupt,
                RequestShutdown,
                AuthorizeStartTurnDispatch,
                AuthorizeCancelAfterBoundary,
                ResolveLastStartTurnPublicTerminal,
                ResolveRuntimeKeepAlive { keep_alive_policy_present: bool },
                ResolveStartTurnDisposition {
                    execution_kind_present: bool,
                    execution_kind: Enum<StartTurnExecutionKind>,
                    prompt_trimmed_text_byte_count: u64,
                    prompt_non_text_block_count: u64,
                    pending_continuation: Enum<PendingContinuationDisposition>,
                },
            }

            effect SessionTurnAdmissionEffect {
                TurnAdmissionProjected {
                    phase: TurnAdmissionPhase,
                    interrupt_pending: bool,
                    shutdown_pending: bool,
                    is_active: bool,
                },
                TurnInterruptRequested { wake: bool },
                StartTurnDispatchResolved { authorization: Enum<StartTurnDispatchAuthorization> },
                CancelAfterBoundaryAuthorized,
                StartTurnDispositionResolved { disposition: Enum<StartTurnDisposition> },
                StartTurnPublicTerminalResolved { terminal: Enum<StartTurnPublicTerminal> },
                RuntimeKeepAliveResolved { persist_keep_alive: bool },
            }

            helper is_active_phase(phase: TurnAdmissionPhase) -> bool {
                phase == Phase::Admitted || phase == Phase::Running || phase == Phase::Completing
            }

            helper prompt_has_content(prompt_trimmed_text_byte_count: u64, prompt_non_text_block_count: u64) -> bool {
                prompt_trimmed_text_byte_count > 0 || prompt_non_text_block_count > 0
            }

            invariant shutdown_phase_is_not_active {
                self.lifecycle_phase != Phase::ShuttingDown || is_active_phase(self.lifecycle_phase) == false
            }

            disposition TurnAdmissionProjected => local,
            disposition TurnInterruptRequested => local,
            disposition StartTurnDispatchResolved => local,
            disposition CancelAfterBoundaryAuthorized => local,
            disposition StartTurnDispositionResolved => local,
            disposition StartTurnPublicTerminalResolved => local,
            disposition RuntimeKeepAliveResolved => local,

            transition ProjectTurnAdmission {
                per_phase [Idle, Admitted, Running, Completing, ShuttingDown]
                on input ProjectTurnAdmission
                update {}
                to Idle
                emit TurnAdmissionProjected {
                    phase: self.lifecycle_phase,
                    interrupt_pending: self.interrupt_pending,
                    shutdown_pending: self.shutdown_pending,
                    is_active: is_active_phase(self.lifecycle_phase),
                }
            }

            transition ClaimTurn {
                on input ClaimTurn
                guard { self.lifecycle_phase == Phase::Idle }
                update {
                    self.interrupt_pending = false;
                    self.shutdown_pending = false;
                    self.last_public_terminal = None;
                }
                to Admitted
                emit TurnAdmissionProjected {
                    phase: self.lifecycle_phase,
                    interrupt_pending: self.interrupt_pending,
                    shutdown_pending: self.shutdown_pending,
                    is_active: is_active_phase(self.lifecycle_phase),
                }
            }

            transition AbortClaim {
                on input AbortClaim
                guard { self.lifecycle_phase == Phase::Admitted }
                update {
                    self.interrupt_pending = false;
                    self.shutdown_pending = false;
                }
                to Idle
                emit TurnAdmissionProjected {
                    phase: self.lifecycle_phase,
                    interrupt_pending: self.interrupt_pending,
                    shutdown_pending: self.shutdown_pending,
                    is_active: is_active_phase(self.lifecycle_phase),
                }
            }

            transition BeginTurn {
                on input BeginTurn
                guard { self.lifecycle_phase == Phase::Admitted }
                update {}
                to Running
                emit TurnAdmissionProjected {
                    phase: self.lifecycle_phase,
                    interrupt_pending: self.interrupt_pending,
                    shutdown_pending: self.shutdown_pending,
                    is_active: is_active_phase(self.lifecycle_phase),
                }
            }

            transition ResolveTurn {
                on input ResolveTurn
                guard { self.lifecycle_phase == Phase::Running }
                update {}
                to Completing
                emit TurnAdmissionProjected {
                    phase: self.lifecycle_phase,
                    interrupt_pending: self.interrupt_pending,
                    shutdown_pending: self.shutdown_pending,
                    is_active: is_active_phase(self.lifecycle_phase),
                }
            }

            transition FinalizeTurnToShutdown {
                on input FinalizeTurn
                guard { self.lifecycle_phase == Phase::Completing && self.shutdown_pending }
                update {
                    self.interrupt_pending = false;
                }
                to ShuttingDown
                emit TurnAdmissionProjected {
                    phase: self.lifecycle_phase,
                    interrupt_pending: self.interrupt_pending,
                    shutdown_pending: self.shutdown_pending,
                    is_active: is_active_phase(self.lifecycle_phase),
                }
            }

            transition FinalizeTurnToIdle {
                on input FinalizeTurn
                guard { self.lifecycle_phase == Phase::Completing && self.shutdown_pending == false }
                update {
                    self.interrupt_pending = false;
                    self.shutdown_pending = false;
                }
                to Idle
                emit TurnAdmissionProjected {
                    phase: self.lifecycle_phase,
                    interrupt_pending: self.interrupt_pending,
                    shutdown_pending: self.shutdown_pending,
                    is_active: is_active_phase(self.lifecycle_phase),
                }
            }

            transition RequestInterruptAdmittedFirst {
                on input RequestInterrupt
                guard { self.lifecycle_phase == Phase::Admitted && self.interrupt_pending == false }
                update {
                    self.interrupt_pending = true;
                }
                to Admitted
                emit TurnInterruptRequested { wake: true }
                emit TurnAdmissionProjected {
                    phase: self.lifecycle_phase,
                    interrupt_pending: self.interrupt_pending,
                    shutdown_pending: self.shutdown_pending,
                    is_active: is_active_phase(self.lifecycle_phase),
                }
            }

            transition RequestInterruptAdmittedDuplicate {
                on input RequestInterrupt
                guard { self.lifecycle_phase == Phase::Admitted && self.interrupt_pending }
                update {}
                to Admitted
                emit TurnInterruptRequested { wake: false }
                emit TurnAdmissionProjected {
                    phase: self.lifecycle_phase,
                    interrupt_pending: self.interrupt_pending,
                    shutdown_pending: self.shutdown_pending,
                    is_active: is_active_phase(self.lifecycle_phase),
                }
            }

            transition RequestInterruptRunningFirst {
                on input RequestInterrupt
                guard { self.lifecycle_phase == Phase::Running && self.interrupt_pending == false }
                update {
                    self.interrupt_pending = true;
                }
                to Running
                emit TurnInterruptRequested { wake: true }
                emit TurnAdmissionProjected {
                    phase: self.lifecycle_phase,
                    interrupt_pending: self.interrupt_pending,
                    shutdown_pending: self.shutdown_pending,
                    is_active: is_active_phase(self.lifecycle_phase),
                }
            }

            transition RequestInterruptRunningDuplicate {
                on input RequestInterrupt
                guard { self.lifecycle_phase == Phase::Running && self.interrupt_pending }
                update {}
                to Running
                emit TurnInterruptRequested { wake: false }
                emit TurnAdmissionProjected {
                    phase: self.lifecycle_phase,
                    interrupt_pending: self.interrupt_pending,
                    shutdown_pending: self.shutdown_pending,
                    is_active: is_active_phase(self.lifecycle_phase),
                }
            }

            transition RequestShutdownImmediateIdle {
                on input RequestShutdown
                guard { self.lifecycle_phase == Phase::Idle }
                update {
                    self.interrupt_pending = false;
                    self.shutdown_pending = true;
                }
                to ShuttingDown
                emit TurnAdmissionProjected {
                    phase: self.lifecycle_phase,
                    interrupt_pending: self.interrupt_pending,
                    shutdown_pending: self.shutdown_pending,
                    is_active: is_active_phase(self.lifecycle_phase),
                }
            }

            transition RequestShutdownImmediateAdmitted {
                on input RequestShutdown
                guard { self.lifecycle_phase == Phase::Admitted }
                update {
                    self.interrupt_pending = false;
                    self.shutdown_pending = true;
                }
                to ShuttingDown
                emit TurnAdmissionProjected {
                    phase: self.lifecycle_phase,
                    interrupt_pending: self.interrupt_pending,
                    shutdown_pending: self.shutdown_pending,
                    is_active: is_active_phase(self.lifecycle_phase),
                }
            }

            transition RequestShutdownDeferredRunning {
                on input RequestShutdown
                guard { self.lifecycle_phase == Phase::Running }
                update {
                    self.shutdown_pending = true;
                }
                to Running
                emit TurnAdmissionProjected {
                    phase: self.lifecycle_phase,
                    interrupt_pending: self.interrupt_pending,
                    shutdown_pending: self.shutdown_pending,
                    is_active: is_active_phase(self.lifecycle_phase),
                }
            }

            transition RequestShutdownDeferredCompleting {
                on input RequestShutdown
                guard { self.lifecycle_phase == Phase::Completing }
                update {
                    self.shutdown_pending = true;
                }
                to Completing
                emit TurnAdmissionProjected {
                    phase: self.lifecycle_phase,
                    interrupt_pending: self.interrupt_pending,
                    shutdown_pending: self.shutdown_pending,
                    is_active: is_active_phase(self.lifecycle_phase),
                }
            }

            transition RequestShutdownAlreadyShuttingDown {
                on input RequestShutdown
                guard { self.lifecycle_phase == Phase::ShuttingDown }
                update {}
                to ShuttingDown
                emit TurnAdmissionProjected {
                    phase: self.lifecycle_phase,
                    interrupt_pending: self.interrupt_pending,
                    shutdown_pending: self.shutdown_pending,
                    is_active: is_active_phase(self.lifecycle_phase),
                }
            }

            transition AuthorizeCancelAfterBoundaryAdmitted {
                on input AuthorizeCancelAfterBoundary
                guard { self.lifecycle_phase == Phase::Admitted }
                update {}
                to Admitted
                emit CancelAfterBoundaryAuthorized
            }

            transition AuthorizeStartTurnDispatchAdmitted {
                on input AuthorizeStartTurnDispatch
                guard { self.lifecycle_phase == Phase::Admitted }
                update {}
                to Admitted
                emit StartTurnDispatchResolved { authorization: StartTurnDispatchAuthorization::Authorized }
            }

            transition AuthorizeStartTurnDispatchShuttingDown {
                on input AuthorizeStartTurnDispatch
                guard { self.lifecycle_phase == Phase::ShuttingDown }
                update {}
                to ShuttingDown
                emit StartTurnDispatchResolved { authorization: StartTurnDispatchAuthorization::Cancelled }
            }

            transition AuthorizeCancelAfterBoundaryRunning {
                on input AuthorizeCancelAfterBoundary
                guard { self.lifecycle_phase == Phase::Running }
                update {}
                to Running
                emit CancelAfterBoundaryAuthorized
            }

            transition ResolveDispositionContentTurn {
                on input ResolveStartTurnDisposition {
                    execution_kind_present,
                    execution_kind,
                    prompt_trimmed_text_byte_count,
                    prompt_non_text_block_count,
                    pending_continuation
                }
                guard {
                    self.lifecycle_phase == Phase::Admitted
                    && execution_kind_present
                    && execution_kind == StartTurnExecutionKind::ContentTurn
                }
                update {
                    self.last_public_terminal = None;
                }
                to Admitted
                emit StartTurnDispositionResolved { disposition: StartTurnDisposition::RunContentTurn }
            }

            transition ResolveDispositionResumePendingWithBoundary {
                on input ResolveStartTurnDisposition {
                    execution_kind_present,
                    execution_kind,
                    prompt_trimmed_text_byte_count,
                    prompt_non_text_block_count,
                    pending_continuation
                }
                guard {
                    self.lifecycle_phase == Phase::Admitted
                    && execution_kind_present
                    && execution_kind == StartTurnExecutionKind::ResumePending
                    && pending_continuation == PendingContinuationDisposition::RunPending
                }
                update {
                    self.last_public_terminal = None;
                }
                to Admitted
                emit StartTurnDispositionResolved { disposition: StartTurnDisposition::RunPending }
            }

            transition ResolveDispositionResumePendingWithoutBoundary {
                on input ResolveStartTurnDisposition {
                    execution_kind_present,
                    execution_kind,
                    prompt_trimmed_text_byte_count,
                    prompt_non_text_block_count,
                    pending_continuation
                }
                guard {
                    self.lifecycle_phase == Phase::Admitted
                    && execution_kind_present
                    && execution_kind == StartTurnExecutionKind::ResumePending
                    && pending_continuation == PendingContinuationDisposition::NoPendingBoundary
                }
                update {
                    self.last_public_terminal = Some(StartTurnPublicTerminal::NoPendingBoundary);
                }
                to Admitted
                emit StartTurnDispositionResolved { disposition: StartTurnDisposition::NoPendingBoundary }
                emit StartTurnPublicTerminalResolved { terminal: StartTurnPublicTerminal::NoPendingBoundary }
            }

            transition ResolveDispositionDirectPrompt {
                on input ResolveStartTurnDisposition {
                    execution_kind_present,
                    execution_kind,
                    prompt_trimmed_text_byte_count,
                    prompt_non_text_block_count,
                    pending_continuation
                }
                guard {
                    self.lifecycle_phase == Phase::Admitted
                    && execution_kind_present == false
                    && prompt_has_content(prompt_trimmed_text_byte_count, prompt_non_text_block_count)
                }
                update {
                    self.last_public_terminal = None;
                }
                to Admitted
                emit StartTurnDispositionResolved { disposition: StartTurnDisposition::RunContentTurn }
            }

            transition ResolveDispositionDirectPending {
                on input ResolveStartTurnDisposition {
                    execution_kind_present,
                    execution_kind,
                    prompt_trimmed_text_byte_count,
                    prompt_non_text_block_count,
                    pending_continuation
                }
                guard {
                    self.lifecycle_phase == Phase::Admitted
                    && execution_kind_present == false
                    && prompt_has_content(prompt_trimmed_text_byte_count, prompt_non_text_block_count) == false
                    && pending_continuation == PendingContinuationDisposition::RunPending
                }
                update {
                    self.last_public_terminal = None;
                }
                to Admitted
                emit StartTurnDispositionResolved { disposition: StartTurnDisposition::RunPending }
            }

            transition ResolveDispositionDirectNoPending {
                on input ResolveStartTurnDisposition {
                    execution_kind_present,
                    execution_kind,
                    prompt_trimmed_text_byte_count,
                    prompt_non_text_block_count,
                    pending_continuation
                }
                guard {
                    self.lifecycle_phase == Phase::Admitted
                    && execution_kind_present == false
                    && prompt_has_content(prompt_trimmed_text_byte_count, prompt_non_text_block_count) == false
                    && pending_continuation == PendingContinuationDisposition::NoPendingBoundary
                }
                update {
                    self.last_public_terminal = Some(StartTurnPublicTerminal::NoPendingBoundary);
                }
                to Admitted
                emit StartTurnDispositionResolved { disposition: StartTurnDisposition::NoPendingBoundary }
                emit StartTurnPublicTerminalResolved { terminal: StartTurnPublicTerminal::NoPendingBoundary }
            }

            transition ResolveRuntimeKeepAliveEnable {
                on input ResolveRuntimeKeepAlive { keep_alive_policy_present }
                guard { self.lifecycle_phase == Phase::Admitted && keep_alive_policy_present }
                update {}
                to Admitted
                emit RuntimeKeepAliveResolved { persist_keep_alive: true }
            }

            transition ResolveRuntimeKeepAlivePreserve {
                on input ResolveRuntimeKeepAlive { keep_alive_policy_present }
                guard { self.lifecycle_phase == Phase::Admitted && keep_alive_policy_present == false }
                update {}
                to Admitted
                emit RuntimeKeepAliveResolved { persist_keep_alive: false }
            }

            transition ResolveLastStartTurnPublicTerminalNoPending {
                per_phase [Idle, Admitted, Running, Completing, ShuttingDown]
                on input ResolveLastStartTurnPublicTerminal
                guard { self.last_public_terminal == Some(StartTurnPublicTerminal::NoPendingBoundary) }
                update {}
                to Idle
                emit StartTurnPublicTerminalResolved { terminal: StartTurnPublicTerminal::NoPendingBoundary }
            }
        }
    }
}

pub(crate) use authority::StartTurnDispatchAuthorization;
pub(crate) use authority::StartTurnDisposition;
pub(crate) use authority::StartTurnPublicTerminal;
pub(crate) use authority::TurnAdmissionPhase;
pub use meerkat_core::pending_continuation_admission::ObservedSessionTailKind;

/// Generated projection of the session-visible turn-admission state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TurnAdmissionProjection {
    pub(crate) phase: TurnAdmissionPhase,
    pub(crate) is_active: bool,
}

/// Error returned when the generated authority rejects an operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TurnAdmissionError {
    pub(crate) from: TurnAdmissionPhase,
    pub(crate) op: &'static str,
}

impl fmt::Display for TurnAdmissionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "illegal turn-admission operation {:?} from phase {:?}",
            self.op, self.from
        )
    }
}

impl std::error::Error for TurnAdmissionError {}

/// Outcome of a successful `finalize` call — tells the caller whether the
/// session is continuing (back to `Idle`) or draining (`ShuttingDown`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FinalizeOutcome {
    pub(crate) next_phase: TurnAdmissionPhase,
}

/// Generated start-turn admission feedback from a single authority transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StartTurnResolution {
    pub(crate) disposition: StartTurnDisposition,
    pub(crate) public_terminal: Option<StartTurnPublicTerminal>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StartTurnPromptObservation {
    trimmed_text_byte_count: u64,
    non_text_block_count: u64,
}

fn observe_start_turn_prompt(
    prompt: &meerkat_core::types::ContentInput,
) -> StartTurnPromptObservation {
    match prompt {
        meerkat_core::types::ContentInput::Text(text) => StartTurnPromptObservation {
            trimmed_text_byte_count: usize_to_u64(text.trim().len()),
            non_text_block_count: 0,
        },
        meerkat_core::types::ContentInput::Blocks(blocks) => {
            let text = meerkat_core::types::text_content(blocks);
            let non_text_block_count = blocks
                .iter()
                .filter(|block| !matches!(block, meerkat_core::types::ContentBlock::Text { .. }))
                .count();
            StartTurnPromptObservation {
                trimmed_text_byte_count: usize_to_u64(text.trim().len()),
                non_text_block_count: usize_to_u64(non_text_block_count),
            }
        }
    }
}

fn usize_to_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

/// Serialized turn-admission state for a single session.
pub(crate) struct TurnAdmissionSlot {
    authority: authority::SessionTurnAdmissionMachineAuthority,
    projection: TurnAdmissionProjection,
}

impl TurnAdmissionSlot {
    pub(crate) fn new() -> Self {
        let mut slot = Self {
            authority: authority::SessionTurnAdmissionMachineAuthority::new(),
            projection: TurnAdmissionProjection {
                phase: TurnAdmissionPhase::Idle,
                is_active: false,
            },
        };
        #[allow(clippy::expect_used)]
        slot.refresh_projection()
            .expect("generated initial turn-admission projection should be available");
        slot
    }

    pub(crate) fn phase(&self) -> TurnAdmissionPhase {
        self.authority.state().phase()
    }

    pub(crate) fn projection(&self) -> TurnAdmissionProjection {
        self.projection
    }

    pub(crate) fn interrupt_pending(&self) -> bool {
        self.authority.state().interrupt_pending
    }

    #[cfg(test)]
    pub(crate) fn is_active(&self) -> bool {
        self.projection.is_active
    }

    /// Claim the turn slot before dispatching `StartTurn`. `Idle -> Admitted`.
    pub(crate) fn claim(&mut self) -> Result<TurnAdmissionPhase, TurnAdmissionError> {
        self.apply_projected(authority::SessionTurnAdmissionInput::ClaimTurn, "claim")
    }

    /// Release an admitted slot without running the turn. `Admitted -> Idle`.
    pub(crate) fn abort_claim(&mut self) -> Result<TurnAdmissionPhase, TurnAdmissionError> {
        self.apply_projected(
            authority::SessionTurnAdmissionInput::AbortClaim,
            "abort_claim",
        )
    }

    /// Mark the admitted slot as actively running. `Admitted -> Running`.
    pub(crate) fn begin(&mut self) -> Result<TurnAdmissionPhase, TurnAdmissionError> {
        self.apply_projected(authority::SessionTurnAdmissionInput::BeginTurn, "begin")
    }

    /// Move a finished run into the finalization window. `Running -> Completing`.
    pub(crate) fn resolve(&mut self) -> Result<TurnAdmissionPhase, TurnAdmissionError> {
        self.apply_projected(authority::SessionTurnAdmissionInput::ResolveTurn, "resolve")
    }

    /// Close the finalization window. `Completing -> Idle` unless shutdown was
    /// requested mid-turn, in which case `Completing -> ShuttingDown`.
    pub(crate) fn finalize(&mut self) -> Result<FinalizeOutcome, TurnAdmissionError> {
        let next_phase = self.apply_projected(
            authority::SessionTurnAdmissionInput::FinalizeTurn,
            "finalize",
        )?;
        Ok(FinalizeOutcome { next_phase })
    }

    /// Request an interrupt through generated legality and wake feedback.
    pub(crate) fn request_interrupt(&mut self) -> Result<bool, TurnAdmissionError> {
        let from = self.phase();
        let transition = authority::SessionTurnAdmissionMachineMutator::apply(
            &mut self.authority,
            authority::SessionTurnAdmissionInput::RequestInterrupt,
        )
        .map_err(|_| TurnAdmissionError {
            from,
            op: "request_interrupt",
        })?;
        let wake = transition
            .effects()
            .iter()
            .find_map(|effect| match effect {
                authority::SessionTurnAdmissionEffect::TurnInterruptRequested { wake } => {
                    Some(*wake)
                }
                _ => None,
            })
            .ok_or(TurnAdmissionError {
                from,
                op: "request_interrupt",
            })?;
        self.update_projection_from_transition(&transition, from, "request_interrupt")?;
        Ok(wake)
    }

    /// Authorize boundary cancellation through generated admission state.
    pub(crate) fn authorize_cancel_after_boundary(&mut self) -> Result<(), TurnAdmissionError> {
        let from = self.phase();
        let transition = authority::SessionTurnAdmissionMachineMutator::apply(
            &mut self.authority,
            authority::SessionTurnAdmissionInput::AuthorizeCancelAfterBoundary,
        )
        .map_err(|_| TurnAdmissionError {
            from,
            op: "authorize_cancel_after_boundary",
        })?;
        if transition.effects().iter().any(|effect| {
            matches!(
                effect,
                authority::SessionTurnAdmissionEffect::CancelAfterBoundaryAuthorized
            )
        }) {
            Ok(())
        } else {
            Err(TurnAdmissionError {
                from,
                op: "authorize_cancel_after_boundary",
            })
        }
    }

    pub(crate) fn authorize_start_turn_dispatch(
        &mut self,
    ) -> Result<StartTurnDispatchAuthorization, TurnAdmissionError> {
        let from = self.phase();
        let transition = authority::SessionTurnAdmissionMachineMutator::apply(
            &mut self.authority,
            authority::SessionTurnAdmissionInput::AuthorizeStartTurnDispatch,
        )
        .map_err(|_| TurnAdmissionError {
            from,
            op: "authorize_start_turn_dispatch",
        })?;
        transition
            .effects()
            .iter()
            .find_map(|effect| match effect {
                authority::SessionTurnAdmissionEffect::StartTurnDispatchResolved {
                    authorization,
                } => Some(*authorization),
                _ => None,
            })
            .ok_or(TurnAdmissionError {
                from,
                op: "authorize_start_turn_dispatch",
            })
    }

    /// Flag a shutdown request. Immediate and deferred shutdown outcomes are
    /// generated by `SessionTurnAdmissionMachine`.
    pub(crate) fn request_shutdown(&mut self) -> Result<TurnAdmissionPhase, TurnAdmissionError> {
        self.apply_projected(
            authority::SessionTurnAdmissionInput::RequestShutdown,
            "request_shutdown",
        )
    }

    pub(crate) fn resolve_start_turn_disposition(
        &mut self,
        execution_kind: Option<RuntimeExecutionKind>,
        prompt: &meerkat_core::types::ContentInput,
        session_tail: ObservedSessionTailKind,
        staged_tool_result_count: u64,
    ) -> Result<StartTurnResolution, TurnAdmissionError> {
        let (execution_kind_present, execution_kind) = match execution_kind {
            Some(RuntimeExecutionKind::ContentTurn) => {
                (true, authority::StartTurnExecutionKind::ContentTurn)
            }
            Some(RuntimeExecutionKind::ResumePending) => {
                (true, authority::StartTurnExecutionKind::ResumePending)
            }
            None => (false, authority::StartTurnExecutionKind::ContentTurn),
        };
        let prompt_observation = observe_start_turn_prompt(prompt);
        let pending_resolution =
            meerkat_core::pending_continuation_admission::resolve_pending_continuation(
                session_tail,
                staged_tool_result_count,
            )
            .map_err(|_| TurnAdmissionError {
                from: self.phase(),
                op: "resolve_pending_continuation",
            })?;
        if pending_resolution.disposition
            == meerkat_core::pending_continuation_admission::PendingContinuationDisposition::NoPendingBoundary
            && pending_resolution.public_terminal
                != Some(
                    meerkat_core::pending_continuation_admission::PendingContinuationPublicTerminal::NoPendingBoundary,
                )
        {
            return Err(TurnAdmissionError {
                from: self.phase(),
                op: "resolve_pending_continuation_terminal",
            });
        }
        let from = self.phase();
        let transition = authority::SessionTurnAdmissionMachineMutator::apply(
            &mut self.authority,
            authority::SessionTurnAdmissionInput::ResolveStartTurnDisposition {
                execution_kind_present,
                execution_kind,
                prompt_trimmed_text_byte_count: prompt_observation.trimmed_text_byte_count,
                prompt_non_text_block_count: prompt_observation.non_text_block_count,
                pending_continuation: pending_resolution.disposition,
            },
        )
        .map_err(|_| TurnAdmissionError {
            from,
            op: "resolve_start_turn_disposition",
        })?;
        let disposition = transition
            .effects()
            .iter()
            .find_map(|effect| match effect {
                authority::SessionTurnAdmissionEffect::StartTurnDispositionResolved {
                    disposition,
                } => Some(*disposition),
                _ => None,
            })
            .ok_or(TurnAdmissionError {
                from,
                op: "resolve_start_turn_disposition",
            })?;
        let public_terminal = transition.effects().iter().find_map(|effect| match effect {
            authority::SessionTurnAdmissionEffect::StartTurnPublicTerminalResolved { terminal } => {
                Some(*terminal)
            }
            _ => None,
        });
        Ok(StartTurnResolution {
            disposition,
            public_terminal,
        })
    }

    pub(crate) fn resolve_last_start_turn_public_terminal(
        &mut self,
    ) -> Result<StartTurnPublicTerminal, TurnAdmissionError> {
        let from = self.phase();
        let transition = authority::SessionTurnAdmissionMachineMutator::apply(
            &mut self.authority,
            authority::SessionTurnAdmissionInput::ResolveLastStartTurnPublicTerminal,
        )
        .map_err(|_| TurnAdmissionError {
            from,
            op: "resolve_last_start_turn_public_terminal",
        })?;
        transition
            .effects()
            .iter()
            .find_map(|effect| match effect {
                authority::SessionTurnAdmissionEffect::StartTurnPublicTerminalResolved {
                    terminal,
                } => Some(*terminal),
                _ => None,
            })
            .ok_or(TurnAdmissionError {
                from,
                op: "resolve_last_start_turn_public_terminal",
            })
    }

    pub(crate) fn resolve_runtime_keep_alive(
        &mut self,
        keep_alive_policy_present: bool,
    ) -> Result<bool, TurnAdmissionError> {
        let from = self.phase();
        let transition = authority::SessionTurnAdmissionMachineMutator::apply(
            &mut self.authority,
            authority::SessionTurnAdmissionInput::ResolveRuntimeKeepAlive {
                keep_alive_policy_present,
            },
        )
        .map_err(|_| TurnAdmissionError {
            from,
            op: "resolve_runtime_keep_alive",
        })?;
        transition
            .effects()
            .iter()
            .find_map(|effect| match effect {
                authority::SessionTurnAdmissionEffect::RuntimeKeepAliveResolved {
                    persist_keep_alive,
                } => Some(*persist_keep_alive),
                _ => None,
            })
            .ok_or(TurnAdmissionError {
                from,
                op: "resolve_runtime_keep_alive",
            })
    }

    fn apply_projected(
        &mut self,
        input: authority::SessionTurnAdmissionInput,
        op: &'static str,
    ) -> Result<TurnAdmissionPhase, TurnAdmissionError> {
        let from = self.phase();
        let transition =
            authority::SessionTurnAdmissionMachineMutator::apply(&mut self.authority, input)
                .map_err(|_| TurnAdmissionError { from, op })?;
        self.update_projection_from_transition(&transition, from, op)?;
        Ok(self.phase())
    }

    fn refresh_projection(&mut self) -> Result<(), TurnAdmissionError> {
        let from = self.phase();
        let transition = authority::SessionTurnAdmissionMachineMutator::apply(
            &mut self.authority,
            authority::SessionTurnAdmissionInput::ProjectTurnAdmission,
        )
        .map_err(|_| TurnAdmissionError {
            from,
            op: "project_turn_admission",
        })?;
        self.update_projection_from_transition(&transition, from, "project_turn_admission")
    }

    fn update_projection_from_transition(
        &mut self,
        transition: &authority::SessionTurnAdmissionMachineTransition,
        from: TurnAdmissionPhase,
        op: &'static str,
    ) -> Result<(), TurnAdmissionError> {
        let Some(projection) = transition.effects().iter().find_map(|effect| match effect {
            authority::SessionTurnAdmissionEffect::TurnAdmissionProjected {
                phase,
                interrupt_pending: _,
                shutdown_pending: _,
                is_active,
            } => Some(TurnAdmissionProjection {
                phase: *phase,
                is_active: *is_active,
            }),
            _ => None,
        }) else {
            return Err(TurnAdmissionError { from, op });
        };
        self.projection = projection;
        Ok(())
    }
}

impl Default for TurnAdmissionSlot {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    fn claimed_disposition(
        execution_kind: Option<RuntimeExecutionKind>,
        prompt: meerkat_core::types::ContentInput,
        session_tail: ObservedSessionTailKind,
        staged_tool_result_count: u64,
    ) -> StartTurnDisposition {
        let mut slot = TurnAdmissionSlot::new();
        slot.claim().expect("claim should admit disposition check");
        slot.resolve_start_turn_disposition(
            execution_kind,
            &prompt,
            session_tail,
            staged_tool_result_count,
        )
        .expect("generated disposition should resolve")
        .disposition
    }

    #[test]
    fn claim_reserves_slot() {
        let mut slot = TurnAdmissionSlot::new();
        let phase = slot.claim().expect("idle session should admit a turn");
        assert_eq!(phase, TurnAdmissionPhase::Admitted);
        assert!(slot.is_active());
        assert!(slot.projection().is_active);
    }

    #[test]
    fn interrupt_allowed_after_turn_admission() {
        let mut slot = TurnAdmissionSlot::new();
        let err = slot
            .request_interrupt()
            .expect_err("idle session cannot be interrupted");
        assert_eq!(err.from, TurnAdmissionPhase::Idle);

        slot.claim().unwrap();
        let admitted_woke = slot
            .request_interrupt()
            .expect("admitted session should accept interrupt");
        assert!(admitted_woke);
        assert!(slot.interrupt_pending());

        slot.begin().unwrap();
        let woke = slot
            .request_interrupt()
            .expect("running session should retain interrupt");
        assert!(!woke);
        assert_eq!(slot.phase(), TurnAdmissionPhase::Running);
        assert!(slot.interrupt_pending());
    }

    #[test]
    fn shutdown_gracefully_drains_running_turn() {
        let mut slot = TurnAdmissionSlot::new();
        slot.claim().unwrap();
        slot.begin().unwrap();
        slot.request_shutdown().unwrap();
        assert_eq!(slot.phase(), TurnAdmissionPhase::Running);

        slot.resolve().unwrap();
        let outcome = slot
            .finalize()
            .expect("finalize should enter shutting down");
        assert_eq!(outcome.next_phase, TurnAdmissionPhase::ShuttingDown);
        assert!(!slot.projection().is_active);
    }

    #[test]
    fn shutdown_cancels_admitted_before_run() {
        let mut slot = TurnAdmissionSlot::new();
        slot.claim().unwrap();
        let phase = slot
            .request_shutdown()
            .expect("admitted turn should be shut down before run");
        assert_eq!(phase, TurnAdmissionPhase::ShuttingDown);
    }

    #[test]
    fn cancel_after_boundary_authorization_comes_from_machine_phase() {
        let mut slot = TurnAdmissionSlot::new();
        assert!(slot.authorize_cancel_after_boundary().is_err());
        slot.claim().unwrap();
        slot.authorize_cancel_after_boundary()
            .expect("admitted slot should accept boundary cancel");
        slot.begin().unwrap();
        slot.authorize_cancel_after_boundary()
            .expect("running slot should accept boundary cancel");
        slot.resolve().unwrap();
        assert!(slot.authorize_cancel_after_boundary().is_err());
    }

    #[test]
    fn start_turn_dispatch_authorization_comes_from_machine_phase() {
        let mut slot = TurnAdmissionSlot::new();
        assert!(slot.authorize_start_turn_dispatch().is_err());
        slot.claim().unwrap();
        let authorization = slot
            .authorize_start_turn_dispatch()
            .expect("admitted slot should authorize dispatch");
        assert_eq!(authorization, StartTurnDispatchAuthorization::Authorized);
        slot.request_shutdown().unwrap();
        let authorization = slot
            .authorize_start_turn_dispatch()
            .expect("shutting-down slot should produce cancellation feedback");
        assert_eq!(authorization, StartTurnDispatchAuthorization::Cancelled);
    }

    #[test]
    fn content_turn_always_runs() {
        let disposition = claimed_disposition(
            Some(RuntimeExecutionKind::ContentTurn),
            meerkat_core::types::ContentInput::Text(String::new()),
            ObservedSessionTailKind::Empty,
            0,
        );
        assert_eq!(disposition, StartTurnDisposition::RunContentTurn);
    }

    #[test]
    fn resume_pending_with_session_boundary() {
        let disposition = claimed_disposition(
            Some(RuntimeExecutionKind::ResumePending),
            meerkat_core::types::ContentInput::Text(String::new()),
            ObservedSessionTailKind::User,
            0,
        );
        assert_eq!(disposition, StartTurnDisposition::RunPending);

        let disposition = claimed_disposition(
            Some(RuntimeExecutionKind::ResumePending),
            meerkat_core::types::ContentInput::Text(String::new()),
            ObservedSessionTailKind::ToolResults,
            0,
        );
        assert_eq!(disposition, StartTurnDisposition::RunPending);
    }

    #[test]
    fn resume_pending_with_staged_tool_results() {
        let disposition = claimed_disposition(
            Some(RuntimeExecutionKind::ResumePending),
            meerkat_core::types::ContentInput::Text(String::new()),
            ObservedSessionTailKind::Empty,
            1,
        );
        assert_eq!(disposition, StartTurnDisposition::RunPending);
    }

    #[test]
    fn resume_pending_no_boundary_no_staged() {
        let mut slot = TurnAdmissionSlot::new();
        slot.claim().expect("claim should admit disposition check");
        let disposition = slot
            .resolve_start_turn_disposition(
                Some(RuntimeExecutionKind::ResumePending),
                &meerkat_core::types::ContentInput::Text(String::new()),
                ObservedSessionTailKind::Assistant,
                0,
            )
            .expect("generated disposition should resolve");
        assert_eq!(
            disposition.public_terminal,
            Some(StartTurnPublicTerminal::NoPendingBoundary)
        );
        let terminal = slot
            .resolve_last_start_turn_public_terminal()
            .expect("generated public terminal should be retained");
        assert_eq!(terminal, StartTurnPublicTerminal::NoPendingBoundary);
        assert_eq!(
            disposition.disposition,
            StartTurnDisposition::NoPendingBoundary
        );
    }

    #[test]
    fn non_terminal_disposition_has_no_public_terminal() {
        let mut slot = TurnAdmissionSlot::new();
        slot.claim().expect("claim should admit disposition check");
        let disposition = slot
            .resolve_start_turn_disposition(
                Some(RuntimeExecutionKind::ContentTurn),
                &meerkat_core::types::ContentInput::Text(String::new()),
                ObservedSessionTailKind::Empty,
                0,
            )
            .expect("generated disposition should resolve");
        assert_eq!(
            disposition.disposition,
            StartTurnDisposition::RunContentTurn
        );
        assert_eq!(disposition.public_terminal, None);
        assert!(slot.resolve_last_start_turn_public_terminal().is_err());
    }

    #[test]
    fn direct_no_pending_public_terminal_is_generated() {
        let mut slot = TurnAdmissionSlot::new();
        slot.claim().expect("claim should admit disposition check");
        let disposition = slot
            .resolve_start_turn_disposition(
                None,
                &meerkat_core::types::ContentInput::Text(String::new()),
                ObservedSessionTailKind::Empty,
                0,
            )
            .expect("generated disposition should resolve");
        assert_eq!(
            disposition.public_terminal,
            Some(StartTurnPublicTerminal::NoPendingBoundary)
        );
        let terminal = slot
            .resolve_last_start_turn_public_terminal()
            .expect("generated public terminal should be retained");
        assert_eq!(terminal, StartTurnPublicTerminal::NoPendingBoundary);
        assert_eq!(
            disposition.disposition,
            StartTurnDisposition::NoPendingBoundary
        );
    }

    #[test]
    fn none_with_prompt_runs_content_turn() {
        let disposition = claimed_disposition(
            None,
            meerkat_core::types::ContentInput::Text("hello".into()),
            ObservedSessionTailKind::Empty,
            0,
        );
        assert_eq!(disposition, StartTurnDisposition::RunContentTurn);
    }

    #[test]
    fn none_empty_prompt_with_boundary_runs_pending() {
        let disposition = claimed_disposition(
            None,
            meerkat_core::types::ContentInput::Text(String::new()),
            ObservedSessionTailKind::User,
            0,
        );
        assert_eq!(disposition, StartTurnDisposition::RunPending);
    }

    #[test]
    fn none_empty_prompt_no_boundary_is_no_pending() {
        let disposition = claimed_disposition(
            None,
            meerkat_core::types::ContentInput::Text(String::new()),
            ObservedSessionTailKind::Empty,
            0,
        );
        assert_eq!(disposition, StartTurnDisposition::NoPendingBoundary);
    }

    #[test]
    fn none_empty_prompt_staged_tool_results_runs_pending() {
        let disposition = claimed_disposition(
            None,
            meerkat_core::types::ContentInput::Text(String::new()),
            ObservedSessionTailKind::Empty,
            1,
        );
        assert_eq!(disposition, StartTurnDisposition::RunPending);
    }
}

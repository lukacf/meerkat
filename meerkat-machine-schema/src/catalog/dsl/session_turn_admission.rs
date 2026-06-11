//! SessionTurnAdmissionMachine — canonical ephemeral turn-admission gate.
//!
//! This is the live turn-admission lifecycle owned by
//! `EphemeralSessionService` (the `turn_admission: Arc<Mutex<TurnAdmissionSlot>>`
//! slot). It models the multi-phase admission lifecycle
//! (`Idle`/`Admitted`/`Running`/`Completing`/`ShuttingDown`) and the
//! start-turn disposition / interrupt / shutdown / dispatch-authorization
//! decisions. It was previously an inline `machine!` in
//! `meerkat-session/src/turn_admission.rs` with no TLA model; it is now a
//! canonical machine with a generated schema-free authority and a standalone
//! TLA spec (LUC-524, P0 Dogma Invariant 1).
//!
//! Its `ResolveStartTurnDisposition` consumes the pending-continuation
//! disposition emitted by the canonical
//! [`crate::catalog::dsl::session_document`] machine as a typed input
//! (`PendingContinuationDisposition`); the meerkat-session shell drives the
//! SessionDocumentMachine first and MIRRORS that disposition into this
//! machine's input.

use meerkat_machine_dsl::machine;

/// Execution kind requested for a start-turn (or absent, meaning a direct
/// prompt-driven turn).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum StartTurnExecutionKind {
    #[default]
    ContentTurn,
    ResumePending,
}

/// Resolved start-turn disposition emitted by the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum StartTurnDisposition {
    #[default]
    RunContentTurn,
    RunPending,
    NoPendingBoundary,
}

/// Public terminal witness emitted alongside a `NoPendingBoundary` disposition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum StartTurnPublicTerminal {
    #[default]
    NoPendingBoundary,
}

/// Start-turn dispatch authorization verdict from the admission phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum StartTurnDispatchAuthorization {
    #[default]
    Authorized,
    Cancelled,
}

/// Pending-continuation disposition. Owned by the canonical
/// [`crate::catalog::dsl::session_document`] machine and consumed here as a
/// typed input mirrored from that machine's emitted decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum PendingContinuationDisposition {
    RunPending,
    #[default]
    NoPendingBoundary,
}

/// Tri-state runtime keep-alive request carried into the admission machine.
///
/// This is the canonical typed decision the shell previously collapsed into an
/// `Option<bool>`: `Some(true)` -> [`Enable`](RuntimeKeepAliveRequest::Enable),
/// `Some(false)` -> [`Disable`](RuntimeKeepAliveRequest::Disable), and absent
/// -> [`Preserve`](RuntimeKeepAliveRequest::Preserve). `Disable` is distinct
/// from `Preserve`: both resolve `persist_keep_alive = false`, but `Disable`
/// records an explicit operator intent to turn keep-alive off, whereas
/// `Preserve` leaves the prior keep-alive standing unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RuntimeKeepAliveRequest {
    Enable,
    Disable,
    #[default]
    Preserve,
}

/// Machine-resolved keep-alive persistence decision.
///
/// The former `persist_keep_alive: bool` effect payload collapsed `Disable`
/// (persist keep-alive = false) and `Preserve` (leave the existing setting
/// unchanged) into the same `false`, so an explicit disable could never reach
/// the durable session metadata. This closed tri-state is the canonical
/// decision the shell mirrors: `PersistEnabled` writes `keep_alive = true`,
/// `PersistDisabled` writes `keep_alive = false`, `PreserveExisting` writes
/// nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RuntimeKeepAlivePersistenceDecision {
    PersistEnabled,
    PersistDisabled,
    #[default]
    PreserveExisting,
}

machine! {
    machine SessionTurnAdmissionMachine {
        version: 1,
        rust: "self" / "catalog::dsl::session_turn_admission",

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
            ResolveRuntimeKeepAlive { keep_alive_request: Enum<RuntimeKeepAliveRequest> },
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
            RuntimeKeepAliveResolved { decision: Enum<RuntimeKeepAlivePersistenceDecision> },
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

        disposition TurnAdmissionProjected => local seam NoOwnerRealization,
        disposition TurnInterruptRequested => local seam NoOwnerRealization,
        disposition StartTurnDispatchResolved => local seam NoOwnerRealization,
        disposition CancelAfterBoundaryAuthorized => local seam NoOwnerRealization,
        disposition StartTurnDispositionResolved => local seam NoOwnerRealization,
        disposition StartTurnPublicTerminalResolved => local seam NoOwnerRealization,
        disposition RuntimeKeepAliveResolved => local seam NoOwnerRealization,

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
            on input ResolveRuntimeKeepAlive { keep_alive_request }
            guard {
                self.lifecycle_phase == Phase::Admitted
                && keep_alive_request == RuntimeKeepAliveRequest::Enable
            }
            update {}
            to Admitted
            emit RuntimeKeepAliveResolved { decision: RuntimeKeepAlivePersistenceDecision::PersistEnabled }
        }

        transition ResolveRuntimeKeepAliveDisable {
            on input ResolveRuntimeKeepAlive { keep_alive_request }
            guard {
                self.lifecycle_phase == Phase::Admitted
                && keep_alive_request == RuntimeKeepAliveRequest::Disable
            }
            update {}
            to Admitted
            emit RuntimeKeepAliveResolved { decision: RuntimeKeepAlivePersistenceDecision::PersistDisabled }
        }

        transition ResolveRuntimeKeepAlivePreserve {
            on input ResolveRuntimeKeepAlive { keep_alive_request }
            guard {
                self.lifecycle_phase == Phase::Admitted
                && keep_alive_request == RuntimeKeepAliveRequest::Preserve
            }
            update {}
            to Admitted
            emit RuntimeKeepAliveResolved { decision: RuntimeKeepAlivePersistenceDecision::PreserveExisting }
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

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
//!
//! Teardown discipline (0.7.2, undisciplined-shell-inputs D1/D2a):
//!
//! - **D1 — machine-owned admission drain.** Every transition INTO
//!   `ShuttingDown` mints a drain obligation (`admission_drain_pending`,
//!   `PendingAdmissionDrainRequested`): the owning shell must flush the
//!   session command queue, resolving every pending turn-admission waiter
//!   with a typed terminal outcome instead of dropping reply channels, then
//!   close the obligation with `ResolvePendingAdmissionDrained`. The final
//!   teardown step (`AuthorizeSessionTeardown` -> `SessionTeardownAuthorized`)
//!   is guarded on the drain obligation being closed, so "torn down" is
//!   *defined* as "pending admission work resolved".
//! - **D2a — total inputs in `ShuttingDown`.** Inputs that legitimately race
//!   archive/discard teardown (`ClaimTurn`, `AbortClaim`, `BeginTurn`,
//!   `ResolveStartTurnDisposition`, `ResolveRuntimeKeepAlive`, and the
//!   runtime-system-context application authorization) are accepted in
//!   `ShuttingDown` with an explicit no-op or typed
//!   "session archived" terminal (`TurnAdmissionShutdownTerminalResolved`,
//!   `RuntimeSystemContextApplicationResolved { SessionArchived }`) instead
//!   of guard rejections, mirroring the existing
//!   `AuthorizeStartTurnDispatchShuttingDown` -> `Cancelled` precedent.

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

/// Authorization verdict for applying runtime-owned system context to the
/// live session (the `ApplyRuntimeSystemContext` /
/// `ApplyRuntimeSystemContextForTurn` session commands).
///
/// `SessionArchived` is the typed benign terminal for applications that
/// arrive while the machine is `ShuttingDown` (archive/discard teardown won
/// the race): the arrival is legitimate, the application is dropped with the
/// archived session, and the caller resolves its waiter with the archived
/// outcome instead of a guard rejection or a dropped reply channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RuntimeSystemContextApplicationAuthorization {
    #[default]
    Authorized,
    SessionArchived,
}

/// Typed terminal disposition for turn-admission work that arrives while the
/// machine is `ShuttingDown` (the session was archived or its live handle
/// was discarded in favor of durable authority). This is the machine-owned
/// answer for legitimately-racing inputs: never a guard rejection, never a
/// generic internal error for a committed teardown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum TurnAdmissionShutdownTerminal {
    #[default]
    SessionArchived,
}

machine! {
    machine SessionTurnAdmissionMachine {
        version: 1,
        rust: "self" / "catalog::dsl::session_turn_admission",

        state {
            lifecycle_phase: TurnAdmissionPhase,
            interrupt_pending: bool,
            shutdown_pending: bool,
            // D1 drain obligation: minted on every entry into ShuttingDown,
            // closed by the owning shell's ResolvePendingAdmissionDrained
            // feedback after it has flushed the session command queue and
            // resolved every pending admission waiter with a typed terminal.
            admission_drain_pending: bool,
            last_public_terminal: Option<Enum<StartTurnPublicTerminal>>,
        }

        init(Idle) {
            interrupt_pending = false,
            shutdown_pending = false,
            admission_drain_pending = false,
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
            AuthorizeRuntimeSystemContextApplication,
            ResolvePendingAdmissionDrained,
            AuthorizeSessionTeardown,
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
            PendingAdmissionDrainRequested,
            RuntimeSystemContextApplicationResolved {
                authorization: Enum<RuntimeSystemContextApplicationAuthorization>,
            },
            TurnAdmissionShutdownTerminalResolved { terminal: Enum<TurnAdmissionShutdownTerminal> },
            SessionTeardownAuthorized,
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

        // The drain obligation only exists while shutting down: it is minted
        // exclusively on entries into ShuttingDown and ShuttingDown has no
        // exits, so a pending obligation outside ShuttingDown is
        // unrepresentable.
        invariant drain_obligation_only_while_shutting_down {
            self.admission_drain_pending == false || self.lifecycle_phase == Phase::ShuttingDown
        }

        disposition TurnAdmissionProjected => local seam NoOwnerRealization,
        disposition TurnInterruptRequested => local seam NoOwnerRealization,
        disposition StartTurnDispatchResolved => local seam NoOwnerRealization,
        disposition CancelAfterBoundaryAuthorized => local seam NoOwnerRealization,
        disposition StartTurnDispositionResolved => local seam NoOwnerRealization,
        disposition StartTurnPublicTerminalResolved => local seam NoOwnerRealization,
        disposition RuntimeKeepAliveResolved => local seam NoOwnerRealization,
        disposition PendingAdmissionDrainRequested => local seam NoOwnerRealization,
        disposition RuntimeSystemContextApplicationResolved => local seam NoOwnerRealization,
        disposition TurnAdmissionShutdownTerminalResolved => local seam NoOwnerRealization,
        disposition SessionTeardownAuthorized => local seam NoOwnerRealization,

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

        // D2a: a claim attempt that lost the race against archive/discard
        // teardown gets the typed "session archived" terminal instead of a
        // guard rejection. No admission is granted; the machine stays in
        // ShuttingDown. (Deliberately no projection emit: pre-cutover mirrors
        // that only understand projections keep failing closed.)
        transition ClaimTurnShuttingDown {
            on input ClaimTurn
            guard { self.lifecycle_phase == Phase::ShuttingDown }
            update {}
            to ShuttingDown
            emit TurnAdmissionShutdownTerminalResolved {
                terminal: TurnAdmissionShutdownTerminal::SessionArchived,
            }
        }

        // D2a: an abort for a claim that teardown already discarded
        // (RequestShutdownImmediateAdmitted reset the claim state) is an
        // explicit no-op, not an error to be swallowed by the shell.
        transition AbortClaimShuttingDown {
            on input AbortClaim
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

        // D2a: archive/discard committed RequestShutdownImmediateAdmitted
        // between the admitted preflight and the begin step. The run must not
        // start; the waiter resolves with the typed "session archived"
        // terminal instead of an illegal-transition internal error.
        // (Deliberately no projection emit: the run may only begin on a
        // projected Running transition.)
        transition BeginTurnShuttingDown {
            on input BeginTurn
            guard { self.lifecycle_phase == Phase::ShuttingDown }
            update {}
            to ShuttingDown
            emit TurnAdmissionShutdownTerminalResolved {
                terminal: TurnAdmissionShutdownTerminal::SessionArchived,
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
                self.admission_drain_pending = true;
            }
            to ShuttingDown
            emit PendingAdmissionDrainRequested
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
                self.admission_drain_pending = true;
            }
            to ShuttingDown
            emit PendingAdmissionDrainRequested
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
                self.admission_drain_pending = true;
            }
            to ShuttingDown
            emit PendingAdmissionDrainRequested
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

        // D1 drain feedback: the owning shell flushed the session command
        // queue and resolved every pending admission waiter with a typed
        // terminal outcome; the obligation closes. A second feedback without
        // an open obligation is a shell bug and stays rejected.
        transition ResolvePendingAdmissionDrained {
            on input ResolvePendingAdmissionDrained
            guard { self.lifecycle_phase == Phase::ShuttingDown && self.admission_drain_pending }
            update {
                self.admission_drain_pending = false;
            }
            to ShuttingDown
            emit TurnAdmissionProjected {
                phase: self.lifecycle_phase,
                interrupt_pending: self.interrupt_pending,
                shutdown_pending: self.shutdown_pending,
                is_active: is_active_phase(self.lifecycle_phase),
            }
        }

        // D1 final teardown gate: dropping the session task / live state is
        // only authorized once the drain obligation has closed, so "torn
        // down" is defined as "pending admission work resolved".
        transition AuthorizeSessionTeardown {
            on input AuthorizeSessionTeardown
            guard {
                self.lifecycle_phase == Phase::ShuttingDown
                && self.admission_drain_pending == false
            }
            update {}
            to ShuttingDown
            emit SessionTeardownAuthorized
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

        // Machine-owned legality decision for applying runtime-owned system
        // context to the live session (the ApplyRuntimeSystemContext /
        // ApplyRuntimeSystemContextForTurn session commands). The shell fires
        // this before touching the live agent; it decides nothing itself.
        transition AuthorizeRuntimeSystemContextApplicationActive {
            per_phase [Idle, Admitted, Running, Completing]
            on input AuthorizeRuntimeSystemContextApplication
            update {}
            to Idle
            emit RuntimeSystemContextApplicationResolved {
                authorization: RuntimeSystemContextApplicationAuthorization::Authorized,
            }
        }

        // D2a: a context application that lost the race against
        // archive/discard teardown resolves with the typed SessionArchived
        // verdict — the appends are dropped with the archived session and the
        // caller's waiter resolves benignly instead of erroring or hanging.
        transition AuthorizeRuntimeSystemContextApplicationShuttingDown {
            on input AuthorizeRuntimeSystemContextApplication
            guard { self.lifecycle_phase == Phase::ShuttingDown }
            update {}
            to ShuttingDown
            emit RuntimeSystemContextApplicationResolved {
                authorization: RuntimeSystemContextApplicationAuthorization::SessionArchived,
            }
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

        // D2a: a start-turn disposition request that lost the race against
        // archive/discard teardown (RequestShutdownImmediateAdmitted landed
        // between the dispatch authorization and this resolution) gets the
        // typed "session archived" terminal instead of a guard rejection.
        // (Deliberately no StartTurnDispositionResolved emit: no runnable
        // disposition exists for a shutting-down session.)
        transition ResolveStartTurnDispositionShuttingDown {
            on input ResolveStartTurnDisposition {
                execution_kind_present,
                execution_kind,
                prompt_trimmed_text_byte_count,
                prompt_non_text_block_count,
                pending_continuation
            }
            guard { self.lifecycle_phase == Phase::ShuttingDown }
            update {}
            to ShuttingDown
            emit TurnAdmissionShutdownTerminalResolved {
                terminal: TurnAdmissionShutdownTerminal::SessionArchived,
            }
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

        // D2a: a keep-alive resolution that lost the race against
        // archive/discard teardown gets the typed "session archived" terminal
        // instead of a guard rejection. No persistence decision is emitted —
        // nothing may be persisted for a session whose teardown committed.
        transition ResolveRuntimeKeepAliveShuttingDown {
            on input ResolveRuntimeKeepAlive { keep_alive_request }
            guard { self.lifecycle_phase == Phase::ShuttingDown }
            update {}
            to ShuttingDown
            emit TurnAdmissionShutdownTerminalResolved {
                terminal: TurnAdmissionShutdownTerminal::SessionArchived,
            }
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

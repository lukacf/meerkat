//! Turn admission shell for `EphemeralSessionService`.
//!
//! The mutex/notify/channel code around this type is service mechanics. The
//! semantic facts it carries — phase, transition legality, active projection,
//! interrupt/shutdown acceptance, and start-turn disposition — are owned by the
//! canonical `SessionTurnAdmissionMachine`. Its generated, schema-free
//! authority is emitted into `crate::generated::session_turn_admission` by
//! `xtask protocol-codegen` (DSL in
//! `meerkat-machine-schema/src/catalog/dsl/session_turn_admission.rs`, TLA
//! model in `specs/machines/session_turn_admission/`). This shell drives the
//! generated authority and MIRRORS the emitted decisions; it owns no semantics.

use std::fmt;

use meerkat_core::lifecycle::RuntimeExecutionKind;

mod authority {
    pub(crate) use crate::generated::session_turn_admission::{
        RuntimeKeepAlivePersistenceDecision, RuntimeKeepAliveRequest, SessionTurnAdmissionEffect,
        SessionTurnAdmissionMachineAuthority, StartTurnDispatchAuthorization, StartTurnDisposition,
        StartTurnExecutionKind, StartTurnPublicTerminal, TurnAdmissionPhase,
    };
}

pub(crate) use authority::RuntimeKeepAlivePersistenceDecision;
pub(crate) use authority::RuntimeKeepAliveRequest;
pub(crate) use authority::StartTurnDispatchAuthorization;
pub(crate) use authority::StartTurnDisposition;
pub(crate) use authority::StartTurnPublicTerminal;
pub(crate) use authority::TurnAdmissionPhase;
pub use meerkat_core::pending_continuation::ObservedSessionTailKind;

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

    /// Generated D1 drain-obligation fact: true between an entry into
    /// `ShuttingDown` and the shell's `ResolvePendingAdmissionDrained`
    /// feedback.
    #[cfg(test)]
    pub(crate) fn admission_drain_pending(&self) -> bool {
        self.authority.state().admission_drain_pending
    }

    /// Claim the turn slot before dispatching `StartTurn`. `Idle -> Admitted`.
    pub(crate) fn claim(&mut self) -> Result<TurnAdmissionPhase, TurnAdmissionError> {
        let from = self.phase();
        let effects = self
            .authority
            .claim_turn()
            .map_err(|_| TurnAdmissionError { from, op: "claim" })?;
        self.update_projection_from_effects(&effects, from, "claim")?;
        Ok(self.phase())
    }

    /// Release an admitted slot without running the turn. `Admitted -> Idle`.
    pub(crate) fn abort_claim(&mut self) -> Result<TurnAdmissionPhase, TurnAdmissionError> {
        let from = self.phase();
        let effects = self
            .authority
            .abort_claim()
            .map_err(|_| TurnAdmissionError {
                from,
                op: "abort_claim",
            })?;
        self.update_projection_from_effects(&effects, from, "abort_claim")?;
        Ok(self.phase())
    }

    /// Mark the admitted slot as actively running. `Admitted -> Running`.
    pub(crate) fn begin(&mut self) -> Result<TurnAdmissionPhase, TurnAdmissionError> {
        let from = self.phase();
        let effects = self
            .authority
            .begin_turn()
            .map_err(|_| TurnAdmissionError { from, op: "begin" })?;
        self.update_projection_from_effects(&effects, from, "begin")?;
        Ok(self.phase())
    }

    /// Move a finished run into the finalization window. `Running -> Completing`.
    pub(crate) fn resolve(&mut self) -> Result<TurnAdmissionPhase, TurnAdmissionError> {
        let from = self.phase();
        let effects = self
            .authority
            .resolve_turn()
            .map_err(|_| TurnAdmissionError {
                from,
                op: "resolve",
            })?;
        self.update_projection_from_effects(&effects, from, "resolve")?;
        Ok(self.phase())
    }

    /// Close the finalization window. `Completing -> Idle` unless shutdown was
    /// requested mid-turn, in which case `Completing -> ShuttingDown`.
    pub(crate) fn finalize(&mut self) -> Result<FinalizeOutcome, TurnAdmissionError> {
        let from = self.phase();
        let effects = self
            .authority
            .finalize_turn()
            .map_err(|_| TurnAdmissionError {
                from,
                op: "finalize",
            })?;
        self.update_projection_from_effects(&effects, from, "finalize")?;
        Ok(FinalizeOutcome {
            next_phase: self.phase(),
        })
    }

    /// Request an interrupt through generated legality and wake feedback.
    pub(crate) fn request_interrupt(&mut self) -> Result<bool, TurnAdmissionError> {
        let from = self.phase();
        let effects = self
            .authority
            .request_interrupt()
            .map_err(|_| TurnAdmissionError {
                from,
                op: "request_interrupt",
            })?;
        let wake = effects
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
        self.update_projection_from_effects(&effects, from, "request_interrupt")?;
        Ok(wake)
    }

    /// Authorize boundary cancellation through generated admission state.
    pub(crate) fn authorize_cancel_after_boundary(&mut self) -> Result<(), TurnAdmissionError> {
        let from = self.phase();
        let effects = self
            .authority
            .authorize_cancel_after_boundary()
            .map_err(|_| TurnAdmissionError {
                from,
                op: "authorize_cancel_after_boundary",
            })?;
        if effects.iter().any(|effect| {
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
        let effects = self
            .authority
            .authorize_start_turn_dispatch()
            .map_err(|_| TurnAdmissionError {
                from,
                op: "authorize_start_turn_dispatch",
            })?;
        effects
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
        let from = self.phase();
        let effects = self
            .authority
            .request_shutdown()
            .map_err(|_| TurnAdmissionError {
                from,
                op: "request_shutdown",
            })?;
        self.update_projection_from_effects(&effects, from, "request_shutdown")?;
        Ok(self.phase())
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
        // Drive the canonical SessionDocumentMachine pending-continuation
        // decision first, then MIRROR its disposition into this machine's
        // start-turn-disposition input.
        let pending_resolution = meerkat_core::pending_continuation::resolve_pending_continuation(
            session_tail,
            staged_tool_result_count,
        )
        .map_err(|_| TurnAdmissionError {
            from: self.phase(),
            op: "resolve_pending_continuation",
        })?;
        if pending_resolution.disposition
            == meerkat_core::session_document::PendingContinuationDisposition::NoPendingBoundary
            && pending_resolution.public_terminal
                != Some(
                    meerkat_core::session_document::PendingContinuationPublicTerminal::NoPendingBoundary,
                )
        {
            return Err(TurnAdmissionError {
                from: self.phase(),
                op: "resolve_pending_continuation_terminal",
            });
        }
        let from = self.phase();
        let effects = self
            .authority
            .resolve_start_turn_disposition(
                execution_kind_present,
                execution_kind,
                prompt_observation.trimmed_text_byte_count,
                prompt_observation.non_text_block_count,
                pending_resolution.disposition,
            )
            .map_err(|_| TurnAdmissionError {
                from,
                op: "resolve_start_turn_disposition",
            })?;
        let disposition = effects
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
        let public_terminal = effects.iter().find_map(|effect| match effect {
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
        let effects = self
            .authority
            .resolve_last_start_turn_public_terminal()
            .map_err(|_| TurnAdmissionError {
                from,
                op: "resolve_last_start_turn_public_terminal",
            })?;
        effects
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
        request: RuntimeKeepAliveRequest,
    ) -> Result<RuntimeKeepAlivePersistenceDecision, TurnAdmissionError> {
        let from = self.phase();
        let effects = self
            .authority
            .resolve_runtime_keep_alive(request)
            .map_err(|_| TurnAdmissionError {
                from,
                op: "resolve_runtime_keep_alive",
            })?;
        effects
            .iter()
            .find_map(|effect| match effect {
                authority::SessionTurnAdmissionEffect::RuntimeKeepAliveResolved { decision } => {
                    Some(*decision)
                }
                _ => None,
            })
            .ok_or(TurnAdmissionError {
                from,
                op: "resolve_runtime_keep_alive",
            })
    }

    fn refresh_projection(&mut self) -> Result<(), TurnAdmissionError> {
        let from = self.phase();
        let effects = self
            .authority
            .project_turn_admission()
            .map_err(|_| TurnAdmissionError {
                from,
                op: "project_turn_admission",
            })?;
        self.update_projection_from_effects(&effects, from, "project_turn_admission")
    }

    fn update_projection_from_effects(
        &mut self,
        effects: &[authority::SessionTurnAdmissionEffect],
        from: TurnAdmissionPhase,
        op: &'static str,
    ) -> Result<(), TurnAdmissionError> {
        let Some(projection) = effects.iter().find_map(|effect| match effect {
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
                ObservedSessionTailKind::BlockAssistant,
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

/// Machine-level acceptance/no-op/disposition tests for the 0.7.2
/// disciplined-shell-inputs DSL deltas (D1 drain obligations + D2a typed
/// terminals in `ShuttingDown`). These drive the generated authority
/// directly: the shell mirrors are rewired in the Stage B cutover.
#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod shutdown_drain_machine_tests {
    use crate::generated::session_turn_admission::{
        PendingContinuationDisposition, RuntimeKeepAlivePersistenceDecision,
        RuntimeKeepAliveRequest, RuntimeSystemContextApplicationAuthorization,
        SessionTurnAdmissionEffect, SessionTurnAdmissionMachineAuthority, StartTurnExecutionKind,
        TurnAdmissionPhase, TurnAdmissionShutdownTerminal,
    };

    fn drain_requested(effects: &[SessionTurnAdmissionEffect]) -> bool {
        effects.iter().any(|effect| {
            matches!(
                effect,
                SessionTurnAdmissionEffect::PendingAdmissionDrainRequested
            )
        })
    }

    fn shutdown_terminal(
        effects: &[SessionTurnAdmissionEffect],
    ) -> Option<TurnAdmissionShutdownTerminal> {
        effects.iter().find_map(|effect| match effect {
            SessionTurnAdmissionEffect::TurnAdmissionShutdownTerminalResolved { terminal } => {
                Some(*terminal)
            }
            _ => None,
        })
    }

    fn projected_phase(effects: &[SessionTurnAdmissionEffect]) -> Option<TurnAdmissionPhase> {
        effects.iter().find_map(|effect| match effect {
            SessionTurnAdmissionEffect::TurnAdmissionProjected { phase, .. } => Some(*phase),
            _ => None,
        })
    }

    fn context_application_authorization(
        effects: &[SessionTurnAdmissionEffect],
    ) -> Option<RuntimeSystemContextApplicationAuthorization> {
        effects.iter().find_map(|effect| match effect {
            SessionTurnAdmissionEffect::RuntimeSystemContextApplicationResolved {
                authorization,
            } => Some(*authorization),
            _ => None,
        })
    }

    fn shutting_down_authority() -> SessionTurnAdmissionMachineAuthority {
        let mut authority = SessionTurnAdmissionMachineAuthority::new();
        authority
            .request_shutdown()
            .expect("idle authority should accept shutdown");
        assert_eq!(
            authority.state().phase(),
            TurnAdmissionPhase::ShuttingDown,
            "idle shutdown should be immediate"
        );
        authority
    }

    fn drained_shutting_down_authority() -> SessionTurnAdmissionMachineAuthority {
        let mut authority = shutting_down_authority();
        authority
            .resolve_pending_admission_drained()
            .expect("open drain obligation should close");
        assert!(!authority.state().admission_drain_pending);
        authority
    }

    fn resolve_disposition_input(
        authority: &mut SessionTurnAdmissionMachineAuthority,
    ) -> Result<
        Vec<SessionTurnAdmissionEffect>,
        crate::generated::session_turn_admission::SessionTurnAdmissionError,
    > {
        authority.resolve_start_turn_disposition(
            true,
            StartTurnExecutionKind::ContentTurn,
            5,
            0,
            PendingContinuationDisposition::NoPendingBoundary,
        )
    }

    // ------------------------------------------------------------------
    // D1: drain obligation minting
    // ------------------------------------------------------------------

    #[test]
    fn request_shutdown_from_idle_mints_drain_obligation() {
        let mut authority = SessionTurnAdmissionMachineAuthority::new();
        assert!(!authority.state().admission_drain_pending);
        let effects = authority
            .request_shutdown()
            .expect("idle shutdown should commit");
        assert!(drain_requested(&effects));
        assert_eq!(
            projected_phase(&effects),
            Some(TurnAdmissionPhase::ShuttingDown)
        );
        assert!(authority.state().admission_drain_pending);
    }

    #[test]
    fn request_shutdown_from_admitted_mints_drain_obligation() {
        let mut authority = SessionTurnAdmissionMachineAuthority::new();
        authority.claim_turn().expect("idle claims");
        let effects = authority
            .request_shutdown()
            .expect("admitted shutdown should commit");
        assert!(drain_requested(&effects));
        assert_eq!(
            authority.state().phase(),
            TurnAdmissionPhase::ShuttingDown
        );
        assert!(authority.state().admission_drain_pending);
    }

    #[test]
    fn deferred_shutdown_mints_drain_obligation_at_finalize() {
        let mut authority = SessionTurnAdmissionMachineAuthority::new();
        authority.claim_turn().expect("idle claims");
        authority.begin_turn().expect("admitted begins");
        let deferred = authority
            .request_shutdown()
            .expect("running shutdown defers");
        assert!(
            !drain_requested(&deferred),
            "deferred shutdown must not mint the drain obligation early"
        );
        assert!(!authority.state().admission_drain_pending);
        assert_eq!(authority.state().phase(), TurnAdmissionPhase::Running);

        authority.resolve_turn().expect("running resolves");
        let finalized = authority
            .finalize_turn()
            .expect("completing finalizes into shutdown");
        assert!(drain_requested(&finalized));
        assert_eq!(
            authority.state().phase(),
            TurnAdmissionPhase::ShuttingDown
        );
        assert!(authority.state().admission_drain_pending);
    }

    #[test]
    fn duplicate_request_shutdown_does_not_remint_drain_obligation() {
        let mut authority = drained_shutting_down_authority();
        let effects = authority
            .request_shutdown()
            .expect("duplicate shutdown is a no-op");
        assert!(!drain_requested(&effects));
        assert!(!authority.state().admission_drain_pending);
    }

    // ------------------------------------------------------------------
    // D1: drain feedback + teardown gate
    // ------------------------------------------------------------------

    #[test]
    fn resolve_pending_admission_drained_closes_obligation_exactly_once() {
        let mut authority = shutting_down_authority();
        assert!(authority.state().admission_drain_pending);
        let effects = authority
            .resolve_pending_admission_drained()
            .expect("open obligation should close");
        assert_eq!(
            projected_phase(&effects),
            Some(TurnAdmissionPhase::ShuttingDown)
        );
        assert!(!authority.state().admission_drain_pending);
        // A second feedback without an open obligation is a shell bug.
        assert!(authority.resolve_pending_admission_drained().is_err());
    }

    #[test]
    fn resolve_pending_admission_drained_rejected_outside_shutdown() {
        let mut authority = SessionTurnAdmissionMachineAuthority::new();
        assert!(authority.resolve_pending_admission_drained().is_err());
        authority.claim_turn().expect("idle claims");
        assert!(authority.resolve_pending_admission_drained().is_err());
    }

    #[test]
    fn session_teardown_blocked_until_drain_obligation_closes() {
        let mut authority = shutting_down_authority();
        assert!(
            authority.authorize_session_teardown().is_err(),
            "teardown must be blocked while the drain obligation is open"
        );
        authority
            .resolve_pending_admission_drained()
            .expect("open obligation should close");
        let effects = authority
            .authorize_session_teardown()
            .expect("drained shutdown should authorize teardown");
        assert!(effects.iter().any(|effect| matches!(
            effect,
            SessionTurnAdmissionEffect::SessionTeardownAuthorized
        )));
    }

    #[test]
    fn session_teardown_rejected_outside_shutdown() {
        let mut authority = SessionTurnAdmissionMachineAuthority::new();
        assert!(authority.authorize_session_teardown().is_err());
        authority.claim_turn().expect("idle claims");
        assert!(authority.authorize_session_teardown().is_err());
    }

    // ------------------------------------------------------------------
    // D2a: typed terminals / no-ops in ShuttingDown
    // ------------------------------------------------------------------

    #[test]
    fn claim_turn_in_shutdown_resolves_session_archived_terminal() {
        let mut authority = shutting_down_authority();
        let effects = authority
            .claim_turn()
            .expect("claim racing teardown is a legitimate arrival");
        assert_eq!(
            shutdown_terminal(&effects),
            Some(TurnAdmissionShutdownTerminal::SessionArchived)
        );
        assert_eq!(
            projected_phase(&effects),
            None,
            "no admission projection may be emitted for a shutdown claim"
        );
        assert_eq!(
            authority.state().phase(),
            TurnAdmissionPhase::ShuttingDown
        );
    }

    #[test]
    fn abort_claim_in_shutdown_is_explicit_noop() {
        let mut authority = shutting_down_authority();
        let effects = authority
            .abort_claim()
            .expect("abort racing teardown is a legitimate no-op");
        assert_eq!(
            projected_phase(&effects),
            Some(TurnAdmissionPhase::ShuttingDown)
        );
        assert_eq!(
            authority.state().phase(),
            TurnAdmissionPhase::ShuttingDown
        );
    }

    #[test]
    fn begin_turn_in_shutdown_resolves_session_archived_terminal() {
        let mut authority = shutting_down_authority();
        let effects = authority
            .begin_turn()
            .expect("begin racing teardown is a legitimate arrival");
        assert_eq!(
            shutdown_terminal(&effects),
            Some(TurnAdmissionShutdownTerminal::SessionArchived)
        );
        assert_eq!(
            projected_phase(&effects),
            None,
            "a run may only begin on a projected Running transition"
        );
        assert_eq!(
            authority.state().phase(),
            TurnAdmissionPhase::ShuttingDown
        );
    }

    #[test]
    fn resolve_start_turn_disposition_in_shutdown_resolves_session_archived_terminal() {
        let mut authority = shutting_down_authority();
        let effects = resolve_disposition_input(&mut authority)
            .expect("disposition racing teardown is a legitimate arrival");
        assert_eq!(
            shutdown_terminal(&effects),
            Some(TurnAdmissionShutdownTerminal::SessionArchived)
        );
        assert!(
            !effects.iter().any(|effect| matches!(
                effect,
                SessionTurnAdmissionEffect::StartTurnDispositionResolved { .. }
            )),
            "no runnable disposition exists for a shutting-down session"
        );
        assert_eq!(
            authority.state().phase(),
            TurnAdmissionPhase::ShuttingDown
        );
    }

    #[test]
    fn resolve_runtime_keep_alive_in_shutdown_resolves_session_archived_terminal() {
        for request in [
            RuntimeKeepAliveRequest::Enable,
            RuntimeKeepAliveRequest::Disable,
            RuntimeKeepAliveRequest::Preserve,
        ] {
            let mut authority = shutting_down_authority();
            let effects = authority
                .resolve_runtime_keep_alive(request)
                .expect("keep-alive racing teardown is a legitimate arrival");
            assert_eq!(
                shutdown_terminal(&effects),
                Some(TurnAdmissionShutdownTerminal::SessionArchived)
            );
            assert!(
                !effects.iter().any(|effect| matches!(
                    effect,
                    SessionTurnAdmissionEffect::RuntimeKeepAliveResolved { .. }
                )),
                "nothing may be persisted for a session whose teardown committed"
            );
        }
    }

    // ------------------------------------------------------------------
    // Runtime system-context application authorization
    // ------------------------------------------------------------------

    #[test]
    fn runtime_system_context_application_authorized_in_all_live_phases() {
        let mut authority = SessionTurnAdmissionMachineAuthority::new();
        for expected_phase in [
            TurnAdmissionPhase::Idle,
            TurnAdmissionPhase::Admitted,
            TurnAdmissionPhase::Running,
            TurnAdmissionPhase::Completing,
        ] {
            match expected_phase {
                TurnAdmissionPhase::Idle => {}
                TurnAdmissionPhase::Admitted => {
                    authority.claim_turn().expect("idle claims");
                }
                TurnAdmissionPhase::Running => {
                    authority.begin_turn().expect("admitted begins");
                }
                TurnAdmissionPhase::Completing => {
                    authority.resolve_turn().expect("running resolves");
                }
                TurnAdmissionPhase::ShuttingDown => unreachable!("not a live phase"),
            }
            assert_eq!(authority.state().phase(), expected_phase);
            let effects = authority
                .authorize_runtime_system_context_application()
                .expect("live session accepts context application");
            assert_eq!(
                context_application_authorization(&effects),
                Some(RuntimeSystemContextApplicationAuthorization::Authorized)
            );
            assert_eq!(
                authority.state().phase(),
                expected_phase,
                "authorization must not move the phase"
            );
        }
    }

    #[test]
    fn runtime_system_context_application_resolves_archived_in_shutdown() {
        let mut authority = shutting_down_authority();
        let effects = authority
            .authorize_runtime_system_context_application()
            .expect("context application racing teardown is a legitimate arrival");
        assert_eq!(
            context_application_authorization(&effects),
            Some(RuntimeSystemContextApplicationAuthorization::SessionArchived)
        );
        assert_eq!(
            authority.state().phase(),
            TurnAdmissionPhase::ShuttingDown
        );
    }

    // ------------------------------------------------------------------
    // Vocabulary sanity: shutdown keep-alive terminal does not leak a
    // persistence decision default.
    // ------------------------------------------------------------------

    #[test]
    fn keep_alive_persistence_default_preserves_existing() {
        assert_eq!(
            RuntimeKeepAlivePersistenceDecision::default(),
            RuntimeKeepAlivePersistenceDecision::PreserveExisting
        );
    }
}

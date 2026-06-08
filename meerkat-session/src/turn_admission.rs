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
        RuntimeKeepAliveRequest, SessionTurnAdmissionEffect, SessionTurnAdmissionMachineAuthority,
        StartTurnDispatchAuthorization, StartTurnDisposition, StartTurnExecutionKind,
        StartTurnPublicTerminal, TurnAdmissionPhase,
    };
}

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
    ) -> Result<bool, TurnAdmissionError> {
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

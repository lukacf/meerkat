//! A host's request that a session's next compaction check compact
//! ([`MeerkatMachine::request_compaction`]).

use super::{MeerkatMachine, dsl};
use crate::compaction_request::CompactionRequestReceipt;
use crate::traits::RuntimeDriverError;

impl MeerkatMachine {
    /// Ask `session_id`'s next compaction check to compact, now: without a
    /// rebuild of the session and without changing any compaction threshold.
    ///
    /// The machine decides (`RequestCompaction`), under the session's
    /// mutation gate. A registration that serves (idle, attached, or running
    /// a turn) records the request and answers
    /// [`CompactionRequestReceipt::Requested`], or
    /// [`CompactionRequestReceipt::AlreadyRequested`] when it already holds
    /// one that no check consumed yet. The request is consumed by the next
    /// compaction check of a run: the check the agent evaluates before its
    /// next model request, in the running turn (once its tool results are
    /// in) or in the session's next run. That check compacts without asking
    /// the compactor's trigger (its thresholds and cadence); the compaction's
    /// own rules still hold, so a run in flight keeps its request and its
    /// newest round, a check with nothing to fold compacts nothing, and so
    /// does the session's first model request (a request made before it is
    /// consumed there). The
    /// compaction publishes the session's ordinary `CompactionStarted` and
    /// `CompactionCompleted` events.
    ///
    /// The request is a fact of this registration only: it is never
    /// persisted. A re-registration of the same session keeps it; an
    /// unregister, a durability reload or the end of the process drops it.
    /// A per-build compaction threshold override is neither read nor
    /// changed, and neither is the realm's compaction configuration.
    ///
    /// The call never wakes, loads or attaches a session.
    ///
    /// # Errors
    /// [`RuntimeDriverError::NotReady`] with the registration's state when it
    /// is initializing, retired, stopped or destroyed, and with
    /// [`crate::RuntimeState::Destroyed`] for a session with no registration
    /// in this process (a dormant session is never woken for a request); the
    /// refusal of a registration that needs a durability reload; and the
    /// refusal of one whose unregister is owed or running.
    pub async fn request_compaction(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<CompactionRequestReceipt, RuntimeDriverError> {
        let _gate = self
            .lock_current_durability_ready_session_mutation_gate(session_id)
            .await?;
        if let Some(refusal) = self
            .lifecycle_mutation_refused_by_unregister(session_id)
            .await
        {
            return Err(refusal);
        }
        let (previous, effects) = self
            .apply_session_dsl_input(
                session_id,
                dsl::MeerkatMachineInput::RequestCompaction,
                "RequestCompaction",
            )
            .await
            .map_err(RuntimeDriverError::Internal)?;
        let verdict = effects
            .as_slice()
            .iter()
            .find_map(|effect| match effect {
                dsl::MeerkatMachineEffect::CompactionRequestResolved { verdict } => Some(*verdict),
                _ => None,
            })
            .ok_or_else(|| {
                RuntimeDriverError::Internal(
                    "RequestCompaction committed without CompactionRequestResolved".to_string(),
                )
            })?;
        match verdict {
            dsl::CompactionRequestVerdict::Requested => Ok(CompactionRequestReceipt::Requested),
            dsl::CompactionRequestVerdict::AlreadyRequested => {
                Ok(CompactionRequestReceipt::AlreadyRequested)
            }
            // The refusing arms change nothing, so the phase before the
            // input is the phase that refused it.
            dsl::CompactionRequestVerdict::NotServing => Err(RuntimeDriverError::NotReady {
                state: super::dsl_authority::write_back_phase(previous.state().lifecycle_phase),
            }),
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::dsl;

    fn authority_in(phase: dsl::MeerkatPhase, requested: bool) -> dsl::MeerkatMachineAuthority {
        let state = dsl::MeerkatMachineState {
            lifecycle_phase: phase,
            session_id: Some(dsl::SessionId("compaction-request-session".to_string())),
            compaction_requested: requested,
            ..Default::default()
        };
        dsl::MeerkatMachineAuthority::recover_from_state(state)
            .unwrap_or_else(|error| panic!("a {phase:?} state recovers: {error:?}"))
    }

    fn request(authority: &mut dsl::MeerkatMachineAuthority) -> dsl::CompactionRequestVerdict {
        let transition = dsl::MeerkatMachineMutator::apply(
            authority,
            dsl::MeerkatMachineInput::RequestCompaction,
        )
        .expect("RequestCompaction is total over every phase");
        let verdicts = transition
            .into_effects()
            .into_iter()
            .filter_map(|effect| match effect {
                dsl::MeerkatMachineEffect::CompactionRequestResolved { verdict } => Some(verdict),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(verdicts.len(), 1, "exactly one verdict: {verdicts:?}");
        verdicts[0]
    }

    /// Every phase answers a compaction request, and only a serving
    /// registration (idle or attached here; a running one in the
    /// integration rows) records it, once.
    #[test]
    fn a_request_is_recorded_only_by_a_serving_registration_and_only_once() {
        for phase in [dsl::MeerkatPhase::Idle, dsl::MeerkatPhase::Attached] {
            let mut authority = authority_in(phase, false);
            assert_eq!(
                request(&mut authority),
                dsl::CompactionRequestVerdict::Requested,
                "{phase:?}"
            );
            assert!(authority.state().compaction_requested, "{phase:?}");
            let recorded = authority.state().clone();
            assert_eq!(
                request(&mut authority),
                dsl::CompactionRequestVerdict::AlreadyRequested,
                "{phase:?}"
            );
            assert_eq!(
                authority.state(),
                &recorded,
                "{phase:?}: a repeat changes nothing"
            );
        }
        for phase in [
            dsl::MeerkatPhase::Initializing,
            dsl::MeerkatPhase::Retired,
            dsl::MeerkatPhase::Stopped,
            dsl::MeerkatPhase::Destroyed,
        ] {
            for held in [false, true] {
                let mut authority = authority_in(phase, held);
                let before = authority.state().clone();
                assert_eq!(
                    request(&mut authority),
                    dsl::CompactionRequestVerdict::NotServing,
                    "{phase:?}"
                );
                assert_eq!(
                    authority.state(),
                    &before,
                    "{phase:?}: a refused request records nothing"
                );
            }
        }
    }

    /// No phase without a current run consumes a held request: the
    /// consumption is answered, and the request waits for a run's check.
    #[test]
    fn a_registration_without_a_current_run_keeps_a_held_request() {
        for phase in [
            dsl::MeerkatPhase::Initializing,
            dsl::MeerkatPhase::Idle,
            dsl::MeerkatPhase::Attached,
            dsl::MeerkatPhase::Stopped,
            dsl::MeerkatPhase::Destroyed,
        ] {
            let mut authority = authority_in(phase, true);
            let before = authority.state().clone();
            let transition = dsl::MeerkatMachineMutator::apply(
                &mut authority,
                dsl::MeerkatMachineInput::ConsumeCompactionRequest {
                    run_id: dsl::RunId("compaction-request-run".to_string()),
                },
            )
            .expect("ConsumeCompactionRequest is total over every phase");
            assert!(
                transition.into_effects().is_empty(),
                "{phase:?}: nothing is consumed"
            );
            assert_eq!(authority.state(), &before, "{phase:?}: the request stays");
        }
    }
}

use super::*;

/// Effects produced by an actual MeerkatMachine DSL transition.
///
/// The constructor is private to this module so runtime shell code cannot wrap
/// hand-authored generated effect payloads and turn them into executable
/// runtime-loop effects.
#[derive(Debug, Clone)]
pub(crate) struct DslTransitionEffects {
    effects: Vec<dsl::MeerkatMachineEffect>,
}

impl DslTransitionEffects {
    fn new(effects: Vec<dsl::MeerkatMachineEffect>) -> Self {
        Self { effects }
    }

    pub(crate) fn as_slice(&self) -> &[dsl::MeerkatMachineEffect] {
        &self.effects
    }
}

impl std::ops::Deref for DslTransitionEffects {
    type Target = [dsl::MeerkatMachineEffect];

    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl MeerkatMachine {
    pub(super) async fn stage_session_dsl_input(
        &self,
        session_id: &SessionId,
        input: dsl::MeerkatMachineInput,
        context: &str,
    ) -> Result<Box<dsl::MeerkatMachineState>, String> {
        self.stage_session_dsl_transition(session_id, input, context)
            .await
            .map(|staged| staged.previous_state)
    }

    pub(super) async fn stage_session_dsl_transition(
        &self,
        session_id: &SessionId,
        input: dsl::MeerkatMachineInput,
        context: &str,
    ) -> Result<StagedSessionDslInput, String> {
        let authority = self.session_dsl_authority(session_id).await?;
        Self::stage_dsl_transition_on_authority(&authority, input, context)
    }

    pub(super) fn stage_dsl_transition_on_authority(
        authority: &crate::driver::ephemeral::SharedIngressDslAuthority,
        input: dsl::MeerkatMachineInput,
        context: &str,
    ) -> Result<StagedSessionDslInput, String> {
        let mut authority = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous_state = Box::new(authority.state.clone());
        let effects = dsl::MeerkatMachineMutator::apply(&mut *authority, input)
            .map(|transition| DslTransitionEffects::new(transition.effects))
            .map_err(|err| dsl_authority::map_error(err, context))?;
        Ok(StagedSessionDslInput {
            previous_state,
            effects,
        })
    }

    pub(super) async fn apply_session_dsl_input(
        &self,
        session_id: &SessionId,
        input: dsl::MeerkatMachineInput,
        context: &str,
    ) -> Result<(Box<dsl::MeerkatMachineState>, DslTransitionEffects), String> {
        self.apply_session_dsl_input_with_dispatch_failure(
            session_id,
            input,
            context,
            CommittedEffectDispatchFailure::RestorePreviousDslState,
        )
        .await
    }

    pub(super) async fn apply_session_dsl_input_with_dispatch_failure(
        &self,
        session_id: &SessionId,
        input: dsl::MeerkatMachineInput,
        context: &str,
        dispatch_failure: CommittedEffectDispatchFailure,
    ) -> Result<(Box<dsl::MeerkatMachineState>, DslTransitionEffects), String> {
        let authority = self.session_dsl_authority(session_id).await?;
        let (previous_state, effects) = {
            let mut authority = authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let previous_state = Box::new(authority.state.clone());
            let effects = dsl::MeerkatMachineMutator::apply(&mut *authority, input)
                .map(|transition| DslTransitionEffects::new(transition.effects))
                .map_err(|err| dsl_authority::map_error(err, context))?;
            (previous_state, effects)
        };
        if let Err(error) = self.dispatch_routed_signals_from_effects(&effects).await {
            match dispatch_failure {
                CommittedEffectDispatchFailure::PreserveCommittedDslState => {}
                CommittedEffectDispatchFailure::RestorePreviousDslState => {
                    self.restore_session_dsl_state(session_id, previous_state)
                        .await;
                }
            }
            return Err(format!(
                "DSL authority ({context}): committed effect dispatch failed: {error}"
            ));
        }
        Ok((previous_state, effects))
    }
}

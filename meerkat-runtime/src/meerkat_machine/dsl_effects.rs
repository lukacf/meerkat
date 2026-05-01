use super::*;
use crate::meerkat_machine_types::{
    MeerkatMachineFieldlessRuntimeInternalInput,
    canonical_meerkat_machine_runtime_internal_fieldless_input_variant_manifest,
    canonical_meerkat_machine_runtime_internal_input_variant_manifest,
};

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

pub(crate) fn apply_dsl_transition_on_authority(
    authority: &crate::driver::ephemeral::SharedIngressDslAuthority,
    input: dsl::MeerkatMachineInput,
    context: &str,
) -> Result<DslTransitionEffects, String> {
    MeerkatMachine::reject_raw_fieldless_runtime_internal_dsl_input(&input)?;
    let mut authority = authority
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    dsl::MeerkatMachineMutator::apply(&mut *authority, input)
        .map(|transition| DslTransitionEffects::new(transition.effects))
        .map_err(|err| dsl_authority::map_error(err, context))
}

impl std::ops::Deref for DslTransitionEffects {
    type Target = [dsl::MeerkatMachineEffect];

    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl MeerkatMachine {
    pub(super) async fn stage_session_runtime_internal_dsl_input(
        &self,
        session_id: &SessionId,
        input: MeerkatMachineFieldlessRuntimeInternalInput,
    ) -> Result<Box<dsl::MeerkatMachineState>, String> {
        self.stage_session_runtime_internal_dsl_transition(session_id, input)
            .await
            .map(|staged| staged.previous_state)
    }

    pub(super) async fn stage_session_runtime_internal_dsl_transition(
        &self,
        session_id: &SessionId,
        input: MeerkatMachineFieldlessRuntimeInternalInput,
    ) -> Result<StagedSessionDslInput, String> {
        let authority = self.session_dsl_authority(session_id).await?;
        Self::stage_runtime_internal_dsl_transition_on_authority(&authority, input)
    }

    pub(super) fn stage_runtime_internal_dsl_transition_on_authority(
        authority: &crate::driver::ephemeral::SharedIngressDslAuthority,
        input: MeerkatMachineFieldlessRuntimeInternalInput,
    ) -> Result<StagedSessionDslInput, String> {
        let variant = input.input_variant();
        if !canonical_meerkat_machine_runtime_internal_input_variant_manifest().contains(&variant) {
            return Err(format!(
                "runtime-internal input {variant:?} is absent from the typed production manifest"
            ));
        }
        if !canonical_meerkat_machine_runtime_internal_fieldless_input_variant_manifest()
            .contains(&variant)
        {
            return Err(format!(
                "runtime-internal input {variant:?} is absent from the typed fieldless manifest"
            ));
        }
        if !input.requires_typed_runtime_internal_stager() {
            return Err(format!(
                "fieldless runtime-internal input {variant:?} is owned by {:?}, not the typed runtime-internal stager",
                input.authority()
            ));
        }
        Self::stage_dsl_transition_on_authority_after_typed_gate(
            authority,
            input.dsl_input(),
            variant.as_str(),
        )
    }

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
        Self::reject_raw_fieldless_runtime_internal_dsl_input(&input)?;
        Self::stage_dsl_transition_on_authority_after_typed_gate(authority, input, context)
    }

    fn stage_dsl_transition_on_authority_after_typed_gate(
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

    pub(super) fn reject_raw_fieldless_runtime_internal_dsl_input(
        input: &dsl::MeerkatMachineInput,
    ) -> Result<(), String> {
        MeerkatMachineFieldlessRuntimeInternalInput::reject_raw_dsl_input(input)
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
        Self::reject_raw_fieldless_runtime_internal_dsl_input(&input)?;
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

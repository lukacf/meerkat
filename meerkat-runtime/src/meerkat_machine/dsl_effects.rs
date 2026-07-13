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
        .map(|transition| DslTransitionEffects::new(transition.into_effects()))
        .map_err(|err| dsl_authority::map_error(err, context))
}

impl std::ops::Deref for DslTransitionEffects {
    type Target = [dsl::MeerkatMachineEffect];

    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl MeerkatMachine {
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

    /// Stage a fieldless transition on behalf of the runtime owner itself.
    /// Runtime-owner observations are intentionally excluded from the generic
    /// typed-internal stager so only the attachment owner can publish them.
    pub(super) fn stage_runtime_owner_dsl_transition_on_authority(
        authority: &crate::driver::ephemeral::SharedIngressDslAuthority,
        input: MeerkatMachineFieldlessRuntimeInternalInput,
    ) -> Result<StagedSessionDslInput, String> {
        let mut authority = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Self::stage_runtime_owner_dsl_transition_on_locked_authority(&mut authority, input)
    }

    pub(super) fn stage_runtime_owner_dsl_transition_on_locked_authority(
        authority: &mut dsl::MeerkatMachineAuthority,
        input: MeerkatMachineFieldlessRuntimeInternalInput,
    ) -> Result<StagedSessionDslInput, String> {
        let variant = input.input_variant();
        if !canonical_meerkat_machine_runtime_internal_input_variant_manifest().contains(&variant) {
            return Err(format!(
                "runtime-owner input {variant:?} is absent from the typed production manifest"
            ));
        }
        if !canonical_meerkat_machine_runtime_internal_fieldless_input_variant_manifest()
            .contains(&variant)
        {
            return Err(format!(
                "runtime-owner input {variant:?} is absent from the typed fieldless manifest"
            ));
        }
        if input.authority()
            != crate::meerkat_machine_types::MeerkatMachineFieldlessRuntimeInternalAuthority::RuntimeOwner
        {
            return Err(format!(
                "fieldless runtime-internal input {variant:?} is owned by {:?}, not RuntimeOwner",
                input.authority()
            ));
        }
        Self::stage_dsl_transition_on_locked_authority_after_typed_gate(
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
    ) -> Result<dsl::MeerkatMachineAuthoritySnapshot, String> {
        self.stage_session_dsl_transition(session_id, input, context)
            .await
            .map(|staged| staged.previous_snapshot)
    }

    pub(super) async fn stage_session_dsl_transition(
        &self,
        session_id: &SessionId,
        input: dsl::MeerkatMachineInput,
        context: &str,
    ) -> Result<StagedSessionDslInput, String> {
        let sessions = self.sessions.read().await;
        let entry = sessions.get(session_id).ok_or_else(|| {
            RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            }
            .to_string()
        })?;
        if let Some(error) = entry.dsl_mutation_blocked_by_unregister(session_id) {
            return Err(error.to_string());
        }
        Self::stage_dsl_transition_on_authority(&entry.dsl_authority, input, context)
    }

    pub(super) fn stage_dsl_transition_on_authority(
        authority: &crate::driver::ephemeral::SharedIngressDslAuthority,
        input: dsl::MeerkatMachineInput,
        context: &str,
    ) -> Result<StagedSessionDslInput, String> {
        Self::reject_raw_fieldless_runtime_internal_dsl_input(&input)?;
        Self::stage_dsl_transition_on_authority_after_typed_gate(authority, input, context)
    }

    /// Stage a transition while the caller retains the shared authority's
    /// synchronous mutex. This is reserved for no-await publication handoffs
    /// that must prevent session-owned handles from interleaving between a
    /// generated claim and its exact shell attachment.
    pub(super) fn stage_dsl_transition_on_locked_authority(
        authority: &mut dsl::MeerkatMachineAuthority,
        input: dsl::MeerkatMachineInput,
        context: &str,
    ) -> Result<StagedSessionDslInput, String> {
        Self::reject_raw_fieldless_runtime_internal_dsl_input(&input)?;
        Self::stage_dsl_transition_on_locked_authority_after_typed_gate(authority, input, context)
    }

    fn stage_dsl_transition_on_authority_after_typed_gate(
        authority: &crate::driver::ephemeral::SharedIngressDslAuthority,
        input: dsl::MeerkatMachineInput,
        context: &str,
    ) -> Result<StagedSessionDslInput, String> {
        let mut authority = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Self::stage_dsl_transition_on_locked_authority_after_typed_gate(
            &mut authority,
            input,
            context,
        )
    }

    fn stage_dsl_transition_on_locked_authority_after_typed_gate(
        authority: &mut dsl::MeerkatMachineAuthority,
        input: dsl::MeerkatMachineInput,
        context: &str,
    ) -> Result<StagedSessionDslInput, String> {
        let previous_snapshot = authority.snapshot();
        let effects = dsl::MeerkatMachineMutator::apply(authority, input)
            .map(|transition| DslTransitionEffects::new(transition.into_effects()))
            .map_err(|err| dsl_authority::map_error(err, context))?;
        let committed_snapshot = authority.snapshot();
        Ok(StagedSessionDslInput {
            previous_snapshot,
            committed_snapshot,
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
    ) -> Result<(dsl::MeerkatMachineAuthoritySnapshot, DslTransitionEffects), String> {
        self.apply_session_dsl_input_with_dispatch_failure(
            session_id,
            input,
            context,
            CommittedEffectDispatchFailure::PreserveCommittedDslState,
        )
        .await
    }

    pub(super) async fn apply_session_dsl_input_with_dispatch_failure(
        &self,
        session_id: &SessionId,
        input: dsl::MeerkatMachineInput,
        context: &str,
        dispatch_failure: CommittedEffectDispatchFailure,
    ) -> Result<(dsl::MeerkatMachineAuthoritySnapshot, DslTransitionEffects), String> {
        Self::reject_raw_fieldless_runtime_internal_dsl_input(&input)?;
        let sessions = self.sessions.read().await;
        let entry = sessions.get(session_id).ok_or_else(|| {
            RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            }
            .to_string()
        })?;
        if let Some(error) = entry.dsl_mutation_blocked_by_unregister(session_id) {
            return Err(error.to_string());
        }
        let (previous_snapshot, effects) = {
            let mut authority = entry
                .dsl_authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let previous_snapshot = authority.snapshot();
            let effects = dsl::MeerkatMachineMutator::apply(&mut *authority, input)
                .map(|transition| DslTransitionEffects::new(transition.into_effects()))
                .map_err(|err| dsl_authority::map_error(err, context))?;
            (previous_snapshot, effects)
        };
        drop(sessions);
        if let Err(error) = self.dispatch_routed_signals_from_effects(&effects).await {
            let CommittedEffectDispatchFailure::PreserveCommittedDslState = dispatch_failure;
            return Err(format!(
                "DSL authority ({context}): committed effect dispatch failed: {error}"
            ));
        }
        Ok((previous_snapshot, effects))
    }

    /// Typed-refusal variant of [`Self::apply_session_dsl_input_with_dispatch_failure`]
    /// for the formal-composition routed-input seam: every rejection leg keeps
    /// its stable typed discriminant (per-variant for generated-machine
    /// rejections) instead of collapsing into a bare string the consumer
    /// surface would re-wrap under one generic code.
    pub(super) async fn apply_routed_session_dsl_input(
        &self,
        session_id: &SessionId,
        input: dsl::MeerkatMachineInput,
        context: &str,
    ) -> Result<
        (dsl::MeerkatMachineAuthoritySnapshot, DslTransitionEffects),
        dsl_authority::DslTransitionRefusal,
    > {
        if let Err(reason) = Self::reject_raw_fieldless_runtime_internal_dsl_input(&input) {
            return Err(dsl_authority::DslTransitionRefusal::other(
                "routed_raw_internal_input_rejected",
                reason,
            ));
        }
        let sessions = self.sessions.read().await;
        let entry = sessions.get(session_id).ok_or_else(|| {
            dsl_authority::DslTransitionRefusal::other(
                "session_authority_unavailable",
                RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                }
                .to_string(),
            )
        })?;
        if let Some(error) = entry.dsl_mutation_blocked_by_unregister(session_id) {
            return Err(dsl_authority::DslTransitionRefusal::other(
                "unregister_finalization_pending",
                error.to_string(),
            ));
        }
        let (previous_snapshot, effects) = {
            let mut authority = entry
                .dsl_authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let previous_snapshot = authority.snapshot();
            let effects = dsl::MeerkatMachineMutator::apply(&mut *authority, input)
                .map(|transition| DslTransitionEffects::new(transition.into_effects()))
                .map_err(|err| dsl_authority::refusal(err, context))?;
            (previous_snapshot, effects)
        };
        drop(sessions);
        if let Err(error) = self.dispatch_routed_signals_from_effects(&effects).await {
            // CommittedEffectDispatchFailure::PreserveCommittedDslState
            // semantics: the committed DSL state is preserved; only the
            // dispatch fault is surfaced (typed).
            return Err(dsl_authority::DslTransitionRefusal::other(
                "committed_effect_dispatch_failed",
                format!("DSL authority ({context}): committed effect dispatch failed: {error}"),
            ));
        }
        Ok((previous_snapshot, effects))
    }
}

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

/// A machine-owned refusal of a staged session registration.
///
/// `SessionRegistrationRejected` is a surface-result-alignment effect: the
/// generated machine ACCEPTS the input (no state change) and returns this
/// verdict, so a caller that staged a registration must convert it into a typed
/// error. Treating the accepted transition as success would make a registration
/// against a replaced entry look like an idempotent no-op.
#[derive(Debug, Clone)]
pub(crate) struct SessionRegistrationRefusal {
    reason: dsl::SessionRegistrationRejectReasonKind,
    registered_runtime_epoch_id: Option<dsl::RuntimeEpochId>,
    attempted_runtime_epoch_id: Option<dsl::RuntimeEpochId>,
    unregister_runtime_loop_drain_pending: bool,
    unregister_comms_drain_exit_pending: bool,
    unregister_completion_waiter_drain_pending: bool,
}

impl SessionRegistrationRefusal {
    /// Convert the machine's verdict into the caller-facing typed error.
    ///
    /// The reason kind decides the error, not the call site: an epoch conflict
    /// means the entry was replaced underneath the caller and retrying from the
    /// same in-memory state is forbidden (`StaleAuthority`), while a teardown
    /// refusal is RETRYABLE and must map to `UnregisterInProgress`, whose
    /// contract is "join the same saga". Collapsing the second into the first
    /// would tell the host its session is permanently unusable because cleanup
    /// had not finished.
    pub(crate) fn into_runtime_driver_error(
        self,
        session_id: &SessionId,
    ) -> crate::RuntimeDriverError {
        match self.reason {
            dsl::SessionRegistrationRejectReasonKind::UnregisterTeardownInProgress => {
                crate::RuntimeDriverError::UnregisterInProgress {
                    runtime_id: LogicalRuntimeId::for_session(session_id),
                }
            }
            dsl::SessionRegistrationRejectReasonKind::RuntimeEpochConflict => {
                crate::RuntimeDriverError::StaleAuthority {
                    reason: format!("session {session_id}: {self}"),
                }
            }
        }
    }
}

impl std::fmt::Display for SessionRegistrationRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let format_epoch = |epoch: &Option<dsl::RuntimeEpochId>| match epoch {
            Some(epoch) => epoch.0.clone(),
            None => "none".to_string(),
        };
        match self.reason {
            dsl::SessionRegistrationRejectReasonKind::RuntimeEpochConflict => write!(
                f,
                "session registration refused: runtime epoch conflict (registered {}, attempted {})",
                format_epoch(&self.registered_runtime_epoch_id),
                format_epoch(&self.attempted_runtime_epoch_id)
            ),
            dsl::SessionRegistrationRejectReasonKind::UnregisterTeardownInProgress => {
                let mut open: Vec<&'static str> = Vec::new();
                if self.unregister_runtime_loop_drain_pending {
                    open.push("runtime loop");
                }
                if self.unregister_comms_drain_exit_pending {
                    open.push("comms drain");
                }
                if self.unregister_completion_waiter_drain_pending {
                    open.push("completion waiters");
                }
                let open = if open.is_empty() {
                    "none (awaiting final unregister commit)".to_string()
                } else {
                    open.join(", ")
                };
                write!(
                    f,
                    "session registration refused: unregister teardown still draining \
                     (epoch {}, still open: {open}); retry once teardown concludes",
                    format_epoch(&self.registered_runtime_epoch_id)
                )
            }
        }
    }
}

impl DslTransitionEffects {
    fn new(effects: Vec<dsl::MeerkatMachineEffect>) -> Self {
        Self { effects }
    }

    pub(crate) fn as_slice(&self) -> &[dsl::MeerkatMachineEffect] {
        &self.effects
    }

    /// The machine's registration refusal verdict, if this transition emitted
    /// one.
    pub(crate) fn session_registration_refusal(&self) -> Option<SessionRegistrationRefusal> {
        self.effects.iter().find_map(|effect| match effect {
            dsl::MeerkatMachineEffect::SessionRegistrationRejected {
                reason,
                registered_runtime_epoch_id,
                attempted_runtime_epoch_id,
                unregister_runtime_loop_drain_pending,
                unregister_comms_drain_exit_pending,
                unregister_completion_waiter_drain_pending,
                ..
            } => Some(SessionRegistrationRefusal {
                reason: *reason,
                registered_runtime_epoch_id: registered_runtime_epoch_id.clone(),
                attempted_runtime_epoch_id: attempted_runtime_epoch_id.clone(),
                unregister_runtime_loop_drain_pending: *unregister_runtime_loop_drain_pending,
                unregister_comms_drain_exit_pending: *unregister_comms_drain_exit_pending,
                unregister_completion_waiter_drain_pending:
                    *unregister_completion_waiter_drain_pending,
            }),
            _ => None,
        })
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

    /// Supply the truthful entry runtime epoch to a routed input that declares
    /// the field but cannot carry it.
    ///
    /// The formal composition route projects MobMachine-owned facts only, and
    /// the runtime epoch is a meerkat-owned fact about a meerkat-owned session
    /// entry - the producer machine must never learn it. Resolving it here, from
    /// the ENTRY (never from the authority's own state, which would make the
    /// receiving guard tautological), under the same `sessions` guard that
    /// resolved the entry and with the caller's session mutation gate held,
    /// keeps the value atomic with the transition and exactly symmetric with the
    /// authoritative writer. An input that already states an epoch is left
    /// alone: the machine then asserts it, so a stale-epoch writer is still
    /// refused.
    fn resolve_routed_entry_runtime_epoch(
        input: &mut dsl::MeerkatMachineInput,
        entry_epoch_id: &meerkat_core::RuntimeEpochId,
    ) {
        let declared_epoch = match input {
            dsl::MeerkatMachineInput::PrepareBindings {
                runtime_epoch_id, ..
            }
            | dsl::MeerkatMachineInput::Ingest {
                runtime_epoch_id, ..
            } => runtime_epoch_id,
            _ => return,
        };
        if declared_epoch.is_none() {
            *declared_epoch = Some(dsl::RuntimeEpochId::from_domain(entry_epoch_id));
        }
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

    /// Typed variant for recovery-owned callers that must distinguish a
    /// temporarily unavailable session authority from a semantic refusal.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(super) async fn apply_session_dsl_input_typed(
        &self,
        session_id: &SessionId,
        input: dsl::MeerkatMachineInput,
        context: &str,
    ) -> Result<(dsl::MeerkatMachineAuthoritySnapshot, DslTransitionEffects), RuntimeDriverError>
    {
        Self::reject_raw_fieldless_runtime_internal_dsl_input(&input)
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
        let sessions = self.sessions.read().await;
        let entry = sessions
            .get(session_id)
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;
        if let Some(error) = entry.dsl_mutation_blocked_by_unregister(session_id) {
            return Err(error);
        }
        let (previous_snapshot, effects) = {
            let mut authority = entry
                .dsl_authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let previous_snapshot = authority.snapshot();
            let effects = dsl::MeerkatMachineMutator::apply(&mut *authority, input)
                .map(|transition| DslTransitionEffects::new(transition.into_effects()))
                .map_err(|error| RuntimeDriverError::ValidationFailed {
                    reason: dsl_authority::map_error(error, context),
                })?;
            (previous_snapshot, effects)
        };
        drop(sessions);
        // Terminal recording currently emits a local-only authority receipt.
        // This narrow test fault exercises the real typed post-commit error
        // boundary without inventing a composition route for that effect.
        #[cfg(test)]
        if self
            .test_fail_next_typed_dsl_post_commit_dispatch
            .swap(false, std::sync::atomic::Ordering::AcqRel)
        {
            return Err(RuntimeDriverError::RecoveryBackoff {
                reason: format!(
                    "DSL authority ({context}) committed but routed effect dispatch needs recovery: injected post-commit dispatch failure"
                ),
            });
        }
        self.dispatch_routed_signals_from_effects(&effects)
            .await
            .map_err(|reason| RuntimeDriverError::RecoveryBackoff {
                reason: format!(
                    "DSL authority ({context}) committed but routed effect dispatch needs recovery: {reason}"
                ),
            })?;
        Ok((previous_snapshot, effects))
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
    ///
    /// This lane stays single-shot where the authoritative placement writer is
    /// two-phase (stage, sync the driver control projection, commit). The
    /// authoritative writer is handed the `SharedDriver` whose projection it
    /// owns; this lane's caller is the seam consumer surface, which holds no
    /// driver handle, so a routed apply has no projection to own and reaching
    /// for the driver would introduce a `sessions`-then-driver lock edge no
    /// production path takes. What a routed transition does own is exact at
    /// return: the session's DSL authority, which is what work admission and
    /// the compaction projection coordinator read.
    pub(super) async fn apply_routed_session_dsl_input(
        &self,
        session_id: &SessionId,
        mut input: dsl::MeerkatMachineInput,
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
        Self::resolve_routed_entry_runtime_epoch(&mut input, &entry.epoch_id);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// Every `meerkat_mob_seam` route into the meerkat consumer whose target
    /// input variant declares `runtime_epoch_id`.
    ///
    /// No seam route can bind that field - MobMachine owns no epoch fact - so
    /// each name here is exactly one variant whose epoch
    /// `resolve_routed_entry_runtime_epoch` has to supply from the session
    /// entry.
    fn seam_routed_inputs_declaring_runtime_epoch() -> BTreeSet<String> {
        let seam = meerkat_machine_schema::meerkat_mob_seam_composition();
        let machine = meerkat_machine_schema::catalog::dsl::dsl_meerkat_machine();
        let consumer = crate::generated::meerkat_mob_seam::producers::meerkat_instance_id();
        seam.routes
            .iter()
            .filter(|route| route.to.machine == consumer)
            .filter_map(|route| match &route.to.input_variant {
                meerkat_machine_schema::RouteVariantId::Input(variant) => Some(variant.as_str()),
                meerkat_machine_schema::RouteVariantId::Signal(_) => None,
            })
            .filter(|variant| {
                machine.inputs.variants.iter().any(|declared| {
                    declared.name.as_str() == *variant
                        && declared
                            .fields
                            .iter()
                            .any(|field| field.name.as_str() == "runtime_epoch_id")
                })
            })
            .map(str::to_owned)
            .collect()
    }

    fn routed_runtime_epoch(
        input: &dsl::MeerkatMachineInput,
    ) -> Option<&Option<dsl::RuntimeEpochId>> {
        match input {
            dsl::MeerkatMachineInput::PrepareBindings {
                runtime_epoch_id, ..
            }
            | dsl::MeerkatMachineInput::Ingest {
                runtime_epoch_id, ..
            } => Some(runtime_epoch_id),
            _ => None,
        }
    }

    fn epochless_routed_inputs() -> Vec<(&'static str, dsl::MeerkatMachineInput)> {
        vec![
            (
                "PrepareBindings",
                dsl::MeerkatMachineInput::PrepareBindings {
                    agent_runtime_id: dsl::AgentRuntimeId("mob-runtime".into()),
                    fence_token: dsl::FenceToken(1),
                    generation: Some(dsl::Generation(0)),
                    runtime_epoch_id: None,
                    session_id: dsl::SessionId("routed-session".into()),
                },
            ),
            (
                "Ingest",
                dsl::MeerkatMachineInput::Ingest {
                    session_id: dsl::SessionId("routed-session".into()),
                    runtime_id: dsl::AgentRuntimeId("mob-runtime".into()),
                    fence_token: dsl::FenceToken(1),
                    generation: Some(dsl::Generation(0)),
                    runtime_epoch_id: None,
                    work_id: dsl::WorkId("work-1".into()),
                    origin: dsl::WorkOrigin::Ingest,
                },
            ),
        ]
    }

    /// Schema-enumerated completeness gate for the routed epoch fill (same
    /// shape as the seam's `lift_covers_every_schema_declared_meerkat_signal_route`).
    ///
    /// A routed variant that declares the epoch and is NOT filled here carries
    /// `None` into the machine, which is the 0.8.23 wedge: the placement is
    /// refused (or, before the epoch became a registration fact, silently
    /// nulled the live entry epoch) and durable compaction persistence can
    /// never authorize again.
    #[test]
    fn routed_epoch_resolution_covers_every_seam_input_declaring_the_field() {
        let declared = seam_routed_inputs_declaring_runtime_epoch();
        let covered: BTreeSet<String> = epochless_routed_inputs()
            .iter()
            .map(|(name, _)| (*name).to_owned())
            .collect();
        assert_eq!(
            covered,
            BTreeSet::from(["Ingest".to_owned(), "PrepareBindings".to_owned()]),
            "the routed epoch-bearing fixture set is the gate's own anchor and must stay explicit"
        );
        assert_eq!(
            declared, covered,
            "meerkat_mob_seam routes an input declaring `runtime_epoch_id` that this gate does \
             not cover; fill it in MeerkatMachine::resolve_routed_entry_runtime_epoch \
             (meerkat-runtime/src/meerkat_machine/dsl_effects.rs) and add it here"
        );

        let entry_epoch_id = meerkat_core::RuntimeEpochId::new();
        let expected = Some(dsl::RuntimeEpochId::from_domain(&entry_epoch_id));
        for (name, mut input) in epochless_routed_inputs() {
            MeerkatMachine::resolve_routed_entry_runtime_epoch(&mut input, &entry_epoch_id);
            assert_eq!(
                routed_runtime_epoch(&input),
                Some(&expected),
                "routed `{name}` must carry the entry epoch after resolution"
            );
        }
    }

    /// Fill-only, never override: the exact-match guard exists to fence a
    /// stale-epoch writer, and a resolver that rewrote every stated value
    /// would make that guard compare the entry epoch against itself.
    #[test]
    fn routed_epoch_resolution_leaves_a_stated_epoch_alone() {
        let stated = Some(dsl::RuntimeEpochId("stated-epoch".into()));
        let entry_epoch_id = meerkat_core::RuntimeEpochId::new();
        for (name, mut input) in epochless_routed_inputs() {
            match &mut input {
                dsl::MeerkatMachineInput::PrepareBindings {
                    runtime_epoch_id, ..
                }
                | dsl::MeerkatMachineInput::Ingest {
                    runtime_epoch_id, ..
                } => *runtime_epoch_id = stated.clone(),
                _ => panic!("routed fixture `{name}` declares no runtime epoch field"),
            }
            MeerkatMachine::resolve_routed_entry_runtime_epoch(&mut input, &entry_epoch_id);
            assert_eq!(
                routed_runtime_epoch(&input),
                Some(&stated),
                "routed `{name}` must keep the epoch its producer stated"
            );
        }
    }
}

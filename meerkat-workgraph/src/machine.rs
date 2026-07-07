use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Duration, Utc};
use serde_json::json;

use crate::WorkGraphError;
use crate::machines::{work_attention_lifecycle as attention_dsl, workgraph_lifecycle as wg_dsl};
use crate::types::{
    AddEvidenceRequest, AttentionDelegatedAuthority, ClaimWorkItemRequest, CloseWorkItemRequest,
    CreateWorkItemRequest, PolicyEscalateRequest, ProjectedAttentionAuthority,
    ReleaseWorkItemRequest, UpdateWorkItemRequest, WorkAttentionBinding, WorkAttentionMode,
    WorkAttentionStatus, WorkClaim, WorkCompletionPolicy, WorkEdge, WorkEdgeKind, WorkGraphEvent,
    WorkGraphEventKind, WorkGraphMachineState, WorkItem, WorkItemId, WorkNamespace, WorkStatus,
};

/// Machine-owned public error classification surfaced to REST/RPC callers.
///
/// Re-exported from the canonical `WorkGraphLifecycleMachine` DSL: the machine
/// is the sole authority for the variant->class POLICY (see
/// `WorkGraphMachine::public_error_class`). Surfaces mirror the emitted class.
pub use wg_dsl::WorkGraphPublicErrorClass;

/// Machine-owned public-confirmation admission verdict surfaced to the
/// public-confirm surface.
///
/// Re-exported from the canonical `WorkGraphLifecycleMachine` DSL: the machine
/// is the sole authority for the trust-scoped eligibility (see
/// `WorkGraphMachine::classify_public_confirmation_admission`). The surface
/// mirrors the emitted verdict.
pub use wg_dsl::WorkPublicConfirmationAdmissionKind;

/// Machine-owned admission verdict for a requested mutation of a work item's
/// `completion_policy`.
///
/// Re-exported from the canonical `WorkGraphLifecycleMachine` DSL: the machine is
/// the sole authority for the invariant "a work item's completion policy is
/// fixed at creation, monotonically tightenable, and never update-writable" (see
/// `WorkGraphMachine::classify_completion_policy_mutation_admission`). The shell
/// mirrors the emitted verdict.
pub use wg_dsl::WorkCompletionPolicyMutationAdmissionKind;
pub use wg_dsl::WorkPolicyEscalationAdmissionKind;

#[derive(Debug, Default, Clone, Copy)]
pub struct WorkAttentionMachine;

impl WorkAttentionMachine {
    pub fn pause(
        mut binding: WorkAttentionBinding,
        expected_revision: u64,
        until: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) -> Result<WorkAttentionBinding, WorkGraphError> {
        let input = attention_dsl::WorkAttentionLifecycleInput::Pause {
            expected_revision,
            until_utc_ms: until.map(datetime_to_millis),
        };
        binding.machine_state = apply_attention_dsl(&binding, input, Some(expected_revision))?;
        sync_attention_from_machine_state(&mut binding);
        binding.updated_at = now;
        Ok(binding)
    }

    pub fn resume(
        mut binding: WorkAttentionBinding,
        expected_revision: u64,
        now: DateTime<Utc>,
    ) -> Result<WorkAttentionBinding, WorkGraphError> {
        let input = attention_dsl::WorkAttentionLifecycleInput::Resume { expected_revision };
        binding.machine_state = apply_attention_dsl(&binding, input, Some(expected_revision))?;
        sync_attention_from_machine_state(&mut binding);
        binding.updated_at = now;
        Ok(binding)
    }

    pub fn stop(
        mut binding: WorkAttentionBinding,
        expected_revision: u64,
        now: DateTime<Utc>,
    ) -> Result<WorkAttentionBinding, WorkGraphError> {
        let input = attention_dsl::WorkAttentionLifecycleInput::Stop {
            expected_revision,
            at_utc_ms: datetime_to_millis(now),
        };
        binding.machine_state = apply_attention_dsl(&binding, input, Some(expected_revision))?;
        sync_attention_from_machine_state(&mut binding);
        binding.updated_at = now;
        Ok(binding)
    }

    pub fn supersede(
        mut binding: WorkAttentionBinding,
        expected_revision: u64,
        superseded_by_binding_id: &crate::types::WorkAttentionBindingId,
        now: DateTime<Utc>,
    ) -> Result<WorkAttentionBinding, WorkGraphError> {
        let input = attention_dsl::WorkAttentionLifecycleInput::Supersede {
            expected_revision,
            superseded_by_binding_key: attention_dsl::WorkAttentionBindingKey(
                superseded_by_binding_id.as_str().to_string(),
            ),
            at_utc_ms: datetime_to_millis(now),
        };
        binding.machine_state = apply_attention_dsl(&binding, input, Some(expected_revision))?;
        sync_attention_from_machine_state(&mut binding);
        binding.updated_at = now;
        Ok(binding)
    }

    /// Resolve attention-projection eligibility for the binding at `now`.
    ///
    /// The shell extracts only the raw wall-clock `now` (a pure observation) and
    /// drives the canonical `WorkAttentionLifecycleMachine`'s
    /// `ClassifyAttentionEligibility` input over the recovered binding state. The
    /// machine owns the eligibility POLICY — including the Paused deadline-elapsed
    /// rule (`paused_until <= now`) — and emits the verdict; this function only
    /// mirrors the emitted `AttentionEligibilityClassified.eligible`. It fails
    /// closed (returns `Err`) if the machine refuses to classify or emits no
    /// verdict; it decides nothing.
    pub fn classify_eligibility_at(
        binding: &WorkAttentionBinding,
        now: DateTime<Utc>,
    ) -> Result<bool, WorkGraphError> {
        let mut dsl_auth =
            attention_dsl::WorkAttentionLifecycleMachineAuthority::recover_from_state(
                binding.machine_state.clone(),
            )
            .map_err(|error| {
                WorkGraphError::InvalidTransition(format!(
                    "attention binding {} refused eligibility recovery: {error:?}",
                    binding.binding_id
                ))
            })?;
        let transition = attention_dsl::WorkAttentionLifecycleMachineMutator::apply(
            &mut dsl_auth,
            attention_dsl::WorkAttentionLifecycleInput::ClassifyAttentionEligibility {
                now_utc_ms: datetime_to_millis(now),
            },
        )
        .map_err(|error| {
            WorkGraphError::InvalidTransition(format!(
                "attention binding {} refused eligibility classification: {error:?}",
                binding.binding_id
            ))
        })?;

        let mut classified = None;
        for effect in transition.effects() {
            if let attention_dsl::WorkAttentionLifecycleEffect::AttentionEligibilityClassified {
                eligible,
            } = effect
                && classified.replace(*eligible).is_some()
            {
                return Err(WorkGraphError::Store(format!(
                    "attention binding {} emitted multiple eligibility verdicts",
                    binding.binding_id
                )));
            }
        }

        classified.ok_or_else(|| {
            WorkGraphError::Store(format!(
                "attention binding {} emitted no eligibility verdict",
                binding.binding_id
            ))
        })
    }

    /// Resolve the projected attention authority for the binding.
    ///
    /// The shell extracts only the raw binding facts (`mode`,
    /// `delegated_authority`) and drives the canonical
    /// `WorkAttentionLifecycleMachine`'s `ClassifyAttentionAuthority` input over
    /// the recovered binding state. The machine owns the COMPLETE per-stance
    /// tool-admission POLICY (which stances may read, add evidence, release,
    /// update, block, create, link, close their own review item, or
    /// close-if-policy-allows) and emits the capability verdict; this function
    /// mirrors the emitted `AttentionAuthorityClassified` capability bits. Fails
    /// closed.
    pub fn classify_authority(
        binding: &WorkAttentionBinding,
    ) -> Result<ProjectedAttentionAuthority, WorkGraphError> {
        let mode = attention_mode_to_dsl(binding.mode);
        let delegated_authority = attention_delegated_authority_to_dsl(binding.delegated_authority);
        let mut dsl_auth =
            attention_dsl::WorkAttentionLifecycleMachineAuthority::recover_from_state(
                binding.machine_state.clone(),
            )
            .map_err(|error| {
                WorkGraphError::InvalidTransition(format!(
                    "attention binding {} refused authority recovery: {error:?}",
                    binding.binding_id
                ))
            })?;
        let transition = attention_dsl::WorkAttentionLifecycleMachineMutator::apply(
            &mut dsl_auth,
            attention_dsl::WorkAttentionLifecycleInput::ClassifyAttentionAuthority {
                mode,
                delegated_authority,
            },
        )
        .map_err(|error| {
            WorkGraphError::InvalidTransition(format!(
                "attention binding {} refused authority classification: {error:?}",
                binding.binding_id
            ))
        })?;

        let mut classified = None;
        for effect in transition.effects() {
            if let attention_dsl::WorkAttentionLifecycleEffect::AttentionAuthorityClassified {
                can_get,
                can_add_evidence,
                can_release,
                can_update,
                can_block,
                can_create,
                can_link,
                can_link_parent,
                can_link_related,
                can_link_derived_from,
                can_close_own_review_item,
                can_close_if_policy_allows,
            } = effect
            {
                let verdict = ProjectedAttentionAuthority {
                    can_get: *can_get,
                    can_add_evidence: *can_add_evidence,
                    can_release: *can_release,
                    can_update: *can_update,
                    can_block: *can_block,
                    can_create: *can_create,
                    can_link: *can_link,
                    can_link_parent: *can_link_parent,
                    can_link_related: *can_link_related,
                    can_link_derived_from: *can_link_derived_from,
                    can_close_own_review_item: *can_close_own_review_item,
                    can_close_if_policy_allows: *can_close_if_policy_allows,
                };
                if classified.replace(verdict).is_some() {
                    return Err(WorkGraphError::Store(format!(
                        "attention binding {} emitted multiple authority verdicts",
                        binding.binding_id
                    )));
                }
            }
        }

        classified.ok_or_else(|| {
            WorkGraphError::Store(format!(
                "attention binding {} emitted no authority verdict",
                binding.binding_id
            ))
        })
    }
}

fn attention_mode_to_dsl(mode: WorkAttentionMode) -> attention_dsl::WorkAttentionMode {
    match mode {
        WorkAttentionMode::Pursue => attention_dsl::WorkAttentionMode::Pursue,
        WorkAttentionMode::Coordinate => attention_dsl::WorkAttentionMode::Coordinate,
        WorkAttentionMode::Review => attention_dsl::WorkAttentionMode::Review,
        WorkAttentionMode::Falsify => attention_dsl::WorkAttentionMode::Falsify,
        WorkAttentionMode::Judge => attention_dsl::WorkAttentionMode::Judge,
        WorkAttentionMode::Observe => attention_dsl::WorkAttentionMode::Observe,
    }
}

fn attention_delegated_authority_to_dsl(
    authority: AttentionDelegatedAuthority,
) -> attention_dsl::AttentionDelegatedAuthority {
    match authority {
        AttentionDelegatedAuthority::AddEvidence => {
            attention_dsl::AttentionDelegatedAuthority::AddEvidence
        }
        AttentionDelegatedAuthority::CloseOwnReviewItem => {
            attention_dsl::AttentionDelegatedAuthority::CloseOwnReviewItem
        }
        AttentionDelegatedAuthority::RequestClosure => {
            attention_dsl::AttentionDelegatedAuthority::RequestClosure
        }
        AttentionDelegatedAuthority::CloseIfPolicyAllows => {
            attention_dsl::AttentionDelegatedAuthority::CloseIfPolicyAllows
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct WorkGraphMachine;

impl WorkGraphMachine {
    /// Mechanically assert that a `WorkItem`'s projected lifecycle fields agree
    /// with its machine-owned `machine_state` authority.
    ///
    /// This is a *pure structural check*: it borrows `item` immutably and can
    /// only return `Ok(())` (every projection matches the machine state) or an
    /// `Err` rejecting the drift. It never synthesizes, repairs, or derives any
    /// lifecycle/revision field — the canonical truth lives in
    /// `WorkGraphMachineState`, and this guard merely rejects projections that
    /// disagree with it.
    pub fn validate_item_projection(item: &WorkItem) -> Result<(), WorkGraphError> {
        validate_item_machine_projection(item)
    }

    /// Resolve the public error class for a `WorkGraphError`.
    ///
    /// The shell performs only a pure typed extraction of the error variant
    /// into a `WorkGraphErrorKind` discriminant (one kind per variant, no
    /// grouping). The variant->class POLICY is owned by the canonical
    /// `WorkGraphLifecycleMachine`: this drives the machine's
    /// `ClassifyWorkGraphPublicError` input and mirrors the emitted
    /// `WorkGraphPublicErrorClassified` effect. The shell decides nothing.
    pub fn public_error_class(
        error: &WorkGraphError,
    ) -> Result<WorkGraphPublicErrorClass, WorkGraphError> {
        let kind = work_graph_error_kind(error);
        let mut dsl_auth = wg_dsl::WorkGraphLifecycleMachineAuthority::new();
        let transition = wg_dsl::WorkGraphLifecycleMachineMutator::apply(
            &mut dsl_auth,
            wg_dsl::WorkGraphLifecycleInput::ClassifyWorkGraphPublicError { kind },
        )
        .map_err(|transition_error| {
            WorkGraphError::Store(format!(
                "generated WorkGraph public error classification refused kind {kind:?}: {transition_error:?}"
            ))
        })?;

        let mut classified = None;
        for effect in transition.effects() {
            if let wg_dsl::WorkGraphLifecycleEffect::WorkGraphPublicErrorClassified {
                kind: emitted_kind,
                public_class,
            } = effect
            {
                if *emitted_kind != kind {
                    return Err(WorkGraphError::Store(format!(
                        "generated WorkGraph public error classification emitted kind {emitted_kind:?} while classifying {kind:?}"
                    )));
                }
                if classified.replace(*public_class).is_some() {
                    return Err(WorkGraphError::Store(format!(
                        "generated WorkGraph public error classification emitted multiple classes for kind {kind:?}"
                    )));
                }
            }
        }

        classified.ok_or_else(|| {
            WorkGraphError::Store(format!(
                "generated WorkGraph public error classification did not emit a class for kind {kind:?}"
            ))
        })
    }

    /// Resolve whether an untrusted PUBLIC caller may confirm an item with the
    /// given machine-owned completion policy.
    ///
    /// The trust-scoped eligibility "only a self-attested completion policy may
    /// be confirmed by a public caller; every other policy requires trusted
    /// in-process host authority" is owned by the canonical
    /// `WorkGraphLifecycleMachine`, not the public-confirm surface. The shell
    /// performs only a pure typed extraction of the machine-owned
    /// `completion_policy` into the DSL observation, drives the machine's
    /// `ClassifyPublicConfirmationAdmission` input, and mirrors the emitted
    /// `PublicConfirmationAdmissionClassified` verdict. The shell decides
    /// nothing and fails closed if the machine refuses or emits no verdict.
    pub fn classify_public_confirmation_admission(
        completion_policy: &crate::types::WorkCompletionPolicy,
    ) -> Result<wg_dsl::WorkPublicConfirmationAdmissionKind, WorkGraphError> {
        let policy = completion_policy.to_machine();
        let mut dsl_auth = wg_dsl::WorkGraphLifecycleMachineAuthority::new();
        let transition = wg_dsl::WorkGraphLifecycleMachineMutator::apply(
            &mut dsl_auth,
            wg_dsl::WorkGraphLifecycleInput::ClassifyPublicConfirmationAdmission {
                completion_policy: policy,
            },
        )
        .map_err(|error| {
            WorkGraphError::InvalidInput(format!(
                "WorkGraphLifecycle refused public-confirmation admission for {policy:?}: {error:?}"
            ))
        })?;

        let mut admission = None;
        for effect in transition.effects() {
            if let wg_dsl::WorkGraphLifecycleEffect::PublicConfirmationAdmissionClassified {
                admission: emitted,
            } = effect
                && admission.replace(*emitted).is_some()
            {
                return Err(WorkGraphError::Store(format!(
                    "WorkGraphLifecycle public-confirmation admission emitted multiple verdicts for {policy:?}"
                )));
            }
        }

        admission.ok_or_else(|| {
            WorkGraphError::Store(format!(
                "WorkGraphLifecycle public-confirmation admission emitted no verdict for {policy:?}"
            ))
        })
    }

    /// Resolve whether a requested completion policy is admissible at CREATE for
    /// a non-goal work item.
    ///
    /// The creation policy "non-goal work items must use the self-attest
    /// completion policy" is owned by the canonical `WorkGraphLifecycleMachine`,
    /// not the create shell. The shell performs only a pure typed extraction of
    /// the requested completion policy into the DSL observation, drives the
    /// machine's `ClassifyCreateCompletionPolicyAdmission` input over a fresh
    /// authority, and mirrors the emitted
    /// `CreateCompletionPolicyAdmissionClassified` verdict. The shell decides
    /// nothing and fails closed if the machine refuses or emits no verdict.
    pub fn classify_create_completion_policy_admission(
        completion_policy: &crate::types::WorkCompletionPolicy,
    ) -> Result<wg_dsl::WorkCreateCompletionPolicyAdmissionKind, WorkGraphError> {
        let policy = completion_policy.to_machine();
        let mut dsl_auth = wg_dsl::WorkGraphLifecycleMachineAuthority::new();
        let transition = wg_dsl::WorkGraphLifecycleMachineMutator::apply(
            &mut dsl_auth,
            wg_dsl::WorkGraphLifecycleInput::ClassifyCreateCompletionPolicyAdmission {
                completion_policy: policy,
            },
        )
        .map_err(|error| {
            WorkGraphError::InvalidInput(format!(
                "WorkGraphLifecycle refused create completion-policy admission for {policy:?}: {error:?}"
            ))
        })?;

        let mut admission = None;
        for effect in transition.effects() {
            if let wg_dsl::WorkGraphLifecycleEffect::CreateCompletionPolicyAdmissionClassified {
                admission: emitted,
            } = effect
                && admission.replace(*emitted).is_some()
            {
                return Err(WorkGraphError::Store(format!(
                    "WorkGraphLifecycle create completion-policy admission emitted multiple verdicts for {policy:?}"
                )));
            }
        }

        admission.ok_or_else(|| {
            WorkGraphError::Store(format!(
                "WorkGraphLifecycle create completion-policy admission emitted no verdict for {policy:?}"
            ))
        })
    }

    /// Resolve whether a requested completion-policy mutation is admissible.
    ///
    /// The invariant "a work item's completion policy is fixed at creation,
    /// monotonically tightenable, and never update-writable" is owned by the
    /// canonical `WorkGraphLifecycleMachine`, not the shell. The shell performs only a pure
    /// typed extraction of the requested completion policy into the DSL
    /// observation (variant plus supervisor owner key plus reviewer quorum
    /// threshold), drives the machine's `ClassifyCompletionPolicyMutationAdmission`
    /// input over the item's recovered machine state, and mirrors the emitted
    /// `CompletionPolicyMutationAdmissionClassified` verdict. The machine compares
    /// the requested policy — in full — against its own machine-owned completion
    /// policy; this function decides nothing and fails closed if the machine
    /// refuses or emits no verdict.
    pub fn classify_completion_policy_mutation_admission(
        item: &WorkItem,
        requested: &crate::types::WorkCompletionPolicy,
    ) -> Result<wg_dsl::WorkCompletionPolicyMutationAdmissionKind, WorkGraphError> {
        validate_item_machine_projection(item)?;
        let mut dsl_auth = wg_dsl::WorkGraphLifecycleMachineAuthority::recover_from_state(
            item.machine_state.clone(),
        )
        .map_err(|error| WorkGraphError::InvalidTransition(format!("{error:?}")))?;
        let transition = wg_dsl::WorkGraphLifecycleMachineMutator::apply(
            &mut dsl_auth,
            wg_dsl::WorkGraphLifecycleInput::ClassifyCompletionPolicyMutationAdmission {
                requested_completion_policy: requested.to_machine(),
                requested_completion_supervisor_owner_key: requested.supervisor_owner_key(),
                requested_completion_reviewer_quorum_threshold: requested
                    .reviewer_quorum_threshold(),
            },
        )
        .map_err(|error| {
            WorkGraphError::InvalidTransition(format!(
                "work item {} refused completion-policy mutation classification: {error:?}",
                item.id
            ))
        })?;

        let mut admission = None;
        for effect in transition.effects() {
            if let wg_dsl::WorkGraphLifecycleEffect::CompletionPolicyMutationAdmissionClassified {
                admission: emitted,
            } = effect
                && admission.replace(*emitted).is_some()
            {
                return Err(WorkGraphError::Store(format!(
                    "work item {} emitted multiple completion-policy mutation verdicts",
                    item.id
                )));
            }
        }

        admission.ok_or_else(|| {
            WorkGraphError::Store(format!(
                "work item {} emitted no completion-policy mutation verdict",
                item.id
            ))
        })
    }

    /// Resolve whether a trusted-path goal confirmation is admissible for a work
    /// item's machine-owned completion policy.
    ///
    /// The eligibility "is this confirming principal + supplied evidence kind
    /// admissible for this completion policy" is owned by the canonical
    /// `WorkGraphLifecycleMachine`, not the goal-confirm shell. The shell
    /// performs only pure typed extraction of the observations (the machine-owned
    /// completion policy + its supervisor owner key, the requested confirming
    /// principal owner key + kind, and the typed evidence-kind observation
    /// projected from the evidence's typed confirmation classification), drives
    /// the machine's
    /// `ClassifyConfirmationAdmission` input, and mirrors the emitted
    /// `ConfirmationAdmissionClassified` verdict. This function decides nothing
    /// and fails closed if the machine refuses or emits no verdict.
    pub fn classify_confirmation_admission(
        completion_policy: &crate::types::WorkCompletionPolicy,
        requested_principal: Option<&crate::types::WorkOwnerKey>,
        supplied_evidence_kind: wg_dsl::WorkConfirmationEvidenceObservation,
    ) -> Result<wg_dsl::WorkConfirmationAdmissionKind, WorkGraphError> {
        let policy = completion_policy.to_machine();
        let requested_principal_owner_key =
            requested_principal.map(crate::types::work_owner_key_to_machine);
        let requested_principal_kind = requested_principal
            .map(|principal| crate::types::work_owner_kind_to_machine(principal.kind));
        let mut dsl_auth = wg_dsl::WorkGraphLifecycleMachineAuthority::new();
        let transition = wg_dsl::WorkGraphLifecycleMachineMutator::apply(
            &mut dsl_auth,
            wg_dsl::WorkGraphLifecycleInput::ClassifyConfirmationAdmission {
                completion_policy: policy,
                completion_supervisor_owner_key: completion_policy.supervisor_owner_key(),
                requested_principal_owner_key,
                requested_principal_kind,
                supplied_evidence_kind,
            },
        )
        .map_err(|error| {
            WorkGraphError::Store(format!(
                "WorkGraphLifecycle refused confirmation admission for {policy:?}: {error:?}"
            ))
        })?;

        let mut admission = None;
        for effect in transition.effects() {
            if let wg_dsl::WorkGraphLifecycleEffect::ConfirmationAdmissionClassified {
                admission: emitted,
            } = effect
                && admission.replace(*emitted).is_some()
            {
                return Err(WorkGraphError::Store(format!(
                    "WorkGraphLifecycle confirmation admission emitted multiple verdicts for {policy:?}"
                )));
            }
        }

        admission.ok_or_else(|| {
            WorkGraphError::Store(format!(
                "WorkGraphLifecycle confirmation admission emitted no verdict for {policy:?}"
            ))
        })
    }

    /// Resolve whether a work item is terminal.
    ///
    /// The shell extracts no fact: it drives the canonical
    /// `WorkGraphLifecycleMachine`'s `ClassifyTerminality` input over the item's
    /// recovered machine state. The machine owns the lifecycle_phase and the
    /// terminality verdict (which phases are terminal); this function mirrors the
    /// emitted `WorkItemTerminalityClassified.terminal`, failing closed if the
    /// machine refuses or emits no verdict. It decides nothing.
    pub fn classify_terminality(item: &WorkItem) -> Result<bool, WorkGraphError> {
        validate_item_machine_projection(item)?;
        let mut dsl_auth = wg_dsl::WorkGraphLifecycleMachineAuthority::recover_from_state(
            item.machine_state.clone(),
        )
        .map_err(|error| WorkGraphError::InvalidTransition(format!("{error:?}")))?;
        let transition = wg_dsl::WorkGraphLifecycleMachineMutator::apply(
            &mut dsl_auth,
            wg_dsl::WorkGraphLifecycleInput::ClassifyTerminality {},
        )
        .map_err(|error| {
            WorkGraphError::InvalidTransition(format!(
                "work item {} refused terminality classification: {error:?}",
                item.id
            ))
        })?;

        let mut classified = None;
        for effect in transition.effects() {
            if let wg_dsl::WorkGraphLifecycleEffect::WorkItemTerminalityClassified { terminal } =
                effect
                && classified.replace(*terminal).is_some()
            {
                return Err(WorkGraphError::Store(format!(
                    "work item {} emitted multiple terminality verdicts",
                    item.id
                )));
            }
        }

        classified.ok_or_else(|| {
            WorkGraphError::Store(format!(
                "work item {} emitted no terminality verdict",
                item.id
            ))
        })
    }

    /// Resolve whether a single blocking edge is satisfied.
    ///
    /// The shell extracts only the raw blocker lifecycle phase (a pure
    /// observation projected from the blocker's own machine state) and whether
    /// the blocker was resolvable at all, then drives the canonical
    /// `WorkGraphLifecycleMachine`'s `ClassifyBlockerSatisfied` input over the
    /// gated item's recovered state. The machine owns the satisfaction POLICY (a
    /// blocking edge is satisfied iff its blocker reached terminal SUCCESS,
    /// `Completed`) and emits the verdict; this function mirrors it. The caller
    /// mechanically fans-in (counts) the unsatisfied edges. Fails closed.
    pub fn classify_blocker_satisfied(
        gated_item: &WorkItem,
        blocker: Option<&WorkItem>,
    ) -> Result<bool, WorkGraphError> {
        validate_item_machine_projection(gated_item)?;
        let blocker_lifecycle_phase = match blocker {
            Some(blocker) => blocker.machine_state.lifecycle_phase,
            None => wg_dsl::WorkLifecycleState::Absent,
        };
        let mut dsl_auth = wg_dsl::WorkGraphLifecycleMachineAuthority::recover_from_state(
            gated_item.machine_state.clone(),
        )
        .map_err(|error| WorkGraphError::InvalidTransition(format!("{error:?}")))?;
        let transition = wg_dsl::WorkGraphLifecycleMachineMutator::apply(
            &mut dsl_auth,
            wg_dsl::WorkGraphLifecycleInput::ClassifyBlockerSatisfied {
                blocker_present: blocker.is_some(),
                blocker_lifecycle_phase,
            },
        )
        .map_err(|error| {
            WorkGraphError::InvalidTransition(format!(
                "work item {} refused blocker satisfaction classification: {error:?}",
                gated_item.id
            ))
        })?;

        let mut classified = None;
        for effect in transition.effects() {
            if let wg_dsl::WorkGraphLifecycleEffect::BlockerSatisfactionClassified { satisfied } =
                effect
                && classified.replace(*satisfied).is_some()
            {
                return Err(WorkGraphError::Store(format!(
                    "work item {} emitted multiple blocker satisfaction verdicts",
                    gated_item.id
                )));
            }
        }

        classified.ok_or_else(|| {
            WorkGraphError::Store(format!(
                "work item {} emitted no blocker satisfaction verdict",
                gated_item.id
            ))
        })
    }

    pub fn create_item(
        request: CreateWorkItemRequest,
        realm_id: String,
        namespace: WorkNamespace,
        now: DateTime<Utc>,
    ) -> Result<(WorkItem, WorkGraphEvent), WorkGraphError> {
        let title = validate_title(request.title)?;
        let status = request.status.unwrap_or_default();
        // The creation policy "a new work item may only start open or blocked"
        // is owned by WorkGraphLifecycleMachine, not this shell. We extract the
        // requested status as a pure typed observation, drive the machine's
        // admission classifier, and mirror the verdict: AdmittedOpen ->
        // CreateOpen, AdmittedBlocked -> CreateBlocked, Denied -> the same
        // InvalidTransition rejection. Fails closed.
        let input = match classify_create_status_admission(status)? {
            wg_dsl::WorkCreateStatusAdmissionKind::AdmittedOpen => {
                wg_dsl::WorkGraphLifecycleInput::CreateOpen {
                    due_at_utc_ms: request.due_at.map(datetime_to_millis),
                    not_before_utc_ms: request.not_before.map(datetime_to_millis),
                    snoozed_until_utc_ms: request.snoozed_until.map(datetime_to_millis),
                    completion_policy: request.completion_policy.clone().to_machine(),
                    completion_supervisor_owner_key: request
                        .completion_policy
                        .supervisor_owner_key(),
                    completion_reviewer_quorum_threshold: request
                        .completion_policy
                        .reviewer_quorum_threshold(),
                    unresolved_blocker_count: 0,
                }
            }
            wg_dsl::WorkCreateStatusAdmissionKind::AdmittedBlocked => {
                wg_dsl::WorkGraphLifecycleInput::CreateBlocked {
                    due_at_utc_ms: request.due_at.map(datetime_to_millis),
                    not_before_utc_ms: request.not_before.map(datetime_to_millis),
                    snoozed_until_utc_ms: request.snoozed_until.map(datetime_to_millis),
                    completion_policy: request.completion_policy.clone().to_machine(),
                    completion_supervisor_owner_key: request
                        .completion_policy
                        .supervisor_owner_key(),
                    completion_reviewer_quorum_threshold: request
                        .completion_policy
                        .reviewer_quorum_threshold(),
                    unresolved_blocker_count: 0,
                }
            }
            wg_dsl::WorkCreateStatusAdmissionKind::Denied => {
                return Err(WorkGraphError::InvalidTransition(
                    "new work items may only start open or blocked".to_string(),
                ));
            }
        };
        let dsl_state = apply_new_item_dsl(input)?;
        let mut item = WorkItem {
            id: WorkItemId::generated(),
            realm_id,
            namespace,
            title,
            description: request.description,
            status: work_status_from_dsl(dsl_state.lifecycle_phase)?,
            completion_policy: request.completion_policy,
            priority: request.priority,
            labels: normalize_labels(request.labels)?,
            owner: None,
            claim: None,
            machine_state: dsl_state.clone(),
            revision: dsl_state.revision,
            due_at: dsl_state.due_at_utc_ms.and_then(millis_to_datetime),
            not_before: dsl_state.not_before_utc_ms.and_then(millis_to_datetime),
            snoozed_until: dsl_state.snoozed_until_utc_ms.and_then(millis_to_datetime),
            created_at: now,
            updated_at: now,
            terminal_at: dsl_state.terminal_at_utc_ms.and_then(millis_to_datetime),
            external_refs: request.external_refs,
            evidence_refs: request.evidence_refs,
        };
        sync_item_from_machine_state(&mut item)?;
        let event = item_event(&item, WorkGraphEventKind::Created, now)?;
        Ok((item, event))
    }

    pub fn update_item(
        mut item: WorkItem,
        request: UpdateWorkItemRequest,
        now: DateTime<Utc>,
    ) -> Result<(WorkItem, WorkGraphEvent), WorkGraphError> {
        let due_at = request.due_at.or(item.due_at);
        let not_before = request.not_before.or(item.not_before);
        let snoozed_until = request.snoozed_until.or(item.snoozed_until);
        let completion_policy = request
            .completion_policy
            .unwrap_or_else(|| item.completion_policy.clone());
        let dsl_state = apply_item_dsl(
            &item,
            item.machine_state.unresolved_blocker_count,
            wg_dsl::WorkGraphLifecycleInput::Update {
                expected_revision: request.expected_revision,
                due_at_utc_ms: due_at.map(datetime_to_millis),
                not_before_utc_ms: not_before.map(datetime_to_millis),
                snoozed_until_utc_ms: snoozed_until.map(datetime_to_millis),
                completion_policy: completion_policy.clone().to_machine(),
                completion_supervisor_owner_key: completion_policy.supervisor_owner_key(),
                completion_reviewer_quorum_threshold: completion_policy.reviewer_quorum_threshold(),
                unresolved_blocker_count: item.machine_state.unresolved_blocker_count,
            },
            Some(request.expected_revision),
        )?;

        if let Some(title) = request.title {
            item.title = validate_title(title)?;
        }
        if let Some(description) = request.description {
            item.description = Some(description);
        }
        if let Some(priority) = request.priority {
            item.priority = priority;
        }
        if let Some(labels) = request.labels {
            item.labels = normalize_labels(labels)?;
        }
        item.machine_state = dsl_state;
        sync_item_from_machine_state(&mut item)?;
        if !request.external_refs.is_empty() {
            item.external_refs = request.external_refs;
        }
        item.updated_at = now;
        let event = item_event(&item, WorkGraphEventKind::Updated, now)?;
        Ok((item, event))
    }

    pub fn escalate_policy(
        mut item: WorkItem,
        request: PolicyEscalateRequest,
        now: DateTime<Utc>,
    ) -> Result<(WorkItem, WorkGraphEvent), WorkGraphError> {
        validate_item_machine_projection(&item)?;
        let mut state = item.machine_state.clone();
        state.unresolved_blocker_count = item.machine_state.unresolved_blocker_count;
        let mut dsl_auth = wg_dsl::WorkGraphLifecycleMachineAuthority::recover_from_state(state)
            .map_err(|error| WorkGraphError::InvalidTransition(format!("{error:?}")))?;
        let transition = wg_dsl::WorkGraphLifecycleMachineMutator::apply(
            &mut dsl_auth,
            wg_dsl::WorkGraphLifecycleInput::PolicyEscalate {
                expected_revision: request.expected_revision,
                requested_completion_policy: request.completion_policy.to_machine(),
                requested_completion_supervisor_owner_key: request
                    .completion_policy
                    .supervisor_owner_key(),
                requested_completion_reviewer_quorum_threshold: request
                    .completion_policy
                    .reviewer_quorum_threshold(),
            },
        )
        .map_err(|error| {
            if item.revision != request.expected_revision {
                return WorkGraphError::StaleRevision {
                    id: item.id.clone(),
                    expected: request.expected_revision,
                    actual: item.revision,
                };
            }
            WorkGraphError::InvalidTransition(format!("{error:?}"))
        })?;

        let mut admission = None;
        for effect in transition.effects() {
            if let wg_dsl::WorkGraphLifecycleEffect::PolicyEscalationAdmissionClassified {
                admission: emitted,
            } = effect
                && admission.replace(*emitted).is_some()
            {
                return Err(WorkGraphError::Store(format!(
                    "work item {} emitted multiple policy escalation verdicts",
                    item.id
                )));
            }
        }
        match admission.ok_or_else(|| {
            WorkGraphError::Store(format!(
                "work item {} emitted no policy escalation verdict",
                item.id
            ))
        })? {
            WorkPolicyEscalationAdmissionKind::Admitted => {}
            WorkPolicyEscalationAdmissionKind::Denied => {
                return Err(WorkGraphError::InvalidInput(format!(
                    "completion policy for work item {} can only be monotonically tightened",
                    item.id
                )));
            }
        }

        item.machine_state = dsl_auth.state().clone();
        sync_item_from_machine_state(&mut item)?;
        item.updated_at = now;
        let event = item_event(&item, WorkGraphEventKind::Updated, now)?;
        Ok((item, event))
    }

    pub fn claim_item(
        item: WorkItem,
        request: ClaimWorkItemRequest,
        now: DateTime<Utc>,
    ) -> Result<(WorkItem, WorkGraphEvent), WorkGraphError> {
        Self::claim_ready_item(item, request, now)
    }

    pub fn claim_ready_item(
        item: WorkItem,
        request: ClaimWorkItemRequest,
        now: DateTime<Utc>,
    ) -> Result<(WorkItem, WorkGraphEvent), WorkGraphError> {
        Self::claim_item_with_unresolved_blockers(
            item.clone(),
            item.machine_state.unresolved_blocker_count,
            request,
            now,
        )
    }

    pub fn refresh_eligibility(
        mut item: WorkItem,
        unresolved_blocker_count: u64,
        now: DateTime<Utc>,
    ) -> Result<Option<(WorkItem, WorkGraphEvent)>, WorkGraphError> {
        if item.machine_state.unresolved_blocker_count == unresolved_blocker_count {
            return Ok(None);
        }
        let dsl_state = apply_item_dsl(
            &item,
            unresolved_blocker_count,
            wg_dsl::WorkGraphLifecycleInput::RefreshEligibility {
                unresolved_blocker_count,
            },
            None,
        )?;
        item.machine_state = dsl_state;
        sync_item_from_machine_state(&mut item)?;
        item.updated_at = now;
        let event = item_event(&item, WorkGraphEventKind::Updated, now)?;
        Ok(Some((item, event)))
    }

    pub(crate) fn claim_item_with_unresolved_blockers(
        mut item: WorkItem,
        unresolved_blocker_count: u64,
        request: ClaimWorkItemRequest,
        now: DateTime<Utc>,
    ) -> Result<(WorkItem, WorkGraphEvent), WorkGraphError> {
        let lease_expires_at = request.lease_expires_at.or_else(|| {
            request
                .lease_seconds
                .map(|seconds| now + seconds_to_duration(seconds))
        });
        let owner_key = work_owner_key(&request.owner)?;
        let dsl_state = apply_item_dsl(
            &item,
            unresolved_blocker_count,
            wg_dsl::WorkGraphLifecycleInput::Claim {
                expected_revision: request.expected_revision,
                owner_key,
                now_utc_ms: datetime_to_millis(now),
                lease_expires_at_utc_ms: lease_expires_at.map(datetime_to_millis),
            },
            Some(request.expected_revision),
        )?;
        item.owner = Some(request.owner.clone());
        item.claim = Some(WorkClaim {
            owner: request.owner,
            claimed_at: now,
            lease_expires_at,
        });
        item.machine_state = dsl_state;
        sync_item_from_machine_state(&mut item)?;
        item.updated_at = now;
        let event = item_event(&item, WorkGraphEventKind::Claimed, now)?;
        Ok((item, event))
    }

    pub fn release_item(
        mut item: WorkItem,
        request: ReleaseWorkItemRequest,
        now: DateTime<Utc>,
    ) -> Result<(WorkItem, WorkGraphEvent), WorkGraphError> {
        let dsl_state = apply_item_dsl(
            &item,
            item.machine_state.unresolved_blocker_count,
            wg_dsl::WorkGraphLifecycleInput::Release {
                expected_revision: request.expected_revision,
            },
            Some(request.expected_revision),
        )?;
        item.claim = None;
        item.owner = None;
        item.machine_state = dsl_state;
        sync_item_from_machine_state(&mut item)?;
        item.updated_at = now;
        let event = item_event(&item, WorkGraphEventKind::Released, now)?;
        Ok((item, event))
    }

    pub fn block_item(
        mut item: WorkItem,
        expected_revision: u64,
        now: DateTime<Utc>,
    ) -> Result<(WorkItem, WorkGraphEvent), WorkGraphError> {
        let dsl_state = apply_item_dsl(
            &item,
            item.machine_state.unresolved_blocker_count,
            wg_dsl::WorkGraphLifecycleInput::Block { expected_revision },
            Some(expected_revision),
        )?;
        item.claim = None;
        item.owner = None;
        item.machine_state = dsl_state;
        sync_item_from_machine_state(&mut item)?;
        item.updated_at = now;
        let event = item_event(&item, WorkGraphEventKind::Blocked, now)?;
        Ok((item, event))
    }

    pub fn close_item(
        mut item: WorkItem,
        request: CloseWorkItemRequest,
        now: DateTime<Utc>,
    ) -> Result<(WorkItem, WorkGraphEvent), WorkGraphError> {
        // The lifecycle-class fact "close requires a terminal status" is owned
        // by WorkGraphLifecycleMachine, not this shell. We extract the requested
        // target status as a pure typed observation, drive the machine's
        // admission classifier, and mirror the verdict: AdmittedCompleted ->
        // CloseCompleted, AdmittedCancelled -> CloseCancelled, AdmittedFailed ->
        // CloseFailed, DeniedNonTerminal -> the exact same InvalidTransition
        // rejection. Fails closed.
        let dsl_input = match classify_close_status_admission(request.status)? {
            wg_dsl::WorkCloseStatusAdmissionKind::AdmittedCompleted => {
                wg_dsl::WorkGraphLifecycleInput::CloseCompleted {
                    expected_revision: request.expected_revision,
                    at_utc_ms: datetime_to_millis(now),
                }
            }
            wg_dsl::WorkCloseStatusAdmissionKind::AdmittedCancelled => {
                wg_dsl::WorkGraphLifecycleInput::CloseCancelled {
                    expected_revision: request.expected_revision,
                    at_utc_ms: datetime_to_millis(now),
                }
            }
            wg_dsl::WorkCloseStatusAdmissionKind::AdmittedFailed => {
                wg_dsl::WorkGraphLifecycleInput::CloseFailed {
                    expected_revision: request.expected_revision,
                    at_utc_ms: datetime_to_millis(now),
                }
            }
            wg_dsl::WorkCloseStatusAdmissionKind::DeniedNonTerminal => {
                return Err(WorkGraphError::InvalidTransition(
                    "close requires a terminal status".to_string(),
                ));
            }
        };
        let dsl_state = apply_item_dsl(
            &item,
            item.machine_state.unresolved_blocker_count,
            dsl_input,
            Some(request.expected_revision),
        )?;
        item.claim = None;
        item.owner = None;
        item.machine_state = dsl_state;
        sync_item_from_machine_state(&mut item)?;
        item.updated_at = now;
        let event = item_event(&item, WorkGraphEventKind::Closed, now)?;
        Ok((item, event))
    }

    pub fn add_evidence(
        mut item: WorkItem,
        request: AddEvidenceRequest,
        now: DateTime<Utc>,
    ) -> Result<(WorkItem, WorkGraphEvent), WorkGraphError> {
        let evidence_kind = request
            .evidence
            .confirmation_kind
            .unwrap_or(crate::types::WorkEvidenceKind::SelfAttest)
            .to_machine();
        let confirming_owner_key = request
            .evidence
            .confirming_owner_key
            .as_ref()
            .map(crate::types::work_owner_key_to_machine);
        let dsl_state = apply_item_dsl(
            &item,
            item.machine_state.unresolved_blocker_count,
            wg_dsl::WorkGraphLifecycleInput::AddEvidence {
                expected_revision: request.expected_revision,
                evidence_kind,
                confirming_owner_key,
            },
            Some(request.expected_revision),
        )?;
        item.evidence_refs.push(request.evidence);
        item.machine_state = dsl_state;
        sync_item_from_machine_state(&mut item)?;
        item.updated_at = now;
        let event = item_event(&item, WorkGraphEventKind::EvidenceAdded, now)?;
        Ok((item, event))
    }

    /// Classify whether a work item is ready to claim at `now`.
    ///
    /// The shell extracts only the raw wall-clock `now` (a pure observation) and
    /// drives the canonical `WorkGraphLifecycleMachine`'s `ClassifyReadiness`
    /// input over the item's recovered machine state. The machine owns the
    /// readiness POLICY — reproducing exactly the `Claim` transition guards
    /// (`ClaimOpen` / `ClaimExpiredInProgress`) — and emits the verdict; this
    /// function mirrors the emitted `WorkItemReadinessClassified.ready`, failing
    /// closed if the machine refuses or emits no verdict. It decides nothing.
    pub fn classify_readiness(item: &WorkItem, now: DateTime<Utc>) -> Result<bool, WorkGraphError> {
        validate_item_machine_projection(item)?;
        let mut dsl_auth = wg_dsl::WorkGraphLifecycleMachineAuthority::recover_from_state(
            item.machine_state.clone(),
        )
        .map_err(|error| WorkGraphError::InvalidTransition(format!("{error:?}")))?;
        let transition = wg_dsl::WorkGraphLifecycleMachineMutator::apply(
            &mut dsl_auth,
            wg_dsl::WorkGraphLifecycleInput::ClassifyReadiness {
                now_utc_ms: datetime_to_millis(now),
            },
        )
        .map_err(|error| {
            WorkGraphError::InvalidTransition(format!(
                "work item {} refused readiness classification: {error:?}",
                item.id
            ))
        })?;

        let mut classified = None;
        for effect in transition.effects() {
            if let wg_dsl::WorkGraphLifecycleEffect::WorkItemReadinessClassified { ready } = effect
                && classified.replace(*ready).is_some()
            {
                return Err(WorkGraphError::Store(format!(
                    "work item {} emitted multiple readiness verdicts",
                    item.id
                )));
            }
        }

        classified.ok_or_else(|| {
            WorkGraphError::Store(format!(
                "work item {} emitted no readiness verdict",
                item.id
            ))
        })
    }

    /// Mirror the machine-owned readiness verdict as a `bool`, failing closed.
    ///
    /// The readiness verdict belongs to the canonical
    /// `WorkGraphLifecycleMachine` (see
    /// [`WorkGraphMachine::classify_readiness`]). This filter-facing projection
    /// mirrors the machine bool; if the machine refuses to classify, it fails
    /// closed by treating the item as NOT ready so an unclassifiable item is
    /// never offered as claimable work.
    pub fn is_ready(item: &WorkItem, now: DateTime<Utc>) -> bool {
        Self::classify_readiness(item, now).unwrap_or(false)
    }

    pub fn ready_items(items: Vec<WorkItem>, now: DateTime<Utc>) -> Vec<WorkItem> {
        items
            .into_iter()
            .filter(|item| Self::is_ready(item, now))
            .collect()
    }

    pub fn validate_link(
        edge: &WorkEdge,
        existing_items: &[WorkItem],
        existing_edges: &[WorkEdge],
    ) -> Result<(), WorkGraphError> {
        let topology_state = topology_state(existing_items, existing_edges);
        apply_link_validation_dsl(
            topology_state,
            wg_dsl::WorkGraphLifecycleInput::ValidateLink {
                kind: dsl_edge_kind(edge.kind),
                from_item_key: work_item_key(&edge.from_id),
                to_item_key: work_item_key(&edge.to_id),
                edge_key: work_edge_key(edge.kind, &edge.from_id, &edge.to_id),
                reverse_path_key: dependency_path_key(edge.kind, &edge.to_id, &edge.from_id),
            },
        )?;
        Ok(())
    }
}

/// Pure typed extraction of a `WorkGraphError` into the machine's typed
/// error-kind discriminant. This is a 1:1 variant->kind map with NO grouping;
/// the many-to-one variant->public-class POLICY lives in the canonical
/// `WorkGraphLifecycleMachine`, not here.
fn work_graph_error_kind(error: &WorkGraphError) -> wg_dsl::WorkGraphErrorKind {
    match error {
        WorkGraphError::NotFound { .. } => wg_dsl::WorkGraphErrorKind::NotFound,
        WorkGraphError::AttentionNotFound { .. } => wg_dsl::WorkGraphErrorKind::AttentionNotFound,
        WorkGraphError::StaleRevision { .. } => wg_dsl::WorkGraphErrorKind::StaleRevision,
        WorkGraphError::Conflict(_) => wg_dsl::WorkGraphErrorKind::Conflict,
        WorkGraphError::InvalidTransition(_) => wg_dsl::WorkGraphErrorKind::InvalidTransition,
        WorkGraphError::InvalidInput(_) => wg_dsl::WorkGraphErrorKind::InvalidInput,
        WorkGraphError::InvalidTimestampMillis { .. } => {
            wg_dsl::WorkGraphErrorKind::InvalidTimestampMillis
        }
        WorkGraphError::Store(_) => wg_dsl::WorkGraphErrorKind::Store,
        WorkGraphError::UnsupportedBackend(_) => wg_dsl::WorkGraphErrorKind::UnsupportedBackend,
    }
}

pub(crate) fn completion_policy_name(policy: &WorkCompletionPolicy) -> &'static str {
    match policy {
        WorkCompletionPolicy::SelfAttest => "self_attest",
        WorkCompletionPolicy::HostConfirmed => "host_confirmed",
        WorkCompletionPolicy::PrincipalConfirmed => "principal_confirmed",
        WorkCompletionPolicy::Supervisor { .. } => "supervisor",
        WorkCompletionPolicy::ReviewerQuorum { .. } => "reviewer_quorum",
    }
}

fn validate_title(title: String) -> Result<String, WorkGraphError> {
    let title = title.trim();
    if title.is_empty() {
        return Err(WorkGraphError::InvalidInput(
            "work item title must not be empty".to_string(),
        ));
    }
    Ok(title.to_string())
}

fn normalize_labels(labels: BTreeSet<String>) -> Result<BTreeSet<String>, WorkGraphError> {
    let mut normalized = BTreeSet::new();
    for label in labels {
        let label = label.trim();
        if label.is_empty() {
            return Err(WorkGraphError::InvalidInput(
                "work item labels must not be empty".to_string(),
            ));
        }
        normalized.insert(label.to_string());
    }
    Ok(normalized)
}

fn apply_new_item_dsl(
    input: wg_dsl::WorkGraphLifecycleInput,
) -> Result<wg_dsl::WorkGraphLifecycleMachineState, WorkGraphError> {
    let mut dsl_auth = wg_dsl::WorkGraphLifecycleMachineAuthority::new();
    wg_dsl::WorkGraphLifecycleMachineMutator::apply(&mut dsl_auth, input)
        .map_err(|error| WorkGraphError::InvalidTransition(format!("{error:?}")))?;
    Ok(dsl_auth.state().clone())
}

/// Resolve whether a requested INITIAL work status is an admissible creation
/// state.
///
/// The shell performs only a pure typed extraction of the requested
/// `WorkStatus` into the machine's `WorkLifecycleState` observation. The
/// creation POLICY "a new work item may only start open or blocked" is owned by
/// the canonical `WorkGraphLifecycleMachine`: this drives its
/// `ClassifyCreateStatusAdmission` input over a fresh authority and mirrors the
/// emitted `CreateStatusAdmissionClassified` verdict. The shell decides nothing
/// and fails closed if the machine refuses or emits no verdict.
fn classify_create_status_admission(
    status: WorkStatus,
) -> Result<wg_dsl::WorkCreateStatusAdmissionKind, WorkGraphError> {
    let requested_status = crate::types::work_lifecycle_state_from_status(status);
    let mut dsl_auth = wg_dsl::WorkGraphLifecycleMachineAuthority::new();
    let transition = wg_dsl::WorkGraphLifecycleMachineMutator::apply(
        &mut dsl_auth,
        wg_dsl::WorkGraphLifecycleInput::ClassifyCreateStatusAdmission { requested_status },
    )
    .map_err(|error| {
        WorkGraphError::InvalidTransition(format!(
            "WorkGraphLifecycle refused create-status admission for {requested_status:?}: {error:?}"
        ))
    })?;

    let mut admission = None;
    for effect in transition.effects() {
        if let wg_dsl::WorkGraphLifecycleEffect::CreateStatusAdmissionClassified {
            admission: emitted,
        } = effect
            && admission.replace(*emitted).is_some()
        {
            return Err(WorkGraphError::Store(format!(
                "WorkGraphLifecycle create-status admission emitted multiple verdicts for {requested_status:?}"
            )));
        }
    }

    admission.ok_or_else(|| {
        WorkGraphError::Store(format!(
            "WorkGraphLifecycle create-status admission emitted no verdict for {requested_status:?}"
        ))
    })
}

/// Resolve whether a requested target lifecycle status is an admissible CLOSE
/// target.
///
/// The shell performs only a pure typed extraction of the requested
/// `WorkStatus` into the machine's `WorkLifecycleState` observation. The
/// lifecycle-class fact "close requires a terminal status" is owned by the
/// canonical `WorkGraphLifecycleMachine`: this drives its
/// `ClassifyCloseStatusAdmission` input over a fresh authority and mirrors the
/// emitted `CloseStatusAdmissionClassified` verdict. The shell decides nothing
/// and fails closed if the machine refuses or emits no verdict.
fn classify_close_status_admission(
    status: WorkStatus,
) -> Result<wg_dsl::WorkCloseStatusAdmissionKind, WorkGraphError> {
    let requested_status = crate::types::work_lifecycle_state_from_status(status);
    let mut dsl_auth = wg_dsl::WorkGraphLifecycleMachineAuthority::new();
    let transition = wg_dsl::WorkGraphLifecycleMachineMutator::apply(
        &mut dsl_auth,
        wg_dsl::WorkGraphLifecycleInput::ClassifyCloseStatusAdmission { requested_status },
    )
    .map_err(|error| {
        WorkGraphError::InvalidTransition(format!(
            "WorkGraphLifecycle refused close-status admission for {requested_status:?}: {error:?}"
        ))
    })?;

    let mut admission = None;
    for effect in transition.effects() {
        if let wg_dsl::WorkGraphLifecycleEffect::CloseStatusAdmissionClassified {
            admission: emitted,
        } = effect
            && admission.replace(*emitted).is_some()
        {
            return Err(WorkGraphError::Store(format!(
                "WorkGraphLifecycle close-status admission emitted multiple verdicts for {requested_status:?}"
            )));
        }
    }

    admission.ok_or_else(|| {
        WorkGraphError::Store(format!(
            "WorkGraphLifecycle close-status admission emitted no verdict for {requested_status:?}"
        ))
    })
}

fn apply_link_validation_dsl(
    state: WorkGraphMachineState,
    input: wg_dsl::WorkGraphLifecycleInput,
) -> Result<(), WorkGraphError> {
    let mut dsl_auth = wg_dsl::WorkGraphLifecycleMachineAuthority::recover_from_state(state)
        .map_err(|error| WorkGraphError::InvalidTransition(format!("{error:?}")))?;
    wg_dsl::WorkGraphLifecycleMachineMutator::apply(&mut dsl_auth, input)
        .map_err(|error| WorkGraphError::InvalidTransition(format!("{error:?}")))?;
    Ok(())
}

fn apply_attention_dsl(
    binding: &WorkAttentionBinding,
    input: attention_dsl::WorkAttentionLifecycleInput,
    expected_revision: Option<u64>,
) -> Result<attention_dsl::WorkAttentionLifecycleMachineState, WorkGraphError> {
    let mut dsl_auth = attention_dsl::WorkAttentionLifecycleMachineAuthority::recover_from_state(
        binding.machine_state.clone(),
    )
    .map_err(|error| {
        WorkGraphError::InvalidTransition(format!(
            "attention binding {} refused recovery: {error:?}",
            binding.binding_id
        ))
    })?;
    attention_dsl::WorkAttentionLifecycleMachineMutator::apply(&mut dsl_auth, input).map_err(
        |error| {
            if let Some(expected) = expected_revision
                && binding.machine_state.revision != expected
            {
                return WorkGraphError::StaleRevision {
                    id: binding.work_ref.item_id.clone(),
                    expected,
                    actual: binding.machine_state.revision,
                };
            }
            WorkGraphError::InvalidTransition(format!(
                "attention binding {} refused transition: {error:?}",
                binding.binding_id
            ))
        },
    )?;
    Ok(dsl_auth.state().clone())
}

fn apply_item_dsl(
    item: &WorkItem,
    unresolved_blocker_count: u64,
    input: wg_dsl::WorkGraphLifecycleInput,
    expected_revision: Option<u64>,
) -> Result<WorkGraphMachineState, WorkGraphError> {
    validate_item_machine_projection(item)?;
    let mut state = item.machine_state.clone();
    state.unresolved_blocker_count = unresolved_blocker_count;
    let mut dsl_auth = wg_dsl::WorkGraphLifecycleMachineAuthority::recover_from_state(state)
        .map_err(|error| WorkGraphError::InvalidTransition(format!("{error:?}")))?;
    wg_dsl::WorkGraphLifecycleMachineMutator::apply(&mut dsl_auth, input).map_err(|error| {
        if let Some(expected) = expected_revision
            && item.revision != expected
        {
            return WorkGraphError::StaleRevision {
                id: item.id.clone(),
                expected,
                actual: item.revision,
            };
        }
        WorkGraphError::InvalidTransition(format!("{error:?}"))
    })?;
    Ok(dsl_auth.state().clone())
}

fn sync_attention_from_machine_state(binding: &mut WorkAttentionBinding) {
    binding.status = match binding.machine_state.lifecycle_phase {
        attention_dsl::WorkAttentionLifecycleState::Active => WorkAttentionStatus::Active,
        attention_dsl::WorkAttentionLifecycleState::Paused => WorkAttentionStatus::Paused {
            until: binding
                .machine_state
                .paused_until_utc_ms
                .and_then(millis_to_datetime),
        },
        attention_dsl::WorkAttentionLifecycleState::Superseded => WorkAttentionStatus::Superseded,
        attention_dsl::WorkAttentionLifecycleState::Stopped => WorkAttentionStatus::Stopped,
    };
}

fn work_status_from_dsl(status: wg_dsl::WorkLifecycleState) -> Result<WorkStatus, WorkGraphError> {
    match status {
        wg_dsl::WorkLifecycleState::Open => Ok(WorkStatus::Open),
        wg_dsl::WorkLifecycleState::InProgress => Ok(WorkStatus::InProgress),
        wg_dsl::WorkLifecycleState::Blocked => Ok(WorkStatus::Blocked),
        wg_dsl::WorkLifecycleState::Completed => Ok(WorkStatus::Completed),
        wg_dsl::WorkLifecycleState::Cancelled => Ok(WorkStatus::Cancelled),
        wg_dsl::WorkLifecycleState::Failed => Ok(WorkStatus::Failed),
        wg_dsl::WorkLifecycleState::Absent => Err(WorkGraphError::InvalidTransition(
            "work item lifecycle state is absent".to_string(),
        )),
    }
}

fn sync_item_from_machine_state(item: &mut WorkItem) -> Result<(), WorkGraphError> {
    item.status = work_status_from_dsl(item.machine_state.lifecycle_phase)?;
    item.revision = item.machine_state.revision;
    item.due_at = item
        .machine_state
        .due_at_utc_ms
        .and_then(millis_to_datetime);
    item.not_before = item
        .machine_state
        .not_before_utc_ms
        .and_then(millis_to_datetime);
    item.snoozed_until = item
        .machine_state
        .snoozed_until_utc_ms
        .and_then(millis_to_datetime);
    item.completion_policy = crate::types::WorkCompletionPolicy::from_machine(
        item.machine_state.completion_policy,
        item.machine_state.completion_supervisor_owner_key.clone(),
        item.machine_state.completion_reviewer_quorum_threshold,
    );
    item.terminal_at = item
        .machine_state
        .terminal_at_utc_ms
        .and_then(millis_to_datetime);
    Ok(())
}

fn validate_item_machine_projection(item: &WorkItem) -> Result<(), WorkGraphError> {
    let status = work_status_from_dsl(item.machine_state.lifecycle_phase)?;
    if item.status != status {
        return Err(WorkGraphError::Store(format!(
            "work item {} status projection {:?} does not match machine state {:?}",
            item.id, item.status, status
        )));
    }
    if item.revision != item.machine_state.revision {
        return Err(WorkGraphError::Store(format!(
            "work item {} revision projection {} does not match machine state {}",
            item.id, item.revision, item.machine_state.revision
        )));
    }
    if item.due_at.map(datetime_to_millis) != item.machine_state.due_at_utc_ms {
        return Err(WorkGraphError::Store(format!(
            "work item {} due_at projection does not match machine state",
            item.id
        )));
    }
    if item.not_before.map(datetime_to_millis) != item.machine_state.not_before_utc_ms {
        return Err(WorkGraphError::Store(format!(
            "work item {} not_before projection does not match machine state",
            item.id
        )));
    }
    if item.snoozed_until.map(datetime_to_millis) != item.machine_state.snoozed_until_utc_ms {
        return Err(WorkGraphError::Store(format!(
            "work item {} snoozed_until projection does not match machine state",
            item.id
        )));
    }
    if item.completion_policy
        != crate::types::WorkCompletionPolicy::from_machine(
            item.machine_state.completion_policy,
            item.machine_state.completion_supervisor_owner_key.clone(),
            item.machine_state.completion_reviewer_quorum_threshold,
        )
    {
        return Err(WorkGraphError::Store(format!(
            "work item {} completion_policy projection does not match machine state",
            item.id
        )));
    }
    if item.terminal_at.map(datetime_to_millis) != item.machine_state.terminal_at_utc_ms {
        return Err(WorkGraphError::Store(format!(
            "work item {} terminal_at projection does not match machine state",
            item.id
        )));
    }
    if let Some(claim) = &item.claim {
        let claim_owner_key = work_owner_key(&claim.owner)?;
        if item.machine_state.claim_owner_key.as_ref() != Some(&claim_owner_key) {
            return Err(WorkGraphError::Store(format!(
                "work item {} claim owner projection does not match machine state",
                item.id
            )));
        }
        if item.machine_state.claimed_at_utc_ms != Some(datetime_to_millis(claim.claimed_at)) {
            return Err(WorkGraphError::Store(format!(
                "work item {} claim time projection does not match machine state",
                item.id
            )));
        }
        if item.machine_state.lease_expires_at_utc_ms
            != claim.lease_expires_at.map(datetime_to_millis)
        {
            return Err(WorkGraphError::Store(format!(
                "work item {} claim lease projection does not match machine state",
                item.id
            )));
        }
    } else if item.machine_state.claim_owner_key.is_some()
        || item.machine_state.claimed_at_utc_ms.is_some()
        || item.machine_state.lease_expires_at_utc_ms.is_some()
    {
        return Err(WorkGraphError::Store(format!(
            "work item {} machine state has a claim without a claim projection",
            item.id
        )));
    }
    Ok(())
}

fn work_owner_key(owner: &crate::types::WorkOwner) -> Result<wg_dsl::WorkOwnerKey, WorkGraphError> {
    let kind = match owner.key.kind {
        crate::types::WorkOwnerKind::Principal => wg_dsl::WorkOwnerKind::Principal,
        crate::types::WorkOwnerKind::Agent => wg_dsl::WorkOwnerKind::Agent,
        crate::types::WorkOwnerKind::Session => wg_dsl::WorkOwnerKind::Session,
        crate::types::WorkOwnerKind::Mob => wg_dsl::WorkOwnerKind::Mob,
        crate::types::WorkOwnerKind::Label => wg_dsl::WorkOwnerKind::Label,
    };
    Ok(wg_dsl::WorkOwnerKey {
        kind,
        id: owner.key.id.clone(),
    })
}

fn topology_state(
    existing_items: &[WorkItem],
    existing_edges: &[WorkEdge],
) -> WorkGraphMachineState {
    WorkGraphMachineState {
        topology_item_keys: existing_items
            .iter()
            .map(|item| work_item_key(&item.id))
            .collect(),
        topology_edge_keys: existing_edges
            .iter()
            .map(|edge| work_edge_key(edge.kind, &edge.from_id, &edge.to_id))
            .collect(),
        blocks_reachability: dependency_reachability(existing_edges, WorkEdgeKind::Blocks),
        parent_reachability: dependency_reachability(existing_edges, WorkEdgeKind::Parent),
        ..Default::default()
    }
}

fn dependency_reachability(
    edges: &[WorkEdge],
    kind: WorkEdgeKind,
) -> BTreeSet<wg_dsl::WorkDependencyPathKey> {
    let mut adjacency = BTreeMap::<WorkItemId, BTreeSet<WorkItemId>>::new();
    for edge in edges.iter().filter(|edge| edge.kind == kind) {
        adjacency
            .entry(edge.from_id.clone())
            .or_default()
            .insert(edge.to_id.clone());
    }

    let mut reachability = BTreeSet::new();
    for start in adjacency.keys() {
        let mut stack = adjacency
            .get(start)
            .into_iter()
            .flat_map(|targets| targets.iter().cloned())
            .collect::<Vec<_>>();
        let mut seen = BTreeSet::new();
        while let Some(current) = stack.pop() {
            if !seen.insert(current.clone()) {
                continue;
            }
            reachability.insert(dependency_path_key(kind, start, &current));
            if let Some(targets) = adjacency.get(&current) {
                stack.extend(targets.iter().cloned());
            }
        }
    }
    reachability
}

fn work_item_key(id: &WorkItemId) -> wg_dsl::WorkItemKey {
    wg_dsl::WorkItemKey(id.as_str().to_string())
}

fn work_edge_key(
    kind: WorkEdgeKind,
    from_id: &WorkItemId,
    to_id: &WorkItemId,
) -> wg_dsl::WorkEdgeKey {
    wg_dsl::WorkEdgeKey(format!(
        "{}:{}:{}",
        edge_kind_key(kind),
        from_id.as_str(),
        to_id.as_str()
    ))
}

fn dependency_path_key(
    kind: WorkEdgeKind,
    from_id: &WorkItemId,
    to_id: &WorkItemId,
) -> wg_dsl::WorkDependencyPathKey {
    wg_dsl::WorkDependencyPathKey(format!(
        "{}:{}:{}",
        edge_kind_key(kind),
        from_id.as_str(),
        to_id.as_str()
    ))
}

fn dsl_edge_kind(kind: WorkEdgeKind) -> wg_dsl::WorkEdgeKind {
    match kind {
        WorkEdgeKind::Blocks => wg_dsl::WorkEdgeKind::Blocks,
        WorkEdgeKind::Parent => wg_dsl::WorkEdgeKind::Parent,
        WorkEdgeKind::Related => wg_dsl::WorkEdgeKind::Related,
        WorkEdgeKind::Supersedes => wg_dsl::WorkEdgeKind::Supersedes,
        WorkEdgeKind::DerivedFrom => wg_dsl::WorkEdgeKind::DerivedFrom,
    }
}

fn edge_kind_key(kind: WorkEdgeKind) -> &'static str {
    match kind {
        WorkEdgeKind::Blocks => "blocks",
        WorkEdgeKind::Parent => "parent",
        WorkEdgeKind::Related => "related",
        WorkEdgeKind::Supersedes => "supersedes",
        WorkEdgeKind::DerivedFrom => "derived_from",
    }
}

fn datetime_to_millis(dt: DateTime<Utc>) -> u64 {
    u64::try_from(dt.timestamp_millis()).unwrap_or(0)
}

fn millis_to_datetime(ms: u64) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp_millis(i64::try_from(ms).ok()?)
}

fn item_event(
    item: &WorkItem,
    kind: WorkGraphEventKind,
    at: DateTime<Utc>,
) -> Result<WorkGraphEvent, WorkGraphError> {
    Ok(WorkGraphEvent::item(
        item.realm_id.clone(),
        item.namespace.clone(),
        item.id.clone(),
        kind,
        at,
        json!({ "item": item }),
    ))
}

fn seconds_to_duration(seconds: u64) -> Duration {
    let seconds = i64::try_from(seconds).unwrap_or(i64::MAX);
    Duration::seconds(seconds)
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::redundant_clone
)]
mod tests {
    use super::*;
    use crate::types::{
        AddEvidenceRequest, ClaimWorkItemRequest, CloseWorkItemRequest, UpdateWorkItemRequest,
        WorkEvidenceKind, WorkEvidenceRef, WorkOwner, WorkOwnerKey,
    };

    fn create(title: &str, now: DateTime<Utc>) -> WorkItem {
        create_with_policy(title, WorkCompletionPolicy::SelfAttest, now)
    }

    fn create_with_policy(
        title: &str,
        completion_policy: WorkCompletionPolicy,
        now: DateTime<Utc>,
    ) -> WorkItem {
        WorkGraphMachine::create_item(
            CreateWorkItemRequest {
                realm_id: None,
                namespace: None,
                title: title.to_string(),
                description: None,
                priority: Default::default(),
                completion_policy,
                labels: BTreeSet::new(),
                due_at: None,
                not_before: None,
                snoozed_until: None,
                external_refs: Vec::new(),
                evidence_refs: Vec::new(),
                status: None,
            },
            "realm".to_string(),
            WorkNamespace::default(),
            now,
        )
        .expect("create")
        .0
    }

    fn owner(id: &str) -> WorkOwner {
        WorkOwner::new(WorkOwnerKey::label(id).expect("owner key"))
    }

    fn create_with_status(
        status: Option<WorkStatus>,
        now: DateTime<Utc>,
    ) -> Result<WorkItem, WorkGraphError> {
        WorkGraphMachine::create_item(
            CreateWorkItemRequest {
                realm_id: None,
                namespace: None,
                title: "status-create".to_string(),
                description: None,
                priority: Default::default(),
                completion_policy: WorkCompletionPolicy::SelfAttest,
                labels: BTreeSet::new(),
                due_at: None,
                not_before: None,
                snoozed_until: None,
                external_refs: Vec::new(),
                evidence_refs: Vec::new(),
                status,
            },
            "realm".to_string(),
            WorkNamespace::default(),
            now,
        )
        .map(|(item, _)| item)
    }

    #[test]
    fn create_status_admission_is_decided_by_machine() {
        use wg_dsl::WorkCreateStatusAdmissionKind as Admission;
        // The machine owns the "only open or blocked" creation policy.
        assert_eq!(
            classify_create_status_admission(WorkStatus::Open).expect("classify open"),
            Admission::AdmittedOpen
        );
        assert_eq!(
            classify_create_status_admission(WorkStatus::Blocked).expect("classify blocked"),
            Admission::AdmittedBlocked
        );
        for status in [
            WorkStatus::InProgress,
            WorkStatus::Completed,
            WorkStatus::Cancelled,
            WorkStatus::Failed,
        ] {
            assert_eq!(
                classify_create_status_admission(status)
                    .unwrap_or_else(|error| panic!("classify {status:?}: {error:?}")),
                Admission::Denied,
                "requested status {status:?} must be denied as a creation state"
            );
        }
    }

    #[test]
    fn create_item_admits_open_and_blocked_and_rejects_the_rest() {
        let now = Utc::now();
        assert_eq!(
            create_with_status(Some(WorkStatus::Open), now)
                .expect("open admitted")
                .status,
            WorkStatus::Open
        );
        assert_eq!(
            create_with_status(Some(WorkStatus::Blocked), now)
                .expect("blocked admitted")
                .status,
            WorkStatus::Blocked
        );
        // Default (None) resolves to Open and must be admitted.
        assert_eq!(
            create_with_status(None, now)
                .expect("default admitted")
                .status,
            WorkStatus::Open
        );
        for status in [
            WorkStatus::InProgress,
            WorkStatus::Completed,
            WorkStatus::Cancelled,
            WorkStatus::Failed,
        ] {
            let error = create_with_status(Some(status), now)
                .expect_err("non-open/blocked create status must be rejected");
            match error {
                WorkGraphError::InvalidTransition(message) => assert_eq!(
                    message, "new work items may only start open or blocked",
                    "rejection message preserved for {status:?}"
                ),
                other => panic!("expected InvalidTransition for {status:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn public_confirmation_admission_is_decided_by_machine() {
        use wg_dsl::WorkPublicConfirmationAdmissionKind as Admission;
        // The machine owns the trust-scoped eligibility: only SelfAttest is
        // publicly confirmable; every other policy requires trusted host.
        assert_eq!(
            WorkGraphMachine::classify_public_confirmation_admission(
                &WorkCompletionPolicy::SelfAttest
            )
            .expect("self-attest admission"),
            Admission::Admitted
        );
        let owner_key = WorkOwnerKey::label("supervisor").expect("owner key");
        let denied = [
            WorkCompletionPolicy::HostConfirmed,
            WorkCompletionPolicy::PrincipalConfirmed,
            WorkCompletionPolicy::Supervisor {
                owner_key: owner_key.clone(),
            },
            WorkCompletionPolicy::ReviewerQuorum { threshold: 2 },
        ];
        for policy in denied {
            assert_eq!(
                WorkGraphMachine::classify_public_confirmation_admission(&policy)
                    .unwrap_or_else(|error| panic!("classify {policy:?}: {error:?}")),
                Admission::DeniedRequiresTrustedHost,
                "policy {policy:?} must require trusted host for public confirmation"
            );
        }
    }

    #[test]
    fn create_completion_policy_admission_is_decided_by_machine() {
        use wg_dsl::WorkCreateCompletionPolicyAdmissionKind as Admission;
        // The machine owns the "non-goal work items must use self_attest"
        // creation policy.
        assert_eq!(
            WorkGraphMachine::classify_create_completion_policy_admission(
                &WorkCompletionPolicy::SelfAttest
            )
            .expect("self-attest admission"),
            Admission::Admitted
        );
        let owner_key = WorkOwnerKey::label("supervisor").expect("owner key");
        let denied = [
            WorkCompletionPolicy::HostConfirmed,
            WorkCompletionPolicy::PrincipalConfirmed,
            WorkCompletionPolicy::Supervisor { owner_key },
            WorkCompletionPolicy::ReviewerQuorum { threshold: 2 },
        ];
        for policy in denied {
            assert_eq!(
                WorkGraphMachine::classify_create_completion_policy_admission(&policy)
                    .unwrap_or_else(|error| panic!("classify {policy:?}: {error:?}")),
                Admission::DeniedNonSelfAttest,
                "policy {policy:?} must be denied at create for a non-goal work item"
            );
        }
    }

    #[test]
    fn close_status_admission_is_decided_by_machine() {
        use wg_dsl::WorkCloseStatusAdmissionKind as Admission;
        // The machine owns the "close requires a terminal status" lifecycle
        // class fact.
        assert_eq!(
            classify_close_status_admission(WorkStatus::Completed).expect("classify completed"),
            Admission::AdmittedCompleted
        );
        assert_eq!(
            classify_close_status_admission(WorkStatus::Cancelled).expect("classify cancelled"),
            Admission::AdmittedCancelled
        );
        assert_eq!(
            classify_close_status_admission(WorkStatus::Failed).expect("classify failed"),
            Admission::AdmittedFailed
        );
        for status in [
            WorkStatus::Open,
            WorkStatus::InProgress,
            WorkStatus::Blocked,
        ] {
            assert_eq!(
                classify_close_status_admission(status)
                    .unwrap_or_else(|error| panic!("classify {status:?}: {error:?}")),
                Admission::DeniedNonTerminal,
                "requested close status {status:?} must be denied as a non-terminal target"
            );
        }
    }

    #[test]
    fn close_item_rejects_non_terminal_status_with_preserved_message() {
        let now = Utc::now();
        for status in [
            WorkStatus::Open,
            WorkStatus::InProgress,
            WorkStatus::Blocked,
        ] {
            let item = create("close-target", now);
            let error = WorkGraphMachine::close_item(
                item.clone(),
                CloseWorkItemRequest {
                    id: item.id.clone(),
                    realm_id: None,
                    namespace: None,
                    expected_revision: item.revision,
                    status,
                },
                now,
            )
            .expect_err("non-terminal close status must be rejected");
            match error {
                WorkGraphError::InvalidTransition(message) => assert_eq!(
                    message, "close requires a terminal status",
                    "rejection message preserved for {status:?}"
                ),
                other => panic!("expected InvalidTransition for {status:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn blocked_items_are_never_ready() {
        let now = Utc::now();
        let item = create("blocked", now);
        let (item, _) = WorkGraphMachine::block_item(item, 1, now).expect("block");
        assert!(WorkGraphMachine::ready_items(vec![item], now).is_empty());
    }

    #[test]
    fn future_due_items_are_not_ready() {
        let now = Utc::now();
        let item = create("future", now);
        let (item, _) = WorkGraphMachine::update_item(
            item,
            UpdateWorkItemRequest {
                id: WorkItemId::generated(),
                realm_id: None,
                namespace: None,
                expected_revision: 1,
                title: None,
                description: None,
                priority: None,
                completion_policy: None,
                labels: None,
                due_at: Some(now + Duration::hours(1)),
                not_before: None,
                snoozed_until: None,
                external_refs: Vec::new(),
            },
            now,
        )
        .expect("update due");

        assert!(WorkGraphMachine::ready_items(vec![item], now).is_empty());
    }

    #[test]
    fn readiness_is_decided_by_machine_matching_claim_guards() {
        let now = Utc::now();

        // A fresh open item with no blockers and no time windows is ready —
        // exactly what the `ClaimOpen` guard accepts.
        let open = create("ready-open", now);
        assert_eq!(open.status, WorkStatus::Open);
        assert!(
            WorkGraphMachine::classify_readiness(&open, now).expect("classify open"),
            "an unblocked, due-eligible open item must be machine-classified ready"
        );
        assert!(WorkGraphMachine::is_ready(&open, now));

        // A future-due open item is not ready (mirrors `due_eligible` guard).
        let (future, _) = WorkGraphMachine::update_item(
            open,
            UpdateWorkItemRequest {
                id: WorkItemId::generated(),
                realm_id: None,
                namespace: None,
                expected_revision: 1,
                title: None,
                description: None,
                priority: None,
                completion_policy: None,
                labels: None,
                due_at: Some(now + Duration::hours(1)),
                not_before: None,
                snoozed_until: None,
                external_refs: Vec::new(),
            },
            now,
        )
        .expect("update future due");
        assert!(
            !WorkGraphMachine::classify_readiness(&future, now).expect("classify future"),
            "a future-due open item must be machine-classified not ready"
        );
        assert!(!WorkGraphMachine::is_ready(&future, now));

        // A claimed (InProgress) item with a live lease is NOT ready; once the
        // lease expires it becomes ready (mirrors `ClaimExpiredInProgress`).
        let claimable = create("reclaim", now);
        let (claimed, _) = WorkGraphMachine::claim_item(
            claimable,
            ClaimWorkItemRequest {
                id: WorkItemId::generated(),
                realm_id: None,
                namespace: None,
                expected_revision: 1,
                owner: owner("worker"),
                lease_seconds: Some(30),
                lease_expires_at: None,
            },
            now,
        )
        .expect("claim");
        assert_eq!(claimed.status, WorkStatus::InProgress);
        assert!(
            !WorkGraphMachine::classify_readiness(&claimed, now).expect("classify live lease"),
            "an in-progress item with a live lease must not be machine-classified ready"
        );
        let after_lease = now + Duration::seconds(31);
        assert!(
            WorkGraphMachine::classify_readiness(&claimed, after_lease)
                .expect("classify expired lease"),
            "an in-progress item with an expired lease must be machine-classified ready"
        );
        assert!(WorkGraphMachine::is_ready(&claimed, after_lease));
    }

    #[test]
    fn terminal_items_cannot_be_claimed() {
        let now = Utc::now();
        let item = create("done", now);
        let (item, _) = WorkGraphMachine::close_item(
            item,
            CloseWorkItemRequest {
                id: WorkItemId::generated(),
                realm_id: None,
                namespace: None,
                expected_revision: 1,
                status: WorkStatus::Completed,
            },
            now,
        )
        .expect("close");
        let error = WorkGraphMachine::claim_item(
            item,
            ClaimWorkItemRequest {
                id: WorkItemId::generated(),
                realm_id: None,
                namespace: None,
                expected_revision: 2,
                owner: owner("worker"),
                lease_seconds: None,
                lease_expires_at: None,
            },
            now,
        )
        .expect_err("terminal claim should fail");
        assert!(matches!(error, WorkGraphError::InvalidTransition(_)));
    }

    #[test]
    fn completed_close_is_completion_policy_gated_by_machine() {
        let now = Utc::now();
        let item = create_with_policy(
            "needs host confirmation",
            WorkCompletionPolicy::HostConfirmed,
            now,
        );

        // The machine's completion_policy_satisfied guard refuses the
        // CloseCompleted transition while no host confirmation evidence has
        // been recorded.
        let error = WorkGraphMachine::close_item(
            item.clone(),
            CloseWorkItemRequest {
                id: item.id.clone(),
                realm_id: None,
                namespace: None,
                expected_revision: 1,
                status: WorkStatus::Completed,
            },
            now,
        )
        .expect_err("machine must reject completed close without policy evidence");
        assert!(matches!(error, WorkGraphError::InvalidTransition(_)));

        // Once typed host-confirmation evidence is recorded, the machine's
        // owned host_confirmation_count satisfies the policy and the close
        // transition is admitted.
        let (item, _) = WorkGraphMachine::add_evidence(
            item,
            AddEvidenceRequest {
                id: WorkItemId::generated(),
                realm_id: None,
                namespace: None,
                expected_revision: 1,
                evidence: WorkEvidenceRef {
                    kind: "host_confirmation".to_string(),
                    id: "acceptance".to_string(),
                    label: None,
                    summary: None,
                    confirmation_kind: Some(WorkEvidenceKind::HostConfirmation),
                    confirming_owner_key: None,
                },
            },
            now,
        )
        .expect("record host confirmation evidence");

        let (closed, _) = WorkGraphMachine::close_item(
            item.clone(),
            CloseWorkItemRequest {
                id: item.id,
                realm_id: None,
                namespace: None,
                expected_revision: item.revision,
                status: WorkStatus::Completed,
            },
            now,
        )
        .expect("machine admits close once host confirmation is satisfied");
        assert_eq!(closed.status, WorkStatus::Completed);
    }

    #[test]
    fn reviewer_quorum_close_requires_distinct_reviewers() {
        let now = Utc::now();
        let item = create_with_policy(
            "needs two reviewers",
            WorkCompletionPolicy::ReviewerQuorum { threshold: 2 },
            now,
        );

        let reviewer_evidence = |reviewer: &str| WorkEvidenceRef {
            kind: "reviewer_confirmation".to_string(),
            id: reviewer.to_string(),
            label: Some(reviewer.to_string()),
            summary: None,
            confirmation_kind: Some(WorkEvidenceKind::ReviewerConfirmation),
            confirming_owner_key: Some(
                WorkOwnerKey::principal(reviewer).expect("reviewer principal"),
            ),
        };

        // One distinct reviewer is short of the quorum: machine refuses close.
        let (item, _) = WorkGraphMachine::add_evidence(
            item,
            AddEvidenceRequest {
                id: WorkItemId::generated(),
                realm_id: None,
                namespace: None,
                expected_revision: 1,
                evidence: reviewer_evidence("alice"),
            },
            now,
        )
        .expect("record first reviewer");

        let error = WorkGraphMachine::close_item(
            item.clone(),
            CloseWorkItemRequest {
                id: item.id.clone(),
                realm_id: None,
                namespace: None,
                expected_revision: item.revision,
                status: WorkStatus::Completed,
            },
            now,
        )
        .expect_err("single reviewer must not satisfy a quorum of two");
        assert!(matches!(error, WorkGraphError::InvalidTransition(_)));

        // A duplicate confirmation from the same reviewer does not advance the
        // distinct-reviewer count; the machine still refuses.
        let expected_revision = item.revision;
        let (item, _) = WorkGraphMachine::add_evidence(
            item,
            AddEvidenceRequest {
                id: WorkItemId::generated(),
                realm_id: None,
                namespace: None,
                expected_revision,
                evidence: reviewer_evidence("alice"),
            },
            now,
        )
        .expect("record duplicate reviewer");

        let error = WorkGraphMachine::close_item(
            item.clone(),
            CloseWorkItemRequest {
                id: item.id.clone(),
                realm_id: None,
                namespace: None,
                expected_revision: item.revision,
                status: WorkStatus::Completed,
            },
            now,
        )
        .expect_err("duplicate reviewer must not satisfy a quorum of two");
        assert!(matches!(error, WorkGraphError::InvalidTransition(_)));

        // A second distinct reviewer reaches the quorum; machine admits close.
        let expected_revision = item.revision;
        let (item, _) = WorkGraphMachine::add_evidence(
            item,
            AddEvidenceRequest {
                id: WorkItemId::generated(),
                realm_id: None,
                namespace: None,
                expected_revision,
                evidence: reviewer_evidence("bob"),
            },
            now,
        )
        .expect("record second reviewer");

        let (closed, _) = WorkGraphMachine::close_item(
            item.clone(),
            CloseWorkItemRequest {
                id: item.id,
                realm_id: None,
                namespace: None,
                expected_revision: item.revision,
                status: WorkStatus::Completed,
            },
            now,
        )
        .expect("two distinct reviewers satisfy the quorum");
        assert_eq!(closed.status, WorkStatus::Completed);
    }

    #[test]
    fn stale_revisions_fail() {
        let now = Utc::now();
        let item = create("stale", now);
        let error =
            WorkGraphMachine::block_item(item, 7, now).expect_err("stale transition should fail");
        assert!(matches!(error, WorkGraphError::StaleRevision { .. }));
    }

    #[test]
    fn public_error_class_is_machine_owned() {
        use crate::types::{WorkAttentionBindingId, WorkNamespace};

        let cases: &[(WorkGraphError, WorkGraphPublicErrorClass)] = &[
            (
                WorkGraphError::not_found(
                    "realm".to_string(),
                    WorkNamespace::default(),
                    WorkItemId::generated(),
                ),
                WorkGraphPublicErrorClass::NotFound,
            ),
            (
                WorkGraphError::attention_not_found(
                    "realm".to_string(),
                    WorkNamespace::default(),
                    WorkAttentionBindingId::generated(),
                ),
                WorkGraphPublicErrorClass::NotFound,
            ),
            (
                WorkGraphError::StaleRevision {
                    id: WorkItemId::generated(),
                    expected: 1,
                    actual: 2,
                },
                WorkGraphPublicErrorClass::Conflict,
            ),
            (
                WorkGraphError::Conflict("conflict".to_string()),
                WorkGraphPublicErrorClass::Conflict,
            ),
            (
                WorkGraphError::InvalidTransition("bad".to_string()),
                WorkGraphPublicErrorClass::InvalidTransition,
            ),
            (
                WorkGraphError::InvalidInput("bad".to_string()),
                WorkGraphPublicErrorClass::InvalidArguments,
            ),
            (
                WorkGraphError::InvalidTimestampMillis {
                    field: "due_at",
                    millis: -1,
                },
                WorkGraphPublicErrorClass::InvalidArguments,
            ),
            (
                WorkGraphError::Store("store".to_string()),
                WorkGraphPublicErrorClass::StoreError,
            ),
            (
                WorkGraphError::UnsupportedBackend("backend".to_string()),
                WorkGraphPublicErrorClass::CapabilityUnavailable,
            ),
        ];

        for (error, expected) in cases {
            let class = WorkGraphMachine::public_error_class(error)
                .expect("machine must classify every WorkGraphError variant");
            assert_eq!(class, *expected, "unexpected public class for {error:?}");
        }
    }

    #[test]
    fn only_one_active_claim_can_exist() {
        let now = Utc::now();
        let item = create("claim", now);
        let (claimed, _) = WorkGraphMachine::claim_item(
            item,
            ClaimWorkItemRequest {
                id: WorkItemId::generated(),
                realm_id: None,
                namespace: None,
                expected_revision: 1,
                owner: owner("worker"),
                lease_seconds: Some(60),
                lease_expires_at: None,
            },
            now,
        )
        .expect("claim");
        let error = WorkGraphMachine::claim_item(
            claimed,
            ClaimWorkItemRequest {
                id: WorkItemId::generated(),
                realm_id: None,
                namespace: None,
                expected_revision: 2,
                owner: owner("worker-2"),
                lease_seconds: Some(60),
                lease_expires_at: None,
            },
            now,
        )
        .expect_err("double claim should fail");
        assert!(matches!(error, WorkGraphError::InvalidTransition(_)));
    }

    #[test]
    fn validate_item_projection_only_rejects_never_derives() {
        let now = Utc::now();

        // A freshly machine-built item agrees with its machine_state authority.
        let clean = create("projection-guard", now);
        WorkGraphMachine::validate_item_projection(&clean)
            .expect("a machine-built projection must agree with its machine state");

        // Tampering the projected status away from the machine-owned lifecycle
        // phase is rejected — the guard does not silently re-derive `status`
        // from the machine state, it fails closed on the drift.
        let mut status_drift = clean.clone();
        status_drift.status = WorkStatus::Completed;
        let err = WorkGraphMachine::validate_item_projection(&status_drift)
            .expect_err("status projection drift must be rejected, never repaired");
        assert!(
            matches!(&err, WorkGraphError::Store(message) if message.contains("status projection")),
            "rejection must cite the status projection drift, got: {err:?}"
        );

        // Likewise for the revision projection: the machine_state.revision is
        // canonical, and a divergent projected revision is rejected rather than
        // synthesized back into agreement.
        let mut revision_drift = clean.clone();
        revision_drift.revision = revision_drift.revision.wrapping_add(1);
        let err = WorkGraphMachine::validate_item_projection(&revision_drift)
            .expect_err("revision projection drift must be rejected, never repaired");
        assert!(
            matches!(&err, WorkGraphError::Store(message) if message.contains("revision projection")),
            "rejection must cite the revision projection drift, got: {err:?}"
        );
    }
}

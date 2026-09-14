use super::*;
use crate::identifiers::LogicalRuntimeId;
use crate::input::Input;
use crate::input_state::{InputLifecycleState, InputStatePersistenceRecord};
use crate::live_grant::LiveExecutionGrantRecord;
use crate::live_request::LiveExecutionRequestRecord;
use crate::live_source::{LiveSourceDisposition, LiveSourceEntryRecord};
use crate::store::{MachineLifecycleCommit, MachineLifecycleObservation};
use crate::traits::RuntimeDriverError;
use meerkat_core::execution_scope::{
    RunEffectScopeFormat, RunEffectScopeId, RunEffectScopeRecord, ScopedEffectBudget,
    ScopedRunPolicyRecord,
};
use meerkat_core::live_execution::activation::{LiveExecutorSelector, LiveToolRestriction};
use meerkat_core::ops::ToolAccessPolicy;

#[derive(Debug, thiserror::Error)]
pub(crate) enum LiveInputStageError {
    #[error("{0}")]
    Uncommitted(#[from] RuntimeDriverError),
    #[error("{0}")]
    CommitUncertain(RuntimeDriverError),
}

struct PreparedLiveInputStage {
    commit: PreparedLiveLedgerCommit,
    fence: Arc<CurrentLiveRequestFence>,
    scope_id: RunEffectScopeId,
    scope: RunEffectScopeRecord,
    scope_revision: std::num::NonZeroU64,
}

impl LiveRequestStoreOwner {
    pub(crate) async fn commit_input_stage(
        &self,
        input: InputStatePersistenceRecord,
        lifecycle: MachineLifecycleCommit,
        binding: LiveAdmissionRuntimeBinding,
    ) -> Result<meerkat_core::execution_scope::ScopedRunAuthority, LiveInputStageError> {
        let prepared = self.prepare_input_stage(input, lifecycle, binding).await?;
        let ops = self.store.live_ledger_ops().ok_or_else(|| {
            stage_error("Live staging store capability disappeared before commit")
        })?;
        let expected_head = prepared.commit.successor().reference.clone();
        let outcome = ops
            .commit_live_ledger(prepared.commit, prepared.fence)
            .await
            .map_err(|error| LiveInputStageError::CommitUncertain(stage_error(error)))?;
        match outcome {
            LiveLedgerCommitOutcome::Committed { head } if head == expected_head => {
                seal_committed_scope(prepared.scope_id, prepared.scope, prepared.scope_revision)
                    .map_err(|error| LiveInputStageError::CommitUncertain(stage_error(error)))
            }
            outcome @ (LiveLedgerCommitOutcome::Conflict { .. }
            | LiveLedgerCommitOutcome::ActorConflict { .. }
            | LiveLedgerCommitOutcome::SourceConflict { .. }) => {
                Err(LiveInputStageError::Uncommitted(stage_error(format!(
                    "joint Live stage refused without commit: {outcome:?}"
                ))))
            }
            outcome => Err(LiveInputStageError::CommitUncertain(stage_error(format!(
                "joint Live stage returned unexpected committed authority: {outcome:?}"
            )))),
        }
    }

    async fn prepare_input_stage(
        &self,
        input: InputStatePersistenceRecord,
        lifecycle: MachineLifecycleCommit,
        binding: LiveAdmissionRuntimeBinding,
    ) -> Result<PreparedLiveInputStage, RuntimeDriverError> {
        let fail = |error: String| RuntimeDriverError::ValidationFailed { reason: error };
        let ops = self
            .store
            .live_ledger_ops()
            .filter(|ops| ops.ledger_write_profile().supports_input_staging())
            .ok_or_else(|| fail("store does not support joint Live run staging".into()))?;
        let bundle = input.as_stored();
        let Some(Input::LiveRequest(request_input)) = &bundle.state.persisted_input else {
            return Err(fail(
                "Live staging requires its original typed input".into(),
            ));
        };
        let run_id = bundle
            .seed
            .last_run_id
            .as_ref()
            .ok_or_else(|| fail("Live staged input has no generated run binding".into()))?;
        let key = bundle
            .state
            .idempotency_key
            .as_ref()
            .ok_or_else(|| fail("Live staged input has no source index".into()))?;
        let runtime_id = LogicalRuntimeId::for_session(&self.session_id);
        let (previous, input_version) = self
            .store
            .load_input_state_by_idempotency_key(&runtime_id, key)
            .await
            .map_err(stage_error)?
            .ok_or_else(|| fail("Live staged input lost its durable predecessor".into()))?
            .into_parts();
        if previous.state.input_id != bundle.state.input_id
            || previous.seed.phase != InputLifecycleState::Queued
            || previous.seed.last_run_id.is_some()
            || serde_json::to_vec(&previous.state.persisted_input).map_err(stage_error)?
                != serde_json::to_vec(&bundle.state.persisted_input).map_err(stage_error)?
        {
            return Err(fail("Live input is not the exact queued admission".into()));
        }
        let (provenance, source_row) = request_input.request.source_reference();
        let source = ops
            .lookup_live_source(provenance.source())
            .await
            .map_err(stage_error)?
            .ok_or_else(|| fail("Live staged input lost its source".into()))?;
        let LiveSourceEntryRecord::Reservation { record } = source.record().map_err(stage_error)?
        else {
            return Err(fail("Live staged source has no reservation".into()));
        };
        let LiveSourceDisposition::Admitted { receipt } = record.disposition() else {
            return Err(fail("Live staged source is not admitted".into()));
        };
        if record.request_id() != provenance.request_id()
            || record.frozen_digest().map_err(stage_error)? != *source_row
        {
            return Err(fail("Live staged source and input disagree".into()));
        }
        let head = ops
            .load_live_head(&self.session_id)
            .await
            .map_err(stage_error)?
            .ok_or_else(|| fail("Live staged input has no request owner".into()))?;
        head.validate_payload().map_err(stage_error)?;
        let state = crate::generated::live_request_state::decode(&head.payload.request_snapshot)
            .map_err(stage_error)?;
        let (admission_commit, callback_continuation) = match &request_input.request {
            LiveExecutionRequestRecord::LiveRequest { .. } => {
                if receipt.input_id() != &bundle.state.input_id {
                    return Err(fail("original Live admission input changed".into()));
                }
                (receipt.admission_commit(), None)
            }
            LiveExecutionRequestRecord::CallbackContinuation {
                continuation,
                admission_commit,
                ..
            } => {
                if bundle.state.runtime_semantics.is_none_or(|semantics| {
                    semantics.execution_kind()
                        != meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending
                        || semantics.boundary()
                            != meerkat_core::lifecycle::RunApplyBoundary::RunStart
                }) {
                    return Err(fail(
                        "callback input lost its generated resume semantics".into(),
                    ));
                }
                request_input
                    .request
                    .validate_continuation_binding(&state, &bundle.state.input_id)
                    .map_err(stage_error)?;
                if key
                    != &super::admission::continuation_input_key(
                        provenance.request_id(),
                        &continuation.target,
                    )?
                {
                    return Err(fail("callback continuation input index changed".into()));
                }
                (admission_commit, Some(continuation.clone()))
            }
        };
        let request_id = record.request_id().to_string();
        let grant: LiveExecutionGrantRecord<()> = serde_json::from_str(
            state
                .request_grant_records
                .get(&request_id)
                .ok_or_else(|| fail("Live request has no retained grant".into()))?,
        )
        .map_err(stage_error)?;
        if !matches!(
            grant.executor().selector,
            LiveExecutorSelector::Session { .. }
        ) || grant.grant_ref() != *receipt.grant()
            || grant.executor().binding != *receipt.executor()
        {
            return Err(fail(
                "Live stage lacks its exact session-owned grant".into(),
            ));
        }
        let observed = self
            .store
            .observe_machine_lifecycle(&runtime_id)
            .await
            .map_err(stage_error)?;
        let MachineLifecycleObservation::Decoded {
            record: observed,
            version,
        } = observed
        else {
            return Err(fail("Live stage requires decoded current lifecycle".into()));
        };
        if observed.binding() != lifecycle.snapshot().binding()
            || observed.binding().runtime_generation()
                != Some(receipt.executor().binding_generation)
            || observed.binding().runtime_epoch_id()
                != Some(receipt.executor().runtime_epoch.to_string().as_str())
        {
            return Err(fail("Live stage runtime binding changed".into()));
        }
        let permission = &grant.declaration().permission;
        let tool_access = match &permission.tools {
            LiveToolRestriction::AllowListed { names } => {
                ToolAccessPolicy::AllowList(names.clone())
            }
            LiveToolRestriction::Unrestricted {} => ToolAccessPolicy::DenyList(Default::default()),
        };
        let remaining = state
            .remaining_effects
            .get(&request_id)
            .copied()
            .ok_or_else(|| fail("Live stage has no remaining-effect authority".into()))?;
        let scope_id = RunEffectScopeId::from_uuid(uuid::Uuid::new_v4());
        let scope = RunEffectScopeRecord {
            format: RunEffectScopeFormat::V1,
            request_id: record.request_id().clone(),
            grant: receipt.grant().clone(),
            executor: receipt.executor().clone(),
            input_id: bundle.state.input_id.clone(),
            run_id: run_id.clone(),
            admission_commit: admission_commit.clone(),
            parent_scope: None,
            callback_continuation,
            policy: ScopedRunPolicyRecord::new(tool_access, permission.allowed_mutations.clone())
                .map_err(stage_error)?,
            remaining: ScopedEffectBudget {
                model_computations: remaining,
                tool_dispatches: remaining,
                descendant_admissions: remaining,
            },
        };
        if state
            .request_parents
            .get(&request_id)
            .is_none_or(|parent| !parent.is_empty())
        {
            return Err(fail(
                "root Live stage cannot discard descendant scope lineage".into(),
            ));
        }
        let owner =
            dsl::LiveRequestMachineAuthority::recover_from_state(state).map_err(stage_error)?;
        let encoded_scope = serde_json::to_string(&scope).map_err(stage_error)?;
        let encoded_admission = serde_json::to_string(admission_commit).map_err(stage_error)?;
        let executor = serde_json::to_string(receipt.executor()).map_err(stage_error)?;
        let profile_revision =
            serde_json::to_string(&grant.declaration().profile_revision).map_err(stage_error)?;
        let mut command = match &scope.callback_continuation {
            None => dsl::LiveRequestInput::Stage {
                request_id: request_id.clone(),
                input_id: scope.input_id.to_string(),
                admission_commit: encoded_admission,
                run_id: run_id.to_string(),
                scope_id: scope_id.as_uuid().to_string(),
                scope_record: encoded_scope,
                executor,
                profile_revision,
                now: 0,
            },
            Some(continuation) => {
                let digest: sha2::digest::Output<sha2::Sha256> = continuation.results_digest.into();
                dsl::LiveRequestInput::StageCallbackContinuation {
                    request_id: request_id.clone(),
                    previous_run_id: continuation.target.run_id().to_string(),
                    input_id: scope.input_id.to_string(),
                    admission_commit: encoded_admission,
                    result_digest: format!("{digest:x}"),
                    run_id: run_id.to_string(),
                    scope_id: scope_id.as_uuid().to_string(),
                    scope_record: encoded_scope,
                    executor,
                    profile_revision,
                    now: 0,
                }
            }
        };
        refresh_command_time(&mut command, self.clock.as_ref()).map_err(stage_error)?;
        let mut candidate = owner.prepare_authority();
        let transition = dsl::LiveRequestMachineMutator::apply(&mut candidate, command.clone())
            .map_err(stage_error)?;
        if !matches!(transition.effects(), [dsl::LiveRequestEffect::RunScopeBound {
            request_id: request, run_id: run, scope_id: scope,
        }] if request == &request_id && run == &run_id.to_string()
            && scope == &scope_id.as_uuid().to_string())
        {
            return Err(fail(
                "generated Live stage did not bind its exact run scope".into(),
            ));
        }
        let native_fence = binding.fence(
            bundle
                .state
                .persisted_input
                .as_ref()
                .ok_or_else(|| fail("Live input missing".into()))?,
            receipt.executor(),
        )?;
        let prepared = PreparedLiveLedgerCommit::from_request_transition(
            &self.session_id,
            Some(&head),
            &candidate,
        )
        .map_err(stage_error)?
        .with_input_stage(
            input.with_expected_row_digest(input_version),
            source,
            lifecycle,
            version,
        )
        .map_err(stage_error)?;
        let fence = Arc::new(CurrentLiveRequestFence {
            time: LiveRequestTimeFence {
                predecessor: owner,
                input: command,
                expected_snapshot: Arc::clone(&prepared.successor().payload.request_snapshot),
                clock: Arc::clone(&self.clock),
            },
            registration: native_fence,
        });
        let expected_head = prepared.successor().reference.clone();
        let scope_revision = std::num::NonZeroU64::new(expected_head.revision)
            .ok_or_else(|| stage_error("scope commit revision is zero"))?;
        Ok(PreparedLiveInputStage {
            commit: prepared,
            fence,
            scope_id,
            scope,
            scope_revision,
        })
    }
}

struct LiveRequestScopeBridgeToken;
static LIVE_REQUEST_SCOPE_BRIDGE_TOKEN: LiveRequestScopeBridgeToken = LiveRequestScopeBridgeToken;

#[doc(hidden)]
#[allow(improper_ctypes_definitions, unsafe_code)]
#[unsafe(export_name = concat!(
    "__meerkat_runtime_generated_authority_bridge_token_is_valid_v1_live_request_scope_",
    env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")
))]
pub extern "Rust" fn live_request_scope_bridge_token_is_valid(
    token: &(dyn std::any::Any + Send + Sync),
) -> bool {
    token.is::<LiveRequestScopeBridgeToken>()
}

pub(super) fn seal_committed_scope(
    scope_id: RunEffectScopeId,
    record: RunEffectScopeRecord,
    scope_revision: std::num::NonZeroU64,
) -> Result<meerkat_core::execution_scope::ScopedRunAuthority, String> {
    #[allow(improper_ctypes_definitions, unsafe_code)]
    unsafe extern "Rust" {
        #[link_name = concat!(
            "__meerkat_core_runtime_generated_live_request_scope_build_v1_",
            env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")
        )]
        fn core_runtime_generated_live_request_scope_build(
            token: &'static (dyn std::any::Any + Send + Sync),
            scope_id: RunEffectScopeId,
            record: RunEffectScopeRecord,
            scope_revision: std::num::NonZeroU64,
        ) -> Result<meerkat_core::execution_scope::ScopedRunAuthority, String>;
    }
    #[allow(unsafe_code)]
    unsafe {
        core_runtime_generated_live_request_scope_build(
            &LIVE_REQUEST_SCOPE_BRIDGE_TOKEN,
            scope_id,
            record,
            scope_revision,
        )
    }
}

pub(super) fn seal_committed_effect(
    scope: &meerkat_core::execution_scope::ScopedRunAuthority,
    claim: meerkat_core::execution_scope::ScopedEffectClaimRecord<
        meerkat_core::execution_scope::ScopedEffectTarget,
    >,
) -> Result<
    meerkat_core::execution_scope::ScopedEffectStartPermit<
        meerkat_core::execution_scope::ScopedEffectTarget,
    >,
    String,
> {
    use meerkat_core::execution_scope::{
        ScopedEffectClaimRecord, ScopedEffectStartPermit, ScopedEffectTarget, ScopedRunAuthority,
    };
    #[allow(improper_ctypes_definitions, unsafe_code)]
    unsafe extern "Rust" {
        #[link_name = concat!(
            "__meerkat_core_runtime_generated_live_request_effect_build_v1_",
            env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")
        )]
        fn core_runtime_generated_live_request_effect_build(
            token: &'static (dyn std::any::Any + Send + Sync),
            scope: &ScopedRunAuthority,
            claim: ScopedEffectClaimRecord<ScopedEffectTarget>,
        ) -> Result<ScopedEffectStartPermit<ScopedEffectTarget>, String>;
    }
    #[allow(unsafe_code)]
    unsafe {
        core_runtime_generated_live_request_effect_build(
            &LIVE_REQUEST_SCOPE_BRIDGE_TOKEN,
            scope,
            claim,
        )
    }
}

fn stage_error(error: impl std::fmt::Display) -> RuntimeDriverError {
    RuntimeDriverError::Internal(format!("joint Live input staging: {error}"))
}

pub(super) fn seal_committed_callback_application(
    scope: meerkat_core::execution_scope::ScopedRunAuthority,
    revision: std::num::NonZeroU64,
) -> Result<meerkat_core::execution_scope::ScopedCallbackApplicationPermit, String> {
    use meerkat_core::execution_scope::{ScopedCallbackApplicationPermit, ScopedRunAuthority};
    #[allow(improper_ctypes_definitions, unsafe_code)]
    unsafe extern "Rust" {
        #[link_name = concat!(
            "__meerkat_core_runtime_generated_live_callback_application_build_v1_",
            env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")
        )]
        fn core_runtime_generated_live_callback_application_build(
            token: &'static (dyn std::any::Any + Send + Sync),
            scope: ScopedRunAuthority,
            revision: std::num::NonZeroU64,
        ) -> Result<ScopedCallbackApplicationPermit, String>;
    }
    #[allow(unsafe_code)]
    unsafe {
        core_runtime_generated_live_callback_application_build(
            &LIVE_REQUEST_SCOPE_BRIDGE_TOKEN,
            scope,
            revision,
        )
    }
}

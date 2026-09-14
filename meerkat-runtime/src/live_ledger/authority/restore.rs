use super::*;
use crate::identifiers::LogicalRuntimeId;
use crate::input::{Input, InputDurability, InputOrigin};
use crate::input_state::InputLifecycleState;
use crate::live_grant::LiveExecutionGrantRecord;
use crate::live_request::LiveExecutionRequestRecord;
use crate::live_source::{LiveSourceDisposition, LiveSourceEntryRecord};
use crate::store::MachineLifecycleObservation;
use meerkat_core::execution_scope::{RunEffectScopeId, RunEffectScopeRecord, ScopedRunAuthority};
use meerkat_core::live_execution::activation::LiveExecutorSelector;
use meerkat_core::live_execution::request::LiveSourceKey;

pub(in crate::live_ledger) struct PreparedLiveScopeRestoration {
    scope_id: RunEffectScopeId,
    scope: RunEffectScopeRecord,
    commit: PreparedLiveLedgerCommit,
    time: LiveRequestTimeFence,
}

pub(super) struct ObservedLiveRun {
    pub owner: dsl::LiveRequestMachineAuthority,
    pub head: crate::live_ledger::write::LiveLedgerStoredHead,
    pub scope_id: RunEffectScopeId,
    pub scope: RunEffectScopeRecord,
    pub input: crate::store::ExactInputStateObservation,
    pub lifecycle: MachineLifecycleObservationVersion,
    pub profile_revision: String,
}

fn validate_callback_scope_link(
    state: &dsl::LiveRequestMachineState,
    scope_id: RunEffectScopeId,
    scope: &RunEffectScopeRecord,
) -> Result<(), LiveRequestAuthorityError> {
    let invalid = LiveRequestAuthorityError::ScopeNotCurrent;
    scope
        .validate_callback_continuation(scope_id)
        .map_err(invalid)?;
    let predecessor = state
        .run_predecessors
        .get(&scope.run_id.to_string())
        .ok_or_else(|| invalid("scope run lost its predecessor"))?;
    match (predecessor.is_empty(), scope.callback_continuation.as_ref()) {
        (true, None) => Ok(()),
        (true, Some(_)) => Err(invalid("root run cannot carry a callback continuation")),
        (false, None) => Err(invalid("continuation run lost its exact callback target")),
        (false, Some(continuation)) => {
            let target = &continuation.target;
            let digest: sha2::digest::Output<sha2::Sha256> = continuation.results_digest.into();
            if target.run_id().to_string() != *predecessor
                || state.run_scopes.get(predecessor)
                    != target
                        .execution_scope()
                        .map(|id| id.as_uuid().to_string())
                        .as_ref()
                || state.run_callback_records.get(predecessor)
                    != Some(&serde_json::to_string(target)?)
                || state.run_continuation_result_digests.get(predecessor)
                    != Some(&format!("{digest:x}"))
            {
                return Err(invalid(
                    "scope callback target differs from its committed predecessor",
                ));
            }
            Ok(())
        }
    }
}

impl LiveRequestStoreOwner {
    pub(crate) async fn resolve_model_attempt(
        &self,
        scope: &ScopedRunAuthority,
        request_id: &meerkat_core::ops::OperationId,
    ) -> Result<
        meerkat_core::execution_scope::ScopedModelAttemptResolution,
        LiveRequestAuthorityError,
    > {
        let observed = self
            .observe_run_scope(scope.scope_id(), scope.record().clone())
            .await?;
        let chain_id =
            meerkat_core::execution_scope::model_attempt_chain_id(scope.scope_id(), request_id)
                .to_string();
        let mut candidate = observed.owner.prepare_authority();
        let transition = dsl::LiveRequestMachineMutator::apply(
            &mut candidate,
            dsl::LiveRequestInput::ResolveModelAttempt {
                request_id: scope.record().request_id.to_string(),
                run_id: scope.record().run_id.to_string(),
                scope_id: scope.scope_id().as_uuid().to_string(),
                chain_id: chain_id.clone(),
                now: (self.clock)()?,
            },
        )?;
        if let [dsl::LiveRequestEffect::ModelTokenBudgetExhausted { used, limit }] =
            transition.effects()
        {
            return Ok(
                meerkat_core::execution_scope::ScopedModelAttemptResolution::TokenBudgetExhausted {
                    used: *used,
                    limit: *limit,
                },
            );
        }
        let [
            dsl::LiveRequestEffect::ModelAttemptResolved {
                chain_id: resolved,
                attempt,
            },
        ] = transition.effects()
        else {
            return Err(LiveRequestAuthorityError::ScopeNotCurrent(
                "missing generated model attempt quotation",
            ));
        };
        if resolved != &chain_id {
            return Err(LiveRequestAuthorityError::ScopeNotCurrent(
                "model attempt quotation has a different lineage",
            ));
        }
        u32::try_from(*attempt)
            .map(
                |ordinal| meerkat_core::execution_scope::ScopedModelAttemptResolution::Ready {
                    ordinal,
                },
            )
            .map_err(|_| {
                LiveRequestAuthorityError::ScopeNotCurrent("model attempt ordinal overflow")
            })
    }

    pub(crate) async fn restore_run_scope(
        &self,
        scope_id: RunEffectScopeId,
        scope: RunEffectScopeRecord,
    ) -> Result<ScopedRunAuthority, LiveRequestAuthorityError> {
        let prepared = self.prepare_scope_restoration(scope_id, scope).await?;
        self.commit_scope_restoration(prepared).await
    }

    pub(super) async fn observe_run_scope(
        &self,
        scope_id: RunEffectScopeId,
        scope: RunEffectScopeRecord,
    ) -> Result<ObservedLiveRun, LiveRequestAuthorityError> {
        let invalid = LiveRequestAuthorityError::ScopeNotCurrent;
        let ops = self
            .store
            .live_ledger_ops()
            .filter(|ops| ops.ledger_write_profile().supports_execution_fence())
            .ok_or(LiveRequestAuthorityError::Unsupported)?;
        if scope.executor.session_id != self.session_id {
            return Err(LiveRequestAuthorityError::SessionMismatch);
        }
        if scope.parent_scope.is_some() {
            return Err(invalid(
                "descendant restoration requires its canonical owner",
            ));
        }
        let head = ops
            .load_live_head(&self.session_id)
            .await?
            .ok_or_else(|| invalid("missing Live request owner"))?;
        head.validate_payload()?;
        let state = crate::generated::live_request_state::decode(&head.payload.request_snapshot)?;
        let request_id = scope.request_id.to_string();
        validate_callback_scope_link(&state, scope_id, &scope)?;
        let source: LiveSourceKey = serde_json::from_str(
            state
                .request_sources
                .get(&request_id)
                .ok_or_else(|| invalid("missing retained source identity"))?,
        )?;
        if source.session_id() != &self.session_id {
            return Err(invalid("retained source belongs to another session"));
        }
        let key = admission::run_input_key(
            &state,
            &self.session_id,
            &scope.request_id,
            &scope.run_id.to_string(),
        )
        .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
        let runtime_id = LogicalRuntimeId::for_session(&self.session_id);
        let input = self
            .store
            .load_input_state_by_idempotency_key(&runtime_id, &key)
            .await?
            .ok_or_else(|| invalid("missing durable input"))?;
        let bundle = input.state();
        let Some(Input::LiveRequest(original)) = &bundle.state.persisted_input else {
            return Err(invalid("input is not an original Live request"));
        };
        if bundle.state.input_id != scope.input_id
            || bundle.seed.phase != InputLifecycleState::Staged
            || bundle.seed.last_run_id.as_ref() != Some(&scope.run_id)
            || bundle.seed.terminal_outcome.is_some()
            || bundle.state.durability != Some(InputDurability::Durable)
            || original.header.id != scope.input_id
            || original.header.source != InputOrigin::LiveRequest
            || original.header.durability != InputDurability::Durable
            || original.header.idempotency_key.as_ref() != Some(&key)
            || bundle.state.idempotency_key.as_ref() != Some(&key)
        {
            return Err(invalid("input does not retain the exact staged run"));
        }
        let (provenance, source_row) = original.request.source_reference();
        let row = ops
            .lookup_live_source(&source)
            .await?
            .ok_or_else(|| invalid("missing committed source receipt"))?;
        let LiveSourceEntryRecord::Reservation { record } = row.record()? else {
            return Err(invalid("source is not a request reservation"));
        };
        let LiveSourceDisposition::Admitted { receipt } = record.disposition() else {
            return Err(invalid("source is not admitted"));
        };
        let evidence = record
            .frozen_request()
            .ok_or_else(|| invalid("source has no frozen request"))?;
        match &original.request {
            LiveExecutionRequestRecord::LiveRequest { .. } => {
                if receipt.input_id() != &scope.input_id
                    || receipt.admission_commit() != &scope.admission_commit
                    || scope.callback_continuation.is_some()
                {
                    return Err(invalid("original scope admission changed"));
                }
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
                    return Err(invalid(
                        "callback input lost its generated resume semantics",
                    ));
                }
                original
                    .request
                    .validate_continuation_binding(&state, &scope.input_id)
                    .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
                if scope.callback_continuation.as_ref() != Some(continuation)
                    || &scope.admission_commit != admission_commit
                {
                    return Err(invalid("callback scope admission changed"));
                }
            }
        }
        if provenance.source() != &source
            || provenance.request_id() != &scope.request_id
            || record.request_id() != &scope.request_id
            || record
                .frozen_digest()
                .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?
                != *source_row
            || receipt.executor() != &scope.executor
            || receipt.grant() != &scope.grant
            || provenance.evidence_kind() != evidence.kind()
            || provenance.request_digest() != evidence.request().digest()
        {
            return Err(invalid(
                "scope, input provenance and committed source disagree",
            ));
        }
        let grant: LiveExecutionGrantRecord<()> = serde_json::from_str(
            state
                .request_grant_records
                .get(&request_id)
                .ok_or_else(|| invalid("missing retained grant"))?,
        )?;
        if !matches!(
            grant.executor().selector,
            LiveExecutorSelector::Session { .. }
        ) || grant.grant_ref() != scope.grant
            || grant.executor().binding != scope.executor
        {
            return Err(invalid("scope does not retain its exact session grant"));
        }
        let MachineLifecycleObservation::Decoded {
            record: lifecycle,
            version,
        } = self.store.observe_machine_lifecycle(&runtime_id).await?
        else {
            return Err(invalid("runtime lifecycle cannot be decoded"));
        };
        if lifecycle.runtime_state() != Some(crate::RuntimeState::Running)
            || lifecycle.run().current_run_id() != Some(&scope.run_id)
            || lifecycle.run().pre_run_phase().is_none()
            || lifecycle.binding().runtime_generation() != Some(scope.executor.binding_generation)
            || lifecycle.binding().runtime_epoch_id()
                != Some(scope.executor.runtime_epoch.to_string().as_str())
            || lifecycle.binding().agent_runtime_id().is_none()
            || lifecycle.binding().fence_token().is_none()
        {
            return Err(invalid("executor no longer owns this exact durable run"));
        }
        let owner = dsl::LiveRequestMachineAuthority::recover_from_state(state)?;
        Ok(ObservedLiveRun {
            owner,
            head,
            scope_id,
            scope,
            input,
            lifecycle: version,
            profile_revision: serde_json::to_string(&grant.declaration().profile_revision)?,
        })
    }

    pub(in crate::live_ledger) async fn prepare_scope_restoration(
        &self,
        scope_id: RunEffectScopeId,
        scope: RunEffectScopeRecord,
    ) -> Result<PreparedLiveScopeRestoration, LiveRequestAuthorityError> {
        let ObservedLiveRun {
            owner,
            head,
            scope_id,
            scope,
            input,
            lifecycle,
            profile_revision,
        } = self.observe_run_scope(scope_id, scope).await?;
        let invalid = LiveRequestAuthorityError::ScopeNotCurrent;
        let request_id = scope.request_id.to_string();
        let mut command = dsl::LiveRequestInput::RestoreScope {
            request_id: request_id.clone(),
            input_id: scope.input_id.to_string(),
            admission_commit: serde_json::to_string(&scope.admission_commit)?,
            run_id: scope.run_id.to_string(),
            scope_id: scope_id.as_uuid().to_string(),
            scope_record: serde_json::to_string(&scope)?,
            parent_scope: String::new(),
            executor: serde_json::to_string(&scope.executor)?,
            profile_revision,
            now: 0,
        };
        refresh_command_time(&mut command, self.clock.as_ref())?;
        let mut candidate = owner.prepare_authority();
        let transition = dsl::LiveRequestMachineMutator::apply(&mut candidate, command.clone())?;
        if !matches!(transition.effects(), [dsl::LiveRequestEffect::ScopeRestored {
            request_id: request, run_id: run, scope_id: restored,
        }] if request == &request_id && run == &scope.run_id.to_string() && restored == &scope_id.as_uuid().to_string())
        {
            return Err(invalid(
                "generated restoration did not bind the exact scope",
            ));
        }
        let commit = PreparedLiveLedgerCommit::from_request_transition(
            &self.session_id,
            Some(&head),
            &candidate,
        )?
        .with_execution_fence(&input, lifecycle)?;
        let time = LiveRequestTimeFence {
            predecessor: owner,
            input: command,
            expected_snapshot: Arc::clone(&commit.successor().payload.request_snapshot),
            clock: Arc::clone(&self.clock),
        };
        Ok(PreparedLiveScopeRestoration {
            scope_id,
            scope,
            commit,
            time,
        })
    }

    pub(in crate::live_ledger) async fn commit_scope_restoration(
        &self,
        prepared: PreparedLiveScopeRestoration,
    ) -> Result<ScopedRunAuthority, LiveRequestAuthorityError> {
        let ops = self
            .store
            .live_ledger_ops()
            .filter(|ops| ops.ledger_write_profile().supports_execution_fence())
            .ok_or(LiveRequestAuthorityError::Unsupported)?;
        if prepared.commit.session_id() != &self.session_id {
            return Err(LiveRequestAuthorityError::SessionMismatch);
        }
        let expected = prepared.commit.successor().reference.clone();
        let revision = std::num::NonZeroU64::new(expected.revision).ok_or(
            LiveRequestAuthorityError::ScopeNotCurrent("zero scope revision"),
        )?;
        let outcome = ops
            .commit_live_ledger(prepared.commit, Arc::new(prepared.time))
            .await?;
        if !matches!(&outcome, LiveLedgerCommitOutcome::Committed { head } if head == &expected) {
            return Err(LiveRequestAuthorityError::NotNewlyCommitted(Box::new(
                outcome,
            )));
        }
        stage::seal_committed_scope(prepared.scope_id, prepared.scope, revision)
            .map_err(|error| RuntimeStoreError::WriteFailed(error).into())
    }
}

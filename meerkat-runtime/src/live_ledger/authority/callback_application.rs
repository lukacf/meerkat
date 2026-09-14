use super::*;
use meerkat_core::execution_scope::{ScopedCallbackApplicationPermit, ScopedRunAuthority};
use std::num::NonZeroU64;

pub(in crate::live_ledger) struct PreparedLiveCallbackApplication {
    scope: ScopedRunAuthority,
    commit: PreparedLiveLedgerCommit,
    time: LiveRequestTimeFence,
    revision: NonZeroU64,
}

impl LiveRequestStoreOwner {
    pub(crate) async fn claim_callback_application(
        &self,
        scope: ScopedRunAuthority,
    ) -> Result<ScopedCallbackApplicationPermit, LiveRequestAuthorityError> {
        let prepared = self.prepare_callback_application(scope).await?;
        self.commit_callback_application(prepared).await
    }

    pub(in crate::live_ledger) async fn prepare_callback_application(
        &self,
        scope: ScopedRunAuthority,
    ) -> Result<PreparedLiveCallbackApplication, LiveRequestAuthorityError> {
        let continuation = scope.record().callback_continuation.as_ref().ok_or(
            LiveRequestAuthorityError::ScopeNotCurrent("run has no callback continuation"),
        )?;
        let callback_record = serde_json::to_string(&continuation.target)?;
        let digest: sha2::digest::Output<sha2::Sha256> = continuation.results_digest.into();
        let result_digest = format!("{digest:x}");
        let observed = self
            .observe_run_scope(scope.scope_id(), scope.record().clone())
            .await?;
        let request_id = scope.record().request_id.to_string();
        let run_id = scope.record().run_id.to_string();
        let scope_id = scope.scope_id().as_uuid().to_string();
        let command = dsl::LiveRequestInput::ClaimCallbackApplication {
            request_id: request_id.clone(),
            run_id: run_id.clone(),
            input_id: scope.record().input_id.to_string(),
            admission_commit: serde_json::to_string(&scope.record().admission_commit)?,
            scope_id: scope_id.clone(),
            scope_record: serde_json::to_string(scope.record())?,
            callback_record: callback_record.clone(),
            result_digest: result_digest.clone(),
            executor: serde_json::to_string(&scope.record().executor)?,
            profile_revision: observed.profile_revision,
            now: (self.clock)()?,
        };
        let mut candidate = observed.owner.prepare_authority();
        let transition = dsl::LiveRequestMachineMutator::apply(&mut candidate, command.clone())?;
        if !matches!(transition.effects(),
            [dsl::LiveRequestEffect::CallbackApplicationClaimed {
                request_id: request, run_id: run, scope_id: issued_scope,
                callback_record: target, result_digest: results,
            }] if request == &request_id && run == &run_id && issued_scope == &scope_id
                && target == &callback_record && results == &result_digest)
        {
            return Err(LiveRequestAuthorityError::ScopeNotCurrent(
                "generated callback claim did not identify the exact continuation",
            ));
        }
        let commit = PreparedLiveLedgerCommit::from_request_transition(
            &self.session_id,
            Some(&observed.head),
            &candidate,
        )?
        .with_execution_fence(&observed.input, observed.lifecycle)?;
        let revision = NonZeroU64::new(commit.successor().reference.revision).ok_or(
            LiveRequestAuthorityError::ScopeNotCurrent("callback claim has no commit revision"),
        )?;
        let time = LiveRequestTimeFence {
            predecessor: observed.owner,
            input: command,
            expected_snapshot: Arc::clone(&commit.successor().payload.request_snapshot),
            clock: Arc::clone(&self.clock),
        };
        Ok(PreparedLiveCallbackApplication {
            scope,
            commit,
            time,
            revision,
        })
    }

    pub(in crate::live_ledger) async fn commit_callback_application(
        &self,
        prepared: PreparedLiveCallbackApplication,
    ) -> Result<ScopedCallbackApplicationPermit, LiveRequestAuthorityError> {
        if prepared.commit.session_id() != &self.session_id {
            return Err(LiveRequestAuthorityError::SessionMismatch);
        }
        let ops = self
            .store
            .live_ledger_ops()
            .filter(|ops| ops.ledger_write_profile().supports_execution_fence())
            .ok_or(LiveRequestAuthorityError::Unsupported)?;
        let expected = prepared.commit.successor().reference.clone();
        let outcome = ops
            .commit_live_ledger(prepared.commit, Arc::new(prepared.time))
            .await?;
        if !matches!(&outcome, LiveLedgerCommitOutcome::Committed { head } if head == &expected) {
            return Err(LiveRequestAuthorityError::NotNewlyCommitted(Box::new(
                outcome,
            )));
        }
        stage::seal_committed_callback_application(prepared.scope, prepared.revision)
            .map_err(|error| RuntimeStoreError::WriteFailed(error).into())
    }
}

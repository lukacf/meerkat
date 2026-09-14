//! Trusted native-host entrypoint. Public profile selectors never carry this
//! declaration or the store's external write fence.

use super::*;
use crate::identifiers::LogicalRuntimeId;
use crate::live_ledger::authority::dsl::LiveRequestInput;
use crate::live_ledger::authority::store::{LiveRequestAuthorityError, LiveRequestStoreOwner};
use crate::live_ledger::write::LiveLedgerCommitOutcome;
use crate::store::{
    MachineLifecycleObservation, RuntimeStore, RuntimeStoreError, RuntimeStoreWriteFence,
};
use meerkat_core::live_execution::activation::LiveToolRestriction;

pub struct LiveGrantActivationRequest<Member> {
    pub activation_id: LiveActivationId,
    pub declaration: LiveActivationDeclaration<Member>,
    pub requesting_realm: RealmId,
    pub executor: LiveResolvedExecutorRecord<Member>,
}

/// Native hosts inject this store-backed issuer alongside their trusted
/// declaration source. Construction installs no activation or default grant.
pub struct LiveExecutionGrantIssuer {
    store: Arc<dyn RuntimeStore>,
}

impl LiveExecutionGrantIssuer {
    pub fn new(store: Arc<dyn RuntimeStore>) -> Self {
        Self { store }
    }

    /// The host resolves its exact session/member before calling. Both the
    /// observed lifecycle row and external registration fence are checked in
    /// the physical activation transaction; neither a decoded declaration nor
    /// a stale executor observation can produce the returned receipt.
    pub async fn activate<Member: PartialEq + Serialize>(
        &self,
        request: LiveGrantActivationRequest<Member>,
        write_fence: Arc<dyn RuntimeStoreWriteFence>,
    ) -> Result<LiveExecutionGrant<Member>, LiveGrantActivationError> {
        if self
            .store
            .live_ledger_ops()
            .is_none_or(|ops| !ops.ledger_write_profile().supports_lifecycle_fence())
        {
            return Err(LiveGrantActivationError::Unsupported);
        }
        let session_id = request.executor.binding.session_id.clone();
        let observation = self
            .store
            .observe_machine_lifecycle(&LogicalRuntimeId::for_session(&session_id))
            .await?;
        let MachineLifecycleObservation::Decoded {
            record: lifecycle,
            version,
        } = observation
        else {
            return Err(LiveGrantActivationError::ExecutorNotCurrent);
        };
        let binding = &request.executor.binding;
        if lifecycle.binding().runtime_epoch_id()
            != Some(binding.runtime_epoch.to_string().as_str())
            || lifecycle.binding().runtime_generation() != Some(binding.binding_generation)
            || lifecycle.binding().agent_runtime_id().is_none()
            || lifecycle.binding().fence_token().is_none()
            || !matches!(
                lifecycle.runtime_state(),
                Some(
                    crate::RuntimeState::Idle
                        | crate::RuntimeState::Attached
                        | crate::RuntimeState::Running
                )
            )
        {
            return Err(LiveGrantActivationError::ExecutorNotCurrent);
        }
        let record = LiveExecutionGrantRecord::new(
            ExecutionGrantId::from_uuid(uuid::Uuid::new_v4()),
            request.activation_id,
            request.declaration,
            request.requesting_realm,
            request.executor,
            Utc::now(),
        )?;
        let encoded = serde_json::to_string(&record)?;
        let grant = record.grant_ref();
        let grant_id = grant.id.as_uuid().to_string();
        let declaration = record.declaration();
        let limits = declaration.permission.limits;
        let (tools_restricted, tools) = match &declaration.permission.tools {
            LiveToolRestriction::AllowListed { names } => (
                true,
                names.iter().map(|name| name.as_str().to_owned()).collect(),
            ),
            LiveToolRestriction::Unrestricted {} => (false, Default::default()),
        };
        let now = u64::try_from(record.issued_at.timestamp_millis())
            .map_err(|_| LiveGrantActivationError::ClockOutOfRange)?;
        let expires_at = declaration
            .expires_at
            .map(|expires| u64::try_from(expires.timestamp_millis()))
            .transpose()
            .map_err(|_| LiveGrantActivationError::ClockOutOfRange)?
            .unwrap_or(u64::MAX);
        let input = LiveRequestInput::Activate {
            grant_id: grant_id.clone(),
            generation: grant.generation.get(),
            expires_at,
            executor: serde_json::to_string(&record.executor().binding)?,
            record: encoded.clone(),
            profile_revision: serde_json::to_string(&declaration.profile_revision)?,
            evidence: declaration.allowed_evidence.clone(),
            mutations: declaration.permission.allowed_mutations.clone(),
            tools_restricted,
            tools,
            max_requests: u64::from(limits.max_requests().get()),
            max_concurrent_requests: u64::from(limits.max_concurrent_requests().get()),
            max_effects: u64::from(limits.max_effects_per_request().get()),
            max_tokens: limits.max_tokens_per_request().get(),
            max_duration_ms: limits.max_duration_ms().get(),
            now,
        };
        let committed = LiveRequestStoreOwner::new(Arc::clone(&self.store), session_id)
            .commit_for_runtime(input, version, write_fence)
            .await?;
        let activation_commit =
            committed.into_activation_commit(&grant_id, grant.generation.get(), &encoded)?;
        Ok(LiveExecutionGrant {
            record: Arc::new(record),
            activation_commit,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LiveGrantActivationError {
    #[error("Live activation requires an exact current, bound runtime executor")]
    ExecutorNotCurrent,
    #[error("Live activation time is outside the supported clock domain")]
    ClockOutOfRange,
    #[error(transparent)]
    Record(#[from] LiveGrantRecordError),
    #[error("Live activation encoding failed: {0}")]
    Encoding(#[from] serde_json::Error),
    #[error(transparent)]
    Store(#[from] RuntimeStoreError),
    #[error(transparent)]
    RecoveryObservation(crate::RuntimeDriverError),
    #[error(transparent)]
    Transition(crate::live_ledger::authority::dsl::LiveRequestMachineTransitionError),
    #[error("Live activation requires independent Live ledger storage")]
    Unsupported,
    #[error("Live activation returned a mismatched committed identity")]
    CommitIdentityMismatch,
    #[error("Live activation did not acquire a new committed boundary: {0:?}")]
    CommitConflict(LiveLedgerCommitOutcome),
}

impl From<LiveRequestAuthorityError> for LiveGrantActivationError {
    fn from(error: LiveRequestAuthorityError) -> Self {
        match error {
            LiveRequestAuthorityError::Unsupported => Self::Unsupported,
            LiveRequestAuthorityError::ScopeNotCurrent(_) => Self::ExecutorNotCurrent,
            LiveRequestAuthorityError::InvalidEffectFeedback(detail)
            | LiveRequestAuthorityError::InvalidSourceCancellation(detail)
            | LiveRequestAuthorityError::InvalidOrdinaryCompletion(detail) => {
                Self::Store(RuntimeStoreError::Internal(format!(
                    "unexpected feedback error during activation: {detail}"
                )))
            }
            LiveRequestAuthorityError::SessionMismatch
            | LiveRequestAuthorityError::ActivationReceiptMismatch => Self::CommitIdentityMismatch,
            LiveRequestAuthorityError::Store(error) => Self::Store(error),
            LiveRequestAuthorityError::RecoveryObservation(error) => {
                Self::RecoveryObservation(error)
            }
            LiveRequestAuthorityError::Snapshot(error) => Self::Encoding(error),
            LiveRequestAuthorityError::Transition(error) => Self::Transition(error),
            LiveRequestAuthorityError::NotNewlyCommitted(outcome) => Self::CommitConflict(*outcome),
        }
    }
}

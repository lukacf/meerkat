use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

use super::{
    ScopedEffectCustody, ScopedEffectOutcome, ScopedEffectPolicyRevision, ScopedEffectSettlement,
    ScopedEffectTarget, ScopedExecutionContext,
};
use crate::{
    LoweredRequestProvenance, PolicyDigest, Provider, ProviderNativeToolPolicy, RunId,
    SessionLlmIdentity, ToolError, ops::OperationId,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ScopedModelEffectSupport {
    #[default]
    Unsupported,
    PhysicalDispatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopedModelAttemptResolution {
    Ready { ordinal: u32 },
    TokenBudgetExhausted { used: u64, limit: u64 },
}

#[derive(Debug, thiserror::Error)]
pub enum ScopedModelPreparationError {
    #[error(transparent)]
    Authority(#[from] ToolError),
    #[error("scoped observed token budget exhausted: {used} >= {limit}")]
    TokenBudgetExhausted { used: u64, limit: u64 },
}

impl ScopedModelPreparationError {
    pub(crate) fn into_agent_error(self) -> crate::AgentError {
        match self {
            Self::Authority(error) => crate::AgentError::ConfigError(error.to_string()),
            Self::TokenBudgetExhausted { used, limit } => {
                crate::AgentError::TokenBudgetExceeded { used, limit }
            }
        }
    }
}

impl ScopedModelAttemptResolution {
    fn ordinal(self) -> Result<u32, ScopedModelPreparationError> {
        match self {
            Self::Ready { ordinal } => Ok(ordinal),
            Self::TokenBudgetExhausted { used, limit } => {
                Err(ScopedModelPreparationError::TokenBudgetExhausted { used, limit })
            }
        }
    }
}

/// An immutable selected-model policy for one logical request. This is not
/// start permission; each physical send still needs a durable claim.
pub struct ScopedModelRequest {
    context: ScopedExecutionContext,
    request_id: OperationId,
    provider: Provider,
    model: String,
    policy_digest: PolicyDigest,
    physical_attempt: Arc<ModelPhysicalAttempt>,
    settled_attempt: Arc<Mutex<Option<SettledModelAttempt>>>,
}

struct ModelPhysicalAttempt {
    ordinal: u32,
    claim_entered: AtomicBool,
}

#[derive(Clone)]
struct SettledModelAttempt {
    physical_attempt: Arc<ModelPhysicalAttempt>,
    provider: Provider,
    model: String,
    policy_digest: PolicyDigest,
}

impl ScopedModelRequest {
    pub fn request_id(&self) -> &OperationId {
        &self.request_id
    }

    pub fn physical_attempt_ordinal(&self) -> u32 {
        self.physical_attempt.ordinal
    }

    /// Read a generated durable attempt quotation. This is not permission:
    /// claiming still compares the exact lineage against the current store head.
    pub async fn restore(
        context: ScopedExecutionContext,
        request_id: OperationId,
        identity: SessionLlmIdentity,
    ) -> Result<Self, ScopedModelPreparationError> {
        let mut request = Self::new(context, request_id, identity)?;
        let ordinal = request
            .context
            .host
            .resolve_model_attempt(request.context.scope().clone(), request.request_id.clone())
            .await?
            .ordinal()?;
        request.physical_attempt = Arc::new(ModelPhysicalAttempt {
            ordinal,
            claim_entered: AtomicBool::new(false),
        });
        Ok(request)
    }

    pub(crate) async fn for_actor_preparation(
        &self,
        identity: SessionLlmIdentity,
    ) -> Result<Arc<Self>, ScopedModelPreparationError> {
        let ordinal = self
            .context
            .host
            .resolve_model_attempt(self.context.scope().clone(), self.request_id.clone())
            .await?
            .ordinal()?;
        if ordinal != self.physical_attempt.ordinal {
            return Err(ToolError::execution_failed(
                "actor preparation has a stale model attempt quotation",
            )
            .into());
        }
        self.for_preparation(identity).map_err(Into::into)
    }

    pub fn new(
        context: ScopedExecutionContext,
        request_id: OperationId,
        identity: SessionLlmIdentity,
    ) -> Result<Self, ToolError> {
        if identity.model.trim().is_empty() {
            return Err(ToolError::execution_failed(
                "scoped model request requires a selected model",
            ));
        }
        let policy = serde_json::to_vec(&(
            "meerkat.scoped-model-policy.v1",
            &identity,
            context.scope().native_tools(),
        ))
        .map_err(|error| ToolError::execution_failed(error.to_string()))?;
        Ok(Self {
            context,
            request_id,
            provider: identity.provider,
            model: identity.model,
            policy_digest: PolicyDigest::from_canonical_bytes(&policy),
            physical_attempt: Arc::new(ModelPhysicalAttempt {
                ordinal: 0,
                claim_entered: AtomicBool::new(false),
            }),
            settled_attempt: Arc::new(Mutex::new(None)),
        })
    }

    /// A process-local handoff of committed conclusive feedback, not a restored
    /// ledger cursor. A successor already used by an adapter is not returned.
    pub fn settled_successor(&self) -> Result<Option<Arc<Self>>, ToolError> {
        let receipt = self.settled_attempt.lock().map_err(|error| {
            ToolError::execution_failed(format!("model completion handoff poisoned: {error}"))
        })?;
        let Some(receipt) = receipt.as_ref() else {
            return Ok(None);
        };
        if receipt
            .physical_attempt
            .claim_entered
            .load(Ordering::Acquire)
        {
            return Ok(None);
        }
        Ok(Some(Arc::new(Self {
            context: self.context.clone(),
            request_id: self.request_id.clone(),
            provider: receipt.provider,
            model: receipt.model.clone(),
            policy_digest: receipt.policy_digest.clone(),
            physical_attempt: Arc::clone(&receipt.physical_attempt),
            settled_attempt: Arc::clone(&self.settled_attempt),
        })))
    }

    /// Reprepare a committed conclusive attempt for the actor's selected fallback
    /// identity without changing its logical request or physical attempt.
    pub fn with_retry_identity(
        &self,
        identity: SessionLlmIdentity,
    ) -> Result<Arc<Self>, ToolError> {
        if self.physical_attempt.ordinal == 0 {
            return Err(ToolError::execution_failed(
                "model retry identity requires an unused committed completion successor",
            ));
        }
        self.for_preparation(identity)
    }

    pub(crate) fn matches_context(&self, context: &ScopedExecutionContext) -> bool {
        &self.context == context
    }

    pub(crate) fn for_preparation(
        &self,
        identity: SessionLlmIdentity,
    ) -> Result<Arc<Self>, ToolError> {
        if self.physical_attempt.claim_entered.load(Ordering::Acquire) {
            return Err(ToolError::execution_failed(
                "model preparation cannot replace an entered physical claim without conclusive committed feedback",
            ));
        }
        let mut request = Self::new(self.context.clone(), self.request_id.clone(), identity)?;
        request.physical_attempt = Arc::clone(&self.physical_attempt);
        request.settled_attempt = Arc::clone(&self.settled_attempt);
        Ok(Arc::new(request))
    }

    /// Called by the physical adapter after its awaited authorization and
    /// final native-tool validation. Only committed conclusive feedback can
    /// return a successor request; an unknown send has no retry successor.
    pub async fn claim_request(
        self: Arc<Self>,
        model: &str,
        route: &str,
        provenance: LoweredRequestProvenance,
        native_tools: ProviderNativeToolPolicy,
    ) -> Result<ScopedModelEffectCustody, ToolError> {
        if self.provider != provenance.provider
            || self.model != model
            || native_tools != self.context.scope().native_tools()
        {
            return Err(ToolError::execution_failed(
                "physical model request does not match its selected scoped policy",
            ));
        }
        self.physical_attempt
            .claim_entered
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| {
                ToolError::execution_failed("physical model request has already entered its claim")
            })?;
        let scope = self.context.scope();
        let identity = serde_json::to_vec(&(
            "meerkat.scoped-model-effect.v1",
            scope.scope_id(),
            &scope.record().run_id,
            &self.request_id,
            self.physical_attempt.ordinal,
        ))
        .map_err(|error| ToolError::execution_failed(error.to_string()))?;
        let effect_id = OperationId(uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, &identity));
        let invocation = serde_json::to_vec(&(
            "meerkat.scoped-model-invocation.v1",
            &effect_id,
            model,
            route,
            provenance,
        ))
        .map_err(|error| ToolError::execution_failed(error.to_string()))?;
        let target = ScopedEffectTarget::ModelComputation {
            request_id: self.request_id.clone(),
            attempt: self.physical_attempt.ordinal,
            invocation_digest: Sha256::digest(invocation).into(),
        };
        let evaluation = EvaluatedModelRequestPolicy {
            run_id: scope.record().run_id.clone(),
            target: target.clone(),
            policy_digest: self.policy_digest.clone(),
        };
        let permit = self
            .context
            .host
            .claim_model_effect(scope.clone(), effect_id.clone(), evaluation)
            .await?;
        let custody = ScopedEffectCustody::from_permit(
            Arc::clone(&self.context.host),
            permit,
            &effect_id,
            scope,
            &target,
        )?;
        Ok(ScopedModelEffectCustody {
            custody,
            request: self,
        })
    }
}

pub struct ScopedModelEffectCustody {
    custody: ScopedEffectCustody,
    request: Arc<ScopedModelRequest>,
}

enum ConclusiveModelOutcome {
    Completed,
    Rejected,
}

impl ScopedModelEffectCustody {
    pub fn observe_usage(&mut self, usage: &crate::TurnUsage) {
        self.custody.token_accounting = super::ScopedEffectTokenAccounting::from_turn_usage(
            usage,
            self.request.provider,
            &self.request.model,
        );
    }

    pub fn begin_invocation(&mut self) -> Result<(), ToolError> {
        self.custody.begin_invocation()
    }

    pub fn settle(self, outcome: ScopedEffectOutcome) -> ScopedEffectSettlement {
        self.custody.settle(outcome)
    }

    /// The physical adapter observed an explicit rejection, not a transport or
    /// parser failure. Persist it before permitting the next physical attempt.
    pub async fn settle_rejected(self) -> Result<Arc<ScopedModelRequest>, ToolError> {
        self.settle_conclusive(ConclusiveModelOutcome::Rejected)
            .await
            .and_then(|successor| {
                successor.ok_or_else(|| {
                    ToolError::execution_failed(
                        "model physical attempt identity exhausted after committed rejection",
                    )
                })
            })
    }

    /// The adapter observed terminal success. Commit it before handing off an
    /// unused slot; the actor's generated retry policy still decides whether
    /// an empty response needs another request.
    pub async fn settle_completed(self) -> Result<(), ToolError> {
        self.settle_conclusive(ConclusiveModelOutcome::Completed)
            .await
            .map(|_| ())
    }

    async fn settle_conclusive(
        self,
        outcome: ConclusiveModelOutcome,
    ) -> Result<Option<Arc<ScopedModelRequest>>, ToolError> {
        if !self.custody.was_invoked() {
            return Err(ToolError::execution_failed(
                "an uninvoked model request cannot report provider completion",
            ));
        }
        self.custody
            .settle(match outcome {
                ConclusiveModelOutcome::Completed => ScopedEffectOutcome::Succeeded,
                ConclusiveModelOutcome::Rejected => ScopedEffectOutcome::Failed,
            })
            .await?;
        let Some(physical_attempt) = self.request.physical_attempt.ordinal.checked_add(1) else {
            return Ok(None);
        };
        let physical_attempt = Arc::new(ModelPhysicalAttempt {
            ordinal: physical_attempt,
            claim_entered: AtomicBool::new(false),
        });
        let successor = Arc::new(ScopedModelRequest {
            context: self.request.context.clone(),
            request_id: self.request.request_id.clone(),
            provider: self.request.provider,
            model: self.request.model.clone(),
            policy_digest: self.request.policy_digest.clone(),
            physical_attempt: Arc::clone(&physical_attempt),
            settled_attempt: Arc::clone(&self.request.settled_attempt),
        });
        let mut receipt = self.request.settled_attempt.lock().map_err(|error| {
            ToolError::execution_failed(format!("model completion handoff poisoned: {error}"))
        })?;
        if receipt
            .as_ref()
            .is_some_and(|prior| prior.physical_attempt.ordinal >= physical_attempt.ordinal)
        {
            return Err(ToolError::execution_failed(
                "model completion handoff would overwrite a newer committed successor",
            ));
        }
        *receipt = Some(SettledModelAttempt {
            physical_attempt,
            provider: successor.provider,
            model: successor.model.clone(),
            policy_digest: successor.policy_digest.clone(),
        });
        Ok(Some(successor))
    }
}

impl std::fmt::Debug for ScopedModelRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScopedModelRequest")
            .field("request_id", &self.request_id)
            .field("provider", &self.provider)
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

/// Produced only by the selected request's physical-policy check. It cannot
/// be deserialized or re-paired with a caller-supplied target or revision.
pub struct EvaluatedModelRequestPolicy {
    run_id: RunId,
    target: ScopedEffectTarget,
    policy_digest: PolicyDigest,
}

impl EvaluatedModelRequestPolicy {
    pub fn run_id(&self) -> &RunId {
        &self.run_id
    }

    pub fn target(&self) -> &ScopedEffectTarget {
        &self.target
    }

    pub fn revision(&self) -> ScopedEffectPolicyRevision {
        ScopedEffectPolicyRevision::Immutable {
            ordinary_policy: self.policy_digest.clone(),
        }
    }

    /// This owner has immutable request-local policy, not a managed policy
    /// revision. Route and credential fencing remain the physical adapter's job.
    pub fn publish_immutable<R>(&self, operation: impl FnOnce() -> R) -> R {
        operation()
    }
}

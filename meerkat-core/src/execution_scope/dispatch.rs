use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use super::{
    ScopedEffectClaimRecord, ScopedEffectNotStartedProof, ScopedEffectStartPermit,
    ScopedEffectTarget, ScopedRunAuthority,
};
use crate::{EvaluatedToolExecutionPolicy, ToolError};

#[cfg(not(target_arch = "wasm32"))]
pub type ScopedEffectSettlement =
    Pin<Box<dyn Future<Output = Result<(), ToolError>> + Send + 'static>>;
#[cfg(target_arch = "wasm32")]
pub type ScopedEffectSettlement = Pin<Box<dyn Future<Output = Result<(), ToolError>> + 'static>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopedEffectOutcome {
    Succeeded,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ScopedToolEffectSupport {
    #[default]
    Unsupported,
    /// This dispatcher rejects every call without starting work.
    RejectsAllCalls,
    /// Physical work retains its scoped context, including any descendants,
    /// but requires an outer claim owner.
    RequiresOuterClaim,
    PhysicalDispatch,
}

#[derive(Debug)]
pub enum ScopedEffectFeedback {
    NotStarted(ScopedEffectNotStartedProof<ScopedEffectTarget>),
    Observed {
        claim: ScopedEffectClaimRecord<ScopedEffectTarget>,
        outcome: ScopedEffectOutcome,
        token_accounting: super::ScopedEffectTokenAccounting,
    },
}

/// Runtime-owned claims and feedback. Record content cannot implement the
/// one-use claim return without the generated authority's sealed permit.
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
pub trait ScopedEffectHost: Send + Sync {
    async fn claim_callback_application(
        &self,
        _scope: ScopedRunAuthority,
    ) -> Result<super::ScopedCallbackApplicationPermit, ToolError> {
        Err(ToolError::execution_failed(
            "effect host does not implement scoped callback application claims",
        ))
    }

    async fn resolve_model_attempt(
        &self,
        _scope: ScopedRunAuthority,
        _request_id: crate::ops::OperationId,
    ) -> Result<super::ScopedModelAttemptResolution, ToolError> {
        Err(ToolError::execution_failed(
            "effect host does not implement durable model attempt resolution",
        ))
    }

    async fn claim_tool_effect(
        &self,
        scope: ScopedRunAuthority,
        effect_id: crate::ops::OperationId,
        evaluation: EvaluatedToolExecutionPolicy,
    ) -> Result<ScopedEffectStartPermit<ScopedEffectTarget>, ToolError>;

    async fn claim_model_effect(
        &self,
        _scope: ScopedRunAuthority,
        _effect_id: crate::ops::OperationId,
        _evaluation: super::EvaluatedModelRequestPolicy,
    ) -> Result<ScopedEffectStartPermit<ScopedEffectTarget>, ToolError> {
        Err(ToolError::execution_failed(
            "effect host does not implement scoped model claims",
        ))
    }

    /// Transfer feedback to owned runtime work synchronously. Dropping the
    /// returned observation future must not cancel that work. Implementations
    /// must surface persistence failures even when no observer remains.
    fn submit_feedback(&self, feedback: ScopedEffectFeedback) -> ScopedEffectSettlement;
}

#[derive(Clone)]
pub struct ScopedExecutionContext {
    scope: ScopedRunAuthority,
    pub(super) host: Arc<dyn ScopedEffectHost>,
    binding: ScopedExecutionBinding,
    invocation: ScopedInvocationIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
enum ScopedInvocationIdentity {
    HostRequest,
    TurnBoundary(crate::ops::OperationId),
}

fn tool_effect_id(
    scope_id: super::RunEffectScopeId,
    run_id: &crate::RunId,
    invocation: &ScopedInvocationIdentity,
    call_id: &str,
) -> Result<crate::ops::OperationId, serde_json::Error> {
    let identity = serde_json::to_vec(&(
        "meerkat.scoped-tool-effect.v2",
        scope_id,
        run_id,
        invocation,
        call_id,
    ))?;
    Ok(crate::ops::OperationId(uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_URL,
        &identity,
    )))
}

pub(crate) fn callback_tool_effect_id(
    scope_id: super::RunEffectScopeId,
    run_id: &crate::RunId,
    boundary: &crate::ops::OperationId,
    call_id: &str,
) -> Result<crate::ops::OperationId, serde_json::Error> {
    tool_effect_id(
        scope_id,
        run_id,
        &ScopedInvocationIdentity::TurnBoundary(boundary.clone()),
        call_id,
    )
}

#[derive(Clone)]
enum ScopedExecutionBinding {
    Host,
    SessionActor(Arc<crate::EpochCursorState>),
}

impl ScopedExecutionContext {
    pub async fn claim_callback_application(
        &self,
    ) -> Result<super::ScopedCallbackApplicationPermit, ToolError> {
        let permit = self
            .host
            .claim_callback_application(self.scope.clone())
            .await?;
        if permit.scope() != &self.scope {
            return Err(ToolError::execution_failed(
                "callback application claim belongs to another scope",
            ));
        }
        Ok(permit)
    }

    pub fn new(scope: ScopedRunAuthority, host: Arc<dyn ScopedEffectHost>) -> Self {
        Self {
            scope,
            host,
            binding: ScopedExecutionBinding::Host,
            invocation: ScopedInvocationIdentity::HostRequest,
        }
    }

    pub fn for_session_actor(
        scope: ScopedRunAuthority,
        host: Arc<dyn ScopedEffectHost>,
        bindings: &crate::SessionRuntimeBindings,
    ) -> Result<Self, super::ScopeRecordError> {
        if &scope.record().executor.runtime_epoch != bindings.epoch_id() {
            return Err(super::ScopeRecordError::ClaimScopeMismatch);
        }
        Ok(Self {
            scope,
            host,
            binding: ScopedExecutionBinding::SessionActor(Arc::clone(bindings.cursor_state())),
            invocation: ScopedInvocationIdentity::HostRequest,
        })
    }

    /// Physical actor attachment identity, not a currentness verdict. Every
    /// subsequent effect claim still checks the current native executor.
    pub fn matches_actor_cursor(&self, cursor: &Arc<crate::EpochCursorState>) -> bool {
        match &self.binding {
            ScopedExecutionBinding::Host => false,
            ScopedExecutionBinding::SessionActor(bound) => Arc::ptr_eq(bound, cursor),
        }
    }

    pub fn at_turn_boundary(
        &self,
        turn: &dyn crate::TurnStateHandle,
        cursor: &Arc<crate::EpochCursorState>,
    ) -> Result<Self, ToolError> {
        let snapshot = turn.snapshot();
        if !self.matches_actor_cursor(cursor)
            || snapshot.active_run_id.as_ref() != Some(&self.scope.record().run_id)
        {
            return Err(ToolError::execution_failed(
                super::ScopeRecordError::ClaimScopeMismatch.to_string(),
            ));
        }
        let encoded = serde_json::to_vec(&(
            "meerkat.scoped-turn-invocation.v1",
            &self.scope.record().run_id,
            snapshot.boundary_count,
            snapshot.extraction_active,
            snapshot.extraction_attempts,
        ))
        .map_err(|error| {
            ToolError::execution_failed(format!("scoped invocation identity: {error}"))
        })?;
        let mut context = self.clone();
        context.invocation = ScopedInvocationIdentity::TurnBoundary(crate::ops::OperationId(
            uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, &encoded),
        ));
        Ok(context)
    }

    pub fn model_request_id(&self) -> Result<&crate::ops::OperationId, super::ScopeRecordError> {
        match &self.invocation {
            ScopedInvocationIdentity::HostRequest => {
                Err(super::ScopeRecordError::ClaimScopeMismatch)
            }
            ScopedInvocationIdentity::TurnBoundary(id) => Ok(id),
        }
    }

    pub fn scope(&self) -> &ScopedRunAuthority {
        &self.scope
    }

    pub(crate) async fn claim(
        &self,
        evaluation: EvaluatedToolExecutionPolicy,
    ) -> Result<ScopedEffectCustody, ToolError> {
        let target = evaluation
            .target()
            .map_err(|error| ToolError::execution_failed(error.to_string()))?;
        // Repeating a claimed call must not mint another physical attempt,
        // including when the previous observation was lost.
        let effect_id = tool_effect_id(
            self.scope.scope_id(),
            evaluation.run_id(),
            &self.invocation,
            evaluation.call().id,
        )
        .map_err(|error| ToolError::execution_failed(error.to_string()))?;
        let permit = self
            .host
            .claim_tool_effect(self.scope.clone(), effect_id.clone(), evaluation)
            .await?;
        ScopedEffectCustody::from_permit(
            Arc::clone(&self.host),
            permit,
            &effect_id,
            &self.scope,
            &target,
        )
    }
}

impl std::fmt::Debug for ScopedExecutionContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScopedExecutionContext")
            .field("scope", &self.scope)
            .field("invocation", &self.invocation)
            .finish_non_exhaustive()
    }
}

impl PartialEq for ScopedExecutionContext {
    fn eq(&self, other: &Self) -> bool {
        self.scope == other.scope
            && Arc::ptr_eq(&self.host, &other.host)
            && self.invocation == other.invocation
            && match (&self.binding, &other.binding) {
                (ScopedExecutionBinding::Host, ScopedExecutionBinding::Host) => true,
                (
                    ScopedExecutionBinding::SessionActor(left),
                    ScopedExecutionBinding::SessionActor(right),
                ) => Arc::ptr_eq(left, right),
                _ => false,
            }
    }
}

impl Eq for ScopedExecutionContext {}

/// Per-run actor handoff, distinct from the persistable authority record.
/// A scoped handoff always carries an actual effect host.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum RunExecutionContext {
    #[default]
    SessionPolicy,
    Scoped(ScopedExecutionContext),
}

impl RunExecutionContext {
    pub const fn is_session_policy(&self) -> bool {
        matches!(self, Self::SessionPolicy)
    }

    pub fn authority(&self) -> super::RunExecutionAuthority {
        match self {
            Self::SessionPolicy => super::RunExecutionAuthority::SessionPolicy,
            Self::Scoped(context) => super::RunExecutionAuthority::Scoped(context.scope().clone()),
        }
    }
}

enum DispatchCustodyState {
    NotInvoked(ScopedEffectStartPermit<ScopedEffectTarget>),
    Invoked(ScopedEffectClaimRecord<ScopedEffectTarget>),
}

impl DispatchCustodyState {
    fn feedback(
        self,
        outcome: ScopedEffectOutcome,
        token_accounting: super::ScopedEffectTokenAccounting,
    ) -> ScopedEffectFeedback {
        match self {
            Self::NotInvoked(permit) => ScopedEffectFeedback::NotStarted(permit.into_not_started()),
            Self::Invoked(claim) => ScopedEffectFeedback::Observed {
                claim,
                outcome,
                token_accounting,
            },
        }
    }
}

pub struct ScopedEffectCustody {
    host: Arc<dyn ScopedEffectHost>,
    state: Option<DispatchCustodyState>,
    pub(super) token_accounting: super::ScopedEffectTokenAccounting,
}

impl ScopedEffectCustody {
    pub(super) fn was_invoked(&self) -> bool {
        matches!(self.state, Some(DispatchCustodyState::Invoked(_)))
    }

    pub(super) fn from_permit(
        host: Arc<dyn ScopedEffectHost>,
        permit: ScopedEffectStartPermit<ScopedEffectTarget>,
        effect_id: &crate::ops::OperationId,
        scope: &ScopedRunAuthority,
        target: &ScopedEffectTarget,
    ) -> Result<Self, ToolError> {
        let custody = Self {
            host,
            state: Some(DispatchCustodyState::NotInvoked(permit)),
            token_accounting: super::ScopedEffectTokenAccounting::for_unmeasured_target(target),
        };
        if let Some(DispatchCustodyState::NotInvoked(permit)) = &custody.state {
            if &permit.claim().effect_id != effect_id {
                return Err(ToolError::execution_failed(
                    "scoped claim returned a different effect identity",
                ));
            }
            permit
                .claim()
                .validate_scope_binding(scope.scope_id(), scope.record(), target)
                .map_err(|error| ToolError::execution_failed(error.to_string()))?;
        }
        Ok(custody)
    }

    pub fn begin_invocation(&mut self) -> Result<(), ToolError> {
        match self.state.take() {
            Some(DispatchCustodyState::NotInvoked(permit)) => {
                self.state = Some(DispatchCustodyState::Invoked(permit.into_claim()));
                Ok(())
            }
            state => {
                self.state = state;
                Err(ToolError::execution_failed(
                    "scoped effect start permit has already been consumed",
                ))
            }
        }
    }

    pub fn settle(mut self, outcome: ScopedEffectOutcome) -> ScopedEffectSettlement {
        match self.state.take() {
            Some(state) => self
                .host
                .submit_feedback(state.feedback(outcome, self.token_accounting)),
            None => Box::pin(async {
                Err(ToolError::execution_failed(
                    "scoped effect feedback custody has already been transferred",
                ))
            }),
        }
    }
}

impl Drop for ScopedEffectCustody {
    fn drop(&mut self) {
        if let Some(state) = self.state.take() {
            drop(self.host.submit_feedback(
                state.feedback(ScopedEffectOutcome::Unknown, self.token_accounting),
            ));
        }
    }
}

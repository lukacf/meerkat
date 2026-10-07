//! Retained prepared checks forwarded through existing operation adapters.

use std::fmt;
use std::sync::Arc;

use super::{
    OperationAuthorizationError, OperationObservation, OperationObservationError,
    OperationObservedOutcome, OperationRefusalKind, OperationRefused, PreparedAuthorizationBinding,
    PreparedOperationAuthorization, WorkAuthorizationContext,
};

struct PreparedCheck {
    context: WorkAuthorizationContext,
    binding: PreparedAuthorizationBinding,
    decision: Arc<dyn PreparedOperationAuthorization>,
}

/// One immutable prepared decision beside its exact operation binding.
///
/// Adapters must preserve this association with the actual request. It is not
/// a permit for another payload or a serialized proof. Final sinks check after
/// all relevant waits and before entry; completed effects are never relabelled
/// by a later failed check.
#[derive(Clone)]
pub struct PreparedOperationCheck(Arc<PreparedCheck>);

impl PreparedOperationCheck {
    pub fn prepare(
        context: WorkAuthorizationContext,
        binding: PreparedAuthorizationBinding,
    ) -> Result<Self, OperationAuthorizationError> {
        let decision = context.authorization().prepare(&binding)?;
        Ok(Self(Arc::new(PreparedCheck {
            context,
            binding,
            decision,
        })))
    }

    /// Check the exact retained operation without allocation or policy traversal.
    /// On one stale projection, return a newly prepared check from current owners.
    /// The caller must forward the returned association to the real sink.
    pub fn current(&self) -> Result<Self, OperationAuthorizationError> {
        match self.0.decision.check_current(&self.0.binding) {
            Ok(()) => Ok(self.clone()),
            Err(OperationAuthorizationError::Refused(refusal))
                if refusal.kind() == OperationRefusalKind::ReprepareRequired =>
            {
                let refreshed = Self::prepare(self.0.context.clone(), self.0.binding.clone())?;
                if let Err(error) = refreshed.0.decision.check_current(&refreshed.0.binding) {
                    return Err(refreshed.observe_check_failure(error));
                }
                Ok(refreshed)
            }
            Err(error) => Err(self.observe_check_failure(error)),
        }
    }

    fn observe_check_failure(
        &self,
        error: OperationAuthorizationError,
    ) -> OperationAuthorizationError {
        let observation = match error {
            OperationAuthorizationError::Refused(refusal) => {
                OperationObservation::Refused(refusal.kind())
            }
            OperationAuthorizationError::Unavailable => {
                OperationObservation::AuthorizationUnavailable
            }
            OperationAuthorizationError::ObservationUnavailable(_) => return error,
        };
        match self.0.decision.observe(&self.0.binding, observation) {
            Ok(()) => error,
            Err(error) => error.into(),
        }
    }

    #[must_use]
    pub fn binding(&self) -> &PreparedAuthorizationBinding {
        &self.0.binding
    }

    /// Stage entry after the final current check and immediately before the
    /// actual body/send. A staging failure is an infrastructure error.
    /// This does not replace currentness or establish one-use execution.
    pub fn observe_entry(&self) -> Result<(), OperationObservationError> {
        self.0
            .decision
            .observe(&self.0.binding, OperationObservation::Entry)
    }

    /// Preserve the actual returned observation before subsequent bookkeeping.
    /// A failure must accompany the original physical result, never replace it.
    pub fn observe_outcome(
        &self,
        outcome: OperationObservedOutcome,
    ) -> Result<(), OperationObservationError> {
        self.0
            .decision
            .observe(&self.0.binding, OperationObservation::Outcome(outcome))
    }

    /// Record a refusal without declaring that any prior physical effect failed.
    pub fn observe_refusal(
        &self,
        refusal: OperationRefused,
    ) -> Result<(), OperationObservationError> {
        self.0.decision.observe(
            &self.0.binding,
            OperationObservation::Refused(refusal.kind()),
        )
    }

    /// Check the retained operation against the physical call entering an
    /// adapter. Payload and plan references must be the exact immutable data
    /// owned by this binding, so retargeting cannot reuse another call's check.
    /// This is constant-time: it does not hash or compare argument bytes.
    pub fn current_tool(
        &self,
        work: &WorkAuthorizationContext,
        call: crate::ToolCallView<'_>,
        plan: Option<&crate::ResolvedToolExecutionPlan>,
        run_id: Option<&crate::RunId>,
    ) -> Result<Self, OperationAuthorizationError> {
        let malformed = || OperationRefused::new(OperationRefusalKind::MalformedFacts);
        let (bound_call, bound_plan) = self.tool_dispatch_parts().ok_or_else(malformed)?;
        if !self.0.context.same_context(work)
            || !std::ptr::eq(call.id, bound_call.id)
            || !std::ptr::eq(call.name, bound_call.name)
            || !std::ptr::eq(call.args, bound_call.args)
            || plan.is_some_and(|actual| !std::ptr::eq(actual, bound_plan))
            || self.0.binding.facts().run_id.as_ref() != run_id
        {
            return Err(malformed().into());
        }
        self.current()
    }

    pub(crate) fn tool_dispatch_parts(
        &self,
    ) -> Option<(crate::ToolCallView<'_>, &crate::ResolvedToolExecutionPlan)> {
        let super::AuthorizationOperation::Tool(tool) = &self.0.binding.facts().operation else {
            return None;
        };
        let super::ToolAuthorizationTarget::Dispatcher(plan) = &tool.target else {
            return None;
        };
        Some((tool.call(), plan.as_ref()))
    }

    #[must_use]
    pub fn same_check(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl fmt::Debug for PreparedOperationCheck {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PreparedOperationCheck([REDACTED])")
    }
}

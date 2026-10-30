//! Retained prepared checks forwarded through existing operation adapters.

use std::fmt;
use std::sync::Arc;

use super::{
    ObservedAuthorizationResult, OperationAuthorizationError, OperationObservation,
    OperationObservationError, OperationObservedOutcome, OperationRefusalKind, OperationRefused,
    PolicyPublicationObservation, PreparedAuthorizationBinding, PreparedOperationAuthorization,
    WorkAuthorizationContext,
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
        Self::prepare_observed(context, binding).result
    }

    pub fn prepare_observed(
        context: WorkAuthorizationContext,
        binding: PreparedAuthorizationBinding,
    ) -> ObservedAuthorizationResult<Self> {
        // A review child or a tool delegate is prepared only under the exact
        // work its origin was prepared in.
        if binding
            .review_attribution()
            .is_some_and(|link| !link.origin().work_authorization().same_context(&context))
            || binding
                .delegate_attribution()
                .is_some_and(|link| !link.work_authorization().same_context(&context))
        {
            return ObservedAuthorizationResult::unobserved(Err(OperationRefused::new(
                OperationRefusalKind::MalformedFacts,
            )
            .into()));
        }
        context
            .authorization()
            .prepare_observed(&binding)
            .map(|decision| {
                Self(Arc::new(PreparedCheck {
                    context,
                    binding,
                    decision,
                }))
            })
    }

    /// Check the exact retained operation without allocation or policy traversal.
    /// On one stale projection, return a newly prepared check from current owners.
    /// The caller must forward the returned association to the real sink.
    pub fn current(&self) -> Result<Self, OperationAuthorizationError> {
        self.current_observed().result
    }

    /// One canonical current/reprepare path, retaining only the observation of
    /// the actual decision or coherent failed preparation returned by that path.
    pub fn current_observed(&self) -> ObservedAuthorizationResult<Self> {
        match self.0.decision.check_current(&self.0.binding) {
            Ok(()) => ObservedAuthorizationResult {
                result: Ok(self.clone()),
                policy: self.policy_observation(),
            },
            Err(OperationAuthorizationError::Refused(refusal))
                if refusal.kind() == OperationRefusalKind::ReprepareRequired =>
            {
                let observed =
                    Self::prepare_observed(self.0.context.clone(), self.0.binding.clone());
                match observed.result {
                    Ok(refreshed) => {
                        let mut policy = refreshed.policy_observation();
                        let result = match refreshed.0.decision.check_current(&refreshed.0.binding)
                        {
                            Ok(()) => Ok(refreshed),
                            Err(error) => {
                                if !matches!(error, OperationAuthorizationError::Refused(_)) {
                                    policy = None;
                                }
                                Err(refreshed.observe_check_failure(error))
                            }
                        };
                        ObservedAuthorizationResult { result, policy }
                    }
                    Err(error) => ObservedAuthorizationResult {
                        result: Err(error),
                        policy: observed.policy,
                    },
                }
            }
            Err(error) => ObservedAuthorizationResult {
                result: Err(self.observe_check_failure(error)),
                policy: if matches!(error, OperationAuthorizationError::Refused(_)) {
                    self.policy_observation()
                } else {
                    None
                },
            },
        }
    }

    pub fn policy_observation(&self) -> Option<PolicyPublicationObservation> {
        self.0.decision.policy_observation()
    }

    /// [`Self::current`] for a held consent decision (ADR-001 D9): its
    /// owners prepare the exact retained operation afresh also when this
    /// decision's own check refuses it rather than asks for a re-preparation
    /// (its owner's validity window for it ended, for example), and their
    /// answer is returned as is. Nothing of this decision (its window
    /// included) is carried forward: past a real owner deadline the fresh
    /// preparation refuses. A failure of this decision's own check that is
    /// not a refusal is observed and returned, as on [`Self::current`].
    pub(crate) fn reprepared(&self) -> Result<Self, OperationAuthorizationError> {
        match self.0.decision.check_current(&self.0.binding) {
            Ok(()) => Ok(self.clone()),
            Err(OperationAuthorizationError::Refused(_)) => {
                let refreshed = Self::prepare(self.0.context.clone(), self.0.binding.clone())?;
                match refreshed.0.decision.check_current(&refreshed.0.binding) {
                    Ok(()) => Ok(refreshed),
                    Err(error) => Err(refreshed.observe_check_failure(error)),
                }
            }
            Err(error) => Err(self.observe_check_failure(error)),
        }
    }

    /// The prepared owner's own coherent current check of this exact retained
    /// decision, and nothing else: no re-preparation, no observation, no
    /// callback. Owners hold their commit reservations only across this; a
    /// failure is observed later through
    /// [`Self::observe_retained_check_failure`], once every lock is released.
    pub(crate) fn check_retained_current(&self) -> Result<(), OperationAuthorizationError> {
        self.0.decision.check_current(&self.0.binding)
    }

    /// Observe a failure [`Self::check_retained_current`] returned, outside
    /// every owner lock. A re-preparation request is not observed (the
    /// retained decision is simply no longer current), as on the
    /// [`Self::current`] path.
    pub(crate) fn observe_retained_check_failure(
        &self,
        error: OperationAuthorizationError,
    ) -> OperationAuthorizationError {
        match error {
            OperationAuthorizationError::Refused(refusal)
                if refusal.kind() == OperationRefusalKind::ReprepareRequired =>
            {
                error
            }
            error => self.observe_check_failure(error),
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

    /// The exact temporal window of this decision, when its owner bounds it
    /// (see [`super::PreparedOperationAuthorization::validity`]).
    #[must_use]
    pub fn validity(&self) -> Option<super::OperationValidity> {
        self.0.decision.validity()
    }

    /// Owner-resolved review tier retained by this exact decision. A
    /// re-prepared decision carries its own freshly resolved tier.
    #[must_use]
    pub fn review_tier(&self) -> super::OperationReviewTier {
        self.0.decision.review_tier()
    }

    #[must_use]
    pub fn binding(&self) -> &PreparedAuthorizationBinding {
        &self.0.binding
    }

    pub(crate) fn work_authorization(&self) -> &WorkAuthorizationContext {
        &self.0.context
    }

    /// R1-only gate for an entry that carries no operation review, applied
    /// to this current decision before its entry observation. Review truth is
    /// this decision's tier; R2/R3 settle locally with zero entry.
    pub fn require_unreviewed_entry(
        &self,
    ) -> Result<(), crate::approval::review::OperationReviewRefusal> {
        crate::approval::review::OperationReviewRefusal::for_unreviewed_entry(self.review_tier())
    }

    /// Stage entry at `boundary` after the final current check and
    /// immediately before the actual body/send. A staging failure is an
    /// infrastructure error. This does not replace currentness or establish
    /// one-use execution; several boundaries can observe one operation's
    /// entry, each naming itself.
    pub fn observe_entry(
        &self,
        boundary: super::OperationEntryBoundary,
    ) -> Result<(), OperationObservationError> {
        self.0
            .decision
            .observe(&self.0.binding, OperationObservation::Entry(boundary))
    }

    pub(crate) fn observe_review_attempt_started(
        &self,
        attempt_ref: crate::approval::review::ReviewAttemptRef,
    ) -> Result<(), OperationObservationError> {
        self.0.decision.observe(
            &self.0.binding,
            OperationObservation::ReviewAttemptStarted { attempt_ref },
        )
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

    /// Prepare this exact retained operation again for a resumed run of its
    /// work: the same operation (its id, call and plan by identity) under
    /// `work`'s execution scope as `run_id`, decided afresh by that work's
    /// own owners. This is the only cross-run association: the consent owner
    /// uses it to move parked custody into the resumed run its committed
    /// notice reached. A review child's or a tool delegate's operation never
    /// moves.
    pub(crate) fn prepare_for_resumed_run(
        &self,
        work: WorkAuthorizationContext,
        run_id: &crate::RunId,
    ) -> Result<Self, OperationAuthorizationError> {
        if self.0.binding.review_attribution().is_some()
            || self.0.binding.delegate_attribution().is_some()
        {
            return Err(OperationRefused::new(OperationRefusalKind::MalformedFacts).into());
        }
        let mut facts = self.0.binding.facts().clone();
        facts.execution_scope = work.execution_scope().clone();
        facts.run_id = Some(run_id.clone());
        Self::prepare(work, PreparedAuthorizationBinding::new(facts))
    }

    /// This exact retained tool operation with its call kept (its id and
    /// arguments by identity) and its plan replaced by `fresh`, the same call
    /// resolved afresh by the session's agent this process rebuilt, and
    /// decided afresh by `work`'s own owners under the same operation id,
    /// execution scope and run. The caller has checked that `fresh` is the
    /// same durable execution
    /// ([`crate::ResolvedToolExecutionPlan::same_durable_execution`]); only
    /// the consent owner rebinds custody this way. A review child's or a
    /// tool delegate's operation is never rebound.
    pub(crate) fn with_reresolved_plan(
        &self,
        work: WorkAuthorizationContext,
        fresh: crate::ResolvedToolExecutionPlan,
    ) -> Result<Self, OperationAuthorizationError> {
        let malformed = || OperationRefused::new(OperationRefusalKind::MalformedFacts).into();
        if self.0.binding.review_attribution().is_some()
            || self.0.binding.delegate_attribution().is_some()
        {
            return Err(malformed());
        }
        let mut facts = self.0.binding.facts().clone();
        let super::AuthorizationOperation::Tool(tool) = &mut facts.operation else {
            return Err(malformed());
        };
        let super::ToolAuthorizationTarget::Dispatcher(plan) = &mut tool.target else {
            return Err(malformed());
        };
        *plan = Arc::new(fresh);
        Self::prepare(work, PreparedAuthorizationBinding::new(facts))
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

    /// Whether this decision is one of exactly `other`'s binding under
    /// exactly its work, by identity: what the canonical current/reprepare
    /// path ([`Self::current`]) returns for `other`, whether or not it is
    /// the same decision. Never an authorization decision itself.
    pub(crate) fn same_operation_and_work(&self, other: &Self) -> bool {
        self.0.binding.same_operation(&other.0.binding)
            && self.0.context.same_context(&other.0.context)
    }
}

impl fmt::Debug for PreparedOperationCheck {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PreparedOperationCheck([REDACTED])")
    }
}

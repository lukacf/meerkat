//! Rebuildable per-work local authorization and constant-time final checks.

use std::sync::Arc;

use meerkat_authorization_contracts::constraints::LifetimeBound;
use meerkat_authorization_contracts::work_association::InputAuthorityAssociation;
use meerkat_core::authorization::OperationReviewTier;
use meerkat_core::authorization::{
    AuthorizationOperation, ModelAuthorizationUse, OperationRefusalKind, OperationRefused,
    PreparedAuthorizationBinding, PreparedOperationAuthorization, WorkAuthorization,
};
use meerkat_core::time_compat::{Duration, Instant};

use crate::policy::{LocalPolicyAllowance, LocalPolicyPurpose, LocalWorkPolicy};
use crate::publication::{LocalAuthorizationPublication, LocalPublicationStamp, PublicationError};

pub use crate::clock::{
    HostAuthorizationClock, LocalAuthorizationClock, LocalAuthorizationTime, LocalClockError,
};

/// Host-installed composition for one immutable native work association.
///
/// The policy owner must verify that association against real admitted work on
/// every preparation. Constructing this handle is not acceptance of a decoded
/// claim. Never persist it or keep it as a session-wide default. All policy-owner
/// mutations must share the same publication instance for this handle's lifetime.
pub struct LocalWorkAuthorization {
    associations: Arc<[InputAuthorityAssociation]>,
    policy: Arc<dyn LocalWorkPolicy>,
    publication: LocalAuthorizationPublication,
    clock: Arc<dyn LocalAuthorizationClock>,
}

impl LocalWorkAuthorization {
    #[must_use]
    pub fn new(
        association: Arc<InputAuthorityAssociation>,
        policy: Arc<dyn LocalWorkPolicy>,
        publication: LocalAuthorizationPublication,
        clock: Arc<dyn LocalAuthorizationClock>,
    ) -> Self {
        Self {
            associations: vec![association.as_ref().clone()].into(),
            policy,
            publication,
            clock,
        }
    }

    /// Compose every retained native contributor, without electing the first
    /// contributor's mandate or unioning permissions. This validates data shape
    /// only; the policy still resolves every original input on preparation.
    pub fn new_batch(
        associations: Arc<[InputAuthorityAssociation]>,
        policy: Arc<dyn LocalWorkPolicy>,
        publication: LocalAuthorizationPublication,
        clock: Arc<dyn LocalAuthorizationClock>,
    ) -> Result<Self, OperationRefused> {
        let first = associations.first().ok_or_else(denied)?;
        if associations
            .iter()
            .any(|other| !first.batch_compatible_with(other))
        {
            return Err(denied());
        }
        Ok(Self {
            associations,
            policy,
            publication,
            clock,
        })
    }

    fn compile(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<CompiledLocalAuthorization, meerkat_core::OperationAuthorizationError> {
        let now = self
            .clock
            .now()
            .map_err(|_| meerkat_core::OperationAuthorizationError::Unavailable)?;
        // All live policy facts are read within this bracket. An old policy
        // view cannot acquire a new publication stamp.
        let mut known_failure = None;
        let observed = self.publication.observe(|| {
            let result = (|| -> Result<_, meerkat_core::OperationAuthorizationError> {
                let mut combined: Option<(u64, u64, Instant, OperationReviewTier)> = None;
                let purposes: &[LocalPolicyPurpose] = match &binding.facts().operation {
                    AuthorizationOperation::Model(facts)
                        if facts.usage == ModelAuthorizationUse::ControllerInference =>
                    {
                        if facts.hosted_capabilities.is_empty() {
                            &[LocalPolicyPurpose::Controller]
                        } else {
                            &[
                                LocalPolicyPurpose::Controller,
                                LocalPolicyPurpose::Operation,
                            ]
                        }
                    }
                    _ => &[LocalPolicyPurpose::Operation],
                };
                for association in self.associations.iter() {
                    for &purpose in purposes {
                        let allowance =
                            self.policy
                                .evaluate_for(association, binding, purpose, now.unix_ms)?;
                        let current =
                            self.validate_allowance(association, binding, purpose, allowance, now)?;
                        combined = Some(match combined {
                            None => current,
                            // Every contributor's owner tier applies; keep
                            // the strictest.
                            Some(previous) => (
                                previous.0.max(current.0),
                                previous.1.min(current.1),
                                previous.2.min(current.2),
                                previous.3.max(current.3),
                            ),
                        });
                    }
                }
                combined.ok_or_else(denied).map_err(Into::into)
            })();
            if matches!(
                result,
                Err(meerkat_core::OperationAuthorizationError::Unavailable
                    | meerkat_core::OperationAuthorizationError::ObservationUnavailable(_))
            ) {
                known_failure = result.as_ref().err().copied();
            }
            result
        });
        if let Some(error) = known_failure {
            return Err(error);
        }
        let (prepared, publication) = observed.map_err(publication_refusal)?;
        let (not_before_ms, expires_at_ms, deadline, review_tier) = prepared?;
        Ok(CompiledLocalAuthorization {
            binding: binding.clone(),
            publication,
            clock: Arc::clone(&self.clock),
            not_before_ms,
            expires_at_ms,
            deadline,
            review_tier,
        })
    }

    fn validate_allowance(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        allowance: LocalPolicyAllowance,
        now: LocalAuthorizationTime,
    ) -> Result<(u64, u64, Instant, OperationReviewTier), OperationRefused> {
        if allowance.operation_values.is_empty() || allowance.expires_at_ms <= now.unix_ms {
            return Err(denied());
        }
        let candidate = association.candidate();
        // Controller and hosted-operation duties are conjunctive, but each
        // duty has its own correlated values and retained ceiling. Applying
        // the controller's infer-only ceiling to web_search would incorrectly
        // disable a separately authorized hosted capability.
        let ceiling = match purpose {
            LocalPolicyPurpose::Controller => {
                let AuthorizationOperation::Model(facts) = &binding.facts().operation else {
                    return Err(denied());
                };
                if facts.usage != ModelAuthorizationUse::ControllerInference
                    || !candidate
                        .controller_model
                        .as_ref()
                        .is_some_and(|selection| selection.matches_model_facts(facts))
                {
                    return Err(denied());
                }
                &candidate.controller_ceiling
            }
            LocalPolicyPurpose::Operation => &candidate.admitted_ceiling,
        };
        let restrictions = ceiling.conjoin(&allowance.restrictions);
        let (not_before_ms, expires_at_ms) = match restrictions.lifetime.bound() {
            LifetimeBound::Unrestricted => (0, allowance.expires_at_ms),
            LifetimeBound::Window {
                not_before_ms,
                expires_at_ms,
            } => (not_before_ms, expires_at_ms.min(allowance.expires_at_ms)),
            LifetimeBound::Empty => return Err(denied()),
        };
        for values in &allowance.operation_values {
            restrictions
                .check_bounds(values.at(now.unix_ms))
                .map_err(|_| denied())?;
        }
        let remaining_ms = expires_at_ms
            .checked_sub(now.unix_ms)
            .filter(|ms| *ms > 0)
            .ok_or_else(denied)?;
        let deadline = now
            .monotonic
            .checked_add(Duration::from_millis(remaining_ms))
            .ok_or_else(denied)?;
        Ok((
            not_before_ms,
            expires_at_ms,
            deadline,
            allowance.review_tier,
        ))
    }
}

impl WorkAuthorization for LocalWorkAuthorization {
    fn controller_model_selection(&self) -> Option<meerkat_core::ControllerModelSelection> {
        self.associations
            .first()?
            .candidate()
            .controller_model
            .clone()
    }

    fn prepare(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<Arc<dyn PreparedOperationAuthorization>, meerkat_core::OperationAuthorizationError>
    {
        // Exactly one bounded re-observation on a concurrent local publication.
        // A denied/malformed operation is not automatically retried.
        let first = self.compile(binding);
        let prepared = match first {
            Err(meerkat_core::OperationAuthorizationError::Refused(refusal))
                if refusal.kind() == OperationRefusalKind::ReprepareRequired =>
            {
                self.compile(binding)?
            }
            result => result?,
        };
        prepared.check_current(binding)?;
        Ok(Arc::new(prepared))
    }
}

/// Use the same actual publication, local clock and exact-entry check as
/// admitted work, while retaining non-input control semantics. Each distinct
/// source/destination rule keeps its correlated values and restrictions.
pub(crate) fn compile_context_control(
    publication: &LocalAuthorizationPublication,
    clock: &Arc<dyn LocalAuthorizationClock>,
    binding: &PreparedAuthorizationBinding,
    evaluate: impl FnOnce(
        u64,
    ) -> Result<
        Vec<LocalPolicyAllowance>,
        meerkat_core::OperationAuthorizationError,
    >,
) -> meerkat_core::authorization::ObservedAuthorizationResult<Arc<dyn PreparedOperationAuthorization>>
{
    use meerkat_core::authorization::ObservedAuthorizationResult;
    let now = match clock.now() {
        Ok(now) => now,
        Err(_) => {
            return ObservedAuthorizationResult::unobserved(Err(
                meerkat_core::OperationAuthorizationError::Unavailable,
            ));
        }
    };
    let mut known_failure = None;
    let observed = publication.observe(|| {
        let result = (|| -> Result<_, meerkat_core::OperationAuthorizationError> {
            let allowances = evaluate(now.unix_ms)?;
            if allowances.is_empty() {
                return Err(denied().into());
            }
            let mut not_before = 0;
            let mut expires = u64::MAX;
            let mut review_tier = OperationReviewTier::R1;
            for allowance in allowances {
                if allowance.operation_values.is_empty() || allowance.expires_at_ms <= now.unix_ms {
                    return Err(denied().into());
                }
                let (start, end) = match allowance.restrictions.lifetime.bound() {
                    LifetimeBound::Unrestricted => (0, allowance.expires_at_ms),
                    LifetimeBound::Window {
                        not_before_ms,
                        expires_at_ms,
                    } => (not_before_ms, expires_at_ms.min(allowance.expires_at_ms)),
                    LifetimeBound::Empty => return Err(denied().into()),
                };
                for values in &allowance.operation_values {
                    allowance
                        .restrictions
                        .check_bounds(values.at(now.unix_ms))
                        .map_err(|_| denied())?;
                }
                not_before = not_before.max(start);
                expires = expires.min(end);
                review_tier = review_tier.max(allowance.review_tier);
            }
            let remaining = expires
                .checked_sub(now.unix_ms)
                .filter(|value| *value > 0)
                .ok_or_else(denied)?;
            let deadline = now
                .monotonic
                .checked_add(Duration::from_millis(remaining))
                .ok_or_else(denied)?;
            Ok((not_before, expires, deadline, review_tier))
        })();
        if matches!(
            result,
            Err(meerkat_core::OperationAuthorizationError::Unavailable
                | meerkat_core::OperationAuthorizationError::ObservationUnavailable(_))
        ) {
            known_failure = result.as_ref().err().copied();
        }
        result
    });
    if let Some(error) = known_failure {
        return ObservedAuthorizationResult::unobserved(Err(error));
    }
    let (result, publication) = match observed {
        Ok(observed) => observed,
        Err(error) => {
            return ObservedAuthorizationResult::unobserved(Err(publication_refusal(error)));
        }
    };
    let policy = Some(publication.policy_observation());
    let result = result.and_then(|(not_before_ms, expires_at_ms, deadline, review_tier)| {
        let prepared = CompiledLocalAuthorization {
            binding: binding.clone(),
            publication,
            clock: Arc::clone(clock),
            not_before_ms,
            expires_at_ms,
            deadline,
            review_tier,
        };
        prepared.check_current(binding)?;
        Ok(Arc::new(prepared) as Arc<dyn PreparedOperationAuthorization>)
    });
    let policy = if matches!(
        result,
        Err(meerkat_core::OperationAuthorizationError::Unavailable
            | meerkat_core::OperationAuthorizationError::ObservationUnavailable(_))
    ) {
        None
    } else {
        policy
    };
    ObservedAuthorizationResult { result, policy }
}

struct CompiledLocalAuthorization {
    binding: PreparedAuthorizationBinding,
    publication: LocalPublicationStamp,
    clock: Arc<dyn LocalAuthorizationClock>,
    not_before_ms: u64,
    expires_at_ms: u64,
    deadline: Instant,
    review_tier: OperationReviewTier,
}

impl PreparedOperationAuthorization for CompiledLocalAuthorization {
    fn policy_observation(
        &self,
    ) -> Option<meerkat_core::authorization::PolicyPublicationObservation> {
        Some(self.publication.policy_observation())
    }

    fn review_tier(&self) -> OperationReviewTier {
        self.review_tier
    }

    fn check_current(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<(), meerkat_core::OperationAuthorizationError> {
        if !self.binding.same_operation(binding) {
            return Err(OperationRefused::new(OperationRefusalKind::ReprepareRequired).into());
        }
        // A local time read precedes the final publication read, so no relevant
        // awaited step or policy traversal occurs after the final current check.
        let now = self
            .clock
            .now()
            .map_err(|_| meerkat_core::OperationAuthorizationError::Unavailable)?;
        if now.unix_ms < self.not_before_ms
            || now.unix_ms >= self.expires_at_ms
            || now.monotonic >= self.deadline
        {
            return Err(denied().into());
        }
        self.publication
            .check_current()
            .map_err(publication_refusal)
    }
}

fn publication_refusal(error: PublicationError) -> meerkat_core::OperationAuthorizationError {
    match error {
        PublicationError::Changed => {
            OperationRefused::new(OperationRefusalKind::ReprepareRequired).into()
        }
        PublicationError::Unavailable => meerkat_core::OperationAuthorizationError::Unavailable,
    }
}

fn denied() -> OperationRefused {
    OperationRefused::new(OperationRefusalKind::Denied)
}

#[cfg(test)]
mod tests;

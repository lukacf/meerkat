//! Process-local origin of a review's independently authorized operations.
//! This carrier is diagnostic association, never permission or review state.

use std::fmt;
use std::sync::Arc;

use crate::authorization::{
    AuthorizationOperation, ModelAuthorizationUse, OperationAuthorizationError,
    OperationAuthorizationFacts, OperationRefusalKind, OperationRefused,
    PreparedAuthorizationBinding, SourceAuthorizationUse, WorkAuthorization,
    WorkAuthorizationContext,
};

use super::{ReviewAttemptHandle, ReviewAttemptRef};

struct ReviewOrigin {
    candidate: PreparedAuthorizationBinding,
    work: WorkAuthorizationContext,
    attempt_ref: ReviewAttemptRef,
}

/// Minted only from the core owner's actual review attempt. A historical audit
/// ID cannot construct this carrier. Retention does not keep an attempt live,
/// grant a source/model operation, or satisfy currentness or one-use entry.
#[derive(Clone)]
pub struct ReviewOperationAttribution(Arc<ReviewOrigin>);

impl ReviewOperationAttribution {
    pub(super) fn from_attempt(handle: &ReviewAttemptHandle) -> Self {
        Self(Arc::new(ReviewOrigin {
            candidate: handle.binding.clone(),
            work: handle.work.clone(),
            attempt_ref: handle.attempt_ref(),
        }))
    }

    pub fn candidate_binding(&self) -> &PreparedAuthorizationBinding {
        &self.0.candidate
    }

    pub fn work_authorization(&self) -> &WorkAuthorizationContext {
        &self.0.work
    }

    pub fn attempt_ref(&self) -> &ReviewAttemptRef {
        &self.0.attempt_ref
    }

    /// Bind a fresh source read inside the original work owner's context read.
    /// The caller must be that exact owner, reading for that exact candidate.
    /// Equal coordinates or another wrapper around an owner cannot retag it.
    pub fn context_read_binding(
        &self,
        candidate: &PreparedAuthorizationBinding,
        owner: &dyn WorkAuthorization,
        facts: OperationAuthorizationFacts,
    ) -> Result<PreparedAuthorizationBinding, OperationAuthorizationError> {
        if !self.0.candidate.same_operation(candidate)
            || !std::ptr::addr_eq(self.0.work.authorization().as_ref(), owner)
        {
            return Err(malformed());
        }
        self.bind_child(facts, &self.0.work, ReviewOperationRole::ContextRead)
    }

    pub(crate) fn bind_child(
        &self,
        facts: OperationAuthorizationFacts,
        work: &WorkAuthorizationContext,
        role: ReviewOperationRole,
    ) -> Result<PreparedAuthorizationBinding, OperationAuthorizationError> {
        let candidate = self.0.candidate.facts();
        let matches_role = match (&facts.operation, role) {
            (AuthorizationOperation::Source(source), ReviewOperationRole::ContextRead) => {
                source.usage == SourceAuthorizationUse::Read
            }
            (AuthorizationOperation::Model(model), ReviewOperationRole::ReviewerInference) => {
                model.usage == ModelAuthorizationUse::Inference
            }
            _ => false,
        };
        if !self.0.work.same_context(work)
            || facts.operation_id == candidate.operation_id
            || facts.execution_scope != candidate.execution_scope
            || &facts.execution_scope != work.execution_scope()
            || facts.run_id != candidate.run_id
            || facts.context_revision != candidate.context_revision
            || !matches_role
        {
            return Err(malformed());
        }
        Ok(PreparedAuthorizationBinding::new_review_child(
            facts,
            ReviewChildAttribution {
                origin: self.clone(),
                role,
            },
        ))
    }
}

/// Role of an independently authorized child operation, never an exemption.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewOperationRole {
    ContextRead,
    ReviewerInference,
}

/// Immutable origin retained beside the exact child binding. Projection to
/// strings belongs to protected audit; decoding those strings cannot recreate
/// this process-local origin or a review allowance.
pub struct ReviewChildAttribution {
    origin: ReviewOperationAttribution,
    role: ReviewOperationRole,
}

impl ReviewChildAttribution {
    pub fn origin(&self) -> &ReviewOperationAttribution {
        &self.origin
    }

    pub fn role(&self) -> ReviewOperationRole {
        self.role
    }
}

impl fmt::Debug for ReviewOperationAttribution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ReviewOperationAttribution([REDACTED])")
    }
}

impl fmt::Debug for ReviewChildAttribution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ReviewChildAttribution([REDACTED])")
    }
}

fn malformed() -> OperationAuthorizationError {
    OperationRefused::new(OperationRefusalKind::MalformedFacts).into()
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests;

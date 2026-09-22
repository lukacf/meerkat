//! Backend seam.
//!
//! A backend decodes provider evidence into [`RawAnswer`]s keyed by the
//! caller's question ids. It owns transport, protocol, and native-signal
//! decoding only. Answer semantics, option membership, level bounds, and the
//! fixed interpretation belong to the service.
//!
//! Every attempt sequence — successful or not — reports the accounting it
//! consumed, so the service can settle the caller's budget from what was
//! actually spent rather than from an assumption of zero.

use async_trait::async_trait;
use meerkat_core::time_compat::{Duration, Instant};

use crate::contracts::{BackendKind, BinaryAnswer, RouteProvenance};
use crate::error::BackendFailure;
use crate::service::DecisionAdmission;
use crate::validate::ValidatedRequest;

/// Absolute deadline for one evaluation; retries and repair draw on it.
#[derive(Debug, Clone, Copy)]
pub struct Deadline {
    at: Instant,
    total: Duration,
}

impl Deadline {
    pub fn after(total: Duration) -> Self {
        Self {
            at: Instant::now() + total,
            total,
        }
    }

    pub fn remaining(&self) -> Duration {
        self.at.saturating_duration_since(Instant::now())
    }

    pub fn is_expired(&self) -> bool {
        self.remaining().is_zero()
    }

    pub const fn total(&self) -> Duration {
        self.total
    }
}

/// Probability distribution over string-keyed alternatives as the backend
/// reported it, before the service checks membership and bounds.
#[derive(Debug, Clone, PartialEq)]
pub struct RawDistribution {
    pub probabilities: Vec<(String, f64)>,
    pub confidence: f64,
}

/// Probability distribution over level indices as the backend reported it.
#[derive(Debug, Clone, PartialEq)]
pub struct RawGradeDistribution {
    pub probabilities: Vec<(u32, f64)>,
    pub confidence: f64,
}

/// One decoded backend answer. Each variant states exactly what the backend
/// supplied; nothing is inferred from a form the backend did not return.
#[derive(Debug, Clone, PartialEq)]
pub enum RawAnswer {
    BinaryCategorical(BinaryAnswer),
    BinaryProbability {
        yes: f64,
    },
    ChoiceSelected {
        option: String,
        distribution: Option<RawDistribution>,
    },
    ChoiceAbstain,
    GradeLevel {
        index: u32,
    },
    GradeAbstain,
    GradeWeighted {
        position: f64,
        distribution: Option<RawGradeDistribution>,
    },
}

/// Accounting for one provider attempt on an LLM route.
#[derive(Debug, Clone, PartialEq)]
pub enum AttemptUsage {
    /// The stream completed and the adapter reported usage.
    Measured(meerkat_core::types::Usage),
    /// The attempt was issued but produced no usage report: the call failed
    /// before its stream completed, or the deadline dropped it mid-flight.
    /// Tokens may have been spent; their number is unknown, never zero.
    Unmeasured,
}

/// Accounting as the backend reported it, covering every attempt made.
#[derive(Debug, Clone, PartialEq)]
pub enum BackendUsage {
    /// One entry per provider call on an LLM route (including bounded repair
    /// attempts), so an attempt that spent tokens without reporting them is
    /// a typed absence rather than a missing entry. Normalized by the
    /// service through the shared [`meerkat_core::types::TurnUsage`] contract.
    Provider(Vec<AttemptUsage>),
    /// Tokens reported by a non-LLM model backend.
    Reported {
        input_tokens: u64,
        output_tokens: u64,
    },
    /// The backend produced no accounting.
    Unmeasured,
}

/// Everything one backend attempt sequence produced.
#[derive(Debug, Clone, PartialEq)]
pub struct BackendResponse {
    /// Answers keyed by the caller's question id strings.
    pub answers: Vec<(String, RawAnswer)>,
    pub route: RouteProvenance,
    pub usage: BackendUsage,
    /// Attempts consumed, including bounded repair and transient backoff.
    pub attempts: u32,
}

/// A failed attempt sequence together with the accounting it consumed.
///
/// A backend that made provider calls before failing reports their usage
/// here so the caller's budget is settled from spent tokens, never released
/// as if nothing happened.
#[derive(Debug, Clone, PartialEq)]
pub struct FailedEvaluation {
    pub failure: BackendFailure,
    pub usage: BackendUsage,
    pub attempts: u32,
}

impl FailedEvaluation {
    /// A failure that happened before any provider call.
    pub fn before_any_call(failure: BackendFailure) -> Self {
        Self {
            failure,
            usage: BackendUsage::Unmeasured,
            attempts: 0,
        }
    }
}

impl From<BackendFailure> for FailedEvaluation {
    fn from(failure: BackendFailure) -> Self {
        Self::before_any_call(failure)
    }
}

/// A decision backend.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait DecisionBackend: Send + Sync {
    fn kind(&self) -> BackendKind;

    /// Evaluate one validated request within `deadline`, using at most
    /// `max_attempts` backend attempts. The admission carries the owner-issued
    /// route for backends bound to the caller's session. A valid answer the
    /// caller may dislike is never a reason to try again.
    async fn evaluate(
        &self,
        admission: &DecisionAdmission,
        request: &ValidatedRequest,
        deadline: Deadline,
        max_attempts: u32,
    ) -> Result<BackendResponse, FailedEvaluation>;
}

//! Backend seam.
//!
//! A backend decodes provider evidence into [`RawAnswer`]s keyed by the
//! caller's question ids. It owns transport, protocol, and native-signal
//! decoding only. Answer semantics, option membership, level bounds, and the
//! fixed interpretation belong to the service.

use async_trait::async_trait;
use meerkat_core::time_compat::{Duration, Instant};

use crate::contracts::{BackendKind, BinaryAnswer, RouteProvenance};
use crate::error::BackendFailure;
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

/// Accounting as the backend reported it.
#[derive(Debug, Clone, PartialEq)]
pub enum BackendUsage {
    /// Raw provider usage from an LLM route; normalized by the service through
    /// the shared [`meerkat_core::types::TurnUsage`] contract.
    Provider(meerkat_core::types::Usage),
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

/// A decision backend.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait DecisionBackend: Send + Sync {
    fn kind(&self) -> BackendKind;

    /// Evaluate one validated request within `deadline`, using at most
    /// `max_attempts` backend attempts. A valid answer the caller may dislike
    /// is never a reason to try again.
    async fn evaluate(
        &self,
        request: &ValidatedRequest,
        deadline: Deadline,
        max_attempts: u32,
    ) -> Result<BackendResponse, BackendFailure>;
}

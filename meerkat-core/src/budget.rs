//! Budget enforcement for Meerkat
//!
//! Tracks and enforces resource limits (tokens, time, tool calls).

use crate::error::AgentError;
use crate::time_compat::{Duration, Instant};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

/// Resource limits for an agent run
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BudgetLimits {
    /// Maximum tokens to consume
    pub max_tokens: Option<u64>,
    /// Maximum duration
    pub max_duration: Option<Duration>,
    /// Maximum tool calls
    pub max_tool_calls: Option<usize>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum BudgetDimension {
    Tokens,
    Time,
    ToolCalls,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct BudgetExceeded {
    pub dimension: BudgetDimension,
    pub used: u64,
    pub limit: u64,
}

impl BudgetExceeded {
    pub fn to_agent_error(self) -> AgentError {
        match self.dimension {
            BudgetDimension::Tokens => AgentError::TokenBudgetExceeded {
                used: self.used,
                limit: self.limit,
            },
            BudgetDimension::Time => AgentError::TimeBudgetExceeded {
                elapsed_secs: self.used,
                limit_secs: self.limit,
            },
            BudgetDimension::ToolCalls => AgentError::ToolCallBudgetExceeded {
                count: saturating_usize(self.used),
                limit: saturating_usize(self.limit),
            },
        }
    }

    pub fn from_agent_error(error: &AgentError) -> Option<Self> {
        match error {
            AgentError::TokenBudgetExceeded { used, limit } => Some(Self {
                dimension: BudgetDimension::Tokens,
                used: *used,
                limit: *limit,
            }),
            AgentError::TimeBudgetExceeded {
                elapsed_secs,
                limit_secs,
            } => Some(Self {
                dimension: BudgetDimension::Time,
                used: *elapsed_secs,
                limit: *limit_secs,
            }),
            AgentError::ToolCallBudgetExceeded { count, limit } => Some(Self {
                dimension: BudgetDimension::ToolCalls,
                used: *count as u64,
                limit: *limit as u64,
            }),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum BudgetObservation {
    WithinLimit,
    Exceeded(BudgetExceeded),
}

impl BudgetObservation {
    pub fn exceeded(self) -> Option<BudgetExceeded> {
        match self {
            Self::WithinLimit => None,
            Self::Exceeded(exceeded) => Some(exceeded),
        }
    }
}

fn saturating_usize(value: u64) -> usize {
    value.min(usize::MAX as u64) as usize
}

impl BudgetLimits {
    /// Create unlimited budget
    pub fn unlimited() -> Self {
        Self::default()
    }

    /// Set max tokens
    pub fn with_max_tokens(mut self, max: u64) -> Self {
        self.max_tokens = Some(max);
        self
    }

    /// Set max duration
    pub fn with_max_duration(mut self, max: Duration) -> Self {
        self.max_duration = Some(max);
        self
    }

    /// Set max tool calls
    pub fn with_max_tool_calls(mut self, max: usize) -> Self {
        self.max_tool_calls = Some(max);
        self
    }
}

/// Budget tracker for a single agent run
#[derive(Debug)]
pub struct Budget {
    limits: BudgetLimits,
    tokens_used: AtomicU64,
    tool_calls_made: AtomicU64,
    start_time: Instant,
}

impl Budget {
    /// Create a new budget with the given limits
    pub fn new(limits: BudgetLimits) -> Self {
        Self {
            limits,
            tokens_used: AtomicU64::new(0),
            tool_calls_made: AtomicU64::new(0),
            start_time: Instant::now(),
        }
    }

    /// Create an unlimited budget
    pub fn unlimited() -> Self {
        Self::new(BudgetLimits::unlimited())
    }

    /// Builder method for max tokens
    pub fn with_max_tokens(mut self, max: u64) -> Self {
        self.limits.max_tokens = Some(max);
        self
    }

    /// Builder method for max duration
    pub fn with_max_duration(mut self, max: Duration) -> Self {
        self.limits.max_duration = Some(max);
        self
    }

    /// Builder method for max tool calls
    pub fn with_max_tool_calls(mut self, max: usize) -> Self {
        self.limits.max_tool_calls = Some(max);
        self
    }

    /// Check if budget is exhausted, returning error if so
    pub fn check(&self) -> Result<(), AgentError> {
        if let BudgetObservation::Exceeded(exceeded) = self.observe() {
            return Err(exceeded.to_agent_error());
        }
        Ok(())
    }

    /// Observe budget state as a typed fact. The caller may route an
    /// exceeded observation through the turn authority instead of locally
    /// choosing a terminal path.
    pub fn observe(&self) -> BudgetObservation {
        // Check token limit
        if let Some(limit) = self.limits.max_tokens {
            let used = self.tokens_used.load(Ordering::Relaxed);
            if used >= limit {
                return BudgetObservation::Exceeded(BudgetExceeded {
                    dimension: BudgetDimension::Tokens,
                    used,
                    limit,
                });
            }
        }

        // Check time limit
        if let Some(limit) = self.limits.max_duration {
            let elapsed = self.start_time.elapsed();
            if elapsed >= limit {
                return BudgetObservation::Exceeded(BudgetExceeded {
                    dimension: BudgetDimension::Time,
                    used: elapsed.as_secs(),
                    limit: limit.as_secs(),
                });
            }
        }

        // Check tool call limit
        if let Some(limit) = self.limits.max_tool_calls {
            let count = self.tool_calls_made.load(Ordering::Relaxed) as usize;
            if count >= limit {
                return BudgetObservation::Exceeded(BudgetExceeded {
                    dimension: BudgetDimension::ToolCalls,
                    used: count as u64,
                    limit: limit as u64,
                });
            }
        }

        BudgetObservation::WithinLimit
    }

    /// Check if budget is exhausted (returns bool)
    pub fn is_exhausted(&self) -> bool {
        self.check().is_err()
    }

    /// Get remaining tokens (0 if unlimited or exhausted)
    pub fn remaining(&self) -> u64 {
        self.remaining_tokens().unwrap_or(u64::MAX)
    }

    /// Record token usage
    pub fn record_tokens(&self, tokens: u64) {
        self.tokens_used.fetch_add(tokens, Ordering::Relaxed);
    }

    /// Record tool calls
    pub fn record_calls(&self, count: usize) {
        self.tool_calls_made
            .fetch_add(count as u64, Ordering::Relaxed);
    }

    /// Record usage from a Usage struct
    pub fn record_usage(&self, usage: &crate::types::Usage) {
        self.record_tokens(usage.total_tokens());
    }

    /// Record a single tool call
    pub fn record_tool_call(&self) {
        self.record_calls(1);
    }

    /// Get token usage (used, limit) if limit is set
    pub fn token_usage(&self) -> Option<(u64, u64)> {
        self.limits
            .max_tokens
            .map(|limit| (self.tokens_used.load(Ordering::Relaxed), limit))
    }

    /// Get time usage (elapsed_ms, limit_ms) if limit is set
    pub fn time_usage(&self) -> Option<(u64, u64)> {
        self.limits.max_duration.map(|limit| {
            (
                self.start_time.elapsed().as_millis() as u64,
                limit.as_millis() as u64,
            )
        })
    }

    /// Get call usage (count, limit) if limit is set
    pub fn call_usage(&self) -> Option<(usize, usize)> {
        self.limits
            .max_tool_calls
            .map(|limit| (self.tool_calls_made.load(Ordering::Relaxed) as usize, limit))
    }

    /// Get remaining tokens (None if unlimited)
    pub fn remaining_tokens(&self) -> Option<u64> {
        self.limits.max_tokens.map(|limit| {
            let used = self.tokens_used.load(Ordering::Relaxed);
            limit.saturating_sub(used)
        })
    }

    /// Get remaining duration (None if unlimited)
    pub fn remaining_duration(&self) -> Option<Duration> {
        self.limits.max_duration.map(|limit| {
            let elapsed = self.start_time.elapsed();
            limit.saturating_sub(elapsed)
        })
    }
}

impl Clone for Budget {
    fn clone(&self) -> Self {
        Self {
            limits: self.limits.clone(),
            tokens_used: AtomicU64::new(self.tokens_used.load(Ordering::Relaxed)),
            tool_calls_made: AtomicU64::new(self.tool_calls_made.load(Ordering::Relaxed)),
            start_time: self.start_time,
        }
    }
}

/// Budget pool for allocating resources to delegated branches
#[derive(Debug)]
pub struct BudgetPool {
    /// Total budget limits
    limits: BudgetLimits,
    /// Tokens allocated so far
    allocated_tokens: AtomicU64,
    /// Tokens actually used by completed operations
    used_tokens: AtomicU64,
    /// Start time for the pool
    start_time: Instant,
}

impl BudgetPool {
    /// Create a new budget pool with the given limits
    pub fn new(limits: BudgetLimits) -> Self {
        Self {
            limits,
            allocated_tokens: AtomicU64::new(0),
            used_tokens: AtomicU64::new(0),
            start_time: Instant::now(),
        }
    }

    /// Reserve budget for a delegated branch
    pub fn reserve(&self, request: &BudgetLimits) -> Result<BudgetLimits, AgentError> {
        // Calculate available budget
        let available_tokens = self.available_tokens();
        let available_duration = self.available_duration();

        // Determine allocation
        let allocated = BudgetLimits {
            max_tokens: request
                .max_tokens
                .map(|r| r.min(available_tokens.unwrap_or(u64::MAX))),
            max_duration: request
                .max_duration
                .map(|r| available_duration.map_or(r, |a| r.min(a))),
            max_tool_calls: request.max_tool_calls,
        };

        // Record allocation
        if let Some(tokens) = allocated.max_tokens {
            self.allocated_tokens.fetch_add(tokens, Ordering::Relaxed);
        }

        Ok(allocated)
    }

    /// Reclaim unused budget from a completed operation
    pub fn reclaim(&self, allocated: &BudgetLimits, used: u64) {
        if let Some(alloc) = allocated.max_tokens {
            // Return unused portion
            let unused = alloc.saturating_sub(used);
            self.allocated_tokens.fetch_sub(unused, Ordering::Relaxed);
        }
        self.used_tokens.fetch_add(used, Ordering::Relaxed);
    }

    /// Get available tokens
    pub fn available_tokens(&self) -> Option<u64> {
        self.limits.max_tokens.map(|limit| {
            let allocated = self.allocated_tokens.load(Ordering::Relaxed);
            limit.saturating_sub(allocated)
        })
    }

    /// Get available duration
    pub fn available_duration(&self) -> Option<Duration> {
        self.limits.max_duration.map(|limit| {
            let elapsed = self.start_time.elapsed();
            limit.saturating_sub(elapsed)
        })
    }

    /// Check if pool is exhausted
    pub fn is_exhausted(&self) -> bool {
        if let Some(available) = self.available_tokens()
            && available == 0
        {
            return true;
        }
        if let Some(available) = self.available_duration()
            && available.is_zero()
        {
            return true;
        }
        false
    }
}

impl Clone for BudgetPool {
    fn clone(&self) -> Self {
        Self {
            limits: self.limits.clone(),
            allocated_tokens: AtomicU64::new(self.allocated_tokens.load(Ordering::Relaxed)),
            used_tokens: AtomicU64::new(self.used_tokens.load(Ordering::Relaxed)),
            start_time: self.start_time,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_budget_unlimited() {
        let budget = Budget::unlimited();
        assert!(budget.check().is_ok());
        assert!(budget.token_usage().is_none());
        assert!(budget.time_usage().is_none());
        assert!(budget.call_usage().is_none());
    }

    #[test]
    fn test_budget_token_limit() {
        let budget = Budget::new(BudgetLimits::default().with_max_tokens(100));

        budget.record_tokens(50);
        assert_eq!(budget.observe(), BudgetObservation::WithinLimit);
        assert_eq!(budget.token_usage(), Some((50, 100)));
        assert_eq!(budget.remaining_tokens(), Some(50));

        budget.record_tokens(50);
        assert_eq!(
            budget.observe(),
            BudgetObservation::Exceeded(BudgetExceeded {
                dimension: BudgetDimension::Tokens,
                used: 100,
                limit: 100,
            })
        );
    }

    #[test]
    fn test_budget_tool_call_limit() {
        let budget = Budget::new(BudgetLimits::default().with_max_tool_calls(5));

        budget.record_calls(3);
        assert_eq!(budget.observe(), BudgetObservation::WithinLimit);
        assert_eq!(budget.call_usage(), Some((3, 5)));

        budget.record_calls(2);
        assert_eq!(
            budget.observe(),
            BudgetObservation::Exceeded(BudgetExceeded {
                dimension: BudgetDimension::ToolCalls,
                used: 5,
                limit: 5,
            })
        );
    }

    #[test]
    fn budget_exceeded_maps_to_legacy_error_for_compatibility() {
        let exceeded = BudgetExceeded {
            dimension: BudgetDimension::Tokens,
            used: 10,
            limit: 10,
        };
        assert!(matches!(
            exceeded.to_agent_error(),
            AgentError::TokenBudgetExceeded {
                used: 10,
                limit: 10
            }
        ));
    }

    #[test]
    fn test_budget_pool_reserve() {
        let pool = BudgetPool::new(BudgetLimits::default().with_max_tokens(1000));

        let request = BudgetLimits::default().with_max_tokens(300);
        let allocated = pool.reserve(&request).unwrap();

        assert_eq!(allocated.max_tokens, Some(300));
        assert_eq!(pool.available_tokens(), Some(700));
    }

    #[test]
    fn test_budget_pool_reclaim() {
        let pool = BudgetPool::new(BudgetLimits::default().with_max_tokens(1000));

        let request = BudgetLimits::default().with_max_tokens(300);
        let allocated = pool.reserve(&request).unwrap();

        // Only used 200 of 300 allocated
        pool.reclaim(&allocated, 200);

        // 100 should be returned
        assert_eq!(pool.available_tokens(), Some(800));
    }
}

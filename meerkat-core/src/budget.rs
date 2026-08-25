//! Budget enforcement for Meerkat
//!
//! Tracks and enforces resource limits (tokens, time, tool calls).

use crate::error::AgentError;
use crate::time_compat::{Duration, Instant};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Resource limits for an agent run
///
/// # Two time horizons, one owner
///
/// [`Budget`] is the single owner of "may this run continue in time". It
/// carries two independent horizons because they answer different questions
/// and are measured from different epochs:
///
/// - [`BudgetLimits::max_duration`] is the **agent-lifetime** horizon. Its
///   epoch is [`Budget::new`], which service-backed surfaces call once when the
///   session's agent is built, not once per turn. It therefore spans every turn
///   of that agent, including the idle wall-clock between turns.
/// - [`BudgetLimits::max_turn_duration`] is the **per-turn aggregate** horizon.
///   Its epoch is re-armed by [`Budget::begin_turn`] at each run entry, so it
///   bounds one turn end-to-end regardless of how many LLM calls, retries, and
///   tool batches that turn contains.
///
/// Every segment of a turn already carries its own bound (per-call LLM timeout,
/// stream-inactivity watchdog, per-tool-call timeout). Before
/// `max_turn_duration` existed, the *sum* of those segments was unbounded: a
/// turn could legally spend an hour without any owner asking whether it was
/// allowed to. `max_turn_duration` is that owner.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BudgetLimits {
    /// Maximum tokens to consume
    pub max_tokens: Option<u64>,
    /// Maximum agent-lifetime duration, measured from [`Budget::new`].
    ///
    /// This is NOT a per-turn deadline: the epoch is agent construction, which
    /// for `SessionService`-backed surfaces is session creation, and it is
    /// never re-armed. Use [`BudgetLimits::max_turn_duration`] to bound one
    /// turn.
    pub max_duration: Option<Duration>,
    /// Maximum aggregate wall-clock for a single turn, re-armed at each run
    /// entry by [`Budget::begin_turn`].
    ///
    /// `None` (the default) means turns are unbounded in aggregate: every
    /// segment still carries its own timeout, but their sum has no ceiling.
    /// Absence is representable and never substituted with an invented
    /// default; a deployment that leaves this unset has chosen unbounded
    /// turns.
    ///
    /// `skip_serializing_if` is load-bearing, not style: a spec carrying no
    /// turn ceiling must keep its historical canonical bytes so the frozen
    /// `spec_digest` pin (`meerkat_contracts::wire::spec_digest`) and every
    /// digest already recorded in host stores still match. Same reasoning as
    /// `PortableToolConfig.read_only` in 0.8.23.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turn_duration: Option<Duration>,
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

    /// Set max agent-lifetime duration
    pub fn with_max_duration(mut self, max: Duration) -> Self {
        self.max_duration = Some(max);
        self
    }

    /// Set the aggregate per-turn wall-clock ceiling
    pub fn with_max_turn_duration(mut self, max: Duration) -> Self {
        self.max_turn_duration = Some(max);
        self
    }

    /// Set max tool calls
    pub fn with_max_tool_calls(mut self, max: usize) -> Self {
        self.max_tool_calls = Some(max);
        self
    }
}

/// Budget tracker owned by one agent.
///
/// Single owner of "may this run continue in time" across both horizons
/// documented on [`BudgetLimits`]. Nothing else in the loop may decide that a
/// run has run out of time: every enforcement point calls [`Budget::observe`]
/// and routes the resulting [`BudgetExceeded`] through the turn authority.
#[derive(Debug)]
pub struct Budget {
    limits: BudgetLimits,
    accounting: Arc<BudgetAccounting>,
    /// Agent-lifetime horizon epoch. Set once at construction, never re-armed.
    start_time: Instant,
    /// Per-turn horizon epoch. Re-armed by [`Budget::begin_turn`] at run entry.
    turn_start: Instant,
}

#[derive(Debug)]
struct BudgetAccounting {
    tokens_used: AtomicU64,
    tool_calls_made: AtomicU64,
}

impl Budget {
    /// Create a new budget with the given limits
    pub fn new(limits: BudgetLimits) -> Self {
        let now = Instant::now();
        Self {
            limits,
            accounting: Arc::new(BudgetAccounting {
                tokens_used: AtomicU64::new(0),
                tool_calls_made: AtomicU64::new(0),
            }),
            start_time: now,
            turn_start: now,
        }
    }

    /// Re-arm the per-turn horizon at the start of a run.
    ///
    /// The agent loop calls this exactly once per run entry (`run`, and
    /// `run_pending` for a continuation). Turn wall-clock is measured from
    /// here, so retries, tool batches, and compaction inside the turn all draw
    /// on one monotonic clock and cannot be double-counted. A turn parked on a
    /// callback and later resumed re-arms: the parked wall-clock belongs to
    /// whoever chose when to resume, not to the loop.
    pub fn begin_turn(&mut self) {
        self.turn_start = Instant::now();
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

    /// Builder method for max agent-lifetime duration
    pub fn with_max_duration(mut self, max: Duration) -> Self {
        self.limits.max_duration = Some(max);
        self
    }

    /// Builder method for the aggregate per-turn wall-clock ceiling
    pub fn with_max_turn_duration(mut self, max: Duration) -> Self {
        self.limits.max_turn_duration = Some(max);
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
            let used = self.accounting.tokens_used.load(Ordering::Relaxed);
            if used >= limit {
                return BudgetObservation::Exceeded(BudgetExceeded {
                    dimension: BudgetDimension::Tokens,
                    used,
                    limit,
                });
            }
        }

        // Check the agent-lifetime time horizon
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

        // Check the per-turn aggregate time horizon.
        //
        // Failing closed is correct here, and it is NOT the same judgement as
        // an accounting or observability fault. A turn that has passed its
        // aggregate deadline has been *invalidated*: we can no longer promise
        // the caller when (or whether) it will produce output, and everything
        // downstream that waits on this turn has already been kept waiting
        // past the contract. That is a semantic fact about the turn, so it
        // terminalizes. It travels the pre-existing time terminal
        // (`BudgetDimension::Time` -> `TurnExecutionInput::BudgetLimitExceeded`
        // -> `TurnTerminalOutcome::TimeBudgetExceeded`) because "this run ran
        // out of time" is one condition with one canonical terminal, not two.
        //
        // The generated authority already draws exactly this distinction and
        // the turn horizon inherits it unchanged: in
        // `generated::terminal_surface_mapping`, `(BudgetExhausted,
        // BudgetExhausted)` classifies as `Success` - an orderly stop that
        // still answers the caller - while `(TimeBudgetExceeded,
        // TimeBudgetExceeded)` classifies as `HardFailure`. A spent token or
        // tool-call budget ends a turn; a spent deadline invalidates it.
        if let Some(limit) = self.limits.max_turn_duration {
            let elapsed = self.turn_start.elapsed();
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
            let count = self.accounting.tool_calls_made.load(Ordering::Relaxed) as usize;
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
        self.accounting
            .tokens_used
            .fetch_add(tokens, Ordering::Relaxed);
    }

    /// Record tool calls
    pub fn record_calls(&self, count: usize) {
        self.accounting
            .tool_calls_made
            .fetch_add(count as u64, Ordering::Relaxed);
    }

    /// Record one provider turn from normalized accounting evidence.
    pub fn record_turn_usage(&self, usage: &crate::types::TurnUsage) {
        self.record_tokens(usage.normalized_total_tokens());
    }

    /// Record a single tool call
    pub fn record_tool_call(&self) {
        self.record_calls(1);
    }

    /// Get token usage (used, limit) if limit is set
    pub fn token_usage(&self) -> Option<(u64, u64)> {
        self.limits
            .max_tokens
            .map(|limit| (self.accounting.tokens_used.load(Ordering::Relaxed), limit))
    }

    /// The time horizon that currently binds, as `(elapsed, limit)`.
    ///
    /// Single owner of "which clock is about to stop this run": when both
    /// horizons are configured, the binding one is whichever has less time
    /// left. `None` means no time horizon is configured at all: absence is
    /// reported, never substituted with an invented limit.
    fn binding_time_horizon(&self) -> Option<(Duration, Duration)> {
        let lifetime = self
            .limits
            .max_duration
            .map(|limit| (self.start_time.elapsed(), limit));
        let turn = self
            .limits
            .max_turn_duration
            .map(|limit| (self.turn_start.elapsed(), limit));
        match (lifetime, turn) {
            (None, None) => None,
            (Some(horizon), None) | (None, Some(horizon)) => Some(horizon),
            (Some(lifetime), Some(turn)) => {
                let lifetime_left = lifetime.1.saturating_sub(lifetime.0);
                let turn_left = turn.1.saturating_sub(turn.0);
                Some(if turn_left < lifetime_left {
                    turn
                } else {
                    lifetime
                })
            }
        }
    }

    /// Get time usage (elapsed_ms, limit_ms) of the binding horizon, if any
    /// time horizon is set.
    pub fn time_usage(&self) -> Option<(u64, u64)> {
        self.binding_time_horizon()
            .map(|(elapsed, limit)| (elapsed.as_millis() as u64, limit.as_millis() as u64))
    }

    /// Get call usage (count, limit) if limit is set
    pub fn call_usage(&self) -> Option<(usize, usize)> {
        self.limits.max_tool_calls.map(|limit| {
            (
                self.accounting.tool_calls_made.load(Ordering::Relaxed) as usize,
                limit,
            )
        })
    }

    /// Get remaining tokens (None if unlimited)
    pub fn remaining_tokens(&self) -> Option<u64> {
        self.limits.max_tokens.map(|limit| {
            let used = self.accounting.tokens_used.load(Ordering::Relaxed);
            limit.saturating_sub(used)
        })
    }

    /// Remaining time on the binding horizon (None if no time horizon is set).
    ///
    /// The LLM-call gate wraps each provider call with this value, so a single
    /// call can never outlive the horizon that will terminalize the turn, and
    /// retry sleeps are capped by it. Because the turn horizon is one
    /// monotonic clock from run entry, retries draw on it exactly once.
    pub fn remaining_duration(&self) -> Option<Duration> {
        self.binding_time_horizon()
            .map(|(elapsed, limit)| limit.saturating_sub(elapsed))
    }

    /// Fork an operation-local turn clock while retaining this agent's exact
    /// lifetime token and tool-call accounting.
    ///
    /// Noncommitting live-bridge execution uses this narrow seam so concurrent
    /// ordinary and bridge turns cannot overwrite each other's per-turn clock,
    /// while neither path can evade the durable member's aggregate limits.
    pub(crate) fn fork_shared_accounting(&self) -> Self {
        Self {
            limits: self.limits.clone(),
            accounting: Arc::clone(&self.accounting),
            start_time: self.start_time,
            turn_start: Instant::now(),
        }
    }
}

impl Clone for Budget {
    fn clone(&self) -> Self {
        Self {
            limits: self.limits.clone(),
            accounting: Arc::new(BudgetAccounting {
                tokens_used: AtomicU64::new(self.accounting.tokens_used.load(Ordering::Relaxed)),
                tool_calls_made: AtomicU64::new(
                    self.accounting.tool_calls_made.load(Ordering::Relaxed),
                ),
            }),
            start_time: self.start_time,
            turn_start: self.turn_start,
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
            // A branch's turn ceiling is capped by what the pool has left, but
            // an absent request stays absent: the pool never invents a turn
            // ceiling for a branch that did not ask for one.
            max_turn_duration: request
                .max_turn_duration
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

    /// The per-turn horizon is exhausted by wall-clock alone and reports the
    /// same `Time` dimension as the agent-lifetime horizon, so it reaches the
    /// one canonical time terminal rather than a second path of its own.
    #[test]
    fn turn_horizon_exhausts_on_wall_clock() {
        let budget =
            Budget::new(BudgetLimits::default().with_max_turn_duration(Duration::from_millis(10)));

        assert_eq!(budget.observe(), BudgetObservation::WithinLimit);

        std::thread::sleep(std::time::Duration::from_millis(25));

        let exceeded = budget
            .observe()
            .exceeded()
            .expect("turn horizon must be exhausted after its wall-clock elapses");
        assert_eq!(exceeded.dimension, BudgetDimension::Time);
        assert!(matches!(
            exceeded.to_agent_error(),
            AgentError::TimeBudgetExceeded { .. }
        ));
        assert_eq!(budget.remaining_duration(), Some(Duration::ZERO));
    }

    /// `begin_turn` re-arms the turn horizon and ONLY the turn horizon. This
    /// is the fact the agent-lifetime horizon cannot express: its epoch is
    /// agent construction, so on a long-lived agent it measures the session,
    /// never the turn.
    #[test]
    fn begin_turn_rearms_only_the_turn_horizon() {
        let mut budget = Budget::new(BudgetLimits {
            max_tokens: None,
            max_duration: Some(Duration::from_millis(10)),
            max_turn_duration: Some(Duration::from_millis(10)),
            max_tool_calls: None,
        });

        std::thread::sleep(std::time::Duration::from_millis(25));
        assert!(
            budget.observe().exceeded().is_some(),
            "both horizons are past their limit before re-arming"
        );

        budget.begin_turn();

        let exceeded = budget
            .observe()
            .exceeded()
            .expect("the agent-lifetime horizon must survive a turn re-arm");
        assert_eq!(exceeded.dimension, BudgetDimension::Time);

        // With the lifetime horizon removed, the same re-arm leaves the turn
        // horizon fresh: re-arming is a turn fact, not an amnesty.
        let mut turn_only =
            Budget::new(BudgetLimits::default().with_max_turn_duration(Duration::from_millis(10)));
        std::thread::sleep(std::time::Duration::from_millis(25));
        assert!(turn_only.observe().exceeded().is_some());
        turn_only.begin_turn();
        assert_eq!(turn_only.observe(), BudgetObservation::WithinLimit);
    }

    /// Both horizons configured: the one with less time left is the one the
    /// loop is told about, so a call wrapped with `remaining_duration` can
    /// never outlive whichever horizon will terminalize the turn.
    #[test]
    fn binding_time_horizon_is_the_one_closest_to_exhaustion() {
        let budget = Budget::new(BudgetLimits {
            max_tokens: None,
            max_duration: Some(Duration::from_secs(3600)),
            max_turn_duration: Some(Duration::from_secs(10)),
            max_tool_calls: None,
        });

        let remaining = budget
            .remaining_duration()
            .expect("a configured horizon reports remaining time");
        assert!(
            remaining <= Duration::from_secs(10),
            "the turn horizon binds: {remaining:?}"
        );
        let (_, limit_ms) = budget.time_usage().expect("binding horizon reports usage");
        assert_eq!(
            limit_ms, 10_000,
            "usage reports the binding horizon's limit"
        );
    }

    /// A pool never invents a turn ceiling for a branch that did not ask for
    /// one: absence is preserved through allocation.
    #[test]
    fn pool_reserve_preserves_absent_turn_horizon() {
        let pool = BudgetPool::new(
            BudgetLimits::default()
                .with_max_tokens(1000)
                .with_max_duration(Duration::from_secs(60)),
        );

        let allocated = pool
            .reserve(&BudgetLimits::default().with_max_tokens(100))
            .expect("reserve succeeds");
        assert_eq!(allocated.max_turn_duration, None);

        let capped = pool
            .reserve(&BudgetLimits::default().with_max_turn_duration(Duration::from_secs(600)))
            .expect("reserve succeeds");
        assert!(
            capped
                .max_turn_duration
                .is_some_and(|turn| turn <= Duration::from_secs(60)),
            "a branch turn ceiling cannot exceed what the pool has left"
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

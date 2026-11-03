//! Background-work resource guards (§8.1).
//!
//! Every judgment stage burns LLM calls, and Meta multiplies stages by
//! identities × interactions × mobs. Stage-level throttles scale *with*
//! activity; these guards are the deterministic containment on top: hard
//! per-window caps on background runs and a concurrency ceiling, consulted
//! before every run. A skipped run is loud: a tracing warn always, plus a
//! `memory.budget.denied` timeline event when a sink is wired (Principle 6).
//!
//! The load-*inverse* control Codex ships (skip background work below
//! provider rate-limit headroom) needs a provider-quota surface Meta does
//! not have; until then the per-realm window budget is the stand-in, and
//! the §16 open question on default budgets is answered by measurement.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::memory::events::{MemoryEventSink, MemoryTimelineEvent};

/// Default hard cap on background runs per realm per window (§8.1; the
/// concrete number is §16 open question 5, this is the measured starting
/// point).
pub const DEFAULT_RUNS_PER_HOUR: u32 = 12;
/// Default background-run concurrency per realm.
pub const DEFAULT_MAX_CONCURRENT: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundBudgetConfig {
    pub runs_per_window: u32,
    pub window: Duration,
    pub max_concurrent: u32,
}

impl Default for BackgroundBudgetConfig {
    fn default() -> Self {
        Self {
            runs_per_window: DEFAULT_RUNS_PER_HOUR,
            window: Duration::from_hours(1),
            max_concurrent: DEFAULT_MAX_CONCURRENT,
        }
    }
}

#[derive(Debug, Default)]
struct RealmBudgetState {
    /// Start times of runs admitted within the sliding window.
    starts: Vec<Instant>,
    concurrent: u32,
}

struct BudgetInner {
    config: BackgroundBudgetConfig,
    realms: HashMap<String, RealmBudgetState>,
    event_sink: Option<Arc<dyn MemoryEventSink>>,
}

/// Per-realm background budget (§8.1): a sliding-window run cap plus a
/// concurrency ceiling. Cheap to clone; clones share state.
#[derive(Clone)]
pub struct BackgroundBudget {
    inner: Arc<Mutex<BudgetInner>>,
}

/// RAII permit for one admitted background run; dropping it releases the
/// concurrency slot (the window slot is consumed permanently).
pub struct BudgetPermit {
    inner: Arc<Mutex<BudgetInner>>,
    realm: String,
}

impl std::fmt::Debug for BudgetPermit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BudgetPermit")
            .field("realm", &self.realm)
            .finish_non_exhaustive()
    }
}

impl Drop for BudgetPermit {
    fn drop(&mut self) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(state) = inner.realms.get_mut(&self.realm) {
            state.concurrent = state.concurrent.saturating_sub(1);
        }
    }
}

/// Why a run was denied. Carried in the warn log and (P3b) timeline event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BudgetDenied {
    WindowExhausted { used: u32, cap: u32 },
    ConcurrencyCeiling { cap: u32 },
}

impl std::fmt::Display for BudgetDenied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WindowExhausted { used, cap } => {
                write!(f, "window budget exhausted ({used}/{cap} runs)")
            }
            Self::ConcurrencyCeiling { cap } => {
                write!(f, "concurrency ceiling reached ({cap} in flight)")
            }
        }
    }
}

impl BackgroundBudget {
    pub fn new(config: BackgroundBudgetConfig) -> Self {
        Self {
            inner: Arc::new(Mutex::new(BudgetInner {
                config,
                realms: HashMap::new(),
                event_sink: None,
            })),
        }
    }

    /// Wire the §9.3 timeline sink so guard denials surface on the console
    /// alongside the tracing warn. Shared across clones.
    pub fn set_event_sink(&self, sink: Arc<dyn MemoryEventSink>) {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .event_sink = Some(sink);
    }

    /// Admit one background run for `realm`, or say loudly why not.
    /// `stage` is only for the log line.
    pub fn try_acquire(&self, realm: &str, stage: &str) -> Result<BudgetPermit, BudgetDenied> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let window = inner.config.window;
        let runs_cap = inner.config.runs_per_window;
        let concurrent_cap = inner.config.max_concurrent;
        let state = inner.realms.entry(realm.to_string()).or_default();
        let now = Instant::now();
        state
            .starts
            .retain(|start| now.duration_since(*start) < window);
        let denied = if state.concurrent >= concurrent_cap {
            Some(BudgetDenied::ConcurrencyCeiling {
                cap: concurrent_cap,
            })
        } else if state.starts.len() as u32 >= runs_cap {
            Some(BudgetDenied::WindowExhausted {
                used: state.starts.len() as u32,
                cap: runs_cap,
            })
        } else {
            None
        };
        if let Some(denied) = denied {
            tracing::warn!(
                realm,
                stage,
                reason = %denied,
                "agent memory background budget: run skipped"
            );
            if let Some(sink) = inner.event_sink.as_ref() {
                sink.emit(MemoryTimelineEvent::BudgetDenied {
                    realm: realm.to_string(),
                    stage: stage.to_string(),
                    reason: denied.to_string(),
                });
            }
            return Err(denied);
        }
        state.starts.push(now);
        state.concurrent += 1;
        Ok(BudgetPermit {
            inner: self.inner.clone(),
            realm: realm.to_string(),
        })
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    fn config(runs: u32, concurrent: u32) -> BackgroundBudgetConfig {
        BackgroundBudgetConfig {
            runs_per_window: runs,
            window: Duration::from_hours(1),
            max_concurrent: concurrent,
        }
    }

    #[test]
    fn window_cap_denies_after_budget_spent() {
        let budget = BackgroundBudget::new(config(2, 10));
        let p1 = budget.try_acquire("realm-a", "distiller").expect("run 1");
        drop(p1);
        let p2 = budget.try_acquire("realm-a", "distiller").expect("run 2");
        drop(p2);
        let denied = budget
            .try_acquire("realm-a", "distiller")
            .expect_err("third run in window must deny");
        assert!(
            matches!(denied, BudgetDenied::WindowExhausted { used: 2, cap: 2 }),
            "{denied:?}"
        );
        // Budgets are per realm: another realm is unaffected.
        budget
            .try_acquire("realm-b", "distiller")
            .expect("other realm has its own window");
    }

    #[test]
    fn denial_emits_timeline_event_when_sink_wired() {
        let budget = BackgroundBudget::new(config(1, 10));
        let sink = Arc::new(crate::memory::events::CollectingEventSink::new());
        budget.set_event_sink(sink.clone());
        let _permit = budget.try_acquire("realm-a", "steward").expect("first");
        let _ = budget
            .try_acquire("realm-a", "steward")
            .expect_err("window spent");
        assert_eq!(sink.types(), vec!["memory.budget.denied"]);
        let events = sink.events.lock().unwrap();
        assert!(matches!(
            &events[0],
            MemoryTimelineEvent::BudgetDenied { realm, stage, .. }
                if realm == "realm-a" && stage == "steward"
        ));
    }

    #[test]
    fn concurrency_ceiling_releases_on_drop() {
        let budget = BackgroundBudget::new(config(10, 1));
        let permit = budget.try_acquire("realm-a", "distiller").expect("first");
        let denied = budget
            .try_acquire("realm-a", "distiller")
            .expect_err("second concurrent run must deny");
        assert!(
            matches!(denied, BudgetDenied::ConcurrencyCeiling { cap: 1 }),
            "{denied:?}"
        );
        drop(permit);
        budget
            .try_acquire("realm-a", "distiller")
            .expect("slot released on drop");
    }
}

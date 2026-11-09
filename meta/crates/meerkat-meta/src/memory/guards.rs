//! Background-work resource guards (§8.1).
//!
//! Every judgment stage burns LLM calls, and Meta multiplies stages by
//! identities × interactions × mobs. Stage-level throttles scale *with*
//! activity; these guards are the deterministic containment on top: hard
//! per-window caps on background runs and a concurrency ceiling, consulted
//! before every run. A skipped run is loud: a tracing warn always, plus a
//! `memory.budget.denied` timeline event when a sink is wired (Principle 6).
//! A run whose caller waits for the budget instead (the lifecycle
//! projection's compaction harvest) is not skipped: it is asked again once
//! the budget may admit it, and its caller reports a denial only when it
//! gives the run up.
//!
//! The load-*inverse* control Codex ships (skip background work below
//! provider rate-limit headroom) needs a provider-quota surface Meta does
//! not have; until then the per-realm window budget is the stand-in, and
//! the §16 open question on default budgets is answered by measurement.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tokio::sync::Notify;

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

impl BudgetInner {
    /// Why a run of `realm` would not be admitted at `now`, or `None` when
    /// it would be. The window forgets the runs it no longer holds.
    fn denial(&mut self, realm: &str, now: Instant) -> Option<BudgetDenied> {
        let window = self.config.window;
        let runs_cap = self.config.runs_per_window;
        let concurrent_cap = self.config.max_concurrent;
        let state = self.realms.entry(realm.to_string()).or_default();
        state
            .starts
            .retain(|start| now.duration_since(*start) < window);
        if state.concurrent >= concurrent_cap {
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
        }
    }

    /// When the window of `realm` frees a run: when its oldest run leaves
    /// it. `None` when it holds none (a window that admits no run never
    /// frees one).
    fn window_frees_at(&self, realm: &str) -> Option<Instant> {
        self.realms
            .get(realm)
            .and_then(|state| state.starts.iter().min())
            .map(|oldest| *oldest + self.config.window)
    }
}

/// Per-realm background budget (§8.1): a sliding-window run cap plus a
/// concurrency ceiling. Cheap to clone; clones share state.
#[derive(Clone)]
pub struct BackgroundBudget {
    inner: Arc<Mutex<BudgetInner>>,
    /// Woken each time an admitted run ends (its permit is dropped): a run
    /// waiting for the concurrency ceiling may be admitted then.
    released: Arc<Notify>,
}

/// RAII permit for one admitted background run; dropping it releases the
/// concurrency slot (the window slot is consumed permanently) and wakes the
/// runs that wait for the budget.
pub struct BudgetPermit {
    inner: Arc<Mutex<BudgetInner>>,
    released: Arc<Notify>,
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
        {
            let mut inner = self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(state) = inner.realms.get_mut(&self.realm) {
                state.concurrent = state.concurrent.saturating_sub(1);
            }
        }
        self.released.notify_waiters();
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
            released: Arc::new(Notify::new()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, BudgetInner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Wire the §9.3 timeline sink so guard denials surface on the console
    /// alongside the tracing warn. Shared across clones.
    pub fn set_event_sink(&self, sink: Arc<dyn MemoryEventSink>) {
        self.lock().event_sink = Some(sink);
    }

    /// Admit one background run for `realm`, or say loudly why not.
    /// `stage` is only for the log line.
    pub fn try_acquire(&self, realm: &str, stage: &str) -> Result<BudgetPermit, BudgetDenied> {
        self.try_admit(realm)
            .inspect_err(|denied| self.report_denied(realm, stage, denied))
    }

    /// Admit one background run for `realm`, or answer why not, unreported:
    /// a caller that waits for the budget asks again once it may admit the
    /// run ([`Self::until_admissible`]), and one that gives the run up
    /// reports the denial ([`Self::report_denied`]).
    pub(crate) fn try_admit(&self, realm: &str) -> Result<BudgetPermit, BudgetDenied> {
        let mut inner = self.lock();
        let now = Instant::now();
        if let Some(denied) = inner.denial(realm, now) {
            return Err(denied);
        }
        let state = inner.realms.entry(realm.to_string()).or_default();
        state.starts.push(now);
        state.concurrent += 1;
        Ok(BudgetPermit {
            inner: Arc::clone(&self.inner),
            released: Arc::clone(&self.released),
            realm: realm.to_string(),
        })
    }

    /// Say loudly that a run of `realm` is given up on `denied`: a tracing
    /// warn always, plus the `memory.budget.denied` timeline event when a
    /// sink is wired. `stage` names the run in both.
    pub(crate) fn report_denied(&self, realm: &str, stage: &str, denied: &BudgetDenied) {
        tracing::warn!(
            realm,
            stage,
            reason = %denied,
            "agent memory background budget: run skipped"
        );
        let sink = self.lock().event_sink.clone();
        if let Some(sink) = sink {
            sink.emit(MemoryTimelineEvent::BudgetDenied {
                realm: realm.to_string(),
                stage: stage.to_string(),
                reason: denied.to_string(),
            });
        }
    }

    /// Wait until the budget may admit a run of `realm`, at most `wait`.
    /// Answers `Ok` once a concurrency slot and a window slot are free; the
    /// run is not admitted by that: [`Self::try_admit`] takes them, and may
    /// find them taken again. It is woken each time an admitted run ends, and
    /// when the window frees a run. Answers the denial that stood when the
    /// wait ran out, and a window's at once when the window frees no run
    /// within the wait. Unreported, like [`Self::try_admit`].
    pub(crate) async fn until_admissible(
        &self,
        realm: &str,
        wait: Duration,
    ) -> Result<(), BudgetDenied> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            // Asked for before the budget is read: a run that ends between
            // the read and the wait still wakes it.
            let released = self.released.notified();
            let (denied, window_frees_at) = {
                let mut inner = self.lock();
                match inner.denial(realm, Instant::now()) {
                    None => return Ok(()),
                    Some(denied) => {
                        let frees_at = inner.window_frees_at(realm);
                        (denied, frees_at)
                    }
                }
            };
            let wake_at = match &denied {
                BudgetDenied::ConcurrencyCeiling { .. } => deadline,
                BudgetDenied::WindowExhausted { .. } => {
                    match window_frees_at.map(tokio::time::Instant::from_std) {
                        Some(frees_at) if frees_at <= deadline => frees_at,
                        // The window frees no run within the wait.
                        _ => return Err(denied),
                    }
                }
            };
            if tokio::time::Instant::now() >= deadline {
                return Err(denied);
            }
            tokio::select! {
                () = released => {}
                () = tokio::time::sleep_until(wake_at) => {}
            }
        }
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

    /// A run whose caller waits for the budget is denied unreported: no
    /// timeline event, until its caller gives the run up and reports it.
    #[test]
    fn an_admission_its_caller_waits_for_is_denied_unreported() {
        let budget = BackgroundBudget::new(config(10, 1));
        let sink = Arc::new(crate::memory::events::CollectingEventSink::new());
        budget.set_event_sink(sink.clone());
        let _permit = budget.try_admit("realm-a").expect("first");
        let denied = budget
            .try_admit("realm-a")
            .expect_err("the ceiling is reached");
        assert!(
            matches!(denied, BudgetDenied::ConcurrencyCeiling { cap: 1 }),
            "{denied:?}"
        );
        assert!(sink.types().is_empty(), "{:?}", sink.types());
        budget.report_denied("realm-a", "distiller", &denied);
        assert_eq!(sink.types(), vec!["memory.budget.denied"]);
    }

    /// A wait for the concurrency ceiling ends as soon as the run that holds
    /// it ends: its dropped permit wakes the wait.
    #[tokio::test]
    async fn a_wait_for_the_ceiling_ends_when_the_run_holding_it_ends() {
        let budget = BackgroundBudget::new(config(10, 1));
        let permit = budget.try_admit("realm-a").expect("first");
        let waiting = tokio::spawn({
            let budget = budget.clone();
            async move {
                budget
                    .until_admissible("realm-a", Duration::from_secs(30))
                    .await
            }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!waiting.is_finished(), "the ceiling is reached: it waits");
        drop(permit);
        tokio::time::timeout(Duration::from_secs(5), waiting)
            .await
            .expect("the dropped permit wakes the wait")
            .expect("the wait task")
            .expect("the budget may admit a run");
        budget.try_admit("realm-a").expect("and admits one");
    }

    /// A wait the budget does not end within it answers the denial that
    /// stood when it ran out.
    #[tokio::test]
    async fn a_wait_the_budget_does_not_end_answers_its_denial() {
        let budget = BackgroundBudget::new(config(10, 1));
        let _permit = budget.try_admit("realm-a").expect("first");
        let started = std::time::Instant::now();
        let denied = budget
            .until_admissible("realm-a", Duration::from_millis(100))
            .await
            .expect_err("the ceiling stays reached");
        assert!(
            matches!(denied, BudgetDenied::ConcurrencyCeiling { cap: 1 }),
            "{denied:?}"
        );
        assert!(
            started.elapsed() >= Duration::from_millis(100),
            "it waited its whole wait: {:?}",
            started.elapsed()
        );
    }

    /// A window that frees no run within the wait answers at once, and one
    /// that frees a run within it ends the wait then.
    #[tokio::test]
    async fn a_wait_for_the_window_ends_when_it_frees_a_run_within_the_wait() {
        let budget = BackgroundBudget::new(BackgroundBudgetConfig {
            runs_per_window: 1,
            window: Duration::from_secs(1),
            max_concurrent: 10,
        });
        drop(budget.try_admit("realm-a").expect("the window's run"));
        let started = std::time::Instant::now();
        let denied = budget
            .until_admissible("realm-a", Duration::from_millis(500))
            .await
            .expect_err("the window frees no run within 500 ms");
        assert!(
            matches!(denied, BudgetDenied::WindowExhausted { used: 1, cap: 1 }),
            "{denied:?}"
        );
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "answered at once: {:?}",
            started.elapsed()
        );
        budget
            .until_admissible("realm-a", Duration::from_secs(5))
            .await
            .expect("the window frees its run within the wait");
        budget.try_admit("realm-a").expect("and admits one");
    }
}

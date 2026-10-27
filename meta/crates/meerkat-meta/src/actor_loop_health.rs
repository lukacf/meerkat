//! Shared observation of the mob actor loop's liveness.
//!
//! meerkat's mob actor is ONE serialized command loop. The unified runtime's
//! probe (`run_actor_loop_probe`) is the only unconditional witness of that
//! loop, and until now its verdict lived solely in the paging channel
//! (`ErrorEvent::ActorLoopStalled` / `ActorLoopRecovered`). The delivery path
//! could not see it: a send issued while a stall was open queued behind the
//! wedged command and waited the full admission budget (600 s by default)
//! before failing with a scope-less timeout. Production 2026-09-04 (operator deployment): five
//! console sends, each abandoned after exactly 600 s, while the probe had
//! already paged the stall.
//!
//! This module is the seam between the two. The probe publishes its verdict
//! here; the bridge's admission path reads it and fails fast, typed and naming
//! the open `stall_id`, instead of waiting. It is OBSERVATION, not authority:
//! nothing here mutates the actor, and a `Live` reading is never proof of
//! health (the probe measures whether the loop drains, not whether the mob is
//! healthy).
//!
//! Three states, deliberately not a boolean:
//!
//! - [`ActorLoopHealthState::Live`]: no open stall. Deliveries proceed under
//!   their ordinary admission budget.
//! - [`ActorLoopHealthState::Stalled`]: the probe's round trip is parked
//!   unanswered. Deliveries fail fast until the probe closes this exact
//!   `stall_id`.
//! - [`ActorLoopHealthState::Terminated`]: the parked probe resolved with the
//!   actor's channels CLOSED. The loop did not recover; it is gone. Nothing in
//!   this process can bring it back, so the state is terminal and deliveries
//!   fail fast with a distinct error telling the operator to restart.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

/// One probe verdict about the serialized mob actor loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActorLoopHealthState {
    /// No open stall.
    Live,
    /// The probe's round trip has been parked unanswered past its budget.
    Stalled {
        /// The `ErrorEvent::ActorLoopStalled { stall_id }` this corresponds
        /// to, so an operator can join the fast-failed delivery to the page.
        stall_id: u64,
        /// When the probe opened the stall (monotonic).
        since: Instant,
    },
    /// The actor's command or reply channel closed: the loop terminated.
    /// Terminal for the process; only a restart clears it.
    Terminated {
        /// The stall this resolved, when the termination was observed on a
        /// parked probe rather than on a fresh round trip.
        stall_id: Option<u64>,
        /// The error text the probe observed.
        detail: String,
        /// When the termination was observed (monotonic).
        at: Instant,
    },
}

impl ActorLoopHealthState {
    /// The open stall id, when the loop is stalled or terminated during a
    /// stall.
    #[must_use]
    pub fn open_stall_id(&self) -> Option<u64> {
        match self {
            Self::Live => None,
            Self::Stalled { stall_id, .. } => Some(*stall_id),
            Self::Terminated { stall_id, .. } => *stall_id,
        }
    }

    /// Whether the delivery path must fail fast instead of queueing a
    /// command onto the actor.
    #[must_use]
    pub fn refuses_admission(&self) -> bool {
        !matches!(self, Self::Live)
    }

    /// Wire projection for `meta/member_health` and operator surfaces.
    #[must_use]
    pub fn report(&self) -> ActorLoopHealthReport {
        match self {
            Self::Live => ActorLoopHealthReport {
                state: ActorLoopHealthKind::Live,
                stall_id: None,
                stalled_for_secs: None,
                detail: None,
            },
            Self::Stalled { stall_id, since } => ActorLoopHealthReport {
                state: ActorLoopHealthKind::Stalled,
                stall_id: Some(*stall_id),
                stalled_for_secs: Some(since.elapsed().as_secs()),
                detail: None,
            },
            Self::Terminated {
                stall_id,
                detail,
                at,
            } => ActorLoopHealthReport {
                state: ActorLoopHealthKind::Terminated,
                stall_id: *stall_id,
                stalled_for_secs: Some(at.elapsed().as_secs()),
                detail: Some(detail.clone()),
            },
        }
    }
}

/// Wire vocabulary for [`ActorLoopHealthReport::state`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorLoopHealthKind {
    Live,
    Stalled,
    Terminated,
    /// No probe is wired into this runtime (validation-only compositions),
    /// so the loop's liveness is not observed. Distinct from `Live` so a
    /// reader cannot mistake "nobody is watching" for "healthy".
    Unobserved,
}

/// Serializable projection of the actor loop's health for operator reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActorLoopHealthReport {
    pub state: ActorLoopHealthKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stall_id: Option<u64>,
    /// Seconds since the stall opened (`stalled`) or since termination was
    /// observed (`terminated`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stalled_for_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl ActorLoopHealthReport {
    /// The report for a runtime that has no probe wired.
    #[must_use]
    pub fn unobserved() -> Self {
        Self {
            state: ActorLoopHealthKind::Unobserved,
            stall_id: None,
            stalled_for_secs: None,
            detail: None,
        }
    }
}

/// Shared, late-bound verdict slot. The probe writes; admission paths read
/// and can await the next unhealthy transition.
#[derive(Debug)]
pub struct ActorLoopHealth {
    state: watch::Sender<ActorLoopHealthState>,
}

impl Default for ActorLoopHealth {
    fn default() -> Self {
        Self::new()
    }
}

impl ActorLoopHealth {
    /// A fresh slot reading `Live`.
    #[must_use]
    pub fn new() -> Self {
        let (state, _) = watch::channel(ActorLoopHealthState::Live);
        Self { state }
    }

    /// Convenience for composition roots.
    #[must_use]
    pub fn shared() -> Arc<Self> {
        Arc::new(Self::new())
    }

    /// Current verdict.
    #[must_use]
    pub fn snapshot(&self) -> ActorLoopHealthState {
        self.state.borrow().clone()
    }

    /// Wire projection of the current verdict.
    #[must_use]
    pub fn report(&self) -> ActorLoopHealthReport {
        self.snapshot().report()
    }

    /// The probe opened a stall. A terminated loop never goes back to
    /// stalled: termination is terminal for the process.
    pub fn mark_stalled(&self, stall_id: u64) {
        self.state.send_if_modified(|state| match state {
            ActorLoopHealthState::Terminated { .. } => false,
            _ => {
                *state = ActorLoopHealthState::Stalled {
                    stall_id,
                    since: Instant::now(),
                };
                true
            }
        });
    }

    /// The probe's parked round trip for `stall_id` drained. Only the stall
    /// that is actually open is closed; a late resolution for an older id
    /// cannot clear a newer stall.
    pub fn mark_recovered(&self, stall_id: u64) {
        self.state.send_if_modified(|state| match state {
            ActorLoopHealthState::Stalled { stall_id: open, .. } if *open == stall_id => {
                *state = ActorLoopHealthState::Live;
                true
            }
            _ => false,
        });
    }

    /// The probe observed the actor's channels closed. Terminal.
    pub fn mark_terminated(&self, stall_id: Option<u64>, detail: impl Into<String>) {
        let detail = detail.into();
        self.state.send_if_modified(|state| match state {
            ActorLoopHealthState::Terminated { .. } => false,
            _ => {
                *state = ActorLoopHealthState::Terminated {
                    stall_id,
                    detail,
                    at: Instant::now(),
                };
                true
            }
        });
    }

    /// Resolve as soon as the verdict is (or becomes) `Stalled` or
    /// `Terminated`. Never resolves while the loop stays `Live`, so callers
    /// race it against their own bounded call rather than awaiting it alone.
    pub async fn unhealthy(&self) -> ActorLoopHealthState {
        let mut rx = self.state.subscribe();
        loop {
            {
                let current = rx.borrow_and_update();
                if current.refuses_admission() {
                    return current.clone();
                }
            }
            if rx.changed().await.is_err() {
                // The sender is gone only when the runtime that owned the
                // probe is gone; nothing left to observe, park forever so the
                // caller's own deadline decides.
                std::future::pending::<()>().await;
            }
        }
    }

    /// How long the current stall has been open, when one is open.
    #[must_use]
    pub fn open_stall_duration(&self) -> Option<Duration> {
        match self.snapshot() {
            ActorLoopHealthState::Stalled { since, .. } => Some(since.elapsed()),
            _ => None,
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn starts_live_and_reports_no_stall() {
        let health = ActorLoopHealth::new();
        assert_eq!(health.snapshot(), ActorLoopHealthState::Live);
        assert_eq!(health.report().state, ActorLoopHealthKind::Live);
        assert!(health.snapshot().open_stall_id().is_none());
        assert!(!health.snapshot().refuses_admission());
    }

    #[test]
    fn stall_then_recovery_round_trips_by_id() {
        let health = ActorLoopHealth::new();
        health.mark_stalled(3);
        assert_eq!(health.snapshot().open_stall_id(), Some(3));
        assert!(health.snapshot().refuses_admission());
        // A stale resolution must not clear a different open stall.
        health.mark_recovered(2);
        assert_eq!(health.snapshot().open_stall_id(), Some(3));
        health.mark_recovered(3);
        assert_eq!(health.snapshot(), ActorLoopHealthState::Live);
    }

    #[test]
    fn termination_is_terminal() {
        let health = ActorLoopHealth::new();
        health.mark_stalled(1);
        health.mark_terminated(Some(1), "mob actor command channel closed");
        assert!(matches!(
            health.snapshot(),
            ActorLoopHealthState::Terminated {
                stall_id: Some(1),
                ..
            }
        ));
        // Neither a later stall nor a recovery can undo termination.
        health.mark_stalled(2);
        health.mark_recovered(1);
        assert!(matches!(
            health.snapshot(),
            ActorLoopHealthState::Terminated { .. }
        ));
        let report = health.report();
        assert_eq!(report.state, ActorLoopHealthKind::Terminated);
        assert_eq!(
            report.detail.as_deref(),
            Some("mob actor command channel closed")
        );
    }

    #[tokio::test]
    async fn unhealthy_resolves_immediately_when_already_stalled() {
        let health = ActorLoopHealth::new();
        health.mark_stalled(7);
        let state = tokio::time::timeout(Duration::from_millis(50), health.unhealthy())
            .await
            .expect("an open stall must resolve the wait immediately");
        assert_eq!(state.open_stall_id(), Some(7));
    }

    #[tokio::test]
    async fn unhealthy_wakes_when_a_stall_opens_and_parks_while_live() {
        let health = Arc::new(ActorLoopHealth::new());
        assert!(
            tokio::time::timeout(Duration::from_millis(20), health.unhealthy())
                .await
                .is_err(),
            "a live loop must not resolve the unhealthy wait"
        );
        let waiter = {
            let health = Arc::clone(&health);
            tokio::spawn(async move { health.unhealthy().await })
        };
        tokio::task::yield_now().await;
        health.mark_stalled(11);
        let state = tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("the waiter must wake on the stall")
            .expect("waiter task");
        assert_eq!(state.open_stall_id(), Some(11));
    }

    #[test]
    fn report_wire_shape_is_snake_case_and_minimal() {
        let live = serde_json::to_value(ActorLoopHealthReport::unobserved()).expect("serialize");
        assert_eq!(live, serde_json::json!({ "state": "unobserved" }));
        let health = ActorLoopHealth::new();
        health.mark_stalled(5);
        let stalled = serde_json::to_value(health.report()).expect("serialize");
        assert_eq!(stalled["state"], serde_json::json!("stalled"));
        assert_eq!(stalled["stall_id"], serde_json::json!(5));
        assert!(stalled.get("detail").is_none());
    }
}

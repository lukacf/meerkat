//! [`ToolGrantDeadlines`]: the deadline driver of one runtime owner's
//! served sessions.
//!
//! It keeps the next deadline of each armed session in a min-heap and runs
//! one task that sleeps until the earliest. On waking the task reads the
//! clock first: when the deadline has come, it settles the session's due
//! grants as expired through the grant service (`Expire { now_ms }`,
//! observed by the timer) and re-arms the session with its next deadline;
//! an early wake changes nothing. The sleep is clamped to
//! [`TOOL_GRANT_DEADLINE_HEARTBEAT`], so a wall-clock jump is noticed
//! within one heartbeat.
//!
//! The driver touches only grant rows: it never wakes a dormant member and
//! never loads or registers a session. A session is armed by its own
//! model-request boundaries and by a grant or list that finds it served,
//! and disarmed when its registration is removed.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use meerkat_core::tool_grant::ToolGrantSubject;
use meerkat_core::types::SessionId;

use super::audit::ToolGrantExpiryDriver;
use super::service::ToolGrantService;

/// The longest the deadline driver sleeps before it reads the clock again.
pub const TOOL_GRANT_DEADLINE_HEARTBEAT: Duration = Duration::from_secs(30);

/// The armed deadlines.
#[derive(Default)]
struct Armed {
    /// Each armed session's deadline, in UNIX ms.
    by_session: BTreeMap<SessionId, u64>,
    /// Every deadline armed, earliest first. An entry that no longer
    /// matches `by_session` is stale and skipped.
    heap: BinaryHeap<Reverse<(u64, SessionId)>>,
}

impl Armed {
    /// Arm `session_id` with `deadline_ms` unless an earlier deadline is
    /// armed. Returns whether this deadline is now the session's.
    fn arm(&mut self, session_id: &SessionId, deadline_ms: u64) -> bool {
        if self
            .by_session
            .get(session_id)
            .is_some_and(|armed| *armed <= deadline_ms)
        {
            return false;
        }
        self.by_session.insert(session_id.clone(), deadline_ms);
        self.heap.push(Reverse((deadline_ms, session_id.clone())));
        true
    }

    /// The earliest armed deadline and its session.
    fn earliest(&mut self) -> Option<(u64, SessionId)> {
        while let Some(Reverse((deadline_ms, session_id))) = self.heap.peek() {
            if self.by_session.get(session_id) == Some(deadline_ms) {
                return Some((*deadline_ms, session_id.clone()));
            }
            self.heap.pop();
        }
        None
    }

    /// Take `session_id`'s deadline if it is still `deadline_ms`.
    fn take(&mut self, session_id: &SessionId, deadline_ms: u64) -> bool {
        if self.by_session.get(session_id) == Some(&deadline_ms) {
            self.by_session.remove(session_id);
            true
        } else {
            false
        }
    }
}

struct Shared {
    service: Arc<ToolGrantService>,
    armed: StdMutex<Armed>,
    changed: crate::tokio::sync::Notify,
    started: AtomicBool,
    closed: AtomicBool,
}

impl Shared {
    fn armed(&self) -> std::sync::MutexGuard<'_, Armed> {
        self.armed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// The deadline driver of one runtime owner. See the module docs.
///
/// A clone is another handle on the same driver.
#[derive(Clone)]
pub struct ToolGrantDeadlines {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for ToolGrantDeadlines {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolGrantDeadlines")
            .field("armed", &self.shared.armed().by_session.len())
            .finish_non_exhaustive()
    }
}

impl ToolGrantDeadlines {
    /// The deadline driver that settles grants through `service`. Its task
    /// starts on the first arm.
    #[must_use]
    pub fn new(service: Arc<ToolGrantService>) -> Self {
        Self {
            shared: Arc::new(Shared {
                service,
                armed: StdMutex::new(Armed::default()),
                changed: crate::tokio::sync::Notify::new(),
                started: AtomicBool::new(false),
                closed: AtomicBool::new(false),
            }),
        }
    }

    /// Arm `session_id` with `deadline_ms` (UNIX ms), keeping the session's
    /// earliest armed deadline. Call it from a task: the first arm starts
    /// the driver's task.
    pub fn arm(&self, session_id: &SessionId, deadline_ms: u64) {
        if self.shared.closed.load(Ordering::Acquire) {
            return;
        }
        let earlier = self.shared.armed().arm(session_id, deadline_ms);
        if !self.shared.started.swap(true, Ordering::AcqRel) {
            crate::tokio::spawn(run(Arc::clone(&self.shared)));
        }
        if earlier {
            self.shared.changed.notify_one();
        }
    }

    /// Forget `session_id`'s armed deadline: its registration was removed.
    pub fn disarm(&self, session_id: &SessionId) {
        self.shared.armed().by_session.remove(session_id);
    }

    /// End the driver: its task exits, and a later arm does nothing.
    pub fn close(&self) {
        self.shared.closed.store(true, Ordering::Release);
        self.shared.changed.notify_one();
    }

    /// The deadline armed for `session_id`, if any.
    #[must_use]
    pub fn armed_deadline(&self, session_id: &SessionId) -> Option<u64> {
        self.shared.armed().by_session.get(session_id).copied()
    }
}

async fn run(shared: Arc<Shared>) {
    loop {
        if shared.closed.load(Ordering::Acquire) {
            return;
        }
        let earliest = shared.armed().earliest();
        let Some((deadline_ms, session_id)) = earliest else {
            shared.changed.notified().await;
            continue;
        };
        // The clock is read first: only a deadline that has come settles.
        let now_ms = shared.service.now_ms().ok();
        match now_ms {
            Some(now_ms) if now_ms >= deadline_ms => {
                if !shared.armed().take(&session_id, deadline_ms) {
                    continue;
                }
                let subject = ToolGrantSubject::session(session_id.clone()).key();
                match shared
                    .service
                    .settle(&subject, ToolGrantExpiryDriver::Timer)
                    .await
                {
                    Ok(settled) => {
                        if let Some(next_ms) = settled.next_deadline_ms {
                            shared.armed().arm(&session_id, next_ms);
                        }
                    }
                    Err(error) => {
                        tracing::warn!(
                            %session_id,
                            %error,
                            "the tool grant deadline driver could not settle a session's grants"
                        );
                        let retry_ms = u64::try_from(TOOL_GRANT_DEADLINE_HEARTBEAT.as_millis())
                            .unwrap_or(u64::MAX);
                        shared
                            .armed()
                            .arm(&session_id, now_ms.saturating_add(retry_ms));
                    }
                }
            }
            _ => {
                let wait = now_ms.map_or(TOOL_GRANT_DEADLINE_HEARTBEAT, |now_ms| {
                    Duration::from_millis(deadline_ms.saturating_sub(now_ms))
                        .min(TOOL_GRANT_DEADLINE_HEARTBEAT)
                });
                let mut changed = std::pin::pin!(shared.changed.notified());
                let mut sleep = std::pin::pin!(crate::tokio::time::sleep(wait));
                crate::tokio::select! {
                    () = &mut changed => {}
                    () = &mut sleep => {}
                }
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_session_keeps_its_earliest_deadline_and_stale_entries_are_skipped() {
        let mut armed = Armed::default();
        let first = SessionId::new();
        let second = SessionId::new();
        assert!(armed.arm(&first, 50));
        assert!(
            !armed.arm(&first, 70),
            "a later deadline keeps the earlier one"
        );
        assert!(armed.arm(&first, 30), "an earlier deadline replaces it");
        assert!(armed.arm(&second, 40));
        assert_eq!(armed.earliest(), Some((30, first.clone())));
        assert!(armed.take(&first, 30));
        assert_eq!(
            armed.earliest(),
            Some((40, second.clone())),
            "the first session's older entry is stale"
        );
        armed.by_session.remove(&second);
        assert_eq!(armed.earliest(), None);
    }
}

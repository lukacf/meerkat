//! [`ToolGrantDeadlines`]: the deadline driver of one runtime owner's
//! served sessions.
//!
//! It keeps the next deadline of each armed subject of each served session
//! in a min-heap and runs one task that sleeps until the earliest. A
//! session's subjects are its own and, for a mob member's build, the
//! member's identity. On waking the task reads the clock first: when the
//! deadline has come, it settles the subject's due grants as expired
//! through the grant service (`Expire { now_ms }`, observed by the timer)
//! and re-arms the subject with its next deadline; an early wake changes
//! nothing. The sleep is clamped to [`TOOL_GRANT_DEADLINE_HEARTBEAT`], so a
//! wall-clock jump is noticed within one heartbeat.
//!
//! The driver touches only grant rows: it never wakes a dormant member and
//! never loads or registers a session. A subject is armed by its session's
//! own model-request boundaries and by a grant or list that finds the
//! session serving it, and every subject of a session is disarmed when the
//! session's registration is removed.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use meerkat_core::tool_grant::ToolGrantSubjectKey;
use meerkat_core::types::SessionId;

use super::audit::ToolGrantExpiryDriver;
use super::service::ToolGrantService;

/// The longest the deadline driver sleeps before it reads the clock again.
pub const TOOL_GRANT_DEADLINE_HEARTBEAT: Duration = Duration::from_secs(30);

/// One armed deadline's key: the served session it is armed for, and the
/// subject whose grants it settles.
type ArmKey = (SessionId, ToolGrantSubjectKey);

/// The armed deadlines.
#[derive(Default)]
struct Armed {
    /// Each armed subject's deadline, in UNIX ms, by its session.
    by_key: BTreeMap<ArmKey, u64>,
    /// Every deadline armed, earliest first. An entry that no longer
    /// matches `by_key` is stale and skipped.
    heap: BinaryHeap<Reverse<(u64, SessionId, ToolGrantSubjectKey)>>,
}

impl Armed {
    /// Arm `subject` of `session_id` with `deadline_ms` unless an earlier
    /// deadline is armed for it. Returns whether this deadline is now the
    /// subject's.
    fn arm(
        &mut self,
        session_id: &SessionId,
        subject: &ToolGrantSubjectKey,
        deadline_ms: u64,
    ) -> bool {
        let key = (session_id.clone(), subject.clone());
        if self
            .by_key
            .get(&key)
            .is_some_and(|armed| *armed <= deadline_ms)
        {
            return false;
        }
        self.by_key.insert(key, deadline_ms);
        self.heap
            .push(Reverse((deadline_ms, session_id.clone(), subject.clone())));
        true
    }

    /// The earliest armed deadline, its session and its subject.
    fn earliest(&mut self) -> Option<(u64, SessionId, ToolGrantSubjectKey)> {
        while let Some(Reverse((deadline_ms, session_id, subject))) = self.heap.peek() {
            if self.by_key.get(&(session_id.clone(), subject.clone())) == Some(deadline_ms) {
                return Some((*deadline_ms, session_id.clone(), subject.clone()));
            }
            self.heap.pop();
        }
        None
    }

    /// Take the deadline of `subject` of `session_id` if it is still
    /// `deadline_ms`.
    fn take(
        &mut self,
        session_id: &SessionId,
        subject: &ToolGrantSubjectKey,
        deadline_ms: u64,
    ) -> bool {
        let key = (session_id.clone(), subject.clone());
        if self.by_key.get(&key) == Some(&deadline_ms) {
            self.by_key.remove(&key);
            true
        } else {
            false
        }
    }

    /// Forget every subject armed for `session_id`.
    fn disarm(&mut self, session_id: &SessionId) {
        self.by_key.retain(|(session, _), _| session != session_id);
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
            .field("armed", &self.shared.armed().by_key.len())
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

    /// Arm `subject` of the served session `session_id` with `deadline_ms`
    /// (UNIX ms), keeping the subject's earliest armed deadline. Call it
    /// from a task: an arm with no driver task running starts one, the first
    /// arm and any arm after the task ended (its runtime shut down) alike.
    pub fn arm(&self, session_id: &SessionId, subject: &ToolGrantSubjectKey, deadline_ms: u64) {
        if self.shared.closed.load(Ordering::Acquire) {
            return;
        }
        let earlier = self.shared.armed().arm(session_id, subject, deadline_ms);
        if !self.shared.started.swap(true, Ordering::AcqRel) {
            spawn_driver(Arc::clone(&self.shared));
        }
        if earlier {
            self.shared.changed.notify_one();
        }
    }

    /// Forget every armed deadline of `session_id`'s subjects: its
    /// registration was removed.
    pub fn disarm(&self, session_id: &SessionId) {
        self.shared.armed().disarm(session_id);
    }

    /// End the driver: its task exits, and a later arm does nothing.
    pub fn close(&self) {
        self.shared.closed.store(true, Ordering::Release);
        self.shared.changed.notify_one();
    }

    /// The deadline armed for `subject` of `session_id`, if any.
    #[must_use]
    pub fn armed_deadline(
        &self,
        session_id: &SessionId,
        subject: &ToolGrantSubjectKey,
    ) -> Option<u64> {
        self.shared
            .armed()
            .by_key
            .get(&(session_id.clone(), subject.clone()))
            .copied()
    }
}

/// Clears the driver's started latch when its task ends, however it ends: a
/// return, a panic, or the runtime dropping the task (polled or not). The
/// next arm then starts a new task.
struct StartedLatch(Arc<Shared>);

impl Drop for StartedLatch {
    fn drop(&mut self) {
        self.0.started.store(false, Ordering::Release);
    }
}

/// Start the driver's task. The latch moves into the task's future before it
/// is first polled, so dropping the task always clears it.
fn spawn_driver(shared: Arc<Shared>) {
    let latch = StartedLatch(Arc::clone(&shared));
    crate::tokio::spawn(async move {
        let _latch = latch;
        run(shared).await;
    });
}

async fn run(shared: Arc<Shared>) {
    loop {
        if shared.closed.load(Ordering::Acquire) {
            return;
        }
        let earliest = shared.armed().earliest();
        let Some((deadline_ms, session_id, subject)) = earliest else {
            shared.changed.notified().await;
            continue;
        };
        // The clock is read first: only a deadline that has come settles.
        let now_ms = shared.service.now_ms().ok();
        match now_ms {
            Some(now_ms) if now_ms >= deadline_ms => {
                if !shared.armed().take(&session_id, &subject, deadline_ms) {
                    continue;
                }
                match shared
                    .service
                    .settle(&subject, ToolGrantExpiryDriver::Timer)
                    .await
                {
                    Ok(settled) => {
                        if let Some(next_ms) = settled.next_deadline_ms {
                            shared.armed().arm(&session_id, &subject, next_ms);
                        }
                    }
                    Err(error) => {
                        tracing::warn!(
                            %session_id,
                            %subject,
                            %error,
                            "the tool grant deadline driver could not settle a subject's grants"
                        );
                        let retry_ms = u64::try_from(TOOL_GRANT_DEADLINE_HEARTBEAT.as_millis())
                            .unwrap_or(u64::MAX);
                        shared
                            .armed()
                            .arm(&session_id, &subject, now_ms.saturating_add(retry_ms));
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
    use meerkat_core::tool_grant::ToolGrantSubject;

    fn session_key(session_id: &SessionId) -> ToolGrantSubjectKey {
        ToolGrantSubject::session(session_id.clone()).key()
    }

    #[test]
    fn a_subject_keeps_its_earliest_deadline_and_stale_entries_are_skipped() {
        let mut armed = Armed::default();
        let first = SessionId::new();
        let second = SessionId::new();
        let first_key = session_key(&first);
        let second_key = session_key(&second);
        assert!(armed.arm(&first, &first_key, 50));
        assert!(
            !armed.arm(&first, &first_key, 70),
            "a later deadline keeps the earlier one"
        );
        assert!(
            armed.arm(&first, &first_key, 30),
            "an earlier deadline replaces it"
        );
        assert!(armed.arm(&second, &second_key, 40));
        assert_eq!(
            armed.earliest(),
            Some((30, first.clone(), first_key.clone()))
        );
        assert!(armed.take(&first, &first_key, 30));
        assert_eq!(
            armed.earliest(),
            Some((40, second.clone(), second_key.clone())),
            "the first session's older entry is stale"
        );
        armed.by_key.remove(&(second.clone(), second_key));
        assert_eq!(armed.earliest(), None);
    }

    /// A member's session arms its identity and its own subject apart, and
    /// a disarm of the session forgets both.
    #[test]
    fn a_session_arms_each_of_its_subjects_and_a_disarm_forgets_them_all() {
        let mut armed = Armed::default();
        let session = SessionId::new();
        let other = SessionId::new();
        let identity = ToolGrantSubject::Identity {
            mob_id: "mob".into(),
            mob_incarnation: "incarnation".into(),
            identity: "worker".into(),
        }
        .key();
        assert!(armed.arm(&session, &identity, 80));
        assert!(armed.arm(&session, &session_key(&session), 60));
        assert!(armed.arm(&other, &session_key(&other), 90));
        assert_eq!(
            armed.earliest(),
            Some((60, session.clone(), session_key(&session)))
        );
        armed.disarm(&session);
        assert_eq!(
            armed.earliest(),
            Some((90, other.clone(), session_key(&other))),
            "both of the session's subjects are forgotten"
        );
    }

    /// A driver whose task ended (here its runtime shut down before the task
    /// ever ran) starts a new task on the next arm, and a closed driver never
    /// does.
    #[test]
    fn a_driver_whose_task_ended_starts_again_on_the_next_arm() {
        let deadlines = ToolGrantDeadlines::new(Arc::new(ToolGrantService::new(Arc::new(
            crate::store::InMemoryRuntimeStore::new(),
        ))));
        let session = SessionId::new();
        let subject = session_key(&session);
        let runtime = || {
            crate::tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime")
        };
        let started =
            |deadlines: &ToolGrantDeadlines| deadlines.shared.started.load(Ordering::Acquire);

        let first = runtime();
        first.block_on(async { deadlines.arm(&session, &subject, u64::MAX) });
        assert!(started(&deadlines), "the first arm starts the driver");
        drop(first);
        assert!(
            !started(&deadlines),
            "the runtime's shutdown ended the task, which clears the latch"
        );

        let second = runtime();
        second.block_on(async { deadlines.arm(&session, &subject, u64::MAX - 1) });
        assert!(started(&deadlines), "the next arm starts a new driver");
        drop(second);

        deadlines.close();
        let third = runtime();
        third.block_on(async { deadlines.arm(&session, &subject, u64::MAX - 2) });
        assert!(!started(&deadlines), "a closed driver never starts again");
    }
}

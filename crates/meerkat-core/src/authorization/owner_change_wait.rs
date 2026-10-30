//! Bounded waiting for an owner change in progress on a work's own
//! authorization publication.
//!
//! An owner that publishes a change keeps the publication's sequence odd
//! until the change completes, and every observation made meanwhile refuses
//! with `ReprepareRequired`. The built-in rebuilds of preparation and of the
//! final check run back to back, so a single change held across a storage
//! commit outlasts all of them. An operation that refused that way waits here
//! for the change to complete before it rebuilds. Waiting only delays: the
//! rebuild still requires a coherent observation and every current check, and
//! no refusal is weakened.
//!
//! A governed step of a tool dispatch takes the work owners' exclusion
//! without waiting. When another actor holds it (an owner change still being
//! prepared, a held exclusion or another operation's final entry), the
//! native work owner refuses to rebuild and reports that hold as a change in
//! progress until it ends, so the step's waiting form waits here too, only
//! after the step returned and released everything it took. A hold that
//! ended before the step read it is no ordinary refusal there: the step
//! pauses one backoff step and rebuilds, as one of its free rebuilds
//! ([`OwnerChangeChurn::wait_for_refused_step`]). This is shell
//! synchronization: no machine input changes.

use crate::time_compat::{DeadlineInstant, Duration};
#[cfg(target_arch = "wasm32")]
use crate::tokio;

use super::{WaitReport, WorkAuthorization};

/// The longest one operation waits, in total wall-clock time from its first
/// wait and including the rebuilds in between, for owner changes in progress.
/// A storage commit with fsync typically completes well within 100 ms; a
/// change held longer than this is a stuck owner, and the operation then
/// refuses as it would without waiting.
pub const OWNER_CHANGE_WAIT_BOUND: Duration = Duration::from_secs(2);

/// Rebuilds one operation may make after a settled owner change without
/// spending its own rebuild budget.
pub const OWNER_CHANGE_FREE_REBUILDS: u8 = 3;

const FIRST_BACKOFF: Duration = Duration::from_millis(1);
const MAX_BACKOFF: Duration = Duration::from_millis(64);

/// One operation's waiting state across its rebuilds: a single wall-clock
/// deadline, set at its first wait and never re-armed, the free rebuilds it
/// has spent, and where its waits are reported to the host's operation
/// refusal observer, when one is installed.
#[derive(Debug, Default)]
pub struct OwnerChangeChurn {
    deadline: Option<DeadlineInstant>,
    free_rebuilds: u8,
    report: Option<WaitReport>,
}

/// How waiting for one refused operation's owner change ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OwnerChangeWait {
    /// No change is in progress any more: the operation may rebuild once
    /// without spending its own rebuild budget. This does not mean the
    /// change completed: a publication poisoned during the wait reports no
    /// change in progress either, and the rebuild then refuses
    /// `Unavailable`, exactly as without waiting.
    Settled {
        /// Time spent waiting for this change.
        waited: Duration,
    },
    /// No free rebuild: the deadline passed, or the operation already spent
    /// every free rebuild. The refusal takes its ordinary path.
    NotSettled {
        /// Time spent waiting for this change.
        waited: Duration,
    },
}

impl OwnerChangeWait {
    /// Whether the operation may rebuild without spending its own budget.
    #[must_use]
    pub fn settled(self) -> bool {
        matches!(self, Self::Settled { .. })
    }

    /// Time spent waiting.
    #[must_use]
    pub fn waited(self) -> Duration {
        match self {
            Self::Settled { waited } | Self::NotSettled { waited } => waited,
        }
    }

    /// Record this wait of `operation` at `site` as one `debug` event of
    /// target `meerkat::authz::owner_change_wait`, with `waited_ms` and
    /// `settled`, so a host can tell a waited refusal from a final one.
    pub fn trace(self, site: &'static str, operation: Option<&crate::OperationId>) {
        tracing::debug!(
            target: "meerkat::authz::owner_change_wait",
            site,
            operation_id = operation.map(tracing::field::display),
            waited_ms = u64::try_from(self.waited().as_millis()).unwrap_or(u64::MAX),
            settled = self.settled(),
            "operation waited for an owner change"
        );
    }
}

impl OwnerChangeChurn {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A waiting state whose waits are reported through `report`, the host's
    /// operation refusal observer for this operation, when one is installed.
    #[must_use]
    pub(crate) fn reporting(report: Option<WaitReport>) -> Self {
        Self {
            report,
            ..Self::default()
        }
    }

    /// After a `ReprepareRequired` refusal of an operation of `work`: when
    /// `work` reports an owner change in progress on its own publication,
    /// wait for that change, and for any later one, to complete. The first
    /// call sets the operation's deadline to [`OWNER_CHANGE_WAIT_BOUND`] from
    /// now, clamped to `budget_remaining`, and later calls keep it. A free
    /// rebuild is granted only when a read finds no change in progress, and
    /// at most [`OWNER_CHANGE_FREE_REBUILDS`] times per operation. The
    /// sequence only moves forward, so such a read means the sequence has
    /// moved past the change seen here and is not odd again. A publication
    /// poisoned during the wait also ends it as settled; see
    /// [`OwnerChangeWait::Settled`].
    ///
    /// Each wait is traced at `site` for `operation` ([`OwnerChangeWait::trace`])
    /// and reported to the operation refusal observer of the host that
    /// composed the agent, when one is installed.
    ///
    /// `None` when no change is in progress: the refusal takes its ordinary
    /// path, exactly as without this wait.
    pub async fn wait_for_settled_change(
        &mut self,
        work: &dyn WorkAuthorization,
        budget_remaining: Option<Duration>,
        site: &'static str,
        operation: Option<&crate::OperationId>,
    ) -> Option<OwnerChangeWait> {
        work.owner_change_in_progress()?;
        Some(
            self.wait_and_report(work, budget_remaining, site, operation, false)
                .await,
        )
    }

    /// After a governed step of a tool dispatch refused to rebuild: as
    /// [`Self::wait_for_settled_change`], except that a read finding no
    /// change in progress is no ordinary refusal. The step took the work
    /// owners' exclusion without waiting, and the hold that refused it
    /// (often another operation's final entry, shorter than the step's way
    /// to this read) may already have ended, so the step pauses one backoff
    /// step and waits for any change in progress after that pause. Either
    /// way the wait is bounded, traced, reported and counted as every other
    /// one: a settled wait is one of the operation's
    /// [`OWNER_CHANGE_FREE_REBUILDS`], and no wait starts past the deadline.
    pub(crate) async fn wait_for_refused_step(
        &mut self,
        work: &dyn WorkAuthorization,
        budget_remaining: Option<Duration>,
        site: &'static str,
        operation: Option<&crate::OperationId>,
    ) -> OwnerChangeWait {
        let pause_first = work.owner_change_in_progress().is_none();
        self.wait_and_report(work, budget_remaining, site, operation, pause_first)
            .await
    }

    /// One bounded wait of this operation: its deadline, set at its first
    /// wait, and its free rebuilds, with the wait traced and reported.
    async fn wait_and_report(
        &mut self,
        work: &dyn WorkAuthorization,
        budget_remaining: Option<Duration>,
        site: &'static str,
        operation: Option<&crate::OperationId>,
        pause_first: bool,
    ) -> OwnerChangeWait {
        let wait = if self.free_rebuilds >= OWNER_CHANGE_FREE_REBUILDS {
            OwnerChangeWait::NotSettled {
                waited: Duration::ZERO,
            }
        } else {
            let deadline = *self.deadline.get_or_insert_with(|| {
                let bound = budget_remaining.map_or(OWNER_CHANGE_WAIT_BOUND, |remaining| {
                    remaining.min(OWNER_CHANGE_WAIT_BOUND)
                });
                DeadlineInstant::now() + bound
            });
            let wait = await_owner_change(work, deadline, pause_first).await;
            if wait.settled() {
                self.free_rebuilds += 1;
            }
            wait
        };
        wait.trace(site, operation);
        if let Some(report) = self.report.as_ref() {
            report.report(wait, operation);
        }
        wait
    }
}

/// Wait until no change is in progress on `work`'s publication, or until
/// `deadline`. A change that began after the one refused keeps the wait
/// going under the same deadline; only a read with no change in progress
/// settles it. With `pause_first`, the first read follows one backoff step,
/// and nothing settles once the deadline has passed before that pause.
async fn await_owner_change(
    work: &dyn WorkAuthorization,
    deadline: DeadlineInstant,
    pause_first: bool,
) -> OwnerChangeWait {
    let start = DeadlineInstant::now();
    let mut backoff = FIRST_BACKOFF;
    if pause_first {
        if start >= deadline {
            return OwnerChangeWait::NotSettled {
                waited: Duration::ZERO,
            };
        }
        tokio::time::sleep(backoff.min(deadline.saturating_duration_since(start))).await;
        backoff = backoff.saturating_mul(2).min(MAX_BACKOFF);
    }
    loop {
        let now = DeadlineInstant::now();
        if work.owner_change_in_progress().is_none() {
            return OwnerChangeWait::Settled {
                waited: now.saturating_duration_since(start),
            };
        }
        if now >= deadline {
            return OwnerChangeWait::NotSettled {
                waited: now.saturating_duration_since(start),
            };
        }
        tokio::time::sleep(backoff.min(deadline.saturating_duration_since(now))).await;
        backoff = backoff.saturating_mul(2).min(MAX_BACKOFF);
    }
}

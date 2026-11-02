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
//! A pure observation of the publication (a host's own, for example) can end
//! stale after a change it never saw in progress: one that waited for an
//! owner's own lock while another actor's change held it ends stale only once
//! that change published, often after it completed. Such a read waits with
//! [`OwnerChangeChurn::wait_after_stale_read`], which with no change in
//! progress as it starts pauses one backoff step first; a settled wait is one
//! of the free rebuilds either way.
//!
//! A governed step of a tool dispatch takes the work owners' exclusion
//! without waiting. When another actor holds it (an owner change still being
//! prepared, a held exclusion or another operation's final entry), the
//! native work owner refuses to rebuild and reports that hold as a change in
//! progress until it ends, so the step's waiting form waits here too, only
//! after the step returned and released everything it took. A step refused
//! to rebuild ran nothing, so it takes the same wait as a stale read: a hold
//! that ended before the step read it is no ordinary refusal there, and the
//! step pauses one backoff step and rebuilds, as one of its free rebuilds.
//! This is shell synchronization: no machine input changes.

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
    /// rebuild is granted only when a read before the deadline finds no
    /// change in progress, and at most [`OWNER_CHANGE_FREE_REBUILDS`] times
    /// per operation. The sequence only moves forward, so such a read means
    /// the sequence has moved past the change seen here and is not odd
    /// again. A publication poisoned during the wait also ends it as
    /// settled; see [`OwnerChangeWait::Settled`].
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

    /// The bounded wait after a stale read: a pure observation of `work`'s
    /// own authorization publication (one that reads owners and runs no
    /// effect, such as `LocalAuthorizationPublication::observe` around an
    /// observed lineage resolution) found that an owner change published
    /// during it (`PublicationError::Changed`,
    /// `LineageResolutionRefusal::Changed`), and so did its one rebuild.
    ///
    /// It waits, as [`Self::wait_for_settled_change`] does, until a read
    /// finds no owner change in progress on that publication (a reservation
    /// of its writer that has not published counts as one where `work` names
    /// it), however many changes begin meanwhile. Unlike that wait, it also
    /// waits when none is in progress as it starts: the change that made the
    /// observation stale has often completed already (an observation that
    /// waited for an owner's own lock while the change held it ends stale
    /// only once that change published), so it then pauses one backoff step
    /// (1 ms) before it reads.
    ///
    /// - [`OwnerChangeWait::Settled`]: make the same observation once more,
    ///   afresh from the owners. The wait was one of this churn's
    ///   [`OWNER_CHANGE_FREE_REBUILDS`]. It promises nothing about the next
    ///   read: that may be stale again, or `Unavailable` once the publication
    ///   was poisoned.
    /// - [`OwnerChangeWait::NotSettled`]: stop. The deadline passed, or every
    ///   free rebuild is spent. Answer the observation as stale and keep it
    ///   typed (for an operation, `ReprepareRequired`), never as
    ///   unavailability. A governed helper admission (below) answers with
    ///   its fence's own last refusal instead, `Unavailable`, the only
    ///   non-denial its refusal type has.
    ///
    /// Call it only after a stale read, between attempts of the same pure
    /// observation, and hold nothing across it: no publication guard or
    /// exclusion, no owner or host lock, no store transaction. Never call it
    /// around an effect: read, claim or record an effect once, after the
    /// observation answered. Any other answer (current, a denial, a genuine
    /// `Unavailable`) is final and never waits.
    ///
    /// One `Unavailable` is contention, not genuine, and is retryable at the
    /// host level: a governed helper admission fence
    /// (`GovernedHelperAdmissionFence` in the `meerkat` facade) refuses as
    /// `Unavailable`, with nothing run or committed, while another writer
    /// holds the publication its check excludes owner changes on (a
    /// reservation of the writer that has not published) or an owner change
    /// is in progress there. The fence runs inside the store call and never
    /// waits. Wait here instead, after that call returned, outside every lock
    /// and store transaction, then make the same idempotent create again with
    /// a new fence (a fence serves one attempt). The refused attempt ran no
    /// effect and the admission key keeps the create idempotent, so this is
    /// no wait around an effect.
    /// `GovernedHelperAdmissionFence::create_idempotent_after_owner_change`
    /// does exactly that. On the prepared path it waits only when the fence
    /// named such a change or hold as it refused; on the dispatch paths it
    /// waits after every refusal to rebuild, as the waiting step forms do
    /// (`GovernedDispatchOperation::run_if_current_within_after_owner_change`).
    /// Any other `Unavailable` (a refusal to rebuild inside the prepared
    /// path's own exclusion, clock or custody unavailable, a poisoned
    /// publication) it answers at once. An `Unavailable` an owner answers for
    /// itself is genuine: an owner never answers its own transient lock
    /// contention that way.
    ///
    /// The first wait of this churn arms its one deadline,
    /// [`OWNER_CHANGE_WAIT_BOUND`] from then, clamped to `budget_remaining`
    /// (in a tool body, `ToolDispatchContext::remaining_tool_time`). Later
    /// waits keep it: none runs past it, none starts once it passed, and a
    /// read at or after it never settles.
    /// Take a fresh churn for each decision point
    /// (`ToolDispatchContext::owner_change_churn` in a tool body, otherwise
    /// [`Self::new`]): a churn already spent elsewhere would cut this wait
    /// short. Each wait is traced at `site` for `operation`
    /// ([`OwnerChangeWait::trace`]) and reported to the operation refusal
    /// observer of the host that composed the agent, when one is installed.
    ///
    /// Cancellation is safe: the wait holds nothing, so dropping the future
    /// releases and undoes nothing. A cancelled wait spends no free rebuild
    /// and is neither traced nor reported; a deadline it armed stays armed.
    /// Shell synchronization only: it decides nothing, and every attempt is
    /// still decided afresh by the owners.
    pub async fn wait_after_stale_read(
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
/// going under the same deadline. The deadline is checked before every
/// read: only a read before it that finds no change in progress settles,
/// and a wait at or past it ends unsettled without reading. With
/// `pause_first`, the first read follows one backoff step.
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
        if now >= deadline {
            return OwnerChangeWait::NotSettled {
                waited: now.saturating_duration_since(start),
            };
        }
        if work.owner_change_in_progress().is_none() {
            return OwnerChangeWait::Settled {
                waited: now.saturating_duration_since(start),
            };
        }
        tokio::time::sleep(backoff.min(deadline.saturating_duration_since(now))).await;
        backoff = backoff.saturating_mul(2).min(MAX_BACKOFF);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;
    use crate::authorization::{
        OperationAuthorizationError, OwnerChangeInProgress, PreparedAuthorizationBinding,
        PreparedOperationAuthorization,
    };

    const SITE: &str = "stale_read";

    /// A work whose own publication has an owner change in progress while
    /// `held` is set. It decides nothing.
    #[derive(Default)]
    struct Work {
        held: AtomicBool,
    }

    impl Work {
        fn held() -> Self {
            Self {
                held: AtomicBool::new(true),
            }
        }
    }

    impl WorkAuthorization for Work {
        fn prepare(
            &self,
            _binding: &PreparedAuthorizationBinding,
        ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError> {
            Err(OperationAuthorizationError::Unavailable)
        }

        fn owner_change_in_progress(&self) -> Option<OwnerChangeInProgress> {
            if self.held.load(Ordering::SeqCst) {
                Some(OwnerChangeInProgress::new(uuid::Uuid::nil(), 1))
            } else {
                None
            }
        }
    }

    /// A work whose owner change is in progress until `release_at` on the
    /// paused clock, so a row needs no task to complete it.
    struct ReleasedAt {
        release_at: DeadlineInstant,
    }

    impl WorkAuthorization for ReleasedAt {
        fn prepare(
            &self,
            _binding: &PreparedAuthorizationBinding,
        ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError> {
            Err(OperationAuthorizationError::Unavailable)
        }

        fn owner_change_in_progress(&self) -> Option<OwnerChangeInProgress> {
            if DeadlineInstant::now() < self.release_at {
                Some(OwnerChangeInProgress::new(uuid::Uuid::nil(), 1))
            } else {
                None
            }
        }
    }

    /// A work whose owner change is in progress at its first read only.
    #[derive(Default)]
    struct ClearsAfterFirstRead {
        read: AtomicBool,
    }

    impl WorkAuthorization for ClearsAfterFirstRead {
        fn prepare(
            &self,
            _binding: &PreparedAuthorizationBinding,
        ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError> {
            Err(OperationAuthorizationError::Unavailable)
        }

        fn owner_change_in_progress(&self) -> Option<OwnerChangeInProgress> {
            if self.read.swap(true, Ordering::SeqCst) {
                None
            } else {
                Some(OwnerChangeInProgress::new(uuid::Uuid::nil(), 1))
            }
        }
    }

    /// A read that waited for an owner's own lock ends stale only once the
    /// change it met published, so that change may complete before the
    /// caller asks. With nothing in progress the settled-change wait has
    /// nothing to wait for; the stale-read wait pauses one backoff step and
    /// settles as one free rebuild.
    ///
    /// RED on the base: no public wait settled such a read.
    #[tokio::test(start_paused = true)]
    async fn a_stale_read_whose_change_completed_reads_again_after_one_pause() {
        let work = Work::default();
        let mut churn = OwnerChangeChurn::new();
        assert_eq!(
            churn.wait_for_settled_change(&work, None, SITE, None).await,
            None,
            "nothing in progress: the settled-change wait has nothing to wait for"
        );
        let wait = churn.wait_after_stale_read(&work, None, SITE, None).await;
        assert!(wait.settled(), "read again: {wait:?}");
        assert!(
            wait.waited() >= FIRST_BACKOFF && wait.waited() < OWNER_CHANGE_WAIT_BOUND,
            "after one pause: {wait:?}"
        );
    }

    /// No wait runs past the churn's one deadline (the bound, or the
    /// caller's budget when less), none starts once it passed, and no more
    /// than [`OWNER_CHANGE_FREE_REBUILDS`] waits of one churn settle.
    #[tokio::test(start_paused = true)]
    async fn a_stale_read_wait_never_outlasts_its_deadline_or_its_free_rebuilds() {
        let held = Work::held();
        for (budget, deadline) in [
            (None, OWNER_CHANGE_WAIT_BOUND),
            (Some(Duration::from_secs(10)), OWNER_CHANGE_WAIT_BOUND),
            (Some(Duration::from_millis(300)), Duration::from_millis(300)),
        ] {
            let mut churn = OwnerChangeChurn::new();
            let started = DeadlineInstant::now();
            let wait = churn.wait_after_stale_read(&held, budget, SITE, None).await;
            assert!(
                !wait.settled() && wait.waited() >= deadline && started.elapsed() <= deadline,
                "held throughout: it ends at the deadline, not past it: {wait:?}"
            );
            let again = churn.wait_after_stale_read(&held, budget, SITE, None).await;
            assert_eq!(
                (again.settled(), again.waited()),
                (false, Duration::ZERO),
                "no wait starts past the deadline"
            );
        }

        let settled = Work::default();
        let mut churn = OwnerChangeChurn::new();
        for _ in 0..OWNER_CHANGE_FREE_REBUILDS {
            assert!(
                churn
                    .wait_after_stale_read(&settled, None, SITE, None)
                    .await
                    .settled()
            );
        }
        let started = DeadlineInstant::now();
        let spent = churn
            .wait_after_stale_read(&settled, None, SITE, None)
            .await;
        assert_eq!(
            (spent.settled(), spent.waited(), started.elapsed()),
            (false, Duration::ZERO, Duration::ZERO),
            "every free rebuild spent: no wait at all"
        );
    }

    /// Dropping a wait cancels it: it holds nothing, so nothing is released
    /// or undone, and it spends no free rebuild. The deadline its first poll
    /// armed stays armed for the churn's later waits.
    #[tokio::test(start_paused = true)]
    async fn a_cancelled_stale_read_wait_spends_nothing_and_keeps_its_deadline() {
        let work = Work::held();
        let mut churn = OwnerChangeChurn::new();
        let mut wait = Box::pin(churn.wait_after_stale_read(&work, None, SITE, None));
        assert!(
            futures::poll!(wait.as_mut()).is_pending(),
            "waiting for the change"
        );
        drop(wait);
        work.held.store(false, Ordering::SeqCst);
        for _ in 0..OWNER_CHANGE_FREE_REBUILDS {
            assert!(
                churn
                    .wait_after_stale_read(&work, None, SITE, None)
                    .await
                    .settled(),
                "the cancelled wait spent no free rebuild"
            );
        }

        let mut churn = OwnerChangeChurn::new();
        let mut wait = Box::pin(churn.wait_after_stale_read(&work, None, SITE, None));
        assert!(
            futures::poll!(wait.as_mut()).is_pending(),
            "pausing before its read"
        );
        drop(wait);
        tokio::time::advance(OWNER_CHANGE_WAIT_BOUND).await;
        let late = churn.wait_after_stale_read(&work, None, SITE, None).await;
        assert_eq!(
            (late.settled(), late.waited()),
            (false, Duration::ZERO),
            "the cancelled first wait armed the deadline"
        );
    }

    /// With nothing in progress the stale-read wait pauses one backoff step
    /// first, and no read at or after its deadline settles: a budget within
    /// that pause (500 us, or exactly 1 ms) ends the wait unsettled, with no
    /// free rebuild spent, while a budget past it (2 ms) settles after the
    /// pause. The timer rounds a sleep up to whole milliseconds, so a wait
    /// ends at or after its budget.
    ///
    /// RED on the read-first loop: the read after the pause, at or past the
    /// deadline, found nothing in progress and settled.
    #[tokio::test(start_paused = true)]
    async fn a_budget_within_the_pause_ends_the_stale_read_wait_unsettled() {
        let work = Work::default();
        for budget in [Duration::from_micros(500), Duration::from_millis(1)] {
            let mut churn = OwnerChangeChurn::new();
            let wait = churn
                .wait_after_stale_read(&work, Some(budget), SITE, None)
                .await;
            assert!(
                !wait.settled() && wait.waited() >= budget,
                "RED (a) no holder, budget {budget:?}: settled at or past its deadline: {wait:?}"
            );
            assert_eq!(
                churn.free_rebuilds, 0,
                "RED (a) no holder, budget {budget:?}: spent a free rebuild past its deadline"
            );
        }

        let mut churn = OwnerChangeChurn::new();
        let wait = churn
            .wait_after_stale_read(&work, Some(Duration::from_millis(2)), SITE, None)
            .await;
        assert!(
            wait.settled() && wait.waited() >= FIRST_BACKOFF,
            "guard (a) no holder, budget 2 ms: settles after the pause: {wait:?}"
        );
        assert_eq!(churn.free_rebuilds, 1, "guard (a): one free rebuild spent");
    }

    /// A change in progress until a fixed instant. Completing exactly at the
    /// deadline, it ends both waits unsettled there: no read at or after the
    /// deadline settles. Completing well before the deadline, it lets both
    /// waits settle at their first read after it.
    ///
    /// RED on the read-first loop: the read at the deadline found the change
    /// completed and settled, a free rebuild past the bound.
    #[tokio::test(start_paused = true)]
    async fn a_change_completing_at_the_deadline_settles_neither_wait() {
        let work = ReleasedAt {
            release_at: DeadlineInstant::now() + OWNER_CHANGE_WAIT_BOUND,
        };
        let mut churn = OwnerChangeChurn::new();
        let wait = churn.wait_for_settled_change(&work, None, SITE, None).await;
        assert!(
            wait.is_some_and(|ended| !ended.settled() && ended.waited() >= OWNER_CHANGE_WAIT_BOUND),
            "RED (b) wait_for_settled_change: a change completing at the deadline settled: {wait:?}"
        );
        assert_eq!(
            churn.free_rebuilds, 0,
            "RED (b) wait_for_settled_change: spent a free rebuild at the deadline"
        );

        let work = ReleasedAt {
            release_at: DeadlineInstant::now() + OWNER_CHANGE_WAIT_BOUND,
        };
        let mut churn = OwnerChangeChurn::new();
        let wait = churn.wait_after_stale_read(&work, None, SITE, None).await;
        assert!(
            !wait.settled() && wait.waited() >= OWNER_CHANGE_WAIT_BOUND,
            "RED (b) wait_after_stale_read: a change completing at the deadline settled: {wait:?}"
        );
        assert_eq!(
            churn.free_rebuilds, 0,
            "RED (b) wait_after_stale_read: spent a free rebuild at the deadline"
        );

        let released_after = Duration::from_millis(100);
        let work = ReleasedAt {
            release_at: DeadlineInstant::now() + released_after,
        };
        let wait = OwnerChangeChurn::new()
            .wait_for_settled_change(&work, None, SITE, None)
            .await;
        assert!(
            wait.is_some_and(|ended| ended.settled()
                && ended.waited() >= released_after
                && ended.waited() < released_after + MAX_BACKOFF),
            "guard (b) wait_for_settled_change: settles at its first read after the change: {wait:?}"
        );
        let work = ReleasedAt {
            release_at: DeadlineInstant::now() + released_after,
        };
        let wait = OwnerChangeChurn::new()
            .wait_after_stale_read(&work, None, SITE, None)
            .await;
        assert!(
            wait.settled()
                && wait.waited() >= released_after
                && wait.waited() < released_after + MAX_BACKOFF,
            "guard (b) wait_after_stale_read: settles at its first read after the change: {wait:?}"
        );
    }

    /// Once a churn's deadline has passed no wait settles, even when the
    /// change a wait was asked about clears between its first read and the
    /// next one.
    ///
    /// RED on the read-first loop: the loop's first read found the change
    /// cleared and settled with nothing waited, past the deadline.
    #[tokio::test(start_paused = true)]
    async fn past_the_deadline_a_change_clearing_between_reads_settles_neither_wait() {
        let mut churn = OwnerChangeChurn::new();
        let armed = churn
            .wait_after_stale_read(&Work::held(), Some(Duration::from_millis(10)), SITE, None)
            .await;
        assert!(
            !armed.settled(),
            "the earlier wait ends at its deadline: {armed:?}"
        );

        let wait = churn
            .wait_for_settled_change(&ClearsAfterFirstRead::default(), None, SITE, None)
            .await;
        assert_eq!(
            wait,
            Some(OwnerChangeWait::NotSettled {
                waited: Duration::ZERO
            }),
            "RED (c) wait_for_settled_change: settled past the deadline"
        );
        assert_eq!(
            churn.free_rebuilds, 0,
            "RED (c) wait_for_settled_change: spent a free rebuild past the deadline"
        );

        let wait = churn
            .wait_after_stale_read(&ClearsAfterFirstRead::default(), None, SITE, None)
            .await;
        assert_eq!(
            wait,
            OwnerChangeWait::NotSettled {
                waited: Duration::ZERO
            },
            "RED (c) wait_after_stale_read: settled past the deadline"
        );
        assert_eq!(
            churn.free_rebuilds, 0,
            "RED (c) wait_after_stale_read: spent a free rebuild past the deadline"
        );
    }
}

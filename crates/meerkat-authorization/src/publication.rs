//! Coherent local publication for disposable authorization projections.
//!
//! This is synchronization, not a policy store or grant authority. The host
//! composes every relevant owner mutation through one publication instance:
//! policy, identity, grant ancestors, resource labels, destinations and route
//! accounts. The owner still decides and persists each actual change.
//!
//! Acquire the publication guard before owner write guards, and retain it until
//! the accepted facts are locally visible. Never hold it across network I/O.
//! A retained projection cannot be retagged; a changed stamp requires rebuilding
//! from its owners. A missed owner mutation is a composition defect, not a
//! freshness claim that this helper can repair.

use std::fmt;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering, fence};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};

const INVALID: u64 = u64::MAX;

struct PublicationInner {
    instance: uuid::Uuid,
    sequence: AtomicU64,
    writer: Mutex<()>,
    /// Reservations of `writer` alive now, published or not. A waiting hint
    /// only ([`LocalAuthorizationPublication::change_or_reservation_in_progress`]),
    /// never read for a stamp, an observation or a decision. Counted after
    /// the writer is taken and released after the writer is, so it is never
    /// zero while a reservation holds the writer.
    reservations: AtomicUsize,
}

/// Process-local ordering shared by all authorization-relevant fact owners.
///
/// There is no serialization or recovery constructor. Restart reconstructs
/// decisions from retained owner facts under a new publication instance.
#[derive(Clone)]
pub struct LocalAuthorizationPublication {
    inner: Arc<PublicationInner>,
}

impl fmt::Debug for LocalAuthorizationPublication {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalAuthorizationPublication")
            .finish_non_exhaustive()
    }
}

impl Default for LocalAuthorizationPublication {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalAuthorizationPublication {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(PublicationInner {
                instance: uuid::Uuid::new_v4(),
                sequence: AtomicU64::new(0),
                writer: Mutex::new(()),
                reservations: AtomicUsize::new(0),
            }),
        }
    }

    /// Start publishing an owner-authorized local change. This never authorizes
    /// the mutation and does not acquire any owner-specific state or store lock.
    ///
    /// # Errors
    /// A poisoned or exhausted publication cannot issue fresh projections.
    pub fn begin_owner_change(&self) -> Result<LocalPublicationGuard<'_>, PublicationError> {
        let mut guard = self.reserve_owner_change()?;
        if guard.publish() {
            Ok(guard)
        } else {
            Err(PublicationError::Unavailable)
        }
    }

    /// [`Self::begin_owner_change`] without waiting, for a synchronous step
    /// that holds native custody it must not wait under.
    ///
    /// Take this writer first, then try-lock the owner. When the owner is
    /// contended, change no facts and drop the guard: dropping it advances the
    /// sequence, which conservatively invalidates every disposable observation
    /// without claiming that a change happened. Retry by rebuilding from the
    /// actual owners; never revalidate an observation made before the guard.
    ///
    /// # Errors
    /// Writer contention or poison is `Unavailable` before any mutation and
    /// leaves publication and every stamp unchanged. A poisoned or exhausted
    /// publication stays unavailable, as for [`Self::begin_owner_change`].
    pub fn try_begin_owner_change(&self) -> Result<LocalPublicationGuard<'_>, PublicationError> {
        let mut guard = self.try_reserve_owner_change()?;
        if guard.publish() {
            Ok(guard)
        } else {
            Err(PublicationError::Unavailable)
        }
    }

    /// Permanently poison this publication because an owner that shares it
    /// can no longer be trusted, for example a host owner whose own entry
    /// lock a panic poisoned. From then on every retained stamp, prepared
    /// final entry and grant check answers `Unavailable`, retained consent
    /// eligibility is no longer current, and no new observation or owner
    /// change is admitted. A change reserved before the poison may still
    /// apply its facts, but no guard write overwrites the poison, so those
    /// facts stay unusable too. Effects that already passed their final entry
    /// keep their outcomes. Idempotent.
    ///
    /// Takes no lock and never waits, so an owner can call it from a callback
    /// that runs under a held reservation of this publication.
    ///
    /// To recover, quiesce every owner that shares this publication, then
    /// reconstruct each of them under one new replacement publication (for
    /// the grant owner,
    /// [`LocalGrantAuthority::reconstruct`](crate::grants::LocalGrantAuthority::reconstruct)).
    /// Reconstruction keeps every retained row and revocation. A revocation
    /// refused while poisoned (`Unavailable`) did not happen and its row
    /// stays unrevoked: a caller holding a revocation intent must retry it
    /// after reconstruction, never drop it.
    pub fn poison(&self) {
        self.inner.sequence.store(INVALID, Ordering::Release);
    }

    /// A sentinel that [`poisons`](Self::poison) this publication when it is
    /// dropped during an unwind, for an owner that shares this publication
    /// and guards its own state with an entry lock. Dropped normally, it does
    /// nothing.
    ///
    /// Create it immediately after acquiring the entry lock, in the same
    /// scope, and hold it for as long as the lock is held, reads included:
    /// the unwind that poisons the lock then poisons this publication before
    /// any outer `catch_unwind` can swallow the panic. A `catch_unwind`
    /// placed between the lock guard and the sentinel would poison the lock
    /// and leave this publication healthy.
    ///
    /// As defense in depth, an owner callback that finds its entry lock
    /// poisoned, or catches a panic, returns an error and never reads the
    /// owner's state through `PoisonError::into_inner`.
    #[must_use = "the sentinel poisons only while it is held"]
    pub fn poison_on_unwind(&self) -> OwnerPoisonSentinel<'_> {
        OwnerPoisonSentinel { publication: self }
    }

    /// Reserve the writer without invalidating observations yet. This private
    /// path is only for a mutation whose complete facts stay inaccessible under
    /// one owner mutex until `publish` is called. Validate exhaustion before any
    /// mutation; after successful apply, publication cannot fail.
    ///
    /// Lock order remains publication writer, then owner. A rejected canonical
    /// apply may drop the reservation unchanged only when it changed no facts.
    pub(crate) fn reserve_owner_change(
        &self,
    ) -> Result<LocalPublicationGuard<'_>, PublicationError> {
        let writer = self
            .inner
            .writer
            .lock()
            .map_err(|_| PublicationError::Unavailable)?;
        self.reservation_with_writer(writer)
    }

    /// Reserve without waiting while native custody remains held. Contention
    /// or poison returns before mutation and leaves publication unchanged.
    ///
    /// The refusal keeps the two apart for an operation-time step, which
    /// answers contention as a refusal to rebuild and poison as
    /// unavailability. Every host-facing caller answers both as
    /// [`PublicationError::Unavailable`] (the `From` conversion).
    pub(crate) fn try_reserve_owner_change(
        &self,
    ) -> Result<LocalPublicationGuard<'_>, TryReserveRefusal> {
        let writer = match self.inner.writer.try_lock() {
            Ok(writer) => writer,
            // Another reservation holds the writer of a publication that is
            // still healthy: it is released without waiting on this caller.
            Err(TryLockError::WouldBlock)
                if self.inner.sequence.load(Ordering::Acquire) != INVALID =>
            {
                return Err(TryReserveRefusal::Busy);
            }
            Err(_) => return Err(TryReserveRefusal::Unavailable),
        };
        self.reservation_with_writer(writer)
            .map_err(|_| TryReserveRefusal::Unavailable)
    }

    fn reservation_with_writer<'a>(
        &'a self,
        writer: MutexGuard<'a, ()>,
    ) -> Result<LocalPublicationGuard<'a>, PublicationError> {
        let previous = self.inner.sequence.load(Ordering::Acquire);
        if previous >= INVALID - 2 || !previous.is_multiple_of(2) {
            self.inner.sequence.store(INVALID, Ordering::Release);
            return Err(PublicationError::Unavailable);
        }
        // Counted while the writer is held; the guard stops counting only
        // after it released the writer.
        self.inner.reservations.fetch_add(1, Ordering::AcqRel);
        Ok(LocalPublicationGuard {
            inner: &self.inner,
            completed_sequence: previous + 2,
            published: false,
            writer: Some(writer),
        })
    }

    /// Permanently retire this publication before reconstructing its owners.
    /// Taking the actual writer lock waits for any older guard's final store;
    /// no guard can subsequently overwrite retirement with a healthy sequence.
    /// Poison is retained. This never resets or reuses a publication instance.
    pub(crate) fn retire(&self) {
        let _writer = match self.inner.writer.lock() {
            Ok(writer) => writer,
            Err(poisoned) => poisoned.into_inner(),
        };
        self.inner.sequence.store(INVALID, Ordering::Release);
    }

    pub(crate) fn same_instance(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    /// The owner change in progress on this publication, if any: a token for
    /// waiting until it completes, never a permission. `None` while no change
    /// is in progress, and for a poisoned or exhausted publication, whose
    /// observations stay unavailable.
    #[must_use]
    pub fn change_in_progress(&self) -> Option<meerkat_core::authorization::OwnerChangeInProgress> {
        let sequence = self.inner.sequence.load(Ordering::Acquire);
        (sequence != INVALID && !sequence.is_multiple_of(2)).then(|| {
            meerkat_core::authorization::OwnerChangeInProgress::new(self.inner.instance, sequence)
        })
    }

    /// [`Self::change_in_progress`], or else a reservation of the writer
    /// held now that has not published: an owner change still being
    /// prepared (an issuance before its acknowledged append publishes), a
    /// held exclusion (`try_with_owner_changes_excluded`) or another
    /// operation's own final entry. The waiting hint for an operation-time
    /// step refused because the writer was held, so that it waits for the
    /// holder's release. Such a reservation is named by the odd sequence it
    /// would publish at, so consecutive unpublished reservations share one
    /// token. A hint only, never a permission and never an observation:
    /// `None` for a poisoned or exhausted publication.
    pub(crate) fn change_or_reservation_in_progress(
        &self,
    ) -> Option<meerkat_core::authorization::OwnerChangeInProgress> {
        let sequence = self.inner.sequence.load(Ordering::Acquire);
        if sequence == INVALID {
            return None;
        }
        if !sequence.is_multiple_of(2) {
            return Some(meerkat_core::authorization::OwnerChangeInProgress::new(
                self.inner.instance,
                sequence,
            ));
        }
        (self.inner.reservations.load(Ordering::Acquire) > 0).then(|| {
            meerkat_core::authorization::OwnerChangeInProgress::new(
                self.inner.instance,
                sequence + 1,
            )
        })
    }

    /// The instance and live sequence, for an audit read of a refused
    /// attempt. `None` once poisoned or exhausted. Not a permission and not
    /// an observation: nothing may be decided from it.
    pub(crate) fn current_read(&self) -> Option<(uuid::Uuid, u64)> {
        let sequence = self.inner.sequence.load(Ordering::Acquire);
        (sequence != INVALID).then_some((self.inner.instance, sequence))
    }

    /// Observe the current owners and bind that disposable view to one coherent
    /// publication. The closure must read real owner data or their immutable
    /// snapshots, never a separately refreshed policy cache. It must not do I/O.
    ///
    /// # Errors
    /// A concurrent owner mutation invalidates the observation. The caller may
    /// rebuild once; it must not relabel old facts with a newer stamp.
    pub fn observe<T>(
        &self,
        observe_owners: impl FnOnce() -> T,
    ) -> Result<(T, LocalPublicationStamp), PublicationError> {
        let sequence = self.inner.sequence.load(Ordering::Acquire);
        if sequence == INVALID {
            return Err(PublicationError::Unavailable);
        }
        if !sequence.is_multiple_of(2) {
            return Err(PublicationError::Changed);
        }
        let value = observe_owners();
        let stamp = LocalPublicationStamp {
            inner: Arc::clone(&self.inner),
            sequence,
        };
        // Complete the owner reads before validating the enclosing sequence.
        fence(Ordering::Acquire);
        stamp.check_current()?;
        Ok((value, stamp))
    }
}

/// Only holds publication custody; all semantic change stays with its owner.
pub struct LocalPublicationGuard<'a> {
    inner: &'a PublicationInner,
    completed_sequence: u64,
    published: bool,
    /// Held until this guard drops, then released before the reservation
    /// stops being counted.
    writer: Option<MutexGuard<'a, ()>>,
}

impl LocalPublicationGuard<'_> {
    /// Publish this reservation and return whether it is published. This
    /// cannot fail after reservation unless the publication was
    /// [poisoned](LocalAuthorizationPublication::poison) since: the guard then
    /// stays unpublished, this returns `false` and the poison stands, so facts
    /// already applied under this reservation stay unusable until their owners
    /// are reconstructed. A caller that installs owners under this
    /// publication must check the result.
    ///
    /// For the deferred path this is the mutation linearization point:
    /// canonical apply has succeeded, but the actual owner lock must still
    /// hide its facts. Keep that lock until this returns.
    #[must_use = "a poisoned publication stays unpublished; say why the result is not needed"]
    pub(crate) fn publish(&mut self) -> bool {
        if !self.published {
            // While this guard holds the writer, only a poison can move the
            // sequence, and the poison stays.
            self.published = self
                .inner
                .sequence
                .compare_exchange(
                    self.completed_sequence - 2,
                    self.completed_sequence - 1,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok();
        }
        self.published
    }

    /// Whether the publication is still exactly as this reservation found
    /// it: not published and not poisoned since. One `Acquire` load. A poison
    /// raised earlier on this thread, for example by a caught panic in a
    /// callback run under this reservation, is always seen here; one raised
    /// by another thread after this check is not.
    pub(crate) fn reservation_is_intact(&self) -> bool {
        self.inner.sequence.load(Ordering::Acquire) == self.completed_sequence - 2
    }

    /// Whether `stamp` was observed on this same publication and no owner
    /// change has published since. [`LocalPublicationStamp::check_current`]
    /// alone compares only with the stamp's own publication, so a stamp of
    /// another publication must be refused here first. No other owner change
    /// can publish while this reservation is held, so the answer holds until
    /// this guard publishes or drops.
    pub(crate) fn stamp_is_current(&self, stamp: &LocalPublicationStamp) -> bool {
        std::ptr::eq(self.inner, Arc::as_ptr(&stamp.inner)) && stamp.check_current().is_ok()
    }
}

impl Drop for LocalPublicationGuard<'_> {
    fn drop(&mut self) {
        // Even a deferred reservation poisons publication on panic: the owner
        // may have been partially mutated. Ordinary rejected inputs changed no
        // state and leave the sequence intact. Completion fails only on a
        // poison since publication, which stays.
        if std::thread::panicking() {
            self.inner.sequence.store(INVALID, Ordering::Release);
        } else if self.published {
            let _ = self.inner.sequence.compare_exchange(
                self.completed_sequence - 1,
                self.completed_sequence,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }
        // Release the writer, then stop counting this reservation: a waiter
        // that reads no reservation in progress finds this one's writer
        // released (a later reservation is counted once it holds the writer).
        drop(self.writer.take());
        self.inner.reservations.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Poisons its publication when dropped during a panic. See
/// [`LocalAuthorizationPublication::poison_on_unwind`].
pub struct OwnerPoisonSentinel<'a> {
    publication: &'a LocalAuthorizationPublication,
}

impl fmt::Debug for OwnerPoisonSentinel<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OwnerPoisonSentinel")
            .finish_non_exhaustive()
    }
}

impl Drop for OwnerPoisonSentinel<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.publication.poison();
        }
    }
}

/// Immutable, nonserializable stamp for a rebuilt view of local owner facts.
/// It is not an authorization decision and contains no policy or resource data.
#[derive(Clone)]
pub struct LocalPublicationStamp {
    inner: Arc<PublicationInner>,
    sequence: u64,
}

impl fmt::Debug for LocalPublicationStamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalPublicationStamp")
            .finish_non_exhaustive()
    }
}

impl LocalPublicationStamp {
    /// Historical data from this exact coherent read, never the latest counter.
    pub(crate) fn policy_observation(
        &self,
    ) -> meerkat_core::authorization::PolicyPublicationObservation {
        meerkat_core::authorization::PolicyPublicationObservation::LocalPublication {
            instance: self.inner.instance,
            sequence: self.observation_sequence(),
        }
    }

    /// Historical process-local diagnostic data only. Not a permission,
    /// durable policy epoch, or cross-process freshness proof.
    pub(crate) fn observation_sequence(&self) -> u64 {
        self.sequence
    }

    /// The instance this read belongs to; its sequences compare only with
    /// that instance's.
    pub(crate) fn instance(&self) -> uuid::Uuid {
        self.inner.instance
    }

    /// One allocation-free local atomic read at the final operation boundary.
    /// The prepared operation must separately retain its exact binding and
    /// deadline. No awaited preparation may follow the final entry check.
    ///
    /// # Errors
    /// Any publication since observation, including a writer in progress,
    /// requires a new owner observation. Poison is permanently unavailable.
    pub fn check_current(&self) -> Result<(), PublicationError> {
        match self.inner.sequence.load(Ordering::Acquire) {
            INVALID => Err(PublicationError::Unavailable),
            current if current == self.sequence => Ok(()),
            _ => Err(PublicationError::Changed),
        }
    }
}

/// Human-consent approver eligibility as of one coherent publication of the
/// host's eligibility owner facts.
///
/// The approval owner checks it again when a consent decision commits, with
/// the same allocation-free atomic read as an operation's final entry check:
/// any owner change published since the observation (a revocation, a role
/// change) refuses the decision. It is evidence of eligibility, never an
/// authentication: the approver is still the host's authenticated assertion.
pub struct PublishedConsentEligibility {
    stamp: LocalPublicationStamp,
}

impl PublishedConsentEligibility {
    /// Observe the eligibility owner under `publication`. `eligible` must
    /// read the owner's real facts for the exact approver and approval,
    /// without I/O. An ineligible approver, or an owner change during the
    /// observation, yields no evidence.
    ///
    /// The two are one error here. A host route that resolves eligibility
    /// with this and keeps the default
    /// [`meerkat_core::approval::consent::HumanConsentRoute::eligibility_answer`]
    /// therefore answers an owner change in progress (or one published
    /// during the read) as a refusal, which withdraws an approved consent at
    /// its spend: such a route overrides `eligibility_answer` with
    /// [`Self::observe_answer`].
    ///
    /// # Errors
    /// [`meerkat_core::approval::consent::ConsentIneligible`] when there is
    /// no current evidence of eligibility.
    pub fn observe(
        publication: &LocalAuthorizationPublication,
        eligible: impl FnOnce() -> bool,
    ) -> Result<
        Arc<dyn meerkat_core::approval::consent::ConsentEligibility>,
        meerkat_core::approval::consent::ConsentIneligible,
    > {
        match publication.observe(eligible) {
            Ok((true, stamp)) => Ok(Arc::new(Self { stamp })),
            Ok((false, _)) | Err(_) => Err(meerkat_core::approval::consent::ConsentIneligible),
        }
    }

    /// [`Self::observe`] as the eligibility owner's answer, for
    /// [`meerkat_core::approval::consent::HumanConsentRoute::eligibility_answer`]:
    /// `Eligible` with this evidence when `eligible` reads the owner's facts
    /// as eligible, `Refused` when it reads them as not eligible, and
    /// `Unsettled` when there is no coherent read of them now (an owner
    /// change in progress, or one published during the observation, or a
    /// poisoned publication, which stays so until its owners are
    /// reconstructed). Nothing is decided on an unsettled answer: the
    /// approval owner asks again.
    pub fn observe_answer(
        publication: &LocalAuthorizationPublication,
        eligible: impl FnOnce() -> bool,
    ) -> meerkat_core::approval::consent::ConsentEligibilityAnswer {
        use meerkat_core::approval::consent::ConsentEligibilityAnswer;
        match publication.observe(eligible) {
            Ok((true, stamp)) => ConsentEligibilityAnswer::Eligible(Arc::new(Self { stamp })),
            Ok((false, _)) => ConsentEligibilityAnswer::Refused,
            Err(PublicationError::Changed | PublicationError::Unavailable) => {
                ConsentEligibilityAnswer::Unsettled
            }
        }
    }
}

impl fmt::Debug for PublishedConsentEligibility {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PublishedConsentEligibility")
            .finish_non_exhaustive()
    }
}

impl meerkat_core::approval::consent::ConsentEligibility for PublishedConsentEligibility {
    fn check_current(&self) -> Result<(), meerkat_core::approval::consent::ConsentIneligible> {
        self.stamp
            .check_current()
            .map_err(|_| meerkat_core::approval::consent::ConsentIneligible)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PublicationError {
    #[error("local authorization facts changed")]
    Changed,
    #[error("local authorization facts are unavailable")]
    Unavailable,
}

/// Why [`LocalAuthorizationPublication::try_reserve_owner_change`] took no
/// reservation. Nothing changed either way. Crate-private: every public
/// non-waiting entry answers both as [`PublicationError::Unavailable`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TryReserveRefusal {
    /// Another reservation holds the writer of this healthy publication now
    /// (an owner change, published or not, a held exclusion or another
    /// operation's final entry). It is released without waiting on the
    /// caller, and [`LocalAuthorizationPublication::change_or_reservation_in_progress`]
    /// names it until then.
    Busy,
    /// Poisoned, retired or exhausted, as [`PublicationError::Unavailable`].
    Unavailable,
}

impl From<TryReserveRefusal> for PublicationError {
    fn from(_: TryReserveRefusal) -> Self {
        Self::Unavailable
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn publication_invalidates_before_owner_change_and_cannot_retag_old_view() {
        let publication = LocalAuthorizationPublication::new();
        let (value, original) = publication.observe(|| 7).expect("initial view");
        assert_eq!(value, 7);
        assert_eq!(original.check_current(), Ok(()));
        let change = publication.begin_owner_change().expect("owner publication");
        assert_eq!(original.check_current(), Err(PublicationError::Changed));
        assert!(matches!(
            publication.observe(|| 8),
            Err(PublicationError::Changed)
        ));
        drop(change);
        assert_eq!(original.check_current(), Err(PublicationError::Changed));
        let (_, rebuilt) = publication.observe(|| 8).expect("new owner view");
        assert_eq!(rebuilt.check_current(), Ok(()));
    }

    #[test]
    fn historical_observation_retains_its_instance_and_captured_sequence() {
        let publication = LocalAuthorizationPublication::new();
        let ((), original) = publication.observe(|| ()).expect("initial view");
        let before = original.policy_observation();
        drop(publication.begin_owner_change().expect("owner change"));
        let ((), refreshed) = publication.observe(|| ()).expect("fresh view");
        assert_eq!(original.policy_observation(), before);
        assert_ne!(refreshed.policy_observation(), before);
        let other = LocalAuthorizationPublication::new();
        let ((), same_sequence) = other.observe(|| ()).expect("other publication");
        assert_eq!(
            original.observation_sequence(),
            same_sequence.observation_sequence()
        );
        assert_ne!(same_sequence.policy_observation(), before);
    }

    #[test]
    fn owner_change_during_preparation_refuses_mixed_snapshot() {
        let publication = LocalAuthorizationPublication::new();
        let observed = publication.observe(|| {
            let change = publication.begin_owner_change().expect("concurrent change");
            drop(change);
            11
        });
        assert!(matches!(observed, Err(PublicationError::Changed)));
    }

    #[test]
    #[allow(clippy::panic)] // Deliberately inject a partial owner-mutation panic.
    fn panic_during_owner_mutation_never_publishes_partial_facts() {
        let publication = LocalAuthorizationPublication::new();
        let ((), stamp) = publication.observe(|| ()).expect("initial stamp");
        let result = std::panic::catch_unwind(|| {
            let _change = publication.begin_owner_change().expect("owner change");
            panic!("simulated partial owner mutation");
        });
        assert!(result.is_err());
        assert_eq!(stamp.check_current(), Err(PublicationError::Unavailable));
        assert!(matches!(
            publication.observe(|| ()),
            Err(PublicationError::Unavailable)
        ));
        assert!(matches!(
            publication.begin_owner_change(),
            Err(PublicationError::Unavailable)
        ));
    }

    #[test]
    fn a_contended_try_begin_changes_nothing_and_keeps_the_stamp() {
        let publication = LocalAuthorizationPublication::new();
        let ((), stamp) = publication.observe(|| ()).expect("initial stamp");
        let reservation = publication
            .try_reserve_owner_change()
            .expect("another owner holds the writer");
        assert!(matches!(
            publication.try_begin_owner_change(),
            Err(PublicationError::Unavailable)
        ));
        assert_eq!(stamp.check_current(), Ok(()));
        drop(reservation);
        assert_eq!(stamp.check_current(), Ok(()));
        let ((), unchanged) = publication.observe(|| ()).expect("nothing was published");
        assert_eq!(
            unchanged.observation_sequence(),
            stamp.observation_sequence()
        );
    }

    #[test]
    fn a_contended_source_owner_keeps_its_facts_and_invalidates_the_stamp() {
        let publication = LocalAuthorizationPublication::new();
        let owner = Mutex::new("not admitted");
        let (facts, stamp) = publication
            .observe(|| *owner.lock().expect("owner"))
            .expect("initial view");
        let busy = owner.lock().expect("the owner is busy elsewhere");
        let change = publication
            .try_begin_owner_change()
            .expect("the writer is free");
        assert!(owner.try_lock().is_err(), "the source owner is contended");
        drop(change);
        drop(busy);
        assert_eq!(*owner.lock().expect("owner"), facts, "no fact changed");
        assert_eq!(stamp.check_current(), Err(PublicationError::Changed));

        let (rebuilt, retry_stamp) = publication
            .observe(|| *owner.lock().expect("owner"))
            .expect("a retry rebuilds from the actual owner");
        assert_eq!(rebuilt, facts);
        let change = publication
            .try_begin_owner_change()
            .expect("the writer is free");
        *owner.try_lock().expect("the source owner is free") = "admitted";
        drop(change);
        assert_eq!(retry_stamp.check_current(), Err(PublicationError::Changed));
        let (after, current) = publication
            .observe(|| *owner.lock().expect("owner"))
            .expect("the retry published");
        assert_eq!(after, "admitted");
        assert_eq!(current.check_current(), Ok(()));
    }

    #[test]
    fn a_reservation_admits_only_a_current_stamp_of_its_own_publication() {
        let publication = LocalAuthorizationPublication::new();
        let ((), stale) = publication.observe(|| ()).expect("initial stamp");
        drop(publication.begin_owner_change().expect("owner change"));
        let ((), current) = publication.observe(|| ()).expect("current stamp");
        let unrelated = LocalAuthorizationPublication::new();
        let ((), foreign) = unrelated.observe(|| ()).expect("unrelated stamp");
        assert_eq!(foreign.check_current(), Ok(()), "current on its own");

        let reservation = publication
            .try_reserve_owner_change()
            .expect("the writer is free");
        assert!(reservation.stamp_is_current(&current));
        assert!(!reservation.stamp_is_current(&stale));
        assert!(!reservation.stamp_is_current(&foreign));
        drop(reservation);
        assert_eq!(current.check_current(), Ok(()), "nothing was published");
    }

    #[test]
    fn a_successful_try_begin_publishes_coherently() {
        let publication = LocalAuthorizationPublication::new();
        let owner = Mutex::new("before");
        let (_, before) = publication
            .observe(|| *owner.lock().expect("owner"))
            .expect("initial view");
        let change = publication
            .try_begin_owner_change()
            .expect("the writer is free");
        assert_eq!(before.check_current(), Err(PublicationError::Changed));
        assert!(matches!(
            publication.observe(|| *owner.lock().expect("owner")),
            Err(PublicationError::Changed)
        ));
        *owner.try_lock().expect("the source owner is free") = "after";
        drop(change);
        let (after, stamp) = publication
            .observe(|| *owner.lock().expect("owner"))
            .expect("a coherent view of the new facts");
        assert_eq!(after, "after");
        assert_eq!(stamp.check_current(), Ok(()));
        assert_eq!(before.check_current(), Err(PublicationError::Changed));
    }

    #[test]
    #[allow(clippy::panic)] // Deliberately inject a partial owner-mutation panic.
    fn a_panic_under_try_begin_stays_poisoned() {
        let publication = LocalAuthorizationPublication::new();
        let ((), stamp) = publication.observe(|| ()).expect("initial stamp");
        let result = std::panic::catch_unwind(|| {
            let _change = publication.try_begin_owner_change().expect("owner change");
            panic!("simulated partial owner mutation");
        });
        assert!(result.is_err());
        assert_eq!(stamp.check_current(), Err(PublicationError::Unavailable));
        assert!(matches!(
            publication.try_begin_owner_change(),
            Err(PublicationError::Unavailable)
        ));
        assert!(matches!(
            publication.begin_owner_change(),
            Err(PublicationError::Unavailable)
        ));
        assert!(matches!(
            publication.observe(|| ()),
            Err(PublicationError::Unavailable)
        ));
    }

    #[test]
    fn sequence_exhaustion_does_not_reuse_an_old_projection() {
        let publication = LocalAuthorizationPublication::new();
        publication
            .inner
            .sequence
            .store(INVALID - 1, Ordering::Release);
        let ((), stamp) = publication.observe(|| ()).expect("last even stamp");
        assert!(matches!(
            publication.begin_owner_change(),
            Err(PublicationError::Unavailable)
        ));
        assert_eq!(stamp.check_current(), Err(PublicationError::Unavailable));
    }

    #[test]
    fn poison_takes_no_writer_and_refuses_every_stamp_and_owner_change() {
        let publication = LocalAuthorizationPublication::new();
        let ((), stamp) = publication.observe(|| ()).expect("initial stamp");
        let eligibility = PublishedConsentEligibility::observe(&publication, || true)
            .expect("eligible before the poison");
        assert!(eligibility.check_current().is_ok());
        // This thread holds the writer: poison must return, not wait on it.
        let reservation = publication
            .try_reserve_owner_change()
            .expect("a held reservation");
        publication.poison();
        assert_eq!(stamp.check_current(), Err(PublicationError::Unavailable));
        drop(reservation);
        assert_eq!(stamp.check_current(), Err(PublicationError::Unavailable));
        assert!(
            eligibility.check_current().is_err(),
            "retained consent eligibility is no longer current"
        );
        assert!(matches!(
            publication.observe(|| ()),
            Err(PublicationError::Unavailable)
        ));
        assert!(matches!(
            publication.try_begin_owner_change(),
            Err(PublicationError::Unavailable)
        ));
        assert!(matches!(
            publication.begin_owner_change(),
            Err(PublicationError::Unavailable)
        ));
    }

    #[test]
    #[allow(clippy::panic)] // Deliberately poison an owner's own lock.
    fn the_sentinel_poisons_only_during_an_unwind() {
        let publication = LocalAuthorizationPublication::new();
        let ((), stamp) = publication.observe(|| ()).expect("initial stamp");
        drop(publication.poison_on_unwind());
        assert_eq!(
            stamp.check_current(),
            Ok(()),
            "a normal drop poisons nothing"
        );

        let owner = Mutex::new(());
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _entry = owner.lock().expect("owner entry");
            let _sentinel = publication.poison_on_unwind();
            panic!("simulated owner callback panic");
        }));
        assert!(caught.is_err(), "an outer catch swallowed the panic");
        assert!(owner.is_poisoned(), "the owner's own lock is poisoned");
        assert_eq!(
            stamp.check_current(),
            Err(PublicationError::Unavailable),
            "the same unwind poisoned the publication"
        );
    }

    #[test]
    fn a_sentinel_dropped_without_a_panic_poisons_nothing() {
        let publication = LocalAuthorizationPublication::new();
        let owner = Mutex::new("before");
        let (_, stamp) = publication
            .observe(|| *owner.lock().expect("owner"))
            .expect("initial view");
        {
            let entry = owner.lock().expect("owner entry");
            let _sentinel = publication.poison_on_unwind();
            assert_eq!(*entry, "before", "an ordinary read under the sentinel");
        }
        assert_eq!(
            stamp.check_current(),
            Ok(()),
            "a normal drop poisons nothing"
        );
        let ((), unchanged) = publication
            .observe(|| ())
            .expect("observations are still admitted");
        assert_eq!(
            unchanged.observation_sequence(),
            stamp.observation_sequence()
        );

        let change = publication
            .begin_owner_change()
            .expect("later owner changes are still admitted");
        {
            let mut entry = owner.lock().expect("owner entry");
            let _sentinel = publication.poison_on_unwind();
            *entry = "after";
        }
        drop(change);
        assert_eq!(
            stamp.check_current(),
            Err(PublicationError::Changed),
            "the change published; nothing poisoned it"
        );
        let (after, current) = publication
            .observe(|| *owner.lock().expect("owner"))
            .expect("a coherent view of the new facts");
        assert_eq!(after, "after");
        assert_eq!(current.check_current(), Ok(()));
    }

    #[test]
    fn a_poison_during_a_published_owner_change_survives_its_completion() {
        let publication = LocalAuthorizationPublication::new();
        let ((), stamp) = publication.observe(|| ()).expect("initial stamp");
        let change = publication
            .begin_owner_change()
            .expect("an owner change in progress");
        publication.poison();
        drop(change);
        assert_eq!(
            stamp.check_current(),
            Err(PublicationError::Unavailable),
            "the completed change must not overwrite the poison"
        );
        assert!(
            matches!(
                publication.observe(|| ()),
                Err(PublicationError::Unavailable)
            ),
            "no observation is admitted after the poison"
        );
    }

    #[test]
    fn a_poison_under_an_unpublished_reservation_survives_its_later_publication() {
        let publication = LocalAuthorizationPublication::new();
        let ((), stamp) = publication.observe(|| ()).expect("initial stamp");
        let mut reservation = publication
            .try_reserve_owner_change()
            .expect("an unpublished reservation");
        publication.poison();
        assert!(
            !reservation.publish(),
            "a poisoned reservation does not publish"
        );
        assert_eq!(
            stamp.check_current(),
            Err(PublicationError::Unavailable),
            "the publication point must not overwrite the poison"
        );
        drop(reservation);
        assert_eq!(stamp.check_current(), Err(PublicationError::Unavailable));
        assert!(matches!(
            publication.observe(|| ()),
            Err(PublicationError::Unavailable)
        ));
    }

    #[test]
    fn a_poison_from_another_thread_between_publish_and_drop_stands() {
        let barrier = std::sync::Barrier::new(2);

        let publication = LocalAuthorizationPublication::new();
        let ((), stamp) = publication.observe(|| ()).expect("initial stamp");
        std::thread::scope(|scope| {
            let owner = scope.spawn(|| {
                let change = publication
                    .begin_owner_change()
                    .expect("a published owner change");
                barrier.wait();
                // The other thread poisons here.
                barrier.wait();
                drop(change);
            });
            barrier.wait();
            publication.poison();
            barrier.wait();
            owner.join().expect("owner thread");
        });
        assert_eq!(stamp.check_current(), Err(PublicationError::Unavailable));
        assert!(matches!(
            publication.observe(|| ()),
            Err(PublicationError::Unavailable)
        ));

        let publication = LocalAuthorizationPublication::new();
        let ((), stamp) = publication.observe(|| ()).expect("initial stamp");
        std::thread::scope(|scope| {
            let owner = scope.spawn(|| {
                let mut reservation = publication
                    .try_reserve_owner_change()
                    .expect("an unpublished reservation");
                barrier.wait();
                // The other thread poisons here.
                barrier.wait();
                let published = reservation.publish();
                drop(reservation);
                published
            });
            barrier.wait();
            publication.poison();
            barrier.wait();
            assert!(
                !owner.join().expect("owner thread"),
                "a reservation poisoned from another thread does not publish"
            );
        });
        assert_eq!(stamp.check_current(), Err(PublicationError::Unavailable));
        assert!(matches!(
            publication.observe(|| ()),
            Err(PublicationError::Unavailable)
        ));
    }

    #[test]
    fn change_in_progress_names_only_a_published_change_of_this_instance() {
        let publication = LocalAuthorizationPublication::new();
        assert_eq!(publication.change_in_progress(), None);
        // A reservation is no change in progress until it publishes.
        drop(publication.reserve_owner_change().expect("reservation"));
        assert_eq!(publication.change_in_progress(), None);
        let change = publication.begin_owner_change().expect("owner change");
        let first = publication
            .change_in_progress()
            .expect("a published change is in progress");
        assert_eq!(publication.change_in_progress(), Some(first));
        let other = LocalAuthorizationPublication::new();
        let other_change = other.begin_owner_change().expect("other owner change");
        assert_ne!(
            other.change_in_progress(),
            Some(first),
            "another publication names its own change"
        );
        drop(other_change);
        drop(change);
        assert_eq!(publication.change_in_progress(), None);
        let later = publication.begin_owner_change().expect("later change");
        assert_ne!(
            publication.change_in_progress(),
            Some(first),
            "a later change is another token"
        );
        publication.poison();
        assert_eq!(
            publication.change_in_progress(),
            None,
            "a poisoned publication has no change to wait for"
        );
        drop(later);
        assert_eq!(publication.change_in_progress(), None);
    }

    #[test]
    fn a_held_writer_is_busy_inside_the_crate_and_poison_stays_unavailable() {
        let publication = LocalAuthorizationPublication::new();
        let ((), stamp) = publication.observe(|| ()).expect("initial stamp");
        let held = publication
            .try_reserve_owner_change()
            .expect("the writer is free");
        assert!(
            matches!(
                publication.try_reserve_owner_change(),
                Err(TryReserveRefusal::Busy)
            ),
            "another reservation holds the writer: contention, not poison"
        );
        assert!(
            matches!(
                publication.try_begin_owner_change(),
                Err(PublicationError::Unavailable)
            ),
            "the public non-waiting entry keeps one answer for both"
        );
        assert_eq!(stamp.check_current(), Ok(()), "nothing changed");
        publication.poison();
        assert!(
            matches!(
                publication.try_reserve_owner_change(),
                Err(TryReserveRefusal::Unavailable)
            ),
            "a poisoned publication is unavailable even while its writer is held"
        );
        drop(held);
        assert!(
            matches!(
                publication.try_reserve_owner_change(),
                Err(TryReserveRefusal::Unavailable)
            ),
            "and after its release"
        );
    }

    #[test]
    fn a_reservation_is_named_in_progress_until_released_and_only_a_published_one_otherwise() {
        let publication = LocalAuthorizationPublication::new();
        assert_eq!(publication.change_or_reservation_in_progress(), None);
        let reservation = publication
            .try_reserve_owner_change()
            .expect("the writer is free");
        let named = publication
            .change_or_reservation_in_progress()
            .expect("a reservation that has not published is named");
        assert_eq!(
            publication.change_in_progress(),
            None,
            "change_in_progress still names published changes only"
        );
        drop(reservation);
        assert_eq!(
            publication.change_or_reservation_in_progress(),
            None,
            "released: nothing in progress"
        );

        let mut change = publication
            .reserve_owner_change()
            .expect("the writer is free");
        assert_eq!(
            publication.change_or_reservation_in_progress(),
            Some(named),
            "named by the odd sequence it would publish at"
        );
        assert!(change.publish(), "a healthy reservation publishes");
        assert_eq!(
            publication.change_in_progress(),
            Some(named),
            "published: the same token"
        );
        assert_eq!(publication.change_or_reservation_in_progress(), Some(named));
        drop(change);
        assert_eq!(publication.change_or_reservation_in_progress(), None);

        let held = publication
            .try_reserve_owner_change()
            .expect("the writer is free");
        publication.poison();
        assert_eq!(
            publication.change_or_reservation_in_progress(),
            None,
            "a poisoned publication has nothing to wait for, even while held"
        );
        drop(held);
        assert_eq!(publication.change_or_reservation_in_progress(), None);
    }

    #[test]
    fn an_eligibility_answer_tells_a_refusal_from_an_owner_that_cannot_say() {
        use meerkat_core::approval::consent::ConsentEligibilityAnswer;
        let publication = LocalAuthorizationPublication::new();
        let evidence = match PublishedConsentEligibility::observe_answer(&publication, || true) {
            ConsentEligibilityAnswer::Eligible(evidence) => Some(evidence),
            _ => None,
        }
        .expect("eligible facts answer with evidence");
        assert!(evidence.check_current().is_ok());
        assert!(matches!(
            PublishedConsentEligibility::observe_answer(&publication, || false),
            ConsentEligibilityAnswer::Refused
        ));

        // An owner change in progress: no coherent read, so no answer
        // either way.
        let change = publication.begin_owner_change().expect("owner change");
        assert!(matches!(
            PublishedConsentEligibility::observe_answer(&publication, || true),
            ConsentEligibilityAnswer::Unsettled
        ));
        assert!(matches!(
            PublishedConsentEligibility::observe_answer(&publication, || false),
            ConsentEligibilityAnswer::Unsettled
        ));
        drop(change);
        assert!(
            evidence.check_current().is_err(),
            "the completed change made the earlier evidence stale"
        );

        // A change published during the observation.
        let answer = PublishedConsentEligibility::observe_answer(&publication, || {
            drop(publication.begin_owner_change().expect("owner change"));
            true
        });
        assert!(matches!(answer, ConsentEligibilityAnswer::Unsettled));

        publication.poison();
        assert!(matches!(
            PublishedConsentEligibility::observe_answer(&publication, || true),
            ConsentEligibilityAnswer::Unsettled
        ));
    }
}

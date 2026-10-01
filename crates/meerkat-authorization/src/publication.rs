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
use std::sync::atomic::{AtomicU64, Ordering, fence};
use std::sync::{Arc, Mutex, MutexGuard};

const INVALID: u64 = u64::MAX;

struct PublicationInner {
    sequence: AtomicU64,
    writer: Mutex<()>,
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
                sequence: AtomicU64::new(0),
                writer: Mutex::new(()),
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
        guard.publish();
        Ok(guard)
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
        let previous = self.inner.sequence.load(Ordering::Acquire);
        if previous >= INVALID - 2 || !previous.is_multiple_of(2) {
            self.inner.sequence.store(INVALID, Ordering::Release);
            return Err(PublicationError::Unavailable);
        }
        Ok(LocalPublicationGuard {
            inner: &self.inner,
            completed_sequence: previous + 2,
            published: false,
            _writer: writer,
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
    _writer: MutexGuard<'a, ()>,
}

impl LocalPublicationGuard<'_> {
    /// Infallible after reservation. For the deferred path this is the mutation
    /// linearization point: canonical apply has succeeded, but the actual owner
    /// lock must still hide its facts. Keep that lock until this returns.
    pub(crate) fn publish(&mut self) {
        if !self.published {
            self.inner
                .sequence
                .swap(self.completed_sequence - 1, Ordering::AcqRel);
            self.published = true;
        }
    }
}

impl Drop for LocalPublicationGuard<'_> {
    fn drop(&mut self) {
        // Even a deferred reservation poisons publication on panic: the owner
        // may have been partially mutated. Ordinary rejected inputs changed no
        // state and leave the sequence intact.
        if std::thread::panicking() {
            self.inner.sequence.store(INVALID, Ordering::Release);
        } else if self.published {
            self.inner
                .sequence
                .store(self.completed_sequence, Ordering::Release);
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
    /// Historical process-local diagnostic data only. Not a permission,
    /// durable policy epoch, or cross-process freshness proof.
    pub(crate) fn observation_sequence(&self) -> u64 {
        self.sequence
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PublicationError {
    #[error("local authorization facts changed")]
    Changed,
    #[error("local authorization facts are unavailable")]
    Unavailable,
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
}

//! Mechanical execution custody for one actual RuntimeStore backend.
//!
//! This guard grants no input, model, resource or credential permission. It
//! excludes a second execution owner while a governed machine owns this store.
//! Backend clones and decorators must expose the same owner, and every actual
//! machine or independently constructed persistent driver retains its claim.
use std::sync::{Arc, Mutex, TryLockError};

#[cfg(feature = "sqlite-store")]
mod physical;

/// A bounded custody attempt could not establish the requested store scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RuntimeStoreExecutionCustodyError {
    /// Another execution owner is active, or the short owner lock is busy.
    #[error("runtime store execution custody is busy")]
    Busy,
    /// This backend does not implement execution-lifetime custody.
    #[error("runtime store execution custody is unsupported")]
    Unsupported,
    /// The actual custody owner is poisoned or cannot represent a new claim.
    #[error("runtime store execution custody is unavailable")]
    Unavailable,
}

#[derive(Debug, Default)]
struct ExecutionOwners {
    shared: usize,
    governed: bool,
}

/// Backend-owned execution-lifetime exclusion, separate from row transactions.
///
/// A backend constructs this once for its actual shared state. A clone keeps
/// that same owner; constructing a new value around an existing backend would
/// violate the carrier's contract. SQLite binds the physical database instead
/// of the in-memory counters. Its lifetime lock is separate from row I/O, and
/// its admission gate is held only while acquiring or upgrading a claim.
#[derive(Debug, Clone)]
pub struct RuntimeStoreExecutionCustody {
    inner: ExecutionCustodyOwner,
}

#[derive(Debug, Clone)]
enum ExecutionCustodyOwner {
    Memory(Arc<Mutex<ExecutionOwners>>),
    #[cfg(feature = "sqlite-store")]
    Physical(Arc<physical::PhysicalExecutionCustody>),
}

impl Default for RuntimeStoreExecutionCustody {
    fn default() -> Self {
        Self {
            inner: ExecutionCustodyOwner::Memory(Arc::default()),
        }
    }
}

impl RuntimeStoreExecutionCustody {
    /// Construct the mechanical owner for a newly created backend.
    pub fn new() -> Self {
        Self::default()
    }

    /// Bind the actual SQLite owner's canonical, validated database path.
    /// This is private to store implementations; it grants no row authority.
    #[cfg(feature = "sqlite-store")]
    pub(super) fn for_sqlite_database(path: &std::path::Path) -> Self {
        Self {
            inner: ExecutionCustodyOwner::Physical(Arc::new(
                physical::PhysicalExecutionCustody::new(path),
            )),
        }
    }

    /// Attempt one ordinary execution claim without waiting or polling.
    pub fn try_acquire_shared(
        &self,
    ) -> Result<RuntimeStoreExecutionClaim, RuntimeStoreExecutionCustodyError> {
        let inner = match &self.inner {
            ExecutionCustodyOwner::Memory(inner) => inner,
            #[cfg(feature = "sqlite-store")]
            ExecutionCustodyOwner::Physical(owner) => {
                return owner
                    .try_acquire(false)
                    .map(|claim| RuntimeStoreExecutionClaim {
                        owner: ExecutionClaimOwner::Physical(claim),
                    });
            }
        };
        let mut current = inner.try_lock().map_err(custody_lock_error)?;
        if current.governed {
            return Err(RuntimeStoreExecutionCustodyError::Busy);
        }
        current.shared = current
            .shared
            .checked_add(1)
            .ok_or(RuntimeStoreExecutionCustodyError::Unavailable)?;
        Ok(RuntimeStoreExecutionClaim {
            owner: ExecutionClaimOwner::Memory {
                owner: Arc::clone(inner),
                governed: false,
            },
        })
    }

    /// Attempt exclusive execution custody before constructing a governed owner.
    /// A failed attempt leaves every existing claim unchanged.
    pub fn try_acquire_governed(
        &self,
    ) -> Result<RuntimeStoreExecutionClaim, RuntimeStoreExecutionCustodyError> {
        let inner = match &self.inner {
            ExecutionCustodyOwner::Memory(inner) => inner,
            #[cfg(feature = "sqlite-store")]
            ExecutionCustodyOwner::Physical(owner) => {
                return owner
                    .try_acquire(true)
                    .map(|claim| RuntimeStoreExecutionClaim {
                        owner: ExecutionClaimOwner::Physical(claim),
                    });
            }
        };
        let mut current = inner.try_lock().map_err(custody_lock_error)?;
        if current.governed || current.shared != 0 {
            return Err(RuntimeStoreExecutionCustodyError::Busy);
        }
        current.governed = true;
        Ok(RuntimeStoreExecutionClaim {
            owner: ExecutionClaimOwner::Memory {
                owner: Arc::clone(inner),
                governed: true,
            },
        })
    }
}

fn custody_lock_error<T>(error: TryLockError<T>) -> RuntimeStoreExecutionCustodyError {
    match error {
        TryLockError::WouldBlock => RuntimeStoreExecutionCustodyError::Busy,
        TryLockError::Poisoned(_) => RuntimeStoreExecutionCustodyError::Unavailable,
    }
}

/// One real execution owner's retained claim. Share its lifetime through Arc,
/// rather than acquiring a second claim for a driver of the same machine.
#[derive(Debug)]
pub struct RuntimeStoreExecutionClaim {
    owner: ExecutionClaimOwner,
}

#[derive(Debug)]
enum ExecutionClaimOwner {
    Memory {
        owner: Arc<Mutex<ExecutionOwners>>,
        governed: bool,
    },
    #[cfg(feature = "sqlite-store")]
    Physical(physical::PhysicalExecutionClaim),
}

impl RuntimeStoreExecutionClaim {
    /// Atomically upgrade the only shared owner, before sharing the machine.
    /// A busy attempt preserves ordinary custody. If physical restoration is
    /// uncertain, the failed claim retains admission exclusion until drop and
    /// every retry refuses with `Unavailable`.
    pub fn try_upgrade_to_governed(&mut self) -> Result<(), RuntimeStoreExecutionCustodyError> {
        let (inner, governed) = match &mut self.owner {
            ExecutionClaimOwner::Memory { owner, governed } => (owner, governed),
            #[cfg(feature = "sqlite-store")]
            ExecutionClaimOwner::Physical(claim) => return claim.try_upgrade(),
        };
        let mut current = inner.try_lock().map_err(custody_lock_error)?;
        if *governed {
            return Ok(());
        }
        if current.governed || current.shared != 1 {
            return Err(RuntimeStoreExecutionCustodyError::Busy);
        }
        current.shared = 0;
        current.governed = true;
        *governed = true;
        Ok(())
    }

    /// Whether this retained claim exclusively owns execution of this backend.
    pub fn is_governed(&self) -> bool {
        match &self.owner {
            ExecutionClaimOwner::Memory { governed, .. } => *governed,
            #[cfg(feature = "sqlite-store")]
            ExecutionClaimOwner::Physical(claim) => claim.is_governed(),
        }
    }
}

impl Drop for RuntimeStoreExecutionClaim {
    fn drop(&mut self) {
        // This lock contains only mechanical counters. Poison remains set, so
        // releasing an old claim cannot make a poisoned backend usable again.
        let (inner, governed) = match &self.owner {
            ExecutionClaimOwner::Memory { owner, governed } => (owner, governed),
            #[cfg(feature = "sqlite-store")]
            ExecutionClaimOwner::Physical(_) => return,
        };
        let mut current = inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *governed {
            current.governed = false;
        } else {
            current.shared = current.shared.saturating_sub(1);
        }
    }
}

#[cfg(test)]
mod tests;

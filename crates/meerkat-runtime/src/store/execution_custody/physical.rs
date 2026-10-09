//! Physical execution lifetime and admission locks for the actual SQLite file.
//!
//! The store owner supplies one canonical database path in its stable trusted
//! storage namespace. Lock files are never removed or replaced by this owner.
//! Each claim opens its own descriptors; no process-local registry admits a
//! second execution owner or substitutes for the operating system locks.
//! Descriptor-scoped std locking is used on macOS, Linux and Windows. Other
//! platforms refuse; process-associated locks cannot represent these claims.

use std::fs::{File, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::RuntimeStoreExecutionCustodyError as CustodyError;

#[derive(Debug)]
pub(super) struct PhysicalExecutionCustody {
    lifetime_path: PathBuf,
    admission_path: PathBuf,
    #[cfg(test)]
    upgrade_fault: std::sync::atomic::AtomicU8,
}

impl PhysicalExecutionCustody {
    pub(super) fn new(database: &Path) -> Self {
        Self {
            lifetime_path: lock_path(database, ".execution-custody"),
            admission_path: lock_path(database, ".execution-custody-admission"),
            #[cfg(test)]
            upgrade_fault: std::sync::atomic::AtomicU8::new(0),
        }
    }

    fn try_admission(&self) -> Result<File, CustodyError> {
        let gate = open_lock_file(&self.admission_path)?;
        gate.try_lock().map_err(lock_error)?;
        Ok(gate)
    }

    pub(super) fn try_acquire(
        self: &Arc<Self>,
        governed: bool,
    ) -> Result<PhysicalExecutionClaim, CustodyError> {
        let _gate = self.try_admission()?;
        let lifetime = open_lock_file(&self.lifetime_path)?;
        if governed {
            lifetime.try_lock()
        } else {
            lifetime.try_lock_shared()
        }
        .map_err(lock_error)?;
        Ok(PhysicalExecutionClaim {
            lifetime,
            owner: Arc::clone(self),
            state: if governed {
                PhysicalClaimState::Governed
            } else {
                PhysicalClaimState::Shared
            },
        })
    }

    fn unlock_for_upgrade(&self, lifetime: &File) -> io::Result<()> {
        #[cfg(test)]
        if self.take_upgrade_fault(UpgradeFault::Unlock) {
            return Err(io::Error::other("injected physical custody unlock failure"));
        }
        #[cfg(test)]
        if self.take_upgrade_fault(UpgradeFault::UnlockAfterRelease) {
            lifetime.unlock()?;
            return Err(io::Error::other(
                "injected failure after physical custody unlock",
            ));
        }
        lifetime.unlock()
    }

    fn restore_shared(&self, lifetime: &File) -> Result<(), TryLockError> {
        #[cfg(test)]
        if self.take_upgrade_fault(UpgradeFault::Restore) {
            return Err(TryLockError::Error(io::Error::other(
                "injected physical custody restoration failure",
            )));
        }
        lifetime.try_lock_shared()
    }

    #[cfg(test)]
    fn take_upgrade_fault(&self, fault: UpgradeFault) -> bool {
        self.upgrade_fault
            .compare_exchange(
                fault as u8,
                0,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
            )
            .is_ok()
    }
}

/// The lifetime file drops before a retained failed-state admission gate.
/// Sharing this claim through the existing Arc keeps both exclusions alive.
#[derive(Debug)]
pub(super) struct PhysicalExecutionClaim {
    lifetime: File,
    owner: Arc<PhysicalExecutionCustody>,
    state: PhysicalClaimState,
}

#[derive(Debug)]
enum PhysicalClaimState {
    Shared,
    Governed,
    /// An uncertain lifetime lock never releases admission to another owner.
    Failed {
        _admission: File,
    },
}

impl PhysicalExecutionClaim {
    pub(super) fn try_upgrade(&mut self) -> Result<(), CustodyError> {
        match &self.state {
            PhysicalClaimState::Governed => return Ok(()),
            PhysicalClaimState::Failed { .. } => return Err(CustodyError::Unavailable),
            PhysicalClaimState::Shared => {}
        }
        let gate = self.owner.try_admission()?;
        // Relocking an already locked descriptor (including lock conversion)
        // is unspecified by std. Admit nobody across the explicit unlock gap.
        if self.owner.unlock_for_upgrade(&self.lifetime).is_err() {
            self.state = PhysicalClaimState::Failed { _admission: gate };
            return Err(CustodyError::Unavailable);
        }
        match self.lifetime.try_lock() {
            Ok(()) => {
                self.state = PhysicalClaimState::Governed;
                Ok(())
            }
            Err(TryLockError::WouldBlock) => {
                if self.owner.restore_shared(&self.lifetime).is_ok() {
                    // The admission gate drops only after shared custody has
                    // been restored. Busy cannot publish a lost shared claim.
                    Err(CustodyError::Busy)
                } else {
                    self.state = PhysicalClaimState::Failed { _admission: gate };
                    Err(CustodyError::Unavailable)
                }
            }
            Err(TryLockError::Error(_)) => {
                // The remaining lock state is uncertain. Do not relock it or
                // reopen admissions until this actual failed claim is dropped.
                self.state = PhysicalClaimState::Failed { _admission: gate };
                Err(CustodyError::Unavailable)
            }
        }
    }

    pub(super) fn is_governed(&self) -> bool {
        matches!(self.state, PhysicalClaimState::Governed)
    }

    pub(super) fn belongs_to(&self, owner: &Arc<PhysicalExecutionCustody>) -> bool {
        Arc::ptr_eq(&self.owner, owner)
    }
}

fn lock_path(database: &Path, suffix: &str) -> PathBuf {
    let mut path = database.as_os_str().to_os_string();
    path.push(suffix);
    PathBuf::from(path)
}

#[cfg(any(target_os = "macos", target_os = "linux", windows))]
fn open_lock_file(path: &Path) -> Result<File, CustodyError> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(io_error)?;
    let metadata = file.metadata().map_err(io_error)?;
    if !metadata.is_file() {
        return Err(CustodyError::Unsupported);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if metadata.nlink() != 1 {
            return Err(CustodyError::Unsupported);
        }
    }
    #[cfg(windows)]
    if winapi_util::file::information(&file)
        .map_err(io_error)?
        .number_of_links()
        != 1
    {
        return Err(CustodyError::Unsupported);
    }
    Ok(file)
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn open_lock_file(_path: &Path) -> Result<File, CustodyError> {
    Err(CustodyError::Unsupported)
}

fn io_error(error: io::Error) -> CustodyError {
    if error.kind() == io::ErrorKind::Unsupported {
        CustodyError::Unsupported
    } else {
        CustodyError::Unavailable
    }
}

fn lock_error(error: TryLockError) -> CustodyError {
    match error {
        TryLockError::WouldBlock => CustodyError::Busy,
        TryLockError::Error(error) => io_error(error),
    }
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum UpgradeFault {
    Unlock = 1,
    Restore = 2,
    UnlockAfterRelease = 3,
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux", windows)))]
mod tests;

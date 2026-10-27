//! The exclusive claim one Meta runtime holds on its state directory.
//!
//! Every composition that opens a state directory's persistent stores takes
//! this claim before it opens any of them: `rpc_gateway` and `meta_gateway`
//! at startup, and the builder's `persistent_state` together with the
//! [`MobBootstrapSpec::persistent`](crate::mob_handle_runtime::MobBootstrapSpec::persistent)
//! constructors under it. The claim is an exclusive OS lock (`flock`) on
//! [`STATE_DIR_LOCK_FILE_NAME`] in that directory. It is held for as long as
//! the claim lives and released by the kernel when the process exits: there
//! is no lease, TTL or heartbeat to renew or to expire. A second runtime on
//! the same directory is refused typed, in another process or in this one
//! (the lock belongs to the open file description, so a second open in the
//! same process does not share it).
//!
//! A runtime gives its claim up when a shutdown that stood its mob down
//! cleanly has run to its end, or when it drops; a successor in the same
//! process can then claim the directory.
//!
//! Meerkat's per-session hosting claims refuse only the sessions a second
//! runtime tries to host, after it has opened every store; this claim refuses
//! the second runtime before it opens anything.

use std::path::{Path, PathBuf};

use fs2::FileExt;

use crate::storage_layout::{STATE_DIR_LOCK_FILE_NAME, StorageLayoutError};

/// Exclusive ownership of one state directory by one Meta runtime. Dropping
/// the last holder releases the OS lock.
#[derive(Debug)]
pub struct StateDirClaim {
    state_dir: PathBuf,
    _lock: std::fs::File,
}

impl StateDirClaim {
    /// Claim `state_dir`, creating the directory and its lock file when they
    /// do not exist yet. Never waits: a directory another runtime holds is
    /// refused at once with [`StorageLayoutError::StateDirInUse`].
    pub fn acquire(state_dir: &Path) -> Result<Self, StorageLayoutError> {
        let unavailable = |error: std::io::Error| StorageLayoutError::StateDirUnavailable {
            state_dir: state_dir.to_path_buf(),
            detail: error.to_string(),
        };
        std::fs::create_dir_all(state_dir).map_err(unavailable)?;
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(state_dir.join(STATE_DIR_LOCK_FILE_NAME))
            .map_err(unavailable)?;
        if let Err(error) = lock.try_lock_exclusive() {
            return Err(
                if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() {
                    StorageLayoutError::StateDirInUse {
                        state_dir: state_dir.to_path_buf(),
                    }
                } else {
                    unavailable(error)
                },
            );
        }
        Ok(Self {
            state_dir: state_dir.to_path_buf(),
            _lock: lock,
        })
    }

    /// The claimed directory.
    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_second_claim_on_the_same_directory_is_refused_until_the_first_drops() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state_dir = temp.path().join("state");
        let first = StateDirClaim::acquire(&state_dir).expect("first claim");
        assert!(state_dir.join(STATE_DIR_LOCK_FILE_NAME).is_file());

        let refused = StateDirClaim::acquire(&state_dir).expect_err("second claim refused");
        assert_eq!(
            refused,
            StorageLayoutError::StateDirInUse {
                state_dir: state_dir.clone()
            }
        );
        assert_eq!(
            refused.to_string(),
            format!(
                "state directory {} is in use by another Meta runtime",
                state_dir.display()
            )
        );

        drop(first);
        let again = StateDirClaim::acquire(&state_dir).expect("claim after release");
        assert_eq!(again.state_dir(), state_dir.as_path());
    }

    #[test]
    fn another_spelling_of_the_same_directory_is_the_same_claim() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state_dir = temp.path().join("state");
        let _first = StateDirClaim::acquire(&state_dir).expect("first claim");
        let other_spelling = temp.path().join("state").join("..").join("state");
        assert!(matches!(
            StateDirClaim::acquire(&other_spelling),
            Err(StorageLayoutError::StateDirInUse { .. })
        ));
    }
}

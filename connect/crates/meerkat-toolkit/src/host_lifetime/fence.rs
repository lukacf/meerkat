//! Shared physical entry fence for bounded local operations.
//! No database transaction or async wait is held through this file lock.

use super::control::HostLifetimeError;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub(crate) struct LocalFence {
    path: PathBuf,
}

impl LocalFence {
    pub(crate) fn open(path: PathBuf) -> Result<Self, HostLifetimeError> {
        let parent = path
            .parent()
            .ok_or(HostLifetimeError::InvalidSelection)?
            .canonicalize()
            .map_err(|_| HostLifetimeError::Unavailable)?;
        let name = path
            .file_name()
            .ok_or(HostLifetimeError::InvalidSelection)?;
        let path = parent.join(name);
        let result = Self { path };
        let _ = result.file()?;
        Ok(result)
    }

    fn file(&self) -> Result<File, HostLifetimeError> {
        // Exclusive creation cannot follow a dangling sidecar symlink and
        // create a file elsewhere. Existing names are opened without create,
        // then compared with the no-follow directory entry before locking.
        let file = match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&self.path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => OpenOptions::new()
                .read(true)
                .write(true)
                .open(&self.path)
                .map_err(|_| HostLifetimeError::Unavailable)?,
            Err(_) => return Err(HostLifetimeError::Unavailable),
        };
        let named =
            std::fs::symlink_metadata(&self.path).map_err(|_| HostLifetimeError::Unavailable)?;
        let opened = file
            .metadata()
            .map_err(|_| HostLifetimeError::Unavailable)?;
        if !named.is_file() || !opened.is_file() || !same_identity(&named, &opened) {
            return Err(HostLifetimeError::InvalidSelection);
        }
        Ok(file)
    }

    pub(crate) fn shared(&self) -> Result<File, HostLifetimeError> {
        let file = self.file()?;
        file.try_lock_shared()
            .map_err(|_| HostLifetimeError::Unavailable)?;
        Ok(file)
    }

    pub(crate) fn exclusive(&self) -> Result<File, HostLifetimeError> {
        let file = self.file()?;
        file.try_lock()
            .map_err(|_| HostLifetimeError::Unavailable)?;
        Ok(file)
    }
}

#[cfg(unix)]
fn same_identity(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    left.dev() == right.dev() && left.ino() == right.ino() && left.nlink() == 1
}
#[cfg(not(unix))]
fn same_identity(_: &std::fs::Metadata, _: &std::fs::Metadata) -> bool {
    // The current Python/native adapter composes POSIX flock on inherited
    // local process authority. Other physical profiles need their own adapter.
    false
}

pub(crate) fn canonical_database(path: &Path) -> Result<PathBuf, HostLifetimeError> {
    let path = path
        .canonicalize()
        .map_err(|_| HostLifetimeError::Unavailable)?;
    let metadata = std::fs::metadata(&path).map_err(|_| HostLifetimeError::Unavailable)?;
    if !metadata.is_file() || !same_identity(&metadata, &metadata) {
        return Err(HostLifetimeError::InvalidSelection);
    }
    Ok(path)
}

pub(crate) fn inbox_fence_path(database: &Path) -> PathBuf {
    let mut path = database.as_os_str().to_owned();
    path.push(".request-use.lock");
    PathBuf::from(path)
}

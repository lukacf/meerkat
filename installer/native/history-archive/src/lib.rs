//! Bounded private JSON retention, not a native report or ingestion receipt.
//! The caller owns the selected root's lifetime protection against path/UID
//! substitution. This library never authenticates that caller or root.
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    io::{self, Write},
    path::{Path, PathBuf},
};

pub const DIRECTORY: &str = "forge-ghe-history-v1";
pub const MAX_ARCHIVE_BYTES: usize = 256 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum ArchiveError {
    InvalidRoot,
    InvalidInstance,
    InvalidLimit,
    Storage,
}

pub struct SavedArchive {
    pub path: PathBuf,
    pub bytes: usize,
    pub sha256: String,
}

fn filename(instance: &str) -> Result<String, ArchiveError> {
    if instance.is_empty() || instance.len() > 256 || instance.chars().any(char::is_control) {
        return Err(ArchiveError::InvalidInstance);
    }
    // Filename is only a collision-resistant locator. Consumers must match the
    // full instance and original report/context, not trust a digest as authority.
    Ok(format!("{:x}.json", Sha256::digest(instance.as_bytes())))
}

struct BoundedWriter<W> {
    writer: W,
    bytes: usize,
    limit: usize,
    digest: Sha256,
}
impl<W: Write> Write for BoundedWriter<W> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if data.len() > self.limit.saturating_sub(self.bytes) {
            return Err(io::Error::other(
                "Historical archive exceeds its byte limit",
            ));
        }
        let written = self.writer.write(data)?;
        self.digest.update(&data[..written]);
        self.bytes += written;
        Ok(written)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

/// Write a new immutable-by-convention historical data file under one protected
/// state root. File and directory sync complete before success. Existing final
/// or pending files are never replaced. Failed partial bytes stay as `.pending`
/// and cannot be mistaken for a completed archive. No content is logged.
///
/// Only call outside native synchronous callbacks. Root's same-UID isolation and
/// stable directory ownership must continue through this write and collection.
#[cfg(unix)]
pub fn save<T: Serialize>(
    state_root: &Path,
    instance: &str,
    data: &T,
    max_bytes: usize,
) -> Result<SavedArchive, ArchiveError> {
    use std::{
        fs,
        fs::{File, OpenOptions},
        os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    };
    let name = filename(instance)?;
    if max_bytes == 0 || max_bytes > MAX_ARCHIVE_BYTES {
        return Err(ArchiveError::InvalidLimit);
    }
    let storage = || ArchiveError::Storage;
    let root = fs::symlink_metadata(state_root).map_err(|_| ArchiveError::InvalidRoot)?;
    if !state_root.is_absolute()
        || !root.is_dir()
        || root.mode() & 0o7777 != 0o700
        || state_root
            .canonicalize()
            .map_err(|_| ArchiveError::InvalidRoot)?
            != state_root
    {
        return Err(ArchiveError::InvalidRoot);
    }
    let directory = state_root.join(DIRECTORY);
    match fs::DirBuilder::new().mode(0o700).create(&directory) {
        Ok(()) => File::open(state_root)
            .and_then(|f| f.sync_all())
            .map_err(|_| storage())?,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
        Err(_) => return Err(storage()),
    }
    let metadata = fs::symlink_metadata(&directory).map_err(|_| storage())?;
    if !metadata.is_dir() || metadata.uid() != root.uid() || metadata.mode() & 0o7777 != 0o700 {
        return Err(ArchiveError::InvalidRoot);
    }
    let directory_handle = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&directory)
        .map_err(|_| storage())?;
    let opened = directory_handle.metadata().map_err(|_| storage())?;
    if (opened.dev(), opened.ino()) != (metadata.dev(), metadata.ino()) {
        return Err(ArchiveError::InvalidRoot);
    }
    let destination = directory.join(&name);
    let pending = directory.join(format!("{name}.pending"));
    // Refuse the normal duplicate case before writing any new data. The atomic
    // hard link below also refuses a destination created after this check.
    match fs::symlink_metadata(&destination) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => (),
        _ => return Err(storage()),
    }
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&pending)
        .map_err(|_| storage())?;
    let mut writer = BoundedWriter {
        writer: file,
        bytes: 0,
        limit: max_bytes,
        digest: Sha256::new(),
    };
    serde_json::to_writer(&mut writer, data).map_err(|_| storage())?;
    writer.write_all(b"\n").map_err(|_| storage())?;
    writer.flush().map_err(|_| storage())?;
    writer
        .writer
        .set_permissions(fs::Permissions::from_mode(0o400))
        .map_err(|_| storage())?;
    writer.writer.sync_all().map_err(|_| storage())?;
    // This never overwrites an existing name, including a symlink.
    fs::hard_link(&pending, &destination).map_err(|_| storage())?;
    directory_handle.sync_all().map_err(|_| storage())?;
    fs::remove_file(&pending).map_err(|_| storage())?;
    directory_handle.sync_all().map_err(|_| storage())?;
    Ok(SavedArchive {
        path: destination,
        bytes: writer.bytes,
        sha256: format!("{:x}", writer.digest.finalize()),
    })
}

#[cfg(not(unix))]
pub fn save<T: Serialize>(
    _: &Path,
    _: &str,
    _: &T,
    _: usize,
) -> Result<SavedArchive, ArchiveError> {
    Err(ArchiveError::InvalidRoot)
}

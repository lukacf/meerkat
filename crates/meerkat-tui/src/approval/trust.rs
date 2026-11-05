//! The host's record of workspace files it trusts.
//!
//! A file inside a workspace may have arrived with the repository (a cloned
//! `.rkat/` directory, a checked-out realm). rkat-tui therefore trusts such a
//! file only when it is byte-for-byte what rkat-tui itself last wrote there,
//! or what the user explicitly trusted after seeing a summary of it
//! (`/rules trust`). The digests live outside the workspace, in the user's
//! config directory: `<config>/trust/<workspace key>.toml`, one record per
//! workspace and realm. Nothing in the workspace can write that record.
//!
//! A file is trusted when
//!
//! - it is a regular file reached through no symlink (every path component
//!   below the workspace root is checked with `symlink_metadata`), and
//! - its SHA-256 equals the recorded digest for its workspace-relative path.
//!
//! The record is generic over workspace-relative files: the realm rules file
//! (`tui.toml`) and the project MCP config (`.rkat/mcp.toml`) both use it.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::paths::{AtomicWriteError, FileMode, check_no_symlinks, write_atomic};

/// Directory under the user config dir holding the records.
pub const TRUST_DIR: &str = "trust";

/// Longest file the record will hash.
const MAX_TRUSTED_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// Why a file is not trusted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Untrusted {
    /// The file, or a directory on the way to it, is a symlink.
    Symlinked(PathBuf),
    /// The file is not a regular file.
    NotRegular,
    /// rkat-tui never wrote or trusted this file.
    NotRecorded,
    /// The file changed since rkat-tui last wrote or trusted it.
    Changed,
    /// The file could not be inspected or read.
    Unreadable(String),
}

impl fmt::Display for Untrusted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Symlinked(component) => {
                write!(f, "{} is a symbolic link", component.display())
            }
            Self::NotRegular => f.write_str("it is not a regular file"),
            Self::NotRecorded => f.write_str(
                "rkat-tui did not write it and you have not trusted it (it may have come with the repository)",
            ),
            Self::Changed => f.write_str("it changed since rkat-tui last wrote or you trusted it"),
            Self::Unreadable(err) => write!(f, "it cannot be read ({err})"),
        }
    }
}

/// The trust state of one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustStatus {
    /// The file does not exist (nothing to trust or distrust).
    Missing,
    /// The file is what rkat-tui wrote or the user trusted.
    Trusted,
    /// The file is not trusted.
    Untrusted(Untrusted),
}

/// Failures writing the trust record.
#[derive(Debug, thiserror::Error)]
pub enum TrustError {
    /// The file to trust cannot be trusted at all (symlink, not regular,
    /// unreadable).
    #[error("cannot trust {}: {reason}", path.display())]
    Refused {
        /// The file.
        path: PathBuf,
        /// Why.
        reason: Untrusted,
    },
    /// The record could not be serialized.
    #[error("cannot encode the trust record: {0}")]
    Encode(#[from] toml::ser::Error),
    /// The record could not be written.
    #[error(transparent)]
    Write(#[from] AtomicWriteError),
}

/// On-disk form of a record.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct RecordFile {
    /// The workspace the record is for (informational).
    #[serde(default)]
    workspace: String,
    /// Workspace-relative path → hex SHA-256.
    #[serde(default)]
    files: BTreeMap<String, String>,
}

/// The digests of one workspace's trusted files.
#[derive(Debug, Clone)]
pub struct HostTrustRecord {
    /// Where the record lives (`None`: kept in memory only).
    path: Option<PathBuf>,
    /// The user config directory (the trusted root of `path`).
    config_dir: Option<PathBuf>,
    /// The workspace root files are relative to.
    workspace: PathBuf,
    record: RecordFile,
}

impl HostTrustRecord {
    /// The record of `workspace` (and `realm_id`) under `config_dir`. A
    /// missing or unreadable record is empty: nothing is trusted.
    #[must_use]
    pub fn open(config_dir: &Path, workspace: &Path, realm_id: &str) -> Self {
        let key = workspace_key(workspace, realm_id);
        let path = config_dir.join(TRUST_DIR).join(format!("{key}.toml"));
        let record = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default();
        Self {
            path: Some(path),
            config_dir: Some(config_dir.to_path_buf()),
            workspace: workspace.to_path_buf(),
            record,
        }
    }

    /// A record kept in memory only (no config directory).
    #[must_use]
    pub fn in_memory(workspace: &Path) -> Self {
        Self {
            path: None,
            config_dir: None,
            workspace: workspace.to_path_buf(),
            record: RecordFile::default(),
        }
    }

    /// Where the record is stored, if anywhere.
    #[must_use]
    pub fn record_path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The key a file is recorded under: its path relative to the
    /// workspace, or its absolute path when it lies outside.
    fn key(&self, file: &Path) -> String {
        file.strip_prefix(&self.workspace)
            .unwrap_or(file)
            .to_string_lossy()
            .into_owned()
    }

    /// Inspect `file` and return its contents when it may be trusted at all.
    fn inspect(&self, file: &Path) -> Result<Option<Vec<u8>>, Untrusted> {
        let checked = match file.strip_prefix(&self.workspace) {
            Ok(rel) => {
                let mut current = self.workspace.clone();
                let mut result = Ok(());
                for component in rel.components() {
                    current.push(component.as_os_str());
                    match std::fs::symlink_metadata(&current) {
                        Ok(meta) if meta.file_type().is_symlink() => {
                            result = Err(Untrusted::Symlinked(current.clone()));
                            break;
                        }
                        Ok(_) => {}
                        Err(err) if err.kind() == std::io::ErrorKind::NotFound => break,
                        Err(err) => {
                            result = Err(Untrusted::Unreadable(err.to_string()));
                            break;
                        }
                    }
                }
                result
            }
            Err(_) => check_no_symlinks(file).map_err(|err| match err {
                AtomicWriteError::Symlink { component, .. } => Untrusted::Symlinked(component),
                other => Untrusted::Unreadable(other.to_string()),
            }),
        };
        checked?;
        let meta = match std::fs::symlink_metadata(file) {
            Ok(meta) => meta,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(Untrusted::Unreadable(err.to_string())),
        };
        if !meta.file_type().is_file() {
            return Err(Untrusted::NotRegular);
        }
        if meta.len() > MAX_TRUSTED_FILE_BYTES {
            return Err(Untrusted::Unreadable("the file is too large".to_string()));
        }
        let bytes = std::fs::read(file).map_err(|e| Untrusted::Unreadable(e.to_string()))?;
        Ok(Some(bytes))
    }

    /// The trust state of `file`.
    #[must_use]
    pub fn check(&self, file: &Path) -> TrustStatus {
        let bytes = match self.inspect(file) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return TrustStatus::Missing,
            Err(reason) => return TrustStatus::Untrusted(reason),
        };
        match self.record.files.get(&self.key(file)) {
            None => TrustStatus::Untrusted(Untrusted::NotRecorded),
            Some(digest) if *digest == hex_digest(&bytes) => TrustStatus::Trusted,
            Some(_) => TrustStatus::Untrusted(Untrusted::Changed),
        }
    }

    /// Trust `file` as it is now (rkat-tui just wrote it, or the user
    /// trusted it after seeing a summary) and save the record. A missing
    /// file forgets its digest.
    ///
    /// # Errors
    ///
    /// [`TrustError::Refused`] when the file is symlinked, not regular or
    /// unreadable; [`TrustError::Write`] when the record cannot be saved.
    pub fn record_file(&mut self, file: &Path) -> Result<(), TrustError> {
        let key = self.key(file);
        match self.inspect(file) {
            Ok(Some(bytes)) => {
                self.record.files.insert(key, hex_digest(&bytes));
            }
            Ok(None) => {
                self.record.files.remove(&key);
            }
            Err(reason) => {
                return Err(TrustError::Refused {
                    path: file.to_path_buf(),
                    reason,
                });
            }
        }
        self.save()
    }

    fn save(&mut self) -> Result<(), TrustError> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        self.record.workspace = self.workspace.to_string_lossy().into_owned();
        let text = toml::to_string_pretty(&self.record)?;
        write_atomic(
            path,
            text.as_bytes(),
            FileMode::Private,
            self.config_dir.as_deref(),
        )?;
        Ok(())
    }
}

/// Lower-case hex SHA-256 of `bytes`.
#[must_use]
pub fn hex_digest(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

/// The record file name of a workspace and realm: a digest of both, so the
/// name reveals nothing and cannot collide across workspaces.
fn workspace_key(workspace: &Path, realm_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(workspace.to_string_lossy().as_bytes());
    hasher.update([0]);
    hasher.update(realm_id.as_bytes());
    let digest = hasher.finalize();
    hex_digest(&digest).chars().take(32).collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn written_files_are_trusted_until_they_change() {
        let config = tempfile::tempdir().unwrap();
        let ws = tempfile::tempdir().unwrap();
        let file = ws.path().join(".rkat/realm/tui.toml");
        let mut record = HostTrustRecord::open(config.path(), ws.path(), "r");
        assert_eq!(record.check(&file), TrustStatus::Missing);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "[[allow]]\ntool = \"shell\"\n").unwrap();
        // It came with the repository: not trusted.
        assert_eq!(
            record.check(&file),
            TrustStatus::Untrusted(Untrusted::NotRecorded)
        );
        record.record_file(&file).unwrap();
        assert_eq!(record.check(&file), TrustStatus::Trusted);
        // The record survives a restart.
        let reopened = HostTrustRecord::open(config.path(), ws.path(), "r");
        assert_eq!(reopened.check(&file), TrustStatus::Trusted);
        // Another realm of the same workspace has its own record.
        let other = HostTrustRecord::open(config.path(), ws.path(), "s");
        assert_ne!(other.check(&file), TrustStatus::Trusted);
        // Edited behind rkat-tui's back.
        std::fs::write(&file, "[[allow]]\ntool = \"*\"\n").unwrap();
        assert_eq!(
            reopened.check(&file),
            TrustStatus::Untrusted(Untrusted::Changed)
        );
        // The record itself is outside the workspace and private.
        let path = reopened.record_path().unwrap();
        assert!(path.starts_with(config.path()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_files_and_directories_are_never_trusted() {
        let config = tempfile::tempdir().unwrap();
        let ws = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("tui.toml"), "x").unwrap();
        // The file itself is a symlink.
        let link = ws.path().join("tui.toml");
        std::os::unix::fs::symlink(outside.path().join("tui.toml"), &link).unwrap();
        let mut record = HostTrustRecord::open(config.path(), ws.path(), "r");
        assert!(matches!(
            record.check(&link),
            TrustStatus::Untrusted(Untrusted::Symlinked(_))
        ));
        assert!(record.record_file(&link).is_err());
        // A directory on the way is a symlink.
        std::os::unix::fs::symlink(outside.path(), ws.path().join(".rkat")).unwrap();
        let through = ws.path().join(".rkat/tui.toml");
        assert!(matches!(
            record.check(&through),
            TrustStatus::Untrusted(Untrusted::Symlinked(_))
        ));
    }
}

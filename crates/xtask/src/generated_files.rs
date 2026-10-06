//! Writing generated artifacts without touching unchanged files.
//!
//! The machine and protocol generators re-render every artifact on each run
//! (the pre-push machine hook runs both). Rewriting a byte-identical file
//! still bumps its mtime, and Cargo's dep-info tracks generated sources such
//! as `crates/meerkat-core/src/generated/session_document.rs`: a retry that
//! only touched TLA or docs then rebuilt meerkat-core and everything
//! downstream. Generation and drift checks are unchanged; only the write is
//! skipped when the bytes on disk already match.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use anyhow::{Context, Result};

/// What [`write_if_changed`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeneratedWrite {
    /// The file was missing or differed and now holds `contents`.
    Written,
    /// The file already held exactly `contents`; it was not touched.
    Unchanged,
}

/// Write `contents` to `path` only when the file is missing or its bytes
/// differ, creating parent directories as needed.
pub fn write_if_changed(path: &Path, contents: &[u8]) -> Result<GeneratedWrite> {
    match fs::read(path) {
        Ok(existing) if existing == contents => return Ok(GeneratedWrite::Unchanged),
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("read {}", path.display()));
        }
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create dir {}", parent.display()))?;
    }
    fs::write(path, contents).with_context(|| format!("write {}", path.display()))?;
    Ok(GeneratedWrite::Written)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::{GeneratedWrite, write_if_changed};
    use std::fs::{self, File};
    use std::time::{Duration, SystemTime};

    fn set_old_mtime(path: &std::path::Path) -> SystemTime {
        let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        File::options()
            .write(true)
            .open(path)
            .and_then(|file| file.set_modified(old))
            .expect("set an old mtime");
        old
    }

    #[test]
    fn unchanged_bytes_leave_the_file_untouched_and_changed_bytes_write() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested").join("generated.rs");

        assert_eq!(
            write_if_changed(&path, b"one").expect("first write"),
            GeneratedWrite::Written
        );
        assert_eq!(fs::read(&path).expect("read"), b"one");

        let old = set_old_mtime(&path);
        assert_eq!(
            write_if_changed(&path, b"one").expect("same bytes"),
            GeneratedWrite::Unchanged
        );
        assert_eq!(
            fs::metadata(&path)
                .and_then(|m| m.modified())
                .expect("mtime"),
            old,
            "a byte-identical rewrite must not bump the mtime"
        );

        assert_eq!(
            write_if_changed(&path, b"two").expect("new bytes"),
            GeneratedWrite::Written
        );
        assert_eq!(fs::read(&path).expect("read"), b"two");
        assert_ne!(
            fs::metadata(&path)
                .and_then(|m| m.modified())
                .expect("mtime"),
            old,
            "changed bytes must be written"
        );
    }
}

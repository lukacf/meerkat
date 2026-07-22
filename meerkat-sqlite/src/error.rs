//! Crate error type and storage-level error classification.

use std::path::PathBuf;

/// Errors produced by the shared SQLite mechanics.
#[derive(Debug, thiserror::Error)]
pub enum SqliteStoreError {
    /// Filesystem-level failure (creating parent directories, fence lock
    /// files, ...).
    #[error("sqlite store io error: {0}")]
    Io(#[from] std::io::Error),

    /// Underlying SQLite failure.
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    /// The file's schema ledger records a version newer than this binary
    /// supports. This is a refusal, not a corruption: a newer binary has
    /// migrated the file and this binary must not touch it. Surfaces report
    /// it as a typed, health-visible certification failure (a rollback
    /// candidate fails cleanly) rather than crash-looping.
    #[error(
        "schema for domain `{domain}` is from the future: file has version {found}, \
         this binary supports up to {supported}"
    )]
    SchemaFromTheFuture {
        domain: String,
        found: i64,
        supported: i64,
    },

    /// A registered migration failed while being applied. The surrounding
    /// transaction is rolled back; the file is left at its prior version.
    #[error("migration {version} (`{name}`) for domain `{domain}` failed: {source}")]
    MigrationFailed {
        domain: String,
        version: i64,
        name: String,
        #[source]
        source: rusqlite::Error,
    },

    /// A domain registered an invalid migration list (non-contiguous or
    /// not starting at version 1). This is a programming error in the store
    /// crate, caught before any file is touched.
    #[error("domain `{domain}` registered an invalid migration list: {detail}")]
    InvalidMigrationList { domain: String, detail: String },

    /// The connection profile refused the requested open (for example a
    /// non-creating profile pointed at a missing file).
    #[error("cannot open `{path}` with profile {profile}: {detail}")]
    OpenRefused {
        path: PathBuf,
        profile: &'static str,
        detail: String,
    },

    /// The exclusive maintenance fence is held for this database: storage is
    /// under offline maintenance and the operation must not proceed. (Also
    /// returned by [`crate::fence::ExclusiveFence::acquire`] when in-flight
    /// operations did not drain within the deadline.)
    #[error("maintenance fence is held for `{path}`; storage is under offline maintenance")]
    MaintenanceFenceHeld { path: PathBuf },
}

/// Storage-level classification of a SQLite error.
///
/// This is deliberately narrower than the store-boundary taxonomy
/// (transient / stale / corrupt): staleness (CAS conflicts, revision guards)
/// is a store-contract concept invisible at this layer, so store crates map
/// their own guard failures to their stale variants and use this
/// classification for everything that reaches raw SQLite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqliteErrorClass {
    /// Lock contention or interruption; safe to retry only for idempotent or
    /// CAS-keyed operations (see the crate-level retryability note).
    Transient,
    /// The file is not (or no longer) a usable database.
    Corrupt,
    /// Everything else: constraint violations, misuse, API errors. The store
    /// layer decides what these mean.
    Other,
}

/// Classify a rusqlite error at the storage level.
pub fn classify_sqlite_error(error: &rusqlite::Error) -> SqliteErrorClass {
    use rusqlite::ErrorCode;
    match error {
        rusqlite::Error::SqliteFailure(f, _) => match f.code {
            ErrorCode::DatabaseBusy
            | ErrorCode::DatabaseLocked
            | ErrorCode::OperationInterrupted => SqliteErrorClass::Transient,
            ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase => SqliteErrorClass::Corrupt,
            _ => SqliteErrorClass::Other,
        },
        _ => SqliteErrorClass::Other,
    }
}

/// True when the error is SQLITE_BUSY or SQLITE_LOCKED — the nonblocking
/// admission probes (write fences) map exactly these to a typed backoff.
pub fn is_busy_or_locked(error: &rusqlite::Error) -> bool {
    use rusqlite::ErrorCode;
    matches!(
        error,
        rusqlite::Error::SqliteFailure(f, _)
            if matches!(f.code, ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sqlite_failure(code: rusqlite::ErrorCode) -> rusqlite::Error {
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code,
                extended_code: 0,
            },
            None,
        )
    }

    #[test]
    fn busy_and_locked_classify_transient() {
        for code in [
            rusqlite::ErrorCode::DatabaseBusy,
            rusqlite::ErrorCode::DatabaseLocked,
        ] {
            let err = sqlite_failure(code);
            assert_eq!(classify_sqlite_error(&err), SqliteErrorClass::Transient);
            assert!(is_busy_or_locked(&err));
        }
    }

    #[test]
    fn corruption_classifies_corrupt() {
        for code in [
            rusqlite::ErrorCode::DatabaseCorrupt,
            rusqlite::ErrorCode::NotADatabase,
        ] {
            let err = sqlite_failure(code);
            assert_eq!(classify_sqlite_error(&err), SqliteErrorClass::Corrupt);
            assert!(!is_busy_or_locked(&err));
        }
    }

    #[test]
    fn constraint_violation_classifies_other() {
        let err = sqlite_failure(rusqlite::ErrorCode::ConstraintViolation);
        assert_eq!(classify_sqlite_error(&err), SqliteErrorClass::Other);
        assert!(!is_busy_or_locked(&err));
    }
}

//! Crate error type and storage-level error classification.

use std::path::PathBuf;

/// Whether the explicit offline pre-0.8.10 bridge can authenticate an
/// unledgered on-disk catalog.
///
/// This exists so a refusal can say what is actually true about the file in
/// front of it. Naming the bridge as a remedy for a catalog it cannot
/// authenticate sends the operator into a dead end.
/// SCOPE. Both variants are decided from the file's **catalog shape** alone.
/// No durable record is read, decoded, or admitted to reach this answer, so
/// `CatalogAuthenticated` must never be rendered as a promise that every
/// record survives the bridge. What it does justify is running the bridge:
/// preparation callbacks refuse per record, naming what stayed behind, rather
/// than aborting the domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeEligibility {
    /// Some frozen released catalog registered by this binary authenticates
    /// the on-disk shape exactly, so the explicit bridge will run on this
    /// domain instead of refusing it outright. Individual records may still
    /// be left behind; the bridge names each one it could not carry.
    CatalogAuthenticated,
    /// No frozen released catalog authenticates the on-disk shape. The file
    /// predates, or diverges from, everything this binary can prove.
    Unrecognized,
}

impl BridgeEligibility {
    /// True when the explicit bridge can authenticate the catalog and will
    /// therefore run. Says nothing about individual records; see the type
    /// docs.
    pub fn catalog_authenticates(self) -> bool {
        matches!(self, Self::CatalogAuthenticated)
    }

    /// The operator-facing remedy sentence for an unledgered domain with this
    /// eligibility, ready to append to a refusal.
    ///
    /// It lives here, beside the answer it is derived from, because the same
    /// refusal reaches operators through more than one error type. A remedy
    /// carried by only one of them is how a caller ends up staring at a bare
    /// "refusing to infer or stamp an unversioned schema" with no next step,
    /// and two hand-copied remedies are how one of them ends up stale.
    pub fn remedy_sentence(self) -> &'static str {
        match self {
            Self::CatalogAuthenticated => {
                " This realm was last written before the 0.8.10 durable-state floor and its \
                 on-disk schema is one this binary recognizes, so run the explicit \
                 current-binary bridge once (`rkat --state-root <ROOT> --realm <REALM> storage \
                 migrate --apply --bridge-pre-0-8-10`), then retry the original command. Only \
                 the schema shape has been checked here: the bridge inspects the stored \
                 records themselves and prints any it cannot carry forward, leaving those \
                 records' bytes untouched"
            }
            Self::Unrecognized => {
                " No source catalog the `--bridge-pre-0-8-10` bridge can recover matches this \
                 domain's on-disk shape, so running that bridge will not recover this domain. \
                 The file predates, or diverges from, every source catalog this binary can \
                 bridge here. Nothing is deleted or rewritten: keep the realm and open it with \
                 the version that wrote it, or start a new realm for this binary. The \
                 read-only `rkat --state-root <ROOT> --realm <REALM> storage migrate` dry run \
                 prints the per-domain detail; report that object list if you need this shape \
                 bridged"
            }
        }
    }
}

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

    /// The ledger records a real domain version, but that version is not one
    /// of the exact released predecessors this binary is willing to upgrade.
    /// This covers both versions below the compatibility floor and gaps
    /// between an allowed predecessor and the current schema. Refusal happens
    /// before schema or ledger mutation.
    #[error(
        "schema for domain `{domain}` is not a supported predecessor: file has version {found}, \
         this binary supports version {supported} and accepts existing versions {allowed:?}"
    )]
    UnsupportedSchemaPredecessor {
        domain: String,
        found: i64,
        supported: i64,
        allowed: Vec<i64>,
    },

    /// A ledger row names an otherwise allowed current or released version,
    /// but the domain-owned catalog does not exactly match that version's
    /// frozen schema fingerprint. The row alone is not authority to
    /// reinterpret a candidate or partially migrated schema.
    #[error(
        "schema fingerprint for domain `{domain}` version {version} does not match the \
         allowed schema: {detail}"
    )]
    SchemaFingerprintMismatch {
        domain: String,
        version: i64,
        detail: String,
    },

    /// A file has no ledger row for a domain but already contains one or more
    /// objects owned by that domain. At the 0.8.10 compatibility floor this
    /// is neither a fresh domain nor an authenticated released predecessor:
    /// silently running idempotent DDL over it would bless an unknown or
    /// unreleased candidate schema.
    #[error(
        "schema domain `{domain}` has no ledger row but already owns objects {objects:?}; \
         refusing to infer or stamp an unversioned schema.{}",
        bridgeable.remedy_sentence()
    )]
    UnledgeredDomainObjects {
        domain: String,
        objects: Vec<String>,
        /// Whether the explicit offline bridge can authenticate this exact
        /// on-disk catalog, decided at the raise site where the domain's
        /// frozen verifiers and the connection are both in scope. Callers
        /// that offer the bridge as a remedy must consult this rather than
        /// naming it unconditionally.
        bridgeable: BridgeEligibility,
    },

    /// Explicit maintenance could not authenticate an unledgered owned
    /// catalog as any exact migration prefix or frozen predecessor through
    /// the requested target. No preparation, migration, or ledger mutation
    /// has run.
    #[error(
        "unledgered schema domain `{domain}` does not match any authorized source catalog \
         through version {target_version}; found owned objects {objects:?}"
    )]
    UnledgeredSchemaNoMatch {
        domain: String,
        target_version: i64,
        objects: Vec<String>,
    },

    /// Explicit maintenance found more than one exact authorized source
    /// version for an unledgered catalog. Inferring a version would be
    /// ambiguous, so the file remains untouched.
    #[error(
        "unledgered schema domain `{domain}` ambiguously matches source versions {matches:?} \
         through requested target version {target_version}"
    )]
    UnledgeredSchemaAmbiguous {
        domain: String,
        target_version: i64,
        matches: Vec<i64>,
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

    /// A migration body ended the runner's IMMEDIATE transaction (COMMIT or
    /// ROLLBACK, with or without re-BEGINning a fresh one), separating its
    /// schema work from the ledger stamp. Custody is verified after every
    /// body via a runner-owned savepoint; the domain is left unstamped.
    #[error(
        "migration {version} (`{name}`) for domain `{domain}` ended the runner's transaction; \
         migration bodies must not COMMIT or ROLLBACK"
    )]
    MigrationBrokeTransaction {
        domain: String,
        version: i64,
        name: String,
    },

    /// The `meerkat_schema` ledger table exists but is not the pinned shape
    /// (`domain TEXT PRIMARY KEY, version INTEGER NOT NULL`), carries more
    /// than one row for a domain, or records a non-positive version. This is
    /// corrupt or foreign ledger state: it is refused, never healed by
    /// re-running migrations over it.
    #[error("meerkat_schema ledger is malformed: {detail}")]
    LedgerMalformed { detail: String },

    /// A profile whose journal policy is
    /// [`JournalPolicy::EstablishWal`](crate::JournalPolicy::EstablishWal)
    /// asked SQLite to establish `journal_mode=WAL` and SQLite reported a
    /// different effective mode without raising an error (the journal-mode
    /// pragma can silently keep the old mode). The connection does not
    /// satisfy the profile's durability policy.
    #[error("could not establish journal_mode=WAL on `{path}`: effective mode is `{actual}`")]
    WalNotEstablished { path: PathBuf, actual: String },

    /// Converting an existing rollback-journal database to WAL needs a brief
    /// exclusive lock, and the journal-mode pragma reports `SQLITE_BUSY`
    /// without consulting the busy handler while another connection holds the
    /// file. The bounded retry spent its whole budget without winning that
    /// lock, so the open fails closed rather than serving durable read-write
    /// traffic from a rollback-journal database, where every write takes a
    /// database-wide EXCLUSIVE lock with no reader/writer separation.
    ///
    /// The database is left exactly as found; the operator remedy is to retry
    /// once the contending connection releases the file (a second boot
    /// attempt normally wins it, since the conversion is a no-op the moment
    /// the file is WAL).
    #[error(
        "could not establish journal_mode=WAL on `{path}`: the conversion stayed lock-contended \
         for {waited_ms} ms"
    )]
    WalConversionContended {
        path: PathBuf,
        waited_ms: u64,
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
///
/// Adoption contract: store crates route every raw [`rusqlite::Error`]
/// through this one classifier when deciding transient-vs-corrupt at their
/// store boundary, instead of re-matching SQLite error codes locally.
/// [`SqliteErrorClass::Other`] is the store layer's to interpret (constraint
/// violations become CAS/stale semantics there, not here). Classification
/// alone never authorizes a retry — see the crate-level retryability note.
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
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
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

    /// This refusal reaches operators through more than one error type, and
    /// after a partial bridge it reached them through this one with no next
    /// step at all: a bare "refusing to infer or stamp an unversioned schema".
    /// The remedy belongs to the eligibility answer, so every rendering of the
    /// refusal carries it.
    #[test]
    fn unledgered_domain_objects_carry_their_remedy_in_every_rendering() {
        let authenticated = SqliteStoreError::UnledgeredDomainObjects {
            domain: "session-store".to_string(),
            objects: vec!["table:sessions (expected table)".to_string()],
            bridgeable: BridgeEligibility::CatalogAuthenticated,
        }
        .to_string();
        assert!(
            authenticated.contains("--bridge-pre-0-8-10"),
            "a bridgeable catalog must name the command that recovers it: {authenticated}"
        );

        let unrecognized = SqliteStoreError::UnledgeredDomainObjects {
            domain: "session-store".to_string(),
            objects: vec!["table:sessions (expected table)".to_string()],
            bridgeable: BridgeEligibility::Unrecognized,
        }
        .to_string();
        assert!(
            !unrecognized.contains("--apply"),
            "an unrecognized catalog must not be handed a runnable apply command: {unrecognized}"
        );
        assert!(
            unrecognized.contains("will not recover this domain"),
            "the refusal must say plainly that the bridge cannot help: {unrecognized}"
        );
    }

    /// The whole point of the remedy sentences is that an operator can read
    /// them. A stray run of spaces from a botched line continuation shipped
    /// once already.
    #[test]
    fn remedy_sentences_carry_no_botched_line_continuation() {
        for eligibility in [
            BridgeEligibility::CatalogAuthenticated,
            BridgeEligibility::Unrecognized,
        ] {
            let sentence = eligibility.remedy_sentence();
            assert!(
                !sentence.contains("  "),
                "{eligibility:?} remedy carries a run of spaces: {sentence:?}"
            );
        }
    }
}

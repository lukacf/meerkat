//! Shared SQLite mechanics for Meerkat stores.
//!
//! This crate is the single home for the connection policy, schema-evolution,
//! and maintenance-fence machinery that was previously copy-pasted across the
//! store crates (six independently authored PRAGMA setups with divergent busy
//! timeouts, `CREATE TABLE IF NOT EXISTS` everywhere, and no version ledger).
//! See `docs/plans/storage-unification-plan.md` (Phase 3) in the workspace
//! root for the design rationale.
//!
//! What lives here:
//!
//! - [`profile`]: DDL-free connection opening under named policy profiles
//!   ([`ConnectionProfile::Primary`], [`ConnectionProfile::ReadOnly`],
//!   [`ConnectionProfile::Maintenance`]). Opening a connection never runs
//!   schema DDL; stores apply their [`ledger`] domain after opening. The
//!   optional [`OpenOptions::schema_preflight`] refuses a future file before
//!   any mutating pragma, and [`WriteContact`] types each profile's honest
//!   no-write guarantee (WAL reads may need sidecar files).
//! - [`ledger`]: the per-file migration ledger (`meerkat_schema(domain,
//!   version)`) with the pinned concurrent-open transaction protocol and the
//!   typed [`SqliteStoreError::SchemaFromTheFuture`] refusal. Ledger state
//!   that is malformed (wrong shape, duplicate rows, non-positive versions)
//!   is refused typed, never healed.
//! - [`json_column`]: the TEXT-or-BLOB tolerant JSON column codec.
//! - [`fence`]: the per-operation maintenance-fence guards. Store operations
//!   take a shared guard; offline migration takes the exclusive side and
//!   waits for in-flight operations to drain. Stores built on this crate hold
//!   no bespoke opener, which is what makes them fence-aware for free.
//! - [`error`]: the crate error type plus the storage-level error
//!   classification (transient / corrupt) that store crates layer their
//!   staleness semantics on top of. Adoption contract: store crates route
//!   every raw rusqlite error through [`classify_sqlite_error`] instead of
//!   re-matching SQLite error codes locally.
//!
//! # Retryability
//!
//! Error classification alone does not authorize a retry: a transient failure
//! can land after a write committed but before success became observable, and
//! blind retry duplicates non-idempotent effects. Automatic retry is only
//! sound for idempotent or CAS-keyed operations; an indeterminate
//! non-idempotent write requires outcome reconciliation (read back, then
//! decide) before any retry. Bounded waiting for lock contention is the
//! connection busy handler's job, not a caller retry loop.

pub mod error;
pub mod fence;
pub mod json_column;
pub mod ledger;
pub mod profile;

pub use error::{SqliteErrorClass, SqliteStoreError, classify_sqlite_error, is_busy_or_locked};
pub use fence::{ExclusiveFence, OperationGuard, fence_lock_path};
pub use json_column::JsonColumnBytes;
pub use ledger::{
    LedgerReport, Migration, SchemaDomain, apply_domain_migrations, domain_version,
    refuse_future_schema,
};
pub use profile::{
    ConnectionProfile, OpenOptions, SHARED_BUSY_TIMEOUT, WriteContact, begin_immediate, open,
    open_with,
};

//! Per-file schema migration ledger.
//!
//! Every SQLite file carries a `meerkat_schema(domain TEXT PRIMARY KEY,
//! version INTEGER NOT NULL)` table with exactly one row per schema domain.
//! Each store registers its ordered migrations, the exact released versions
//! it may upgrade, and every `main.sqlite_schema` object it owns.
//!
//! # The pinned transaction protocol
//!
//! Idempotent migration functions alone do not make concurrent opens safe,
//! so the runner pins a minimal protocol (consumed unchanged by downstream
//! adopters):
//!
//! 1. exactly one ledger row per domain;
//! 2. `BEGIN IMMEDIATE`;
//! 3. re-read the version *inside* that transaction;
//! 4. reject a future version before any mutation
//!    ([`SqliteStoreError::SchemaFromTheFuture`]);
//! 5. execute the pending migrations and the ledger update atomically in the
//!    same transaction — custody is verified with a runner-owned savepoint
//!    around each body, so a body that COMMITs or ROLLBACKs underneath the
//!    runner is refused ([`SqliteStoreError::MigrationBrokeTransaction`])
//!    even when it re-BEGINs a fresh transaction afterwards.
//!
//! A table merely *named* `meerkat_schema` is not trusted: before any read
//! the pinned column shape is validated against `main`'s catalog, versions
//! must be positive, and at most one row may exist per domain
//! ([`SqliteStoreError::LedgerMalformed`] otherwise). All ledger SQL is
//! `main.`-qualified, so a TEMP table shadowing the name can neither satisfy
//! nor bypass the ledger.
//!
//! Concurrent opens race safely: the loser's in-transaction re-read sees the
//! winner's committed version and applies nothing.
//!
//! # Compatibility floor
//!
//! A missing domain row is accepted only when none of that domain's declared
//! objects exist. That is a fresh domain (possibly in a file containing
//! foreign co-tenant domains), so its dedicated `initialize_current`
//! function may build the current shape directly. A missing row plus an
//! owned table, index, trigger, or view is refused as
//! [`SqliteStoreError::UnledgeredDomainObjects`]; this runner never infers a
//! version from ambient DDL or stamps an unauthenticated historical shape.
//!
//! A present row may be current or one of the exact released predecessor
//! versions declared by the domain. Pre-floor versions and gaps are refused
//! as [`SqliteStoreError::UnsupportedSchemaPredecessor`]. Eligibility is
//! re-established under the same `BEGIN IMMEDIATE` transaction as the DDL
//! and ledger update. The ledger table itself is not created until after that
//! decision, so a refusal leaves both schema and ledger unchanged.
//!
//! Foreign domain rows (other stores co-tenanting the same file) are never
//! read or written; the ledger keys strictly by domain name.

use rusqlite::{Connection, OptionalExtension, Transaction};
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use crate::error::SqliteStoreError;

const CREATE_LEDGER_SQL: &str = "CREATE TABLE IF NOT EXISTS main.meerkat_schema (
    domain TEXT PRIMARY KEY,
    version INTEGER NOT NULL
)";

/// Custody marker established inside the runner's transaction immediately
/// before each migration body. A savepoint is discarded when its enclosing
/// transaction ends — by COMMIT or ROLLBACK alike — so it survives the body
/// exactly when the runner's transaction does.
const CUSTODY_SAVEPOINT_SQL: &str = "SAVEPOINT meerkat_migration_custody";
const CUSTODY_RELEASE_SQL: &str = "RELEASE SAVEPOINT meerkat_migration_custody";

/// One schema migration step for a domain.
#[derive(Debug)]
pub struct Migration {
    /// Target version this migration brings the domain to. Versions are
    /// contiguous and start at 1.
    pub version: i64,
    /// Stable human-readable name (shows up in errors and reports).
    pub name: &'static str,
    /// The migration body. Runs inside the runner's IMMEDIATE transaction;
    /// it must not end that transaction (nested savepoints of its own are
    /// fine). Bodies lifted from historical upgrade functions keep their
    /// internal idempotence guards.
    pub apply: fn(&Transaction<'_>) -> Result<(), rusqlite::Error>,
}

/// Frozen verifier for one released predecessor version.
#[derive(Debug)]
pub struct SchemaPredecessor {
    pub version: i64,
    pub verify: fn(&Connection) -> Result<(), String>,
}

/// SQLite catalog object kind owned by a schema domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaObjectKind {
    Table,
    Index,
    Trigger,
    View,
}

impl SchemaObjectKind {
    fn sqlite_name(self) -> &'static str {
        match self {
            Self::Table => "table",
            Self::Index => "index",
            Self::Trigger => "trigger",
            Self::View => "view",
        }
    }
}

/// One exact `main.sqlite_schema` object name owned by a domain.
///
/// Names are the eligibility boundary, not merely documentation: any object
/// using one of these names makes an unledgered domain non-fresh, including
/// an object of the wrong kind. The expected kind is retained for validation
/// and health-visible diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchemaObject {
    pub kind: SchemaObjectKind,
    pub name: &'static str,
}

/// A store's schema domain: its ledger name plus the ordered migration list.
#[derive(Debug)]
pub struct SchemaDomain {
    /// Ledger key. Kebab-case, stable forever (it is persisted in files).
    pub name: &'static str,
    /// Ordered migrations, versions contiguous from 1.
    pub migrations: &'static [Migration],
    /// Initialize a genuinely fresh domain directly at the current schema.
    ///
    /// This is intentionally separate from historical upgrades. A current
    /// base initializer may already contain objects that a released
    /// predecessor transition creates or rebuilds; replaying the transition
    /// on fresh state would either collide or weaken strict collision
    /// detection with idempotent DDL.
    pub initialize_current: fn(&Transaction<'_>) -> Result<(), rusqlite::Error>,
    /// Exact existing released versions that this binary may open. The
    /// current supported version is included for an explicit manifest even
    /// though it needs no migration.
    pub allowed_existing_versions: &'static [i64],
    /// Exact catalog verifiers for every allowed version below current.
    pub released_predecessors: &'static [SchemaPredecessor],
    /// Complete set of catalog objects owned by this domain across its
    /// current schema. Foreign co-tenant objects are deliberately absent.
    pub owned_objects: &'static [SchemaObject],
    /// Names owned by supported predecessors but intentionally absent from
    /// the current schema. They remain reserved for fresh-domain detection
    /// and predecessor fingerprints.
    pub retired_objects: &'static [SchemaObject],
}

impl SchemaDomain {
    /// Highest version this binary knows for the domain.
    pub fn supported_version(&self) -> i64 {
        self.migrations.last().map_or(0, |m| m.version)
    }

    fn validate(&self) -> Result<(), SqliteStoreError> {
        for (idx, migration) in self.migrations.iter().enumerate() {
            let expected = idx as i64 + 1;
            if migration.version != expected {
                return Err(SqliteStoreError::InvalidMigrationList {
                    domain: self.name.to_string(),
                    detail: format!(
                        "migration at position {idx} has version {}, expected {expected} \
                         (versions must be contiguous from 1)",
                        migration.version
                    ),
                });
            }
        }
        let supported = self.supported_version();
        let mut previous = None;
        for &version in self.allowed_existing_versions {
            if version <= 0 || version > supported {
                return Err(SqliteStoreError::InvalidMigrationList {
                    domain: self.name.to_string(),
                    detail: format!(
                        "allowed existing version {version} is outside 1..={supported}"
                    ),
                });
            }
            if previous.is_some_and(|value| value >= version) {
                return Err(SqliteStoreError::InvalidMigrationList {
                    domain: self.name.to_string(),
                    detail: "allowed existing versions must be strictly increasing".to_string(),
                });
            }
            previous = Some(version);
        }
        if !self.allowed_existing_versions.contains(&supported) {
            return Err(SqliteStoreError::InvalidMigrationList {
                domain: self.name.to_string(),
                detail: format!(
                    "allowed existing versions must explicitly include current version {supported}"
                ),
            });
        }
        for &version in self
            .allowed_existing_versions
            .iter()
            .filter(|&&version| version < supported)
        {
            let matches = self
                .released_predecessors
                .iter()
                .filter(|predecessor| predecessor.version == version)
                .count();
            if matches != 1 {
                return Err(SqliteStoreError::InvalidMigrationList {
                    domain: self.name.to_string(),
                    detail: format!(
                        "allowed predecessor version {version} must have exactly one frozen \
                         verifier, found {matches}"
                    ),
                });
            }
        }
        for predecessor in self.released_predecessors {
            if predecessor.version >= supported
                || !self
                    .allowed_existing_versions
                    .contains(&predecessor.version)
            {
                return Err(SqliteStoreError::InvalidMigrationList {
                    domain: self.name.to_string(),
                    detail: format!(
                        "fingerprint verifier for version {} is not an allowed predecessor",
                        predecessor.version
                    ),
                });
            }
        }
        for (idx, object) in self
            .owned_objects
            .iter()
            .chain(self.retired_objects)
            .enumerate()
        {
            if object.name.is_empty() || object.name == "meerkat_schema" {
                return Err(SqliteStoreError::InvalidMigrationList {
                    domain: self.name.to_string(),
                    detail: format!(
                        "owned object at position {idx} has reserved or empty name `{}`",
                        object.name
                    ),
                });
            }
            if self
                .owned_objects
                .iter()
                .chain(self.retired_objects)
                .take(idx)
                .any(|prior| prior.name == object.name)
            {
                return Err(SqliteStoreError::InvalidMigrationList {
                    domain: self.name.to_string(),
                    detail: format!("owned object name `{}` is duplicated", object.name),
                });
            }
        }
        Ok(())
    }

    fn accepts_existing_version(&self, version: i64) -> bool {
        self.allowed_existing_versions.contains(&version)
    }

    fn verify_predecessor(&self, conn: &Connection, version: i64) -> Result<(), SqliteStoreError> {
        if version == self.supported_version() {
            return verify_current_schema_fingerprint(conn, self).map_err(|detail| {
                SqliteStoreError::SchemaFingerprintMismatch {
                    domain: self.name.to_string(),
                    version,
                    detail,
                }
            });
        }
        let predecessor = self
            .released_predecessors
            .iter()
            .find(|predecessor| predecessor.version == version)
            .ok_or_else(|| unsupported_predecessor(self, version))?;
        (predecessor.verify)(conn).map_err(|detail| SqliteStoreError::SchemaFingerprintMismatch {
            domain: self.name.to_string(),
            version,
            detail,
        })
    }
}

/// Outcome of [`apply_domain_migrations`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedgerReport {
    /// Version found before this call (0 = no ledger row).
    pub from_version: i64,
    /// Version after this call.
    pub to_version: i64,
}

impl LedgerReport {
    /// True when this call applied at least one migration.
    pub fn migrated(&self) -> bool {
        self.to_version > self.from_version
    }
}

/// Read a domain's ledger version without applying anything.
///
/// `Ok(None)` means the file has no ledger table or no row for the domain.
/// It says nothing about eligibility: an owning store separately proves that
/// the domain owns zero objects before treating it as fresh.
/// A ledger table that fails the pinned-shape or version validation yields
/// [`SqliteStoreError::LedgerMalformed`], never a healed reading.
pub fn domain_version(conn: &Connection, domain: &str) -> Result<Option<i64>, SqliteStoreError> {
    if !ledger_table_exists(conn)? {
        return Ok(None);
    }
    validate_ledger_shape(conn)?;
    read_version(conn, domain)
}

/// Establish read-only schema eligibility before a profile's mutating
/// pragmas: current and released predecessor rows must match their exact
/// catalog fingerprints; future, pre-floor, gap, and unledgered-owned shapes
/// are refused.
///
/// This is the [`crate::profile::OpenOptions::schema_preflight`] hook: the
/// Primary profile runs it before its mutating pragmas so an old binary
/// leaves an ineligible database's logical content unmodified. Reading the
/// ledger of a WAL-mode file over a read-write connection may still touch
/// its `-wal`/`-shm` sidecars
/// ([`crate::profile::WriteContact::ReadOnlyWalSidecars`]); the main
/// database file itself is not written. A missing row passes only when the
/// domain owns zero catalog objects; the pinned in-transaction re-check in
/// [`apply_domain_migrations`] remains the migration-time authority.
pub fn preflight_schema_eligibility(
    conn: &Connection,
    domain: &SchemaDomain,
) -> Result<(), SqliteStoreError> {
    domain.validate()?;
    let supported = domain.supported_version();
    match domain_version(conn, domain.name)? {
        Some(found) if found > supported => {
            return Err(SqliteStoreError::SchemaFromTheFuture {
                domain: domain.name.to_string(),
                found,
                supported,
            });
        }
        Some(found) if !domain.accepts_existing_version(found) => {
            return Err(unsupported_predecessor(domain, found));
        }
        Some(found) => domain.verify_predecessor(conn, found)?,
        None => {
            let objects = find_owned_objects(conn, domain)?;
            if !objects.is_empty() {
                return Err(SqliteStoreError::UnledgeredDomainObjects {
                    domain: domain.name.to_string(),
                    objects,
                });
            }
        }
    }
    Ok(())
}

/// Bring `domain` up to date in the file behind `conn`, per the pinned
/// protocol. Returns the version movement.
///
/// Eligibility, including the current-version no-op, is established under
/// one IMMEDIATE transaction. A future or unsupported version is refused
/// before any schema or ledger mutation.
pub fn apply_domain_migrations(
    conn: &mut Connection,
    domain: &SchemaDomain,
) -> Result<LedgerReport, SqliteStoreError> {
    domain.validate()?;
    let supported = domain.supported_version();

    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    // Establish eligibility inside the write transaction. No ledger or
    // domain DDL has run yet.
    let current = if ledger_table_exists(&tx)? {
        validate_ledger_shape(&tx)?;
        read_version(&tx, domain.name)?
    } else {
        None
    };
    if let Some(found) = current {
        if found > supported {
            return Err(SqliteStoreError::SchemaFromTheFuture {
                domain: domain.name.to_string(),
                found,
                supported,
            });
        }
        if !domain.accepts_existing_version(found) {
            return Err(unsupported_predecessor(domain, found));
        }
        domain.verify_predecessor(&tx, found)?;
    } else {
        let objects = find_owned_objects(&tx, domain)?;
        if !objects.is_empty() {
            return Err(SqliteStoreError::UnledgeredDomainObjects {
                domain: domain.name.to_string(),
                objects,
            });
        }
    }
    let current = current.unwrap_or(0);
    if current == supported {
        return Ok(LedgerReport {
            from_version: current,
            to_version: current,
        });
    }

    // Eligibility is now pinned by the IMMEDIATE transaction. Only now may
    // the runner materialize its ledger table.
    if !ledger_table_exists(&tx)? {
        tx.execute_batch(CREATE_LEDGER_SQL)?;
        validate_ledger_shape(&tx)?;
    }

    if current == 0 {
        tx.execute_batch(CUSTODY_SAVEPOINT_SQL)?;
        (domain.initialize_current)(&tx).map_err(|source| SqliteStoreError::MigrationFailed {
            domain: domain.name.to_string(),
            version: supported,
            name: "initialize-current".to_string(),
            source,
        })?;
        if tx.is_autocommit() || tx.execute_batch(CUSTODY_RELEASE_SQL).is_err() {
            return Err(SqliteStoreError::MigrationBrokeTransaction {
                domain: domain.name.to_string(),
                version: supported,
                name: "initialize-current".to_string(),
            });
        }
    } else {
        for migration in domain.migrations.iter().filter(|m| m.version > current) {
            tx.execute_batch(CUSTODY_SAVEPOINT_SQL)?;
            (migration.apply)(&tx).map_err(|source| SqliteStoreError::MigrationFailed {
                domain: domain.name.to_string(),
                version: migration.version,
                name: migration.name.to_string(),
                source,
            })?;
            // The `&Transaction` handed to the body cannot type-prevent COMMIT /
            // ROLLBACK statements, so custody is verified instead. Autocommit
            // going true is the cheap first line, but it misses a body that
            // ended the transaction and then re-BEGAN one; the savepoint is the
            // authority: RELEASE fails exactly when the savepoint no longer
            // exists, i.e. the body ended the runner's transaction (COMMIT and
            // ROLLBACK both discard it), whether or not it opened a new one.
            // Stamping the ledger inside such a foreign transaction would commit
            // separately from — or after rollback of — the schema work.
            if tx.is_autocommit() || tx.execute_batch(CUSTODY_RELEASE_SQL).is_err() {
                return Err(SqliteStoreError::MigrationBrokeTransaction {
                    domain: domain.name.to_string(),
                    version: migration.version,
                    name: migration.name.to_string(),
                });
            }
        }
    }
    verify_current_schema_fingerprint(&tx, domain).map_err(|detail| {
        SqliteStoreError::SchemaFingerprintMismatch {
            domain: domain.name.to_string(),
            version: supported,
            detail,
        }
    })?;
    tx.execute(
        "INSERT INTO main.meerkat_schema (domain, version) VALUES (?1, ?2)
         ON CONFLICT(domain) DO UPDATE SET version = excluded.version",
        rusqlite::params![domain.name, supported],
    )?;
    tx.commit()?;

    Ok(LedgerReport {
        from_version: current,
        to_version: supported,
    })
}

static EXPECTED_CURRENT_CATALOGS: OnceLock<Mutex<BTreeMap<String, Result<String, String>>>> =
    OnceLock::new();

/// Verify a current row against the exact catalog built by the current
/// initializer. The expected side is process-global pure code-derived state;
/// every actual connection is still read and bound independently.
fn verify_current_schema_fingerprint(
    actual: &Connection,
    domain: &SchemaDomain,
) -> Result<(), String> {
    let expected = {
        let cache = EXPECTED_CURRENT_CATALOGS.get_or_init(|| Mutex::new(BTreeMap::new()));
        let key = current_catalog_cache_key(domain);
        let cached = cache
            .lock()
            .map_err(|_| "current catalog cache lock is poisoned".to_string())?
            .get(&key)
            .cloned();
        if let Some(cached) = cached {
            cached?
        } else {
            let built = build_current_catalog_fingerprint(domain);
            cache
                .lock()
                .map_err(|_| "current catalog cache lock is poisoned".to_string())?
                .insert(key, built.clone());
            built?
        }
    };
    let actual = compact_catalog_fingerprint(actual, domain, domain.owned_objects)?;
    if actual != expected {
        return Err(format!(
            "current owned catalog differs: expected {expected}, found {actual}"
        ));
    }
    Ok(())
}

/// Bind the pure expected-catalog cache to the complete code-derived domain
/// identity. Name + version alone is insufficient: tests, embedders, or a
/// faulty registration can construct two manifests with the same persisted
/// identity but different initializer code or object ownership.
fn current_catalog_cache_key(domain: &SchemaDomain) -> String {
    let mut key = format!(
        "{}\u{1f}{}\u{1f}{:x}",
        domain.name,
        domain.supported_version(),
        domain.initialize_current as usize
    );
    for object in domain.owned_objects {
        key.push_str(&format!(
            "\u{1e}current:{}:{}",
            object.kind.sqlite_name(),
            object.name
        ));
    }
    for object in domain.retired_objects {
        key.push_str(&format!(
            "\u{1e}retired:{}:{}",
            object.kind.sqlite_name(),
            object.name
        ));
    }
    key
}

fn build_current_catalog_fingerprint(domain: &SchemaDomain) -> Result<String, String> {
    let mut expected =
        Connection::open_in_memory().map_err(|error| format!("open current oracle: {error}"))?;
    let tx = expected
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|error| format!("begin current oracle: {error}"))?;
    (domain.initialize_current)(&tx).map_err(|error| format!("build current oracle: {error}"))?;
    tx.commit()
        .map_err(|error| format!("commit current oracle: {error}"))?;
    compact_catalog_fingerprint(&expected, domain, domain.owned_objects)
}

fn compact_catalog_fingerprint(
    conn: &Connection,
    domain: &SchemaDomain,
    expected_objects: &[SchemaObject],
) -> Result<String, String> {
    let all_objects = all_domain_objects(domain);
    let owned_by_name = all_objects
        .iter()
        .map(|object| (object.name, object))
        .collect::<BTreeMap<_, _>>();
    let current_by_name = expected_objects
        .iter()
        .map(|object| (object.name, object))
        .collect::<BTreeMap<_, _>>();
    let mut actual_names = Vec::new();
    let mut entries = Vec::with_capacity(expected_objects.len());
    let mut statement = conn
        .prepare(
            "SELECT type, name, tbl_name, sql
             FROM main.sqlite_schema
             WHERE name NOT LIKE 'sqlite_%'
             ORDER BY type, name",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })
        .map_err(|error| error.to_string())?;
    for row in rows {
        let (kind, name, table_name, sql) = row.map_err(|error| error.to_string())?;
        if owned_by_name.contains_key(name.as_str()) {
            actual_names.push((kind.clone(), name.clone()));
        }
        if current_by_name.contains_key(name.as_str()) {
            entries.push(format!(
                "{kind}\u{1f}{name}\u{1f}{table_name}\u{1f}{}",
                sql.map(|sql| normalize_schema_sql(&sql))
                    .unwrap_or_default()
            ));
        }
    }
    actual_names.sort();
    let mut expected_names = expected_objects
        .iter()
        .map(|object| {
            (
                object.kind.sqlite_name().to_string(),
                object.name.to_string(),
            )
        })
        .collect::<Vec<_>>();
    expected_names.sort();
    if actual_names != expected_names {
        return Err(format!(
            "owned object set differs: expected {expected_names:?}, found {actual_names:?}"
        ));
    }

    entries.sort();
    Ok(entries.join("\u{1e}"))
}

fn all_domain_objects(domain: &SchemaDomain) -> Vec<SchemaObject> {
    domain
        .owned_objects
        .iter()
        .chain(domain.retired_objects)
        .copied()
        .collect()
}

/// Verify an on-disk predecessor against a frozen released schema builder.
///
/// The builder is run only in a private in-memory database. The comparison is
/// structured over `main.sqlite_schema`: exact owned object names/kinds,
/// normalized CREATE SQL, table xinfo and foreign keys, plus explicit-index
/// uniqueness/partial flags and xinfo. Foreign co-tenant objects are ignored.
///
/// Store crates use this from a [`SchemaPredecessor`] verifier, passing DDL
/// copied from the released tag rather than current initializer constants.
pub fn verify_released_schema_fingerprint(
    actual: &Connection,
    domain: &SchemaDomain,
    released_objects: &[SchemaObject],
    build_released: fn(&Transaction<'_>) -> Result<(), rusqlite::Error>,
) -> Result<(), String> {
    let mut expected = Connection::open_in_memory()
        .map_err(|error| format!("open fingerprint oracle: {error}"))?;
    let tx = expected
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|error| format!("begin fingerprint oracle: {error}"))?;
    build_released(&tx).map_err(|error| format!("build fingerprint oracle: {error}"))?;
    tx.commit()
        .map_err(|error| format!("commit fingerprint oracle: {error}"))?;

    let expected_names = catalog_names(&expected, released_objects)
        .map_err(|error| format!("read fingerprint oracle: {error}"))?;
    let mut declared_expected = released_objects
        .iter()
        .map(|object| {
            (
                object.kind.sqlite_name().to_string(),
                object.name.to_string(),
            )
        })
        .collect::<Vec<_>>();
    declared_expected.sort();
    if expected_names != declared_expected {
        return Err(format!(
            "frozen builder produced {expected_names:?}, manifest declares {declared_expected:?}"
        ));
    }

    let actual_names = catalog_names(actual, &all_domain_objects(domain))
        .map_err(|error| format!("read actual catalog: {error}"))?;
    if actual_names != declared_expected {
        return Err(format!(
            "owned object set differs: expected {declared_expected:?}, found {actual_names:?}"
        ));
    }

    for object in released_objects {
        let wanted = catalog_fingerprint(&expected, object)
            .map_err(|error| format!("fingerprint oracle {}: {error}", object.name))?;
        let found = catalog_fingerprint(actual, object)
            .map_err(|error| format!("fingerprint actual {}: {error}", object.name))?;
        if found != wanted {
            return Err(format!(
                "object `{}` differs: expected {wanted:?}, found {found:?}",
                object.name
            ));
        }
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
struct CatalogObjectFingerprint {
    kind: String,
    name: String,
    table_name: String,
    normalized_sql: Option<String>,
    table_columns: Vec<TableColumnFingerprint>,
    foreign_keys: Vec<ForeignKeyFingerprint>,
    index: Option<IndexFingerprint>,
}

#[derive(Debug, PartialEq, Eq)]
struct TableColumnFingerprint {
    cid: i64,
    name: String,
    declared_type: String,
    not_null: bool,
    default_value: Option<String>,
    primary_key_position: i64,
    hidden: i64,
}

#[derive(Debug, PartialEq, Eq)]
struct ForeignKeyFingerprint {
    id: i64,
    sequence: i64,
    target_table: String,
    from_column: String,
    to_column: Option<String>,
    on_update: String,
    on_delete: String,
    match_clause: String,
}

#[derive(Debug, PartialEq, Eq)]
struct IndexFingerprint {
    unique: bool,
    origin: String,
    partial: bool,
    columns: Vec<IndexColumnFingerprint>,
}

#[derive(Debug, PartialEq, Eq)]
struct IndexColumnFingerprint {
    sequence: i64,
    column_id: i64,
    name: Option<String>,
    descending: bool,
    collation: Option<String>,
    key: bool,
}

fn catalog_names(
    conn: &Connection,
    objects: &[SchemaObject],
) -> Result<Vec<(String, String)>, rusqlite::Error> {
    let mut found = Vec::new();
    let mut statement = conn.prepare(
        "SELECT type, name FROM main.sqlite_schema
         WHERE name = ?1 AND name NOT LIKE 'sqlite_%'
         ORDER BY type, name",
    )?;
    for object in objects {
        let rows = statement.query_map([object.name], |row| Ok((row.get(0)?, row.get(1)?)))?;
        found.extend(rows.collect::<Result<Vec<_>, _>>()?);
    }
    found.sort();
    found.dedup();
    Ok(found)
}

fn catalog_fingerprint(
    conn: &Connection,
    object: &SchemaObject,
) -> Result<CatalogObjectFingerprint, rusqlite::Error> {
    let (kind, name, table_name, sql): (String, String, String, Option<String>) = conn.query_row(
        "SELECT type, name, tbl_name, sql FROM main.sqlite_schema WHERE name = ?1",
        [object.name],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    let (table_columns, foreign_keys) = if kind == "table" || kind == "view" {
        (
            table_columns(conn, object.name)?,
            foreign_keys(conn, object.name)?,
        )
    } else {
        (Vec::new(), Vec::new())
    };
    let index = if kind == "index" {
        Some(index_fingerprint(conn, &table_name, object.name)?)
    } else {
        None
    };
    Ok(CatalogObjectFingerprint {
        kind,
        name,
        table_name,
        normalized_sql: sql.map(|sql| normalize_schema_sql(&sql)),
        table_columns,
        foreign_keys,
        index,
    })
}

fn table_columns(
    conn: &Connection,
    table: &str,
) -> Result<Vec<TableColumnFingerprint>, rusqlite::Error> {
    let mut statement = conn.prepare(
        "SELECT cid, name, type, \"notnull\", dflt_value, pk, hidden
         FROM pragma_table_xinfo(?1, 'main')
         ORDER BY cid",
    )?;
    let rows = statement.query_map([table], |row| {
        Ok(TableColumnFingerprint {
            cid: row.get(0)?,
            name: row.get(1)?,
            declared_type: row.get(2)?,
            not_null: row.get(3)?,
            default_value: row.get(4)?,
            primary_key_position: row.get(5)?,
            hidden: row.get(6)?,
        })
    })?;
    rows.collect()
}

fn foreign_keys(
    conn: &Connection,
    table: &str,
) -> Result<Vec<ForeignKeyFingerprint>, rusqlite::Error> {
    let mut statement = conn.prepare(
        "SELECT id, seq, \"table\", \"from\", \"to\", on_update, on_delete, \"match\"
         FROM pragma_foreign_key_list(?1, 'main')
         ORDER BY id, seq",
    )?;
    let rows = statement.query_map([table], |row| {
        Ok(ForeignKeyFingerprint {
            id: row.get(0)?,
            sequence: row.get(1)?,
            target_table: row.get(2)?,
            from_column: row.get(3)?,
            to_column: row.get(4)?,
            on_update: row.get(5)?,
            on_delete: row.get(6)?,
            match_clause: row.get(7)?,
        })
    })?;
    rows.collect()
}

fn index_fingerprint(
    conn: &Connection,
    table: &str,
    index: &str,
) -> Result<IndexFingerprint, rusqlite::Error> {
    let (unique, origin, partial): (bool, String, bool) = conn.query_row(
        "SELECT \"unique\", origin, partial
         FROM pragma_index_list(?1, 'main')
         WHERE name = ?2",
        rusqlite::params![table, index],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let mut statement = conn.prepare(
        "SELECT seqno, cid, name, desc, coll, key
         FROM pragma_index_xinfo(?1, 'main')
         ORDER BY seqno",
    )?;
    let columns = statement
        .query_map([index], |row| {
            Ok(IndexColumnFingerprint {
                sequence: row.get(0)?,
                column_id: row.get(1)?,
                name: row.get(2)?,
                descending: row.get(3)?,
                collation: row.get(4)?,
                key: row.get(5)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(IndexFingerprint {
        unique,
        origin,
        partial,
        columns,
    })
}

fn normalize_schema_sql(sql: &str) -> String {
    sql.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace(" IF NOT EXISTS ", " ")
}

fn unsupported_predecessor(domain: &SchemaDomain, found: i64) -> SqliteStoreError {
    SqliteStoreError::UnsupportedSchemaPredecessor {
        domain: domain.name.to_string(),
        found,
        supported: domain.supported_version(),
        allowed: domain.allowed_existing_versions.to_vec(),
    }
}

/// Return owned catalog names already present in `main`.
///
/// A wrong-kind collision is included (and annotated with its actual kind)
/// because object names themselves are the ownership boundary.
fn find_owned_objects(
    conn: &Connection,
    domain: &SchemaDomain,
) -> Result<Vec<String>, SqliteStoreError> {
    let mut found = Vec::new();
    let mut stmt = conn.prepare(
        "SELECT type, name FROM main.sqlite_schema
         WHERE name = ?1 AND name NOT LIKE 'sqlite_%'
         ORDER BY type, name",
    )?;
    for expected in domain.owned_objects.iter().chain(domain.retired_objects) {
        let rows = stmt.query_map([expected.name], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (actual_kind, name) = row?;
            found.push(format!(
                "{actual_kind}:{name} (expected {})",
                expected.kind.sqlite_name()
            ));
        }
    }
    found.sort();
    found.dedup();
    Ok(found)
}

fn ledger_table_exists(conn: &Connection) -> Result<bool, SqliteStoreError> {
    let exists = conn
        .query_row(
            "SELECT 1 FROM main.sqlite_master WHERE type = 'table' AND name = 'meerkat_schema'",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    Ok(exists)
}

fn malformed(detail: String) -> SqliteStoreError {
    SqliteStoreError::LedgerMalformed { detail }
}

/// Validate the pinned ledger shape against `main`'s real catalog.
///
/// `domain` must be `TEXT` and the *sole* primary-key column (a composite
/// key would permit multiple rows per domain); `version` must be
/// `INTEGER NOT NULL`. Columns beyond the pinned pair are tolerated as long
/// as they carry no primary-key position, so a future ledger protocol can
/// extend the table compatibly. The schema-qualified `pragma_table_info`
/// resolves in `main`, so a TEMP shadow cannot satisfy this check.
fn validate_ledger_shape(conn: &Connection) -> Result<(), SqliteStoreError> {
    let mut stmt = conn.prepare(
        "SELECT name, type, \"notnull\", pk FROM pragma_table_info('meerkat_schema', 'main')",
    )?;
    let mut rows = stmt.query([])?;
    let mut domain_ok = false;
    let mut version_ok = false;
    while let Some(row) = rows.next()? {
        let name: String = row.get(0)?;
        let decl_type: String = row.get(1)?;
        let notnull: bool = row.get(2)?;
        let pk: i64 = row.get(3)?;
        match name.as_str() {
            "domain" => {
                if !decl_type.eq_ignore_ascii_case("TEXT") || pk != 1 {
                    return Err(malformed(format!(
                        "column `domain` must be `TEXT PRIMARY KEY`, found type `{decl_type}` \
                         with pk position {pk}"
                    )));
                }
                domain_ok = true;
            }
            "version" => {
                if !decl_type.eq_ignore_ascii_case("INTEGER") || !notnull || pk != 0 {
                    return Err(malformed(format!(
                        "column `version` must be non-key `INTEGER NOT NULL`, found type \
                         `{decl_type}` notnull={notnull} pk position {pk}"
                    )));
                }
                version_ok = true;
            }
            other => {
                if pk != 0 {
                    return Err(malformed(format!(
                        "unexpected primary-key column `{other}`"
                    )));
                }
            }
        }
    }
    if !domain_ok || !version_ok {
        return Err(malformed(
            "table lacks the pinned `domain`/`version` columns".to_string(),
        ));
    }
    Ok(())
}

/// Read one domain's version, refusing corrupt ledger state typed: more than
/// one row per domain (impossible under the validated single-column primary
/// key; kept as defense in depth) and non-positive versions (0 is the
/// implicit "no row" reading and negatives are meaningless — a stored
/// non-positive version is damage to refuse, not an old schema to re-migrate
/// over).
fn read_version(conn: &Connection, domain: &str) -> Result<Option<i64>, SqliteStoreError> {
    let mut stmt = conn.prepare("SELECT version FROM main.meerkat_schema WHERE domain = ?1")?;
    let mut rows = stmt.query([domain])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let version: i64 = row.get(0)?;
    if rows.next()?.is_some() {
        return Err(malformed(format!(
            "multiple ledger rows for domain `{domain}`"
        )));
    }
    if version <= 0 {
        return Err(malformed(format!(
            "domain `{domain}` records non-positive version {version}"
        )));
    }
    Ok(Some(version))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::profile::{ConnectionProfile, open};

    fn create_t1(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
        tx.execute_batch("CREATE TABLE IF NOT EXISTS t1 (x INTEGER)")
    }

    fn add_column_guarded(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
        let has_column = tx
            .prepare("PRAGMA table_info(t1)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?
            .iter()
            .any(|name| name == "y");
        if !has_column {
            tx.execute_batch("ALTER TABLE t1 ADD COLUMN y TEXT")?;
        }
        Ok(())
    }

    fn initialize_v2(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
        create_t1(tx)?;
        add_column_guarded(tx)
    }

    fn initialize_v2_alt(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
        tx.execute_batch("CREATE TABLE t1 (x INTEGER, z BLOB)")
    }

    const RELEASED_V1_OBJECTS: &[SchemaObject] = &[SchemaObject {
        kind: SchemaObjectKind::Table,
        name: "t1",
    }];

    fn verify_v1(conn: &Connection) -> Result<(), String> {
        verify_released_schema_fingerprint(conn, &DOMAIN_V2, RELEASED_V1_OBJECTS, create_t1)
    }

    const DOMAIN_V1: SchemaDomain = SchemaDomain {
        name: "test-domain",
        migrations: &[Migration {
            version: 1,
            name: "base",
            apply: create_t1,
        }],
        initialize_current: create_t1,
        allowed_existing_versions: &[1],
        released_predecessors: &[],
        owned_objects: &[SchemaObject {
            kind: SchemaObjectKind::Table,
            name: "t1",
        }],
        retired_objects: &[],
    };

    const DOMAIN_V2: SchemaDomain = SchemaDomain {
        name: "test-domain",
        migrations: &[
            Migration {
                version: 1,
                name: "base",
                apply: create_t1,
            },
            Migration {
                version: 2,
                name: "add-y",
                apply: add_column_guarded,
            },
        ],
        initialize_current: initialize_v2,
        allowed_existing_versions: &[1, 2],
        released_predecessors: &[SchemaPredecessor {
            version: 1,
            verify: verify_v1,
        }],
        owned_objects: &[SchemaObject {
            kind: SchemaObjectKind::Table,
            name: "t1",
        }],
        retired_objects: &[],
    };

    const DOMAIN_V2_ALT_INITIALIZER: SchemaDomain = SchemaDomain {
        name: "test-domain",
        migrations: &[
            Migration {
                version: 1,
                name: "base",
                apply: create_t1,
            },
            Migration {
                version: 2,
                name: "alt-current",
                apply: add_column_guarded,
            },
        ],
        initialize_current: initialize_v2_alt,
        allowed_existing_versions: &[2],
        released_predecessors: &[],
        owned_objects: &[SchemaObject {
            kind: SchemaObjectKind::Table,
            name: "t1",
        }],
        retired_objects: &[],
    };

    fn temp_conn(dir: &tempfile::TempDir) -> Connection {
        open(&dir.path().join("db.sqlite3"), ConnectionProfile::PRIMARY).expect("open")
    }

    #[test]
    fn fresh_file_initializes_current_and_stamps() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut conn = temp_conn(&dir);
        let report = apply_domain_migrations(&mut conn, &DOMAIN_V2).expect("apply");
        assert_eq!(
            report,
            LedgerReport {
                from_version: 0,
                to_version: 2
            }
        );
        assert_eq!(domain_version(&conn, "test-domain").expect("read"), Some(2));
        conn.execute("INSERT INTO t1 (x, y) VALUES (1, 'a')", [])
            .expect("schema converged");
    }

    #[test]
    fn second_open_is_current_noop() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut conn = temp_conn(&dir);
        apply_domain_migrations(&mut conn, &DOMAIN_V2).expect("first");
        let report = apply_domain_migrations(&mut conn, &DOMAIN_V2).expect("second");
        assert!(!report.migrated());
    }

    #[test]
    fn current_oracle_cache_binds_initializer_and_manifest_not_only_name_version() {
        let first_dir = tempfile::tempdir().expect("first tempdir");
        let mut first = temp_conn(&first_dir);
        apply_domain_migrations(&mut first, &DOMAIN_V2).expect("first current");

        let second_dir = tempfile::tempdir().expect("second tempdir");
        let mut second = temp_conn(&second_dir);
        apply_domain_migrations(&mut second, &DOMAIN_V2_ALT_INITIALIZER).expect("alt current");
        let columns: Vec<String> = second
            .prepare("PRAGMA table_info(t1)")
            .expect("prepare")
            .query_map([], |row| row.get(1))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("columns");
        assert_eq!(columns, vec!["x", "z"]);
    }

    #[test]
    fn current_row_with_partial_catalog_is_refused_before_noop() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut conn = temp_conn(&dir);
        apply_domain_migrations(&mut conn, &DOMAIN_V2).expect("current");
        conn.execute_batch("ALTER TABLE t1 ADD COLUMN candidate_partial TEXT")
            .expect("partial candidate mutation");
        let err = apply_domain_migrations(&mut conn, &DOMAIN_V2).expect_err("refuse current shape");
        assert!(matches!(
            err,
            SqliteStoreError::SchemaFingerprintMismatch { version: 2, .. }
        ));
        assert_eq!(
            domain_version(&conn, DOMAIN_V2.name).expect("ledger"),
            Some(2)
        );
    }

    #[test]
    fn upgrade_applies_only_pending() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut conn = temp_conn(&dir);
        apply_domain_migrations(&mut conn, &DOMAIN_V1).expect("v1");
        let report = apply_domain_migrations(&mut conn, &DOMAIN_V2).expect("v2");
        assert_eq!(
            report,
            LedgerReport {
                from_version: 1,
                to_version: 2
            }
        );
    }

    #[test]
    fn allowed_version_with_wrong_catalog_is_refused_without_migration() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut conn = temp_conn(&dir);
        apply_domain_migrations(&mut conn, &DOMAIN_V1).expect("v1");
        conn.execute_batch("ALTER TABLE t1 ADD COLUMN candidate_only TEXT")
            .expect("candidate shape");
        let err = apply_domain_migrations(&mut conn, &DOMAIN_V2).expect_err("refuse fingerprint");
        assert!(matches!(
            err,
            SqliteStoreError::SchemaFingerprintMismatch { version: 1, .. }
        ));
        assert_eq!(
            domain_version(&conn, DOMAIN_V1.name).expect("ledger"),
            Some(1),
            "fingerprint refusal advanced the ledger"
        );
        let columns: Vec<String> = conn
            .prepare("PRAGMA table_info(t1)")
            .expect("prepare")
            .query_map([], |row| row.get(1))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("columns");
        assert_eq!(columns, vec!["x", "candidate_only"]);
    }

    #[test]
    fn pre_floor_and_gap_versions_are_refused_without_mutation() {
        fn no_op(_tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
            Ok(())
        }
        fn verify_v2(_conn: &Connection) -> Result<(), String> {
            Ok(())
        }
        const DOMAIN_V3_FLOOR_2: SchemaDomain = SchemaDomain {
            name: "floor-domain",
            migrations: &[
                Migration {
                    version: 1,
                    name: "base",
                    apply: no_op,
                },
                Migration {
                    version: 2,
                    name: "released-floor",
                    apply: no_op,
                },
                Migration {
                    version: 3,
                    name: "current",
                    apply: no_op,
                },
            ],
            initialize_current: no_op,
            allowed_existing_versions: &[2, 3],
            released_predecessors: &[SchemaPredecessor {
                version: 2,
                verify: verify_v2,
            }],
            owned_objects: &[],
            retired_objects: &[],
        };
        const DOMAIN_V4_GAP_3: SchemaDomain = SchemaDomain {
            name: "gap-domain",
            migrations: &[
                Migration {
                    version: 1,
                    name: "old",
                    apply: no_op,
                },
                Migration {
                    version: 2,
                    name: "released-floor",
                    apply: no_op,
                },
                Migration {
                    version: 3,
                    name: "unreleased-candidate",
                    apply: no_op,
                },
                Migration {
                    version: 4,
                    name: "current",
                    apply: no_op,
                },
            ],
            initialize_current: no_op,
            allowed_existing_versions: &[2, 4],
            released_predecessors: &[SchemaPredecessor {
                version: 2,
                verify: verify_v2,
            }],
            owned_objects: &[],
            retired_objects: &[],
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let mut conn = temp_conn(&dir);
        conn.execute_batch(CREATE_LEDGER_SQL).expect("ledger");
        conn.execute(
            "INSERT INTO main.meerkat_schema (domain, version) VALUES (?1, 1)",
            [DOMAIN_V3_FLOOR_2.name],
        )
        .expect("pre-floor row");
        let err =
            apply_domain_migrations(&mut conn, &DOMAIN_V3_FLOOR_2).expect_err("refuse pre-floor");
        assert!(matches!(
            err,
            SqliteStoreError::UnsupportedSchemaPredecessor { found: 1, .. }
        ));
        assert_eq!(
            domain_version(&conn, DOMAIN_V3_FLOOR_2.name).expect("ledger"),
            Some(1)
        );

        conn.execute(
            "INSERT INTO main.meerkat_schema (domain, version) VALUES (?1, 3)",
            [DOMAIN_V4_GAP_3.name],
        )
        .expect("gap row");
        let err = apply_domain_migrations(&mut conn, &DOMAIN_V4_GAP_3).expect_err("refuse gap");
        assert!(matches!(
            err,
            SqliteStoreError::UnsupportedSchemaPredecessor { found: 3, .. }
        ));
        assert_eq!(
            domain_version(&conn, DOMAIN_V4_GAP_3.name).expect("ledger"),
            Some(3)
        );
    }

    #[test]
    fn unledgered_owned_objects_are_refused_without_mutation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut conn = temp_conn(&dir);
        conn.execute_batch("CREATE TABLE t1 (x INTEGER)")
            .expect("unknown unledgered ddl");
        let err = apply_domain_migrations(&mut conn, &DOMAIN_V2).expect_err("refuse");
        assert!(matches!(
            err,
            SqliteStoreError::UnledgeredDomainObjects { .. }
        ));
        assert!(
            !ledger_table_exists(&conn).expect("ledger presence"),
            "eligibility refusal must not create the ledger"
        );
        let columns: Vec<String> = conn
            .prepare("PRAGMA table_info(t1)")
            .expect("prepare")
            .query_map([], |row| row.get(1))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("columns");
        assert_eq!(columns, vec!["x"], "refusal mutated unknown schema");
    }

    #[test]
    fn fresh_domain_ignores_foreign_cotenant_objects() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut conn = temp_conn(&dir);
        conn.execute_batch("CREATE TABLE foreign_table (value TEXT)")
            .expect("foreign ddl");
        let report = apply_domain_migrations(&mut conn, &DOMAIN_V2).expect("fresh domain");
        assert_eq!(
            report,
            LedgerReport {
                from_version: 0,
                to_version: 2
            }
        );
        conn.execute("INSERT INTO foreign_table VALUES ('kept')", [])
            .expect("foreign object survives");
    }

    #[test]
    fn future_version_is_refused_before_any_mutation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut conn = temp_conn(&dir);
        apply_domain_migrations(&mut conn, &DOMAIN_V2).expect("stamp v2");
        // An older binary knows only v1.
        let err = apply_domain_migrations(&mut conn, &DOMAIN_V1).expect_err("refuse");
        match err {
            SqliteStoreError::SchemaFromTheFuture {
                domain,
                found,
                supported,
            } => {
                assert_eq!(domain, "test-domain");
                assert_eq!(found, 2);
                assert_eq!(supported, 1);
            }
            other => panic!("wrong error: {other}"),
        }
        // Nothing moved.
        assert_eq!(domain_version(&conn, "test-domain").expect("read"), Some(2));
    }

    #[test]
    fn foreign_domain_rows_are_untouched() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut conn = temp_conn(&dir);
        apply_domain_migrations(&mut conn, &DOMAIN_V2).expect("mine");
        conn.execute(
            "INSERT INTO meerkat_schema (domain, version) VALUES ('foreign-domain', 7)",
            [],
        )
        .expect("foreign row");
        apply_domain_migrations(&mut conn, &DOMAIN_V2).expect("noop");
        let foreign: i64 = conn
            .query_row(
                "SELECT version FROM meerkat_schema WHERE domain = 'foreign-domain'",
                [],
                |r| r.get(0),
            )
            .expect("foreign row survives");
        assert_eq!(foreign, 7);
    }

    #[test]
    fn invalid_migration_list_is_refused_without_touching_the_file() {
        const BAD: SchemaDomain = SchemaDomain {
            name: "bad-domain",
            migrations: &[Migration {
                version: 3,
                name: "gap",
                apply: create_t1,
            }],
            initialize_current: create_t1,
            allowed_existing_versions: &[3],
            released_predecessors: &[],
            owned_objects: &[],
            retired_objects: &[],
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let mut conn = temp_conn(&dir);
        let err = apply_domain_migrations(&mut conn, &BAD).expect_err("refuse");
        assert!(matches!(err, SqliteStoreError::InvalidMigrationList { .. }));
        assert!(!ledger_table_exists(&conn).expect("check"));
    }

    #[test]
    fn failed_migration_rolls_back_atomically() {
        fn fail(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
            tx.execute_batch("CREATE TABLE half_done (x INTEGER)")?;
            tx.execute_batch("THIS IS NOT SQL")
        }
        fn initialize_failing(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
            create_t1(tx)?;
            fail(tx)
        }
        const FAILING: SchemaDomain = SchemaDomain {
            name: "failing-domain",
            migrations: &[
                Migration {
                    version: 1,
                    name: "base",
                    apply: create_t1,
                },
                Migration {
                    version: 2,
                    name: "explodes",
                    apply: fail,
                },
            ],
            initialize_current: initialize_failing,
            allowed_existing_versions: &[1, 2],
            released_predecessors: &[SchemaPredecessor {
                version: 1,
                verify: verify_v1,
            }],
            owned_objects: &[
                SchemaObject {
                    kind: SchemaObjectKind::Table,
                    name: "t1",
                },
                SchemaObject {
                    kind: SchemaObjectKind::Table,
                    name: "half_done",
                },
            ],
            retired_objects: &[],
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let mut conn = temp_conn(&dir);
        let err = apply_domain_migrations(&mut conn, &FAILING).expect_err("must fail");
        assert!(matches!(
            err,
            SqliteStoreError::MigrationFailed { version: 2, .. }
        ));
        // Atomic: neither the v1 table, the half-done table, nor a ledger row
        // survives.
        assert_eq!(domain_version(&conn, "failing-domain").expect("read"), None);
        let tables: Vec<String> = conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type='table' AND name IN ('t1','half_done')",
            )
            .expect("prepare")
            .query_map([], |r| r.get(0))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows");
        assert!(tables.is_empty(), "rollback left tables behind: {tables:?}");
    }

    #[test]
    fn malformed_ledger_shape_is_refused_not_healed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut conn = temp_conn(&dir);
        // A foreign table wearing the ledger's name.
        conn.execute_batch("CREATE TABLE meerkat_schema (x INTEGER)")
            .expect("foreign ddl");
        let err = domain_version(&conn, "test-domain").expect_err("refuse read");
        assert!(matches!(err, SqliteStoreError::LedgerMalformed { .. }));
        let err = apply_domain_migrations(&mut conn, &DOMAIN_V1).expect_err("refuse migrate");
        assert!(matches!(err, SqliteStoreError::LedgerMalformed { .. }));
        // Refused, not healed: the foreign table is untouched and unstamped.
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM meerkat_schema", [], |r| r.get(0))
            .expect("foreign table survives");
        assert_eq!(count, 0);
    }

    #[test]
    fn non_positive_versions_are_refused_not_healed() {
        for bad_version in [0i64, -3] {
            let dir = tempfile::tempdir().expect("tempdir");
            let mut conn = temp_conn(&dir);
            conn.execute_batch(
                "CREATE TABLE meerkat_schema (domain TEXT PRIMARY KEY, version INTEGER NOT NULL)",
            )
            .expect("ledger ddl");
            conn.execute(
                "INSERT INTO meerkat_schema (domain, version) VALUES ('test-domain', ?1)",
                [bad_version],
            )
            .expect("seed bad version");
            let err = domain_version(&conn, "test-domain").expect_err("refuse read");
            assert!(matches!(err, SqliteStoreError::LedgerMalformed { .. }));
            let err = apply_domain_migrations(&mut conn, &DOMAIN_V1).expect_err("refuse migrate");
            assert!(matches!(err, SqliteStoreError::LedgerMalformed { .. }));
            // The bad row must survive untouched for forensics.
            let stored: i64 = conn
                .query_row(
                    "SELECT version FROM meerkat_schema WHERE domain = 'test-domain'",
                    [],
                    |r| r.get(0),
                )
                .expect("row survives");
            assert_eq!(stored, bad_version);
        }
    }

    #[test]
    fn duplicate_domain_rows_are_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = temp_conn(&dir);
        // No primary key: shape validation would already refuse this table;
        // the row-cardinality guard is exercised directly as defense in
        // depth.
        conn.execute_batch(
            "CREATE TABLE meerkat_schema (domain TEXT, version INTEGER NOT NULL);
             INSERT INTO meerkat_schema VALUES ('dup-domain', 1);
             INSERT INTO meerkat_schema VALUES ('dup-domain', 2);",
        )
        .expect("seed duplicates");
        let err = read_version(&conn, "dup-domain").expect_err("refuse duplicates");
        match err {
            SqliteStoreError::LedgerMalformed { detail } => {
                assert!(detail.contains("multiple ledger rows"), "{detail}");
            }
            other => panic!("wrong error: {other}"),
        }
    }

    #[test]
    fn temp_shadowing_cannot_hijack_the_ledger() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut conn = temp_conn(&dir);
        apply_domain_migrations(&mut conn, &DOMAIN_V1).expect("stamp v1");
        // A TEMP shadow claiming a future version: unqualified reads would
        // see 999 and refuse; the main-qualified ledger keeps reading truth.
        conn.execute_batch(
            "CREATE TEMP TABLE meerkat_schema (domain TEXT PRIMARY KEY, version INTEGER NOT NULL);
             INSERT INTO temp.meerkat_schema VALUES ('test-domain', 999);",
        )
        .expect("temp shadow");
        assert_eq!(domain_version(&conn, "test-domain").expect("read"), Some(1));
        let report = apply_domain_migrations(&mut conn, &DOMAIN_V1).expect("noop against main");
        assert!(!report.migrated());
    }

    #[test]
    fn migration_that_ends_the_transaction_is_refused_unstamped() {
        fn commits_underneath(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
            tx.execute_batch("CREATE TABLE escaped_commit (x INTEGER); COMMIT")
        }
        fn rolls_back_underneath(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
            tx.execute_batch("ROLLBACK")
        }
        // The re-BEGIN variants leave autocommit false at the custody check:
        // only the savepoint detects that the runner's transaction is gone
        // and the ledger stamp would land in a foreign one.
        fn commits_then_begins(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
            tx.execute_batch("CREATE TABLE escaped_commit_begin (x INTEGER); COMMIT; BEGIN")
        }
        fn rolls_back_then_begins(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
            tx.execute_batch("ROLLBACK; BEGIN")
        }
        const COMMITS: SchemaDomain = SchemaDomain {
            name: "custody-commit",
            migrations: &[Migration {
                version: 1,
                name: "commits-underneath",
                apply: commits_underneath,
            }],
            initialize_current: commits_underneath,
            allowed_existing_versions: &[1],
            released_predecessors: &[],
            owned_objects: &[SchemaObject {
                kind: SchemaObjectKind::Table,
                name: "escaped_commit",
            }],
            retired_objects: &[],
        };
        const ROLLS_BACK: SchemaDomain = SchemaDomain {
            name: "custody-rollback",
            migrations: &[Migration {
                version: 1,
                name: "rolls-back-underneath",
                apply: rolls_back_underneath,
            }],
            initialize_current: rolls_back_underneath,
            allowed_existing_versions: &[1],
            released_predecessors: &[],
            owned_objects: &[],
            retired_objects: &[],
        };
        const COMMITS_THEN_BEGINS: SchemaDomain = SchemaDomain {
            name: "custody-commit-begin",
            migrations: &[Migration {
                version: 1,
                name: "commits-then-begins",
                apply: commits_then_begins,
            }],
            initialize_current: commits_then_begins,
            allowed_existing_versions: &[1],
            released_predecessors: &[],
            owned_objects: &[SchemaObject {
                kind: SchemaObjectKind::Table,
                name: "escaped_commit_begin",
            }],
            retired_objects: &[],
        };
        const ROLLS_BACK_THEN_BEGINS: SchemaDomain = SchemaDomain {
            name: "custody-rollback-begin",
            migrations: &[Migration {
                version: 1,
                name: "rolls-back-then-begins",
                apply: rolls_back_then_begins,
            }],
            initialize_current: rolls_back_then_begins,
            allowed_existing_versions: &[1],
            released_predecessors: &[],
            owned_objects: &[],
            retired_objects: &[],
        };
        for (domain, expected_name) in [
            (&COMMITS, "commits-underneath"),
            (&ROLLS_BACK, "rolls-back-underneath"),
            (&COMMITS_THEN_BEGINS, "commits-then-begins"),
            (&ROLLS_BACK_THEN_BEGINS, "rolls-back-then-begins"),
        ] {
            let dir = tempfile::tempdir().expect("tempdir");
            let mut conn = temp_conn(&dir);
            let err = apply_domain_migrations(&mut conn, domain).expect_err("custody violation");
            match err {
                SqliteStoreError::MigrationBrokeTransaction {
                    domain: err_domain,
                    version,
                    name,
                } => {
                    assert_eq!(err_domain, domain.name);
                    assert_eq!(version, 1);
                    assert_eq!(name, expected_name);
                }
                other => panic!("wrong error: {other}"),
            }
            // The stamp never landed: custody broke before the ledger write.
            assert_eq!(domain_version(&conn, domain.name).expect("read"), None);
        }
    }

    #[test]
    fn migration_using_its_own_savepoints_keeps_custody() {
        // A body may nest its own savepoints; custody only trips when the
        // runner's enclosing transaction (and with it the custody savepoint)
        // is gone.
        fn nests_savepoints(tx: &Transaction<'_>) -> Result<(), rusqlite::Error> {
            tx.execute_batch(
                "SAVEPOINT body_sp;
                 CREATE TABLE sp_t (x INTEGER);
                 RELEASE SAVEPOINT body_sp",
            )
        }
        const NESTED: SchemaDomain = SchemaDomain {
            name: "custody-nested-savepoint",
            migrations: &[Migration {
                version: 1,
                name: "nests-savepoints",
                apply: nests_savepoints,
            }],
            initialize_current: nests_savepoints,
            allowed_existing_versions: &[1],
            released_predecessors: &[],
            owned_objects: &[SchemaObject {
                kind: SchemaObjectKind::Table,
                name: "sp_t",
            }],
            retired_objects: &[],
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let mut conn = temp_conn(&dir);
        let report = apply_domain_migrations(&mut conn, &NESTED).expect("apply");
        assert_eq!(report.to_version, 1);
        assert_eq!(domain_version(&conn, NESTED.name).expect("read"), Some(1));
    }

    #[test]
    fn schema_preflight_passes_fresh_and_current_refuses_future() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut conn = temp_conn(&dir);
        preflight_schema_eligibility(&conn, &DOMAIN_V1).expect("no ledger yet");
        apply_domain_migrations(&mut conn, &DOMAIN_V2).expect("stamp v2");
        preflight_schema_eligibility(&conn, &DOMAIN_V2).expect("current");
        let err =
            preflight_schema_eligibility(&conn, &DOMAIN_V1).expect_err("future for old binary");
        assert!(matches!(
            err,
            SqliteStoreError::SchemaFromTheFuture {
                found: 2,
                supported: 1,
                ..
            }
        ));
    }

    #[test]
    fn concurrent_opens_race_safely() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("db.sqlite3");
        let mut handles = Vec::new();
        for _ in 0..8 {
            let path = path.clone();
            handles.push(std::thread::spawn(move || {
                let mut conn = open(&path, ConnectionProfile::PRIMARY).expect("open");
                apply_domain_migrations(&mut conn, &DOMAIN_V2).expect("apply")
            }));
        }
        let mut migrated = 0;
        for handle in handles {
            let report = handle.join().expect("thread");
            assert_eq!(report.to_version, 2);
            if report.migrated() {
                migrated += 1;
            }
        }
        assert!(migrated >= 1, "someone must have migrated");
        let conn = open(&path, ConnectionProfile::ReadOnly).expect("reopen");
        assert_eq!(domain_version(&conn, "test-domain").expect("read"), Some(2));
    }
}

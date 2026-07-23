//! Offline migration primitives behind `rkat storage migrate` / `rkat
//! storage prune` (Phase 6 of the storage unification arc).
//!
//! This module is a **reusable library layer** over an arbitrary state
//! directory — standalone mobkit gateways with no `rkat` CLI on the box run
//! their migrations through these same primitives. It owns:
//!
//! - [`RealmMaintenanceFence`]: the realm-wide exclusive maintenance fence.
//!   It takes the realm-level write-admission fence
//!   (`<realm_dir>/realm.mfence`, honored by the JSONL session store and the
//!   filesystem blob/artifact stores on every write) first, then the
//!   per-file [`meerkat_sqlite::ExclusiveFence`] on the **full fixed
//!   inventory** ([`REALM_SQLITE_FILES`], whether or not each database
//!   exists yet — the fence file is a sibling lock), then the dynamic
//!   `mobs/*.db` set with a re-enumeration check so a database created
//!   mid-acquisition cannot escape. Acquisition order is deterministic
//!   (admission fence first serializes concurrent migrators; per-file
//!   fences are sorted), waiting up to a deadline for in-flight
//!   per-operation guards to drain. Acquisition is all-or-nothing: any
//!   failure releases everything already acquired (RAII).
//! - The **backup naming discipline**: structural changes rename, never
//!   delete. [`backup_artifact_name`] produces
//!   `<original>.pre-<workspace-version>-<unix-ts>[.<purpose>]`; doctor
//!   lists `*.pre-*` as `backup-artifact` findings and `rkat storage prune`
//!   owns their lifecycle. External retention tooling can recognize backup
//!   artifacts by the `.pre-` name segment.
//! - The shape-stable **report vocabulary** ([`MigrateReport`],
//!   [`PruneReport`], ...) serialized by `rkat storage migrate --json` /
//!   `rkat storage prune --json` and reused by downstream orchestrators.
//! - Read-only helpers the orchestration layer composes: realm-directory
//!   listing, ledger-version reads, the legacy-checkpoint census, and the
//!   split-brain divergence computation (sessions row-level by id + content
//!   digest; other domains at file-digest level in v1).
//!
//! Orchestration (which store constructors to run, when to fence, what to
//! archive) lives with the caller — the CLI's `storage migrate` verb for
//! disk realms — because only the top of the dependency graph can see every
//! store crate.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use meerkat_core::{
    REALM_MANIFEST_FILE_NAME, SessionCheckpointMetadataState, SessionId,
    session_checkpoint_metadata_state,
};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::StoreError;

/// SQLite database files a realm directory can materialize, relative to the
/// realm root. This is the same inventory `doctor` sweeps and the file set
/// the [`RealmMaintenanceFence`] covers — every path in this list is fenced
/// whether or not the database exists yet (plus the per-mob `mobs/*.db`
/// databases, which are enumerated dynamically).
pub const REALM_SQLITE_FILES: &[&str] = &[
    "sessions.sqlite3",
    "runtime.sqlite3",
    "workgraph.sqlite3",
    "memory/memory.sqlite3",
    "tasks.db",
    "sessions_jsonl/session_index.sqlite3",
];

/// Stable finding code: a legacy `<home>/.rkat/sessions` directory exists
/// (report-only; migrate never moves it).
pub const FINDING_LEGACY_HOME_SESSIONS_DIR: &str = "legacy-home-sessions-dir";

/// Enumerate every SQLite database file currently materialized under a realm
/// directory: the fixed [`REALM_SQLITE_FILES`] list plus per-mob `mobs/*.db`
/// databases. Sorted (deterministic order). Symlinked entries are excluded
/// (see [`enumerate_realm_sqlite_inventory`] for the reporting variant).
pub fn enumerate_realm_sqlite_files(realm_dir: &Path) -> Vec<PathBuf> {
    enumerate_realm_sqlite_inventory(realm_dir).files
}

/// Realm SQLite inventory with symlinked entries reported instead of
/// silently followed.
#[non_exhaustive]
#[derive(Debug, Default)]
pub struct RealmSqliteInventory {
    /// Regular database files, sorted.
    pub files: Vec<PathBuf>,
    /// Inventory paths occupied by symlinks, sorted. Never followed:
    /// digesting or archiving through a link would reach a target outside
    /// the realm. Callers surface these as per-realm errors/notes.
    pub symlinks: Vec<PathBuf>,
}

/// [`enumerate_realm_sqlite_files`] variant that also reports symlinked
/// inventory entries (checked via `symlink_metadata`, never followed).
pub fn enumerate_realm_sqlite_inventory(realm_dir: &Path) -> RealmSqliteInventory {
    let mut inventory = RealmSqliteInventory::default();
    for path in REALM_SQLITE_FILES
        .iter()
        .map(|relative| realm_dir.join(relative))
    {
        classify_inventory_path(path, &mut inventory);
    }
    if let Ok(entries) = fs::read_dir(realm_dir.join("mobs")) {
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) == Some("db") {
                classify_inventory_path(path, &mut inventory);
            }
        }
    }
    inventory.files.sort();
    inventory.symlinks.sort();
    inventory
}

fn classify_inventory_path(path: PathBuf, inventory: &mut RealmSqliteInventory) {
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() => inventory.symlinks.push(path),
        Ok(metadata) if metadata.is_file() => inventory.files.push(path),
        // Absent, wrong-typed, or unprobeable: not a digestible database.
        _ => {}
    }
}

/// The dynamic per-mob `mobs/*.db` databases currently materialized under a
/// realm directory, sorted.
fn mob_database_files(realm_dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = fs::read_dir(realm_dir.join("mobs")) {
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) == Some("db") && path.is_file() {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// File-name stem of the realm-level write-admission fence: guards and the
/// maintenance fence both target `<realm_dir>/realm`, so the shared lock
/// file is `<realm_dir>/realm.mfence` (see
/// [`meerkat_sqlite::fence_lock_path`]).
///
/// SQLite stores are quiesced by their per-file fences; the JSONL session
/// store and the filesystem blob/artifact stores have no database file, so
/// their write paths take the shared [`meerkat_sqlite::OperationGuard`] on
/// this target instead. Holding the exclusive side (as
/// [`RealmMaintenanceFence`] does) therefore excludes every live durable
/// writer in the realm, not just the SQLite ones.
pub const REALM_WRITE_ADMISSION_STEM: &str = "realm";

/// The path whose `.mfence` sibling is the realm-level write-admission lock
/// for `realm_dir`.
pub fn realm_write_admission_target(realm_dir: &Path) -> PathBuf {
    realm_dir.join(REALM_WRITE_ADMISSION_STEM)
}

/// Detect at store construction whether `store_dir` sits inside a realm
/// directory (its parent holds a realm manifest); if so, return the
/// write-admission target the store's per-operation guards must use.
///
/// Deriving this once at construction keeps the write hot path cheap:
/// standalone stores (arbitrary directories, tests) get `None` and skip the
/// guard entirely.
pub fn store_realm_admission_target(store_dir: &Path) -> Option<PathBuf> {
    let parent = store_dir.parent()?;
    parent
        .join(REALM_MANIFEST_FILE_NAME)
        .is_file()
        .then(|| realm_write_admission_target(parent))
}

/// The realm-wide exclusive maintenance fence.
///
/// Holds the realm-level write-admission fence
/// ([`realm_write_admission_target`], honored per operation by the JSONL
/// session store and the filesystem blob/artifact stores) plus one
/// [`meerkat_sqlite::ExclusiveFence`] per SQLite database in the **full
/// fixed inventory** ([`REALM_SQLITE_FILES`], whether or not the file exists
/// yet) and per currently-materialized `mobs/*.db` database (re-enumerated
/// until stable). While held, every foreign process's per-operation guard
/// fails typed (`MaintenanceFenceHeld`); the holder's own in-process store
/// operations self-admit (see `meerkat_sqlite::fence`), which is what lets
/// bulk maintenance reuse production store code paths.
///
/// Not covered: a `mobs/*.db` database created by a foreign process *after*
/// acquisition completes (mob store openers take only their own per-file
/// guard), and writers that bypass the guard seams entirely.
///
/// Acquisition blocks the calling thread (bounded by the deadline); async
/// callers should wrap it in `spawn_blocking`.
#[derive(Debug)]
pub struct RealmMaintenanceFence {
    admission: meerkat_sqlite::ExclusiveFence,
    fences: Vec<meerkat_sqlite::ExclusiveFence>,
    databases: Vec<PathBuf>,
}

impl RealmMaintenanceFence {
    /// Fence `realm_dir`, waiting up to `deadline` (total, across all locks)
    /// for in-flight operations to drain.
    ///
    /// The write-admission fence is taken first (serializing concurrent
    /// migrators outright), then the fixed inventory in sorted order, then
    /// the dynamic mob databases — so two concurrent migrators acquire in
    /// the same sequence instead of deadlocking ABBA. Fixed-inventory fence
    /// files are sibling locks; creating a missing enclosing directory (for
    /// example `memory/`) is the only mutation acquisition performs. On any
    /// failure the already-acquired fences are released (RAII) and the typed
    /// error surfaces ([`StoreError::MaintenanceFenceHeld`] when a foreign
    /// holder owns a fence past the deadline).
    pub fn acquire(realm_dir: &Path, deadline: Duration) -> Result<Self, StoreError> {
        let started = Instant::now();
        let remaining = |started: &Instant| deadline.saturating_sub(started.elapsed());

        let admission = meerkat_sqlite::ExclusiveFence::acquire(
            &realm_write_admission_target(realm_dir),
            remaining(&started),
        )
        .map_err(StoreError::from)?;

        // Full fixed inventory, existing or not: a database created during
        // the maintenance window is already excluded by its sibling fence.
        let mut databases: Vec<PathBuf> = REALM_SQLITE_FILES
            .iter()
            .map(|relative| realm_dir.join(relative))
            .collect();
        databases.sort();
        let mut fences = Vec::with_capacity(databases.len());
        for database in &databases {
            if let Some(parent) = database.parent() {
                fs::create_dir_all(parent)?;
            }
            let fence = meerkat_sqlite::ExclusiveFence::acquire(database, remaining(&started))
                .map_err(|error| {
                    // Dropping `admission`/`fences` here releases everything.
                    StoreError::from(error)
                })?;
            fences.push(fence);
        }

        // Dynamic per-mob databases, enumerated under the already-held fixed
        // fences. Re-enumerate until the set is stable: a database created
        // between enumeration and fencing must not escape the fence. The
        // first pass always runs; later growth is bounded by the deadline.
        let mut first_pass = true;
        loop {
            let new: Vec<PathBuf> = mob_database_files(realm_dir)
                .into_iter()
                .filter(|database| !databases.contains(database))
                .collect();
            if new.is_empty() {
                break;
            }
            if !first_pass && started.elapsed() >= deadline {
                return Err(StoreError::Internal(format!(
                    "mob databases kept appearing under '{}' during maintenance-fence \
                     acquisition; realm is not quiescent",
                    realm_dir.join("mobs").display()
                )));
            }
            first_pass = false;
            for database in new {
                let fence = meerkat_sqlite::ExclusiveFence::acquire(&database, remaining(&started))
                    .map_err(StoreError::from)?;
                fences.push(fence);
                databases.push(database);
            }
        }
        databases.sort();

        Ok(Self {
            admission,
            fences,
            databases,
        })
    }

    /// The database files this fence covers (fixed inventory plus mob
    /// databases), sorted. Fixed-inventory entries are fenced even when the
    /// database does not exist yet.
    pub fn fenced_databases(&self) -> &[PathBuf] {
        &self.databases
    }

    /// The lock file of the realm-level write-admission fence.
    pub fn admission_lock_path(&self) -> &Path {
        self.admission.lock_path()
    }

    /// Number of held per-file fences (excludes the admission fence).
    pub fn len(&self) -> usize {
        self.fences.len()
    }

    /// True when no per-file fence is held. The full fixed inventory is
    /// always fenced, so this is never true after a successful `acquire`.
    pub fn is_empty(&self) -> bool {
        self.fences.is_empty()
    }
}

/// Compose the registered backup-artifact name for `original`:
/// `<original>.pre-<workspace-version>-<unix-ts>[.<purpose>]`.
///
/// The `.pre-` segment is the **registered recognition token**: doctor lists
/// matching paths as `backup-artifact` findings, `rkat storage prune` owns
/// their lifecycle, and external retention tooling may rely on it. The
/// version is the workspace version this binary was built from; the
/// timestamp is Unix seconds; the optional purpose suffix names why the
/// artifact exists (for example `split-brain`).
pub fn backup_artifact_name(original: &str, purpose: &str) -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    let version = env!("CARGO_PKG_VERSION");
    if purpose.is_empty() {
        format!("{original}.pre-{version}-{timestamp}")
    } else {
        format!("{original}.pre-{version}-{timestamp}.{purpose}")
    }
}

/// True when a file/directory name matches the registered backup-artifact
/// naming ([`backup_artifact_name`]): `<original>.pre-<version>-<unix-ts>`
/// with an optional trailing `.<purpose>` segment.
///
/// The full suffix shape is validated — a mere `.pre-` substring (for
/// example `notes.pre-release`) is NOT a registered artifact, so prune can
/// never sweep unrelated files.
pub fn is_backup_artifact_name(name: &str) -> bool {
    let Some(idx) = name.rfind(".pre-") else {
        return false;
    };
    if idx == 0 {
        // No original name before the marker.
        return false;
    }
    // suffix = <version>-<unix-ts>[.<purpose>]; the version contains no '-'
    // (plain x.y.z workspace versions), so the first '-' ends it.
    let suffix = &name[idx + ".pre-".len()..];
    let Some((version, rest)) = suffix.split_once('-') else {
        return false;
    };
    let version_ok = !version.is_empty()
        && version.split('.').count() >= 2
        && version
            .split('.')
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()));
    if !version_ok {
        return false;
    }
    let (timestamp, purpose) = match rest.split_once('.') {
        Some((timestamp, purpose)) => (timestamp, Some(purpose)),
        None => (rest, None),
    };
    let timestamp_ok = !timestamp.is_empty() && timestamp.chars().all(|c| c.is_ascii_digit());
    let purpose_ok = purpose.is_none_or(|p| {
        !p.is_empty()
            && p.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    });
    timestamp_ok && purpose_ok
}

/// True when a file name matches the registered index-quarantine naming
/// (`*.corrupt-<timestamp>`): the suffix after `.corrupt-` must be all
/// digits, and something must precede the marker.
pub fn is_quarantine_artifact_name(name: &str) -> bool {
    let Some(idx) = name.rfind(".corrupt-") else {
        return false;
    };
    if idx == 0 {
        return false;
    }
    let timestamp = &name[idx + ".corrupt-".len()..];
    !timestamp.is_empty() && timestamp.chars().all(|c| c.is_ascii_digit())
}

/// One completed archive rename plus its post-rename warnings.
///
/// The rename is the preservation; read-only hardening and parent-directory
/// durability are best-effort and surface here as warnings instead of
/// failing an archive that already succeeded.
#[non_exhaustive]
#[derive(Debug)]
pub struct ArchivedPath {
    /// The archive path the original was renamed to.
    pub archive: PathBuf,
    /// Post-rename hardening/durability failures (write-permission strip,
    /// parent-directory fsync). Report-only.
    pub warnings: Vec<String>,
}

/// Rename `path` (file or directory) to its registered backup-artifact name
/// next to the original, strip write permission so the archive is
/// read-only, and fsync the parent directory so the rename is
/// crash-durable. Hardening/durability failures never fail the archive —
/// they surface in [`ArchivedPath::warnings`].
pub fn archive_path_read_only_reported(
    path: &Path,
    purpose: &str,
) -> Result<ArchivedPath, StoreError> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            StoreError::Internal(format!(
                "cannot archive '{}': path has no UTF-8 file name",
                path.display()
            ))
        })?;
    let archive = path.with_file_name(backup_artifact_name(name, purpose));
    fs::rename(path, &archive)?;
    let mut warnings = Vec::new();
    strip_write_permissions(&archive, &mut warnings);
    sync_parent_dir_reported(&archive, &mut warnings);
    Ok(ArchivedPath { archive, warnings })
}

/// [`archive_path_read_only_reported`] with the hardening/durability
/// warnings dropped. Prefer the reported variant so partial hardening is
/// visible in reports.
pub fn archive_path_read_only(path: &Path, purpose: &str) -> Result<PathBuf, StoreError> {
    archive_path_read_only_reported(path, purpose).map(|archived| archived.archive)
}

/// Fsync the parent directory of `path` so a completed rename survives a
/// crash (unix; directory handles are not fsyncable elsewhere). Failures
/// are warnings: the rename itself already happened.
fn sync_parent_dir_reported(path: &Path, warnings: &mut Vec<String>) {
    #[cfg(unix)]
    {
        let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        else {
            return;
        };
        let synced = fs::File::open(parent).and_then(|dir| dir.sync_all());
        if let Err(error) = synced {
            warnings.push(format!(
                "archive rename to '{}' is not crash-durable: fsync of parent directory '{}' \
                 failed: {error}",
                path.display(),
                parent.display()
            ));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (path, &mut *warnings);
    }
}

/// Recursively strip write permission, recording failures as warnings.
/// Symlinks are skipped entirely: `fs::set_permissions` follows them, and a
/// link inside an archive must never chmod its external target.
fn strip_write_permissions(path: &Path, warnings: &mut Vec<String>) {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => {
            warnings.push(format!(
                "archive hardening skipped for '{}': {error}",
                path.display()
            ));
            return;
        }
    };
    if metadata.file_type().is_symlink() {
        return;
    }
    if metadata.is_dir() {
        match fs::read_dir(path) {
            Ok(entries) => {
                for entry in entries.filter_map(Result::ok) {
                    strip_write_permissions(&entry.path(), warnings);
                }
            }
            Err(error) => warnings.push(format!(
                "archive hardening could not list '{}': {error}",
                path.display()
            )),
        }
    }
    let mut permissions = metadata.permissions();
    permissions.set_readonly(true);
    if let Err(error) = fs::set_permissions(path, permissions) {
        warnings.push(format!(
            "archive '{}' may remain writable: {error}",
            path.display()
        ));
    }
}

fn restore_write_permissions_best_effort(path: &Path) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    if metadata.file_type().is_symlink() {
        // `fs::set_permissions` follows symlinks; never chmod through one.
        return;
    }
    #[allow(clippy::permissions_set_readonly_false)] // deliberate: prune restores
    // write permission on registered archives it is about to delete.
    {
        let mut permissions = metadata.permissions();
        permissions.set_readonly(false);
        let _ = fs::set_permissions(path, permissions);
    }
    if metadata.is_dir()
        && let Ok(entries) = fs::read_dir(path)
    {
        for entry in entries.filter_map(Result::ok) {
            restore_write_permissions_best_effort(&entry.path());
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Shape-stable report vocabulary (serde; `#[non_exhaustive]` + defaults,
// like the `meerkat_core::storage_diagnostics` types).
// ─────────────────────────────────────────────────────────────────────────

/// Dry-run vs apply.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MigrateMode {
    /// Read-only: report what would change; touch nothing.
    #[default]
    DryRun,
    /// Fenced structural migration.
    Apply,
}

/// The full `storage migrate` report.
#[non_exhaustive]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MigrateReport {
    /// Dry-run or apply.
    #[serde(default)]
    pub mode: MigrateMode,
    /// State roots swept (explicit roots, or the resolver's candidates).
    #[serde(default)]
    pub swept_roots: Vec<PathBuf>,
    /// Per-realm migration outcomes (cases 1, 2, and 4).
    #[serde(default)]
    pub realms: Vec<RealmMigrateReport>,
    /// Split-brain twins and their resolution (case 3).
    #[serde(default)]
    pub split_brain: Vec<SplitBrainReport>,
    /// Deprecated-leftover findings (case 5; report-only) plus sweep
    /// context, reusing the doctor finding vocabulary.
    #[serde(default)]
    pub findings: Vec<meerkat_core::StorageFinding>,
    /// Fail-closed refusals and hard failures. Non-empty ⇒ nonzero exit.
    #[serde(default)]
    pub errors: Vec<String>,
}

impl MigrateReport {
    /// New empty report for one run.
    pub fn new(mode: MigrateMode, swept_roots: Vec<PathBuf>) -> Self {
        Self {
            mode,
            swept_roots,
            ..Self::default()
        }
    }

    /// True when the run must exit nonzero (refusals, fence failures,
    /// per-realm errors).
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty() || self.realms.iter().any(|realm| !realm.errors.is_empty())
    }
}

/// Migration outcome for one realm materialization.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RealmMigrateReport {
    /// Realm id (manifest identity; directory name when unreadable).
    pub realm: String,
    /// The realm directory (case 2 — state-root adoption is report-only:
    /// the realm is used where it lies).
    pub root: PathBuf,
    /// Backend pinned in the manifest, when readable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    /// Ledger baseline entries per database × domain (case 1).
    #[serde(default)]
    pub ledger: Vec<LedgerBaselineEntry>,
    /// Checkpoint-evidence adoption outcome (case 4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adoption: Option<CheckpointAdoptionOutcome>,
    /// Human-readable per-realm notes (skips, report-only carve-outs).
    #[serde(default)]
    pub notes: Vec<String>,
    /// Per-realm failures (fence not acquirable, store open failures).
    #[serde(default)]
    pub errors: Vec<String>,
}

impl RealmMigrateReport {
    /// New empty per-realm report.
    pub fn new(realm: impl Into<String>, root: PathBuf) -> Self {
        Self {
            realm: realm.into(),
            root,
            backend: None,
            ledger: Vec::new(),
            adoption: None,
            notes: Vec::new(),
            errors: Vec::new(),
        }
    }
}

/// One database × ledger-domain baseline row.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerBaselineEntry {
    /// Database file.
    pub database: PathBuf,
    /// Ledger domain (`session-store`, `runtime-store`, ...).
    pub domain: String,
    /// Version before the run (`None` = no ledger row).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<i64>,
    /// Version after the run (`None` in dry-run).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<i64>,
    /// What happened (or would happen).
    pub action: LedgerBaselineAction,
}

impl LedgerBaselineEntry {
    /// New entry with no before/after versions (set the public fields).
    pub fn new(database: PathBuf, domain: impl Into<String>, action: LedgerBaselineAction) -> Self {
        Self {
            database,
            domain: domain.into(),
            before: None,
            after: None,
            action,
        }
    }
}

/// Disposition of one ledger baseline row.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LedgerBaselineAction {
    /// Dry-run: no ledger row; the owning store baseline-stamps on
    /// `--apply` (guarded migrations converge files of any vintage).
    WouldStamp,
    /// Dry-run: a ledger row exists; any pending migrations converge on
    /// `--apply` through the owning store's normal constructor.
    Recorded,
    /// Apply: the ledger row was created or advanced.
    Stamped,
    /// Apply: already at the current version; no-op.
    AlreadyCurrent,
    /// Not migrated by this verb in v1 (per-mob databases); the owning
    /// store converges the file on its next open.
    ReportOnly,
}

/// Case 4 outcome: bulk checkpoint-evidence adoption (or its census).
#[non_exhaustive]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CheckpointAdoptionOutcome {
    /// Session rows enumerated by the apply sweep.
    #[serde(default)]
    pub scanned: usize,
    /// Rows already carrying verified checkpoint authority.
    #[serde(default)]
    pub already_verified: usize,
    /// Rows adopted by the apply sweep.
    #[serde(default)]
    pub adopted: usize,
    /// Dry-run census: legacy-unverified rows pending adoption.
    #[serde(default)]
    pub legacy_pending: usize,
    /// Per-session refusals (divergent pairs, invalid evidence).
    #[serde(default)]
    pub refused: Vec<CheckpointAdoptionRefusal>,
    /// Per-session load/adopt failures the sweep continued past
    /// (`session id` + error detail).
    #[serde(default)]
    pub failures: Vec<(String, String)>,
    /// Set when adoption did not run (dry-run census, jsonl backend).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
}

/// One per-session adoption refusal.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointAdoptionRefusal {
    /// Session id.
    pub session_id: String,
    /// Typed refusal reason (reported, never synthesized around).
    pub reason: String,
}

impl CheckpointAdoptionRefusal {
    /// New refusal entry.
    pub fn new(session_id: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            reason: reason.into(),
        }
    }
}

/// Case 3: one split-brain realm (same realm id under 2+ swept roots).
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SplitBrainReport {
    /// Realm id.
    pub realm: String,
    /// Every realm directory materializing this id.
    pub locations: Vec<PathBuf>,
    /// Sessions identical across every copy (count only; not enumerated).
    #[serde(default)]
    pub sessions_equal: usize,
    /// Divergent / single-copy sessions (row-level compare by id + content
    /// digest over the session tables).
    #[serde(default)]
    pub sessions: Vec<SessionDivergenceEntry>,
    /// Per-file content-digest comparison over every authoritative store the
    /// realm materializes: the SQLite databases (runtime / workgraph /
    /// schedule / memory / tasks / index / mob), the canonical
    /// `sessions_jsonl/*.jsonl` session files, and the `blobs/` and
    /// `artifacts/` trees.
    #[serde(default)]
    pub files: Vec<FileDivergenceEntry>,
    /// How the twin was (or was not) resolved.
    pub resolution: SplitBrainResolution,
    /// Divergence-computation failures (report-only; archiving is
    /// non-destructive either way, but see
    /// [`SplitBrainReport::comparison_is_conclusive`]).
    #[serde(default)]
    pub errors: Vec<String>,
}

impl SplitBrainReport {
    /// New unresolved (fail-closed) split-brain report for one realm.
    pub fn new(realm: impl Into<String>, locations: Vec<PathBuf>) -> Self {
        Self {
            realm: realm.into(),
            locations,
            sessions_equal: 0,
            sessions: Vec::new(),
            files: Vec::new(),
            resolution: SplitBrainResolution::Refused {
                reason: "split-brain unresolved: rerun with `--apply --adopt-root <path>` to \
                         adopt one root and archive the other copies read-only"
                    .to_string(),
            },
            errors: Vec::new(),
        }
    }

    /// True when every authoritative entry on every copy was readable and
    /// conclusively classified: no comparison errors and no
    /// [`DivergenceStatus::Unknown`] entries.
    ///
    /// An adopt-and-archive decision must not rest on an inconclusive
    /// report — an unreadable side may hold the only copy of content the
    /// report could not account for. (Divergent entries do not make a
    /// report inconclusive; archiving preserves divergent content.)
    pub fn comparison_is_conclusive(&self) -> bool {
        self.errors.is_empty()
            && self
                .sessions
                .iter()
                .all(|entry| entry.status != DivergenceStatus::Unknown)
            && self
                .files
                .iter()
                .all(|entry| entry.status != DivergenceStatus::Unknown)
    }
}

/// One non-equal session across split-brain copies.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionDivergenceEntry {
    /// Session id.
    pub session_id: String,
    /// Divergence classification.
    pub status: DivergenceStatus,
}

/// One database file compared across split-brain copies.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDivergenceEntry {
    /// Path relative to the realm directory.
    pub file: String,
    /// Divergence classification.
    pub status: DivergenceStatus,
}

/// Divergence classification across split-brain copies.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum DivergenceStatus {
    /// Present in every copy with identical content.
    Equal,
    /// Present in more than one copy with differing content (or missing
    /// from some copies).
    Divergent,
    /// Present in exactly one copy.
    OnlyIn {
        /// The realm directory holding the single copy.
        location: PathBuf,
    },
    /// A read failure on some copy poisoned this comparison: the entry may
    /// be equal, divergent, or unique, and no archive decision may rest on
    /// it. The failure is recorded in [`SplitBrainReport::errors`].
    Unknown,
}

/// Case 3 resolution.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum SplitBrainResolution {
    /// Fail-closed refusal: nothing moved. Pass `--apply --adopt-root
    /// <path>` to adopt one root and archive the others read-only.
    Refused {
        /// Why the run refused.
        reason: String,
    },
    /// One copy adopted where it lies; every other copy archived read-only
    /// under the registered backup naming. No synthesis, no merging —
    /// divergent content is preserved in the archives.
    Archived {
        /// The adopted realm directory (left untouched).
        adopted: PathBuf,
        /// Archive paths of the non-adopted copies.
        archived: Vec<PathBuf>,
    },
    /// Archiving stopped partway: the archives listed succeeded before the
    /// failure and stay where they are (renames are the preservation;
    /// nothing is rolled back). Rerun after fixing the fault. A partial
    /// archive must never be reported as "nothing moved".
    ArchiveFailed {
        /// The adopted realm directory (left untouched).
        adopted: PathBuf,
        /// Archive paths that succeeded before the failure.
        archived: Vec<PathBuf>,
        /// The failure that stopped the run.
        reason: String,
    },
}

/// The full `storage prune` report.
#[non_exhaustive]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PruneReport {
    /// Dry-run or apply.
    #[serde(default)]
    pub mode: MigrateMode,
    /// State roots swept.
    #[serde(default)]
    pub swept_roots: Vec<PathBuf>,
    /// Age threshold in days (artifacts at least this old are deleted on
    /// apply; `0` = all).
    #[serde(default)]
    pub older_than_days: u64,
    /// Registered artifacts found, with dispositions.
    #[serde(default)]
    pub artifacts: Vec<PruneArtifact>,
    /// Failures (delete errors). Non-empty ⇒ nonzero exit.
    #[serde(default)]
    pub errors: Vec<String>,
}

impl PruneReport {
    /// New empty report for one run.
    pub fn new(mode: MigrateMode, swept_roots: Vec<PathBuf>, older_than_days: u64) -> Self {
        Self {
            mode,
            swept_roots,
            older_than_days,
            ..Self::default()
        }
    }
}

/// One registered maintenance artifact.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PruneArtifact {
    /// Artifact path (file or directory).
    pub path: PathBuf,
    /// Which registered naming pattern matched.
    pub kind: PruneArtifactKind,
    /// Total size in bytes (recursive for directories).
    #[serde(default)]
    pub bytes: u64,
    /// Age in whole days (from mtime).
    #[serde(default)]
    pub age_days: u64,
    /// Disposition.
    pub action: PruneAction,
}

/// Registered artifact classes prune may touch. Anything else is never
/// touched.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PruneArtifactKind {
    /// `*.pre-<version>-<timestamp>[.<purpose>]` migration backup.
    BackupArtifact,
    /// `*.corrupt-<timestamp>` quarantined index.
    QuarantinedIndex,
}

/// Prune disposition for one artifact.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PruneAction {
    /// Dry-run: old enough; `--apply` would delete it.
    WouldDelete,
    /// Apply: deleted.
    Deleted,
    /// Younger than the threshold; kept.
    Kept,
    /// Apply: deletion failed (reason in `PruneReport::errors`).
    DeleteFailed,
}

// ─────────────────────────────────────────────────────────────────────────
// Read-only helpers for the orchestration layer.
// ─────────────────────────────────────────────────────────────────────────

/// One materialized realm directory under a state root.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct RealmDirEntry {
    /// Realm id (manifest identity; sanitized directory name when the
    /// manifest is unreadable).
    pub realm_id: String,
    /// Backend string from the manifest, when readable.
    pub backend: Option<String>,
    /// The state root swept.
    pub state_root: PathBuf,
    /// The realm directory.
    pub dir: PathBuf,
    /// False when the manifest failed to read/parse.
    pub manifest_readable: bool,
}

/// Lenient realm-directory listing under one state root: directories with a
/// `realm_manifest.json`, read as raw JSON (no backend validation), skipping
/// registered backup artifacts (`*.pre-*`). An absent root lists empty.
pub fn list_realm_dirs(state_root: &Path) -> Vec<RealmDirEntry> {
    let Ok(entries) = fs::read_dir(state_root) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();

    let mut realms = Vec::new();
    for dir in dirs {
        let name = dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if is_backup_artifact_name(&name) {
            continue; // archived realm copy, not a live realm
        }
        let manifest_path = dir.join(REALM_MANIFEST_FILE_NAME);
        if !manifest_path.is_file() {
            continue;
        }
        let parsed = fs::read(&manifest_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .and_then(|value| {
                let realm_id = value.get("realm_id")?.as_str()?.to_string();
                let backend = value
                    .get("backend")
                    .and_then(|backend| backend.as_str())
                    .map(ToString::to_string);
                Some((realm_id, backend))
            });
        let (realm_id, backend, manifest_readable) = match parsed {
            Some((realm_id, backend)) => (realm_id, backend, true),
            None => (name, None, false),
        };
        realms.push(RealmDirEntry {
            realm_id,
            backend,
            state_root: state_root.to_path_buf(),
            dir,
            manifest_readable,
        });
    }
    realms
}

/// Read the schema-ledger rows of one database, read-only. `Ok(None)` = no
/// `meerkat_schema` table (pre-ledger file).
pub fn read_domain_versions(db_path: &Path) -> Result<Option<Vec<(String, i64)>>, StoreError> {
    // No schema preflight: this read-only seam is how future versions get
    // reported, so it must open files it would otherwise refuse.
    let conn = meerkat_sqlite::open(db_path, meerkat_sqlite::ConnectionProfile::ReadOnly)
        .map_err(StoreError::from)?;
    let table_exists = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'meerkat_schema'",
            [],
            |_| Ok(()),
        )
        .optional()?;
    if table_exists.is_none() {
        return Ok(None);
    }
    let mut statement =
        conn.prepare("SELECT domain, version FROM meerkat_schema ORDER BY domain")?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some(rows))
}

/// Highest ledger version this binary supports for domains whose owning
/// store crates are visible from `meerkat-store`. Domains owned by crates
/// above this one in the dependency order (runtime-store, workgraph,
/// memory, mob, tools-tasks) return `None` and are reported without
/// judgment.
pub fn supported_domain_version(domain: &str) -> Option<i64> {
    match domain {
        #[cfg(feature = "sqlite")]
        "session-store" => Some(crate::sqlite_store::SESSION_STORE_DOMAIN.supported_version()),
        #[cfg(feature = "sqlite")]
        "schedule-store" => {
            Some(crate::schedule_sqlite_store::SCHEDULE_STORE_DOMAIN.supported_version())
        }
        "jsonl-index" => Some(crate::index::JSONL_INDEX_DOMAIN.supported_version()),
        _ => None,
    }
}

/// One database × domain ledger reading (`None` = expected domain with no
/// ledger row).
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct LedgerDomainReading {
    /// Database file.
    pub database: PathBuf,
    /// Ledger domain.
    pub domain: String,
    /// Recorded version, when a row exists.
    pub version: Option<i64>,
}

/// One domain recorded at a version newer than this binary supports.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct FutureDomainVersion {
    /// Database file.
    pub database: PathBuf,
    /// Ledger domain.
    pub domain: String,
    /// Version recorded in the file.
    pub found: i64,
    /// Highest version this binary supports for the domain.
    pub supported: i64,
}

/// Read-only ledger baseline of one realm's inventoried databases, with
/// failures surfaced typed.
#[non_exhaustive]
#[derive(Debug, Default)]
pub struct RealmLedgerBaseline {
    /// Readable ledger rows plus expected-domain gaps.
    pub rows: Vec<LedgerDomainReading>,
    /// Domains recorded at a future version — dry-run must report these as
    /// refusals exactly as `--apply`'s guarded constructors would refuse
    /// them ([`StoreError::SchemaFromTheFuture`]).
    pub future: Vec<FutureDomainVersion>,
    /// Ledger read failures. A corrupt/unreadable database is a per-realm
    /// error, never a missing ledger (it must not be reported as
    /// would-stamp).
    pub errors: Vec<String>,
}

/// Read the ledger baseline of every inventoried database currently on disk
/// under `realm_dir` (the doctor's file × domain matrix), read-only.
///
/// Unlike a bare [`read_domain_versions`] sweep, read failures and future
/// versions are first-class outcomes: dry-run reporting built on this can
/// never launder a corrupt database into "no ledger" or a future version
/// into "safely recorded".
pub fn read_realm_ledger_baseline(realm_dir: &Path) -> RealmLedgerBaseline {
    let mut baseline = RealmLedgerBaseline::default();
    for (relative, expected_domains) in crate::doctor::REALM_DATABASE_FILES {
        let db_path = realm_dir.join(relative);
        if !db_path.is_file() {
            continue;
        }
        let rows = match read_domain_versions(&db_path) {
            Ok(rows) => rows.unwrap_or_default(),
            Err(error) => {
                baseline.errors.push(format!(
                    "ledger unreadable for {}: {error}",
                    db_path.display()
                ));
                continue;
            }
        };
        for (domain, version) in &rows {
            if let Some(supported) = supported_domain_version(domain)
                && *version > supported
            {
                baseline.future.push(FutureDomainVersion {
                    database: db_path.clone(),
                    domain: domain.clone(),
                    found: *version,
                    supported,
                });
            }
            baseline.rows.push(LedgerDomainReading {
                database: db_path.clone(),
                domain: domain.clone(),
                version: Some(*version),
            });
        }
        for expected in *expected_domains {
            if !rows.iter().any(|(domain, _)| domain == expected) {
                baseline.rows.push(LedgerDomainReading {
                    database: db_path.clone(),
                    domain: (*expected).to_string(),
                    version: None,
                });
            }
        }
    }
    baseline
}

/// Checkpoint-evidence census of one sqlite session database (read-only).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct LegacySessionCensus {
    /// Rows carrying verified checkpoint evidence.
    #[serde(default)]
    pub verified: usize,
    /// Rows pending the one-time adoption (legacy-unverified).
    #[serde(default)]
    pub legacy: usize,
    /// Rows with malformed evidence (never laundered into legacy).
    #[serde(default)]
    pub invalid: usize,
}

fn table_exists(conn: &rusqlite::Connection, table: &str) -> Result<bool, rusqlite::Error> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [table],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Count verified / legacy-unverified / invalid session documents in a
/// sqlite session database, read-only — the same classification doctor's
/// census applies (`session_heads` canonical over `sessions`).
pub fn census_legacy_sessions(db_path: &Path) -> Result<LegacySessionCensus, StoreError> {
    // No schema preflight: a read-only census must also observe files whose
    // domain is ahead of this binary.
    let conn = meerkat_sqlite::open(db_path, meerkat_sqlite::ConnectionProfile::ReadOnly)
        .map_err(StoreError::from)?;
    let mut census = LegacySessionCensus::default();
    let mut classify = |session_id: &str, metadata_json: &str| {
        let Ok(id) = SessionId::parse(session_id) else {
            census.invalid += 1;
            return;
        };
        let Ok(metadata) =
            serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(metadata_json)
        else {
            census.invalid += 1;
            return;
        };
        match session_checkpoint_metadata_state(&id, &metadata) {
            Ok(SessionCheckpointMetadataState::Stamped(_)) => census.verified += 1,
            Ok(SessionCheckpointMetadataState::LegacyUnverified { .. }) => census.legacy += 1,
            Err(_) => census.invalid += 1,
        }
    };

    let heads_exist = table_exists(&conn, "session_heads")?;
    if heads_exist {
        let mut statement = conn.prepare("SELECT session_id, metadata_json FROM session_heads")?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let session_id: String = row.get(0)?;
            let metadata_json: String = row.get(1)?;
            classify(&session_id, &metadata_json);
        }
    }
    if table_exists(&conn, "sessions")? {
        let sql = if heads_exist {
            "SELECT session_id, metadata_json FROM sessions \
             WHERE session_id NOT IN (SELECT session_id FROM session_heads)"
        } else {
            "SELECT session_id, metadata_json FROM sessions"
        };
        let mut statement = conn.prepare(sql)?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let session_id: String = row.get(0)?;
            let metadata_json: String = row.get(1)?;
            classify(&session_id, &metadata_json);
        }
    }
    Ok(census)
}

// ─────────────────────────────────────────────────────────────────────────
// Split-brain divergence (case 3).
// ─────────────────────────────────────────────────────────────────────────

type SessionDigests = BTreeMap<String, [u8; 32]>;

/// Per-session content digest over the session-domain tables (`sessions`,
/// `session_heads`, `session_strand_messages`, `session_rewrites`), fed in a
/// fixed table order with ordered rows so digests are comparable across
/// copies.
fn session_digests(db_path: &Path) -> Result<SessionDigests, StoreError> {
    // No schema preflight: divergence comparison is read-only and must not
    // refuse a copy merely because a newer binary migrated it.
    let conn = meerkat_sqlite::open(db_path, meerkat_sqlite::ConnectionProfile::ReadOnly)
        .map_err(StoreError::from)?;
    let mut hashers: BTreeMap<String, Sha256> = BTreeMap::new();
    let mut feed = |session_id: String, tag: &str, chunks: &[&[u8]]| {
        let hasher = hashers.entry(session_id).or_default();
        hasher.update(tag.as_bytes());
        hasher.update([0u8]);
        for chunk in chunks {
            hasher.update(chunk);
            hasher.update([0u8]);
        }
    };

    // `JsonColumnBytes` tolerates TEXT-or-BLOB storage for the JSON columns.
    if table_exists(&conn, "sessions")? {
        let mut statement = conn.prepare(
            "SELECT session_id, metadata_json, session_json FROM sessions ORDER BY session_id",
        )?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let session_id: String = row.get(0)?;
            let metadata = row
                .get::<_, meerkat_sqlite::JsonColumnBytes>(1)?
                .into_bytes();
            let document = row
                .get::<_, meerkat_sqlite::JsonColumnBytes>(2)?
                .into_bytes();
            feed(session_id, "sessions", &[&metadata, &document]);
        }
    }
    if table_exists(&conn, "session_heads")? {
        let mut statement = conn.prepare(
            "SELECT session_id, metadata_json, head_json FROM session_heads ORDER BY session_id",
        )?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let session_id: String = row.get(0)?;
            let metadata = row
                .get::<_, meerkat_sqlite::JsonColumnBytes>(1)?
                .into_bytes();
            let head = row
                .get::<_, meerkat_sqlite::JsonColumnBytes>(2)?
                .into_bytes();
            feed(session_id, "head", &[&metadata, &head]);
        }
    }
    if table_exists(&conn, "session_strand_messages")? {
        let mut statement = conn.prepare(
            "SELECT session_id, strand, seq, message_json FROM session_strand_messages \
             ORDER BY session_id, strand, seq",
        )?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let session_id: String = row.get(0)?;
            let strand: String = row.get(1)?;
            let seq: i64 = row.get(2)?;
            let message = row
                .get::<_, meerkat_sqlite::JsonColumnBytes>(3)?
                .into_bytes();
            feed(
                session_id,
                "message",
                &[strand.as_bytes(), seq.to_be_bytes().as_slice(), &message],
            );
        }
    }
    if table_exists(&conn, "session_rewrites")? {
        let mut statement = conn.prepare(
            "SELECT session_id, rewrite_idx, commit_json FROM session_rewrites \
             ORDER BY session_id, rewrite_idx",
        )?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let session_id: String = row.get(0)?;
            let index: i64 = row.get(1)?;
            let commit = row
                .get::<_, meerkat_sqlite::JsonColumnBytes>(2)?
                .into_bytes();
            feed(
                session_id,
                "rewrite",
                &[index.to_be_bytes().as_slice(), &commit],
            );
        }
    }

    Ok(hashers
        .into_iter()
        .map(|(session_id, hasher)| (session_id, hasher.finalize().into()))
        .collect())
}

/// Stream one file's bytes into `hasher` through the fixed-size buffer
/// `std::io::copy` maintains — divergence hashing must never materialize an
/// entire database (or blob) in memory.
fn stream_file_into(hasher: &mut Sha256, path: &Path) -> Result<(), StoreError> {
    let mut file = fs::File::open(path)?;
    std::io::copy(&mut file, hasher)?;
    Ok(())
}

/// Streamed digest of one file's bytes; for SQLite databases the `-wal`
/// sidecar (if present) is folded in, so uncheckpointed frames register as
/// divergence (conservative: never reports "equal" for possibly-different
/// content).
fn file_digest(path: &Path) -> Result<[u8; 32], StoreError> {
    let mut hasher = Sha256::new();
    stream_file_into(&mut hasher, path)?;
    let mut wal = path.as_os_str().to_os_string();
    wal.push("-wal");
    let wal = PathBuf::from(wal);
    match fs::symlink_metadata(&wal) {
        // A symlinked sidecar would smuggle external bytes into the digest;
        // refuse it so the entry poisons as Unknown instead of comparing.
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(StoreError::Internal(format!(
                "refusing to follow symlink {} while digesting {}",
                wal.display(),
                path.display()
            )));
        }
        Ok(metadata) if metadata.is_file() => stream_file_into(&mut hasher, &wal)?,
        _ => {}
    }
    Ok(hasher.finalize().into())
}

/// Everything authoritative one split-brain copy materializes, read in a
/// single sequential pass for that side.
#[derive(Debug)]
struct RealmContentSnapshot {
    /// `None`: a sessions database exists but its rows could not be read —
    /// the whole session comparison is poisoned (a failed read is not an
    /// empty side). An absent database is legitimately empty (`Some`).
    sessions: Option<SessionDigests>,
    /// Relative path (`/`-separated) → streamed content digest.
    files: BTreeMap<String, [u8; 32]>,
    /// Entries that exist on this side but could not be read/enumerated.
    /// Directory entries carry a trailing `/` and poison every path under
    /// the prefix.
    unreadable: Vec<String>,
    /// Human-readable read failures (folded into the report's errors).
    errors: Vec<String>,
}

impl RealmContentSnapshot {
    fn poisons(&self, relative: &str) -> bool {
        self.unreadable.iter().any(|entry| {
            entry == relative || (entry.ends_with('/') && relative.starts_with(entry.as_str()))
        })
    }

    fn record_unreadable(&mut self, relative: String, error: String) {
        self.errors.push(error);
        self.unreadable.push(relative);
    }
}

fn snapshot_relative(location: &Path, path: &Path) -> Option<String> {
    path.strip_prefix(location)
        .ok()
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
}

/// Digest one directory entry into the snapshot. Symlinks are never
/// followed (a link could smuggle external bytes into — or hide realm bytes
/// from — the comparison) and non-regular files cannot be accounted for:
/// both poison their entry instead of silently passing.
fn digest_entry_into(path: &Path, relative: String, snapshot: &mut RealmContentSnapshot) {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => snapshot.record_unreadable(
            relative,
            format!(
                "refusing to follow symlink {} during divergence comparison",
                path.display()
            ),
        ),
        Ok(metadata) if metadata.is_file() => match file_digest(path) {
            Ok(digest) => {
                snapshot.files.insert(relative, digest);
            }
            Err(error) => snapshot.record_unreadable(
                relative,
                format!("file digest unavailable for {}: {error}", path.display()),
            ),
        },
        Ok(_) => snapshot.record_unreadable(
            relative,
            format!(
                "{} is not a regular file; divergence cannot account for it",
                path.display()
            ),
        ),
        Err(error) => {
            snapshot
                .record_unreadable(relative, format!("cannot stat {}: {error}", path.display()));
        }
    }
}

/// The canonical JSONL session files (`sessions_jsonl/*.jsonl`) — the
/// durable truth of a jsonl-backend realm; the SQLite index next to them is
/// a derived projection covered by the database sweep.
fn snapshot_jsonl_sessions(location: &Path, snapshot: &mut RealmContentSnapshot) {
    let dir = location.join("sessions_jsonl");
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            snapshot.record_unreadable(
                "sessions_jsonl/".to_string(),
                format!("cannot list {}: {error}", dir.display()),
            );
            return;
        }
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    paths.sort();
    for path in paths {
        if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            continue;
        }
        let Some(relative) = snapshot_relative(location, &path) else {
            continue;
        };
        digest_entry_into(&path, relative, snapshot);
    }
}

/// Recursive content walk of one authoritative tree (`blobs/`,
/// `artifacts/`).
fn snapshot_tree(location: &Path, tree: &str, snapshot: &mut RealmContentSnapshot) {
    let root = location.join(tree);
    match fs::symlink_metadata(&root) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            snapshot.record_unreadable(
                format!("{tree}/"),
                format!(
                    "refusing to follow symlink {} during divergence comparison",
                    root.display()
                ),
            );
            return;
        }
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) | Err(_) => return, // absent (or a stray file the db sweep ignores)
    }
    walk_tree_into(location, &root, snapshot);
}

fn walk_tree_into(location: &Path, dir: &Path, snapshot: &mut RealmContentSnapshot) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            let relative = snapshot_relative(location, dir).unwrap_or_default();
            snapshot.record_unreadable(
                format!("{relative}/"),
                format!("cannot list {}: {error}", dir.display()),
            );
            return;
        }
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    paths.sort();
    for path in paths {
        let Some(relative) = snapshot_relative(location, &path) else {
            continue;
        };
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                walk_tree_into(location, &path, snapshot);
            }
            _ => digest_entry_into(&path, relative, snapshot),
        }
    }
}

/// Snapshot one copy: session rows first, then every authoritative file's
/// bytes, in one sequential pass. The caller's fence is what quiesces the
/// side; reading rows and bytes together per side is what makes the
/// snapshot represent a single moment under that fence.
fn snapshot_realm_content(location: &Path) -> RealmContentSnapshot {
    let mut snapshot = RealmContentSnapshot {
        sessions: Some(SessionDigests::new()),
        files: BTreeMap::new(),
        unreadable: Vec::new(),
        errors: Vec::new(),
    };

    let sessions_db = location.join("sessions.sqlite3");
    match fs::symlink_metadata(&sessions_db) {
        // A symlinked sessions database is never opened (the row-level SQL
        // would read the external target); the inventory sweep below records
        // the refusal, and `None` poisons the session comparison.
        Ok(metadata) if metadata.file_type().is_symlink() => snapshot.sessions = None,
        Ok(metadata) if metadata.is_file() => match session_digests(&sessions_db) {
            Ok(digests) => snapshot.sessions = Some(digests),
            Err(error) => {
                snapshot.sessions = None;
                snapshot.errors.push(format!(
                    "session divergence unavailable for {}: {error}",
                    sessions_db.display()
                ));
            }
        },
        _ => {}
    }

    let inventory = enumerate_realm_sqlite_inventory(location);
    for link in &inventory.symlinks {
        let Some(relative) = snapshot_relative(location, link) else {
            continue;
        };
        snapshot.record_unreadable(
            relative,
            format!(
                "refusing to follow symlink {} during divergence comparison",
                link.display()
            ),
        );
    }
    for db in inventory.files {
        let Some(relative) = snapshot_relative(location, &db) else {
            continue;
        };
        match file_digest(&db) {
            Ok(digest) => {
                snapshot.files.insert(relative, digest);
            }
            Err(error) => snapshot.record_unreadable(
                relative,
                format!("file digest unavailable for {}: {error}", db.display()),
            ),
        }
    }

    snapshot_jsonl_sessions(location, &mut snapshot);
    snapshot_tree(location, "blobs", &mut snapshot);
    snapshot_tree(location, "artifacts", &mut snapshot);
    snapshot
}

/// Compute the per-domain divergence report for one split-brain realm.
///
/// Sessions are compared row-level (id + content digest over the session
/// tables, read-only SQL); every other authoritative store — the SQLite
/// databases, the canonical `sessions_jsonl/*.jsonl` files, and the
/// `blobs/` / `artifacts/` trees — is compared at streamed file-digest
/// level. Read failures are folded into [`SplitBrainReport::errors`] AND
/// poison the affected entries as [`DivergenceStatus::Unknown`]: a failed
/// read must never manufacture an equal / only-in claim
/// ([`SplitBrainReport::comparison_is_conclusive`] is the archive-decision
/// gate).
///
/// Callers that act on the report (`--apply --adopt-root`) must hold the
/// [`RealmMaintenanceFence`] on every location for the whole
/// compare-to-archive interval: the fence excludes foreign guarded writers,
/// and each side is then read in a single sequential pass (rows, then
/// bytes) so the snapshot reflects one quiesced moment. Unfenced (dry-run)
/// reports are advisory.
pub fn compute_split_brain_report(realm: &str, locations: &[PathBuf]) -> SplitBrainReport {
    let mut report = SplitBrainReport::new(realm, locations.to_vec());

    let snapshots: Vec<(PathBuf, RealmContentSnapshot)> = locations
        .iter()
        .map(|location| (location.clone(), snapshot_realm_content(location)))
        .collect();
    for (_, snapshot) in &snapshots {
        report.errors.extend(snapshot.errors.iter().cloned());
    }

    // Row-level session comparison. One unreadable sessions database
    // poisons every session entry (Unknown, never equal / only-in).
    let sessions_poisoned = snapshots
        .iter()
        .any(|(_, snapshot)| snapshot.sessions.is_none());
    let mut all_ids: Vec<String> = snapshots
        .iter()
        .filter_map(|(_, snapshot)| snapshot.sessions.as_ref())
        .flat_map(|digests| digests.keys().cloned())
        .collect();
    all_ids.sort();
    all_ids.dedup();
    for session_id in all_ids {
        if sessions_poisoned {
            report.sessions.push(SessionDivergenceEntry {
                session_id,
                status: DivergenceStatus::Unknown,
            });
            continue;
        }
        let holders: Vec<(&PathBuf, &[u8; 32])> = snapshots
            .iter()
            .filter_map(|(location, snapshot)| {
                snapshot
                    .sessions
                    .as_ref()?
                    .get(&session_id)
                    .map(|digest| (location, digest))
            })
            .collect();
        if holders.len() == 1 {
            report.sessions.push(SessionDivergenceEntry {
                session_id,
                status: DivergenceStatus::OnlyIn {
                    location: holders[0].0.clone(),
                },
            });
        } else if holders.len() == snapshots.len()
            && holders.iter().all(|(_, digest)| *digest == holders[0].1)
        {
            report.sessions_equal += 1;
        } else {
            report.sessions.push(SessionDivergenceEntry {
                session_id,
                status: DivergenceStatus::Divergent,
            });
        }
    }

    // Per-file comparison across every authoritative store.
    let mut relative_files: Vec<String> = snapshots
        .iter()
        .flat_map(|(_, snapshot)| {
            snapshot.files.keys().cloned().chain(
                snapshot
                    .unreadable
                    .iter()
                    .filter(|entry| !entry.ends_with('/'))
                    .cloned(),
            )
        })
        .collect();
    relative_files.sort();
    relative_files.dedup();
    for relative in relative_files {
        if snapshots
            .iter()
            .any(|(_, snapshot)| snapshot.poisons(&relative))
        {
            report.files.push(FileDivergenceEntry {
                file: relative,
                status: DivergenceStatus::Unknown,
            });
            continue;
        }
        let holders: Vec<(&PathBuf, &[u8; 32])> = snapshots
            .iter()
            .filter_map(|(location, snapshot)| {
                snapshot
                    .files
                    .get(&relative)
                    .map(|digest| (location, digest))
            })
            .collect();
        let status = if holders.len() == 1 {
            DivergenceStatus::OnlyIn {
                location: holders[0].0.clone(),
            }
        } else if holders.len() == snapshots.len()
            && holders.iter().all(|(_, digest)| *digest == holders[0].1)
        {
            DivergenceStatus::Equal
        } else {
            DivergenceStatus::Divergent
        };
        report.files.push(FileDivergenceEntry {
            file: relative,
            status,
        });
    }

    report
}

// ─────────────────────────────────────────────────────────────────────────
// Prune (registered backup-artifact lifecycle).
// ─────────────────────────────────────────────────────────────────────────

fn recursive_size(path: &Path) -> u64 {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return 0;
    };
    if metadata.is_dir() {
        let Ok(entries) = fs::read_dir(path) else {
            return 0;
        };
        entries
            .filter_map(Result::ok)
            .map(|entry| recursive_size(&entry.path()))
            .sum()
    } else {
        metadata.len()
    }
}

/// The Unix timestamp a registered artifact name embeds (backup names carry
/// `.pre-<version>-<ts>[.purpose]`, quarantines `.corrupt-<ts>`). `None`
/// for names outside the registered patterns.
pub fn registered_artifact_timestamp(name: &str) -> Option<u64> {
    if is_backup_artifact_name(name) {
        let idx = name.rfind(".pre-")?;
        let suffix = &name[idx + ".pre-".len()..];
        let (_, rest) = suffix.split_once('-')?;
        let timestamp = rest
            .split_once('.')
            .map_or(rest, |(timestamp, _)| timestamp);
        return timestamp.parse().ok();
    }
    if is_quarantine_artifact_name(name) {
        let idx = name.rfind(".corrupt-")?;
        return name[idx + ".corrupt-".len()..].parse().ok();
    }
    None
}

/// Age of one registered artifact, from the timestamp its name registered
/// at archive time. `fs::rename` preserves the source's mtime, so a
/// long-idle file archived today would look 30+ days old to an
/// mtime-based clock and be prunable immediately; the registered name is
/// the archival record. Filesystem mtime is only the fallback for
/// registered names whose timestamp overflows.
fn age_days(path: &Path, name: &str) -> u64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    if let Some(registered) = registered_artifact_timestamp(name) {
        return now.saturating_sub(registered) / 86_400;
    }
    fs::symlink_metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .map(|age| age.as_secs() / 86_400)
        .unwrap_or(0)
}

fn artifact_kind(name: &str) -> Option<PruneArtifactKind> {
    if is_backup_artifact_name(name) {
        Some(PruneArtifactKind::BackupArtifact)
    } else if is_quarantine_artifact_name(name) {
        Some(PruneArtifactKind::QuarantinedIndex)
    } else {
        None
    }
}

/// The original name a registered artifact was archived from (the part
/// before the registered `.pre-` / `.corrupt-` suffix). `None` for names
/// outside the registered patterns.
pub fn registered_artifact_original(name: &str) -> Option<&str> {
    if is_backup_artifact_name(name) {
        return Some(&name[..name.rfind(".pre-")?]);
    }
    if is_quarantine_artifact_name(name) {
        return Some(&name[..name.rfind(".corrupt-")?]);
    }
    None
}

fn push_artifacts_in(
    dir: &Path,
    dirs_too: bool,
    original_filter: Option<&str>,
    artifacts: &mut Vec<PruneArtifact>,
) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    paths.sort();
    for path in paths {
        if path.is_dir() && !dirs_too {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(kind) = artifact_kind(name) else {
            continue;
        };
        // Quarantine artifacts are owned index FILES; a directory (or
        // symlink) wearing the `.corrupt-*` name is not one and must never
        // enter prune's deletion authority.
        if kind == PruneArtifactKind::QuarantinedIndex
            && !fs::symlink_metadata(&path)
                .map(|metadata| metadata.is_file())
                .unwrap_or(false)
        {
            continue;
        }
        if let Some(filter) = original_filter
            && registered_artifact_original(name) != Some(filter)
        {
            continue;
        }
        artifacts.push(PruneArtifact {
            bytes: recursive_size(&path),
            age_days: age_days(&path, name),
            path,
            kind,
            action: PruneAction::Kept,
        });
    }
}

/// Enumerate every registered maintenance artifact under the swept state
/// roots: root-level `*.pre-*` archives (files or whole archived realm
/// directories) and, inside each realm directory, `*.pre-*` backups and
/// `*.corrupt-*` quarantines next to the databases (realm root, `memory/`,
/// `sessions_jsonl/`, `mobs/`). Nothing outside these naming patterns is
/// ever returned — prune's deletion authority is exactly this listing.
pub fn enumerate_maintenance_artifacts(state_roots: &[PathBuf]) -> Vec<PruneArtifact> {
    enumerate_maintenance_artifacts_filtered(state_roots, None)
}

/// [`enumerate_maintenance_artifacts`] scoped to one realm. Realm-directory
/// artifacts are returned only for realms whose manifest identity matches;
/// root-level archives are returned only when they were archived from the
/// realm's directory name (whole-realm split-brain archives). Root-level
/// file archives that carry no realm identity are excluded under a filter —
/// a scoped prune must never delete another realm's preserved copy.
pub fn enumerate_maintenance_artifacts_filtered(
    state_roots: &[PathBuf],
    realm_filter: Option<&str>,
) -> Vec<PruneArtifact> {
    let realm_dir_name = realm_filter.map(meerkat_core::sanitize_realm_id);
    let mut artifacts = Vec::new();
    let mut seen_roots: Vec<PathBuf> = Vec::new();
    for root in state_roots {
        let canonical = fs::canonicalize(root).unwrap_or_else(|_| root.clone());
        if seen_roots.contains(&canonical) {
            continue;
        }
        seen_roots.push(canonical);

        // Root level: archived realm directories and archived files.
        push_artifacts_in(root, true, realm_dir_name.as_deref(), &mut artifacts);

        // Inside each live realm directory (backup artifacts inside archived
        // copies are part of the archive, owned by the archive's own entry).
        for realm in list_realm_dirs(root) {
            if let Some(filter) = realm_filter
                && realm.realm_id != filter
            {
                continue;
            }
            for scan_dir in [
                realm.dir.clone(),
                realm.dir.join("memory"),
                realm.dir.join("sessions_jsonl"),
                realm.dir.join("mobs"),
            ] {
                push_artifacts_in(&scan_dir, false, None, &mut artifacts);
            }
        }
    }
    artifacts
}

/// Delete one registered artifact (file or directory), restoring write
/// permissions first (archives are stored read-only). Refuses paths whose
/// name does not match the registered patterns — prune never touches
/// anything else, even if asked.
pub fn remove_maintenance_artifact(path: &Path) -> Result<(), StoreError> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if artifact_kind(name).is_none() {
        return Err(StoreError::Internal(format!(
            "refusing to remove '{}': not a registered maintenance artifact \
             (*.pre-* / *.corrupt-*)",
            path.display()
        )));
    }
    restore_write_permissions_best_effort(path);
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {

    #[test]
    fn artifact_name_validation_rejects_lookalikes() {
        // Full-shape validation: a mere ".pre-" substring is not registered.
        for lookalike in [
            "notes.pre-release",
            "config.pre-prod",
            ".pre-0.8.3-1700000000",
            "db.pre-0.8.3-notadigit",
            "db.pre-083-1700000000",
            "db.pre-0.8.3-1700000000.",
            "db.pre-0.8.3-1700000000.bad purpose",
        ] {
            assert!(!is_backup_artifact_name(lookalike), "{lookalike}");
        }
        for valid in [
            "sessions.sqlite3.pre-0.8.3-1700000000",
            "sessions.sqlite3.pre-0.8.3-1700000000.split-brain",
            "realm-dir.pre-10.20.30-1.adopt_other",
        ] {
            assert!(is_backup_artifact_name(valid), "{valid}");
        }
        for lookalike in ["notes.corrupt-ish", ".corrupt-123", "x.corrupt-12a"] {
            assert!(!is_quarantine_artifact_name(lookalike), "{lookalike}");
        }
        assert!(is_quarantine_artifact_name(
            "session_index.sqlite3.corrupt-1700000000"
        ));
    }
    use super::*;
    use rusqlite::Connection;

    fn create_db(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let conn = Connection::open(path).unwrap();
        conn.execute_batch("CREATE TABLE t (x INTEGER)").unwrap();
    }

    fn write_manifest(state_root: &Path, realm_id: &str, backend: &str) -> PathBuf {
        let dir = state_root.join(meerkat_core::sanitize_realm_id(realm_id));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(REALM_MANIFEST_FILE_NAME),
            serde_json::to_vec(&serde_json::json!({
                "realm_id": realm_id,
                "backend": backend,
                "origin": "explicit",
                "created_at": "0",
            }))
            .unwrap(),
        )
        .unwrap();
        dir
    }

    #[test]
    fn fence_acquires_full_inventory_and_mobs_in_sorted_order() {
        let temp = tempfile::tempdir().unwrap();
        let realm = temp.path().join("realm");
        create_db(&realm.join("sessions.sqlite3"));
        create_db(&realm.join("workgraph.sqlite3"));
        create_db(&realm.join("memory/memory.sqlite3"));
        create_db(&realm.join("mobs/alpha.db"));
        create_db(&realm.join("mobs/realm_profiles.db"));
        // Non-database files are not fenced.
        fs::write(realm.join("notes.txt"), b"x").unwrap();

        let fence = RealmMaintenanceFence::acquire(&realm, Duration::from_secs(1)).unwrap();
        // Full fixed inventory (whether or not the file exists) + mob dbs.
        assert_eq!(fence.len(), REALM_SQLITE_FILES.len() + 2);
        assert!(!fence.is_empty());
        let mut expected: Vec<PathBuf> = REALM_SQLITE_FILES
            .iter()
            .map(|relative| realm.join(relative))
            .collect();
        expected.push(realm.join("mobs/alpha.db"));
        expected.push(realm.join("mobs/realm_profiles.db"));
        expected.sort();
        assert_eq!(fence.fenced_databases(), expected.as_slice());
        // Every fence lock file exists — including for databases that do
        // not exist yet (the fence file is a sibling lock).
        for database in fence.fenced_databases() {
            assert!(meerkat_sqlite::fence_lock_path(database).is_file());
        }
        assert!(!realm.join("tasks.db").exists());
        // The realm-level write-admission fence is held too.
        assert!(fence.admission_lock_path().ends_with("realm.mfence"));
        assert!(fence.admission_lock_path().is_file());
    }

    #[test]
    fn foreign_admission_holder_blocks_fence_acquisition() {
        let temp = tempfile::tempdir().unwrap();
        let realm = temp.path().join("realm");
        create_db(&realm.join("sessions.sqlite3"));

        // A FOREIGN process holding the write-admission fence (raw exclusive
        // lock, no in-process holder registry entry) blocks the realm fence.
        let admission_lock = meerkat_sqlite::fence_lock_path(&realm_write_admission_target(&realm));
        let foreign = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&admission_lock)
            .unwrap();
        foreign.try_lock().unwrap();

        let error = RealmMaintenanceFence::acquire(&realm, Duration::from_millis(100))
            .expect_err("foreign admission holder must fail acquisition");
        assert!(
            matches!(error, StoreError::MaintenanceFenceHeld { ref path }
                if path.ends_with(REALM_WRITE_ADMISSION_STEM)),
            "{error:?}"
        );
        drop(foreign);
        RealmMaintenanceFence::acquire(&realm, Duration::from_secs(1)).unwrap();
    }

    #[test]
    fn foreign_holder_fails_typed_and_releases_partial_acquisition() {
        let temp = tempfile::tempdir().unwrap();
        let realm = temp.path().join("realm");
        create_db(&realm.join("sessions.sqlite3"));
        create_db(&realm.join("workgraph.sqlite3"));

        // Simulate a FOREIGN process holding the second fence: a raw
        // exclusive lock without the in-process holder registry entry.
        let foreign_lock = meerkat_sqlite::fence_lock_path(&realm.join("workgraph.sqlite3"));
        let foreign = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&foreign_lock)
            .unwrap();
        foreign.try_lock().unwrap();

        let error = RealmMaintenanceFence::acquire(&realm, Duration::from_millis(100))
            .expect_err("foreign fence holder must fail acquisition");
        assert!(
            matches!(error, StoreError::MaintenanceFenceHeld { ref path }
                if path.ends_with("workgraph.sqlite3")),
            "{error:?}"
        );

        // RAII: the earlier fences (admission, sessions) were released on
        // failure — fresh exclusive acquisitions succeed immediately.
        let reacquired =
            meerkat_sqlite::ExclusiveFence::try_acquire(&realm.join("sessions.sqlite3")).unwrap();
        assert!(reacquired.is_some(), "partial acquisition must be released");
        drop(reacquired);
        let admission =
            meerkat_sqlite::ExclusiveFence::try_acquire(&realm_write_admission_target(&realm))
                .unwrap();
        assert!(admission.is_some(), "admission fence must be released");
        drop(admission);
        drop(foreign);

        // With the foreign lock gone, full acquisition succeeds.
        let fence = RealmMaintenanceFence::acquire(&realm, Duration::from_secs(1)).unwrap();
        assert_eq!(fence.len(), REALM_SQLITE_FILES.len());
    }

    #[test]
    fn empty_realm_still_fences_the_full_fixed_inventory() {
        let temp = tempfile::tempdir().unwrap();
        let fence = RealmMaintenanceFence::acquire(temp.path(), Duration::ZERO).unwrap();
        // A database created during maintenance is excluded by its sibling
        // fence, so the fixed inventory is fenced even when no file exists.
        assert_eq!(fence.len(), REALM_SQLITE_FILES.len());
        assert!(!fence.is_empty());
        for relative in REALM_SQLITE_FILES {
            let lock = meerkat_sqlite::fence_lock_path(&temp.path().join(relative));
            assert!(lock.is_file(), "{}", lock.display());
        }
    }

    #[test]
    fn backup_names_follow_the_registered_discipline() {
        let name = backup_artifact_name("sessions.sqlite3", "split-brain");
        assert!(
            name.starts_with(&format!(
                "sessions.sqlite3.pre-{}-",
                env!("CARGO_PKG_VERSION")
            )),
            "{name}"
        );
        assert!(name.ends_with(".split-brain"), "{name}");
        assert!(is_backup_artifact_name(&name));
        let bare = backup_artifact_name("team", "");
        assert!(is_backup_artifact_name(&bare));
        assert!(!bare.ends_with('.'), "{bare}");
        assert!(is_quarantine_artifact_name(
            "session_index.sqlite3.corrupt-1"
        ));
        assert!(!is_backup_artifact_name("sessions.sqlite3"));
    }

    #[test]
    fn archive_renames_and_strips_write_permission() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("team");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("sessions.sqlite3"), b"data").unwrap();

        let archive = archive_path_read_only(&dir, "split-brain").unwrap();
        assert!(!dir.exists(), "original must be gone");
        assert!(archive.is_dir());
        assert!(archive.join("sessions.sqlite3").is_file());
        let name = archive.file_name().unwrap().to_str().unwrap();
        assert!(is_backup_artifact_name(name), "{name}");
        let inner = fs::metadata(archive.join("sessions.sqlite3")).unwrap();
        assert!(inner.permissions().readonly(), "archive must be read-only");

        // Prune can remove it (write permission restored first).
        remove_maintenance_artifact(&archive).unwrap();
        assert!(!archive.exists());
    }

    #[test]
    fn archive_reported_returns_archive_path_and_no_warnings_on_success() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("sessions.sqlite3");
        fs::write(&file, b"data").unwrap();

        let archived = archive_path_read_only_reported(&file, "split-brain").unwrap();
        assert!(archived.archive.is_file());
        assert!(
            archived.warnings.is_empty(),
            "successful hardening + parent fsync must not warn: {:?}",
            archived.warnings
        );
        let name = archived.archive.file_name().unwrap().to_str().unwrap();
        assert!(is_backup_artifact_name(name), "{name}");
        remove_maintenance_artifact(&archived.archive).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn archive_hardening_never_chmods_through_symlinks() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let external = temp.path().join("external.txt");
        fs::write(&external, b"do not chmod").unwrap();
        let dir = temp.path().join("team");
        fs::create_dir_all(&dir).unwrap();
        std::os::unix::fs::symlink(&external, dir.join("link")).unwrap();

        let archived = archive_path_read_only_reported(&dir, "split-brain").unwrap();
        let mode = fs::metadata(&external).unwrap().permissions().mode();
        assert!(
            mode & 0o200 != 0,
            "external symlink target must stay writable (mode {mode:o})"
        );
        // Restore-then-delete must not chmod through the link either.
        remove_maintenance_artifact(&archived.archive).unwrap();
        let mode = fs::metadata(&external).unwrap().permissions().mode();
        assert!(mode & 0o200 != 0, "mode {mode:o}");
        assert!(external.is_file());
    }

    #[test]
    fn remove_refuses_unregistered_paths() {
        let temp = tempfile::tempdir().unwrap();
        let victim = temp.path().join("precious.txt");
        fs::write(&victim, b"do not touch").unwrap();
        let error = remove_maintenance_artifact(&victim).expect_err("must refuse");
        assert!(matches!(error, StoreError::Internal(_)));
        assert!(victim.is_file(), "unregistered path must survive");
    }

    #[test]
    fn split_brain_divergence_classifies_sessions_and_files() {
        let temp = tempfile::tempdir().unwrap();
        let dir_a = temp.path().join("a/team");
        let dir_b = temp.path().join("b/team");
        for dir in [&dir_a, &dir_b] {
            fs::create_dir_all(dir).unwrap();
        }
        let ddl = "CREATE TABLE sessions (
            session_id TEXT PRIMARY KEY,
            created_at_ms INTEGER NOT NULL,
            updated_at_ms INTEGER NOT NULL,
            message_count INTEGER NOT NULL,
            total_tokens INTEGER NOT NULL,
            metadata_json TEXT NOT NULL,
            session_json BLOB NOT NULL
        )";
        let insert = |conn: &Connection, id: &str, body: &str| {
            conn.execute(
                "INSERT INTO sessions VALUES (?1, 0, 0, 0, 0, '{}', ?2)",
                rusqlite::params![id, body.as_bytes()],
            )
            .unwrap();
        };
        let shared = "00000000-0000-4000-8000-000000000001";
        let divergent = "00000000-0000-4000-8000-000000000002";
        let only_a = "00000000-0000-4000-8000-000000000003";
        {
            let conn = Connection::open(dir_a.join("sessions.sqlite3")).unwrap();
            conn.execute_batch(ddl).unwrap();
            insert(&conn, shared, "same");
            insert(&conn, divergent, "version-a");
            insert(&conn, only_a, "solo");
        }
        {
            let conn = Connection::open(dir_b.join("sessions.sqlite3")).unwrap();
            conn.execute_batch(ddl).unwrap();
            insert(&conn, shared, "same");
            insert(&conn, divergent, "version-b");
        }
        // A file present only under B.
        create_db(&dir_b.join("workgraph.sqlite3"));

        let report = compute_split_brain_report("team", &[dir_a.clone(), dir_b.clone()]);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(report.sessions_equal, 1);
        let status_of = |id: &str| {
            report
                .sessions
                .iter()
                .find(|entry| entry.session_id == id)
                .map(|entry| entry.status.clone())
        };
        assert_eq!(status_of(divergent), Some(DivergenceStatus::Divergent));
        assert_eq!(
            status_of(only_a),
            Some(DivergenceStatus::OnlyIn { location: dir_a })
        );
        assert_eq!(
            status_of(shared),
            None,
            "equal sessions are counted, not listed"
        );
        let workgraph = report
            .files
            .iter()
            .find(|entry| entry.file == "workgraph.sqlite3")
            .expect("workgraph file entry");
        assert_eq!(
            workgraph.status,
            DivergenceStatus::OnlyIn { location: dir_b }
        );
        let sessions_file = report
            .files
            .iter()
            .find(|entry| entry.file == "sessions.sqlite3")
            .expect("sessions file entry");
        assert_eq!(sessions_file.status, DivergenceStatus::Divergent);
        assert!(report.comparison_is_conclusive());
    }

    #[test]
    fn split_brain_covers_jsonl_blobs_and_artifacts() {
        let temp = tempfile::tempdir().unwrap();
        let dir_a = temp.path().join("a/team");
        let dir_b = temp.path().join("b/team");
        for dir in [&dir_a, &dir_b] {
            fs::create_dir_all(dir.join("sessions_jsonl")).unwrap();
            fs::create_dir_all(dir.join("artifacts")).unwrap();
        }
        // Canonical JSONL session files: one equal, one divergent.
        for dir in [&dir_a, &dir_b] {
            fs::write(dir.join("sessions_jsonl/equal.jsonl"), b"{\"same\":1}").unwrap();
        }
        fs::write(dir_a.join("sessions_jsonl/split.jsonl"), b"version-a").unwrap();
        fs::write(dir_b.join("sessions_jsonl/split.jsonl"), b"version-b").unwrap();
        // A blob present only under A (nested shard directory).
        fs::create_dir_all(dir_a.join("blobs/ab")).unwrap();
        fs::write(dir_a.join("blobs/ab/abcd.json"), b"blob-bytes").unwrap();
        // An artifact record that diverges.
        fs::write(dir_a.join("artifacts/r1.json"), b"{\"v\":1}").unwrap();
        fs::write(dir_b.join("artifacts/r1.json"), b"{\"v\":2}").unwrap();

        let report = compute_split_brain_report("team", &[dir_a.clone(), dir_b]);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        let status_of = |file: &str| {
            report
                .files
                .iter()
                .find(|entry| entry.file == file)
                .map(|entry| entry.status.clone())
        };
        assert_eq!(
            status_of("sessions_jsonl/equal.jsonl"),
            Some(DivergenceStatus::Equal)
        );
        assert_eq!(
            status_of("sessions_jsonl/split.jsonl"),
            Some(DivergenceStatus::Divergent)
        );
        assert_eq!(
            status_of("blobs/ab/abcd.json"),
            Some(DivergenceStatus::OnlyIn { location: dir_a })
        );
        assert_eq!(
            status_of("artifacts/r1.json"),
            Some(DivergenceStatus::Divergent)
        );
        assert!(report.comparison_is_conclusive());
    }

    #[test]
    fn split_brain_read_failure_poisons_the_session_comparison() {
        let temp = tempfile::tempdir().unwrap();
        let dir_a = temp.path().join("a/team");
        let dir_b = temp.path().join("b/team");
        for dir in [&dir_a, &dir_b] {
            fs::create_dir_all(dir).unwrap();
        }
        // Copy A's sessions database is unreadable garbage; copy B holds a
        // real session row. B's row must classify Unknown, never OnlyIn —
        // a failed read is not an empty side.
        fs::write(dir_a.join("sessions.sqlite3"), b"this is not sqlite").unwrap();
        {
            let conn = Connection::open(dir_b.join("sessions.sqlite3")).unwrap();
            conn.execute_batch(
                "CREATE TABLE sessions (
                    session_id TEXT PRIMARY KEY,
                    created_at_ms INTEGER NOT NULL,
                    updated_at_ms INTEGER NOT NULL,
                    message_count INTEGER NOT NULL,
                    total_tokens INTEGER NOT NULL,
                    metadata_json TEXT NOT NULL,
                    session_json BLOB NOT NULL
                );
                INSERT INTO sessions VALUES
                    ('00000000-0000-4000-8000-000000000001', 0, 0, 0, 0, '{}', X'AA');",
            )
            .unwrap();
        }

        let report = compute_split_brain_report("team", &[dir_a, dir_b]);
        assert!(!report.errors.is_empty());
        assert_eq!(report.sessions_equal, 0);
        assert_eq!(report.sessions.len(), 1);
        assert_eq!(report.sessions[0].status, DivergenceStatus::Unknown);
        assert!(!report.comparison_is_conclusive());
    }

    #[cfg(unix)]
    #[test]
    fn split_brain_never_follows_symlinks_and_marks_them_unknown() {
        let temp = tempfile::tempdir().unwrap();
        let outside = temp.path().join("outside.json");
        fs::write(&outside, b"external bytes").unwrap();
        let dir_a = temp.path().join("a/team");
        let dir_b = temp.path().join("b/team");
        for dir in [&dir_a, &dir_b] {
            fs::create_dir_all(dir.join("blobs")).unwrap();
        }
        std::os::unix::fs::symlink(&outside, dir_a.join("blobs/link.json")).unwrap();
        fs::write(dir_b.join("blobs/link.json"), b"external bytes").unwrap();

        let report = compute_split_brain_report("team", &[dir_a, dir_b]);
        assert!(!report.errors.is_empty());
        let entry = report
            .files
            .iter()
            .find(|entry| entry.file == "blobs/link.json")
            .expect("symlinked entry");
        assert_eq!(entry.status, DivergenceStatus::Unknown);
        assert!(!report.comparison_is_conclusive());
    }

    /// A symlinked database in the fixed inventory must never be digested
    /// through to its external target: the entry is refused (Unknown), the
    /// refusal is a per-realm error, and for `sessions.sqlite3` the
    /// row-level session comparison is poisoned too.
    #[cfg(unix)]
    #[test]
    fn split_brain_never_digests_through_symlinked_databases() {
        let temp = tempfile::tempdir().unwrap();
        let outside = temp.path().join("outside.sqlite3");
        {
            let conn = Connection::open(&outside).unwrap();
            conn.execute_batch(
                "CREATE TABLE sessions (
                    session_id TEXT PRIMARY KEY,
                    created_at_ms INTEGER NOT NULL,
                    updated_at_ms INTEGER NOT NULL,
                    message_count INTEGER NOT NULL,
                    total_tokens INTEGER NOT NULL,
                    metadata_json TEXT NOT NULL,
                    session_json BLOB NOT NULL
                );
                INSERT INTO sessions VALUES
                    ('00000000-0000-4000-8000-000000000001', 0, 0, 0, 0, '{}', X'AA');",
            )
            .unwrap();
        }
        let dir_a = temp.path().join("a/team");
        let dir_b = temp.path().join("b/team");
        fs::create_dir_all(&dir_a).unwrap();
        fs::create_dir_all(&dir_b).unwrap();
        std::os::unix::fs::symlink(&outside, dir_a.join("sessions.sqlite3")).unwrap();
        fs::copy(&outside, dir_b.join("sessions.sqlite3")).unwrap();

        let inventory = enumerate_realm_sqlite_inventory(&dir_a);
        assert!(inventory.files.is_empty());
        assert_eq!(inventory.symlinks, vec![dir_a.join("sessions.sqlite3")]);
        assert!(enumerate_realm_sqlite_files(&dir_a).is_empty());

        let report = compute_split_brain_report("team", &[dir_a, dir_b]);
        assert!(
            report
                .errors
                .iter()
                .any(|error| error.contains("refusing to follow symlink")),
            "{:?}",
            report.errors
        );
        let entry = report
            .files
            .iter()
            .find(|entry| entry.file == "sessions.sqlite3")
            .expect("sessions database entry");
        assert_eq!(entry.status, DivergenceStatus::Unknown);
        // The poisoned side must also poison the row-level comparison.
        assert_eq!(report.sessions.len(), 1, "{:?}", report.sessions);
        assert_eq!(report.sessions[0].status, DivergenceStatus::Unknown);
        assert!(!report.comparison_is_conclusive());
    }

    #[test]
    fn ledger_baseline_surfaces_read_failures_and_future_versions() {
        let temp = tempfile::tempdir().unwrap();
        let realm = temp.path().join("team");
        fs::create_dir_all(realm.join("sessions_jsonl")).unwrap();
        // An unreadable database must surface as an error, never as a
        // missing ledger a dry-run could report as would-stamp.
        fs::write(realm.join("tasks.db"), b"this is not sqlite").unwrap();
        // A future jsonl-index version must be reported as a refusal in
        // dry-run exactly as apply's guarded constructor would refuse it.
        {
            let conn =
                Connection::open(realm.join("sessions_jsonl/session_index.sqlite3")).unwrap();
            conn.execute_batch(
                "CREATE TABLE meerkat_schema (domain TEXT PRIMARY KEY, version INTEGER NOT NULL);",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO meerkat_schema VALUES ('jsonl-index', ?1)",
                [i64::MAX],
            )
            .unwrap();
        }
        // A ledger-less database is a plain would-stamp row, not an error.
        create_db(&realm.join("runtime.sqlite3"));

        let baseline = read_realm_ledger_baseline(&realm);
        assert_eq!(baseline.errors.len(), 1, "{:?}", baseline.errors);
        assert!(
            baseline.errors[0].contains("tasks.db"),
            "{:?}",
            baseline.errors
        );
        assert!(
            !baseline
                .rows
                .iter()
                .any(|row| row.database.ends_with("tasks.db")),
            "an unreadable database must contribute no ledger rows"
        );
        assert_eq!(baseline.future.len(), 1);
        assert_eq!(baseline.future[0].domain, "jsonl-index");
        assert_eq!(baseline.future[0].found, i64::MAX);
        assert_eq!(
            baseline.future[0].supported,
            crate::index::JSONL_INDEX_DOMAIN.supported_version()
        );
        let runtime_row = baseline
            .rows
            .iter()
            .find(|row| row.database.ends_with("runtime.sqlite3"))
            .expect("runtime row");
        assert_eq!(runtime_row.domain, "runtime-store");
        assert_eq!(runtime_row.version, None);
    }

    #[test]
    fn prune_enumeration_sees_only_registered_patterns() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("realms");
        let realm_dir = write_manifest(&root, "artifacts", "sqlite");
        fs::write(
            realm_dir.join("sessions.sqlite3.pre-0.0.1-1700000000"),
            b"backup",
        )
        .unwrap();
        let jsonl = realm_dir.join("sessions_jsonl");
        fs::create_dir_all(&jsonl).unwrap();
        fs::write(jsonl.join("session_index.sqlite3.corrupt-42"), b"q").unwrap();
        // Root-level archived realm directory.
        let archived = root.join("team.pre-0.0.1-1700000000.split-brain");
        fs::create_dir_all(&archived).unwrap();
        fs::write(archived.join("sessions.sqlite3"), b"old").unwrap();
        // Distractors that must never appear.
        fs::write(realm_dir.join("sessions.sqlite3"), b"live").unwrap();
        fs::write(realm_dir.join("notes.txt"), b"keep me").unwrap();

        let artifacts = enumerate_maintenance_artifacts(&[root.clone(), root]);
        let mut names: Vec<String> = artifacts
            .iter()
            .map(|artifact| {
                artifact
                    .path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "session_index.sqlite3.corrupt-42".to_string(),
                "sessions.sqlite3.pre-0.0.1-1700000000".to_string(),
                "team.pre-0.0.1-1700000000.split-brain".to_string(),
            ],
            "duplicate roots must not double-count"
        );
        let archived_entry = artifacts
            .iter()
            .find(|artifact| artifact.path == archived)
            .expect("archived dir entry");
        assert_eq!(archived_entry.kind, PruneArtifactKind::BackupArtifact);
        assert_eq!(archived_entry.bytes, 3);
    }

    #[test]
    fn quarantine_artifacts_must_be_files_even_at_root_level() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("realms");
        fs::create_dir_all(&root).unwrap();
        // Quarantine names belong to owned index FILES; a directory wearing
        // the name must not enter prune's deletion authority even at the
        // root level, where archived realm DIRECTORIES (backup naming) are
        // legitimately enumerated.
        fs::create_dir_all(root.join("data.corrupt-1700000000")).unwrap();
        fs::write(root.join("index.sqlite3.corrupt-1700000000"), b"q").unwrap();

        let artifacts = enumerate_maintenance_artifacts(&[root]);
        let names: Vec<&str> = artifacts
            .iter()
            .filter_map(|artifact| artifact.path.file_name().and_then(|name| name.to_str()))
            .collect();
        assert_eq!(names, vec!["index.sqlite3.corrupt-1700000000"]);
    }

    #[test]
    fn report_shapes_round_trip_through_json() {
        let mut report = MigrateReport {
            mode: MigrateMode::Apply,
            ..MigrateReport::default()
        };
        let mut realm = RealmMigrateReport::new("team", PathBuf::from("/roots/a/team"));
        realm.backend = Some("sqlite".to_string());
        realm.ledger.push(LedgerBaselineEntry {
            database: PathBuf::from("/roots/a/team/sessions.sqlite3"),
            domain: "session-store".to_string(),
            before: None,
            after: Some(1),
            action: LedgerBaselineAction::Stamped,
        });
        realm.adoption = Some(CheckpointAdoptionOutcome {
            failures: Vec::new(),
            scanned: 3,
            already_verified: 1,
            adopted: 2,
            legacy_pending: 0,
            refused: vec![],
            skipped: None,
        });
        report.realms.push(realm);
        report.split_brain.push(SplitBrainReport {
            realm: "team".to_string(),
            locations: vec![
                PathBuf::from("/roots/a/team"),
                PathBuf::from("/roots/b/team"),
            ],
            sessions_equal: 4,
            sessions: vec![SessionDivergenceEntry {
                session_id: "s".to_string(),
                status: DivergenceStatus::Divergent,
            }],
            files: vec![],
            resolution: SplitBrainResolution::Archived {
                adopted: PathBuf::from("/roots/a/team"),
                archived: vec![PathBuf::from("/roots/b/team.pre-0.8.3-1-split-brain")],
            },
            errors: vec![],
        });
        report.split_brain.push(SplitBrainReport {
            realm: "solo".to_string(),
            locations: vec![PathBuf::from("/roots/a/solo")],
            sessions_equal: 0,
            sessions: vec![SessionDivergenceEntry {
                session_id: "s2".to_string(),
                status: DivergenceStatus::Unknown,
            }],
            files: vec![],
            resolution: SplitBrainResolution::ArchiveFailed {
                adopted: PathBuf::from("/roots/a/solo"),
                archived: vec![PathBuf::from("/roots/b/solo.pre-0.8.3-1.split-brain")],
                reason: "rename failed".to_string(),
            },
            errors: vec!["sessions unreadable".to_string()],
        });
        let json = serde_json::to_string(&report).expect("serialize");
        let parsed: MigrateReport = serde_json::from_str(&json).expect("deserialize");
        assert!(matches!(parsed.mode, MigrateMode::Apply));
        assert_eq!(parsed.realms.len(), 1);
        assert_eq!(
            parsed.realms[0].ledger[0].action,
            LedgerBaselineAction::Stamped
        );
        assert!(matches!(
            parsed.split_brain[0].resolution,
            SplitBrainResolution::Archived { .. }
        ));
        // Partial-archive vocabulary round-trips: successes stay visible
        // next to the failure, and Unknown entries mark the report
        // inconclusive.
        assert!(matches!(
            &parsed.split_brain[1].resolution,
            SplitBrainResolution::ArchiveFailed { archived, .. } if archived.len() == 1
        ));
        assert!(!parsed.split_brain[1].comparison_is_conclusive());
        assert!(parsed.split_brain[0].comparison_is_conclusive());
        assert!(!parsed.has_errors());

        // Forward compatibility: unknown fields tolerated, defaults fill in.
        let sparse: PruneReport = serde_json::from_str(r#"{"future_field":1}"#).expect("sparse");
        assert!(matches!(sparse.mode, MigrateMode::DryRun));
        assert!(sparse.artifacts.is_empty());
    }

    #[test]
    fn census_counts_legacy_rows() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("sessions.sqlite3");
        {
            let conn = Connection::open(&db).unwrap();
            conn.execute_batch(
                "CREATE TABLE sessions (
                    session_id TEXT PRIMARY KEY,
                    created_at_ms INTEGER NOT NULL,
                    updated_at_ms INTEGER NOT NULL,
                    message_count INTEGER NOT NULL,
                    total_tokens INTEGER NOT NULL,
                    metadata_json TEXT NOT NULL,
                    session_json BLOB NOT NULL
                )",
            )
            .unwrap();
            let session = meerkat_core::Session::new();
            conn.execute(
                "INSERT INTO sessions VALUES (?1, 0, 0, 0, 0, ?2, ?3)",
                rusqlite::params![
                    session.id().to_string(),
                    serde_json::to_string(session.metadata()).unwrap(),
                    serde_json::to_vec(&session).unwrap(),
                ],
            )
            .unwrap();
        }
        let census = census_legacy_sessions(&db).unwrap();
        assert_eq!(census.legacy, 1);
        assert_eq!(census.verified, 0);
        assert_eq!(census.invalid, 0);
    }

    #[test]
    fn read_domain_versions_reads_ledgers_read_only() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("db.sqlite3");
        create_db(&db);
        assert!(read_domain_versions(&db).unwrap().is_none());
        {
            let conn = Connection::open(&db).unwrap();
            conn.execute_batch(
                "CREATE TABLE meerkat_schema (domain TEXT PRIMARY KEY, version INTEGER NOT NULL);
                 INSERT INTO meerkat_schema VALUES ('session-store', 1);",
            )
            .unwrap();
        }
        assert_eq!(
            read_domain_versions(&db).unwrap(),
            Some(vec![("session-store".to_string(), 1)])
        );
    }

    #[test]
    fn list_realm_dirs_skips_archives_and_reads_manifests_leniently() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("realms");
        write_manifest(&root, "alpha", "sqlite");
        let corrupt = root.join("corrupt");
        fs::create_dir_all(&corrupt).unwrap();
        fs::write(corrupt.join(REALM_MANIFEST_FILE_NAME), b"not-json").unwrap();
        let archived = root.join("beta.pre-0.0.1-1700000000");
        fs::create_dir_all(&archived).unwrap();
        fs::write(
            archived.join(REALM_MANIFEST_FILE_NAME),
            serde_json::to_vec(&serde_json::json!({
                "realm_id": "beta", "backend": "sqlite",
                "origin": "explicit", "created_at": "0",
            }))
            .unwrap(),
        )
        .unwrap();
        // A directory without a manifest is not a realm.
        fs::create_dir_all(root.join("not-a-realm")).unwrap();

        let realms = list_realm_dirs(&root);
        let ids: Vec<&str> = realms.iter().map(|realm| realm.realm_id.as_str()).collect();
        assert_eq!(ids, vec!["alpha", "corrupt"]);
        assert!(realms[0].manifest_readable);
        assert_eq!(realms[0].backend.as_deref(), Some("sqlite"));
        assert!(!realms[1].manifest_readable);
    }
}

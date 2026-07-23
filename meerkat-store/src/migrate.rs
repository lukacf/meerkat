//! Offline migration primitives behind `rkat storage migrate` / `rkat
//! storage prune` (Phase 6 of the storage unification arc).
//!
//! This module is a **reusable library layer** over an arbitrary state
//! directory — standalone mobkit gateways with no `rkat` CLI on the box run
//! their migrations through these same primitives. It owns:
//!
//! - [`RealmMaintenanceFence`]: the realm-wide exclusive maintenance fence.
//!   It enumerates every SQLite database file materialized under a realm
//!   directory and takes the per-file [`meerkat_sqlite::ExclusiveFence`] on
//!   each, in deterministic sorted order (two concurrent migrators can never
//!   ABBA-deadlock), waiting up to a deadline for in-flight per-operation
//!   guards to drain. Acquisition is all-or-nothing: any failure releases
//!   everything already acquired (RAII).
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
/// the [`RealmMaintenanceFence`] covers (plus the per-mob `mobs/*.db`
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
/// databases. Sorted (deterministic fence order).
pub fn enumerate_realm_sqlite_files(realm_dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = REALM_SQLITE_FILES
        .iter()
        .map(|relative| realm_dir.join(relative))
        .filter(|path| path.is_file())
        .collect();
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

/// The realm-wide exclusive maintenance fence.
///
/// Holds one [`meerkat_sqlite::ExclusiveFence`] per SQLite database file
/// materialized under the realm directory. While held, every foreign
/// process's per-operation guard fails typed
/// (`MaintenanceFenceHeld`); the holder's own in-process store operations
/// self-admit (see `meerkat_sqlite::fence`), which is what lets bulk
/// maintenance reuse production store code paths.
///
/// Acquisition blocks the calling thread (bounded by the deadline); async
/// callers should wrap it in `spawn_blocking`.
#[derive(Debug)]
pub struct RealmMaintenanceFence {
    fences: Vec<meerkat_sqlite::ExclusiveFence>,
    databases: Vec<PathBuf>,
}

impl RealmMaintenanceFence {
    /// Fence every SQLite database under `realm_dir`, waiting up to
    /// `deadline` (total, across all files) for in-flight operations to
    /// drain.
    ///
    /// Files are fenced in sorted order so two concurrent migrators acquire
    /// in the same sequence instead of deadlocking ABBA. On any failure the
    /// already-acquired fences are released (RAII) and the typed error
    /// surfaces ([`StoreError::MaintenanceFenceHeld`] when a foreign holder
    /// owns a fence past the deadline).
    pub fn acquire(realm_dir: &Path, deadline: Duration) -> Result<Self, StoreError> {
        let started = Instant::now();
        let databases = enumerate_realm_sqlite_files(realm_dir);
        let mut fences = Vec::with_capacity(databases.len());
        for database in &databases {
            let remaining = deadline.saturating_sub(started.elapsed());
            let fence =
                meerkat_sqlite::ExclusiveFence::acquire(database, remaining).map_err(|error| {
                    // Dropping `fences` here releases everything acquired.
                    StoreError::from(error)
                })?;
            fences.push(fence);
        }
        Ok(Self { fences, databases })
    }

    /// The database files this fence covers, in acquisition (sorted) order.
    pub fn fenced_databases(&self) -> &[PathBuf] {
        &self.databases
    }

    /// Number of held per-file fences.
    pub fn len(&self) -> usize {
        self.fences.len()
    }

    /// True when the realm materializes no SQLite files (nothing to fence).
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

/// Rename `path` (file or directory) to its registered backup-artifact name
/// next to the original, then best-effort strip write permission so the
/// archive is read-only. Returns the archive path.
pub fn archive_path_read_only(path: &Path, purpose: &str) -> Result<PathBuf, StoreError> {
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
    strip_write_permissions_best_effort(&archive);
    Ok(archive)
}

fn strip_write_permissions_best_effort(path: &Path) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    if metadata.is_dir()
        && let Ok(entries) = fs::read_dir(path)
    {
        for entry in entries.filter_map(Result::ok) {
            strip_write_permissions_best_effort(&entry.path());
        }
    }
    let mut permissions = metadata.permissions();
    permissions.set_readonly(true);
    let _ = fs::set_permissions(path, permissions);
}

fn restore_write_permissions_best_effort(path: &Path) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
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
    /// Per-database file-digest comparison (runtime / workgraph / schedule /
    /// memory / tasks / index / mob databases; v1 granularity).
    #[serde(default)]
    pub files: Vec<FileDivergenceEntry>,
    /// How the twin was (or was not) resolved.
    pub resolution: SplitBrainResolution,
    /// Divergence-computation failures (report-only; archiving is
    /// non-destructive either way).
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

/// Digest of one file's bytes; for SQLite databases the `-wal` sidecar (if
/// present) is folded in, so uncheckpointed frames register as divergence
/// (conservative: never reports "equal" for possibly-different content).
fn file_digest(path: &Path) -> Result<[u8; 32], StoreError> {
    let mut hasher = Sha256::new();
    hasher.update(fs::read(path)?);
    let mut wal = path.as_os_str().to_os_string();
    wal.push("-wal");
    let wal = PathBuf::from(wal);
    if wal.is_file() {
        hasher.update(fs::read(&wal)?);
    }
    Ok(hasher.finalize().into())
}

/// Compute the per-domain divergence report for one split-brain realm.
///
/// Sessions are compared row-level (id + content digest over the session
/// tables, read-only SQL); every other database is compared at file-digest
/// level in v1. Comparison failures are folded into
/// [`SplitBrainReport::errors`] — the report stays usable because case-3
/// resolution never merges or deletes anything either way.
pub fn compute_split_brain_report(realm: &str, locations: &[PathBuf]) -> SplitBrainReport {
    let mut report = SplitBrainReport::new(realm, locations.to_vec());

    // Row-level session comparison across every copy with a sessions db.
    let mut per_location: Vec<(PathBuf, SessionDigests)> = Vec::new();
    for location in locations {
        let db = location.join("sessions.sqlite3");
        if !db.is_file() {
            per_location.push((location.clone(), SessionDigests::new()));
            continue;
        }
        match session_digests(&db) {
            Ok(digests) => per_location.push((location.clone(), digests)),
            Err(error) => {
                report.errors.push(format!(
                    "session divergence unavailable for {}: {error}",
                    db.display()
                ));
                per_location.push((location.clone(), SessionDigests::new()));
            }
        }
    }
    let mut all_ids: Vec<String> = per_location
        .iter()
        .flat_map(|(_, digests)| digests.keys().cloned())
        .collect();
    all_ids.sort();
    all_ids.dedup();
    for session_id in all_ids {
        let holders: Vec<(&PathBuf, &[u8; 32])> = per_location
            .iter()
            .filter_map(|(location, digests)| {
                digests.get(&session_id).map(|digest| (location, digest))
            })
            .collect();
        if holders.len() == 1 {
            report.sessions.push(SessionDivergenceEntry {
                session_id,
                status: DivergenceStatus::OnlyIn {
                    location: holders[0].0.clone(),
                },
            });
        } else if holders.len() == per_location.len()
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

    // File-digest comparison over every named database (plus mob dbs).
    let mut relative_files: Vec<String> = Vec::new();
    for location in locations {
        for db in enumerate_realm_sqlite_files(location) {
            if let Ok(relative) = db.strip_prefix(location) {
                let relative = relative.to_string_lossy().replace('\\', "/");
                if !relative_files.contains(&relative) {
                    relative_files.push(relative);
                }
            }
        }
    }
    relative_files.sort();
    for relative in relative_files {
        let mut holders: Vec<(&PathBuf, [u8; 32])> = Vec::new();
        let mut failed = false;
        for location in locations {
            let path = location.join(&relative);
            if !path.is_file() {
                continue;
            }
            match file_digest(&path) {
                Ok(digest) => holders.push((location, digest)),
                Err(error) => {
                    report.errors.push(format!(
                        "file digest unavailable for {}: {error}",
                        path.display()
                    ));
                    failed = true;
                }
            }
        }
        if failed {
            report.files.push(FileDivergenceEntry {
                file: relative,
                status: DivergenceStatus::Divergent,
            });
            continue;
        }
        let status = if holders.len() == 1 {
            DivergenceStatus::OnlyIn {
                location: holders[0].0.clone(),
            }
        } else if holders.len() == locations.len()
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

fn age_days(path: &Path) -> u64 {
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

fn push_artifacts_in(dir: &Path, dirs_too: bool, artifacts: &mut Vec<PruneArtifact>) {
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
        artifacts.push(PruneArtifact {
            bytes: recursive_size(&path),
            age_days: age_days(&path),
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
    let mut artifacts = Vec::new();
    let mut seen_roots: Vec<PathBuf> = Vec::new();
    for root in state_roots {
        let canonical = fs::canonicalize(root).unwrap_or_else(|_| root.clone());
        if seen_roots.contains(&canonical) {
            continue;
        }
        seen_roots.push(canonical);

        // Root level: archived realm directories and archived files.
        push_artifacts_in(root, true, &mut artifacts);

        // Inside each live realm directory (backup artifacts inside archived
        // copies are part of the archive, owned by the archive's own entry).
        for realm in list_realm_dirs(root) {
            for scan_dir in [
                realm.dir.clone(),
                realm.dir.join("memory"),
                realm.dir.join("sessions_jsonl"),
                realm.dir.join("mobs"),
            ] {
                push_artifacts_in(&scan_dir, false, &mut artifacts);
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
    fn fence_acquires_every_database_in_sorted_order() {
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
        assert_eq!(fence.len(), 5);
        assert!(!fence.is_empty());
        let mut expected = vec![
            realm.join("memory/memory.sqlite3"),
            realm.join("mobs/alpha.db"),
            realm.join("mobs/realm_profiles.db"),
            realm.join("sessions.sqlite3"),
            realm.join("workgraph.sqlite3"),
        ];
        expected.sort();
        assert_eq!(fence.fenced_databases(), expected.as_slice());
        // Every fence lock file exists.
        for database in fence.fenced_databases() {
            assert!(meerkat_sqlite::fence_lock_path(database).is_file());
        }
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

        // RAII: the first fence (sessions) was released on failure — a fresh
        // exclusive acquisition succeeds immediately.
        let reacquired =
            meerkat_sqlite::ExclusiveFence::try_acquire(&realm.join("sessions.sqlite3")).unwrap();
        assert!(reacquired.is_some(), "partial acquisition must be released");
        drop(reacquired);
        drop(foreign);

        // With the foreign lock gone, full acquisition succeeds.
        let fence = RealmMaintenanceFence::acquire(&realm, Duration::from_secs(1)).unwrap();
        assert_eq!(fence.len(), 2);
    }

    #[test]
    fn empty_realm_fences_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let fence = RealmMaintenanceFence::acquire(temp.path(), Duration::ZERO).unwrap();
        assert!(fence.is_empty());
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

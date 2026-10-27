//! Offline migration and prune for Meta state directories (Phase M6 of
//! the storage-unification arc): the mutation half of Meta's storage
//! maintenance, behind `meta_gateway storage-migrate` / `storage-prune`.
//!
//! Meta builds no second fence: [`MetaMaintenanceFence`] composes one
//! [`meerkat_sqlite::ExclusiveFence`] per materialized database (meerkat's
//! Phase 6 primitive), and the backup discipline is meerkat's verbatim —
//! [`meerkat_store::migrate::backup_artifact_name`] renames
//! (`<original>.pre-<version>-<timestamp>[.<purpose>]`), never deletes;
//! doctor lists `*.pre-*` artifacts and [`prune_state_dir`] owns their
//! lifecycle.
//!
//! # The five migration cases ([`migrate_state_dir`])
//!
//! 1. **Ledger baseline (auto-safe).** Under the fence, every existing
//!    Meta-owned database is opened through its NORMAL M3 constructor —
//!    the guarded `meerkat_schema` ledger migrations ARE the structural
//!    verification (continuity via `LocalContinuityStore::open`, metadata
//!    via `SqliteMetadataStore::open`, console via
//!    `SqliteConsoleLogStore::open`, per-realm agent-memory files via the
//!    `SqliteAgentMemoryStore` realm-connection path). Dry-run is a
//!    read-only version matrix. The workgraph admission sidecar is
//!    **exempt** (M3 decision: the lock database deliberately carries no
//!    tables and no ledger); most meerkat-shared databases (sessions, runtime,
//!    schedule, workgraph) are report-only here. The inherited jobs database
//!    is the deliberate exception: this verb invokes Meerkat's canonical
//!    `SqliteDetachedJobStore` constructor while holding the same state-wide
//!    fence, so offline migration covers the Phase-4 jobs domain without
//!    introducing a Meta-owned schema authority.
//! 2. **File-name unification (auto-safe, rename-only).** A lone legacy
//!    spelling ([`crate::storage_layout::DatabaseSlot::legacy_names`])
//!    renames to its M2 canonical name under the fence, WITH its `-wal` /
//!    `-shm` siblings: a non-empty WAL is checkpointed (`TRUNCATE`) through
//!    a maintenance connection first, and the move refuses if it cannot be.
//!    Renames preserve content, so the "registered backup" is a registered
//!    **rename marker** (`<legacy>.pre-<version>-<ts>.renamed`, a small JSON
//!    record) — doctor lists it as a backup artifact and prune owns it, so
//!    retention tooling sees every rename.
//! 3. **Twin reconciliation (manual, fail-closed).** Both spellings
//!    populated → a per-domain divergence report (row-level by primary key +
//!    content digest for continuity / metadata / console; file-digest for
//!    the rest) and a typed refusal. Byte-identical twins dedup under plain
//!    `--apply` (exact-equality dedup, the one auto resolution the plan
//!    sanctions); divergent twins resolve only with `--apply --adopt
//!    <path>`: the adopted file keeps its place (renamed to canonical if
//!    legacy-named), every other copy is archived read-only under the
//!    registered backup naming. **No synthesis** — continuity fencing
//!    tokens and console `AUTOINCREMENT` cursors are per-database sequences;
//!    merging them corrupts CAS and cursor replay.
//! 4. **(Retired with the 0.8.11 reset.)** Continuity checkpoint adoption
//!    minted embedded checkpoint stamps meerkat no longer reads; released
//!    0.8.10 documents now convert through the explicit one-time importer.
//! 5. **Deprecated leftovers (report-only).** The doctor's artifact
//!    findings (legacy sharded-FS blobs, `*.pre-*` backups, `*.corrupt-*`
//!    quarantines, the admission sidecar, fence lock files) plus dead
//!    `tux-runtimes.json` registry entries (recorded pid no longer alive).
//!    Nothing in case 5 is ever moved.
//!
//! Any unresolved twin fails the whole run closed (mirroring `rkat storage
//! migrate`): the divergence report is the entire output and cases 1/2
//! do not run, because every one of them would have to pick a spelling.
//!
//! # `--session-db`: a session database a 0.9.3 `store_path` named
//!
//! A 0.9.3 `meta_gateway` whose `store_path` named a file kept its sessions in
//! that file, under whatever name it had. With `--session-db <file>` the run
//! takes the file as one more spelling of the Sessions slot: the fence covers
//! it, a lone copy renames to `sessions.sqlite3` under case 2, and a copy next
//! to another Sessions spelling is a case-3 twin whose refusal names the
//! `--adopt` command for it. Once the file has moved, its old path gets the
//! retired stamp of the session-store domain, so a 0.9.3 gateway started again
//! with that `store_path` refuses the file instead of starting an empty
//! projection. Before anything moves, and whether or not `--apply` is set, the
//! run refuses typed ([`SessionDbRefusal`]): a path that does not exist, is
//! not a file, or does not sit in the state directory; a file that is not a
//! session database; a directory a 0.9.4 runtime already started on; and a
//! directory whose runtime locks another process holds. A file that already
//! is the stamp, or that is `sessions.sqlite3` itself, is a typed no-op.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use meerkat_core::storage_diagnostics::{DiagnoseScope, FindingSeverity, StorageFinding};
pub use meerkat_store::migrate::{DivergenceStatus, MigrateMode, PruneAction, PruneArtifactKind};
use meerkat_store::migrate::{archive_path_read_only, remove_maintenance_artifact};
use meerkat_store::sqlite_store::SESSION_STORE_DOMAIN;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::console_aggregator::SqliteConsoleLogStore;
use crate::identity_first::LocalContinuityStore;
use crate::identity_first::local_store::HeadCanonicalBackfillReport;
use crate::memory::sqlite_store::SqliteAgentMemoryStore;
use crate::runtime::SqliteMetadataStore;
use crate::storage_doctor::{self, DATABASE_FAMILIES, MEMORY_LEDGER_DOMAIN, MEMORY_ROOT_SPELLINGS};
use crate::storage_layout::{
    DatabaseProvenance, DatabaseSlot, MetaStorageLayout, RUNTIME_REGISTRY_FILE_NAME,
    StorageLayoutError,
};
use crate::workgraph_admission::WORKGRAPH_ADMISSION_SIDECAR_FILE;

/// How long the fence waits (total, across all files) for in-flight
/// per-operation guards to drain before reporting a foreign holder.
const FENCE_DRAIN_DEADLINE: Duration = Duration::from_secs(10);

/// Stable finding code: a `tux-runtimes.json` registry entry whose recorded
/// pid is no longer alive (report-only; migrate never edits the registry).
pub const FINDING_DEAD_RUNTIME_REGISTRY_ENTRY: &str = "dead-runtime-registry-entry";

/// Strict validation of the COMPLETE generated backup-artifact shape:
/// `<original>.pre-<version>-<unix-ts>[.<purpose>]` — non-empty
/// `<original>`, `<version>` a dotted crate version (two or more all-digit
/// components, e.g. `0.8.3`), an all-digit Unix timestamp, and, when
/// present, a non-empty purpose suffix.
///
/// Deliberately stricter than matching a `.pre-` substring: prune's
/// deletion authority is exactly the names this accepts, and a loose
/// substring match would claim unrelated user files like
/// `notes.pre-release`. Every name [`meerkat_store::migrate::backup_artifact_name`] generates
/// (including the `-wal`/`-shm` sibling archives, whose suffix folds into
/// the purpose) validates.
pub fn is_registered_backup_artifact_name(name: &str) -> bool {
    let Some(index) = name.rfind(".pre-") else {
        return false;
    };
    if index == 0 {
        return false; // empty <original>
    }
    let rest = &name[index + ".pre-".len()..];
    let Some((version, tail)) = rest.split_once('-') else {
        return false;
    };
    let version_components: Vec<&str> = version.split('.').collect();
    if version_components.len() < 2
        || version_components
            .iter()
            .any(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return false;
    }
    let (timestamp, purpose) = match tail.split_once('.') {
        Some((timestamp, purpose)) => (timestamp, Some(purpose)),
        None => (tail, None),
    };
    if timestamp.is_empty() || !timestamp.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    purpose != Some("")
}

/// Strict validation of the registered index-quarantine shape: an exact
/// `.corrupt-<digits>` suffix on a non-empty original name (same rationale
/// as [`is_registered_backup_artifact_name`] — `report.corrupt-12a` or a
/// `.bak` copy of a quarantine is never prune's to delete).
pub fn is_registered_quarantine_artifact_name(name: &str) -> bool {
    let Some(index) = name.rfind(".corrupt-") else {
        return false;
    };
    if index == 0 {
        return false;
    }
    let digits = &name[index + ".corrupt-".len()..];
    !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
}

/// Case-5 doctor finding codes copied into the migrate report (deprecated
/// leftovers; report-only).
const LEFTOVER_FINDING_CODES: &[&str] = &[
    storage_doctor::FINDING_LEGACY_FS_BLOBS,
    storage_doctor::FINDING_BACKUP_ARTIFACT,
    storage_doctor::FINDING_QUARANTINE_ARTIFACT,
    storage_doctor::FINDING_WORKGRAPH_ADMISSION_SIDECAR,
    storage_doctor::FINDING_MAINTENANCE_FENCE_LOCK,
];

// ─────────────────────────────────────────────────────────────────────────
// The maintenance fence (composed from meerkat's per-file primitive).
// ─────────────────────────────────────────────────────────────────────────

/// Enumerate every SQLite database file currently materialized under a
/// Meta state directory: each [`DatabaseSlot`]'s canonical **and** legacy
/// spellings that exist as files, the inherited canonical jobs database, plus
/// the per-realm agent-memory databases under both memory-root spellings.
/// Sorted (deterministic fence order).
///
/// The workgraph admission sidecar is deliberately excluded: it carries no
/// per-operation fence guard by M3 design (it IS a cross-process lock), so
/// fencing it gates nothing, and migrate never mutates it.
pub fn enumerate_state_dir_databases(state_dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = Vec::new();
    for slot in DatabaseSlot::ALL {
        if slot == DatabaseSlot::AgentMemory {
            continue; // a directory slot; realm files are enumerated below
        }
        let mut names: Vec<&str> = vec![slot.canonical_name()];
        names.extend(slot.legacy_names());
        for name in names {
            let path = state_dir.join(name);
            if path.is_file() {
                files.push(path);
            }
        }
    }
    // The extension document store is a slot only with the extension-state
    // feature, but a state directory an embedder wrote with it can hold one:
    // every build fences it, and adoption carries it over.
    #[cfg(not(feature = "extension-state"))]
    {
        let path = state_dir.join(crate::storage_layout::EXTENSION_STATE_DB_FILE_NAME);
        if path.is_file() {
            files.push(path);
        }
    }
    // The mob announcement database is not a slot (it has no legacy
    // spelling), and only a gateway that enables mob announcements opens it:
    // every build fences it.
    let announcements = state_dir.join(crate::storage_layout::MOB_ANNOUNCEMENTS_DB_FILE_NAME);
    if announcements.is_file() {
        files.push(announcements);
    }
    for spelling in MEMORY_ROOT_SPELLINGS {
        let root = state_dir.join(spelling);
        let Ok(entries) = fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("sqlite3") {
                files.push(path);
            }
        }
    }
    files.extend(crate::storage_predecessor::realm_jobs_databases(state_dir));
    // Durable child mobs: per-mob event logs plus the realm-scoped profile
    // and custody databases in Meerkat's child root.
    files.extend(crate::child_mob_storage::child_mob_databases(state_dir));
    files.sort();
    files.dedup();
    files
}

/// The state-dir-wide exclusive maintenance fence: one
/// [`meerkat_sqlite::ExclusiveFence`] per database from
/// [`enumerate_state_dir_databases`], acquired in sorted order (two
/// concurrent migrators can never ABBA-deadlock), all-or-nothing — any
/// failure releases everything already acquired (RAII). While held, foreign
/// processes' per-operation guards fail typed
/// ([`meerkat_sqlite::SqliteStoreError::MaintenanceFenceHeld`]); this
/// process's own store operations self-admit, which is what lets the
/// migrate pass reuse production store constructors.
#[derive(Debug)]
pub struct MetaMaintenanceFence {
    fences: Vec<meerkat_sqlite::ExclusiveFence>,
    databases: Vec<PathBuf>,
}

impl MetaMaintenanceFence {
    /// Fence every materialized database under `state_dir`, waiting up to
    /// `deadline` (total, across all files) for in-flight operations to
    /// drain. Blocking; async callers should wrap it in `spawn_blocking`.
    pub fn acquire(
        state_dir: &Path,
        deadline: Duration,
    ) -> Result<Self, meerkat_sqlite::SqliteStoreError> {
        Self::acquire_including(state_dir, &[], deadline)
    }

    /// [`Self::acquire`], also fencing `extra` databases the layout does not
    /// name (the session database `--session-db` names), in the same sorted
    /// all-or-nothing acquisition.
    pub fn acquire_including(
        state_dir: &Path,
        extra: &[PathBuf],
        deadline: Duration,
    ) -> Result<Self, meerkat_sqlite::SqliteStoreError> {
        let started = std::time::Instant::now();
        let mut databases = enumerate_state_dir_databases(state_dir);
        databases.extend(extra.iter().cloned());
        databases.sort();
        databases.dedup();
        let mut fences = Vec::with_capacity(databases.len());
        for database in &databases {
            let remaining = deadline.saturating_sub(started.elapsed());
            // Dropping `fences` on error releases everything acquired.
            let fence = meerkat_sqlite::ExclusiveFence::acquire(database, remaining)?;
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

    /// True when the state dir materializes no databases (nothing to fence).
    pub fn is_empty(&self) -> bool {
        self.fences.is_empty()
    }

    /// Re-fence after a rename: the original fence rides the OLD lock path,
    /// so the file's NEW canonical path must get a fresh fence for the rest
    /// of the pass. Failure is fail-closed for the renamed database — the
    /// caller records the typed error and skips further mutation of it,
    /// never continuing unfenced.
    fn cover_renamed(&mut self, path: &Path) -> Result<(), meerkat_sqlite::SqliteStoreError> {
        // Canonical-name-first fencing may have covered the canonical path
        // up front (twin adoption renames INTO a path this fence already
        // holds); custody is already ours, and a second exclusive fence on
        // it would self-refuse.
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if self
            .databases
            .iter()
            .any(|held| std::fs::canonicalize(held).unwrap_or_else(|_| held.clone()) == canonical)
        {
            return Ok(());
        }
        match meerkat_sqlite::ExclusiveFence::try_acquire(path)? {
            Some(fence) => {
                self.fences.push(fence);
                self.databases.push(path.to_path_buf());
                Ok(())
            }
            None => Err(meerkat_sqlite::SqliteStoreError::MaintenanceFenceHeld {
                path: path.to_path_buf(),
            }),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Report vocabulary (serde; `#[non_exhaustive]` + defaults, mirroring
// `meerkat_store::migrate`'s shapes).
// ─────────────────────────────────────────────────────────────────────────

/// The full `storage-migrate` report for one state directory.
#[non_exhaustive]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MetaMigrateReport {
    /// Dry-run or apply.
    #[serde(default)]
    pub mode: MigrateMode,
    /// The state directory swept.
    #[serde(default)]
    pub state_dir: PathBuf,
    /// Databases the fence covers (in apply mode, the fence actually held;
    /// in dry-run, what an apply run would fence).
    #[serde(default)]
    pub fenced_databases: Vec<PathBuf>,
    /// Ledger baseline entries per database × domain (case 1).
    #[serde(default)]
    pub ledger: Vec<LedgerBaselineEntry>,
    /// File-name unification renames (case 2, plus case-3 canonicalization
    /// renames — every physical rename this run performed or would perform).
    #[serde(default)]
    pub renames: Vec<FileRenameEntry>,
    /// File-name twins and their resolution (case 3).
    #[serde(default)]
    pub twins: Vec<TwinReport>,
    /// Head-canonical backfill of legacy v1 sessions (apply only). The
    /// `meta-continuity` v2 stamp rides on this: it advances only when
    /// every legacy blob has been converted, so a partial crossing leaves
    /// the file at v1 with rollback available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_canonical_backfill: Option<HeadCanonicalBackfillReport>,
    /// What `--session-db` did, or would do, with the session database a
    /// 0.9.3 `store_path` named (`None` without `--session-db`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_db: Option<SessionDbReport>,
    /// Deprecated-leftover findings (case 5; report-only), reusing the
    /// doctor finding vocabulary.
    #[serde(default)]
    pub findings: Vec<StorageFinding>,
    /// Human-readable notes (exemptions, report-only carve-outs).
    #[serde(default)]
    pub notes: Vec<String>,
    /// Fail-closed refusals and hard failures. Non-empty ⇒ nonzero exit.
    #[serde(default)]
    pub errors: Vec<String>,
}

impl MetaMigrateReport {
    /// New empty report for one run.
    pub fn new(mode: MigrateMode, state_dir: &Path) -> Self {
        Self {
            mode,
            state_dir: state_dir.to_path_buf(),
            ..Self::default()
        }
    }

    /// True when the run must exit nonzero (refusals, fence failures,
    /// constructor failures, adoption refusals).
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }
}

/// One database × ledger-domain baseline row (case 1).
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerBaselineEntry {
    /// Database file.
    pub database: PathBuf,
    /// Ledger domain (`meta-continuity`, `session-store`, ...).
    pub domain: String,
    /// Version before the run (`None` = no ledger row).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<i64>,
    /// Version after the run (`None` in dry-run and for report-only rows).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<i64>,
    /// What happened (or would happen).
    pub action: LedgerBaselineAction,
}

/// Disposition of one ledger baseline row.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LedgerBaselineAction {
    /// Dry-run: no ledger row; the owning store baseline-stamps on
    /// `--apply` (guarded migrations converge files of any vintage).
    WouldStamp,
    /// Dry-run: a ledger row exists; pending migrations converge on
    /// `--apply` through the owning store's normal constructor.
    Recorded,
    /// Apply: the ledger row was created or advanced.
    Stamped,
    /// Apply: already at the current version; no-op.
    AlreadyCurrent,
    /// A meerkat-owned database (sessions / runtime / schedule /
    /// workgraph): versions are reported read-only; the owning meerkat
    /// store converges the file on its next open.
    ReportOnly,
    /// The workgraph admission sidecar: ledger-exempt by design (M3) — the
    /// lock database deliberately carries no tables and no ledger.
    Exempt,
}

/// One physical rename (case 2, or a case-3 canonicalization).
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRenameEntry {
    /// The layout slot this rename converges.
    pub slot: String,
    /// Legacy path.
    pub from: PathBuf,
    /// Canonical path.
    pub to: PathBuf,
    /// `-wal` / `-shm` sibling files moved along with the database.
    #[serde(default)]
    pub siblings: Vec<SiblingRename>,
    /// True when a non-empty WAL was checkpointed before the move.
    #[serde(default)]
    pub wal_checkpointed: bool,
    /// The registered rename marker (`<legacy>.pre-<version>-<ts>.renamed`),
    /// written on apply so doctor and prune see the rename.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marker: Option<PathBuf>,
    /// What happened (or would happen).
    pub action: RenameAction,
}

/// One `-wal` / `-shm` sibling moved with its database.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiblingRename {
    pub from: PathBuf,
    pub to: PathBuf,
}

/// Disposition of one rename.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RenameAction {
    /// Dry-run: a lone legacy spelling; `--apply` renames it.
    WouldRename,
    /// Apply: renamed (siblings and marker recorded alongside).
    Renamed,
    /// Apply: refused (reason in [`MetaMigrateReport::errors`]) — for
    /// example a WAL that cannot be checkpointed.
    Refused,
}

/// Case 3: one file-name twin (two or more spellings of one slot populated
/// in the same state directory).
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TwinReport {
    /// The layout slot.
    pub slot: String,
    /// Every populated spelling (canonical first when present).
    pub paths: Vec<PathBuf>,
    /// True when every copy is byte-identical (database bytes plus any WAL
    /// sidecar) — the exact-equality dedup precondition.
    #[serde(default)]
    pub byte_identical: bool,
    /// Rows identical across every copy (count only; not enumerated).
    #[serde(default)]
    pub rows_equal: usize,
    /// Divergent / single-copy rows. For continuity / metadata / console
    /// the key is `table/<primary key>`; for the agent-memory directory
    /// twin the key is the relative realm-database name.
    #[serde(default)]
    pub rows: Vec<RowDivergenceEntry>,
    /// How the twin was (or was not) resolved.
    pub resolution: TwinResolution,
    /// Per-slot notes (the no-synthesis rationale, digest granularity).
    #[serde(default)]
    pub notes: Vec<String>,
    /// Divergence-computation failures (report-only; resolution is
    /// non-destructive either way).
    #[serde(default)]
    pub errors: Vec<String>,
}

/// One non-equal row (or file) across twin copies.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RowDivergenceEntry {
    /// `table/<primary key>` (row-level domains) or a relative file name.
    pub key: String,
    /// Divergence classification.
    pub status: DivergenceStatus,
}

/// Case 3 resolution.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum TwinResolution {
    /// Fail-closed refusal: nothing moved.
    Refused {
        /// Why the run refused.
        reason: String,
    },
    /// Exact-equality dedup: the copies were byte-identical; the canonical
    /// spelling was kept (renaming it into place when needed) and every
    /// redundant copy archived read-only.
    Deduped {
        /// The surviving copy (at its canonical path).
        kept: PathBuf,
        /// Archive paths of the redundant copies.
        archived: Vec<PathBuf>,
    },
    /// One copy adopted as authority (at its canonical path); every other
    /// copy archived read-only under the registered backup naming. No
    /// synthesis — divergent content is preserved in the archives.
    Adopted {
        /// The adopted copy (at its canonical path).
        adopted: PathBuf,
        /// Archive paths of the non-adopted copies.
        archived: Vec<PathBuf>,
    },
}

/// `--session-db`: the session database a 0.9.3 `meta_gateway` kept under
/// the name its `store_path` gave it.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionDbReport {
    /// The named file.
    pub path: PathBuf,
    /// What happened (or would happen).
    pub outcome: SessionDbOutcome,
}

/// What `--session-db` did with the named file.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum SessionDbOutcome {
    /// The file joins the Sessions slot: renamed to `sessions.sqlite3` when
    /// it is the slot's only copy, else reconciled as a twin (see `renames`
    /// and `twins`). A dry run stops here, and so does an apply run an
    /// unresolved twin refused.
    WouldMove,
    /// The file moved into the layout (renamed, adopted, or archived as a
    /// byte-identical twin), and its old path now holds the retired stamp of
    /// the session-store domain, which a 0.9.3 binary refuses.
    Moved {
        /// The retired stamp at the file's old path.
        tripwire: PathBuf,
    },
    /// The file already is the retired stamp: an earlier run moved it.
    AlreadyMoved,
    /// The file is gone, `sessions.sqlite3` is in place, and an earlier
    /// run's artifact of the file (its rename marker, or its archive after a
    /// dedup or an adopt) lies beside it: that run moved the file and stopped
    /// before it wrote the retired stamp. An apply run writes the stamp.
    TripwireOwed,
    /// The file is `sessions.sqlite3` itself: it already is in the layout.
    AlreadyInLayout,
    /// Refused before anything moved; nothing was written.
    Refused {
        /// Why.
        refusal: SessionDbRefusal,
    },
}

/// Why `--session-db` refused, before anything moved.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "reason")]
pub enum SessionDbRefusal {
    /// Nothing exists at the named path.
    Missing { path: PathBuf },
    /// The named path is not a regular file.
    NotAFile { path: PathBuf },
    /// The named file is not in the state directory. 0.9.3 made the
    /// directory holding the `store_path` file its state directory, so the
    /// file belongs to the directory it sits in.
    OutsideStateDir { path: PathBuf, state_dir: PathBuf },
    /// The named file is not a 0.9.3 session database: not SQLite, or no
    /// `meerkat_schema` row of the session-store domain.
    NotASessionDatabase { path: PathBuf, detail: String },
    /// A 0.9.4 runtime already started on the directory: `runtime.sqlite`
    /// carries Core's head-canonical durable claim, so the native migration
    /// can no longer read it, and the file's sessions stay where they are.
    NativeRuntimeAlreadyStarted { state_dir: PathBuf },
    /// A process holds a runtime lock of the directory (a 0.9.3 gateway
    /// hosting a runtime, or a 0.9.4 Meta runtime). It may still have the
    /// named file open.
    RuntimeInUse { file: PathBuf },
    /// A file the checks must read could not be read.
    Unreadable { path: PathBuf, detail: String },
}

impl std::fmt::Display for SessionDbRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing { path } => write!(
                f,
                "--session-db {}: no such file; name the file a 0.9.3 meta_gateway's store_path \
                 named",
                path.display()
            ),
            Self::NotAFile { path } => write!(
                f,
                "--session-db {}: not a file; a state directory needs no --session-db, pass it \
                 as --state-dir",
                path.display()
            ),
            Self::OutsideStateDir { path, state_dir } => write!(
                f,
                "--session-db {} is not in the state directory {}; 0.9.3 used the directory \
                 that holds the store_path file as its state directory, so pass that directory \
                 as --state-dir",
                path.display(),
                state_dir.display()
            ),
            Self::NotASessionDatabase { path, detail } => write!(
                f,
                "--session-db {} is not a 0.9.3 session database ({detail}); nothing was moved",
                path.display()
            ),
            Self::NativeRuntimeAlreadyStarted { state_dir } => write!(
                f,
                "a 0.9.4 runtime has already started on the state directory {}: its \
                 runtime.sqlite carries the head-canonical profile, so the native migration \
                 cannot bring the named file's sessions into it. Nothing was moved; the file's \
                 0.9.3 sessions stay in that file. Move the file out of the state directory",
                state_dir.display()
            ),
            Self::RuntimeInUse { file } => write!(
                f,
                "a process still uses the state directory ({} is locked) and may have the \
                 named file open; stop every process using the directory, 0.9.3 gateways \
                 started with that store_path included, then run again",
                file.display()
            ),
            Self::Unreadable { path, detail } => write!(
                f,
                "--session-db could not read {}: {detail}; nothing was moved",
                path.display()
            ),
        }
    }
}

/// The full `storage-prune` report for one state directory. Shape mirrors
/// `meerkat_store::migrate::PruneReport` (meta re-declares the structs
/// because meerkat's are `#[non_exhaustive]` and cannot be constructed
/// here; the enums are shared).
#[non_exhaustive]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MetaPruneReport {
    /// Dry-run or apply.
    #[serde(default)]
    pub mode: MigrateMode,
    /// The state directory swept.
    #[serde(default)]
    pub state_dir: PathBuf,
    /// Age threshold in days (artifacts at least this old are deleted on
    /// apply; `0` = all).
    #[serde(default)]
    pub older_than_days: u64,
    /// Registered artifacts found, with dispositions.
    #[serde(default)]
    pub artifacts: Vec<MetaPruneArtifact>,
    /// Failures (delete errors). Non-empty ⇒ nonzero exit.
    #[serde(default)]
    pub errors: Vec<String>,
}

impl MetaPruneReport {
    /// New empty report for one run.
    pub fn new(mode: MigrateMode, state_dir: &Path, older_than_days: u64) -> Self {
        Self {
            mode,
            state_dir: state_dir.to_path_buf(),
            older_than_days,
            ..Self::default()
        }
    }

    /// True when the run must exit nonzero.
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }
}

/// One registered maintenance artifact.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetaPruneArtifact {
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

// ─────────────────────────────────────────────────────────────────────────
// The migrate pass.
// ─────────────────────────────────────────────────────────────────────────

/// Run the five-case migration pass over one Meta state directory.
/// Blocking (async callers should wrap it in `spawn_blocking`); the caller
/// renders the report and maps [`MetaMigrateReport::has_errors`] to a
/// nonzero exit.
///
/// `adopt` is the case-3 twin resolution: the twin copy to adopt as
/// authority (only meaningful with [`MigrateMode::Apply`]).
pub fn migrate_state_dir(
    state_dir: &Path,
    mode: MigrateMode,
    adopt: Option<&Path>,
) -> MetaMigrateReport {
    migrate_state_dir_acknowledging_skipped(
        state_dir,
        mode,
        adopt,
        &std::collections::BTreeSet::new(),
    )
}

/// [`migrate_state_dir`], with explicit acknowledgement of blob rows that
/// cannot be parsed as sessions. Without it the ledger refuses to advance
/// while any such row exists — see the backfill's stamp guard.
pub fn migrate_state_dir_acknowledging_skipped(
    state_dir: &Path,
    mode: MigrateMode,
    adopt: Option<&Path>,
    acknowledged_rows: &std::collections::BTreeSet<String>,
) -> MetaMigrateReport {
    migrate_state_dir_with_session_db(state_dir, mode, adopt, acknowledged_rows, None)
}

/// [`migrate_state_dir_acknowledging_skipped`], taking `session_db`, the
/// session database a 0.9.3 `store_path` named, as one more spelling of the
/// Sessions slot (see the module docs).
pub fn migrate_state_dir_with_session_db(
    state_dir: &Path,
    mode: MigrateMode,
    adopt: Option<&Path>,
    acknowledged_rows: &std::collections::BTreeSet<String>,
    session_db: Option<&Path>,
) -> MetaMigrateReport {
    let mut report = MetaMigrateReport::new(mode, state_dir);
    if !state_dir.is_dir() {
        report.errors.push(format!(
            "state directory {} does not exist",
            state_dir.display()
        ));
        return report;
    }
    let predecessor = crate::storage_predecessor::predecessor_state_files(state_dir);
    if !predecessor.is_empty() {
        report.errors.push(
            StorageLayoutError::PredecessorState {
                state_dir: state_dir.to_path_buf(),
                paths: predecessor,
            }
            .to_string(),
        );
        return report;
    }
    if let Some(detail) = crate::storage_predecessor::unfinished_adoption(state_dir) {
        report.errors.push(
            StorageLayoutError::AdoptionIncomplete {
                path: state_dir.join(crate::storage_predecessor::ADOPTION_JOURNAL_FILE_NAME),
                detail,
            }
            .to_string(),
        );
        return report;
    }
    let apply = mode == MigrateMode::Apply;
    let layout = MetaStorageLayout::with_injected_roots(state_dir.to_path_buf(), None);
    if let Err(refusal) = layout.meerkat_level_realm() {
        report.errors.push(refusal.to_string());
        return report;
    }

    // `--session-db`: read-only checks before the fence, then the runtime
    // locks, held to the end of the run so no runtime starts meanwhile.
    let mut session_db_plan: Option<SessionDbPlan> = None;
    let mut _runtime_locks = None;
    if let Some(named) = session_db {
        let outcome = match plan_session_db(state_dir, named) {
            Ok(plan) => {
                match crate::storage_migrate_native::census::exclude_live_runtimes(state_dir) {
                    Ok(locks) => {
                        _runtime_locks = Some(locks);
                        let outcome = if plan.tripwire_owed {
                            SessionDbOutcome::TripwireOwed
                        } else {
                            SessionDbOutcome::WouldMove
                        };
                        session_db_plan = Some(plan);
                        outcome
                    }
                    Err(refusal) => SessionDbOutcome::Refused {
                        refusal: runtime_lock_refusal(state_dir, refusal),
                    },
                }
            }
            Err(outcome) => outcome,
        };
        if let SessionDbOutcome::Refused { refusal } = &outcome {
            report.errors.push(refusal.to_string());
            report.session_db = Some(SessionDbReport {
                path: named.to_path_buf(),
                outcome,
            });
            return report;
        }
        report.session_db = Some(SessionDbReport {
            path: named.to_path_buf(),
            outcome,
        });
    }
    let session_db_override = session_db_plan
        .as_ref()
        .and_then(|plan| plan.override_path());

    // Exclusive maintenance fence over every materialized database (apply
    // only; dry-run is read-only and reports what an apply run would fence).
    let extra_fenced: Vec<PathBuf> = session_db_override
        .iter()
        .map(|path| path.to_path_buf())
        .collect();
    let mut fence = if apply {
        match MetaMaintenanceFence::acquire_including(
            state_dir,
            &extra_fenced,
            FENCE_DRAIN_DEADLINE,
        ) {
            Ok(fence) => {
                report.fenced_databases = fence.fenced_databases().to_vec();
                Some(fence)
            }
            Err(error) => {
                report
                    .errors
                    .push(format!("maintenance fence not acquirable: {error}"));
                return report;
            }
        }
    } else {
        let mut fenced = enumerate_state_dir_databases(state_dir);
        fenced.extend(extra_fenced.iter().cloned());
        fenced.sort();
        fenced.dedup();
        report.fenced_databases = fenced;
        None
    };

    // Canonical paths whose post-rename fence could not be re-acquired:
    // fail-closed — the fence failure is recorded as an error and the
    // remaining cases never mutate these databases.
    let mut unfenced: Vec<PathBuf> = Vec::new();

    // ── Case 3: twin reconciliation (fail-closed). ───────────────────────
    let mut unresolved_twin = false;
    for slot in DatabaseSlot::ALL {
        let paths = if slot == DatabaseSlot::Sessions {
            // The slot's copies, the file `--session-db` names included: a
            // twin when there are two or more, else the slot's only copy
            // (case 2 below).
            let paths = sessions_copies(state_dir, session_db_override);
            if paths.len() < 2 {
                continue;
            }
            paths
        } else {
            let Err(StorageLayoutError::FileNameTwins { paths, .. }) =
                layout.resolve_database(slot)
            else {
                continue;
            };
            paths
        };
        let twin = reconcile_twin(
            slot,
            &paths,
            mode,
            adopt,
            &mut report.renames,
            fence.as_mut(),
            &mut report.errors,
            &mut unfenced,
            state_dir,
            session_db_plan.as_ref().map(|plan| plan.named.as_path()),
        );
        if let TwinResolution::Refused { reason } = &twin.resolution {
            unresolved_twin = true;
            report
                .errors
                .push(format!("file-name twins for the {slot} store: {reason}"));
        }
        report.twins.push(twin);
    }
    if unresolved_twin {
        // Fail-closed: the divergence report is the whole output — every
        // remaining case would have to pick a spelling.
        return report;
    }

    // ── Case 2: lone-legacy-spelling renames (auto-safe, rename-only). ───
    for slot in DatabaseSlot::ALL {
        let to = state_dir.join(slot.canonical_name());
        let from = if slot == DatabaseSlot::Sessions {
            // A lone legacy spelling, or a lone named file, renames to the
            // canonical name; two or more copies were reconciled above.
            match sessions_copies(state_dir, session_db_override).as_slice() {
                [only] if *only != to => only.clone(),
                _ => continue,
            }
        } else {
            let Ok(resolved) = layout.resolve_database(slot) else {
                continue; // twins were resolved (or absent) above
            };
            let DatabaseProvenance::LegacySpelling(_) = resolved.provenance else {
                continue;
            };
            resolved.path
        };
        let entry = rename_to_canonical(slot, &from, &to, apply, &mut report.errors);
        if entry.action == RenameAction::Renamed
            && slot != DatabaseSlot::AgentMemory
            && let Some(fence) = fence.as_mut()
            && let Err(error) = fence.cover_renamed(&to)
        {
            report.errors.push(format!(
                "renamed {slot} store is not fenced at its canonical path {}: {error}; \
                 skipping further mutation of this database",
                to.display()
            ));
            unfenced.push(to.clone());
        }
        report.renames.push(entry);
    }

    // ── `--session-db`: the tripwire at the named file's old path. ──────
    if let Some(plan) = &session_db_plan
        && apply
    {
        settle_session_db(plan, &mut report);
    }

    // ── Case 1: ledger baseline. ─────────────────────────────────────────
    let before = read_ledger_matrix(state_dir);
    if apply {
        let mut backfill_report: Option<HeadCanonicalBackfillReport> = None;
        open_stores_through_ledgered_constructors(
            &layout,
            &unfenced,
            &mut report.errors,
            &mut backfill_report,
            acknowledged_rows,
        );
        report.head_canonical_backfill = backfill_report;
        let after = read_ledger_matrix(state_dir);
        let before_of = |database: &Path, domain: &str| -> Option<i64> {
            before
                .iter()
                .find(|(db, dom, _)| db == database && dom == domain)
                .and_then(|(_, _, version)| *version)
        };
        for (database, domain, after_version) in after {
            let class = ledger_domain_class(&domain);
            let before_version = before_of(&database, &domain);
            let (action, after_field) = match class {
                LedgerDomainClass::Stampable => {
                    if after_version == before_version {
                        (LedgerBaselineAction::AlreadyCurrent, after_version)
                    } else {
                        (LedgerBaselineAction::Stamped, after_version)
                    }
                }
                LedgerDomainClass::Exempt => (LedgerBaselineAction::Exempt, None),
                LedgerDomainClass::ReportOnly => (LedgerBaselineAction::ReportOnly, None),
            };
            report.ledger.push(LedgerBaselineEntry {
                database,
                domain,
                before: before_version,
                after: after_field,
                action,
            });
        }
    } else {
        for (database, domain, version) in before {
            let action = match ledger_domain_class(&domain) {
                LedgerDomainClass::Stampable => {
                    if version.is_some() {
                        LedgerBaselineAction::Recorded
                    } else {
                        LedgerBaselineAction::WouldStamp
                    }
                }
                LedgerDomainClass::Exempt => LedgerBaselineAction::Exempt,
                LedgerDomainClass::ReportOnly => LedgerBaselineAction::ReportOnly,
            };
            report.ledger.push(LedgerBaselineEntry {
                database,
                domain,
                before: version,
                after: None,
                action,
            });
        }
    }
    // The head-canonical continuity bump is the one migration a gateway open
    // deliberately never commits (it locks previous releases out of the
    // file), so the operator has to see it coming.
    if report.ledger.iter().any(|entry| {
        entry.domain == "meta-continuity"
            && entry.before.is_some_and(|version| {
                version < crate::identity_first::HEAD_CANONICAL_CONTINUITY_SCHEMA_VERSION
            })
    }) {
        report.notes.push(format!(
            "continuity database is below the head-canonical schema version \
             (meta-continuity v{}); {} — this bump is ONE-WAY: binaries older than this \
             release refuse a stamped file at open (SchemaFromTheFuture). Back up continuity.* \
             before applying.",
            crate::identity_first::HEAD_CANONICAL_CONTINUITY_SCHEMA_VERSION,
            if apply {
                "--apply committed it under the exclusive maintenance fence"
            } else {
                "--apply would commit it under the exclusive maintenance fence, and the first \
                 incremental session write would otherwise commit it lazily"
            }
        ));
    }
    if state_dir.join(WORKGRAPH_ADMISSION_SIDECAR_FILE).is_file() {
        report.notes.push(
            "workgraph admission sidecar is ledger-exempt by design (M3): the lock database \
             deliberately carries no tables; stamping a ledger row on open would contend the \
             cross-process admission lock"
                .to_string(),
        );
    }
    if report
        .ledger
        .iter()
        .any(|entry| entry.action == LedgerBaselineAction::ReportOnly)
    {
        report.notes.push(
            "sessions / runtime / schedule / workgraph are meerkat-owned stores: their ledgers \
             are reported read-only here and converge through the owning meerkat store's next \
             open (normal gateway boot)"
                .to_string(),
        );
    }

    // Cases 4 and 6 (continuity checkpoint adoption, digest-format marker
    // stamping) were retired with the 0.8.11 reset: the checkpoint stamps
    // adoption minted and the marker the stamping walk respelled are
    // vocabulary meerkat no longer reads. Released 0.8.10 documents now
    // surface in the doctor census and convert through the explicit
    // one-time importer instead.

    // A DRY RUN MUST STILL REPORT THE CENSUS. The whole purpose of dry run
    // is to answer "what would --apply do to this corpus", and the answer is
    // dominated by how many legacy sessions would be converted. Without this
    // the operator gets renames and leftovers and total silence on the one
    // irreversible thing the verb does.
    //
    // The engine's own dry-run path opens READ-ONLY and mutates nothing,
    // including the journal mode, so this is safe to run in the same mode
    // that promises to change nothing.
    if !apply
        && let Ok(resolved) = layout.continuity_db()
        && resolved.path.is_file()
        && !unfenced.contains(&resolved.path)
    {
        match LocalContinuityStore::backfill_head_canonical_sessions_at(
            &resolved.path,
            false,
            acknowledged_rows,
        ) {
            Ok(census) => report.head_canonical_backfill = Some(census),
            Err(error) => report
                .errors
                .push(format!("continuity head-canonical census failed: {error}")),
        }
    }

    // ── Case 5: deprecated leftovers (report-only). ──────────────────────
    let scope = DiagnoseScope::new(vec![state_dir.to_path_buf()]);
    let diagnosis = storage_doctor::diagnose_state_dir_blocking(&scope, None);
    report.findings.extend(
        diagnosis
            .findings
            .into_iter()
            .filter(|finding| LEFTOVER_FINDING_CODES.contains(&finding.code.as_str())),
    );
    sweep_runtime_registry(state_dir, &mut report.findings, &mut report.notes);

    drop(fence);
    report
}

/// Ledger-domain classes for case 1.
enum LedgerDomainClass {
    /// A Meta-owned domain whose M3 constructor this verb runs.
    Stampable,
    /// The admission sidecar (no ledger by design).
    Exempt,
    /// A meerkat-owned domain (reported read-only).
    ReportOnly,
}

fn ledger_domain_class(domain: &str) -> LedgerDomainClass {
    match domain {
        // `mob` is the child-mob databases' Meerkat domain; like `jobs`, this
        // verb runs Meerkat's own constructors for it under the fence.
        "meta-continuity" | "meta-metadata" | "meta-console" | "jobs" | "mob" => {
            LedgerDomainClass::Stampable
        }
        "meta-workgraph-admission" => LedgerDomainClass::Exempt,
        _ if domain == MEMORY_LEDGER_DOMAIN => LedgerDomainClass::Stampable,
        _ => LedgerDomainClass::ReportOnly,
    }
}

/// Read the schema-ledger rows of one database, read-only. `Ok(None)` = no
/// `meerkat_schema` table (pre-ledger file).
fn read_domain_versions(db_path: &Path) -> Result<Option<Vec<(String, i64)>>, String> {
    let conn = meerkat_sqlite::open(db_path, meerkat_sqlite::ConnectionProfile::ReadOnly)
        .map_err(|error| error.to_string())?;
    if !table_exists(&conn, "meerkat_schema").map_err(|error| error.to_string())? {
        return Ok(None);
    }
    let result = (|| -> Result<Vec<(String, i64)>, rusqlite::Error> {
        let mut statement =
            conn.prepare("SELECT domain, version FROM meerkat_schema ORDER BY domain")?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })();
    result.map(Some).map_err(|error| error.to_string())
}

fn table_exists(conn: &rusqlite::Connection, table: &str) -> Result<bool, rusqlite::Error> {
    use rusqlite::OptionalExtension;
    Ok(conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [table],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Ledger versions for every inventoried database currently on disk:
/// `(database, domain, version)` for ledger rows, `None` for expected
/// domains without a row. Read-only.
fn read_ledger_matrix(state_dir: &Path) -> Vec<(PathBuf, String, Option<i64>)> {
    let mut matrix = Vec::new();
    let mut push_db = |db_path: PathBuf, expected: &[&str]| {
        let rows = read_domain_versions(&db_path)
            .ok()
            .flatten()
            .unwrap_or_default();
        for (domain, version) in &rows {
            matrix.push((db_path.clone(), domain.clone(), Some(*version)));
        }
        for domain in expected {
            if !rows.iter().any(|(name, _)| name == domain) {
                matrix.push((db_path.clone(), (*domain).to_string(), None));
            }
        }
    };
    for family in DATABASE_FAMILIES {
        for spelling in family.spellings {
            let db_path = state_dir.join(spelling);
            if db_path.is_file() {
                push_db(db_path, family.ledger_domains);
            }
        }
    }
    for spelling in MEMORY_ROOT_SPELLINGS {
        let root = state_dir.join(spelling);
        let Ok(entries) = fs::read_dir(&root) else {
            continue;
        };
        let mut realm_dbs: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("sqlite3")
            })
            .collect();
        realm_dbs.sort();
        for db_path in realm_dbs {
            push_db(db_path, &[MEMORY_LEDGER_DOMAIN]);
        }
    }
    for jobs in crate::storage_predecessor::realm_jobs_databases(state_dir) {
        push_db(jobs, &["jobs"]);
    }
    for db_path in crate::child_mob_storage::child_mob_databases(state_dir) {
        push_db(db_path, &["mob"]);
    }
    matrix
}

/// Case 1 apply: run each existing Meta-owned database through its normal
/// M3 constructor. The guarded ledger migrations are the structural
/// verification; a constructor that fails is a per-store error, never an
/// abort of the pass. Only files that already exist are opened — migrate
/// converges existing state, it never materializes new stores. Databases in
/// `unfenced` (canonical fence lost after a rename) are skipped fail-closed;
/// the fence failure is already recorded as an error.
fn open_stores_through_ledgered_constructors(
    layout: &MetaStorageLayout,
    unfenced: &[PathBuf],
    errors: &mut Vec<String>,
    backfill_report: &mut Option<HeadCanonicalBackfillReport>,
    acknowledged_rows: &std::collections::BTreeSet<String>,
) {
    match layout.continuity_db() {
        Ok(resolved) if resolved.path.is_file() && !unfenced.contains(&resolved.path) => {
            if let Err(error) = LocalContinuityStore::open(&resolved.path) {
                errors.push(format!("continuity store open failed: {error}"));
            } else {
                // The head-canonical (`meta-continuity` v2) bump is the one
                // migration a plain gateway open deliberately does NOT
                // commit: it locks previous releases out of the file, so it
                // is committed only here, under the exclusive maintenance
                // fence, as an explicit operator action — or lazily, in the
                // transaction of a delta write that actually creates a head
                // row (a refused write leaves the file at v1).
                //
                // It is committed here ONLY BEHIND THE BACKFILL. Applying the
                // schema alone used to stamp v2 and create the three empty
                // head-canonical tables while converting nothing, so an
                // operator paid the whole one-way cost — a file no `<= 0.8.5`
                // binary can open again — and received a corpus still wholly
                // on the legacy blob path. The stamp now advances only when
                // every legacy blob carries a head row, so a partial or
                // failed crossing leaves the file at v1 and rollback intact.
                match LocalContinuityStore::backfill_head_canonical_sessions_at(
                    &resolved.path,
                    true,
                    acknowledged_rows,
                ) {
                    Ok(backfill) => {
                        for (session_id, failure) in &backfill.failures {
                            errors.push(if session_id.is_empty() {
                                format!("continuity head-canonical backfill refused: {failure}")
                            } else {
                                format!(
                                    "continuity head-canonical backfill failed for session \
                                     {session_id}: {failure}"
                                )
                            });
                        }
                        *backfill_report = Some(backfill);
                    }
                    Err(error) => errors.push(format!(
                        "continuity head-canonical backfill failed: {error}"
                    )),
                }
            }
        }
        Ok(_) => {}
        Err(error) => errors.push(format!("continuity locator unresolved: {error}")),
    }
    match layout.metadata_db() {
        Ok(resolved) if resolved.path.is_file() && !unfenced.contains(&resolved.path) => {
            if let Err(error) = SqliteMetadataStore::open(&resolved.path) {
                errors.push(format!("metadata store open failed: {error}"));
            }
        }
        Ok(_) => {}
        Err(error) => errors.push(format!("metadata locator unresolved: {error}")),
    }
    match layout.console_db() {
        Ok(resolved) if resolved.path.is_file() && !unfenced.contains(&resolved.path) => {
            if let Err(error) = SqliteConsoleLogStore::open(&resolved.path) {
                errors.push(format!("console store open failed: {error}"));
            }
        }
        Ok(_) => {}
        Err(error) => errors.push(format!("console locator unresolved: {error}")),
    }
    match layout.agent_memory_root() {
        Ok(resolved) if resolved.path.is_dir() => {
            match SqliteAgentMemoryStore::open(&resolved.path) {
                Ok(store) => match store.known_realms() {
                    Ok(realms) => {
                        for realm in realms {
                            if let Err(error) = store.open_realm_ledgered(&realm) {
                                errors.push(format!(
                                    "agent-memory realm '{realm}' open failed: {error}"
                                ));
                            }
                        }
                    }
                    Err(error) => {
                        errors.push(format!("agent-memory realm listing failed: {error}"));
                    }
                },
                Err(error) => errors.push(format!("agent-memory store open failed: {error}")),
            }
        }
        Ok(_) => {}
        Err(error) => errors.push(format!("agent-memory locator unresolved: {error}")),
    }
    match layout.jobs_db() {
        Ok(jobs) => {
            if jobs.is_file()
                && !unfenced.contains(&jobs)
                && let Err(error) = meerkat::SqliteDetachedJobStore::open(&jobs)
            {
                errors.push(format!("detached-job store open failed: {error}"));
            }
        }
        Err(error) => errors.push(format!("detached-job store locator unresolved: {error}")),
    }
    // Durable child mobs: Meerkat owns this schema, so its own constructors
    // run the guarded migrations, under the same state-wide fence. No second
    // migration owner.
    for database in crate::child_mob_storage::child_mob_databases(layout.state_dir()) {
        if unfenced.contains(&database) {
            continue;
        }
        if let Err(error) = open_child_mob_database(&database) {
            errors.push(format!(
                "child mob database {} open failed: {error}",
                database.display()
            ));
        }
    }
}

/// Open one child-mob database through the Meerkat constructor that owns it:
/// the realm-scoped profile and custody databases by their reserved names,
/// every other file as a per-mob event log.
fn open_child_mob_database(database: &Path) -> Result<(), String> {
    use crate::child_mob_storage::ChildMobDatabase;
    match ChildMobDatabase::classify(database) {
        ChildMobDatabase::RealmProfiles => {
            meerkat_mob::store::SqliteRealmProfileStore::open(database)
                .map(drop)
                .map_err(|error| error.to_string())
        }
        ChildMobDatabase::ParticipantCustody => {
            meerkat_mob::store::SqliteForkedParticipantStore::open(database)
                .map(drop)
                .map_err(|error| error.to_string())
        }
        ChildMobDatabase::MobEventLog => meerkat_mob::store::SqliteMobStores::open(database)
            .map(drop)
            .map_err(|error| error.to_string()),
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Case 2 mechanics: WAL-safe database moves + registered rename markers.
// ─────────────────────────────────────────────────────────────────────────

fn path_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut os = path.as_os_str().to_os_string();
    os.push(suffix);
    PathBuf::from(os)
}

/// Checkpoint a non-empty WAL (`TRUNCATE`) through a maintenance
/// connection so a subsequent move carries no live frames. Returns whether
/// a checkpoint ran. Refuses (Err) when the WAL cannot be fully
/// checkpointed — moving a database away from live WAL frames would lose
/// them.
fn checkpoint_wal_if_nonempty(db: &Path) -> Result<bool, String> {
    let wal = path_with_suffix(db, "-wal");
    let wal_len = fs::metadata(&wal).map(|meta| meta.len()).unwrap_or(0);
    if wal_len == 0 {
        return Ok(false);
    }
    let conn = meerkat_sqlite::open(
        db,
        meerkat_sqlite::ConnectionProfile::Maintenance { write: true },
    )
    .map_err(|error| format!("open for WAL checkpoint failed: {error}"))?;
    let busy: i64 = conn
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))
        .map_err(|error| format!("wal_checkpoint failed: {error}"))?;
    if busy != 0 {
        return Err(format!(
            "WAL for {} cannot be checkpointed (readers active); refusing to move it",
            db.display()
        ));
    }
    Ok(true)
}

/// Move one database file to a new name, WAL-safely: checkpoint a non-empty
/// WAL first (refusing when impossible), rename the database, then move any
/// remaining `-wal` / `-shm` siblings so they stay paired with their
/// database. Returns the sibling moves and whether a checkpoint ran.
pub(crate) fn move_database_file(
    from: &Path,
    to: &Path,
) -> Result<(Vec<SiblingRename>, bool), String> {
    if to.exists() {
        return Err(format!(
            "cannot move {} to {}: target already exists",
            from.display(),
            to.display()
        ));
    }
    let checkpointed = checkpoint_wal_if_nonempty(from)?;
    fs::rename(from, to)
        .map_err(|error| format!("rename {} -> {}: {error}", from.display(), to.display()))?;
    let mut siblings = Vec::new();
    for suffix in ["-wal", "-shm"] {
        let src = path_with_suffix(from, suffix);
        if !src.exists() {
            continue;
        }
        let dst = path_with_suffix(to, suffix);
        fs::rename(&src, &dst)
            .map_err(|error| format!("rename {} -> {}: {error}", src.display(), dst.display()))?;
        siblings.push(SiblingRename { from: src, to: dst });
    }
    Ok((siblings, checkpointed))
}

/// [`meerkat_store::migrate::backup_artifact_name`] stamped with the Meta version instead of the
/// Meerkat store version: Meta-owned artifacts (the adoption rename marker)
/// record the Meta release that wrote them. Same registered shape, so
/// [`is_registered_backup_artifact_name`] still recognizes it.
pub(crate) fn meta_backup_artifact_name(original: &str, purpose: &str) -> String {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    let version = env!("CARGO_PKG_VERSION");
    format!("{original}.pre-{version}-{timestamp}.{purpose}")
}

/// Write the registered rename marker next to where the legacy file was:
/// `<legacy name>.pre-<version>-<ts>.renamed`, holding a small JSON record.
/// The `.pre-` segment makes it a registered backup artifact — doctor lists
/// it, prune owns its lifecycle, retention tooling recognizes it.
pub(crate) fn write_rename_marker(from: &Path, to: &Path) -> Result<PathBuf, String> {
    let from_name = from
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("{} has no UTF-8 file name", from.display()))?;
    let to_name = to
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let marker = from.with_file_name(meta_backup_artifact_name(from_name, "renamed"));
    let body = serde_json::json!({
        "renamed_from": from_name,
        "renamed_to": to_name,
    });
    let bytes = serde_json::to_vec_pretty(&body).map_err(|error| error.to_string())?;
    fs::write(&marker, bytes)
        .map_err(|error| format!("write rename marker {}: {error}", marker.display()))?;
    Ok(marker)
}

/// Case 2 for one slot: rename the lone legacy spelling to its canonical
/// name (dry-run reports [`RenameAction::WouldRename`]).
fn rename_to_canonical(
    slot: DatabaseSlot,
    from: &Path,
    to: &Path,
    apply: bool,
    errors: &mut Vec<String>,
) -> FileRenameEntry {
    let mut entry = FileRenameEntry {
        slot: slot.to_string(),
        from: from.to_path_buf(),
        to: to.to_path_buf(),
        siblings: Vec::new(),
        wal_checkpointed: false,
        marker: None,
        action: RenameAction::WouldRename,
    };
    if !apply {
        return entry;
    }
    let moved = if slot == DatabaseSlot::AgentMemory {
        // A directory slot: the realm databases (and their WAL siblings)
        // move together with the directory.
        if to.exists() {
            Err(format!(
                "cannot move {} to {}: target already exists",
                from.display(),
                to.display()
            ))
        } else {
            fs::rename(from, to)
                .map_err(|error| format!("rename {} -> {}: {error}", from.display(), to.display()))
                .map(|()| (Vec::new(), false))
        }
    } else {
        move_database_file(from, to)
    };
    match moved {
        Ok((siblings, checkpointed)) => {
            entry.siblings = siblings;
            entry.wal_checkpointed = checkpointed;
            entry.action = RenameAction::Renamed;
            match write_rename_marker(from, to) {
                Ok(marker) => entry.marker = Some(marker),
                Err(error) => errors.push(error),
            }
        }
        Err(error) => {
            entry.action = RenameAction::Refused;
            errors.push(format!("rename of the {slot} store refused: {error}"));
        }
    }
    entry
}

// ─────────────────────────────────────────────────────────────────────────
// Case 3 mechanics: divergence + fail-closed reconciliation.
// ─────────────────────────────────────────────────────────────────────────

/// Digest of one file's bytes; for SQLite databases the `-wal` sidecar (if
/// present) is folded in, so uncheckpointed frames register as divergence
/// (conservative: never reports "equal" for possibly-different content).
fn file_digest(path: &Path) -> Result<[u8; 32], String> {
    let mut hasher = Sha256::new();
    hasher.update(fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?);
    let wal = path_with_suffix(path, "-wal");
    if wal.is_file() {
        hasher.update(fs::read(&wal).map_err(|error| format!("{}: {error}", wal.display()))?);
    }
    Ok(hasher.finalize().into())
}

fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

type RowDigests = BTreeMap<String, [u8; 32]>;

/// Per-row content digests over every user table of one database, keyed
/// `table/<primary key>` (rowid when a table declares no primary key), fed
/// in declared column order so digests are comparable across copies. The
/// `meerkat_schema` ledger table is excluded — twins routinely differ only
/// in whether a copy was ever opened by an M3 binary, and that is not data
/// divergence (the byte-identity check still sees it).
fn row_digests(db_path: &Path) -> Result<RowDigests, String> {
    let conn = meerkat_sqlite::open(db_path, meerkat_sqlite::ConnectionProfile::ReadOnly)
        .map_err(|error| error.to_string())?;
    let inner = || -> Result<RowDigests, rusqlite::Error> {
        let tables: Vec<String> = {
            let mut statement = conn.prepare(
                "SELECT name FROM sqlite_master WHERE type = 'table' \
                 AND name NOT LIKE 'sqlite_%' AND name <> 'meerkat_schema' ORDER BY name",
            )?;
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?
        };
        let mut digests = RowDigests::new();
        for table in tables {
            // Primary-key column positions, in key order.
            let mut pk_columns: Vec<(i64, usize)> = Vec::new();
            conn.pragma(None, "table_info", &table, |row| {
                let cid: i64 = row.get(0)?;
                let pk: i64 = row.get(5)?;
                if pk > 0 {
                    pk_columns.push((pk, cid as usize));
                }
                Ok(())
            })?;
            pk_columns.sort_unstable();

            let (sql, key_indexes, data_start): (String, Vec<usize>, usize) =
                if pk_columns.is_empty() {
                    (
                        format!(
                            "SELECT rowid, * FROM {} ORDER BY rowid",
                            quote_ident(&table)
                        ),
                        vec![0],
                        1,
                    )
                } else {
                    let order: Vec<String> = pk_columns
                        .iter()
                        .map(|(_, cid)| format!("{}", cid + 1))
                        .collect();
                    (
                        format!(
                            "SELECT * FROM {} ORDER BY {}",
                            quote_ident(&table),
                            order.join(", ")
                        ),
                        pk_columns.iter().map(|(_, cid)| *cid).collect(),
                        0,
                    )
                };
            let mut statement = conn.prepare(&sql)?;
            let column_count = statement.column_count();
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                let mut key = format!("{table}/");
                for (position, index) in key_indexes.iter().enumerate() {
                    if position > 0 {
                        key.push('|');
                    }
                    key.push_str(&value_key(row.get_ref(*index)?));
                }
                let mut hasher = Sha256::new();
                for index in data_start..column_count {
                    feed_value(&mut hasher, row.get_ref(index)?);
                }
                digests.insert(key, hasher.finalize().into());
            }
        }
        Ok(digests)
    };
    inner().map_err(|error| format!("{}: {error}", db_path.display()))
}

fn value_key(value: rusqlite::types::ValueRef<'_>) -> String {
    use rusqlite::types::ValueRef;
    match value {
        ValueRef::Null => "null".to_string(),
        ValueRef::Integer(int) => int.to_string(),
        ValueRef::Real(real) => real.to_string(),
        ValueRef::Text(text) => String::from_utf8_lossy(text).into_owned(),
        ValueRef::Blob(blob) => {
            use std::fmt::Write as _;
            blob.iter().fold(String::new(), |mut hex, byte| {
                let _ = write!(hex, "{byte:02x}");
                hex
            })
        }
    }
}

fn feed_value(hasher: &mut Sha256, value: rusqlite::types::ValueRef<'_>) {
    use rusqlite::types::ValueRef;
    let (tag, bytes): (u8, Vec<u8>) = match value {
        ValueRef::Null => (0, Vec::new()),
        ValueRef::Integer(int) => (1, int.to_be_bytes().to_vec()),
        ValueRef::Real(real) => (2, real.to_be_bytes().to_vec()),
        ValueRef::Text(text) => (3, text.to_vec()),
        ValueRef::Blob(blob) => (4, blob.to_vec()),
    };
    hasher.update([tag]);
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(&bytes);
}

/// Per-file digests inside one directory twin (the agent-memory roots),
/// keyed by relative file name.
fn dir_file_digests(dir: &Path) -> Result<RowDigests, String> {
    let mut digests = RowDigests::new();
    let entries = fs::read_dir(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .collect();
    files.sort();
    for file in files {
        let Some(name) = file.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        // WAL/SHM siblings fold into their database's digest.
        if name.ends_with("-wal") || name.ends_with("-shm") {
            continue;
        }
        digests.insert(name.to_string(), file_digest(&file)?);
    }
    Ok(digests)
}

/// Classify per-key divergence across every twin copy: equal keys are
/// counted, divergent and single-copy keys are listed.
fn classify_divergence(
    per_copy: &[(PathBuf, RowDigests)],
    rows_equal: &mut usize,
    rows: &mut Vec<RowDivergenceEntry>,
) {
    let mut all_keys: Vec<String> = per_copy
        .iter()
        .flat_map(|(_, digests)| digests.keys().cloned())
        .collect();
    all_keys.sort();
    all_keys.dedup();
    for key in all_keys {
        let holders: Vec<(&PathBuf, &[u8; 32])> = per_copy
            .iter()
            .filter_map(|(path, digests)| digests.get(&key).map(|digest| (path, digest)))
            .collect();
        if holders.len() == 1 {
            rows.push(RowDivergenceEntry {
                key,
                status: DivergenceStatus::OnlyIn {
                    location: holders[0].0.clone(),
                },
            });
        } else if holders.len() == per_copy.len()
            && holders.iter().all(|(_, digest)| *digest == holders[0].1)
        {
            *rows_equal += 1;
        } else {
            rows.push(RowDivergenceEntry {
                key,
                status: DivergenceStatus::Divergent,
            });
        }
    }
}

/// Archive one twin copy read-only under the registered backup naming,
/// folding a non-empty WAL into the database first so the archive is
/// self-contained; any leftover siblings are archived alongside (their
/// names inherit the `.pre-` segment). Directories archive as a unit.
fn archive_twin_copy(
    path: &Path,
    purpose: &str,
    apply_checkpoint: bool,
) -> Result<Vec<PathBuf>, String> {
    if path.is_file() && apply_checkpoint {
        checkpoint_wal_if_nonempty(path)?;
    }
    let siblings: Vec<PathBuf> = ["-wal", "-shm"]
        .iter()
        .map(|suffix| path_with_suffix(path, suffix))
        .filter(|sibling| sibling.exists())
        .collect();
    let archive = archive_path_read_only(path, purpose).map_err(|error| error.to_string())?;
    let mut archived = vec![archive.clone()];
    for sibling in siblings {
        let Some(suffix) = sibling
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.rsplit_once('-').map(|(_, tail)| format!("-{tail}")))
        else {
            continue;
        };
        let dst = path_with_suffix(&archive, &suffix);
        fs::rename(&sibling, &dst)
            .map_err(|error| format!("archive sibling {}: {error}", sibling.display()))?;
        if let Ok(metadata) = fs::symlink_metadata(&dst) {
            let mut permissions = metadata.permissions();
            permissions.set_readonly(true);
            let _ = fs::set_permissions(&dst, permissions);
        }
        archived.push(dst);
    }
    Ok(archived)
}

/// The command that adopts the copy `--session-db` named, with the state
/// directory as the run was given it. The named file's parent cannot stand
/// in for it: a bare file name's parent is empty.
fn session_db_adopt_command(state_dir: &Path, named: &Path) -> String {
    format!(
        "meta_gateway storage-migrate --state-dir {state_dir} --session-db {named} --apply \
         --adopt {named}",
        state_dir = state_dir.display(),
        named = named.display(),
    )
}

/// Case 3 for one slot: compute the per-domain divergence report, then
/// refuse, dedup (exact equality), or adopt-and-archive. A canonicalization
/// rename that cannot be re-fenced at its new path records a pass error in
/// `errors` and the path in `unfenced` (the later cases skip it fail-closed).
#[allow(clippy::too_many_arguments)]
fn reconcile_twin(
    slot: DatabaseSlot,
    paths: &[PathBuf],
    mode: MigrateMode,
    adopt: Option<&Path>,
    renames: &mut Vec<FileRenameEntry>,
    fence: Option<&mut MetaMaintenanceFence>,
    errors: &mut Vec<String>,
    unfenced: &mut Vec<PathBuf>,
    state_dir: &Path,
    session_db: Option<&Path>,
) -> TwinReport {
    // With `--session-db`, a divergent twin's refusal names the copy 0.9.3
    // served and the exact command that adopts it.
    let divergent_reason =
        || match session_db.filter(|named| paths.iter().any(|path| path == named)) {
            Some(named) => format!(
                "divergent twin copies; {named} is the copy 0.9.3 read and wrote while its \
             store_path named that file: rerun with `{command}` to adopt it and archive the \
             other copies read-only (no synthesis)",
                named = named.display(),
                command = session_db_adopt_command(state_dir, named),
            ),
            None => "divergent twin copies; rerun with `--apply --adopt <path>` to adopt one copy \
                 and archive the rest read-only (no synthesis)"
                .to_string(),
        };
    let mut twin = TwinReport {
        slot: slot.to_string(),
        paths: paths.to_vec(),
        byte_identical: false,
        rows_equal: 0,
        rows: Vec::new(),
        resolution: TwinResolution::Refused {
            reason: "unresolved".to_string(),
        },
        notes: Vec::new(),
        errors: Vec::new(),
    };
    match slot {
        DatabaseSlot::Continuity => twin.notes.push(
            "no synthesis: continuity fencing tokens are per-database sequences; merging twin \
             histories would corrupt CAS — adopt one copy, archive the rest"
                .to_string(),
        ),
        DatabaseSlot::Console => twin.notes.push(
            "no synthesis: console cursor_seq is a per-database AUTOINCREMENT sequence; merging \
             twin timelines would corrupt cursor replay — adopt one copy, archive the rest"
                .to_string(),
        ),
        _ => {}
    }

    // Divergence: row-level for the three row-compared domains, per-file for
    // the memory directory twin, file-digest for everything else.
    let row_level = matches!(
        slot,
        DatabaseSlot::Continuity | DatabaseSlot::Metadata | DatabaseSlot::Console
    );
    if row_level || slot == DatabaseSlot::AgentMemory {
        let mut per_copy: Vec<(PathBuf, RowDigests)> = Vec::new();
        for path in paths {
            let digests = if row_level {
                row_digests(path)
            } else {
                dir_file_digests(path)
            };
            match digests {
                Ok(digests) => per_copy.push((path.clone(), digests)),
                Err(error) => {
                    twin.errors.push(format!(
                        "divergence unavailable for {}: {error}",
                        path.display()
                    ));
                    per_copy.push((path.clone(), RowDigests::new()));
                }
            }
        }
        classify_divergence(&per_copy, &mut twin.rows_equal, &mut twin.rows);
    } else {
        twin.notes
            .push("divergence computed at file-digest level for this slot".to_string());
    }

    // Byte identity (exact-equality dedup precondition): file digests with
    // the WAL folded in; directory twins compare their full file maps.
    let byte_identical = if slot == DatabaseSlot::AgentMemory {
        let mut maps = Vec::new();
        let mut readable = true;
        for path in paths {
            match dir_file_digests(path) {
                Ok(map) => maps.push(map),
                Err(_) => {
                    readable = false;
                    break;
                }
            }
        }
        readable && maps.windows(2).all(|pair| pair[0] == pair[1])
    } else {
        let mut digests = Vec::new();
        let mut readable = true;
        for path in paths {
            match file_digest(path) {
                Ok(digest) => digests.push(digest),
                Err(error) => {
                    twin.errors.push(format!(
                        "file digest unavailable for {}: {error}",
                        path.display()
                    ));
                    readable = false;
                    break;
                }
            }
        }
        readable && digests.windows(2).all(|pair| pair[0] == pair[1])
    };
    twin.byte_identical = byte_identical;

    if mode != MigrateMode::Apply {
        twin.resolution = TwinResolution::Refused {
            reason: if byte_identical {
                "twin copies are byte-identical; rerun with `--apply` to dedup (keep the \
                 canonical spelling, archive the redundant copy read-only)"
                    .to_string()
            } else {
                divergent_reason()
            },
        };
        return twin;
    }

    // Apply: --adopt wins; byte-identical twins dedup; anything else refuses.
    let canonical_of = |path: &Path| fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let adopted_member = adopt.and_then(|adopt_path| {
        let adopt_canonical = canonical_of(adopt_path);
        paths
            .iter()
            .find(|path| canonical_of(path) == adopt_canonical)
            .cloned()
    });
    let (kept, purpose, dedup) = if let Some(adopted) = adopted_member {
        (adopted, "twin", false)
    } else if byte_identical {
        let canonical_name = slot.canonical_name();
        let kept = paths
            .iter()
            .find(|path| path.file_name().and_then(|name| name.to_str()) == Some(canonical_name))
            .unwrap_or(&paths[0])
            .clone();
        (kept, "twin-dedup", true)
    } else {
        twin.resolution = TwinResolution::Refused {
            reason: if adopt.is_some() {
                format!(
                    "--adopt does not name one of this slot's twin copies (candidates: {})",
                    paths
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            } else {
                divergent_reason()
            },
        };
        return twin;
    };

    // Archive the non-kept copies first (freeing the canonical name when the
    // kept copy is legacy-named), then canonicalize the kept copy.
    let mut archived = Vec::new();
    for path in paths {
        if *path == kept {
            continue;
        }
        match archive_twin_copy(path, purpose, true) {
            Ok(mut paths) => archived.append(&mut paths),
            Err(error) => {
                twin.resolution = TwinResolution::Refused {
                    reason: format!("archive of {} failed: {error}", path.display()),
                };
                return twin;
            }
        }
    }
    let canonical_path = kept.parent().map_or_else(
        || PathBuf::from(slot.canonical_name()),
        |parent| parent.join(slot.canonical_name()),
    );
    let final_path = if kept == canonical_path {
        kept
    } else {
        let mut rename_errors = Vec::new();
        let entry = rename_to_canonical(slot, &kept, &canonical_path, true, &mut rename_errors);
        let renamed = entry.action == RenameAction::Renamed;
        renames.push(entry);
        if renamed {
            if slot != DatabaseSlot::AgentMemory
                && let Some(fence) = fence
                && let Err(error) = fence.cover_renamed(&canonical_path)
            {
                errors.push(format!(
                    "adopted {slot} store is not fenced at its canonical path {}: {error}; \
                     skipping further mutation of this database",
                    canonical_path.display()
                ));
                unfenced.push(canonical_path.clone());
            }
            canonical_path
        } else {
            twin.resolution = TwinResolution::Refused {
                reason: format!(
                    "adopted copy could not be renamed to its canonical name: {}",
                    rename_errors.join("; ")
                ),
            };
            return twin;
        }
    };
    twin.resolution = if dedup {
        TwinResolution::Deduped {
            kept: final_path,
            archived,
        }
    } else {
        TwinResolution::Adopted {
            adopted: final_path,
            archived,
        }
    };
    twin
}

// ─────────────────────────────────────────────────────────────────────────
// `--session-db`: the session database a 0.9.3 `store_path` named.
// ─────────────────────────────────────────────────────────────────────────

/// A `--session-db` file the run takes in.
struct SessionDbPlan {
    /// The file as the operator named it.
    named: PathBuf,
    /// Whether the name is one of the Sessions slot's own spellings (a legacy
    /// one: the canonical file is a no-op). The ordinary passes move a legacy
    /// spelling; the run only adds the tripwire.
    legacy_spelling: bool,
    /// An earlier run moved the file and did not write its retired stamp
    /// (see [`SessionDbOutcome::TripwireOwed`]).
    tripwire_owed: bool,
}

impl SessionDbPlan {
    /// The file when it carries a name the layout does not know: the one
    /// the run adds to the Sessions slot.
    fn override_path(&self) -> Option<&Path> {
        (!self.legacy_spelling).then_some(self.named.as_path())
    }
}

/// The first bytes of every SQLite database file.
const SQLITE_HEADER: &[u8; 16] = b"SQLite format 3\0";

/// The read-only checks `--session-db` runs before the fence: what the named
/// file is, and whether the directory may take it in. A no-op or a refusal is
/// the outcome to report.
fn plan_session_db(state_dir: &Path, named: &Path) -> Result<SessionDbPlan, SessionDbOutcome> {
    let refuse = |refusal: SessionDbRefusal| SessionDbOutcome::Refused { refusal };
    let unreadable = |path: &Path, detail: String| {
        refuse(SessionDbRefusal::Unreadable {
            path: path.to_path_buf(),
            detail,
        })
    };
    match fs::metadata(named) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => {
            return Err(refuse(SessionDbRefusal::NotAFile {
                path: named.to_path_buf(),
            }));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if tripwire_owed(state_dir, named) {
                let legacy_spelling = named
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| DatabaseSlot::Sessions.legacy_names().contains(&name));
                return Ok(SessionDbPlan {
                    named: named.to_path_buf(),
                    legacy_spelling,
                    tripwire_owed: true,
                });
            }
            return Err(refuse(SessionDbRefusal::Missing {
                path: named.to_path_buf(),
            }));
        }
        Err(error) => return Err(unreadable(named, error.to_string())),
    }
    // Canonicalized on both sides, so a symlinked --state-dir neither
    // refuses its own file nor accepts a file outside it.
    let file = fs::canonicalize(named).map_err(|error| unreadable(named, error.to_string()))?;
    let dir =
        fs::canonicalize(state_dir).map_err(|error| unreadable(state_dir, error.to_string()))?;
    if file.parent() != Some(dir.as_path()) {
        return Err(refuse(SessionDbRefusal::OutsideStateDir {
            path: named.to_path_buf(),
            state_dir: state_dir.to_path_buf(),
        }));
    }
    let name = file.file_name().and_then(|name| name.to_str());
    if name == Some(DatabaseSlot::Sessions.canonical_name()) {
        return Err(SessionDbOutcome::AlreadyInLayout);
    }
    let legacy_spelling =
        name.is_some_and(|name| DatabaseSlot::Sessions.legacy_names().contains(&name));

    // Header first: a file that is not SQLite is never opened, so the check
    // leaves nothing beside it.
    let mut header = [0_u8; 16];
    let is_sqlite = match fs::File::open(named) {
        Ok(mut handle) => {
            use std::io::Read as _;
            handle.read_exact(&mut header).is_ok() && &header == SQLITE_HEADER
        }
        Err(error) => return Err(unreadable(named, error.to_string())),
    };
    if !is_sqlite {
        return Err(refuse(SessionDbRefusal::NotASessionDatabase {
            path: named.to_path_buf(),
            detail: "not a SQLite database".to_string(),
        }));
    }
    let conn = meerkat_sqlite::open(named, meerkat_sqlite::ConnectionProfile::ReadOnly)
        .map_err(|error| unreadable(named, error.to_string()))?;
    if crate::storage_migrate_native::layout_status::is_retired(&conn, SESSION_STORE_DOMAIN.name)
        .map_err(|detail| unreadable(named, detail))?
    {
        return Err(SessionDbOutcome::AlreadyMoved);
    }
    let version = meerkat_sqlite::domain_version(&conn, SESSION_STORE_DOMAIN.name)
        .map_err(|error| unreadable(named, error.to_string()))?;
    drop(conn);
    if version.is_none() {
        return Err(refuse(SessionDbRefusal::NotASessionDatabase {
            path: named.to_path_buf(),
            detail: format!("no `{}` schema row", SESSION_STORE_DOMAIN.name),
        }));
    }
    match crate::storage_migrate_native::runtime_conversion_began(state_dir) {
        Ok(false) => {}
        Ok(true) => {
            return Err(refuse(SessionDbRefusal::NativeRuntimeAlreadyStarted {
                state_dir: state_dir.to_path_buf(),
            }));
        }
        Err(detail) => {
            return Err(unreadable(
                &state_dir.join(crate::storage_layout::RUNTIME_DB_FILE_NAME),
                detail,
            ));
        }
    }
    Ok(SessionDbPlan {
        named: named.to_path_buf(),
        legacy_spelling,
        tripwire_owed: false,
    })
}

/// Whether `named`, which is missing, is a file an earlier `--session-db`
/// run moved before a crash kept it from writing the retired stamp: `named`
/// names a file directly in the state directory, `sessions.sqlite3` is in
/// place, and a registered artifact of the file (`<name>.pre-...`: its
/// rename marker, or its archive after a dedup or an adopt) lies beside it.
fn tripwire_owed(state_dir: &Path, named: &Path) -> bool {
    let Some(name) = named.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let parent = match named.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let in_state_dir = matches!(
        (fs::canonicalize(parent), fs::canonicalize(state_dir)),
        (Ok(parent), Ok(dir)) if parent == dir
    );
    let prefix = format!("{name}.pre-");
    in_state_dir
        && state_dir
            .join(DatabaseSlot::Sessions.canonical_name())
            .is_file()
        && fs::read_dir(parent).is_ok_and(|entries| {
            entries.flatten().any(|entry| {
                entry.file_name().to_str().is_some_and(|file| {
                    file.starts_with(&prefix) && is_registered_backup_artifact_name(file)
                })
            })
        })
}

/// The census's runtime-lock refusal, as `--session-db` reports it.
fn runtime_lock_refusal(
    state_dir: &Path,
    refusal: crate::storage_migrate_native::NativeMigrateRefusal,
) -> SessionDbRefusal {
    match refusal {
        crate::storage_migrate_native::NativeMigrateRefusal::RuntimeInUse { file } => {
            SessionDbRefusal::RuntimeInUse {
                file: PathBuf::from(file),
            }
        }
        crate::storage_migrate_native::NativeMigrateRefusal::RuntimeLockUnreadable {
            file,
            detail,
        } => SessionDbRefusal::Unreadable {
            path: PathBuf::from(file),
            detail,
        },
        // The lock check returns only the two refusals above. Any other one
        // is reported against the state directory, in the census's own
        // serialized form (the refusal has no Display text).
        other => SessionDbRefusal::Unreadable {
            path: state_dir.to_path_buf(),
            detail: serde_json::to_string(&other).unwrap_or_else(|error| error.to_string()),
        },
    }
}

/// The copies of the Sessions slot in `state_dir`, canonical first, then
/// the legacy spellings, then `named` (the file `--session-db` names) while
/// it is still in place. A legacy spelling that holds the session-store
/// retired stamp is the tripwire an earlier `--session-db` run left there,
/// not a copy: it is never a twin and never renamed.
fn sessions_copies(state_dir: &Path, named: Option<&Path>) -> Vec<PathBuf> {
    let slot = DatabaseSlot::Sessions;
    let canonical = state_dir.join(slot.canonical_name());
    let mut copies: Vec<PathBuf> = canonical
        .exists()
        .then_some(canonical)
        .into_iter()
        .collect();
    copies.extend(
        slot.legacy_names()
            .iter()
            .map(|name| state_dir.join(name))
            .filter(|path| path.exists() && !is_session_store_tripwire(path)),
    );
    if let Some(named) = named
        && named.exists()
        && !copies.iter().any(|copy| copy == named)
    {
        copies.push(named.to_path_buf());
    }
    copies
}

/// Whether `path` is the session-store retired stamp, by the census's own
/// reading. Anything that cannot be read as one (not SQLite, unreadable, or
/// stamped with other content) is not, so it stays a copy and the twin rules
/// judge it.
pub(crate) fn is_session_store_tripwire(path: &Path) -> bool {
    crate::storage_migrate_native::layout_status::is_retired_stamp_file(
        path,
        SESSION_STORE_DOMAIN.name,
    )
    .unwrap_or(false)
}

/// After an apply run's twin and rename cases: once the named file has left
/// its path (renamed, adopted or archived), leave the retired stamp there.
fn settle_session_db(plan: &SessionDbPlan, report: &mut MetaMigrateReport) {
    let Some(entry) = report.session_db.as_mut() else {
        return;
    };
    if plan.named.exists() {
        // An unresolved twin stopped the run, or the rename refused: the
        // file is where it was, and the run's errors say why.
        return;
    }
    match write_session_db_tripwire(&plan.named) {
        Ok(()) => {
            entry.outcome = SessionDbOutcome::Moved {
                tripwire: plan.named.clone(),
            };
        }
        Err(error) => report.errors.push(format!(
            "the session database moved, but its tripwire at {} could not be written: {error}; \
             a 0.9.3 gateway started with that store_path would start empty there",
            plan.named.display()
        )),
    }
}

/// The retired stamp of the session-store domain at `path`, synced with its
/// directory: a 0.9.3 gateway started again with the `store_path` that named
/// it refuses the file as a schema from the future instead of starting empty.
fn write_session_db_tripwire(path: &Path) -> Result<(), String> {
    crate::storage_migrate_native::layout_status::write_retired_stamp(
        path,
        SESSION_STORE_DOMAIN.name,
    )?;
    fs::File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|error| format!("sync {}: {error}", path.display()))?;
    if let Some(dir) = path.parent() {
        fs::File::open(dir)
            .and_then(|dir| dir.sync_all())
            .map_err(|error| format!("sync {}: {error}", dir.display()))?;
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────
// Case 5: dead runtime-registry entries.
// ─────────────────────────────────────────────────────────────────────────

/// Liveness from a `/proc`-style directory: `Some(true)` when the pid's
/// entry exists, `Some(false)` only when its absence is proven (not found),
/// `None` on any other lookup error.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn proc_entry_liveness(proc_root: &Path, pid: u64) -> Option<bool> {
    proc_root.join(pid.to_string()).try_exists().ok()
}

/// `Some(true)` when the pid is alive, `Some(false)` only when its absence
/// is proven, `None` when the probe is unavailable or inconclusive. A pid
/// owned by another user still probes correctly.
fn pid_alive(pid: u64) -> Option<bool> {
    // Linux: `/proc` answers directly once it is proven mounted. A lookup
    // error is unknown, never absence.
    #[cfg(target_os = "linux")]
    if matches!(Path::new("/proc/self").try_exists(), Ok(true)) {
        return proc_entry_liveness(Path::new("/proc"), pid);
    }
    #[cfg(unix)]
    {
        // `ps -p <pid> -o pid=` prints the pid when it exists and exits 1
        // with no output when it does not. Anything else (no `ps`, an error
        // message, another exit code) proves nothing.
        let output = std::process::Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "pid="])
            .output()
            .ok()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        if output.status.success() && stdout.trim() == pid.to_string() {
            return Some(true);
        }
        if output.status.code() == Some(1) && stdout.trim().is_empty() && output.stderr.is_empty() {
            return Some(false);
        }
        None
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        None
    }
}

/// Every entry of the runtime registry at `registry_path` with its pid and
/// liveness (`None` when not determinable). A missing registry has no
/// entries; an unreadable or unparseable one is an error.
pub(crate) fn registered_runtime_liveness(
    registry_path: &Path,
) -> Result<Vec<(String, u64, Option<bool>)>, String> {
    let bytes = match fs::read(registry_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("read {}: {error}", registry_path.display())),
    };
    let value = serde_json::from_slice::<serde_json::Value>(&bytes)
        .map_err(|error| format!("decode {}: {error}", registry_path.display()))?;
    // `entries`, and each row's `runtime_id` and `pid`, are required fields
    // of the registry the gateway writes; a registry without them is
    // malformed evidence, never an empty one.
    let entries = value
        .get("entries")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| format!("{} has no entries array", registry_path.display()))?;
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let malformed = |field: &str| {
                format!(
                    "{} row {index} has no valid {field}",
                    registry_path.display()
                )
            };
            let pid = entry
                .get("pid")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| malformed("pid"))?;
            let runtime_id = entry
                .get("runtime_id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| malformed("runtime_id"))?
                .to_string();
            Ok((runtime_id, pid, pid_alive(pid)))
        })
        .collect()
}

/// Report `tux-runtimes.json` entries whose recorded pid is no longer alive
/// (report-only; migrate never edits the registry — the gateway prunes dead
/// entries itself on next boot).
fn sweep_runtime_registry(
    state_dir: &Path,
    findings: &mut Vec<StorageFinding>,
    notes: &mut Vec<String>,
) {
    let registry_path = state_dir.join(RUNTIME_REGISTRY_FILE_NAME);
    if !registry_path.is_file() {
        return;
    }
    let Ok(bytes) = fs::read(&registry_path) else {
        return;
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        notes.push(format!(
            "runtime registry {} is unparseable; dead-entry census skipped",
            registry_path.display()
        ));
        return;
    };
    let Some(entries) = value.get("entries").and_then(serde_json::Value::as_array) else {
        return;
    };
    for entry in entries {
        let runtime_id = entry
            .get("runtime_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("<unknown>");
        let Some(pid) = entry.get("pid").and_then(serde_json::Value::as_u64) else {
            continue;
        };
        match pid_alive(pid) {
            Some(false) => findings.push(
                StorageFinding::new(
                    FindingSeverity::Info,
                    FINDING_DEAD_RUNTIME_REGISTRY_ENTRY,
                    format!(
                        "runtime registry entry '{runtime_id}' records pid {pid}, which is no \
                         longer alive (report-only; the gateway prunes dead entries on next boot)"
                    ),
                )
                .with_path(registry_path.clone()),
            ),
            Some(true) => {}
            None => notes.push(format!(
                "runtime registry entry '{runtime_id}': pid liveness not determinable on this \
                 platform"
            )),
        }
    }
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
        .and_then(|modified| std::time::SystemTime::now().duration_since(modified).ok())
        .map(|age| age.as_secs() / 86_400)
        .unwrap_or(0)
}

fn artifact_kind(name: &str) -> Option<PruneArtifactKind> {
    if is_registered_backup_artifact_name(name) {
        Some(PruneArtifactKind::BackupArtifact)
    } else if is_registered_quarantine_artifact_name(name) {
        Some(PruneArtifactKind::QuarantinedIndex)
    } else {
        None
    }
}

fn push_artifacts_in(dir: &Path, dirs_too: bool, artifacts: &mut Vec<MetaPruneArtifact>) {
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
        artifacts.push(MetaPruneArtifact {
            bytes: recursive_size(&path),
            age_days: age_days(&path),
            path,
            kind,
            action: PruneAction::Kept,
        });
    }
}

/// Enumerate every registered maintenance artifact under one Meta state
/// directory: `*.pre-*` backups (files or archived memory-root directories)
/// and `*.corrupt-*` quarantines at the state-dir root and inside the live
/// agent-memory roots. Artifacts inside an archived directory belong to the
/// archive's own entry. Nothing outside these naming patterns is ever
/// returned — prune's deletion authority is exactly this listing.
pub fn enumerate_state_dir_artifacts(state_dir: &Path) -> Vec<MetaPruneArtifact> {
    let mut artifacts = Vec::new();
    push_artifacts_in(state_dir, true, &mut artifacts);
    for spelling in MEMORY_ROOT_SPELLINGS {
        let root = state_dir.join(spelling);
        if root.is_dir() {
            push_artifacts_in(&root, false, &mut artifacts);
        }
    }
    artifacts
}

/// Whether `path` names a byte copy of the first-boot manifest
/// (`storage-migrate-native.first-boot.json.pre-*`).
fn is_first_boot_manifest_copy(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix(crate::identity_first::FIRST_BOOT_MANIFEST_FILE))
        .is_some_and(|rest| rest.starts_with(".pre-"))
}

/// Registered maintenance-artifact lifecycle over one state directory:
/// enumerate `*.pre-*` / `*.corrupt-*` artifacts; with
/// [`MigrateMode::Apply`], delete those at least `older_than_days` old
/// (deletion goes through
/// [`meerkat_store::migrate::remove_maintenance_artifact`], which refuses
/// anything outside the registered naming). Blocking.
pub fn prune_state_dir(
    state_dir: &Path,
    older_than_days: u64,
    mode: MigrateMode,
) -> MetaPruneReport {
    let mut report = MetaPruneReport::new(mode, state_dir, older_than_days);
    let mut artifacts = enumerate_state_dir_artifacts(state_dir);
    // The first-boot manifest's copies are the remedy the manifest reader
    // names while rows are pending: kept for as long as the manifest is.
    let manifest_pending = crate::identity_first::FirstBootManifest::path(state_dir).exists();
    for artifact in &mut artifacts {
        if artifact.age_days < older_than_days
            || (manifest_pending && is_first_boot_manifest_copy(&artifact.path))
        {
            artifact.action = PruneAction::Kept;
            continue;
        }
        if mode != MigrateMode::Apply {
            artifact.action = PruneAction::WouldDelete;
            continue;
        }
        match remove_maintenance_artifact(&artifact.path) {
            Ok(()) => artifact.action = PruneAction::Deleted,
            Err(error) => {
                artifact.action = PruneAction::DeleteFailed;
                report.errors.push(format!(
                    "failed to delete {}: {error}",
                    artifact.path.display()
                ));
            }
        }
    }
    report.artifacts = artifacts;
    report
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_store::migrate::backup_artifact_name;
    use rusqlite::Connection;

    /// The exact 0.7.x-era continuity DDL (no `meerkat_schema` ledger).
    const LEGACY_CONTINUITY_DDL: &str = "CREATE TABLE continuity_records (
            identity       TEXT PRIMARY KEY,
            agent_runtime_id TEXT NOT NULL,
            session_id     TEXT NOT NULL,
            generation     INTEGER NOT NULL,
            checkpoint_version INTEGER NOT NULL,
            fencing_token  INTEGER NOT NULL
        );
        CREATE TABLE session_snapshots (
            session_id     TEXT PRIMARY KEY,
            identity       TEXT NOT NULL,
            generation     INTEGER NOT NULL,
            checkpoint_version INTEGER NOT NULL,
            fencing_token  INTEGER NOT NULL,
            data           BLOB NOT NULL
        );";

    // Legacy-NAMED but ledgered at the v1 floor: pre-ledger files are below
    // the mobkit 0.8.8 floor and refuse typed at open since the 0.8.11
    // reset, so supported fixtures always carry their ledger row.
    fn create_legacy_continuity(path: &Path) -> Connection {
        let conn = Connection::open(path).expect("create fixture db");
        conn.execute_batch(LEGACY_CONTINUITY_DDL)
            .expect("apply legacy ddl");
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS meerkat_schema (
                domain TEXT PRIMARY KEY,
                version INTEGER NOT NULL
            );
            INSERT INTO meerkat_schema (domain, version) VALUES ('meta-continuity', 1);",
        )
        .expect("stamp fixture ledger at the v1 floor");
        conn
    }

    /// A well-formed `(session_id, serialized Session)` pair. Fixtures that
    /// plant continuity blobs need real sessions now that `--apply` converts
    /// them before stamping the ledger.
    fn legacy_session_blob(text: &str) -> (String, Vec<u8>) {
        let mut session = meerkat_core::Session::new();
        session.push(meerkat_core::Message::User(
            meerkat_core::UserMessage::text(text.to_string()),
        ));
        let id = session.id().to_string();
        (id, serde_json::to_vec(&session).expect("encode session"))
    }

    fn insert_snapshot(conn: &Connection, session_id: &str, identity: &str, data: &[u8]) {
        conn.execute(
            "INSERT INTO session_snapshots \
             (session_id, identity, generation, checkpoint_version, fencing_token, data) \
             VALUES (?1, ?2, 3, 4, 7, ?3)",
            rusqlite::params![session_id, identity, data],
        )
        .expect("insert snapshot");
    }

    fn insert_record(conn: &Connection, identity: &str, session_id: &str) {
        conn.execute(
            "INSERT INTO continuity_records \
             (identity, agent_runtime_id, session_id, generation, checkpoint_version, \
              fencing_token) VALUES (?1, 'rt-1', ?2, 3, 4, 7)",
            rusqlite::params![identity, session_id],
        )
        .expect("insert record");
    }

    fn legacy_session_bytes() -> (String, Vec<u8>) {
        let session = meerkat_core::Session::new();
        let id = session.id().to_string();
        let bytes = serde_json::to_vec(&session).expect("serialize legacy session");
        (id, bytes)
    }

    fn file_digest_hex(path: &Path) -> String {
        format!("{:x}", Sha256::digest(fs::read(path).expect("read db")))
    }

    fn ledger_entry<'a>(
        report: &'a MetaMigrateReport,
        file_name: &str,
        domain: &str,
    ) -> &'a LedgerBaselineEntry {
        report
            .ledger
            .iter()
            .find(|entry| entry.database.ends_with(file_name) && entry.domain == domain)
            .unwrap_or_else(|| panic!("no ledger entry for {file_name} [{domain}]"))
    }

    #[test]
    fn fence_enumerates_layout_slots_and_memory_realms_sorted() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = temp.path();
        drop(create_legacy_continuity(&state.join("continuity.db")));
        drop(Connection::open(state.join("sessions.sqlite3")).expect("sessions"));
        drop(Connection::open(state.join("runtime.sqlite")).expect("runtime"));
        fs::create_dir_all(state.join("agent-memory")).expect("memory root");
        drop(Connection::open(state.join("agent-memory/alpha.sqlite3")).expect("realm"));
        let jobs = MetaStorageLayout::with_injected_roots(state.to_path_buf(), None)
            .jobs_db()
            .expect("fresh installation resolves");
        drop(meerkat::SqliteDetachedJobStore::open(&jobs).expect("jobs"));
        // Excluded: the admission sidecar (no fence guard by design) and
        // non-database files.
        drop(Connection::open(state.join(WORKGRAPH_ADMISSION_SIDECAR_FILE)).expect("sidecar"));
        fs::write(state.join("notes.txt"), b"x").expect("notes");

        let fence = MetaMaintenanceFence::acquire(state, Duration::from_secs(1)).expect("fence");
        let mut expected = vec![
            state.join("agent-memory/alpha.sqlite3"),
            state.join("continuity.db"),
            jobs,
            state.join("runtime.sqlite"),
            state.join("sessions.sqlite3"),
        ];
        expected.sort();
        assert_eq!(fence.fenced_databases(), expected.as_slice());
        assert_eq!(fence.len(), 5);
        assert!(!fence.is_empty());
        for database in fence.fenced_databases() {
            assert!(meerkat_sqlite::fence_lock_path(database).is_file());
        }
    }

    #[test]
    fn fence_foreign_holder_fails_typed_and_releases_partial_acquisition() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = temp.path();
        drop(create_legacy_continuity(&state.join("continuity.db")));
        drop(Connection::open(state.join("sessions.sqlite3")).expect("sessions"));

        // A FOREIGN process holding the second fence: a raw exclusive lock
        // without this process's holder registry entry.
        let foreign_lock = meerkat_sqlite::fence_lock_path(&state.join("sessions.sqlite3"));
        let foreign = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&foreign_lock)
            .expect("open foreign lock");
        foreign.try_lock().expect("foreign exclusive lock");

        let error = MetaMaintenanceFence::acquire(state, Duration::from_millis(100))
            .expect_err("foreign holder must refuse acquisition");
        assert!(
            matches!(error, meerkat_sqlite::SqliteStoreError::MaintenanceFenceHeld { ref path }
                if path.ends_with("sessions.sqlite3")),
            "{error:?}"
        );

        // RAII: the continuity fence was released on failure.
        let reacquired = meerkat_sqlite::ExclusiveFence::try_acquire(&state.join("continuity.db"))
            .expect("try acquire");
        assert!(reacquired.is_some(), "partial acquisition must be released");
        drop(reacquired);
        drop(foreign);

        // And a fenced apply run surfaces the same typed refusal as an error.
        let foreign = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&foreign_lock)
            .expect("reopen foreign lock");
        foreign.try_lock().expect("foreign exclusive lock");
        let report = migrate_state_dir(state, MigrateMode::Apply, None);
        assert!(report.has_errors());
        assert!(
            report.errors[0].contains("maintenance fence"),
            "{:?}",
            report.errors
        );
        drop(foreign);
    }

    #[test]
    fn failed_canonical_fence_after_rename_fails_closed() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = temp.path();
        let legacy_db = state.join("continuity.db");
        let (sid, bytes) = legacy_session_bytes();
        {
            let conn = create_legacy_continuity(&legacy_db);
            insert_record(&conn, "test:alice", &sid);
            insert_snapshot(&conn, &sid, "test:alice", &bytes);
        }

        // A FOREIGN holder on the CANONICAL path's fence lock: the pass
        // fence rides the legacy lock path, so acquisition and the rename
        // itself succeed — but the post-rename re-fence must fail, and the
        // pass must not keep mutating the now-unfenced database.
        let canonical = state.join("continuity.sqlite3");
        let foreign_lock = meerkat_sqlite::fence_lock_path(&canonical);
        let foreign = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&foreign_lock)
            .expect("open foreign lock");
        foreign.try_lock().expect("foreign exclusive lock");

        let report = migrate_state_dir(state, MigrateMode::Apply, None);

        assert!(canonical.is_file(), "rename itself must have happened");
        assert!(report.has_errors(), "{:?}", report.errors);
        assert!(
            report
                .errors
                .iter()
                .any(|error| error.contains("not fenced at its canonical path")),
            "{:?}",
            report.errors
        );

        // Fail-closed: the ledgered constructor never ran against the
        // unfenced database (the v1 fixture ledger was not advanced).
        {
            let conn = Connection::open(&canonical).expect("open canonical");
            assert_eq!(
                meerkat_sqlite::domain_version(&conn, "meta-continuity").expect("ledger"),
                Some(1),
                "constructor must not run against an unfenced database"
            );
        }
        drop(foreign);
    }

    /// The operator crossing must CONVERT before it stamps. Until 0.8.16
    /// `--apply` ran the guarded domain migration directly: it stamped v2
    /// and created the three empty head-canonical tables while converting
    /// nothing, so the operator paid the whole one-way cost — a file no
    /// `<= 0.8.5` binary can open again — and still had a corpus wholly on
    /// the legacy blob path, which no amount of rebooting could leave.
    #[test]
    fn apply_converts_legacy_sessions_before_stamping_the_continuity_ledger() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = temp.path();
        let db = state.join("continuity.sqlite3");
        let mut session = meerkat_core::Session::new();
        session.push(meerkat_core::Message::User(
            meerkat_core::UserMessage::text("hello".to_string()),
        ));
        let session_id = session.id().to_string();
        {
            let conn = create_legacy_continuity(&db);
            insert_snapshot(
                &conn,
                &session_id,
                "test:alice",
                &serde_json::to_vec(&session).expect("encode session"),
            );
        }

        let applied = migrate_state_dir(state, MigrateMode::Apply, None);
        assert!(!applied.has_errors(), "{:?}", applied.errors);

        let backfill = applied
            .head_canonical_backfill
            .as_ref()
            .expect("apply must report a backfill");
        assert_eq!(backfill.converted, vec![session_id.clone()]);
        assert!(backfill.complete());
        assert!(backfill.ledger_stamped);

        // The session actually crossed: a head row exists, and the blob is
        // retained as a frozen archive rather than deleted.
        let probe = Connection::open(&db).expect("probe");
        let heads: i64 = probe
            .query_row(
                "SELECT COUNT(*) FROM continuity_session_heads WHERE session_id = ?1",
                rusqlite::params![session_id],
                |row| row.get(0),
            )
            .expect("count heads");
        assert_eq!(heads, 1, "apply stamped without converting the session");
        let blobs: i64 = probe
            .query_row("SELECT COUNT(*) FROM session_snapshots", [], |row| {
                row.get(0)
            })
            .expect("count blobs");
        assert_eq!(blobs, 1, "the legacy blob must be retained");
        assert_eq!(
            meerkat_sqlite::domain_version(&probe, "meta-continuity").expect("ledger"),
            Some(crate::identity_first::HEAD_CANONICAL_CONTINUITY_SCHEMA_VERSION)
        );
    }

    /// The one-way door stays shut when the crossing is incomplete: a corpus
    /// containing a session that cannot convert must be left at v1, so
    /// rollback to a pre-head-canonical release survives a failed migration.
    #[test]
    fn apply_leaves_the_continuity_ledger_at_v1_when_a_session_cannot_convert() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = temp.path();
        let db = state.join("continuity.sqlite3");
        // A well-formed session id whose blob is not a decodable Session:
        // a real conversion failure, not structural garbage.
        let doomed = meerkat_core::types::SessionId::new().to_string();
        {
            let conn = create_legacy_continuity(&db);
            insert_snapshot(&conn, &doomed, "test:alice", b"not-a-session");
        }

        let applied = migrate_state_dir(state, MigrateMode::Apply, None);
        assert!(
            applied.has_errors(),
            "an unconvertible session must surface as an error"
        );
        let backfill = applied
            .head_canonical_backfill
            .as_ref()
            .expect("apply must report a backfill");
        assert!(!backfill.complete());
        assert!(
            !backfill.ledger_stamped,
            "a partial crossing must NOT stamp the ledger"
        );
        let probe = Connection::open(&db).expect("probe");
        assert_eq!(
            meerkat_sqlite::domain_version(&probe, "meta-continuity").expect("ledger"),
            Some(1),
            "a failed crossing must leave the file rollback-safe at v1"
        );
    }

    /// M4b: the one-way head-canonical continuity bump is an OPERATOR
    /// action. Opening the store (what a gateway launch does) must leave the
    /// file at the rollback-safe baseline; dry-run must announce the pending
    /// bump without touching the file; `--apply` commits it under the fence.
    #[test]
    fn head_canonical_continuity_bump_is_operator_gated() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = temp.path();
        let db = state.join("continuity.sqlite3");
        drop(create_legacy_continuity(&db));

        // A gateway launch: opening the store converges the baseline only.
        LocalContinuityStore::open(&db).expect("gateway-style open");
        let probe = Connection::open(&db).expect("probe");
        assert_eq!(
            meerkat_sqlite::domain_version(&probe, "meta-continuity").expect("ledger"),
            Some(1),
            "launching a new gateway must not lock the previous release out of the file"
        );
        drop(probe);

        let before = file_digest_hex(&db);
        let dry = migrate_state_dir(state, MigrateMode::DryRun, None);
        assert!(!dry.has_errors(), "{:?}", dry.errors);
        assert!(
            dry.notes
                .iter()
                .any(|note| note.contains("head-canonical") && note.contains("ONE-WAY")),
            "dry-run must announce the pending one-way bump: {:?}",
            dry.notes
        );
        assert_eq!(
            file_digest_hex(&db),
            before,
            "dry-run must not mutate the database"
        );

        let applied = migrate_state_dir(state, MigrateMode::Apply, None);
        assert!(!applied.has_errors(), "{:?}", applied.errors);
        let entry = ledger_entry(&applied, "continuity.sqlite3", "meta-continuity");
        assert_eq!(entry.action, LedgerBaselineAction::Stamped);
        assert_eq!(entry.before, Some(1));
        assert_eq!(
            entry.after,
            Some(crate::identity_first::HEAD_CANONICAL_CONTINUITY_SCHEMA_VERSION)
        );

        // Idempotent: a second apply is a no-op.
        let second = migrate_state_dir(state, MigrateMode::Apply, None);
        assert!(!second.has_errors(), "{:?}", second.errors);
        assert_eq!(
            ledger_entry(&second, "continuity.sqlite3", "meta-continuity").action,
            LedgerBaselineAction::AlreadyCurrent
        );
    }

    #[test]
    fn dry_run_is_a_read_only_version_matrix() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = temp.path();
        drop(create_legacy_continuity(&state.join("continuity.db")));
        {
            let conn = Connection::open(state.join("sessions.db")).expect("sessions");
            conn.execute_batch("CREATE TABLE sessions (session_id TEXT PRIMARY KEY)")
                .expect("sessions ddl");
        }
        drop(Connection::open(state.join(WORKGRAPH_ADMISSION_SIDECAR_FILE)).expect("sidecar"));
        let jobs = MetaStorageLayout::with_injected_roots(state.to_path_buf(), None)
            .jobs_db()
            .expect("fresh installation resolves");
        drop(meerkat::SqliteDetachedJobStore::open(&jobs).expect("jobs"));
        let before = file_digest_hex(&state.join("continuity.db"));
        let jobs_before = file_digest_hex(&jobs);

        let report = migrate_state_dir(state, MigrateMode::DryRun, None);
        assert!(!report.has_errors(), "{:?}", report.errors);
        // The fixture carries its v1 floor ledger row (pre-ledger files are
        // refused typed since the 0.8.11 reset), so dry-run records it
        // rather than promising a stamp.
        assert_eq!(
            ledger_entry(&report, "continuity.db", "meta-continuity").action,
            LedgerBaselineAction::Recorded
        );
        assert_eq!(
            ledger_entry(&report, "sessions.db", "session-store").action,
            LedgerBaselineAction::ReportOnly
        );
        assert_eq!(
            ledger_entry(&report, "jobs.sqlite3", "jobs").action,
            LedgerBaselineAction::Recorded
        );
        assert_eq!(
            ledger_entry(
                &report,
                WORKGRAPH_ADMISSION_SIDECAR_FILE,
                "meta-workgraph-admission"
            )
            .action,
            LedgerBaselineAction::Exempt
        );
        assert!(
            report
                .notes
                .iter()
                .any(|note| note.contains("ledger-exempt")),
            "{:?}",
            report.notes
        );
        assert!(
            report
                .renames
                .iter()
                .any(|entry| entry.action == RenameAction::WouldRename
                    && entry.from.ends_with("continuity.db")),
            "dry-run must report the pending rename: {:?}",
            report.renames
        );
        assert_eq!(
            file_digest_hex(&state.join("continuity.db")),
            before,
            "dry-run must leave the database byte-identical"
        );
        assert_eq!(
            file_digest_hex(&jobs),
            jobs_before,
            "dry-run must leave the inherited jobs database byte-identical"
        );
        assert!(
            state.join("continuity.db").is_file(),
            "dry-run must not rename"
        );
    }

    #[test]
    fn apply_renames_with_wal_stamps_ledgers_adopts_and_is_idempotent() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = temp.path();

        // Legacy-named continuity db in WAL mode with a leaked reader so a
        // non-empty -wal survives on disk at migrate time, holding one
        // adoptable legacy snapshot.
        let legacy_db = state.join("continuity.db");
        let (sid, legacy_bytes) = legacy_session_bytes();
        {
            let conn = Connection::open(&legacy_db).expect("create fixture");
            conn.pragma_update(None, "journal_mode", "wal")
                .expect("wal mode");
            conn.execute_batch(LEGACY_CONTINUITY_DDL).expect("ddl");
            // Ledgered at the v1 floor like `create_legacy_continuity`:
            // pre-ledger files refuse typed since the 0.8.11 reset.
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS meerkat_schema (
                    domain TEXT PRIMARY KEY,
                    version INTEGER NOT NULL
                );
                INSERT INTO meerkat_schema (domain, version) VALUES ('meta-continuity', 1);",
            )
            .expect("stamp fixture ledger at the v1 floor");
            insert_record(&conn, "test:alice", &sid);
            insert_snapshot(&conn, &sid, "test:alice", &legacy_bytes);
            // Leak the connection: the -wal file persists on disk.
            std::mem::forget(conn);
        }
        let wal = path_with_suffix(&legacy_db, "-wal");
        assert!(
            fs::metadata(&wal).map(|meta| meta.len()).unwrap_or(0) > 0,
            "fixture must leave a non-empty WAL"
        );

        // Canonical metadata built through the normal (ledgered)
        // constructor: pre-ledger fixtures are below the 0.8.8 floor.
        let metadata = state.join("meta_metadata.sqlite3");
        drop(SqliteMetadataStore::open(&metadata).expect("metadata fixture"));

        // Legacy memory root with one ledgered realm database.
        let legacy_memory = state.join("agent-memory-sqlite");
        {
            let store = SqliteAgentMemoryStore::open(&legacy_memory).expect("memory fixture");
            store.open_realm_ledgered("alpha").expect("realm fixture");
        }

        let report = migrate_state_dir(state, MigrateMode::Apply, None);
        assert!(!report.has_errors(), "{:?}", report.errors);

        // Case 2: db + WAL moved together, marker registered.
        let continuity_rename = report
            .renames
            .iter()
            .find(|entry| entry.slot == "continuity")
            .expect("continuity rename entry");
        assert_eq!(continuity_rename.action, RenameAction::Renamed);
        assert!(continuity_rename.wal_checkpointed);
        assert!(
            continuity_rename
                .siblings
                .iter()
                .any(|sibling| sibling.to.ends_with("continuity.sqlite3-wal")),
            "{:?}",
            continuity_rename.siblings
        );
        let marker = continuity_rename.marker.as_ref().expect("rename marker");
        assert!(marker.is_file());
        assert!(is_registered_backup_artifact_name(
            marker.file_name().and_then(|name| name.to_str()).unwrap()
        ));
        assert!(!legacy_db.exists());
        let canonical = state.join("continuity.sqlite3");
        assert!(canonical.is_file());
        let memory_rename = report
            .renames
            .iter()
            .find(|entry| entry.slot == "agent-memory")
            .expect("memory rename entry");
        assert_eq!(memory_rename.action, RenameAction::Renamed);
        assert!(state.join("agent-memory/alpha.sqlite3").is_file());
        assert!(!legacy_memory.exists());

        // Case 1: the continuity v1 fixture is bumped to head-canonical v2;
        // the already-current metadata/memory ledgers are left alone.
        let continuity_entry = ledger_entry(&report, "continuity.sqlite3", "meta-continuity");
        assert_eq!(continuity_entry.action, LedgerBaselineAction::Stamped);
        assert!(continuity_entry.after.is_some());
        for (file, domain) in [
            ("meta_metadata.sqlite3", "meta-metadata"),
            ("alpha.sqlite3", MEMORY_LEDGER_DOMAIN),
        ] {
            let entry = ledger_entry(&report, file, domain);
            assert_eq!(entry.action, LedgerBaselineAction::AlreadyCurrent, "{file}");
        }
        // The fixture snapshot row survives the pass byte-for-byte (the
        // retired adoption walk no longer rewrites payloads).
        {
            let conn = Connection::open(&canonical).expect("reopen canonical");
            let data: Vec<u8> = conn
                .query_row(
                    "SELECT data FROM session_snapshots WHERE session_id = ?1",
                    [&sid],
                    |row| row.get(0),
                )
                .expect("snapshot row");
            assert_eq!(data, legacy_bytes, "payload bytes untouched");
        }

        // The layout now resolves everything canonically.
        let layout = MetaStorageLayout::with_injected_roots(state.to_path_buf(), None);
        assert_eq!(
            layout.continuity_db().expect("resolve").provenance,
            DatabaseProvenance::Canonical
        );

        // Idempotence: a second apply renames nothing, stamps nothing new,
        // re-adopts nothing.
        let second = migrate_state_dir(state, MigrateMode::Apply, None);
        assert!(!second.has_errors(), "{:?}", second.errors);
        assert!(
            second
                .renames
                .iter()
                .all(|entry| entry.action != RenameAction::Renamed),
            "{:?}",
            second.renames
        );
        assert_eq!(
            ledger_entry(&second, "continuity.sqlite3", "meta-continuity").action,
            LedgerBaselineAction::AlreadyCurrent
        );
    }

    #[test]
    fn twin_dry_run_reports_row_divergence_and_refuses() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = temp.path();
        let db_a = state.join("continuity.db");
        let db_b = state.join("continuity.sqlite3");
        {
            let conn = create_legacy_continuity(&db_a);
            insert_snapshot(&conn, "s-shared", "test:alice", b"same");
            insert_snapshot(&conn, "s-divergent", "test:alice", b"version-a");
            insert_snapshot(&conn, "s-only-a", "test:alice", b"solo");
        }
        {
            let conn = create_legacy_continuity(&db_b);
            insert_snapshot(&conn, "s-shared", "test:alice", b"same");
            insert_snapshot(&conn, "s-divergent", "test:alice", b"version-b");
        }

        let report = migrate_state_dir(state, MigrateMode::DryRun, None);
        assert!(report.has_errors(), "twins must fail closed");
        assert!(
            report.errors[0].contains("continuity"),
            "{:?}",
            report.errors
        );
        // Fail-closed: the divergence report is the whole output.
        assert!(report.ledger.is_empty());
        assert!(report.renames.is_empty());

        let twin = &report.twins[0];
        assert_eq!(twin.slot, "continuity");
        assert!(!twin.byte_identical);
        assert_eq!(twin.rows_equal, 1, "{:?}", twin.rows);
        let status_of = |key: &str| {
            twin.rows
                .iter()
                .find(|row| row.key == format!("session_snapshots/{key}"))
                .map(|row| row.status.clone())
        };
        assert_eq!(status_of("s-divergent"), Some(DivergenceStatus::Divergent));
        assert_eq!(
            status_of("s-only-a"),
            Some(DivergenceStatus::OnlyIn { location: db_a })
        );
        assert!(matches!(
            twin.resolution,
            TwinResolution::Refused { ref reason } if reason.contains("--adopt")
        ));
        assert!(
            twin.notes
                .iter()
                .any(|note| note.contains("fencing tokens")),
            "{:?}",
            twin.notes
        );
        // Both copies untouched.
        assert!(db_b.is_file());
    }

    #[test]
    fn twin_apply_with_adopt_archives_read_only_and_canonicalizes() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = temp.path();
        let legacy = state.join("continuity.db");
        let canonical = state.join("continuity.sqlite3");
        // Real sessions, because apply now CONVERTS before it stamps: a row
        // that is not a decodable session would (correctly) withhold the
        // ledger bump and this test is about twin adoption, not conversion.
        let keep = legacy_session_blob("keep-me");
        let lose = legacy_session_blob("archive-me");
        {
            let conn = create_legacy_continuity(&legacy);
            insert_snapshot(&conn, &keep.0, "test:alice", &keep.1);
        }
        {
            let conn = create_legacy_continuity(&canonical);
            insert_snapshot(&conn, &lose.0, "test:alice", &lose.1);
        }
        let other_digest = file_digest_hex(&canonical);

        // Apply without --adopt still refuses divergent twins.
        let refused = migrate_state_dir(state, MigrateMode::Apply, None);
        assert!(refused.has_errors());

        let report = migrate_state_dir(state, MigrateMode::Apply, Some(&legacy));
        assert!(!report.has_errors(), "{:?}", report.errors);
        let twin = &report.twins[0];
        let TwinResolution::Adopted { adopted, archived } = &twin.resolution else {
            panic!("expected adoption, got {:?}", twin.resolution);
        };
        assert_eq!(
            adopted, &canonical,
            "adopted copy lands at the canonical name"
        );
        assert_eq!(archived.len(), 1);
        let archive = &archived[0];
        assert!(archive.is_file());
        let archive_name = archive.file_name().and_then(|name| name.to_str()).unwrap();
        assert!(
            is_registered_backup_artifact_name(archive_name),
            "{archive_name}"
        );
        assert!(archive_name.ends_with(".twin"), "{archive_name}");
        assert!(
            fs::metadata(archive)
                .expect("archive meta")
                .permissions()
                .readonly(),
            "archive must be read-only"
        );
        assert_eq!(
            file_digest_hex(archive),
            other_digest,
            "archived content preserved exactly"
        );

        // The adopted content lives at the canonical path; the resolver and
        // a fresh migrate see no twins.
        let conn = Connection::open(&canonical).expect("reopen canonical");
        let data: Vec<u8> = conn
            .query_row(
                "SELECT data FROM session_snapshots WHERE session_id = ?1",
                rusqlite::params![keep.0],
                |row| row.get(0),
            )
            .expect("adopted row");
        assert_eq!(data, keep.1, "adopted copy must carry the kept session");
        // The adopted copy is the one that survived: the archived twin's
        // session is absent from the canonical file.
        let lost: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM session_snapshots WHERE session_id = ?1",
                rusqlite::params![lose.0],
                |row| row.get(0),
            )
            .expect("archived row absent");
        assert_eq!(
            lost, 0,
            "archived twin's session must not be in the adopted copy"
        );
        drop(conn);
        assert!(!legacy.exists());
        let layout = MetaStorageLayout::with_injected_roots(state.to_path_buf(), None);
        assert!(layout.continuity_db().is_ok());
        let again = migrate_state_dir(state, MigrateMode::DryRun, None);
        assert!(again.twins.is_empty(), "{:?}", again.twins);
    }

    #[test]
    fn byte_identical_twins_dedup_under_plain_apply() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = temp.path();
        let canonical = state.join("continuity.sqlite3");
        drop(LocalContinuityStore::open(&canonical).expect("continuity fixture"));
        let legacy = state.join("continuity.db");
        fs::copy(&canonical, &legacy).expect("copy twin");

        let report = migrate_state_dir(state, MigrateMode::Apply, None);
        assert!(!report.has_errors(), "{:?}", report.errors);
        let twin = report
            .twins
            .iter()
            .find(|twin| twin.slot == "continuity")
            .expect("continuity twin");
        assert!(twin.byte_identical);
        let TwinResolution::Deduped { kept, archived } = &twin.resolution else {
            panic!("expected dedup, got {:?}", twin.resolution);
        };
        assert_eq!(kept, &canonical);
        // The archive is the database plus any `-wal` / `-shm` sidecars that
        // existed at archive time (read-only divergence opens materialize
        // empty ones on a WAL-mode database) — sidecars always travel with
        // their database.
        assert!(
            archived[0]
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap()
                .ends_with(".twin-dedup")
        );
        assert!(
            archived.iter().skip(1).all(|path| {
                let name = path.file_name().and_then(|name| name.to_str()).unwrap();
                name.ends_with("-wal") || name.ends_with("-shm")
            }),
            "{archived:?}"
        );
        assert!(canonical.is_file());
        assert!(!legacy.exists());
    }

    #[test]
    fn adopt_path_outside_the_twin_refuses() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = temp.path();
        {
            let conn = create_legacy_continuity(&state.join("continuity.db"));
            insert_snapshot(&conn, "s-a", "test:alice", b"a");
        }
        {
            let conn = create_legacy_continuity(&state.join("continuity.sqlite3"));
            insert_snapshot(&conn, "s-b", "test:alice", b"b");
        }
        let elsewhere = state.join("unrelated.db");
        fs::write(&elsewhere, b"x").expect("unrelated");

        let report = migrate_state_dir(state, MigrateMode::Apply, Some(&elsewhere));
        assert!(report.has_errors());
        assert!(matches!(
            report.twins[0].resolution,
            TwinResolution::Refused { ref reason } if reason.contains("candidates")
        ));
        assert!(state.join("continuity.db").is_file(), "nothing moved");
        assert!(state.join("continuity.sqlite3").is_file());
    }

    #[test]
    fn leftovers_census_reports_blobs_sidecar_artifacts_and_dead_registry_entries() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = temp.path();
        // Legacy sharded-FS blob layout beside the current object layout.
        let shard = state.join("blobs").join("aa");
        fs::create_dir_all(&shard).expect("shard");
        fs::write(shard.join(format!("{}.json", "a".repeat(64))), b"{}").expect("legacy blob");
        drop(Connection::open(state.join(WORKGRAPH_ADMISSION_SIDECAR_FILE)).expect("sidecar"));
        fs::write(state.join("continuity.db.pre-0.0.1-1700000000"), b"backup")
            .expect("backup artifact");
        // The runtime lock stays between runs; it is inventory, not a leftover.
        fs::write(
            state.join(crate::storage_layout::STATE_DIR_LOCK_FILE_NAME),
            b"",
        )
        .expect("runtime lock file");
        // A dead pid: a reaped child. A live pid: this process.
        let mut child = std::process::Command::new("true").spawn().expect("spawn");
        let dead_pid = child.id();
        child.wait().expect("reap");
        fs::write(
            state.join(RUNTIME_REGISTRY_FILE_NAME),
            serde_json::to_vec(&serde_json::json!({
                "entries": [
                    { "key": "k1", "runtime_id": "tux-dead", "http_base_url": "http://127.0.0.1:1",
                      "pid": dead_pid, "updated_at_ms": 0 },
                    { "key": "k2", "runtime_id": "tux-live", "http_base_url": "http://127.0.0.1:2",
                      "pid": std::process::id(), "updated_at_ms": 0 },
                ]
            }))
            .expect("registry json"),
        )
        .expect("write registry");

        let report = migrate_state_dir(state, MigrateMode::DryRun, None);
        assert!(!report.has_errors(), "{:?}", report.errors);
        let codes: Vec<&str> = report
            .findings
            .iter()
            .map(|finding| finding.code.as_str())
            .collect();
        for expected in [
            storage_doctor::FINDING_LEGACY_FS_BLOBS,
            storage_doctor::FINDING_WORKGRAPH_ADMISSION_SIDECAR,
            storage_doctor::FINDING_BACKUP_ARTIFACT,
            FINDING_DEAD_RUNTIME_REGISTRY_ENTRY,
        ] {
            assert!(codes.contains(&expected), "missing {expected}: {codes:?}");
        }
        assert!(
            !codes.contains(&storage_doctor::FINDING_STATE_DIR_LOCK),
            "the runtime lock is no leftover: {codes:?}"
        );
        let dead: Vec<_> = report
            .findings
            .iter()
            .filter(|finding| finding.code == FINDING_DEAD_RUNTIME_REGISTRY_ENTRY)
            .collect();
        assert_eq!(dead.len(), 1, "only the dead entry is flagged: {dead:?}");
        assert!(dead[0].message.contains("tux-dead"));
    }

    #[test]
    fn rename_marker_names_carry_the_meta_version() {
        let name = meta_backup_artifact_name("continuity.db", "renamed");
        let prefix = format!("continuity.db.pre-{}-", env!("CARGO_PKG_VERSION"));
        assert!(name.starts_with(&prefix), "{name}");
        assert!(name.ends_with(".renamed"), "{name}");
    }

    #[test]
    fn strict_artifact_name_validation_rejects_lookalikes() {
        // Everything this run generates validates, including sibling
        // archives and purposeless names.
        for valid in [
            backup_artifact_name("sessions.sqlite3", "").as_str(),
            backup_artifact_name("continuity.db", "renamed").as_str(),
            backup_artifact_name("team", "split-brain").as_str(),
            meta_backup_artifact_name("continuity.db", "renamed").as_str(),
            "sessions.sqlite3.pre-0.8.3-1700000000",
            "continuity.db.pre-0.8.2-1700000000.renamed",
            "meta_metadata.sqlite.pre-0.8.3-123.twin-dedup-wal",
            "agent-memory-sqlite.pre-0.0.1-1700000000.twin",
        ] {
            assert!(is_registered_backup_artifact_name(valid), "{valid}");
        }
        // Loose `.pre-` substrings are NOT registered artifacts — prune's
        // deletion authority must never claim user files.
        for invalid in [
            "notes.pre-release",
            "data.pre-view.txt",
            ".pre-0.8.3-1700000000",
            "foo.pre-1-1700000000",
            "foo.pre-0..3-1700000000",
            "foo.pre-0.8.3-17000x",
            "foo.pre-0.8.3-",
            "foo.pre-0.8.3-1700000000.",
            "sessions.sqlite3",
        ] {
            assert!(!is_registered_backup_artifact_name(invalid), "{invalid}");
        }
        for valid in [
            "alpha.sqlite3.corrupt-42",
            "session_index.sqlite3.corrupt-1700000000",
        ] {
            assert!(is_registered_quarantine_artifact_name(valid), "{valid}");
        }
        for invalid in [
            "report.corrupt-12a",
            "x.corrupt-",
            ".corrupt-42",
            "a.corrupt-1.bak",
            "notes.corrupted-1",
        ] {
            assert!(
                !is_registered_quarantine_artifact_name(invalid),
                "{invalid}"
            );
        }
    }

    /// The first-boot manifest's byte copy is the remedy the manifest reader
    /// names while rows are pending: prune keeps it for as long as the
    /// manifest exists, however old, and deletes it like any other backup
    /// once the manifest is gone.
    #[test]
    fn prune_keeps_the_first_boot_manifest_copy_while_the_manifest_is_pending() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = temp.path();
        let manifest = state.join(crate::identity_first::FIRST_BOOT_MANIFEST_FILE);
        let copy = state.join(format!(
            "{}.pre-0.9.4-1700000000.native-migration",
            crate::identity_first::FIRST_BOOT_MANIFEST_FILE
        ));
        let other = state.join("runtime.sqlite.pre-0.9.4-1700000000.native-migration");
        fs::write(&manifest, b"{}").expect("manifest");
        fs::write(&copy, b"{}").expect("copy");
        fs::write(&other, b"backup").expect("other backup");

        let report = prune_state_dir(state, 0, MigrateMode::Apply);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert!(copy.exists(), "kept while the manifest is pending");
        assert!(!other.exists(), "every other backup still goes");

        fs::remove_file(&manifest).expect("the last row carried out");
        let report = prune_state_dir(state, 0, MigrateMode::Apply);
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert!(!copy.exists(), "deleted once the manifest is gone");
    }

    #[test]
    fn prune_respects_threshold_and_registered_patterns_only() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = temp.path();
        let old_backup = state.join("continuity.db.pre-0.0.1-1700000000.renamed");
        fs::write(&old_backup, b"old").expect("old backup");
        let old_time = std::time::SystemTime::now() - Duration::from_hours(40 * 24);
        fs::File::options()
            .write(true)
            .open(&old_backup)
            .expect("open for mtime")
            .set_modified(old_time)
            .expect("set mtime");
        // An old archived memory-root DIRECTORY.
        let old_dir = state.join("agent-memory-sqlite.pre-0.0.1-1700000000.twin");
        fs::create_dir_all(&old_dir).expect("old dir");
        fs::write(old_dir.join("alpha.sqlite3"), b"realm").expect("realm bytes");
        fs::File::options()
            .write(true)
            .open(old_dir.join("alpha.sqlite3"))
            .expect("open for mtime")
            .set_modified(old_time)
            .expect("set inner mtime");
        // Directory mtimes: setting via the contained file is not enough on
        // all platforms; force the dir mtime through a fresh handle.
        fs::File::open(&old_dir)
            .expect("open dir")
            .set_modified(old_time)
            .expect("set dir mtime");
        let young_quarantine = state.join("alpha.sqlite3.corrupt-42");
        fs::write(&young_quarantine, b"young").expect("young quarantine");
        let distractor = state.join("notes.txt");
        fs::write(&distractor, b"keep me").expect("distractor");
        // Lookalikes with a `.pre-` / `.corrupt-` substring but not the
        // registered full shape: old enough to delete, never enumerated.
        let lookalike_backup = state.join("notes.pre-release");
        let lookalike_quarantine = state.join("report.corrupt-12a");
        for lookalike in [&lookalike_backup, &lookalike_quarantine] {
            fs::write(lookalike, b"user file").expect("lookalike");
            fs::File::options()
                .write(true)
                .open(lookalike)
                .expect("open for mtime")
                .set_modified(old_time)
                .expect("set lookalike mtime");
        }

        let dry = prune_state_dir(state, 30, MigrateMode::DryRun);
        assert!(!dry.has_errors());
        assert_eq!(dry.artifacts.len(), 3, "{:?}", dry.artifacts);
        let action_of = |report: &MetaPruneReport, path: &Path| {
            report
                .artifacts
                .iter()
                .find(|artifact| artifact.path == path)
                .map(|artifact| artifact.action)
                .unwrap_or_else(|| panic!("no artifact entry for {}", path.display()))
        };
        assert_eq!(action_of(&dry, &old_backup), PruneAction::WouldDelete);
        assert_eq!(action_of(&dry, &old_dir), PruneAction::WouldDelete);
        assert_eq!(action_of(&dry, &young_quarantine), PruneAction::Kept);
        assert!(old_backup.is_file(), "dry-run deletes nothing");

        let applied = prune_state_dir(state, 30, MigrateMode::Apply);
        assert!(!applied.has_errors(), "{:?}", applied.errors);
        assert_eq!(action_of(&applied, &old_backup), PruneAction::Deleted);
        assert_eq!(action_of(&applied, &old_dir), PruneAction::Deleted);
        assert!(!old_backup.exists());
        assert!(!old_dir.exists());
        assert!(young_quarantine.is_file(), "young artifacts are kept");
        assert!(distractor.is_file(), "unregistered names are never touched");

        // Threshold 0 = everything registered; lookalikes still survive.
        let sweep = prune_state_dir(state, 0, MigrateMode::Apply);
        assert!(!sweep.has_errors());
        assert!(!young_quarantine.exists());
        assert!(
            lookalike_backup.is_file() && lookalike_quarantine.is_file(),
            "names outside the registered full shape are never enumerated or deleted"
        );
    }

    #[test]
    fn report_shapes_round_trip_through_json() {
        let mut report = MetaMigrateReport::new(MigrateMode::Apply, Path::new("/state"));
        report.ledger.push(LedgerBaselineEntry {
            database: PathBuf::from("/state/continuity.sqlite3"),
            domain: "meta-continuity".to_string(),
            before: None,
            after: Some(1),
            action: LedgerBaselineAction::Stamped,
        });
        report.renames.push(FileRenameEntry {
            slot: "continuity".to_string(),
            from: PathBuf::from("/state/continuity.db"),
            to: PathBuf::from("/state/continuity.sqlite3"),
            siblings: vec![SiblingRename {
                from: PathBuf::from("/state/continuity.db-wal"),
                to: PathBuf::from("/state/continuity.sqlite3-wal"),
            }],
            wal_checkpointed: true,
            marker: Some(PathBuf::from("/state/continuity.db.pre-0.8.2-1.renamed")),
            action: RenameAction::Renamed,
        });
        report.twins.push(TwinReport {
            slot: "console".to_string(),
            paths: vec![PathBuf::from("/state/meta_console.sqlite")],
            byte_identical: false,
            rows_equal: 2,
            rows: vec![RowDivergenceEntry {
                key: "console_frames/7".to_string(),
                status: DivergenceStatus::Divergent,
            }],
            resolution: TwinResolution::Adopted {
                adopted: PathBuf::from("/state/meta_console.sqlite3"),
                archived: vec![PathBuf::from("/state/meta_console.sqlite.pre-0.8.2-1.twin")],
            },
            notes: vec![],
            errors: vec![],
        });
        let json = serde_json::to_string(&report).expect("serialize");
        let parsed: MetaMigrateReport = serde_json::from_str(&json).expect("deserialize");
        assert!(matches!(parsed.mode, MigrateMode::Apply));
        assert_eq!(parsed.ledger[0].action, LedgerBaselineAction::Stamped);
        assert_eq!(parsed.renames[0].action, RenameAction::Renamed);
        assert!(matches!(
            parsed.twins[0].resolution,
            TwinResolution::Adopted { .. }
        ));
        assert!(!parsed.has_errors());

        // Forward compatibility: unknown fields tolerated, defaults fill in.
        let sparse: MetaMigrateReport =
            serde_json::from_str(r#"{"future_field":1}"#).expect("sparse migrate");
        assert!(matches!(sparse.mode, MigrateMode::DryRun));
        let sparse: MetaPruneReport =
            serde_json::from_str(r#"{"future_field":1}"#).expect("sparse prune");
        assert!(sparse.artifacts.is_empty());
    }

    #[test]
    fn missing_state_dir_errors_typed() {
        let temp = tempfile::tempdir().expect("tempdir");
        let missing = temp.path().join("nope");
        let report = migrate_state_dir(&missing, MigrateMode::DryRun, None);
        assert!(report.has_errors());
        assert!(report.errors[0].contains("does not exist"));
    }

    /// Malformed registry evidence is an error from the parser itself,
    /// whatever the liveness of the pids it names.
    #[test]
    fn malformed_registry_rows_are_errors_independent_of_liveness() {
        let dir = tempfile::tempdir().expect("tempdir");
        let registry = dir.path().join(RUNTIME_REGISTRY_FILE_NAME);
        for (body, well_formed) in [
            (serde_json::json!({ "entries": {} }), false),
            (serde_json::json!({}), false),
            (
                serde_json::json!({ "entries": [{ "runtime_id": "r" }] }),
                false,
            ),
            (
                serde_json::json!({ "entries": [{ "pid": 4_000_000_000_u64, "runtime_id": 7 }] }),
                false,
            ),
            (
                serde_json::json!({ "entries": [{ "pid": 4_000_000_000_u64, "runtime_id": "r" }] }),
                true,
            ),
        ] {
            fs::write(&registry, serde_json::to_vec(&body).unwrap()).unwrap();
            assert_eq!(
                registered_runtime_liveness(&registry).is_ok(),
                well_formed,
                "{body}"
            );
        }
    }

    /// `/proc` evidence: an entry is alive, a missing entry is proven
    /// absence, and a lookup that errors (here through a non-directory) is
    /// unknown, never absence.
    #[test]
    fn proc_lookup_errors_are_unknown_not_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let proc_root = dir.path().join("proc");
        fs::create_dir_all(proc_root.join("42")).unwrap();
        assert_eq!(proc_entry_liveness(&proc_root, 42), Some(true));
        assert_eq!(proc_entry_liveness(&proc_root, 43), Some(false));
        let not_a_dir = dir.path().join("file");
        fs::write(&not_a_dir, b"x").unwrap();
        assert_eq!(proc_entry_liveness(&not_a_dir, 42), None);
    }

    // ── `--session-db` ───────────────────────────────────────────────────

    /// A 0.9.3 session database as a `store_path` override left it: the
    /// session-store ledger row, and one session row carrying `marker`.
    fn create_session_db(path: &Path, marker: &str) {
        let conn = Connection::open(path).expect("create session db");
        conn.execute_batch(
            "CREATE TABLE meerkat_schema (domain TEXT PRIMARY KEY, version INTEGER NOT NULL);
             CREATE TABLE sessions (id TEXT PRIMARY KEY, data BLOB NOT NULL);",
        )
        .expect("session db schema");
        conn.execute(
            "INSERT INTO meerkat_schema (domain, version) VALUES (?1, 1)",
            [SESSION_STORE_DOMAIN.name],
        )
        .expect("session-store ledger row");
        conn.execute(
            "INSERT INTO sessions (id, data) VALUES ('s-1', ?1)",
            [marker.as_bytes()],
        )
        .expect("session row");
    }

    fn session_marker(path: &Path) -> String {
        let conn = Connection::open(path).expect("open session db");
        let data: Vec<u8> = conn
            .query_row("SELECT data FROM sessions WHERE id = 's-1'", [], |row| {
                row.get(0)
            })
            .expect("session row");
        String::from_utf8(data).expect("marker text")
    }

    /// Names and bytes of every file directly in `dir`.
    fn dir_snapshot(dir: &Path) -> BTreeMap<String, Vec<u8>> {
        fs::read_dir(dir)
            .expect("read dir")
            .map(|entry| entry.expect("entry").path())
            .filter(|path| path.is_file())
            .map(|path| {
                (
                    path.file_name().unwrap().to_string_lossy().into_owned(),
                    fs::read(&path).expect("read file"),
                )
            })
            .collect()
    }

    fn session_db_outcome(report: &MetaMigrateReport) -> &SessionDbOutcome {
        &report
            .session_db
            .as_ref()
            .expect("session-db report")
            .outcome
    }

    fn session_db_run(
        state: &Path,
        mode: MigrateMode,
        adopt: Option<&Path>,
        named: &Path,
    ) -> MetaMigrateReport {
        migrate_state_dir_with_session_db(
            state,
            mode,
            adopt,
            &std::collections::BTreeSet::new(),
            Some(named),
        )
    }

    /// Case 2 plus the tripwire: a lone named file renames to `sessions.sqlite3`
    /// (the dry run reports it and changes nothing), its old path then holds
    /// the session-store retired stamp, a rename marker records the move, and
    /// a rerun naming the same file is the typed "already moved" no-op.
    #[test]
    fn a_lone_session_db_moves_into_the_layout_and_leaves_the_tripwire() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = temp.path();
        let named = state.join("my-sessions.db");
        create_session_db(&named, "override");
        let before = dir_snapshot(state);

        let dry = session_db_run(state, MigrateMode::DryRun, None, &named);
        assert!(!dry.has_errors(), "{:?}", dry.errors);
        assert_eq!(session_db_outcome(&dry), &SessionDbOutcome::WouldMove);
        assert!(
            dry.renames
                .iter()
                .any(|entry| entry.from == named && entry.action == RenameAction::WouldRename),
            "{:?}",
            dry.renames
        );
        assert!(dry.fenced_databases.contains(&named));
        assert_eq!(dir_snapshot(state), before, "a dry run changes nothing");

        let applied = session_db_run(state, MigrateMode::Apply, None, &named);
        assert!(!applied.has_errors(), "{:?}", applied.errors);
        assert_eq!(
            session_db_outcome(&applied),
            &SessionDbOutcome::Moved {
                tripwire: named.clone()
            }
        );
        let canonical = state.join("sessions.sqlite3");
        assert_eq!(session_marker(&canonical), "override");
        assert!(is_session_store_tripwire(&named), "the tripwire stands");
        assert!(
            fs::read_dir(state).unwrap().any(|entry| {
                let name = entry.unwrap().file_name().to_string_lossy().into_owned();
                name.starts_with("my-sessions.db.pre-") && name.ends_with(".renamed")
            }),
            "a rename marker records the move"
        );

        let again = session_db_run(state, MigrateMode::Apply, None, &named);
        assert!(!again.has_errors(), "{:?}", again.errors);
        assert_eq!(session_db_outcome(&again), &SessionDbOutcome::AlreadyMoved);
        assert_eq!(session_marker(&canonical), "override");
    }

    /// Naming the canonical file itself is the typed "already in the
    /// layout" no-op, and leaves no tripwire.
    #[test]
    fn a_session_db_that_is_the_canonical_file_is_already_in_the_layout() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = temp.path();
        let canonical = state.join("sessions.sqlite3");
        create_session_db(&canonical, "canonical");
        let report = session_db_run(state, MigrateMode::Apply, None, &canonical);
        assert!(!report.has_errors(), "{:?}", report.errors);
        assert_eq!(
            session_db_outcome(&report),
            &SessionDbOutcome::AlreadyInLayout
        );
        assert_eq!(session_marker(&canonical), "canonical");
        assert!(!is_session_store_tripwire(&canonical));
    }

    /// The refusals before the fence: a missing path, a directory, a file
    /// outside the state directory, a file that is not SQLite, and SQLite
    /// without the session-store row. Each refuses typed in an apply run and
    /// leaves the state directory byte-identical; the non-SQLite file is
    /// never opened, so nothing appears beside it.
    #[test]
    fn session_db_refusals_move_nothing() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = temp.path().join("state");
        fs::create_dir(&state).unwrap();
        let elsewhere = temp.path().join("elsewhere");
        fs::create_dir(&elsewhere).unwrap();
        let outside = elsewhere.join("my-sessions.db");
        create_session_db(&outside, "outside");
        let directory = state.join("store.d");
        fs::create_dir(&directory).unwrap();
        let text = state.join("notes.db");
        fs::write(&text, b"not a database at all, just text").unwrap();
        let foreign = state.join("other.db");
        Connection::open(&foreign)
            .unwrap()
            .execute_batch("CREATE TABLE things (id INTEGER PRIMARY KEY);")
            .unwrap();
        let before = dir_snapshot(&state);

        type RefusalCase = (PathBuf, fn(&SessionDbRefusal) -> bool);
        let cases: [RefusalCase; 5] = [
            (state.join("absent.db"), |refusal| {
                matches!(refusal, SessionDbRefusal::Missing { .. })
            }),
            (directory, |refusal| {
                matches!(refusal, SessionDbRefusal::NotAFile { .. })
            }),
            (outside.clone(), |refusal| {
                matches!(refusal, SessionDbRefusal::OutsideStateDir { .. })
            }),
            (text.clone(), |refusal| {
                matches!(refusal, SessionDbRefusal::NotASessionDatabase { .. })
            }),
            (foreign, |refusal| {
                matches!(refusal, SessionDbRefusal::NotASessionDatabase { .. })
            }),
        ];
        for (named, expected) in cases {
            let report = session_db_run(&state, MigrateMode::Apply, None, &named);
            assert!(report.has_errors(), "{} must refuse", named.display());
            let SessionDbOutcome::Refused { refusal } = session_db_outcome(&report) else {
                panic!("{}: {:?}", named.display(), report.session_db);
            };
            assert!(expected(refusal), "{}: {refusal:?}", named.display());
            assert!(
                report.fenced_databases.is_empty(),
                "refused before the fence"
            );
            assert_eq!(dir_snapshot(&state), before, "{}", named.display());
        }
        assert!(!path_with_suffix(&text, "-wal").exists());
        assert!(!path_with_suffix(&text, "-shm").exists());
        assert_eq!(session_marker(&outside), "outside");
    }

    /// The parent check compares canonical paths, so a state directory
    /// named through a symlink takes the file inside it.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_state_dir_takes_its_own_session_db() {
        let temp = tempfile::tempdir().expect("tempdir");
        let real = temp.path().join("real");
        fs::create_dir(&real).unwrap();
        let link = temp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        create_session_db(&real.join("my-sessions.db"), "override");
        let report = session_db_run(
            &link,
            MigrateMode::DryRun,
            None,
            &link.join("my-sessions.db"),
        );
        assert!(!report.has_errors(), "{:?}", report.errors);
        assert_eq!(session_db_outcome(&report), &SessionDbOutcome::WouldMove);
    }

    /// Once a 0.9.4 runtime started on the directory (runtime.sqlite
    /// carries Core's head-canonical profile pin), the run refuses typed and
    /// the named file is untouched. Control: the same directory without the
    /// pin moves it.
    #[test]
    fn a_native_runtime_already_started_refuses_the_session_db() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = temp.path();
        let named = state.join("my-sessions.db");
        create_session_db(&named, "override");
        let runtime = state.join(crate::storage_layout::RUNTIME_DB_FILE_NAME);
        Connection::open(&runtime)
            .unwrap()
            .execute_batch(
                "CREATE TABLE runtime_head_canonical_profile_pin (singleton INTEGER PRIMARY KEY);
                 INSERT INTO runtime_head_canonical_profile_pin (singleton) VALUES (1);",
            )
            .unwrap();
        let before = dir_snapshot(state);
        let report = session_db_run(state, MigrateMode::Apply, None, &named);
        assert!(report.has_errors());
        assert!(matches!(
            session_db_outcome(&report),
            SessionDbOutcome::Refused {
                refusal: SessionDbRefusal::NativeRuntimeAlreadyStarted { .. }
            }
        ));
        assert_eq!(dir_snapshot(state), before);

        fs::remove_file(&runtime).unwrap();
        let control = session_db_run(state, MigrateMode::Apply, None, &named);
        assert!(!control.has_errors(), "{:?}", control.errors);
        assert!(matches!(
            session_db_outcome(&control),
            SessionDbOutcome::Moved { .. }
        ));
    }

    /// The live-runtime rule: while another process holds a runtime
    /// lock of the directory (here a shared lock on the execution-custody
    /// file, as Core takes per hosted runtime), the run refuses typed and
    /// moves nothing. Control: once the lock is released the file moves.
    #[test]
    fn a_live_runtime_lock_refuses_the_session_db() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = temp.path();
        let named = state.join("my-sessions.db");
        create_session_db(&named, "override");
        let mut custody = state
            .join(crate::storage_layout::RUNTIME_DB_FILE_NAME)
            .into_os_string();
        custody.push(".execution-custody");
        let holder = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(PathBuf::from(custody))
            .unwrap();
        holder.lock_shared().unwrap();
        let before = dir_snapshot(state);
        let report = session_db_run(state, MigrateMode::Apply, None, &named);
        assert!(report.has_errors());
        assert!(matches!(
            session_db_outcome(&report),
            SessionDbOutcome::Refused {
                refusal: SessionDbRefusal::RuntimeInUse { .. }
            }
        ));
        assert_eq!(dir_snapshot(state), before);

        drop(holder);
        let control = session_db_run(state, MigrateMode::Apply, None, &named);
        assert!(!control.has_errors(), "{:?}", control.errors);
        assert!(matches!(
            session_db_outcome(&control),
            SessionDbOutcome::Moved { .. }
        ));
    }

    /// A named file next to a divergent `sessions.sqlite3` is a twin.
    /// The refusal names the named file and the exact `--adopt` command;
    /// apply without `--adopt` refuses too; adopting the named file puts its
    /// rows at the canonical name, archives the stale copy read-only, and
    /// leaves the tripwire at the old path.
    #[test]
    fn a_session_db_beside_a_divergent_canonical_names_its_adopt_command() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = temp.path();
        let named = state.join("my-sessions.db");
        let canonical = state.join("sessions.sqlite3");
        create_session_db(&named, "override");
        create_session_db(&canonical, "stale");

        let dry = session_db_run(state, MigrateMode::DryRun, None, &named);
        assert!(dry.has_errors());
        let twin = dry
            .twins
            .iter()
            .find(|twin| twin.slot == "sessions")
            .expect("sessions twin");
        let TwinResolution::Refused { reason } = &twin.resolution else {
            panic!("{:?}", twin.resolution);
        };
        let command = format!(
            "`meta_gateway storage-migrate --state-dir {} --session-db {} --apply --adopt {}`",
            state.display(),
            named.display(),
            named.display()
        );
        assert!(reason.contains(&command), "{reason}");
        assert!(reason.contains("the copy 0.9.3 read and wrote"), "{reason}");
        assert_eq!(session_db_outcome(&dry), &SessionDbOutcome::WouldMove);
        // A bare file name (`--state-dir . --session-db my-sessions.db`, run
        // from the state directory) has an empty parent: the command still
        // names the state directory the run was given.
        assert_eq!(
            session_db_adopt_command(Path::new("."), Path::new("my-sessions.db")),
            "meta_gateway storage-migrate --state-dir . --session-db my-sessions.db --apply \
             --adopt my-sessions.db"
        );

        let refused = session_db_run(state, MigrateMode::Apply, None, &named);
        assert!(refused.has_errors());
        assert_eq!(session_marker(&named), "override");
        assert_eq!(session_marker(&canonical), "stale");

        let adopted = session_db_run(state, MigrateMode::Apply, Some(&named), &named);
        assert!(!adopted.has_errors(), "{:?}", adopted.errors);
        assert_eq!(session_marker(&canonical), "override");
        assert!(is_session_store_tripwire(&named));
        let twin = adopted
            .twins
            .iter()
            .find(|twin| twin.slot == "sessions")
            .expect("sessions twin");
        let TwinResolution::Adopted { archived, .. } = &twin.resolution else {
            panic!("{:?}", twin.resolution);
        };
        assert_eq!(session_marker(&archived[0]), "stale");
        assert!(fs::metadata(&archived[0]).unwrap().permissions().readonly());
        assert!(matches!(
            session_db_outcome(&adopted),
            SessionDbOutcome::Moved { .. }
        ));
    }

    /// The adopt command names the state directory as the run was given it,
    /// never the named file's parent: through a symlinked state directory
    /// the two differ.
    #[cfg(unix)]
    #[test]
    fn a_divergent_session_db_twin_names_the_state_dir_the_run_was_given() {
        let temp = tempfile::tempdir().expect("tempdir");
        let real = temp.path().join("real");
        fs::create_dir(&real).unwrap();
        let link = temp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let named = real.join("my-sessions.db");
        create_session_db(&named, "override");
        create_session_db(&real.join("sessions.sqlite3"), "stale");

        let dry = session_db_run(&link, MigrateMode::DryRun, None, &named);
        let twin = dry
            .twins
            .iter()
            .find(|twin| twin.slot == "sessions")
            .expect("sessions twin");
        let TwinResolution::Refused { reason } = &twin.resolution else {
            panic!("{:?}", twin.resolution);
        };
        assert!(
            reason.contains(&format!(
                "--state-dir {} --session-db {}",
                link.display(),
                named.display()
            )),
            "{reason}"
        );
    }

    /// A run that moved the named file and stopped before its stamp (a crash
    /// between the two) leaves the stamp owed: a rerun naming the missing
    /// file reports it, and an apply run writes it. Control: a missing file
    /// with no artifact beside it is still refused as missing.
    #[test]
    fn a_rerun_writes_the_tripwire_an_interrupted_run_owes() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = temp.path();
        let named = state.join("my-sessions.db");
        let canonical = state.join("sessions.sqlite3");
        create_session_db(&named, "override");
        // The interrupted run: the rename and its marker, then no stamp.
        fs::rename(&named, &canonical).unwrap();
        write_rename_marker(&named, &canonical).unwrap();

        let dry = session_db_run(state, MigrateMode::DryRun, None, &named);
        assert!(!dry.has_errors(), "{:?}", dry.errors);
        assert_eq!(session_db_outcome(&dry), &SessionDbOutcome::TripwireOwed);
        assert!(!named.exists());

        let applied = session_db_run(state, MigrateMode::Apply, None, &named);
        assert!(!applied.has_errors(), "{:?}", applied.errors);
        assert!(is_session_store_tripwire(&named));
        assert!(matches!(
            session_db_outcome(&applied),
            SessionDbOutcome::Moved { .. }
        ));
        assert_eq!(session_marker(&canonical), "override");

        let other = state.join("other-sessions.db");
        let missing = session_db_run(state, MigrateMode::DryRun, None, &other);
        assert!(matches!(
            session_db_outcome(&missing),
            SessionDbOutcome::Refused {
                refusal: SessionDbRefusal::Missing { .. }
            }
        ));
        assert!(!other.exists());
    }

    /// A named file byte-identical to `sessions.sqlite3` dedups under plain
    /// `--apply`: the canonical copy stays, the named copy is archived, and
    /// the tripwire takes its path.
    #[test]
    fn a_byte_identical_session_db_dedups_and_leaves_the_tripwire() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = temp.path();
        let canonical = state.join("sessions.sqlite3");
        create_session_db(&canonical, "same");
        let named = state.join("my-sessions.db");
        fs::copy(&canonical, &named).unwrap();

        let report = session_db_run(state, MigrateMode::Apply, None, &named);
        assert!(!report.has_errors(), "{:?}", report.errors);
        let twin = report
            .twins
            .iter()
            .find(|twin| twin.slot == "sessions")
            .expect("sessions twin");
        let TwinResolution::Deduped { kept, archived } = &twin.resolution else {
            panic!("{:?}", twin.resolution);
        };
        assert_eq!(kept, &canonical);
        assert!(
            archived[0]
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(
                    |name| name.starts_with("my-sessions.db.pre-") && name.ends_with(".twin-dedup")
                ),
            "{archived:?}"
        );
        assert!(is_session_store_tripwire(&named));
        assert_eq!(session_marker(&canonical), "same");
    }

    /// A legacy spelling named with `--session-db` takes case 2 plus the
    /// tripwire. The stamp at the legacy name is a tripwire, not a copy: a
    /// later run, with or without `--session-db`, sees no twin and leaves it
    /// in place.
    #[test]
    fn a_legacy_spelling_session_db_takes_case_two_and_the_tripwire() {
        let temp = tempfile::tempdir().expect("tempdir");
        let state = temp.path();
        let legacy = state.join("sessions.db");
        create_session_db(&legacy, "legacy");

        let report = session_db_run(state, MigrateMode::Apply, None, &legacy);
        assert!(!report.has_errors(), "{:?}", report.errors);
        assert_eq!(
            session_db_outcome(&report),
            &SessionDbOutcome::Moved {
                tripwire: legacy.clone()
            }
        );
        assert_eq!(session_marker(&state.join("sessions.sqlite3")), "legacy");
        assert!(is_session_store_tripwire(&legacy));

        let plain = migrate_state_dir(state, MigrateMode::Apply, None);
        assert!(!plain.has_errors(), "{:?}", plain.errors);
        assert!(
            plain.twins.iter().all(|twin| twin.slot != "sessions"),
            "{:?}",
            plain.twins
        );
        assert!(is_session_store_tripwire(&legacy), "the tripwire stays");
        let diagnosis = storage_doctor::diagnose_state_dir_blocking(
            &DiagnoseScope::new(vec![state.to_path_buf()]),
            None,
        );
        assert!(
            diagnosis
                .findings
                .iter()
                .all(|finding| finding.code != storage_doctor::FINDING_FILE_NAME_TWINS),
            "the doctor sees no twin beside the tripwire: {:?}",
            diagnosis.findings
        );
        let rerun = session_db_run(state, MigrateMode::Apply, None, &legacy);
        assert_eq!(session_db_outcome(&rerun), &SessionDbOutcome::AlreadyMoved);
    }
}
